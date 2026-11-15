//! Cloud-sync jobs (linkSlot CLI) run out of process and are polled from the
//! frame loop, following the `LinkStarter` pattern in `link_start.rs`: nothing
//! here may block the render thread.
//!
//! Project book v0.6: the handheld only runs `linkSlot sync`.  A conflict
//! answer is retried by the UI with `sync --resolve upload|download`; the CLI
//! then backs up whatever it overwrites.  The CLI is invoked through
//! `/bin/sh <root>/System/linkSlot/shell/linkslot.sh` (FAT32 cards cannot hold an executable
//! bit); every argument is passed as its own argv entry, never spliced into a
//! shell string, so Chinese names and spaces are safe.
//!
//! Progress arrives as `LINKSLOT_STAGE=` and `LINKSLOT_DIRECTION=` lines on
//! stdout; the final `LINKSLOT_*` block gives the aggregate result, direction,
//! version, error category, conflict copy path and the optional cloud/local
//! version lines used by the conflict choice UI.
//!
//! v0.0.10 syncs two units (state = 即时存档, sav = 电池存档) and the CLI adds
//! one `LINKSLOT_STATE_*` / `LINKSLOT_SAV_*` group per unit plus repeated
//! `LINKSLOT_CONFLICT_UNIT=` lines.  Both shapes are accepted: old output
//! without per-unit keys still parses as a single-unit outcome, so the Web page
//! and older scripts keep working.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use slot_mods::ActionUi;

/// How long the app lets one sync run before it cancels it (project book §6).
pub const SYNC_TIMEOUT_MS: u64 = 120_000;

/// How long the app shows the result banner before returning to Idle.
pub const SYNC_RESULT_MS: u64 = 1_500;

/// How long the confirm key must be held to start a sync (project book §6.1).
pub const SYNC_HOLD_MS: u64 = 1_618;

/// How many stderr lines are kept for the banner/log when stdout gave no detail.
const STDERR_TAIL: usize = 16;

/// Which side wins when the user resolves a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolve {
    Upload,
    Download,
}

impl Resolve {
    pub const ALL: [Resolve; 2] = [Resolve::Upload, Resolve::Download];

    /// The `--resolve` value the CLI expects.
    pub fn verb(self) -> &'static str {
        match self {
            Resolve::Upload => "upload",
            Resolve::Download => "download",
        }
    }

    /// The direction the transfer takes.
    pub fn direction(self) -> Direction {
        match self {
            Resolve::Upload => Direction::Upload,
            Resolve::Download => Direction::Download,
        }
    }

    pub fn parse(text: &str) -> Option<Resolve> {
        match text.trim() {
            "upload" => Some(Resolve::Upload),
            "download" => Some(Resolve::Download),
            _ => None,
        }
    }
}

/// Per-unit directions chosen with `--resolve`.
///
/// A bare `--resolve upload|download` fills both fields (see
/// [`ResolveSpec::all`]); `--resolve state=upload,sav=download` fills one or
/// both. A unit left as `None` keeps its conflict instead of guessing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolveSpec {
    pub state: Option<Direction>,
    pub sav: Option<Direction>,
}

impl ResolveSpec {
    /// Bare `--resolve upload|download`: every conflicting unit gets it.
    pub fn all(direction: Direction) -> ResolveSpec {
        ResolveSpec {
            state: Some(direction),
            sav: Some(direction),
        }
    }

    pub fn get(&self, kind: UnitKind) -> Option<Direction> {
        match kind {
            UnitKind::State => self.state,
            UnitKind::Sav => self.sav,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.sav.is_none()
    }

    /// The `--resolve` value; a bare direction when both units agree.
    pub fn argv_value(self) -> Option<String> {
        match (self.state, self.sav) {
            (None, None) => None,
            (Some(state), Some(sav)) if state == sav => Some(state.verb().to_string()),
            (Some(state), Some(sav)) => {
                Some(format!("state={},sav={}", state.verb(), sav.verb()))
            }
            (Some(state), None) => Some(format!("state={}", state.verb())),
            (None, Some(sav)) => Some(format!("sav={}", sav.verb())),
        }
    }
}

/// Cloud versions the user's decision was based on (`--expect state=12,sav=7`).
///
/// Checked by the CLI before it transfers anything; a mismatch aborts the call
/// with `changed-since` so the UI can ask again with fresh information.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExpectedVersions {
    pub state: Option<u32>,
    pub sav: Option<u32>,
}

impl ExpectedVersions {
    pub fn get(&self, kind: UnitKind) -> Option<u32> {
        match kind {
            UnitKind::State => self.state,
            UnitKind::Sav => self.sav,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.sav.is_none()
    }

    /// The `--expect` value in a stable order (`state` before `sav`).
    pub fn argv_value(self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(version) = self.state {
            parts.push(format!("state={version}"));
        }
        if let Some(version) = self.sav {
            parts.push(format!("sav={version}"));
        }
        (!parts.is_empty()).then(|| parts.join(","))
    }
}

/// Which way the bytes moved, as reported by `LINKSLOT_DIRECTION=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

impl Direction {
    pub fn parse(text: &str) -> Option<Direction> {
        match text.trim() {
            "upload" => Some(Direction::Upload),
            "download" => Some(Direction::Download),
            _ => None,
        }
    }

    /// The `--resolve` value.
    pub fn verb(self) -> &'static str {
        match self {
            Direction::Upload => "upload",
            Direction::Download => "download",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Direction::Upload => "上传",
            Direction::Download => "下载",
        }
    }
}

/// Which sync unit a per-unit result belongs to (`state` = 即时存档,
/// `sav` = 电池存档).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnitKind {
    State,
    Sav,
}

impl UnitKind {
    pub const ALL: [UnitKind; 2] = [UnitKind::State, UnitKind::Sav];

    /// The `LINKSLOT_STATE_*` / `--only` name.
    pub fn as_str(self) -> &'static str {
        match self {
            UnitKind::State => "state",
            UnitKind::Sav => "sav",
        }
    }

    /// Full Chinese label used in detail lines.
    pub fn label(self) -> &'static str {
        match self {
            UnitKind::State => "即时存档",
            UnitKind::Sav => "电池存档",
        }
    }

    /// Short label used in result banners (`已同步：即时 v2 / 电池 v3`).
    pub fn short(self) -> &'static str {
        match self {
            UnitKind::State => "即时",
            UnitKind::Sav => "电池",
        }
    }

    pub fn parse(text: &str) -> Option<UnitKind> {
        match text.trim() {
            "state" => Some(UnitKind::State),
            "sav" => Some(UnitKind::Sav),
            _ => None,
        }
    }
}

/// One side of a conflict, as the CLI knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: Option<u32>,
    pub device: Option<String>,
    pub updated: Option<String>,
    /// File count from `LINKSLOT_{STATE,SAV}_{CLOUD,LOCAL}_FILES`.
    pub files: Option<u32>,
}

impl VersionInfo {
    pub fn is_empty(&self) -> bool {
        self.version.is_none()
            && self.device.is_none()
            && self.updated.is_none()
            && self.files.is_none()
    }
}

/// Cloud/local version information shown by the conflict choice UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictInfo {
    pub cloud: VersionInfo,
    pub local: VersionInfo,
}

/// One unit's slice of a v0.0.10 sync result (`LINKSLOT_STATE_*` /
/// `LINKSLOT_SAV_*`).  Older CLI output has no per-unit block, so the fields of
/// [`SyncOutcome`] stay the single-unit compatibility path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitOutcome {
    pub kind: UnitKind,
    pub result: Option<SyncResult>,
    pub direction: Option<Direction>,
    pub version: Option<u32>,
    pub error: Option<String>,
    pub info: Option<ConflictInfo>,
}

