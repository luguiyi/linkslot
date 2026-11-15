//! Read-only mod UI assets and the text-file bridge used by mod-owned pages.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::ModError;
use crate::path;
use crate::store;

/// A single editable text file is deliberately small; Web UI assets are served
/// through their own static route instead.
pub const MAX_TEXT_FILE_BYTES: usize = 128 * 1024;
/// Maximum number of mod tree entries returned to the Web UI.
pub const MAX_LISTED_FILES: usize = 256;
/// Maximum number of directory levels returned by `list_files`.
const MAX_LIST_DEPTH: usize = 8;
/// Maximum size of one static mod UI asset.
pub const MAX_UI_ASSET_BYTES: usize = 2 * 1024 * 1024;
/// Maximum size of a chooser sticker PNG (the frontend caps it the same way).
pub const MAX_STICKER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModFileEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Text files can be opened by the bridge; managed files stay read-only.
    pub editable: bool,
    /// A hint for the Web UI when another editor owns the file.
    pub managed_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModFileDetail {
    pub path: String,
    pub content: String,
    pub revision: String,
    pub editable: bool,
    pub size: u64,
}

/// A mod's own Web UI entry point, if present.
pub fn has_web_ui(root: &Path, id: &str) -> bool {
    web_ui_path(root, id).is_some()
}

/// Path to `mods/<id>/web/index.html`, only when it is a regular non-symlink
/// file below the mod directory.
pub fn web_ui_path(root: &Path, id: &str) -> Option<PathBuf> {
    let mod_dir = safe_mod_dir(root, id).ok()?;
    let path = mod_dir.join("web").join("index.html");
    regular_file(&path).then_some(path)
}

/// Read a static file below `mods/<id>/web/`. An empty path serves index.html.
///
/// This route is used by the sandboxed iframe; it deliberately does not expose
/// any file outside the current mod's `web/` directory.
pub fn read_ui_asset(root: &Path, id: &str, relative: &str) -> Result<Vec<u8>, ModError> {
    let mod_dir = safe_mod_dir(root, id)?;
    let relative = relative.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let segments = validate_relative(relative)?;
    if segments.first().map(String::as_str) != Some("web") {
        return Err(ModError::Invalid("只能读取 mod 的 web/ 资源".to_string()));
    }
    let asset_segments = &segments[1..];
    if asset_segments.is_empty() {
        return Err(ModError::NotFound("mod UI 资源不存在".to_string()));
    }
    let web_root = mod_dir.join("web");
    let target = asset_segments
        .iter()
        .fold(web_root, |base, segment| base.join(segment));
    read_asset(&mod_dir, &target)
}

/// List files and directories below one mod. Hidden/trash paths and symlinks
/// are skipped. Ordering is stable by relative path.
pub fn list_files(root: &Path, id: &str) -> Result<Vec<ModFileEntry>, ModError> {
    let mod_dir = safe_mod_dir(root, id)?;
    let mut out = Vec::new();
    walk(&mod_dir, &mod_dir, 0, &mut out)?;
    out.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(out)
}

fn walk(
    mod_dir: &Path,
    directory: &Path,
    depth: usize,
    out: &mut Vec<ModFileEntry>,
) -> Result<(), ModError> {
    if depth > MAX_LIST_DEPTH || out.len() >= MAX_LISTED_FILES {
        return Ok(());
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| ModError::Io(format!("无法读取 {}：{error}", directory.display())))?;
    let mut children: Vec<_> = entries.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if out.len() >= MAX_LISTED_FILES {
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let relative = match path.strip_prefix(mod_dir) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if file_type.is_dir() {
            out.push(ModFileEntry {
                path: relative,
                is_dir: true,
                size: 0,
                editable: false,
                managed_by: None,
            });
            walk(mod_dir, &path, depth + 1, out)?;
        } else if file_type.is_file() {
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            let editable = is_editable(&relative);
            out.push(ModFileEntry {
                path: relative.clone(),
                is_dir: false,
                size,
                editable,
                managed_by: managed_by(&relative, editable),
            });
        }
    }
    Ok(())
}

