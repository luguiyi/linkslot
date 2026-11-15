mod common;

use std::collections::BTreeMap;

use common::tmp_root;
use slot_model::MachineId;
use slot_store::{
    atomic_write, read_slot_state, write_slot_state, GbPalette, Platform, ResumeKind, Shader,
    SlotState, FF_SPEEDS, FF_SPEED_DEFAULT,
};
use tempfile::tempdir;

#[test]
fn atomic_write_leaves_no_partial_file_and_no_temp_behind() {
    let d = tempdir().unwrap();
    let p = d.path().join("x.bin");
    atomic_write(&p, b"first").unwrap();
    atomic_write(&p, &vec![7u8; 4_000_000]).unwrap();
    assert_eq!(std::fs::read(&p).unwrap().len(), 4_000_000);
    let strays: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != "x.bin")
        .collect();
    assert!(strays.is_empty(), "temp files left behind: {strays:?}");
}

#[test]
fn a_name_the_card_accepts_is_a_name_that_can_be_written() {
    let d = tempdir().unwrap();
    for len in [8usize, 100, 200, 239, 240, 245, 250, 251] {
        let p = d.path().join(format!("{}.sav", "a".repeat(len)));
        if std::fs::write(&p, b"control").is_err() {
            continue;
        }
        atomic_write(&p, b"save")
            .unwrap_or_else(|e| panic!("{len} character name: {e}, and a plain write took it"));
        assert_eq!(std::fs::read(&p).unwrap(), b"save");
    }
}

#[test]
fn a_long_multibyte_name_is_written_rather_than_panicking() {
    let d = tempdir().unwrap();
    for pad in 0..3 {
        let p = d
            .path()
            .join(format!("{}{}.sav", "x".repeat(pad), "ポ".repeat(82)));
        if std::fs::write(&p, b"control").is_err() {
            continue;
        }
        atomic_write(&p, b"save")
            .unwrap_or_else(|e| panic!("a Japanese cart name, pad {pad}: {e}"));
        assert_eq!(std::fs::read(&p).unwrap(), b"save");
    }
}

#[test]
fn the_default_timezone_is_beijing_and_an_old_zero_migrates_once() {
    assert_eq!(SlotState::default().utc_offset_min, 480);
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=3\nblue_light=1\nvolume=40\nmuted=0\nclock_set=1\nutc_offset_min=0\n",
    )
    .unwrap();
    assert_eq!(read_slot_state(d.path()).utc_offset_min, 480);

    let mut state = read_slot_state(d.path());
    state.utc_offset_min = 0;
    write_slot_state(d.path(), &state).unwrap();
    assert_eq!(
        read_slot_state(d.path()).utc_offset_min,
        0,
        "an explicit UTC+0 written after migration must survive"
    );
}

#[test]
fn corrupt_slot_state_reads_as_default_rather_than_panicking() {
    let d = tmp_root();
    std::fs::write(d.path().join("linkSlot/Config/slot.state"), b"\x00\xff not json").unwrap();
    assert_eq!(read_slot_state(d.path()), SlotState::default());
}

#[test]
fn slot_state_round_trips_including_a_stem_with_an_equals_sign() {
    let d = tmp_root();
    let s = SlotState {
        cart: Some("Cheats = On".into()),
        brightness: 3,
        blue_light: 9,
        volume: 71,
        muted: true,
        clock_set: true,
        utc_offset_min: 0,
        ..SlotState::default()
    };
    write_slot_state(d.path(), &s).unwrap();
    assert_eq!(read_slot_state(d.path()), s);
}

#[test]
fn a_slot_state_missing_a_key_reads_as_default_not_half_populated() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=3\nblue_light=1\n",
    )
    .unwrap();
    assert_eq!(read_slot_state(d.path()), SlotState::default());
}

#[test]
fn an_out_of_range_level_reads_as_default() {
    let d = tmp_root();
    for body in [
        "cart=\nbrightness=10\nblue_light=1\nvolume=50\n",
        "cart=\nbrightness=3\nblue_light=10\nvolume=50\n",
        "cart=\nbrightness=3\nblue_light=1\nvolume=101\n",
    ] {
        std::fs::write(d.path().join("linkSlot/Config/slot.state"), body).unwrap();
        assert_eq!(
            read_slot_state(d.path()),
            SlotState::default(),
            "accepted {body:?}"
        );
    }
}

