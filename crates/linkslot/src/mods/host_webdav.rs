//! Sandboxed WebDAV access for Lua mods.
//!
//! Each configured WebDAV sync method exposes its own `remote_root` to Lua.
//! Credentials never enter the Lua state; the mod can only address the roots
//! of sync methods the user configured.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::RemoteProfile;
use crate::hash::sha256_hex;
use crate::mods::{MAX_REMOTE_GET, MAX_REMOTE_PUT};
use crate::output::Reporter;
use crate::webdav::{DavClient, DavEntry, DavTarget, MoveOutcome};

pub struct RemoteClients {
    root: PathBuf,
    profiles: Vec<RemoteProfile>,
    clients: RefCell<HashMap<String, Rc<DavClient>>>,
    requests: Cell<usize>,
    bytes: Cell<usize>,
    max_requests: usize,
    max_total: usize,
}

fn direct_put_with_backup(
    client: &DavClient,
    rep: &mut Reporter,
    target: &[&str],
    temp: &[&str],
    backup: &[&str],
    data: &[u8],
) -> Result<(), String> {
    let original = match client.get_limited(rep, target, MAX_REMOTE_GET) {
        Ok(value) => value,
        Err(error) => {
            let _ = client.delete(rep, temp);
            return Err(format!(
                "MOVE 失败且无法读取原 archive，已中止，未覆盖原文件：{}",
                error.message()
            ));
        }
    };
    let original_hash = original.as_ref().map(|bytes| sha256_hex(bytes));
    if let Some(original) = &original {
        if let Err(error) = client.put(rep, backup, original) {
            let _ = client.delete(rep, backup);
            let _ = client.delete(rep, temp);
            return Err(format!(
                "MOVE 失败且备份原 archive 失败，已中止：{}",
                error.message()
            ));
        }
        let backup_ok = client
            .get_limited(rep, backup, MAX_REMOTE_GET)
            .ok()
            .flatten()
            .map(|bytes| Some(sha256_hex(&bytes)) == original_hash)
            .unwrap_or(false);
        if !backup_ok {
            let _ = client.delete(rep, backup);
            let _ = client.delete(rep, temp);
            return Err("MOVE 失败且备份校验失败，已中止，未覆盖原文件".to_string());
        }
    }

    if let Err(error) = client.put(rep, target, data) {
        let _ = client.delete(rep, temp);
        return Err(format!(
            "MOVE 失败，直接写入 archive 也失败：{}",
            error.message()
        ));
    }
    let new_hash = sha256_hex(data);
    let written = client
        .get_limited(rep, target, MAX_REMOTE_GET)
        .ok()
        .flatten()
        .map(|bytes| sha256_hex(&bytes) == new_hash)
        .unwrap_or(false);
    if !written {
        if let Some(original) = &original {
            if let Err(error) = client.put(rep, target, original) {
                let _ = client.delete(rep, temp);
                return Err(format!(
                    "新 archive 校验失败，且从备份恢复失败（备份保留在 {}）：{}",
                    backup.join("/"),
                    error.message()
                ));
            }
        }
        let _ = client.delete(rep, temp);
        return Err("新 archive 校验失败，已恢复原 archive".to_string());
    }
    let _ = client.delete(rep, temp);
    Ok(())
}

impl RemoteClients {
    pub fn new(
        root: &Path,
        profiles: &[RemoteProfile],
        max_requests: usize,
        max_total: usize,
    ) -> RemoteClients {
        RemoteClients {
            root: root.to_path_buf(),
            profiles: profiles.to_vec(),
            clients: RefCell::new(HashMap::new()),
            requests: Cell::new(0),
            bytes: Cell::new(0),
            max_requests,
            max_total,
        }
    }

    pub fn profiles(&self) -> &[RemoteProfile] {
        &self.profiles
    }

