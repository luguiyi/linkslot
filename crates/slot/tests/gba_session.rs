//! The mGBA machines (GBA / GB / GBC) use the same lockstep cable as the
//! Famicom, so they get the same session: one socket that outlives a cart.
//!
//! gpSP does not: its link is libretro netpacket, and `link_player` — the mark
//! of a cable session — is never set for it.

mod common;

use slot::app::{FollowAnswer, Phase};
use slot_input::{Action, Btn};
use slot_store::Platform;
use slot_ui::Language;

#[test]
fn a_gba_link_session_ends_when_leaving_the_game() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");

    a.begin_link(0);
    assert_eq!(a.link_player(), Some(0));
    assert!(a.cable_session(), "a mGBA link is not a cable session");

    a.apply(Action::Eject);

    assert!(!a.link_active(), "leaving the game kept the host room alive");
    assert!(!matches!(a.phase(), Phase::Playing { .. }));
}

#[test]
fn a_gba_joiner_is_asked_before_switching_games() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.begin_link(1);

    a.ask_follow("Emerald", "Emerald", slot_store::Core::Mgba.index() as u8);
    let (title, rows, _) = a.utility_text().expect("the question did not open");
    assert!(title.contains("Emerald"), "{title}");
    assert!(rows.iter().any(|row| row == "加入这张卡"), "{rows:?}");

    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(a.take_follow_answer(), Some(FollowAnswer::Accept));

    for _ in 0..600 {
        a.update(1.0 / 60.0);
        if a.seated_cart().map(|cart| cart.stem.as_str()) == Some("Emerald")
            && matches!(a.phase(), Phase::Inserting { .. })
        {
            break;
        }
    }
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Emerald"),
        "the follow did not insert the host's GBA cart"
    );
    assert!(a.link_active());
}

#[test]
fn a_gpsp_link_ends_with_the_game() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.set_core(slot_store::Core::Gpsp);
    a.begin_link(0);
    assert!(!a.cable_session(), "gpSP does not run on the lockstep cable");

    a.apply(Action::Eject);
    assert!(!a.link_active(), "the game's end left the socket running");
}

#[test]
fn only_a_cart_whose_core_can_link_is_announced() {
    // A title the wireless heuristic recognises (see `link_kind::pokemon`).
    let d = common::tmp_root_with_carts(&["Fusion", "POKEMON Emerald"]);

    // A plain cart links on the cable core and never on gpSP: gpSP only
    // emulates the wireless adapter.
    let mut plain = common::app_playing_in(d.path(), "Fusion");
    plain.set_core(slot_store::Core::Mgba);
    assert!(plain.game_can_link(), "a cable cart on mGBA must be linkable");
    plain.set_core(slot_store::Core::Gpsp);
    assert!(!plain.game_can_link(), "a plain cart must not link on gpSP");

    // A Pokémon card is a wireless cart: gpSP can carry it.
    let mut wireless = common::app_playing_in(d.path(), "POKEMON Emerald");
    wireless.set_core(slot_store::Core::Gpsp);
    assert!(wireless.game_can_link(), "Pokémon cannot link on gpSP");
}

#[test]
fn following_a_cart_adopts_the_hosts_core() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.set_core(slot_store::Core::Mgba);
    a.begin_link(0);

    a.ask_follow("Emerald", "Emerald", slot_store::Core::Gpsp.index() as u8);
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(a.take_follow_answer(), Some(FollowAnswer::Accept));
    for _ in 0..600 {
        a.update(1.0 / 60.0);
        if a.seated_cart().map(|cart| cart.stem.as_str()) == Some("Emerald")
            && matches!(a.phase(), Phase::Inserting { .. })
        {
            break;
        }
    }
    let chosen = slot_store::read_selected_cores(d.path());
    assert_eq!(
        chosen.get("Emerald"),
        Some(&slot_store::Core::Gpsp),
        "the joiner kept its own core instead of the host's"
    );
}

