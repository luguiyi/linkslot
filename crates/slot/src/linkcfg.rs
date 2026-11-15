//! LinkSlot configuration editing for the on-device settings screens.
//!
//! The file is owned by the `linkSlot` CLI and lives at
//! `<root>/linkSlot/Config/config.toml`; `crates/linkslot/src/config.rs` holds
//! the canonical template and validation rules.  The device UI only edits a
//! handful of keys and must not re-serialize the whole document: comments,
//! unknown sections and unknown keys belong to the user and survive every
//! write.
//!
//! Reading is deliberately forgiving (missing file, malformed lines and
//! unknown values fall back to defaults) because it runs every time a settings
//! screen opens.  Writing validates first and then rewrites only the known
//! keys, using [`slot_store::atomic_write`] so a power cut cannot leave a
//! half-written TOML file on the FAT32 card.
//!
//! Secrets never round-trip through the UI: [`WebdavConfig`] and
//! [`WifiConfig`] only expose `password_set` / `psk_set`.  A write can set a
//! new password, but leaving [`WebdavEdit::password`] as `None` keeps the
//! existing one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use slot_store::atomic_write;

/// Config path under the card root (same location the CLI uses).
pub const CONFIG_REL: &str = "linkSlot/Config/config.toml";
/// Sidecar holding the named sync methods.  Absent means "legacy single
/// WebDAV from config.toml", which keeps older cards and older CLI binaries
/// working without a migration step.
pub const STORAGES_REL: &str = "linkSlot/Config/storages.toml";
pub const LOG_LEVEL_REL: &str = "linkSlot/Config/log_level.txt";
pub const DEFAULT_STORAGE_ID: &str = "default";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    pub fn allows(self, level: Self) -> bool {
        level <= self
    }
}

pub fn read_log_level(root: &Path) -> LogLevel {
    std::fs::read_to_string(root.join(LOG_LEVEL_REL))
        .ok()
        .and_then(|text| LogLevel::parse(&text))
        .unwrap_or_default()
}

pub fn write_log_level(root: &Path, level: LogLevel) -> Result<(), String> {
    let path = root.join(LOG_LEVEL_REL);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("无法创建日志目录 {}：{error}", dir.display()))?;
    }
    atomic_write(&path, format!("{}\n", level.as_str()).as_bytes())
        .map_err(|error| format!("无法写入日志级别 {}：{error}", path.display()))
}

/// A marker that lives only while a game is running or a link is open. It is
/// deleted on every clean exit, so one left behind at boot means the process
/// died mid-run (a crash, or the rail was cut); the boot log says so.
pub const LAST_RUN_REL: &str = "linkSlot/Config/last-run.txt";

pub fn read_last_run(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(LAST_RUN_REL))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub fn write_last_run(root: &Path, text: &str) {
    let path = root.join(LAST_RUN_REL);
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, text);
}

pub fn clear_last_run(root: &Path) {
    let _ = std::fs::remove_file(root.join(LAST_RUN_REL));
}

/// The name this device shows to a link peer.
///
/// Its own small file rather than a `config.toml` field: the name is set from
/// the link page on the device, the config file is edited on a PC, and a
/// one-line file cannot be corrupted into losing the WebDAV settings.
pub const DEVICE_NAME_REL: &str = "linkSlot/Config/device_name.txt";

/// Longest device name kept, in **bytes** — it rides in a fixed slot of the
/// link hello. Sixteen bytes is four Chinese characters or a short hostname.
pub const DEVICE_NAME_MAX: usize = 16;

/// What a device calls itself when nothing has been set and there is no
/// hostname to borrow.
pub const DEVICE_NAME_FALLBACK: &str = "linkSlot";

/// The name to put on the wire for this card.
pub fn read_device_name(root: &Path) -> String {
    std::fs::read_to_string(root.join(DEVICE_NAME_REL))
        .ok()
        .and_then(|text| sanitize_device_name(&text))
        .unwrap_or_else(default_device_name)
}

pub fn write_device_name(root: &Path, name: &str) -> Result<String, String> {
    let clean = sanitize_device_name(name)
        .ok_or_else(|| "设备名不能为空，且不能超过 16 字节".to_string())?;
    let path = root.join(DEVICE_NAME_REL);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("无法创建设置目录 {}：{error}", dir.display()))?;
    }
    atomic_write(&path, format!("{clean}\n").as_bytes())
        .map_err(|error| format!("无法写入设备名 {}：{error}", path.display()))?;
    Ok(clean)
}

/// Trim, drop control characters, and cut to [`DEVICE_NAME_MAX`] bytes on a
/// character boundary. `None` when nothing usable is left.
pub fn sanitize_device_name(raw: &str) -> Option<String> {
    let trimmed: String = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect();
    if trimmed.is_empty() {
        return None;
    }
    let mut end = 0;
    for (at, ch) in trimmed.char_indices() {
        let next = at + ch.len_utf8();
        if next > DEVICE_NAME_MAX {
            break;
        }
        end = next;
    }
    if end == 0 {
        // A single character wider than the slot: keep its first byte-safe
        // slice out rather than handing the wire an empty name.
        return Some(trimmed.chars().next()?.to_string());
    }
    Some(trimmed[..end].trim().to_string()).filter(|name| !name.is_empty())
}

/// The host name, which is usually the model plus a serial suffix.
pub fn default_device_name() -> String {
    let host = |path: &str| std::fs::read_to_string(path).ok();
    let raw = host("/etc/hostname")
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_default();
    sanitize_device_name(&raw).unwrap_or_else(|| DEVICE_NAME_FALLBACK.to_string())
}

/// `linkSlot`'s own default for `webdav.remote_root`.
const DEFAULT_REMOTE_ROOT: &str = "/linkSlot";
/// Default idle minutes before the device turns Wi-Fi off (`0` = never).
const DEFAULT_AUTO_OFF_MINUTES: u16 = 10;
/// How long the fallback `linkSlot config init` may take before it is killed.
const INIT_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// WebDAV part of the config, as shown by the settings screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebdavConfig {
    pub url: String,
    pub remote_root: String,
    pub username: String,
    /// True when the file holds a non-empty password (never the password).
    pub password_set: bool,
    pub ca_cert: String,
    pub insecure: bool,
    pub keep_remote: bool,
}

impl Default for WebdavConfig {
    fn default() -> Self {
        WebdavConfig {
            url: String::new(),
            remote_root: DEFAULT_REMOTE_ROOT.to_string(),
            username: String::new(),
            password_set: false,
            ca_cert: String::new(),
            insecure: false,
            keep_remote: false,
        }
    }
}

/// A named sync method as shown in the Web page.  Only one can be active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageProfile {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub remote_root: String,
    pub username: String,
    /// True when the sidecar holds a non-empty password (never the password).
    pub password_set: bool,
    pub ca_cert: String,
    pub insecure: bool,
}

/// Editable form of one profile.  `password: None` keeps the stored one;
/// `Some("")` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageEdit {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub remote_root: String,
    pub username: String,
    pub password: Option<String>,
    pub ca_cert: String,
    pub insecure: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StorageFile {
    #[serde(default)]
    active: String,
    #[serde(default)]
    storage: Vec<StorageEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct StorageEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    remote_root: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    ca_cert: String,
    #[serde(default)]
    insecure: bool,
}

/// Wi-Fi part of the config, as shown by the settings screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiConfig {
    pub ssid: String,
    /// True when the file holds a non-empty PSK (never the PSK).
    pub psk_set: bool,
    pub manage: bool,
    /// Idle minutes before the device turns Wi-Fi off; `0` means never.
    pub auto_off_minutes: u16,
}

impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            ssid: String::new(),
            psk_set: false,
            manage: true,
            auto_off_minutes: DEFAULT_AUTO_OFF_MINUTES,
        }
    }
}

pub const WIFI_SAVED_REL: &str = "linkSlot/Config/wifi_saved.toml";

/// A saved Wi-Fi network as shown by the device and Web settings screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedWifi {
    pub ssid: String,
    /// True when a non-empty password is stored (never the password).
    pub password_set: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct WifiSavedFile {
    #[serde(default)]
    active: String,
    #[serde(default)]
    saved: Vec<WifiSavedEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct WifiSavedEntry {
    #[serde(default)]
    ssid: String,
    #[serde(default)]
    psk: String,
}

/// Selectable `wifi.auto_off_minutes` values; `0` means "never turn off".
pub const WIFI_AUTO_OFF_CHOICES: [u16; 8] = [1, 3, 5, 10, 30, 60, 120, 0];

fn normalized_auto_off(minutes: u16) -> u16 {
    if WIFI_AUTO_OFF_CHOICES.contains(&minutes) {
        minutes
    } else {
        DEFAULT_AUTO_OFF_MINUTES
    }
}

/// Human-readable label for an `auto_off_minutes` choice.
///
/// Unknown values are treated as the default 10 minutes.
pub fn auto_off_label(minutes: u16) -> &'static str {
    match normalized_auto_off(minutes) {
        1 => "1 分钟",
        3 => "3 分钟",
        5 => "5 分钟",
        30 => "30 分钟",
        60 => "1 小时",
        120 => "2 小时",
        0 => "永不",
        _ => "10 分钟",
    }
}

/// Next/previous [`WIFI_AUTO_OFF_CHOICES`] entry (wraps around).
///
/// Unknown values first fall back to the default 10 minutes.
pub fn next_auto_off(minutes: u16, right: bool) -> u16 {
    let current = normalized_auto_off(minutes);
    let count = WIFI_AUTO_OFF_CHOICES.len();
    let index = WIFI_AUTO_OFF_CHOICES
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(1);
    let next = if right {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    };
    WIFI_AUTO_OFF_CHOICES[next]
}

/// What a sync run includes, from `[sync] mode` in `config.toml`.
///
/// `All` is both the default and the fallback for unknown values, so an older
/// or hand-edited config never disables a unit by accident.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncMode {
    /// Both units (`all`).
    #[default]
    All,
    /// 即时存档 only.
    State,
    /// 电池存档 only.
    Sav,
}

