//! Lua mod hooks.
//!
//! Without the `mods` feature this module is a no-op: the device build enables
//! it explicitly, while host builds stay free of the Lua C toolchain.  The
//! `slot-mods` crate owns the manifest/config format and is always available.

use std::path::Path;
use std::time::Duration;
#[cfg(feature = "mods")]
use std::time::Instant;

use crate::config::Config;
use crate::error::ErrorKind;
use crate::output::{Direction, Reporter, ResultKind};
use crate::unit::{Kind, Unit};

pub use slot_mods::Timing;

#[cfg(feature = "mods")]
mod host_files;
#[cfg(feature = "mods")]
mod host_http;
#[cfg(feature = "mods")]
mod host_webdav;
#[cfg(feature = "mods")]
mod runtime;

/// Per-script wall clock cap for background hooks.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
/// Manual actions may wait for a remote AI answer, so they get a longer cap
/// that still fits inside the device's 120 s sync job window.
pub const MANUAL_SCRIPT_TIMEOUT: Duration = Duration::from_secs(90);
/// Whole hook batch cap; the sync keeps its existing 120 s budget.
pub const TOTAL_HOOK_BUDGET: Duration = Duration::from_secs(30);
/// Lua heap cap.
pub const LUA_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
/// Local file read/write caps.
pub const MAX_LOCAL_READ: usize = 64 * 1024 * 1024;
pub const MAX_LOCAL_WRITE: usize = 32 * 1024 * 1024;
pub const MAX_LOCAL_ENTRIES: usize = 1000;
/// Remote request/byte caps per script. `after_sync` is a batch-oriented hook
/// over the whole card, so it gets a larger, still bounded budget.
pub const MAX_REMOTE_REQUESTS: usize = 16;
pub const MAX_REMOTE_TOTAL: usize = 64 * 1024 * 1024;
pub const SYNC_MAX_REMOTE_REQUESTS: usize = 64;
pub const SYNC_MAX_REMOTE_TOTAL: usize = 128 * 1024 * 1024;
pub const MAX_REMOTE_GET: usize = 16 * 1024 * 1024;
pub const MAX_REMOTE_PUT: usize = 32 * 1024 * 1024;
/// Scoped mod HTTP bridge (`http.*`).  One manual sync needs about eight
/// requests; the cap leaves room for a retry while keeping a broken script
/// from flooding the LAN.
pub const MAX_HTTP_REQUESTS: usize = 16;
pub const MAX_HTTP_TOTAL: usize = 64 * 1024 * 1024;
pub const MAX_HTTP_READ: usize = 16 * 1024 * 1024;
pub const MAX_HTTP_WRITE: usize = 32 * 1024 * 1024;
pub const MAX_HTTP_URL: usize = 2048;
pub const MAX_HTTP_HEADERS: usize = 16;
pub const MAX_HTTP_HEADER_BYTES: usize = 8192;
pub const MAX_MOD_CONFIG: usize = slot_mods::MAX_TEXT_FILE_BYTES;
/// WebDAV PROPFIND response cap.
pub const MAX_XML_BYTES: usize = 2 * 1024 * 1024;
/// Per-script log line cap.
pub const MAX_LOG_LINES: usize = 200;
/// Maximum executed scripts; the store itself allows more.
pub const MAX_EXECUTED_ACTIONS: usize = slot_mods::MAX_TOTAL_ACTIONS;

/// Result/version information available to `after_*` hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferInfo {
    pub direction: Direction,
    pub result: ResultKind,
    pub version: Option<u64>,
}

/// Everything a Lua mod may read about the current transfer or sync.
pub struct HookContext<'a> {
    pub root: &'a Path,
    pub config: &'a Config,
    /// `None` for `after_sync`, which runs once for the whole sync.
    pub unit: Option<&'a Unit>,
    pub kind: Option<Kind>,
    pub timing: Timing,
    /// Present for `after_*` transfer hooks.
    pub transfer: Option<TransferInfo>,
    /// Id of the active sync method (`default` for the legacy config).
    pub active_storage: &'a str,
    pub device_id: &'a str,
    /// Choices already made for this manual batch (`--choice key=value`).
    pub choices: &'a [(String, String)],
}

