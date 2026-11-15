//! Path safety for the save-file endpoints.
//!
//! Only relative paths below `States/` or `Saves/` are accepted.  Absolute
//! paths, `.`/`..` segments, backslashes, NUL bytes and anything that escapes
//! the card root through a symlink are rejected before a file is opened.

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const EXTENSIONS: [&str; 4] = ["state", "png", "sav", "srm"];
const ROOTS: [&str; 2] = ["States", "Saves"];
const MAX_PATH_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub struct PathReject {
    pub status: u16,
    pub message: String,
}

impl PathReject {
    pub fn new(status: u16, message: impl Into<String>) -> PathReject {
        PathReject {
            status,
            message: message.into(),
        }
    }
}

/// A validated, still-relative save path (`States/GBA/mgba/Game/resume.state`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafePath {
    pub segments: Vec<String>,
    pub rel: String,
}

impl SafePath {
    pub fn file_name(&self) -> &str {
        self.segments.last().map(String::as_str).unwrap_or("")
    }

    pub fn extension(&self) -> String {
        extension_of(self.file_name())
    }

    pub fn is_image(&self) -> bool {
        self.extension() == "png"
    }
}

/// Percent-decode a UTF-8 string (query values and request paths).
pub fn percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(());
                }
                let high = hex_value(bytes[index + 1]).ok_or(())?;
                let low = hex_value(bytes[index + 2]).ok_or(())?;
                out.push((high << 4) | low);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Validate a raw `path` query value and return its segments.
pub fn validate(raw: &str) -> Result<SafePath, PathReject> {
    if raw.is_empty() {
        return Err(PathReject::new(400, "缺少 path 参数"));
    }
    if raw.len() > MAX_PATH_BYTES {
        return Err(PathReject::new(400, "path 参数过长"));
    }
    if raw.contains('\0') || raw.contains('\\') {
        return Err(PathReject::new(400, "路径包含非法字符"));
    }
    if raw.starts_with('/') || Path::new(raw).is_absolute() {
        return Err(PathReject::new(400, "不允许绝对路径"));
    }
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        if segment.is_empty() {
            return Err(PathReject::new(400, "路径包含空目录名"));
        }
        if segment == "." || segment == ".." {
            return Err(PathReject::new(400, "不允许 . 或 .. 路径段"));
        }
        if segment.contains(':') {
            return Err(PathReject::new(400, "路径包含非法字符"));
        }
        if segment.chars().any(|ch| ch.is_control()) {
            return Err(PathReject::new(400, "路径包含控制字符"));
        }
        segments.push(segment.to_string());
    }
    let first = segments.first().map(String::as_str).unwrap_or("");
    if !ROOTS.contains(&first) {
        return Err(PathReject::new(403, "只允许访问 States/ 与 Saves/ 下的存档文件"));
    }
    if segments.len() < 2 {
        return Err(PathReject::new(400, "路径缺少文件名"));
    }
    let extension = extension_of(segments.last().map(String::as_str).unwrap_or(""));
    if !EXTENSIONS.contains(&extension.as_str()) {
        return Err(PathReject::new(
            400,
            format!("不允许的文件类型（仅支持 {}）", EXTENSIONS.join(" / ")),
        ));
    }
    let rel = segments.join("/");
    Ok(SafePath { segments, rel })
}

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Resolve a read path, canonicalising and re-checking that it stays inside
/// the card root (symlink escape protection).  Missing files are 404.
pub fn resolve_read(root: &Path, safe: &SafePath) -> Result<PathBuf, PathReject> {
    let canon_root = root
        .canonicalize()
        .map_err(|_| PathReject::new(500, "存储根目录不可用"))?;
    let candidate = safe
        .segments
        .iter()
        .fold(canon_root.clone(), |path, segment| path.join(segment));
    let canonical = candidate
        .canonicalize()
        .map_err(|_| PathReject::new(404, "文件不存在"))?;
    if !canonical.starts_with(&canon_root) {
        return Err(PathReject::new(403, "路径越界"));
    }
    check_top_component(&canonical, &canon_root, safe)?;
    if !canonical.is_file() {
        return Err(PathReject::new(404, "文件不存在"));
    }
    Ok(canonical)
}

fn check_top_component(canonical: &Path, canon_root: &Path, safe: &SafePath) -> Result<(), PathReject> {
    let Ok(relative) = canonical.strip_prefix(canon_root) else {
        return Err(PathReject::new(403, "路径越界"));
    };
    let mut components = relative.components();
    match components.next() {
        Some(Component::Normal(component)) if component.to_string_lossy() == safe.segments[0] => Ok(()),
        _ => Err(PathReject::new(403, "路径越界")),
    }
}

