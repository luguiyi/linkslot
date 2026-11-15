//! On-demand Wi-Fi / network checks.
//!
//! The actual wpa_supplicant/udhcpc choreography lives in
//! `<root>/System/linkSlot/shell/linkslot-net.sh`, which is invoked through `/bin/sh` (the
//! card is FAT32 and may not carry executable bits). This module only:
//! 1. runs `linkslot-net.sh status|up` (with a timeout),
//! 2. relays the script's human readable progress to stderr,
//! 3. does a direct TCP probe (<=2s) to the configured WebDAV host.
//!
//! `NetMode::Skip` is used by tests and offline diagnostics: no network
//! probing and no script execution at all.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{ErrorKind, LinkError, Result};
use crate::output::Reporter;

pub const NET_SCRIPT_REL: &str = "System/linkSlot/shell/linkslot-net.sh";
pub const TCP_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const UP_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetMode {
    /// Probe and, when allowed, bring Wi-Fi up (normal device behavior).
    Real,
    /// Assume the network is usable (tests / offline diagnostics).
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetStatus {
    pub connected: bool,
    pub detail: String,
}

pub fn script_path(root: &Path) -> PathBuf {
    root.join(NET_SCRIPT_REL)
}

/// Result of one `linkslot-net.sh` invocation.
#[derive(Clone, Debug, Default)]
pub struct ScriptOutput {
    /// Exit code, or `None` when the script had to be killed.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl ScriptOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Last non-empty stderr line, used as the failure detail.
    pub fn last_stderr(&self) -> Option<&str> {
        self.stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
    }
}

/// Run `/bin/sh <root>/System/linkSlot/shell/linkslot-net.sh <subcommand>`.
pub fn run_script(
    root: &Path,
    cfg: &Config,
    subcommand: &str,
    timeout: Duration,
) -> Result<ScriptOutput> {
    let script = script_path(root);
    if !script.is_file() {
        return Err(LinkError::new(
            ErrorKind::Net,
            format!("联网脚本不存在：{}", script.display()),
        ));
    }
    let mut command = Command::new("/bin/sh");
    command
        .arg(&script)
        .arg(subcommand)
        .env("LINKSLOT_WIFI_SSID", &cfg.wifi().ssid)
        .env("LINKSLOT_WIFI_PSK", &cfg.wifi().psk)
        .env("LINKSLOT_WIFI_TIMEOUT", cfg.wifi().timeout_secs.to_string())
        .env(
            "LINKSLOT_LOG_LEVEL",
            crate::output::read_log_level(root).as_str(),
        )
        .env("SLOT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| {
        LinkError::new(
            ErrorKind::Net,
            format!("无法执行联网脚本 {}：{err}", script.display()),
        )
    })?;
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });

    let deadline = Instant::now() + timeout;
    let mut code = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                code = status.code();
                break;
            }
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(LinkError::new(
                    ErrorKind::Net,
                    format!("等待联网脚本失败：{err}"),
                ));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    Ok(ScriptOutput {
        code,
        stdout,
        stderr,
    })
}

fn join_reader(handle: Option<std::thread::JoinHandle<String>>) -> String {
    match handle {
        Some(handle) => handle.join().unwrap_or_default(),
        None => String::new(),
    }
}

/// Direct TCP probe to the WebDAV host/port (used as the final
/// `net status` criterion).
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    for addr in addrs {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        if TcpStream::connect_timeout(&addr, remaining).is_ok() {
            return true;
        }
    }
    false
}

/// Full `net status`: script probe + TCP probe to the WebDAV endpoint.
///
/// The script is optional here: a machine that has no on-card net script (PC
/// development) can still be checked through the TCP probe.
pub fn status(root: &Path, cfg: &Config) -> Result<NetStatus> {
    let endpoint = cfg.endpoint();
    if script_path(root).is_file() {
        let output = run_script(root, cfg, "status", STATUS_TIMEOUT)?;
        if !output.success() {
            let reason = output
                .last_stderr()
                .map(str::to_string)
                .unwrap_or_else(|| "网络未连接".to_string());
            return Ok(NetStatus {
                connected: false,
                detail: reason,
            });
        }
    }
    let target = format!("{}:{}", endpoint.socket_host(), endpoint.port);
    if tcp_probe(endpoint.socket_host(), endpoint.port, TCP_PROBE_TIMEOUT) {
        Ok(NetStatus {
            connected: true,
            detail: "网络已连接".to_string(),
        })
    } else {
        Ok(NetStatus {
            connected: false,
            detail: format!("无法连接 WebDAV 主机 {target}"),
        })
    }
}

