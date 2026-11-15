//! The in-game menu that one press of M opens.
//!
//! A flat seven-row list behind `App::utility`. What is pinned here is the list
//! itself — order, one page — and what its bottom two rows actually do, since
//! both of them end the running session.

mod common;

use std::path::Path;

use slot::app::{App, GameMenu, LinkRow, Phase};
use slot_input::{Action, Btn};
use slot_store::{Binding, Core, Device, Platform, Shortcut, ShortcutKeys, SlotState};

fn playing(root: &Path, stem: &str) -> App {
    common::app_playing_in(root, stem)
}

fn open_menu(app: &mut App) -> Vec<String> {
    app.apply(Action::QuickMenu);
    let (title, rows, _) = app
        .utility_text()
        .expect("the game menu did not open on M");
    assert_eq!(title, "游戏菜单");
    rows
}

fn down(app: &mut App, times: usize) {
    for _ in 0..times {
        app.apply(Action::GbaDown(Btn::Down));
    }
}

fn step_until(app: &mut App, what: &str, mut done: impl FnMut(&App) -> bool) {
    for _ in 0..600 {
        if done(app) {
            return;
        }
        app.update(1.0 / 60.0);
    }
    panic!("{what} never happened");
}

#[test]
fn the_menu_lists_seven_rows_on_one_page() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");

    let rows = open_menu(&mut a);

    assert_eq!(
        rows,
        [
            "即时存档相册",
            "金手指",
            "查看攻略",
            "联机",
            "快捷键一览",
            "重新开始游戏",
            "退出游戏",
        ],
        "the game menu changed shape"
    );
    assert_eq!(rows.len(), 7, "the game menu must stay on one screen");
    assert_eq!(
        a.utility_rows_per_page(),
        7,
        "the panel paged the seven rows"
    );
}

#[test]
fn a_txt_guide_opens_from_the_fixed_guide_row() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    std::fs::create_dir_all(d.path().join("Guides/GBA")).unwrap();
    std::fs::write(d.path().join("Guides/GBA/Emerald.txt"), "攻略").unwrap();
    let mut a = playing(d.path(), "Emerald");

    let rows = open_menu(&mut a);
    let at = rows
        .iter()
        .position(|row| row == "查看攻略")
        .expect("the guide row was missing");
    assert_eq!(at, 2, "the guide row replaced 快捷键一览");
    down(&mut a, at);
    a.apply(Action::GbaDown(Btn::A));
    assert!(a.guide_open(), "A on the guide row did not open the reader");
}

/// A guide long enough to need several pages, so a bookmark has somewhere to go.
fn write_long_guide(root: &Path) {
    std::fs::create_dir_all(root.join("Guides/GBA")).unwrap();
    let mut text = String::new();
    for line in 1..=40 {
        text.push_str(&format!("第 {line} 行：攻略内容。\n"));
    }
    std::fs::write(root.join("Guides/GBA/Emerald.txt"), text).unwrap();
}

fn open_guide(app: &mut App) {
    let on_menu = app
        .utility_text()
        .map(|(title, _, _)| title == "游戏菜单")
        .unwrap_or(false);
    if !on_menu {
        open_menu(app);
    }
    let rows = app.utility_text().map(|(_, rows, _)| rows).unwrap_or_default();
    let at = rows
        .iter()
        .position(|row| row == "查看攻略")
        .expect("the guide row was missing");
    let selected = app.utility_text().map(|(_, _, at)| at).unwrap_or(0);
    down(app, at.saturating_sub(selected));
    app.apply(Action::GbaDown(Btn::A));
    assert!(app.guide_open(), "the guide did not open");
}

#[test]
fn a_guide_reopens_on_the_page_it_was_closed_on() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");
    write_long_guide(d.path());

    open_guide(&mut a);
    let (_, _, first, pages) = a.guide_page().expect("no page");
    assert_eq!(first, 0, "the first read did not start at the top");
    assert!(pages >= 3, "the fixture is not long enough: {pages}");

    down(&mut a, 2);
    let (_, _, page, _) = a.guide_page().unwrap();
    assert_eq!(page, 2);
    a.apply(Action::GbaDown(Btn::B));
    assert!(!a.guide_open(), "B did not close the reader");

    // Reopening from the menu resumes instead of rewinding.
    open_guide(&mut a);
    let (_, _, resumed, _) = a.guide_page().unwrap();
    assert_eq!(resumed, 2, "the reader forgot the page it was closed on");
}

