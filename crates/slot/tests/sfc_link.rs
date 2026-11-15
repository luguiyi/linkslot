//! The Super Famicom's link entry point.
//!
//! Everything about *how* the two ends talk lives in `cable.rs` and `emu.rs`;
//! what is pinned here is that an SFC cart reaches the link menu at all, on the
//! cart it already has, and without the Advance's hardware picker. The SFC runs
//! the same single-machine cable as the Famicom.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use slot::app::{App, GameMenu, LinkRow};
use slot::link_radio::{RadioJob, RadioJobs};
use slot_input::{Action, Btn};
use slot_store::{Core, Platform, SlotState};

#[derive(Clone, Default)]
struct RadioLog(Arc<Mutex<Vec<RadioJob>>>);

impl RadioLog {
    fn jobs(&self) -> Vec<RadioJob> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl RadioJobs for RadioLog {
    fn ask(&mut self, job: RadioJob) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(job);
    }

    fn warmed(&self) -> bool {
        false
    }
}

fn sfc_root(stems: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in slot::root::DIRS {
        std::fs::create_dir_all(d.path().join(sub)).expect("content dir");
    }
    for stem in stems {
        // No SFC header magic is needed: nothing here runs a core.
        std::fs::write(
            d.path().join(format!("Games/SFC/{stem}.sfc")),
            vec![0u8; 0x100],
        )
        .expect("write rom");
    }
    d
}

fn playing_sfc(root: &Path, stem: &str) -> (App, RadioLog) {
    let mut a = common::app_playing_with_state(
        root,
        stem,
        common::StubSnapshot::boxed(),
        SlotState {
            cart_platform: Some(Platform::Sfc),
            ..Default::default()
        },
    );
    // A session does this when it opens the core for a seated cart; the bare
    // app harness has no session, so it is done by hand.
    a.set_platform(Platform::Sfc);
    a.set_core(Core::Snes);
    let log = RadioLog::default();
    a.set_radio_jobs(Box::new(log.clone()));
    (a, log)
}

#[test]
fn an_sfc_cart_reaches_the_link_menu() {
    let d = sfc_root(&["Mario"]);
    let (mut a, log) = playing_sfc(d.path(), "Mario");
    assert_eq!(a.platform(), Platform::Sfc);
    assert!(a.single_machine_link(), "the SFC is not on the cable path");
    assert!(a.game_can_link(), "the SFC link was reported unavailable");

    a.apply(Action::GameMenu);

    assert_eq!(
        a.game_menu(),
        Some(GameMenu::Pick(LinkRow::Host)),
        "an SFC cart never reached the link menu"
    );
    assert!(
        log.jobs().contains(&RadioJob::Warm),
        "the link menu opened without warming the radio"
    );
}

#[test]
fn an_sfc_link_swaps_roles_and_cancels_cleanly() {
    let d = sfc_root(&["Mario"]);
    let (mut a, _log) = playing_sfc(d.path(), "Mario");
    a.apply(Action::GameMenu);

    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(a.game_menu(), Some(GameMenu::Pick(LinkRow::Join)));

    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(a.game_menu(), None);
    assert_eq!(a.link_player(), None, "cancelling named a player anyway");
}

/// The in-game menu's 「联机」 row is the same door as SELECT+M: it must no
/// longer read 「联机（暂不支持）」 and pressing it must open the link menu.
#[test]
fn the_sfc_game_menu_link_row_opens_the_link_menu() {
    let d = sfc_root(&["Mario"]);
    let (mut a, _log) = playing_sfc(d.path(), "Mario");

    a.apply(Action::QuickMenu);
    let (title, rows, _) = a.utility_text().expect("the game menu did not open");
    assert_eq!(title, "游戏菜单");
    assert!(
        rows.iter().any(|row| row == "联机"),
        "the menu lost its link row: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("暂不支持")),
        "the SFC link row is still marked unavailable: {rows:?}"
    );

    // Rows run 即时存档相册 / 金手指 / 查看攻略 / 联机: three downs reach it.
    for _ in 0..3 {
        a.apply(Action::GbaDown(Btn::Down));
    }
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(
        a.game_menu(),
        Some(GameMenu::Pick(LinkRow::Host)),
        "the menu's link row did not open the link menu"
    );
}

/// An SFC link is a session on the single-machine cable: a console with two
/// pads, not a pair of handhelds.
#[test]
fn an_sfc_link_runs_on_the_single_machine_cable() {
    let d = sfc_root(&["Mario"]);
    let (mut a, _log) = playing_sfc(d.path(), "Mario");

    a.begin_link(1);

    assert_eq!(a.link_player(), Some(1));
    assert!(a.cable_session(), "an SFC link is not a cable session");
    assert!(a.game_can_link());
}

/// The joiner's local card is searched by the Super Famicom platform too:
/// following the host into an SFC game must not fall into the "different
/// machine" branch just because the platform was appended after FC.
#[test]
fn a_peer_sfc_cart_is_found_on_the_local_card() {
    let d = sfc_root(&["Mario", "Contra"]);
    // Different bytes: the host's Contra is not the same file as Mario.
    std::fs::write(d.path().join("Games/SFC/Contra.sfc"), vec![1u8; 0x200])
        .expect("write contra");
    let (mut a, _log) = playing_sfc(d.path(), "Mario");
    let contra = d.path().join("Games/SFC/Contra.sfc");
    let hash = slot::rom_id::digest_file(&contra).expect("hash contra");

    assert_eq!(
        a.find_follow_cart(Platform::Sfc.index() as u8, Core::Snes.index() as u8, hash),
        Some("Contra".to_string()),
        "the joiner could not find the host's SFC cart by content"
    );
    assert_eq!(
        a.find_follow_cart_named(
            Platform::Sfc.index() as u8,
            Core::Snes.index() as u8,
            hash,
            "Contra"
        ),
        Some("Contra".to_string()),
        "the joiner could not find the host's SFC cart by name"
    );
    assert_eq!(
        a.find_follow_cart(Platform::Fc.index() as u8, Core::Nes.index() as u8, hash),
        None,
        "an SFC rom matched a Famicom request"
    );
}

/// While linked, MENU is a question about the session: the link page, not the
/// end-of-link prompt (which is what the Advance's cable gets).
#[test]
fn the_sfc_link_page_reports_the_session() {
    let d = sfc_root(&["Mario", "Contra"]);
    let (mut a, _log) = playing_sfc(d.path(), "Mario");
    a.begin_link(0);

    a.apply(Action::GameMenu);

    let (title, rows, _) = a.utility_text().expect("the link page did not open");
    assert_eq!(title, "联机管理");
    assert!(
        rows.iter().any(|row| row.contains("主机")),
        "the page does not say this device is the host: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row == "断开联机"),
        "a linked session offers no way out: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row == "开始联机"),
        "a linked session still offers to start another: {rows:?}"
    );
}
