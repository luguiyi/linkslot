//! Sync-unit identity, validation and local whitelist scanning.
//!
//! Locally a save-state unit is `<root>/States/<platform>/<core>/<game>/`; the
//! whitelist is `resume.state` plus `<stamp>.state` / `<stamp>.png` pairs.
//! Everything else (`resume-refused-*`, `.*.tmp`, `.linkslot-*`, unknown files)
//! is local-only and must never be uploaded or deleted.
//!
//! The cloud identity is the *full-file SHA-256 of the ROM*:
//! `<platform>/<rom sha256>/<kind>[/<core>]` (v0.0.10 layout). The game name
//! only names the local directory, so renaming a ROM keeps the same cloud
//! unit. There is no game-name identity: a unit without a ROM hash cannot be
//! synchronised.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::error::{fs_error, ErrorKind, LinkError, Result};
use crate::hash;
use crate::util::is_stamp;

/// Slot's ring max; recorded in `meta.ring.max` but never enforced by the CLI.
pub const RING_MAX: usize = 10;
pub const STATES_DIR: &str = "States";
pub const SAVES_DIR: &str = "Saves";
pub const RESUME: &str = "resume.state";
/// The cloud name of the battery save; the local file keeps the game name.
pub const BATTERY: &str = "battery.sav";
/// Local battery-save extension (preferred) and its read fallback.
pub const SAV_EXT: &str = "sav";
pub const SRM_EXT: &str = "srm";
/// 同步单元可以命名的机种，拼写与卡上 `Games/<dir>/` 完全一致。
///
/// 直接投影 `slot-model` 的机种表：机种一旦加入前端，同步端自动认识它，
/// 不会再出现"前端有货架、云端不认"的两份清单。不在此表里的机种会在
/// `validate_platform` 被拒绝，读写都不会发生。
pub fn platform_all() -> impl Iterator<Item = &'static str> {
    slot_model::MACHINES.iter().map(|m| m.dir)
}

/// 单元可以挂在哪些核心下，来自核心表（`slot-model::CORES`）。
///
/// 核心能跑哪些机种由核心表的 `machines` 字段声明，`validate_emulator`
/// 直接查表，不再维护"例外配对表"。
pub fn emulators() -> impl Iterator<Item = &'static str> {
    slot_model::core_ids()
}

/// Cloud directory names of the sync units (`<platform>/<sha>/<kind>[/<core>]`).
pub const STATE_DIR: &str = "state";
pub const SAV_DIR: &str = "sav";
/// Cache-key segment for a unit without a core scope.
pub const NO_SCOPE: &str = "-";

/// Layout marker at the remote root; the first thing a client validates.
pub const LAYOUT_NAME: &str = "layout.json";
pub const LAYOUT_VERSION: u32 = 1;
pub const LAYOUT_APP: &str = "linkSlot";

const FAT_ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
const GAME_MAX_BYTES: usize = 200;
const CORE_MAX_BYTES: usize = 32;

/// Identity of one sync unit. Strings are validated so they can be used both
/// as FAT directory names and WebDAV path segments.
///
/// `rom_sha256` is the full-file SHA-256 of the ROM in lowercase hex (no
/// `sha256:` prefix). It is the only cloud identity: a unit without it is
/// local-only and every remote path/cache lookup is a configuration error.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Unit {
    pub platform: String,
    pub emulator: String,
    pub game: String,
    pub rom_sha256: Option<String>,
}

impl Unit {
    pub fn new(platform: &str, emulator: &str, game: &str) -> Result<Self> {
        validate_platform(platform)?;
        validate_emulator(platform, emulator)?;
        validate_game(game)?;
        Ok(Unit {
            platform: platform.to_string(),
            emulator: emulator.to_string(),
            game: game.to_string(),
            rom_sha256: None,
        })
    }

    /// Attach the ROM identity. Accepts `sha256:<hex>` or bare hex in any
    /// case and stores the normalized lowercase bare digest.
    pub fn with_rom_sha256(mut self, digest: &str) -> Result<Self> {
        let normalized = hash::normalize_digest(digest);
        if !hash::is_valid_digest(normalized) {
            return Err(LinkError::config(format!(
                "ROM SHA-256 无效：{digest:?}（需要 64 位十六进制）"
            )));
        }
        self.rom_sha256 = Some(normalized.to_ascii_lowercase());
        Ok(self)
    }

    /// The ROM digest this unit is identified by, or a configuration error.
    ///
    /// The cloud layout (<platform>/<rom sha256>/...) has no game-name
    /// identity, so every remote operation must call this first instead of
    /// falling back to `game`.
    pub fn require_rom_sha256(&self) -> Result<&str> {
        match &self.rom_sha256 {
            Some(digest) if hash::is_valid_digest(digest) => Ok(digest),
            _ => Err(LinkError::config(
                "无法确定 ROM 的 SHA-256，云端身份缺失；请用 --rom 指定 ROM 路径",
            )),
        }
    }

    /// Local battery save Slot uses: `<root>/Saves/<platform>/<game>.sav`.
    pub fn local_sav_path(&self, root: &Path) -> PathBuf {
        self.sav_dir(root)
            .join(format!("{}.{}", self.game, SAV_EXT))
    }

    /// Read fallback for emulators that leave `<game>.srm` behind.
    pub fn local_srm_path(&self, root: &Path) -> PathBuf {
        self.sav_dir(root)
            .join(format!("{}.{}", self.game, SRM_EXT))
    }

    /// `<root>/Saves/<platform>` — shared by every game of the platform, so a
    /// unit must only ever touch its own file inside it.
    pub fn sav_dir(&self, root: &Path) -> PathBuf {
        root.join(SAVES_DIR).join(&self.platform)
    }

    /// Stable unit id used for logs: `GBA/mgba/<rom sha256>`.
    ///
    /// The game name is deliberately not part of it — v0.0.10 dropped the
    /// game-name identity, so a unit without a ROM SHA-256 has no id at all.
    pub fn id(&self) -> Result<String> {
        Ok(format!(
            "{}/{}/{}",
            self.platform,
            self.emulator,
            self.require_rom_sha256()?
        ))
    }

