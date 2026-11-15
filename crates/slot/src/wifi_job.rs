//! Wi-Fi status/scan/connect jobs for the on-device settings screens.
//!
//! All device work happens out of process in `<root>/System/linkSlot/shell/slot-wifi.sh`
//! (invoked through `/bin/sh` because FAT32 cards cannot hold an executable
//! bit, exactly like `sync_job` calls `linkslot.sh`).  The script prints a
//! small machine-readable contract on stdout; progress and errors go to stderr
//! in Chinese.  This module parses that contract and never blocks the render
//! thread: the blocking variants are used from worker threads that report
//! through [`WifiJob`].
//!
//! ```text
//! status:  state=connected|disconnected / ssid=... / ip=...
//! scan:    SSID<TAB>signal<TAB>secured(1/0), strongest first, SSID-deduped
//! radio:   radio=on|off
//! connect: same as status after delegating to linkslot-net.sh up
//! ```
//!
//! Every child process is bounded: status/scan/radio get 20 s and connect
//! 60 s (the second `status` probe after connect gets its own status budget).
//! A timeout or [`WifiJob::cancel`] kills the child and reports a Chinese
//! error summary instead of hanging the settings screen.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCRIPT_NAME: &str = "slot-wifi.sh";
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const SCAN_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long to wait for the stdout/stderr reader threads after the child
/// exited; a detached grandchild may still hold the pipe, and the settings
/// screen must never hang on that.
const READER_GRACE: Duration = Duration::from_secs(1);
/// Upper bound for the stderr text embedded in an error message.
const STDERR_SUMMARY_MAX: usize = 400;

/// Current Wi-Fi state as reported by the device script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiStatus {
    Connected { ssid: String, ip: String },
    Disconnected,
}

/// One network returned by a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    /// Signal level in dBm (`wpa_cli scan_results`), e.g. `-45`; bigger is
    /// stronger.
    pub signal: i32,
    pub secured: bool,
}

/// A finished background Wi-Fi operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiMsg {
    Status(Result<WifiStatus, String>),
    Scan(Result<Vec<WifiNetwork>, String>),
    Connect(Result<WifiStatus, String>),
    /// `Ok(true)` = the radio is on, `Ok(false)` = the radio is off.
    Radio(Result<bool, String>),
}

/// A background Wi-Fi operation, polled from the frame loop.
pub struct WifiJob {
    rx: Receiver<WifiMsg>,
    cancelled: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    finished: bool,
}

impl WifiJob {
    /// Non-blocking: `None` means the job is still running (or was cancelled).
    pub fn try_recv(&mut self) -> Option<WifiMsg> {
        if self.finished {
            return None;
        }
        match self.rx.try_recv() {
            Ok(message) => {
                self.finished = true;
                Some(message)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                None
            }
        }
    }

    /// Ask the worker to kill its child process and stop.  Returns immediately;
    /// the child is signalled within one poll interval.
    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Drop for WifiJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            // Never block the UI on a stuck script: a finished worker is
            // reaped, a running one is detached and exits after the kill.
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

/// Blocking device Wi-Fi access through `System/linkSlot/shell/slot-wifi.sh`.
pub struct SystemWifi {
    root: PathBuf,
}

impl SystemWifi {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        SystemWifi { root: root.into() }
    }

    pub fn status(&mut self) -> Result<WifiStatus, String> {
        self.status_with(None)
    }

    pub fn scan(&mut self) -> Result<Vec<WifiNetwork>, String> {
        self.scan_with(None)
    }

    /// Connect and confirm with a status probe; an error is returned when no
    /// IPv4 address was obtained.
    pub fn connect(&mut self, ssid: &str, psk: &str) -> Result<WifiStatus, String> {
        self.connect_with(ssid, psk, None)
    }

