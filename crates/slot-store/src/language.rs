//! `linkSlot/Config/language.txt`: the one source of truth for the UI language.
//!
//! Kept out of `slot.state` on purpose:
//!
//! * the system menu, the Web page and `linkSlot` can all read and write a
//!   three-byte file without parsing or rewriting the whole state file;
//! * a language change from the Web page does not have to round-trip through
//!   the device's in-memory state;
//! * the existing `slot.state` parser and its tests stay untouched.
//!
//! The file holds one code, `zh-CN` or `en-US`.  Anything else — a missing
//! file, an empty file, a half-written file, a language this build does not
//! ship — reads as Simplified Chinese, which is also the build's source
//! language, so a broken card still shows a usable menu.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use slot_i18n::Language;

/// Relative to the card root.
pub const LANGUAGE_FILE: &str = "linkSlot/Config/language.txt";

pub fn language_path(root: &Path) -> PathBuf {
    root.join(LANGUAGE_FILE)
}

/// The saved language, or Chinese when the file is missing or unreadable.
///
/// A card the device cannot read from must not stop it from booting, so every
/// failure is deliberate here: no error, just the default.
pub fn read_language(root: &Path) -> Language {
    read_language_code(root).unwrap_or_default()
}

/// [`read_language`] without the fallback, for callers that want to tell
/// "explicitly Chinese" from "nothing on the card" (the Web API reports the
/// difference in its debug output).
pub fn read_language_code(root: &Path) -> Option<Language> {
    let text = std::fs::read_to_string(language_path(root)).ok()?;
    // Notepad and friends happily prepend a BOM; it is not part of the code.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Language::parse(text)
}

/// Writes the language, creating `Config/` if the card does not have it yet.
///
/// Atomic: a card pulled mid-write keeps the previous code rather than a
/// truncated one.  The error string is user-facing (it reaches a notice on the
/// device and the Web page), so it names the file rather than an OS code.
pub fn write_language(root: &Path, language: Language) -> Result<(), String> {
    let path = language_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 Config 目录：{error}"))?;
    }
    let body = format!("{}\n", language.code());
    crate::atomic_write(&path, body.as_bytes())
        .map_err(|error| format!("无法写入 {}：{error}", LANGUAGE_FILE))
}

/// When `linkSlot/Config/language.txt` last changed, or `None` if it does not exist.
///
/// The device polls this once a second so a language set from the Web page
/// reaches the menu without a reboot; the poll must stay this cheap, which is
/// why it is a `metadata` call and not a read.
pub fn language_mtime(root: &Path) -> Option<SystemTime> {
    std::fs::metadata(language_path(root))
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_missing_file_reads_as_chinese() {
        let dir = tempdir().unwrap();
        assert_eq!(read_language(dir.path()), Language::ZhCn);
        assert_eq!(read_language_code(dir.path()), None);
        assert_eq!(language_mtime(dir.path()), None);
    }

    #[test]
    fn an_empty_or_unknown_file_falls_back_to_chinese() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        for body in ["", "   \n", "fr-FR\n", "english\n"] {
            std::fs::write(language_path(dir.path()), body).unwrap();
            assert_eq!(
                read_language(dir.path()),
                Language::ZhCn,
                "{body:?} should not select a language"
            );
        }
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_a_valid_code() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        std::fs::write(language_path(dir.path()), "\u{feff}en-US\r\n").unwrap();
        assert_eq!(read_language(dir.path()), Language::EnUs);
    }

    #[test]
    fn the_aliases_from_the_plan_all_resolve() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("linkSlot/Config")).unwrap();
        for (body, want) in [
            ("zh\n", Language::ZhCn),
            ("zh-CN\n", Language::ZhCn),
            ("zh_CN\n", Language::ZhCn),
            ("en\n", Language::EnUs),
            ("en-US\n", Language::EnUs),
            ("en_US\n", Language::EnUs),
            ("  EN-us  \n", Language::EnUs),
        ] {
            std::fs::write(language_path(dir.path()), body).unwrap();
            assert_eq!(read_language(dir.path()), want, "{body:?}");
        }
    }

    #[test]
    fn a_written_language_reads_back_and_leaves_no_temp_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // `Config/` does not exist yet: writing must create it.
        write_language(root, Language::EnUs).unwrap();
        assert_eq!(read_language(root), Language::EnUs);
        assert_eq!(
            std::fs::read_to_string(language_path(root)).unwrap(),
            "en-US\n"
        );
        write_language(root, Language::ZhCn).unwrap();
        assert_eq!(read_language(root), Language::ZhCn);
        let leftovers: Vec<_> = std::fs::read_dir(root.join("linkSlot/Config"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "language.txt")
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    #[test]
    fn a_writable_language_file_has_a_mtime_and_it_moves() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_language(root, Language::EnUs).unwrap();
        let first = language_mtime(root).expect("a written file has an mtime");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_language(root, Language::ZhCn).unwrap();
        let second = language_mtime(root).expect("still there");
        assert!(second >= first);
    }

    #[test]
    fn an_unwritable_card_reports_an_error_instead_of_panicking() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // A *file* named `linkSlot` makes `create_dir_all` fail on every platform,
        // which is the portable stand-in for a read-only card.
        std::fs::write(root.join("linkSlot"), b"not a directory").unwrap();
        let error = write_language(root, Language::EnUs).unwrap_err();
        assert!(error.contains("Config"), "{error}");
        // The in-memory choice is the caller's problem; reading still answers.
        assert_eq!(read_language(root), Language::ZhCn);
    }
}
