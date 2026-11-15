//! Sandboxed local file access for Lua mods.
//!
//! The allowlist is `States/`, `Saves/` and the script's own
//! `mods/<id>/` directory. Paths are validated segment-by-segment and then
//! canonicalised so a symlink cannot redirect a read/write outside the card
//! root.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mods::{MAX_LOCAL_ENTRIES, MAX_LOCAL_READ, MAX_LOCAL_WRITE};

const MAX_PATH_BYTES: usize = 1024;
const MAX_SEGMENTS: usize = 32;
const FAT_ILLEGAL: [char; 8] = ['\\', ':', '*', '?', '"', '<', '>', '|'];

pub struct LocalFiles {
    root: PathBuf,
    mod_id: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

impl LocalFiles {
    pub fn new(root: &Path, mod_id: &str) -> LocalFiles {
        LocalFiles {
            root: root.to_path_buf(),
            mod_id: mod_id.to_string(),
        }
    }

    pub fn read(&self, raw: &str) -> Result<Option<Vec<u8>>, String> {
        let segments = self.validate(raw)?;
        if segments.len() < 2 {
            return Err("需要文件路径".to_string());
        }
        let Some(path) = self.resolve_existing(&segments, false)? else {
            return Ok(None);
        };
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        if metadata.len() > MAX_LOCAL_READ as u64 {
            return Err(format!(
                "文件超过 {} MiB 读取上限",
                MAX_LOCAL_READ / (1024 * 1024)
            ));
        }
        std::fs::read(&path)
            .map(Some)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))
    }

    pub fn write(&self, raw: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_LOCAL_WRITE {
            return Err(format!(
                "写入超过 {} MiB 上限",
                MAX_LOCAL_WRITE / (1024 * 1024)
            ));
        }
        let segments = self.validate(raw)?;
        self.check_write_shape(&segments)?;
        let target = self.resolve_write(&segments)?;
        crate::store::atomic_write(&target, bytes)
            .map_err(|error| format!("写入 {} 失败：{}", target.display(), error.message()))
    }

    pub fn list(&self, raw: &str) -> Result<Vec<Entry>, String> {
        let segments = self.validate(raw)?;
        let Some(path) = self.resolve_existing(&segments, true)? else {
            return Ok(Vec::new());
        };
        if !path.is_dir() {
            return Err("目标不是目录".to_string());
        }
        let read_dir = std::fs::read_dir(&path)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        let mut out = Vec::new();
        for entry in read_dir.flatten() {
            if out.len() >= MAX_LOCAL_ENTRIES {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            out.push(Entry {
                name,
                is_dir: metadata.is_dir(),
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                mtime: modified_secs(&metadata),
            });
        }
        out.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(out)
    }

    pub fn stat(&self, raw: &str) -> Result<Option<Entry>, String> {
        let segments = self.validate(raw)?;
        let Some(path) = self.resolve_existing(&segments, true)? else {
            return Ok(None);
        };
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        Ok(Some(Entry {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            is_dir: metadata.is_dir(),
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            mtime: modified_secs(&metadata),
        }))
    }

    pub fn exists(&self, raw: &str) -> Result<bool, String> {
        let segments = self.validate(raw)?;
        Ok(self.resolve_existing(&segments, true)?.is_some())
    }

    pub fn mkdir(&self, raw: &str) -> Result<(), String> {
        let segments = self.validate(raw)?;
        self.resolve_dir_create(&segments)?;
        Ok(())
    }

    fn validate(&self, raw: &str) -> Result<Vec<String>, String> {
        if raw.is_empty() {
            return Err("路径不能为空".to_string());
        }
        if raw.len() > MAX_PATH_BYTES {
            return Err("路径过长".to_string());
        }
        if raw.starts_with('/') || raw.contains('\\') {
            return Err("不允许绝对路径或反斜杠".to_string());
        }
        if raw.contains('\0') {
            return Err("路径包含 NUL".to_string());
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            if segment.is_empty() {
                return Err("路径包含空目录名".to_string());
            }
            if segment == "." || segment == ".." {
                return Err("不允许 . 或 .. 路径段".to_string());
            }
            if segment.chars().any(char::is_control) {
                return Err("路径包含控制字符".to_string());
            }
            if segment.chars().any(|ch| FAT_ILLEGAL.contains(&ch)) {
                return Err("路径包含 FAT 非法字符".to_string());
            }
            segments.push(segment.to_string());
        }
        if segments.len() > MAX_SEGMENTS {
            return Err("路径层级过深".to_string());
        }
        let first = segments[0].as_str();
        if first == "States" || first == "Saves" {
            for segment in &segments[1..] {
                if segment.starts_with('.') {
                    return Err("不允许访问隐藏文件/目录".to_string());
                }
            }
        } else if first.eq_ignore_ascii_case("linkSlot") {
            if segments.len() < 3
                || !segments[1].eq_ignore_ascii_case("mods")
                || segments[2] != self.mod_id
            {
                return Err("只允许访问当前 mod 自己的 linkSlot/mods/<id>/ 目录".to_string());
            }
        } else {
            return Err("只允许访问 States/、Saves/ 和当前 mod 目录".to_string());
        }
        Ok(segments)
    }

    fn check_write_shape(&self, segments: &[String]) -> Result<(), String> {
        let first = segments[0].as_str();
        if first == "States" || first == "Saves" {
            if segments.len() < 2 {
                return Err("需要文件路径".to_string());
            }
            let last = segments.last().expect("non-empty");
            let extension = last
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
                .unwrap_or_default();
            let allowed: &[&str] = if first == "States" {
                if last.eq_ignore_ascii_case("playtime.json") {
                    &["json"]
                } else {
                    &["state", "png"]
                }
            } else {
                &["sav", "srm"]
            };
            if !allowed.contains(&extension.as_str()) {
                return Err(format!(
                    "{} 只允许写入 {} 文件",
                    first,
                    allowed
                        .iter()
                        .map(|extension| format!(".{extension}"))
                        .collect::<Vec<_>>()
                        .join(" / ")
                ));
            }
        }
        Ok(())
    }

    fn resolve_existing(
        &self,
        segments: &[String],
        allow_dir: bool,
    ) -> Result<Option<PathBuf>, String> {
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|error| format!("存储根目录不可用：{error}"))?;
        let candidate = segments
            .iter()
            .fold(canon_root.clone(), |path, segment| path.join(segment));
        let canonical = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法解析 {}：{error}", candidate.display())),
        };
        if !canonical.starts_with(&canon_root) {
            return Err("路径越界".to_string());
        }
        self.check_top(&canonical, &canon_root, segments)?;
        if !allow_dir && canonical.is_dir() {
            return Err("目标是目录".to_string());
        }
        Ok(Some(canonical))
    }

    fn resolve_write(&self, segments: &[String]) -> Result<PathBuf, String> {
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|error| format!("存储根目录不可用：{error}"))?;
        let mut current = canon_root.clone();
        for segment in &segments[..segments.len() - 1] {
            let next = current.join(segment);
            match std::fs::symlink_metadata(&next) {
                Ok(_) => {
                    let canonical = next
                        .canonicalize()
                        .map_err(|error| format!("无法解析 {}：{error}", next.display()))?;
                    if !canonical.starts_with(&canon_root) {
                        return Err("路径越界".to_string());
                    }
                    if !canonical.is_dir() {
                        return Err("父路径不是目录".to_string());
                    }
                    current = canonical;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir_all(&next)
                        .map_err(|error| format!("无法创建 {}：{error}", next.display()))?;
                    let canonical = next
                        .canonicalize()
                        .map_err(|error| format!("无法解析 {}：{error}", next.display()))?;
                    if !canonical.starts_with(&canon_root) {
                        return Err("路径越界".to_string());
                    }
                    current = canonical;
                }
                Err(error) => return Err(format!("无法访问 {}：{error}", next.display())),
            }
        }
        self.check_top(&current, &canon_root, segments)?;
        let target = current.join(segments.last().expect("non-empty"));
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.is_dir() {
                return Err("目标是目录".to_string());
            }
        }
        Ok(target)
    }

    fn resolve_dir_create(&self, segments: &[String]) -> Result<PathBuf, String> {
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|error| format!("存储根目录不可用：{error}"))?;
        let mut current = canon_root.clone();
        for segment in segments {
            let next = current.join(segment);
            match std::fs::symlink_metadata(&next) {
                Ok(_) => {
                    let canonical = next
                        .canonicalize()
                        .map_err(|error| format!("无法解析 {}：{error}", next.display()))?;
                    if !canonical.starts_with(&canon_root) {
                        return Err("路径越界".to_string());
                    }
                    if !canonical.is_dir() {
                        return Err("目标不是目录".to_string());
                    }
                    current = canonical;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir_all(&next)
                        .map_err(|error| format!("无法创建 {}：{error}", next.display()))?;
                    let canonical = next
                        .canonicalize()
                        .map_err(|error| format!("无法解析 {}：{error}", next.display()))?;
                    if !canonical.starts_with(&canon_root) {
                        return Err("路径越界".to_string());
                    }
                    current = canonical;
                }
                Err(error) => return Err(format!("无法访问 {}：{error}", next.display())),
            }
        }
        self.check_top(&current, &canon_root, segments)?;
        Ok(current)
    }

    fn check_top(&self, path: &Path, canon_root: &Path, segments: &[String]) -> Result<(), String> {
        let relative = path
            .strip_prefix(canon_root)
            .map_err(|_| "路径越界".to_string())?;
        let mut components = relative.components();
        let Some(Component::Normal(first)) = components.next() else {
            return Err("路径越界".to_string());
        };
        if !first.to_string_lossy().eq_ignore_ascii_case(&segments[0]) {
            return Err("路径越界".to_string());
        }
        // 软链兜底：canonical 路径必须仍在当前 mod 自己的 linkSlot/mods/<id>/ 下。
        if segments[0].eq_ignore_ascii_case("linkSlot") {
            let Some(Component::Normal(second)) = components.next() else {
                return Err("路径越界".to_string());
            };
            if !second.to_string_lossy().eq_ignore_ascii_case("mods") {
                return Err("路径越界".to_string());
            }
            let Some(Component::Normal(third)) = components.next() else {
                return Err("路径越界".to_string());
            };
            if third.to_string_lossy() != self.mod_id {
                return Err("路径越界".to_string());
            }
        }
        Ok(())
    }
}

