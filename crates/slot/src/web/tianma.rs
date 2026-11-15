//! Web endpoints for the Tianma info-only importer.  The actual scan runs on
//! a worker; the mod page polls the small status snapshot.

use std::sync::atomic::Ordering;

use serde_json::json;

use crate::tianma::{self, Status};
use crate::web::http::Request;
use crate::web::Shared;
use crate::web::http::Response;

pub(crate) fn start(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        if shared.tianma_status.lock().map(|s| s.running).unwrap_or(false) {
            return Err(Response::error(409, "已有导入任务在运行"));
        }
        let value = super::mods::json_request(request)?;
        let relative = value
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(tianma::DEFAULT_SOURCE);
        let overwrite = value
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let source = tianma::resolve_source(&shared.root, relative)
            .map_err(|error| Response::error(400, error))?;
        {
            let mut state = shared.tianma_status.lock().unwrap_or_else(|e| e.into_inner());
            *state = Status {
                running: true,
                stage: "准备导入".to_string(),
                ..Status::default()
            };
        }
        let status = shared.tianma_status.clone();
        shared.tianma_cancel.store(false, Ordering::Release);
        shared.busy.store(true, Ordering::Release);
        let cancel = shared.tianma_cancel.clone();
        let busy = shared.busy.clone();
        let changed = shared.library_changed.clone();
        let root = shared.root.clone();
        let panic_status = status.clone();
        std::thread::Builder::new()
            .name("slot-tianma".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tianma::run(root, source, overwrite, status, cancel);
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
                busy.store(false, Ordering::Release);
                changed.store(true, Ordering::Release);
            })
            .map_err(|_| Response::error(500, "无法启动导入任务"))?;
        Ok(status_json(shared))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(crate) fn status(shared: &Shared) -> Response {
    status_json(shared)
}

pub(crate) fn cancel(shared: &Shared) -> Response {
    shared.tianma_cancel.store(true, Ordering::Release);
    status_json(shared)
}

fn status_json(shared: &Shared) -> Response {
    let status = shared
        .tianma_status
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    Response::json(
        200,
        json!({
            "running": status.running,
            "finished": status.finished,
            "cancelled": status.cancelled,
            "stage": status.stage,
            "total": status.total,
            "done": status.done,
            "matched": status.matched,
            "imported": status.imported,
            "skipped": status.skipped,
            "errors": status.errors,
        }),
    )
}