/// Ensure connectivity before push/pull, bringing Wi-Fi up when configured.
pub fn ensure_connected(
    root: &Path,
    cfg: &Config,
    mode: NetMode,
    force_up: bool,
    timeout_override: Option<u64>,
    rep: &mut Reporter,
) -> Result<()> {
    if mode == NetMode::Skip {
        rep.info("已跳过网络检查（测试/离线模式）");
        return Ok(());
    }
    let current = status(root, cfg)?;
    if current.connected {
        rep.info(format!("网络可用（{}）", current.detail));
        return Ok(());
    }
    rep.info(format!("网络不可用：{}", current.detail));
    let auto = cfg.wifi().manage || force_up;
    if !auto {
        return Err(LinkError::new(
            ErrorKind::Net,
            format!("网络未连接（{}）", current.detail),
        ));
    }
    if cfg.wifi().ssid.trim().is_empty() {
        return Err(LinkError::config(
            "配置错误：wifi.manage=true 但 wifi.ssid 为空，无法自动联网",
        ));
    }
    rep.progress("网络未连接，正在连接 Wi-Fi…");
    let timeout_secs = timeout_override.unwrap_or(cfg.wifi().timeout_secs);
    rep.debug(format!(
        "wifi: net up ssid={:?} timeout={}s",
        cfg.wifi().ssid,
        timeout_secs
    ));
    let output = run_script(
        root,
        cfg,
        "up",
        Duration::from_secs(timeout_secs) + UP_GRACE,
    )?;
    relay_script_output(rep, &output);
    rep.debug(format!(
        "wifi: net up exit={:?} stderr_lines={}",
        output.code,
        output.stderr.lines().count()
    ));
    if !output.success() {
        let reason = output
            .last_stderr()
            .map(str::to_string)
            .unwrap_or_else(|| "未知原因".to_string());
        return Err(LinkError::new(
            ErrorKind::Net,
            format!(
                "联网失败（退出码 {}）：{reason}",
                output
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "超时".to_string())
            ),
        ));
    }
    let after = status(root, cfg)?;
    if after.connected {
        rep.progress("网络已连接");
        Ok(())
    } else {
        Err(LinkError::new(
            ErrorKind::Net,
            format!("联网后仍无法连接：{}", after.detail),
        ))
    }
}

/// Ensure connectivity before a manual `mods run` job.
///
/// Unlike [`ensure_connected`], this does not probe the configured WebDAV
/// endpoint: a manual mod may only talk to a LAN peer such as GBAStation.  The
/// optional target is probed after Wi-Fi is up, and a failed target does not
/// trigger another Wi-Fi association attempt.
pub fn ensure_connected_to(
    root: &Path,
    cfg: &Config,
    mode: NetMode,
    force_up: bool,
    timeout_override: Option<u64>,
    target: Option<(&str, u16, &str)>,
    rep: &mut Reporter,
) -> Result<()> {
    if mode == NetMode::Skip {
        rep.info("已跳过网络检查（测试/离线模式）");
        return Ok(());
    }

    let wifi_ready = match status_of_wifi(root, cfg)? {
        Some((true, detail)) => {
            rep.info(format!("网络可用（{detail}）"));
            true
        }
        Some((false, detail)) => {
            rep.info(format!("网络不可用：{detail}"));
            false
        }
        None => true,
    };

    if !wifi_ready {
        let auto = cfg.wifi().manage || force_up;
        if !auto {
            return Err(LinkError::new(
                ErrorKind::Net,
                "网络未连接（无法自动联网）".to_string(),
            ));
        }
        if cfg.wifi().ssid.trim().is_empty() {
            return Err(LinkError::config(
                "配置错误：wifi.manage=true 但 wifi.ssid 为空，无法自动联网",
            ));
        }
        rep.progress("网络未连接，正在连接 Wi-Fi…");
        let timeout_secs = timeout_override.unwrap_or(cfg.wifi().timeout_secs);
        let output = run_script(
            root,
            cfg,
            "up",
            Duration::from_secs(timeout_secs) + UP_GRACE,
        )?;
        relay_script_output(rep, &output);
        if !output.success() {
            let reason = output
                .last_stderr()
                .map(str::to_string)
                .unwrap_or_else(|| "未知原因".to_string());
            return Err(LinkError::new(
                ErrorKind::Net,
                format!(
                    "联网失败（退出码 {}）：{reason}",
                    output
                        .code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "超时".to_string())
                ),
            ));
        }
        rep.progress("网络已连接");
    }

    if let Some((host, port, label)) = target {
        if tcp_probe(host, port, TCP_PROBE_TIMEOUT) {
            Ok(())
        } else {
            Err(LinkError::new(
                ErrorKind::Net,
                format!("无法连接 {label} 主机 {host}:{port}"),
            ))
        }
    } else {
        Ok(())
    }
}