    /// Path under the card root: `<root>/States/<platform>/<emulator>/<game>`.
    /// The local directory always uses the game name, even for hash units.
    pub fn local_dir(&self, root: &Path) -> PathBuf {
        root.join(STATES_DIR)
            .join(&self.platform)
            .join(&self.emulator)
            .join(&self.game)
    }

    /// Parent directory (`States/<platform>/<emulator>`), used for conflict
    /// copies which live next to the unit directory.
    pub fn local_parent(&self, root: &Path) -> PathBuf {
        root.join(STATES_DIR)
            .join(&self.platform)
            .join(&self.emulator)
    }

    /// Cloud *game* directory segments below the remote root:
    /// `<platform>/<rom sha256>`.
    ///
    /// v0.0.10 removed the game-name identity: a unit without a ROM SHA-256
    /// has no cloud path at all, so this returns an empty vector. Network
    /// callers must use [`Unit::try_remote_segments`] or
    /// [`UnitSpec::remote_segments`] to get an error instead.
    pub fn remote_segments(&self) -> Vec<String> {
        self.try_remote_segments().unwrap_or_default()
    }

    /// Fallible form of [`Unit::remote_segments`]: a missing ROM SHA-256 is a
    /// configuration error rather than a game-name fall-back.
    pub fn try_remote_segments(&self) -> Result<Vec<String>> {
        Ok(vec![
            self.platform.clone(),
            self.require_rom_sha256()?.to_string(),
        ])
    }
}

// ---------------------------------------------------------------------------
// Cloud layout marker
// ---------------------------------------------------------------------------

/// `layout.json` at the remote root.
///
/// It is written once, before the first unit is uploaded, and validated on
/// every read. A cloud whose marker is missing (while unit data exists) or
/// incompatible is rejected instead of being silently adopted: v0.0.10 has no
/// migration path, and old `<platform>/<core>/<game>` directories are never
/// read, moved or deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub layout: u32,
    pub app: String,
    pub units: Vec<String>,
}

impl Layout {
    /// The layout this build writes: `{"layout":1,"app":"linkSlot",
    /// "units":["state","sav"]}`.
    pub fn current() -> Layout {
        Layout {
            layout: LAYOUT_VERSION,
            app: LAYOUT_APP.to_string(),
            units: Kind::ALL
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|err| {
            LinkError::new(ErrorKind::Config, format!("layout.json 生成失败：{err}"))
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Layout> {
        serde_json::from_slice::<Layout>(bytes).map_err(|err| {
            LinkError::new(
                ErrorKind::Verify,
                format!("云端 layout.json 格式无效：{err}"),
            )
        })
    }

    /// Accept exactly this layout version and app, and require every unit this
    /// build knows about. Unknown extra units are tolerated so a newer client
    /// can add one without locking older readers out.
    pub fn validate(&self) -> Result<()> {
        if self.layout != LAYOUT_VERSION {
            return Err(LinkError::new(
                ErrorKind::Verify,
                format!(
                    "云端 layout.json 版本不支持（{}，本机支持 {LAYOUT_VERSION}）",
                    self.layout
                ),
            ));
        }
        if self.app != LAYOUT_APP {
            return Err(LinkError::new(
                ErrorKind::Verify,
                format!(
                    "云端 layout.json 属于其它应用（{:?}，期望 {LAYOUT_APP:?}）",
                    self.app
                ),
            ));
        }
        for kind in Kind::ALL {
            if !self.units.iter().any(|unit| unit == kind.as_str()) {
                return Err(LinkError::new(
                    ErrorKind::Verify,
                    format!("云端 layout.json 缺少单元 {:?}", kind.as_str()),
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sync-unit model (v0.0.10)
// ---------------------------------------------------------------------------

/// Which kind of data a sync unit carries.
///
/// The cloud directory is named after this value (`state/`, `sav/`), and
/// `meta.json` records it so a state meta can never be read as a battery meta
/// (or the other way round).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Save states (`resume.state` + ring files + thumbnails).
    #[default]
    State,
    /// Battery save (`battery.sav`), shared by every core of one ROM.
    Sav,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::State, Kind::Sav];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::State => STATE_DIR,
            Kind::Sav => SAV_DIR,
        }
    }

    /// Cloud directory name; identical to [`Kind::as_str`].
    pub fn dir_name(self) -> &'static str {
        self.as_str()
    }

    pub fn parse(value: &str) -> Option<Kind> {
        match value {
            STATE_DIR => Some(Kind::State),
            SAV_DIR => Some(Kind::Sav),
            _ => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Kind {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Kind::parse(&value).ok_or_else(|| {
            de::Error::custom(format!("未知的同步单元类型 {value:?}（支持 state/sav）"))
        })
    }
}

/// Where a unit's content is scoped. Save states are per emulator core, the
/// battery save is shared by every core that plays the same ROM.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Core(String),
    None,
}

impl Scope {
    pub fn core(&self) -> Option<&str> {
        match self {
            Scope::Core(core) => Some(core),
            Scope::None => None,
        }
    }

    /// Extra cloud path segment (`state/<core>/`), empty for [`Scope::None`].
    pub fn segments(&self) -> Vec<String> {
        match self {
            Scope::Core(core) => vec![core.clone()],
            Scope::None => Vec::new(),
        }
    }
}

/// Local target of a unit: a directory (`States/<p>/<core>/<game>/`) or a
/// single file (`Saves/<p>/<game>.sav`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalTarget {
    Dir(PathBuf),
    File(PathBuf),
}

impl LocalTarget {
    pub fn path(&self) -> &Path {
        match self {
            LocalTarget::Dir(path) | LocalTarget::File(path) => path,
        }
    }

    pub fn as_dir(&self) -> Option<&Path> {
        match self {
            LocalTarget::Dir(path) => Some(path),
            LocalTarget::File(_) => None,
        }
    }

    pub fn as_file(&self) -> Option<&Path> {
        match self {
            LocalTarget::Dir(_) => None,
            LocalTarget::File(path) => Some(path),
        }
    }
}

/// Which local/cloud file names a unit owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whitelist {
    /// `resume.state`, `<stamp>.state` and `<stamp>.png` (thumbnail only when
    /// its state exists).
    StateRing,
    /// Exactly one file: `battery.sav`.
    SingleBattery,
}

impl Whitelist {
    /// Whether `name` belongs to the unit. `battery.sav` is never a state file
    /// and a state file is never a battery file, so the two kinds can never
    /// claim each other's data.
    pub fn allows(self, name: &str) -> bool {
        match self {
            Whitelist::StateRing => is_meta_file_name(name),
            Whitelist::SingleBattery => name == BATTERY,
        }
    }

    /// The file a push cannot do without (`resume.state` / `battery.sav`).
    pub fn required(self) -> &'static str {
        match self {
            Whitelist::StateRing => RESUME,
            Whitelist::SingleBattery => BATTERY,
        }
    }
}

/// How the unit's data is moved. Save states need the multi-file journal
/// protocol; the battery save is a single-file rename inside the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    MultiFileJournal,
    SingleFileRename,
}

/// Name of the backup made when a unit is overwritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictCopy {
    /// `<unit dir>/<game>.conflict.<stamp>/`
    Dir,
    /// `<game>.conflict.<stamp>.sav` next to the battery save.
    File,
}

