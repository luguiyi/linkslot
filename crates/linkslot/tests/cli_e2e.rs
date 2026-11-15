//! End-to-end CLI tests (parse -> config -> sync -> stdout contract) against
//! the mock WebDAV server, including Chinese names, spaces and exit codes.

mod support;

use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::unit::Unit;
use support::*;

const PUSH_KEYS: [&str; 5] = [
    "LINKSLOT_ACTION",
    "LINKSLOT_RESULT",
    "LINKSLOT_VERSION",
    "LINKSLOT_ERROR",
    "LINKSLOT_DETAIL",
];

fn keys(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| line.starts_with("LINKSLOT_") && !line.starts_with("LINKSLOT_STAGE="))
        .map(|line| line.split('=').next().unwrap().to_string())
        .collect()
}

/// Run a unit command for `game` plus extra options, against this harness.
fn cli_unit(harness: &Harness, command: &str, game: &str, extra: &[&str]) -> (i32, String, String) {
    let mut args: Vec<&str> = vec![
        command,
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        game,
    ];
    args.extend_from_slice(extra);
    harness.cli(&args)
}

#[test]
fn cli_push_status_pull_roundtrip() {
    let harness = Harness::new("cli-roundtrip");
    let unit = harness.unit();
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"cli-resume".as_slice()),
            ("2026-10-20_10-00-00.state", b"cli-state".as_slice()),
        ],
    );

    let (code, stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(keys(&stdout), PUSH_KEYS);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("push"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("1"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("none"));
    let stage_values = stage_lines(&stdout);
    assert!(!stage_values.is_empty(), "no stage lines: {stdout}");
    for stage in &stage_values {
        assert!(
            ["net", "connect", "upload", "download", "verify", "commit"].contains(&stage.as_str()),
            "unexpected stage {stage}"
        );
    }

    let (code, stdout, stderr) = harness.cli(&[
        "status",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("status"));
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("1"));
    assert!(contract_value(&contract, "LINKSLOT_DETAIL")
        .unwrap()
        .contains("本地 v1 / 云端 v1"));

    // Up to date pull -> noop, exit 0, no VERSION line.
    let (code, stdout, _stderr) = harness.cli(&[
        "pull",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert!(contract_value(&contract, "LINKSLOT_VERSION").is_none());
}

#[test]
fn cli_push_game_name_with_spaces_and_chinese() {
    let harness = Harness::new("cli-spaces");
    let game = "Pokemon - 绿宝石";
    // The fixture ROM under Games/GBA/ uses the same game name, so the CLI can
    // auto-discover it and hash it into the cloud identity.
    let unit = harness.unit_named(game);
    harness.seed_local(&unit, &[("resume.state", b"spaced-resume".as_slice())]);

    let (code, _stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        game,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(harness
        .dav
        .has_file(&harness.remote_path_unit(&unit, "resume.state")));
    // The cloud path is the ROM hash: the game name (spaces/Chinese) never
    // reaches WebDAV, so every segment is plain ASCII.
    let puts: Vec<_> = harness
        .dav
        .requests()
        .into_iter()
        .filter(|record| record.method == "PUT")
        .collect();
    // layout.json is uploaded first; the resume upload is the one that must
    // land under the ROM-hash directory.
    let put = puts
        .iter()
        .find(|record| record.path.contains("/resume.state.linkslot-tmp."))
        .expect("resume.state upload missing");
    let hash = unit.rom_sha256.clone().unwrap();
    assert!(
        put.raw_path
            .contains(&format!("/GBA/{hash}/state/mgba/resume.state")),
        "raw path: {}",
        put.raw_path
    );
    assert!(
        puts.iter().all(|record| record.raw_path.is_ascii()),
        "cloud paths should be ASCII: {:?}",
        puts.iter()
            .map(|record| &record.raw_path)
            .collect::<Vec<_>>()
    );
    // The local directory keeps the game name with spaces and Chinese.
    assert_eq!(
        harness.unit_dir(&unit),
        harness.root.join("States/GBA/mgba").join(game)
    );

    // Fresh pull on a new card (no local unit + no cache) restores the save.
    std::fs::remove_dir_all(unit.local_dir(&harness.root)).unwrap();
    linkslot::cache::remove(
        &harness.root,
        &linkslot::unit::UnitSpec::state(&unit, &harness.root),
        &unit,
    )
    .unwrap();
    let (code, stdout, stderr) = harness.cli(&[
        "pull",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        game,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"spaced-resume"
    );
    assert!(stdout.contains("LINKSLOT_RESULT=ok"));
}

#[test]
fn cli_conflict_exits_2() {
    let harness = Harness::new("cli-conflict");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other".as_slice())],
        2,
        "other-dev",
    );
    let (code, stdout, _stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 2);
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_RESULT"),
        Some("conflict")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_ERROR"),
        Some("no-cache")
    );
}

#[test]
fn cli_verify_error_exits_4() {
    use linkslot::hash;
    use linkslot::meta::Meta;

    let harness = Harness::new("cli-verify");
    let unit = harness.unit();
    let files: Vec<(&str, &[u8])> = vec![("resume.state", b"body".as_slice())];
    let hashed = vec![("resume.state".to_string(), 4u64, hash::sha256_hex(b"body"))];
    let mut meta = Meta::build(&unit, 1, "other", &hashed, 0, None);
    meta.files.get_mut("resume.state").unwrap().sha256 = "f".repeat(64);
    meta.aggregate_hash = hash::aggregate_hash(
        meta.files
            .iter()
            .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
    );
    harness.seed_remote_meta(&unit, &meta, &files);

    let (code, stdout, _stderr) = harness.cli(&[
        "pull",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 4);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("error"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("verify"));
}

#[test]
fn cli_missing_config_exits_3() {
    let dir = TestDir::new("cli-noconfig");
    let root = dir.path().join("sd");
    std::fs::create_dir_all(&root).unwrap();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let args: Vec<String> = vec![
        "push".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        GAME.to_string(),
        "--root".to_string(),
        root.to_string_lossy().to_string(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    assert_eq!(code, 3);
    let stdout = String::from_utf8_lossy(&stdout);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("error"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("config"));
    assert!(stderr.is_empty() || !stderr.is_empty()); // stderr may carry the summary
}

#[test]
fn cli_config_init_reports_created_then_existing() {
    let dir = TestDir::new("cli-init");
    let root = dir.path().join("sd");
    std::fs::create_dir_all(&root).unwrap();
    let args: Vec<String> = vec![
        "config".to_string(),
        "init".to_string(),
        "--root".to_string(),
        root.to_string_lossy().to_string(),
    ];
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    assert_eq!(code, 0);
    let contract = parse_contract(&String::from_utf8_lossy(&stdout));
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("config"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert!(root.join("linkSlot/Config/config.toml").exists());

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    assert_eq!(code, 0);
    let contract = parse_contract(&String::from_utf8_lossy(&stdout));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_DETAIL"),
        Some("配置已存在")
    );
}

#[test]
fn cli_net_status_uses_tcp_probe() {
    let harness = Harness::new("cli-net");
    let (code, stdout, _stderr) = harness.cli(&["net", "status"]);
    assert_eq!(code, 0);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("net"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_DETAIL"),
        Some("网络已连接")
    );
}

#[test]
fn cli_usage_errors_are_config_errors() {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = linkslot::cli::execute(&["bogus".to_string()], &mut stdout, &mut stderr);
    assert_eq!(code, 3);
    let contract = parse_contract(&String::from_utf8_lossy(&stdout));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("config"));

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = linkslot::cli::execute(&["--help".to_string()], &mut stdout, &mut stderr);
    assert_eq!(code, 0);
    let help = String::from_utf8_lossy(&stdout);
    assert!(help.contains("linkSlot push"));
    assert!(help.contains("--rom"), "help should document --rom: {help}");
}

#[test]
fn cli_push_full_reuploads_unchanged_files() {
    let harness = Harness::new("cli-full");
    let unit = harness.unit();
    let stamp_state = "2026-10-20_10-00-00.state";
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"cli-v1".as_slice()),
            (stamp_state, b"cli-state".as_slice()),
        ],
    );
    let (code, _stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    harness.seed_local(&unit, &[("resume.state", b"cli-v2".as_slice())]);
    let puts_before = harness.dav.put_paths().len();
    let (code, _stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
        "--full",
        "--verbose",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stderr.contains("全量模式"), "stderr: {stderr}");
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    // Both whitelist files (as temp names) plus meta.json.
    assert_eq!(puts.len(), 3, "unexpected uploads: {puts:?}");
    assert!(
        puts.iter()
            .any(|path| path.contains(&format!("/{stamp_state}.linkslot-tmp."))),
        "unchanged file missing from --full upload: {puts:?}"
    );
}

#[test]
fn cli_force_push_reports_conflict_path() {
    let harness = Harness::new("cli-force");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other".as_slice())],
        2,
        "other-dev",
    );
    let (code, stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
        "--force",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert!(
        contract_value(&contract, "LINKSLOT_CONFLICT")
            .unwrap()
            .contains("/.conflict."),
        "expected cloud backup directory URL in LINKSLOT_CONFLICT"
    );
    let outcome_version = contract_value(&contract, "LINKSLOT_VERSION");
    assert_eq!(outcome_version, Some("3"));
    assert_eq!(
        harness.push(Default::default()).outcome.result(),
        ResultKind::Noop
    );
}

// ---------------------------------------------------------------------------
// ROM SHA-256 identity
// ---------------------------------------------------------------------------

/// A unit whose hash is the fixed fixture ROM's digest.
fn fixture_unit(game: &str) -> Unit {
    Unit::new("GBA", "mgba", game)
        .unwrap()
        .with_rom_sha256(&hash::sha256_hex(FIXTURE_ROM))
        .unwrap()
}

#[test]
fn cli_push_names_the_cloud_unit_by_rom_sha256() {
    let harness = Harness::new("cli-hash-id");
    let unit = harness.unit();
    let digest = unit.rom_sha256.clone().expect("harness unit has a hash");
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"hash-resume".as_slice()),
            ("2026-10-20_10-00-00.state", b"hash-state".as_slice()),
        ],
    );

    let (code, _stdout, stderr) = cli_unit(&harness, "push", GAME, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // Remote layout: <remote_root>/GBA/<64 hex sha256>/state/mgba/...
    let game_dir = harness.remote_game_dir(&unit);
    let remote_dir = harness.remote_unit_dir(&unit);
    assert!(game_dir.ends_with(&digest), "game dir: {game_dir}");
    assert_eq!(remote_dir, format!("{game_dir}/state/mgba"));
    assert!(!remote_dir.contains(GAME), "remote dir: {remote_dir}");
    assert!(harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
    assert!(harness.dav.has_dir(&remote_dir));
    // The old game-name collection must NOT be created.
    assert!(!harness.dav.has_dir(&harness.remote_legacy_unit_dir(&unit)));

    // meta.json carries the hash and still round-trips.
    let meta = Meta::parse(
        &harness
            .dav
            .file(&harness.remote_meta_path(&unit))
            .expect("meta uploaded"),
    )
    .unwrap();
    assert_eq!(meta.unit.rom_sha256, digest);
    assert_eq!(meta.unit.game, GAME);
    meta.validate(&unit).unwrap();

    // Cache: hash-keyed file and unit record carrying the ROM hash.
    let cache = harness.load_cache(&unit).cache.expect("cache written");
    assert_eq!(cache.unit.rom_sha256, digest);
    let cache_spec = linkslot::unit::UnitSpec::state(&unit, &harness.root);
    let cache_json = std::fs::read_to_string(
        linkslot::cache::cache_path(&harness.root, &cache_spec, &unit).unwrap(),
    )
    .expect("cache json on disk");
    assert!(
        cache_json.contains(&format!("\"rom_sha256\": \"{digest}\"")),
        "cache json: {cache_json}"
    );
}

#[test]
fn cli_renamed_rom_reuses_the_same_cloud_unit() {
    let harness = Harness::new("cli-rename");
    let name_a = "Name A";
    let name_b = "Name B";
    // Both ROMs are byte-identical (the fixture payload) but live under
    // different stems, i.e. different local States directories.
    let unit_a = harness.unit_named(name_a);
    let unit_b = harness.unit_named(name_b);
    assert_eq!(unit_a.rom_sha256, unit_b.rom_sha256);
    // Same cloud identity (hash), different local directories (game names).
    assert_eq!(unit_a.id().unwrap(), unit_b.id().unwrap());
    assert_ne!(
        unit_a.local_dir(&harness.root),
        unit_b.local_dir(&harness.root)
    );
    assert!(harness.local_names(&unit_b).is_empty());

    harness.seed_local(&unit_a, &[("resume.state", b"rename-resume".as_slice())]);
    let (code, _stdout, stderr) = cli_unit(&harness, "push", name_a, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // Sync the renamed ROM on the same card: same hash -> same cloud unit, so
    // it downloads into the new local directory.
    let (code, stdout, stderr) = cli_unit(&harness, "sync", name_b, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("LINKSLOT_DIRECTION=download"),
        "stdout: {stdout}"
    );
    assert_eq!(
        harness.local_file(&unit_b, "resume.state").unwrap(),
        b"rename-resume"
    );

    // Exactly one cloud unit directory exists: the hash one.
    assert_eq!(
        harness.remote_unit_dir(&unit_a),
        harness.remote_unit_dir(&unit_b)
    );
    for unit in [&unit_a, &unit_b] {
        assert!(
            !harness.dav.has_dir(&harness.remote_legacy_unit_dir(unit)),
            "game-named cloud dir appeared: {:?}",
            harness.dav.dir_names()
        );
        for record in harness.dav.requests() {
            assert!(
                !record
                    .path
                    .starts_with(&harness.remote_legacy_unit_dir(unit)),
                "legacy path touched: {} {}",
                record.method,
                record.path
            );
        }
    }
}

#[test]
fn cli_same_game_with_different_rom_bytes_keeps_both_cloud_units() {
    let harness = Harness::new("cli-diff-rom");
    let game = "Same Game";
    let unit_v1 = harness.unit_with_rom(game, FIXTURE_ROM);
    harness.seed_local(&unit_v1, &[("resume.state", b"save-v1".as_slice())]);
    let (code, _stdout, stderr) = cli_unit(&harness, "push", game, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // Same stem, different bytes -> different ROM SHA-256 -> different unit.
    let unit_v2 = harness.unit_with_rom(game, OTHER_ROM);
    assert_ne!(unit_v1.rom_sha256, unit_v2.rom_sha256);
    assert_ne!(
        harness.remote_unit_dir(&unit_v1),
        harness.remote_unit_dir(&unit_v2)
    );
    harness.seed_local(&unit_v2, &[("resume.state", b"save-v2".as_slice())]);
    let (code, _stdout, stderr) = cli_unit(&harness, "push", game, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // The two cloud units coexist without overwriting each other.
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit_v1, "resume.state"))
            .unwrap(),
        b"save-v1"
    );
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit_v2, "resume.state"))
            .unwrap(),
        b"save-v2"
    );
    assert!(!harness
        .dav
        .has_dir(&harness.remote_legacy_unit_dir(&unit_v1)));
}