impl SyncMode {
    pub const ALL: [SyncMode; 3] = [SyncMode::All, SyncMode::State, SyncMode::Sav];

    /// The TOML value (`all` / `state` / `sav`).
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::All => "all",
            SyncMode::State => "state",
            SyncMode::Sav => "sav",
        }
    }

    pub fn parse(text: &str) -> Option<SyncMode> {
        match text.trim() {
            "all" => Some(SyncMode::All),
            "state" => Some(SyncMode::State),
            "sav" => Some(SyncMode::Sav),
            _ => None,
        }
    }

    /// Chinese label for the QuickMenu row.
    pub fn label(self) -> &'static str {
        match self {
            SyncMode::All => "全部",
            SyncMode::State => "仅即时存档",
            SyncMode::Sav => "仅电池存档",
        }
    }

    /// Next/previous option (wraps around).
    pub fn next(self, right: bool) -> SyncMode {
        let index = SyncMode::ALL
            .iter()
            .position(|mode| *mode == self)
            .unwrap_or(0);
        let count = SyncMode::ALL.len();
        let next = if right {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        SyncMode::ALL[next]
    }

    /// Whether the state unit participates in a sync of this mode.
    pub fn state_enabled(self) -> bool {
        !matches!(self, SyncMode::Sav)
    }

    /// Whether the sav unit participates in a sync of this mode.
    pub fn sav_enabled(self) -> bool {
        !matches!(self, SyncMode::State)
    }
}

/// `[sync]` settings the device UI can change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncConfig {
    pub mode: SyncMode,
}

/// Everything the settings screens read at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkConfig {
    /// The active profile, kept as the existing single-WebDAV view so the
    /// device settings screen and cache matching need no second path.
    pub webdav: WebdavConfig,
    pub storages: Vec<StorageProfile>,
    pub active_storage: String,
    pub storages_warning: Option<String>,
    pub wifi: WifiConfig,
    pub sync: SyncConfig,
}

/// Editable WebDAV fields.
///
/// `password: None` keeps the password already in the file; `Some(value)`
/// replaces it, so `Some(String::new())` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebdavEdit {
    pub url: String,
    pub remote_root: String,
    pub username: String,
    pub password: Option<String>,
    pub ca_cert: String,
    pub insecure: bool,
    pub keep_remote: bool,
}

/// Read the config, falling back to defaults for a missing/unreadable file.
///
/// This never panics and never writes to the card.
pub fn read(root: &Path) -> LinkConfig {
    let mut config = match std::fs::read_to_string(config_path(root)) {
        Ok(text) => parse_document(&text),
        Err(_) => LinkConfig::default(),
    };
    match read_storage_file(root) {
        Ok(Some(file)) if !file.storage.is_empty() => {
            if let Err(warning) = apply_storage_file(&mut config, file) {
                config.storages_warning = Some(warning);
            }
        }
        Ok(_) => {}
        Err(warning) => config.storages_warning = Some(warning),
    }
    if config.storages.is_empty() {
        config.storages = vec![default_storage(&config.webdav)];
        config.active_storage = DEFAULT_STORAGE_ID.to_string();
    }
    config
}

fn storage_path(root: &Path) -> PathBuf {
    root.join(STORAGES_REL)
}

fn read_storage_file(root: &Path) -> Result<Option<StorageFile>, String> {
    let path = storage_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("无法读取 {}：{err}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    toml::from_str(&text)
        .map(Some)
        .map_err(|err| format!("{} 解析失败：{err}", path.display()))
}

/// Password for one profile, for an in-memory connection test.  The Web page
/// never receives it in `/api/sync`.
pub fn storage_password(root: &Path, id: &str) -> Option<String> {
    if let Ok(Some(file)) = read_storage_file(root) {
        if let Some(entry) = file.storage.iter().find(|entry| entry.id == id) {
            return Some(entry.password.clone());
        }
    }
    if id.is_empty() || id == DEFAULT_STORAGE_ID {
        return Some(existing_raw(root).password);
    }
    None
}

/// Make one saved profile active while preserving its password and mirroring
/// it into the legacy `[webdav]` section used by older CLI versions.
pub fn activate_storage(root: &Path, id: &str) -> Result<(), String> {
    let id = id.trim();
    let mut file = read_storage_file(root)?
        .ok_or_else(|| "卡内还没有命名的同步方式".to_string())?;
    let at = file
        .storage
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| format!("同步方式「{id}」不存在"))?;
    let entry = &file.storage[at];
    if entry.kind != "webdav" {
        return Err(format!("同步方式「{id}」类型 {} 暂不支持", entry.kind));
    }
    validate_storage_entry(entry)?;
    let keep_remote = read(root).webdav.keep_remote;
    file.active = id.to_string();
    write_storage_file(root, &file)?;
    let active = file
        .storage
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("同步方式「{id}」不存在"))?;
    write_legacy_webdav(
        root,
        WebdavEdit {
            url: active.url.clone(),
            remote_root: active.remote_root.clone(),
            username: active.username.clone(),
            password: Some(active.password.clone()),
            ca_cert: active.ca_cert.clone(),
            insecure: active.insecure,
            keep_remote,
        },
    )
}

/// A temporary `config.toml` for testing a non-active WebDAV profile without
/// changing the card's real configuration.
pub struct TestConfig {
    dir: PathBuf,
    path: PathBuf,
}

impl TestConfig {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Serialize)]
struct TestDocument<'a> {
    webdav: TestWebdav<'a>,
    sync: TestSync,
}

#[derive(Serialize)]
struct TestWebdav<'a> {
    url: &'a str,
    remote_root: &'a str,
    username: &'a str,
    password: &'a str,
    ca_cert: &'a str,
    insecure: bool,
}

#[derive(Serialize)]
struct TestSync {
    timeout_secs: u64,
}

/// Build a temporary config for the selected profile.  The CLI is intentionally
/// tested with the same `[webdav]` schema it will use after the switch.
pub fn profile_test_config(root: &Path, id: &str) -> Result<TestConfig, String> {
    let id = id.trim();
    let file = read_storage_file(root)?
        .ok_or_else(|| "卡内还没有命名的同步方式".to_string())?;
    let entry = file
        .storage
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("同步方式「{id}」不存在"))?;
    if entry.kind != "webdav" {
        return Err(format!("同步方式「{id}」类型 {} 暂不支持", entry.kind));
    }
    validate_storage_entry(entry)?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "linkslot-test-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("无法创建测试配置目录 {}：{error}", dir.display()))?;
    let path = dir.join("config.toml");
    let document = TestDocument {
        webdav: TestWebdav {
            url: &entry.url,
            remote_root: &entry.remote_root,
            username: &entry.username,
            password: &entry.password,
            ca_cert: &entry.ca_cert,
            insecure: entry.insecure,
        },
        sync: TestSync { timeout_secs: 20 },
    };
    let text = toml::to_string(&document)
        .map_err(|error| format!("无法生成测试配置：{error}"))?;
    atomic_write(&path, text.as_bytes())
        .map_err(|error| format!("无法写入测试配置 {}：{error}", path.display()))?;
    Ok(TestConfig { dir, path })
}

fn default_storage(webdav: &WebdavConfig) -> StorageProfile {
    StorageProfile {
        id: DEFAULT_STORAGE_ID.to_string(),
        name: "默认 WebDAV".to_string(),
        kind: "webdav".to_string(),
        url: webdav.url.clone(),
        remote_root: webdav.remote_root.clone(),
        username: webdav.username.clone(),
        password_set: webdav.password_set,
        ca_cert: webdav.ca_cert.clone(),
        insecure: webdav.insecure,
    }
}

fn profile_from_entry(entry: &StorageEntry) -> StorageProfile {
    StorageProfile {
        id: entry.id.clone(),
        name: entry.name.clone(),
        kind: entry.kind.clone(),
        url: entry.url.clone(),
        remote_root: entry.remote_root.clone(),
        username: entry.username.clone(),
        password_set: !entry.password.is_empty(),
        ca_cert: entry.ca_cert.clone(),
        insecure: entry.insecure,
    }
}

fn apply_storage_file(config: &mut LinkConfig, file: StorageFile) -> Result<(), String> {
    let active = file
        .storage
        .iter()
        .find(|entry| entry.id == file.active)
        .ok_or_else(|| {
            format!(
                "{} 的 active=\"{}\" 不存在，已回退旧 WebDAV 配置",
                STORAGES_REL, file.active
            )
        })?;
    if active.kind != "webdav" {
        return Err(format!(
            "{} 的生效配置 kind=\"{}\" 暂不支持，已回退旧 WebDAV 配置",
            STORAGES_REL, active.kind
        ));
    }
    let webdav = WebdavConfig {
        url: active.url.clone(),
        remote_root: if active.remote_root.is_empty() {
            DEFAULT_REMOTE_ROOT.to_string()
        } else {
            active.remote_root.clone()
        },
        username: active.username.clone(),
        password_set: !active.password.is_empty(),
        ca_cert: active.ca_cert.clone(),
        insecure: active.insecure,
        keep_remote: config.webdav.keep_remote,
    };
    config.storages = file.storage.iter().map(profile_from_entry).collect();
    config.active_storage = file.active;
    config.webdav = webdav;
    Ok(())
}