/// Complete description of one sync unit: identity scope, paths, whitelist,
/// transfer strategy and conflict-copy rule.
///
/// `Unit` stays the *game* identity (platform/emulator/game/ROM hash); a
/// `UnitSpec` adds the unit kind on top. Adding a future unit (cheats, config)
/// means adding a [`Kind`] value and one constructor here, not changing the
/// cloud layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitSpec {
    pub kind: Kind,
    pub scope: Scope,
    pub local: LocalTarget,
    pub whitelist: Whitelist,
    pub transfer: Transfer,
    pub conflict_copy: ConflictCopy,
    /// A push needs [`Whitelist::required`] to exist locally.
    pub required_for_push: bool,
}

impl UnitSpec {
    /// Save states: `States/<p>/<core>/<game>/` and `<p>/<sha>/state/<core>/`.
    pub fn state(unit: &Unit, root: &Path) -> UnitSpec {
        UnitSpec {
            kind: Kind::State,
            scope: Scope::Core(unit.emulator.clone()),
            local: LocalTarget::Dir(unit.local_dir(root)),
            whitelist: Whitelist::StateRing,
            transfer: Transfer::MultiFileJournal,
            conflict_copy: ConflictCopy::Dir,
            required_for_push: true,
        }
    }

    /// Battery save: `Saves/<p>/<game>.sav` and `<p>/<sha>/sav/`.
    ///
    /// The unit is shared by every core, so it has no scope segment.
    pub fn sav(unit: &Unit, root: &Path) -> UnitSpec {
        UnitSpec {
            kind: Kind::Sav,
            scope: Scope::None,
            local: LocalTarget::File(unit.local_sav_path(root)),
            whitelist: Whitelist::SingleBattery,
            transfer: Transfer::SingleFileRename,
            conflict_copy: ConflictCopy::File,
            required_for_push: true,
        }
    }

    pub fn for_kind(kind: Kind, unit: &Unit, root: &Path) -> UnitSpec {
        match kind {
            Kind::State => UnitSpec::state(unit, root),
            Kind::Sav => UnitSpec::sav(unit, root),
        }
    }

    /// Cloud path segments of the unit below the remote root:
    /// `<platform>/<rom sha256>/<kind>[/<core>]`.
    ///
    /// A unit without a ROM SHA-256 has no cloud identity; this is a
    /// configuration error, never a silent fall-back to the game name.
    pub fn remote_segments(&self, unit: &Unit) -> Result<Vec<String>> {
        let digest = unit.require_rom_sha256()?;
        let mut segments = vec![unit.platform.clone(), digest.to_string()];
        segments.push(self.kind.dir_name().to_string());
        segments.extend(self.scope.segments());
        Ok(segments)
    }

    /// Stable id used for cache file names and logs, e.g.
    /// `state/GBA/mgba/<sha256>` or `sav/GBA/-/<sha256>`. Every identity
    /// dimension (kind, platform, core, ROM hash) is part of the key, so a
    /// state and a battery cache of the same game can never collide. `-`
    /// stands for "no scope" (a core id can never start with `-`).
    pub fn key(&self, unit: &Unit) -> Result<String> {
        let digest = unit.require_rom_sha256()?;
        let mut parts = vec![self.kind.as_str().to_string(), unit.platform.clone()];
        match &self.scope {
            Scope::Core(core) => parts.push(core.clone()),
            Scope::None => parts.push(NO_SCOPE.to_string()),
        }
        parts.push(digest.to_string());
        Ok(parts.join("/"))
    }
}

pub fn validate_platform(platform: &str) -> Result<()> {
    if platform_all().any(|dir| dir == platform) {
        Ok(())
    } else {
        // 列表由 `slot-model` 的机种表生成，避免与前端漂移。
        Err(LinkError::config(format!(
            "--platform 取值必须是 {}，收到 {platform:?}",
            platform_all().collect::<Vec<_>>().join("/")
        )))
    }
}

pub fn validate_emulator(platform: &str, emulator: &str) -> Result<()> {
    let Some(spec) = slot_model::core(emulator) else {
        return Err(LinkError::config(format!(
            "--emulator 取值必须是 {}，收到 {emulator:?}",
            emulators().collect::<Vec<_>>().join("/")
        )));
    };
    // 核心能跑哪些机种由核心表声明；不在表里的一律拒绝，避免同步出
    // 前端根本不会产生的 unit（例如 `States/GBA/fceumm/`）。
    let runs = spec
        .machines
        .iter()
        .any(|m| m.0 == platform);
    if !runs {
        return Err(LinkError::config(format!(
            "{emulator} 不支持 {platform}"
        )));
    }
    Ok(())
}

