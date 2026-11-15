//! A persisted SHA-256 per cart, built in the background.
//!
//! Matching a peer's cart by ROM SHA-256 used to mean reading every cart on
//! the platform in the session's main loop. On a full card that is minutes of
//! I/O, so the link heartbeat died while one question was being answered.
//! This index is loaded at boot, filled one cart at a time on a low-priority
//! worker, and consulted from memory when a request arrives.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use crate::rom_id::{self, Digest};

/// Where the index lives on the card. Kept under `linkSlot/Config` so it
/// travels with the rest of the link settings and survives upgrades.
pub const INDEX_FILE: &str = "linkSlot/Config/rom_sha256.tsv";
const HEADER: &str = "# linkSlot rom hashes v3";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    size: u64,
    mtime: u64,
    digest: Digest,
}

struct Update {
    path: PathBuf,
    entry: Entry,
}

/// A completed SHA-256 that the app should persist to the carts table.
pub struct ShaUpdate {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub mtime: u64,
}

/// The in-memory index plus the optional worker filling it.
pub struct RomIndex {
    path: PathBuf,
    entries: HashMap<PathBuf, Entry>,
    rx: Option<Receiver<Update>>,
    pause: Arc<AtomicBool>,
    started: bool,
    sha_updates: Vec<ShaUpdate>,
}

impl RomIndex {
    /// No root yet (the app's `new` before `boot` gives it one).
    pub fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            entries: HashMap::new(),
            rx: None,
            pause: Arc::new(AtomicBool::new(false)),
            started: false,
            sha_updates: Vec::new(),
        }
    }

    pub fn load(root: &Path) -> Self {
        let path = root.join(INDEX_FILE);
        let entries = read_index(&path);
        Self {
            path,
            entries,
            rx: None,
            pause: Arc::new(AtomicBool::new(false)),
            started: false,
            sha_updates: Vec::new(),
        }
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// SHA-256 for `path`, but only when the file still matches the index.
    pub fn hash(&self, path: &Path) -> Option<Digest> {
        let meta = fs::metadata(path).ok()?;
        let size = meta.len();
        let mtime = modified_stamp(&meta);
        self.entries
            .get(path)
            .filter(|entry| entry.size == size && entry.mtime == mtime)
            .map(|entry| entry.digest)
    }

    /// Drain completed SHA-256 values for persistence after [`Self::poll`].
    pub fn take_sha_updates(&mut self) -> Vec<ShaUpdate> {
        std::mem::take(&mut self.sha_updates)
    }

    /// Kick off the background fill. One worker per app; a second call is a
    /// no-op.
    pub fn start(&mut self, root: &Path, carts: Vec<PathBuf>) {
        if self.started || carts.is_empty() {
            return;
        }
        self.started = true;
        self.path = root.join(INDEX_FILE);
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let pause = Arc::clone(&self.pause);
        let path = self.path.clone();
        let mut entries = self.entries.clone();
        let _ = std::thread::Builder::new()
            .name("slot-rom-index".into())
            .spawn(move || fill(&path, &mut entries, carts, tx, pause));
    }

    /// Drain hashes the worker finished. Called once a frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        while let Ok(update) = rx.try_recv() {
            self.sha_updates.push(ShaUpdate {
                path: update.path.clone(),
                sha256: rom_id::digest_hex(&update.entry.digest),
                size: update.entry.size,
                mtime: update.entry.mtime,
            });
            self.entries.insert(update.path, update.entry);
        }
    }

    /// Stop reading the card while a game or a link owns it.
    pub fn set_paused(&self, paused: bool) {
        self.pause.store(paused, Ordering::Relaxed);
    }

    /// Number of fresh entries, for diagnostics and tests.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn fill(
    index_path: &Path,
    entries: &mut HashMap<PathBuf, Entry>,
    carts: Vec<PathBuf>,
    tx: Sender<Update>,
    pause: Arc<AtomicBool>,
) {
    // Never compete with the boot face/texture build.  Once the index has
    // SHA-256 for a cart, linking only touches memory.
    #[cfg(not(test))]
    std::thread::sleep(Duration::from_secs(2));
    let total = carts.len();
    let mut done = 0usize;
    for path in carts {
        while pause.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let size = meta.len();
        let mtime = modified_stamp(&meta);
        if entries
            .get(&path)
            .is_some_and(|entry| entry.size == size && entry.mtime == mtime)
        {
            continue;
        }
        let Some(digest) = rom_id::digest_file(&path).ok() else {
            continue;
        };
        let entry = Entry {
            size,
            mtime,
            digest,
        };
        entries.insert(path.clone(), entry.clone());
        if tx.send(Update { path, entry }).is_err() {
            break;
        }
        done += 1;
        if done % 16 == 0 {
            write_index(index_path, entries);
        }
        // One cart per turn keeps the SD card available for the UI and the
        // game; a 32 MB read is already the bulk of the work.
        std::thread::sleep(Duration::from_millis(20));
    }
    write_index(index_path, entries);
    eprintln!("slot: rom index: {done}/{total} updated");
}

