//! GBAStation Web ROM importer.
//!
//! The mod's management page cannot make cross-origin requests or write
//! `Games/` through the sandboxed mod bridge. This module is the server-side
//! half: it reads GBAStation's `/api/games`, exposes only a metadata list, and
//! downloads the selected ROMs into the card's normal platform folders.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use slot_store::Platform;

/// GBAStation's own `/api/games` response cap. A Switch library is far
/// smaller, but a broken server must not turn into an unbounded allocation.
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;
/// Largest single ROM accepted from GBAStation. GBA is 32 MiB, so this leaves
/// room for a small archive around it.
const MAX_ROM_BYTES: u64 = 64 * 1024 * 1024;
/// Largest decompressed content accepted from one archive.
const MAX_ZIP_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 256;
const MAX_LIST_GAMES: usize = 4096;
const MAX_SELECTED: usize = 256;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Progress shared with the Web page.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub running: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub stage: String,
    pub total: usize,
    pub done: usize,
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub current: String,
}

/// A row safe to send to the mod page. It deliberately has no remote path.
#[derive(Clone, Debug)]
pub struct Game {
    pub key: String,
    pub title: String,
    pub platform: String,
    pub platform_name: String,
    pub file: String,
    pub kind: &'static str,
    pub supported: bool,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rom,
    Zip,
    Unsupported,
}

#[derive(Clone, Debug)]
struct RemoteGame {
    key: String,
    title: String,
    platform: Option<Platform>,
    platform_label: String,
    file: String,
    path: String,
    kind: Kind,
    supported: bool,
    reason: String,
}

/// Normalize the address typed into the mod page. GBAStation serves plain
/// HTTP; only an origin is accepted so a pasted `/index.html` cannot become
/// part of the `/api/...` URL.
pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("请填写 GBAStation Web 地址".to_string());
    }
    let Some(rest) = raw.strip_prefix("http://") else {
        if raw.starts_with("https://") {
            return Err("GBAStation Web 服务使用 http，不支持 https".to_string());
        }
        return Err("地址必须以 http:// 开头".to_string());
    };
    if rest.is_empty() {
        return Err("地址缺少主机名或 IP".to_string());
    }
    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if !tail.is_empty() {
        return Err("只填写 http://IP:端口，不要带路径".to_string());
    }
    if authority.contains('@') || authority.contains('?') || authority.contains('#') {
        return Err("地址不能带用户名、查询或片段".to_string());
    }
    let (host, port) = split_host_port(authority)?;
    if host.is_empty() {
        return Err("地址缺少主机名或 IP".to_string());
    }
    if host.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("主机名包含非法字符".to_string());
    }
    let host = if host.starts_with('[') && host.ends_with(']') {
        host.to_string()
    } else if host.contains(':') {
        return Err("IPv6 地址请使用 [addr]:port".to_string());
    } else {
        host.to_ascii_lowercase()
    };
    match port {
        Some(0) => Err("端口必须在 1..=65535".to_string()),
        Some(port) => Ok(format!("http://{host}:{port}")),
        None => Ok(format!("http://{host}:8080")),
    }
}

fn split_host_port(authority: &str) -> Result<(String, Option<u16>), String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err("IPv6 地址缺少 ]".to_string());
        };
        let host = format!("[{}]", &rest[..end]);
        let after = &rest[end + 1..];
        if after.is_empty() {
            return Ok((host, None));
        }
        let Some(port) = after.strip_prefix(':') else {
            return Err("IPv6 地址后的端口格式不正确".to_string());
        };
        return Ok((host, Some(parse_port(port)?)));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() {
            return Err("地址缺少主机名或 IP".to_string());
        }
        return Ok((host.to_string(), Some(parse_port(port)?)));
    }
    Ok((authority.to_string(), None))
}

fn parse_port(raw: &str) -> Result<u16, String> {
    if raw.is_empty() || !raw.chars().all(|c| c.is_ascii_digit()) {
        return Err("端口必须是数字".to_string());
    }
    raw.parse::<u16>()
        .map_err(|_| "端口必须在 1..=65535".to_string())
}