    fn status_with(&mut self, cancel: Option<&AtomicBool>) -> Result<WifiStatus, String> {
        let stdout = run_script(&self.root, &["status"], STATUS_TIMEOUT, cancel)?;
        let status = parse_status(&stdout);
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("status result: {status:?}"),
        );
        status
    }

    fn scan_with(&mut self, cancel: Option<&AtomicBool>) -> Result<Vec<WifiNetwork>, String> {
        let stdout = run_script(&self.root, &["scan"], SCAN_TIMEOUT, cancel)?;
        let networks = parse_scan(&stdout);
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            match &networks {
                Ok(networks) => format!("scan result: {} networks", networks.len()),
                Err(error) => format!("scan error: {error}"),
            },
        );
        networks
    }

    fn connect_with(
        &mut self,
        ssid: &str,
        psk: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<WifiStatus, String> {
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("connect start: ssid={ssid:?}"),
        );
        let result = run_script(&self.root, &["connect", ssid, psk], CONNECT_TIMEOUT, cancel);
        if let Err(error) = result {
            crate::diag::write(
                &self.root,
                crate::linkcfg::LogLevel::Warn,
                "wifi",
                format!("connect failed: {error}"),
            );
            return Err(error);
        }
        if is_cancelled(cancel) {
            return Err("Wi-Fi 操作已取消".to_string());
        }
        let status = self.status_with(cancel)?;
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("connect finished: {status:?}"),
        );
        match status {
            WifiStatus::Connected { .. } => Ok(status),
            WifiStatus::Disconnected => {
                Err("Wi-Fi 已关联但未能获取 IP 地址，请检查网络".to_string())
            }
        }
    }

    /// True when the Wi-Fi radio is on, as reported by `radio-status`.
    ///
    /// This is a query only; use [`SystemWifi::set_radio`] to change it.
    pub fn radio_on(&mut self) -> Result<bool, String> {
        self.radio_on_with(None)
    }

    /// Turn the Wi-Fi radio on or off and return the resulting state.
    ///
    /// The script owns the rfkill/link ordering; `false` on a turn-off means
    /// the radio could not be disabled (e.g. a hardware rfkill switch).
    pub fn set_radio(&mut self, on: bool) -> Result<bool, String> {
        self.set_radio_with(on, None)
    }

    fn radio_on_with(&mut self, cancel: Option<&AtomicBool>) -> Result<bool, String> {
        let stdout = run_script(&self.root, &["radio-status"], STATUS_TIMEOUT, cancel)?;
        let result = parse_radio(&stdout);
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("radio query: {result:?}"),
        );
        result
    }

    fn set_radio_with(&mut self, on: bool, cancel: Option<&AtomicBool>) -> Result<bool, String> {
        let argument = if on { "on" } else { "off" };
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("radio request: {argument}"),
        );
        let stdout = run_script(&self.root, &["radio", argument], STATUS_TIMEOUT, cancel)?;
        let result = parse_radio(&stdout);
        crate::diag::write(
            &self.root,
            crate::linkcfg::LogLevel::Debug,
            "wifi",
            format!("radio result: {result:?}"),
        );
        result
    }
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

/// Synchronous status probe, for callers that are not on the render thread.
pub fn status_sync(root: &Path) -> Result<WifiStatus, String> {
    SystemWifi::new(root).status()
}

/// Synchronous radio state probe, for callers that are not on the render
/// thread.  `Ok(true)` = radio on.
pub fn radio_sync(root: &Path) -> Result<bool, String> {
    SystemWifi::new(root).radio_on()
}

pub fn spawn_status(root: PathBuf) -> WifiJob {
    spawn_job(root, |wifi, cancel| {
        WifiMsg::Status(wifi.status_with(cancel))
    })
}

pub fn spawn_scan(root: PathBuf) -> WifiJob {
    spawn_job(root, |wifi, cancel| WifiMsg::Scan(wifi.scan_with(cancel)))
}

pub fn spawn_connect(root: PathBuf, ssid: String, psk: String) -> WifiJob {
    spawn_job(root, move |wifi, cancel| {
        WifiMsg::Connect(wifi.connect_with(&ssid, &psk, cancel))
    })
}

