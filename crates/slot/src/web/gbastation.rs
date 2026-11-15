//! `/api/gbastation/**`: the Web-only GBAStation ROM importer.
//!
//! The mod page never talks to GBAStation itself. It calls these handlers
//! through the existing mod bridge, so the sandboxed iframe keeps
//! `connect-src 'none'` and the card write stays on the server.

use std::sync::Arc;

use serde_json::json;
use slot_store::Platform;

use crate::gbastation;
use crate::web::http::{Request, Response};
use crate::web::Shared;

const MAX_KEYS: usize = 256;

/// `POST /api/gbastation/games`: read the remote list, download nothing.
pub(crate) fn games(_shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let value = super::mods::json_request(request)?;
        let base = required_string(&value, "base_url")?;
        let games = gbastation::list(&base)
            .map_err(|error| Response::error(502, format!("读取 GBAStation 游戏列表失败：{error}")))?;
        Ok(Response::json(
            200,
            json!({
                "supported": Platform::ALL.iter().map(|platform| platform.dir_name()).collect::<Vec<_>>(),
                "games": gbastation::list_json(&games),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/gbastation/start`: download only the selected keys.
pub(crate) fn start(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        {
            let state = shared
                .gbastation_status
                .lock()
                .map_err(|_| Response::error(500, "GBAStation 导入状态不可用"))?;
            if state.running {
                return Err(Response::error(409, "已有 GBAStation 导入任务在运行"));
            }
        }
        if shared.busy.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Response::error(409, "设备正在同步或启动游戏，请稍后再试"));
        }
        let value = super::mods::json_request(request)?;
        let base = required_string(&value, "base_url")?;
        let base = gbastation::normalize_base_url(&base)
            .map_err(|error| Response::error(400, error))?;
        let keys = required_keys(&value)?;
        if keys.is_empty() {
            return Err(Response::error(400, "没有选择要同步的游戏"));
        }
        if keys.len() > MAX_KEYS {
            return Err(Response::error(400, format!("一次最多同步 {MAX_KEYS} 个游戏")));
        }
        let overwrite = value
            .get("overwrite")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        {
            let mut state = shared
                .gbastation_status
                .lock()
                .map_err(|_| Response::error(500, "GBAStation 导入状态不可用"))?;
            *state = gbastation::Status {
                running: true,
                stage: "准备导入".to_string(),
                total: keys.len(),
                ..gbastation::Status::default()
            };
        }
        shared.gbastation_cancel.store(false, std::sync::atomic::Ordering::Release);
        shared.busy.store(true, std::sync::atomic::Ordering::Release);

        let status = Arc::clone(&shared.gbastation_status);
        let cancel = Arc::clone(&shared.gbastation_cancel);
        let busy = Arc::clone(&shared.busy);
        let changed = Arc::clone(&shared.library_changed);
        let generation = Arc::clone(&shared.library_generation);
        let root = shared.root.clone();
        let panic_status = Arc::clone(&status);
        let spawned = std::thread::Builder::new()
            .name("slot-gbastation".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    gbastation::run(root, base, keys, overwrite, status, cancel);
                }))
                .is_err()
                {
                    if let Ok(mut state) = panic_status.lock() {
                        state.running = false;
                        state.finished = true;
                        state.stage = "导入失败".to_string();
                        state.errors.push("导入线程崩溃".to_string());
                    }
                }
                busy.store(false, std::sync::atomic::Ordering::Release);
                changed.store(true, std::sync::atomic::Ordering::Release);
                generation.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            });
        if let Err(error) = spawned {
            shared.busy.store(false, std::sync::atomic::Ordering::Release);
            if let Ok(mut state) = shared.gbastation_status.lock() {
                state.running = false;
                state.finished = true;
                state.stage = "导入失败".to_string();
                state.errors = vec![format!("无法启动导入任务：{error}")];
            }
            return Err(Response::error(500, "无法启动 GBAStation 导入任务"));
        }
        Ok(status_response(shared))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/gbastation/status`.
pub(crate) fn status(shared: &Shared) -> Response {
    status_response(shared)
}

/// `POST /api/gbastation/cancel`.
pub(crate) fn cancel(shared: &Shared) -> Response {
    shared
        .gbastation_cancel
        .store(true, std::sync::atomic::Ordering::Release);
    if let Ok(mut state) = shared.gbastation_status.lock() {
        if state.running {
            state.stage = "正在取消".to_string();
        }
    }
    status_response(shared)
}

fn status_response(shared: &Shared) -> Response {
    let state = shared
        .gbastation_status
        .lock()
        .map(|state| state.clone())
        .unwrap_or_default();
    Response::json(200, gbastation::status_json(&state))
}

fn required_string(value: &serde_json::Value, key: &str) -> Result<String, Response> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Response::error(400, format!("缺少 {key} 字符串")))
}

fn required_keys(value: &serde_json::Value) -> Result<Vec<String>, Response> {
    let items = value
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Response::error(400, "缺少 keys 数组"))?;
    let mut keys = Vec::new();
    for item in items {
        let key = item
            .as_str()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| Response::error(400, "keys 只能包含字符串"))?;
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_string());
        }
    }
    Ok(keys)
}
