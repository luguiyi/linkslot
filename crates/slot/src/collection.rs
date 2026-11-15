//! Web game-library collection export/import.
//!
//! A collection is a ZIP with a small manifest plus the unmodified ROMs and
//! matching `Labels/<platform>/` sidecars.  Export streams directly to disk
//! and the Web layer downloads the resulting file; import accepts that same
//! package without touching saves or other platforms.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use slot_store::cart_meta::{self, CartMeta};
use slot_store::Platform;

pub const MANIFEST: &str = "linkSlot-collection.json";
pub const UPLOAD_TEMP: &str = "Imports/.collection-upload.zip";

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub running: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub kind: String,
    pub platform: String,
    pub stage: String,
    pub total: usize,
    pub done: usize,
    pub imported: usize,
    pub skipped: usize,
    pub duplicates: usize,
    pub verified: usize,
    pub bytes: u64,
    pub file: Option<String>,
    pub errors: Vec<String>,
}

pub struct Upload {
    pub path: PathBuf,
    pub offset: u64,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    platform: String,
    created: u64,
    games: Vec<String>,
    /// Full SHA-256 per `Games/...` ZIP entry (manifest v2).  Older packages
    /// omit it; their hashes are still computed from the actual ROM bytes.
    #[serde(default)]
    hashes: HashMap<String, String>,
    /// Present for a single-game package.  Platform export leaves it out.
    #[serde(default)]
    metadata: Option<CartMeta>,
}

pub fn platform(raw: &str) -> Option<Platform> {
    Platform::ALL
        .into_iter()
        .find(|platform| platform.dir_name().eq_ignore_ascii_case(raw.trim()))
}

pub fn export_platform(
    root: &Path,
    platform: Platform,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let games_dir = root.join("Games").join(platform.dir_name());
    if !games_dir.is_dir() {
        return Err(format!("没有 Games/{} 目录", platform.dir_name()));
    }
    let mut roms = Vec::new();
    walk(&games_dir, platform, &mut roms);
    roms.sort();
    if roms.is_empty() {
        return Err(format!("Games/{} 里没有 ROM", platform.dir_name()));
    }


    // ZIP entry name, source path.  The manifest goes first.
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut games = Vec::new();
    let mut seen_labels = HashSet::new();
    for rom in &roms {
        let Ok(relative) = rom.strip_prefix(&games_dir) else {
            continue;
        };
        let rel = relative.to_string_lossy().replace('\\', "/");
        games.push(rel.clone());
        files.push((
            format!("Games/{}/{rel}", platform.dir_name()),
            rom.clone(),
        ));
        let Some(stem) = rom.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        for extension in ["png", "txt", "mp4"] {
            let dir = root.join(cart_meta::asset_dir(extension)).join(platform.dir_name());
            let asset = dir.join(format!("{stem}.{extension}"));
            if asset.is_file() && seen_labels.insert(asset.clone()) {
                files.push((
                    format!(
                        "{}/{}/{stem}.{extension}",
                        cart_meta::asset_dir(extension),
                        platform.dir_name()
                    ),
                    asset,
                ));
            }
        }
    }

    // Front faces are part of a game's presentation, not user progress.  Carry
    // the whole platform front folder so selected variants and their editable
    // projects survive an export/import round trip.
    let fronts_dir = root.join("linkSlot/Fronts").join(platform.dir_name());
    let mut front_files = Vec::new();
    walk_files(&fronts_dir, &mut front_files);
    for path in front_files {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "png" | "json") {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            files.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    if let Ok(mut status) = status.lock() {
        status.stage = "计算 ROM SHA-256".to_string();
    }
    let mut hashes = HashMap::new();
    for rom in &roms {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".to_string());
        }
        let Ok(relative) = rom.strip_prefix(&games_dir) else {
            continue;
        };
        let rel = relative.to_string_lossy().replace('\\', "/");
        let digest =
            crate::rom_id::sha256_file(rom).map_err(|error| format!("计算 SHA-256 失败：{error}"))?;
        hashes.insert(format!("Games/{}/{rel}", platform.dir_name()), digest);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let manifest = Manifest {
        format: "linkslot-collection".to_string(),
        version: 2,
        platform: platform.dir_name().to_string(),
        created: stamp,
        games,
        hashes,
        metadata: None,
    };
    let manifest = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("生成合集清单失败：{error}"))?;
    let exports = root.join("Exports");
    fs::create_dir_all(&exports).map_err(|error| format!("创建 Exports 失败：{error}"))?;
    let target = exports.join(format!(
        "linkslot-{}-{stamp}.zip",
        platform.dir_name().to_ascii_lowercase()
    ));
    let temp = target.with_extension("zip.part");
    if let Ok(mut status) = status.lock() {
        status.total = files.len() + 1;
        status.done = 0;
        status.bytes = 0;
        status.stage = "导出 ROM 与游戏信息".to_string();
    }

    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&temp).map_err(|error| format!("创建合集失败：{error}"))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(MANIFEST, options)
            .map_err(|error| format!("写入合集清单失败：{error}"))?;
        zip.write_all(&manifest)
            .map_err(|error| format!("写入合集清单失败：{error}"))?;
        bump(status, |status| status.done += 1);
        for (name, path) in &files {
            if cancel.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&temp);
                return Err("已取消".to_string());
            }
            let mut source =
                fs::File::open(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
            zip.start_file(name.clone(), options)
                .map_err(|error| format!("写入 {name} 失败：{error}"))?;
            let size = std::io::copy(&mut source, &mut zip)
                .map_err(|error| format!("复制 {} 失败：{error}", path.display()))?;
            bump(status, |status| {
                status.done += 1;
                status.bytes += size;
            });
        }
        zip.finish()
            .map_err(|error| format!("完成合集失败：{error}"))?;
        fs::rename(&temp, &target).map_err(|error| format!("保存合集失败：{error}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let relative = target
        .strip_prefix(root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if let Ok(mut status) = status.lock() {
        status.file = Some(relative);
    }
    Ok(target)
}

fn walk(dir: &Path, platform: Platform, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, platform, out);
        } else if platform.accepts(&path) {
            out.push(path);
        }
    }
}