#[test]
fn start_takes_the_guide_back_to_the_first_page_and_sticks() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");
    write_long_guide(d.path());

    open_guide(&mut a);
    down(&mut a, 3);
    assert_eq!(a.guide_page().map(|(_, _, page, _)| page), Some(3));
    a.apply(Action::GbaDown(Btn::Start));
    assert_eq!(a.guide_page().map(|(_, _, page, _)| page), Some(0));
    a.apply(Action::GbaDown(Btn::B));

    open_guide(&mut a);
    assert_eq!(
        a.guide_page().map(|(_, _, page, _)| page),
        Some(0),
        "从头看 was not saved"
    );
}

#[test]
fn a_shorter_guide_clamps_the_remembered_page() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");
    write_long_guide(d.path());

    open_guide(&mut a);
    down(&mut a, 4);
    a.apply(Action::GbaDown(Btn::B));

    // The guide is replaced by a one-page file: the bookmark must not overrun.
    std::fs::write(d.path().join("Guides/GBA/Emerald.txt"), "短攻略").unwrap();
    open_guide(&mut a);
    let (_, _, page, pages) = a.guide_page().unwrap();
    assert_eq!(pages, 1);
    assert_eq!(page, 0, "the bookmark overran a shorter guide");
}

#[test]
fn the_guide_row_is_always_shown_and_says_when_there_is_none() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");

    let rows = open_menu(&mut a);
    let at = rows
        .iter()
        .position(|row| row == "查看攻略")
        .expect("the guide row was hidden without a guide");
    assert_eq!(at, 2);
    down(&mut a, at);
    a.apply(Action::GbaDown(Btn::A));

    assert!(!a.guide_open(), "a missing guide opened a reader");
    let (title, rows, at) = a
        .utility_text()
        .expect("the missing guide did not raise a modal");
    assert_eq!(title, "暂无攻略");
    assert!(a.utility_is_notice(), "the missing guide is not a notice");
    assert_eq!(at, 0, "a notice should not carry a cursor");
    assert!(
        rows.iter().any(|row| row == "当前游戏还没有可读取的攻略。"),
        "the modal did not explain the missing guide: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row == "知道了"),
        "a notice should not fake a dismissal row: {rows:?}"
    );

    // Any dismiss key returns, because there is nothing to choose.
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(
        a.utility_text().map(|(title, _, _)| title),
        Some("游戏菜单".to_string()),
        "A did not return from the missing-guide notice"
    );
}

#[test]
fn the_missing_guide_modal_also_returns_with_b() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");
    let rows = open_menu(&mut a);
    let at = rows.iter().position(|row| row == "查看攻略").unwrap();
    down(&mut a, at);
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(
        a.utility_text().map(|(title, _, _)| title),
        Some("暂无攻略".to_string())
    );

    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(
        a.utility_text().map(|(title, _, _)| title),
        Some("游戏菜单".to_string()),
        "B did not return from the missing-guide modal"
    );
}

#[test]
fn game_shortcuts_show_only_in_game_actions() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");

    let rows = open_menu(&mut a);
    let at = rows
        .iter()
        .position(|row| row == "快捷键一览")
        .expect("the shortcut row was missing");
    down(&mut a, at);
    a.apply(Action::GbaDown(Btn::A));

    let (title, rows, _) = a.utility_text().expect("the game shortcut page did not open");
    assert_eq!(title, "游戏快捷键");
    assert!(
        rows.iter().any(|row| row == "即时存档：SEL+R1"),
        "the live default binding was not shown: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row == "快进（按住；双击锁定）：R2"),
        "the fast-forward binding was not shown: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.starts_with("主界面")),
        "shelf-only help leaked into the in-game page: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("云同步")),
        "cloud help leaked into the in-game page: {rows:?}"
    );

    a.apply(Action::GbaDown(Btn::B));
    let (title, _, _) = a.utility_text().expect("B closed the whole panel");
    assert_eq!(title, "游戏菜单", "B did not return to the game menu");
}