/// How many scripts ran in one hook batch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookReport {
    pub ran: usize,
    pub failed: usize,
}

/// Status a manual action reports through `mods.report`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualStatus {
    Ok,
    Noop,
    /// The action asked the user to choose; the device shows a modal.
    Choice,
    Error,
}

impl ManualStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ManualStatus::Ok => "ok",
            ManualStatus::Noop => "noop",
            ManualStatus::Choice => "choice",
            ManualStatus::Error => "error",
        }
    }
}

/// One `mods.report` call from a manual action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualReportValue {
    pub status: ManualStatus,
    pub direction: Option<Direction>,
    pub error: Option<ErrorKind>,
    pub detail: Option<String>,
}

/// Aggregated result of one `linkSlot mods run` batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualSummary {
    pub ran: usize,
    pub failed: usize,
    pub status: ManualStatus,
    pub direction: Option<Direction>,
    pub error: ErrorKind,
    pub detail: String,
    pub choice: Option<slot_mods::ChoiceRequest>,
}

impl Default for ManualSummary {
    fn default() -> Self {
        ManualSummary {
            ran: 0,
            failed: 0,
            status: ManualStatus::Noop,
            direction: None,
            error: ErrorKind::None,
            detail: String::new(),
            choice: None,
        }
    }
}

impl ManualSummary {
    fn finish(mut self) -> ManualSummary {
        if self.failed > 0 {
            self.status = ManualStatus::Error;
        }
        if self.detail.trim().is_empty() {
            self.detail = match self.status {
                ManualStatus::Ok => "手动 mod 执行完成".to_string(),
                ManualStatus::Noop => "没有启用的手动 mod 动作".to_string(),
                ManualStatus::Choice => "需要用户选择".to_string(),
                ManualStatus::Error => "手动 mod 执行失败".to_string(),
            };
        }
        if self.status == ManualStatus::Ok && self.error != ErrorKind::None {
            self.error = ErrorKind::None;
        }
        self
    }
}

