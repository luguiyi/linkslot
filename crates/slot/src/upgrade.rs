use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use flate2::read::DeflateDecoder;

pub(crate) const UPGRADE_REL: &str = "linkSlot/Config/upgrade";
pub(crate) const STAGE_DIR: &str = "stage";
pub(crate) const STATUS_FILE: &str = "status.json";
const MAX_ZIP_BYTES: usize = 32 * 1024 * 1024;
const MAX_UNCOMPRESSED: usize = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const MAX_ENTRY: usize = 64 * 1024 * 1024;
/// 开发/测试期允许重装同版本；v1.0.0 正式发布时改为 false，只接受更高版本。
pub(crate) const ALLOW_SAME_VERSION_REINSTALL: bool = true;
const REQUIRED_FILES: [&str; 4] = [
    "System/linkSlot/bin/linkslot",
    "System/linkSlot/bin/linkslot_sync",
    "System/launch_frontend.sh",
    "System/linkSlot/shell/linkslot.sh",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Status {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub files: usize,
    #[serde(default)]
    pub bytes: usize,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub message: String,
    /// 目标版本与当前版本相同（开发期允许的重装）。
    #[serde(default)]
    pub same_version: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            target: String::new(),
            files: 0,
            bytes: 0,
            build: String::new(),
            message: String::new(),
            same_version: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Manifest {
    schema: u32,
    product: String,
    version: String,
    #[serde(default)]
    build: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug)]
struct ZipEntry {
    name: String,
    data: Vec<u8>,
}

pub(crate) fn upgrade_dir(root: &Path) -> PathBuf {
    root.join(UPGRADE_REL)
}

fn status_path(root: &Path) -> PathBuf {
    upgrade_dir(root).join(STATUS_FILE)
}

/// Read the persisted status.  Missing/corrupt status means idle; a failed
/// status is returned as an error so the device can tell the user why.
pub(crate) fn staged(root: &Path) -> Result<Option<Status>, String> {
    let mut status = read_status(root);
    reconcile_installed(root, &mut status);
    if status.state == "staged" {
        let slot = upgrade_dir(root).join(STAGE_DIR).join("System/linkSlot/bin/linkslot");
        if slot.is_file() {
            return Ok(Some(status));
        }
        return Err("升级包已损坏，请重新上传".into());
    }
    if status.state == "failed" {
        return Err(if status.message.is_empty() {
            "上次升级失败".into()
        } else {
            status.message
        });
    }
    Ok(None)
}

pub(crate) fn read_status(root: &Path) -> Status {
    let Ok(bytes) = std::fs::read(status_path(root)) else {
        return Status::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// After a successful reboot the target version is now running; clear the
/// stale "rebooting" marker and staging tree so About shows an idle state and
/// the next upload starts clean.  The old-System backup is intentionally kept.
fn reconcile_installed(root: &Path, status: &mut Status) {
    if (status.state == "applying" || status.state == "rebooting")
        && status.target == slot_ui::PROJECT_VERSION
    {
        let dir = upgrade_dir(root);
        let _ = std::fs::remove_dir_all(dir.join(STAGE_DIR));
        let _ = std::fs::remove_file(dir.join("apply.sh"));
        let _ = std::fs::remove_file(dir.join("apply.log"));
        let _ = std::fs::remove_file(dir.join("dirs.list"));
        let _ = std::fs::remove_file(dir.join("files.list"));
        *status = Status::default();
        let _ = write_status(root, status);
    }
}

fn write_status(root: &Path, status: &Status) -> Result<(), String> {
    let dir = upgrade_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建升级目录失败：{e}"))?;
    let bytes = serde_json::to_vec_pretty(status).map_err(|e| format!("状态序列化失败：{e}"))?;
    let path = status_path(root);
    let tmp = dir.join("status.json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入升级状态失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("提交升级状态失败：{e}"))?;
    Ok(())
}

pub(crate) fn status_value(root: &Path) -> Value {
    let mut status = read_status(root);
    reconcile_installed(root, &mut status);
    json!({
        "state": status.state,
        "target": status.target,
        "files": status.files,
        "bytes": status.bytes,
        "build": status.build,
        "message": status.message,
        "same_version": status.same_version,
        "current": slot_ui::PROJECT_VERSION,
    })
}

/// Validate a version zip and extract only System/** + linkSlot/version.json into the
/// hidden staging directory.  Existing System/ is never touched here.
pub(crate) fn stage(root: &Path, bytes: &[u8]) -> Result<Status, String> {
    if bytes.len() > MAX_ZIP_BYTES {
        return Err(format!(
            "升级包超过 {} MiB 上限",
            MAX_ZIP_BYTES / (1024 * 1024)
        ));
    }
    let entries = parse_zip(bytes)?;
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    let mut manifest_bytes = None;
    for entry in entries {
        if entry.name == "linkSlot/version.json" || entry.name == "version.json" {
            if manifest_bytes.replace(entry.data).is_some() {
                return Err("升级包包含重复的 version.json".into());
            }
            continue;
        }
        if entry.name == "System/" || entry.name.starts_with("System/") {
            if entry.name.ends_with('/') {
                continue;
            }
            if files.insert(entry.name.clone(), entry.data).is_some() {
                return Err(format!("升级包包含重复文件：{}", entry.name));
            }
            continue;
        }
        // A release zip also carries user-data directory skeletons, Config/
        // bootlogo and README.  They are deliberately ignored: only System/**
        // is installed and user data is never overwritten.
        continue;
    }
    let Some(manifest_bytes) = manifest_bytes else {
        return Err("升级包缺少 linkSlot/version.json，请使用 v1.0.0 或更新的版本包".into());
    };
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("version.json 格式错误：{e}"))?;
    if manifest.schema != 1 || manifest.product != "linkslot" {
        return Err("version.json 不是 linkSlot 版本清单".into());
    }
    let current = slot_ui::PROJECT_VERSION;
    let same_version = version_is_equal(&manifest.version, current);
    if !version_is_newer(&manifest.version, current) {
        if same_version && ALLOW_SAME_VERSION_REINSTALL {
            // 开发期：允许重装同版本，方便反复验证。
        } else if same_version {
            return Err(format!(
                "目标版本 {} 与当前版本相同；正式版只接受更高版本",
                manifest.version
            ));
        } else {
            return Err(format!(
                "目标版本 {} 低于当前版本 {}，已拒绝升级",
                manifest.version, current
            ));
        }
    }
    if manifest.files.is_empty() {
        return Err("version.json 没有文件清单".into());
    }
    for required in REQUIRED_FILES {
        if !files.contains_key(required) {
            return Err(format!("升级包缺少必需文件：{required}"));
        }
        if !manifest.files.contains_key(required) {
            return Err(format!("version.json 未列出必需文件：{required}"));
        }
    }
    let mut total = 0usize;
    for (name, data) in &files {
        if !manifest.files.contains_key(name) {
            return Err(format!("version.json 未列出文件：{name}"));
        }
        total = total
            .checked_add(data.len())
            .ok_or_else(|| "升级包解压总量越界".to_string())?;
        if total > MAX_UNCOMPRESSED {
            return Err(format!(
                "升级包解压超过 {} MiB 上限",
                MAX_UNCOMPRESSED / (1024 * 1024)
            ));
        }
    }
    for (name, expected) in &manifest.files {
        // 升级只安装 System/**；清单里的其它条目（图鉴、README 等）不参与替换。
        if !name.starts_with("System/") {
            continue;
        }
        let Some(data) = files.get(name) else {
            return Err(format!("version.json 列出的文件不存在：{name}"));
        };
        let expected = expected.trim();
        let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
        if !digest_eq(expected, data) {
            return Err(format!("文件校验失败：{name}"));
        }
    }

    let dir = upgrade_dir(root);
    let stage = dir.join(STAGE_DIR);
    let stage_system = stage.join("System");
    if stage.exists() {
        std::fs::remove_dir_all(&stage).map_err(|e| format!("清理旧升级暂存失败：{e}"))?;
    }
    std::fs::create_dir_all(&stage_system).map_err(|e| format!("创建升级暂存失败：{e}"))?;
    let manifest_path = stage.join("linkSlot/version.json");
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建版本清单目录失败：{e}"))?;
    }
    std::fs::write(&manifest_path, &manifest_bytes)
        .map_err(|e| format!("写入版本清单失败：{e}"))?;
    for (name, data) in &files {
        let rel = name.strip_prefix("System/").unwrap_or(name);
        let dest = stage_system.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建暂存目录失败：{e}"))?;
        }
        std::fs::write(&dest, data).map_err(|e| format!("写入暂存文件失败：{e}"))?;
    }
    let status = Status {
        state: "staged".into(),
        target: manifest.version.clone(),
        files: files.len(),
        bytes: total,
        build: manifest.build,
        message: if same_version {
            "已上传同版本安装包，可在掌机 系统设置 → 系统升级 重装".into()
        } else {
            "已上传，请在掌机 系统设置 → 系统升级 确认安装".into()
        },
        same_version,
    };
    write_status(root, &status)?;
    Ok(status)
}

pub(crate) fn apply(root: &Path) -> Result<(), String> {
    let status = staged(root)?.ok_or_else(|| "没有已上传的版本包".to_string())?;
    if !cfg!(unix) {
        return Err("当前平台不支持设备升级".into());
    }
    let dir = upgrade_dir(root);
    let stage = dir.join(STAGE_DIR);
    let script = dir.join("apply.sh");
    let helper = helper_script();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建升级目录失败：{e}"))?;
    std::fs::write(&script, helper).map_err(|e| format!("写入升级脚本失败：{e}"))?;
    let mut applying = status;
    applying.state = "applying".into();
    applying.message = "正在升级，设备即将重启".into();
    write_status(root, &applying)?;
    let target = applying.target.clone();
    let child = Command::new("/bin/sh")
        .arg(&script)
        .arg(root)
        .arg(&stage)
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            let failed = Status {
                state: "failed".into(),
                target: target.clone(),
                message: format!("无法启动升级进程：{e}"),
                ..Status::default()
            };
            let _ = write_status(root, &failed);
            format!("无法启动升级进程：{e}")
        })?;
    // Reap in the background; the helper reboots the device on success.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

