//! `/api/mods/**`: the Lua script store and mod management.
//!
//! The actual storage/validation lives in the pure `slot-mods` crate; this
//! module only decodes HTTP input, guards writes and maps [`ModError`] onto the
//! page's existing JSON error scheme.  Passwords never pass through here:
//! `linkcfg` exposes only `password_set`, and the CLI reads credentials from
//! the card itself.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use slot_mods::{
    ActionDetail, ActionEdit, ActionEntry, ModDetail, ModEdit, ModEntry, ModError, ModFileDetail,
    ModFileEntry, Timing,
};

use crate::linkcfg::StorageProfile;
use crate::web::http::{self, Request, Response};
use crate::web::util;
use crate::web::Shared;

/// `linkSlot mods check` only compiles one file; keep it bounded.
const CHECK_TIMEOUT_SECS: u64 = 20;
/// `linkSlot mods test` issues a PROPFIND against the sync profile's root.
const TEST_TIMEOUT_SECS: u64 = 30;
const MODS_LOG_TAIL_LINES: usize = 120;
const MODS_LOG_TAIL_BYTES: u64 = 128 * 1024;
/// CLI output is truncated in the JSON reply; the firmware log keeps all.
const CLI_OUTPUT_CHARS: usize = 4000;

/// `GET /api/mods`: script list, public sync profiles and the cached `mods=1`
/// runtime flag.  Read-only.
pub(crate) fn list(shared: &Shared) -> Response {
    let entries = match slot_mods::list(&shared.root) {
        Ok(entries) => entries,
        Err(error) => return mod_error(error),
    };
    let config = crate::linkcfg::read(&shared.root);
    let storages: Vec<Value> = config
        .storages
        .iter()
        .map(|profile| storage_value(profile, profile.id == config.active_storage))
        .collect();
    let mods: Vec<Value> = entries.iter().map(mod_entry_value).collect();
    let timings: Vec<Value> = Timing::ALL
        .iter()
        .map(|timing| json!({ "value": timing.as_str(), "label": timing.label() }))
        .collect();
    let supported = runtime_supported(shared);
    let log_path = shared.root.join("linkSlot/Logs").join("linkSlot.log");
    let log_tail = util::tail_lines(&log_path, MODS_LOG_TAIL_LINES, MODS_LOG_TAIL_BYTES);
    Response::json(
        200,
        json!({
            "mods": mods,
            "timings": timings,
            "storages": storages,
            "active_storage": &config.active_storage,
            "runtime_supported": supported,
            "runtime_hint": runtime_hint(supported),
            "max_script_bytes": slot_mods::MAX_SCRIPT_BYTES,
            "max_mods": slot_mods::MAX_MODS,
            "log_tail": log_tail,
        }),
    )
}

