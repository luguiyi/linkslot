//! `/api/files/**`: the card file browser shown on the Web management page.
//!
//! Paths are always relative to the card root.  Absolute paths, `.`/`..`,
//! backslashes and symlinks that escape the card are rejected.  `Config/` is
//! hidden and read-protected because it contains WebDAV credentials; `System/`
//! is readable but write-protected so a file-manager mistake cannot remove the
//! launch scripts.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::web::http::{self, Request, Response};
use crate::web::Shared;

const MAX_ENTRIES: usize = 2000;
const MAX_PATH_BYTES: usize = 1024;
const MAX_NAME_BYTES: usize = 255;
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// Game packs live under `Exports/` and hold a ROM plus a demo video, so they
/// are allowed to be bigger than an ordinary file download.
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const PROTECTED_READ: &[&str] = &["linkSlot", "Config"];
const PROTECTED_WRITE: [&[&str]; 2] = [&["linkSlot", "Config"], &["System"]];

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserPath {
    segments: Vec<String>,
}

impl BrowserPath {
    fn parse(raw: &str) -> Result<BrowserPath, Response> {
        if raw.starts_with('/') {
            return Err(Response::error(400, "不允许绝对路径"));
        }
        let raw = raw.trim_end_matches('/');
        if raw.is_empty() {
            return Ok(BrowserPath {
                segments: Vec::new(),
            });
        }
        if raw.len() > MAX_PATH_BYTES {
            return Err(Response::error(400, "path 参数过长"));
        }
        if raw.contains('\0') || raw.contains('\\') {
            return Err(Response::error(400, "路径包含非法字符"));
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Err(Response::error(400, "路径包含非法目录名"));
            }
            if segment.contains(':') || segment.chars().any(char::is_control) {
                return Err(Response::error(400, "路径包含非法字符"));
            }
            segments.push(segment.to_string());
        }
        Ok(BrowserPath { segments })
    }

    fn rel(&self) -> String {
        self.segments.join("/")
    }

    fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    fn first(&self) -> &str {
        self.segments.first().map(String::as_str).unwrap_or("")
    }

    /// 路径是否以 `prefix` 开头（逐段、忽略大小写）。
    fn starts_with(&self, prefix: &[&str]) -> bool {
        self.segments.len() >= prefix.len()
            && self
                .segments
                .iter()
                .zip(prefix)
                .all(|(segment, want)| segment.eq_ignore_ascii_case(want))
    }

    fn read_protected(&self) -> bool {
        self.starts_with(PROTECTED_READ)
    }

    fn write_protected(&self) -> bool {
        PROTECTED_WRITE.iter().any(|prefix| self.starts_with(prefix))
    }

    fn parent_rel(&self) -> Option<String> {
        if self.is_root() {
            None
        } else {
            Some(self.segments[..self.segments.len() - 1].join("/"))
        }
    }
}

fn root(shared: &Shared) -> Result<PathBuf, Response> {
    shared
        .root
        .canonicalize()
        .map_err(|_| Response::error(500, "存储卡根目录不可用"))
}

fn resolve_existing(shared: &Shared, path: &BrowserPath) -> Result<PathBuf, Response> {
    let root = root(shared)?;
    if path.is_root() {
        return Ok(root);
    }
    let candidate = path
        .segments
        .iter()
        .fold(root.clone(), |current, segment| current.join(segment));
    if let Ok(metadata) = std::fs::symlink_metadata(&candidate) {
        if metadata.file_type().is_symlink() {
            return Err(Response::error(403, "不允许通过符号链接访问"));
        }
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| Response::error(404, "文件或目录不存在"))?;
    if !canonical.starts_with(&root) {
        return Err(Response::error(403, "路径越界"));
    }
    Ok(canonical)
}

fn resolve_dir(shared: &Shared, path: &BrowserPath) -> Result<PathBuf, Response> {
    if path.read_protected() {
        return Err(Response::error(403, "linkSlot/Config 目录包含凭据，禁止浏览"));
    }
    let target = resolve_existing(shared, path)?;
    if !target.is_dir() {
        return Err(Response::error(404, "目录不存在"));
    }
    Ok(target)
}