/// Validate a core/scope id used as a cloud path segment (`state/<core>/`).
///
/// Cores are ASCII identifiers; separators, dots and control characters are
/// rejected so a scope can never escape its unit directory.
pub fn validate_core(core: &str) -> Result<()> {
    if core.is_empty() {
        return Err(LinkError::config("core 不能为空"));
    }
    if core.len() > CORE_MAX_BYTES {
        return Err(LinkError::config(format!(
            "core 过长（{} 字节，上限 {CORE_MAX_BYTES}）",
            core.len()
        )));
    }
    if core == "." || core == ".." {
        return Err(LinkError::config("core 不能是 . 或 .."));
    }
    let mut chars = core.chars();
    if !chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric()) {
        return Err(LinkError::config("core 必须以字母或数字开头"));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        return Err(LinkError::config(format!(
            "core 只能包含字母、数字、_ 和 -，收到 {core:?}"
        )));
    }
    Ok(())
}

pub fn validate_game(game: &str) -> Result<()> {
    if game.is_empty() {
        return Err(LinkError::config("--game 不能为空"));
    }
    if game == "." || game == ".." {
        return Err(LinkError::config("--game 不能是 . 或 .."));
    }
    if game.len() > GAME_MAX_BYTES {
        return Err(LinkError::config(format!(
            "--game 过长（{} 字节，上限 {GAME_MAX_BYTES}）",
            game.len()
        )));
    }
    for ch in game.chars() {
        if ch.is_control() {
            return Err(LinkError::config("--game 不能包含控制字符"));
        }
        if FAT_ILLEGAL.contains(&ch) {
            return Err(LinkError::config(format!(
                "--game 包含 FAT 非法字符 {ch:?}"
            )));
        }
    }
    if game.ends_with(' ') || game.ends_with('.') {
        return Err(LinkError::config("--game 不能以空格或点结尾"));
    }
    Ok(())
}

/// A local whitelist file with its size (hash is added later).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub files: Vec<ScanFile>,
    pub has_resume: bool,
    pub ring_count: usize,
}

impl Scan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Scan a unit directory using the push whitelist:
/// `resume.state` + `<stamp>.state` + `<stamp>.png` (png only when its state
/// exists). Unknown and excluded files are ignored. A missing directory is an
/// empty scan.
pub fn scan_dir(dir: &Path) -> Result<Scan> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Scan::default()),
        Err(err) => return Err(fs_error(format!("无法读取目录 {}", dir.display()), err)),
    };

    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| fs_error(format!("无法读取目录 {}", dir.display()), err))?;
        let file_type = entry
            .file_type()
            .map_err(|err| fs_error(format!("无法读取 {}", entry.path().display()), err))?;
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        names.push(name);
    }

    let mut files = Vec::new();
    let mut has_resume = false;
    let mut ring_count = 0usize;
    let states: std::collections::HashSet<String> = names
        .iter()
        .filter_map(|name| state_stamp(name).map(str::to_string))
        .collect();

    for name in names {
        let path = dir.join(&name);
        let size = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(fs_error(format!("无法读取 {}", path.display()), err)),
        };
        if name == RESUME {
            has_resume = true;
            files.push(ScanFile { name, path, size });
            continue;
        }
        if let Some(stamp) = state_stamp(&name) {
            if states.contains(stamp) {
                ring_count += 1;
            }
            files.push(ScanFile { name, path, size });
            continue;
        }
        if let Some(stamp) = png_stamp(&name) {
            // A thumbnail without its state is not part of the v1 whitelist.
            if states.contains(stamp) {
                files.push(ScanFile { name, path, size });
            }
        }
    }

    files.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    Ok(Scan {
        files,
        has_resume,
        ring_count,
    })
}

/// Scan using the unit identity.
pub fn scan(root: &Path, unit: &Unit) -> Result<Scan> {
    scan_dir(&unit.local_dir(root))
}

/// All local files the pull path is allowed to replace or delete: the push
/// whitelist plus orphan `<stamp>.png` thumbnails (which are still "legal ring
/// files" per the deletion rules).
pub fn local_replaceable_files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(fs_error(format!("无法读取目录 {}", dir.display()), err)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| fs_error(format!("无法读取目录 {}", dir.display()), err))?;
        let file_type = entry
            .file_type()
            .map_err(|err| fs_error(format!("无法读取 {}", entry.path().display()), err))?;
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name == RESUME || state_stamp(&name).is_some() || png_stamp(&name).is_some() {
            out.push((name, entry.path()));
        }
    }
    out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(out)
}

/// `<stamp>.state` -> `<stamp>` when the stamp is valid.
pub fn state_stamp(name: &str) -> Option<&str> {
    let stamp = name.strip_suffix(".state")?;
    if stamp == "resume" || !is_stamp(stamp) {
        return None;
    }
    Some(stamp)
}

/// `<stamp>.png` -> `<stamp>` when the stamp is valid.
pub fn png_stamp(name: &str) -> Option<&str> {
    let stamp = name.strip_suffix(".png")?;
    if !is_stamp(stamp) {
        return None;
    }
    Some(stamp)
}

/// Names that are allowed in a save-state `meta.json` (schema 2 whitelist).
pub fn is_meta_file_name(name: &str) -> bool {
    name == RESUME || state_stamp(name).is_some() || png_stamp(name).is_some()
}

/// Convert the scan into `(name, size, sha256)` triples. The hashes are
/// computed in one pass; I/O errors abort the push before any network access.
pub fn hash_scan(scan: &Scan) -> Result<Vec<(String, u64, String)>> {
    let mut out = Vec::with_capacity(scan.files.len());
    for file in &scan.files {
        let digest = crate::hash::sha256_file(&file.path)?;
        out.push((file.name.clone(), file.size, digest));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Battery save (.sav / .srm)
// ---------------------------------------------------------------------------

/// Which local file the battery save was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavSource {
    /// `<game>.sav` — Slot's own file, always written by a pull.
    Sav,
    /// `<game>.srm` — emulator fallback, read but never written.
    Srm,
}

impl SavSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SavSource::Sav => SAV_EXT,
            SavSource::Srm => SRM_EXT,
        }
    }
}