/// Read GBAStation's game list. No ROM bytes are requested here.
pub fn list(base_url: &str) -> Result<Vec<Game>, String> {
    let base = normalize_base_url(base_url)?;
    let games = fetch_games(&base)?;
    Ok(games
        .into_iter()
        .map(|game| Game {
            key: game.key,
            title: game.title,
            platform: game
                .platform
                .map(|platform| platform.dir_name().to_string())
                .unwrap_or_else(|| game.platform_label.clone()),
            platform_name: game.platform_label,
            file: game.file,
            kind: match game.kind {
                Kind::Rom => "rom",
                Kind::Zip => "zip",
                Kind::Unsupported => "unsupported",
            },
            supported: game.supported,
            reason: game.reason,
        })
        .collect())
}

fn fetch_games(base: &str) -> Result<Vec<RemoteGame>, String> {
    let agent = http_agent(CONNECT_TIMEOUT)?;
    let url = format!("{base}/api/games");
    let body = get_bytes(&agent, &url, MAX_LIST_BYTES)?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("GBAStation /api/games 不是有效 JSON：{error}"))?;
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("GBAStation 返回 ok=false");
        return Err(message.to_string());
    }
    let entries = value
        .get("games")
        .and_then(Value::as_array)
        .ok_or_else(|| "GBAStation /api/games 缺少 games 数组".to_string())?;
    let mut seen = HashSet::new();
    let mut games = Vec::new();
    for entry in entries.iter().take(MAX_LIST_GAMES) {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let key = key_for(path);
        if !seen.insert(key.clone()) {
            continue;
        }
        let file = file_name(path);
        let (platform, platform_label) = platform_of(entry);
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(file.as_str())
            .to_string();
        let kind = kind_of(&file, platform);
        let supported = platform.is_some() && kind != Kind::Unsupported;
        let reason = if platform.is_none() {
            "本机不支持该机种".to_string()
        } else if kind == Kind::Unsupported {
            format!("不支持的 ROM 格式：{file}")
        } else {
            String::new()
        };
        games.push(RemoteGame {
            key,
            title,
            platform,
            platform_label,
            file,
            path: path.to_string(),
            kind,
            supported,
            reason,
        });
    }
    games.sort_by(|left, right| {
        left.platform
            .map(Platform::dir_name)
            .cmp(&right.platform.map(Platform::dir_name))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    Ok(games)
}

fn platform_of(entry: &Value) -> (Option<Platform>, String) {
    let raw = entry.get("platform");
    let numeric = raw
        .and_then(Value::as_u64)
        .or_else(|| raw.and_then(Value::as_str).and_then(|s| s.trim().parse().ok()));
    let platform = match numeric {
        Some(1) => Some(Platform::Gba),
        Some(2) => Some(Platform::Gbc),
        Some(3) => Some(Platform::Gb),
        Some(4) => Some(Platform::Fc),
        Some(5) => Some(Platform::Sfc),
        Some(8) => Some(Platform::Md),
        _ => raw
            .and_then(Value::as_str)
            .map(str::trim)
            .map(|s| s.to_ascii_uppercase())
            .and_then(|name| match name.as_str() {
                "GBA" => Some(Platform::Gba),
                "GBC" => Some(Platform::Gbc),
                "GB" => Some(Platform::Gb),
                "NES" | "FC" | "FAMICOM" => Some(Platform::Fc),
                "SNES" | "SFC" | "SUPER FAMICOM" => Some(Platform::Sfc),
                "GENESIS" | "MD" | "MEGA DRIVE" => Some(Platform::Md),
                _ => None,
            }),
    };
    let label = entry
        .get("platformName")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::trim)
        .unwrap_or_else(|| {
            platform
                .map(Platform::name)
                .unwrap_or("未知机种")
        })
        .to_string();
    (platform, label)
}

fn kind_of(file: &str, platform: Option<Platform>) -> Kind {
    let extension = Path::new(file)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "zip" {
        return Kind::Zip;
    }
    if matches!(extension.as_str(), "7z" | "rar" | "gz" | "xz") {
        return Kind::Unsupported;
    }
    match platform {
        Some(platform) if platform.accepts(Path::new(file)) => Kind::Rom,
        _ => Kind::Unsupported,
    }
}

fn key_for(path: &str) -> String {
    crate::rom_id::sha256_hex(path.as_bytes())
        .chars()
        .take(16)
        .collect()
}

fn file_name(path: &str) -> String {
    let name = path
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(path)
        .trim();
    if name.is_empty() {
        "rom".to_string()
    } else {
        name.to_string()
    }
}