#[test]
fn a_first_boot_is_neither_dark_nor_silent() {
    let d = tmp_root();
    let s = read_slot_state(d.path());
    assert!(s.cart.is_none());
    assert!(s.brightness > 0, "boots with the backlight off");
    assert!(s.volume > 0, "boots muted");
}

#[test]
fn slot_state_round_trips_a_negative_utc_offset() {
    let d = tmp_root();
    let s = SlotState {
        cart: None,
        brightness: 5,
        blue_light: 0,
        volume: 60,
        muted: false,
        clock_set: true,
        utc_offset_min: -450,
        ..SlotState::default()
    };
    write_slot_state(d.path(), &s).unwrap();
    assert_eq!(read_slot_state(d.path()).utc_offset_min, -450);
}

#[test]
fn a_line_the_reader_does_not_know_is_skipped() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=3\nfrom_a_later_build=7\nblue_light=1\nvolume=40\nmuted=1\n\
         no equals sign at all\nclock_set=1\nutc_offset_min=-300\n",
    )
    .unwrap();
    let s = read_slot_state(d.path());
    assert_eq!(
        s.cart.as_deref(),
        Some("Emerald"),
        "the file was thrown away"
    );
    assert_eq!(
        (
            s.brightness,
            s.blue_light,
            s.volume,
            s.muted,
            s.clock_set,
            s.utc_offset_min
        ),
        (3, 1, 40, true, true, -300)
    );
}

#[test]
fn a_first_boot_rumbles_and_fast_forwards_silently_at_the_default() {
    let s = SlotState::default();
    assert!(s.rumble, "boots with the motor off");
    assert_eq!(s.ff_speed, FF_SPEED_DEFAULT);
    assert!(!s.ff_sound, "boots with fast forward audible");
    assert!(!s.colour_correction, "boots with the picture tinted");
    assert_eq!(s.shader_gba, Shader::Grid);
    assert_eq!(s.shader_gb, Shader::Simpletex);
    assert!(!s.turbo, "boots with button turbo enabled");
}

#[test]
fn a_card_from_before_the_settings_keeps_all_its_values() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=3\nblue_light=1\nvolume=40\nmuted=1\nclock_set=1\nutc_offset_min=-300\n",
    )
    .unwrap();
    assert_eq!(
        read_slot_state(d.path()),
        SlotState {
            cart: Some("Emerald".into()),
            cart_platform: None,
            brightness: 3,
            blue_light: 1,
            volume: 40,
            volume_hp: 40,
            muted: true,
            muted_hp: true,
            clock_set: true,
            utc_offset_min: -300,
            rumble: true,
            ff_speed: FF_SPEED_DEFAULT,
            ff_sound: false,
            colour_correction: false,
            shader_gba: Shader::Grid,
            shader_gb: Shader::Simpletex,
            // No resume keys at all: every platform follows the default.
            resume_load: BTreeMap::new(),
            resume_save: BTreeMap::new(),
            turbo: false,
            rewind: true,
            gb_palettes: false,
            gb_palette: GbPalette::DEFAULT,
            ..SlotState::default()
        }
    );
}

#[test]
fn old_turbo_key_does_not_enable_turbo_in_v007() {
    let d = tmp_root();
    std::fs::write(d.path().join("linkSlot/Config/slot.state"), "turbo=1\n").unwrap();
    let s = read_slot_state(d.path());
    assert!(!s.turbo, "legacy turbo=1 must not turn button turbo on");
    assert!(s.rewind);
}

#[test]
fn the_quick_menu_settings_round_trip_as_their_own_lines() {
    let d = tmp_root();
    let s = SlotState {
        clock_set: true,
        rumble: false,
        ff_speed: 2,
        ff_sound: true,
        colour_correction: true,
        shader_gba: Shader::Dot,
        shader_gb: Shader::Off,
        resume_load: [
            (MachineId("GBA"), false),
            (MachineId("GB"), true),
            (MachineId("GBC"), true),
            (MachineId("FC"), false),
        ]
        .into_iter()
        .collect(),
        resume_save: [
            (MachineId("GBA"), true),
            (MachineId("GB"), false),
            (MachineId("GBC"), false),
            (MachineId("FC"), true),
        ]
        .into_iter()
        .collect(),
        turbo: false,
        rewind: false,
        gb_palettes: true,
        gb_palette: GbPalette::parse("GBC 深绿 →A").unwrap(),
        ..SlotState::default()
    };
    write_slot_state(d.path(), &s).unwrap();
    assert_eq!(read_slot_state(d.path()), s);
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
    for line in [
        "rumble=0",
        "ff_speed=2",
        "ff_sound=1",
        "colour_correction=1",
        "shader_gba=dot",
        "shader_gb=off",
        "auto_load=fc:0,gb:1,gba:0,gbc:1",
        "auto_save=fc:1,gb:0,gba:1,gbc:0",
        "turbo_enabled=0",
        "rewind=0",
        "gb_palettes=1",
        "gb_palette=GBC 深绿 →A",
    ] {
        assert!(text.lines().any(|l| l == line), "no {line} in {text:?}");
    }
}