/// One located battery save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavScan {
    /// Absolute path of the local file that was found.
    pub path: PathBuf,
    pub size: u64,
    pub source: SavSource,
}

impl SavScan {
    /// Cloud name of the battery data file (`battery.sav`, never the local
    /// game-name file).
    pub fn remote_name(&self) -> &'static str {
        BATTERY
    }
}

/// Locate the battery save of exactly this game.
///
/// `<root>/Saves/<platform>/<game>.sav` wins; `<game>.srm` is the read
/// fallback used by emulators that never write `.sav`. Only these two paths
/// are ever inspected: `Saves/<platform>/` is shared by every game, so it is
/// never listed, filtered or cleaned here. A missing file is `Ok(None)`; an
/// unreadable file or a non-file at one of the two paths is an `Fs` error.
pub fn scan_sav(root: &Path, unit: &Unit) -> Result<Option<SavScan>> {
    for (path, source) in [
        (unit.local_sav_path(root), SavSource::Sav),
        (unit.local_srm_path(root), SavSource::Srm),
    ] {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(fs_error(format!("无法读取 {}", path.display()), err)),
        };
        if !meta.is_file() {
            return Err(LinkError::fs(format!(
                "{} 不是普通文件，无法作为电池存档读取",
                path.display()
            )));
        }
        // Open once so an unreadable file is reported here instead of failing
        // halfway through a push.
        File::open(&path).map_err(|err| fs_error(format!("无法读取 {}", path.display()), err))?;
        return Ok(Some(SavScan {
            path,
            size: meta.len(),
            source,
        }));
    }
    Ok(None)
}

/// Error detail for a card without a battery save.
pub fn missing_sav_detail() -> &'static str {
    "本地没有电池存档"
}

