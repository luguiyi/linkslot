use std::path::Path;

use slot_store::{
    available_options, core_for, core_for_platform, read_selected_cores, write_selected_core, Core,
    Platform,
};
use tempfile::tempdir;

/// 让核心"在卡上"：可用性检查只看 `System/linkSlot/cores/<id>/<dylib>_libretro` 是否存在。
fn install_core(root: &Path, core: Core) {
    let dir = root.join("System/linkSlot/cores").join(core.id());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!(
            "{}_libretro.{}",
            core.as_str(),
            std::env::consts::DLL_EXTENSION
        )),
        b"",
    )
    .unwrap();
}

fn root_with(ini: Option<&str>) -> tempfile::TempDir {
    let d = tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("linkSlot/Config")).unwrap();
    if let Some(text) = ini {
        std::fs::write(d.path().join("linkSlot/Config/selected_core.txt"), text).unwrap();
    }
    d
}

#[test]
fn absent_file_means_everything_defaults() {
    let d = root_with(None);
    assert!(read_selected_cores(d.path()).is_empty());
    assert_eq!(core_for(d.path(), "Emerald"), Core::Mgba);
}

#[test]
fn a_listed_stem_gets_its_core() {
    let d = root_with(Some("Pokemon - Emerald Version (USA, Europe) = gpsp\n"));
    assert_eq!(
        core_for(d.path(), "Pokemon - Emerald Version (USA, Europe)"),
        Core::Gpsp
    );
}

#[test]
fn an_unlisted_stem_defaults_to_mgba() {
    let d = root_with(Some("Emerald = gpsp\n"));
    assert_eq!(core_for(d.path(), "Metroid Fusion"), Core::Mgba);
}

#[test]
fn malformed_lines_are_ignored_rather_than_fatal() {
    let d = root_with(Some(concat!(
        "\n",
        "# a comment\n",
        "; another comment\n",
        "[cores]\n",
        "no equals sign here\n",
        "Emerald = notacore\n",
        "  Spaced Out   =   gpsp  \n",
        "= gpsp\n",
        "Trailing =\n",
    )));
    let map = read_selected_cores(d.path());
    assert_eq!(
        map.get("Spaced Out"),
        Some(&Core::Gpsp),
        "whitespace not trimmed"
    );
    assert_eq!(
        map.get("Emerald"),
        None,
        "an unknown core name must not be stored"
    );
    assert_eq!(core_for(d.path(), "Emerald"), Core::Mgba);
    assert_eq!(map.len(), 1, "only the one good line should survive");
}

#[test]
fn a_later_duplicate_wins() {
    let d = root_with(Some("Emerald = mgba\nEmerald = gpsp\n"));
    assert_eq!(core_for(d.path(), "Emerald"), Core::Gpsp);
}

#[test]
fn the_last_line_for_a_cart_wins_even_when_it_is_the_typo() {
    let d = root_with(Some("Emerald = gpsp\nEmerald = notacore\n"));
    assert_eq!(
        core_for(d.path(), "Emerald"),
        Core::Mgba,
        "a line the later one superseded is still in force"
    );
    assert_eq!(
        read_selected_cores(d.path()).get("Emerald"),
        None,
        "the map and core_for read the same two lines differently"
    );

    let d = root_with(Some("Emerald = notacore\nEmerald = gpsp\n"));
    assert_eq!(core_for(d.path(), "Emerald"), Core::Gpsp);
    assert_eq!(
        read_selected_cores(d.path()).get("Emerald"),
        Some(&Core::Gpsp)
    );
}

#[test]
fn core_names_round_trip() {
    for c in Core::ALL {
        assert_eq!(Core::parse(c.as_str()), Some(c));
    }
    assert_eq!(
        Core::parse("MGBA"),
        Some(Core::Mgba),
        "case is not the user's problem"
    );
    assert_eq!(Core::parse("nonsense"), None);
    assert_eq!(Core::default(), Core::Mgba);
}

#[test]
fn writing_a_core_creates_the_file_when_absent() {
    let d = root_with(None);
    slot_store::write_selected_core(d.path(), "Emerald", Core::Gpsp).unwrap();
    assert_eq!(core_for(d.path(), "Emerald"), Core::Gpsp);
}

