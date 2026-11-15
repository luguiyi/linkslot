//! Atomic file helpers, revisions and unique names.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::error::ModError;
use crate::path::{mod_dir, mods_dir, trash_dir};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ModError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ModError::Io(format!(
            "无法读取 {}：{error}",
            path.display()
        ))),
    }
}

/// Write `bytes` to `path` through a same-directory temporary file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ModError> {
    let parent = path
        .parent()
        .ok_or_else(|| ModError::Io(format!("{} 没有父目录", path.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", parent.display())))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| ModError::Io(format!("{} 缺少文件名", path.display())))?;
    let unique = next_unique();
    let temp = parent.join(format!(".{file_name}.{}.{unique}.tmp", std::process::id()));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(ModError::Io(format!(
            "写入临时文件 {} 失败：{error}",
            temp.display()
        )));
    }

    match std::fs::rename(&temp, path) {
        Ok(()) => {
            // Best-effort directory sync; FAT32/Windows may reject it.
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Err(first_error) => {
            // Host-side Windows cannot rename over an existing file.
            if path.is_file() {
                let _ = std::fs::remove_file(path);
                if std::fs::rename(&temp, path).is_ok() {
                    return Ok(());
                }
            }
            let _ = std::fs::remove_file(&temp);
            Err(ModError::Io(format!(
                "替换 {} 失败：{first_error}",
                path.display()
            )))
        }
    }
}

fn next_unique() -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{counter:x}", now_secs())
}

/// A fresh directory id that does not collide with an existing mod.
pub fn new_mod_id(root: &Path) -> String {
    for _ in 0..64 {
        let nano = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("m-{nano:x}-{counter:x}");
        if !mod_dir(root, &id).exists() {
            return id;
        }
    }
    // Extremely unlikely; still safe (id is only a directory name).
    format!("m-{}", std::process::id())
}

/// A fresh action id that does not collide inside one mod.
pub fn new_action_id(root: &Path, mod_id: &str) -> String {
    for _ in 0..64 {
        let nano = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("a-{nano:x}-{counter:x}");
        if !crate::path::action_dir(root, mod_id, &id).exists() {
            return id;
        }
    }
    format!("a-{}", std::process::id())
}

/// A fresh target directory under `mods/.trash/`.
pub fn trash_target(root: &Path, id: &str) -> Result<PathBuf, ModError> {
    let dir = trash_dir(root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", dir.display())))?;
    unique_target(&dir, id)
}

/// A fresh action trash directory under `mods/<mod>/.trash/actions/`.
pub fn action_trash_target(
    root: &Path,
    mod_id: &str,
    action_id: &str,
) -> Result<PathBuf, ModError> {
    let dir = crate::path::action_trash_dir(root, mod_id);
    std::fs::create_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", dir.display())))?;
    unique_target(&dir, action_id)
}

fn unique_target(dir: &Path, base: &str) -> Result<PathBuf, ModError> {
    let stamp = now_secs();
    let base = format!("{base}-{stamp}");
    let mut target = dir.join(&base);
    let mut suffix = 1u32;
    while target.exists() {
        target = dir.join(format!("{base}-{suffix}"));
        suffix += 1;
        if suffix > 10_000 {
            return Err(ModError::Io("无法生成回收目录名".to_string()));
        }
    }
    Ok(target)
}

/// Revision covers both files, so toggling or editing either invalidates it.
pub fn revision_of(meta_bytes: &[u8], script_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"linkSlot-mod-v1\0");
    hasher.update(meta_bytes);
    hasher.update(b"\0");
    hasher.update(script_bytes);
    format!("{:x}", hasher.finalize())
}

/// True when `mods/` exists (used to keep the empty case cheap).
pub fn mods_dir_exists(root: &Path) -> bool {
    mods_dir(root).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/mod.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn revisions_change_with_either_file() {
        let first = revision_of(b"meta", b"script");
        let meta_changed = revision_of(b"meta2", b"script");
        let script_changed = revision_of(b"meta", b"script2");
        assert_ne!(first, meta_changed);
        assert_ne!(first, script_changed);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn generated_ids_are_valid_and_unique() {
        let dir = tempfile::tempdir().unwrap();
        let first = new_mod_id(dir.path());
        let second = new_mod_id(dir.path());
        assert_ne!(first, second);
        crate::path::validate_id(&first).unwrap();
        crate::path::validate_id(&second).unwrap();
    }
}