/// Resolve an upload target.  Missing parent directories are created below the
/// validated root; every existing ancestor is canonicalised and containment is
/// re-checked, so an in-card symlink cannot redirect a write outside.
pub fn resolve_write(root: &Path, safe: &SafePath) -> Result<PathBuf, PathReject> {
    let canon_root = root
        .canonicalize()
        .map_err(|_| PathReject::new(500, "存储根目录不可用"))?;
    let file_name = safe.file_name().to_string();
    let directories = &safe.segments[..safe.segments.len() - 1];
    let mut current = canon_root.clone();
    for segment in directories {
        let next = current.join(segment);
        match std::fs::symlink_metadata(&next) {
            Ok(_metadata) => {
                let canonical = next
                    .canonicalize()
                    .map_err(|_| PathReject::new(500, "无法解析目录路径"))?;
                if !canonical.starts_with(&canon_root) {
                    return Err(PathReject::new(403, "路径越界"));
                }
                if !canonical.is_dir() {
                    return Err(PathReject::new(400, "父路径不是目录"));
                }
                current = canonical;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&next)
                    .map_err(|_| PathReject::new(500, "无法创建存档目录"))?;
                let canonical = next
                    .canonicalize()
                    .map_err(|_| PathReject::new(500, "无法解析目录路径"))?;
                if !canonical.starts_with(&canon_root) {
                    return Err(PathReject::new(403, "路径越界"));
                }
                current = canonical;
            }
            Err(_) => return Err(PathReject::new(500, "无法访问存档目录")),
        }
    }
    check_top_component(&current, &canon_root, safe)?;
    let target = current.join(&file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&target) {
        if metadata.is_dir() {
            return Err(PathReject::new(400, "目标路径是目录"));
        }
    }
    Ok(target)
}

/// Write `body` to a hidden temp file next to `target`, then rename it into
/// place.  A crashed upload therefore never leaves a truncated save state.
pub fn atomic_upload(target: &Path, body: &[u8]) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "上传目标没有父目录".to_string())?;
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "上传目标缺少文件名".to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = parent.join(format!(".{file_name}.{}.{stamp}.tmp", std::process::id()));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(body)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("写入临时文件失败：{err}"));
    }

    match std::fs::rename(&temp, target) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            // Windows cannot rename over an existing file.  The device is
            // Linux, so this fallback only matters to host-side tests.
            let replaced = std::fs::remove_file(target).is_ok() && std::fs::rename(&temp, target).is_ok();
            if replaced {
                return Ok(());
            }
            let _ = std::fs::remove_file(&temp);
            Err(format!("替换存档文件失败：{first_error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("a%20b").unwrap(), "a b");
        assert_eq!(percent_decode("%E5%AD%98%E6%A1%A3").unwrap(), "存档");
        assert_eq!(percent_decode("plain").unwrap(), "plain");
        assert!(percent_decode("%zz").is_err());
        assert!(percent_decode("%2").is_err());
        // Invalid UTF-8 byte sequence.
        assert!(percent_decode("%FF").is_err());
    }

    #[test]
    fn accepts_valid_save_paths() {
        for raw in [
            "States/GBA/mgba/宝可梦/resume.state",
            "States/GBA/mgba/Game/2026-10-21_10-00-00.png",
            "Saves/GBA/Game.sav",
            "Saves/GBC/Game.srm",
        ] {
            let safe = validate(raw).unwrap();
            assert_eq!(safe.rel, raw);
        }
        assert_eq!(validate("Saves/GBA/Game.sav").unwrap().extension(), "sav");
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        for raw in [
            "",
            "/etc/passwd",
            "States/../etc/passwd",
            "States/GBA/./x.state",
            "States//x.state",
            "States\\GBA\\x.state",
            "States/GBA/x.state\0",
            "Other/GBA/x.state",
            "States/GBA/x.txt",
            "States/GBA/noext",
            "States",
            "Saves/x.sav/",
        ] {
            let err = validate(raw).unwrap_err();
            assert!(err.status == 400 || err.status == 403, "{raw}: {:?}", err);
        }
        // A decoded `..` arrives as one segment and is rejected just the same.
        let decoded = percent_decode("States%2F..%2Fetc%2Fpasswd").unwrap();
        assert!(validate(&decoded).is_err());
    }

    #[test]
    fn read_paths_stay_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("States/GBA/mgba/Game")).unwrap();
        std::fs::write(root.join("States/GBA/mgba/Game/resume.state"), b"x").unwrap();

        let safe = validate("States/GBA/mgba/Game/resume.state").unwrap();
        let resolved = resolve_read(root, &safe).unwrap();
        assert!(resolved.ends_with("resume.state"));

        // Symlink escaping the root must be refused.
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("secret.state"), b"secret").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("States/GBA/mgba/Evil")).unwrap();
            let evil = validate("States/GBA/mgba/Evil/secret.state").unwrap();
            assert!(resolve_read(root, &evil).is_err());
        }
    }

    #[test]
    fn write_paths_create_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("States")).unwrap();
        let safe = validate("States/GBA/mgba/新游戏/resume.state").unwrap();
        let target = resolve_write(root, &safe).unwrap();
        atomic_upload(&target, b"state-bytes").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"state-bytes");
        // No temp file may survive.
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn symlinked_parent_cannot_escape_root() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("root");
            std::fs::create_dir_all(root.join("States")).unwrap();
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("States/EvilCore")).unwrap();
            let safe = validate("States/EvilCore/Game/resume.state").unwrap();
            assert!(resolve_write(&root, &safe).is_err());
        }
    }

    #[test]
    fn atomic_upload_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Saves/Game.sav");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"old").unwrap();
        atomic_upload(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }
}
