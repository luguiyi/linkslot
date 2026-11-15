//! The Famicom's link entry point.
//!
//! Everything about *how* the two ends talk lives in `cable.rs` and `emu.rs`;
//! what is pinned here is that a Famicom cart reaches the link menu at all, on
//! the cart it already has, and without the Advance's hardware picker.

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

fn fc_root(stem: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in slot::root::DIRS {
        std::fs::create_dir_all(d.path().join(sub)).expect("content dir");
    }
    // A `.nes` with no iNES magic is still a cart as far as the shelf is
    // concerned; nothing here runs a core.
    std::fs::write(
        d.path().join(format!("Games/FC/{stem}.nes")),
        vec![0u8; 0x100],
    )
    .expect("write rom");
    d
}

fn playing_fc(root: &Path, stem: &str) -> (App, RadioLog) {
    let mut a = common::app_playing_with_state(
        root,
        stem,
        common::StubSnapshot::boxed(),
        SlotState {
            cart_platform: Some(Platform::Fc),
            ..Default::default()
        },
    );
    // A session does this when it opens the core for a seated cart; the bare
    // app harness has no session, so it is done by hand.
    a.set_platform(Platform::Fc);
    a.set_core(Core::Nes);
    let log = RadioLog::default();
    a.set_radio_jobs(Box::new(log.clone()));
    (a, log)
}

#[test]
fn a_famicom_cart_reaches_the_link_menu() {
    let d = fc_root("Mario");
    let (mut a, log) = playing_fc(d.path(), "Mario");
    assert_eq!(a.platform(), Platform::Fc);

    a.apply(Action::GameMenu);

    assert_eq!(
        a.game_menu(),
        Some(GameMenu::Pick(LinkRow::Host)),
        "an FC cart never reached the link menu"
    );
    assert!(
        log.jobs().contains(&RadioJob::Warm),
        "the link menu opened without warming the radio"
    );
}

#[test]
fn a_famicom_link_swaps_roles_and_cancels_cleanly() {
    let d = fc_root("Mario");
    let (mut a, _log) = playing_fc(d.path(), "Mario");
    a.apply(Action::GameMenu);

    // Left/right swaps host and join, as on every other machine.
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(a.game_menu(), Some(GameMenu::Pick(LinkRow::Join)));

    // B leaves without naming a player or leaving a starter behind.
    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(a.game_menu(), None);
    assert_eq!(a.link_player(), None, "cancelling named a player anyway");
}

fn fc_root_with(stems: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in slot::root::DIRS {
        std::fs::create_dir_all(d.path().join(sub)).expect("content dir");
    }
    for stem in stems {
        std::fs::write(
            d.path().join(format!("Games/FC/{stem}.nes")),
            vec![0u8; 0x100],
        )
        .expect("write rom");
    }
    d
}

/// A Famicom link is a session, not a property of one cart: leaving the game
/// keeps both ends connected so the next cart can be played with the same
/// peer, which is the whole point of the link page.
#[test]
fn a_famicom_link_ends_when_leaving_the_game() {
    let d = fc_root_with(&["Mario", "Contra"]);
    let (mut a, _log) = playing_fc(d.path(), "Mario");
    a.begin_link(0);

    a.apply(Action::Eject);

    assert!(
        !a.link_active(),
        "leaving the game left the host room running"
    );
    assert!(
        !matches!(a.phase(), slot::app::Phase::Playing { .. }),
        "the cart did not leave the slot"
    );
}

/// While linked, MENU is a question about the session: the link page, not the
/// end-of-link prompt (which is what the Advance's cable gets).
#[test]
fn the_link_page_reports_the_session() {
    let d = fc_root_with(&["Mario", "Contra"]);
    let (mut a, _log) = playing_fc(d.path(), "Mario");
    a.begin_link(0);
    a.apply(Action::GbaDown(Btn::Y));

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

/// The joiner is asked before its own game is replaced: the host's new cart
/// is one A press away, and a "no" leaves everything where it was.
#[test]
fn the_joiner_is_asked_before_switching_games() {
    let d = fc_root_with(&["Mario", "Contra"]);
    let (mut a, _log) = playing_fc(d.path(), "Mario");
    a.begin_link(1);

    a.ask_follow("Contra", "Contra", slot_store::Core::Nes.index() as u8);
    let (title, rows, _) = a.utility_text().expect("the question did not open");
    assert!(title.contains("Contra"), "the question names no game: {title}");
    assert!(rows.iter().any(|row| row == "加入这张卡"), "{rows:?}");
    assert!(rows.iter().any(|row| row == "取消"), "{rows:?}");

    // A on the first row accepts.
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(
        a.take_follow_answer(),
        Some(slot::app::FollowAnswer::Accept)
    );

    // The switch is an eject followed by a clean insert of the peer's cart.
    for _ in 0..600 {
        a.update(1.0 / 60.0);
        if a.seated_cart().map(|cart| cart.stem.as_str()) == Some("Contra")
            && matches!(a.phase(), slot::app::Phase::Inserting { .. })
        {
            break;
        }
    }
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Contra"),
        "the follow did not insert the host's cart"
    );
    assert!(a.starting_clean(), "the host's state would not be applied");
    assert!(
        a.link_active(),
        "following must keep the session the cart came through"
    );
}

#[test]
fn a_declined_question_is_a_no_and_nothing_moves() {
    let d = fc_root_with(&["Mario", "Contra"]);
    let (mut a, _log) = playing_fc(d.path(), "Mario");
    a.begin_link(1);
    a.ask_follow("Contra", "Contra", slot_store::Core::Nes.index() as u8);

    // B on the question: the same as 取消.
    a.apply(Action::GbaDown(Btn::B));

    assert_eq!(
        a.take_follow_answer(),
        Some(slot::app::FollowAnswer::Refuse(
            slot::cable::FollowRefusal::Declined
        ))
    );
    assert_eq!(
        a.seated_cart().map(|cart| cart.stem.as_str()),
        Some("Mario"),
        "declining switched the cart anyway"
    );
    assert!(a.link_active());
}

/// what makes "连一次，玩好几张卡" possible. This walks the real path, M →
/// 网络设置 → 联机管理 → 开始联机.
#[test]
fn the_fc_mask_only_applies_to_the_famicom_and_only_when_switched_on() {
    let d = fc_root_with(&["Mario"]);
    std::fs::create_dir_all(d.path().join("Overlay/FC")).unwrap();
    std::fs::write(d.path().join("Overlay/FC/FC01.png"), b"x").unwrap();
    let mut a = common::app_playing_with_state(
        d.path(),
        "Mario",
        common::StubSnapshot::boxed(),
        SlotState {
            cart_platform: Some(Platform::Fc),
            fc_mask: Some("FC01.png".to_string()),
            ..Default::default()
        },
    );
    a.set_platform(Platform::Fc);
    assert!(a.fc_mask_wanted(), "the switch is on but the mask is not wanted");
    a.set_platform(Platform::Gba);
    assert!(
        !a.fc_mask_wanted(),
        "a Game Boy cart must not get the Famicom mask"
    );
}