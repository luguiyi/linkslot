use slot::cable::{decode, encode, Cable, Pairing, DELAY};
use slot_retro::ButtonMask;

const A: ButtonMask = ButtonMask(ButtonMask::A);
const B: ButtonMask = ButtonMask(ButtonMask::B);

#[test]
fn a_fresh_cable_can_run_the_delay_before_the_peer_says_anything() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    for _ in 0..DELAY {
        assert!(c.ready().is_some(), "a seeded frame needed the peer");
        c.advance();
    }
    assert!(
        c.ready().is_none(),
        "it ran past the seed without the peer's masks"
    );
}

#[test]
fn a_sampled_mask_is_stamped_for_a_later_frame() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    let (frame, mask) = decode(&c.sample(A)).expect("its own packet did not parse");
    assert_eq!(frame, DELAY);
    assert_eq!(mask, A);
}

#[test]
fn each_end_reports_the_pair_in_port_order() {
    let mut p0 = Cable::new(0, Pairing::PairedGba);
    let mut p1 = Cable::new(1, Pairing::PairedGba);
    p1.prime();
    for _ in 0..DELAY {
        let a = p0.sample(A);
        let b = p1.sample(B);
        assert!(p0.accept(&b));
        assert!(p1.accept(&a));
        p0.advance();
        p1.advance();
    }

    assert_eq!(p0.ready(), Some((A, B)), "player 0 read the pair swapped");
    assert_eq!(
        p1.ready(),
        Some((A, B)),
        "player 1 disagreed with player 0 about port order"
    );
}

/// The Super Famicom's two pads carry X/Y and the shoulders as well as A/B.
/// The single-machine cable must keep the host on pad one and the joiner on
/// pad two, with every SFC button surviving the wire.
#[test]
fn a_super_famicom_pair_keeps_the_host_on_pad_one() {
    let sfc = |rom| {
        let mut hello = hello(rom);
        hello.platform = 4; // Platform::Sfc
        hello.core = 3; // Core::Snes (supafaust)
        hello
    };
    let host_pad = ButtonMask(ButtonMask::A | ButtonMask::X | ButtonMask::L);
    let join_pad = ButtonMask(
        ButtonMask::B | ButtonMask::Y | ButtonMask::R | ButtonMask::START | ButtonMask::SELECT,
    );
    let mut p0 = Cable::new(0, Pairing::SingleMachine(sfc([7u8; 32])));
    let mut p1 = Cable::new(1, Pairing::SingleMachine(sfc([7u8; 32])));
    p1.prime();
    for _ in 0..DELAY {
        let a = p0.sample(host_pad);
        let b = p1.sample(join_pad);
        assert!(p0.accept(&b));
        assert!(p1.accept(&a));
        p0.advance();
        p1.advance();
    }
    assert_eq!(
        p0.ready(),
        Some((host_pad, join_pad)),
        "the host did not take the first pad"
    );
    assert_eq!(
        p1.ready(),
        Some((host_pad, join_pad)),
        "the joiner did not take the second pad"
    );
}

#[test]
fn a_frame_waits_rather_than_guessing_a_missing_mask() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    for _ in 0..DELAY {
        c.sample(A);
        c.advance();
    }
    c.sample(A);
    assert!(c.ready().is_none(), "it ran without the peer's mask");
    c.stall();
    c.stall();
    assert_eq!(c.stalled(), 2);

    assert!(c.accept(&encode(DELAY, B)));
    assert_eq!(c.ready(), Some((A, B)));
    c.advance();
    assert_eq!(c.stalled(), 0, "the stall count survived a frame running");
}