/// Wi-Fi-only status (no WebDAV TCP probe).  `None` means the card has no
/// `linkslot-net.sh`, so the caller should assume networking is managed
/// elsewhere (host builds and custom images).
fn status_of_wifi(root: &Path, cfg: &Config) -> Result<Option<(bool, String)>> {
    if !script_path(root).is_file() {
        return Ok(None);
    }
    let output = run_script(root, cfg, "status", STATUS_TIMEOUT)?;
    if output.success() {
        Ok(Some((true, "Wi-Fi 已连接".to_string())))
    } else {
        Ok(Some((
            false,
            output
                .last_stderr()
                .map(str::to_string)
                .unwrap_or_else(|| "网络未连接".to_string()),
        )))
    }
}

/// Helper used by the `net up` CLI command: bring up unless already connected.
pub fn bring_up(
    root: &Path,
    cfg: &Config,
    timeout_override: Option<u64>,
    rep: &mut Reporter,
) -> Result<NetStatus> {
    let current = status(root, cfg)?;
    if current.connected {
        return Ok(NetStatus {
            connected: true,
            detail: "网络已连接".to_string(),
        });
    }
    if cfg.wifi().ssid.trim().is_empty() {
        return Err(LinkError::config("配置错误：wifi.ssid 为空，无法自动联网"));
    }
    let timeout_secs = timeout_override.unwrap_or(cfg.wifi().timeout_secs);
    let output = run_script(
        root,
        cfg,
        "up",
        Duration::from_secs(timeout_secs) + UP_GRACE,
    )?;
    relay_script_output(rep, &output);
    if !output.success() {
        let reason = output
            .last_stderr()
            .map(str::to_string)
            .unwrap_or_else(|| "未知原因".to_string());
        return Err(LinkError::new(ErrorKind::Net, reason));
    }
    let after = status(root, cfg)?;
    if after.connected {
        Ok(NetStatus {
            connected: true,
            detail: "已连接 Wi-Fi".to_string(),
        })
    } else {
        Err(LinkError::new(
            ErrorKind::Net,
            format!("联网后仍无法连接：{}", after.detail),
        ))
    }
}

/// Forward the script's human readable progress lines (both stdout and stderr;
/// the script writes progress to stderr, but tolerate either) to the panel.
fn relay_script_output(rep: &mut Reporter, output: &ScriptOutput) {
    for stream in [&output.stdout, &output.stderr] {
        for line in stream.lines().filter(|line| !line.trim().is_empty()) {
            let line = line.trim_end();
            if line.trim_start().starts_with("[debug]") {
                rep.debug(format!("  {}", line.trim_start()));
            } else {
                rep.progress(format!("  {line}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[cfg(unix)]
    fn test_config(url: &str, root: &Path) -> crate::config::Config {
        let mut file = crate::config::ConfigFile::default();
        file.webdav.url = url.to_string();
        crate::config::Config::from_file(file, root.join("config.toml")).unwrap()
    }

    #[test]
    fn tcp_probe_detects_open_and_closed_ports() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_probe("127.0.0.1", port, Duration::from_secs(1)));
        drop(listener);
        // Port is (very likely) closed now.
        assert!(!tcp_probe("127.0.0.1", port, Duration::from_millis(200)));
        assert!(!tcp_probe(
            "invalid.invalid.invalid",
            80,
            Duration::from_millis(200)
        ));
    }

    #[test]
    fn script_output_helpers() {
        let output = ScriptOutput {
            code: Some(1),
            stdout: "STAGE=dhcp\n".to_string(),
            stderr: "linkslot-net: no supplicant\n".to_string(),
        };
        assert!(!output.success());
        assert_eq!(output.last_stderr(), Some("linkslot-net: no supplicant"));
        let output = ScriptOutput::default();
        assert!(!output.success());
        assert_eq!(output.last_stderr(), None);
    }

    #[cfg(unix)]
    #[test]
    fn runs_fake_script_through_sh() {
        let root =
            std::env::temp_dir().join(format!("linkslot-net-{}", crate::util::unique_suffix()));
        let dir = root.join("System");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("linkslot-net.sh"),
            "#!/bin/sh\necho STAGE=associate\necho oops >&2\nexit 1\n",
        )
        .unwrap();
        let config = test_config("http://127.0.0.1:9/dav", &root);
        let output = run_script(&root, &config, "status", Duration::from_secs(2)).unwrap();
        assert_eq!(output.code, Some(1));
        assert!(output.stdout.contains("STAGE=associate"));
        assert!(output.stderr.contains("oops"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