/// Validate + save the WebDAV/sync settings, preserving everything else.
/// Validate + save the active WebDAV profile and its legacy `[webdav]` mirror.
pub fn write_webdav(root: &Path, edit: WebdavEdit) -> Result<(), String> {
    let Some(mut file) = read_storage_file(root)? else {
        return write_legacy_webdav(root, edit);
    };
    if file.storage.is_empty() {
        return write_legacy_webdav(root, edit);
    }
    let at = file
        .storage
        .iter()
        .position(|entry| entry.id == file.active)
        .ok_or_else(|| format!("{} 的 active 配置不存在", STORAGES_REL))?;
    let mirror = {
        let entry = &mut file.storage[at];
        if entry.kind != "webdav" {
            return Err(format!("生效配置 kind=\"{}\" 暂不支持", entry.kind));
        }
        entry.url = edit.url.trim().to_string();
        entry.remote_root = normalize_remote_root(&edit.remote_root)?;
        entry.username = edit.username.trim().to_string();
        if let Some(password) = &edit.password {
            entry.password = password.clone();
        }
        entry.ca_cert = edit.ca_cert.trim().to_string();
        entry.insecure = edit.insecure;
        validate_storage_entry(entry)?;
        WebdavEdit {
            url: entry.url.clone(),
            remote_root: entry.remote_root.clone(),
            username: entry.username.clone(),
            password: Some(entry.password.clone()),
            ca_cert: entry.ca_cert.clone(),
            insecure: entry.insecure,
            keep_remote: edit.keep_remote,
        }
    };
    write_storage_file(root, &file)?;
    write_legacy_webdav(root, mirror)
}

/// Validate and save a complete profile list.  Exactly one id must be active;
/// the active profile is mirrored into the legacy `[webdav]` section.
pub fn save_storages(
    root: &Path,
    active: &str,
    edits: &[StorageEdit],
    keep_remote: bool,
) -> Result<(), String> {
    let active = active.trim();
    if edits.is_empty() {
        return Err("至少保留一个同步方式".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for edit in edits {
        let id = edit.id.trim();
        if !valid_storage_id(id) {
            return Err(format!("同步方式 id「{id}」无效"));
        }
        if !ids.insert(id.to_string()) {
            return Err(format!("同步方式 id「{id}」重复"));
        }
        let name = edit.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("同步方式名称必须为 1–64 个字符".to_string());
        }
        if !names.insert(name.to_string()) {
            return Err(format!("同步方式名称「{name}」重复"));
        }
    }
    if !ids.contains(active) {
        return Err(format!("生效同步方式「{active}」不存在"));
    }

    let existing = read_storage_file(root)?.unwrap_or_default();
    let mut entries = Vec::new();
    for edit in edits {
        let old = existing
            .storage
            .iter()
            .find(|entry| entry.id == edit.id.trim());
        let mut entry = StorageEntry {
            id: edit.id.trim().to_string(),
            name: edit.name.trim().to_string(),
            kind: edit.kind.trim().to_string(),
            url: edit.url.trim().to_string(),
            remote_root: normalize_remote_root(&edit.remote_root)?,
            username: edit.username.trim().to_string(),
            password: match &edit.password {
                Some(password) => password.clone(),
                None => old.map(|entry| entry.password.clone()).unwrap_or_default(),
            },
            ca_cert: edit.ca_cert.trim().to_string(),
            insecure: edit.insecure,
        };
        if entry.kind.is_empty() {
            entry.kind = "webdav".to_string();
        }
        if entry.id == active && entry.kind != "webdav" {
            return Err(format!("生效同步方式 kind=\"{}\" 暂不支持", entry.kind));
        }
        if entry.kind == "webdav" || entry.id == active {
            validate_storage_entry(&entry)?;
        }
        entries.push(entry);
    }

    let file = StorageFile {
        active: active.to_string(),
        storage: entries,
    };
    write_storage_file(root, &file)?;
    let active_entry = file
        .storage
        .iter()
        .find(|entry| entry.id == active)
        .ok_or_else(|| format!("生效同步方式「{active}」不存在"))?;
    write_legacy_webdav(
        root,
        WebdavEdit {
            url: active_entry.url.clone(),
            remote_root: active_entry.remote_root.clone(),
            username: active_entry.username.clone(),
            password: Some(active_entry.password.clone()),
            ca_cert: active_entry.ca_cert.clone(),
            insecure: active_entry.insecure,
            keep_remote,
        },
    )
}

fn valid_storage_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && id.chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric())
}

fn validate_storage_entry(entry: &StorageEntry) -> Result<(), String> {
    let url = validate_url(&entry.url)?;
    normalize_remote_root(&entry.remote_root)?;
    validate_ca_cert(entry.ca_cert.trim())?;
    if entry.username.is_empty() && !entry.password.is_empty() {
        return Err("已设置 WebDAV 密码时，用户名不能为空".to_string());
    }
    if url.is_empty() {
        return Err("WebDAV 地址不能为空".to_string());
    }
    Ok(())
}

fn write_storage_file(root: &Path, file: &StorageFile) -> Result<(), String> {
    let text = toml::to_string_pretty(file)
        .map_err(|error| format!("无法生成 {}：{error}", STORAGES_REL))?;
    let path = storage_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("无法创建配置目录 {}：{error}", dir.display()))?;
    }
    atomic_write(&path, text.as_bytes())
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

fn write_legacy_webdav(root: &Path, edit: WebdavEdit) -> Result<(), String> {
    let url = validate_url(&edit.url)?;
    let remote_root = normalize_remote_root(&edit.remote_root)?;
    let username = edit.username.trim().to_string();
    let ca_cert = edit.ca_cert.trim().to_string();
    validate_ca_cert(&ca_cert)?;

    let current = existing_raw(root);
    let password = match &edit.password {
        Some(password) => password.clone(),
        None => current.password.clone(),
    };
    if username.is_empty() && !password.is_empty() {
        return Err("已设置 WebDAV 密码时，用户名不能为空".to_string());
    }

    let text = ensure_config(root)?;
    let mut edits = vec![
        Edit {
            section: Section::Webdav,
            key: "url",
            value: EditValue::Str(&url),
        },
        Edit {
            section: Section::Webdav,
            key: "remote_root",
            value: EditValue::Str(&remote_root),
        },
        Edit {
            section: Section::Webdav,
            key: "username",
            value: EditValue::Str(&username),
        },
    ];
    if let Some(password) = &edit.password {
        edits.push(Edit {
            section: Section::Webdav,
            key: "password",
            value: EditValue::Str(password),
        });
    }
    edits.push(Edit {
        section: Section::Webdav,
        key: "ca_cert",
        value: EditValue::Str(&ca_cert),
    });
    edits.push(Edit {
        section: Section::Webdav,
        key: "insecure",
        value: EditValue::Bool(edit.insecure),
    });
    edits.push(Edit {
        section: Section::Sync,
        key: "keep_remote",
        value: EditValue::Bool(edit.keep_remote),
    });

    let updated = apply_edits(&text, &edits);
    write_config(root, &updated)
}

/// Validate + save the Wi-Fi settings and switch `wifi.manage` on.
///
/// An empty `psk` means an open network.
pub fn write_wifi(root: &Path, ssid: &str, psk: &str) -> Result<(), String> {
    validate_ssid(ssid)?;
    validate_psk(psk)?;

    let text = ensure_config(root)?;
    let edits = [
        Edit {
            section: Section::Wifi,
            key: "manage",
            value: EditValue::Bool(true),
        },
        Edit {
            section: Section::Wifi,
            key: "ssid",
            value: EditValue::Str(ssid),
        },
        Edit {
            section: Section::Wifi,
            key: "psk",
            value: EditValue::Str(psk),
        },
    ];
    let updated = apply_edits(&text, &edits);
    write_config(root, &updated)?;
    if let Err(error) = remember_saved_wifi(root, ssid, psk) {
        eprintln!("slot: saved Wi-Fi list: {error}");
    }
    Ok(())
}

/// Saved Wi-Fi networks, newest active selection first.
pub fn read_saved_wifi(root: &Path) -> Vec<SavedWifi> {
    let mut out = Vec::new();
    if let Ok(Some(file)) = read_wifi_saved_file(root) {
        for entry in file.saved {
            if entry.ssid.is_empty() {
                continue;
            }
            out.push(SavedWifi {
                ssid: entry.ssid,
                password_set: !entry.psk.is_empty(),
            });
        }
    }
    let raw = existing_raw(root);
    if !raw.ssid.is_empty() && !out.iter().any(|saved| saved.ssid == raw.ssid) {
        // A card used with the single-network UI before the list existed.
        out.push(SavedWifi {
            ssid: raw.ssid,
            password_set: !raw.psk.is_empty(),
        });
    }
    out
}

/// Password for selecting a saved network; never returned by the read view.
pub fn saved_wifi_password(root: &Path, ssid: &str) -> Option<String> {
    if let Ok(Some(file)) = read_wifi_saved_file(root) {
        if let Some(entry) = file.saved.iter().find(|entry| entry.ssid == ssid) {
            return Some(entry.psk.clone());
        }
    }
    let raw = existing_raw(root);
    (raw.ssid == ssid).then_some(raw.psk)
}

/// Make a saved network the active `[wifi]` profile and connect to it.
pub fn activate_saved_wifi(root: &Path, ssid: &str) -> Result<(), String> {
    let psk = saved_wifi_password(root, ssid)
        .ok_or_else(|| format!("已保存 WiFi「{ssid}」不存在"))?;
    write_wifi(root, ssid, &psk)
}

fn wifi_saved_path(root: &Path) -> PathBuf {
    root.join(WIFI_SAVED_REL)
}

fn read_wifi_saved_file(root: &Path) -> Result<Option<WifiSavedFile>, String> {
    let path = wifi_saved_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("无法读取 {}：{err}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    toml::from_str(&text)
        .map(Some)
        .map_err(|err| format!("{} 解析失败：{err}", path.display()))
}

fn save_wifi_saved_file(root: &Path, file: &WifiSavedFile) -> Result<(), String> {
    let text = toml::to_string_pretty(file)
        .map_err(|error| format!("无法生成 {}：{error}", WIFI_SAVED_REL))?;
    let path = wifi_saved_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("无法创建配置目录 {}：{error}", dir.display()))?;
    }
    atomic_write(&path, text.as_bytes())
        .map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

fn remember_saved_wifi(root: &Path, ssid: &str, psk: &str) -> Result<(), String> {
    let mut file = read_wifi_saved_file(root)?.unwrap_or_default();
    file.saved.retain(|entry| entry.ssid != ssid);
    file.saved.insert(
        0,
        WifiSavedEntry {
            ssid: ssid.to_string(),
            psk: psk.to_string(),
        },
    );
    file.saved.truncate(16);
    file.active = ssid.to_string();
    save_wifi_saved_file(root, &file)
}

