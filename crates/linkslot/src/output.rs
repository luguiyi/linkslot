//! Stdout/stderr contract and logging.
//!
//! The Slot frontend parses stdout only. Every command ends with a fixed-order
//! block of `LINKSLOT_*` key/value lines; `LINKSLOT_STAGE=` lines may appear
//! before it while work progresses. Human readable text goes to stderr, and a
//! copy is appended to `<root>/Logs/linkSlot.log`.
//!
//! Contract (project book v0.7 §4.4):
//! 1. `LINKSLOT_ACTION=sync|push|pull|status|net|config|mods`  (always)
//! 2. `LINKSLOT_RESULT=ok|noop|conflict|error`            (always)
//! 3. `LINKSLOT_DIRECTION=upload|download`                (only for `sync` transfers)
//! 4. `LINKSLOT_VERSION=<n>`                              (only when known)
//! 5. `LINKSLOT_ERROR=none|net|...|busy`                  (always)
//! 6. `LINKSLOT_DETAIL=<single line Chinese text>`        (always)
//! 7. `LINKSLOT_CLOUD_VERSION|CLOUD_DEVICE|CLOUD_UPDATED` (optional, conflict)
//! 8. `LINKSLOT_LOCAL_VERSION|LOCAL_DEVICE|LOCAL_UPDATED` (optional, conflict)
//! 9. `LINKSLOT_CONFLICT=<path or URL>`                   (only when one was made)
//!
//! `LINKSLOT_DIRECTION` is also emitted *early* on its own as soon as the
//! sync direction is decided (before any file transfer), so the Slot panel can
//! draw the cartridge↔cloud animation without waiting for the final block.
//!
//! `LINKSLOT_STAGE` values are exactly `net|connect|upload|download|verify|commit|mods`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::error::{ErrorKind, LinkError};
use crate::unit::Kind;
use crate::util;

pub const LOG_REL: &str = "linkSlot/Logs/linkSlot.log";
/// One-line log level shared by the CLI and the Slot Web settings page.
pub const LOG_LEVEL_REL: &str = "linkSlot/Config/log_level.txt";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    fn rank(level: &str) -> u8 {
        match level {
            "error" | "conflict" => 0,
            "warn" => 1,
            "info" | "progress" | "stage" | "direction" | "ok" => 2,
            "debug" => 3,
            _ => 2,
        }
    }
}

pub fn read_log_level(root: &Path) -> LogLevel {
    std::fs::read_to_string(root.join(LOG_LEVEL_REL))
        .ok()
        .and_then(|text| LogLevel::parse(&text))
        .unwrap_or(LogLevel::Info)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Net,
    Connect,
    Upload,
    Download,
    Verify,
    Commit,
    /// A `mods run` manual job (shelf long-press Y).
    Mods,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Net => "net",
            Stage::Connect => "connect",
            Stage::Upload => "upload",
            Stage::Download => "download",
            Stage::Verify => "verify",
            Stage::Commit => "commit",
            Stage::Mods => "mods",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Ok,
    Noop,
    /// Some units transferred, at least one failed (`sync` with two units).
    Partial,
    Conflict,
    /// A manual mod asked the user to choose; the device shows a modal and
    /// relaunches `mods run` with `--choice`.
    Choice,
    Error,
}

impl ResultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultKind::Ok => "ok",
            ResultKind::Noop => "noop",
            ResultKind::Partial => "partial",
            ResultKind::Conflict => "conflict",
            ResultKind::Choice => "choice",
            ResultKind::Error => "error",
        }
    }
}

/// Direction of a `sync` transfer (`upload` = local → cloud,
/// `download` = cloud → local), exposed to the Slot UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Upload => "upload",
            Direction::Download => "download",
        }
    }
}

/// Optional cloud/local version details attached to a conflict so the UI can
/// show "云端 v7 / 设备 / 时间" without extra requests. Fields that are
/// unknown are omitted from the stdout block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictInfo {
    pub cloud_version: Option<u64>,
    pub cloud_device: Option<String>,
    pub cloud_updated: Option<String>,
    pub local_version: Option<u64>,
    pub local_device: Option<String>,
    pub local_updated: Option<String>,
}