#[test]
fn only_the_host_is_told_that_the_peer_arrived_or_is_expected() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);

    // The host sees both the wait and the arrival.
    let mut host = common::app_playing_in(d.path(), "Fusion");
    host.begin_link(0);
    host.link_waiting_peer();
    assert_eq!(
        host.toast().map(|toast| toast.text(Language::ZhCn)),
        Some("等待对方加入…"),
        "the host is not told that it is waiting"
    );
    host.link_joined();
    assert_eq!(
        host.toast().map(|toast| toast.text(Language::ZhCn)),
        Some("对方已加入"),
        "the host is not told when the peer joins"
    );

    // The joiner answered its own question; it gets neither.
    let mut joiner = common::app_playing_in(d.path(), "Fusion");
    joiner.begin_link(1);
    joiner.link_joined();
    assert_eq!(
        joiner.toast().map(|toast| toast.text(Language::ZhCn)),
        None,
        "the joiner was told that it joined itself"
    );
}

#[test]
fn a_peer_cart_is_found_by_content_not_by_name() {
    let d = common::tmp_root_with_carts(&["Fusion", "Emerald"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    let emerald = d.path().join("Games/GBA/Emerald.gba");
    let hash = slot::rom_id::digest_file(&emerald).unwrap();
    assert_eq!(
        a.find_follow_cart(Platform::Gba.index() as u8, 0, hash),
        Some("Emerald".to_string()),
        "the same cart under another name was not found"
    );
    assert_eq!(a.find_follow_cart(Platform::Gba.index() as u8, 0, [0x34u8; 32]), None);
    // A Famicom cart cannot satisfy a Game Boy request.
    assert_eq!(
        a.find_follow_cart(Platform::Fc.index() as u8, 0, hash),
        None,
        "a GBA rom matched a Famicom request"
    );
}

#[test]
fn the_joiner_takes_the_hosts_clock_on_connect() {
    let d = common::tmp_root_with_carts(&["Fusion"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.begin_link(1);

    a.apply_clock_sync(1_782_000_000, 480);

    assert_eq!(a.utc_offset_min(), 480, "the offset did not move");
    assert_eq!(
        a.toast().map(|toast| toast.text(Language::ZhCn)),
        Some("时间已与主机同步"),
        "the joiner was not told"
    );
    // It is persisted, so the next boot keeps the host's time.
    let state = slot_store::read_slot_state(d.path());
    assert!(state.clock_set);
    assert_eq!(state.utc_offset_min, 480);
    // And the host's payload is what would be sent.
    assert_eq!(a.clock_sync_payload().1, 480);
}
#[test]
fn a_cart_on_another_shelf_is_switched_to_and_started() {
    let d = common::tmp_root_with_carts(&["Fusion"]);
    // The peer's game lives on the Famicom shelf, while this device is sitting
    // on the GBA shelf: the follow must find it by name on its own shelf.
    let fc = d.path().join("Games/FC");
    std::fs::create_dir_all(&fc).expect("fc dir");
    let mut rom = vec![0u8; 0x4000];
    rom[..4].copy_from_slice(b"NES\x1a");
    std::fs::write(fc.join("Contra.nes"), rom).expect("write fc cart");

    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.begin_link(1);

    assert!(a.follow_game("Contra", slot_store::Core::Nes.index() as u8).is_ok());

    for _ in 0..600 {
        a.update(1.0 / 60.0);
        if a.seated_cart().map(|cart| cart.stem.as_str()) == Some("Contra")
            && matches!(a.phase(), Phase::Inserting { .. })
        {
            break;
        }
    }
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Contra"),
        "the follow did not switch to the Famicom shelf and start the cart"
    );
}
#[test]
fn the_joiner_is_told_when_the_host_restarts() {
    let d = common::tmp_root_with_carts(&["Fusion"]);
    let mut a = common::app_playing_in(d.path(), "Fusion");
    a.begin_link(1);
    a.link_restarted();
    assert_eq!(
        a.toast().map(|toast| toast.text(Language::ZhCn)),
        Some("主机重新开始了游戏"),
        "the joiner was not told about the restart"
    );

    // The host never shows it: it is the one doing the restarting.
    let d2 = common::tmp_root_with_carts(&["Fusion"]);
    let mut host = common::app_playing_in(d2.path(), "Fusion");
    host.begin_link(0);
    host.link_restarted();
    assert_eq!(host.toast(), None);
}