/// Read one mod's optional `config.json` (128 KiB text cap).
pub fn read_mod_config(
    root: &Path,
    mod_id: &str,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let detail = match slot_mods::read_text_file(root, mod_id, "config.json") {
        Ok(detail) => detail,
        Err(slot_mods::ModError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if detail.content.trim().is_empty() {
        return Ok(None);
    }
    if detail.content.len() > MAX_MOD_CONFIG {
        return Err(format!("config.json 超过 {} KiB", MAX_MOD_CONFIG / 1024));
    }
    serde_json::from_str(&detail.content)
        .map(Some)
        .map_err(|error| format!("config.json 解析失败：{error}"))
}

/// The endpoint declared by the mod that owns the shelf long-press.
///
/// Used by `mods run` to probe the actual peer (GBAStation) instead of the
/// configured WebDAV host. Only the winner is considered: probing a mod that
/// is about to be skipped would drag the radio up for nothing, and a winner
/// that talks WebDAV (SaveDeck) has no `base_url` at all — which must mean
/// "no peer to probe", not "ask the next mod down".
pub fn manual_endpoints(root: &Path) -> Vec<(String, crate::webdav::Endpoint)> {
    let Ok(entries) = slot_mods::list(root) else {
        return Vec::new();
    };
    let mut winner = None;
    for entry in entries {
        if !entry.enabled {
            continue;
        }
        let Ok(detail) = slot_mods::read_mod(root, &entry.id) else {
            continue;
        };
        let has_manual = detail.actions.iter().any(|action| {
            action.enabled && action.timing == Some(Timing::Manual) && action.error.is_none()
        });
        if has_manual {
            winner = Some(entry);
            break;
        }
    }
    let Some(entry) = winner else {
        return Vec::new();
    };
    let Ok(Some(config)) = read_mod_config(root, &entry.id) else {
        return Vec::new();
    };
    let Some(base_url) = config
        .get("base_url")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Vec::new();
    };
    match crate::webdav::Endpoint::parse(base_url) {
        Ok(endpoint) => vec![(entry.id.clone(), endpoint)],
        Err(_) => Vec::new(),
    }
}

/// A snapshot of enabled mods loaded once at the start of a sync.
pub struct ModSet {
    #[cfg(feature = "mods")]
    actions: Vec<LoadedAction>,
    #[cfg(feature = "mods")]
    remote_profiles: Vec<RemoteProfile>,
    #[cfg(not(feature = "mods"))]
    _empty: (),
}

#[cfg(feature = "mods")]
pub(crate) mod zip;

#[cfg(feature = "mods")]
struct LoadedAction {
    mod_id: String,
    mod_name: String,
    action_id: String,
    action_name: String,
    /// The owning mod's priority, which decides whose `manual` action the
    /// shelf long-press runs. Lower runs first.
    mod_order: i32,
    timing: Timing,
    order: i32,
    source: String,
}

#[cfg(feature = "mods")]
#[derive(Clone)]
pub(crate) struct RemoteProfile {
    pub id: String,
    pub name: String,
    pub remote_root: String,
    pub endpoint: crate::webdav::Endpoint,
    pub username: String,
    pub password: String,
    pub ca_cert: String,
    pub insecure: bool,
    pub timeout_secs: u64,
}

impl ModSet {
    pub fn empty() -> ModSet {
        ModSet {
            #[cfg(feature = "mods")]
            actions: Vec::new(),
            #[cfg(feature = "mods")]
            remote_profiles: Vec::new(),
            #[cfg(not(feature = "mods"))]
            _empty: (),
        }
    }

    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "mods")]
        {
            self.actions.is_empty()
        }
        #[cfg(not(feature = "mods"))]
        {
            true
        }
    }
}

