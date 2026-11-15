//! Import the informational half of a Tianma/Pegasus GBA pack.
//!
//! The pack ships zipped ROMs plus a `metadata.pegasus.txt` per rack and a
//! `media/<game>/` sidecar with box art and an MP4.  This importer never
//! copies a ROM: it hashes the `.gba` inside each ZIP, matches that against the
//! unzipped ROMs under `Games/GBA`, and writes only the matched game's
//! description, video and (when missing) cover into `Labels/GBA`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::rom_id::{self, Digest};

pub const DEFAULT_SOURCE: &str = "Imports/TianmaG";

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub running: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub stage: String,
    pub total: usize,
    pub done: usize,
    pub matched: usize,
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

struct LocalRom {
    stem: String,
    hash: Digest,
}

#[derive(Default)]
struct Game {
    title: String,
    base: PathBuf,
    files: Vec<String>,
    description: String,
    box_asset: Option<String>,
    video_asset: Option<String>,
}

/// Resolve a user-supplied source below `<root>/Imports/`.
pub fn resolve_source(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = relative.trim();
    if relative.is_empty() {
        return Err("请填写整合包目录".to_string());
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err("整合包路径必须是卡内相对路径".to_string());
    }
    let imports = root.join("Imports");
    let imports = fs::canonicalize(&imports)
        .map_err(|_| format!("找不到 {}，请先把整合包复制到卡内", imports.display()))?;
    let source = fs::canonicalize(root.join(path))
        .map_err(|_| format!("找不到整合包目录 {}", root.join(path).display()))?;
    if !source.starts_with(&imports) || !source.is_dir() {
        return Err("整合包必须放在 Imports/ 下".to_string());
    }
    Ok(source)
}

pub fn run(
    root: PathBuf,
    source: PathBuf,
    overwrite: bool,
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
) {
    let result = import(&root, &source, overwrite, &status, &cancel);
    if let Ok(mut status) = status.lock() {
        status.running = false;
        status.finished = true;
        match result {
            Ok(()) => {
                status.stage = if status.cancelled {
                    "已取消".to_string()
                } else {
                    "导入完成".to_string()
                };
            }
            Err(error) => {
                status.stage = "导入失败".to_string();
                status.errors.push(error);
            }
        }
    }
}

fn import(
    root: &Path,
    source: &Path,
    overwrite: bool,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    set_stage(status, "读取本地 ROM 哈希");
    let local = local_roms(root, status, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        mark_cancelled(status);
        return Ok(());
    }
    let mut by_hash: HashMap<Digest, Vec<String>> = HashMap::new();
    for rom in local {
        by_hash.entry(rom.hash).or_default().push(rom.stem);
    }
    if by_hash.is_empty() {
        return Err("Games/GBA 下没有未压缩的 .gba ROM".to_string());
    }

    let mut metadata = Vec::new();
    find_metadata(source, 0, &mut metadata);
    if metadata.is_empty() {
        return Err("整合包里没有 metadata.pegasus.txt".to_string());
    }
    metadata.sort();
    let mut games = Vec::new();
    for path in &metadata {
        let base = path.parent().unwrap_or(source).to_path_buf();
        games.extend(parse_metadata(path, &base));
    }
    if games.is_empty() {
        return Err("metadata.pegasus.txt 里没有可导入的游戏".to_string());
    }

    set_stage(status, "哈希天马 ROM 并匹配");
    if let Ok(mut s) = status.lock() {
        s.total = games.len();
        s.done = 0;
    }
    let labels = root.join("Labels").join("GBA");
    let info = root.join("Info").join("GBA");
    let mut zip_hashes: HashMap<PathBuf, Vec<Digest>> = HashMap::new();
    let mut seen_stems: HashSet<String> = HashSet::new();
    for (index, game) in games.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            mark_cancelled(status);
            return Ok(());
        }
        let base = &game.base;
        let mut matched = Vec::new();
        for file in &game.files {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let path = base.join(file);
            let hashes = if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip")) {
                if let Some(hashes) = zip_hashes.get(&path) {
                    hashes.clone()
                } else {
                    let hashes = hash_zip(&path).unwrap_or_default();
                    zip_hashes.insert(path.clone(), hashes.clone());
                    hashes
                }
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("gba")) {
                hash_file(&path).into_iter().collect()
            } else {
                Vec::new()
            };
            for hash in hashes {
                if let Some(stems) = by_hash.get(&hash) {
                    matched.extend(stems.iter().cloned());
                }
            }
        }
        matched.sort();
        matched.dedup();
        if !matched.is_empty() {
            if let Ok(mut s) = status.lock() {
                s.matched += 1;
            }
            let cover = cover_for(&base, game);
            let video = video_for(&base, game);
            for stem in matched {
                if !seen_stems.insert(stem.clone()) {
                    continue;
                }
                write_info(&labels, &info, &stem, game, cover.as_deref(), video.as_deref(), overwrite, status);
            }
        }
        if let Ok(mut s) = status.lock() {
            s.done = index + 1;
        }
    }
    Ok(())
}

