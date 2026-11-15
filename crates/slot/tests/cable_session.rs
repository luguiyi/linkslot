mod common;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slot::audio::Ring;
use slot::cable::Pairing;
use slot::emu::{CoreState, EmuHandle, Speed};
use slot_retro::{LinkChannel, MockCore};

#[derive(Default)]
struct Wire {
    a: VecDeque<Vec<u8>>,
    b: VecDeque<Vec<u8>>,
}

struct End {
    wire: Arc<Mutex<Wire>>,
    first: bool,
}

impl LinkChannel for End {
    fn send(&mut self, _flags: i32, buf: &[u8]) {
        let mut w = self.wire.lock().unwrap();
        match self.first {
            true => w.a.push_back(buf.to_vec()),
            false => w.b.push_back(buf.to_vec()),
        }
    }
    fn try_recv(&mut self) -> Option<Vec<u8>> {
        let mut w = self.wire.lock().unwrap();
        match self.first {
            true => w.b.pop_front(),
            false => w.a.pop_front(),
        }
    }
}

fn spawn(ring: Arc<Ring>) -> EmuHandle {
    let emu = EmuHandle::spawn(
        Box::new(MockCore::new()),
        PathBuf::from("mock"),
        ring,
        None,
        None,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while emu.state() == CoreState::Loading {
        assert!(Instant::now() < deadline, "the core never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
    emu
}

#[test]
fn a_cable_session_steps_both_consoles() {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let host = spawn(Arc::new(Ring::new(4096)));
    let join = spawn(Arc::new(Ring::new(4096)));

    host.begin_cable(
        0,
        Box::new(End {
            wire: wire.clone(),
            first: true,
        }),
        Pairing::PairedGba,
    );
    join.begin_cable(
        1,
        Box::new(End {
            wire: wire.clone(),
            first: false,
        }),
        Pairing::PairedGba,
    );
    host.set_speed(Speed::Normal);
    join.set_speed(Speed::Normal);

    let deadline = Instant::now() + Duration::from_secs(5);
    while host.linked_frames() < 30 || join.linked_frames() < 30 {
        assert!(
            Instant::now() < deadline,
            "the pair stalled at host {} / joiner {} linked frames",
            host.linked_frames(),
            join.linked_frames()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_joiner_runs_the_hosts_machine_and_not_its_own() {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let host = spawn(Arc::new(Ring::new(4096)));
    let join = spawn(Arc::new(Ring::new(4096)));

    join.set_speed(Speed::Normal);
    let deadline = Instant::now() + Duration::from_secs(5);
    while join.published_count() < 20 {
        assert!(Instant::now() < deadline, "the joiner never ran alone");
        std::thread::sleep(Duration::from_millis(10));
    }
    join.set_speed(Speed::Paused);

    host.begin_cable(
        0,
        Box::new(End {
            wire: wire.clone(),
            first: true,
        }),
        Pairing::PairedGba,
    );
    join.begin_cable(
        1,
        Box::new(End {
            wire: wire.clone(),
            first: false,
        }),
        Pairing::PairedGba,
    );
    host.set_speed(Speed::Normal);
    join.set_speed(Speed::Normal);

    let deadline = Instant::now() + Duration::from_secs(10);
    while join.linked_frames() < 20 {
        assert!(
            Instant::now() < deadline,
            "the joiner never started: {} linked frames",
            join.linked_frames()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_peer_that_never_speaks_stalls_rather_than_guessing() {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let lonely = spawn(Arc::new(Ring::new(4096)));
    lonely.begin_cable(
        0,
        Box::new(End {
            wire: wire.clone(),
            first: true,
        }),
        Pairing::PairedGba,
    );
    lonely.set_speed(Speed::Normal);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !lonely.link_lost() {
        assert!(
            Instant::now() < deadline,
            "a silent peer was never reported lost; linked frames {}",
            lonely.linked_frames()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        lonely.linked_frames() <= slot::cable::DELAY,
        "it ran {} frames with nobody on the other end",
        lonely.linked_frames()
    );
}

/// A single-machine hello: `platform`/`core` are the `slot-store`
/// discriminants. Famicom is 3/2, Super Famicom 4/3.
fn single(platform: u8, core: u8, rom: [u8; 32]) -> Pairing {
    Pairing::SingleMachine(slot::cable::Hello {
        version: slot::cable::HELLO_VERSION,
        platform,
        core,
        rom,
        device: [0; slot::cable::HELLO_DEVICE],
    })
}

/// One console, two pads: every machine the cable carries this way.
const SINGLE_MACHINES: [(u8, u8, &str); 3] = [
    (3, 2, "Famicom"),
    (4, 3, "Super Famicom"),
    (5, 4, "Mega Drive"),
];

/// The single-machine path end to end: two devices, one machine, each holding
/// the other's pad. No peer core and no BIOS — just the same state on both
/// ends. The Famicom and the Super Famicom share this path.
#[test]
fn a_single_machine_pair_runs_both_pads_on_one_machine() {
    for (platform, core, label) in SINGLE_MACHINES {
        let wire = Arc::new(Mutex::new(Wire::default()));
        let host = spawn(Arc::new(Ring::new(4096)));
        let join = spawn(Arc::new(Ring::new(4096)));

        host.begin_cable(
            0,
            Box::new(End {
                wire: wire.clone(),
                first: true,
            }),
            single(platform, core, [7u8; 32]),
        );
        join.begin_cable(
            1,
            Box::new(End {
                wire: wire.clone(),
                first: false,
            }),
            single(platform, core, [7u8; 32]),
        );
        host.set_speed(Speed::Normal);
        join.set_speed(Speed::Normal);

        let deadline = Instant::now() + Duration::from_secs(10);
        while host.linked_frames() < 20 || join.linked_frames() < 20 {
            assert!(
                Instant::now() < deadline,
                "the {label} pair stalled at host {} / joiner {} linked frames \
                 (mismatch: {} / {})",
                host.linked_frames(),
                join.linked_frames(),
                host.peer_mismatch(),
                join.peer_mismatch()
            );
            assert!(
                !host.peer_mismatch() && !join.peer_mismatch(),
                "two identical {label} carts were taken for different games"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        host.end_link();
        join.end_link();
    }
}

/// Two different carts must not trade save states: a single-machine state has
/// no header that would say it belongs to another game.
#[test]
fn two_different_single_machine_carts_never_run_a_frame() {
    for (platform, core, label) in SINGLE_MACHINES {
        let wire = Arc::new(Mutex::new(Wire::default()));
        let host = spawn(Arc::new(Ring::new(4096)));
        let join = spawn(Arc::new(Ring::new(4096)));

        host.begin_cable(
            0,
            Box::new(End {
                wire: wire.clone(),
                first: true,
            }),
            single(platform, core, [7u8; 32]),
        );
        join.begin_cable(
            1,
            Box::new(End {
                wire: wire.clone(),
                first: false,
            }),
            single(platform, core, [9u8; 32]),
        );
        host.set_speed(Speed::Normal);
        join.set_speed(Speed::Normal);

        let deadline = Instant::now() + Duration::from_secs(10);
        while !(host.peer_mismatch() && join.peer_mismatch()) {
            assert!(
                Instant::now() < deadline,
                "the {label} mismatch was never noticed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            host.linked_frames(),
            0,
            "{label}: the host ran a frame for a game it is not playing"
        );
        assert_eq!(
            join.linked_frames(),
            0,
            "{label}: the joiner ran the host's game on its own cart"
        );
        host.end_link();
        join.end_link();
    }
}