fn safe_file_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "rom".to_string();
    }
    if cleaned.chars().count() > 120 {
        let path = Path::new(&cleaned);
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("rom");
        let room = 120usize.saturating_sub(extension.chars().count() + 1);
        let mut stem_out = String::new();
        for c in stem.chars().take(room) {
            stem_out.push(c);
        }
        cleaned = if extension.is_empty() {
            stem_out
        } else {
            format!("{stem_out}.{extension}")
        };
    }
    cleaned
}

fn with_suffix(name: &str, index: usize) -> String {
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("rom");
    let extension = path.extension().and_then(|s| s.to_str());
    match extension {
        Some(extension) => format!("{stem}-{index}.{extension}"),
        None => format!("{stem}-{index}"),
    }
}

fn http_agent(timeout: Duration) -> Result<ureq::Agent, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build();
    let agent: ureq::Agent = config.into();
    Ok(agent)
}

fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| format!("请求 {url} 失败：{error}"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let detail = response
            .body_mut()
            .with_config()
            .limit(4096)
            .read_to_vec()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default();
        return Err(if detail.is_empty() {
            format!("GBAStation HTTP {status}")
        } else {
            format!("GBAStation HTTP {status}: {detail}")
        });
    }
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| format!("读取 {url} 响应失败：{error}"))
}

fn download_url(base: &str, path: &str) -> String {
    format!(
        "{base}/api/files/download?path={}",
        utf8_percent_encode(path, NON_ALPHANUMERIC)
    )
}

/// Run one import job. The caller owns `status.running`/`shared.busy` and
/// always releases them after this returns.
pub fn run(
    root: PathBuf,
    base_url: String,
    keys: Vec<String>,
    overwrite: bool,
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
) {
    let result = run_inner(&root, &base_url, &keys, overwrite, &status, &cancel);
    if let Ok(mut state) = status.lock() {
        state.running = false;
        state.finished = true;
        if state.cancelled {
            state.stage = "已取消".to_string();
        } else if let Err(error) = result {
            state.stage = "导入失败".to_string();
            state.errors.push(error);
        } else if state.errors.is_empty() {
            state.stage = "导入完成".to_string();
        } else {
            state.stage = "导入完成（部分失败）".to_string();
        }
    }
}

fn run_inner(
    root: &Path,
    base_url: &str,
    keys: &[String],
    overwrite: bool,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    if keys.is_empty() {
        return Err("没有选择要同步的游戏".to_string());
    }
    if keys.len() > MAX_SELECTED {
        return Err(format!("一次最多同步 {MAX_SELECTED} 个游戏"));
    }
    let base = normalize_base_url(base_url)?;
    set_stage(status, "读取 GBAStation 游戏列表");
    let games = fetch_games(&base)?;
    let by_key: HashMap<&str, &RemoteGame> =
        games.iter().map(|game| (game.key.as_str(), game)).collect();
    let mut selected = Vec::new();
    for key in keys {
        let Some(game) = by_key.get(key.as_str()).copied() else {
            return Err(format!("GBAStation 中已找不到所选游戏（{key}）"));
        };
        if !game.supported {
            return Err(format!("「{}」不支持导入：{}", game.title, game.reason));
        }
        selected.push(game.clone());
    }
    if let Ok(mut state) = status.lock() {
        state.total = selected.len();
        state.done = 0;
        state.imported = 0;
        state.skipped = 0;
        state.errors.clear();
        state.stage = "准备下载".to_string();
    }
    let agent = http_agent(DOWNLOAD_TIMEOUT)?;
    let mut created = HashSet::new();
    for game in selected {
        if cancel.load(Ordering::Relaxed) {
            if let Ok(mut state) = status.lock() {
                state.cancelled = true;
            }
            return Ok(());
        }
        if let Ok(mut state) = status.lock() {
            state.current = game.title.clone();
            state.stage = format!("下载 {}", game.title);
        }
        let result = (|| -> Result<(usize, usize), String> {
            let platform = game
                .platform
                .ok_or_else(|| format!("「{}」没有可导入的机种", game.title))?;
            let url = download_url(&base, &game.path);
            let bytes = get_bytes(&agent, &url, MAX_ROM_BYTES)?;
            let games_dir = root.join("Games").join(platform.dir_name());
            fs::create_dir_all(&games_dir)
                .map_err(|error| format!("创建 {} 失败：{error}", games_dir.display()))?;
            match game.kind {
                Kind::Rom => import_rom(
                    &games_dir,
                    platform,
                    &game.file,
                    &bytes,
                    overwrite,
                    &mut created,
                ),
                Kind::Zip => import_zip(
                    &games_dir,
                    platform,
                    &bytes,
                    overwrite,
                    &mut created,
                ),
                Kind::Unsupported => Err(game.reason.clone()),
            }
        })();
        if let Ok(mut state) = status.lock() {
            match result {
                Ok((imported, skipped)) => {
                    state.imported += imported;
                    state.skipped += skipped;
                }
                Err(error) => state.errors.push(format!("{}: {error}", game.title)),
            }
            state.done += 1;
        }
    }
    Ok(())
}