/// Export one ROM together with its front faces, cover, intro, video, guide,
/// cheats and metadata.  Saves and save states stay on the card: a game pack is
/// a way to move the game, not a backup of someone's progress.
///
/// The ZIP uses the same collection manifest as a platform export, so the
/// existing import UI can accept either one.
pub fn export_game(
    root: &Path,
    rom: &str,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let rom_rel = rom.trim().replace('\\', "/");
    let rom_path = Path::new(&rom_rel);
    if rom_rel.is_empty()
        || rom_path.is_absolute()
        || !rom_rel.starts_with("Games/")
        || rom_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("ROM 路径无效".to_string());
    }
    let carts = slot_store::scan(root).map_err(|error| format!("扫描 ROM 失败：{error}"))?;
    let cart = carts
        .into_iter()
        .find(|cart| cart_meta::relative(root, &cart.rom) == rom_rel)
        .ok_or_else(|| "找不到要导出的 ROM".to_string())?;
    let platform = cart.platform;
    if let Ok(mut status) = status.lock() {
        status.platform = platform.dir_name().to_string();
        status.stage = "导出单个游戏".to_string();
    }
    let mut metadata = cart_meta::read(root, platform, &cart.rom, &cart.stem)
        .unwrap_or_else(|| CartMeta::from_cart(root, &cart));
    metadata.platform = platform.dir_name().to_string();
    metadata.rom = rom_rel.clone();

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();
    add_pack_file(
        &mut files,
        &mut seen,
        rom_rel.clone(),
        cart.rom.clone(),
    );

    for (extension, slot) in [
        ("png", &mut metadata.cover),
        ("txt", &mut metadata.intro),
        ("mp4", &mut metadata.video),
    ] {
        let source = match extension {
            "png" => cart_meta::cover_path(root, &cart),
            "txt" => cart_meta::intro_path(root, &cart),
            _ => cart_meta::video_path(root, &cart),
        };
        let Some(source) = source else {
            *slot = None;
            continue;
        };
        let dir = root
            .join(cart_meta::asset_dir(extension))
            .join(platform.dir_name());
        let name = if let Ok(relative) = source.strip_prefix(&dir) {
            let mut name = format!("{}/{}", cart_meta::asset_dir(extension), platform.dir_name());
            name.push('/');
            name.push_str(&relative.to_string_lossy().replace('\\', "/"));
            name
        } else {
            format!(
                "{}/{}/{}.{}",
                cart_meta::asset_dir(extension),
                platform.dir_name(),
                cart.stem,
                extension
            )
        };
        add_pack_file(&mut files, &mut seen, name.clone(), source);
        *slot = Some(name);
    }

    let guides = root.join("Guides").join(platform.dir_name());
    if let Some(path) = cart_meta::guide_path(root, &cart) {
        let name = if let Ok(relative) = path.strip_prefix(&guides) {
            let mut name = format!("Guides/{}/", platform.dir_name());
            name.push_str(&relative.to_string_lossy().replace('\\', "/"));
            name
        } else {
            format!(
                "Guides/{}/{}.{}",
                platform.dir_name(),
                cart.stem,
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("txt")
            )
        };
        add_pack_file(&mut files, &mut seen, name.clone(), path);
        metadata.guide = Some(name);
    } else if metadata.guide.is_some() {
        metadata.guide = None;
    }

    let cheats = root.join("Cheats").join(platform.dir_name());
    let mut cheat_files = Vec::new();
    walk_files(&cheats, &mut cheat_files);
    for path in cheat_files {
        let is_cheat = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cht"));
        if !is_cheat
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(cart.stem.as_str())
        {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            add_pack_file(
                &mut files,
                &mut seen,
                relative.to_string_lossy().replace('\\', "/"),
                path,
            );
        }
    }
    if let Some(path) = metadata.cheat_path(root) {
        let name = if let Ok(relative) = path.strip_prefix(&cheats) {
            let mut name = format!("Cheats/{}/", platform.dir_name());
            name.push_str(&relative.to_string_lossy().replace('\\', "/"));
            name
        } else {
            format!(
                "Cheats/{}/custom/{}.cht",
                platform.dir_name(),
                cart.stem
            )
        };
        add_pack_file(&mut files, &mut seen, name.clone(), path);
        metadata.cheat = Some(name);
    } else if metadata.cheat.is_some() {
        metadata.cheat = None;
    }

    // A front face travels with the game.  Include both the flattened PNG and
    // the optional editable project, including the current selection stored in
    // `metadata.front_face`.
    let fronts = slot_store::fronts::face_dir(root, platform, &cart.stem);
    let mut front_files = Vec::new();
    walk_files(&fronts, &mut front_files);
    for path in front_files {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "png" | "json") {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            add_pack_file(
                &mut files,
                &mut seen,
                relative.to_string_lossy().replace('\\', "/"),
                path,
            );
        }
    }

    files.sort_by(|left, right| left.0.cmp(&right.0));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let digest = crate::rom_id::sha256_file(&cart.rom)
        .map_err(|error| format!("计算 ROM SHA-256 失败：{error}"))?;
    metadata.sha256 = Some(digest.clone());
    let mut hashes = HashMap::new();
    hashes.insert(rom_rel.clone(), digest);
    let manifest = Manifest {
        format: "linkslot-collection".to_string(),
        version: 2,
        platform: platform.dir_name().to_string(),
        created: stamp,
        games: vec![rom_rel],
        hashes,
        metadata: Some(metadata),
    };
    let manifest = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("生成游戏清单失败：{error}"))?;
    let exports = root.join("Exports");
    fs::create_dir_all(&exports).map_err(|error| format!("创建 Exports 失败：{error}"))?;
    let target = exports.join(format!("{}.zip", safe_file_name(&cart.stem)));
    let temp = target.with_extension("zip.part");
    if let Ok(mut status) = status.lock() {
        status.total = files.len() + 1;
        status.done = 0;
        status.bytes = 0;
    }
    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&temp).map_err(|error| format!("创建游戏包失败：{error}"))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(MANIFEST, options)
            .map_err(|error| format!("写入游戏清单失败：{error}"))?;
        zip.write_all(&manifest)
            .map_err(|error| format!("写入游戏清单失败：{error}"))?;
        bump(status, |status| status.done += 1);
        for (name, path) in &files {
            if cancel.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&temp);
                return Err("已取消".to_string());
            }
            let mut source =
                fs::File::open(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
            zip.start_file(name.clone(), options)
                .map_err(|error| format!("写入 {name} 失败：{error}"))?;
            let size = std::io::copy(&mut source, &mut zip)
                .map_err(|error| format!("复制 {} 失败：{error}", path.display()))?;
            bump(status, |status| {
                status.done += 1;
                status.bytes += size;
            });
        }
        zip.finish()
            .map_err(|error| format!("完成游戏包失败：{error}"))?;
        fs::rename(&temp, &target).map_err(|error| format!("保存游戏包失败：{error}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let relative = target
        .strip_prefix(root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if let Ok(mut status) = status.lock() {
        status.file = Some(relative);
    }
    Ok(target)
}

fn add_pack_file(
    files: &mut Vec<(String, PathBuf)>,
    seen: &mut HashSet<String>,
    name: String,
    source: PathBuf,
) {
    if source.is_file() && seen.insert(name.clone()) {
        files.push((name, source));
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            walk_files(&path, out);
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

/// A file name that keeps the game's own name, including Chinese, and only
/// replaces what a FAT card cannot store.
fn safe_file_name(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            ch if (ch as u32) < 0x20 => out.push('_'),
            _ => out.push(ch),
        }
    }
    let trimmed = out.trim().trim_matches('.').trim();
    let trimmed = if trimmed.chars().count() > 60 {
        trimmed.chars().take(60).collect::<String>()
    } else {
        trimmed.to_string()
    };
    if trimmed.is_empty() {
        "game".to_string()
    } else {
        trimmed
    }
}

fn entry_sha256(
    archive: &mut zip::ZipArchive<fs::File>,
    index: usize,
    name: &str,
) -> Result<(String, u64), String> {
    let entry = archive
        .by_index(index)
        .map_err(|error| format!("读取 {name} 失败：{error}"))?;
    let size = entry.size();
    let digest = crate::rom_id::sha256_reader(entry)
        .map_err(|error| format!("计算 {name} SHA-256 失败：{error}"))?;
    Ok((digest, size))
}

/// Find a local ROM with this content.  The normal path is one indexed row;
/// the fallback hashes only same-size rows whose background SHA is not ready.
fn duplicate_content(
    root: &Path,
    platform: Platform,
    digest: &str,
    size: u64,
) -> Result<Option<CartMeta>, String> {
    if let Some(meta) = cart_meta::read_by_sha(root, platform, digest) {
        if cart_meta::stamp_matches(root, &meta) {
            return Ok(Some(meta));
        }
        let path = root.join(&meta.rom);
        if let Ok(actual) = crate::rom_id::sha256_file(&path) {
            if let (Some(size), Some(mtime)) = (meta.size, meta.mtime) {
                let _ = cart_meta::set_sha256(root, &path, &actual, size, mtime);
            }
            if actual.eq_ignore_ascii_case(digest) {
                return Ok(Some(meta));
            }
        }
    }
    for candidate in cart_meta::candidates_by_size(root, platform, size) {
        let path = root.join(&candidate.rom);
        let Ok(actual) = crate::rom_id::sha256_file(&path) else {
            continue;
        };
        if let (Some(candidate_size), Some(candidate_mtime)) = (candidate.size, candidate.mtime) {
            let _ = cart_meta::set_sha256(
                root,
                &path,
                &actual,
                candidate_size,
                candidate_mtime,
            );
        }
        if actual.eq_ignore_ascii_case(digest) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn entry_stem(relative: &Path) -> Option<String> {
    relative
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn mentions_stem(relative: &Path, stems: &HashSet<String>) -> bool {
    relative.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        let part = part.as_ref();
        stems.contains(part)
            || Path::new(part)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stems.contains(stem))
    })
}

pub fn import_zip(
    root: &Path,
    zip_path: &Path,
    overwrite: bool,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|error| format!("打开合集失败：{error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("合集不是有效 ZIP：{error}"))?;
    let manifest: Manifest = {
        let mut entry = archive
            .by_name(MANIFEST)
            .map_err(|_| format!("合集缺少 {MANIFEST}"))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("读取合集清单失败：{error}"))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("合集清单损坏：{error}"))?
    };
    if manifest.format != "linkslot-collection" || !matches!(manifest.version, 1 | 2) {
        return Err("不支持的合集格式".to_string());
    }
    let platform = platform(&manifest.platform).ok_or_else(|| "合集平台无效".to_string())?;
    if let Ok(mut status) = status.lock() {
        status.platform = platform.dir_name().to_string();
        status.stage = "解压合集".to_string();
    }

    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Some(relative) = allowed_entry(entry.name(), platform) else {
            continue;
        };
        let full_name = entry.name().replace('\\', "/");
        entries.push((index, relative, full_name));
    }

    // Hash every ROM before touching the card.  A duplicate is identified by
    // actual bytes, not by the path or by trusting the package manifest.
    let mut duplicate_roms = HashSet::new();
    let mut duplicate_stems = HashSet::new();
    let mut written_hashes: HashMap<String, (String, u64)> = HashMap::new();
    let mut seen_hashes = HashSet::new();
    let mut verified = 0usize;
    for (index, relative, name) in &entries {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".to_string());
        }
        if !relative.starts_with("Games/") {
            continue;
        }
        let (digest, size) = entry_sha256(&mut archive, *index, name)?;
        if let Some(expected) = manifest.hashes.get(name) {
            if !crate::rom_id::is_sha256(expected) || !digest.eq_ignore_ascii_case(expected) {
                return Err(format!("ROM SHA-256 校验失败：{name}"));
            }
            verified += 1;
        }
        let already_seen = !seen_hashes.insert(digest.clone());
        let existing = if already_seen {
            true
        } else {
            duplicate_content(root, platform, &digest, size)?.is_some()
        };
        if existing {
            duplicate_roms.insert(relative.clone());
            if let Some(stem) = entry_stem(relative) {
                duplicate_stems.insert(stem);
            }
        } else {
            written_hashes.insert(relative.to_string_lossy().replace('\\', "/"), (digest, size));
        }
    }

    if let Ok(mut status) = status.lock() {
        status.total = entries.len();
        status.done = 0;
        status.bytes = 0;
        status.duplicates = duplicate_roms.len();
        status.verified = verified;
    }
    for (index, relative, name) in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".to_string());
        }
        if duplicate_roms.contains(&relative)
            || (!relative.starts_with("Games/") && mentions_stem(&relative, &duplicate_stems))
        {
            bump(status, |status| {
                status.done += 1;
                status.skipped += 1;
            });
            continue;
        }
        let target = root.join(&relative);
        let target = crate::web::roms::checked_target(root, &target).map_err(|response| {
            format!(
                "目标路径无效：{}",
                String::from_utf8_lossy(&response.body)
            )
        })?;
        if target.exists() && !overwrite {
            written_hashes.remove(&relative.to_string_lossy().replace('\\', "/"));
            bump(status, |status| {
                status.done += 1;
                status.skipped += 1;
            });
            continue;
        }
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 {name} 失败：{error}"))?;
        let temp = target.with_extension(format!(
            "{}.part",
            target.extension().and_then(|ext| ext.to_str()).unwrap_or("tmp")
        ));
        let result = (|| -> Result<u64, String> {
            let mut output =
                fs::File::create(&temp).map_err(|error| format!("创建 {name} 失败：{error}"))?;
            let size = std::io::copy(&mut entry, &mut output)
                .map_err(|error| format!("解压 {name} 失败：{error}"))?;
            output
                .sync_all()
                .map_err(|error| format!("同步 {name} 失败：{error}"))?;
            if target.exists() {
                fs::remove_file(&target)
                    .map_err(|error| format!("替换 {name} 失败：{error}"))?;
            }
            fs::rename(&temp, &target).map_err(|error| format!("保存 {name} 失败：{error}"))?;
            Ok(size)
        })();
        match result {
            Ok(size) => bump(status, |status| {
                status.done += 1;
                status.imported += 1;
                status.bytes += size;
            }),
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        }
    }

    if let Some(mut metadata) = manifest.metadata {
        let duplicate = entry_stem(Path::new(&metadata.rom))
            .is_some_and(|stem| duplicate_stems.contains(&stem));
        if duplicate {
            bump(status, |status| status.skipped += 1);
        } else {
            let rom = Path::new(&metadata.rom);
            let platform_prefix = format!("Games/{}/", platform.dir_name());
            if !metadata.rom.starts_with(&platform_prefix)
                || rom.is_absolute()
                || rom
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err("游戏元数据里的 ROM 路径无效".to_string());
            }
            let target = root.join(&metadata.rom);
            let target = crate::web::roms::checked_target(root, &target).map_err(|response| {
                format!(
                    "游戏元数据路径无效：{}",
                    String::from_utf8_lossy(&response.body)
                )
            })?;
            if !target.is_file() {
                return Err("游戏包缺少元数据对应的 ROM".to_string());
            }
            let digest = crate::rom_id::sha256_file(&target)
                .map_err(|error| format!("计算 ROM SHA-256 失败：{error}"))?;
            if let Some(expected) = metadata.sha256.as_deref() {
                if !digest.eq_ignore_ascii_case(expected) {
                    return Err("游戏元数据里的 ROM SHA-256 不匹配".to_string());
                }
            }
            metadata.sha256 = Some(digest);
            if let Ok(file) = fs::metadata(&target) {
                metadata.size = Some(file.len());
                metadata.mtime = file
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_nanos() as u64);
            }
            metadata.platform = platform.dir_name().to_string();
            metadata.rom = metadata.rom.replace('\\', "/");
            cart_meta::write(root, platform, &target, &metadata)
                .map_err(|error| format!("写入游戏元数据失败：{error}"))?;
        }
    }

    // Ensure newly imported paths have their SHA in the database immediately,
    // even when the platform package had no metadata row.
    if !written_hashes.is_empty() {
        let carts = slot_store::scan_quiet(root)
            .map_err(|error| format!("扫描导入后的游戏库失败：{error}"))?;
        for cart in carts {
            let relative = cart_meta::relative(root, &cart.rom);
            let Some((digest, size)) = written_hashes.get(&relative) else {
                continue;
            };
            let _ = cart_meta::ensure(root, &cart);
            let mtime = fs::metadata(&cart.rom)
                .ok()
                .and_then(|file| file.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or_default();
            let _ = cart_meta::set_sha256(root, &cart.rom, digest, *size, mtime);
        }
    }
    Ok(())
}

