//! The Mega Drive joins the same one-console/two-pad cable as the Famicom
//! and Super Famicom. This is not Sega Net Link: genesis_plus_gx has no
//! serial/netpacket interface, so the session runs one MD and two pads.

mod common;

use slot::app::App;
use slot_store::{Core, Platform, SlotState};

fn playing_md(stem: &str) -> (tempfile::TempDir, App) {
    let d = common::tmp_root_with_md_carts(&[stem]);
    let mut a = common::app_playing_with_state(
        d.path(),
        stem,
        common::StubSnapshot::boxed(),
        SlotState {
            cart_platform: Some(Platform::Md),
            ..Default::default()
        },
    );
    // A real session does this when it opens the cart's core.
    a.set_platform(Platform::Md);
    a.set_core(Core::Gpgx);
    (d, a)
}

#[test]
fn md_uses_the_single_machine_cable() {
    let (_d, mut a) = playing_md("Sonic");

    assert_eq!(a.platform(), Platform::Md);
    assert!(a.single_machine_link(), "MD is not on the two-pad cable");
    assert!(a.game_can_link(), "MD still refuses to link");

    a.begin_link(0);
    assert_eq!(a.link_player(), Some(0));
    assert!(
        a.cable_session(),
        "MD did not enter the lockstep cable session"
    );
}

#[test]
fn md_and_another_machine_do_not_agree() {
    let hello = |platform: Platform| slot::cable::Hello {
        version: slot::cable::HELLO_VERSION,
        platform: platform.index() as u8,
        core: slot_store::Core::Gpgx.index() as u8,
        rom: [7; 32],
        device: [0; slot::cable::HELLO_DEVICE],
    };
    let md = hello(Platform::Md);
    let fc = hello(Platform::Fc);
    assert!(!md.agrees(&fc), "MD accepted a Famicom peer");
    assert!(!fc.agrees(&md), "the Famicom accepted an MD peer");
    assert!(md.agrees(&hello(Platform::Md)));
}