pub fn missing_resume_error() -> LinkError {
    LinkError::new(ErrorKind::Fs, "本地没有 resume.state，请先玩一下再推送")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::unique_suffix;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("linkslot-unit-{tag}-{}", unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn unit() -> Unit {
        Unit::new("GBA", "mgba", "合金弹头").unwrap()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[test]
    fn validates_unit_identity() {
        assert!(Unit::new("GBA", "mgba", "合金弹头").is_ok());
        assert!(Unit::new("GB", "mgba", "a b").is_ok());
        assert!(Unit::new("gba", "mgba", "x").is_err());
        assert!(Unit::new("GBA", "gpSP", "x").is_err());
        assert!(Unit::new("GB", "gpsp", "x").is_err());
        assert!(Unit::new("GBA", "mgba", "").is_err());
        assert!(Unit::new("GBA", "mgba", "..").is_err());
        assert!(Unit::new("GBA", "mgba", "a/b").is_err());
        assert!(Unit::new("GBA", "mgba", "a\\b").is_err());
        assert!(Unit::new("GBA", "mgba", "a:b").is_err());
        assert!(Unit::new("GBA", "mgba", "trail.").is_err());
        assert!(Unit::new("GBA", "mgba", "trail ").is_err());
        assert!(Unit::new("GBA", "mgba", "line\nbreak").is_err());
        // A hashless unit is local-only: no cloud id, no cloud path. The game
        // name still names the local directory.
        assert_eq!(unit().id().unwrap_err().kind(), ErrorKind::Config);
        assert!(unit().remote_segments().is_empty());
        assert!(unit().try_remote_segments().is_err());
        assert_eq!(
            unit().local_dir(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/GBA/mgba/合金弹头")
        );
    }

    /// FC reaches the network through the same unit model as everything else,
    /// and only on the one core that can run it. The two refusals are what stop
    /// a hand-written `--emulator` from minting a unit the shelf can never show
    /// (`States/GBA/fceumm/`) or one nothing can load (`States/FC/mgba/`).
    #[test]
    fn the_famicom_syncs_on_fceumm_and_nothing_else() {
        let fc = Unit::new("FC", "fceumm", "超级马里奥").expect("FC on fceumm was refused");
        assert_eq!(
            fc.local_dir(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/FC/fceumm/超级马里奥")
        );
        assert_eq!(
            fc.local_sav_path(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/Saves/FC/超级马里奥.sav")
        );

        assert!(Unit::new("FC", "mgba", "x").is_err(), "mGBA cannot run FC");
        assert!(Unit::new("FC", "gpsp", "x").is_err(), "gpSP cannot run FC");
        assert!(
            Unit::new("GBA", "fceumm", "x").is_err(),
            "fceumm cannot run GBA"
        );
        assert!(Unit::new("GB", "fceumm", "x").is_err());
        // Case still matters: the directory on the card is `Games/FC/`.
        assert!(Unit::new("fc", "fceumm", "x").is_err());

        // The cloud side is the same `<platform>/<sha>/<kind>/<core>` layout as
        // every other machine — nothing about FC is special-cased there.
        let hashed = fc.with_rom_sha256(&digest_a()).unwrap();
        assert_eq!(hashed.remote_segments(), ["FC", digest_a().as_str()]);
        assert_eq!(hashed.id().unwrap(), format!("FC/fceumm/{}", digest_a()));
    }

    /// The Super Famicom reaches the network on supafaust only. It is the same
    /// unit model as the Famicom: `States/SFC/supafaust/<game>` and
    /// `Saves/SFC/<game>.sav`, and a hand-written core that cannot run it is
    /// refused before anything is read or written.
    #[test]
    fn the_super_famicom_syncs_on_supafaust_and_nothing_else() {
        let sfc = Unit::new("SFC", "supafaust", "超级马里奥世界")
            .expect("SFC on supafaust was refused");
        assert_eq!(
            sfc.local_dir(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/SFC/supafaust/超级马里奥世界")
        );
        assert_eq!(
            sfc.local_sav_path(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/Saves/SFC/超级马里奥世界.sav")
        );

        assert!(
            Unit::new("SFC", "fceumm", "x").is_err(),
            "fceumm cannot run SFC"
        );
        assert!(
            Unit::new("FC", "supafaust", "x").is_err(),
            "supafaust cannot run FC"
        );
        assert!(Unit::new("sfc", "supafaust", "x").is_err());

        let hashed = sfc.with_rom_sha256(&digest_a()).unwrap();
        assert_eq!(hashed.remote_segments(), ["SFC", digest_a().as_str()]);
        assert_eq!(hashed.id().unwrap(), format!("SFC/supafaust/{}", digest_a()));
    }

    /// Mega Drive reaches the network on genesis_plus_gx only.
    #[test]
    fn the_mega_drive_syncs_on_genesis_plus_gx_and_nothing_else() {
        let md = Unit::new("MD", "genesis_plus_gx", "Sonic")
            .expect("MD on genesis_plus_gx was refused");
        assert_eq!(
            md.local_dir(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/MD/genesis_plus_gx/Sonic")
        );
        assert_eq!(
            md.local_sav_path(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/Saves/MD/Sonic.sav")
        );
        assert!(
            Unit::new("MD", "supafaust", "x").is_err(),
            "supafaust cannot run MD"
        );
        assert!(
            Unit::new("FC", "genesis_plus_gx", "x").is_err(),
            "genesis_plus_gx cannot run FC"
        );
        assert!(Unit::new("md", "genesis_plus_gx", "x").is_err());

        let hashed = md.with_rom_sha256(&digest_a()).unwrap();
        assert_eq!(
            hashed.id().unwrap(),
            format!("MD/genesis_plus_gx/{}", digest_a())
        );
    }

    fn digest_a() -> String {
        "a".repeat(64)
    }

    #[test]
    fn rom_hash_replaces_the_cloud_identity_but_not_the_local_dir() {
        let hashed = unit().with_rom_sha256(&digest_a()).unwrap();
        assert_eq!(hashed.id().unwrap(), format!("GBA/mgba/{}", digest_a()));
        // The cloud *game* directory is <platform>/<sha>; the kind/scope
        // segments are added by `UnitSpec`.
        assert_eq!(hashed.remote_segments(), ["GBA", digest_a().as_str()]);
        // The card layout still follows the game name (rename keeps the dir).
        assert_eq!(
            hashed.local_dir(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/GBA/mgba/合金弹头")
        );
        assert_eq!(
            hashed.local_parent(Path::new("/mnt/sdcard")),
            PathBuf::from("/mnt/sdcard/States/GBA/mgba")
        );
        // `sha256:` prefixes and upper case are normalized to bare lowercase.
        let normalized = unit()
            .with_rom_sha256(&format!("sha256:{}", "A".repeat(64)))
            .unwrap();
        assert_eq!(normalized.rom_sha256.as_deref(), Some(digest_a().as_str()));
        assert_eq!(normalized, hashed);
    }

    #[test]
    fn with_rom_sha256_rejects_invalid_digests() {
        for bad in ["", "abc", &"z".repeat(64), &"a".repeat(63)] {
            let err = unit().with_rom_sha256(bad).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Config, "{bad:?}");
        }
    }

    #[test]
    fn scans_whitelist_and_ignores_the_rest() {
        let root = temp_root("scan");
        let dir = unit().local_dir(&root);
        write(&dir, "resume.state", b"resume");
        write(&dir, "2026-10-20_10-00-00.state", b"state1");
        write(&dir, "2026-10-20_10-00-00.png", b"png1");
        write(&dir, "2026-10-19_09-00-00.state", b"state0");
        // png without state: excluded from push whitelist
        write(&dir, "2026-10-18_08-00-00.png", b"png-orphan");
        write(&dir, "resume-refused-2026-10-20_09-58-11.state", b"old");
        write(&dir, ".resume.state.123.0.tmp", b"tmp");
        let backup_dir = dir.join(".linkslot-backup-abc");
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(backup_dir.join("resume.state"), b"bak").unwrap();
        write(&dir, "unknown.txt", b"unknown");
        write(&dir, "20261020-100000.state", b"bad-stamp");

        let scan = scan_dir(&dir).unwrap();
        let names: Vec<&str> = scan.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "2026-10-19_09-00-00.state",
                "2026-10-20_10-00-00.png",
                "2026-10-20_10-00-00.state",
                "resume.state",
            ]
        );
        assert!(scan.has_resume);
        assert_eq!(scan.ring_count, 2);

        let replaceable = local_replaceable_files(&dir).unwrap();
        let replaceable: Vec<&str> = replaceable.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            replaceable,
            [
                "2026-10-18_08-00-00.png",
                "2026-10-19_09-00-00.state",
                "2026-10-20_10-00-00.png",
                "2026-10-20_10-00-00.state",
                "resume.state",
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_dir_scans_empty() {
        let root = temp_root("missing");
        let scan = scan(&root, &unit()).unwrap();
        assert!(scan.is_empty());
        assert!(!scan.has_resume);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hash_scan_collects_hashes() {
        let root = temp_root("hash");
        let dir = unit().local_dir(&root);
        write(&dir, "resume.state", b"abc");
        let scan = scan_dir(&dir).unwrap();
        let hashed = hash_scan(&scan).unwrap();
        assert_eq!(hashed.len(), 1);
        assert_eq!(
            hashed[0].2,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn meta_names_are_whitelisted() {
        assert!(is_meta_file_name("resume.state"));
        assert!(is_meta_file_name("2026-10-20_10-00-00.state"));
        assert!(is_meta_file_name("2026-10-20_10-00-00.png"));
        assert!(!is_meta_file_name(
            "resume-refused-2026-10-20_10-00-00.state"
        ));
        assert!(!is_meta_file_name("meta.json"));
        assert!(!is_meta_file_name("../resume.state"));
        assert!(!is_meta_file_name("foo.state"));
    }

    #[test]
    fn stamp_extractors() {
        assert_eq!(
            state_stamp("2026-10-20_10-00-00.state"),
            Some("2026-10-20_10-00-00")
        );
        assert_eq!(state_stamp("resume.state"), None);
        assert_eq!(state_stamp("2026-10-20_10-00-00.png"), None);
        assert_eq!(
            png_stamp("2026-10-20_10-00-00.png"),
            Some("2026-10-20_10-00-00")
        );
        assert_eq!(png_stamp("foo.png"), None);
    }

    // ---- v0.0.10 unit model ---------------------------------------------

    fn spec_unit() -> Unit {
        unit().with_rom_sha256(&digest_a()).unwrap()
    }

    #[test]
    fn kind_roundtrips_through_json_and_display() {
        for kind in Kind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(serde_json::from_str::<Kind>(&json).unwrap(), kind);
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!(Kind::default(), Kind::State);
        assert_eq!(Kind::State.dir_name(), "state");
        assert_eq!(Kind::Sav.dir_name(), "sav");
        assert_eq!(Kind::parse("cheats"), None);
        let err = serde_json::from_str::<Kind>("\"cheats\"").unwrap_err();
        assert!(err.to_string().contains("state/sav"), "{err}");
    }

    #[test]
    fn scope_keeps_state_per_core_and_battery_core_free() {
        assert_eq!(Scope::Core("mgba".to_string()).core(), Some("mgba"));
        assert_eq!(Scope::None.core(), None);
        assert_eq!(Scope::Core("mgba".to_string()).segments(), ["mgba"]);
        assert!(Scope::None.segments().is_empty());
    }

    #[test]
    fn state_spec_paths_follow_the_v010_layout() {
        let unit = spec_unit();
        let root = Path::new("/mnt/sdcard");
        let spec = UnitSpec::state(&unit, root);
        assert_eq!(spec.kind, Kind::State);
        assert_eq!(spec.scope, Scope::Core("mgba".to_string()));
        assert_eq!(
            spec.local,
            LocalTarget::Dir(PathBuf::from("/mnt/sdcard/States/GBA/mgba/合金弹头"))
        );
        assert_eq!(spec.whitelist, Whitelist::StateRing);
        assert_eq!(spec.transfer, Transfer::MultiFileJournal);
        assert_eq!(spec.conflict_copy, ConflictCopy::Dir);
        assert!(spec.required_for_push);
        assert_eq!(
            spec.remote_segments(&unit).unwrap(),
            ["GBA", digest_a().as_str(), "state", "mgba"]
        );
        assert_eq!(
            spec.key(&unit).unwrap(),
            format!("state/GBA/mgba/{}", digest_a())
        );
    }

    #[test]
    fn sav_spec_paths_have_no_core_segment() {
        let unit = spec_unit();
        let root = Path::new("/mnt/sdcard");
        let spec = UnitSpec::sav(&unit, root);
        assert_eq!(spec.kind, Kind::Sav);
        assert_eq!(spec.scope, Scope::None);
        assert_eq!(
            spec.local,
            LocalTarget::File(PathBuf::from("/mnt/sdcard/Saves/GBA/合金弹头.sav"))
        );
        assert_eq!(spec.whitelist, Whitelist::SingleBattery);
        assert_eq!(spec.transfer, Transfer::SingleFileRename);
        assert_eq!(spec.conflict_copy, ConflictCopy::File);
        assert_eq!(
            spec.remote_segments(&unit).unwrap(),
            ["GBA", digest_a().as_str(), "sav"]
        );
        assert_eq!(
            spec.key(&unit).unwrap(),
            format!("sav/GBA/-/{}", digest_a())
        );
        // The read fallback is the same directory, `.srm` extension.
        assert_eq!(
            unit.local_srm_path(root),
            PathBuf::from("/mnt/sdcard/Saves/GBA/合金弹头.srm")
        );
        assert_eq!(spec.local.path(), unit.local_sav_path(root));
    }

    #[test]
    fn specs_never_share_paths_or_keys() {
        let unit = spec_unit();
        let root = Path::new("/mnt/sdcard");
        let state = UnitSpec::state(&unit, root);
        let sav = UnitSpec::sav(&unit, root);
        assert_ne!(
            state.remote_segments(&unit).unwrap(),
            sav.remote_segments(&unit).unwrap()
        );
        assert_ne!(state.key(&unit).unwrap(), sav.key(&unit).unwrap());
        // Same ROM under another game name: the cloud identity stays.
        let renamed = Unit::new("GBA", "mgba", "另一个名字")
            .unwrap()
            .with_rom_sha256(&digest_a())
            .unwrap();
        assert_eq!(
            state.remote_segments(&unit).unwrap(),
            state.remote_segments(&renamed).unwrap()
        );
        // A different ROM never maps onto the same unit.
        let other = unit.clone().with_rom_sha256(&"b".repeat(64)).unwrap();
        assert_ne!(state.key(&unit).unwrap(), state.key(&other).unwrap());
    }

    #[test]
    fn specs_require_a_rom_digest_instead_of_a_game_name() {
        let plain = unit();
        let root = Path::new("/mnt/sdcard");
        let spec = UnitSpec::state(&plain, root);
        let err = spec.remote_segments(&plain).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("SHA-256"), "{}", err.message());
        assert_eq!(spec.key(&plain).unwrap_err().kind(), ErrorKind::Config);
        // Local paths still work: a hashless unit is local-only.
        assert_eq!(
            plain.local_sav_path(root),
            root.join("Saves/GBA/合金弹头.sav")
        );
    }

    #[test]
    fn whitelists_never_claim_each_other() {
        assert!(Whitelist::StateRing.allows("resume.state"));
        assert!(Whitelist::StateRing.allows("2026-10-20_10-00-00.state"));
        assert!(Whitelist::StateRing.allows("2026-10-20_10-00-00.png"));
        assert!(!Whitelist::StateRing.allows("battery.sav"));
        assert!(Whitelist::SingleBattery.allows("battery.sav"));
        assert!(!Whitelist::SingleBattery.allows("resume.state"));
        assert!(!Whitelist::SingleBattery.allows("2026-10-20_10-00-00.sav"));
        assert!(!Whitelist::SingleBattery.allows("battery.sav.new"));
        assert_eq!(Whitelist::StateRing.required(), "resume.state");
        assert_eq!(Whitelist::SingleBattery.required(), "battery.sav");
    }

    #[test]
    fn core_validation_rejects_path_tricks() {
        for good in ["mgba", "gpsp", "gbaStation", "core_2"] {
            validate_core(good).unwrap();
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "core.1", "two words", "-lead"] {
            let err = validate_core(bad).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Config, "{bad:?}");
        }
        assert!(validate_core(&"c".repeat(CORE_MAX_BYTES + 1)).is_err());
    }

    // ---- layout.json -----------------------------------------------------

    #[test]
    fn layout_current_matches_the_planned_document() {
        let layout = Layout::current();
        assert_eq!(layout.layout, 1);
        assert_eq!(layout.app, "linkSlot");
        assert_eq!(layout.units, ["state", "sav"]);
        layout.validate().unwrap();

        let text = String::from_utf8(layout.to_bytes().unwrap()).unwrap();
        assert_eq!(
            text,
            "{\n  \"layout\": 1,\n  \"app\": \"linkSlot\",\n  \"units\": [\n    \"state\",\n    \"sav\"\n  ]\n}\n"
        );
        assert_eq!(Layout::parse(text.as_bytes()).unwrap(), layout);
    }

    #[test]
    fn layout_tolerates_extra_units_from_a_newer_client() {
        let mut layout = Layout::current();
        layout.units.push("cheats".to_string());
        layout.validate().unwrap();
        assert_eq!(Layout::parse(&layout.to_bytes().unwrap()).unwrap(), layout);
    }

    #[test]
    fn layout_rejects_other_versions_apps_and_missing_units() {
        let mut layout = Layout::current();
        layout.layout = 2;
        assert_eq!(layout.validate().unwrap_err().kind(), ErrorKind::Verify);
        assert!(layout
            .validate()
            .unwrap_err()
            .message()
            .contains("版本不支持"));

        let mut layout = Layout::current();
        layout.app = "otherApp".to_string();
        assert_eq!(layout.validate().unwrap_err().kind(), ErrorKind::Verify);
        assert!(layout
            .validate()
            .unwrap_err()
            .message()
            .contains("其它应用"));

        let mut layout = Layout::current();
        layout.units = vec!["state".to_string()];
        assert_eq!(layout.validate().unwrap_err().kind(), ErrorKind::Verify);
        assert!(layout.validate().unwrap_err().message().contains("sav"));
    }

    // ---- battery save scanning -------------------------------------------

    fn write_sav_file(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = root.join(SAVES_DIR).join("GBA");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn scan_sav_prefers_the_sav_file() {
        let root = temp_root("sav-prefer");
        let unit = spec_unit();
        let path = write_sav_file(&root, &format!("{}.sav", unit.game), b"SRAM-DATA");
        let scan = scan_sav(&root, &unit).unwrap().expect("battery save found");
        assert_eq!(scan.path, path);
        assert_eq!(scan.size, 9);
        assert_eq!(scan.source, SavSource::Sav);
        assert_eq!(scan.source.as_str(), "sav");
        assert_eq!(scan.remote_name(), BATTERY);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_falls_back_to_srm() {
        let root = temp_root("sav-srm");
        let unit = spec_unit();
        let path = write_sav_file(&root, &format!("{}.srm", unit.game), b"SRM");
        let scan = scan_sav(&root, &unit).unwrap().expect("srm fallback");
        assert_eq!(scan.path, path);
        assert_eq!(scan.source, SavSource::Srm);
        assert_eq!(scan.source.as_str(), "srm");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_prefers_sav_when_both_exist() {
        let root = temp_root("sav-both");
        let unit = spec_unit();
        write_sav_file(&root, &format!("{}.srm", unit.game), b"SRM");
        let sav = write_sav_file(&root, &format!("{}.sav", unit.game), b"SAV");
        let scan = scan_sav(&root, &unit).unwrap().expect("battery save found");
        assert_eq!(scan.path, sav);
        assert_eq!(scan.source, SavSource::Sav);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_accepts_an_empty_file() {
        let root = temp_root("sav-empty");
        let unit = spec_unit();
        write_sav_file(&root, &format!("{}.sav", unit.game), b"");
        let scan = scan_sav(&root, &unit)
            .unwrap()
            .expect("empty save is valid");
        assert_eq!(scan.size, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_missing_returns_none() {
        let root = temp_root("sav-missing");
        let unit = spec_unit();
        // No Saves directory at all.
        assert!(scan_sav(&root, &unit).unwrap().is_none());
        // A Saves directory with other games only.
        write_sav_file(&root, "Other Game.sav", b"other");
        assert!(scan_sav(&root, &unit).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_never_touches_other_games_files() {
        let root = temp_root("sav-only-target");
        let unit = spec_unit();
        let other_a = write_sav_file(&root, "Other A.sav", b"a");
        let other_b = write_sav_file(&root, "Other B.srm", b"b");
        let mine = write_sav_file(&root, &format!("{}.sav", unit.game), b"mine");
        let before: Vec<String> = {
            let mut names: Vec<String> = std::fs::read_dir(root.join(SAVES_DIR).join("GBA"))
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        };
        let scan = scan_sav(&root, &unit).unwrap().unwrap();
        assert_eq!(scan.path, mine);
        // Nothing else was read, moved, deleted or created.
        let after: Vec<String> = {
            let mut names: Vec<String> = std::fs::read_dir(root.join(SAVES_DIR).join("GBA"))
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            names
        };
        assert_eq!(before, after);
        assert!(other_a.exists() && other_b.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_sav_rejects_a_directory_instead_of_a_file() {
        let root = temp_root("sav-dir");
        let unit = spec_unit();
        let path = root
            .join(SAVES_DIR)
            .join("GBA")
            .join(format!("{}.sav", unit.game));
        std::fs::create_dir_all(&path).unwrap();
        let err = scan_sav(&root, &unit).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Fs);
        assert!(err.message().contains("不是普通文件"), "{}", err.message());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn layout_parse_reports_corrupt_json() {
        let err = Layout::parse(b"{not json").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Verify);
        assert!(err.message().contains("格式无效"), "{}", err.message());
        assert!(Layout::parse(br#"{"layout":1,"app":"linkSlot"}"#).is_err());
    }
}
