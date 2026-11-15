//! `/api/**` request handlers.
//!
//! All responses are same-origin JSON (or raw file bytes for `/api/file`).
//! Every mutating endpoint requires the `X-Slot-Web: 1` header (the page sets
//! it) and is refused with 409 while the Slot device is not writable (game or
//! sync in progress).  There is deliberately no authentication: the service is
//! only ever offered on the home LAN.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::web::http::{self, HttpError, Method, Request, Response};
use crate::web::paths::{self, PathReject};
use crate::web::saves;
use crate::web::util;
use crate::web::Shared;

/// The CLI sync may legitimately take a while on a slow WebDAV server.
const SYNC_RUN_TIMEOUT_SECS: u64 = 120;
/// A PROPFIND connection test is bounded much tighter than a full sync.
const SYNC_TEST_TIMEOUT_SECS: u64 = 20;
/// Larger outputs are truncated in the JSON reply (the firmware log keeps all).
const SYNC_OUTPUT_CHARS: usize = 8000;
const LOG_TAIL_LINES: usize = 200;
const LOG_TAIL_BYTES: u64 = 256 * 1024;
/// The Wi-Fi probe runs a shell script; page polls reuse the answer this long
/// instead of forking one per request.
const NETWORK_CACHE_TTL: Duration = Duration::from_secs(10);
/// The status header's storage numbers walk `States/` and `Saves/`; reuse them
/// briefly so a visible management page does not rescan the card every poll.
const STORAGE_CACHE_TTL: Duration = Duration::from_secs(10);

/// One `/api/**` endpoint: method + exact path + handler.
pub struct Route {
    pub method: Method,
    pub path: &'static str,
    pub handler: fn(&Shared, &Request) -> Response,
}

macro_rules! route {
    ($method:ident, $path:literal, $handler:path) => {
        Route {
            method: Method::$method,
            path: $path,
            handler: $handler,
        }
    };
}

/// The single source of truth for `/api/**`.
///
/// 新增资源 = 加一行；`tests` 会检查没有重复的 (method, path)，并且
/// "路径存在但方法不对" 会得到 405 而不是 404。
pub const ROUTES: &[Route] = &[
    route!(Get, "/api/status", w_status),
    route!(Get, "/api/saves", w_list_saves),
    route!(Post, "/api/saves/delete", delete_save),
    route!(Get, "/api/file", file_get),
    route!(Put, "/api/file", file_put),
    route!(Get, "/api/files", super::files::list),
    route!(Get, "/api/files/download", super::files::download),
    route!(Get, "/api/logs/export", w_logs_export),
    route!(Get, "/api/files/view", super::files::view),
    route!(Get, "/api/files/text", super::files::text),
    route!(Post, "/api/files/upload", super::files::upload),
    route!(Post, "/api/files/mkdir", super::files::mkdir),
    route!(Post, "/api/files/rename", super::files::rename),
    route!(Post, "/api/files/move", super::files::move_entry),
    route!(Post, "/api/files/delete", super::files::delete),
    route!(Get, "/api/play/rom", super::play::rom),
    route!(Get, "/api/play/save", super::play::save),
    route!(Put, "/api/play/save", super::play::save_put),
    route!(Get, "/api/roms", w_roms_list),
    route!(Get, "/api/rom/cover", super::roms::cover),
    route!(Put, "/api/rom/cover", super::roms::cover_put),
    route!(Get, "/api/rom/intro", super::roms::intro_get),
    route!(Put, "/api/rom/intro", super::roms::intro_put),
    route!(Put, "/api/rom/video", super::roms::video_put),
    route!(Put, "/api/rom/color", super::roms::color_put),
    route!(Put, "/api/rom/model", super::roms::model_put),
    route!(Get, "/api/rom/fronts", super::roms::fronts_get),
    route!(Get, "/api/rom/fronts/index", super::roms::fronts_index),
    route!(Get, "/api/rom/front", super::roms::front_get),
    route!(Get, "/api/rom/front/template", super::roms::front_template),
    route!(Post, "/api/rom/fronts", super::roms::front_put),
    route!(Put, "/api/rom/fronts/select", super::roms::front_select),
    route!(Post, "/api/rom/fronts/delete", super::roms::front_delete),
    route!(Get, "/api/rom/fronts/project", super::roms::front_project_get),
    route!(Put, "/api/rom/fronts/project", super::roms::front_project_put),
    route!(Post, "/api/rom/fronts/copy", super::roms::front_copy),
    route!(Get, "/api/rom/fronts/export", super::roms::front_export),
    route!(Post, "/api/rom/fronts/import", super::roms::front_import),
    route!(Get, "/api/rom/zip/list", super::zip_import::list),
    route!(Post, "/api/rom/zip/extract", super::zip_import::extract),
    route!(Put, "/api/rom/cheat", super::roms::cheat_put),
    route!(Put, "/api/rom/guide", super::roms::guide_put),
    route!(Post, "/api/rom/guide", super::roms::guide_set),
    route!(Post, "/api/library/migrate", library_migrate),
    route!(Post, "/api/collection/export", super::collection::export),
    route!(Get, "/api/collection/status", w_collection_status),
    route!(Post, "/api/collection/cancel", w_collection_cancel),
    route!(Post, "/api/collection/upload/start", super::collection::upload_start),
    route!(Put, "/api/collection/upload", super::collection::upload_chunk),
    route!(Post, "/api/collection/upload/finish", super::collection::upload_finish),
    route!(Post, "/api/collection/upload/cancel", w_collection_upload_cancel),
    route!(Post, "/api/rom/add", super::roms::add),
    route!(Post, "/api/rom/delete", super::roms::delete),
    route!(Post, "/api/rom/rename", super::roms::rename),
    route!(Put, "/api/rom", super::roms::rom_put),
    route!(Get, "/api/sync", w_sync_info),
    route!(Post, "/api/sync/config", sync_config),
    route!(Post, "/api/sync/test", sync_test),
    route!(Post, "/api/sync/run", sync_run),
    route!(Get, "/api/network", w_network_status),
    route!(Get, "/api/settings", w_settings_info),
    route!(Post, "/api/settings", settings_save),
    route!(Get, "/api/i18n", super::i18n::info),
    route!(Get, "/api/network/scan", w_network_scan),
    route!(Post, "/api/network/connect", network_connect),
    route!(Post, "/api/log/level", log_level_set),
    route!(Get, "/api/mods", w_mods_list),
    route!(Get, "/api/mods/item", super::mods::item),
    route!(Post, "/api/mods/item", super::mods::save_item),
    route!(Get, "/api/mods/files", super::mods::files),
    route!(Get, "/api/mods/file", super::mods::file_item),
    route!(Post, "/api/mods/file", super::mods::save_file),
    route!(Post, "/api/mods/toggle", super::mods::toggle),
    route!(Post, "/api/mods/delete", super::mods::delete),
    route!(Post, "/api/mods/uninstall", super::mods::uninstall),
    route!(Get, "/api/mods/action", super::mods::action_item),
    route!(Post, "/api/mods/action", super::mods::save_action),
    route!(Post, "/api/mods/action/toggle", super::mods::toggle_action),
    route!(Post, "/api/mods/action/delete", super::mods::delete_action),
    route!(Post, "/api/mods/test", super::mods::test),
    route!(Post, "/api/mods/check", super::mods::check),
    route!(Post, "/api/upgrade", super::upgrade::upload),
    route!(Get, "/api/upgrade/status", w_upgrade_status),
    route!(Post, "/api/tianma/start", super::tianma::start),
    route!(Get, "/api/tianma/status", w_tianma_status),
    route!(Post, "/api/tianma/cancel", w_tianma_cancel),
    route!(Post, "/api/gbastation/games", super::gbastation::games),
    route!(Post, "/api/gbastation/start", super::gbastation::start),
    route!(Get, "/api/gbastation/status", w_gbastation_status),
    route!(Post, "/api/gbastation/cancel", w_gbastation_cancel),
];