pub fn spawn_radio_status(root: PathBuf) -> WifiJob {
    spawn_job(root, |wifi, cancel| {
        WifiMsg::Radio(wifi.radio_on_with(cancel))
    })
}

pub fn spawn_radio(root: PathBuf, on: bool) -> WifiJob {
    spawn_job(root, move |wifi, cancel| {
        WifiMsg::Radio(wifi.set_radio_with(on, cancel))
    })
}

fn spawn_job<F>(root: PathBuf, run: F) -> WifiJob
where
    F: FnOnce(&mut SystemWifi, Option<&AtomicBool>) -> WifiMsg + Send + 'static,
{
    let (sender, receiver) = channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let worker = std::thread::Builder::new()
        .name("slot-wifi".to_string())
        .spawn(move || {
            let mut wifi = SystemWifi::new(root);
            let message = run(&mut wifi, Some(flag.as_ref()));
            let _ = sender.send(message);
        })
        .ok();
    WifiJob {
        rx: receiver,
        cancelled,
        worker,
        finished: false,
    }
}

/// Run `<root>/System/linkSlot/shell/slot-wifi.sh <args...>` with a bounded wait.
fn run_script(
    root: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    let script = root.join(crate::root::SYSTEM_SHELL).join(SCRIPT_NAME);
    let mut command = Command::new("/bin/sh");
    command
        .arg(&script)
        .args(args)
        .env(
            "LINKSLOT_LOG_LEVEL",
            crate::linkcfg::read_log_level(root).as_str(),
        )
        .env("SLOT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::diag::write(
        root,
        crate::linkcfg::LogLevel::Debug,
        "wifi",
        format!(
            "script start: {} args={:?} timeout={}s",
            args.first().copied().unwrap_or("?"),
            args.len().saturating_sub(1),
            timeout.as_secs()
        ),
    );
    let mut child = spawn_script(&mut command)
        .map_err(|error| format!("无法启动 Wi-Fi 脚本 {}：{error}", script.display()))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || read_stream(stdout));
    let stderr_reader = std::thread::spawn(move || read_stream(stderr));

    let deadline = Instant::now() + timeout;
    let mut cancelled = false;
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => {
                terminate(&mut child);
                break None;
            }
        }
        if is_cancelled(cancel) {
            cancelled = true;
            terminate(&mut child);
            break None;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate(&mut child);
            break None;
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let (stdout, stderr) = collect_output(stdout_reader, stderr_reader);

    for line in stderr.lines().filter(|line| !line.trim().is_empty()) {
        let line = line.trim();
        if line.starts_with("[debug]") {
            crate::diag::write(
                root,
                crate::linkcfg::LogLevel::Debug,
                "wifi",
                line.trim_start_matches("[debug]").trim(),
            );
        }
    }
    crate::diag::write(
        root,
        crate::linkcfg::LogLevel::Debug,
        "wifi",
        format!(
            "script exit: {:?} stdout_lines={} stderr_lines={}",
            exit.as_ref().and_then(|status| status.code()),
            stdout.lines().count(),
            stderr.lines().count()
        ),
    );
    if cancelled {
        return Err("Wi-Fi 操作已取消".to_string());
    }
    if timed_out {
        return Err(format!("Wi-Fi 操作超时（{} 秒）", timeout.as_secs()));
    }
    let Some(status) = exit else {
        return Err("Wi-Fi 脚本被异常终止".to_string());
    };
    if !status.success() {
        return Err(failure_message(status.code(), &stderr));
    }
    Ok(stdout)
}