#[test]
fn both_ends_agree_on_every_frame() {
    let mut p0 = Cable::new(0, Pairing::PairedGba);
    let mut p1 = Cable::new(1, Pairing::PairedGba);
    p1.prime();
    let script = [A, B, A, ButtonMask::default(), B, B, A];

    let mut seen0 = Vec::new();
    let mut seen1 = Vec::new();
    for (i, m) in script.iter().enumerate() {
        let a = p0.sample(*m);
        let b = p1.sample(script[script.len() - 1 - i]);
        p0.accept(&b);
        p1.accept(&a);
        if let Some(pair) = p0.ready() {
            seen0.push(pair);
            p0.advance();
        }
        if let Some(pair) = p1.ready() {
            seen1.push(pair);
            p1.advance();
        }
    }
    assert_eq!(seen0, seen1, "the two ends ran different frames");
    assert!(seen0.len() >= script.len() - DELAY as usize);
}

#[test]
fn junk_and_duplicates_are_dropped_rather_than_raised() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    assert!(!c.accept(&[9, 2, 3]), "a short packet was taken");
    assert!(!c.accept(&[]), "an empty packet was taken");
    for _ in 0..DELAY {
        c.sample(B);
        c.advance();
    }
    assert!(c.accept(&encode(DELAY, A)));
    c.sample(B);
    assert_eq!(c.ready(), Some((B, A)));
    c.advance();
    assert!(
        !c.accept(&encode(DELAY, A)),
        "a mask for a frame already run was taken back in"
    );
}

#[test]
fn a_stall_does_not_add_to_the_input_delay() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    for _ in 0..DELAY {
        c.sample(A);
        c.advance();
    }
    let (stamped, _) = decode(&c.sample(B)).expect("packet");
    let ahead = stamped - c.frame();

    for _ in 0..20 {
        c.sample(ButtonMask::default());
        c.stall();
    }
    assert_eq!(c.stalled(), 20);

    let (stamped, mask) = decode(&c.sample(ButtonMask::default())).expect("packet");
    assert_eq!(
        stamped - c.frame(),
        ahead,
        "the input delay grew across a stall: masks are being stamped per present again"
    );
    assert_eq!(
        mask, B,
        "a decided mask was revised, which desyncs the pair"
    );
}

#[test]
fn a_joiner_runs_nothing_until_it_has_the_hosts_machine() {
    let mut join = Cable::new(1, Pairing::PairedGba);
    assert!(!join.primed(), "a fresh joiner claimed to be ready");
    assert!(
        join.ready().is_none(),
        "the joiner ran a seeded frame on its own machine"
    );

    let state: Vec<u8> = (0..150_000u32).map(|n| n as u8).collect();
    let packets = slot::cable::state_packets(&state);
    assert!(packets.len() > 1, "a state this size must be cut up");
    for (i, p) in packets.iter().enumerate() {
        assert!(p.len() <= 65_535, "chunk {i} cannot cross the wire");
        assert!(join.accept(p));
        if i + 1 < packets.len() {
            assert!(
                join.take_state().is_none(),
                "a part was taken for the whole"
            );
        }
    }
    assert_eq!(join.take_state().as_deref(), Some(state.as_slice()));
    assert!(
        join.take_state().is_none(),
        "the state was handed over twice"
    );

    join.prime();
    assert!(join.ready().is_some(), "priming did not let the frames run");
}

#[test]
fn a_host_is_ready_from_the_start() {
    let host = Cable::new(0, Pairing::PairedGba);
    assert!(host.primed());
    assert!(host.ready().is_some());
}

#[test]
fn a_state_chunk_is_never_read_as_a_button_mask() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    for p in slot::cable::state_packets(&[9u8; 32]) {
        assert!(decode(&p).is_none(), "a state chunk parsed as a mask");
        assert!(c.accept(&p));
    }
    assert!(c.accept(&encode(DELAY, A)));
}

#[test]
fn a_stall_during_the_swap_is_not_a_lost_peer() {
    let mut host = Cable::new(0, Pairing::PairedGba);
    assert!(
        !host.armed(),
        "the host armed before the joiner said it was running"
    );
    for _ in 0..200 {
        host.stall();
    }
    assert!(
        !host.armed(),
        "a long swap was read as a peer that vanished"
    );

    assert!(host.accept(&slot::cable::ready_packet()));
    assert!(host.armed(), "the joiner said it was ready and was ignored");
}

