//! Scan the card for save units shown on the Web page.
//!
//! Layout (see the linkSlot project book):
//!
//! ```text
//! States/<platform>/<emulator>/<game>/resume.state      resume slot
//! States/<platform>/<emulator>/<game>/<stamp>.state     ring snapshots
//! States/<platform>/<emulator>/<game>/<stamp>.png       thumbnails
//! Saves/<platform>/<game>.sav | .srm                    battery saves
//! ```
//!
//! Directory walking never follows symlinks; unreadable entries are skipped
//! so one corrupt card entry cannot take the whole page down.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::web::util;

const STAMP_SHAPE: &[u8] = b"0000-00-00_00-00-00";

#[derive(Clone, Debug)]
pub struct StateFile {
    pub name: String,
    pub resume: bool,
    pub stamp: Option<String>,
    pub size: u64,
    pub mtime: u64,
    pub thumb: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BatteryFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Clone, Debug)]
pub struct SaveUnit {
    pub platform: String,
    pub emulator: String,
    pub game: String,
    pub states: Vec<StateFile>,
    pub battery: Vec<BatteryFile>,
    /// Seconds played on this device only (`playtime.json` → `seconds`).
    pub playtime_seconds: u64,
    /// Seconds played across all devices (`playtime.json` → `total_seconds`).
    pub playtime_total_seconds: u64,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub units: Vec<SaveUnit>,
    pub games: usize,
    pub states: usize,
    pub battery: usize,
}

/// Walk `States/` and `Saves/` and group the result into save units.
///
/// Each `States/<platform>/<emulator>/<game>/` folder is also asked for its
/// `playtime.json` (owned by `crate::playtime`); a missing or corrupt file
/// reads as 0 seconds and never breaks the scan.
pub fn scan(root: &Path) -> Scan {
    let mut units: Vec<SaveUnit> = Vec::new();
    for (platform, platform_dir) in read_dirs(&root.join("States")) {
        for (emulator, emulator_dir) in read_dirs(&platform_dir) {
            for (game, game_dir) in read_dirs(&emulator_dir) {
                let states = scan_state_dir(&game_dir, &platform, &emulator, &game);
                if states.is_empty() {
                    continue;
                }
                let playtime = crate::playtime::read(&game_dir);
                units.push(SaveUnit {
                    platform: platform.clone(),
                    emulator: emulator.clone(),
                    game,
                    states,
                    battery: Vec::new(),
                    playtime_seconds: playtime.seconds,
                    playtime_total_seconds: playtime.total_seconds,
                });
            }
        }
    }

    let mut battery_files = 0usize;
    for (platform, platform_dir) in read_dirs(&root.join("Saves")) {
        for (name, size, mtime) in read_files(&platform_dir) {
            let Some((stem, extension)) = name.rsplit_once('.') else {
                continue;
            };
            if !matches!(extension.to_ascii_lowercase().as_str(), "sav" | "srm") {
                continue;
            }
            battery_files += 1;
            let file = BatteryFile {
                name: name.clone(),
                path: format!("Saves/{platform}/{name}"),
                size,
                mtime,
            };
            let mut attached = false;
            for unit in units
                .iter_mut()
                .filter(|unit| unit.platform == platform && unit.game == stem)
            {
                unit.battery.push(file.clone());
                attached = true;
            }
            if !attached {
                // A battery save whose state folder no longer exists (or lives
                // on another emulator): still list it so it can be downloaded.
                // There is no per-game folder to read `playtime.json` from.
                units.push(SaveUnit {
                    platform: platform.clone(),
                    emulator: String::new(),
                    game: stem.to_string(),
                    states: Vec::new(),
                    battery: vec![file],
                    playtime_seconds: 0,
                    playtime_total_seconds: 0,
                });
            }
        }
    }

    units.sort_by(|left, right| {
        left.platform
            .cmp(&right.platform)
            .then_with(|| left.emulator.cmp(&right.emulator))
            .then_with(|| left.game.cmp(&right.game))
    });
    for unit in &mut units {
        unit.battery.sort_by(|left, right| left.name.cmp(&right.name));
    }

    let states = units.iter().map(|unit| unit.states.len()).sum();
    let games = units.len();
    let battery = battery_files;
    Scan {
        units,
        games,
        states,
        battery,
    }
}

/// `{"units": [...]}` payload for `GET /api/saves`.
pub fn units_value(scan: &Scan) -> Value {
    let units: Vec<Value> = scan
        .units
        .iter()
        .map(|unit| {
            let states: Vec<Value> = unit
                .states
                .iter()
                .map(|state| {
                    json!({
                        "name": &state.name,
                        "resume": state.resume,
                        "stamp": &state.stamp,
                        "size": state.size,
                        "mtime": state.mtime,
                        "thumb": &state.thumb,
                    })
                })
                .collect();
            let battery: Vec<Value> = unit
                .battery
                .iter()
                .map(|file| {
                    json!({
                        "name": &file.name,
                        "path": &file.path,
                        "size": file.size,
                        "mtime": file.mtime,
                    })
                })
                .collect();
            json!({
                "platform": &unit.platform,
                "emulator": &unit.emulator,
                "game": &unit.game,
                "states": states,
                "battery": battery,
                "playtime_seconds": unit.playtime_seconds,
                "playtime_total_seconds": unit.playtime_total_seconds,
            })
        })
        .collect();
    json!({ "units": units })
}