/// Handlers that need only the shared state; adapted to the uniform signature.
fn w_status(shared: &Shared, _r: &Request) -> Response {
    status(shared)
}
fn w_list_saves(shared: &Shared, _r: &Request) -> Response {
    list_saves(shared)
}
fn w_roms_list(shared: &Shared, _r: &Request) -> Response {
    super::roms::list(shared)
}
fn w_tianma_cancel(shared: &Shared, _r: &Request) -> Response {
    super::tianma::cancel(shared)
}
fn w_gbastation_status(shared: &Shared, _r: &Request) -> Response {
    super::gbastation::status(shared)
}
fn w_gbastation_cancel(shared: &Shared, _r: &Request) -> Response {
    super::gbastation::cancel(shared)
}
fn w_logs_export(shared: &Shared, _r: &Request) -> Response {
    super::logs::export(shared)
}
fn w_collection_status(shared: &Shared, _r: &Request) -> Response {
    super::collection::status(shared)
}
fn w_collection_cancel(shared: &Shared, _r: &Request) -> Response {
    super::collection::cancel(shared)
}
fn w_collection_upload_cancel(shared: &Shared, _r: &Request) -> Response {
    super::collection::upload_cancel(shared)
}
fn w_sync_info(shared: &Shared, _r: &Request) -> Response {
    sync_info(shared)
}
fn w_network_status(shared: &Shared, _r: &Request) -> Response {
    network_status(shared)
}
fn w_settings_info(shared: &Shared, _r: &Request) -> Response {
    settings_info(shared)
}
fn w_network_scan(shared: &Shared, _r: &Request) -> Response {
    network_scan(shared)
}
fn w_mods_list(shared: &Shared, _r: &Request) -> Response {
    super::mods::list(shared)
}
fn w_upgrade_status(shared: &Shared, _r: &Request) -> Response {
    super::upgrade::status(shared)
}
fn w_tianma_status(shared: &Shared, _r: &Request) -> Response {
    super::tianma::status(shared)
}

fn lookup(method: Method, path: &str) -> Option<&'static Route> {
    ROUTES
        .iter()
        .find(|route| route.method == method && route.path == path)
}

fn known_path(path: &str) -> bool {
    ROUTES.iter().any(|route| route.path == path)
}

/// Dispatch one `/api/**` request.  Static assets are handled by the caller.
pub fn route(shared: &Shared, request: &Request) -> Response {
    if let Some(route) = lookup(request.method, request.path.as_str()) {
        return (route.handler)(shared, request);
    }
    if known_path(request.path.as_str()) {
        localized_error(shared, 405, "web_api.error.method_not_allowed")
    } else {
        localized_error(shared, 404, "web_api.error.route_not_found")
    }
}


fn http_error(error: HttpError) -> Response {
    Response::error(error.status, error.message)
}

/// User-facing API errors follow `linkSlot/Config/language.txt`, the same setting the
/// device and the management page use.
pub(super) fn localized_error(shared: &Shared, status: u16, key: &'static str) -> Response {
    Response::error(
        status,
        slot_i18n::t(slot_store::read_language(&shared.root), key),
    )
}

pub(super) fn localized_error_args(
    shared: &Shared,
    status: u16,
    key: &'static str,
    args: &[(&str, &str)],
) -> Response {
    Response::error(
        status,
        slot_i18n::t_args(slot_store::read_language(&shared.root), key, args),
    )
}

