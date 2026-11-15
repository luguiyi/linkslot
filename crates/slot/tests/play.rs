mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{boot, tmp_root_with_carts, tmp_root_with_gb_carts};
use slot::app::Phase;
use slot::persist;
use slot::session::Session;
use slot_input::{Action, Btn, RawEvent};
use slot_store::{read_slot_state, write_slot_state, Core, Platform, ResumeKind, StateRing};

#[test]
fn a_tap_resumes_and_a_hold_starts_clean() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald")
        .write_resume(&[7u8; 64])
        .unwrap();

    let mut tap = boot(d.path());
    tap.apply_at(Action::GbaDown(Btn::A), 0);
    tap.apply_at(Action::GbaUp(Btn::A), 120);
    assert!(
        matches!(tap.phase(), Phase::Inserting { .. }),
        "a tap did not put the cart in: {:?}",
        tap.phase()
    );
    assert!(!tap.starting_clean(), "a tap threw the state away");

    let mut hold = boot(d.path());
    hold.apply_at(Action::GbaDown(Btn::A), 0);
    hold.tick_ms(500);
    assert!(hold.starting_clean(), "holding A still resumed");
}

#[test]
fn a_clean_start_leaves_the_state_on_disk() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let r = StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald");
    r.write_resume(&[7u8; 64]).unwrap();
    let mut a = boot(d.path());
    a.apply_at(Action::GbaDown(Btn::A), 0);
    a.tick_ms(500);
    assert_eq!(
        r.read_resume().unwrap(),
        Some(vec![7u8; 64]),
        "the state was deleted"
    );
}

#[test]
fn the_clean_start_fires_on_the_threshold_not_the_release() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = boot(d.path());
    a.apply_at(Action::GbaDown(Btn::A), 0);
    a.tick_ms(499);
    assert!(matches!(a.phase(), Phase::Shelf), "it seated early");
    a.tick_ms(500);
    assert!(
        matches!(a.phase(), Phase::Inserting { .. }),
        "nothing happened on the hold: {:?}",
        a.phase()
    );
}

#[test]
fn the_release_after_a_hold_is_not_another_press() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = boot(d.path());
    a.apply_at(Action::GbaDown(Btn::A), 0);
    a.tick_ms(500);
    a.apply_at(Action::GbaUp(Btn::A), 700);
    assert!(a.starting_clean(), "the release resumed the cart after all");
}

fn counter_after(root: &Path, hold: bool) -> u64 {
    common::clocked(root);
    let mut s = Session::boot(root.to_path_buf());
    s.feed([RawEvent::Down(Btn::A)], 16);
    if !hold {
        s.feed([RawEvent::Up(Btn::A)], 32);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(s.app().phase(), Phase::Playing { .. }) {
        assert!(Instant::now() < deadline, "the cart never seated");
        s.update(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(1));
    }
    s.app_mut().tick_ms(60_000);
    s.app_mut().settle_saves();
    let state = persist::read_resume(root, Platform::Gba, Core::Mgba, "Emerald")
        .expect("nothing was flushed");
    u64::from_le_bytes(state.try_into().expect("the mock's state is 8 bytes"))
}

#[test]
fn a_hold_hands_the_core_no_state_and_a_tap_hands_it_the_resume() {
    let tapped = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let held = tmp_root_with_carts(&["Emerald", "Fusion"]);
    for d in [&tapped, &held] {
        persist::flush(
            d.path(),
            Platform::Gba,
            Core::Mgba,
            "Emerald",
            Some(&500_000u64.to_le_bytes()),
            None,
        )
        .unwrap();
    }
    assert!(
        counter_after(tapped.path(), false) >= 500_000,
        "a tap started the cart cold"
    );
    assert!(
        counter_after(held.path(), true) < 500_000,
        "a hold resumed the state it was meant to skip"
    );
}

#[test]
fn a_platform_with_load_off_starts_clean_on_a_tap() {
    let d = tmp_root_with_gb_carts(&["Tetris"]);
    StateRing::new(d.path(), Platform::Gb, Core::Mgba, "Tetris")
        .write_resume(&[7u8; 64])
        .unwrap();
    let mut switches = read_slot_state(d.path());
    switches.set_resume(Platform::Gb, ResumeKind::Load, Some(false));
    write_slot_state(d.path(), &switches).unwrap();

    let mut tap = boot(d.path());
    tap.apply_at(Action::GbaDown(Btn::A), 0);
    tap.apply_at(Action::GbaUp(Btn::A), 120);
    assert!(
        matches!(tap.phase(), Phase::Inserting { .. }),
        "the cart never went in: {:?}",
        tap.phase()
    );
    assert!(
        tap.starting_clean(),
        "GB opted out of loading, but the tap still resumed"
    );
}

#[test]
fn a_bad_platform_switch_leaves_the_others_alone() {
    let d = tmp_root_with_carts(&["Emerald"]);
    StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald")
        .write_resume(&[7u8; 64])
        .unwrap();
    let mut switches = read_slot_state(d.path());
    switches.set_resume(Platform::Gb, ResumeKind::Load, Some(false));
    write_slot_state(d.path(), &switches).unwrap();

    let mut tap = boot(d.path());
    tap.apply_at(Action::GbaDown(Btn::A), 0);
    tap.apply_at(Action::GbaUp(Btn::A), 120);
    assert!(
        !tap.starting_clean(),
        "GBA stopped resuming because GB opted out"
    );
}