/// `storage` object for `GET /api/status`.
///
/// `playtime_seconds` is the sum of each unit's cross-device
/// `playtime_total_seconds` (the unit folders do not overlap).
pub fn storage_value(scan: &Scan) -> Value {
    let playtime_seconds: u64 = scan
        .units
        .iter()
        .map(|unit| unit.playtime_total_seconds)
        .sum();
    json!({
        "games": scan.games,
        "states": scan.states,
        "battery": scan.battery,
        "playtime_seconds": playtime_seconds,
    })
}

fn scan_state_dir(dir: &Path, platform: &str, emulator: &str, game: &str) -> Vec<StateFile> {
    let files = read_files(dir);
    let names: Vec<&str> = files.iter().map(|(name, _, _)| name.as_str()).collect();
    let mut states = Vec::new();
    for (name, size, mtime) in &files {
        let Some(stem) = name.strip_suffix(".state") else {
            continue;
        };
        let (resume, stamp) = if stem == "resume" {
            (true, None)
        } else if is_stamp(stem) {
            (false, Some(stem.to_string()))
        } else {
            // `resume-refused-*` and temporary/foreign files are not archives.
            continue;
        };
        let thumb_name = format!("{stem}.png");
        let thumb = names
            .iter()
            .any(|candidate| candidate == &thumb_name)
            .then(|| {
                let relative = format!("States/{platform}/{emulator}/{game}/{thumb_name}");
                format!("/api/file?path={}", util::encode_query(&relative))
            });
        states.push(StateFile {
            name: stem.to_string(),
            resume,
            stamp,
            size: *size,
            mtime: *mtime,
            thumb,
        });
    }
    states.sort_by(|left, right| match (left.resume, right.resume) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => right.name.cmp(&left.name),
    });
    states
}

/// Strict 19-character stamp: `YYYY-MM-DD_HH-MM-SS`.
pub fn is_stamp(text: &str) -> bool {
    text.len() == STAMP_SHAPE.len()
        && text.bytes().zip(STAMP_SHAPE).all(|(byte, shape)| match shape {
            b'0' => byte.is_ascii_digit(),
            other => byte == *other,
        })
}

fn read_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        dirs.push((name.to_string(), path));
    }
    dirs.sort_by(|left, right| left.0.cmp(&right.0));
    dirs
}