fn path_error(error: PathReject) -> Response {
    Response::error(error.status, error.message)
}

/// Writes require the page's custom header and a writable device.
pub(super) fn guard_write(shared: &Shared, request: &Request) -> Result<(), Response> {
    if request.header("x-slot-web") != Some("1") {
        return Err(localized_error(shared, 403, "web_api.error.write_header"));
    }
    if !shared.writable() {
        return Err(localized_error(shared, 409, "web_api.error.readonly"));
    }
    Ok(())
}

// ------------------------------------------------------------------ 概览

fn status(shared: &Shared) -> Response {
    let wifi = cached_network(shared);
    let storage = cached_storage(shared);
    let linkslot = shared
        .link_version
        .get_or_init(|| read_link_version(&shared.root))
        .clone();
    Response::json(
        200,
        json!({
            "slot": slot_ui::PROJECT_VERSION,
            "linkslot": linkslot,
            "uptime_secs": shared.started.elapsed().as_secs(),
            "web": {
                "port": shared.port,
                "url": &shared.url,
            },
            "network": wifi,
            "storage": storage,
            "writable": shared.writable(),
        }),
    )
}

struct WifiSnapshot {
    connected: bool,
    ssid: Option<String>,
    ip: Option<String>,
}

fn wifi_snapshot(root: &Path) -> WifiSnapshot {
    match crate::wifi_job::status_sync(root) {
        Ok(crate::wifi_job::WifiStatus::Connected { ssid, ip }) => WifiSnapshot {
            connected: true,
            ssid: Some(ssid),
            ip: Some(ip),
        },
        Ok(_) => WifiSnapshot {
            connected: false,
            ssid: None,
            ip: None,
        },
        Err(_) => WifiSnapshot {
            connected: false,
            ssid: None,
            ip: None,
        },
    }
}

/// Lazy, cached `linkSlot version`.  Never blocks startup; a missing or broken
/// CLI yields `"unknown"` and is not retried.
fn read_link_version(root: &Path) -> String {
    let executable = root.join(crate::root::SYSTEM_BIN).join("linkslot_sync");
    let mut direct = std::process::Command::new(&executable);
    direct.arg("version");
    if let Ok(output) = util::run_with_timeout(direct, Duration::from_secs(2)) {
        let version = parse_version_output(&output.stdout);
        if version != "unknown" {
            return version;
        }
    }
    // FAT32 cards cannot mark files executable: retry through /bin/sh.
    #[cfg(unix)]
    {
        let mut shell = std::process::Command::new("/bin/sh");
        shell.arg(&executable).arg("version");
        if let Ok(output) = util::run_with_timeout(shell, Duration::from_secs(2)) {
            let version = parse_version_output(&output.stdout);
            if version != "unknown" {
                return version;
            }
        }
    }
    "unknown".to_string()
}

fn parse_version_output(stdout: &str) -> String {
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `linkSlot 0.0.2` -> `0.0.2`; otherwise keep the first line.
        let token = line.split_whitespace().find(|token| {
            let token = token.trim_start_matches('v');
            token.contains('.')
                && token
                    .chars()
                    .next()
                    .map(|ch| ch.is_ascii_digit())
                    .unwrap_or(false)
        });
        return util::truncate_chars(token.unwrap_or(line).trim_start_matches('v'), 64);
    }
    "unknown".to_string()
}

// ------------------------------------------------------------------ 存档

fn list_saves(shared: &Shared) -> Response {
    let scan = saves::scan(&shared.root);
    Response::json(200, saves::units_value(&scan))
}

fn delete_save(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let raw = http::query_param(request, "path").map_err(http_error)?;
        let raw = raw.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let safe = paths::validate(&raw).map_err(path_error)?;
        let extension = safe.extension();
        if !matches!(extension.as_str(), "state" | "sav" | "srm") {
            return Err(Response::error(400, "只允许删除 .state、.sav 或 .srm 存档"));
        }
        let target = paths::resolve_read(&shared.root, &safe).map_err(path_error)?;
        std::fs::remove_file(&target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Response::error(404, "存档不存在")
            } else {
                Response::error(500, "删除存档失败")
            }
        })?;

        // A state archive has a sibling thumbnail.  Delete it with the state;
        // a missing thumbnail is normal and is never an error.
        let mut thumbnail_removed = false;
        if extension == "state" {
            let thumbnail = target.with_extension("png");
            if let Ok(metadata) = std::fs::symlink_metadata(&thumbnail) {
                if metadata.is_file() || metadata.file_type().is_symlink() {
                    if std::fs::remove_file(&thumbnail).is_ok() {
                        thumbnail_removed = true;
                    }
                }
            }
        }

        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "path": &safe.rel,
                "thumbnail_removed": thumbnail_removed,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn file_get(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let raw = http::query_param(request, "path").map_err(http_error)?;
        let raw = raw.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let safe = paths::validate(&raw).map_err(path_error)?;
        let target = paths::resolve_read(&shared.root, &safe).map_err(path_error)?;
        let metadata =
            std::fs::metadata(&target).map_err(|_| Response::error(404, "文件不存在"))?;
        if metadata.len() > http::MAX_BODY_BYTES as u64 {
            return Err(Response::error(413, "文件过大，无法通过网页读取"));
        }
        let bytes = std::fs::read(&target).map_err(|_| Response::error(500, "读取文件失败"))?;
        let content_type = if safe.is_image() {
            "image/png"
        } else {
            "application/octet-stream"
        };
        // Thumbnails are shown inline; everything else downloads as an
        // attachment with a UTF-8 filename (Chinese save names work).
        let disposition = if safe.is_image() {
            "inline"
        } else {
            "attachment"
        };
        Ok(Response::new(200, content_type, bytes)
            .with_header(
                "Content-Disposition",
                http::content_disposition(disposition, safe.file_name()),
            )
            .with_header("Cache-Control", "no-store"))
    })();
    handled.unwrap_or_else(|response| response)
}