/// Load the enabled actions and the per-profile compatibility roots once.
#[cfg(feature = "mods")]
pub fn load_mod_set(root: &Path, config: &Config, rep: &mut Reporter) -> ModSet {
    let mut actions = Vec::new();
    match slot_mods::list(root) {
        Ok(entries) => {
            for entry in entries {
                if !entry.enabled {
                    continue;
                }
                if let Some(error) = &entry.error {
                    // An invalid action is reported here too, but its siblings
                    // may still be valid and should run.
                    rep.warn(format!("mod {} 有无效内容：{error}", entry.id));
                }
                let detail = match slot_mods::read_mod(root, &entry.id) {
                    Ok(detail) => detail,
                    Err(error) => {
                        rep.warn(format!("无法读取 mod {}：{error}", entry.id));
                        continue;
                    }
                };
                for action in detail.actions {
                    if !action.enabled {
                        continue;
                    }
                    let Some(timing) = action.timing else {
                        rep.warn(format!(
                            "mod {}/动作 {} 已跳过：timing 无效",
                            entry.id, action.id
                        ));
                        continue;
                    };
                    if let Some(error) = &action.error {
                        rep.warn(format!(
                            "mod {}/动作 {} 已跳过：{error}",
                            entry.id, action.id
                        ));
                        continue;
                    }
                    let action_detail = match slot_mods::read_action(root, &entry.id, &action.id) {
                        Ok(detail) => detail,
                        Err(error) => {
                            rep.warn(format!(
                                "无法读取 mod {}/动作 {}：{error}",
                                entry.id, action.id
                            ));
                            continue;
                        }
                    };
                    if let Some(error) = &action_detail.entry.error {
                        rep.warn(format!(
                            "mod {}/动作 {} 已跳过：{error}",
                            entry.id, action.id
                        ));
                        continue;
                    }
                    actions.push(LoadedAction {
                        mod_id: entry.id.clone(),
                        mod_name: entry.name.clone(),
                        action_id: action.id.clone(),
                        action_name: action.name.clone(),
                        mod_order: entry.order,
                        timing,
                        order: action.order,
                        source: action_detail.content,
                    });
                }
            }
        }
        Err(error) => rep.warn(format!("无法读取 mods 目录：{error}")),
    }
    // The mod's own priority comes first, then the action's: a lower `order`
    // runs (and wins) earlier, and the device's mod list shows the same queue.
    // So moving a mod to the top of that list also moves it in front of the
    // shelf long-press, which is the only thing that decides whose manual
    // action runs when two mods both ask for it.
    actions.sort_by(|left, right| {
        left.mod_order
            .cmp(&right.mod_order)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| {
                left.mod_name
                    .to_lowercase()
                    .cmp(&right.mod_name.to_lowercase())
            })
            .then_with(|| left.mod_id.cmp(&right.mod_id))
            .then_with(|| {
                left.action_name
                    .to_lowercase()
                    .cmp(&right.action_name.to_lowercase())
            })
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    if actions.len() > MAX_EXECUTED_ACTIONS {
        rep.warn(format!(
            "启用的动作超过 {MAX_EXECUTED_ACTIONS} 个，仅执行排序靠前的部分"
        ));
        actions.truncate(MAX_EXECUTED_ACTIONS);
    }

    let remote_profiles = load_remote_profiles(root, config, rep);
    ModSet {
        actions,
        remote_profiles,
    }
}

#[cfg(feature = "mods")]
fn load_remote_profiles(_root: &Path, config: &Config, rep: &mut Reporter) -> Vec<RemoteProfile> {
    let timeout_secs = config.sync_cfg().timeout_secs.clamp(1, 10);
    let mut profiles = Vec::new();
    for storage in config.all_storages() {
        if storage.kind != "webdav" || storage.url.trim().is_empty() {
            continue;
        }
        let endpoint = match crate::webdav::Endpoint::parse(&storage.url) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                rep.warn(format!(
                    "同步方式「{}」的地址无效，远程 mods API 已跳过：{}",
                    storage.id,
                    error.message()
                ));
                continue;
            }
        };
        profiles.push(RemoteProfile {
            id: storage.id,
            name: if storage.name.trim().is_empty() {
                "WebDAV".to_string()
            } else {
                storage.name
            },
            remote_root: storage.remote_root,
            endpoint,
            username: storage.username,
            password: storage.password,
            ca_cert: storage.ca_cert,
            insecure: storage.insecure,
            timeout_secs,
        });
    }
    profiles
}

/// Run every enabled action for `ctx.timing`; no-op without the `mods` feature.
#[cfg(feature = "mods")]
pub fn run_hooks(set: &ModSet, ctx: &HookContext<'_>, rep: &mut Reporter) -> HookReport {
    let mut report = HookReport::default();
    let start = Instant::now();
    for loaded in &set.actions {
        if loaded.timing != ctx.timing {
            continue;
        }
        if start.elapsed() >= TOTAL_HOOK_BUDGET {
            rep.warn("mods 总执行时间超过 30 秒，剩余动作已跳过");
            break;
        }
        report.ran += 1;
        match runtime::run_one(loaded, ctx, &set.remote_profiles, rep) {
            Ok(runtime::ScriptEnd::Done(_)) => {}
            Ok(runtime::ScriptEnd::Choice(_)) => {
                report.failed += 1;
                rep.warn(format!(
                    "mod {}/动作 {} 在后台同步中请求了用户选择，已跳过",
                    loaded.mod_id, loaded.action_id
                ));
            }
            Err(error) => {
                report.failed += 1;
                rep.warn(format!(
                    "mod {}/动作 {} 在 {} 执行失败：{}",
                    loaded.mod_id,
                    loaded.action_id,
                    ctx.timing.label(),
                    error
                ));
            }
        }
    }
    report
}

