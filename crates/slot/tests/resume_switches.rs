mod common;

use common::{app_playing_with_state, tmp_root_with_carts, StubSnapshot};
use slot_input::Action;
use slot_store::{read_slot_state, Core, Platform, ResumeKind, SlotState, StateRing};

fn save_off() -> SlotState {
    let mut state = SlotState::default();
    state.set_resume(Platform::Gba, ResumeKind::Save, Some(false));
    state
}

#[test]
fn an_eject_with_save_off_writes_the_battery_but_not_the_resume() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = app_playing_with_state(
        d.path(),
        "Emerald",
        StubSnapshot::boxed_with_sav(b"save-ram"),
        save_off(),
    );

    a.apply(Action::Eject);

    let ring = StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald");
    assert!(
        ring.read_resume().unwrap().is_none(),
        "eject wrote resume.state although GBA opted out"
    );
    assert_eq!(
        std::fs::read(d.path().join("Saves/GBA/Emerald.sav")).unwrap(),
        b"save-ram",
        "the battery save must still be written on eject"
    );
    assert_eq!(
        read_slot_state(d.path()).cart,
        None,
        "the slot never cleared"
    );
}

#[test]
fn a_gb_switch_never_touches_a_gba_cart() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut switches = SlotState::default();
    switches.set_resume(Platform::Gb, ResumeKind::Save, Some(false));
    let mut a = app_playing_with_state(d.path(), "Emerald", StubSnapshot::boxed(), switches);

    a.apply(Action::Eject);

    let ring = StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald");
    assert!(
        ring.read_resume().unwrap().is_some(),
        "disabling GB saving also disabled the GBA cart"
    );
}

#[test]
fn turning_save_off_never_deletes_an_existing_resume() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let ring = StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald");
    ring.write_resume(&[7u8; 64]).unwrap();

    let mut a = app_playing_with_state(d.path(), "Emerald", StubSnapshot::boxed(), save_off());
    a.apply(Action::Eject);

    assert_eq!(
        ring.read_resume().unwrap(),
        Some(vec![7u8; 64]),
        "save=off must not delete the old state (off is not erase)"
    );
}


#[test]
fn an_eject_writes_the_resume_thumbnail_for_the_cover() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = app_playing_with_state(
        d.path(),
        "Emerald",
        StubSnapshot::boxed(),
        SlotState::default(),
    );

    a.apply(Action::Eject);

    let thumb = d.path().join("States/GBA/mgba/Emerald/resume.png");
    assert_eq!(
        std::fs::read(&thumb).expect("long M exit wrote no resume.png"),
        b"png",
        "the resume screenshot must be the snapshot thumbnail"
    );
}

#[test]
fn save_off_does_not_leave_a_stale_thumbnail_without_a_resume() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = app_playing_with_state(
        d.path(),
        "Emerald",
        StubSnapshot::boxed(),
        save_off(),
    );

    a.apply(Action::Eject);

    assert!(
        !d.path().join("States/GBA/mgba/Emerald/resume.png").exists(),
        "save=off wrote a thumbnail without a resume state"
    );
}


#[test]
fn a_real_cover_skips_only_the_resume_thumbnail() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let labels = d.path().join("Labels/GBA");
    std::fs::create_dir_all(&labels).unwrap();
    std::fs::write(labels.join("Emerald.png"), b"real cover").unwrap();

    let mut a = app_playing_with_state(
        d.path(),
        "Emerald",
        StubSnapshot::boxed(),
        SlotState::default(),
    );
    a.apply(Action::Eject);

    let ring = StateRing::new(d.path(), Platform::Gba, Core::Mgba, "Emerald");
    assert!(
        ring.read_resume().unwrap().is_some(),
        "the resume state must still be written"
    );
    assert!(
        !d.path().join("States/GBA/mgba/Emerald/resume.png").exists(),
        "a game with a real cover does not need a fallback thumbnail"
    );
}