/// `GET /api/mods/files?id=`: read-only tree for the built-in detail page.
pub(crate) fn files(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let id = required_query(request, "id")?;
        let entries = slot_mods::list_files(&shared.root, &id).map_err(mod_error)?;
        Ok(Response::json(
            200,
            json!({
                "id": id,
                "has_ui": slot_mods::has_web_ui(&shared.root, &id),
                "files": entries.iter().map(mod_file_value).collect::<Vec<_>>(),
                "max_files": slot_mods::MAX_LISTED_FILES,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/mods/file?id=&path=`: text bridge used by mod-owned pages.
pub(crate) fn file_item(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let id = required_query(request, "id")?;
        let path = required_query(request, "path")?;
        let detail = slot_mods::read_text_file(&shared.root, &id, &path).map_err(mod_error)?;
        Ok(Response::json(200, mod_file_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/file`: write a text file scoped to one mod.
pub(crate) fn save_file(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if request
            .header("content-type")
            .is_some_and(|kind| kind.starts_with("image/png"))
        {
            let id = required_query(request, "id")?;
            let path = required_query(request, "path")?;
            if request.body.len() > slot_mods::MAX_STICKER_BYTES {
                return Err(Response::error(413, "mod 贴纸超过 4 MiB 限制"));
            }
            let detail = slot_mods::save_sticker_file(&shared.root, &id, &path, &request.body)
                .map_err(mod_error)?;
            return Ok(Response::json(200, mod_file_detail_value(&detail)));
        }
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let path = required_string(&value, "path", "文件路径")?;
        let content = value
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| Response::error(400, "缺少 content 字符串"))?;
        let revision = http::json_string(&value, "revision")
            .map(|revision| revision.trim().to_string())
            .filter(|revision| !revision.is_empty());
        let detail =
            slot_mods::save_text_file(&shared.root, &id, &path, content, revision.as_deref())
                .map_err(mod_error)?;
        Ok(Response::json(200, mod_file_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/mods/item?id=`: mod metadata + action summaries.
pub(crate) fn item(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let id = required_query(request, "id")?;
        let detail = slot_mods::read_mod(&shared.root, &id).map_err(mod_error)?;
        Ok(Response::json(200, mod_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/item`: create or update one mod (name/version/enabled).
pub(crate) fn save_item(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let name = http::json_string(&value, "name").unwrap_or_default();
        let version = http::json_string(&value, "version")
            .unwrap_or_else(|| slot_mods::DEFAULT_MOD_VERSION.to_string());
        let id = http::json_string(&value, "id")
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        let revision = http::json_string(&value, "revision")
            .map(|revision| revision.trim().to_string())
            .filter(|revision| !revision.is_empty());
        let author = http::json_string(&value, "author").unwrap_or_default();
        let detail = slot_mods::save_mod(
            &shared.root,
            ModEdit {
                id,
                name,
                version,
                author,
                enabled: http::json_bool(&value, "enabled", false),
            allow_create: http::json_bool(&value, "allow_create", false),
                revision,
            },
        )
        .map_err(mod_error)?;
        Ok(Response::json(200, mod_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/mods/action?id=&action=`: full action source + revision.
pub(crate) fn action_item(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let id = required_query(request, "id")?;
        let action = required_query(request, "action")?;
        let detail = slot_mods::read_action(&shared.root, &id, &action).map_err(mod_error)?;
        Ok(Response::json(200, action_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/action`: create or update one action.
pub(crate) fn save_action(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let mod_id = required_string(&value, "id", "mod id")?;
        let action_id = http::json_string(&value, "action_id")
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        let timing_text = http::json_string(&value, "timing").unwrap_or_default();
        let timing = Timing::parse(&timing_text).ok_or_else(|| {
            Response::error(
                400,
                "执行时机必须是 before_upload / after_upload / before_download / after_download / after_sync / after_boot / manual",
            )
        })?;
        let boot_delay_minutes = match json_boot_delay(&value)? {
            Some(minutes) => minutes,
            None => action_id
                .as_ref()
                .and_then(|id| slot_mods::read_action(&shared.root, &mod_id, id).ok())
                .map(|detail| detail.entry.boot_delay_minutes)
                .unwrap_or(0),
        };
        let content = http::json_string(&value, "content").unwrap_or_default();
        if content.len() > slot_mods::MAX_SCRIPT_BYTES {
            return Err(Response::error(
                400,
                format!(
                    "动作内容超过 {} KiB 限制",
                    slot_mods::MAX_SCRIPT_BYTES / 1024
                ),
            ));
        }
        let revision = http::json_string(&value, "revision")
            .map(|revision| revision.trim().to_string())
            .filter(|revision| !revision.is_empty());
        let ui = match value.get("ui") {
            Some(value) if !value.is_null() => Some(
                serde_json::from_value::<slot_mods::ActionUi>(value.clone())
                    .map_err(|error| Response::error(400, format!("ui 无效：{error}")))?,
            ),
            _ => None,
        };
        let capabilities = match value.get("capabilities") {
            Some(value) if !value.is_null() => {
                let list = value.as_array().ok_or_else(|| {
                    Response::error(400, "capabilities 必须是字符串数组")
                })?;
                let mut parsed = Vec::with_capacity(list.len());
                for item in list {
                    let item = item.as_str().ok_or_else(|| {
                        Response::error(400, "capabilities 的每一项必须是字符串")
                    })?;
                    parsed.push(item.to_string());
                }
                Some(parsed)
            }
            _ => None,
        };
        let detail = slot_mods::save_action(
            &shared.root,
            ActionEdit {
                mod_id,
                action_id,
                name: http::json_string(&value, "name").unwrap_or_default(),
                timing,
                enabled: http::json_bool(&value, "enabled", true),
                order: json_order(&value)?,
                boot_delay_minutes,
                content,
                ui,
                capabilities,
                revision,
            },
        )
        .map_err(mod_error)?;
        Ok(Response::json(200, action_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/toggle`: flip only the `enabled` flag.
pub(crate) fn toggle(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| Response::error(400, "缺少 enabled 布尔值"))?;
        let detail = slot_mods::toggle_mod(&shared.root, &id, enabled).map_err(mod_error)?;
        Ok(Response::json(200, mod_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/action/toggle`: flip one action's `enabled` flag.
pub(crate) fn toggle_action(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let action = required_string(&value, "action", "action id")?;
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| Response::error(400, "缺少 enabled 布尔值"))?;
        let detail =
            slot_mods::toggle_action(&shared.root, &id, &action, enabled).map_err(mod_error)?;
        Ok(Response::json(200, action_detail_value(&detail)))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/action/delete`: move one action to the mod's trash.
pub(crate) fn delete_action(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let action = required_string(&value, "action", "action id")?;
        let trash = slot_mods::delete_action(&shared.root, &id, &action).map_err(mod_error)?;
        let shown = trash
            .strip_prefix(&shared.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| trash.to_string_lossy().into_owned());
        Ok(Response::json(200, json!({ "ok": true, "trash": shown })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/delete`: move one mod to `mods/.trash/`.
pub(crate) fn delete(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let trash = slot_mods::delete_mod(&shared.root, &id).map_err(mod_error)?;
        let shown = trash
            .strip_prefix(&shared.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| trash.to_string_lossy().into_owned());
        Ok(Response::json(200, json!({ "ok": true, "trash": shown })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/uninstall`: permanently remove a mod and all of its actions.
pub(crate) fn uninstall(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let removed = slot_mods::uninstall_mod(&shared.root, &id).map_err(mod_error)?;
        let shown = removed
            .strip_prefix(&shared.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| removed.to_string_lossy().into_owned());
        Ok(Response::json(200, json!({ "ok": true, "removed": shown })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/test`: `linkSlot mods test --profile <id>`.
pub(crate) fn test(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let profile_id = http::json_string(&value, "profile")
            .or_else(|| http::json_string(&value, "id"))
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Response::error(400, "缺少 profile 参数"))?;
        let config = crate::linkcfg::read(&shared.root);
        if !config
            .storages
            .iter()
            .any(|profile| profile.id == profile_id)
        {
            return Err(Response::error(
                400,
                format!("未知的同步方式 id「{profile_id}」"),
            ));
        }
        let supported = runtime_supported(shared);
        let args = vec![
            "mods".to_string(),
            "test".to_string(),
            "--profile".to_string(),
            profile_id,
        ];
        Ok(cli_result(
            "test",
            run_mods_cli(shared, &args, Duration::from_secs(TEST_TIMEOUT_SECS)),
            supported,
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/mods/check`: compile one action, or every action of a mod.
pub(crate) fn check(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        let value = json_request(request)?;
        let id = required_string(&value, "id", "mod id")?;
        let action = http::json_string(&value, "action")
            .map(|action| action.trim().to_string())
            .filter(|action| !action.is_empty());
        match &action {
            Some(action) => {
                slot_mods::read_action(&shared.root, &id, action).map_err(mod_error)?;
            }
            None => {
                slot_mods::read_mod(&shared.root, &id).map_err(mod_error)?;
            }
        }
        let supported = runtime_supported(shared);
        let mut args = vec![
            "mods".to_string(),
            "check".to_string(),
            "--id".to_string(),
            id,
        ];
        if let Some(action) = action {
            args.push("--action".to_string());
            args.push(action);
        }
        Ok(cli_result(
            "check",
            run_mods_cli(shared, &args, Duration::from_secs(CHECK_TIMEOUT_SECS)),
            supported,
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

// ------------------------------------------------------------------ helpers

fn mod_error(error: ModError) -> Response {
    match error {
        ModError::Invalid(message) | ModError::TooLarge(message) => Response::error(400, message),
        ModError::NotFound(message) => Response::error(404, message),
        ModError::Conflict(message) => Response::error(409, message),
        ModError::Io(message) => Response::error(500, message),
    }
}

fn mod_entry_value(entry: &ModEntry) -> Value {
    json!({
        "id": &entry.id,
        "name": &entry.name,
        "version": &entry.version,
        "author": author_value(&entry.author),
        "enabled": entry.enabled,
        "action_count": entry.action_count,
        "legacy_actions": entry.legacy_actions,
        "has_ui": entry.has_ui,
        "category": &entry.category,
        "error": &entry.error,
        "updated_at": entry.updated_at,
    })
}

fn author_value(author: &str) -> &str {
    let author = author.trim();
    if author.is_empty() {
        "无作者信息"
    } else {
        author
    }
}

fn mod_file_value(entry: &ModFileEntry) -> Value {
    json!({
        "path": &entry.path,
        "is_dir": entry.is_dir,
        "size": entry.size,
        "editable": entry.editable,
        "managed_by": &entry.managed_by,
    })
}

fn mod_file_detail_value(detail: &ModFileDetail) -> Value {
    json!({
        "path": &detail.path,
        "content": &detail.content,
        "revision": &detail.revision,
        "editable": detail.editable,
        "size": detail.size,
    })
}

fn action_entry_value(entry: &ActionEntry) -> Value {
    let (timing, timing_label) = match entry.timing {
        Some(timing) => (json!(timing.as_str()), json!(timing.label())),
        None => (Value::Null, Value::Null),
    };
    json!({
        "id": &entry.id,
        "name": &entry.name,
        "timing": timing,
        "timing_label": timing_label,
        "enabled": entry.enabled,
        "order": entry.order,
        "boot_delay_minutes": entry.boot_delay_minutes,
        "capabilities": &entry.capabilities,
        "capabilities_declared": entry.capabilities_declared,
        "legacy_capabilities": !entry.capabilities_declared,
        "error": &entry.error,
        "script_bytes": entry.script_bytes,
        "updated_at": entry.updated_at,
    })
}

fn mod_detail_value(detail: &ModDetail) -> Value {
    let mut value = mod_entry_value(&detail.entry);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "actions".to_string(),
            Value::Array(detail.actions.iter().map(action_entry_value).collect()),
        );
        object.insert("revision".to_string(), json!(&detail.revision));
        object.insert("meta_present".to_string(), json!(detail.meta_present));
    }
    value
}

fn action_detail_value(detail: &ActionDetail) -> Value {
    let mut value = action_entry_value(&detail.entry);
    if let Some(object) = value.as_object_mut() {
        object.insert("content".to_string(), json!(&detail.content));
        object.insert("revision".to_string(), json!(&detail.revision));
        object.insert("meta_present".to_string(), json!(detail.meta_present));
        object.insert("script_present".to_string(), json!(detail.script_present));
    }
    value
}

/// Public sync-method fields only: the page never receives credentials,
/// usernames or certificate paths.
fn storage_value(profile: &StorageProfile, active: bool) -> Value {
    json!({
        "id": &profile.id,
        "name": &profile.name,
        "kind": &profile.kind,
        "url": &profile.url,
        "remote_root": &profile.remote_root,
        "active": active,
        "password_set": profile.password_set,
    })
}

fn runtime_hint(supported: bool) -> Value {
    if supported {
        Value::Null
    } else {
        json!(
            "设备上的 linkSlot CLI 尚未报告 mods=1：同步时不会执行 mods，语法检查不可用。请升级设备端 linkSlot 以执行 mods。"
        )
    }
}

fn required_query(request: &Request, name: &str) -> Result<String, Response> {
    match http::query_param(request, name) {
        Ok(Some(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Ok(_) => Err(Response::error(400, format!("缺少 {name} 参数"))),
        Err(error) => Err(Response::error(error.status, error.message)),
    }
}

pub(crate) fn json_request(request: &Request) -> Result<Value, Response> {
    http::json_body(request).map_err(|error| Response::error(error.status, error.message))
}

fn json_boot_delay(value: &Value) -> Result<Option<u32>, Response> {
    let Some(field) = value.get("boot_delay_minutes") else {
        return Ok(None);
    };
    let Some(minutes) = field.as_u64() else {
        return Err(Response::error(400, "boot_delay_minutes 必须是非负整数"));
    };
    if minutes > slot_mods::MAX_BOOT_DELAY_MINUTES as u64 {
        return Err(Response::error(
            400,
            format!(
                "boot_delay_minutes 必须在 0..={} 之间",
                slot_mods::MAX_BOOT_DELAY_MINUTES
            ),
        ));
    }
    Ok(Some(minutes as u32))
}

fn required_string(value: &Value, key: &str, label: &str) -> Result<String, Response> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Response::error(400, format!("缺少{label}")))
}

fn json_order(value: &Value) -> Result<i32, Response> {
    match value.get("order") {
        None | Some(Value::Null) => Ok(slot_mods::DEFAULT_ORDER),
        Some(field) => match field.as_i64() {
            Some(order)
                if (slot_mods::MIN_ORDER as i64..=slot_mods::MAX_ORDER as i64).contains(&order) =>
            {
                Ok(order as i32)
            }
            Some(_) => Err(Response::error(
                400,
                format!(
                    "执行顺序必须在 {}..={} 之间",
                    slot_mods::MIN_ORDER,
                    slot_mods::MAX_ORDER
                ),
            )),
            None => Err(Response::error(400, "执行顺序必须是整数")),
        },
    }
}

/// Cached `mods=1` marker from `linkSlot version`.
fn runtime_supported(shared: &Shared) -> bool {
    *shared
        .mods_supported
        .get_or_init(|| read_mods_flag(&shared.root))
}

fn read_mods_flag(root: &Path) -> bool {
    let executable = root.join(crate::root::SYSTEM_BIN).join("linkslot_sync");
    let mut direct = Command::new(&executable);
    direct.arg("version");
    if let Ok(output) = util::run_with_timeout(direct, Duration::from_secs(2)) {
        if has_mods_flag(&output.combined()) {
            return true;
        }
    }
    // FAT32 cards cannot mark files executable: retry through /bin/sh.
    #[cfg(unix)]
    {
        let mut shell = Command::new("/bin/sh");
        shell.arg(&executable).arg("version");
        if let Ok(output) = util::run_with_timeout(shell, Duration::from_secs(2)) {
            if has_mods_flag(&output.combined()) {
                return true;
            }
        }
    }
    false
}

fn has_mods_flag(text: &str) -> bool {
    text.split_whitespace().any(|token| token == "mods=1")
}

/// `/bin/sh <root>/System/linkSlot/shell/linkslot.sh mods ...` with `SLOT_ROOT` pointing at
/// the card, exactly like the other Web-triggered CLI calls.
fn run_mods_cli(
    shared: &Shared,
    args: &[String],
    timeout: Duration,
) -> Result<util::RunOutput, String> {
    let mut command = Command::new("/bin/sh");
    command
        .arg(shared.root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh"))
        .args(args)
        .env("SLOT_ROOT", &shared.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    util::run_with_timeout(command, timeout)
}

fn cli_result(action: &str, result: Result<util::RunOutput, String>, supported: bool) -> Response {
    let label = if action == "check" {
        "语法检查"
    } else {
        "连接测试"
    };
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            return Response::json(
                200,
                json!({
                    "ok": false,
                    "code": Value::Null,
                    "timed_out": false,
                    "detail": util::truncate_chars(&error, 500),
                    "hint": format!(
                        "{label}无法启动 linkSlot CLI（System/linkSlot/shell/linkslot.sh）：请确认脚本存在且可执行；旧版 linkSlot 需升级后才支持 mods 子命令。"
                    ),
                    "output": "",
                    "runtime_supported": supported,
                }),
            )
        }
    };
    let timed_out = output.timed_out;
    let code = output.code;
    let text = output.combined();
    let detail = cli_detail(&output);
    let ok = !timed_out && code == Some(0);
    let hint = if ok {
        if action == "check" {
            "语法检查通过：mod 可以加载。".to_string()
        } else {
            "兼容存档目录连接正常。".to_string()
        }
    } else if timed_out {
        format!("{label}超时，已终止。")
    } else if looks_unsupported(&text, supported) {
        "当前 linkSlot CLI 可能不支持 mods 功能（缺少 mods 子命令）：请升级设备端 linkSlot 后再试。"
            .to_string()
    } else {
        match code {
            Some(code) => format!(
                "{label}失败（退出码 {code}）：{}",
                detail
                    .clone()
                    .unwrap_or_else(|| "请查看输出详情。".to_string())
            ),
            None => format!("{label}被意外终止，请查看输出详情。"),
        }
    };
    Response::json(
        200,
        json!({
            "ok": ok,
            "code": code,
            "timed_out": timed_out,
            "detail": detail,
            "hint": hint,
            "output": util::truncate_chars(&text, CLI_OUTPUT_CHARS),
            "runtime_supported": supported,
        }),
    )
}

/// Old CLIs print their usage/unknown-subcommand message instead of running
/// `mods`; surface that as an upgrade hint rather than a raw failure.
fn looks_unsupported(text: &str, supported: bool) -> bool {
    let lower = text.to_lowercase();
    lower.contains("unknown subcommand")
        || lower.contains("unrecognized")
        || lower.contains("invalid subcommand")
        || lower.contains("未知命令")
        || lower.contains("未知的子命令")
        || lower.contains("不支持")
        || (!supported && (lower.contains("usage") || lower.trim().is_empty()))
}

/// Prefer the CLI's `LINKSLOT_DETAIL` contract line, then the last meaningful
/// stderr/stdout line.
fn cli_detail(output: &util::RunOutput) -> Option<String> {
    let text = output.combined();
    let mut detail = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("LINKSLOT_DETAIL=") {
            let value = value.trim();
            if !value.is_empty() {
                detail = Some(util::truncate_chars(value, 500));
            }
        }
    }
    if detail.is_some() {
        return detail;
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(util::summarize(&text, 500))
}