#[test]
fn every_speed_the_row_offers_round_trips_as_its_own_number() {
    for speed in FF_SPEEDS {
        let d = tmp_root();
        let s = SlotState {
            clock_set: true,
            ff_speed: speed,
            ..SlotState::default()
        };
        write_slot_state(d.path(), &s).unwrap();
        assert_eq!(read_slot_state(d.path()), s, "{speed}x did not come back");
        let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
        let want = format!("ff_speed={speed}");
        assert!(text.lines().any(|l| l == want), "no {want} in {text:?}");
    }
}

#[test]
fn an_older_build_reads_the_new_speed_as_its_own_default() {
    for speed in FF_SPEEDS.iter().filter(|&&n| n > 4) {
        assert!(
            !(2..=4).contains(speed),
            "{speed}x is inside the range an older build accepts, so it would read as a speed"
        );
    }
    assert_eq!(SlotState::default().ff_speed, FF_SPEED_DEFAULT);
    assert!(
        FF_SPEEDS.contains(&FF_SPEED_DEFAULT),
        "the default is not one of the speeds the row offers"
    );
}

#[test]
fn an_out_of_range_setting_falls_back_to_its_default() {
    let d = tmp_root();
    let known = "cart=Emerald\nbrightness=3\nblue_light=1\nvolume=40\nmuted=0\nclock_set=1\nutc_offset_min=0\n";
    for bad in [
        "rumble=2\nff_speed=5\nff_sound=9\n",
        "rumble=\nff_speed=1\nff_sound=on\n",
        "rumble=-1\nff_speed=1\nff_sound=-1\n",
        "ff_speed=x\n",
        "colour_correction=2\n",
        "colour_correction=\n",
        "colour_correction=Auto\n",
        "ff_speed=5\n",
        "ff_speed=7\n",
        "ff_speed=8\n",
        "ff_speed=255\n",
    ] {
        std::fs::write(d.path().join("linkSlot/Config/slot.state"), format!("{known}{bad}")).unwrap();
        let s = read_slot_state(d.path());
        assert_eq!(
            (s.rumble, s.ff_speed, s.ff_sound, s.colour_correction),
            (true, FF_SPEED_DEFAULT, false, false),
            "accepted {bad:?}"
        );
        assert_eq!(
            (s.cart.as_deref(), s.brightness, s.volume, s.clock_set),
            (Some("Emerald"), 3, 40, true),
            "{bad:?} took the rest of the card with it"
        );
    }
}

#[test]
fn every_platform_round_trips_as_its_own_line() {
    for platform in Platform::ALL {
        let d = tmp_root();
        let s = SlotState {
            cart: Some("Tetris".into()),
            cart_platform: Some(platform),
            clock_set: true,
            ..SlotState::default()
        };
        write_slot_state(d.path(), &s).unwrap();
        assert_eq!(
            read_slot_state(d.path()),
            s,
            "{platform:?} did not come back"
        );
        let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
        let want = format!("cart_platform={}", platform.dir_name().to_lowercase());
        assert!(text.lines().any(|l| l == want), "no {want} in {text:?}");
    }
}

#[test]
fn an_empty_slot_writes_an_empty_platform() {
    let d = tmp_root();
    let s = SlotState {
        clock_set: true,
        ..SlotState::default()
    };
    write_slot_state(d.path(), &s).unwrap();
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
    assert!(
        text.lines().any(|l| l == "cart_platform="),
        "no empty cart_platform line in {text:?}"
    );
    assert_eq!(read_slot_state(d.path()).cart_platform, None);
}