fn allowed_entry(name: &str, platform: Platform) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.contains('\0') || name.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    // 本前端私有区里的前盖工程：linkSlot/Fronts/<机种>/...
    if parts[0] == "linkSlot" {
        if parts.len() < 4 || !parts[1].eq_ignore_ascii_case("Fronts") {
            return None;
        }
        if !parts[2].eq_ignore_ascii_case(platform.dir_name()) {
            return None;
        }
        if parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        {
            return None;
        }
        let extension = Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if !matches!(extension.to_ascii_lowercase().as_str(), "png" | "json") {
            return None;
        }
        return Some(PathBuf::from(&name));
    }
    if !matches!(
        parts[0],
        "Games" | "Labels" | "Info" | "Cheats" | "Saves" | "States" | "Guides" | "Fronts"
    ) {
        return None;
    }
    if !parts[1].eq_ignore_ascii_case(platform.dir_name()) {
        return None;
    }
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    let path = PathBuf::from(&name);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    match parts[0] {
        "Games" => {
            if !platform.accepts(&path) {
                return None;
            }
        }
        "Labels" => {
            if !matches!(extension, "png") {
                return None;
            }
        }
        "Info" => {
            if !matches!(extension, "txt" | "mp4") {
                return None;
            }
        }
        "Cheats" => {
            if parts.len() < 3 || !extension.eq_ignore_ascii_case("cht") {
                return None;
            }
        }
        "Saves" => {
            if parts.len() != 3 || !matches!(extension, "sav" | "srm") {
                return None;
            }
        }
        "States" => {
            if parts.len() < 5 {
                return None;
            }
        }
        "Guides" => {
            if !matches!(
                extension.to_ascii_lowercase().as_str(),
                "txt" | "epub"
            ) {
                return None;
            }
        }
        "Fronts" => {
            if parts.len() < 4
                || !matches!(extension.to_ascii_lowercase().as_str(), "png" | "json")
            {
                return None;
            }
        }
        _ => return None,
    }
    Some(path)
}

