//! Build identity for the About screen.
//!
//! These values used to be hard-coded, so every package claimed the same old
//! date and commit.  They are now derived from Git and the build clock, with
//! explicit environment overrides for reproducible builds.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or(manifest);

    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/refs/heads").display()
    );
    println!("cargo:rerun-if-changed=build.rs");
    for key in [
        "SLOT_GIT_HASH",
        "SLOT_GIT_DIRTY",
        "SLOT_BUILD_DATE",
        "SOURCE_DATE_EPOCH",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let hash = env("SLOT_GIT_HASH")
        .or_else(|| git(&root, &["rev-parse", "--short=6", "HEAD"]))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = env("SLOT_GIT_DIRTY")
        .map(|value| value == "1")
        .or_else(|| {
            git(&root, &["status", "--porcelain", "--untracked-files=no"])
                .map(|value| !value.is_empty())
        })
        .unwrap_or(false);
    let date = env("SLOT_BUILD_DATE")
        .or_else(|| {
            env("SOURCE_DATE_EPOCH")
                .and_then(|epoch| epoch.parse::<u64>().ok())
                .map(date_from_unix)
        })
        .or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| date_from_unix(duration.as_secs()))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=SLOT_GIT_HASH={hash}");
    println!("cargo:rustc-env=SLOT_GIT_DIRTY={}", u8::from(dirty));
    println!("cargo:rustc-env=SLOT_BUILD_DATE={date}");
}

fn env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn date_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant's `civil_from_days` algorithm; avoids a date dependency in
/// the build script while still handling leap years and century rules.
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