fn safe_name(raw: &str) -> Result<String, Response> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Response::error(400, "名称不能为空"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(Response::error(400, "名称过长"));
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(Response::error(400, "名称包含非法字符"));
    }
    if name.contains('\0') || name.contains(':') || name.chars().any(char::is_control) {
        return Err(Response::error(400, "名称包含非法字符"));
    }
    Ok(name.to_string())
}

fn guard_write(shared: &Shared, request: &Request) -> Result<(), Response> {
    if request.header("x-slot-web") != Some("1") {
        return Err(Response::error(
            403,
            "写操作需要 X-Slot-Web: 1 请求头（请从设备管理页面操作）",
        ));
    }
    if !shared.writable() {
        return Err(Response::error(409, "设备正在游戏中或同步中，暂时不能写入"));
    }
    Ok(())
}

/// A log download is renamed after the machine and its local day, so two
/// devices' logs never overwrite each other:
/// `Logs/linkSlot.log` arrives as `linkslot_rg34xx_20261029.log`.
fn download_name(shared: &Shared, browser: &BrowserPath, name: &str) -> String {
    let under_logs = browser
        .segments
        .windows(2)
        .any(|pair| pair[0].eq_ignore_ascii_case("linkSlot") && pair[1].eq_ignore_ascii_case("Logs"));
    if !under_logs {
        return name.to_string();
    }
    let device = slot_device::DeviceInfo::current().slug();
    log_name(name, &device, &day_stamp(shared)).unwrap_or_else(|| name.to_string())
}

/// `linkSlot.log` + `rg34xx` + `20261029` -> `linkslot_rg34xx_20261029.log`.
fn log_name(name: &str, device: &str, day: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".log")?;
    if stem.is_empty() {
        return None;
    }
    Some(format!("{stem}_{device}_{day}.log"))
}

/// The card's local calendar day as `YYYYMMDD` (the clock offset lives in
/// `linkSlot/Config/slot.state`).
fn day_stamp(shared: &Shared) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let offset = i64::from(slot_store::read_slot_state(&shared.root).utc_offset_min);
    let stamp = slot_store::format_stamp(secs + offset * 60);
    stamp
        .get(..10)
        .map(|day| day.replace('-', ""))
        .unwrap_or_default()
}

fn query(request: &Request, name: &str) -> Result<Option<String>, Response> {
    http::query_param(request, name).map_err(|error| Response::error(error.status, error.message))
}

fn json_field(value: &Value, key: &str) -> Result<String, Response> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Response::error(400, format!("缺少 {key} 参数")))
}