impl ConflictInfo {
    pub fn is_empty(&self) -> bool {
        self.cloud_version.is_none()
            && self.cloud_device.is_none()
            && self.cloud_updated.is_none()
            && self.local_version.is_none()
            && self.local_device.is_none()
            && self.local_updated.is_none()
    }
}

/// One side of a unit's version info (cloud or local), for the
/// `LINKSLOT_STATE_*` / `LINKSLOT_SAV_*` block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitSide {
    pub version: Option<u64>,
    pub device: Option<String>,
    pub updated: Option<String>,
    pub files: Option<usize>,
}

impl UnitSide {
    pub fn is_empty(&self) -> bool {
        self.version.is_none()
            && self.device.is_none()
            && self.updated.is_none()
            && self.files.is_none()
    }
}

/// Machine readable result of one sync unit (`state` / `sav`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitOutcome {
    pub kind: Kind,
    pub result: ResultKind,
    pub direction: Option<Direction>,
    pub version: Option<u64>,
    pub error: ErrorKind,
    pub cloud: Option<UnitSide>,
    pub local: Option<UnitSide>,
}

impl UnitOutcome {
    pub fn prefix(&self) -> &'static str {
        match self.kind {
            Kind::State => "STATE",
            Kind::Sav => "SAV",
        }
    }
}

/// Final machine readable outcome of one command.
#[derive(Clone, Debug)]
pub struct Outcome {
    action: &'static str,
    result: ResultKind,
    direction: Option<Direction>,
    version: Option<u64>,
    error: ErrorKind,
    detail: String,
    conflict: Option<PathBuf>,
    conflict_info: Option<ConflictInfo>,
    units: Vec<UnitOutcome>,
    choice: Option<slot_mods::ChoiceRequest>,
}

impl Outcome {
    pub fn ok(action: &'static str, version: Option<u64>, detail: impl Into<String>) -> Self {
        Outcome {
            action,
            result: ResultKind::Ok,
            direction: None,
            version,
            error: ErrorKind::None,
            detail: detail.into(),
            conflict: None,
            conflict_info: None,
            units: Vec::new(),
            choice: None,
        }
    }

    pub fn noop(action: &'static str, detail: impl Into<String>) -> Self {
        Outcome {
            action,
            result: ResultKind::Noop,
            direction: None,
            version: None,
            error: ErrorKind::None,
            detail: detail.into(),
            conflict: None,
            conflict_info: None,
            units: Vec::new(),
            choice: None,
        }
    }

    /// A partial sync: `error` is the failing unit's category (it decides the
    /// exit code); the successful units keep what they transferred.
    pub fn partial(action: &'static str, error: ErrorKind, detail: impl Into<String>) -> Self {
        Outcome {
            action,
            result: ResultKind::Partial,
            direction: None,
            version: None,
            error,
            detail: detail.into(),
            conflict: None,
            conflict_info: None,
            units: Vec::new(),
            choice: None,
        }
    }

    pub fn conflict(
        action: &'static str,
        kind: ErrorKind,
        detail: impl Into<String>,
        conflict: Option<PathBuf>,
    ) -> Self {
        Outcome {
            action,
            result: ResultKind::Conflict,
            direction: None,
            version: None,
            error: kind,
            detail: detail.into(),
            conflict,
            conflict_info: None,
            units: Vec::new(),
            choice: None,
        }
    }

    pub fn error(
        action: &'static str,
        err: &LinkError,
        version: Option<u64>,
        conflict: Option<PathBuf>,
    ) -> Self {
        let kind = err.kind();
        let result = if kind.is_conflict() {
            ResultKind::Conflict
        } else {
            ResultKind::Error
        };
        Outcome {
            action,
            result,
            direction: None,
            version,
            error: kind,
            detail: err.detail(),
            conflict,
            conflict_info: None,
            units: Vec::new(),
            choice: None,
        }
    }