fn bump(status: &Arc<Mutex<Status>>, update: impl FnOnce(&mut Status)) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn exports_and_imports_one_game_with_all_related_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Games/GBA/Sub")).unwrap();
        fs::create_dir_all(root.join("Labels/GBA")).unwrap();
        fs::create_dir_all(root.join("Cheats/GBA/mgba")).unwrap();
        fs::create_dir_all(root.join("Saves/GBA")).unwrap();
        fs::create_dir_all(root.join("States/GBA/mgba/Test")).unwrap();
        fs::create_dir_all(root.join("Guides/GBA")).unwrap();
        fs::create_dir_all(root.join("linkSlot/Fronts/GBA/Test")).unwrap();
        fs::write(root.join("Games/GBA/Sub/Test.gba"), b"rom bytes").unwrap();
        fs::write(root.join("Labels/GBA/Test.png"), b"png bytes").unwrap();
        fs::create_dir_all(root.join("Info/GBA")).unwrap();
        fs::write(root.join("Info/GBA/Test.txt"), "介绍".as_bytes()).unwrap();
        fs::write(root.join("Info/GBA/Test.mp4"), b"video").unwrap();
        fs::write(root.join("Guides/GBA/Test.epub"), b"epub bytes").unwrap();
        fs::write(root.join("Cheats/GBA/mgba/Test.cht"), b"cheat").unwrap();
        fs::write(root.join("Saves/GBA/Test.sav"), b"battery").unwrap();
        fs::write(root.join("States/GBA/mgba/Test/resume.state"), b"state").unwrap();
        fs::write(
            root.join("States/GBA/mgba/Test/playtime.json"),
            br#"{"schema":1,"seconds":12,"total_seconds":34}"#,
        )
        .unwrap();
        fs::write(root.join("linkSlot/Fronts/GBA/Test/index.json"), br#"{"schema":1,"faces":[]}"#).unwrap();
        fs::write(root.join("linkSlot/Fronts/GBA/Test/face.png"), b"front png").unwrap();
        fs::write(root.join("linkSlot/Fronts/GBA/Test/face.json"), b"front project").unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let zip = export_game(
            root,
            "Games/GBA/Sub/Test.gba",
            &status,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            zip.file_name().and_then(|name| name.to_str()),
            Some("Test.zip"),
            "the pack is not named after the game"
        );

        let manifest = {
            let file = fs::File::open(&zip).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let mut entry = archive.by_name(MANIFEST).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        assert_eq!(manifest["version"], 2);
        assert_eq!(
            manifest["hashes"]
                .as_object()
                .unwrap()
                .get("Games/GBA/Sub/Test.gba")
                .and_then(|value| value.as_str())
                .map(str::len),
            Some(64)
        );

        let imported = tempfile::tempdir().unwrap();
        let target = imported.path();
        import_zip(target, &zip, true, &status, &cancel).unwrap();
        assert_eq!(
            fs::read(target.join("Games/GBA/Sub/Test.gba")).unwrap(),
            b"rom bytes"
        );
        assert_eq!(
            fs::read(target.join("Labels/GBA/Test.png")).unwrap(),
            b"png bytes"
        );
        assert_eq!(
            fs::read(target.join("Guides/GBA/Test.epub")).unwrap(),
            b"epub bytes"
        );
        assert_eq!(fs::read(target.join("Cheats/GBA/mgba/Test.cht")).unwrap(), b"cheat");
        assert_eq!(
            fs::read(target.join("linkSlot/Fronts/GBA/Test/face.png")).unwrap(),
            b"front png",
            "the game pack lost its front PNG"
        );
        assert_eq!(
            fs::read(target.join("linkSlot/Fronts/GBA/Test/face.json")).unwrap(),
            b"front project",
            "the game pack lost its editable front project"
        );
        assert!(
            zip_entries(&zip)
                .iter()
                .any(|name| name == "linkSlot/Fronts/GBA/Test/index.json"),
            "the archive does not list the front manifest"
        );
        assert!(
            !target.join("Saves/GBA/Test.sav").exists(),
            "the pack carried a battery save"
        );
        assert!(
            !target.join("States").exists(),
            "the pack carried save states"
        );
        assert!(
            !zip_entries(&zip).iter().any(|name| name.starts_with("Saves/")
                || name.starts_with("States/")),
            "the archive still lists saves"
        );
        let rom = target.join("Games/GBA/Sub/Test.gba");
        let meta = slot_store::cart_meta::read(target, Platform::Gba, &rom, "Test").unwrap();
        assert_eq!(meta.cover.as_deref(), Some("Labels/GBA/Test.png"));
        assert_eq!(meta.intro.as_deref(), Some("Info/GBA/Test.txt"));
        assert_eq!(meta.video.as_deref(), Some("Info/GBA/Test.mp4"));
        assert_eq!(meta.guide.as_deref(), Some("Guides/GBA/Test.epub"));
        assert!(meta.sha256.is_some(), "import did not persist SHA-256");

        let second = Arc::new(Mutex::new(Status::default()));
        import_zip(target, &zip, true, &second, &cancel).unwrap();
        let second = second.lock().unwrap();
        assert_eq!(second.duplicates, 1, "duplicate ROM was not detected");
        assert_eq!(second.imported, 0, "duplicate package rewrote files");
        assert_eq!(
            fs::read(target.join("Games/GBA/Sub/Test.gba")).unwrap(),
            b"rom bytes"
        );

        // Same bytes under a different local path must not create a second
        // card; its package stem "Test" and sidecars are skipped too.
        let other = tempfile::tempdir().unwrap();
        let other_root = other.path();
        fs::create_dir_all(other_root.join("Games/GBA")).unwrap();
        fs::write(other_root.join("Games/GBA/Other.gba"), b"rom bytes").unwrap();
        let carts = slot_store::scan_quiet(other_root).unwrap();
        let cart = carts.iter().find(|cart| cart.stem == "Other").unwrap();
        slot_store::cart_meta::ensure(other_root, cart).unwrap();
        let metadata = fs::metadata(&cart.rom).unwrap();
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap();
        slot_store::cart_meta::set_sha256(
            other_root,
            &cart.rom,
            &crate::rom_id::sha256_file(&cart.rom).unwrap(),
            metadata.len(),
            mtime,
        )
        .unwrap();
        let third = Arc::new(Mutex::new(Status::default()));
        import_zip(other_root, &zip, true, &third, &cancel).unwrap();
        assert_eq!(
            fs::read(other_root.join("Games/GBA/Other.gba")).unwrap(),
            b"rom bytes"
        );
        assert!(
            !other_root.join("Games/GBA/Sub/Test.gba").exists(),
            "same content under a different path was copied again"
        );
        assert_eq!(third.lock().unwrap().duplicates, 1);
    }

    fn mutate_manifest_hash(source: &Path, target: &Path) {
        let file = fs::File::open(source).unwrap();
        let mut source_zip = zip::ZipArchive::new(file).unwrap();
        let output = fs::File::create(target).unwrap();
        let mut output_zip = zip::ZipWriter::new(output);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for index in 0..source_zip.len() {
            let mut entry = source_zip.by_index(index).unwrap();
            let name = entry.name().to_string();
            output_zip.start_file(name.clone(), options).unwrap();
            if name == MANIFEST {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if let Some(hash) = manifest["hashes"]
                    .as_object_mut()
                    .and_then(|hashes| hashes.values_mut().next())
                {
                    *hash = serde_json::Value::String("0".repeat(64));
                }
                output_zip
                    .write_all(serde_json::to_string(&manifest).unwrap().as_bytes())
                    .unwrap();
            } else {
                std::io::copy(&mut entry, &mut output_zip).unwrap();
            }
        }
        output_zip.finish().unwrap();
    }

    fn zip_entries(path: &Path) -> Vec<String> {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        (0..archive.len())
            .filter_map(|index| archive.by_index(index).ok().map(|entry| entry.name().to_string()))
            .collect()
    }

    #[test]
    fn the_pack_keeps_the_games_chinese_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Games/GBA")).unwrap();
        fs::write(root.join("Games/GBA/龙珠大冒险.gba"), b"rom bytes").unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let zip = export_game(root, "Games/GBA/龙珠大冒险.gba", &status, &cancel).unwrap();
        assert_eq!(
            zip.file_name().and_then(|name| name.to_str()),
            Some("龙珠大冒险.zip")
        );
    }

    #[test]
    fn import_rejects_a_manifest_hash_mismatch_before_writing() {
        let source = tempfile::tempdir().unwrap();
        let source_root = source.path();
        fs::create_dir_all(source_root.join("Games/GBA")).unwrap();
        fs::write(source_root.join("Games/GBA/Game.gba"), b"rom bytes").unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let zip = export_game(source_root, "Games/GBA/Game.gba", &status, &cancel).unwrap();
        let bad = source_root.join("bad.zip");
        mutate_manifest_hash(&zip, &bad);

        let target = tempfile::tempdir().unwrap();
        let result = import_zip(target.path(), &bad, true, &status, &cancel);
        assert!(result.is_err(), "mismatched hash was accepted");
        assert!(!target.path().join("Games/GBA/Game.gba").exists());
    }

    #[test]
    fn exports_then_imports_rom_labels_for_one_platform() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Games/GBA")).unwrap();
        fs::create_dir_all(root.join("Labels/GBA")).unwrap();
        fs::create_dir_all(root.join("Info/GBA")).unwrap();
        fs::create_dir_all(root.join("linkSlot/Fronts/GBA/Test")).unwrap();
        fs::write(root.join("Games/GBA/Test.gba"), b"rom bytes").unwrap();
        fs::write(root.join("Labels/GBA/Test.png"), b"png bytes").unwrap();
        fs::create_dir_all(root.join("Info/GBA")).unwrap();
        fs::write(root.join("Info/GBA/Test.txt"), "介绍".as_bytes()).unwrap();
        fs::write(root.join("Info/GBA/Test.mp4"), b"video").unwrap();
        fs::write(root.join("linkSlot/Fronts/GBA/Test/index.json"), b"{}").unwrap();
        fs::write(root.join("linkSlot/Fronts/GBA/Test/front.png"), b"front").unwrap();
        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let zip = export_platform(root, Platform::Gba, &status, &cancel).unwrap();
        assert!(zip.is_file());

        let imported = tempfile::tempdir().unwrap();
        let target = imported.path();
        fs::create_dir_all(target.join("Games/GBA")).unwrap();
        import_zip(target, &zip, true, &status, &cancel).unwrap();
        assert_eq!(
            fs::read(target.join("Games/GBA/Test.gba")).unwrap(),
            b"rom bytes"
        );
        assert_eq!(
            fs::read(target.join("Labels/GBA/Test.png")).unwrap(),
            b"png bytes"
        );
        assert_eq!(
            fs::read_to_string(target.join("Info/GBA/Test.txt")).unwrap(),
            "介绍"
        );
        assert_eq!(
            fs::read(target.join("Info/GBA/Test.mp4")).unwrap(),
            b"video"
        );
        assert_eq!(
            fs::read(target.join("linkSlot/Fronts/GBA/Test/front.png")).unwrap(),
            b"front"
        );
    }
}