fn modified_stamp(meta: &fs::Metadata) -> u64 {
    // Nanoseconds catch a same-size rewrite within the same second; the u64
    // holds nanoseconds until 2554.
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

fn read_index(path: &Path) -> HashMap<PathBuf, Entry> {
    let mut entries = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return entries;
    };
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(raw_path), Some(size), Some(mtime)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(size), Ok(mtime)) = (size.parse::<u64>(), mtime.parse::<u64>()) else {
            continue;
        };
        let digest = fields.find_map(rom_id::digest_from_hex);
        let Some(digest) = digest else {
            continue;
        };
        entries.insert(
            PathBuf::from(unescape(raw_path)),
            Entry {
                size,
                mtime,
                digest,
            },
        );
    }
    entries
}

fn write_index(path: &Path, entries: &HashMap<PathBuf, Entry>) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut lines = vec![HEADER.to_string()];
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (cart, entry) in sorted {
        lines.push(format!(
            "{}\t{}\t{}\t{}",
            escape(&cart.to_string_lossy()),
            entry.size,
            entry.mtime,
            rom_id::digest_hex(&entry.digest)
        ));
    }
    let tmp = path.with_extension("tsv.tmp");
    if fs::write(&tmp, lines.join("\n") + "\n").is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn cart(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join("Games/GBA").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        path
    }

    #[test]
    fn a_digest_survives_a_reload_and_tracks_the_file_stamp() {
        let d = tempfile::tempdir().unwrap();
        let path = cart(d.path(), "Emerald.gba", b"emerald");
        let digest = rom_id::digest_file(&path).unwrap();
        let mut index = RomIndex::load(d.path());
        assert_eq!(index.hash(&path), None);

        index.start(d.path(), vec![path.clone()]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while index.hash(&path).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            index.poll();
        }
        assert_eq!(index.hash(&path), Some(digest));
        assert_eq!(index.take_sha_updates().len(), 1);

        // The worker flushes the TSV after the batch; wait for the file before
        // simulating a fresh boot.
        let index_path = d.path().join(INDEX_FILE);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !index_path.is_file() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        // A fresh boot reads the stamp from disk without reading the ROM.
        let mut loaded = RomIndex::load(d.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.hash(&path), Some(digest));

        // Replacing the file invalidates the entry until the worker catches up.
        std::fs::write(&path, b"a different rom").unwrap();
        assert_eq!(loaded.hash(&path), None);
        loaded.start(d.path(), vec![path.clone()]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while loaded.hash(&path).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            loaded.poll();
        }
        assert_eq!(
            loaded.hash(&path),
            rom_id::digest_file(&path).ok(),
            "the changed file was not re-indexed"
        );
    }

    #[test]
    fn an_old_fnv_index_is_rebuilt_as_sha256() {
        let d = tempfile::tempdir().unwrap();
        let path = cart(d.path(), "Old.gba", b"rom");
        let metadata = std::fs::metadata(&path).unwrap();
        std::fs::create_dir_all(d.path().join("linkSlot/Config")).unwrap();
        std::fs::write(
            d.path().join(INDEX_FILE),
            format!(
                "# linkSlot rom hashes v1\n{}\t{}\t{}\tdeadbeef\n",
                escape(&path.to_string_lossy()),
                metadata.len(),
                modified_stamp(&metadata)
            ),
        )
        .unwrap();
        let mut loaded = RomIndex::load(d.path());
        assert_eq!(loaded.hash(&path), None);
        loaded.start(d.path(), vec![path.clone()]);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while loaded.hash(&path).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            loaded.poll();
        }
        assert_eq!(loaded.hash(&path), rom_id::digest_file(&path).ok());
    }

    #[test]
    fn the_tsv_round_trips_names_with_spaces_and_unicode() {
        let d = tempfile::tempdir().unwrap();
        let path = cart(d.path(), "三国志 英杰传.gba", b"rom");
        let entry = Entry {
            size: 3,
            mtime: 7,
            digest: [7u8; 32],
        };
        let mut entries = HashMap::new();
        entries.insert(path.clone(), entry.clone());
        write_index(&d.path().join(INDEX_FILE), &entries);
        let loaded = read_index(&d.path().join(INDEX_FILE));
        assert_eq!(loaded.get(&path), Some(&entry));
    }
}