/// Run one enabled `after_boot` action (`linkSlot mods boot`).
///
/// The device scheduler selects the action and delay; this function only
/// verifies the exact target and supplies the no-unit hook context.
#[cfg(feature = "mods")]
pub fn run_boot(
    root: &Path,
    config: &Config,
    mod_id: &str,
    action_id: &str,
    device_id: &str,
    rep: &mut Reporter,
) -> Result<HookReport, String> {
    let set = load_mod_set(root, config, rep);
    let loaded = set
        .actions
        .iter()
        .find(|action| action.mod_id == mod_id && action.action_id == action_id)
        .ok_or_else(|| format!("未找到 {mod_id}/{action_id}"))?;
    if loaded.timing != Timing::AfterBoot {
        return Err(format!(
            "{mod_id}/{action_id} 不是 after_boot 动作（当前 {}）",
            loaded.timing.label()
        ));
    }
    let active_storage = config.active_storage_id();
    let ctx = HookContext {
        root,
        config,
        unit: None,
        kind: None,
        timing: Timing::AfterBoot,
        transfer: None,
        active_storage: &active_storage,
        device_id,
        choices: &[],
    };
    let mut report = HookReport::default();
    report.ran = 1;
    match runtime::run_one(loaded, &ctx, &set.remote_profiles, rep) {
        Ok(runtime::ScriptEnd::Done(_)) => {}
        Ok(runtime::ScriptEnd::Choice(_)) => {
            report.failed = 1;
            rep.warn(format!(
                "mod {mod_id}/动作 {action_id} 在开机后执行中请求了用户选择，已跳过"
            ));
        }
        Err(error) => {
            report.failed = 1;
            rep.warn(format!(
                "mod {mod_id}/动作 {action_id} 在开机后执行失败：{error}"
            ));
        }
    }
    Ok(report)
}

/// Run every enabled `manual` action for one game (`linkSlot mods run`).
///
/// `mod_id`/`action_id` narrow the run to one action (the long-press Y picker);
/// without them the highest-priority manual action wins as before.
#[cfg(feature = "mods")]
pub fn run_manual(
    root: &Path,
    config: &Config,
    unit: &Unit,
    device_id: &str,
    choices: &[(String, String)],
    mod_id: Option<&str>,
    action_id: Option<&str>,
    rep: &mut Reporter,
) -> ManualSummary {
    let set = load_mod_set(root, config, rep);
    let active_storage = config.active_storage_id();
    let ctx = HookContext {
        root,
        config,
        unit: Some(unit),
        kind: Some(Kind::Sav),
        timing: Timing::Manual,
        transfer: None,
        active_storage: &active_storage,
        device_id,
        choices,
    };
    run_manual_hooks(&set, &ctx, mod_id, action_id, rep)
}

#[cfg(feature = "mods")]
fn run_manual_hooks(
    set: &ModSet,
    ctx: &HookContext<'_>,
    mod_id: Option<&str>,
    action_id: Option<&str>,
    rep: &mut Reporter,
) -> ManualSummary {
    let mut queue = set.actions.iter().filter(|a| {
        a.timing == Timing::Manual
            && mod_id.is_none_or(|id| a.mod_id == id)
            && action_id.is_none_or(|id| a.action_id == id)
    });
    let Some(loaded) = queue.next() else {
        if mod_id.is_some() || action_id.is_some() {
            rep.warn(format!(
                "没有找到已启用的手动动作：mod={mod_id:?} action={action_id:?}"
            ));
        }
        return ManualSummary::default().finish();
    };
    // One winner takes the long-press.
    //
    // Two bridges that both ask for Y would otherwise run back to back against
    // the same save file: the second would decide against a file the first had
    // just rewritten, and the player would see two transfers and one banner.
    // Priority (`mod.json` `order`, lower first) picks the one that runs; the
    // rest are named in the log so a skipped mod is not a silent mystery.
    for skipped in queue {
        rep.warn(format!(
            "mod「{}」的动作「{}」优先级低于「{}」，长按 Y 已跳过；\
             如需它生效，请在 mod 设置里把它排到前面",
            skipped.mod_name, skipped.action_name, loaded.mod_name
        ));
    }
    let mut summary = ManualSummary::default();
    summary.ran += 1;
    match runtime::run_one(loaded, ctx, &set.remote_profiles, rep) {
        Ok(runtime::ScriptEnd::Done(Some(report))) => {
            if let Some(detail) = report.detail {
                summary.detail = detail;
            }
            if report.direction.is_some() {
                summary.direction = report.direction;
            }
            match report.status {
                ManualStatus::Error => {
                    summary.status = ManualStatus::Error;
                    summary.error = report.error.unwrap_or(ErrorKind::Server);
                }
                ManualStatus::Ok => summary.status = ManualStatus::Ok,
                ManualStatus::Noop | ManualStatus::Choice => {}
            }
        }
        Ok(runtime::ScriptEnd::Done(None)) => summary.status = ManualStatus::Ok,
        Ok(runtime::ScriptEnd::Choice(request)) => {
            summary.status = ManualStatus::Choice;
            summary.choice = Some(request);
        }
        Err(error) => {
            summary.failed += 1;
            summary.status = ManualStatus::Error;
            summary.error = classify_manual_error(&error);
            summary.detail = error.clone();
            rep.warn(format!(
                "mod {}/动作 {} 在 {} 执行失败：{}",
                loaded.mod_id,
                loaded.action_id,
                ctx.timing.label(),
                error
            ));
        }
    }
    summary.finish()
}