/// Spawn the script as the leader of its own process group.
///
/// Killing the group (timeout/cancel) reaches shell grandchildren such as
/// `sleep`, `wpa_cli` or `udhcpc`, so they release our pipes immediately.
fn spawn_script(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Join both pipe readers, but never block forever on a leaked descendant.
fn collect_output(
    stdout_reader: std::thread::JoinHandle<String>,
    stderr_reader: std::thread::JoinHandle<String>,
) -> (String, String) {
    let deadline = Instant::now() + READER_GRACE;
    loop {
        let stdout_done = stdout_reader.is_finished();
        let stderr_done = stderr_reader.is_finished();
        if stdout_done && stderr_done {
            return (
                stdout_reader.join().unwrap_or_default(),
                stderr_reader.join().unwrap_or_default(),
            );
        }
        if Instant::now() >= deadline {
            // Detach: the thread will end when the stray writer closes the
            // pipe; its output is not needed for a bounded error message.
            drop(stdout_reader);
            drop(stderr_reader);
            return (String::new(), String::new());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn read_stream<P: Read>(pipe: Option<P>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut text = String::new();
    let _ = pipe.read_to_string(&mut text);
    text
}

fn failure_message(code: Option<i32>, stderr: &str) -> String {
    let summary = parse_stderr_summary(stderr);
    match (code, summary.is_empty()) {
        (Some(code), false) => format!("Wi-Fi 脚本失败（退出码 {code}）：{summary}"),
        (Some(code), true) => format!("Wi-Fi 脚本失败（退出码 {code}）"),
        (None, false) => format!("Wi-Fi 脚本被信号终止：{summary}"),
        (None, true) => "Wi-Fi 脚本被信号终止".to_string(),
    }
}

/// Collapse stderr into one Chinese error summary, capped at ~400 chars.
pub(crate) fn parse_stderr_summary(stderr: &str) -> String {
    let mut summary = String::new();
    for line in stderr.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !summary.is_empty() {
            summary.push('；');
        }
        summary.push_str(line);
        if summary.chars().count() >= STDERR_SUMMARY_MAX {
            break;
        }
    }
    if summary.chars().count() > STDERR_SUMMARY_MAX {
        let truncated: String = summary.chars().take(STDERR_SUMMARY_MAX - 1).collect();
        return format!("{truncated}…");
    }
    summary
}

/// Parse the `status` contract.  Connected without an IPv4 address is an
/// error: the UI must not show "connected" for a link that cannot sync.
pub(crate) fn parse_status(stdout: &str) -> Result<WifiStatus, String> {
    let mut connected = None;
    let mut ssid = String::new();
    let mut ip = String::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "state" => match value {
                "connected" => connected = Some(true),
                "disconnected" => connected = Some(false),
                _ => return Err(format!("Wi-Fi 状态输出无效：state={value}")),
            },
            "ssid" => ssid = value.to_string(),
            "ip" => ip = value.to_string(),
            _ => {}
        }
    }
    match connected {
        Some(true) if ip.is_empty() => Err("Wi-Fi 已连接但未获取到 IP 地址".to_string()),
        Some(true) => Ok(WifiStatus::Connected { ssid, ip }),
        Some(false) => Ok(WifiStatus::Disconnected),
        None => Err("Wi-Fi 状态输出不完整（缺少 state）".to_string()),
    }
}

/// Parse one `SSID<TAB>signal<TAB>secured` line per network.
///
/// Malformed lines are skipped (the settings screen should still show the
/// networks that did parse); ordering and de-duplication are the script's job
/// so the UI keeps the strongest-first order it was given.
pub(crate) fn parse_scan(stdout: &str) -> Result<Vec<WifiNetwork>, String> {
    let mut networks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(ssid), Some(signal), Some(secured)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if ssid.is_empty() {
            continue;
        }
        let Ok(signal) = signal.trim().parse::<i32>() else {
            continue;
        };
        let secured = matches!(secured.trim(), "1" | "true" | "yes");
        networks.push(WifiNetwork {
            ssid: ssid.to_string(),
            signal,
            secured,
        });
    }
    Ok(networks)
}

