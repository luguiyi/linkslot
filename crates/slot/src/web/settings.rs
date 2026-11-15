//! Web-service settings that are independent of the CLI config.
//!
//! Only the upload ceiling is user-editable today.  Keeping it in its own
//! small file means changing it never rewrites `config.toml` (and therefore
//! never endangers the WebDAV/WiFi comments and secrets kept there).

use std::path::Path;

pub const FILE_REL: &str = "linkSlot/Config/web.toml";
pub const DEFAULT_UPLOAD_MB: u16 = 32;
pub const MIN_UPLOAD_MB: u16 = 1;
pub const MAX_UPLOAD_MB: u16 = 256;

pub fn upload_mb(root: &Path) -> u16 {
    let Ok(text) = std::fs::read_to_string(root.join(FILE_REL)) else {
        return DEFAULT_UPLOAD_MB;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "max_upload_mb" {
            continue;
        }
        if let Ok(value) = value.trim().parse::<u16>() {
            return value.clamp(MIN_UPLOAD_MB, MAX_UPLOAD_MB);
        }
    }
    DEFAULT_UPLOAD_MB
}

pub fn upload_bytes(root: &Path) -> usize {
    upload_mb(root) as usize * 1024 * 1024
}

pub fn write_upload_mb(root: &Path, mb: u16) -> Result<(), String> {
    if !(MIN_UPLOAD_MB..=MAX_UPLOAD_MB).contains(&mb) {
        return Err(format!(
            "上传上限必须在 {MIN_UPLOAD_MB}–{MAX_UPLOAD_MB} MB 之间"
        ));
    }
    let path = root.join(FILE_REL);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 Web 设置目录：{error}"))?;
    }
    let body = format!(
        "# linkSlot Web 服务设置\n# 单个上传请求的最大字节数（MB）。默认 {DEFAULT_UPLOAD_MB}。\nmax_upload_mb = {mb}\n"
    );
    slot_store::atomic_write(&path, body.as_bytes())
        .map_err(|error| format!("无法写入 Web 设置：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_file_means_32_mb_and_values_are_clamped() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert_eq!(upload_mb(root), DEFAULT_UPLOAD_MB);
        std::fs::create_dir_all(root.join("linkSlot/Config")).unwrap();
        std::fs::write(root.join(FILE_REL), "max_upload_mb = 999\n").unwrap();
        assert_eq!(upload_mb(root), MAX_UPLOAD_MB);
        std::fs::write(root.join(FILE_REL), "max_upload_mb = 7\n").unwrap();
        assert_eq!(upload_bytes(root), 7 * 1024 * 1024);
    }

    #[test]
    fn writes_only_the_known_key_and_rejects_out_of_range() {
        let dir = tempdir().unwrap();
        write_upload_mb(dir.path(), 48).unwrap();
        assert_eq!(upload_mb(dir.path()), 48);
        assert!(write_upload_mb(dir.path(), 0).is_err());
        assert!(write_upload_mb(dir.path(), MAX_UPLOAD_MB + 1).is_err());
    }
}