fn read_files(dir: &Path) -> Vec<(String, u64, u64)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        files.push((name.to_string(), metadata.len(), util::mtime_secs(&metadata)));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("States/GBA/mgba/宝可梦/resume.state"), b"resume");
        write_file(&root.join("States/GBA/mgba/宝可梦/resume.png"), b"png0");
        write_file(&root.join("States/GBA/mgba/宝可梦/2026-10-21_10-00-00.state"), b"state1");
        write_file(&root.join("States/GBA/mgba/宝可梦/2026-10-21_10-00-00.png"), b"png1");
        write_file(&root.join("States/GBA/mgba/宝可梦/2026-10-20_09-00-00.state"), b"state2");
        write_file(&root.join("States/GBA/mgba/宝可梦/2026-10-20_09-00-00.png"), b"png2");
        write_file(&root.join("States/GBA/mgba/宝可梦/resume-refused-2026-10-19_08-00-00.state"), b"refused");
        write_file(&root.join("States/GBA/mgba/宝可梦/notes.txt"), b"ignore");
        write_file(
            &root.join("States/GBA/mgba/宝可梦/playtime.json"),
            br#"{"schema":1,"seconds":7200,"total_seconds":10800,"devices":{"dev-a":7200,"dev-b":3600}}"#,
        );
        write_file(&root.join("States/GBA/gpsp/宝可梦/resume.state"), b"other core");
        // Corrupt playtime file: must read as 0, not fail the scan.
        write_file(
            &root.join("States/GBA/gpsp/宝可梦/playtime.json"),
            b"{not valid json",
        );
        write_file(&root.join("States/GBC/mgba/Legend/resume.state"), b"gbc");
        // Legend intentionally has no playtime.json (missing = 0).
        write_file(&root.join("Saves/GBA/宝可梦.sav"), b"battery");
        write_file(&root.join("Saves/GBA/宝可梦.srm"), b"battery2");
        write_file(&root.join("Saves/GBA/Orphan.sav"), b"orphan");
        dir
    }

    #[test]
    fn scan_groups_units_and_skips_non_archives() {
        let dir = fixture();
        let scan = scan(dir.path());
        assert_eq!(scan.games, 4);
        assert_eq!(scan.states, 5);
        assert_eq!(scan.battery, 3);
        let unit = scan
            .units
            .iter()
            .find(|unit| unit.game == "宝可梦" && unit.emulator == "mgba")
            .expect("mgba unit");
        assert_eq!(unit.states.len(), 3);
        assert!(unit.states[0].resume);
        assert_eq!(unit.states[1].name, "2026-10-21_10-00-00");
        assert_eq!(unit.battery.len(), 2);
        assert!(unit.states[1].thumb.is_some());
        assert!(unit.states[0].thumb.is_some(), "resume.png was not shown");
        assert_eq!(unit.playtime_seconds, 7200);
        assert_eq!(unit.playtime_total_seconds, 10800);
        let orphan = scan
            .units
            .iter()
            .find(|unit| unit.game == "Orphan")
            .expect("orphan battery unit");
        assert!(orphan.states.is_empty());
        assert_eq!(orphan.battery.len(), 1);
        assert_eq!(orphan.playtime_seconds, 0);
        assert_eq!(orphan.playtime_total_seconds, 0);
    }

    #[test]
    fn playtime_reads_valid_file_and_defaults_for_bad_or_missing_files() {
        let dir = fixture();
        let scan = scan(dir.path());
        let valid = scan
            .units
            .iter()
            .find(|unit| unit.game == "宝可梦" && unit.emulator == "mgba")
            .expect("mgba unit");
        assert_eq!(valid.playtime_seconds, 7200);
        assert_eq!(valid.playtime_total_seconds, 10800);

        let corrupt = scan
            .units
            .iter()
            .find(|unit| unit.game == "宝可梦" && unit.emulator == "gpsp")
            .expect("gpsp unit");
        assert_eq!(corrupt.playtime_seconds, 0);
        assert_eq!(corrupt.playtime_total_seconds, 0);

        let missing = scan
            .units
            .iter()
            .find(|unit| unit.game == "Legend")
            .expect("Legend unit");
        assert_eq!(missing.playtime_seconds, 0);
        assert_eq!(missing.playtime_total_seconds, 0);

        // Status storage total sums the cross-device totals of all units.
        let storage = storage_value(&scan);
        assert_eq!(storage["playtime_seconds"], 10800);
    }

    #[test]
    fn units_value_has_expected_shape() {
        let dir = fixture();
        let scan = scan(dir.path());
        let value = units_value(&scan);
        let units = value["units"].as_array().unwrap();
        assert_eq!(units.len(), 4);
        let unit = units
            .iter()
            .find(|unit| unit["game"] == "宝可梦" && unit["emulator"] == "mgba")
            .unwrap();
        assert_eq!(unit["platform"], "GBA");
        assert_eq!(unit["states"][0]["resume"], true);
        assert_eq!(unit["states"][0]["stamp"], Value::Null);
        assert!(unit["states"][0]["thumb"]
            .as_str()
            .unwrap()
            .ends_with("resume.png"));
        assert_eq!(unit["states"][1]["resume"], false);
        assert_eq!(unit["states"][1]["stamp"], "2026-10-21_10-00-00");
        assert!(
            unit["states"][1]["thumb"]
                .as_str()
                .unwrap()
                .ends_with("2026-10-21_10-00-00.png")
        );
        assert!(unit["states"][1]["size"].as_u64().unwrap() > 0);
        assert!(unit["states"][1]["mtime"].as_u64().is_some());
        assert_eq!(unit["playtime_seconds"], 7200);
        assert_eq!(unit["playtime_total_seconds"], 10800);
        let corrupt = units
            .iter()
            .find(|unit| unit["game"] == "宝可梦" && unit["emulator"] == "gpsp")
            .unwrap();
        assert_eq!(corrupt["playtime_seconds"], 0);
        assert_eq!(corrupt["playtime_total_seconds"], 0);
        assert_eq!(
            unit["battery"][0]["path"],
            "Saves/GBA/宝可梦.sav"
        );
    }

    #[test]
    fn storage_value_counts() {
        let dir = fixture();
        let scan = scan(dir.path());
        let value = storage_value(&scan);
        assert_eq!(value["games"], 4);
        assert_eq!(value["states"], 5);
        assert_eq!(value["battery"], 3);
        assert_eq!(value["playtime_seconds"], 10800);
    }

    #[test]
    fn stamp_validation_is_strict() {
        assert!(is_stamp("2026-10-21_10-00-00"));
        assert!(!is_stamp("2026-10-21_10-00-0"));
        assert!(!is_stamp("2026-10-21 10-00-00"));
        assert!(!is_stamp("resume"));
        assert!(!is_stamp("2026-10-21_10-00-00.state"));
    }

    #[test]
    fn missing_directories_scan_empty() {
        let dir = tempfile::tempdir().unwrap();
        let scan = scan(dir.path());
        assert!(scan.units.is_empty());
        assert_eq!(scan.games, 0);
        assert_eq!(scan.states, 0);
        assert_eq!(scan.battery, 0);
    }

    #[test]
    fn symlinked_directories_are_not_followed() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(outside.join("GBA/mgba/Game")).unwrap();
            std::fs::write(outside.join("GBA/mgba/Game/resume.state"), b"x").unwrap();
            std::fs::create_dir_all(root.join("States")).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("States/linked")).unwrap();
            let scan = scan(root);
            assert!(scan.units.is_empty());
        }
    }
}
