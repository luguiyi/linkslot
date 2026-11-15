use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::gba::header_title_code;
use crate::Platform;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cart {
    pub platform: Platform,
    pub stem: String,
    pub rom: PathBuf,
    pub label: Option<PathBuf>,
    pub title: String,
    pub code: String,
    pub shell: Option<crate::ShellChoice>,
    /// Which physical Famicom/NES cartridge shell this game uses.
    pub model: crate::CartModel,
    /// The selected custom front-face overlay, if one is available.
    pub front: Option<PathBuf>,
}

impl Cart {
    pub fn read_header(&mut self) {
        match self.platform {
            Platform::Gba => {
                if let Some((title, code)) = header_title_code(&self.rom) {
                    self.title = title;
                    self.code = code;
                }
            }
            Platform::Gb | Platform::Gbc => {
                if let Some(title) = crate::gb::title(&self.rom) {
                    self.title = title;
                }
            }
            // A .nes file has no printable title header; feeding it to the
            // Game Boy reader would only invent mojibake. The shelf falls
            // back to the file name.
            Platform::Fc | Platform::Sfc | Platform::Md => {}
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub fn scan(root: &Path) -> Result<Vec<Cart>, StoreError> {
    let started = Instant::now();
    let carts = collect(root)?;
    let collected = started.elapsed();
    eprintln!(
        "slot: scan: {} games under {}",
        carts.len(),
        root.join("Games").display()
    );
    // The report promises what the scan looks for. Deriving both lines from
    // `Platform::ALL` keeps that promise true when a machine is added, where a
    // hand-written list silently goes stale.
    let expected: Vec<String> = Platform::ALL
        .iter()
        .map(|platform| format!("Games/{}", platform.dir_name()))
        .collect();
    let mut extensions: Vec<&str> = Vec::new();
    for platform in Platform::ALL {
        for extension in platform.extensions() {
            if !extensions.contains(extension) {
                extensions.push(extension);
            }
        }
    }
    let supported: Vec<String> = extensions
        .iter()
        .map(|extension| format!(".{extension}"))
        .collect();
    let mut report = format!(
        "Slot R6 library scan: {} games\nExpected: {} (including subfolders).\nSupported: {}, case-insensitive. Archives are not ROM files.\n",
        carts.len(),
        expected.join(", "),
        supported.join(" / ")
    );
    for c in &carts {
        report.push_str(&format!(
            "{}\t{}\t{}\n",
            c.platform.dir_name(),
            initial(&c.stem),
            c.rom.strip_prefix(root).unwrap_or(&c.rom).display()
        ));
    }
    let report_started = Instant::now();
    let log_dir = root.join("linkSlot/Logs");
    let log_path = log_dir.join("slot-library.log");
    if let Err(e) = std::fs::create_dir_all(&log_dir)
        .and_then(|_| std::fs::write(&log_path, report))
    {
        eprintln!("slot: library report: {e}");
    }
    eprintln!(
        "slot: scan: collect {:.1} ms, report {:.1} ms",
        collected.as_secs_f64() * 1e3,
        report_started.elapsed().as_secs_f64() * 1e3
    );
    Ok(carts)
}

/// Scan the library without writing the report log.
///
/// The Web game library is a read-only view and must not touch the card merely
/// because someone opened the page.  The cartridge and label rules stay shared
/// with [`scan`] through [`collect`].
pub fn scan_quiet(root: &Path) -> Result<Vec<Cart>, StoreError> {
    collect(root)
}

fn collect(root: &Path) -> Result<Vec<Cart>, StoreError> {
    let read = |file: &str| std::fs::read_to_string(root.join(file)).unwrap_or_default();
    let shells = crate::cart_shell::layered(
        &read(crate::CART_SHELL_FILE),
        &read(crate::LABELS_SHELL_FILE),
    );
    let colours = crate::cart_color::read(root);
    let cached = crate::cart_meta::read_all(root);
    let mut carts = Vec::new();
    let mut nested_listings: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    for platform in Platform::ALL {
        let dir = root.join("Games").join(platform.dir_name());
        let entries = rom_entries(&dir);
        let labels_dir = root.join("Labels").join(platform.dir_name());
        let labels = listing(&labels_dir);
        for rom in entries {
            if is_hidden(&rom) || !platform.accepts(&rom) || !rom.is_file() {
                continue;
            }
            let Some(stem) = rom.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let nested_labels = rom.parent().and_then(|parent| parent.strip_prefix(&dir).ok())
                .filter(|rel| !rel.as_os_str().is_empty()).map(|rel| labels_dir.join(rel));
            let label_name = format!("{stem}.png").to_lowercase();
            let label = nested_labels
                .and_then(|folder| {
                    // Thousands of ROMs often share one deep Games/ subfolder.
                    // Listing the matching Labels/ subfolder once per folder,
                    // instead of once per ROM, is the difference between a
                    // large exFAT card scanning in seconds and in tens of
                    // seconds.
                    let entries = nested_listings.entry(folder.clone()).or_insert_with(|| listing(&folder));
                    entries.get(&label_name).map(|name| folder.join(name))
                })
                .or_else(|| labels.get(&label_name).map(|name| labels_dir.join(name)));
            let shell = shells.get(&crate::cart_shell::key(stem)).copied();
            let shell = colours
                .get(&crate::cart_color::key(platform, stem))
                .copied()
                .map(|colour| {
                    let mut choice = shell.unwrap_or(crate::ShellChoice {
                        outline: crate::Outline::Auto,
                        colour,
                        finish: crate::ShellFinish::Solid,
                    });
                    choice.colour = colour;
                    choice
                })
                .or(shell);
            let mut cart = Cart {
                platform,
                stem: stem.to_string(),
                title: String::new(),
                code: String::new(),
                shell,
                label,
                rom,
                model: crate::CartModel::default(),
                front: None,
            };
            let meta = cached
                .get(&crate::cart_meta::relative(root, &cart.rom))
                .cloned();
            if let Some(meta) = meta {
                if let Some(cover) = meta.cover_path(root) {
                    cart.label = Some(cover);
                }
                if let Some(shell) = meta.shell_choice() {
                    cart.shell = Some(shell);
                }
                if platform == Platform::Fc {
                    cart.model = crate::CartModel::from_meta(&meta.extra);
                }
                cart.front = crate::fronts::selected_path(root, &cart, Some(&meta));
            }
            carts.push(cart);
        }
    }
    carts.sort_by(|a, b| {
        (a.platform as u8, sort_key(&a.stem)).cmp(&(b.platform as u8, sort_key(&b.stem)))
    });
    Ok(carts)
}

pub fn listing(dir: &Path) -> HashMap<String, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .map(|name| (name.to_lowercase(), name))
        .collect()
}

/// Collect paths, not `DirEntry`s: a `DirEntry` can keep its parent
/// directory descriptor alive, and a library with thousands of ROMs can then
/// exhaust the process file-descriptor limit (`EMFILE`) before the shelf is
/// even built.
fn rom_entries(dir: &Path) -> Vec<PathBuf> {
    let mut pending = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(folder) = pending.pop() {
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("slot: scan: {}: {e}", folder.display());
                }
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    eprintln!("slot: scan entry: {e}");
                    continue;
                }
            };
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            match entry.file_type() {
                // Do not follow directory symlinks: avoids cycles and
                // scanning outside Games.
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(_) => files.push(path),
                Err(_) => {}
            }
        }
    }
    files
}

/// Sorting and all navigation share the same pinyin/ASCII initial.
pub fn sort_key(stem: &str) -> (u8, String) {
    let letter = initial(stem);
    let group = if letter == '#' { 0 } else { 1 };
    (group, format!("{letter}{}", stem.trim().to_uppercase()))
}

pub fn initial(stem: &str) -> char {
    crate::title_search::initial(stem).unwrap_or('#')
}


pub fn is_hidden(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}