#[test]
fn cli_never_touches_a_legacy_game_name_cloud_dir() {
    let harness = Harness::new("cli-legacy-untouched");
    let unit = harness.unit();
    let legacy_dir = harness.remote_legacy_unit_dir(&unit);
    let legacy_meta = harness.remote_legacy_path(&unit, "meta.json");
    let legacy_resume = harness.remote_legacy_path(&unit, "resume.state");
    harness.dav.insert_file(&legacy_meta, b"legacy-meta-bytes");
    harness
        .dav
        .insert_file(&legacy_resume, b"legacy-resume-bytes");
    let legacy_before = harness.dav.file_names_under(&format!("{legacy_dir}/"));

    // New (hash-identified) content exists on the card.
    harness.seed_local(&unit, &[("resume.state", b"new-hash-resume".as_slice())]);
    let (code, _stdout, stderr) = cli_unit(&harness, "push", GAME, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let (code, _stdout, stderr) = cli_unit(&harness, "sync", GAME, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let (code, _stdout, stderr) = cli_unit(&harness, "pull", GAME, &[]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // The old cloud copy is byte-identical: no read, move, overwrite or delete.
    assert_eq!(
        harness
            .dav
            .file(&legacy_meta)
            .expect("legacy meta still present"),
        b"legacy-meta-bytes"
    );
    assert_eq!(
        harness
            .dav
            .file(&legacy_resume)
            .expect("legacy resume still present"),
        b"legacy-resume-bytes"
    );
    assert_eq!(
        harness.dav.file_names_under(&format!("{legacy_dir}/")),
        legacy_before
    );
    for record in harness.dav.requests() {
        assert!(
            !record.path.starts_with(&legacy_dir),
            "legacy path touched: {} {}",
            record.method,
            record.path
        );
        assert!(
            !record.raw_path.contains("%E9%87%91"),
            "legacy name encoded into a request: {} {}",
            record.method,
            record.raw_path
        );
    }
    // New data went to the hash directory instead.
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"new-hash-resume"
    );
}

#[test]
fn cli_explicit_rom_option_is_hashed_and_overrides_discovery() {
    let harness = Harness::new("cli-explicit-rom");
    let game = "Explicit ROM";
    // The ROM lives outside Games/<platform> and its stem does not matter.
    let rom_dir = harness.root.join("Roms");
    std::fs::create_dir_all(&rom_dir).unwrap();
    let rom = rom_dir.join("cartridge.gba");
    std::fs::write(&rom, OTHER_ROM).unwrap();
    let digest = hash::sha256_hex(OTHER_ROM);
    let unit = Unit::new("GBA", "mgba", game)
        .unwrap()
        .with_rom_sha256(&digest)
        .unwrap();
    harness.seed_local(&unit, &[("resume.state", b"explicit-rom".as_slice())]);

    // Auto-discovery would fail (no Games/GBA/Explicit ROM.gba), but --rom works.
    let rom_arg = rom.to_string_lossy().to_string();
    let (code, _stdout, stderr) = cli_unit(&harness, "push", game, &["--rom", &rom_arg]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
    assert!(harness.dav.has_dir(&harness.remote_unit_dir(&unit)));

    // Even when an auto-discoverable ROM appears, --rom still wins: the same
    // explicit ROM hash is an up-to-date unit, not a new upload.
    harness.write_rom(game, FIXTURE_ROM);
    let (code, stdout, stderr) = cli_unit(&harness, "push", game, &["--rom", &rom_arg]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_RESULT"),
        Some("noop"),
        "stdout: {stdout}"
    );
}

#[test]
fn cli_missing_rom_auto_discovery_exits_3_with_rom_hint() {
    let harness = Harness::new("cli-rom-missing");
    let unit = fixture_unit(GAME);
    harness.seed_local(&unit, &[("resume.state", b"no-rom".as_slice())]);
    // Remove the fixture ROM: there is nothing for --game to hash.
    std::fs::remove_file(harness.rom_path(GAME)).unwrap();

    let (code, stdout, stderr) = cli_unit(&harness, "push", GAME, &[]);
    assert_eq!(code, 3, "stderr: {stderr}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("error"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("config"));
    let detail = contract_value(&contract, "LINKSLOT_DETAIL").unwrap();
    assert!(detail.contains("未找到"), "{detail}");
    assert!(detail.contains("--rom"), "{detail}");
    // Nothing was uploaded, and a hashless unit never reaches the network.
    assert!(harness.dav.file_names().is_empty());
    assert!(
        harness.dav.requests().is_empty(),
        "no request may be made without a ROM hash: {:?}",
        harness.dav.request_methods()
    );
}

#[test]
fn cli_duplicate_rom_auto_discovery_exits_3_listing_candidates() {
    let harness = Harness::new("cli-rom-duplicate");
    // Same stem once more, in a subdirectory of Games/GBA.
    let nested = harness.root.join("Games/GBA/backup");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join(format!("{GAME}.gba")), OTHER_ROM).unwrap();
    let unit = fixture_unit(GAME);
    harness.seed_local(&unit, &[("resume.state", b"dup".as_slice())]);

    let (code, stdout, stderr) = cli_unit(&harness, "push", GAME, &[]);
    assert_eq!(code, 3, "stderr: {stderr}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("config"));
    let detail = contract_value(&contract, "LINKSLOT_DETAIL").unwrap();
    assert!(detail.contains("找到 2 个"), "{detail}");
    assert!(detail.contains("backup"), "{detail}");
    assert!(detail.contains("--rom"), "{detail}");
    assert!(harness.dav.file_names().is_empty());
}

#[test]
fn cli_rom_hash_read_failure_exits_4() {
    let harness = Harness::new("cli-rom-read-fail");
    let missing = harness.root.join("no-such-rom.gba");
    let missing = missing.to_string_lossy().to_string();
    let (code, stdout, _stderr) = cli_unit(&harness, "push", GAME, &["--rom", &missing]);
    assert_eq!(code, 4);
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("error"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("fs"));
    assert!(harness.dav.file_names().is_empty());
}