/// Parse the `radio=on|off` contract line printed by `radio-status` and
/// `radio on|off`.
pub(crate) fn parse_radio(stdout: &str) -> Result<bool, String> {
    for line in stdout.lines() {
        let line = line.trim_end();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "radio" {
            continue;
        }
        return match value.trim() {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(format!("Wi-Fi 无线电状态输出无效：radio={other}")),
        };
    }
    Err("Wi-Fi 无线电状态输出不完整（缺少 radio）".to_string())
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_connected_and_disconnected_status() {
        assert_eq!(
            parse_status("state=connected\nssid=home\nip=192.168.1.5\n").unwrap(),
            WifiStatus::Connected {
                ssid: "home".to_string(),
                ip: "192.168.1.5".to_string(),
            }
        );
        assert_eq!(
            parse_status("state=disconnected\nssid=\nip=\n").unwrap(),
            WifiStatus::Disconnected
        );
        assert!(parse_status("garbage without state").is_err());
        assert!(parse_status("state=connected\nssid=home\n").is_err());
        assert!(parse_status("state=connecting").is_err());
    }

    #[test]
    fn parses_scan_lines_and_skips_malformed_ones() {
        let networks = parse_scan(
            "Cafe Wi-Fi\t-42\t1\nOpen\t-70\t0\ninvalid line\n\t-30\t1\nHome\tbad\t1\nstrong\t-20\t1\n",
        )
        .unwrap();
        assert_eq!(
            networks,
            vec![
                WifiNetwork {
                    ssid: "Cafe Wi-Fi".to_string(),
                    signal: -42,
                    secured: true,
                },
                WifiNetwork {
                    ssid: "Open".to_string(),
                    signal: -70,
                    secured: false,
                },
                WifiNetwork {
                    ssid: "strong".to_string(),
                    signal: -20,
                    secured: true,
                },
            ]
        );
        assert!(parse_scan("").unwrap().is_empty());
    }

    #[test]
    fn summarises_stderr_and_truncates_it() {
        assert_eq!(
            parse_stderr_summary("  \n第一行\n第二行 \n"),
            "第一行；第二行"
        );
        assert_eq!(parse_stderr_summary(""), "");
        let summary = parse_stderr_summary(&"长".repeat(1000));
        assert!(summary.chars().count() <= STDERR_SUMMARY_MAX);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn parses_radio_state() {
        assert!(parse_radio("radio=on\n").unwrap());
        assert!(!parse_radio("radio=off\n").unwrap());
        assert!(parse_radio("radio=maybe").is_err());
        assert!(parse_radio("state=connected").is_err());
        assert!(parse_radio("").is_err());
    }
}

#[cfg(all(test, unix))]
mod script_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    const CONNECTED_SCRIPT: &str = r#"#!/bin/sh
printf 'state=connected\nssid=home\nip=192.168.1.9\n'
"#;

    const DISCONNECTED_SCRIPT: &str = r#"#!/bin/sh
printf 'state=disconnected\nssid=\nip=\n'
"#;

    const SCAN_SCRIPT: &str = r#"#!/bin/sh
printf 'Cafe Wi-Fi\t-42\t1\nOpen\t-60\t0\n'
"#;

    const RADIO_ON_SCRIPT: &str = r#"#!/bin/sh
printf 'radio=on\n'
"#;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn install_script(root: &Path, body: &str) {
        let system = root.join(crate::root::SYSTEM_SHELL);
        fs::create_dir_all(&system).expect("create System dir");
        fs::write(system.join(SCRIPT_NAME), body).expect("write fake script");
    }

    fn wait_for(job: &mut WifiJob, timeout: Duration) -> Option<WifiMsg> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = job.try_recv() {
                return Some(message);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn connect_script(log: &Path, ip: &str) -> String {
        format!(
            r#"#!/bin/sh
case "$1" in
connect)
	printf '%s|%s|%s\n' "$1" "$2" "$3" > '{}'
	;;
status)
	printf 'state=connected\nssid=home\nip={}\n'
	;;
esac
"#,
            log.display(),
            ip
        )
    }

    /// Fake script: `radio-status` reports on, `radio on|off` logs the desired
    /// state and echoes it back as the resulting state.
    fn radio_script(log: &Path) -> String {
        format!(
            r#"#!/bin/sh
case "$1" in
radio)
	printf '%s\n' "$2" > '{}'
	printf 'radio=%s\n' "$2"
	;;
*)
	printf 'radio=on\n'
	;;