    /// A manual mod needs the user to choose one option before continuing.
    pub fn choice(
        action: &'static str,
        request: slot_mods::ChoiceRequest,
        detail: impl Into<String>,
    ) -> Self {
        Outcome {
            action,
            result: ResultKind::Choice,
            direction: None,
            version: None,
            error: ErrorKind::None,
            detail: detail.into(),
            conflict: None,
            conflict_info: None,
            units: Vec::new(),
            choice: Some(request),
        }
    }

    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    pub fn with_conflict(mut self, path: Option<PathBuf>) -> Self {
        self.conflict = path;
        self
    }

    pub fn with_conflict_info(mut self, info: ConflictInfo) -> Self {
        if !info.is_empty() {
            self.conflict_info = Some(info);
        }
        self
    }

    /// Attach the per-unit results (`LINKSLOT_STATE_*` / `LINKSLOT_SAV_*`).
    pub fn with_units(mut self, units: Vec<UnitOutcome>) -> Self {
        self.units = units;
        self
    }

    pub fn units(&self) -> &[UnitOutcome] {
        &self.units
    }

    pub fn result(&self) -> ResultKind {
        self.result
    }

    pub fn direction(&self) -> Option<Direction> {
        self.direction
    }

    pub fn version(&self) -> Option<u64> {
        self.version
    }

    pub fn conflict_path(&self) -> Option<&Path> {
        self.conflict.as_deref()
    }

    pub fn conflict_info(&self) -> Option<&ConflictInfo> {
        self.conflict_info.as_ref()
    }

    pub fn error_kind(&self) -> ErrorKind {
        self.error
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn choice_request(&self) -> Option<&slot_mods::ChoiceRequest> {
        self.choice.as_ref()
    }

    pub fn exit_code(&self) -> i32 {
        match self.result {
            ResultKind::Ok | ResultKind::Noop | ResultKind::Choice => 0,
            ResultKind::Conflict => self.error.exit_code().max(2),
            // partial: the failing unit's category decides (net=1, config=3,
            // fs/verify/busy=4).
            ResultKind::Partial | ResultKind::Error => self.error.exit_code().max(1),
        }
    }

    /// The final `LINKSLOT_*` block, in the fixed order of the stdout contract.
    pub fn print(&self, out: &mut dyn Write) -> io::Result<()> {
        write_line(out, "LINKSLOT_ACTION", self.action)?;
        write_line(out, "LINKSLOT_RESULT", self.result.as_str())?;
        if let Some(direction) = self.direction {
            write_line(out, "LINKSLOT_DIRECTION", direction.as_str())?;
        }
        if let Some(version) = self.version {
            write_line(out, "LINKSLOT_VERSION", &version.to_string())?;
        }
        write_line(out, "LINKSLOT_ERROR", self.error.as_str())?;
        write_line(out, "LINKSLOT_DETAIL", &one_line(&self.detail))?;
        if let Some(choice) = &self.choice {
            let json = serde_json::to_string(choice)
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
            write_line(out, "LINKSLOT_CHOICE", &json)?;
        }
        if let Some(info) = &self.conflict_info {
            if let Some(version) = info.cloud_version {
                write_line(out, "LINKSLOT_CLOUD_VERSION", &version.to_string())?;
            }
            if let Some(device) = &info.cloud_device {
                write_line(out, "LINKSLOT_CLOUD_DEVICE", device)?;
            }
            if let Some(updated) = &info.cloud_updated {
                write_line(out, "LINKSLOT_CLOUD_UPDATED", updated)?;
            }
            if let Some(version) = info.local_version {
                write_line(out, "LINKSLOT_LOCAL_VERSION", &version.to_string())?;
            }
            if let Some(device) = &info.local_device {
                write_line(out, "LINKSLOT_LOCAL_DEVICE", device)?;
            }
            if let Some(updated) = &info.local_updated {
                write_line(out, "LINKSLOT_LOCAL_UPDATED", updated)?;
            }
        }
        if let Some(path) = &self.conflict {
            write_line(out, "LINKSLOT_CONFLICT", &path.to_string_lossy())?;
        }
        for unit in &self.units {
            let prefix = unit.prefix();
            write_line(
                out,
                &format!("LINKSLOT_{prefix}_RESULT"),
                unit.result.as_str(),
            )?;
            if let Some(direction) = unit.direction {
                write_line(
                    out,
                    &format!("LINKSLOT_{prefix}_DIRECTION"),
                    direction.as_str(),
                )?;
            }
            if let Some(version) = unit.version {
                write_line(
                    out,
                    &format!("LINKSLOT_{prefix}_VERSION"),
                    &version.to_string(),
                )?;
            }
            if matches!(unit.result, ResultKind::Conflict | ResultKind::Error) {
                write_line(
                    out,
                    &format!("LINKSLOT_{prefix}_ERROR"),
                    unit.error.as_str(),
                )?;
            }
            if let Some(side) = &unit.cloud {
                write_side(out, prefix, "CLOUD", side)?;
            }
            if let Some(side) = &unit.local {
                write_side(out, prefix, "LOCAL", side)?;
            }
        }
        for unit in &self.units {
            if unit.result == ResultKind::Conflict {
                write_line(out, "LINKSLOT_CONFLICT_UNIT", unit.kind.as_str())?;
            }
        }
        out.flush()
    }

    /// Short human readable summary for stderr.
    pub fn summary(&self) -> String {
        match self.result {
            ResultKind::Ok => match self.version {
                Some(v) => format!("{}（版本 {v}）", self.detail),
                None => self.detail.clone(),
            },
            ResultKind::Noop
            | ResultKind::Partial
            | ResultKind::Conflict
            | ResultKind::Choice
            | ResultKind::Error => self.detail.clone(),
        }
    }
}

fn write_line(out: &mut dyn Write, key: &str, value: &str) -> io::Result<()> {
    writeln!(out, "{key}={value}")
}

/// Emit one unit side's optional fields (`LINKSLOT_{P}_{SIDE}_VERSION` etc.).
fn write_side(out: &mut dyn Write, prefix: &str, side: &str, info: &UnitSide) -> io::Result<()> {
    if let Some(version) = info.version {
        write_line(
            out,
            &format!("LINKSLOT_{prefix}_{side}_VERSION"),
            &version.to_string(),
        )?;
    }
    if let Some(device) = &info.device {
        write_line(out, &format!("LINKSLOT_{prefix}_{side}_DEVICE"), device)?;
    }
    if let Some(updated) = &info.updated {
        write_line(out, &format!("LINKSLOT_{prefix}_{side}_UPDATED"), updated)?;
    }
    if let Some(files) = info.files {
        write_line(
            out,
            &format!("LINKSLOT_{prefix}_{side}_FILES"),
            &files.to_string(),
        )?;
    }
    Ok(())
}

/// Collapse a message to a single printable line (the contract forbids
/// newlines inside `LINKSLOT_*` values).
pub fn one_line(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        cleaned.push(ch);
    }
    cleaned.trim().to_string()
}