/// Read one UTF-8 text file with an optimistic revision.
pub fn read_text_file(
    root: &Path,
    mod_id: &str,
    relative: &str,
) -> Result<ModFileDetail, ModError> {
    let mod_dir = safe_mod_dir(root, mod_id)?;
    let segments = validate_relative(relative)?;
    let path = segments
        .iter()
        .fold(mod_dir.clone(), |base, segment| base.join(segment));
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| ModError::NotFound(format!("文件 {relative} 不存在")))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(ModError::Invalid("只能读取普通文本文件".to_string()));
    }
    if meta.len() > MAX_TEXT_FILE_BYTES as u64 {
        return Err(ModError::TooLarge(format!(
            "mod 文本文件超过 {} KiB 限制",
            MAX_TEXT_FILE_BYTES / 1024
        )));
    }
    ensure_inside(&mod_dir, &path)?;
    let bytes = std::fs::read(&path)
        .map_err(|error| ModError::Io(format!("无法读取 {}：{error}", path.display())))?;
    let content = String::from_utf8(bytes.clone())
        .map_err(|_| ModError::Invalid("mod 文件不是 UTF-8 文本".to_string()))?;
    Ok(ModFileDetail {
        path: relative.to_string(),
        content,
        revision: revision_of(&bytes),
        editable: is_editable(relative),
        size: bytes.len() as u64,
    })
}

/// Save one UTF-8 text file.
///
/// Existing files require the current revision. A missing file may only be
/// created as `config.json`; this is the intentional bridge for older mods.
pub fn save_text_file(
    root: &Path,
    mod_id: &str,
    relative: &str,
    content: &str,
    revision: Option<&str>,
) -> Result<ModFileDetail, ModError> {
    let mod_dir = safe_mod_dir(root, mod_id)?;
    let segments = validate_relative(relative)?;
    let path = segments
        .iter()
        .fold(mod_dir.clone(), |base, segment| base.join(segment));
    let bytes = content.as_bytes();
    if bytes.len() > MAX_TEXT_FILE_BYTES {
        return Err(ModError::TooLarge(format!(
            "mod 文本文件超过 {} KiB 限制",
            MAX_TEXT_FILE_BYTES / 1024
        )));
    }
    if !is_editable(relative) {
        return Err(ModError::Invalid(format!("文件 {relative} 不允许编辑")));
    }
    let exists = std::fs::symlink_metadata(&path).is_ok();
    if exists {
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|error| ModError::Io(format!("无法检查 {}：{error}", path.display())))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(ModError::Invalid("目标不是普通文本文件".to_string()));
        }
        ensure_inside(&mod_dir, &path)?;
        let expected = revision
            .ok_or_else(|| ModError::Conflict("文件已存在，保存时必须携带 revision".to_string()))?;
        let old = std::fs::read(&path)
            .map_err(|error| ModError::Io(format!("无法读取 {}：{error}", path.display())))?;
        if revision_of(&old) != expected {
            return Err(ModError::Conflict(
                "文件已被其他页面修改，请刷新".to_string(),
            ));
        }
    } else {
        if revision.is_some() {
            return Err(ModError::Conflict(
                "新建文本文件不能携带 revision".to_string(),
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| ModError::Invalid("路径没有父目录".to_string()))?;
        std::fs::create_dir_all(parent)
            .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", parent.display())))?;
        ensure_inside(&mod_dir, &path)?;
    }
    if relative.to_ascii_lowercase().ends_with(".json") {
        serde_json::from_str::<serde_json::Value>(content)
            .map_err(|error| ModError::Invalid(format!("JSON 语法错误：{error}")))?;
    }
    store::atomic_write(&path, bytes)?;
    read_text_file(root, mod_id, relative)
}

/// Save a chooser sticker for a manual action. This is the one binary file the
/// Web mod importer may write: `sticker.png` at the mod root, or
/// `actions/<action-id>/sticker.png`; anything else stays text-only.
pub fn save_sticker_file(
    root: &Path,
    mod_id: &str,
    relative: &str,
    bytes: &[u8],
) -> Result<ModFileDetail, ModError> {
    if bytes.len() > MAX_STICKER_BYTES {
        return Err(ModError::TooLarge(format!(
            "mod 贴纸超过 {} MiB 限制",
            MAX_STICKER_BYTES / (1024 * 1024)
        )));
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ModError::Invalid("mod 贴纸必须是 PNG".to_string()));
    }
    let mod_dir = safe_mod_dir(root, mod_id)?;
    let segments = validate_relative(relative)?;
    let is_root = segments.len() == 1 && segments[0] == "sticker.png";
    let is_action = segments.len() == 3
        && segments[0] == "actions"
        && segments[2] == "sticker.png"
        && path::validate_id(&segments[1]).is_ok();
    if !is_root && !is_action {
        return Err(ModError::Invalid(
            "只有 sticker.png 可写为图片".to_string(),
        ));
    }
    let path = segments
        .iter()
        .fold(mod_dir.clone(), |base, segment| base.join(segment));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", parent.display())))?;
    }
    ensure_inside(&mod_dir, &path)?;
    store::atomic_write(&path, bytes)?;
    Ok(ModFileDetail {
        path: relative.to_string(),
        content: String::new(),
        revision: revision_of(bytes),
        editable: false,
        size: bytes.len() as u64,
    })
}

