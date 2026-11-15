//! Per-game playtime, stored inside the save unit:
//! `States/<platform>/<emulator>/<game>/playtime.json`.
//!
//! `seconds` is this device's cumulative counter (Slot increments it while a
//! core is running with the screen on). `total_seconds` and `devices` are the
//! last merged view written by linkSlot; the frontend only advances `seconds`
//! and `total_seconds`, leaving the device map to the sync tool. Keeping the
//! file inside the unit directory means a renamed ROM pulls the merged total
//! back with the save data, and linkSlot can sync it as one small companion
//! file without touching `meta.json`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "playtime.json";
pub const SCHEMA: u32 = 1;

fn schema_default() -> u32 {
    SCHEMA
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playtime {
    #[serde(default = "schema_default")]
    pub schema: u32,
    #[serde(default)]
    pub seconds: u64,
    #[serde(default)]
    pub total_seconds: u64,
    #[serde(default)]
    pub devices: BTreeMap<String, u64>,
}

impl Playtime {
    pub fn is_empty(&self) -> bool {
        self.seconds == 0 && self.total_seconds == 0 && self.devices.is_empty()
    }
}

pub fn path(unit_dir: &Path) -> PathBuf {
    unit_dir.join(FILE_NAME)
}

/// Human-readable duration for the shelf label. Zero returns `None` so the
/// caller can hide the label instead of printing a meaningless "0".
pub fn format_duration(seconds: u64) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let minutes = seconds / 60;
    if minutes == 0 {
        return Some("不足 1 分钟".to_string());
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    Some(match (hours, minutes) {
        (0, minutes) => format!("{minutes} 分钟"),
        (hours, 0) => format!("{hours} 小时"),
        (hours, minutes) => format!("{hours} 小时 {minutes} 分"),
    })
}

/// Read the local file. Missing, unreadable or malformed files read as zero;
/// an unknown schema is ignored rather than partially interpreted.
pub fn read(unit_dir: &Path) -> Playtime {
    let Ok(bytes) = std::fs::read(path(unit_dir)) else {
        return Playtime::default();
    };
    let Ok(playtime) = serde_json::from_slice::<Playtime>(&bytes) else {
        return Playtime::default();
    };
    if playtime.schema != SCHEMA {
        return Playtime::default();
    }
    playtime
}

/// Add `seconds` to this device's counters and write the file atomically.
/// The device map/total from other devices is preserved; linkSlot reconciles
/// the merged total on the next sync.
pub fn add(unit_dir: &Path, seconds: u64) -> io::Result<()> {
    if seconds == 0 {
        return Ok(());
    }
    let mut playtime = read(unit_dir);
    playtime.schema = SCHEMA;
    playtime.seconds = playtime.seconds.saturating_add(seconds);
    playtime.total_seconds = playtime.total_seconds.saturating_add(seconds);
    write(unit_dir, &playtime)
}

pub fn write(unit_dir: &Path, playtime: &Playtime) -> io::Result<()> {
    std::fs::create_dir_all(unit_dir)?;
    let mut bytes = serde_json::to_vec_pretty(playtime).map_err(io::Error::other)?;
    bytes.push(b'\n');
    slot_store::atomic_write(&path(unit_dir), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "slot-playtime-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_and_bad_files_read_as_zero() {
        let dir = temp_dir();
        assert_eq!(read(&dir), Playtime::default());
        std::fs::write(path(&dir), b"{not json").unwrap();
        assert_eq!(read(&dir), Playtime::default());
        std::fs::write(path(&dir), br#"{"schema":9,"seconds":5}"#).unwrap();
        assert_eq!(read(&dir), Playtime::default());
    }

    #[test]
    fn add_accumulates_and_preserves_the_device_map() {
        let dir = temp_dir();
        let mut seeded = Playtime {
            schema: SCHEMA,
            seconds: 10,
            total_seconds: 30,
            devices: [("dev-a".to_string(), 10), ("dev-b".to_string(), 20)].into(),
        };
        write(&dir, &seeded).unwrap();
        add(&dir, 5).unwrap();
        let got = read(&dir);
        assert_eq!(got.seconds, 15);
        assert_eq!(got.total_seconds, 35);
        assert_eq!(got.devices, seeded.devices);
        seeded.seconds = 15;
        seeded.total_seconds = 35;
        assert_eq!(got, seeded);
    }

    #[test]
    fn duration_text_covers_the_shelf_boundaries() {
        assert_eq!(format_duration(0), None);
        assert_eq!(format_duration(1).as_deref(), Some("不足 1 分钟"));
        assert_eq!(format_duration(59).as_deref(), Some("不足 1 分钟"));
        assert_eq!(format_duration(60).as_deref(), Some("1 分钟"));
        assert_eq!(format_duration(3_599).as_deref(), Some("59 分钟"));
        assert_eq!(format_duration(3_600).as_deref(), Some("1 小时"));
        assert_eq!(format_duration(3_660).as_deref(), Some("1 小时 1 分"));
        assert_eq!(format_duration(90_061).as_deref(), Some("25 小时 1 分"));
    }
}