fn digest_eq(expected: &str, data: &[u8]) -> bool {
    let actual = sha256_hex(data);
    expected.len() == 64
        && expected.bytes().all(|b| b.is_ascii_hexdigit())
        && actual.eq_ignore_ascii_case(expected)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn version_is_newer(target: &str, current: &str) -> bool {
    let Some(target) = parse_version(target) else {
        return false;
    };
    let Some(current) = parse_version(current) else {
        return false;
    };
    target > current
}

fn version_is_equal(left: &str, right: &str) -> bool {
    fn trimmed(mut parts: Vec<u64>) -> Vec<u64> {
        while parts.last() == Some(&0) {
            parts.pop();
        }
        parts
    }
    match (parse_version(left), parse_version(right)) {
        (Some(left), Some(right)) => trimmed(left) == trimmed(right),
        _ => false,
    }
}

fn parse_version(text: &str) -> Option<Vec<u64>> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }
    parts
        .iter()
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

fn parse_zip(data: &[u8]) -> Result<Vec<ZipEntry>, String> {
    if data.len() < 22 {
        return Err("不是完整的 ZIP 文件".into());
    }
    let eocd = find_eocd(data).ok_or_else(|| "找不到 ZIP 结束目录".to_string())?;
    let disk = u16_at(data, eocd + 4)?;
    let cd_disk = u16_at(data, eocd + 6)?;
    let entries_this_disk = u16_at(data, eocd + 8)? as usize;
    let total = u16_at(data, eocd + 10)? as usize;
    if disk != 0 || cd_disk != 0 || entries_this_disk != total {
        return Err("不支持分卷 ZIP".into());
    }
    if total > MAX_ENTRIES {
        return Err(format!("ZIP 条目超过 {MAX_ENTRIES} 个"));
    }
    let cd_size = u32_at(data, eocd + 12)? as usize;
    let cd_offset = u32_at(data, eocd + 16)? as usize;
    let cd_end = cd_offset
        .checked_add(cd_size)
        .ok_or_else(|| "ZIP 中央目录越界".to_string())?;
    if cd_end > eocd {
        return Err("ZIP 中央目录越界".into());
    }
    let mut entries = Vec::with_capacity(total);
    let mut pos = cd_offset;
    let mut total_uncompressed = 0usize;
    for _ in 0..total {
        if pos + 46 > cd_end || &data[pos..pos + 4] != b"PK\x01\x02" {
            return Err("ZIP 中央目录损坏".into());
        }
        let flags = u16_at(data, pos + 8)?;
        let method = u16_at(data, pos + 10)?;
        let crc = u32_at(data, pos + 16)?;
        let compressed_size = u32_at(data, pos + 20)? as usize;
        let uncompressed_size = u32_at(data, pos + 24)? as usize;
        let name_len = u16_at(data, pos + 28)? as usize;
        let extra_len = u16_at(data, pos + 30)? as usize;
        let comment_len = u16_at(data, pos + 32)? as usize;
        let local_offset = u32_at(data, pos + 42)? as usize;
        if flags & 1 != 0 {
            return Err("不支持加密 ZIP".into());
        }
        if method != 0 && method != 8 {
            return Err(format!("不支持的 ZIP 压缩方式 {method}"));
        }
        let name_end = pos + 46 + name_len;
        if name_end > cd_end {
            return Err("ZIP 文件名越界".into());
        }
        let raw_name = &data[pos + 46..name_end];
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| "ZIP 文件名不是 UTF-8".to_string())?
            .to_string();
        validate_name(&name)?;
        let is_dir = name.ends_with('/');
        if !is_dir {
            if uncompressed_size > MAX_ENTRY {
                return Err(format!("ZIP 单文件超过 {} MiB", MAX_ENTRY / (1024 * 1024)));
            }
            total_uncompressed = total_uncompressed
                .checked_add(uncompressed_size)
                .ok_or_else(|| "ZIP 解压总量越界".to_string())?;
            if total_uncompressed > MAX_UNCOMPRESSED {
                return Err(format!(
                    "ZIP 解压超过 {} MiB",
                    MAX_UNCOMPRESSED / (1024 * 1024)
                ));
            }
        }
        let data_start = local_data_start(data, local_offset, compressed_size)?;
        let data_end = data_start
            .checked_add(compressed_size)
            .ok_or_else(|| "ZIP 数据越界".to_string())?;
        if data_end > data.len() {
            return Err("ZIP 数据越界".into());
        }
        let entry_data = if is_dir {
            if uncompressed_size != 0 || compressed_size != 0 {
                return Err("ZIP 目录条目包含数据".into());
            }
            Vec::new()
        } else {
            let out = if method == 0 {
                if compressed_size != uncompressed_size {
                    return Err("ZIP 存储条目大小不一致".into());
                }
                data[data_start..data_end].to_vec()
            } else {
                let mut decoder = DeflateDecoder::new(&data[data_start..data_end]);
                let mut out = Vec::new();
                decoder
                    .by_ref()
                    .take(MAX_ENTRY as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("ZIP 解压失败：{e}"))?;
                if out.len() > MAX_ENTRY {
                    return Err("ZIP 单文件解压超过上限".into());
                }
                out
            };
            if out.len() != uncompressed_size {
                return Err("ZIP 条目解压大小不一致".into());
            }
            if crc32fast::hash(&out) != crc {
                return Err(format!("ZIP 文件 CRC 校验失败：{name}"));
            }
            out
        };
        entries.push(ZipEntry {
            name,
            data: entry_data,
        });
        pos = pos
            .checked_add(46 + name_len + extra_len + comment_len)
            .ok_or_else(|| "ZIP 中央目录越界".to_string())?;
    }
    Ok(entries)
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') {
        return Err("ZIP 文件路径不安全".into());
    }
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(format!("ZIP 文件路径不安全：{name}"));
    }
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("ZIP 文件路径不安全：{name}"));
        }
    }
    Ok(())
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 0xffff);
    (start..=data.len() - 22)
        .rev()
        .find(|&pos| &data[pos..pos + 4] == b"PK\x05\x06")
}