fn set_stage(status: &Arc<Mutex<Status>>, stage: &str) {
    if let Ok(mut status) = status.lock() {
        status.stage = stage.to_string();
    }
}

fn mark_cancelled(status: &Arc<Mutex<Status>>) {
    if let Ok(mut status) = status.lock() {
        status.cancelled = true;
    }
}

fn local_roms(
    root: &Path,
    status: &Arc<Mutex<Status>>,
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<LocalRom>, String> {
    let dir = root.join("Games").join("GBA");
    if !dir.is_dir() {
        return Err("找不到 Games/GBA".to_string());
    }
    let mut files = Vec::new();
    walk_gba(&dir, &mut files);
    files.sort();
    if let Ok(mut s) = status.lock() {
        s.total = files.len();
        s.done = 0;
    }
    let mut out = Vec::new();
    for (index, path) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(out);
        }
        if let Some(hash) = hash_file(path) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(LocalRom {
                    stem: stem.to_string(),
                    hash,
                });
            }
        }
        if let Ok(mut s) = status.lock() {
            s.done = index + 1;
        }
    }
    Ok(out)
}

fn walk_gba(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_gba(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gba"))
        {
            out.push(path);
        }
    }
}

fn hash_file(path: &Path) -> Option<Digest> {
    rom_id::digest_file(path).ok()
}

fn hash_zip(path: &Path) -> Option<Vec<Digest>> {
    let file = fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut out = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if !entry.name().to_ascii_lowercase().ends_with(".gba") {
            continue;
        }
        if let Ok(hash) = rom_id::digest_reader(entry) {
            out.push(hash);
        }
    }
    Some(out)
}

fn find_metadata(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_metadata(&path, depth + 1, out);
        } else if path.file_name().is_some_and(|name| name == "metadata.pegasus.txt") {
            out.push(path);
        }
    }
}

fn parse_metadata(path: &Path, base: &Path) -> Vec<Game> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut games = Vec::new();
    let mut current = Game::default();
    let mut in_files = false;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(title) = line.strip_prefix("game: ") {
            push_game(&mut games, &mut current, base);
            current.title = title.trim().to_string();
            in_files = false;
            continue;
        }
        if line.starts_with("file: ") {
            current.files.push(line[6..].trim().to_string());
            in_files = false;
            continue;
        }
        if line.starts_with("files:") {
            in_files = true;
            continue;
        }
        if in_files && line.starts_with("  ") {
            let file = line.trim();
            if !file.is_empty() {
                current.files.push(file.to_string());
            }
            continue;
        }
        if !line.starts_with(' ') {
            in_files = false;
        }
        if let Some(value) = line.strip_prefix("description: ") {
            current.description = value.replace("\\n", "\n");
        } else if let Some(value) = line.strip_prefix("assets.box_front: ") {
            current.box_asset = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("assets.video: ") {
            current.video_asset = Some(value.trim().to_string());
        }
    }
    push_game(&mut games, &mut current, base);
    games
}

fn push_game(games: &mut Vec<Game>, current: &mut Game, base: &Path) {
    if !current.title.is_empty() && !current.files.is_empty() {
        games.push(Game {
            title: std::mem::take(&mut current.title),
            base: base.to_path_buf(),
            files: std::mem::take(&mut current.files),
            description: std::mem::take(&mut current.description),
            box_asset: current.box_asset.take(),
            video_asset: current.video_asset.take(),
        });
    } else {
        *current = Game::default();
    }
}

