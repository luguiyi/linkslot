mod common;

use slot_ui::LinkBadge;
use slot::app::ShelfSync;

/// A dropped peer used to freeze the picture behind a modal. It is now a card
/// under the status bar that ends on its own, so play never stops.
#[test]
fn a_dropped_joiner_gets_a_notice_not_a_dialog() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut app = common::app_playing_in(d.path(), "Emerald");
    // client id 1 is the joiner
    app.begin_link(1);
    app.peer_lost();

    assert!(
        !app.link_popup_open(),
        "a dropped peer opened a dialog again"
    );
    let (title, detail, _) = app.notice_view().expect("no notice was raised");
    assert_eq!(title, "联机已断开");
    assert!(detail.contains("可继续游戏"), "{detail}");

    // The notice doubles as the reconnect window: still linked right now...
    assert!(app.link_active(), "the session ended before the grace window");
    // ...and solo once it is gone.
    app.update(5.1);
    assert!(!app.link_active(), "the joiner stayed in a dead session");
}

#[test]
fn the_host_keeps_the_room_and_only_gets_a_notice() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut app = common::app_playing_in(d.path(), "Emerald");
    app.begin_link(0);
    app.peer_lost();

    assert!(app.link_active(), "the host room ended with the peer");
    assert_eq!(app.link_badge(), LinkBadge::HostingLost);
    let (title, _, _) = app.notice_view().expect("no notice was raised");
    assert_eq!(title, "等待对方加入");
}

#[test]
fn a_notice_fades_away_after_five_seconds() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    let mut app = common::app_playing_in(d.path(), "Emerald");
    app.begin_link(0);
    app.peer_lost();
    assert!(app.notice_view().is_some());

    app.update(5.1);
    assert!(
        app.notice_view().is_none(),
        "the notice outlived its five seconds"
    );
}

#[test]
fn any_key_dismisses_a_notice_without_swallowing_the_press() {
    let d = common::tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut app = common::app_playing_in(d.path(), "Emerald");
    app.begin_link(0);
    app.peer_lost();
    assert!(app.notice_view().is_some());

    app.apply(slot_input::Action::GbaDown(slot_input::Btn::Down));
    assert!(
        app.notice_view().is_none(),
        "the press did not dismiss the notice"
    );
    assert!(
        matches!(app.shelf_sync(), ShelfSync::Idle),
        "the game state was left untouched by the notice"
    );
}
