use slot_input::{Action, Btn, Gestures, RawEvent};
use slot_store::{ButtonLayout as Layout, SlotState, read_slot_state, write_slot_state};
#[path = "../../../crates/slot/src/input/remap.rs"]
mod remap;
#[path = "../../../crates/slot/src/bootlogo.rs"]
#[allow(dead_code)]
mod bootlogo;

pub fn run() {
    for (mode, key, press, release) in [
        (Layout::Both, Btn::L1, Action::RewindStart, Action::RewindStop),
        (Layout::Both, Btn::R1, Action::FfStart, Action::FfStop),
        (Layout::Both, Btn::L2, Action::GbaDown(Btn::L1), Action::GbaUp(Btn::L1)),
        (Layout::Both, Btn::R2, Action::GbaDown(Btn::R1), Action::GbaUp(Btn::R1)),
        (Layout::Both, Btn::X, Action::GbaDown(Btn::L1), Action::GbaUp(Btn::L1)),
        (Layout::Both, Btn::Y, Action::GbaDown(Btn::R1), Action::GbaUp(Btn::R1)),
        (Layout::SwapShoulders, Btn::L1, Action::RewindStart, Action::RewindStop),
        (Layout::SwapShoulders, Btn::R1, Action::FfStart, Action::FfStop),
        (Layout::SwapShoulders, Btn::L2, Action::GbaDown(Btn::L1), Action::GbaUp(Btn::L1)),
        (Layout::SwapShoulders, Btn::R2, Action::GbaDown(Btn::R1), Action::GbaUp(Btn::R1)),
        (Layout::XyShoulders, Btn::X, Action::GbaDown(Btn::L1), Action::GbaUp(Btn::L1)),
        (Layout::XyShoulders, Btn::Y, Action::GbaDown(Btn::R1), Action::GbaUp(Btn::R1)),
        (Layout::Off, Btn::X, Action::GbaDown(Btn::X), Action::GbaUp(Btn::X)),
        (Layout::Off, Btn::L2, Action::RewindStart, Action::RewindStop),
    ] {
        let mut r = remap::Remap::default(); let mut g = Gestures::new();
        let ev = r.feed(RawEvent::Down(key), mode).unwrap();
        assert!(g.feed(ev, 1000).contains(&press), "press {mode:?} {key:?}");
        // Releasing after opening a menu/changing modes must release the old binding.
        let ev = r.feed(RawEvent::Up(key), Layout::Off).unwrap();
        assert!(g.feed(ev, 1400).contains(&release), "release {mode:?} {key:?}");
    }
    let mut r=remap::Remap::default();
    assert_eq!(r.feed(RawEvent::Down(Btn::X), Layout::XyShoulders),Some(RawEvent::Down(Btn::L1)));
    assert_eq!(r.feed(RawEvent::Down(Btn::L1), Layout::XyShoulders),None);
    assert_eq!(r.feed(RawEvent::Up(Btn::X), Layout::Off),None);
    assert_eq!(r.feed(RawEvent::Up(Btn::L1), Layout::Off),Some(RawEvent::Up(Btn::L1)));
    assert_eq!(r.feed(RawEvent::Up(Btn::Lid), Layout::Off),Some(RawEvent::Up(Btn::Lid)));
    for (a,b,target) in [(Btn::X,Btn::L2,Btn::L1),(Btn::Y,Btn::R2,Btn::R1)] {
        for (first,second) in [(a,b),(b,a)] {
            for (release_first,release_last) in [(a,b),(b,a)] {
                let mut r=remap::Remap::default();
                assert_eq!(r.feed(RawEvent::Down(first),Layout::Both),Some(RawEvent::Down(target)));
                assert_eq!(r.feed(RawEvent::Down(second),Layout::Both),None);
                assert_eq!(r.feed(RawEvent::Up(release_first),Layout::Both),None,"other physical key still held");
                assert_eq!(r.feed(RawEvent::Up(release_last),Layout::Off),Some(RawEvent::Up(target)));
            }
        }
    }
    let dir=std::path::Path::new("../qa-card");
    for mode in [Layout::Off,Layout::SwapShoulders,Layout::XyShoulders,Layout::Both] {
        let mut s=SlotState::default();s.button_layout=mode;s.turbo=!mode.uses_xy();
        write_slot_state(dir,&s).unwrap();assert_eq!(read_slot_state(dir),s);
        assert_eq!(mode.step(true).step(false),mode);
        if mode.uses_xy() {
            let state=dir.join("linkSlot/Config/slot.state");
            let text=std::fs::read_to_string(&state).unwrap();
            std::fs::write(&state,text.replace("turbo=0","turbo=1")).unwrap();
            assert!(!read_slot_state(dir).turbo,"XY and Both must suppress stale turbo");
            write_slot_state(dir,&s).unwrap();
        }
    }
    let state=dir.join("linkSlot/Config/slot.state");
    let text=std::fs::read_to_string(&state).unwrap();
    std::fs::write(&state,text.replace("turbo=0","turbo=1")).unwrap();
    assert!(!read_slot_state(dir).turbo,"XY mode suppresses stale turbo settings");
    std::fs::write(&state,text.lines().filter(|l|!l.starts_with("button_layout=")).collect::<Vec<_>>().join("\n")).unwrap();
    assert_eq!(read_slot_state(dir).button_layout,Layout::Off,"old cards retain default mapping");
    let bytes=std::fs::read("../../card/System/linkSlot/assets/bootlogo.bmp").unwrap();
    let mark=bootlogo::decode(&bytes).unwrap();assert_eq!((mark.width,mark.height),(450,200));
    for panel in [(640,480),(480,640)] {
        let logo=bootlogo::compose(&mark,panel).unwrap();
        assert_eq!((logo.width,logo.height),panel);
        assert!(logo.rgb.iter().any(|c|*c==[255,255,255]));
        assert_eq!(bootlogo::size(&bootlogo::encode(&logo)),Some(panel));
    }
    println!("PASS R4: all six combined-mode keys; both shared-L/R press/release orders; mode-change releases; all four settings persist; legacy settings; XY/Both turbo exclusion; unchanged bootlogo");
}