fn modified_seconds(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn entry_value(relative: &str, name: &str, metadata: &std::fs::Metadata) -> Value {
    let is_dir = metadata.is_dir();
    json!({
        "name": name,
        "path": relative,
        "is_dir": is_dir,
        "size": if is_dir { 0 } else { metadata.len() },
        "modified": modified_seconds(metadata),
        "ext": if is_dir { String::new() } else { extension(name) },
    })
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn image_content_type(name: &str) -> Option<&'static str> {
    match extension(name).as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn content_type(name: &str) -> &'static str {
    if let Some(kind) = image_content_type(name) {
        return kind;
    }
    match extension(name).as_str() {
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "lua" => "text/plain; charset=utf-8",
        "sav" | "srm" | "state" => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

/// `GET /api/files?path=`: list one directory.
pub(crate) fn list(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let raw = query(request, "path")?.unwrap_or_default();
        let browser = BrowserPath::parse(&raw)?;
        let dir = resolve_dir(shared, &browser)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        let reader = std::fs::read_dir(&dir).map_err(|_| Response::error(500, "无法读取目录"))?;
        for entry in reader.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if browser.is_root() && name.eq_ignore_ascii_case(slot_model::layout::APP_ID) {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            // Symlinks could point outside the card; the browser never follows
            // them, so they are simply omitted.
            if metadata.file_type().is_symlink() {
                continue;
            }
            if !metadata.is_dir() && !metadata.is_file() {
                continue;
            }
            let relative = if browser.is_root() {
                name.clone()
            } else {
                format!("{}/{}", browser.rel(), name)
            };
            entries.push(entry_value(&relative, &name, &metadata));
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
        }
        entries.sort_by(|left, right| {
            let left_dir = left["is_dir"].as_bool().unwrap_or(false);
            let right_dir = right["is_dir"].as_bool().unwrap_or(false);
            right_dir.cmp(&left_dir).then_with(|| {
                left["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&right["name"].as_str().unwrap_or("").to_lowercase())
            })
        });
        Ok(Response::json(
            200,
            json!({
                "root": "/",
                "path": browser.rel(),
                "parent": browser.parent_rel(),
                "entries": entries,
                "truncated": truncated,
                "max_entries": MAX_ENTRIES,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// Exported game packs are bigger than a plain file, so they get their own cap.
fn download_cap(browser: &BrowserPath) -> u64 {
    let is_package = browser.first().eq_ignore_ascii_case("Exports")
        && browser
            .segments
            .last()
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".zip"));
    if is_package {
        MAX_PACKAGE_BYTES
    } else {
        MAX_DOWNLOAD_BYTES
    }
}

/// `GET /api/files/download?path=`: download one file as an attachment.
pub(crate) fn download(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let raw = query(request, "path")?.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let browser = BrowserPath::parse(&raw)?;
        if browser.is_root() || browser.read_protected() {
            return Err(Response::error(403, "该路径不可下载"));
        }
        let target = resolve_existing(shared, &browser)?;
        if !target.is_file() {
            return Err(Response::error(400, "只能下载文件，不能下载目录"));
        }
        let metadata =
            std::fs::metadata(&target).map_err(|_| Response::error(404, "文件不存在"))?;
        if metadata.len() > download_cap(&browser) {
            return Err(Response::error(413, "文件过大，无法通过网页下载"));
        }
        let bytes = std::fs::read(&target).map_err(|_| Response::error(500, "读取文件失败"))?;
        let name = browser.segments.last().cloned().unwrap_or_default();
        let name = download_name(shared, &browser, &name);
        Ok(Response::new(200, content_type(&name), bytes).with_header(
            "Content-Disposition",
            http::content_disposition("attachment", &name),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/files/view?path=`: inline image preview for the browser dialog.
pub(crate) fn view(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let raw = query(request, "path")?.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let browser = BrowserPath::parse(&raw)?;
        if browser.is_root() || browser.read_protected() {
            return Err(Response::error(403, "该路径不可预览"));
        }
        let name = browser.segments.last().cloned().unwrap_or_default();
        let Some(kind) = image_content_type(&name) else {
            return Err(Response::error(415, "只支持预览图片文件"));
        };
        let target = resolve_existing(shared, &browser)?;
        if !target.is_file() {
            return Err(Response::error(404, "文件不存在"));
        }
        let metadata =
            std::fs::metadata(&target).map_err(|_| Response::error(404, "文件不存在"))?;
        if metadata.len() > MAX_DOWNLOAD_BYTES {
            return Err(Response::error(413, "图片过大，无法预览"));
        }
        let bytes = std::fs::read(&target).map_err(|_| Response::error(500, "读取文件失败"))?;
        Ok(Response::new(200, kind, bytes).with_header("Cache-Control", "no-cache"))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `GET /api/files/text?path=`: bounded UTF-8 text preview.
pub(crate) fn text(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        let raw = query(request, "path")?.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let browser = BrowserPath::parse(&raw)?;
        if browser.is_root() || browser.read_protected() {
            return Err(Response::error(403, "该路径不可预览"));
        }
        let target = resolve_existing(shared, &browser)?;
        if !target.is_file() {
            return Err(Response::error(400, "只能预览文件"));
        }
        let metadata =
            std::fs::metadata(&target).map_err(|_| Response::error(404, "文件不存在"))?;
        if metadata.len() > MAX_TEXT_BYTES {
            return Err(Response::error(413, "文件过大，无法在线预览"));
        }
        let bytes = std::fs::read(&target).map_err(|_| Response::error(500, "读取文件失败"))?;
        let binary = bytes.iter().take(4096).any(|byte| *byte == 0);
        let name = browser.segments.last().cloned().unwrap_or_default();
        Ok(Response::json(
            200,
            json!({
                "path": browser.rel(),
                "name": name,
                "content": String::from_utf8_lossy(&bytes),
                "size": metadata.len(),
                "binary": binary,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/files/upload?path=`: raw request body written to one file.
pub(crate) fn upload(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let raw = query(request, "path")?.ok_or_else(|| Response::error(400, "缺少 path 参数"))?;
        let browser = BrowserPath::parse(&raw)?;
        if browser.is_root() || browser.write_protected() {
            return Err(Response::error(403, "该目录禁止写入"));
        }
        let name = safe_name(browser.segments.last().map(String::as_str).unwrap_or(""))?;
        let parent = BrowserPath {
            segments: browser.segments[..browser.segments.len() - 1].to_vec(),
        };
        let parent = resolve_dir(shared, &parent)?;
        let target = parent.join(&name);
        if target.is_dir() {
            return Err(Response::error(400, "目标路径是目录"));
        }
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("上传内容超过 {mb}MB 限制")));
        }
        crate::web::paths::atomic_upload(&target, &request.body)
            .map_err(|_| Response::error(500, "写入文件失败"))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "path": browser.rel(), "size": request.body.len() }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/files/mkdir`: create one directory below the current directory.
pub(crate) fn mkdir(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let parent = BrowserPath::parse(&json_field(&value, "path")?)?;
        if parent.write_protected() {
            return Err(Response::error(403, "该目录禁止写入"));
        }
        let name = safe_name(&json_field(&value, "name")?)?;
        let parent = resolve_dir(shared, &parent)?;
        let target = parent.join(&name);
        if target.exists() {
            return Err(Response::error(409, "同名文件或目录已存在"));
        }
        std::fs::create_dir(&target).map_err(|_| Response::error(500, "创建目录失败"))?;
        shared.touch_library_changed();
        Ok(Response::json(
            200,
            json!({ "ok": true, "path": target.to_string_lossy() }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/files/rename`: rename one entry in place.
pub(crate) fn rename(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let browser = BrowserPath::parse(&json_field(&value, "path")?)?;
        if browser.is_root() || browser.write_protected() {
            return Err(Response::error(403, "该路径禁止修改"));
        }
        let name = safe_name(&json_field(&value, "name")?)?;
        let target = resolve_existing(shared, &browser)?;
        let parent = target
            .parent()
            .ok_or_else(|| Response::error(400, "无法取得父目录"))?;
        let destination = parent.join(&name);
        if destination.exists() {
            return Err(Response::error(409, "同名文件或目录已存在"));
        }
        std::fs::rename(&target, &destination).map_err(|_| Response::error(500, "改名失败"))?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/files/move`: move one entry to another directory.
pub(crate) fn move_entry(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let source = BrowserPath::parse(&json_field(&value, "path")?)?;
        let destination = BrowserPath::parse(&json_field(&value, "dest")?)?;
        if source.is_root() || source.write_protected() || destination.write_protected() {
            return Err(Response::error(403, "该路径禁止移动"));
        }
        let source_path = resolve_existing(shared, &source)?;
        let destination_dir = resolve_dir(shared, &destination)?;
        let name = source_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| Response::error(400, "无法取得文件名"))?;
        let target = destination_dir.join(name);
        if source_path.is_dir() && target.starts_with(&source_path) {
            return Err(Response::error(400, "不能把目录移动到自身内部"));
        }
        if target.exists() {
            return Err(Response::error(409, "目标位置已有同名文件或目录"));
        }
        std::fs::rename(&source_path, &target).map_err(|_| Response::error(500, "移动失败"))?;
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true })))
    })();
    handled.unwrap_or_else(|response| response)
}

/// `POST /api/files/delete`: delete one file or directory.  Non-empty
/// directories require `recursive: true` from the confirmation dialog.
pub(crate) fn delete(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        guard_write(shared, request)?;
        let value = http::json_body(request)
            .map_err(|error| Response::error(error.status, error.message))?;
        let browser = BrowserPath::parse(&json_field(&value, "path")?)?;
        if browser.is_root() || browser.write_protected() {
            return Err(Response::error(403, "该路径禁止删除"));
        }
        let target = resolve_existing(shared, &browser)?;
        let recursive = value
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if target.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&target)
                    .map_err(|_| Response::error(500, "删除目录失败"))?;
            } else {
                std::fs::remove_dir(&target)
                    .map_err(|_| Response::error(409, "目录非空；请确认后使用递归删除"))?;
            }
        } else {
            std::fs::remove_file(&target).map_err(|_| Response::error(500, "删除文件失败"))?;
        }
        shared.touch_library_changed();
        Ok(Response::json(200, json!({ "ok": true })))
    })();
    handled.unwrap_or_else(|response| response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::http::Method;
    use crate::web::test_support::{json_request, request, test_shared, with_slot_header};
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, Arc<Shared>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Games/GBA")).unwrap();
        std::fs::write(dir.path().join("Games/GBA/game.gba"), b"rom").unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        std::fs::write(dir.path().join("linkSlot/Config/config.toml"), b"secret").unwrap();
        std::fs::create_dir_all(dir.path().join("System")).unwrap();
        std::fs::write(dir.path().join("README.txt"), b"hello").unwrap();
        let shared = test_shared(dir.path().to_path_buf());
        (dir, shared)
    }

    fn list(shared: &Arc<Shared>, path: &str) -> Response {
        let query = format!("path={}", crate::web::util::encode_query(path));
        super::list(shared, &request(Method::Get, "/api/files", &query))
    }

    fn body_json(response: &Response) -> Value {
        serde_json::from_slice(&response.body).unwrap()
    }

    #[test]
    fn lists_root_without_the_protected_config_directory() {
        let (_dir, shared) = fixture();
        let response = list(&shared, "");
        assert_eq!(response.status, 200);
        let value = body_json(&response);
        assert_eq!(value["path"], "");
        assert_eq!(value["parent"], Value::Null);
        let names: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert!(names.contains(&"Games"));
        assert!(names.contains(&"README.txt"));
        assert!(!names.contains(&"Config"));
    }

    #[test]
    fn lists_a_subdirectory_and_rejects_traversal() {
        let (_dir, shared) = fixture();
        let response = list(&shared, "Games/GBA");
        assert_eq!(response.status, 200);
        let value = body_json(&response);
        assert_eq!(value["parent"], "Games");
        assert_eq!(value["entries"][0]["name"], "game.gba");

        for bad in ["../Secret", "/etc", "Games/../../Secret", "linkSlot/Config"] {
            let response = list(&shared, bad);
            assert!(
                response.status == 400 || response.status == 403,
                "{bad} => {}",
                response.status
            );
        }
    }

    #[test]
    fn exported_packages_get_a_bigger_download_cap() {
        let package = BrowserPath::parse("Exports/龙珠大冒险.zip").unwrap();
        assert_eq!(download_cap(&package), MAX_PACKAGE_BYTES);
        let rom = BrowserPath::parse("Games/GBA/Test.gba").unwrap();
        assert_eq!(download_cap(&rom), MAX_DOWNLOAD_BYTES);
        let elsewhere = BrowserPath::parse("Imports/anything.zip").unwrap();
        assert_eq!(download_cap(&elsewhere), MAX_DOWNLOAD_BYTES);
    }

    #[test]
    fn download_and_text_preview_are_read_only() {
        let (_dir, shared) = fixture();
        let query = "path=README.txt";
        let response =
            super::download(&shared, &request(Method::Get, "/api/files/download", query));
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Disposition" && value.starts_with("attachment")));

        let response = super::text(&shared, &request(Method::Get, "/api/files/text", query));
        assert_eq!(response.status, 200);
        assert_eq!(body_json(&response)["content"], "hello");

        let response = super::download(
            &shared,
            &request(
                Method::Get,
                "/api/files/download",
                "path=linkSlot/Config/config.toml",
            ),
        );
        assert_eq!(response.status, 403);
    }

    #[test]
    fn upload_mkdir_rename_move_and_delete_require_the_write_guard() {
        let (dir, shared) = fixture();
        let upload_query = "path=Games/GBA/new.bin";
        let mut upload = request(Method::Post, "/api/files/upload", upload_query);
        upload.body = b"new".to_vec();
        assert_eq!(super::upload(&shared, &upload).status, 403);
        let upload = with_slot_header(upload);
        assert_eq!(super::upload(&shared, &upload).status, 200);
        assert_eq!(
            std::fs::read(dir.path().join("Games/GBA/new.bin")).unwrap(),
            b"new"
        );

        let mkdir = json_request(
            Method::Post,
            "/api/files/mkdir",
            json!({ "path": "Games/GBA", "name": "sub" }),
        );
        assert_eq!(super::mkdir(&shared, &with_slot_header(mkdir)).status, 200);
        assert!(dir.path().join("Games/GBA/sub").is_dir());

        let rename = json_request(
            Method::Post,
            "/api/files/rename",
            json!({ "path": "Games/GBA/new.bin", "name": "renamed.bin" }),
        );
        assert_eq!(
            super::rename(&shared, &with_slot_header(rename)).status,
            200
        );
        assert!(dir.path().join("Games/GBA/renamed.bin").is_file());

        let moved = json_request(
            Method::Post,
            "/api/files/move",
            json!({ "path": "Games/GBA/renamed.bin", "dest": "Games" }),
        );
        assert_eq!(
            super::move_entry(&shared, &with_slot_header(moved)).status,
            200
        );
        assert!(dir.path().join("Games/renamed.bin").is_file());

        let delete = json_request(
            Method::Post,
            "/api/files/delete",
            json!({ "path": "Games/renamed.bin" }),
        );
        assert_eq!(
            super::delete(&shared, &with_slot_header(delete)).status,
            200
        );
        assert!(!dir.path().join("Games/renamed.bin").exists());
    }

    #[test]
    fn routes_files_endpoints() {
        let (_dir, shared) = fixture();
        let response = crate::web::route(&shared, &request(Method::Get, "/api/files", "path="));
        assert_eq!(response.status, 200);
    }

    #[test]
    fn system_writes_and_config_reads_are_refused() {
        let (_dir, shared) = fixture();
        let delete = json_request(
            Method::Post,
            "/api/files/delete",
            json!({ "path": "System" }),
        );
        assert_eq!(
            super::delete(&shared, &with_slot_header(delete)).status,
            403
        );

        let mkdir = json_request(
            Method::Post,
            "/api/files/mkdir",
            json!({ "path": "linkSlot/Config", "name": "x" }),
        );
        assert_eq!(super::mkdir(&shared, &with_slot_header(mkdir)).status, 403);
    }

    #[test]
    fn a_log_download_is_named_after_the_machine_and_the_day() {
        assert_eq!(
            super::log_name("linkSlot.log", "rg34xx", "20261029").as_deref(),
            Some("linkslot_rg34xx_20261029.log")
        );
        assert_eq!(
            super::log_name("slot-library.LOG", "g34xx", "20260101").as_deref(),
            Some("slot-library_g34xx_20260101.log")
        );
        assert_eq!(super::log_name("linkSlot.txt", "rg34xx", "20261029"), None);
        assert_eq!(super::log_name(".log", "rg34xx", "20261029"), None);

        // The local day is an eight digit stamp.
        let (_dir, shared) = fixture();
        let day = super::day_stamp(&shared);
        assert_eq!(day.len(), 8, "{day}");
        assert!(day.bytes().all(|b| b.is_ascii_digit()), "{day}");

        let (dir, shared) = fixture();
        std::fs::create_dir_all(dir.path().join("linkSlot/Logs")).unwrap();
        std::fs::write(dir.path().join("linkSlot/Logs/linkSlot.log"), b"log line\n").unwrap();
        let response = super::download(
            &shared,
            &request(Method::Get, "/api/files/download", "path=linkSlot/Logs/linkSlot.log"),
        );
        assert_eq!(response.status, 200);
        let disposition = response
            .headers
            .iter()
            .find(|(name, _)| name == "Content-Disposition")
            .map(|(_, value)| value.clone())
            .expect("no Content-Disposition");
        assert!(
            disposition.contains("linkslot_") && disposition.contains(".log"),
            "the log was not renamed: {disposition}"
        );
        // A normal file keeps its own name.
        let response = super::download(
            &shared,
            &request(Method::Get, "/api/files/download", "path=README.txt"),
        );
        let disposition = response
            .headers
            .iter()
            .find(|(name, _)| name == "Content-Disposition")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(
            disposition.contains("filename=\"README.txt\""),
            "{disposition}"
        );
    }
}