impl UnitOutcome {
    /// A short Chinese line for this unit's conflict reason.
    pub fn conflict_line(&self) -> &'static str {
        conflict_text(self.error.as_deref())
    }

    /// Two lines of version/device/time for this unit's conflict page.
    pub fn conflict_lines(&self) -> Option<(String, String)> {
        let info = self.info.as_ref()?;
        Some((
            version_line("云端", &info.cloud),
            version_line("本地", &info.local),
        ))
    }
}

/// New cloud information reported when a `--expect` check failed
/// (`LINKSLOT_ERROR=changed-since`).  The conflict pages reset and re-read the
/// unit after showing this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangedSince {
    /// Which unit changed (`None` when the CLI detail did not name one).
    pub unit: Option<UnitKind>,
    /// The cloud version the user's decision was based on.
    pub expected: Option<u32>,
    /// The cloud version found now; `None` when the cloud unit disappeared.
    pub actual: Option<u32>,
}

/// What a shelf long-press launches: cloud sync or manual mods.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncRequestMode {
    #[default]
    Cloud,
    /// `linkSlot mods run`: enabled `manual` actions, e.g. GBAStation sync.
    Mods,
    /// `linkSlot mods boot --id --action`: one scheduled after_boot action.
    Boot,
}

/// Everything the CLI needs to find one sync unit.  `root` is the card root
/// (`/mnt/sdcard` on the device); the rest map to `--platform/--emulator/--game`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRequest {
    pub platform: String,
    pub emulator: String,
    pub stem: String,
    pub root: PathBuf,
    /// Cloud sync or manual mods (`mods run`).
    pub mode: SyncRequestMode,
    /// `--only state|sav`: process just this unit (v0.0.10 conflict pages).
    pub only: Option<UnitKind>,
    /// `--resolve`: per-unit directions when the user resolved a conflict.
    pub resolve: ResolveSpec,
    /// `--expect`: cloud versions the decision was based on.
    pub expect: ExpectedVersions,
    /// `--choice key=option`: answers already made for a mod popup.
    pub choices: Vec<(String, String)>,
    /// Exact target for `mods boot`, or the one action a `mods run` request
    /// picked from the long-press Y chooser; `None` for plain cloud requests.
    pub boot_mod: Option<String>,
    pub boot_action: Option<String>,
    /// Optional declarative animation from the manual action's `action.json`.
    pub ui: Option<ActionUi>,
}

