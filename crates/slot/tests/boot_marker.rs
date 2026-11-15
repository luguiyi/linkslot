//! `linkSlot/Config/last-run.txt` is the "was it playing when it died?" marker.

mod common;

use common::{app_booting_at, tmp_root_with_carts, CLOCK_IS_SET};
use slot::linkcfg;

const MARKER: &str = linkcfg::LAST_RUN_REL;
const PLAYING_LINKED: &str = "cart=Emerald\nlinked=1\n";

#[test]
fn a_marker_left_by_a_crashed_run_is_reported_and_cleared() {
    let d = tmp_root_with_carts(&["Emerald"]);
    linkcfg::write_last_run(d.path(), PLAYING_LINKED);
    let (app, _) = app_booting_at(d.path(), CLOCK_IS_SET);

    let note = app.previous_run_note().expect("the stale marker was ignored");
    assert!(note.contains("cart=Emerald"), "{note}");
    assert!(note.contains("linked=1"), "{note}");

    let log = std::fs::read_to_string(d.path().join("linkSlot/Logs/linkSlot.log"))
        .expect("the boot did not write the link log");
    assert!(
        log.contains("previous run ended unexpectedly (cart=Emerald, linked=1)"),
        "{log}"
    );
    assert!(
        !d.path().join(MARKER).exists(),
        "the stale marker survived the boot"
    );
}

#[test]
fn a_clean_boot_has_nothing_to_report() {
    let d = tmp_root_with_carts(&["Emerald"]);
    let (app, _) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert_eq!(app.previous_run_note(), None);
    assert!(!d.path().join(MARKER).exists());
}

#[test]
fn a_clean_restart_removes_the_marker() {
    let d = tmp_root_with_carts(&["Emerald"]);
    let mut app = slot::app::App::boot(d.path());
    linkcfg::write_last_run(d.path(), "cart=Emerald\nlinked=0\n");
    app.restart();
    assert!(!d.path().join(MARKER).exists(), "restart kept the marker");
}

#[test]
fn a_clean_power_off_removes_the_marker() {
    let d = tmp_root_with_carts(&["Emerald"]);
    let mut app = slot::app::App::boot(d.path());
    linkcfg::write_last_run(d.path(), PLAYING_LINKED);
    app.poweroff();
    assert!(!d.path().join(MARKER).exists(), "power-off kept the marker");
}

#[test]
fn the_marker_helpers_round_trip_and_treat_blank_as_absent() {
    let d = tmp_root_with_carts(&["Emerald"]);
    assert_eq!(linkcfg::read_last_run(d.path()), None);
    linkcfg::write_last_run(d.path(), "linked=1\n");
    assert_eq!(linkcfg::read_last_run(d.path()).as_deref(), Some("linked=1"));
    linkcfg::write_last_run(d.path(), "  \n");
    assert_eq!(linkcfg::read_last_run(d.path()), None);
    linkcfg::clear_last_run(d.path());
    assert_eq!(linkcfg::read_last_run(d.path()), None);
}
