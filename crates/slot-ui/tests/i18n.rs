//! Nothing the menu draws may still be Chinese in English mode.
//!
//! The per-module tests pin exact copy in Chinese; this one sweeps the surfaces
//! WP1 owns and asserts the English side has no CJK left.  It is deliberately
//! coarse: it cannot see a texture, but every string that becomes one is built
//! from one of these tables.

use slot_ui::conflict::{ConflictDirection, ConflictPage};
use slot_ui::navigation;
use slot_ui::{about_lines, thanks_lines, AboutFields, Language, Toast};

fn is_cjk(c: char) -> bool {
    ('\u{3000}'..='\u{303f}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
}

fn fields() -> AboutFields<'static> {
    AboutFields {
        serial: "ABCDEF0",
        date: "2026-11-10 12:00",
        dirty: false,
        device: "Anbernic RG34XX SP",
    }
}

#[test]
fn english_toasts_are_not_chinese() {
    for toast in Toast::all() {
        let text = toast.text(Language::EnUs);
        assert!(!text.chars().any(is_cjk), "{toast:?} is still Chinese: {text}");
        if let Some(detail) = toast.detail(Language::EnUs) {
            assert!(
                !detail.chars().any(is_cjk),
                "{toast:?} detail is still Chinese: {detail}"
            );
        }
        // The Chinese side must keep working, too.
        assert!(!toast.text(Language::ZhCn).is_empty(), "{toast:?}");
    }
}

#[test]
fn english_conflict_pages_are_not_chinese() {
    for page in [ConflictPage::All, ConflictPage::State, ConflictPage::Sav] {
        for text in [page.title(Language::EnUs), page.tab(Language::EnUs)] {
            assert!(!text.chars().any(is_cjk), "{page:?} is still Chinese: {text}");
        }
    }
    for direction in [ConflictDirection::Download, ConflictDirection::Upload] {
        let text = direction.label(Language::EnUs);
        assert!(
            !text.chars().any(is_cjk),
            "{direction:?} is still Chinese: {text}"
        );
    }
}

#[test]
fn english_about_and_thanks_pages_are_not_chinese() {
    // Names are proper nouns and stay as they are; only the prose is checked.
    let names = [
        "@bilibili 泡面loading",
        "@Bilibili仲冬临沧的十一月",
        "@bilibili CrazySix",
    ];
    for line in about_lines(&fields(), Language::EnUs)
        .into_iter()
        .chain(thanks_lines(Language::EnUs))
    {
        if names.iter().any(|name| line.contains(name)) {
            continue;
        }
        assert!(
            !line.chars().any(is_cjk),
            "the English About page is still Chinese: {line}"
        );
    }
}

#[test]
fn english_help_lines_are_not_chinese() {
    let lines = navigation::help(Language::EnUs);
    assert_eq!(lines.len(), navigation::help(Language::ZhCn).len());
    for line in lines {
        assert!(
            !line.chars().any(is_cjk),
            "a help line is still Chinese: {line}"
        );
    }
    // And the Chinese help has not been emptied by the move.
    assert!(navigation::help(Language::ZhCn)
        .iter()
        .all(|line| !line.is_empty()));
}