impl SyncRequest {
    pub fn new(
        platform: impl Into<String>,
        emulator: impl Into<String>,
        stem: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Self {
        SyncRequest {
            platform: platform.into(),
            emulator: emulator.into(),
            stem: stem.into(),
            root: root.into(),
            mode: SyncRequestMode::Cloud,
            only: None,
            resolve: ResolveSpec::default(),
            expect: ExpectedVersions::default(),
            choices: Vec::new(),
            boot_mod: None,
            boot_action: None,
            ui: None,
        }
    }

    /// Attach the manual action's optional animation declaration.
    pub fn with_ui(mut self, ui: Option<ActionUi>) -> Self {
        self.ui = ui;
        self
    }

    /// A manual mods request (`mods run`) for the same cart.
    pub fn manual(
        platform: impl Into<String>,
        emulator: impl Into<String>,
        stem: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Self {
        SyncRequest {
            mode: SyncRequestMode::Mods,
            ..SyncRequest::new(platform, emulator, stem, root)
        }
    }

    /// One chosen manual action: `mods run --id <mod> --action <action>`.
    pub fn manual_for(
        platform: impl Into<String>,
        emulator: impl Into<String>,
        stem: impl Into<String>,
        root: impl Into<PathBuf>,
        mod_id: impl Into<String>,
        action_id: impl Into<String>,
    ) -> Self {
        SyncRequest {
            mode: SyncRequestMode::Mods,
            boot_mod: Some(mod_id.into()),
            boot_action: Some(action_id.into()),
            ..SyncRequest::new(platform, emulator, stem, root)
        }
    }

    /// A scheduled after-boot action, independent of any cartridge.
    pub fn boot(root: impl Into<PathBuf>, mod_id: impl Into<String>, action_id: impl Into<String>) -> Self {
        SyncRequest {
            platform: String::new(),
            emulator: String::new(),
            stem: String::new(),
            root: root.into(),
            mode: SyncRequestMode::Boot,
            only: None,
            resolve: ResolveSpec::default(),
            expect: ExpectedVersions::default(),
            choices: Vec::new(),
            boot_mod: Some(mod_id.into()),
            boot_action: Some(action_id.into()),
            ui: None,
        }
    }

    /// `--only`: transfer just this unit.
    pub fn with_only(mut self, kind: UnitKind) -> Self {
        self.only = Some(kind);
        self
    }

    /// `--resolve`: per-unit conflict directions.
    pub fn with_resolve(mut self, resolve: ResolveSpec) -> Self {
        self.resolve = resolve;
        self
    }

    /// `--expect`: cloud versions the decision was based on.
    pub fn with_expect(mut self, expect: ExpectedVersions) -> Self {
        self.expect = expect;
        self
    }

    /// `--choice`: answer one mod popup and keep earlier answers.
    pub fn with_choice(mut self, key: impl Into<String>, option: impl Into<String>) -> Self {
        let key = key.into();
        self.choices.retain(|(existing, _)| existing != &key);
        self.choices.push((key, option.into()));
        self
    }

    /// `--expect <kind>=<version>`, keeping any other expected unit.
    pub fn expect_unit(mut self, kind: UnitKind, version: u32) -> Self {
        match kind {
            UnitKind::State => self.expect.state = Some(version),
            UnitKind::Sav => self.expect.sav = Some(version),
        }
        self
    }

    /// Bare `--resolve upload|download` for every conflicting unit.
    pub fn resolved(mut self, resolve: Resolve) -> Self {
        self.resolve = ResolveSpec::all(resolve.direction());
        self
    }

    /// The arguments after the script path, as given to the CLI.  Kept as a
    /// list so tests can check the Chinese/space-safe argv without a process.
    pub fn argv(&self) -> Vec<String> {
        if self.mode == SyncRequestMode::Boot {
            let mut argv = vec!["mods".to_string(), "boot".to_string()];
            if let Some(mod_id) = &self.boot_mod {
                argv.push("--id".to_string());
                argv.push(mod_id.clone());
            }
            if let Some(action_id) = &self.boot_action {
                argv.push("--action".to_string());
                argv.push(action_id.clone());
            }
            argv.push("--root".to_string());
            argv.push(self.root.to_string_lossy().into_owned());
            argv.push("--verbose".to_string());
            return argv;
        }
        let mut argv = match self.mode {
            SyncRequestMode::Cloud => vec!["sync".to_string()],
            SyncRequestMode::Mods => vec!["mods".to_string(), "run".to_string()],
            SyncRequestMode::Boot => unreachable!("handled above"),
        };
        argv.extend([
            "--platform".to_string(),
            self.platform.clone(),
            "--emulator".to_string(),
            self.emulator.clone(),
            "--game".to_string(),
            self.stem.clone(),
            "--root".to_string(),
            self.root.to_string_lossy().into_owned(),
        ]);
        if self.mode == SyncRequestMode::Mods {
            if let Some(mod_id) = &self.boot_mod {
                argv.push("--id".to_string());
                argv.push(mod_id.clone());
            }
            if let Some(action_id) = &self.boot_action {
                argv.push("--action".to_string());
                argv.push(action_id.clone());
            }
            for (key, option) in &self.choices {
                argv.push("--choice".to_string());
                argv.push(format!("{key}={option}"));
            }
            return argv;
        }
        if let Some(kind) = self.only {
            argv.push("--only".to_string());
            argv.push(kind.as_str().to_string());
        }
        if let Some(value) = self.resolve.argv_value() {
            argv.push("--resolve".to_string());
            argv.push(value);
        }
        if let Some(value) = self.expect.argv_value() {
            argv.push("--expect".to_string());
            argv.push(value);
        }
        argv
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncStage {
    Net,
    Connect,
    Upload,
    Download,
    Verify,
    Commit,
    /// A manual `mods run` job (shelf long-press Y).
    Mods,
}

impl SyncStage {
    pub const ALL: [SyncStage; 7] = [
        SyncStage::Net,
        SyncStage::Connect,
        SyncStage::Upload,
        SyncStage::Download,
        SyncStage::Verify,
        SyncStage::Commit,
        SyncStage::Mods,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The line the transfer animation shows for this stage.
    pub fn line(self) -> &'static str {
        match self {
            SyncStage::Net => "正在连接网络",
            SyncStage::Connect => "正在连接云端",
            SyncStage::Upload => "正在上传存档",
            SyncStage::Download => "正在下载存档",
            SyncStage::Verify => "正在校验存档",
            SyncStage::Commit => "正在提交云端记录",
            SyncStage::Mods => "正在同步",
        }
    }

    pub fn parse(text: &str) -> Option<SyncStage> {
        match text.trim() {
            "net" | "network" => Some(SyncStage::Net),
            "connect" | "connecting" | "auth" => Some(SyncStage::Connect),
            "upload" | "put" => Some(SyncStage::Upload),
            "download" | "get" => Some(SyncStage::Download),
            "verify" | "check" => Some(SyncStage::Verify),
            "commit" | "meta" => Some(SyncStage::Commit),
            "mods" | "manual" => Some(SyncStage::Mods),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncResult {
    Ok,
    Noop,
    /// One unit transferred and another failed (`LINKSLOT_RESULT=partial`).
    Partial,
    Conflict,
    /// A manual mod needs a device-side choice (`LINKSLOT_RESULT=choice`).
    Choice,
    Error,
}

impl SyncResult {
    pub fn parse(text: &str) -> Option<SyncResult> {
        match text.trim() {
            "ok" => Some(SyncResult::Ok),
            "noop" => Some(SyncResult::Noop),
            "partial" => Some(SyncResult::Partial),
            "conflict" => Some(SyncResult::Conflict),
            "choice" => Some(SyncResult::Choice),
            "error" => Some(SyncResult::Error),
            _ => None,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            SyncResult::Ok => "成功",
            SyncResult::Noop => "已是最新",
            SyncResult::Partial => "部分完成",
            SyncResult::Conflict => "冲突",
            SyncResult::Choice => "需要选择",
            SyncResult::Error => "失败",
        }
    }
}

/// The final report of one sync, as the shelf overlay needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncOutcome {
    pub result: SyncResult,
    /// Legacy single-unit direction (`LINKSLOT_DIRECTION`).
    pub direction: Option<Direction>,
    /// Legacy single-unit version (`LINKSLOT_VERSION`).
    pub version: Option<u32>,
    pub error: Option<String>,
    pub detail: Option<String>,
    pub conflict: Option<String>,
    /// Legacy single-unit conflict versions (`LINKSLOT_CLOUD_*`/`LOCAL_*`).
    pub info: Option<ConflictInfo>,
    /// Per-unit result when the CLI printed `LINKSLOT_STATE_*` (v0.0.10+).
    pub state: Option<UnitOutcome>,
    /// Per-unit result when the CLI printed `LINKSLOT_SAV_*` (v0.0.10+).
    pub sav: Option<UnitOutcome>,
    /// Conflicting units in CLI order, de-duplicated.
    pub conflict_units: Vec<UnitKind>,
    /// Set when `LINKSLOT_ERROR=changed-since`.
    pub changed: Option<ChangedSince>,
    /// Set when `LINKSLOT_RESULT=choice`.
    pub choice: Option<slot_mods::ChoiceRequest>,
}

impl SyncOutcome {
    pub fn error(detail: impl Into<String>) -> Self {
        SyncOutcome {
            result: SyncResult::Error,
            direction: None,
            version: None,
            error: None,
            detail: Some(detail.into()),
            conflict: None,
            info: None,
            state: None,
            sav: None,
            conflict_units: Vec::new(),
            changed: None,
            choice: None,
        }
    }

    /// The per-unit result for `kind`, when the CLI reported it.
    pub fn unit(&self, kind: UnitKind) -> Option<&UnitOutcome> {
        match kind {
            UnitKind::State => self.state.as_ref(),
            UnitKind::Sav => self.sav.as_ref(),
        }
    }

    pub fn unit_result(&self, kind: UnitKind) -> Option<SyncResult> {
        self.unit(kind).and_then(|unit| unit.result)
    }

    /// True when at least one `LINKSLOT_STATE_*`/`SAV_*` block was parsed.
    pub fn has_unit_results(&self) -> bool {
        self.state.is_some() || self.sav.is_some()
    }

    /// The units the CLI reported, in `state` → `sav` order.
    pub fn units(&self) -> Vec<UnitKind> {
        let mut units = Vec::with_capacity(2);
        if self.state.is_some() {
            units.push(UnitKind::State);
        }
        if self.sav.is_some() {
            units.push(UnitKind::Sav);
        }
        units
    }

    /// True when `kind` is in [`SyncOutcome::conflict_units`].
    pub fn is_conflicting(&self, kind: UnitKind) -> bool {
        self.conflict_units.contains(&kind)
    }

    /// A short Chinese line naming the error category from `LINKSLOT_ERROR`.
    pub fn error_line(&self) -> Option<&'static str> {
        match self.result {
            SyncResult::Conflict => Some(self.conflict_line()),
            SyncResult::Choice => None,
            SyncResult::Partial | SyncResult::Error => Some(match self.error.as_deref() {
                Some("net") => "网络未连接",
                Some("auth") => "云端认证失败",
                Some("cert") => "云端证书校验失败",
                Some("clock") => "设备时钟不正确",
                Some("server") => "云端服务器错误",
                Some("config") => "同步配置错误",
                Some("fs") => "存储卡读写失败",
                Some("verify") => "存档校验失败",
                Some("timeout") => "同步超时",
                Some("busy") => "另一个同步正在进行",
                Some("changed-since") => "云端已被其它设备更新",
                _ => "同步失败",
            }),
            _ => None,
        }
    }

    /// A line for a conflict (project book §5.1).
    pub fn conflict_line(&self) -> &'static str {
        conflict_text(self.error.as_deref())
    }

    /// The short banner text for the shelf overlay.
    pub fn banner_text(&self) -> String {
        match self.result {
            SyncResult::Ok => match (self.direction, self.version) {
                (Some(Direction::Download), Some(v)) => format!("已拉取版本 {v}"),
                (Some(Direction::Upload), Some(v)) => format!("已推送版本 {v}"),
                (Some(Direction::Download), None) => "已拉取".to_string(),
                (Some(Direction::Upload), None) => "已推送".to_string(),
                _ => self
                    .detail
                    .clone()
                    .unwrap_or_else(|| "同步完成".to_string()),
            },
            SyncResult::Noop => self
                .detail
                .clone()
                .unwrap_or_else(|| "已是最新".to_string()),
            SyncResult::Partial => self
                .detail
                .clone()
                .or_else(|| self.error_line().map(str::to_string))
                .unwrap_or_else(|| "部分同步完成".to_string()),
            SyncResult::Conflict => self.conflict_line().to_string(),
            SyncResult::Choice => self
                .detail
                .clone()
                .unwrap_or_else(|| "需要选择".to_string()),
            SyncResult::Error => self
                .error_line()
                .map(str::to_string)
                .or_else(|| self.detail.clone())
                .unwrap_or_else(|| "同步失败".to_string()),
        }
    }

    /// Two lines of version/device/time for the legacy single-unit conflict UI.
    pub fn conflict_lines(&self) -> Option<(String, String)> {
        let info = self.info.as_ref()?;
        Some((
            version_line("云端", &info.cloud),
            version_line("本地", &info.local),
        ))
    }
}

/// Wording shared by the aggregate and per-unit conflict lines.
fn conflict_text(error: Option<&str>) -> &'static str {
    match error {
        Some("no-cache") => "云端已有存档但本地无同步记录",
        Some("forked") => "云端与本地已分叉",
        Some("remote-rollback") => "云端版本异常回退",
        Some("remote-newer") => "云端有更新",
        Some("changed-since") => "云端已被其它设备更新，请基于最新版本重新选择方向",
        _ => "云端与本地不一致",
    }
}

fn version_line(side: &str, info: &VersionInfo) -> String {
    let mut parts = Vec::new();
    if let Some(version) = info.version {
        parts.push(format!("v{version}"));
    }
    if let Some(device) = &info.device {
        if !device.is_empty() {
            parts.push(device.clone());
        }
    }
    if let Some(updated) = &info.updated {
        if !updated.is_empty() {
            parts.push(updated.clone());
        }
    }
    if parts.is_empty() {
        format!("{side} 未知")
    } else {
        format!("{side} {}", parts.join(" · "))
    }
}

pub enum SyncUpdate {
    Stage(SyncStage),
    Direction(Direction),
    /// 0.0–1.0 reported by a manual mod (`mods.progress`).
    Progress(f32),
    /// Optional label next to the mod progress bar.
    ProgressLabel(String),
    Finished(SyncOutcome),
}

pub trait SyncRun {
    /// Non-blocking: `None` means the job is still running.
    fn poll(&mut self) -> Option<SyncUpdate>;
    fn cancel(&mut self);
    fn exited(&self) -> bool;
}

pub trait SyncLauncher {
    fn launch(&mut self, request: SyncRequest) -> Box<dyn SyncRun>;
}

/// The production launcher: `/bin/sh <root>/System/linkSlot/shell/linkslot.sh ...` with piped
/// stdout/stderr, read by a worker thread.
#[derive(Default)]
pub struct SystemLauncher;

impl SystemLauncher {
    pub fn new() -> Self {
        SystemLauncher
    }
}

impl SyncLauncher for SystemLauncher {
    fn launch(&mut self, request: SyncRequest) -> Box<dyn SyncRun> {
        Box::new(SystemRun::start(request))
    }
}

/// A background `linkslot.sh net up` used to warm the Wi-Fi while the picker
/// is open. The real sync CLI is only started after this exits, so the two
/// scripts cannot fight over wpa_supplicant.
pub struct WifiWarmup {
    rx: Receiver<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl WifiWarmup {
    pub fn start(root: &Path) -> Option<Self> {
        let script = root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh");
        if !script.is_file() {
            return None;
        }
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        let worker = std::thread::Builder::new()
            .name("slot-wifi-warmup".to_string())
            .spawn(move || {
                let _ = Command::new("/bin/sh")
                    .arg(&script)
                    .args(["net", "up", "--timeout", "30"])
                    .env("SLOT_ROOT", &root)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = tx.send(());
            })
            .ok()?;
        Some(WifiWarmup {
            rx,
            worker: Some(worker),
        })
    }

    pub fn finished(&mut self) -> bool {
        match self.rx.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                true
            }
            Err(TryRecvError::Empty) => false,
        }
    }
}

/// The command that runs one sync.  Public so tests can inspect its argv
/// without spawning anything.
pub fn system_command(request: &SyncRequest) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(request.root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh"));
    for arg in request.argv() {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

pub struct SystemRun {
    child: Arc<Mutex<Option<Child>>>,
    pid: Option<u32>,
    rx: Receiver<SyncUpdate>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    cancelled: Arc<AtomicBool>,
    exited: bool,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SystemRun {
    pub fn start(request: SyncRequest) -> SystemRun {
        let mut command = system_command(&request);
        let (tx, rx) = channel::<SyncUpdate>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
        let child = Arc::new(Mutex::new(None));
        let mut child_process = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let detail = format!("无法启动同步程序：{e}");
                let _ = tx.send(SyncUpdate::Finished(SyncOutcome::error(detail)));
                return SystemRun {
                    child,
                    pid: None,
                    rx,
                    stderr,
                    cancelled,
                    exited: false,
                    worker: None,
                };
            }
        };
        let pid = child_process.id();
        let stdout = child_process.stdout.take();
        let child_stderr = child_process.stderr.take();
        *child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child_process);

        let stderr_slot = stderr.clone();
        let stderr_worker = child_stderr.and_then(|pipe| {
            std::thread::Builder::new()
                .name("slot-sync-stderr".into())
                .spawn(move || {
                    let mut tail = VecDeque::with_capacity(STDERR_TAIL);
                    let mut reader = BufReader::new(pipe);
                    loop {
                        let mut line = String::new();
                        match reader.read_line(&mut line) {
                            Ok(0) => break,
                            Ok(_) => {
                                let line = line.trim_end().to_string();
                                if line.is_empty() {
                                    continue;
                                }
                                if tail.len() == STDERR_TAIL {
                                    tail.pop_front();
                                }
                                tail.push_back(line);
                            }
                            Err(_) => break,
                        }
                    }
                    *stderr_slot.lock().unwrap_or_else(|e| e.into_inner()) = tail;
                })
                .ok()
        });

        let child_slot = child.clone();
        let stderr_slot = stderr.clone();
        let worker = std::thread::Builder::new()
            .name("slot-sync".into())
            .spawn(move || {
                let mut block = Block::default();
                if let Some(stdout) = stdout {
                    read_stdout(stdout, &tx, &mut block);
                }
                let status = match child_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    Some(mut child) => child.wait().ok(),
                    None => None,
                };
                if let Some(handle) = stderr_worker {
                    let _ = handle.join();
                }
                let stderr_tail: Vec<String> = stderr_slot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .cloned()
                    .collect();
                let exit = status.and_then(|s| s.code());
                let outcome = block.finish(exit, &stderr_tail);
                let _ = tx.send(SyncUpdate::Finished(outcome));
            })
            .ok();

        SystemRun {
            child,
            pid: Some(pid),
            rx,
            stderr,
            cancelled,
            exited: false,
            worker,
        }
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn terminate(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(pid) = self.pid {
            terminate_pid(pid, &self.child);
        }
    }
}

impl SyncRun for SystemRun {
    fn poll(&mut self) -> Option<SyncUpdate> {
        if self.exited {
            return None;
        }
        match self.rx.try_recv() {
            Ok(update) => {
                if matches!(update, SyncUpdate::Finished(_)) {
                    self.exited = true;
                }
                Some(update)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.exited = true;
                None
            }
        }
    }

    fn cancel(&mut self) {
        self.terminate();
    }

    fn exited(&self) -> bool {
        self.exited
    }
}

impl Drop for SystemRun {
    fn drop(&mut self) {
        if !self.exited {
            self.terminate();
            // A cancelled CLI exits on SIGTERM; when it does not, the worker
            // thread still reaps it, so never block the UI on it.
            self.worker.take();
        } else if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_stdout(mut pipe: impl Read, tx: &std::sync::mpsc::Sender<SyncUpdate>, block: &mut Block) {
    let mut reader = BufReader::new(&mut pipe);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let line = line.trim_end();
                if let Some(stage) = parse_stage_line(line) {
                    let _ = tx.send(SyncUpdate::Stage(stage));
                } else if let Some(direction) = parse_direction_line(line) {
                    // The final block repeats this key; keep it in the outcome
                    // as well as telling the transfer animation.
                    block.absorb(line);
                    let _ = tx.send(SyncUpdate::Direction(direction));
                } else if let Some(value) = parse_progress_line(line) {
                    let _ = tx.send(SyncUpdate::Progress(value));
                } else if let Some(label) = parse_progress_label_line(line) {
                    let _ = tx.send(SyncUpdate::ProgressLabel(label));
                } else {
                    block.absorb(line);
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(unix)]
fn terminate_pid(pid: u32, _child: &Arc<Mutex<Option<Child>>>) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_pid(_pid: u32, child: &Arc<Mutex<Option<Child>>>) {
    if let Some(child) = child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = child.kill();
    }
}

/// `LINKSLOT_STAGE=upload` → `Some(SyncStage::Upload)`.
pub fn parse_stage_line(line: &str) -> Option<SyncStage> {
    line.strip_prefix("LINKSLOT_STAGE=")
        .and_then(SyncStage::parse)
}

/// `LINKSLOT_PROGRESS=42` → `Some(0.42)`; clamped because a mod is untrusted.
pub fn parse_progress_line(line: &str) -> Option<f32> {
    let value = line.strip_prefix("LINKSLOT_PROGRESS=")?.trim().parse::<f32>().ok()?;
    value.is_finite().then(|| (value / 100.0).clamp(0.0, 1.0))
}

/// `LINKSLOT_PROGRESS_LABEL=正在生成` → the display label.
pub fn parse_progress_label_line(line: &str) -> Option<String> {
    let label = line.strip_prefix("LINKSLOT_PROGRESS_LABEL=")?.trim();
    if label.is_empty() {
        return None;
    }
    let label: String = label
        .chars()
        .filter(|ch| !ch.is_control())
        .take(40)
        .collect();
    (!label.is_empty()).then_some(label)
}

/// `LINKSLOT_DIRECTION=download` → `Some(Direction::Download)`.
pub fn parse_direction_line(line: &str) -> Option<Direction> {
    line.strip_prefix("LINKSLOT_DIRECTION=")
        .and_then(Direction::parse)
}

/// Read the new cloud versions out of a `changed-since` detail line.
///
/// The CLI detail is either `<label>云端版本已变化（期望 v12，实际 v13）` or
/// `<label>云端还没有存档（期望 v12）`, followed by the generic hint.
pub(crate) fn parse_changed_since(detail: &str) -> ChangedSince {
    let unit = if detail.contains("电池存档") {
        Some(UnitKind::Sav)
    } else if detail.contains("即时存档") {
        Some(UnitKind::State)
    } else {
        None
    };
    ChangedSince {
        unit,
        expected: version_after(detail, "期望 v"),
        actual: version_after(detail, "实际 v"),
    }
}

fn version_after(text: &str, marker: &str) -> Option<u32> {
    let rest = text.split_once(marker)?.1;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Aggregates one `LINKSLOT_*` block while the CLI is finishing.
#[derive(Default)]
struct Block {
    result: Option<SyncResult>,
    direction: Option<Direction>,
    version: Option<u32>,
    error: Option<String>,
    detail: Option<String>,
    conflict: Option<String>,
    cloud: VersionInfo,
    local: VersionInfo,
    state: UnitBlock,
    sav: UnitBlock,
    conflict_units: Vec<UnitKind>,
    choice: Option<slot_mods::ChoiceRequest>,
}

/// One `LINKSLOT_STATE_*` / `LINKSLOT_SAV_*` group.
#[derive(Default)]
struct UnitBlock {
    result: Option<SyncResult>,
    direction: Option<Direction>,
    version: Option<u32>,
    error: Option<String>,
    cloud: VersionInfo,
    local: VersionInfo,
}

impl UnitBlock {
    fn absorb(&mut self, key: &str, value: &str) {
        match key {
            "RESULT" => self.result = SyncResult::parse(value),
            "DIRECTION" => self.direction = Direction::parse(value),
            "VERSION" => self.version = value.parse().ok(),
            "ERROR" => self.error = non_empty(value),
            "CLOUD_VERSION" => self.cloud.version = value.parse().ok(),
            "CLOUD_DEVICE" => self.cloud.device = non_empty(value),
            "CLOUD_UPDATED" => self.cloud.updated = non_empty(value),
            "CLOUD_FILES" => self.cloud.files = value.parse().ok(),
            "LOCAL_VERSION" => self.local.version = value.parse().ok(),
            "LOCAL_DEVICE" => self.local.device = non_empty(value),
            "LOCAL_UPDATED" => self.local.updated = non_empty(value),
            "LOCAL_FILES" => self.local.files = value.parse().ok(),
            _ => {}
        }
    }

    fn is_empty(&self) -> bool {
        self.result.is_none()
            && self.direction.is_none()
            && self.version.is_none()
            && self.error.is_none()
            && self.cloud.is_empty()
            && self.local.is_empty()
    }

    fn finish(self, kind: UnitKind) -> UnitOutcome {
        let info = if self.cloud.is_empty() && self.local.is_empty() {
            None
        } else {
            Some(ConflictInfo {
                cloud: self.cloud,
                local: self.local,
            })
        };
        UnitOutcome {
            kind,
            result: self.result,
            direction: self.direction,
            version: self.version,
            error: self.error,
            info,
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

impl Block {
    fn absorb(&mut self, line: &str) {
        let Some(rest) = line.strip_prefix("LINKSLOT_") else {
            return;
        };
        let Some((key, value)) = rest.split_once('=') else {
            return;
        };
        let value = value.trim();
        if let Some(unit_key) = key.strip_prefix("STATE_") {
            self.state.absorb(unit_key, value);
            return;
        }
        if let Some(unit_key) = key.strip_prefix("SAV_") {
            self.sav.absorb(unit_key, value);
            return;
        }
        match key {
            "ACTION" => {} // only `sync` reaches the Slot UI
            "RESULT" => self.result = SyncResult::parse(value),
            "DIRECTION" => self.direction = Direction::parse(value),
            "VERSION" => self.version = value.parse().ok(),
            "ERROR" => self.error = non_empty(value).filter(|error| error != "none"),
            "DETAIL" => self.detail = non_empty(value),
            "CONFLICT" => self.conflict = non_empty(value),
            "CONFLICT_UNIT" => {
                if let Some(kind) = UnitKind::parse(value) {
                    if !self.conflict_units.contains(&kind) {
                        self.conflict_units.push(kind);
                    }
                }
            }
            "CHOICE" => {
                self.choice = serde_json::from_str(value).ok();
            }
            "CLOUD_VERSION" => self.cloud.version = value.parse().ok(),
            "CLOUD_DEVICE" => self.cloud.device = non_empty(value),
            "CLOUD_UPDATED" => self.cloud.updated = non_empty(value),
            "LOCAL_VERSION" => self.local.version = value.parse().ok(),
            "LOCAL_DEVICE" => self.local.device = non_empty(value),
            "LOCAL_UPDATED" => self.local.updated = non_empty(value),
            _ => {}
        }
    }

    fn finish(self, exit: Option<i32>, stderr: &[String]) -> SyncOutcome {
        let result = self.result.unwrap_or(match exit {
            Some(0) => SyncResult::Ok,
            Some(2) => SyncResult::Conflict,
            _ => SyncResult::Error,
        });
        let error = self.error.or_else(|| match (result, exit) {
            (SyncResult::Error | SyncResult::Partial, Some(1)) => Some("net".to_string()),
            (SyncResult::Error | SyncResult::Partial, Some(3)) => Some("config".to_string()),
            (SyncResult::Error | SyncResult::Partial, Some(4)) => Some("fs".to_string()),
            (SyncResult::Conflict, Some(2)) => Some("remote-newer".to_string()),
            _ => None,
        });
        let detail = self.detail.or_else(|| {
            (matches!(result, SyncResult::Error | SyncResult::Partial))
                .then(|| stderr.last().cloned())
                .flatten()
        });
        let changed = (error.as_deref() == Some("changed-since"))
            .then(|| parse_changed_since(detail.as_deref().unwrap_or("")));
        let info = if self.cloud.is_empty() && self.local.is_empty() {
            None
        } else {
            Some(ConflictInfo {
                cloud: self.cloud,
                local: self.local,
            })
        };
        let state = (!self.state.is_empty()).then(|| self.state.finish(UnitKind::State));
        let sav = (!self.sav.is_empty()).then(|| self.sav.finish(UnitKind::Sav));
        let mut conflict_units = self.conflict_units;
        if conflict_units.is_empty() {
            // Older CLI output may omit CONFLICT_UNIT; the per-unit results
            // still say which units conflict.
            for unit in state.iter().chain(sav.iter()) {
                if unit.result == Some(SyncResult::Conflict) {
                    conflict_units.push(unit.kind);
                }
            }
        }
        SyncOutcome {
            result,
            direction: self.direction,
            version: self.version,
            error,
            detail,
            conflict: self.conflict,
            info,
            state,
            sav,
            conflict_units,
            changed,
            choice: self.choice,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(resolve: Option<Resolve>) -> SyncRequest {
        let request = SyncRequest::new("GBA", "mgba", "合金弹头", "/mnt/sdcard");
        match resolve {
            Some(resolve) => request.resolved(resolve),
            None => request,
        }
    }

    fn parse_block(lines: &[&str]) -> SyncOutcome {
        let mut block = Block::default();
        for line in lines {
            block.absorb(line);
        }
        block.finish(Some(0), &[])
    }

    #[test]
    fn an_ok_block_keeps_the_direction_and_version() {
        let outcome = parse_block(&[
            "LINKSLOT_ACTION=sync",
            "LINKSLOT_RESULT=ok",
            "LINKSLOT_DIRECTION=download",
            "LINKSLOT_VERSION=7",
            "LINKSLOT_ERROR=none",
            "LINKSLOT_DETAIL=已拉取版本 7",
        ]);
        assert_eq!(outcome.result, SyncResult::Ok);
        assert_eq!(outcome.direction, Some(Direction::Download));
        assert_eq!(outcome.version, Some(7));
        assert_eq!(outcome.banner_text(), "已拉取版本 7");
        assert_eq!(outcome.error_line(), None);
    }

    #[test]
    fn a_choice_block_parses_the_request() {
        let line = r#"LINKSLOT_CHOICE={"key":"conflict","title":"存档冲突","body":"都改过","options":[{"id":"remote","label":"覆盖本地"},{"id":"local","label":"覆盖远端"}],"default":"remote"}"#;
        let outcome = parse_block(&[
            "LINKSLOT_ACTION=mods",
            "LINKSLOT_RESULT=choice",
            "LINKSLOT_ERROR=none",
            "LINKSLOT_DETAIL=需要选择",
            line,
        ]);
        assert_eq!(outcome.result, SyncResult::Choice);
        let request = outcome.choice.as_ref().expect("choice request");
        assert_eq!(request.key, "conflict");
        assert_eq!(request.options.len(), 2);
        assert_eq!(request.default.as_deref(), Some("remote"));
        assert_eq!(outcome.banner_text(), "需要选择");
    }

    #[test]
    fn request_argv_keeps_accumulated_choices() {
        let request = SyncRequest::manual("GBA", "mgba", "Game", "/mnt/sdcard")
            .with_choice("conflict", "local");
        let argv = request.argv();
        assert!(argv
            .windows(2)
            .any(|pair| pair[0] == "--choice" && pair[1] == "conflict=local"));
    }

    #[test]
    fn an_upload_block_says_pushed() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=ok",
            "LINKSLOT_DIRECTION=upload",
            "LINKSLOT_VERSION=3",
        ]);
        assert_eq!(outcome.direction, Some(Direction::Upload));
        assert_eq!(outcome.banner_text(), "已推送版本 3");
    }

    #[test]
    fn a_conflict_block_carries_both_sides() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_DETAIL=云端与本地均已修改",
            "LINKSLOT_CLOUD_VERSION=7",
            "LINKSLOT_CLOUD_DEVICE=rg34xx-a1b2c3d4",
            "LINKSLOT_CLOUD_UPDATED=2026-10-20T10:00:00Z",
            "LINKSLOT_LOCAL_VERSION=6",
            "LINKSLOT_LOCAL_DEVICE=rg34xx-deadbeef",
            "LINKSLOT_LOCAL_UPDATED=2026-10-19T09:00:00Z",
        ]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        assert_eq!(outcome.conflict_line(), "云端与本地已分叉");
        let (cloud, local) = outcome.conflict_lines().expect("lines");
        assert!(cloud.contains("v7"));
        assert!(cloud.contains("rg34xx-a1b2c3d4"));
        assert!(local.contains("v6"));
        assert!(local.contains("2026-10-19T09:00:00Z"));
    }

    #[test]
    fn missing_conflict_fields_read_as_unknown() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=no-cache",
            "LINKSLOT_CLOUD_VERSION=7",
        ]);
        let (cloud, local) = outcome.conflict_lines().expect("lines");
        assert!(cloud.contains("v7"));
        assert_eq!(local, "本地 未知");
    }

    #[test]
    fn exit_codes_are_the_fallback_when_the_block_is_cut_short() {
        let mut block = Block::default();
        let outcome = block.finish(Some(2), &[]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        block = Block::default();
        let outcome = block.finish(Some(4), &["disk full".to_string()]);
        assert_eq!(outcome.result, SyncResult::Error);
        assert_eq!(outcome.error.as_deref(), Some("fs"));
        assert_eq!(outcome.detail.as_deref(), Some("disk full"));
    }

    #[test]
    fn argv_is_sync_with_an_optional_resolve() {
        assert_eq!(
            request(None).argv(),
            [
                "sync",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "合金弹头",
                "--root",
                "/mnt/sdcard",
            ]
        );
        let forced = request(Some(Resolve::Download)).argv();
        assert_eq!(forced.last().map(String::as_str), Some("download"));
        assert!(forced.contains(&"--resolve".to_string()));
        let forced = request(Some(Resolve::Upload)).argv();
        assert_eq!(forced.last().map(String::as_str), Some("upload"));
    }

    #[test]
    fn resolves_have_verbs_and_directions() {
        for resolve in Resolve::ALL {
            assert_eq!(Resolve::parse(resolve.verb()), Some(resolve));
            assert_eq!(
                resolve.direction(),
                match resolve {
                    Resolve::Upload => Direction::Upload,
                    Resolve::Download => Direction::Download,
                }
            );
        }
        assert_eq!(Resolve::parse("banana"), None);
    }

    #[test]
    fn stages_and_directions_parse_from_the_cli_lines_the_app_shows() {
        assert_eq!(parse_stage_line("LINKSLOT_STAGE=net"), Some(SyncStage::Net));
        assert_eq!(
            parse_stage_line("LINKSLOT_STAGE=upload"),
            Some(SyncStage::Upload)
        );
        assert_eq!(parse_stage_line("LINKSLOT_STAGE=banana"), None);
        assert_eq!(
            parse_stage_line("LINKSLOT_STAGE=mods"),
            Some(SyncStage::Mods)
        );
        assert_eq!(SyncStage::Mods.line(), "正在同步");
        assert_eq!(parse_progress_line("LINKSLOT_PROGRESS=42"), Some(0.42));
        assert_eq!(parse_progress_line("LINKSLOT_PROGRESS=200"), Some(1.0));
        assert_eq!(parse_progress_line("LINKSLOT_PROGRESS=bad"), None);
        assert_eq!(
            parse_progress_label_line("LINKSLOT_PROGRESS_LABEL=正在生成攻略"),
            Some("正在生成攻略".to_string())
        );
        assert_eq!(
            parse_direction_line("LINKSLOT_DIRECTION=download"),
            Some(Direction::Download)
        );
        assert_eq!(parse_direction_line("LINKSLOT_DIRECTION=none"), None);
        assert_eq!(parse_direction_line("LINKSLOT_STAGE=upload"), None);
    }

    #[test]
    fn the_system_command_uses_sh_and_direct_argv() {
        let cmd = system_command(&request(Some(Resolve::Upload)));
        assert_eq!(cmd.get_program(), "/bin/sh");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let script = PathBuf::from("/mnt/sdcard")
            .join(crate::root::SYSTEM_SHELL)
            .join("linkslot.sh");
        assert_eq!(PathBuf::from(&args[0]), script);
        assert_eq!(args[1], "sync");
        assert_eq!(args.last().map(String::as_str), Some("upload"));
    }

    #[test]
    fn a_manual_request_runs_mods_run_without_cloud_flags() {
        let request = SyncRequest::manual("GBA", "mgba", "合金弹头", "/mnt/sdcard")
            .with_only(UnitKind::Sav)
            .resolved(Resolve::Upload);
        let argv = request.argv();
        assert_eq!(&argv[0..2], &["mods", "run"]);
        assert!(!argv.iter().any(|arg| arg == "--only"));
        assert!(!argv.iter().any(|arg| arg == "--resolve"));
        assert_eq!(request.mode, SyncRequestMode::Mods);
        assert_eq!(
            SyncRequest::new("GBA", "mgba", "x", "/r").mode,
            SyncRequestMode::Cloud
        );
    }

    #[test]
    fn a_boot_request_targets_one_after_boot_action() {
        let request = SyncRequest::boot("/mnt/sdcard", "boot", "warmup");
        assert_eq!(request.mode, SyncRequestMode::Boot);
        let argv = request.argv();
        assert_eq!(&argv[0..2], &["mods", "boot"]);
        assert!(argv.windows(2).any(|pair| pair == ["--id", "boot"]));
        assert!(argv.windows(2).any(|pair| pair == ["--action", "warmup"]));
        assert!(argv.windows(2).any(|pair| pair == ["--root", "/mnt/sdcard"]));
        assert!(argv.iter().any(|arg| arg == "--verbose"));
        assert!(!argv.iter().any(|arg| arg == "--platform"));
        assert!(!argv.iter().any(|arg| arg == "--game"));
    }

    // ---- v0.0.10 two-unit results ------------------------------------------

    #[test]
    fn two_conflicting_units_are_parsed_per_unit() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_DETAIL=即时存档与电池存档均存在冲突，请选择覆盖方向",
            "LINKSLOT_STATE_RESULT=conflict",
            "LINKSLOT_STATE_ERROR=forked",
            "LINKSLOT_STATE_CLOUD_VERSION=7",
            "LINKSLOT_STATE_CLOUD_DEVICE=rg34xx-a1b2c3d4",
            "LINKSLOT_STATE_CLOUD_UPDATED=2026-10-20T10:00:00Z",
            "LINKSLOT_STATE_CLOUD_FILES=4",
            "LINKSLOT_STATE_LOCAL_VERSION=6",
            "LINKSLOT_STATE_LOCAL_FILES=4",
            "LINKSLOT_SAV_RESULT=conflict",
            "LINKSLOT_SAV_ERROR=no-cache",
            "LINKSLOT_SAV_CLOUD_VERSION=3",
            "LINKSLOT_SAV_LOCAL_FILES=0",
            "LINKSLOT_CONFLICT_UNIT=state",
            "LINKSLOT_CONFLICT_UNIT=sav",
        ]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        assert_eq!(outcome.error.as_deref(), Some("forked"));
        assert_eq!(outcome.conflict_units, [UnitKind::State, UnitKind::Sav]);
        assert!(outcome.is_conflicting(UnitKind::State));
        assert!(outcome.is_conflicting(UnitKind::Sav));

        let state = outcome.state.as_ref().expect("state unit");
        assert_eq!(state.kind, UnitKind::State);
        assert_eq!(state.result, Some(SyncResult::Conflict));
        assert_eq!(state.error.as_deref(), Some("forked"));
        let info = state.info.as_ref().expect("state info");
        assert_eq!(info.cloud.version, Some(7));
        assert_eq!(info.cloud.files, Some(4));
        assert_eq!(info.local.version, Some(6));
        assert_eq!(info.cloud.device.as_deref(), Some("rg34xx-a1b2c3d4"));
        let (cloud, _local) = state.conflict_lines().expect("state lines");
        assert!(cloud.contains("v7") && cloud.contains("rg34xx-a1b2c3d4"));

        let sav = outcome.sav.as_ref().expect("sav unit");
        assert_eq!(sav.result, Some(SyncResult::Conflict));
        assert_eq!(sav.conflict_line(), "云端已有存档但本地无同步记录");
        let sav_info = sav.info.as_ref().expect("sav info");
        assert_eq!(sav_info.cloud.version, Some(3));
        assert_eq!(sav_info.local.version, None);
        assert_eq!(
            outcome.unit_result(UnitKind::Sav),
            Some(SyncResult::Conflict)
        );
    }

    #[test]
    fn a_single_conflict_still_reports_the_successful_unit() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_DETAIL=电池存档存在冲突，请选择覆盖方向",
            "LINKSLOT_STATE_RESULT=ok",
            "LINKSLOT_STATE_DIRECTION=upload",
            "LINKSLOT_STATE_VERSION=8",
            "LINKSLOT_SAV_RESULT=conflict",
            "LINKSLOT_SAV_ERROR=forked",
            "LINKSLOT_SAV_CLOUD_VERSION=4",
            "LINKSLOT_SAV_LOCAL_VERSION=2",
            "LINKSLOT_CONFLICT_UNIT=sav",
        ]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        assert_eq!(outcome.conflict_units, [UnitKind::Sav]);
        let state = outcome.state.as_ref().expect("state unit");
        assert_eq!(state.result, Some(SyncResult::Ok));
        assert_eq!(state.direction, Some(Direction::Upload));
        assert_eq!(state.version, Some(8));
        assert!(state.info.is_none());
        assert_eq!(outcome.units(), [UnitKind::State, UnitKind::Sav]);
        // The legacy aggregate info stays empty when only per-unit sides exist.
        assert!(outcome.info.is_none());
    }

    #[test]
    fn a_partial_sync_keeps_both_units_and_the_failing_category() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=partial",
            "LINKSLOT_ERROR=net",
            "LINKSLOT_DETAIL=即时存档已同步，电池存档失败：TCP 连接失败",
            "LINKSLOT_STATE_RESULT=ok",
            "LINKSLOT_STATE_DIRECTION=upload",
            "LINKSLOT_STATE_VERSION=9",
            "LINKSLOT_SAV_RESULT=error",
            "LINKSLOT_SAV_ERROR=net",
            "LINKSLOT_SAV_CLOUD_VERSION=3",
        ]);
        assert_eq!(SyncResult::parse("partial"), Some(SyncResult::Partial));
        assert_eq!(outcome.result, SyncResult::Partial);
        assert_eq!(outcome.error.as_deref(), Some("net"));
        assert_eq!(outcome.unit_result(UnitKind::State), Some(SyncResult::Ok));
        assert_eq!(outcome.unit_result(UnitKind::Sav), Some(SyncResult::Error));
        assert!(outcome.conflict_units.is_empty());
        assert_eq!(
            outcome.banner_text(),
            "即时存档已同步，电池存档失败：TCP 连接失败"
        );
        assert_eq!(outcome.error_line(), Some("网络未连接"));
    }

    #[test]
    fn partial_without_a_detail_uses_the_error_category() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=partial",
            "LINKSLOT_ERROR=fs",
            "LINKSLOT_STATE_RESULT=ok",
            "LINKSLOT_SAV_RESULT=error",
        ]);
        assert_eq!(outcome.result, SyncResult::Partial);
        assert_eq!(outcome.error_line(), Some("存储卡读写失败"));
        assert_eq!(outcome.banner_text(), "存储卡读写失败");
    }

    #[test]
    fn changed_since_carries_the_new_cloud_version() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=changed-since",
            "LINKSLOT_DETAIL=电池存档云端版本已变化（期望 v12，实际 v13）；云端已被其它设备更新，请基于最新版本重新选择方向",
        ]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        assert_eq!(outcome.error.as_deref(), Some("changed-since"));
        assert_eq!(
            outcome.changed,
            Some(ChangedSince {
                unit: Some(UnitKind::Sav),
                expected: Some(12),
                actual: Some(13),
            })
        );
        assert!(outcome.conflict_line().contains("云端已被其它设备更新"));
        assert_eq!(outcome.banner_text(), outcome.conflict_line());

        let fresh = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=changed-since",
            "LINKSLOT_DETAIL=即时存档云端还没有存档（期望 v5）；云端已被其它设备更新",
        ]);
        assert_eq!(
            fresh.changed,
            Some(ChangedSince {
                unit: Some(UnitKind::State),
                expected: Some(5),
                actual: None,
            })
        );
    }

    #[test]
    fn changed_since_detail_parses_the_new_cloud_version() {
        assert_eq!(
            parse_changed_since("电池存档云端版本已变化（期望 v12，实际 v13）；提示"),
            ChangedSince {
                unit: Some(UnitKind::Sav),
                expected: Some(12),
                actual: Some(13),
            }
        );
        assert_eq!(
            parse_changed_since("云端版本已变化（期望 v2，实际 v3）"),
            ChangedSince {
                unit: None,
                expected: Some(2),
                actual: Some(3),
            }
        );
        assert_eq!(parse_changed_since("没有版本"), ChangedSince::default());
    }

    #[test]
    fn conflict_units_keep_cli_order_and_are_deduped() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_CONFLICT_UNIT=sav",
            "LINKSLOT_CONFLICT_UNIT=state",
            "LINKSLOT_CONFLICT_UNIT=sav",
        ]);
        assert_eq!(outcome.conflict_units, [UnitKind::Sav, UnitKind::State]);
    }

    #[test]
    fn conflict_units_are_derived_from_unit_results_when_the_cli_omits_them() {
        let outcome = parse_block(&[
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_STATE_RESULT=ok",
            "LINKSLOT_SAV_RESULT=conflict",
            "LINKSLOT_SAV_ERROR=forked",
        ]);
        assert_eq!(outcome.conflict_units, [UnitKind::Sav]);
    }

    #[test]
    fn missing_unit_fields_are_optional_and_never_panic() {
        let outcome = parse_block(&["LINKSLOT_STATE_RESULT=ok"]);
        assert!(outcome.has_unit_results());
        assert_eq!(outcome.units(), [UnitKind::State]);
        let state = outcome.state.as_ref().expect("state unit");
        assert_eq!(state.result, Some(SyncResult::Ok));
        assert!(state.direction.is_none());
        assert!(state.version.is_none());
        assert!(state.info.is_none());
        assert!(outcome.sav.is_none());
        assert!(outcome.changed.is_none());
        assert!(outcome.conflict_units.is_empty());
    }

    #[test]
    fn old_single_unit_output_still_reads_as_before() {
        let outcome = parse_block(&[
            "LINKSLOT_ACTION=sync",
            "LINKSLOT_RESULT=conflict",
            "LINKSLOT_ERROR=forked",
            "LINKSLOT_DETAIL=云端与本地均已修改",
            "LINKSLOT_CLOUD_VERSION=7",
            "LINKSLOT_CLOUD_DEVICE=rg34xx-a1b2c3d4",
            "LINKSLOT_LOCAL_VERSION=6",
        ]);
        assert_eq!(outcome.result, SyncResult::Conflict);
        assert!(!outcome.has_unit_results());
        assert!(outcome.state.is_none());
        assert!(outcome.sav.is_none());
        assert!(outcome.units().is_empty());
        assert!(outcome.conflict_units.is_empty());
        let (cloud, local) = outcome.conflict_lines().expect("legacy lines");
        assert!(cloud.contains("v7"));
        assert!(local.contains("v6"));
    }

    #[test]
    fn argv_carries_only_resolve_and_expect_in_order() {
        let argv = SyncRequest::new("GBA", "mgba", "合金弹头", "/mnt/sdcard")
            .with_only(UnitKind::State)
            .with_resolve(ResolveSpec::all(Direction::Download))
            .with_expect(ExpectedVersions {
                state: Some(12),
                sav: Some(7),
            })
            .argv();
        assert_eq!(
            &argv[argv.len() - 6..],
            [
                "--only",
                "state",
                "--resolve",
                "download",
                "--expect",
                "state=12,sav=7",
            ]
        );
        assert!(argv.iter().any(|arg| arg == "合金弹头"));

        let per_unit = SyncRequest::new("GBA", "mgba", "宝可梦", "/mnt/sdcard")
            .with_only(UnitKind::Sav)
            .with_resolve(ResolveSpec {
                state: Some(Direction::Upload),
                sav: Some(Direction::Download),
            })
            .expect_unit(UnitKind::Sav, 4)
            .argv();
        assert!(per_unit.windows(2).any(|pair| pair == ["--only", "sav"]));
        assert!(per_unit
            .windows(2)
            .any(|pair| pair == ["--resolve", "state=upload,sav=download"]));
        assert!(per_unit
            .windows(2)
            .any(|pair| pair == ["--expect", "sav=4"]));
    }

    #[test]
    fn a_bare_resolve_is_one_argument_for_the_cli() {
        assert_eq!(
            ResolveSpec::all(Direction::Upload).argv_value(),
            Some("upload".to_string())
        );
        assert_eq!(ResolveSpec::default().argv_value(), None);
        assert_eq!(
            ResolveSpec {
                state: Some(Direction::Download),
                sav: None,
            }
            .argv_value(),
            Some("state=download".to_string())
        );
        assert_eq!(ExpectedVersions::default().argv_value(), None);
    }

    #[test]
    fn units_have_cli_names_chinese_labels_and_parse() {
        assert_eq!(UnitKind::ALL, [UnitKind::State, UnitKind::Sav]);
        for kind in UnitKind::ALL {
            assert_eq!(UnitKind::parse(kind.as_str()), Some(kind));
            assert!(!kind.label().is_empty());
            assert!(!kind.short().is_empty());
        }
        assert_eq!(UnitKind::parse("cheats"), None);
        assert_eq!(UnitKind::parse(" state "), Some(UnitKind::State));
    }
}
