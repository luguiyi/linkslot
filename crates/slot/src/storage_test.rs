//! Background WebDAV connection test for the on-device "sync method" screen.
//!
//! The UI must stay responsive while DNS/TLS/network work happens.  This job is
//! deliberately small: create a temporary config for one saved profile, run
//! `linkSlot net test`, and send the result back through a channel.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageTestResult {
    pub ok: bool,
    pub message: String,
}

pub struct StorageTestJob {
    rx: Receiver<StorageTestResult>,
}

impl StorageTestJob {
    pub fn try_recv(&self) -> Option<StorageTestResult> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

pub fn spawn(root: PathBuf, profile_id: String) -> StorageTestJob {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new()
        .name("slot-storage-test".into())
        .spawn(move || {
            let result = run(&root, &profile_id);
            let _ = tx.send(result);
        });
    StorageTestJob { rx }
}

fn run(root: &Path, profile_id: &str) -> StorageTestResult {
    if let Err(message) = ensure_network(root) {
        return StorageTestResult {
            ok: false,
            message,
        };
    }
    let config = match crate::linkcfg::profile_test_config(root, profile_id) {
        Ok(config) => config,
        Err(message) => {
            return StorageTestResult {
                ok: false,
                message,
            };
        }
    };
    let script = root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh");
    let mut command = Command::new("sh");
    command
        .arg(&script)
        .arg("net")
        .arg("test")
        .arg("--config")
        .arg(config.path())
        .env("SLOT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return StorageTestResult {
                ok: false,
                message: format!("无法启动 linkSlot 测试：{error}"),
            };
        }
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < TEST_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(error) => {
                let _ = child.kill();
                return StorageTestResult {
                    ok: false,
                    message: format!("等待 linkSlot 测试失败：{error}"),
                };
            }
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let Some(status) = status else {
        return StorageTestResult {
            ok: false,
            message: format!(
                "连接测试超时（{} 秒），已停止。请检查 WiFi、地址和远程根目录。",
                TEST_TIMEOUT.as_secs()
            ),
        };
    };
    parse_result(status.success(), &stdout, &stderr)
}

fn ensure_network(root: &Path) -> Result<(), String> {
    let script = root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh");
    let mut command = Command::new("sh");
    command
        .arg(&script)
        .arg("net")
        .arg("up")
        .arg("--timeout")
        .arg("20")
        .env("SLOT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 WiFi 联网：{error}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < Duration::from_secs(30) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(error) => {
                let _ = child.kill();
                return Err(format!("等待 WiFi 联网失败：{error}"));
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    match status {
        Some(status) if status.success() => Ok(()),
        Some(_) => {
            let detail = stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("无法连接 WiFi");
            Err(format!("测试前联网失败：{detail}"))
        }
        None => Err("测试前联网超时（30 秒），请检查 WiFi 设置。".to_string()),
    }
}

fn parse_result(success: bool, stdout: &str, stderr: &str) -> StorageTestResult {
    let detail = stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("LINKSLOT_DETAIL="))
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(str::to_string)
        .or_else(|| {
            stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            stdout
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        });
    if success {
        StorageTestResult {
            ok: true,
            message: detail.unwrap_or_else(|| "WebDAV 连接成功".to_string()),
        }
    } else {
        StorageTestResult {
            ok: false,
            message: detail.unwrap_or_else(|| "连接测试失败，请检查 WebDAV 设置。".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_test_reads_the_cli_detail() {
        let result = parse_result(
            true,
            "LINKSLOT_RESULT=ok\nLINKSLOT_DETAIL=WebDAV 连接成功（HTTP 207）\n",
            "",
        );
        assert!(result.ok);
        assert!(result.message.contains("HTTP 207"));
    }

    #[test]
    fn a_failed_test_keeps_the_actionable_reason() {
        let result = parse_result(
            false,
            "LINKSLOT_RESULT=error\nLINKSLOT_DETAIL=远程根目录不存在（HTTP 404），请检查 remote_root\n",
            "",
        );
        assert!(!result.ok);
        assert!(result.message.contains("远程根目录不存在"));
    }
}