#[test]
fn an_unreadable_platform_reads_as_a_card_that_never_said() {
    let d = tmp_root();
    let known = "cart=Tetris\nbrightness=3\nblue_light=1\nvolume=40\nmuted=0\nclock_set=1\nutc_offset_min=0\n";
    for (line, want) in [
        ("cart_platform=\n", None),
        ("cart_platform=nes\n", None),
        ("cart_platform=gameboy\n", None),
        ("cart_platform=0\n", None),
        ("cart_platform=nds\n", None),
        ("cart_platform=GBA\n", Some(Platform::Gba)),
        ("cart_platform=Gbc\n", Some(Platform::Gbc)),
    ] {
        std::fs::write(d.path().join("linkSlot/Config/slot.state"), format!("{known}{line}")).unwrap();
        let s = read_slot_state(d.path());
        assert_eq!(s.cart_platform, want, "read {line:?} wrong");
        assert_eq!(
            (s.cart.as_deref(), s.brightness, s.volume, s.clock_set),
            (Some("Tetris"), 3, 40, true),
            "{line:?} took the rest of the card with it"
        );
    }
}

#[test]
fn an_offset_outside_the_range_of_real_zones_reads_as_default() {
    let d = tmp_root();
    for body in [
        "cart=\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=900\n",
        "cart=\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=-780\n",
    ] {
        std::fs::write(d.path().join("linkSlot/Config/slot.state"), body).unwrap();
        assert_eq!(read_slot_state(d.path()), SlotState::default(), "{body}");
    }
}

#[test]
fn a_missing_or_unknown_palette_falls_back_to_dmg_green() {
    let d = tmp_root();
    let known = "cart=Emerald\nbrightness=3\nblue_light=1\nvolume=40\nmuted=0\nclock_set=1\nutc_offset_min=0\n";
    for bad in [
        "",
        "gb_palette=\n",
        "gb_palette=Purple\n",
        "gb_palettes=2\ngb_palette=dmg green\n",
    ] {
        std::fs::write(d.path().join("linkSlot/Config/slot.state"), format!("{known}{bad}")).unwrap();
        let s = read_slot_state(d.path());
        assert_eq!(
            s.cart.as_deref(),
            Some("Emerald"),
            "{bad:?} lost the other settings"
        );
        assert!(!s.gb_palettes, "{bad:?} turned palettes on");
        assert_eq!(s.gb_palette, GbPalette::DEFAULT, "{bad:?}");
    }
}

#[test]
fn each_platform_remembers_its_own_resume_switches() {
    let d = tmp_root();
    let mut state = SlotState::default();
    state.set_resume(Platform::Gba, ResumeKind::Load, Some(false));
    state.set_resume(Platform::Gbc, ResumeKind::Save, Some(false));
    // The Famicom slot is the appended fourth one; toggling it must reach
    // index 3 of the array and not wrap onto another platform.
    state.set_resume(Platform::Fc, ResumeKind::Load, Some(false));
    state.set_resume(Platform::Fc, ResumeKind::Save, Some(false));
    write_slot_state(d.path(), &state).unwrap();

    let read = read_slot_state(d.path());
    let load: BTreeMap<MachineId, bool> = [
        (MachineId("GBA"), false),
        (MachineId("FC"), false),
    ]
    .into_iter()
    .collect();
    let save: BTreeMap<MachineId, bool> = [
        (MachineId("GBC"), false),
        (MachineId("FC"), false),
    ]
    .into_iter()
    .collect();
    assert_eq!(read.resume_load, load);
    assert_eq!(read.resume_save, save);
    assert!(!read.resume(Platform::Gba, ResumeKind::Load));
    assert!(read.resume(Platform::Gb, ResumeKind::Load));
    assert!(!read.resume(Platform::Gbc, ResumeKind::Save));
    assert!(!read.resume(Platform::Fc, ResumeKind::Load));
    assert!(!read.resume(Platform::Fc, ResumeKind::Save));

    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
    // 新格式只写显式设置过的机种，按 id 排序；未出现的机种跟随默认。
    for line in ["auto_load=fc:0,gba:0", "auto_save=fc:0,gbc:0"] {
        assert!(text.lines().any(|l| l == line), "no {line} in {text:?}");
    }
}