fn file_put(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let raw = http::query_param(request, "path").map_err(http_error)?;
        let raw = raw.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let safe = paths::validate(&raw).map_err(path_error)?;
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("上传内容超过 {mb}MB 限制")));
        }
        let target = paths::resolve_write(&shared.root, &safe).map_err(path_error)?;
        paths::atomic_upload(&target, &request.body).map_err(|err| Response::error(500, err))?;
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "path": &safe.rel,
                "size": request.body.len(),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

// ------------------------------------------------------------------ 云同步

fn settings_info(shared: &Shared) -> Response {
    Response::json(
        200,
        json!({
            "max_upload_mb": super::settings::upload_mb(&shared.root),
            "default_max_upload_mb": super::settings::DEFAULT_UPLOAD_MB,
            "min_upload_mb": super::settings::MIN_UPLOAD_MB,
            "max_limit_mb": super::settings::MAX_UPLOAD_MB,
            "upload_limit_bytes": shared.upload_limit(),
            // The device and this page share `linkSlot/Config/language.txt`; the page
            // reads it here so it can show which language is in force.
            "language": slot_store::read_language(&shared.root).code(),
            "languages": slot_i18n::Language::ALL
                .iter()
                .map(|language| json!({
                    "code": language.code(),
                    "label": language.menu_label(),
                }))
                .collect::<Vec<_>>(),
        }),
    )
}

fn settings_save(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        // The page posts only what it changed, so both fields are optional —
        // but an empty body is still a client error.
        let language = match value.get("language") {
            None => None,
            Some(field) => {
                let saved = slot_store::read_language(&shared.root);
                let Some(raw) = field.as_str() else {
                    let message = slot_i18n::t(saved, "web_api.error.language_missing");
                    return Err(Response::error(400, message));
                };
                let Some(parsed) = slot_i18n::Language::parse(raw) else {
                    let message = slot_i18n::t_args(
                        saved,
                        "web_api.error.language_unknown",
                        &[("code", raw)],
                    );
                    return Err(Response::error(400, message));
                };
                Some(parsed)
            }
        };
        let upload = match value.get("max_upload_mb") {
            None => None,
            Some(field) => {
                let Some(raw) = field.as_u64() else {
                    return Err(localized_error(shared, 400, "web_api.error.missing_max_upload"));
                };
                let Ok(mb) = u16::try_from(raw) else {
                    return Err(localized_error(shared, 400, "web_api.error.max_upload_range"));
                };
                Some(mb)
            }
        };
        if language.is_none() && upload.is_none() {
            // `{}` or a body of unrelated keys: nothing was asked to be saved,
            // which is not "a missing language parameter".
            let message = slot_i18n::t(
                slot_store::read_language(&shared.root),
                "web_api.error.settings_empty",
            );
            return Err(Response::error(400, message));
        }
        if let Some(language) = language {
            slot_store::write_language(&shared.root, language)
                .map_err(|error| Response::error(400, error))?;
        }
        if let Some(mb) = upload {
            super::settings::write_upload_mb(&shared.root, mb)
                .map_err(|error| Response::error(400, error))?;
            shared.set_upload_limit(mb as usize * 1024 * 1024);
        }
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "max_upload_mb": super::settings::upload_mb(&shared.root),
                "upload_limit_bytes": shared.upload_limit(),
                "language": slot_store::read_language(&shared.root).code(),
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn sync_info(shared: &Shared) -> Response {
    let config = crate::linkcfg::read(&shared.root);
    let caches = read_caches(&shared.root, &config);
    let log_path = shared.root.join("linkSlot/Logs").join("linkSlot.log");
    let log_tail = util::tail_lines(&log_path, LOG_TAIL_LINES, LOG_TAIL_BYTES);
    let storages: Vec<Value> = config
        .storages
        .iter()
        .map(|profile| {
            json!({
                "id": &profile.id,
                "name": &profile.name,
                "kind": &profile.kind,
                "url": &profile.url,
                "remote_root": &profile.remote_root,
                "username": &profile.username,
                "password_set": profile.password_set,
                "ca_cert": &profile.ca_cert,
                "insecure": profile.insecure,
            })
        })
        .collect();
    Response::json(
        200,
        json!({
            "webdav": {
                "url": &config.webdav.url,
                "remote_root": &config.webdav.remote_root,
                "username": &config.webdav.username,
                "password_set": config.webdav.password_set,
                "ca_cert": &config.webdav.ca_cert,
                "insecure": config.webdav.insecure,
            },
            "active_storage": &config.active_storage,
            "storages": storages,
            "storages_warning": &config.storages_warning,
            "keep_remote": config.webdav.keep_remote,
            "caches": caches,
            "log_tail": log_tail,
            "log_level": crate::linkcfg::read_log_level(&shared.root).as_str(),
        }),
    )
}

fn read_caches(root: &Path, config: &crate::linkcfg::LinkConfig) -> Vec<Value> {
    let mut caches = Vec::new();
    let dir = root.join("linkSlot/Config/cache");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return caches;
    };
    let current_url = config.webdav.url.trim();
    let current_root = config.webdav.remote_root.trim();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // Corrupt or foreign cache files are skipped, never fatal.
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let unit = &value["unit"];
        let platform = unit["platform"].as_str().unwrap_or("");
        let emulator = unit["emulator"].as_str().unwrap_or("");
        let game = unit["game"].as_str().unwrap_or("");
        if platform.is_empty() && emulator.is_empty() && game.is_empty() {
            continue;
        }
        let remote_matches = value["remote"]["url"].as_str() == Some(current_url)
            && value["remote"]["remote_root"].as_str() == Some(current_root);
        caches.push(json!({
            "platform": platform,
            "emulator": emulator,
            "game": game,
            "version": value["version"].as_u64().unwrap_or(0),
            "synced_at": value["synced_at"].as_str().unwrap_or(""),
            "remote_match": remote_matches,
        }));
    }
    caches.sort_by(|left, right| {
        right["synced_at"]
            .as_str()
            .unwrap_or("")
            .cmp(left["synced_at"].as_str().unwrap_or(""))
    });
    caches
}