fn cover_for(base: &Path, game: &Game) -> Option<PathBuf> {
    if let Some(asset) = &game.box_asset {
        let path = base.join(asset);
        if path.is_file() {
            return Some(path);
        }
    }
    let dir = base.join("media").join(&game.title);
    for name in ["boxfront.png", "mini_boxfront.png", "boxfront.jpg", "mini_boxfront.jpg"] {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn video_for(base: &Path, game: &Game) -> Option<PathBuf> {
    if let Some(asset) = &game.video_asset {
        let path = base.join(asset);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = base.join("media").join(&game.title).join("video.mp4");
    path.is_file().then_some(path)
}

fn write_info(
    labels: &Path,
    info: &Path,
    stem: &str,
    game: &Game,
    cover: Option<&Path>,
    video: Option<&Path>,
    overwrite: bool,
    status: &Arc<Mutex<Status>>,
) {
    if fs::create_dir_all(labels).is_err() {
        return;
    }
    let cover_target = labels.join(format!("{stem}.png"));
    if !cover_target.exists() {
        if let Some(source) = cover {
            if write_cover(source, &cover_target).is_ok() {
                bump(status, |s| s.imported += 1);
            }
        }
    } else {
        bump(status, |s| s.skipped += 1);
    }

    if fs::create_dir_all(info).is_err() {
        return;
    }
    let text_target = info.join(format!("{stem}.txt"));
    if !game.description.trim().is_empty() && (overwrite || !text_target.exists()) {
        if write_atomic(&text_target, game.description.as_bytes()).is_ok() {
            bump(status, |s| s.imported += 1);
        }
    } else if text_target.exists() {
        bump(status, |s| s.skipped += 1);
    }

    let video_target = info.join(format!("{stem}.mp4"));
    if let Some(source) = video {
        if overwrite || !video_target.exists() {
            if copy_atomic(source, &video_target).is_ok() {
                bump(status, |s| s.imported += 1);
            }
        } else {
            bump(status, |s| s.skipped += 1);
        }
    }
}

fn write_cover(source: &Path, target: &Path) -> Result<(), String> {
    if source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        return copy_atomic(source, target);
    }
    let mut decoder = jpeg_decoder::Decoder::new(
        fs::File::open(source).map_err(|error| error.to_string())?,
    );
    let pixels = decoder.decode().map_err(|error| format!("JPEG 解码失败：{error}"))?;
    let info = decoder.info().ok_or_else(|| "JPEG 缺少尺寸".to_string())?;
    let file = fs::File::create(target).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(file, info.width as u32, info.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer.write_image_data(&pixels).map_err(|error| error.to_string())?;
    Ok(())
}

fn copy_atomic(source: &Path, target: &Path) -> Result<(), String> {
    let temp = target.with_extension(format!(
        "{}.part",
        target.extension().and_then(|ext| ext.to_str()).unwrap_or("tmp")
    ));
    fs::copy(source, &temp).map_err(|error| error.to_string())?;
    fs::rename(&temp, target).map_err(|error| error.to_string())?;
    Ok(())
}

fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = target.with_extension("txt.part");
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp, target).map_err(|error| error.to_string())?;
    Ok(())
}

fn bump(status: &Arc<Mutex<Status>>, update: impl FnOnce(&mut Status)) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(&vec![0x40u8; (w * h * 4) as usize]).unwrap();
        }
        out
    }

    #[test]
    fn matches_zipped_rom_hash_and_writes_info_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let rom = b"fake gba rom bytes";
        fs::create_dir_all(root.join("Games/GBA")).unwrap();
        fs::write(root.join("Games/GBA/Test Game.gba"), rom).unwrap();
        let meta_dir = root.join("Imports/TianmaG/GBA");
        let media = root.join("Imports/TianmaG/GBA/media/Test Game");
        fs::create_dir_all(&media).unwrap();
        let zip_path = meta_dir.join("Test Game.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("Test.gba", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(rom).unwrap();
            zip.finish().unwrap();
        }
        fs::write(
            meta_dir.join("metadata.pegasus.txt"),
            "collection: GBA\n\ngame: Test Game\nfile: Test Game.zip\ndescription: line one\\nline two\n",
        )
        .unwrap();
        fs::write(media.join("boxfront.png"), png_bytes(8, 8)).unwrap();
        fs::write(media.join("video.mp4"), b"fake mp4").unwrap();

        let status = Arc::new(Mutex::new(Status::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        run(root.to_path_buf(), root.join("Imports/TianmaG"), false, status.clone(), cancel);
        let status = status.lock().unwrap();
        assert_eq!(status.matched, 1);
        assert_eq!(status.imported, 3, "{:?}", status.errors);
        assert!(root.join("Info/GBA/Test Game.txt").is_file());
        assert!(root.join("Info/GBA/Test Game.mp4").is_file());
        assert!(root.join("Labels/GBA/Test Game.png").is_file());
        let text = fs::read_to_string(root.join("Info/GBA/Test Game.txt")).unwrap();
        assert!(text.contains("line one\nline two"));
        assert!(!root.join("Games/GBA/Imported.gba").exists());
    }
}