#[test]
fn a_v099_state_file_migrates_every_resume_switch() {
    let d = tmp_root();
    // v0.9.9 写的是 12 条逐机种 key（含 `default`），这里逐条验证读取。
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\ncart_platform=gba\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=480\nrumble=1\nff_speed=6\nff_sound=0\ncolour_correction=0\nshader_gba=grid\nshader_gb=simpletex\nturbo_enabled=0\nrewind=1\ngb_palettes=0\ngb_palette=DMG 经典绿\nfc_mask=none\ngb_mask=none\ngbc_mask=none\nsfc_mask=none\nlid_minutes=3\nresume_load_default=1\nresume_save_default=1\nauto_load_gba=1\nauto_load_gb=0\nauto_load_gbc=default\nauto_load_fc=0\nauto_load_sfc=1\nauto_load_md=default\nauto_save_gba=0\nauto_save_gb=1\nauto_save_gbc=default\nauto_save_fc=1\nauto_save_sfc=0\nauto_save_md=default\n",
    )
    .unwrap();

    let s = read_slot_state(d.path());
    assert_eq!(s.resume(Platform::Gba, ResumeKind::Load), true);
    assert_eq!(s.resume(Platform::Gb, ResumeKind::Load), false);
    assert_eq!(s.resume(Platform::Gbc, ResumeKind::Load), true, "default 跟随默认");
    assert_eq!(s.resume(Platform::Fc, ResumeKind::Load), false);
    assert_eq!(s.resume(Platform::Sfc, ResumeKind::Load), true);
    assert_eq!(s.resume(Platform::Md, ResumeKind::Load), true);

    assert_eq!(s.resume(Platform::Gba, ResumeKind::Save), false);
    assert_eq!(s.resume(Platform::Gb, ResumeKind::Save), true);
    assert_eq!(s.resume(Platform::Fc, ResumeKind::Save), true);
    assert_eq!(s.resume(Platform::Sfc, ResumeKind::Save), false);

    // 写回后仍然是同一组显式值（新格式），再读一次不丢。
    write_slot_state(d.path(), &s).unwrap();
    let again = read_slot_state(d.path());
    assert_eq!(again.resume_load, s.resume_load);
    assert_eq!(again.resume_save, s.resume_save);
}

#[test]
fn a_state_file_from_before_the_famicom_reads_its_switches_as_on() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Contra\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\nauto_load_gba=1\nauto_save_gba=1\n",
    )
    .unwrap();
    let s = read_slot_state(d.path());
    assert!(
        s.resume(Platform::Fc, ResumeKind::Load),
        "an older card must not start with Famicom autoload off"
    );
    assert!(
        s.resume(Platform::Fc, ResumeKind::Save),
        "an older card must not start with Famicom autosave off"
    );
}

#[test]
fn old_global_resume_keys_are_ignored_and_new_platform_defaults_are_on() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\nauto_load=0\neject_save=0\n",
    )
    .unwrap();
    let s = read_slot_state(d.path());
    assert_eq!(
        s.resume_load,
        BTreeMap::new(),
        "old auto_load must no longer disable the per-platform switches"
    );
    assert_eq!(
        s.resume_save,
        BTreeMap::new(),
        "old eject_save must no longer disable the per-platform switches"
    );
}


#[test]
fn the_platforms_follow_the_default_until_they_are_set() {
    let mut s = SlotState::default();
    assert!(s.resume(Platform::Gb, ResumeKind::Load), "defaults start on");
    s.set_resume_default(ResumeKind::Load, false);
    assert!(
        !s.resume(Platform::Gb, ResumeKind::Load),
        "a following platform did not move with the default"
    );
    // The machine with no rows of its own keeps following.
    assert!(!s.resume(Platform::Fc, ResumeKind::Load));
    s.set_resume(Platform::Gb, ResumeKind::Load, Some(true));
    assert!(
        s.resume(Platform::Gb, ResumeKind::Load),
        "an explicit platform ignored its own switch"
    );
    s.set_resume(Platform::Gb, ResumeKind::Load, None);
    assert!(!s.resume(Platform::Gb, ResumeKind::Load), "reset did not re-follow");
    // The save side has its own default and switch.
    assert!(s.resume(Platform::Gb, ResumeKind::Save));
}