/// Validate + save `wifi.auto_off_minutes`, preserving every other key.
///
/// Only [`WIFI_AUTO_OFF_CHOICES`] are accepted; `0` means "never turn off".
pub fn write_wifi_auto_off(root: &Path, minutes: u16) -> Result<(), String> {
    if !WIFI_AUTO_OFF_CHOICES.contains(&minutes) {
        return Err(format!(
            "WiFi 自动关闭时间无效：{minutes} 分钟（可选 5/10/30/60/120/0）"
        ));
    }
    let text = ensure_config(root)?;
    let edits = [Edit {
        section: Section::Wifi,
        key: "auto_off_minutes",
        value: EditValue::Int(minutes),
    }];
    let updated = apply_edits(&text, &edits);
    write_config(root, &updated)
}

/// Validate + save `[sync] mode`, preserving every other key and section.
///
/// A missing `[sync]` section is appended with just this key; other sections
/// (WebDAV, WiFi, unknown keys/comments) survive untouched.
pub fn write_sync_mode(root: &Path, mode: SyncMode) -> Result<(), String> {
    let text = ensure_config(root)?;
    let edits = [Edit {
        section: Section::Sync,
        key: "mode",
        value: EditValue::Str(mode.as_str()),
    }];
    let updated = apply_edits(&text, &edits);
    write_config(root, &updated)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Values parsed from the file, including the secrets `read` hides.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawConfig {
    url: String,
    remote_root: Option<String>,
    username: String,
    password: String,
    ca_cert: String,
    insecure: bool,
    keep_remote: bool,
    mode: Option<SyncMode>,
    ssid: String,
    psk: String,
    manage: Option<bool>,
    auto_off_minutes: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Other,
    Webdav,
    Device,
    Sync,
    Wifi,
}

impl Section {
    fn from_header(name: &str) -> Section {
        match name {
            "webdav" => Section::Webdav,
            "device" => Section::Device,
            "sync" => Section::Sync,
            "wifi" => Section::Wifi,
            _ => Section::Other,
        }
    }

    fn header(self) -> &'static str {
        match self {
            Section::Webdav => "webdav",
            Section::Device => "device",
            Section::Sync => "sync",
            Section::Wifi => "wifi",
            Section::Other => "",
        }
    }
}

const SECTION_ORDER: [Section; 4] = [
    Section::Webdav,
    Section::Device,
    Section::Sync,
    Section::Wifi,
];

/// Parse a config body into the public view (no secrets).
pub(crate) fn parse_document(text: &str) -> LinkConfig {
    let raw = parse_raw(text);
    LinkConfig {
        webdav: WebdavConfig {
            url: raw.url,
            remote_root: raw
                .remote_root
                .unwrap_or_else(|| DEFAULT_REMOTE_ROOT.to_string()),
            username: raw.username,
            password_set: !raw.password.is_empty(),
            ca_cert: raw.ca_cert,
            insecure: raw.insecure,
            keep_remote: raw.keep_remote,
        },
        storages: Vec::new(),
        active_storage: String::new(),
        storages_warning: None,
        wifi: WifiConfig {
            ssid: raw.ssid,
            psk_set: !raw.psk.is_empty(),
            manage: raw.manage.unwrap_or(true),
            auto_off_minutes: raw.auto_off_minutes.unwrap_or(DEFAULT_AUTO_OFF_MINUTES),
        },
        sync: SyncConfig {
            mode: raw.mode.unwrap_or_default(),
        },
    }
}

/// Parse a config body, keeping secrets for internal use.
pub(crate) fn parse_raw(text: &str) -> RawConfig {
    let mut raw = RawConfig::default();
    let mut section = Section::Other;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = header_name(line) {
            section = Section::from_header(name);
            continue;
        }
        let Some((key, value)) = key_value(line) else {
            continue;
        };
        match section {
            Section::Webdav => match key {
                "url" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.url = value;
                    }
                }
                "remote_root" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.remote_root = Some(value);
                    }
                }
                "username" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.username = value;
                    }
                }
                "password" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.password = value;
                    }
                }
                "ca_cert" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.ca_cert = value;
                    }
                }
                "insecure" => {
                    if let Some(value) = parse_toml_bool(value) {
                        raw.insecure = value;
                    }
                }
                _ => {}
            },
            Section::Sync => match key {
                "keep_remote" => {
                    if let Some(value) = parse_toml_bool(value) {
                        raw.keep_remote = value;
                    }
                }
                "mode" => {
                    if let Some(value) = parse_toml_string(value) {
                        if let Some(mode) = SyncMode::parse(&value) {
                            raw.mode = Some(mode);
                        } else {
                            // v0.0.10 §8.1: unknown values fall back to `all`
                            // but leave a trace in the device log.
                            eprintln!("slot: [sync] mode 无效（{value:?}），回落 all");
                        }
                    }
                }
                _ => {}
            },
            Section::Wifi => match key {
                "ssid" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.ssid = value;
                    }
                }
                "psk" => {
                    if let Some(value) = parse_toml_string(value) {
                        raw.psk = value;
                    }
                }
                "manage" => {
                    if let Some(value) = parse_toml_bool(value) {
                        raw.manage = Some(value);
                    }
                }
                "auto_off_minutes" => {
                    if let Some(minutes) = parse_toml_int(value) {
                        if WIFI_AUTO_OFF_CHOICES.contains(&minutes) {
                            raw.auto_off_minutes = Some(minutes);
                        }
                    }
                }
                _ => {}
            },
            Section::Device | Section::Other => {}
        }
    }
    raw
}

fn header_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    // `[[array-of-tables]]` and inline tables are not section headers.
    if rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    let name = rest[..end].trim();
    let tail = rest[end + 1..].trim();
    if !tail.is_empty() && !tail.starts_with('#') {
        return None;
    }
    Some(name)
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(is_bare_key_char) {
        return None;
    }
    Some((key, value.trim_start()))
}

fn is_bare_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn parse_toml_string(raw: &str) -> Option<String> {
    let raw = raw.trim_start();
    if let Some(rest) = raw.strip_prefix('"') {
        parse_basic_string(rest)
    } else if let Some(rest) = raw.strip_prefix('\'') {
        parse_literal_string(rest)
    } else {
        None
    }
}

fn parse_basic_string(rest: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        let ch = chars.next()?;
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => out.push(hex_char(&mut chars, 4)?),
                'U' => out.push(hex_char(&mut chars, 8)?),
                _ => return None,
            },
            // A raw control character is invalid TOML; ignore the line.
            ch if ch.is_control() && ch != '\t' => return None,
            ch => out.push(ch),
        }
    }
}

