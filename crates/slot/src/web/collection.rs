//! Web collection export/import endpoints and resumable ZIP upload.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::json;

use crate::collection::{self, Status, Upload};
use crate::web::http::{Request, Response};
use crate::web::Shared;

pub(crate) fn export(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        if shared
            .collection_status
            .lock()
            .map(|status| status.running)
            .unwrap_or(false)
        {
            return Err(Response::error(409, "已有合集任务在运行"));
        }
        let value = super::mods::json_request(request)?;
        let rom = value
            .get("rom")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|rom| !rom.is_empty())
            .map(str::to_string);
        let platform = if rom.is_some() {
            None
        } else {
            let raw = value
                .get("platform")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            Some(
                collection::platform(raw)
                    .ok_or_else(|| Response::error(400, "请选择要导出的平台"))?,
            )
        };
        reset_status(shared, "export", platform.map_or("", |platform| platform.dir_name()));
        let status = shared.collection_status.clone();
        shared.collection_cancel.store(false, Ordering::Release);
        shared.busy.store(true, Ordering::Release);
        let cancel = shared.collection_cancel.clone();
        let busy = shared.busy.clone();
        let root = shared.root.clone();
        let panic_status = status.clone();
        match (rom, platform) {
            (Some(rom), _) => {
                std::thread::Builder::new()
                    .name("slot-collection-export-game".into())
                    .spawn(move || {
                        let result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                collection::export_game(&root, &rom, &status, &cancel)
                                    .map(|_| ())
                            }));
                        finish_worker(&panic_status, result, "导出完成");
                        busy.store(false, Ordering::Release);
                    })
                    .map_err(|_| Response::error(500, "无法启动导出任务"))?;
            }
            (None, Some(platform)) => {
                std::thread::Builder::new()
                    .name("slot-collection-export".into())
                    .spawn(move || {
                        let result =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                collection::export_platform(&root, platform, &status, &cancel)
                                    .map(|_| ())
                            }));
                        finish_worker(&panic_status, result, "导出完成");
                        busy.store(false, Ordering::Release);
                    })
                    .map_err(|_| Response::error(500, "无法启动导出任务"))?;
            }
            (None, None) => return Err(Response::error(400, "请选择要导出的平台")),
        }
        Ok(status_json(shared))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(crate) fn status(shared: &Shared) -> Response {
    status_json(shared)
}

pub(crate) fn cancel(shared: &Shared) -> Response {
    shared.collection_cancel.store(true, Ordering::Release);
    if let Ok(mut status) = shared.collection_status.lock() {
        status.cancelled = true;
        status.stage = "取消中…".to_string();
    }
    status_json(shared)
}