#[test]
fn a_legacy_on_follows_the_default_while_an_off_survives() {
    let d = tmp_root();
    std::fs::write(
        d.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\nauto_load_gb=1\nauto_load_gba=0\nauto_save_gb=1\n",
    )
    .unwrap();
    let s = read_slot_state(d.path());
    assert_eq!(
        s.resume_override(Platform::Gb, ResumeKind::Load),
        None,
        "the old on was the hard default, so it must follow the default now"
    );
    assert_eq!(
        s.resume_override(Platform::Gba, ResumeKind::Load),
        Some(false),
        "an explicit off from an old card must survive"
    );
    assert_eq!(s.resume_override(Platform::Gb, ResumeKind::Save), None);
    assert!(s.resume_load_default && s.resume_save_default);
}

#[test]
fn the_defaults_and_following_switches_round_trip() {
    let d = tmp_root();
    let mut s = SlotState::default();
    s.set_resume_default(ResumeKind::Save, false);
    s.set_resume(Platform::Gbc, ResumeKind::Save, Some(true));
    s.set_resume(Platform::Gba, ResumeKind::Load, Some(false));
    write_slot_state(d.path(), &s).unwrap();
    let read = read_slot_state(d.path());
    assert_eq!(read, s);
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
    for line in [
        "resume_load_default=1",
        "resume_save_default=0",
        "auto_save=gbc:1",
        "auto_load=gba:0",
    ] {
        assert!(text.lines().any(|l| l == line), "no {line} in {text:?}");
    }
}
#[test]
fn the_fc_mask_selection_round_trips() {
    let d = tmp_root();
    let mut s = SlotState::default();
    assert_eq!(s.fc_mask, None, "the mask must start off");
    s.fc_mask = Some("FC02.png".to_string());
    s.gb_mask = Some("GB02.png".to_string());
    s.gbc_mask = Some("GBC03.png".to_string());
    write_slot_state(d.path(), &s).unwrap();
    assert_eq!(read_slot_state(d.path()).fc_mask.as_deref(), Some("FC02.png"));
    assert_eq!(read_slot_state(d.path()).gb_mask.as_deref(), Some("GB02.png"));
    assert_eq!(
        read_slot_state(d.path()).gbc_mask.as_deref(),
        Some("GBC03.png")
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/slot.state")).unwrap();
    assert!(text.lines().any(|l| l == "fc_mask=FC02.png"), "{text}");
    assert!(text.lines().any(|l| l == "gb_mask=GB02.png"), "{text}");
    assert!(text.lines().any(|l| l == "gbc_mask=GBC03.png"), "{text}");

    // A card from before the selection existed wrote 1: keep the mask on,
    // meaning "the first file on the card".
    let legacy = tmp_root();
    std::fs::write(
        legacy.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\nfc_mask=1\n",
    )
    .unwrap();
    let read = read_slot_state(legacy.path());
    assert_eq!(read.fc_mask.as_deref(), Some(""));
    // And "none" means off.
    let off = tmp_root();
    std::fs::write(
        off.path().join("linkSlot/Config/slot.state"),
        "cart=Emerald\nbrightness=5\nblue_light=0\nvolume=60\nmuted=0\nclock_set=1\nutc_offset_min=0\nfc_mask=none\n",
    )
    .unwrap();
    assert_eq!(read_slot_state(off.path()).fc_mask, None);
}
/// 护栏（P5-08）：对 `Platform::ALL` 全遍历做读写回环。
///
/// 新增机种时这个测试自动覆盖——不需要再手写第 13 条 INI key。
#[test]
fn every_machine_resume_switch_round_trips() {
    let d = tmp_root();
    let mut s = SlotState::default();
    for p in Platform::ALL {
        s.set_resume(p, ResumeKind::Load, Some(true));
        s.set_resume(p, ResumeKind::Save, Some(false));
    }
    write_slot_state(d.path(), &s).unwrap();
    let read = read_slot_state(d.path());
    assert_eq!(read.resume_load, s.resume_load);
    assert_eq!(read.resume_save, s.resume_save);
    assert_eq!(read.resume_load.len(), Platform::ALL.len());

    // 默认值（未显式设置）的机种不在表里：清掉一个再写，读回来仍是"跟随默认"。
    let mut s = read;
    s.set_resume(Platform::Gba, ResumeKind::Load, None);
    write_slot_state(d.path(), &s).unwrap();
    let read = read_slot_state(d.path());
    assert_eq!(read.resume_override(Platform::Gba, ResumeKind::Load), None);
    assert!(read.resume(Platform::Gba, ResumeKind::Load));
}