fn modified_secs(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        })
}


#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "linkslot-mod-files-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn playtime_json_is_writable_but_other_state_json_is_not() {
        let root = temp_root("playtime");
        let files = LocalFiles::new(&root, "demo");
        let path = "States/GBA/mgba/Game/playtime.json";
        files.write(path, br#"{"schema":1,"seconds":60}"#).unwrap();
        assert!(std::fs::read(root.join(path)).unwrap().starts_with(b"{\"schema\""));
        assert!(files.write("States/GBA/mgba/Game/extra.json", b"{}").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_mod_can_write_its_own_link_slot_directory_only() {
        let root = temp_root("own");
        std::fs::create_dir_all(root.join("linkSlot/mods/demo/web")).unwrap();
        std::fs::create_dir_all(root.join("linkSlot/mods/other")).unwrap();
        let files = LocalFiles::new(&root, "demo");
        files.write("linkSlot/mods/demo/web/app.js", b"ok").unwrap();
        assert!(root.join("linkSlot/mods/demo/web/app.js").is_file());
        assert!(files.write("linkSlot/mods/other/evil.js", b"x").is_err());
        assert!(files.write("mods/demo/legacy.js", b"x").is_err());
        assert!(files.write("linkSlot/Config/config.toml", b"x").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_another_mod_is_refused() {
        let root = temp_root("link");
        std::fs::create_dir_all(root.join("linkSlot/mods/demo/web")).unwrap();
        std::fs::create_dir_all(root.join("linkSlot/mods/other")).unwrap();
        std::fs::write(root.join("linkSlot/mods/other/secret.bin"), b"x").unwrap();
        std::os::unix::fs::symlink(
            root.join("linkSlot/mods/other"),
            root.join("linkSlot/mods/demo/web/link"),
        )
        .unwrap();
        let files = LocalFiles::new(&root, "demo");
        assert!(
            files.read("linkSlot/mods/demo/web/link/secret.bin").is_err(),
            "another mod was reachable through a symlink"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