fn hex_char(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<char> {
    let mut code = 0u32;
    for _ in 0..digits {
        code = code * 16 + chars.next()?.to_digit(16)?;
    }
    char::from_u32(code)
}

fn parse_literal_string(rest: &str) -> Option<String> {
    let end = rest.find('\'')?;
    let value = &rest[..end];
    if value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

fn parse_toml_bool(raw: &str) -> Option<bool> {
    let token = raw
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == '#')
        .next()
        .unwrap_or("");
    match token {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a plain non-negative decimal TOML integer (e.g. `30`).
fn parse_toml_int(raw: &str) -> Option<u16> {
    let token = raw
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == '#')
        .next()
        .unwrap_or("");
    token.parse::<u16>().ok()
}

fn existing_raw(root: &Path) -> RawConfig {
    match std::fs::read_to_string(config_path(root)) {
        Ok(text) => parse_raw(&text),
        Err(_) => RawConfig::default(),
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum EditValue<'a> {
    Str(&'a str),
    Bool(bool),
    Int(u16),
}

impl EditValue<'_> {
    fn render(self) -> String {
        match self {
            EditValue::Str(value) => toml_quote(value),
            EditValue::Bool(value) => {
                if value {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            EditValue::Int(value) => value.to_string(),
        }
    }
}

struct Edit<'a> {
    section: Section,
    key: &'a str,
    value: EditValue<'a>,
}

/// Rewrite only the edited keys, keeping comments/unknown keys/sections.
///
/// Missing keys are appended at the end of their section; a section that does
/// not exist at all is appended (with just the edited keys) at the end of the
/// file.  Line endings are normalised to LF.
fn apply_edits(text: &str, edits: &[Edit<'_>]) -> String {
    let body = text.trim_end_matches(['\n', '\r']);
    let mut lines: Vec<String> = Vec::new();
    let mut applied = vec![false; edits.len()];
    let mut current = Section::Other;

    for line in body.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim();
        if let Some(name) = header_name(trimmed) {
            flush_section(current, edits, &mut applied, &mut lines);
            current = Section::from_header(name);
            lines.push(line.to_string());
            continue;
        }
        if !trimmed.starts_with('#') {
            if let Some((key, _)) = key_value(trimmed) {
                let mut replaced = false;
                for (index, edit) in edits.iter().enumerate() {
                    if !applied[index] && edit.section == current && edit.key == key {
                        lines.push(format!("{} = {}", edit.key, edit.value.render()));
                        applied[index] = true;
                        replaced = true;
                        break;
                    }
                }
                if replaced {
                    continue;
                }
            }
        }
        lines.push(line.to_string());
    }
    flush_section(current, edits, &mut applied, &mut lines);

    for section in SECTION_ORDER {
        let mut wrote_header = false;
        for (index, edit) in edits.iter().enumerate() {
            if applied[index] || edit.section != section {
                continue;
            }
            if !wrote_header {
                if !lines.is_empty()
                    && !lines
                        .last()
                        .map(|line| line.trim().is_empty())
                        .unwrap_or(true)
                {
                    lines.push(String::new());
                }
                lines.push(format!("[{}]", section.header()));
                wrote_header = true;
            }
            lines.push(format!("{} = {}", edit.key, edit.value.render()));
            applied[index] = true;
        }
    }

    let mut updated = lines.join("\n");
    updated.push('\n');
    updated
}

fn flush_section(
    section: Section,
    edits: &[Edit<'_>],
    applied: &mut [bool],
    lines: &mut Vec<String>,
) {
    for (index, edit) in edits.iter().enumerate() {
        if !applied[index] && edit.section == section {
            lines.push(format!("{} = {}", edit.key, edit.value.render()));
            applied[index] = true;
        }
    }
}

/// Encode a value as a TOML basic string.
pub(crate) fn toml_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                out.push_str(&format!("\\u{:04X}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Make sure a config file exists (CLI `config init` first, built-in template
/// second) and return its current text.
fn ensure_config(root: &Path) -> Result<String, String> {
    let path = config_path(root);
    if let Some(text) = read_non_empty(&path) {
        return Ok(text);
    }
    let _ = init_via_cli(root);
    if let Some(text) = read_non_empty(&path) {
        return Ok(text);
    }
    write_config(root, MINIMAL_TEMPLATE)?;
    Ok(MINIMAL_TEMPLATE.to_string())
}

fn read_non_empty(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Ask `System/linkSlot/shell/linkslot.sh config init` to create the canonical template.
fn init_via_cli(root: &Path) -> bool {
    let script = root.join(crate::root::SYSTEM_SHELL).join("linkslot.sh");
    if !script.is_file() {
        return false;
    }
    let mut command = Command::new("/bin/sh");
    command
        .arg(&script)
        .arg("config")
        .arg("init")
        .arg("--root")
        .arg(root)
        .env("SLOT_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = Instant::now() + INIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn write_config(root: &Path, text: &str) -> Result<(), String> {
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("无法创建配置目录 {}：{error}", dir.display()))?;
    }
    atomic_write(&path, text.as_bytes())
        .map_err(|error| format!("无法写入配置文件 {}：{error}", path.display()))
}

// ---------------------------------------------------------------------------
// Validation (mirrors the CLI so the UI never writes a config it would reject)
// ---------------------------------------------------------------------------

fn validate_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("WebDAV 地址不能为空".to_string());
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err("WebDAV 地址必须以 http:// 或 https:// 开头".to_string());
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => {}
        _ => return Err("WebDAV 地址只支持 http:// 或 https://".to_string()),
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("WebDAV 地址不能包含 ? 或 #".to_string());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err("WebDAV 地址缺少主机名".to_string());
    }
    if authority.contains('@') {
        return Err("WebDAV 地址不能包含用户名密码，请使用用户名/密码字段".to_string());
    }
    if authority
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("WebDAV 地址的主机名无效".to_string());
    }
    if !authority.starts_with('[') {
        if let Some((host, port)) = authority.rsplit_once(':') {
            if host.is_empty() || port.is_empty() || !port.chars().all(|ch| ch.is_ascii_digit()) {
                return Err("WebDAV 地址的主机名或端口无效".to_string());
            }
        }
    }
    Ok(url.to_string())
}

/// Normalise a remote root exactly like `linkSlot` does: `/a/b`, empty = root.
fn normalize_remote_root(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains('\\') {
        return Err("WebDAV 远程目录不能包含反斜杠（\\）".to_string());
    }
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err("WebDAV 远程目录不能包含 ? 或 #".to_string());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return Err("WebDAV 远程目录不能包含 . 或 .. 路径段".to_string());
        }
        if segment.chars().any(char::is_control) {
            return Err("WebDAV 远程目录不能包含控制字符".to_string());
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("/{}", segments.join("/")))
}

fn validate_ca_cert(ca_cert: &str) -> Result<(), String> {
    if ca_cert.is_empty() {
        return Ok(());
    }
    let path = Path::new(ca_cert);
    if !path.is_absolute() {
        return Err(format!("CA 证书必须是绝对路径：{ca_cert}"));
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!("CA 证书不是文件：{ca_cert}")),
        Err(_) => Err(format!("CA 证书不存在或无法读取：{ca_cert}")),
    }
}

fn validate_ssid(ssid: &str) -> Result<(), String> {
    if ssid.is_empty() {
        return Err("Wi-Fi 名称（SSID）不能为空".to_string());
    }
    if ssid.len() > 32 {
        return Err(format!(
            "Wi-Fi 名称（SSID）不能超过 32 字节（当前 {} 字节）",
            ssid.len()
        ));
    }
    Ok(())
}

fn validate_psk(psk: &str) -> Result<(), String> {
    if psk.is_empty() {
        return Ok(());
    }
    if psk.len() == 64 && psk.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(());
    }
    let chars = psk.chars().count();
    if (8..=63).contains(&chars) {
        return Ok(());
    }
    Err("Wi-Fi 密码必须为空（开放网络）、8 到 63 个字符，或 64 位十六进制".to_string())
}

/// The smallest template the device UI writes when the CLI never created one.
///
/// Same shape as `linkSlot config init` (all four tables, the keys the CLI
/// knows) so the file can be parsed, validated and edited by the CLI later.
pub(crate) const MINIMAL_TEMPLATE: &str = r#"# linkSlot 配置文件（由设备端设置界面生成，格式与 `linkSlot config init` 一致）。
# FAT32 无法设置 600 权限；密码建议用环境变量 LINKSLOT_WEBDAV_PASSWORD 注入，
# 本文件中的 password 留空即可。

[webdav]
url = ""
remote_root = "/linkSlot"
username = ""
password = ""
ca_cert = ""
insecure = false

[device]
id = ""

[sync]
mode = "all"
timeout_secs = 20
retries = 2
keep_remote = false

[wifi]
manage = true
ssid = ""
psk = ""
auto_off_minutes = 10
timeout_secs = 30
"#;

/// Config path under a card root.
pub(crate) fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_REL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_round_trips_through_its_own_file() {
        let dir = temp_root();
        let saved = write_device_name(dir.path(), " 客厅机 ").expect("write name");
        assert_eq!(saved, "客厅机");
        assert_eq!(read_device_name(dir.path()), "客厅机");
    }

    #[test]
    fn a_missing_or_broken_device_name_falls_back_to_the_hostname() {
        let dir = temp_root();
        let name = read_device_name(dir.path());
        assert!(!name.is_empty(), "no fallback name at all");
        assert!(
            name.len() <= DEVICE_NAME_MAX,
            "the fallback is longer than the hello slot: {name}"
        );

        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).expect("config dir");
        std::fs::write(dir.path().join(DEVICE_NAME_REL), "\n\n").expect("blank name");
        assert_eq!(read_device_name(dir.path()), default_device_name());
    }

    #[test]
    fn a_device_name_is_clamped_on_a_character_boundary() {
        // Eight Chinese characters are 24 bytes: it has to give way to the
        // hello's 16-byte slot without splitting one.
        let clamped = sanitize_device_name("一二三四五六七八").expect("clamped");
        assert!(clamped.len() <= DEVICE_NAME_MAX, "{clamped}");
        assert_eq!(clamped, "一二三四五");
        assert_eq!(sanitize_device_name("   "), None);
        assert_eq!(sanitize_device_name("\u{7}"), None);
        assert_eq!(write_device_name(temp_root().path(), ""), Err("设备名不能为空，且不能超过 16 字节".to_string()));
    }

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("create temp dir")
    }

    fn write_config(root: &Path, body: &str) {
        let path = config_path(root);
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
        std::fs::write(&path, body).expect("write config");
    }

    fn write_storages(root: &Path, body: &str) {
        let path = root.join(STORAGES_REL);
        std::fs::create_dir_all(path.parent().expect("storage parent")).expect("create storage dir");
        std::fs::write(&path, body).expect("write storages");
    }

    fn read_text(root: &Path) -> String {
        std::fs::read_to_string(config_path(root)).expect("read config")
    }

    fn sample_edit() -> WebdavEdit {
        WebdavEdit {
            url: "https://nas.example.com/dav".to_string(),
            remote_root: "/linkSlot".to_string(),
            username: "rg34xx".to_string(),
            password: Some("secret".to_string()),
            ca_cert: String::new(),
            insecure: false,
            keep_remote: false,
        }
    }

    const FULL_CONFIG: &str = r#"# top comment
[webdav]
# endpoint comment
url = "http://old/dav"
remote_root = "/old"
username = "old"
password = "oldpass"
ca_cert = ""
unknown_key = "keep-me"
insecure = false

[device]
id = "rg34xx-abc"

[sync]
mode = "state"
timeout_secs = 20
keep_remote = false
custom = 7

