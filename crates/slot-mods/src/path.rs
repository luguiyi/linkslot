//! Paths, ids and WebDAV root validation for the mod store.

use std::path::{Path, PathBuf};

use crate::error::ModError;
use crate::{MODS_DIR, TRASH_DIR};

pub fn mods_dir(root: &Path) -> PathBuf {
    root.join(MODS_DIR)
}

pub fn mod_dir(root: &Path, id: &str) -> PathBuf {
    mods_dir(root).join(id)
}

pub fn meta_path(root: &Path, id: &str) -> PathBuf {
    mod_dir(root, id).join("mod.json")
}

pub fn actions_dir(root: &Path, id: &str) -> PathBuf {
    mod_dir(root, id).join("actions")
}

pub fn action_dir(root: &Path, mod_id: &str, action_id: &str) -> PathBuf {
    actions_dir(root, mod_id).join(action_id)
}

pub fn action_meta_path(root: &Path, mod_id: &str, action_id: &str) -> PathBuf {
    action_dir(root, mod_id, action_id).join("action.json")
}

pub fn action_script_path(root: &Path, mod_id: &str, action_id: &str) -> PathBuf {
    action_dir(root, mod_id, action_id).join("main.lua")
}

pub fn action_trash_dir(root: &Path, mod_id: &str) -> PathBuf {
    mod_dir(root, mod_id).join(TRASH_DIR).join("actions")
}

pub fn script_path(root: &Path, id: &str) -> PathBuf {
    mod_dir(root, id).join("main.lua")
}

pub fn trash_dir(root: &Path) -> PathBuf {
    mods_dir(root).join(TRASH_DIR)
}

pub fn ensure_mods_dir(root: &Path) -> Result<(), ModError> {
    let dir = mods_dir(root);
    std::fs::create_dir_all(&dir)
        .map_err(|error| ModError::Io(format!("无法创建 {}：{error}", dir.display())))
}

/// `[a-z0-9][a-z0-9_-]{0,63}`.
pub fn validate_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return Err("mod id 长度必须为 1–64".to_string());
    }
    let first = bytes[0] as char;
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("mod id 必须以小写字母或数字开头".to_string());
    }
    if !bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-'
    }) {
        return Err("mod id 只能包含小写字母、数字、_ 和 -".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_strict() {
        for good in ["a", "a1", "mod-1", "m_2", "0start"] {
            assert!(validate_id(good).is_ok(), "{good}");
        }
        let too_long = "x".repeat(65);
        for bad in ["", "A", "-a", "_a", "a/b", "a b", "a.b", too_long.as_str()] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }
}