#[test]
fn writing_a_core_replaces_that_carts_line_and_leaves_the_rest_alone() {
    let d = root_with(Some(concat!(
        "# my notes\n",
        "\n",
        "Emerald = mgba\n",
        "Metroid Fusion = gpsp\n",
    )));
    slot_store::write_selected_core(d.path(), "Emerald", Core::Gpsp).unwrap();

    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/selected_core.txt")).unwrap();
    assert!(
        text.contains("# my notes"),
        "a hand-written comment was destroyed"
    );
    assert!(
        text.contains("Metroid Fusion = gpsp"),
        "another cart's entry was lost"
    );
    assert_eq!(core_for(d.path(), "Emerald"), Core::Gpsp);
    assert_eq!(core_for(d.path(), "Metroid Fusion"), Core::Gpsp);
    assert_eq!(
        text.matches("Emerald").count(),
        1,
        "the old line was left behind"
    );
}

#[test]
fn writing_a_core_appends_a_cart_the_file_has_never_seen() {
    let d = root_with(Some("Emerald = gpsp\n"));
    slot_store::write_selected_core(d.path(), "Drill Dozer", Core::Gpsp).unwrap();
    assert_eq!(core_for(d.path(), "Emerald"), Core::Gpsp);
    assert_eq!(core_for(d.path(), "Drill Dozer"), Core::Gpsp);
}

#[test]
fn writing_the_default_still_records_it() {
    let d = root_with(Some("Emerald = gpsp\n"));
    slot_store::write_selected_core(d.path(), "Emerald", Core::Mgba).unwrap();
    assert_eq!(core_for(d.path(), "Emerald"), Core::Mgba);
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/selected_core.txt")).unwrap();
    assert!(text.contains("Emerald = mgba"));
}

#[test]
fn the_game_boy_family_defaults_to_mgba() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        core_for_platform(d.path(), "Tetris", Platform::Gb),
        Core::Mgba
    );
    assert_eq!(
        core_for_platform(d.path(), "Tetris Chromatic", Platform::Gbc),
        Core::Mgba
    );
    assert_eq!(
        core_for_platform(d.path(), "Emerald", Platform::Gba),
        Core::Mgba
    );
}

#[test]
fn a_cart_can_ask_for_a_non_default_core_by_hand() {
    let d = tempfile::tempdir().unwrap();
    install_core(d.path(), Core::Mgba);
    install_core(d.path(), Core::Gpsp);
    write_selected_core(d.path(), "Emerald", Core::Gpsp).unwrap();
    assert_ne!(Core::default_for(Platform::Gba), Core::Gpsp);
    assert_eq!(
        core_for_platform(d.path(), "Emerald", Platform::Gba),
        Core::Gpsp
    );
}

#[test]
fn a_line_naming_a_core_the_platform_cannot_run_is_dropped() {
    let d = tempfile::tempdir().unwrap();
    write_selected_core(d.path(), "Tetris", Core::Gpsp).unwrap();
    assert_eq!(
        core_for_platform(d.path(), "Tetris", Platform::Gb),
        Core::Mgba
    );
    assert_eq!(
        core_for_platform(d.path(), "Tetris", Platform::Gbc),
        Core::Mgba
    );
}

#[test]
fn the_picker_board_has_exactly_two_sockets() {
    assert_eq!(Core::ALL.len(), 2);
    assert_eq!(Core::ALL, [Core::Mgba, Core::Gpsp]);
}