[wifi]
manage = false
ssid = "old-wifi"
psk = "oldpsk"
auto_off_minutes = 30
timeout_secs = 30
"#;

    #[test]
    fn reads_named_storages_and_uses_the_selected_one() {
        let root = temp_root();
        write_config(
            root.path(),
            "[webdav]\nurl = \"http://legacy/dav\"\n[sync]\nkeep_remote = true\n",
        );
        write_storages(
            root.path(),
            r#"active = "work"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "https://home.example/dav"
remote_root = "/home"
username = "u1"
password = "p1"

[[storage]]
id = "work"
name = "公司"
kind = "webdav"
url = "https://work.example/dav"
remote_root = "/work"
username = "u2"
password = "p2"
"#,
        );
        let config = read(root.path());
        assert_eq!(config.active_storage, "work");
        assert_eq!(config.webdav.url, "https://work.example/dav");
        assert_eq!(config.webdav.remote_root, "/work");
        assert!(config.webdav.password_set);
        assert!(config.webdav.keep_remote);
        assert_eq!(config.storages.len(), 2);
        assert_eq!(config.storages[0].name, "家里");
        assert_eq!(config.storages[1].id, "work");
    }

    #[test]
    fn activate_storage_switches_the_active_profile_and_test_config_is_temporary() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://old/dav\"\n");
        write_storages(
            root.path(),
            r#"active = "work"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "https://home.example/dav"
remote_root = "/home"
username = "u1"
password = "p1"

[[storage]]
id = "work"
name = "公司"
kind = "webdav"
url = "https://work.example/dav"
remote_root = "/work"
username = "u2"
password = "p2"
"#,
        );
        let test = profile_test_config(root.path(), "home").expect("test config");
        let path = test.path().to_path_buf();
        let text = std::fs::read_to_string(&path).expect("read test config");
        assert!(text.contains("https://home.example/dav"), "{text}");
        assert!(text.contains("password = \"p1\""), "{text}");
        assert_eq!(read(root.path()).active_storage, "work");
        drop(test);
        assert!(!path.exists(), "temporary test config was not removed");

        activate_storage(root.path(), "home").expect("activate home");
        let config = read(root.path());
        assert_eq!(config.active_storage, "home");
        assert_eq!(config.webdav.url, "https://home.example/dav");
        assert!(config.webdav.password_set);
        let legacy = read_text(root.path());
        assert!(legacy.contains("https://home.example/dav"), "{legacy}");
    }

    #[test]
    fn save_storages_writes_the_sidecar_and_legacy_mirror() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://old/dav\"\n");
        let edits = vec![
            StorageEdit {
                id: "home".to_string(),
                name: "家里".to_string(),
                kind: "webdav".to_string(),
                url: "https://home.example/dav".to_string(),
                remote_root: "/home".to_string(),
                username: "u1".to_string(),
                password: Some("p1".to_string()),
                ca_cert: String::new(),
                insecure: false,
            },
            StorageEdit {
                id: "work".to_string(),
                name: "公司".to_string(),
                kind: "webdav".to_string(),
                url: "https://work.example/dav".to_string(),
                remote_root: "/work".to_string(),
                username: "u2".to_string(),
                password: Some("p2".to_string()),
                ca_cert: String::new(),
                insecure: false,
            },
        ];
        save_storages(root.path(), "work", &edits, true).expect("save profiles");
        assert!(root.path().join(STORAGES_REL).is_file());
        let config = read(root.path());
        assert_eq!(config.active_storage, "work");
        assert_eq!(config.webdav.url, "https://work.example/dav");
        assert!(config.webdav.keep_remote);
        assert_eq!(config.storages.len(), 2);
        let legacy = read_text(root.path());
        assert!(legacy.contains("https://work.example/dav"), "{legacy}");

        let mut keep = edits.clone();
        keep[1].password = None;
        save_storages(root.path(), "home", &keep, false).expect("switch active");
        let sidecar = std::fs::read_to_string(root.path().join(STORAGES_REL)).unwrap();
        assert!(sidecar.contains("password = \"p2\""), "{sidecar}");
        let config = read(root.path());
        assert_eq!(config.active_storage, "home");
        assert_eq!(config.webdav.url, "https://home.example/dav");
    }

    #[test]
    fn save_storages_rejects_bad_selection_and_duplicates() {
        let root = temp_root();
        let edit = StorageEdit {
            id: "home".to_string(),
            name: "家里".to_string(),
            kind: "webdav".to_string(),
            url: "https://home.example/dav".to_string(),
            remote_root: "/home".to_string(),
            username: "u".to_string(),
            password: None,
            ca_cert: String::new(),
            insecure: false,
        };
        assert!(save_storages(root.path(), "missing", &[edit.clone()], false).is_err());
        assert!(save_storages(root.path(), "home", &[edit.clone(), edit.clone()], false).is_err());
        let mut bad = edit.clone();
        bad.url = "ftp://bad".to_string();
        assert!(save_storages(root.path(), "home", &[bad], false).is_err());
    }

    #[test]
    fn write_webdav_updates_the_active_sidecar_profile() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://old/dav\"\n");
        let edits = vec![StorageEdit {
            id: "home".to_string(),
            name: "家里".to_string(),
            kind: "webdav".to_string(),
            url: "http://old/dav".to_string(),
            remote_root: "/home".to_string(),
            username: "u".to_string(),
            password: Some("p".to_string()),
            ca_cert: String::new(),
            insecure: false,
        }];
        save_storages(root.path(), "home", &edits, false).expect("initial");
        let mut edit = sample_edit();
        edit.url = "https://new.example/dav".to_string();
        edit.password = None;
        write_webdav(root.path(), edit).expect("update active");
        let config = read(root.path());
        assert_eq!(config.webdav.url, "https://new.example/dav");
        assert!(config.webdav.password_set);
    }

    #[test]
    fn missing_file_reads_defaults() {
        let root = temp_root();
        let config = read(root.path());
        assert_eq!(config.webdav.url, "");
        assert_eq!(config.webdav.remote_root, "/linkSlot");
        assert_eq!(config.webdav.username, "");
        assert!(!config.webdav.password_set);
        assert_eq!(config.webdav.ca_cert, "");
        assert!(!config.webdav.insecure);
        assert!(!config.webdav.keep_remote);
        assert_eq!(config.wifi.ssid, "");
        assert!(!config.wifi.psk_set);
        assert!(config.wifi.manage);
        assert_eq!(config.wifi.auto_off_minutes, 10);
    }

    #[test]
    fn reads_known_keys_and_skips_bad_lines() {
        let root = temp_root();
        write_config(
            root.path(),
            r#"# comment
[webdav]
url = "http://127.0.0.1:8080/dav" # inline
remote_root = "linkSlot/"
username = "u"
password = "p"
insecure = true
bogus = 1
[unknown]
whatever = true
[sync]
keep_remote = true
[wifi]
ssid = 'home-net'
psk = "12345678"
manage = false
auto_off_minutes = 30
this line is not toml
"#,
        );
        let config = read(root.path());
        assert_eq!(config.webdav.url, "http://127.0.0.1:8080/dav");
        assert_eq!(config.webdav.remote_root, "linkSlot/");
        assert_eq!(config.webdav.username, "u");
        assert!(config.webdav.password_set);
        assert!(config.webdav.insecure);
        assert!(config.webdav.keep_remote);
        assert_eq!(config.wifi.ssid, "home-net");
        assert!(config.wifi.psk_set);
        assert!(!config.wifi.manage);
        assert_eq!(config.wifi.auto_off_minutes, 30);
    }

    #[test]
    fn read_never_panics_on_unreadable_file() {
        let root = temp_root();
        let path = config_path(root.path());
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("create dir");
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x41]).expect("write bytes");
        let config = read(root.path());
        assert_eq!(config.webdav, LinkConfig::default().webdav);
        assert_eq!(config.wifi, LinkConfig::default().wifi);
        assert_eq!(config.active_storage, DEFAULT_STORAGE_ID);
        assert_eq!(config.storages.len(), 1);
        assert_eq!(config.storages[0].id, DEFAULT_STORAGE_ID);
    }

    #[test]
    fn saved_wifi_list_tracks_connections_and_can_switch_between_them() {
        let root = temp_root();
        write_wifi(root.path(), "home", "secret123").expect("home");
        write_wifi(root.path(), "office", "office123").expect("office");
        let saved = read_saved_wifi(root.path());
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].ssid, "office");
        assert!(saved[0].password_set);
        assert_eq!(saved_wifi_password(root.path(), "home").as_deref(), Some("secret123"));

        activate_saved_wifi(root.path(), "home").expect("activate home");
        let config = read(root.path());
        assert_eq!(config.wifi.ssid, "home");
        assert!(config.wifi.psk_set);
        assert_eq!(read_saved_wifi(root.path())[0].ssid, "home");
    }

    #[test]
    fn legacy_single_wifi_config_appears_in_the_saved_list() {
        let root = temp_root();
        write_config(
            root.path(),
            "[wifi]\nmanage = true\nssid = \"home\"\npsk = \"secret123\"\n",
        );
        let saved = read_saved_wifi(root.path());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].ssid, "home");
        assert_eq!(saved_wifi_password(root.path(), "home").as_deref(), Some("secret123"));
    }

    #[test]
    fn writes_minimal_template_when_missing() {
        let root = temp_root();
        let mut edit = sample_edit();
        edit.remote_root = "linkSlot/".to_string();
        write_webdav(root.path(), edit).expect("write webdav");

        assert!(config_path(root.path()).is_file());
        let text = read_text(root.path());
        for section in ["[webdav]", "[device]", "[sync]", "[wifi]"] {
            assert!(text.contains(section), "template missing {section}: {text}");
        }
        assert!(text.contains("timeout_secs = 20"));
        assert!(text.contains("auto_off_minutes = 10"));
        assert!(text.contains("mode = \"all\""));

        let config = read(root.path());
        assert_eq!(config.webdav.url, "https://nas.example.com/dav");
        assert_eq!(config.webdav.remote_root, "/linkSlot");
        assert_eq!(config.webdav.username, "rg34xx");
        assert!(config.webdav.password_set);
        assert!(!config.webdav.keep_remote);
        assert!(config.wifi.manage);
        assert_eq!(config.wifi.auto_off_minutes, 10);
        assert_eq!(config.sync.mode, SyncMode::All);
    }

    #[test]
    fn write_preserves_comments_unknown_keys_and_other_sections() {
        let root = temp_root();
        write_config(root.path(), FULL_CONFIG);

        let mut edit = sample_edit();
        edit.url = "http://new/dav".to_string();
        edit.remote_root = "/new/root".to_string();
        edit.username = "new-user".to_string();
        edit.password = None;
        edit.insecure = true;
        edit.keep_remote = true;
        write_webdav(root.path(), edit).expect("write webdav");

        let text = read_text(root.path());
        assert!(text.contains("# top comment"), "top comment lost: {text}");
        assert!(text.contains("# endpoint comment"), "section comment lost");
        assert!(
            text.contains("unknown_key = \"keep-me\""),
            "unknown key lost"
        );
        assert!(text.contains("id = \"rg34xx-abc\""), "device section lost");
        assert!(text.contains("custom = 7"), "unknown sync key lost");
        assert!(text.contains("timeout_secs = 30"), "wifi timeout lost");

        let config = read(root.path());
        assert_eq!(config.webdav.url, "http://new/dav");
        assert_eq!(config.webdav.remote_root, "/new/root");
        assert_eq!(config.webdav.username, "new-user");
        assert!(config.webdav.password_set, "None must keep the password");
        assert!(config.webdav.insecure);
        assert!(config.webdav.keep_remote);
        assert_eq!(config.wifi.ssid, "old-wifi");
        assert_eq!(
            config.wifi.auto_off_minutes,
            30,
            "write_webdav must keep auto_off_minutes"
        );
    }

    #[test]
    fn password_none_keeps_and_some_empty_clears() {
        let root = temp_root();
        write_webdav(root.path(), sample_edit()).expect("initial write");
        assert!(read(root.path()).webdav.password_set);

        let mut keep = sample_edit();
        keep.password = None;
        keep.username = "another".to_string();
        write_webdav(root.path(), keep).expect("keep password write");
        assert!(read(root.path()).webdav.password_set);
        assert!(read_text(root.path()).contains("password = \"secret\""));

        let clear = WebdavEdit {
            username: String::new(),
            password: Some(String::new()),
            ..sample_edit()
        };
        write_webdav(root.path(), clear).expect("clear password write");
        let config = read(root.path());
        assert!(!config.webdav.password_set);
        assert_eq!(config.webdav.username, "");
        assert!(read_text(root.path()).contains("password = \"\""));
    }

    #[test]
    fn escapes_special_characters_in_strings() {
        let root = temp_root();
        let password = "se\"cr\\et\nline\t\u{7}";
        let mut edit = sample_edit();
        edit.username = "user".to_string();
        edit.password = Some(password.to_string());
        write_webdav(root.path(), edit).expect("write escaped password");
        let text = read_text(root.path());
        assert!(text.contains("\\\"cr\\\\et\\nline\\t\\u0007"), "{text}");
        let raw = parse_raw(&text);
        assert_eq!(raw.password, password);
        assert_eq!(raw.username, "user");

        let ssid = "Cafe\"Wi-Fi\\";
        let psk = "p@ss\"word\\1";
        write_wifi(root.path(), ssid, psk).expect("write escaped wifi");
        let text = read_text(root.path());
        let raw = parse_raw(&text);
        assert_eq!(raw.ssid, ssid);
        assert_eq!(raw.psk, psk);
    }

    #[test]
    fn rejects_invalid_webdav_edits_without_creating_a_file() {
        let root = temp_root();
        let base = sample_edit();
        let cases: [(&str, &str, &str); 6] = [
            ("bad url", "", "WebDAV"),
            ("bad scheme", "ftp://host/dav", "WebDAV"),
            ("missing host", "https:///dav", "WebDAV"),
            ("query", "https://host/dav?x=1", "WebDAV"),
            ("dotdot root", "/a/../b", "远程目录"),
            ("backslash root", "a\\b", "远程目录"),
        ];
        for (name, url, expected) in cases {
            let mut edit = base.clone();
            if name == "dotdot root" || name == "backslash root" {
                edit.url = "https://host/dav".to_string();
                edit.remote_root = url.to_string();
            } else {
                edit.url = url.to_string();
            }
            let error = write_webdav(root.path(), edit).expect_err(name);
            assert!(error.contains(expected), "{name}: unexpected error {error}");
            assert!(
                !config_path(root.path()).exists(),
                "{name}: validation failure must not create a config"
            );
        }
    }

    #[test]
    fn rejects_password_without_username() {
        let root = temp_root();
        let mut edit = sample_edit();
        edit.username = String::new();
        edit.password = Some("secret".to_string());
        let error = write_webdav(root.path(), edit).expect_err("empty username");
        assert!(error.contains("用户名"), "unexpected error {error}");
        assert!(!config_path(root.path()).exists());

        write_config(
            root.path(),
            "[webdav]\nurl = \"http://x\"\nusername = \"\"\npassword = \"p\"\n",
        );
        let mut edit = sample_edit();
        edit.url = "http://x".to_string();
        edit.username = String::new();
        edit.password = None;
        let error = write_webdav(root.path(), edit).expect_err("clearing username with password");
        assert!(error.contains("用户名"), "unexpected error {error}");
    }

    #[test]
    fn validates_ca_cert() {
        let root = temp_root();
        let cert = root.path().join("ca.pem");
        std::fs::write(&cert, b"pem").expect("write cert");

        let mut edits = Vec::new();
        for (name, ca_cert) in [
            ("relative", "ca.pem".to_string()),
            (
                "missing",
                root.path().join("nope.pem").to_string_lossy().into_owned(),
            ),
            ("directory", root.path().to_string_lossy().into_owned()),
        ] {
            let mut edit = sample_edit();
            edit.ca_cert = ca_cert;
            let error = write_webdav(root.path(), edit).expect_err(name);
            assert!(
                error.contains("CA 证书"),
                "{name}: unexpected error {error}"
            );
            edits.push(error);
        }

        let mut edit = sample_edit();
        edit.ca_cert = cert.to_string_lossy().into_owned();
        write_webdav(root.path(), edit).expect("valid ca cert");
        assert_eq!(
            read(root.path()).webdav.ca_cert,
            cert.to_string_lossy().into_owned()
        );
    }

    #[test]
    fn validates_wifi_credentials() {
        let root = temp_root();

        let error = write_wifi(root.path(), "", "secret123").expect_err("empty ssid");
        assert!(error.contains("SSID"), "unexpected error {error}");

        let error =
            write_wifi(root.path(), &"a".repeat(33), "secret123").expect_err("ssid too long");
        assert!(error.contains("32 字节"), "unexpected error {error}");

        let error = write_wifi(root.path(), "home", "short").expect_err("short psk");
        assert!(error.contains("密码"), "unexpected error {error}");

        let error = write_wifi(root.path(), "home", &"z".repeat(64)).expect_err("non-hex psk");
        assert!(error.contains("密码"), "unexpected error {error}");

        write_wifi(root.path(), &"a".repeat(32), "12345678").expect("32-byte ssid, 8-char psk");
        assert_eq!(read(root.path()).wifi.ssid, "a".repeat(32));

        write_wifi(root.path(), "home", &"a".repeat(64)).expect("64-hex psk");
        assert!(read(root.path()).wifi.psk_set);

        write_wifi(root.path(), "home", &"p".repeat(63)).expect("63-char psk");
        assert!(read(root.path()).wifi.psk_set);

        write_wifi(root.path(), "open-network", "").expect("open network");
        assert!(!read(root.path()).wifi.psk_set);
    }

    #[test]
    fn write_wifi_enables_manage_and_keeps_webdav() {
        let root = temp_root();
        write_config(root.path(), FULL_CONFIG);

        write_wifi(root.path(), "home", "secret123").expect("write wifi");
        let config = read(root.path());
        assert!(config.wifi.manage, "write_wifi must set manage=true");
        assert_eq!(config.wifi.ssid, "home");
        assert!(config.wifi.psk_set);
        assert_eq!(
            config.wifi.auto_off_minutes,
            30,
            "write_wifi must keep auto_off_minutes"
        );
        assert_eq!(config.webdav.url, "http://old/dav");
        assert_eq!(config.webdav.username, "old");
        assert!(config.webdav.password_set);

        let text = read_text(root.path());
        assert!(text.contains("# top comment"));
        assert!(text.contains("custom = 7"));
        assert!(text.contains("ssid = \"home\""));
        assert!(text.contains("psk = \"secret123\""));
        assert!(text.contains("auto_off_minutes = 30"));
    }

    #[test]
    fn auto_off_defaults_and_ignores_invalid_values() {
        let root = temp_root();
        write_config(root.path(), "[wifi]\nssid = \"home\"\n");
        assert_eq!(read(root.path()).wifi.auto_off_minutes, 10);

        for (body, expected) in [
            ("auto_off_minutes = 0", 0u16),
            ("auto_off_minutes = 5", 5),
            ("auto_off_minutes = 10", 10),
            ("auto_off_minutes = 30", 30),
            ("auto_off_minutes = 60", 60),
            ("auto_off_minutes = 120", 120),
            ("auto_off_minutes = 7", 10),
            ("auto_off_minutes = \"30\"", 10),
            ("auto_off_minutes = -5", 10),
            ("auto_off_minutes = 100_000", 10),
        ] {
            write_config(root.path(), &format!("[wifi]\n{body}\n"));
            assert_eq!(
                read(root.path()).wifi.auto_off_minutes,
                expected,
                "{body}"
            );
        }

        let raw = parse_raw("[wifi]\nauto_off_minutes = 60\n");
        assert_eq!(raw.auto_off_minutes, Some(60));
        let raw = parse_raw("[wifi]\nauto_off_minutes = 7\n");
        assert_eq!(raw.auto_off_minutes, None);
    }

    #[test]
    fn auto_off_choices_labels_and_step() {
        assert_eq!(WIFI_AUTO_OFF_CHOICES, [1, 3, 5, 10, 30, 60, 120, 0]);
        assert_eq!(auto_off_label(1), "1 分钟");
        assert_eq!(auto_off_label(3), "3 分钟");
        assert_eq!(auto_off_label(5), "5 分钟");
        assert_eq!(auto_off_label(10), "10 分钟");
        assert_eq!(auto_off_label(30), "30 分钟");
        assert_eq!(auto_off_label(60), "1 小时");
        assert_eq!(auto_off_label(120), "2 小时");
        assert_eq!(auto_off_label(0), "永不");
        assert_eq!(auto_off_label(7), "10 分钟");

        assert_eq!(next_auto_off(1, true), 3);
        assert_eq!(next_auto_off(3, true), 5);
        assert_eq!(next_auto_off(5, true), 10);
        assert_eq!(next_auto_off(10, true), 30);
        assert_eq!(next_auto_off(120, true), 0);
        assert_eq!(next_auto_off(0, true), 1);
        assert_eq!(next_auto_off(1, false), 0);
        assert_eq!(next_auto_off(0, false), 120);
        assert_eq!(next_auto_off(7, true), 30);
        assert_eq!(next_auto_off(7, false), 5);
    }

    #[test]
    fn write_wifi_auto_off_changes_only_that_key() {
        let root = temp_root();
        write_config(root.path(), FULL_CONFIG);

        write_wifi_auto_off(root.path(), 60).expect("write auto off");
        let config = read(root.path());
        assert_eq!(config.wifi.auto_off_minutes, 60);
        assert_eq!(config.wifi.ssid, "old-wifi");
        assert!(config.wifi.psk_set);
        assert_eq!(config.webdav.url, "http://old/dav");
        assert_eq!(config.webdav.username, "old");
        assert!(config.webdav.password_set);

        let text = read_text(root.path());
        assert!(text.contains("# top comment"));
        assert!(text.contains("custom = 7"));
        assert!(text.contains("ssid = \"old-wifi\""));
        assert!(text.contains("auto_off_minutes = 60"));

        write_wifi_auto_off(root.path(), 0).expect("write never");
        assert_eq!(read(root.path()).wifi.auto_off_minutes, 0);
        assert!(read_text(root.path()).contains("auto_off_minutes = 0"));

        let fresh = temp_root();
        write_wifi_auto_off(fresh.path(), 120).expect("write into new config");
        assert_eq!(read(fresh.path()).wifi.auto_off_minutes, 120);
        assert!(read_text(fresh.path()).contains("[webdav]"));
    }

    #[test]
    fn write_wifi_auto_off_rejects_unknown_choices() {
        let root = temp_root();
        let error = write_wifi_auto_off(root.path(), 7).expect_err("invalid choice");
        assert!(error.contains("自动关闭"), "unexpected error {error}");
        assert!(!config_path(root.path()).exists());
    }

    #[test]
    fn sync_mode_defaults_to_all_and_reads_valid_values() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://x\"\n");
        assert_eq!(read(root.path()).sync.mode, SyncMode::All);

        for (line, expected) in [
            ("mode = \"all\"", SyncMode::All),
            ("mode = \"state\"", SyncMode::State),
            ("mode = \"sav\"", SyncMode::Sav),
            ("mode = \"bogus\"", SyncMode::All),
            ("mode = 1", SyncMode::All),
        ] {
            write_config(root.path(), &format!("[sync]\n{line}\n"));
            assert_eq!(read(root.path()).sync.mode, expected, "{line}");
        }
    }

    #[test]
    fn sync_mode_choices_labels_and_step() {
        assert_eq!(
            SyncMode::ALL,
            [SyncMode::All, SyncMode::State, SyncMode::Sav]
        );
        assert_eq!(SyncMode::ALL.map(SyncMode::as_str), ["all", "state", "sav"]);
        assert_eq!(
            SyncMode::ALL.map(SyncMode::label),
            ["全部", "仅即时存档", "仅电池存档"]
        );
        assert_eq!(SyncMode::next(SyncMode::All, true), SyncMode::State);
        assert_eq!(SyncMode::next(SyncMode::State, true), SyncMode::Sav);
        assert_eq!(SyncMode::next(SyncMode::Sav, true), SyncMode::All);
        assert_eq!(SyncMode::next(SyncMode::All, false), SyncMode::Sav);
        assert_eq!(SyncMode::next(SyncMode::Sav, false), SyncMode::State);
        for mode in SyncMode::ALL {
            assert_eq!(SyncMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(SyncMode::parse("cheats"), None);
        assert!(SyncMode::All.state_enabled() && SyncMode::All.sav_enabled());
        assert!(SyncMode::State.state_enabled() && !SyncMode::State.sav_enabled());
        assert!(!SyncMode::Sav.state_enabled() && SyncMode::Sav.sav_enabled());
    }

    #[test]
    fn write_sync_mode_round_trips_and_keeps_other_keys() {
        let root = temp_root();
        write_config(root.path(), FULL_CONFIG);

        write_sync_mode(root.path(), SyncMode::Sav).expect("write mode");
        assert_eq!(read(root.path()).sync.mode, SyncMode::Sav);
        assert_eq!(read(root.path()).wifi.ssid, "old-wifi");
        assert_eq!(read(root.path()).wifi.auto_off_minutes, 30);
        assert_eq!(read(root.path()).webdav.url, "http://old/dav");
        assert!(read(root.path()).webdav.password_set);

        let text = read_text(root.path());
        assert!(text.contains("mode = \"sav\""));
        assert!(text.contains("timeout_secs = 20"));
        assert!(text.contains("keep_remote = false"));
        assert!(text.contains("custom = 7"));
        assert!(text.contains("# top comment"));
        assert!(text.contains("ssid = \"old-wifi\""));

        write_sync_mode(root.path(), SyncMode::All).expect("write mode again");
        assert_eq!(read(root.path()).sync.mode, SyncMode::All);
        assert!(read_text(root.path()).contains("mode = \"all\""));
    }

    #[test]
    fn write_sync_mode_inserts_a_missing_sync_section() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://x\"\n");

        write_sync_mode(root.path(), SyncMode::State).expect("insert sync section");
        assert_eq!(read(root.path()).sync.mode, SyncMode::State);
        assert_eq!(read(root.path()).webdav.url, "http://x");
        let text = read_text(root.path());
        assert!(text.contains("[sync]"), "missing section not inserted: {text}");
        assert!(text.contains("mode = \"state\""));
    }

    #[test]
    fn write_sync_mode_creates_the_config_when_missing() {
        let root = temp_root();
        write_sync_mode(root.path(), SyncMode::Sav).expect("create config");
        assert_eq!(read(root.path()).sync.mode, SyncMode::Sav);
        let text = read_text(root.path());
        assert!(text.contains("mode = \"sav\""));
        assert!(text.contains("[webdav]"));
        assert!(text.contains("[wifi]"));
    }

    #[test]
    fn write_webdav_and_write_wifi_keep_the_sync_mode() {
        let root = temp_root();
        write_config(root.path(), FULL_CONFIG);

        let mut edit = sample_edit();
        edit.password = None;
        edit.username = "u".to_string();
        write_webdav(root.path(), edit).expect("write webdav");
        assert_eq!(read(root.path()).sync.mode, SyncMode::State);

        write_wifi(root.path(), "home", "secret123").expect("write wifi");
        assert_eq!(read(root.path()).sync.mode, SyncMode::State);
    }

    #[test]
    fn appends_missing_sections_and_keys() {
        let root = temp_root();
        write_config(root.path(), "[webdav]\nurl = \"http://x\"\n");
        let mut edit = sample_edit();
        edit.url = "http://x".to_string();
        edit.remote_root = "/slot".to_string();
        edit.username = "u".to_string();
        edit.password = Some("p".to_string());
        edit.keep_remote = true;
        write_webdav(root.path(), edit).expect("write into partial config");

        let text = read_text(root.path());
        assert!(
            text.contains("[sync]"),
            "missing section not appended: {text}"
        );
        assert!(text.contains("keep_remote = true"));
        let config = read(root.path());
        assert_eq!(config.webdav.url, "http://x");
        assert_eq!(config.webdav.remote_root, "/slot");
        assert!(config.webdav.keep_remote);
    }

    #[test]
    fn tolerates_crlf_and_replaces_empty_file() {
        let root = temp_root();
        write_config(
            root.path(),
            "[webdav]\r\nurl = \"http://old/dav\"\r\nusername = \"u\"\r\npassword = \"p\"\r\n",
        );
        let mut edit = sample_edit();
        edit.url = "http://new/dav".to_string();
        edit.password = None;
        write_webdav(root.path(), edit).expect("write crlf config");
        let text = read_text(root.path());
        assert!(!text.contains('\r'), "output should be LF-only");
        assert_eq!(read(root.path()).webdav.url, "http://new/dav");

        let empty_root = temp_root();
        write_config(empty_root.path(), "");
        write_webdav(empty_root.path(), sample_edit()).expect("write over empty config");
        assert_eq!(
            read(empty_root.path()).webdav.url,
            "https://nas.example.com/dav"
        );
        assert!(read_text(empty_root.path()).contains("[wifi]"));
    }

    #[cfg(unix)]
    #[test]
    fn prefers_cli_generated_template_when_available() {
        let root = temp_root();
        let system = root.path().join(crate::root::SYSTEM_SHELL);
        std::fs::create_dir_all(&system).expect("create System");
        let script = system.join("linkslot.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
# Fake `linkSlot config init`: writes a config flagged as CLI-generated.
root=/mnt/sdcard
while [ "$#" -gt 0 ]; do
	if [ "$1" = "--root" ]; then root="$2"; fi
	shift
done
mkdir -p "$root/linkSlot/Config" || exit 1
cat > "$root/linkSlot/Config/config.toml" <<'EOF'
# generated-by-cli
[webdav]
url = "http://placeholder/dav"
remote_root = "/linkSlot"
username = "u"
password = "p"
ca_cert = ""
insecure = false

[device]
id = ""

[sync]
timeout_secs = 20
retries = 2
keep_remote = false

[wifi]
manage = true
ssid = ""
psk = ""
auto_off_minutes = 10
timeout_secs = 30
EOF
"#,
        )
        .expect("write fake cli");

        let mut edit = sample_edit();
        edit.url = "http://edited/dav".to_string();
        edit.username = "u".to_string();
        edit.password = None;
        write_webdav(root.path(), edit).expect("write with cli init");

        let text = read_text(root.path());
        assert!(
            text.contains("# generated-by-cli"),
            "CLI template was not preferred: {text}"
        );
        assert_eq!(read(root.path()).webdav.url, "http://edited/dav");
        assert!(read(root.path()).webdav.password_set);
        assert_eq!(
            read(root.path()).wifi.auto_off_minutes,
            10,
            "write_webdav must keep auto_off_minutes from the CLI template"
        );
    }

    #[test]
    fn log_level_round_trips_and_invalid_falls_back_to_info() {
        let root = temp_root();
        assert_eq!(read_log_level(root.path()), LogLevel::Info);
        write_log_level(root.path(), LogLevel::Info).expect("write level");
        assert_eq!(read_log_level(root.path()), LogLevel::Info);
        write_log_level(root.path(), LogLevel::Debug).expect("write level");
        assert_eq!(read_log_level(root.path()), LogLevel::Debug);
        std::fs::write(root.path().join(LOG_LEVEL_REL), "verbose\n").unwrap();
        assert_eq!(read_log_level(root.path()), LogLevel::Info);
    }
}
