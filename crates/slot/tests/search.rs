mod common;

use common::{boot, tmp_root_with_carts, write_gb_cart};
use slot::app::{App, KeyboardPurpose, SearchResults};
use slot_input::{Action, Btn};

fn open_search(a: &mut App) {
    a.apply(Action::GbaDown(Btn::X));
    a.apply(Action::GbaUp(Btn::X));
}

fn type_char(a: &mut App, ch: char) {
    let kb = a.keyboard.as_ref().expect("keyboard open");
    let rows = slot_ui::keyboard::rows(kb.cursor.layout);
    let label = ch.to_string();
    let (row, col) = rows
        .iter()
        .enumerate()
        .find_map(|(r, keys)| keys.iter().position(|k| *k == label).map(|c| (r, c)))
        .unwrap_or_else(|| panic!("no key {ch:?}"));
    let layout = kb.cursor.layout;
    a.keyboard.as_mut().unwrap().cursor = slot_ui::keyboard::Cursor { layout, row, col };
    a.apply(Action::GbaDown(Btn::A));
}

fn type_text(a: &mut App, text: &str) {
    for ch in text.chars() {
        type_char(a, ch);
    }
}

#[test]
fn x_opens_the_search_keyboard_and_bkm_finds_a_chinese_game() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    assert!(matches!(
        a.keyboard.as_ref().map(|kb| &kb.purpose),
        Some(KeyboardPurpose::GameSearch)
    ));
    type_text(&mut a, "bkm");
    assert!(
        a.keyboard.as_ref().unwrap().candidates.iter().any(|c| c == "宝可梦"),
        "candidates: {:?}",
        a.keyboard.as_ref().unwrap().candidates
    );
    a.apply(Action::GbaDown(Btn::Start));
    let results = a.search_results.as_ref().expect("search results");
    assert!(results.hits.iter().any(|hit| hit.label == "宝可梦"));
}

#[test]
fn search_results_can_switch_platform_and_select_the_game() {
    let d = tmp_root_with_carts(&["宝可梦"]);
    write_gb_cart(&d, "三国", "三国");
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "sanguo");
    a.apply(Action::GbaDown(Btn::Start));
    let results = a.search_results.as_ref().expect("cross-platform results");
    assert!(results.hits.iter().any(|hit| hit.platform == slot_store::Platform::Gb));
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(a.selected_stem(), Some("三国"));
    assert_eq!(a.shelf_platform_name(), Some("GB"));
}

#[test]
fn a_query_without_matches_keeps_the_keyboard_open() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "zzzz");
    a.apply(Action::GbaDown(Btn::Start));
    assert!(a.search_results.is_none());
    let kb = a.keyboard.as_ref().expect("keyboard stays open");
    assert!(kb.error.as_deref().unwrap_or_default().contains("没有匹配"));
}

#[test]
fn back_from_results_restores_the_query() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "baokemeng");
    a.apply(Action::GbaDown(Btn::Start));
    assert!(a.search_results.is_some());
    a.apply(Action::GbaDown(Btn::B));
    assert!(a.search_results.is_none());
    let kb = a.keyboard.as_ref().expect("keyboard restored");
    assert!(matches!(kb.purpose, KeyboardPurpose::GameSearch));
    assert!(!kb.text.is_empty());
}

#[test]
fn the_candidate_bar_can_commit_a_character() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "bao");
    assert!(!a.keyboard.as_ref().unwrap().candidates.is_empty());
    a.apply(Action::GbaDown(Btn::Select));
    assert!(a.keyboard.as_ref().unwrap().candidate_focus);
    a.apply(Action::GbaDown(Btn::A));
    let kb = a.keyboard.as_ref().unwrap();
    assert!(!kb.text.is_empty(), "no character was committed");
    assert!(kb.composition.is_empty());
    assert!(!kb.candidate_focus);
}

#[test]
fn x_closes_search_and_leaves_a_usable_shelf() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "bao");
    a.apply(Action::GbaDown(Btn::X));
    assert!(a.keyboard.is_none(), "search keyboard stayed open");
    assert!(a.search_index.is_none(), "search index stayed cached");
    assert!(a.search_results.is_none(), "search results stayed open");
    assert!(matches!(a.phase(), slot::app::Phase::Shelf));
    assert!(a.selected_stem().is_some(), "shelf has no selected cart");
}

#[test]
fn search_results_wrap_through_the_keyboard_without_db_panics() {
    let d = tmp_root_with_carts(&["宝可梦", "Fusion"]);
    let mut a = boot(d.path());
    open_search(&mut a);
    type_text(&mut a, "zzzz");
    a.apply(Action::GbaDown(Btn::Start));
    assert!(a.search_results.is_none());
    // Empty results should not be possible from START, but the overlay input
    // must still handle a stale/empty state safely.
    a.search_results = Some(SearchResults {
        query: "x".into(),
        hits: Vec::new(),
        selected: 0,
        offset: 0,
    });
    a.apply(Action::GbaDown(Btn::Down));
    a.apply(Action::GbaDown(Btn::A));
    assert!(a.search_results.is_some(), "empty results must not select");
}
