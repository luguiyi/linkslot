//! `config.toml` loading, validation, template generation and device identity.
//!
//! Location: `<root>/linkSlot/Config/config.toml`. The password is taken from
//! `LINKSLOT_WEBDAV_PASSWORD` when the variable is set (even to an empty
//! string), which keeps secrets off the FAT card when desired.
//!
//! A missing config is a configuration error (exit 3) that points at
//! `linkSlot config init`; the CLI never silently creates a config that could
//! look like it holds credentials.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use slot_device::DeviceInfo;

use crate::error::{fs_error, LinkError, Result};
use crate::hash;
use crate::store;
use crate::webdav::Endpoint;

pub const CONFIG_DIR_REL: &str = "linkSlot/Config";
pub const CONFIG_REL: &str = "linkSlot/Config/config.toml";
pub const STORAGES_FILE: &str = "storages.toml";
pub const DEVICE_FILE_REL: &str = "linkSlot/Config/device.txt";
pub const PASSWORD_ENV: &str = "LINKSLOT_WEBDAV_PASSWORD";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WebDavConfig {
    pub url: String,
    pub remote_root: String,
    pub username: String,
    pub password: String,
    pub ca_cert: String,
    pub insecure: bool,
}

impl Default for WebDavConfig {
    fn default() -> Self {
        WebDavConfig {
            url: String::new(),
            remote_root: "/linkSlot".to_string(),
            username: String::new(),
            password: String::new(),
            ca_cert: String::new(),
            insecure: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageFile {
    pub active: String,
    pub storage: Vec<StorageConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub remote_root: String,
    pub username: String,
    pub password: String,
    pub ca_cert: String,
    pub insecure: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    pub timeout_secs: u64,
    pub retries: u32,
    pub keep_remote: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            timeout_secs: 20,
            retries: 2,
            keep_remote: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WifiConfig {
    pub manage: bool,
    pub ssid: String,
    pub psk: String,
    pub timeout_secs: u64,
}

impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            manage: true,
            ssid: String::new(),
            psk: String::new(),
            timeout_secs: 30,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub webdav: WebDavConfig,
    pub device: DeviceConfig,
    pub sync: SyncConfig,
    pub wifi: WifiConfig,
}

/// Loaded + validated configuration, remembering where it came from.
#[derive(Clone, Debug)]
pub struct Config {
    pub file: ConfigFile,
    pub path: PathBuf,
    endpoint: Endpoint,
}

impl Config {
    pub fn webdav(&self) -> &WebDavConfig {
        &self.file.webdav
    }

    pub fn sync_cfg(&self) -> &SyncConfig {
        &self.file.sync
    }

    pub fn wifi(&self) -> &WifiConfig {
        &self.file.wifi
    }

    pub fn device(&self) -> &DeviceConfig {
        &self.file.device
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn remote_root_segments(&self) -> Vec<String> {
        split_remote_root(&self.file.webdav.remote_root)
    }

    pub fn remote_ref(&self) -> (String, String) {
        (
            self.file.webdav.url.clone(),
            self.file.webdav.remote_root.clone(),
        )
    }

    /// Every configured sync method (`storages.toml`), not filtered or
    /// validated: callers decide which `kind`s they handle.
    ///
    /// The active entry always reflects `self.file.webdav` (which already
    /// includes the environment password override); the other entries keep the
    /// sidecar values verbatim.  Without a usable sidecar this is a single
    /// synthetic `default` entry built from the legacy `[webdav]` section.
    pub fn all_storages(&self) -> Vec<StorageConfig> {
        let Some(storage) = self.read_storage_file() else {
            return vec![self.default_storage()];
        };
        let active = storage.active.trim();
        if active.is_empty() || !storage.storage.iter().any(|entry| entry.id == active) {
            return vec![self.default_storage()];
        }
        storage
            .storage
            .iter()
            .map(|entry| {
                let mut storage = entry.clone();
                if entry.id == active {
                    let webdav = &self.file.webdav;
                    storage.url = webdav.url.clone();
                    storage.remote_root = webdav.remote_root.clone();
                    storage.username = webdav.username.clone();
                    storage.password = webdav.password.clone();
                    storage.ca_cert = webdav.ca_cert.clone();
                    storage.insecure = webdav.insecure;
                }
                storage
            })
            .collect()
    }

    /// Id of the active sync method (`default` when no sidecar is present).
    pub fn active_storage_id(&self) -> String {
        if let Some(storage) = self.read_storage_file() {
            let active = storage.active.trim();
            if !active.is_empty() && storage.storage.iter().any(|entry| entry.id == active) {
                return active.to_string();
            }
        }
        "default".to_string()
    }

    /// Where the optional named-profile sidecar lives (next to `config.toml`).
    pub fn storages_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(STORAGES_FILE)
    }

    fn read_storage_file(&self) -> Option<StorageFile> {
        let text = std::fs::read_to_string(self.storages_path()).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        let storage: StorageFile = toml::from_str(&text).ok()?;
        if storage.storage.is_empty() {
            return None;
        }
        Some(storage)
    }

    fn default_storage(&self) -> StorageConfig {
        let webdav = &self.file.webdav;
        StorageConfig {
            id: "default".to_string(),
            name: "默认".to_string(),
            kind: "webdav".to_string(),
            url: webdav.url.clone(),
            remote_root: webdav.remote_root.clone(),
            username: webdav.username.clone(),
            password: webdav.password.clone(),
            ca_cert: webdav.ca_cert.clone(),
            insecure: webdav.insecure,
        }
    }

    /// Warnings to show once after loading (never include the password).
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.endpoint.tls && self.file.webdav.insecure {
            out.push("webdav.insecure=true，已禁用 TLS 证书校验，仅用于调试".to_string());
        }
        if !self.endpoint.tls {
            out.push("WebDAV 未使用 HTTPS，仅建议在可信局域网内调试".to_string());
        }
        out
    }

    /// Load and validate `<root>/linkSlot/Config/config.toml` (or `explicit`).
    pub fn load(root: &Path, explicit: Option<&Path>) -> Result<Config> {
        let path = match explicit {
            Some(path) => path.to_path_buf(),
            None => root.join(CONFIG_REL),
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(LinkError::config(format!(
                    "未找到配置文件 {}；可运行 `linkSlot config init --root {}` 生成模板后编辑",
                    path.display(),
                    root.display()
                )));
            }
            Err(err) => return Err(fs_error(format!("无法读取 {}", path.display()), err)),
        };
        let mut file: ConfigFile = toml::from_str(&text).map_err(|err| {
            LinkError::config(format!("配置文件 {} 解析失败：{err}", path.display()))
        })?;
        apply_storage_file(&mut file, &path)?;
        apply_env_overrides(&mut file, |name| std::env::var(name).ok());
        let mut config = Config {
            file,
            path,
            endpoint: Endpoint::parse("http://placeholder.invalid").expect("placeholder url"),
        };
        config.validate()?;
        Ok(config)
    }

    /// Load the configuration for a `mods run` manual job.
    ///
    /// Manual mods (for example a LAN peer such as GBAStation) do not need a
    /// configured WebDAV endpoint, so a missing file and an empty
    /// `webdav.url` are both allowed. Everything else (sync/wifi limits, CA
    /// path, storages sidecar) keeps the normal validation.
    pub fn load_for_mods(root: &Path, explicit: Option<&Path>) -> Result<Config> {
        let path = match explicit {
            Some(path) => path.to_path_buf(),
            None => root.join(CONFIG_REL),
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(fs_error(format!("无法读取 {}", path.display()), err)),
        };
        let mut file: ConfigFile = if text.trim().is_empty() {
            ConfigFile::default()
        } else {
            toml::from_str(&text).map_err(|err| {
                LinkError::config(format!("配置文件 {} 解析失败：{err}", path.display()))
            })?
        };
        apply_storage_file(&mut file, &path)?;
        apply_env_overrides(&mut file, |name| std::env::var(name).ok());
        let mut config = Config {
            file,
            path,
            endpoint: Endpoint::parse("http://placeholder.invalid").expect("placeholder url"),
        };
        config.validate_for_mods()?;
        Ok(config)
    }

    /// Build a `Config` from an already-parsed file (tests and callers that
    /// keep the TOML in memory). `path` is only used for messages/logs.
    pub fn from_file(file: ConfigFile, path: PathBuf) -> Result<Config> {
        let mut config = Config {
            file,
            path,
            endpoint: Endpoint::parse("http://placeholder.invalid").expect("placeholder url"),
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate fields and build derived values (endpoint, normalized root).
    pub fn validate(&mut self) -> Result<()> {
        let url = self.file.webdav.url.trim().to_string();
        if url.is_empty() {
            return Err(LinkError::config("配置错误：webdav.url 不能为空"));
        }
        self.apply_webdav_endpoint(&url)?;
        self.validate_limits_and_ca()
    }

    /// Lenient variant used by `mods run`: an empty/missing `[webdav]` is fine
    /// because a manual mod may only talk to a LAN peer from its own config.
    pub fn validate_for_mods(&mut self) -> Result<()> {
        let url = self.file.webdav.url.trim().to_string();
        if !url.is_empty() {
            self.apply_webdav_endpoint(&url)?;
        }
        self.validate_limits_and_ca()
    }

    fn apply_webdav_endpoint(&mut self, url: &str) -> Result<()> {
        let endpoint = Endpoint::parse(url)
            .map_err(|err| LinkError::config(format!("配置错误：webdav.url {}", err.message())))?;
        if endpoint.tls && !cfg!(feature = "tls-rustls") {
            return Err(LinkError::config(
                "配置错误：本构建不支持 HTTPS（未启用 tls-rustls feature）；请改用 http:// 或使用设备发行版",
            ));
        }
        let remote_root =
            normalize_remote_root(&self.file.webdav.remote_root).map_err(|message| {
                LinkError::config(format!("配置错误：webdav.remote_root {message}"))
            })?;
        if self.file.webdav.username.is_empty() && !self.file.webdav.password.is_empty() {
            return Err(LinkError::config(
                "配置错误：webdav.password 已设置但 webdav.username 为空",
            ));
        }
        self.file.webdav.url = url.to_string();
        self.file.webdav.remote_root = remote_root;
        self.endpoint = endpoint;
        Ok(())
    }

    fn validate_limits_and_ca(&mut self) -> Result<()> {
        let timeout = self.file.sync.timeout_secs;
        if timeout == 0 || timeout > 600 {
            return Err(LinkError::config(
                "配置错误：sync.timeout_secs 必须在 1..=600 之间",
            ));
        }
        if self.file.sync.retries > 5 {
            return Err(LinkError::config(
                "配置错误：sync.retries 必须在 0..=5 之间",
            ));
        }
        let wifi_timeout = self.file.wifi.timeout_secs;
        if wifi_timeout == 0 || wifi_timeout > 300 {
            return Err(LinkError::config(
                "配置错误：wifi.timeout_secs 必须在 1..=300 之间",
            ));
        }
        let ca_cert = self.file.webdav.ca_cert.trim();
        if !ca_cert.is_empty() && !self.file.webdav.insecure {
            let path = Path::new(ca_cert);
            match std::fs::metadata(path) {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => {
                    return Err(LinkError::config(format!(
                        "配置错误：webdav.ca_cert {} 不是文件",
                        path.display()
                    )));
                }
                Err(err) => {
                    return Err(LinkError::config(format!(
                        "配置错误：webdav.ca_cert {} 无法读取：{err}",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// True when WebDAV URLs in this config are HTTP (no TLS needed).
    pub fn is_plain_http(&self) -> bool {
        !self.endpoint.tls
    }
}

/// Apply the optional named-profile sidecar.  The sidecar is authoritative for
/// the active WebDAV; an absent/empty sidecar keeps the legacy `[webdav]`.
fn apply_storage_file(file: &mut ConfigFile, config_path: &Path) -> Result<()> {
    let path = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STORAGES_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(LinkError::config(format!(
                "无法读取同步方式配置 {}：{err}",
                path.display()
            )));
        }
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    let storage: StorageFile = toml::from_str(&text).map_err(|err| {
        LinkError::config(format!("同步方式配置 {} 解析失败：{err}", path.display()))
    })?;
    if storage.storage.is_empty() {
        return Ok(());
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for entry in &storage.storage {
        if !valid_storage_id(&entry.id) {
            return Err(LinkError::config(format!(
                "同步方式 id「{}」无效",
                entry.id
            )));
        }
        if !ids.insert(entry.id.as_str()) {
            return Err(LinkError::config(format!(
                "同步方式 id「{}」重复",
                entry.id
            )));
        }
        if entry.name.trim().is_empty() || entry.name.chars().count() > 64 {
            return Err(LinkError::config(format!(
                "同步方式「{}」名称必须为 1–64 个字符",
                entry.id
            )));
        }
        if !names.insert(entry.name.as_str()) {
            return Err(LinkError::config(format!(
                "同步方式名称「{}」重复",
                entry.name
            )));
        }
    }
    let active = storage
        .storage
        .iter()
        .find(|entry| entry.id == storage.active)
        .ok_or_else(|| {
            LinkError::config(format!(
                "{} 的 active=\"{}\" 不存在",
                path.display(),
                storage.active
            ))
        })?;
    if active.kind != "webdav" {
        return Err(LinkError::config(format!(
            "同步方式「{}」的 kind=\"{}\" 暂不支持",
            active.name, active.kind
        )));
    }
    file.webdav = WebDavConfig {
        url: active.url.clone(),
        remote_root: if active.remote_root.trim().is_empty() {
            "/linkSlot".to_string()
        } else {
            active.remote_root.clone()
        },
        username: active.username.clone(),
        password: active.password.clone(),
        ca_cert: active.ca_cert.clone(),
        insecure: active.insecure,
    };
    Ok(())
}

fn valid_storage_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && id
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
}

/// Apply the password environment override. Exposed with an injected getter so
/// tests do not have to touch process-global environment variables.
pub fn apply_env_overrides<F>(file: &mut ConfigFile, get_env: F)
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(password) = get_env(PASSWORD_ENV) {
        file.webdav.password = password;
    }
}

/// Normalize a configured remote root to `/a/b` form (empty means root).
pub fn normalize_remote_root(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains('\\') {
        return Err("不能包含反斜杠".to_string());
    }
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err("不能包含 ? 或 #".to_string());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return Err("不能包含 . 或 .. 路径段".to_string());
        }
        if segment.chars().any(char::is_control) {
            return Err("不能包含控制字符".to_string());
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("/{}", segments.join("/")))
}

/// Split a normalized remote root into its segments.
pub fn split_remote_root(normalized: &str) -> Vec<String> {
    normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// Result of `config init`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitResult {
    Created(PathBuf),
    Existing(PathBuf),
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_REL)
}

/// Create the template config unless it already exists (unless `force`).
pub fn init(root: &Path, force: bool) -> Result<InitResult> {
    let root_meta = std::fs::metadata(root).map_err(|err| {
        LinkError::config(format!("配置错误：根目录 {} 不可用：{err}", root.display()))
    })?;
    if !root_meta.is_dir() {
        return Err(LinkError::config(format!(
            "配置错误：根目录 {} 不是目录",
            root.display()
        )));
    }
    let path = config_path(root);
    if path.exists() && !force {
        return Ok(InitResult::Existing(path));
    }
    let dir = root.join(CONFIG_DIR_REL);
    std::fs::create_dir_all(&dir)
        .map_err(|err| fs_error(format!("无法创建目录 {}", dir.display()), err))?;
    store::atomic_write(&path, TEMPLATE.as_bytes())?;
    Ok(InitResult::Created(path))
}

/// Persistent device id: `<BASEOS_TARGET>-<8 hex>`.
///
/// Order: explicit `[device] id` override -> matching cached `device.txt` ->
/// `/etc/machine-id` -> a generated seed.  Old `rg34xx-*` auto ids are
/// deliberately not migrated in the test build.
pub fn device_id(root: &Path, config: &Config) -> String {
    device_id_with(root, config, &DeviceInfo::current())
}

/// Testable device-id implementation with an injected hardware identity.
pub fn device_id_with(root: &Path, config: &Config, device: &DeviceInfo) -> String {
    let prefix = device.slug();
    let configured = config.file.device.id.trim();
    // An explicit [device] id is a deliberate override (the test harness and
    // users with a pre-existing custom id rely on it).  Automatic ids below
    // always use the actual BaseOS model.
    if !configured.is_empty() {
        return configured.to_string();
    }
    let path = root.join(DEVICE_FILE_REL);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if valid_id(text, &prefix) {
            return text.to_string();
        }
    }
    if let Some(machine_id) = read_machine_id() {
        let id = derive_device_id(&prefix, machine_id.trim().as_bytes());
        let _ = write_device_file(&path, &id);
        return id;
    }
    let seed = format!(
        "{}-{}-{}",
        std::process::id(),
        crate::util::now_secs(),
        crate::util::unique_suffix()
    );
    let id = derive_device_id(&prefix, seed.as_bytes());
    let _ = write_device_file(&path, &id);
    id
}

fn valid_id(id: &str, prefix: &str) -> bool {
    let Some(suffix) = id.strip_prefix(prefix).and_then(|s| s.strip_prefix('-')) else {
        return false;
    };
    suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit())
}

fn derive_device_id(prefix: &str, seed: &[u8]) -> String {
    let digest = hash::sha256_hex(seed);
    format!("{prefix}-{}", &digest[..8])
}

fn write_device_file(path: &Path, id: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    store::atomic_write(path, format!("{id}\n").as_bytes())
        .map_err(|err| std::io::Error::other(err.to_string()))
}

fn read_machine_id() -> Option<String> {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// The template written by `linkSlot config init` (same shape as project book
/// v0.5 §4.5). No password is ever pre-filled.
pub const TEMPLATE: &str = r#"# linkSlot 配置文件（FAT32 上无法设置 600 权限；密码建议用环境变量
# LINKSLOT_WEBDAV_PASSWORD 注入，本文件留空即可）。
#
# 首次使用：编辑本文件后运行 `linkSlot push|pull --platform ... --game ...`。

[webdav]
# WebDAV 端点：scheme + host + base path，不要包含 query/fragment。
url = "https://nas.example.com/dav"
# 云端根目录，拼在 url 之后；linkSlot 会逐段做百分号编码。
remote_root = "/linkSlot"
username = "rg34xx"
# 建议留空，使用环境变量 LINKSLOT_WEBDAV_PASSWORD。
password = ""
# 自签证书/私有 CA 的 PEM 文件路径（可选；留空表示使用内置根证书）。
ca_cert = ""
# 仅调试用：禁用证书校验。生产必须为 false。
insecure = false

[device]
# 留空时自动使用 BaseOS 实际机型 + /etc/machine-id 派生；也可手工指定。
id = ""

[sync]
# 单次 HTTP 请求超时（秒）。
timeout_secs = 20
# 可重试错误（连接失败/超时/5xx）的重试次数。
retries = 2
# false：push 成功后 best-effort 清理云端已不在新 meta 中的旧数据文件。
keep_remote = false

[wifi]
# true：网络不可用时由 linkSlot 按需执行 wpa_supplicant + udhcpc；
# false：只探测，由外部负责联网。
manage = true
ssid = ""
psk = ""
auto_off_minutes = 10
# 联网总超时（秒）。
timeout_secs = 30
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::util::unique_suffix;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("linkslot-cfg-{tag}-{}", unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_config(root: &Path, body: &str) -> PathBuf {
        let path = config_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    const MINIMAL: &str = r#"
[webdav]
url = "http://127.0.0.1:8080/dav"
remote_root = "linkSlot/"
username = "u"
password = "p"
"#;

    #[test]
    fn loads_minimal_config_and_normalizes_root() {
        let root = temp_root("min");
        write_config(&root, MINIMAL);
        let cfg = Config::load(&root, None).unwrap();
        assert_eq!(cfg.file.webdav.remote_root, "/linkSlot");
        assert_eq!(cfg.remote_root_segments(), ["linkSlot"]);
        assert!(!cfg.endpoint().tls);
        assert_eq!(cfg.file.sync.timeout_secs, 20);
        assert_eq!(cfg.file.sync.retries, 2);
        assert!(cfg.file.wifi.manage);
        // http is a debug-only warning; https+insecure is a real warning
        assert!(cfg.warnings().iter().any(|w| w.contains("HTTPS")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_config_is_config_error_with_hint() {
        let root = temp_root("missing");
        let err = Config::load(&root, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("config init"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_for_mods_accepts_missing_config_and_empty_webdav() {
        let root = temp_root("mods-missing");
        let cfg = Config::load_for_mods(&root, None).unwrap();
        assert!(cfg.webdav().url.is_empty());
        assert_eq!(cfg.sync_cfg().timeout_secs, 20);
        assert!(cfg.wifi().manage);
        let _ = std::fs::remove_dir_all(&root);

        let root = temp_root("mods-empty");
        write_config(&root, "[webdav]\nurl = \"\"\n");
        let cfg = Config::load_for_mods(&root, None).unwrap();
        assert!(cfg.webdav().url.is_empty());
        // The strict loader still needs a WebDAV URL.
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_for_mods_still_rejects_bad_toml_and_limits() {
        let root = temp_root("mods-bad");
        write_config(&root, "[webdav\nurl =");
        assert_eq!(
            Config::load_for_mods(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );
        let _ = std::fs::remove_dir_all(&root);

        let root = temp_root("mods-limits");
        write_config(&root, "[webdav]\nurl = \"\"\n[sync]\ntimeout_secs = 0\n");
        assert_eq!(
            Config::load_for_mods(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn explicit_config_path_is_used() {
        let root = temp_root("explicit");
        let path = root.join("elsewhere.toml");
        std::fs::write(&path, MINIMAL).unwrap();
        let cfg = Config::load(&root, Some(&path)).unwrap();
        assert_eq!(cfg.path, path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn named_storages_select_the_active_webdav() {
        let root = temp_root("storages");
        write_config(&root, MINIMAL);
        std::fs::write(
            root.join(CONFIG_DIR_REL).join(STORAGES_FILE),
            r#"active = "work"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "http://home.example/dav"
remote_root = "/home"
username = "u1"
password = "p1"

[[storage]]
id = "work"
name = "公司"
kind = "webdav"
url = "http://work.example/dav"
remote_root = "/work"
username = "u2"
password = "p2"
"#,
        )
        .unwrap();
        let cfg = Config::load(&root, None).unwrap();
        assert_eq!(cfg.webdav().url, "http://work.example/dav");
        assert_eq!(cfg.webdav().username, "u2");
        assert_eq!(cfg.webdav().password, "p2");
        assert_eq!(cfg.remote_root_segments(), ["work"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn named_storages_reject_missing_active_and_unsupported_kind() {
        let root = temp_root("storages-bad");
        write_config(&root, MINIMAL);
        let dir = root.join(CONFIG_DIR_REL);
        std::fs::write(
            dir.join(STORAGES_FILE),
            r#"active = "missing"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "http://home/dav"
"#,
        )
        .unwrap();
        let err = Config::load(&root, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("active"));

        std::fs::write(
            dir.join(STORAGES_FILE),
            r#"active = "home"

[[storage]]
id = "home"
name = "家里"
kind = "http"
url = "http://home/dav"
"#,
        )
        .unwrap();
        let err = Config::load(&root, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("暂不支持"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn named_storages_reject_duplicate_ids_and_names() {
        let root = temp_root("storages-dup");
        write_config(&root, MINIMAL);
        let dir = root.join(CONFIG_DIR_REL);
        std::fs::write(
            dir.join(STORAGES_FILE),
            r#"active = "a"

[[storage]]
id = "a"
name = "same"
kind = "webdav"
url = "http://a/dav"

[[storage]]
id = "a"
name = "same"
kind = "webdav"
url = "http://b/dav"
"#,
        )
        .unwrap();
        let err = Config::load(&root, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("重复"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn password_env_override_applies() {
        let mut file = ConfigFile::default();
        apply_env_overrides(&mut file, |name| {
            (name == PASSWORD_ENV).then(|| "from-env".to_string())
        });
        assert_eq!(file.webdav.password, "from-env");

        let mut file = ConfigFile::default();
        apply_env_overrides(&mut file, |_| None);
        assert_eq!(file.webdav.password, "");
    }

    #[test]
    fn rejects_invalid_configs() {
        let root = temp_root("invalid");
        write_config(&root, "[webdav]\nurl = \"\"\n");
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );

        write_config(&root, "[webdav]\nurl = \"ftp://x\"\n");
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );

        write_config(
            &root,
            "[webdav]\nurl = \"https://x\"\n[webdav.remote_root]\n",
        );
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );

        write_config(
            &root,
            "[webdav]\nurl = \"http://x\"\nremote_root = \"/a/../b\"\n",
        );
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );

        write_config(&root, "[webdav]\nurl = \"http://x\"\npassword = \"p\"\n");
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );

        write_config(
            &root,
            "[webdav]\nurl = \"http://x\"\n[sync]\ntimeout_secs = 0\n",
        );
        assert_eq!(
            Config::load(&root, None).unwrap_err().kind(),
            ErrorKind::Config
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn https_without_feature_is_rejected_when_tls_is_off() {
        if cfg!(feature = "tls-rustls") {
            return;
        }
        let root = temp_root("https");
        write_config(&root, "[webdav]\nurl = \"https://nas.example.com/dav\"\n");
        let err = Config::load(&root, None).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("不支持 HTTPS") || err.message().contains("tls-rustls"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_creates_template_then_reports_existing() {
        let root = temp_root("init");
        let created = init(&root, false).unwrap();
        let InitResult::Created(path) = created else {
            panic!("expected Created");
        };
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[webdav]"));
        assert!(text.contains("remote_root"));
        // No pre-filled password.
        assert!(text.contains("password = \"\""));

        let again = init(&root, false).unwrap();
        assert_eq!(again, InitResult::Existing(path.clone()));

        let forced = init(&root, true).unwrap();
        assert_eq!(forced, InitResult::Created(path));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_missing_root_is_config_error() {
        let root = std::env::temp_dir().join(format!("linkslot-nope-{}", unique_suffix()));
        let err = init(&root, false).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn device_id_uses_the_actual_model_and_is_persisted() {
        let root = temp_root("device");
        write_config(&root, MINIMAL);
        let mut cfg = Config::load(&root, None).unwrap();
        let info = DeviceInfo::new("rg34xxsp", "Anbernic RG34XX SP");
        cfg.file.device.id = "rg34xxsp-deadbeef".to_string();
        assert_eq!(device_id_with(&root, &cfg, &info), "rg34xxsp-deadbeef");

        // Without an explicit override the generated id must carry the actual
        // model prefix; an old rg34xx file must not survive on another model.
        cfg.file.device.id.clear();
        let id = device_id_with(&root, &cfg, &info);
        assert!(id.starts_with("rg34xxsp-"), "unexpected id {id}");
        assert_eq!(id.len(), "rg34xxsp-".len() + 8);
        assert!(id["rg34xxsp-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        let again = device_id_with(&root, &cfg, &info);
        assert_eq!(id, again);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_rg34xx_device_file_is_replaced_for_another_model() {
        let root = temp_root("device-old");
        write_config(&root, MINIMAL);
        let mut cfg = Config::load(&root, None).unwrap();
        cfg.file.device.id.clear();
        let path = root.join(DEVICE_FILE_REL);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "rg34xx-deadbeef\n").unwrap();

        let info = DeviceInfo::new("rg34xxsp", "Anbernic RG34XX SP");
        let id = device_id_with(&root, &cfg, &info);
        assert!(
            id.starts_with("rg34xxsp-"),
            "old rg34xx id survived on RG34XXSP: {id}"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap().trim(), id);
        let other = DeviceInfo::new("rg34xx", "Anbernic RG34XX");
        assert!(device_id_with(&root, &cfg, &other).starts_with("rg34xx-"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn normalizes_and_splits_remote_root() {
        assert_eq!(normalize_remote_root("").unwrap(), "");
        assert_eq!(normalize_remote_root("/").unwrap(), "");
        assert_eq!(normalize_remote_root("/linkSlot").unwrap(), "/linkSlot");
        assert_eq!(normalize_remote_root("linkSlot/").unwrap(), "/linkSlot");
        assert_eq!(normalize_remote_root(" /a/b//c/ ").unwrap(), "/a/b/c");
        assert!(normalize_remote_root("/a/../b").is_err());
        assert!(normalize_remote_root("a\\b").is_err());
        assert_eq!(
            split_remote_root("/a/b"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(split_remote_root("").is_empty());
    }

    #[test]
    fn all_storages_without_sidecar_returns_the_legacy_default() {
        let root = temp_root("all-storages-default");
        write_config(&root, MINIMAL);
        let cfg = Config::load(&root, None).unwrap();
        let storages = cfg.all_storages();
        assert_eq!(storages.len(), 1);
        let storage = &storages[0];
        assert_eq!(storage.id, "default");
        assert_eq!(storage.kind, "webdav");
        assert_eq!(storage.url, cfg.webdav().url);
        assert_eq!(storage.remote_root, "/linkSlot");
        assert_eq!(storage.username, "u");
        assert_eq!(storage.password, "p");
        assert_eq!(cfg.active_storage_id(), "default");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn all_storages_override_the_active_entry_and_keep_the_others() {
        let root = temp_root("all-storages-active");
        write_config(&root, MINIMAL);
        std::fs::write(
            root.join(CONFIG_DIR_REL).join(STORAGES_FILE),
            r#"active = "work"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "http://home.example/dav"
remote_root = "/home"
username = "u1"
password = "p1"
insecure = false

[[storage]]
id = "work"
name = "公司"
kind = "webdav"
url = "http://work.example/dav"
remote_root = "/work"
username = "u2"
password = "p2"
insecure = true
"#,
        )
        .unwrap();
        let mut cfg = Config::load(&root, None).unwrap();
        // The active entry is whatever `self.file.webdav` says after load and
        // the environment override (simulated here without touching env vars).
        cfg.file.webdav.url = "http://env.example/dav".to_string();
        cfg.file.webdav.remote_root = "/env".to_string();
        cfg.file.webdav.username = "env-user".to_string();
        cfg.file.webdav.password = "env-pass".to_string();

        let storages = cfg.all_storages();
        assert_eq!(storages.len(), 2);
        assert_eq!(storages[0].id, "home");
        assert_eq!(storages[0].name, "家里");
        assert_eq!(storages[0].url, "http://home.example/dav");
        assert_eq!(storages[0].remote_root, "/home");
        assert_eq!(storages[0].password, "p1");
        assert!(!storages[0].insecure);

        assert_eq!(storages[1].id, "work");
        assert_eq!(storages[1].name, "公司");
        assert_eq!(storages[1].url, "http://env.example/dav");
        assert_eq!(storages[1].remote_root, "/env");
        assert_eq!(storages[1].username, "env-user");
        assert_eq!(storages[1].password, "env-pass");
        assert!(storages[1].insecure);
        assert_eq!(cfg.active_storage_id(), "work");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn all_storages_fall_back_when_the_active_id_is_missing() {
        let root = temp_root("all-storages-ghost");
        write_config(&root, MINIMAL);
        let mut file = ConfigFile::default();
        file.webdav.url = "http://legacy.example/dav".to_string();
        file.webdav.remote_root = "/legacy".to_string();
        file.webdav.username = "legacy-user".to_string();
        let cfg = Config::from_file(file, root.join(CONFIG_REL)).unwrap();
        std::fs::write(
            root.join(CONFIG_DIR_REL).join(STORAGES_FILE),
            r#"active = "ghost"

[[storage]]
id = "home"
name = "家里"
kind = "webdav"
url = "http://home.example/dav"
"#,
        )
        .unwrap();

        let storages = cfg.all_storages();
        assert_eq!(storages.len(), 1);
        assert_eq!(storages[0].id, "default");
        assert_eq!(storages[0].url, "http://legacy.example/dav");
        assert_eq!(storages[0].remote_root, "/legacy");
        assert_eq!(storages[0].username, "legacy-user");
        assert_eq!(cfg.active_storage_id(), "default");

        // A corrupt/empty sidecar also falls back instead of panicking.
        std::fs::write(root.join(CONFIG_DIR_REL).join(STORAGES_FILE), "not toml").unwrap();
        assert_eq!(cfg.all_storages().len(), 1);
        assert_eq!(cfg.active_storage_id(), "default");
        let _ = std::fs::remove_dir_all(&root);
    }
}