#[test]
fn a_joiner_is_not_armed_until_it_has_restored() {
    let mut join = Cable::new(1, Pairing::PairedGba);
    assert!(!join.armed(), "an unprimed joiner armed");
    join.prime();
    assert!(join.armed(), "the joiner never armed after restoring");
}

fn hello(rom: [u8; 32]) -> slot::cable::Hello {
    slot::cable::Hello {
        version: slot::cable::HELLO_VERSION,
        platform: 3,
        core: 2,
        rom,
        device: [0; slot::cable::HELLO_DEVICE],
    }
}

#[test]
fn a_hello_carries_the_senders_name_but_not_its_identity() {
    let named = hello([7u8; 32]).named("客厅机");
    assert_eq!(named.device_name(), "客厅机");
    assert!(
        named.agrees(&hello([7u8; 32])),
        "a name is not part of agreeing on the game"
    );
    let packet = slot::cable::hello_packet(named);
    assert_eq!(
        slot::cable::decode_hello(&packet).map(|h| h.device_name()),
        Some("客厅机".to_string())
    );
}

#[test]
fn a_name_too_wide_for_the_slot_is_cut_on_a_character_boundary() {
    let wide = hello([7u8; 32]).named("一二三四五六七八");
    assert_eq!(wide.device_name(), "一二三四五");
    assert!(
        std::str::from_utf8(&wide.device).is_ok(),
        "the cut split a character"
    );
}

#[test]
fn a_refusal_survives_the_wire_and_says_why() {
    for reason in [
        slot::cable::FollowRefusal::Declined,
        slot::cable::FollowRefusal::MissingRom,
        slot::cable::FollowRefusal::Busy,
    ] {
        let packet = slot::cable::refuse_packet(reason);
        assert!(slot::cable::is_control(&packet), "a refusal is session traffic");
        assert_eq!(slot::cable::decode_refuse(&packet), Some(reason));
        assert!(!reason.text().is_empty());
    }
    assert_eq!(slot::cable::decode_refuse(&[0x12, 9]), None);
    assert_eq!(slot::cable::decode_refuse(&[0x12]), None);
}

#[test]
fn keepalive_frames_are_session_traffic_and_never_a_button_mask() {
    let ping = slot::cable::ping_packet();
    let pong = slot::cable::pong_packet();
    assert!(slot::cable::is_control(&ping));
    assert!(slot::cable::is_control(&pong));
    assert!(slot::cable::is_ping(&ping));
    assert!(slot::cable::is_pong(&pong));
    assert!(!slot::cable::is_ping(&pong));
    assert!(decode(&ping).is_none(), "a ping parsed as a mask");
}

#[test]
fn a_streamed_digest_matches_the_file_digest() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("cart.nes");
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let streamed = slot::rom_id::digest_reader(&bytes[..]).unwrap();
    assert_eq!(streamed, slot::rom_id::digest_file(&path).unwrap());
}

#[test]
fn a_hello_survives_the_wire_whole() {
    let packet = slot::cable::hello_packet(hello([0xdeu8; 32]));
    assert_eq!(
        slot::cable::decode_hello(&packet),
        Some(hello([0xdeu8; 32]))
    );
}

#[test]
fn a_hello_only_agrees_with_the_same_machine_and_cart() {
    let mine = hello([7u8; 32]);
    assert!(mine.agrees(&hello([7u8; 32])));
    assert!(
        !mine.agrees(&hello([9u8; 32])),
        "two different carts were taken for one"
    );
    let mut other_machine = hello([7u8; 32]);
    other_machine.platform = 0;
    assert!(!mine.agrees(&other_machine));
    let mut other_core = hello([7u8; 32]);
    other_core.core = 0;
    assert!(!mine.agrees(&other_core));
    let mut other_version = hello([7u8; 32]);
    other_version.version += 1;
    assert!(
        !mine.agrees(&other_version),
        "the protocol version was ignored"
    );
}