/// Explicit one-shot import of legacy `Metadata/*.json` sidecars.  The normal
/// boot path never looks at sidecars; this endpoint is called by the migration
/// mod's Web UI after the user clicks the button.
fn library_migrate(shared: &Shared, request: &Request) -> Response {
    if let Err(response) = guard_write(shared, request) {
        return response;
    }
    match slot_store::cart_meta::migrate_sidecars(&shared.root) {
        Ok(report) => {
            shared.touch_library_changed();
            Response::json(200, serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({})))
        }
        Err(error) => Response::error(500, format!("元数据迁移失败：{error}")),
    }
}

fn sync_config(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request).map_err(http_error)?;
        if let Some(profiles) = value.get("profiles").and_then(Value::as_array) {
            let active = http::json_string(&value, "active").unwrap_or_default();
            let mut edits = Vec::with_capacity(profiles.len());
            for profile in profiles {
                edits.push(storage_edit(profile)?);
            }
            if edits.is_empty() {
                return Err(Response::error(400, "至少保留一个同步方式"));
            }
            crate::linkcfg::save_storages(
                &shared.root,
                active.trim(),
                &edits,
                http::json_bool(&value, "keep_remote", false),
            )
            .map_err(|err| {
                Response::error(
                    400,
                    format!("保存同步方式失败：{}", util::summarize(&err, 300)),
                )
            })?;
            return Ok(Response::json(200, json!({ "ok": true })));
        }

        let url = http::json_string(&value, "url").unwrap_or_default();
        let url = url.trim().to_string();
        if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(Response::error(
                400,
                "WebDAV 地址需以 http:// 或 https:// 开头",
            ));
        }
        // Empty / missing password means "keep the stored one"; the separate
        // clear_password switch sends Some("") down to linkcfg to erase it.
        let typed_password = match http::json_string(&value, "password") {
            Some(password) if !password.is_empty() => Some(password),
            _ => None,
        };
        let password = if http::json_bool(&value, "clear_password", false) {
            Some(String::new())
        } else {
            typed_password
        };
        let edit = crate::linkcfg::WebdavEdit {
            url,
            remote_root: http::json_string(&value, "remote_root").unwrap_or_default(),
            username: http::json_string(&value, "username").unwrap_or_default(),
            password,
            ca_cert: http::json_string(&value, "ca_cert").unwrap_or_default(),
            insecure: http::json_bool(&value, "insecure", false),
            keep_remote: http::json_bool(&value, "keep_remote", false),
        };
        crate::linkcfg::write_webdav(&shared.root, edit).map_err(|err| {
            Response::error(
                500,
                format!("保存 WebDAV 设置失败：{}", util::summarize(&err, 300)),
            )
        })?;
        Ok(Response::json(200, json!({ "ok": true })))
    })();
    handled.unwrap_or_else(|response| response)
}