#[test]
fn game_shortcuts_read_the_users_remapping() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut keys = ShortcutKeys::default();
    keys.set(
        Shortcut::SaveState,
        Binding::plain(Device::Y),
    );
    slot_store::write_shortcuts(d.path(), &keys).expect("write shortcut overrides");

    let mut a = playing(d.path(), "Emerald");
    let rows = open_menu(&mut a);
    let at = rows
        .iter()
        .position(|row| row == "快捷键一览")
        .expect("the shortcut row was missing");
    down(&mut a, at);
    a.apply(Action::GbaDown(Btn::A));

    let (_, rows, _) = a.utility_text().expect("the game shortcut page did not open");
    assert!(
        rows.iter().any(|row| row == "即时存档：Y"),
        "the custom binding was ignored: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| row != "即时存档：SEL+R1"),
        "the factory binding was still shown after a remap: {rows:?}"
    );
}

#[test]
fn the_selection_wraps_from_the_last_row_back_to_the_first() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    open_menu(&mut a);

    for _ in 0..6 {
        a.apply(Action::GbaDown(Btn::Down));
    }
    assert_eq!(a.utility_text().map(|(_, _, at)| at), Some(6));

    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(
        a.utility_text().map(|(_, _, at)| at),
        Some(0),
        "the last row did not wrap round to the first"
    );
}

#[test]
fn exit_game_returns_to_the_shelf_and_keeps_the_cart_loadable() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    open_menu(&mut a);
    down(&mut a, 6);

    a.apply(Action::GbaDown(Btn::A));

    assert!(
        matches!(a.phase(), Phase::Ejecting { .. }),
        "退出游戏 did not start an eject"
    );
    step_until(&mut a, "the eject", |app| {
        matches!(app.phase(), Phase::Shelf)
    });
    assert!(
        a.seated_cart().is_none(),
        "the cart stayed seated after 退出游戏"
    );
}

#[test]
fn a_tap_on_restart_reopens_the_cart_from_the_top_in_place() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    open_menu(&mut a);
    down(&mut a, 5);

    a.apply(Action::GbaDown(Btn::A));
    a.apply(Action::GbaUp(Btn::A));

    // The same cart and the same page stay: no eject animation, no shelf.
    assert!(
        matches!(a.phase(), Phase::Playing { .. }),
        "a tap on 重新开始游戏 left the current page: {:?}",
        a.phase()
    );
    assert!(a.starting_clean(), "the restart did not ask for a clean core");
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Emerald")
    );
}

#[test]
fn restart_comes_back_clean_instead_of_resuming() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    open_menu(&mut a);
    down(&mut a, 5);

    a.apply(Action::GbaDown(Btn::A));

    // No trip through the shelf: the same page is still up, and the fresh
    // core is marked clean so the resume state the restart just wrote is not
    // read.
    assert!(
        a.starting_clean(),
        "the cart came back from its own resume state"
    );
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Emerald"),
        "it re-inserted another cart"
    );
    assert!(
        matches!(a.phase(), Phase::Playing { .. }),
        "the restart left the current page"
    );
}

#[test]
fn a_single_cart_card_can_still_restart() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");
    open_menu(&mut a);
    down(&mut a, 5);

    a.apply(Action::GbaDown(Btn::A));
    assert!(
        matches!(a.phase(), Phase::Playing { .. }) && a.starting_clean(),
        "a one-cart card refused to restart itself"
    );
}

#[test]
fn link_while_linked_opens_the_page_and_ending_it_asks_first() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    a.begin_link(0);
    open_menu(&mut a);
    down(&mut a, 3);

    a.apply(Action::GbaDown(Btn::A));

    assert!(
        a.link_active(),
        "联机 dropped a running link without asking"
    );
    let (title, rows, _) = a
        .utility_text()
        .expect("联机 while linked did not open the link page");
    assert_eq!(title, "联机管理");
    assert!(rows.iter().any(|row| row == "断开联机"), "{rows:?}");
    assert!(
        !a.link_popup_open(),
        "the page raised the end question by itself"
    );

    // 断开联机 is still a question, not an instant drop.
    let stop = rows
        .iter()
        .position(|row| row == "断开联机")
        .expect("断开联机 row");
    // The page opens at the status row; walk down to 断开联机.
    let at = a.utility_text().map(|(_, _, at)| at).unwrap_or(0);
    down(&mut a, stop - at);
    a.apply(Action::GbaDown(Btn::A));
    assert!(
        a.link_active() && a.link_popup_open(),
        "断开联机 dropped the peer without a confirmation"
    );
}