#[cfg(feature = "mods")]
fn classify_manual_error(error: &str) -> ErrorKind {
    if error.contains("超时") {
        ErrorKind::Timeout
    } else if error.contains("配置") || error.contains("config.json") {
        ErrorKind::Config
    } else if error.contains("连接")
        || error.contains("网络")
        || error.contains("[net]")
        || error.contains("HTTP")
    {
        ErrorKind::Net
    } else {
        ErrorKind::Server
    }
}

#[cfg(not(feature = "mods"))]
pub fn run_manual(
    _root: &Path,
    _config: &Config,
    _unit: &Unit,
    _device_id: &str,
    _choices: &[(String, String)],
    _mod_id: Option<&str>,
    _action_id: Option<&str>,
    _rep: &mut Reporter,
) -> ManualSummary {
    ManualSummary {
        ran: 0,
        failed: 1,
        status: ManualStatus::Error,
        direction: None,
        error: ErrorKind::Config,
        detail: "本构建不支持 mods（mods feature）".to_string(),
        choice: None,
    }
    .finish()
}

#[cfg(not(feature = "mods"))]
pub fn run_boot(
    _root: &Path,
    _config: &Config,
    _mod_id: &str,
    _action_id: &str,
    _device_id: &str,
    _rep: &mut Reporter,
) -> Result<HookReport, String> {
    Err("本构建不支持 mods（mods feature）".to_string())
}

#[cfg(not(feature = "mods"))]
pub fn load_mod_set(_root: &Path, _config: &Config, _rep: &mut Reporter) -> ModSet {
    ModSet::empty()
}

/// Compile a script without executing it (`linkSlot mods check`).
#[cfg(feature = "mods")]
pub fn check_source(source: &str, id: &str) -> Result<(), String> {
    runtime::check_source(source, id)
}

#[cfg(not(feature = "mods"))]
pub fn check_source(_source: &str, _id: &str) -> Result<(), String> {
    Err("本构建不支持 mods（mods feature）".to_string())
}

#[cfg(not(feature = "mods"))]
pub fn run_hooks(_set: &ModSet, _ctx: &HookContext<'_>, _rep: &mut Reporter) -> HookReport {
    HookReport::default()
}

#[cfg(feature = "mods")]
#[allow(dead_code)]
fn _assert_loaded_action_fields(
    loaded: &LoadedAction,
) -> (&str, &str, &str, &str, Timing, i32, &str) {
    (
        &loaded.mod_id,
        &loaded.mod_name,
        &loaded.action_id,
        &loaded.action_name,
        loaded.timing,
        loaded.order,
        &loaded.source,
    )
}
