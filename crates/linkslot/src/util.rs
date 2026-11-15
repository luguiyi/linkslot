//! Small time/formatting helpers shared by the CLI.
//!
//! The timestamp format is the one Slot uses for ring states
//! (`YYYY-MM-DD_HH-MM-SS`, 19 characters). Conflict copies reuse the same
//! format so the on-card layout stays consistent.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static SEQ: AtomicU64 = AtomicU64::new(0);

const DAY: i64 = 86_400;

/// Seconds since the Unix epoch (0 if the clock is before 1970).
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `<stamp>` for "now": `YYYY-MM-DD_HH-MM-SS` in UTC.
pub fn stamp_now() -> String {
    format_stamp(now_secs())
}

/// `YYYY-MM-DD_HH-MM-SS` in UTC (the Slot ring state stamp format).
pub fn format_stamp(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(DAY));
    let rem = secs.rem_euclid(DAY);
    format!(
        "{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// UTC ISO-8601 timestamp used by `meta.json` (`2026-10-20T10:00:00Z`).
pub fn now_iso8601() -> String {
    iso8601(now_secs())
}

pub fn iso8601(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(DAY));
    let rem = secs.rem_euclid(DAY);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// True when `s` is a valid 19-character Slot ring stamp.
pub fn is_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'_'
        || b[13] != b'-'
        || b[16] != b'-'
    {
        return false;
    }
    let field = |from: usize, to: usize| -> Option<i64> {
        let text = s.get(from..to)?;
        text.bytes().all(|c| c.is_ascii_digit()).then_some(())?;
        text.parse().ok()
    };
    let (y, m, d) = (field(0, 4).unwrap_or(0), field(5, 7), field(8, 10));
    let (Some(m), Some(d), Some(hh), Some(mm), Some(ss)) =
        (m, d, field(11, 13), field(14, 16), field(17, 19))
    else {
        return false;
    };
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) || hh > 23 || mm > 59 || ss > 59 {
        return false;
    }
    true
}

/// A short token unique enough for temporary directory names.
pub fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}{seq:x}", std::process::id(), nanos)
}

/// Age of a file's modification time in seconds (`None` if unknown).
pub fn mtime_age_secs(path: &Path) -> Option<u64> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    let now = SystemTime::now();
    if mtime > now {
        return Some(0);
    }
    now.duration_since(mtime)
        .ok()
        .map(|d: Duration| d.as_secs())
}

/// Newest modification time among `paths`, as a UTC ISO-8601 timestamp
/// (`2026-10-20T10:00:00Z`). Used for `LINKSLOT_LOCAL_UPDATED` in conflict
/// reports; `None` when no file has a readable modification time.
pub fn newest_mtime_iso<'a, I>(paths: I) -> Option<String>
where
    I: IntoIterator<Item = &'a Path>,
{
    paths
        .into_iter()
        .filter_map(|path| {
            std::fs::metadata(path)
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs() as i64)
        })
        .max()
        .map(iso8601)
}

pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y.rem_euclid(4) == 0 && (y.rem_euclid(100) != 0 || y.rem_euclid(400) == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        // 2026-10-20T10:00:00Z
        let secs = 1_792_490_400;
        assert_eq!(format_stamp(secs), "2026-10-20_10-00-00");
        assert_eq!(iso8601(secs), "2026-10-20T10:00:00Z");
        assert!(is_stamp(&format_stamp(secs)));
    }

    #[test]
    fn rejects_bad_stamps() {
        assert!(!is_stamp(""));
        assert!(!is_stamp("2026-10-20_10-00-0"));
        assert!(!is_stamp("2026-13-20_10-00-00"));
        assert!(!is_stamp("2026-02-30_10-00-00"));
        assert!(!is_stamp("2026-10-20T10:00:00"));
        assert!(!is_stamp("2026-10-20_24-00-00"));
        assert!(is_stamp("2024-02-29_00-00-00"));
        assert!(!is_stamp("2023-02-29_00-00-00"));
    }

    #[test]
    fn unique_suffix_is_unique() {
        let a = unique_suffix();
        let b = unique_suffix();
        assert_ne!(a, b);
    }

    #[test]
    fn newest_mtime_iso_reports_latest_file() {
        let root = std::env::temp_dir().join(format!("linkslot-util-{}", unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.state");
        let b = root.join("b.state");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let now = SystemTime::now();
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&a) {
            let _ = file.set_modified(now - Duration::from_secs(3600));
        }
        let paths = [a.as_path(), b.as_path()];
        let iso = newest_mtime_iso(paths).expect("mtime available");
        assert_eq!(iso.len(), 20, "iso was {iso}");
        assert!(iso.contains('T') && iso.ends_with('Z'), "iso was {iso}");
        assert!(newest_mtime_iso([root.join("missing.state").as_path()]).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