esac
"#,
            log.display()
        )
    }

    #[test]
    fn status_sync_parses_script_output() {
        let root = temp_root();
        install_script(root.path(), CONNECTED_SCRIPT);
        assert_eq!(
            status_sync(root.path()).unwrap(),
            WifiStatus::Connected {
                ssid: "home".to_string(),
                ip: "192.168.1.9".to_string(),
            }
        );

        install_script(root.path(), DISCONNECTED_SCRIPT);
        assert_eq!(status_sync(root.path()).unwrap(), WifiStatus::Disconnected);
    }

    #[test]
    fn scan_returns_parsed_networks() {
        let root = temp_root();
        install_script(root.path(), SCAN_SCRIPT);
        let networks = SystemWifi::new(root.path()).scan().unwrap();
        assert_eq!(
            networks,
            vec![
                WifiNetwork {
                    ssid: "Cafe Wi-Fi".to_string(),
                    signal: -42,
                    secured: true,
                },
                WifiNetwork {
                    ssid: "Open".to_string(),
                    signal: -60,
                    secured: false,
                },
            ]
        );
    }

    #[test]
    fn script_failure_reports_the_stderr_summary() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\necho '找不到无线网卡 wlan0' >&2\nexit 3\n",
        );
        let error = status_sync(root.path()).expect_err("script failed");
        assert!(error.contains("找不到无线网卡"), "unexpected error {error}");
        assert!(error.contains("退出码 3"), "unexpected error {error}");
    }

    #[test]
    fn missing_script_reports_an_error() {
        let root = temp_root();
        let error = status_sync(root.path()).expect_err("missing script");
        assert!(error.contains("Wi-Fi"), "unexpected error {error}");
    }

    #[test]
    fn connect_runs_the_script_then_probes_status() {
        let root = temp_root();
        let log = root.path().join("connect.log");
        install_script(root.path(), &connect_script(&log, "10.0.0.7"));

        let status = SystemWifi::new(root.path())
            .connect("home", "secret123")
            .expect("connect");
        assert_eq!(
            status,
            WifiStatus::Connected {
                ssid: "home".to_string(),
                ip: "10.0.0.7".to_string(),
            }
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "connect|home|secret123\n",
            "connect must pass SSID and PSK as separate argv entries"
        );
    }

    #[test]
    fn connect_errors_when_status_has_no_ip() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\ncase \"$1\" in\nconnect) exit 0 ;;\nstatus) printf 'state=connected\\nssid=home\\n' ;;\nesac\n",
        );
        let error = SystemWifi::new(root.path())
            .connect("home", "secret123")
            .expect_err("no ip");
        assert!(error.contains("IP"), "unexpected error {error}");
    }

    #[test]
    fn connect_errors_when_the_script_fails() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\necho '关联 Wi-Fi 超时（30s）' >&2\nexit 1\n",
        );
        let error = SystemWifi::new(root.path())
            .connect("home", "secret123")
            .expect_err("connect failed");
        assert!(
            error.contains("关联 Wi-Fi 超时"),
            "unexpected error {error}"
        );
    }

    #[test]
    fn spawn_status_reports_only_once() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\nsleep 0.5\nprintf 'state=connected\\nssid=home\\nip=10.0.0.7\\n'\n",
        );
        let mut job = spawn_status(root.path().to_path_buf());
        let message = wait_for(&mut job, Duration::from_secs(10)).expect("job finished");
        match message {
            WifiMsg::Status(Ok(status)) => assert_eq!(
                status,
                WifiStatus::Connected {
                    ssid: "home".to_string(),
                    ip: "10.0.0.7".to_string(),
                }
            ),
            other => panic!("unexpected message {other:?}"),
        }
        assert!(job.try_recv().is_none(), "job must report exactly once");
    }

    #[test]
    fn spawn_scan_reports_networks() {
        let root = temp_root();
        install_script(root.path(), SCAN_SCRIPT);
        let mut job = spawn_scan(root.path().to_path_buf());
        let message = wait_for(&mut job, Duration::from_secs(10)).expect("job finished");
        match message {
            WifiMsg::Scan(Ok(networks)) => {
                assert_eq!(networks.len(), 2);
                assert_eq!(networks[0].ssid, "Cafe Wi-Fi");
                assert!(networks[0].secured);
                assert!(!networks[1].secured);
            }
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[test]
    fn spawn_connect_passes_credentials_end_to_end() {
        let root = temp_root();
        let log = root.path().join("spawn-connect.log");
        install_script(root.path(), &connect_script(&log, "192.168.1.20"));
        let mut job = spawn_connect(
            root.path().to_path_buf(),
            "home".to_string(),
            "secret123".to_string(),
        );
        let message = wait_for(&mut job, Duration::from_secs(10)).expect("job finished");
        match message {
            WifiMsg::Connect(Ok(status)) => assert_eq!(
                status,
                WifiStatus::Connected {
                    ssid: "home".to_string(),
                    ip: "192.168.1.20".to_string(),
                }
            ),
            other => panic!("unexpected message {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "connect|home|secret123\n"
        );
    }

    #[test]
    fn cancel_kills_the_running_script() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\nsleep 5\nprintf 'state=disconnected\\n'\n",
        );
        let mut job = spawn_status(root.path().to_path_buf());
        std::thread::sleep(Duration::from_millis(200));
        job.cancel();
        let message = wait_for(&mut job, Duration::from_secs(5)).expect("cancelled job reports");
        match message {
            WifiMsg::Status(Err(error)) => {
                assert!(error.contains("取消"), "unexpected error {error}")
            }
            other => panic!("unexpected message {other:?}"),
        }
        drop(job);
    }

    #[test]
    fn radio_sync_reads_radio_status() {
        let root = temp_root();
        install_script(root.path(), RADIO_ON_SCRIPT);
        assert!(radio_sync(root.path()).expect("radio on"));

        install_script(root.path(), "#!/bin/sh\nprintf 'radio=off\\n'\n");
        assert!(!radio_sync(root.path()).expect("radio off"));
    }

    #[test]
    fn set_radio_runs_radio_on_and_off() {
        let root = temp_root();
        let log = root.path().join("radio.log");
        let script = radio_script(&log);
        install_script(root.path(), &script);

        let mut wifi = SystemWifi::new(root.path());
        assert!(wifi.set_radio(true).expect("radio on"));
        assert_eq!(fs::read_to_string(&log).unwrap(), "on\n");
        assert!(!wifi.set_radio(false).expect("radio off"));
        assert_eq!(fs::read_to_string(&log).unwrap(), "off\n");
    }

    #[test]
    fn radio_failure_reports_the_stderr_summary() {
        let root = temp_root();
        install_script(
            root.path(),
            "#!/bin/sh\necho '开启 WiFi 失败（rfkill 被硬件开关禁止）' >&2\nexit 1\n",
        );
        let error = SystemWifi::new(root.path())
            .set_radio(true)
            .expect_err("radio failure");
        assert!(error.contains("硬件开关"), "unexpected error {error}");
    }

    #[test]
    fn spawn_radio_status_reports_the_radio_state() {
        let root = temp_root();
        install_script(root.path(), "#!/bin/sh\nprintf 'radio=off\\n'\n");
        let mut job = spawn_radio_status(root.path().to_path_buf());
        let message = wait_for(&mut job, Duration::from_secs(10)).expect("job finished");
        match message {
            WifiMsg::Radio(Ok(on)) => assert!(!on),
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[test]
    fn spawn_radio_toggles_and_reports_state() {
        let root = temp_root();
        let log = root.path().join("spawn-radio.log");
        let script = radio_script(&log);
        install_script(root.path(), &script);

        let mut job = spawn_radio(root.path().to_path_buf(), true);
        let message = wait_for(&mut job, Duration::from_secs(10)).expect("job finished");
        match message {
            WifiMsg::Radio(Ok(on)) => assert!(on),
            other => panic!("unexpected message {other:?}"),
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), "on\n");
    }
}