fn storage_edit(value: &Value) -> Result<crate::linkcfg::StorageEdit, Response> {
    // Empty password means keep the stored one; clear_password erases it.
    let typed_password = match value.get("password").and_then(Value::as_str) {
        Some(password) if !password.is_empty() => Some(password.to_string()),
        _ => None,
    };
    let password = if value
        .get("clear_password")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some(String::new())
    } else {
        typed_password
    };
    Ok(crate::linkcfg::StorageEdit {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("webdav")
            .trim()
            .to_string(),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        remote_root: value
            .get("remote_root")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        username: value
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        password,
        ca_cert: value
            .get("ca_cert")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        insecure: value
            .get("insecure")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

struct TestConfig {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Drop for TestConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Write a temporary config containing the form values, so "test connection"
/// tests what is on screen without persisting it.  The temp directory is
/// removed when the response leaves this handler.
fn write_test_config(root: &Path, value: &Value) -> Result<TestConfig, Response> {
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let remote_root = value
        .get("remote_root")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let remote_root = if remote_root.is_empty() {
        "/linkSlot".to_string()
    } else {
        remote_root
    };
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let typed_password = value
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let clear = value
        .get("clear_password")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let id = value.get("id").and_then(Value::as_str).unwrap_or_default();
    let password = if clear {
        String::new()
    } else if !typed_password.is_empty() {
        typed_password.to_string()
    } else {
        crate::linkcfg::storage_password(root, id).unwrap_or_default()
    };

    let mut webdav = toml::Table::new();
    webdav.insert("url".to_string(), toml::Value::String(url));
    webdav.insert("remote_root".to_string(), toml::Value::String(remote_root));
    webdav.insert("username".to_string(), toml::Value::String(username));
    webdav.insert("password".to_string(), toml::Value::String(password));
    webdav.insert(
        "ca_cert".to_string(),
        toml::Value::String(
            value
                .get("ca_cert")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
    );
    webdav.insert(
        "insecure".to_string(),
        toml::Value::Boolean(
            value
                .get("insecure")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    let mut sync = toml::Table::new();
    sync.insert("timeout_secs".to_string(), toml::Value::Integer(20));
    let mut doc = toml::Table::new();
    doc.insert("webdav".to_string(), toml::Value::Table(webdav));
    doc.insert("sync".to_string(), toml::Value::Table(sync));
    let text = toml::to_string(&doc)
        .map_err(|error| Response::error(500, format!("生成测试配置失败：{error}")))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("slot-web-test-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|error| Response::error(500, format!("创建测试配置目录失败：{error}")))?;
    let path = dir.join("config.toml");
    std::fs::write(&path, text)
        .map_err(|error| Response::error(500, format!("写入测试配置失败：{error}")))?;
    Ok(TestConfig { dir, path })
}

fn sync_test(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        if shared
            .busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return Err(localized_error(shared, 409, "web_api.error.sync_busy"));
        }
        struct TestBusy<'a>(&'a Shared);
        impl Drop for TestBusy<'_> {
            fn drop(&mut self) {
                self.0
                    .busy
                    .store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let _busy = TestBusy(shared);

        let value = http::json_body(request).unwrap_or_else(|_| json!({}));
        let mut test_config = None;
        let url = http::json_string(&value, "url").unwrap_or_default();
        if !url.trim().is_empty() {
            test_config = Some(write_test_config(&shared.root, &value)?);
        }

        let mut command = Command::new("/bin/sh");
        command
            .arg(shared.root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh"))
            .arg("net")
            .arg("test");
        if let Some(config) = &test_config {
            command.arg("--config").arg(&config.path);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = util::run_with_timeout(command, Duration::from_secs(SYNC_TEST_TIMEOUT_SECS))
            .map_err(|err| {
            Response::error(
                500,
                format!("无法启动 WebDAV 连接测试：{}", util::summarize(&err, 200)),
            )
        })?;
        if output.timed_out {
            return Err(Response::error(
                500,
                format!("WebDAV 连接测试超时（{SYNC_TEST_TIMEOUT_SECS} 秒），已终止"),
            ));
        }
        let code = output.code.unwrap_or(-1);
        let combined = output.combined();
        let detail = cli_detail(&combined);
        let hint = test_hint(code, detail.as_deref(), &value, &shared.root);
        Ok(Response::json(
            200,
            json!({
                "ok": code == 0,
                "code": code,
                "output": util::truncate_chars(&combined, SYNC_OUTPUT_CHARS),
                "detail": detail,
                "hint": hint,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// Pull the CLI's machine-readable reason out of stdout, falling back to the
/// last human-readable line so a Web user sees the real WebDAV failure instead
/// of only "connection failed".
fn cli_detail(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(detail) = line.trim().strip_prefix("LINKSLOT_DETAIL=") {
            if !detail.trim().is_empty() {
                return Some(detail.trim().to_string());
            }
        }
    }
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn test_hint(code: i32, detail: Option<&str>, value: &Value, root: &Path) -> String {
    if code == 0 {
        return "WebDAV 连接正常，可以执行云同步。".to_string();
    }
    let detail = detail.unwrap_or_default();
    if detail.contains("远程根目录不存在") || detail.contains("HTTP 404") {
        let remote_root = value
            .get("remote_root")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| crate::linkcfg::read(root).webdav.remote_root);
        return format!(
            "远程根目录「{}」不存在：请先在 WebDAV/坚果云中创建这个文件夹，或把远程根目录改成已存在的文件夹。",
            if remote_root.is_empty() {
                "/linkSlot"
            } else {
                &remote_root
            }
        );
    }
    if detail.is_empty() {
        return match code {
            3 => "配置错误：请检查 WebDAV 地址、远程根目录和证书设置。".to_string(),
            _ => "连接失败：请检查地址、远程根目录、用户名/密码、证书和网络。".to_string(),
        };
    }
    if code == 3 || detail.contains("配置错误") {
        format!("配置错误：{detail}")
    } else {
        detail.to_string()
    }
}

fn sync_run(shared: &Shared, request: &Request) -> Response {
    struct SyncBusy<'a>(&'a Shared);
    impl Drop for SyncBusy<'_> {
        fn drop(&mut self) {
            self.0
                .busy
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }
    shared
        .busy
        .store(true, std::sync::atomic::Ordering::Release);
    let _busy = SyncBusy(shared);
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request).map_err(http_error)?;
        let platform = required_field(&value, "platform")?;
        let emulator = required_field(&value, "emulator")?;
        let game = required_field(&value, "game")?;
        validate_sync_field("platform", &platform)?;
        validate_sync_field("emulator", &emulator)?;
        validate_sync_field("game", &game)?;

        // The CLI is invoked through /bin/sh by sync_job; arguments are passed
        // as argv (never a shell command string), so Chinese names are safe.
        let sync_request =
            crate::sync_job::SyncRequest::new(platform, emulator, game, shared.root.clone());
        let command = crate::sync_job::system_command(&sync_request);
        let output = util::run_with_timeout(command, Duration::from_secs(SYNC_RUN_TIMEOUT_SECS))
            .map_err(|err| {
                Response::error(500, format!("无法启动同步：{}", util::summarize(&err, 200)))
            })?;
        if output.timed_out {
            return Err(Response::error(
                500,
                format!(
                    "同步超时（{SYNC_RUN_TIMEOUT_SECS} 秒），已终止。请稍后重试或回到设备查看。"
                ),
            ));
        }
        let code = output.code.unwrap_or(-1);
        let hint = match code {
            2 => Some("检测到冲突：请回到设备上的 Slot 同步界面选择覆盖方向。"),
            3 => Some("配置错误：请检查 WebDAV 设置后重试。"),
            _ => None,
        };
        Ok(Response::json(
            200,
            json!({
                "ok": code == 0,
                "code": code,
                "output": util::truncate_chars(&output.combined(), SYNC_OUTPUT_CHARS),
                "hint": hint,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

fn required_field(value: &Value, key: &str) -> Result<String, Response> {
    match value.get(key).and_then(|field| field.as_str()) {
        Some(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => Err(Response::error(400, format!("缺少 {key} 参数"))),
    }
}

fn validate_sync_field(label: &str, value: &str) -> Result<(), Response> {
    if value.trim().is_empty() {
        return Err(Response::error(400, format!("{label} 不能为空")));
    }
    if value.len() > 128 {
        return Err(Response::error(400, format!("{label} 过长")));
    }
    if value.contains('/') || value.contains('\\') || value.contains('\0') {
        return Err(Response::error(400, format!("{label} 含非法字符")));
    }
    if value == "." || value == ".." {
        return Err(Response::error(400, format!("{label} 不合法")));
    }
    Ok(())
}

fn log_level_set(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request).map_err(http_error)?;
        let raw = required_field(&value, "level")?;
        let level = crate::linkcfg::LogLevel::parse(&raw)
            .ok_or_else(|| localized_error(shared, 400, "web_api.error.log_level"))?;
        crate::linkcfg::write_log_level(&shared.root, level)
            .map_err(|error| Response::error(500, error))?;
        Ok(Response::json(
            200,
            json!({ "ok": true, "level": level.as_str() }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

// ------------------------------------------------------------------ 网络

fn network_status(shared: &Shared) -> Response {
    let saved: Vec<Value> = crate::linkcfg::read_saved_wifi(&shared.root)
        .into_iter()
        .map(|network| {
            json!({
                "ssid": network.ssid,
                "password_set": network.password_set,
            })
        })
        .collect();
    let wifi = cached_network(shared);
    Response::json(
        200,
        json!({
            "connected": wifi["connected"],
            "ssid": wifi["ssid"],
            "ip": wifi["ip"],
            "saved": saved,
        }),
    )
}

/// `{connected, ssid, ip}` from the Wi-Fi probe, cached for
/// [`NETWORK_CACHE_TTL`].  A probe error folds to "disconnected": the page
/// polls this often and a transient script failure should not paint an error.
fn cached_network(shared: &Shared) -> Value {
    let now = Instant::now();
    if let Ok(cache) = shared.network_cache.lock() {
        if let Some((at, value)) = cache.as_ref() {
            if now.duration_since(*at) < NETWORK_CACHE_TTL {
                return value.clone();
            }
        }
    }
    let snapshot = wifi_snapshot(&shared.root);
    let value = json!({
        "connected": snapshot.connected,
        "ssid": snapshot.ssid,
        "ip": snapshot.ip,
    });
    if let Ok(mut cache) = shared.network_cache.lock() {
        *cache = Some((now, value.clone()));
    }
    value
}

/// Storage numbers for the status header, cached for [`STORAGE_CACHE_TTL`].
fn cached_storage(shared: &Shared) -> Value {
    let now = Instant::now();
    if let Ok(cache) = shared.storage_cache.lock() {
        if let Some((at, value)) = cache.as_ref() {
            if now.duration_since(*at) < STORAGE_CACHE_TTL {
                return value.clone();
            }
        }
    }
    let scan = saves::scan(&shared.root);
    let value = saves::storage_value(&scan);
    if let Ok(mut cache) = shared.storage_cache.lock() {
        *cache = Some((now, value.clone()));
    }
    value
}

fn network_scan(shared: &Shared) -> Response {
    let mut wifi = crate::wifi_job::SystemWifi::new(shared.root.clone());
    match wifi.scan() {
        Ok(networks) => {
            // One row per SSID (keep the strongest BSS for duplicate names).
            let mut strongest: std::collections::BTreeMap<String, crate::wifi_job::WifiNetwork> =
                std::collections::BTreeMap::new();
            for network in networks {
                strongest
                    .entry(network.ssid.clone())
                    .and_modify(|existing| {
                        if network.signal > existing.signal {
                            *existing = network.clone();
                        }
                    })
                    .or_insert(network);
            }
            let mut list: Vec<_> = strongest.into_values().collect();
            list.sort_by(|left, right| right.signal.cmp(&left.signal));
            let rows: Vec<Value> = list
                .iter()
                .map(|network| {
                    json!({
                        "ssid": &network.ssid,
                        "signal": network.signal,
                        "secured": network.secured,
                    })
                })
                .collect();
            Response::json(200, json!({ "networks": rows }))
        }
        Err(error) => Response::error(
            500,
            format!("扫描网络失败：{}", util::summarize(&error, 200)),
        ),
    }
}

fn network_connect(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request).map_err(http_error)?;
        let ssid = required_field(&value, "ssid")?;
        if ssid.len() > 128 || ssid.contains('\0') || ssid.contains('\n') || ssid.contains('\r') {
            return Err(localized_error(shared, 400, "web_api.error.wifi_ssid_invalid"));
        }
        let saved = value.get("saved").and_then(Value::as_bool).unwrap_or(false);
        let psk = if saved {
            crate::linkcfg::saved_wifi_password(&shared.root, &ssid).ok_or_else(|| {
                localized_error_args(
                    shared,
                    404,
                    "web_api.error.wifi_saved_missing",
                    &[("ssid", ssid.as_str())],
                )
            })?
        } else {
            http::json_string(&value, "psk").unwrap_or_default()
        };
        if psk.len() > 128 || psk.contains('\0') || psk.contains('\n') || psk.contains('\r') {
            return Err(localized_error(shared, 400, "web_api.error.wifi_password_invalid"));
        }
        let mut wifi = crate::wifi_job::SystemWifi::new(shared.root.clone());
        match wifi.connect(&ssid, &psk) {
            Ok(status) => {
                crate::linkcfg::write_wifi(&shared.root, &ssid, &psk).map_err(|error| {
                    Response::error(
                        500,
                        format!(
                            "WiFi 已连接，但保存配置失败：{}",
                            util::summarize(&error, 200)
                        ),
                    )
                })?;
                let (connected, ip) = match status {
                    crate::wifi_job::WifiStatus::Connected { ip, .. } => (true, Some(ip)),
                    _ => (false, None),
                };
                Ok(Response::json(
                    200,
                    json!({
                        "ok": true,
                        "ssid": ssid,
                        "connected": connected,
                        "ip": ip,
                        "hint": "WiFi 切换后设备 IP 可能变化；若本页失去响应，请用设备上显示的新地址访问。",
                    }),
                ))
            }
            Err(error) => Err(Response::error(
                500,
                format!("连接失败：{}", util::summarize(&error, 300)),
            )),
        }
    })();
    handled.unwrap_or_else(|response| response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_link_slot_version_output() {
        assert_eq!(parse_version_output("linkSlot 0.0.2\n"), "0.0.2");
        assert_eq!(parse_version_output("v1.2.3\n"), "1.2.3");
        assert_eq!(parse_version_output("some error"), "some error");
        assert_eq!(parse_version_output(""), "unknown");
    }

    #[test]
    fn sync_field_validation_rejects_separators() {
        assert!(validate_sync_field("game", "宝可梦").is_ok());
        assert!(validate_sync_field("game", "a/b").is_err());
        assert!(validate_sync_field("game", "..").is_err());
        assert!(validate_sync_field("game", "").is_err());
    }

    #[test]
    fn read_caches_skips_corrupt_files_and_flags_remote_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cache_dir = root.join("linkSlot/Config/cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join("good.json"),
            r#"{"unit":{"platform":"GBA","emulator":"mgba","game":"宝可梦"},
                 "version":3,"synced_at":"2026-10-21T10:00:00Z",
                 "remote":{"url":"https://nas/dav","remote_root":"/linkSlot"}}"#
                .as_bytes(),
        )
        .unwrap();
        std::fs::write(cache_dir.join("bad.json"), b"not json").unwrap();
        std::fs::write(cache_dir.join("other.json"), br#"{"unit":{}}"#).unwrap();

        let mut config = crate::linkcfg::LinkConfig::default();
        config.webdav.url = "https://nas/dav".to_string();
        config.webdav.remote_root = "/linkSlot".to_string();
        let caches = read_caches(root, &config);
        assert_eq!(caches.len(), 1);
        assert_eq!(caches[0]["game"], "宝可梦");
        assert_eq!(caches[0]["version"], 3);
        assert_eq!(caches[0]["remote_match"], true);

        config.webdav.url = "https://other/dav".to_string();
        let caches = read_caches(root, &config);
        assert_eq!(caches[0]["remote_match"], false);
    }

    #[test]
    fn a_missing_remote_root_gets_an_actionable_message() {
        let value = json!({ "remote_root": "/linkSlot" });
        let output = "LINKSLOT_RESULT=error\nLINKSLOT_DETAIL=远程根目录不存在（HTTP 404），请检查 webdav.remote_root\n";
        let detail = cli_detail(output).expect("detail");
        assert_eq!(
            detail,
            "远程根目录不存在（HTTP 404），请检查 webdav.remote_root"
        );
        let hint = test_hint(1, Some(&detail), &value, Path::new("."));
        assert!(hint.contains("远程根目录「/linkSlot」不存在"), "{hint}");
        assert!(hint.contains("创建这个文件夹"), "{hint}");
    }

    #[test]
    fn a_form_test_config_uses_the_typed_values_and_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let value = json!({
            "url": "https://form.example/dav",
            "remote_root": "/form",
            "username": "form-user",
            "password": "form-pass",
            "ca_cert": "",
            "insecure": true
        });
        let config = write_test_config(root.path(), &value).expect("test config");
        let text = std::fs::read_to_string(&config.path).unwrap();
        assert!(text.contains("https://form.example/dav"), "{text}");
        assert!(text.contains("form-user"), "{text}");
        assert!(text.contains("form-pass"), "{text}");
        assert!(text.contains("insecure = true"), "{text}");
        let dir = config.dir.clone();
        drop(config);
        assert!(!dir.exists(), "temporary test config was not removed");
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_route_table_has_no_duplicates() {
        let mut seen = BTreeSet::new();
        for route in ROUTES {
            let key = (format!("{:?}", route.method), route.path);
            assert!(seen.insert(key.clone()), "duplicate route {key:?}");
        }
        assert!(!ROUTES.is_empty());
    }

    #[test]
    fn every_route_resolves_to_itself() {
        for route in ROUTES {
            let found = lookup(route.method, route.path)
                .unwrap_or_else(|| panic!("{} {} does not resolve", format!("{:?}", route.method), route.path));
            assert_eq!(found.path, route.path);
        }
    }

    #[test]
    fn a_known_path_with_another_method_is_a_405_not_a_404() {
        for route in ROUTES {
            let free = [Method::Get, Method::Post, Method::Put, Method::Head]
                .into_iter()
                .find(|method| lookup(*method, route.path).is_none());
            let Some(method) = free else { continue };
            assert!(
                lookup(method, route.path).is_none() && known_path(route.path),
                "{} {} should be method-not-allowed",
                format!("{method:?}"),
                route.path
            );
        }
    }

    #[test]
    fn an_unknown_path_is_not_known() {
        assert!(!known_path("/api/no-such-route"));
        assert!(lookup(Method::Get, "/api/no-such-route").is_none());
    }
}