/// Writes stage lines and human readable diagnostics, and appends to the log.
pub struct Reporter<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    verbose: bool,
    level: LogLevel,
    log: Option<File>,
    log_path: PathBuf,
    last_stage: Option<Stage>,
}

impl<'a> Reporter<'a> {
    pub fn new(root: &Path, verbose: bool, out: &'a mut dyn Write, err: &'a mut dyn Write) -> Self {
        let log_path = root.join(LOG_REL);
        let log = open_log(&log_path);
        Reporter {
            out,
            err,
            verbose,
            level: read_log_level(root),
            log,
            log_path,
            last_stage: None,
        }
    }

    pub fn verbose(&self) -> bool {
        self.verbose
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Emit `LINKSLOT_STAGE=<stage>` when the stage changes; flushed at once so
    /// the Slot panel can poll it line by line.
    ///
    /// A broken stdout pipe must not abort a sync (the frontend may have
    /// cancelled); write errors are logged and ignored.
    pub fn stage(&mut self, stage: Stage) {
        if self.last_stage == Some(stage) {
            return;
        }
        self.last_stage = Some(stage);
        if writeln!(self.out, "LINKSLOT_STAGE={}", stage.as_str()).is_ok() {
            let _ = self.out.flush();
        }
        self.log("stage", stage.as_str());
    }

    /// Emit an early `LINKSLOT_DIRECTION=` line as soon as `sync` has decided
    /// where the data will flow. This happens before the first byte is
    /// transferred so the Slot animation can react immediately; the final
    /// block repeats the same key.
    ///
    /// Write errors are intentionally ignored (cancelled frontend).
    pub fn direction(&mut self, direction: Direction) {
        if writeln!(self.out, "LINKSLOT_DIRECTION={}", direction.as_str()).is_ok() {
            let _ = self.out.flush();
        }
        self.log("direction", direction.as_str());
    }

    /// Write one raw stdout line (used by `mods list --json` for machine
    /// readable output). It is intentionally not echoed to the log.
    pub fn raw_stdout(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    /// Human readable progress (shown regardless of `--verbose`, stderr only).
    pub fn progress(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        let _ = writeln!(self.err, "{message}");
        let _ = self.err.flush();
        self.log("progress", message);
    }

    /// Detail lines: stderr only with `--verbose`, written when the log level
    /// is `info` or `debug`.
    pub fn info(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.verbose {
            let _ = writeln!(self.err, "[linkSlot] {message}");
            let _ = self.err.flush();
        }
        self.log("info", message);
    }

    /// Verbose diagnostic lines, written only at the `debug` level.
    pub fn debug(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.verbose {
            let _ = writeln!(self.err, "[linkSlot:debug] {message}");
            let _ = self.err.flush();
        }
        self.log("debug", message);
    }

    /// Warning: always stderr (short, actionable) and the log.
    pub fn warn(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        let _ = writeln!(self.err, "linkSlot: 警告：{message}");
        let _ = self.err.flush();
        self.log("warn", message);
    }

    /// Final human readable summary line.
    pub fn summary(&mut self, outcome: &Outcome) {
        let line = format!("linkSlot: {}", outcome.summary());
        let _ = writeln!(self.err, "{line}");
        let _ = self.err.flush();
        self.log(
            match outcome.result() {
                ResultKind::Error => "error",
                ResultKind::Conflict => "conflict",
                _ => "ok",
            },
            &format!(
                "{} result={} error={} {}",
                outcome.action,
                outcome.result().as_str(),
                outcome.error_kind().as_str(),
                line
            ),
        );
    }

    /// Emit the final `LINKSLOT_*` block (must be the last thing on stdout)
    /// plus the human summary on stderr.
    pub fn finish(&mut self, outcome: &Outcome) {
        if let Err(err) = outcome.print(&mut *self.out) {
            self.log("error", &format!("写 stdout 失败：{err}"));
        }
        self.summary(outcome);
    }

    pub fn log(&mut self, level: &str, message: &str) {
        if LogLevel::rank(level) > LogLevel::rank(self.level.as_str()) {
            return;
        }
        let Some(file) = self.log.as_mut() else {
            return;
        };
        let _ = writeln!(
            file,
            "{} [{level}] {}",
            util::now_iso8601(),
            one_line(message)
        );
        let _ = file.flush();
    }
}

fn open_log(path: &Path) -> Option<File> {
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(outcome: &Outcome) -> String {
        let mut buf: Vec<u8> = Vec::new();
        outcome.print(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn prints_fixed_block_order() {
        let info = ConflictInfo {
            cloud_version: Some(8),
            cloud_device: Some("rg34xx-other".to_string()),
            cloud_updated: Some("2026-10-20T10:00:00Z".to_string()),
            local_version: Some(7),
            local_device: Some("rg34xx-test".to_string()),
            local_updated: Some("2026-10-19T09:00:00Z".to_string()),
        };
        let outcome = Outcome::ok("sync", Some(9), "推送成功")
            .with_direction(Direction::Upload)
            .with_conflict(Some(PathBuf::from("/mnt/sdcard/States/x.conflict")))
            .with_conflict_info(info);
        let text = render(&outcome);
        let keys: Vec<&str> = text
            .lines()
            .map(|line| line.split('=').next().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "LINKSLOT_ACTION",
                "LINKSLOT_RESULT",
                "LINKSLOT_DIRECTION",
                "LINKSLOT_VERSION",
                "LINKSLOT_ERROR",
                "LINKSLOT_DETAIL",
                "LINKSLOT_CLOUD_VERSION",
                "LINKSLOT_CLOUD_DEVICE",
                "LINKSLOT_CLOUD_UPDATED",
                "LINKSLOT_LOCAL_VERSION",
                "LINKSLOT_LOCAL_DEVICE",
                "LINKSLOT_LOCAL_UPDATED",
                "LINKSLOT_CONFLICT"
            ]
        );
        assert!(text.contains("LINKSLOT_RESULT=ok"));
        assert!(text.contains("LINKSLOT_ERROR=none"));
        assert!(text.contains("LINKSLOT_DIRECTION=upload"));
    }

    #[test]
    fn optional_conflict_keys_are_omitted_when_unknown() {
        let info = ConflictInfo {
            cloud_version: Some(2),
            local_device: Some("rg34xx-test".to_string()),
            ..ConflictInfo::default()
        };
        let outcome = Outcome::conflict("sync", ErrorKind::NoCache, "无法判断", None)
            .with_conflict_info(info);
        let text = render(&outcome);
        assert!(text.contains("LINKSLOT_CLOUD_VERSION=2"));
        assert!(text.contains("LINKSLOT_LOCAL_DEVICE=rg34xx-test"));
        assert!(!text.contains("LINKSLOT_CLOUD_DEVICE"));
        assert!(!text.contains("LINKSLOT_CLOUD_UPDATED"));
        assert!(!text.contains("LINKSLOT_LOCAL_VERSION"));
        assert!(!text.contains("LINKSLOT_LOCAL_UPDATED"));
        assert!(!text.contains("LINKSLOT_DIRECTION"));
        assert!(!text.contains("LINKSLOT_CONFLICT="));
    }

    #[test]
    fn empty_conflict_info_is_not_printed() {
        let outcome = Outcome::conflict("sync", ErrorKind::Forked, "分叉", None)
            .with_conflict_info(ConflictInfo::default());
        assert!(outcome.conflict_info().is_none());
        assert!(!render(&outcome).contains("LINKSLOT_CLOUD_"));
    }

    #[test]
    fn omits_version_and_conflict_when_unknown() {
        let outcome = Outcome::noop("pull", "已是最新");
        let text = render(&outcome);
        assert!(!text.contains("LINKSLOT_VERSION"));
        assert!(!text.contains("LINKSLOT_CONFLICT"));
        assert!(text.contains("LINKSLOT_RESULT=noop"));
        assert!(text.contains("LINKSLOT_ERROR=none"));
    }

    #[test]
    fn conflict_outcome_uses_conflict_exit_code() {
        let outcome = Outcome::conflict("push", ErrorKind::RemoteNewer, "云端更新", None);
        assert_eq!(outcome.exit_code(), 2);
        assert_eq!(outcome.result(), ResultKind::Conflict);
        let text = render(&outcome);
        assert!(text.contains("LINKSLOT_ERROR=remote-newer"));
    }

    #[test]
    fn prints_per_unit_keys_and_conflict_units() {
        let side = |version| UnitSide {
            version: Some(version),
            device: Some("rg34xx-a".to_string()),
            updated: Some("2026-10-22T21:00:00Z".to_string()),
            files: Some(4),
        };
        let outcome = Outcome::ok("sync", None, "已同步：即时 v2 / 电池 v3").with_units(vec![
            UnitOutcome {
                kind: Kind::State,
                result: ResultKind::Ok,
                direction: Some(Direction::Upload),
                version: Some(2),
                error: ErrorKind::None,
                cloud: Some(side(2)),
                local: Some(side(2)),
            },
            UnitOutcome {
                kind: Kind::Sav,
                result: ResultKind::Conflict,
                direction: None,
                version: None,
                error: ErrorKind::Forked,
                cloud: Some(side(3)),
                local: Some(side(3)),
            },
        ]);
        let text = render(&outcome);
        assert!(text.contains("LINKSLOT_STATE_RESULT=ok"));
        assert!(text.contains("LINKSLOT_STATE_DIRECTION=upload"));
        assert!(text.contains("LINKSLOT_STATE_VERSION=2"));
        assert!(text.contains("LINKSLOT_STATE_CLOUD_FILES=4"));
        assert!(text.contains("LINKSLOT_SAV_RESULT=conflict"));
        assert!(text.contains("LINKSLOT_SAV_ERROR=forked"));
        assert!(text.contains("LINKSLOT_SAV_CLOUD_VERSION=3"));
        assert!(text.contains("LINKSLOT_CONFLICT_UNIT=sav"));
        assert!(!text.contains("LINKSLOT_CONFLICT_UNIT=state"));
        // Per-unit blocks come after the legacy block.
        assert!(
            text.find("LINKSLOT_STATE_RESULT").unwrap() > text.find("LINKSLOT_DETAIL").unwrap()
        );
    }

    #[test]
    fn partial_result_uses_the_failing_category_exit_code() {
        let outcome = Outcome::partial("sync", ErrorKind::Net, "即时存档已同步，电池存档失败");
        assert_eq!(outcome.result(), ResultKind::Partial);
        assert_eq!(outcome.exit_code(), 1);
        let text = render(&outcome);
        assert!(text.contains("LINKSLOT_RESULT=partial"));
        assert!(text.contains("LINKSLOT_ERROR=net"));

        let outcome = Outcome::partial("sync", ErrorKind::Fs, "即时存档已同步，电池存档失败");
        assert_eq!(outcome.exit_code(), 4);
    }

    #[test]
    fn detail_is_single_line() {
        let outcome = Outcome::ok("push", None, "第一行\n第二行\r\n第三行");
        let text = render(&outcome);
        let detail = text
            .lines()
            .find(|line| line.starts_with("LINKSLOT_DETAIL="))
            .unwrap();
        assert_eq!(detail, "LINKSLOT_DETAIL=第一行 第二行 第三行");
    }

    #[test]
    fn reporter_emits_early_direction_before_stage() {
        let root = std::env::temp_dir().join(format!("linkslot-out-{}", util::unique_suffix()));
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        {
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            rep.direction(Direction::Download);
            rep.stage(Stage::Download);
        }
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            "LINKSLOT_DIRECTION=download\nLINKSLOT_STAGE=download\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reporter_emits_stage_only_on_change() {
        let root = std::env::temp_dir().join(format!("linkslot-out-{}", util::unique_suffix()));
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        {
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            rep.stage(Stage::Net);
            rep.stage(Stage::Net);
            rep.stage(Stage::Connect);
            rep.info("细节");
            rep.progress("正在连接网络");
        }
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out, "LINKSLOT_STAGE=net\nLINKSLOT_STAGE=connect\n");
        let err = String::from_utf8(err).unwrap();
        assert!(err.contains("正在连接网络"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn log_level_filters_debug_lines_but_keeps_warnings() {
        let root = std::env::temp_dir().join(format!("linkslot-out-{}", util::unique_suffix()));
        std::fs::create_dir_all(root.join("linkSlot/Config")).unwrap();
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        {
            let mut rep = Reporter::new(&root, false, &mut out, &mut err);
            rep.info("普通信息");
            rep.debug("wifi debug 细节");
            rep.warn("警告保留");
        }
        let text = std::fs::read_to_string(root.join(LOG_REL)).unwrap();
        assert!(text.contains("[info] 普通信息"));
        assert!(!text.contains("wifi debug 细节"));
        assert!(text.contains("[warn] 警告保留"));

        std::fs::write(root.join(LOG_LEVEL_REL), "debug\n").unwrap();
        {
            let mut rep = Reporter::new(&root, false, &mut out, &mut err);
            rep.debug("wifi debug 细节");
        }
        let text = std::fs::read_to_string(root.join(LOG_REL)).unwrap();
        assert!(text.contains("[debug] wifi debug 细节"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_log_level_falls_back_to_info() {
        let root = std::env::temp_dir().join(format!("linkslot-out-{}", util::unique_suffix()));
        std::fs::create_dir_all(root.join("linkSlot/Config")).unwrap();
        std::fs::write(root.join(LOG_LEVEL_REL), "verbose-please\n").unwrap();
        assert_eq!(read_log_level(&root), LogLevel::Info);
        let _ = std::fs::remove_dir_all(&root);
    }
}