fn safe_mod_dir(root: &Path, id: &str) -> Result<PathBuf, ModError> {
    path::validate_id(id).map_err(ModError::Invalid)?;
    let dir = path::mod_dir(root, id);
    let meta = std::fs::symlink_metadata(&dir)
        .map_err(|_| ModError::NotFound(format!("mod {id} 不存在")))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(ModError::Invalid("mod 目录无效".to_string()));
    }
    std::fs::canonicalize(&dir)
        .map_err(|error| ModError::Io(format!("无法解析 {}：{error}", dir.display())))
}

fn validate_relative(relative: &str) -> Result<Vec<String>, ModError> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Err(ModError::Invalid("路径不能为空".to_string()));
    }
    if relative.len() > 240 {
        return Err(ModError::Invalid("路径过长".to_string()));
    }
    if relative.starts_with('/') || relative.contains('\\') {
        return Err(ModError::Invalid("路径必须是 mod 内的相对路径".to_string()));
    }
    let mut segments = Vec::new();
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(ModError::Invalid("路径不能包含 . 或 ..".to_string()));
        }
        if segment == "mod.json" {
            return Err(ModError::Invalid(
                "mod.json 不能通过文件 API 修改".to_string(),
            ));
        }
        if segment.starts_with('.') {
            return Err(ModError::Invalid("不能访问隐藏文件".to_string()));
        }
        if segment.chars().any(char::is_control) || segment.contains(':') || segment.contains('\0')
        {
            return Err(ModError::Invalid("路径包含非法字符".to_string()));
        }
        segments.push(segment.to_string());
    }
    Ok(segments)
}

fn ensure_inside(mod_dir: &Path, path: &Path) -> Result<(), ModError> {
    let parent = path
        .parent()
        .ok_or_else(|| ModError::Invalid("路径没有父目录".to_string()))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| ModError::Io(format!("无法解析 {}：{error}", parent.display())))?;
    if !parent.starts_with(mod_dir) {
        return Err(ModError::Invalid("路径越出当前 mod 目录".to_string()));
    }
    Ok(())
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_file() && !meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn read_asset(mod_dir: &Path, path: &Path) -> Result<Vec<u8>, ModError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| ModError::NotFound("mod UI 资源不存在".to_string()))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(ModError::Invalid("mod UI 资源必须是普通文件".to_string()));
    }
    if meta.len() > MAX_UI_ASSET_BYTES as u64 {
        return Err(ModError::TooLarge("mod UI 资源过大".to_string()));
    }
    ensure_inside(mod_dir, path)?;
    std::fs::read(path)
        .map_err(|error| ModError::Io(format!("无法读取 {}：{error}", path.display())))
}

fn has_text_extension(path: &str) -> bool {
    matches!(
        extension(path).as_deref(),
        Some(
            "json"
                | "lua"
                | "md"
                | "txt"
                | "toml"
                | "yml"
                | "yaml"
                | "cfg"
                | "ini"
                | "html"
                | "htm"
                | "css"
                | "js"
                | "mjs"
                | "svg"
        )
    )
}

fn is_editable(path: &str) -> bool {
    !path.eq_ignore_ascii_case("mod.json") && has_text_extension(path)
}

fn managed_by(path: &str, editable: bool) -> Option<String> {
    if !editable {
        return Some("只读或非文本文件".to_string());
    }
    if path.eq_ignore_ascii_case("mod.json") {
        return Some("mod 元数据".to_string());
    }
    if path.starts_with("actions/")
        && (path.ends_with("/action.json") || path.ends_with("/main.lua"))
    {
        return Some("动作编辑器".to_string());
    }
    if path == "web/index.html" {
        return Some("mod UI".to_string());
    }
    None
}