fn local_data_start(data: &[u8], offset: usize, compressed_size: usize) -> Result<usize, String> {
    let header = offset
        .checked_add(30)
        .ok_or_else(|| "ZIP 本地头越界".to_string())?;
    if header > data.len() || &data[offset..offset + 4] != b"PK\x03\x04" {
        return Err("ZIP 本地文件头损坏".into());
    }
    let name_len = u16_at(data, offset + 26)? as usize;
    let extra_len = u16_at(data, offset + 28)? as usize;
    let start = header
        .checked_add(name_len)
        .and_then(|v| v.checked_add(extra_len))
        .ok_or_else(|| "ZIP 本地头越界".to_string())?;
    if start
        .checked_add(compressed_size)
        .is_none_or(|end| end > data.len())
    {
        return Err("ZIP 数据越界".into());
    }
    Ok(start)
}

fn u16_at(data: &[u8], pos: usize) -> Result<u16, String> {
    let bytes = data
        .get(pos..pos + 2)
        .ok_or_else(|| "ZIP 读取越界".to_string())?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], pos: usize) -> Result<u32, String> {
    let bytes = data
        .get(pos..pos + 4)
        .ok_or_else(|| "ZIP 读取越界".to_string())?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn helper_script() -> &'static str {
    r#"#!/bin/sh
set -u
ROOT=$1
STAGE=$2
UP=$3
SYS=$ROOT/System
BACK=$UP/backup
OLD=$BACK/System-before-upgrade
STATUS=$UP/status.json
LOG=$UP/apply.log
TARGET=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$STAGE/version.json" | head -n 1)
mkdir -p "$UP" "$BACK"
log() { printf '%s %s\n' "$(date '+%F %T')" "$*" >> "$LOG" 2>/dev/null; }
status() { printf '{"state":"%s","target":"%s","message":"%s"}\n' "$1" "$TARGET" "$2" > "$STATUS"; }
fail() {
    log "FAILED: $1"
    if [ -d "$OLD" ]; then
        rm -rf "$SYS"
        cp -R "$OLD" "$SYS"
    fi
    status failed "$1"
    sync
    exit 1
}
status applying "正在备份当前系统"
rm -rf "$OLD"
if ! cp -R "$SYS" "$OLD"; then
    status failed "无法备份当前系统"
    exit 1
fi
find "$STAGE/System" -type d > "$UP/dirs.list" || fail "无法枚举升级目录"
while IFS= read -r dir; do
    rel=${dir#"$STAGE/System"}
    [ -n "$rel" ] && { mkdir -p "$SYS$rel" || fail "无法创建目录 $rel"; }
done < "$UP/dirs.list"
find "$STAGE/System" -type f > "$UP/files.list" || fail "无法枚举升级文件"
while IFS= read -r file; do
    rel=${file#"$STAGE/System"}
    dest="$SYS$rel"
    tmp="$dest.upgrade-tmp"
    mkdir -p "$(dirname "$dest")" || fail "无法创建目录 $rel"
    cp "$file" "$tmp" || fail "无法写入 $rel"
    mv -f "$tmp" "$dest" || fail "无法提交 $rel"
done < "$UP/files.list"
VER=$(/lib/ld-linux-aarch64.so.1 "$SYS/bin/linkslot_sync" version 2>&1 || true)
case "$VER" in
    *mods=1*) ;;
    *) log "new linkSlot failed: $VER"; fail "新 linkSlot 无法运行" ;;
esac
# The version manifest is root data, not System/**, so it needs its own
# commit.  Without this copy the binary updates while version.json keeps
# reporting the version that was installed from the SD card.
mkdir -p "$ROOT/linkSlot" || fail "无法创建 linkSlot 目录"
cp "$STAGE/linkSlot/version.json" "$ROOT/linkSlot/version.json.upgrade-tmp" || fail "无法写入 linkSlot/version.json"
mv -f "$ROOT/linkSlot/version.json.upgrade-tmp" "$ROOT/linkSlot/version.json" || fail "无法提交 linkSlot/version.json"
log "installed $TARGET"
status rebooting "升级完成，正在重启"
sync
# The app performs the clean restart (stop web, unload WiFi, settle saves,
# release logs, reboot). A bare reboot from this helper can hang the shutdown
# with the WiFi/DRM drivers still up, leaving the device powered with a dark
# screen and needing a long power-key hold.
log "rebooting is the app's job"
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_store(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len();
            let crc = crc32fast::hash(data);
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0x0800u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x21u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0x0800u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&(offset as u32).to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = out.len();
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&(central_offset as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn manifest(files: &[(&str, &[u8])]) -> String {
        manifest_version(files, "99.0.0")
    }

    fn manifest_version(files: &[(&str, &[u8])], version: &str) -> String {
        let mut map = serde_json::Map::new();
        for (name, data) in files {
            map.insert(
                name.to_string(),
                json!(format!("sha256:{}", sha256_hex(data))),
            );
        }
        serde_json::to_string(&json!({
            "schema": 1,
            "product": "linkslot",
            "version": version,
            "build": "test",
            "files": map,
        }))
        .unwrap()
    }

    fn required_files() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("System/linkSlot/bin/linkslot", b"slot".as_slice()),
            ("System/linkSlot/bin/linkslot_sync", b"link".as_slice()),
            ("System/launch_frontend.sh", b"#!/bin/sh\n".as_slice()),
            ("System/linkSlot/shell/linkslot.sh", b"#!/bin/sh\n".as_slice()),
        ]
    }

    #[test]
    fn the_apply_helper_commits_the_version_manifest() {
        let script = helper_script();
        assert!(script.contains("$STAGE/linkSlot/version.json"));
        assert!(script.contains("$ROOT/linkSlot/version.json.upgrade-tmp"));
        assert!(script.contains("mv -f \"$ROOT/linkSlot/version.json.upgrade-tmp\" \"$ROOT/linkSlot/version.json\""));
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert!(version_is_newer("0.0.36", "0.0.35"));
        assert!(version_is_newer("0.1.0", "0.0.99"));
        assert!(!version_is_newer("0.0.35", "0.0.35"));
        assert!(!version_is_newer("0.0.9", "0.0.10"));
        assert!(!version_is_newer("bad", "0.0.35"));
        assert!(version_is_equal("1.0.0", "1.0"));
        assert!(version_is_equal("1.0.0", "1.0.0"));
        assert!(!version_is_equal("1.0.0", "1.0.1"));
        assert!(!version_is_equal("1.0.0", "bad"));
    }

    #[test]
    fn stages_a_valid_package_and_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let files = [
            ("System/linkSlot/bin/linkslot", b"slot".as_slice()),
            ("System/linkSlot/bin/linkslot_sync", b"link".as_slice()),
            ("System/launch_frontend.sh", b"#!/bin/sh\n".as_slice()),
            ("System/linkSlot/shell/linkslot.sh", b"#!/bin/sh\n".as_slice()),
            ("System/version.json", b"{}\n".as_slice()),
        ];
        let manifest = manifest(&files);
        let mut entries = files.to_vec();
        entries.push(("linkSlot/version.json", manifest.as_bytes()));
        let zip = zip_store(&entries);
        let status = stage(root.path(), &zip).unwrap();
        assert_eq!(status.state, "staged");
        assert_eq!(status.target, "99.0.0");
        assert!(upgrade_dir(root.path()).join("stage/System/linkSlot/bin/linkslot").is_file());

        let bad = zip_store(&[("../evil", b"x".as_slice())]);
        assert!(stage(root.path(), &bad).is_err());
    }

    #[test]
    fn same_version_reinstall_follows_the_development_switch() {
        let root = tempfile::tempdir().unwrap();
        let files = required_files();
        let manifest = manifest_version(&files, slot_ui::PROJECT_VERSION);
        let mut entries = files;
        entries.push(("linkSlot/version.json", manifest.as_bytes()));
        let staged = stage(root.path(), &zip_store(&entries));
        if ALLOW_SAME_VERSION_REINSTALL {
            let status = staged.expect("开发期应允许重装同版本");
            assert_eq!(status.target, slot_ui::PROJECT_VERSION);
            assert!(status.same_version);
        } else {
            assert!(staged.is_err(), "正式版应拒绝同版本");
        }
    }

    #[test]
    fn a_lower_version_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let files = required_files();
        let manifest = manifest_version(&files, "0.0.1");
        let mut entries = files;
        entries.push(("linkSlot/version.json", manifest.as_bytes()));
        let error = stage(root.path(), &zip_store(&entries)).unwrap_err();
        assert!(error.contains("低于"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_old_version_and_missing_manifest() {
        let root = tempfile::tempdir().unwrap();
        let zip = zip_store(&[("System/linkSlot/bin/linkslot", b"slot")]);
        assert!(stage(root.path(), &zip).is_err());
        let mut entries = vec![("System/linkSlot/bin/linkslot", b"slot".as_slice())];
        let manifest = manifest(&[("System/linkSlot/bin/linkslot", b"slot")]);
        entries.push(("linkSlot/version.json", manifest.as_bytes()));
        // Missing required files are rejected even if hashes are right.
        assert!(stage(root.path(), &zip_store(&entries)).is_err());
    }
}