#[test]
fn a_short_or_foreign_packet_is_not_a_hello() {
    assert_eq!(slot::cable::decode_hello(&[]), None);
    assert_eq!(slot::cable::decode_hello(&[4, 1, 3, 2, 0, 0]), None);
    assert_eq!(
        slot::cable::decode_hello(&slot::cable::ready_packet()),
        None,
        "a ready packet was read as a hello"
    );
}

#[test]
fn a_cable_records_what_the_peer_says_it_is() {
    let mut c = Cable::new(0, Pairing::PairedGba);
    assert_eq!(c.peer_hello(), None);
    assert!(c.accept(&slot::cable::hello_packet(hello([11u8; 32]))));
    assert_eq!(c.peer_hello(), Some(hello([11u8; 32])));
}

#[test]
fn a_single_machine_payload_carries_the_state_and_the_battery_save() {
    let state = vec![1u8; 400];
    let sav = vec![2u8; 8 * 1024];
    let payload = slot::cable::single_payload(&state, Some(&sav));
    let (got_state, got_sav) = slot::cable::decode_single_payload(&payload).expect("round trip");
    assert_eq!(got_state, state);
    assert_eq!(got_sav.as_deref(), Some(sav.as_slice()));

    // A cart with no battery save still has to say so, rather than being
    // mistaken for a truncated transfer.
    let bare = slot::cable::single_payload(&state, None);
    let (got_state, got_sav) = slot::cable::decode_single_payload(&bare).expect("round trip");
    assert_eq!(got_state, state);
    assert_eq!(got_sav, None);
}

#[test]
fn a_payload_that_is_not_exactly_a_pair_is_refused() {
    let payload = slot::cable::single_payload(&[0u8; 64], Some(&[9u8; 16]));
    assert!(slot::cable::decode_single_payload(&payload[..payload.len() - 1]).is_none());
    assert!(slot::cable::decode_single_payload(&[]).is_none());
    assert!(slot::cable::decode_single_payload(&[0xff; 4]).is_none());
}

#[test]
fn a_game_announce_round_trips_with_its_name_and_identity() {
    for name in ["Contra", "雪人兄弟(v1.0)(简)[高伟](JP)[ACT](2Mb)", ""] {
        let start = slot::cable::GameStart {
            platform: 3,
            core: 2,
            rom: [0x5au8; 32],
            name: name.to_string(),
        };
        let bytes = slot::cable::game_start_packet(&start);
        assert!(slot::cable::is_control(&bytes));
        let back = slot::cable::decode_game_start(&bytes)
            .unwrap_or_else(|| panic!("an announce did not survive the wire: {name:?}"));
        assert_eq!(back.platform, start.platform);
        assert_eq!(back.core, start.core);
        assert_eq!(back.rom, start.rom);
        assert!(
            back.name.len() <= slot::cable::GAME_NAME_MAX,
            "the name was not cut to the frame"
        );
        if name.len() <= slot::cable::GAME_NAME_MAX {
            assert_eq!(back.name, name, "a short name changed on the wire");
        } else {
            assert!(
                name.starts_with(&back.name),
                "a long name was not cut on a character boundary"
            );
        }
        assert!(decode(&bytes).is_none(), "an announce parsed as a mask");
    }
}
#[test]
fn a_clock_share_round_trips_including_west_of_utc() {
    for (utc, offset) in [(1_782_000_000i64, 480i16), (0, 0), (1_000, -300)] {
        let packet = slot::cable::time_sync_packet(utc, offset);
        assert_eq!(packet.len(), 11);
        assert!(slot::cable::is_control(&packet));
        assert_eq!(
            slot::cable::decode_time_sync(&packet),
            Some((utc, offset)),
            "a clock share did not survive the wire: {utc}/{offset}"
        );
        assert!(decode(&packet).is_none(), "a clock share parsed as a mask");
    }
    assert_eq!(slot::cable::decode_time_sync(&[0x1b]), None, "truncated");
}