fn extension(path: &str) -> Option<String> {
    path.rsplit('.')
        .next()
        .filter(|extension| !extension.is_empty())
        .map(|extension| extension.to_ascii_lowercase())
}

fn revision_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"linkSlot-mod-file-v1\0");
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn demo(root: &Path) -> PathBuf {
        let dir = root.join("linkSlot/mods/demo");
        write(
            &dir.join("mod.json"),
            br#"{"schema":2,"name":"demo","version":"1.0.0"}"#,
        );
        write(&dir.join("config.json"), b"{}");
        write(&dir.join("README.md"), b"# demo\n");
        write(&dir.join("web/index.html"), b"<html></html>");
        write(&dir.join("actions/main/action.json"), b"{}");
        write(&dir.join("actions/main/main.lua"), b"return 1\n");
        dir
    }

    #[test]
    fn lists_and_edits_text_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = demo(tmp.path());
        assert!(has_web_ui(tmp.path(), "demo"));
        let files = list_files(tmp.path(), "demo").unwrap();
        assert!(files.iter().any(|file| file.path == "web/index.html"));
        assert!(files
            .iter()
            .any(|file| file.path == "config.json" && file.editable));
        assert!(files
            .iter()
            .any(|file| file.path == "mod.json" && !file.editable));
        assert!(files.iter().any(|file| file.path == "web" && file.is_dir));

        let detail = read_text_file(tmp.path(), "demo", "config.json").unwrap();
        assert_eq!(detail.content, "{}");
        let saved = save_text_file(
            tmp.path(),
            "demo",
            "config.json",
            "{\"a\":1}",
            Some(&detail.revision),
        )
        .unwrap();
        assert!(saved.content.contains("\"a\""));
        assert!(save_text_file(
            tmp.path(),
            "demo",
            "config.json",
            "{\"b\":2}",
            Some(&detail.revision)
        )
        .is_err());
        assert!(dir.join("config.json").is_file());
    }

    #[test]
    fn rejects_traversal_and_managed_or_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        demo(tmp.path());
        assert!(read_text_file(tmp.path(), "demo", "../mod.json").is_err());
        assert!(read_text_file(tmp.path(), "demo", "mod.json").is_err());
        assert!(save_text_file(tmp.path(), "demo", "mod.json", "{}", None).is_err());
        assert!(save_text_file(tmp.path(), "demo", "config.json", "{", None).is_err());
        assert!(read_text_file(tmp.path(), "demo", "web/index.html")
            .unwrap()
            .content
            .contains("html"));
    }

    #[test]
    fn reads_ui_assets_only_inside_web() {
        let tmp = tempfile::tempdir().unwrap();
        demo(tmp.path());
        let asset = read_ui_asset(tmp.path(), "demo", "web/index.html").unwrap();
        assert_eq!(asset, b"<html></html>");
        assert!(read_ui_asset(tmp.path(), "demo", "README.md").is_err());
        assert!(read_ui_asset(tmp.path(), "demo", "../mod.json").is_err());
    }
}

    #[test]
    fn a_sticker_is_the_only_binary_file_the_bridge_writes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("linkSlot/mods/demo/actions/sync")).unwrap();
        let png = b"\x89PNG\r\n\x1a\nsticker";
        let saved = super::save_sticker_file(tmp.path(), "demo", "sticker.png", png).unwrap();
        assert_eq!(saved.path, "sticker.png");
        assert_eq!(saved.size, png.len() as u64);
        assert!(!saved.editable);
        assert_eq!(
            std::fs::read(tmp.path().join("linkSlot/mods/demo/sticker.png")).unwrap(),
            png
        );
        super::save_sticker_file(tmp.path(), "demo", "actions/sync/sticker.png", png).unwrap();
        assert!(std::fs::read(tmp.path().join("linkSlot/mods/demo/actions/sync/sticker.png")).is_ok());
        assert!(super::save_sticker_file(tmp.path(), "demo", "logo.png", png).is_err());
        assert!(super::save_sticker_file(tmp.path(), "demo", "sticker.png", b"not a png").is_err());
    }