/// Returns `(imported, skipped)` for one downloaded ROM.
fn import_rom(
    games_dir: &Path,
    platform: Platform,
    name: &str,
    bytes: &[u8],
    overwrite: bool,
    created: &mut HashSet<PathBuf>,
) -> Result<(usize, usize), String> {
    let name = safe_file_name(name);
    if !platform.accepts(Path::new(&name)) {
        return Err(format!("扩展名与 {} 不匹配：{name}", platform.dir_name()));
    }
    write_rom(games_dir, &name, bytes, overwrite, created)
}

/// Extract a GBAStation `.zip` and import every ROM that belongs to the
/// selected platform. Nested archives and other platforms are skipped.
fn import_zip(
    games_dir: &Path,
    platform: Platform,
    bytes: &[u8],
    overwrite: bool,
    created: &mut HashSet<PathBuf>,
) -> Result<(usize, usize), String> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|error| format!("不是有效 ZIP：{error}"))?;
    let mut imported = 0;
    let mut skipped = 0;
    let mut total = 0u64;
    let mut errors = Vec::new();
    for index in 0..archive.len().min(MAX_ZIP_ENTRIES) {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else {
            errors.push("压缩包内含越界路径，已跳过".to_string());
            continue;
        };
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(safe_file_name)
            .unwrap_or_default();
        if name.is_empty() || !platform.accepts(Path::new(&name)) {
            skipped += 1;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_ZIP_BYTES {
            return Err(format!("压缩包解压后超过 {} MiB 上限", MAX_ZIP_BYTES / 1024 / 1024));
        }
        let mut data = Vec::with_capacity(entry.size().min(MAX_ROM_BYTES) as usize);
        entry
            .by_ref()
            .take(MAX_ROM_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|error| format!("读取压缩包内 {name} 失败：{error}"))?;
        if data.len() as u64 > MAX_ROM_BYTES {
            return Err(format!("压缩包内 {name} 超过 {} MiB 上限", MAX_ROM_BYTES / 1024 / 1024));
        }
        match write_rom(games_dir, &name, &data, overwrite, created) {
            Ok((1, 0)) => imported += 1,
            Ok((_, skipped_count)) => skipped += skipped_count,
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    if imported == 0 && skipped == 0 {
        return Err(if errors.is_empty() {
            format!("压缩包内没有可导入的 {} ROM", platform.dir_name())
        } else {
            errors.join("; ")
        });
    }
    Ok((imported, skipped))
}

fn write_rom(
    games_dir: &Path,
    name: &str,
    bytes: &[u8],
    overwrite: bool,
    created: &mut HashSet<PathBuf>,
) -> Result<(usize, usize), String> {
    let mut target = games_dir.join(name);
    if target.exists() {
        if !overwrite {
            if created.contains(&target) {
                let mut index = 2;
                loop {
                    let candidate = games_dir.join(with_suffix(name, index));
                    if !candidate.exists() {
                        target = candidate;
                        break;
                    }
                    index += 1;
                }
            } else {
                return Ok((0, 1));
            }
        } else {
            // POSIX rename replaces atomically. Windows rejects that rename,
            // so the host test build removes the old name first.
            #[cfg(windows)]
            fs::remove_file(&target)
                .map_err(|error| format!("覆盖 {} 失败：{error}", target.display()))?;
        }
    }
    let existing = fs::read(&target).ok();
    if existing.as_deref() == Some(bytes) {
        return Ok((0, 1));
    }
    slot_store::atomic_write(&target, bytes)
        .map_err(|error| format!("写入 {} 失败：{error}", target.display()))?;
    created.insert(target);
    Ok((1, 0))
}

fn set_stage(status: &Arc<Mutex<Status>>, stage: impl Into<String>) {
    if let Ok(mut state) = status.lock() {
        state.stage = stage.into();
    }
}

/// JSON shape used by the Web page.
pub fn list_json(games: &[Game]) -> Value {
    Value::Array(
        games
            .iter()
            .map(|game| {
                json!({
                    "key": game.key,
                    "title": game.title,
                    "platform": game.platform,
                    "platformName": game.platform_name,
                    "file": game.file,
                    "kind": game.kind,
                    "supported": game.supported,
                    "reason": game.reason,
                })
            })
            .collect(),
    )
}

pub fn status_json(status: &Status) -> Value {
    json!({
        "running": status.running,
        "finished": status.finished,
        "cancelled": status.cancelled,
        "stage": status.stage,
        "total": status.total,
        "done": status.done,
        "imported": status.imported,
        "skipped": status.skipped,
        "errors": status.errors,
        "current": status.current,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn station(
        games: Value,
        files: HashMap<String, Vec<u8>>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&log);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let mut buf = [0u8; 4096];
                let Ok(read) = stream.read(&mut buf) else {
                    continue;
                };
                let text = String::from_utf8_lossy(&buf[..read]);
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                seen.lock().unwrap().push(path.clone());
                let (content_type, body, status) = if path == "/api/games" {
                    ("application/json", games.to_string().into_bytes(), "200 OK")
                } else if let Some(query) = path.strip_prefix("/api/files/download?path=") {
                    let decoded = percent_encoding::percent_decode_str(query)
                        .decode_utf8_lossy()
                        .into_owned();
                    match files.get(&decoded) {
                        Some(bytes) => ("application/octet-stream", bytes.clone(), "200 OK"),
                        None => ("text/plain", b"not found".to_vec(), "404 Not Found"),
                    }
                } else {
                    ("text/plain", b"not found".to_vec(), "404 Not Found")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (format!("http://127.0.0.1:{port}"), log)
    }

    fn game_entry(path: &str, platform: u64, title: &str) -> Value {
        json!({"path": path, "platform": platform, "title": title, "platformName": "GBA"})
    }

    #[test]
    fn normalizes_origins_and_rejects_paths() {
        assert_eq!(
            normalize_base_url(" http://192.168.1.23:8080/ ").unwrap(),
            "http://192.168.1.23:8080"
        );
        assert_eq!(
            normalize_base_url("http://gbastation.local").unwrap(),
            "http://gbastation.local:8080"
        );
        assert!(normalize_base_url("https://192.168.1.23:8080").is_err());
        assert!(normalize_base_url("http://192.168.1.23:8080/api").is_err());
        assert!(normalize_base_url("http://user:pass@192.168.1.23:8080").is_err());
        assert!(normalize_base_url("http://192.168.1.23:0").is_err());
    }

    #[test]
    fn maps_gbastation_platforms_and_labels() {
        assert_eq!(
            platform_of(&json!({"platform": 1})).0,
            Some(Platform::Gba)
        );
        assert_eq!(
            platform_of(&json!({"platform": 2})).0,
            Some(Platform::Gbc)
        );
        assert_eq!(
            platform_of(&json!({"platform": "Genesis"})).0,
            Some(Platform::Md)
        );
        let unknown = platform_of(&json!({"platform": 6, "platformName": "NDS"}));
        assert_eq!(unknown.0, None);
        assert_eq!(unknown.1, "NDS");
    }

    #[test]
    fn cleans_names_and_keeps_the_extension() {
        assert_eq!(safe_file_name("../Sonic.gba"), ".._Sonic.gba");
        assert_eq!(safe_file_name("a/b\\c.gba"), "a_b_c.gba");
        assert_eq!(safe_file_name("  ...  "), "rom");
        let long = format!("{}.gba", "x".repeat(200));
        let clean = safe_file_name(&long);
        assert!(clean.ends_with(".gba"));
        assert_eq!(clean.chars().count(), 120);
    }

    #[test]
    fn list_does_not_download_and_import_writes_the_selected_rom() {
        let (base, log) = station(
            json!({"ok": true, "games": [
                game_entry("sdmc:/roms/GBA/Sonic.gba", 1, "Sonic"),
                game_entry("sdmc:/roms/GBA/Mario.gba", 1, "Mario"),
            ]}),
            HashMap::from([("sdmc:/roms/GBA/Mario.gba".to_string(), b"rom-bytes".to_vec())]),
        );
        let games = list(&base).unwrap();
        assert_eq!(games.len(), 2);
        assert!(games.iter().all(|game| game.supported));
        assert!(
            !log.lock()
                .unwrap()
                .iter()
                .any(|path| path.starts_with("/api/files/")),
            "listing a game downloaded a ROM"
        );

        let key = games
            .iter()
            .find(|game| game.title == "Mario")
            .unwrap()
            .key
            .clone();
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("Games/GBA")).unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            base,
            vec![key],
            false,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(
            fs::read(root.path().join("Games/GBA/Mario.gba")).unwrap(),
            b"rom-bytes"
        );
        let state = status.lock().unwrap();
        assert!(state.finished);
        assert_eq!(state.imported, 1);
        assert_eq!(state.total, 1);
        assert!(state.errors.is_empty(), "{:?}", state.errors);
    }

    #[test]
    fn zip_is_unpacked_and_unsupported_entries_are_filtered() {
        let mut zip_bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut zip_bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("Sonic.gba", options).unwrap();
            writer.write_all(b"gba-inside-zip").unwrap();
            writer.start_file("readme.txt", options).unwrap();
            writer.write_all(b"ignore").unwrap();
            writer.finish().unwrap();
        }
        let (base, _) = station(
            json!({"ok": true, "games": [
                game_entry("sdmc:/roms/GBA/Sonic.zip", 1, "Sonic"),
            ]}),
            HashMap::from([("sdmc:/roms/GBA/Sonic.zip".to_string(), zip_bytes)]),
        );
        let games = list(&base).unwrap();
        assert_eq!(games[0].kind, "zip");
        let root = tempdir().unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            base,
            vec![games[0].key.clone()],
            false,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(
            fs::read(root.path().join("Games/GBA/Sonic.gba")).unwrap(),
            b"gba-inside-zip"
        );
        let state = status.lock().unwrap();
        assert_eq!(state.imported, 1);
        assert_eq!(state.skipped, 1);
        assert!(state.errors.is_empty(), "{:?}", state.errors);
    }

    #[test]
    fn unsupported_platforms_are_listed_but_not_imported() {
        let (base, _) = station(
            json!({"ok": true, "games": [
                {"path": "sdmc:/roms/NDS/Game.nds", "platform": 6, "title": "Game", "platformName": "NDS"},
            ]}),
            HashMap::new(),
        );
        let games = list(&base).unwrap();
        assert_eq!(games.len(), 1);
        assert!(!games[0].supported);
        assert_eq!(games[0].platform, "NDS");
        let root = tempdir().unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            base,
            vec![games[0].key.clone()],
            false,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        let state = status.lock().unwrap();
        assert!(state.finished);
        assert_eq!(state.imported, 0);
        assert!(!state.errors.is_empty());
    }

    #[test]
    fn existing_files_are_skipped_or_overwritten_explicitly() {
        let (base, _) = station(
            json!({"ok": true, "games": [
                game_entry("sdmc:/roms/GBA/Sonic.gba", 1, "Sonic"),
            ]}),
            HashMap::from([("sdmc:/roms/GBA/Sonic.gba".to_string(), b"new".to_vec())]),
        );
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("Games/GBA")).unwrap();
        fs::write(root.path().join("Games/GBA/Sonic.gba"), b"old").unwrap();
        let games = list(&base).unwrap();
        let key = games[0].key.clone();
        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            base.clone(),
            vec![key.clone()],
            false,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(
            fs::read(root.path().join("Games/GBA/Sonic.gba")).unwrap(),
            b"old"
        );
        assert_eq!(status.lock().unwrap().skipped, 1);

        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            base,
            vec![key],
            true,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(
            fs::read(root.path().join("Games/GBA/Sonic.gba")).unwrap(),
            b"new"
        );
        assert_eq!(status.lock().unwrap().imported, 1);
    }

    #[test]
    fn offline_station_reports_an_error_and_writes_nothing() {
        let root = tempdir().unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        run(
            root.path().to_path_buf(),
            "http://127.0.0.1:1".to_string(),
            vec!["missing".to_string()],
            false,
            Arc::clone(&status),
            Arc::new(AtomicBool::new(false)),
        );
        let state = status.lock().unwrap();
        assert!(state.finished);
        assert!(!state.errors.is_empty());
        assert_eq!(state.imported, 0);
    }
}