#[test]
fn every_core_we_ship_is_in_the_every_list() {
    assert_eq!(Core::EVERY.len(), 6);
    assert_eq!(
        Core::EVERY,
        [
            Core::Mgba,
            Core::Gpsp,
            Core::Nes,
            Core::Snes,
            Core::Gpgx,
            Core::Snes9x
        ]
    );
    for core in Core::EVERY {
        assert_eq!(
            Core::parse(core.as_str()),
            Some(core),
            "{core:?} does not round-trip through its config name"
        );
    }
    assert_eq!(Core::parse("fceumm"), Some(Core::Nes));
    assert_eq!(Core::Nes.as_str(), "fceumm");
    assert_eq!(Core::Nes.text(), "fceumm");
    // The PCB socket and chip wear the actual emulator name, not a generic
    // "SFC emulator" label.
    assert_eq!(Core::Snes.as_str(), "supafaust");
    assert_eq!(Core::Snes.text(), "supafaust");
    assert_eq!(Core::Gpgx.as_str(), "genesis_plus_gx");
    assert_eq!(Core::Gpgx.text(), "Genesis Plus GX");
    assert_eq!(Core::Gpgx.chip_text(), "GPGX");
    assert_eq!(Core::parse("genesis_plus_gx"), Some(Core::Gpgx));
}

#[test]
fn each_core_runs_only_the_platforms_it_can() {
    for platform in [Platform::Gba, Platform::Gb, Platform::Gbc] {
        assert!(
            Core::Mgba.runs(platform),
            "mGBA should still run {platform:?}"
        );
    }
    assert!(
        !Core::Mgba.runs(Platform::Fc),
        "mGBA cannot take a Famicom cart"
    );

    assert!(Core::Gpsp.runs(Platform::Gba));
    assert!(!Core::Gpsp.runs(Platform::Gb));
    assert!(!Core::Gpsp.runs(Platform::Gbc));
    assert!(!Core::Gpsp.runs(Platform::Fc));

    assert!(Core::Nes.runs(Platform::Fc));
    assert!(!Core::Nes.runs(Platform::Gba));
    assert!(!Core::Nes.runs(Platform::Gb));
    assert!(!Core::Nes.runs(Platform::Gbc));
}

#[test]
fn the_famicom_defaults_to_fceumm() {
    assert_eq!(Core::default_for(Platform::Fc), Core::Nes);
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        core_for_platform(d.path(), "Contra", Platform::Fc),
        Core::Nes
    );
}

#[test]
fn a_famicom_core_chosen_for_a_game_boy_cart_is_dropped() {
    let d = tempfile::tempdir().unwrap();
    write_selected_core(d.path(), "Tetris", Core::Nes).unwrap();
    assert_eq!(
        core_for_platform(d.path(), "Tetris", Platform::Gb),
        Core::Mgba
    );
    // ... and the line still serves the FC cart that shares the stem.
    assert_eq!(
        core_for_platform(d.path(), "Tetris", Platform::Fc),
        Core::Nes,
        "the per-cart FC choice was lost"
    );
}

#[test]
fn a_recorded_core_without_its_binary_falls_back_to_the_default() {
    let d = tempfile::tempdir().unwrap();
    install_core(d.path(), Core::Mgba);
    write_selected_core(d.path(), "Emerald", Core::Gpsp).unwrap();
    assert_eq!(
        core_for_platform(d.path(), "Emerald", Platform::Gba),
        Core::Mgba,
        "gpsp 不在卡上时，应回退到机种默认核心而不是报缺核心"
    );
}

#[test]
fn the_picker_only_lists_cores_present_on_the_card() {
    let d = tempfile::tempdir().unwrap();
    // 一个核心都没装：仍返回默认核心，保证 UI 不为空。
    assert_eq!(
        available_options(d.path(), Platform::Sfc),
        vec![Core::Snes9x]
    );
    // 装 snes9x（官方随包）。
    install_core(d.path(), Core::Snes9x);
    assert_eq!(
        available_options(d.path(), Platform::Sfc),
        vec![Core::Snes9x]
    );
    // 用户自备 supafaust 后，它出现在选择器里。
    install_core(d.path(), Core::Snes);
    assert_eq!(
        available_options(d.path(), Platform::Sfc),
        vec![Core::Snes9x, Core::Snes]
    );
}

#[test]
fn sfc_defaults_to_snes9x_and_keeps_supafaust_as_an_option() {
    assert_eq!(Core::default_for(Platform::Sfc), Core::Snes9x);
    assert!(Core::options_for(Platform::Sfc).contains(&Core::Snes9x));
    assert!(Core::options_for(Platform::Sfc).contains(&Core::Snes));
}