#[test]
fn link_from_the_menu_opens_the_link_screen() {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in slot::root::DIRS {
        std::fs::create_dir_all(d.path().join(sub)).expect("content dir");
    }
    std::fs::write(
        d.path().join("Games/FC/Mario.nes"),
        vec![0u8; 0x100],
    )
    .expect("write rom");
    let mut a = common::app_playing_with_state(
        d.path(),
        "Mario",
        common::StubSnapshot::boxed(),
        SlotState {
            cart_platform: Some(Platform::Fc),
            ..Default::default()
        },
    );
    a.set_platform(Platform::Fc);
    a.set_core(Core::Nes);

    open_menu(&mut a);
    down(&mut a, 3);
    a.apply(Action::GbaDown(Btn::A));

    assert_eq!(a.utility_text(), None, "the panel stayed open over the link");
    assert_eq!(
        a.game_menu(),
        Some(GameMenu::Pick(LinkRow::Host)),
        "联机 did not reach the link screen"
    );
    assert!(
        matches!(a.phase(), Phase::Playing { .. }),
        "the link screen opened over a phase other than the game"
    );

    // Under the link screen MENU is not "leave the game": the prompt must stay
    // away so a hold there cannot promise an exit that screen swallows.
    a.apply(Action::QuickMenu);
    a.update(0.5);
    assert_eq!(
        a.hold_exit(),
        None,
        "the exit prompt ran under the link screen"
    );
}

// ---- the hold-to-leave prompt ------------------------------------------------

#[test]
fn a_tap_never_flashes_the_exit_prompt() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");

    a.apply(Action::QuickMenu);
    assert!(
        a.hold_exit().is_none(),
        "the prompt appeared the instant MENU went down"
    );

    // A tap's worth of holding is still inside the grace window.
    a.update(0.15);
    assert!(
        a.hold_exit().is_none(),
        "the prompt appeared during a tap"
    );

    a.apply(Action::MenuUp);
    a.update(1.0);
    assert_eq!(a.hold_exit(), None, "the prompt outlived the release");
}

#[test]
fn holding_menu_fills_the_prompt_and_the_release_cancels_it() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");

    a.apply(Action::QuickMenu);
    a.update(0.35);
    let early = a.hold_exit().expect("no prompt after the grace window");
    a.update(0.30);
    let later = a.hold_exit().expect("the prompt vanished mid-hold");
    assert!(
        later > early,
        "the bar did not fill: {early} then {later}"
    );
    assert!(later < 1.0, "the bar filled before the hold did");

    a.apply(Action::MenuUp);
    assert_eq!(a.hold_exit(), None, "the release did not cancel");
    assert!(
        matches!(a.phase(), Phase::Playing { .. }),
        "cancelling left the game"
    );
    assert!(
        a.utility_text().is_some(),
        "the menu the press opened was lost on cancel"
    );
}

#[test]
fn the_eject_at_the_end_of_the_hold_takes_the_prompt_with_it() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");

    a.apply(Action::QuickMenu);
    a.update(0.95);
    assert!(a.hold_exit().is_some(), "the prompt was not up by the end");

    // The gesture layer sends this at one second; the app only has to get out
    // of its way.
    a.apply(Action::Eject);

    assert!(
        matches!(a.phase(), Phase::Ejecting { .. }),
        "the hold did not leave the game"
    );
    assert_eq!(a.hold_exit(), None, "the prompt outlived the eject");
}

#[test]
fn a_one_cart_card_never_promises_an_exit_it_would_refuse() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut a = playing(d.path(), "Emerald");

    a.apply(Action::QuickMenu);
    a.update(0.6);
    assert!(
        a.hold_exit().is_none(),
        "the prompt offered an exit a one-cart card refuses"
    );
}

#[test]
fn the_shelf_menu_has_no_exit_prompt() {
    let d = common::tmp_root_with_carts(&["Emerald", "Ruby"]);
    let mut a = playing(d.path(), "Emerald");
    a.apply(Action::Eject);
    step_until(&mut a, "the shelf", |app| {
        matches!(app.phase(), Phase::Shelf)
    });

    a.apply(Action::QuickMenu);
    a.update(1.0);

    assert!(
        matches!(a.phase(), Phase::QuickMenu { .. }),
        "M did not open the settings menu on the shelf"
    );
    assert_eq!(
        a.hold_exit(),
        None,
        "the shelf offered to leave a game it is not playing"
    );
}

