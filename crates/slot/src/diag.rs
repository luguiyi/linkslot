//! Small append-only diagnostics channel for the device UI.
//!
//! The Rust UI and the linkSlot CLI share `Logs/linkSlot.log` and
//! `linkSlot/Config/log_level.txt`; the Web service reads that log tail.  This
//! module deliberately keeps the file format compatible with `linkslot`:
//!
//! ```text
//! 2026-10-22T23:40:00Z [debug] [wifi] scan: 8 networks
//! ```

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::linkcfg::LogLevel;

const LOG_REL: &str = "linkSlot/Logs/linkSlot.log";
static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn enabled(root: &Path, level: LogLevel) -> bool {
    crate::linkcfg::read_log_level(root).allows(level)
}

pub fn write(root: &Path, level: LogLevel, category: &str, message: impl AsRef<str>) {
    if !enabled(root, level) {
        return;
    }
    let path = root.join(LOG_REL);
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let line = format!(
        "{} [{}] [{}] {}\n",
        now_iso8601(),
        level.as_str(),
        category,
        one_line(message.as_ref())
    );
    let _guard = LOG_LOCK.lock().ok();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn one_line(message: &str) -> String {
    message
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .take(2000)
        .collect()
}

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_lines_follow_the_shared_level_file() {
        let root = std::env::temp_dir().join(format!("slot-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        crate::linkcfg::write_log_level(&root, LogLevel::Info).unwrap();
        assert!(enabled(&root, LogLevel::Info));
        assert!(!enabled(&root, LogLevel::Debug));
        write(&root, LogLevel::Info, "wifi", "info line");
        write(&root, LogLevel::Debug, "wifi", "debug hidden");
        let text = std::fs::read_to_string(root.join(LOG_REL)).unwrap();
        assert!(text.contains("[info] [wifi] info line"));
        assert!(!text.contains("debug hidden"));

        crate::linkcfg::write_log_level(&root, LogLevel::Debug).unwrap();
        assert!(enabled(&root, LogLevel::Debug));
        write(&root, LogLevel::Debug, "wifi", "debug shown");
        let text = std::fs::read_to_string(root.join(LOG_REL)).unwrap();
        assert!(text.contains("[debug] [wifi] debug shown"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