pub(crate) fn upload_start(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        if shared
            .collection_status
            .lock()
            .map(|status| status.running)
            .unwrap_or(false)
        {
            return Err(Response::error(409, "已有合集任务在运行"));
        }
        let path = shared.root.join(collection::UPLOAD_TEMP);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Response::error(500, format!("创建上传目录失败：{error}")))?;
        }
        std::fs::File::create(&path)
            .map_err(|error| Response::error(500, format!("创建上传文件失败：{error}")))?;
        *shared.collection_upload.lock().unwrap_or_else(|e| e.into_inner()) = Some(Upload {
            path,
            offset: 0,
        });
        Ok(Response::json(200, json!({ "ok": true, "offset": 0 })))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(crate) fn upload_chunk(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        let offset: u64 = super::http::query_param(request, "offset")
            .map_err(|error| Response::error(error.status, error.message))?
            .ok_or_else(|| Response::error(400, "缺少 offset 参数"))?
            .parse()
            .map_err(|_| Response::error(400, "offset 不合法"))?;
        if request.body.is_empty() {
            return Err(Response::error(400, "上传分片为空"));
        }
        let mut upload = shared
            .collection_upload
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(state) = upload.as_mut() else {
            return Err(Response::error(409, "没有开始中的合集上传"));
        };
        if state.offset != offset {
            return Err(Response::error(
                409,
                format!("分片偏移不匹配，期望 {}", state.offset),
            ));
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&state.path)
            .map_err(|error| Response::error(500, format!("打开上传文件失败：{error}")))?;
        file.write_all(&request.body)
            .map_err(|error| Response::error(500, format!("写入上传分片失败：{error}")))?;
        state.offset = state.offset.saturating_add(request.body.len() as u64);
        Ok(Response::json(
            200,
            json!({ "ok": true, "offset": state.offset }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(crate) fn upload_finish(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if !shared.writable() {
            return Err(Response::error(409, "设备当前不可写"));
        }
        let value = super::mods::json_request(request)?;
        let overwrite = value
            .get("overwrite")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let upload = shared
            .collection_upload
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let Some(upload) = upload else {
            return Err(Response::error(409, "没有开始中的合集上传"));
        };
        if upload.offset == 0 || !upload.path.is_file() {
            return Err(Response::error(400, "合集上传为空"));
        }
        if let Ok(file) = std::fs::File::open(&upload.path) {
            if zip::ZipArchive::new(file).is_err() {
                let _ = std::fs::remove_file(&upload.path);
                return Err(Response::error(400, "上传的不是有效 ZIP 合集"));
            }
        }
        reset_status(shared, "import", "");
        let status = shared.collection_status.clone();
        shared.collection_cancel.store(false, Ordering::Release);
        shared.busy.store(true, Ordering::Release);
        let cancel = shared.collection_cancel.clone();
        let busy = shared.busy.clone();
        let changed = shared.library_changed.clone();
        let root = shared.root.clone();
        let path = upload.path.clone();
        let panic_status = status.clone();
        std::thread::Builder::new()
            .name("slot-collection-import".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    collection::import_zip(&root, &path, overwrite, &status, &cancel)
                }));
                let _ = std::fs::remove_file(&path);
                finish_worker(&panic_status, result, "导入完成");
                busy.store(false, Ordering::Release);
                changed.store(true, Ordering::Release);
            })
            .map_err(|_| Response::error(500, "无法启动导入任务"))?;
        Ok(status_json(shared))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(crate) fn upload_cancel(shared: &Shared) -> Response {
    let upload = shared
        .collection_upload
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(upload) = upload {
        let _ = std::fs::remove_file(&upload.path);
    }
    Response::json(200, json!({ "ok": true }))
}

fn reset_status(shared: &Shared, kind: &str, platform: &str) {
    let mut status = shared
        .collection_status
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *status = Status {
        running: true,
        kind: kind.to_string(),
        platform: platform.to_string(),
        stage: if kind == "export" {
            "准备导出".to_string()
        } else {
            "准备导入".to_string()
        },
        ..Status::default()
    };
}

fn finish_worker(
    status: &Arc<std::sync::Mutex<Status>>,
    result: std::thread::Result<Result<(), String>>,
    ok_stage: &str,
) {
    let mut status = status.lock().unwrap_or_else(|error| error.into_inner());
    status.running = false;
    status.finished = true;
    match result {
        Ok(Ok(_)) => {
            status.stage = ok_stage.to_string();
        }
        Ok(Err(error)) => {
            if status.cancelled {
                status.stage = "已取消".to_string();
            } else {
                status.stage = "失败".to_string();
                status.errors.push(error);
            }
        }
        Err(_) => {
            status.stage = "失败".to_string();
            status.errors.push("合集任务线程崩溃".to_string());
        }
    }
}

fn status_json(shared: &Shared) -> Response {
    let status = shared
        .collection_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();
    Response::json(
        200,
        json!({
            "running": status.running,
            "finished": status.finished,
            "cancelled": status.cancelled,
            "kind": status.kind,
            "platform": status.platform,
            "stage": status.stage,
            "total": status.total,
            "done": status.done,
            "imported": status.imported,
            "skipped": status.skipped,
            "duplicates": status.duplicates,
            "verified": status.verified,
            "bytes": status.bytes,
            "file": status.file,
            "errors": status.errors,
        }),
    )
}