    pub fn get(&self, profile_id: &str, raw: &str) -> Result<Option<Vec<u8>>, String> {
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, false)?;
        let rel = refs(&path);
        self.take_request()?;
        let bytes = self.run(profile, |client, rep| {
            client.get_limited(rep, &rel, MAX_REMOTE_GET)
        })?;
        if let Some(bytes) = &bytes {
            self.add_bytes(bytes.len())?;
        }
        Ok(bytes)
    }

    pub fn put(&self, profile_id: &str, raw: &str, data: &[u8]) -> Result<(), String> {
        if data.len() > MAX_REMOTE_PUT {
            return Err(format!(
                "远程写入超过 {} MiB 上限",
                MAX_REMOTE_PUT / (1024 * 1024)
            ));
        }
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, false)?;
        let rel = refs(&path);
        self.add_bytes(data.len())?;
        self.take_request()?;
        self.run(profile, |client, rep| client.put(rep, &rel, data))
    }

    pub fn put_atomic(&self, profile_id: &str, raw: &str, data: &[u8]) -> Result<(), String> {
        if data.len() > MAX_REMOTE_PUT {
            return Err(format!(
                "远程写入超过 {} MiB 上限",
                MAX_REMOTE_PUT / (1024 * 1024)
            ));
        }
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, false)?;
        let rel = refs(&path);
        let mut temp_path = path.clone();
        let last = temp_path.pop().expect("non-empty");
        temp_path.push(format!(
            "{last}.linkslot-mod-tmp.{}",
            crate::util::unique_suffix()
        ));
        let temp_rel = refs(&temp_path);
        let mut backup_path = path.clone();
        backup_path.pop();
        backup_path.push(format!(
            "{last}.linkslot-mod-bak.{}",
            crate::util::unique_suffix()
        ));
        let backup_rel = refs(&backup_path);
        self.add_bytes(data.len())?;
        // PUT + MOVE.  If the server refuses MOVE (or the destination cannot be
        // renamed), fall back to a verified backup + direct PUT.  The original
        // archive is never replaced without a readable backup.
        for _ in 0..3 {
            self.take_request()?;
        }
        self.run(profile, |client, rep| {
            client.put(rep, &temp_rel, data)?;
            match client.move_to(rep, &temp_rel, &rel) {
                Ok(MoveOutcome::Moved) => {
                    let _ = client.delete(rep, &temp_rel);
                    Ok(())
                }
                Ok(MoveOutcome::Unsupported) | Err(_) => {
                    direct_put_with_backup(client, rep, &rel, &temp_rel, &backup_rel, data).map_err(
                        |message| {
                            crate::error::LinkError::new(crate::error::ErrorKind::Server, message)
                        },
                    )
                }
            }
        })
    }

    pub fn list(&self, profile_id: &str, raw: &str) -> Result<Option<Vec<DavEntry>>, String> {
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, true)?;
        let rel = refs(&path);
        self.take_request()?;
        self.run(profile, |client, rep| client.list(rep, &rel))
    }

    pub fn stat(&self, profile_id: &str, raw: &str) -> Result<Option<DavEntry>, String> {
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, false)?;
        let rel = refs(&path);
        self.take_request()?;
        self.run(profile, |client, rep| client.stat(rep, &rel))
    }

    pub fn mkdir(&self, profile_id: &str, raw: &str) -> Result<(), String> {
        let profile = self.find(profile_id)?;
        let path = path_segments(raw, true)?;
        let rel = refs(&path);
        self.take_request()?;
        self.run(profile, |client, rep| client.ensure_dir(rep, &rel))
    }

    fn find(&self, id: &str) -> Result<&RemoteProfile, String> {
        if id.trim().is_empty() {
            return Err("未指定同步方式；请先在 mod 管理页选择 WebDAV 同步方式".to_string());
        }
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| format!("同步方式「{id}」不存在"))
    }

    fn client(&self, profile: &RemoteProfile) -> Result<Rc<DavClient>, String> {
        if let Some(client) = self.clients.borrow().get(&profile.id) {
            return Ok(client.clone());
        }
        let target = DavTarget {
            endpoint: &profile.endpoint,
            remote_root: &profile.remote_root,
            username: &profile.username,
            password: &profile.password,
            ca_cert: &profile.ca_cert,
            insecure: profile.insecure,
            retries: 0,
            timeout_secs: profile.timeout_secs,
            label: "mods",
        };
        let (result, _logs) = with_reporter(&self.root, |rep| DavClient::from_target(&target, rep));
        let client = Rc::new(result.map_err(|error| error.message().to_string())?);
        self.clients
            .borrow_mut()
            .insert(profile.id.clone(), client.clone());
        Ok(client)
    }

    fn run<T>(
        &self,
        profile: &RemoteProfile,
        f: impl FnOnce(&DavClient, &mut Reporter) -> crate::error::Result<T>,
    ) -> Result<T, String> {
        let client = self.client(profile)?;
        let (result, _logs) = with_reporter(&self.root, |rep| f(&client, rep));
        result.map_err(|error| error.message().to_string())
    }

    fn take_request(&self) -> Result<(), String> {
        let count = self.requests.get() + 1;
        if count > self.max_requests {
            return Err(format!("单个 mod 的远程请求超过 {} 次", self.max_requests));
        }
        self.requests.set(count);
        Ok(())
    }

    fn add_bytes(&self, amount: usize) -> Result<(), String> {
        let total = self.bytes.get().saturating_add(amount);
        if total > self.max_total {
            return Err(format!(
                "单个 mod 的远程传输超过 {} MiB 上限",
                self.max_total / (1024 * 1024)
            ));
        }
        self.bytes.set(total);
        Ok(())
    }
}

fn with_reporter<T>(root: &Path, f: impl FnOnce(&mut Reporter) -> T) -> (T, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut reporter = Reporter::new(root, false, &mut out, &mut err);
    let result = f(&mut reporter);
    let logs = String::from_utf8_lossy(&err).into_owned();
    (result, logs)
}

fn refs(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

fn path_segments(raw: &str, allow_empty: bool) -> Result<Vec<String>, String> {
    let trimmed = raw.trim();
    let trimmed = trimmed.trim_matches('/');
    if trimmed.is_empty() {
        return if allow_empty {
            Ok(Vec::new())
        } else {
            Err("远程路径不能为空".to_string())
        };
    }
    if raw.contains('\\') || raw.contains('\0') {
        return Err("远程路径包含非法字符".to_string());
    }
    if trimmed.len() > 1024 {
        return Err("远程路径过长".to_string());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            return Err("远程路径包含空目录名".to_string());
        }
        if segment == "." || segment == ".." {
            return Err("远程路径不允许 . 或 ..".to_string());
        }
        if segment.chars().any(char::is_control) {
            return Err("远程路径包含控制字符".to_string());
        }
        if segment.contains('?') || segment.contains('#') {
            return Err("远程路径不能包含 ? 或 #".to_string());
        }
        segments.push(segment.to_string());
    }
    if segments.len() > 32 {
        return Err("远程路径层级过深".to_string());
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_remote_paths() {
        assert!(path_segments("", false).is_err());
        assert_eq!(path_segments("/", true).unwrap(), Vec::<String>::new());
        assert_eq!(
            path_segments("/GBA/宝可梦/battery.sav", false).unwrap(),
            vec!["GBA", "宝可梦", "battery.sav"]
        );
        for bad in ["../x", "a/../b", "a//b", "a\\b", "a?b", "a\0b"] {
            assert!(path_segments(bad, false).is_err(), "{bad}");
        }
    }
}
