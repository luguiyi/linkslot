//! G1 foundation tests: the v0.0.10 unit model, cloud layout, meta-kind
//! isolation and cache isolation.
//!
//! These tests are deliberately integration-level: they drive the real
//! push/pull/sync phases against the mock WebDAV server, so a regression in the
//! model layer that only shows up end-to-end is caught here as well.

mod support;

use linkslot::cache::{self, Cache};
use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::sync::{PullOptions, PushOptions};
use linkslot::unit::{self, Kind, Unit, UnitSpec};
use support::*;

// ---------------------------------------------------------------------------
// UnitSpec: paths and validation
// ---------------------------------------------------------------------------

#[test]
fn unit_spec_paths_are_kind_scoped() {
    let harness = Harness::new("found-paths");
    let unit = harness.unit();
    let digest = unit.rom_sha256.clone().expect("fixture unit has a hash");
    let root = harness.root.as_path();

    let state = UnitSpec::state(&unit, root);
    assert_eq!(state.kind, Kind::State);
    assert_eq!(
        state.remote_segments(&unit).unwrap(),
        ["GBA", digest.as_str(), "state", "mgba"]
    );
    assert_eq!(
        state.key(&unit).unwrap(),
        format!("state/GBA/mgba/{digest}")
    );
    assert_eq!(
        state.local.path(),
        harness.root.join("States/GBA/mgba").join(GAME)
    );

    let sav = UnitSpec::sav(&unit, root);
    assert_eq!(sav.kind, Kind::Sav);
    assert_eq!(
        sav.remote_segments(&unit).unwrap(),
        ["GBA", digest.as_str(), "sav"]
    );
    assert_eq!(sav.key(&unit).unwrap(), format!("sav/GBA/-/{digest}"));
    assert_eq!(
        sav.local.path(),
        harness.root.join("Saves/GBA").join(format!("{GAME}.sav"))
    );

    // Every identity dimension is part of the key, so the two units of one
    // game never share a cache entry or a cloud directory.
    assert_ne!(
        state.remote_segments(&unit).unwrap(),
        sav.remote_segments(&unit).unwrap()
    );
    assert_ne!(state.key(&unit).unwrap(), sav.key(&unit).unwrap());
}

#[test]
fn invalid_unit_inputs_are_rejected() {
    // platform
    assert!(Unit::new("gba", "mgba", "x").is_err());
    assert!(Unit::new("GBA", "mgba", "").is_err());
    // core restriction
    assert!(Unit::new("GB", "gpsp", "x").is_err());
    // game: separators, traversal, control characters, trailing dots/spaces
    for bad in [
        "a/b",
        "a\\b",
        "..",
        ".",
        "a:b",
        "line\nbreak",
        "trail.",
        "trail ",
    ] {
        assert!(
            Unit::new("GBA", "mgba", bad).is_err(),
            "game {bad:?} should be rejected"
        );
    }
    // ROM digest: must be 64 hex characters
    let unit = Unit::new("GBA", "mgba", GAME).unwrap();
    for bad in ["", "abc", &"z".repeat(64), &"a".repeat(63)] {
        assert!(
            unit.clone().with_rom_sha256(bad).is_err(),
            "digest {bad:?} should be rejected"
        );
    }
    // A hashless unit has no cloud identity: path generation is an error and
    // never falls back to the game name.
    let plain = Unit::new("GBA", "mgba", GAME).unwrap();
    for spec in [
        UnitSpec::state(&plain, &harness_root()),
        UnitSpec::sav(&plain, &harness_root()),
    ] {
        assert!(spec.remote_segments(&plain).is_err());
        assert!(spec.key(&plain).is_err());
    }
    assert!(plain.remote_segments().is_empty());
}

fn harness_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/mnt/sdcard")
}

#[test]
fn local_targets_stay_inside_their_own_layout() {
    let harness = Harness::new("found-local");
    let unit = harness.unit();
    let root = harness.root.as_path();
    // The state directory keeps the game name; the battery file keeps it too,
    // but lives in the shared Saves/<platform>/ directory.
    assert_eq!(
        unit.local_dir(root),
        root.join("States/GBA/mgba").join(GAME)
    );
    assert_eq!(
        unit.local_sav_path(root),
        root.join("Saves/GBA").join(format!("{GAME}.sav"))
    );
    assert_eq!(
        unit.local_srm_path(root),
        root.join("Saves/GBA").join(format!("{GAME}.srm"))
    );
    // `Saves/<platform>` is shared: the unit only owns `<game>.sav`.
    assert_eq!(unit.sav_dir(root), root.join("Saves/GBA"));
}

// ---------------------------------------------------------------------------
// layout.json
// ---------------------------------------------------------------------------

#[test]
fn status_and_pull_validate_the_layout_marker() {
    let harness = Harness::new("found-layout");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"resume".as_slice())]);
    harness.seed_remote(&unit, &[("resume.state", b"cloud".as_slice())], 2, "other");

    // Compatible marker: status reports both versions.
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
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");

    // Incompatible marker: status refuses instead of guessing.
    harness.seed_layout_bytes(br#"{"layout":99,"app":"linkSlot","units":["state","sav"]}"#);
    let (code, stdout, _stderr) = harness.cli(&[
        "status",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 4, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("verify"));

    // A cloud with no marker at all is rejected on pull too.
    harness.remove_remote_file(&harness.remote_layout_path());
    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Error);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Verify);
    // The local save is byte-identical: nothing was downloaded.
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"resume"
    );
}

#[test]
fn an_empty_cloud_needs_no_marker_and_is_left_untouched() {
    let harness = Harness::new("found-empty");
    let unit = harness.unit();

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    // A read-only "nothing to do" must not create a marker or any file.
    assert!(harness.dav.file_names().is_empty());
    assert!(harness.dav.dir_names().is_empty());
    assert!(!harness.dav.has_file(&harness.remote_layout_path()));
    assert!(harness.local_file(&unit, "resume.state").is_none());
}

// ---------------------------------------------------------------------------
// meta kind isolation
// ---------------------------------------------------------------------------

#[test]
fn a_battery_meta_is_never_accepted_as_a_state_meta() {
    let harness = Harness::new("found-kind");
    let unit = harness.unit();
    // A perfectly valid sav meta, written where the state meta belongs.
    let sav_meta = Meta::build_sav(
        &unit,
        4,
        "other",
        &("battery.sav".to_string(), 4, hash::sha256_hex(b"SRAM")),
        None,
    );
    sav_meta.validate_sav(&unit).unwrap();
    harness.seed_remote_meta(&unit, &sav_meta, &[("battery.sav", b"SRAM".as_slice())]);
    harness.seed_layout();

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Error,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Server);
    assert!(
        result.outcome.detail().contains("单元类型不匹配"),
        "{}",
        result.outcome.detail()
    );
    // Nothing was downloaded and no cache entry was written.
    assert!(harness.local_file(&unit, "battery.sav").is_none());
    assert!(harness.local_file(&unit, "resume.state").is_none());
    assert!(harness.load_cache(&unit).cache.is_none());
}

#[test]
fn meta_with_a_damaged_file_table_aborts_without_transfer() {
    let harness = Harness::new("found-damaged-meta");
    let unit = harness.unit();
    let mut meta = Meta::build(
        &unit,
        3,
        "other",
        &[("resume.state".to_string(), 6, hash::sha256_hex(b"resume"))],
        0,
        None,
    );
    // Tamper with the file table but keep the aggregate: the meta must be
    // rejected instead of being used for a download.
    meta.files.get_mut("resume.state").unwrap().sha256 = "f".repeat(64);
    harness
        .dav
        .insert_file(&harness.remote_meta_path(&unit), &meta.to_bytes().unwrap());
    harness.seed_layout();

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Error);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Server);
    assert!(harness.local_file(&unit, "resume.state").is_none());
}

// ---------------------------------------------------------------------------
// cache isolation
// ---------------------------------------------------------------------------

#[test]
fn state_and_sav_cache_entries_are_independent() {
    let harness = Harness::new("found-cache");
    let unit = harness.unit();
    let root = harness.root.as_path();
    let state = UnitSpec::state(&unit, root);
    let sav = UnitSpec::sav(&unit, root);

    let state_meta = Meta::build(
        &unit,
        3,
        "dev",
        &[("resume.state".to_string(), 3, hash::sha256_hex(b"abc"))],
        0,
        None,
    );
    let sav_meta = Meta::build_sav(
        &unit,
        9,
        "dev",
        &("battery.sav".to_string(), 4, hash::sha256_hex(b"SRAM")),
        None,
    );
    harness.seed_cache(&unit, &state_meta);
    let remote = linkslot::cache::RemoteRef {
        url: harness.config.webdav().url.clone(),
        remote_root: harness.config.webdav().remote_root.clone(),
    };
    cache::save(
        &harness.root,
        &sav,
        &unit,
        &Cache::from_meta(&sav_meta, &unit, remote),
    )
    .unwrap();

    // Both entries live side by side and read back their own version.
    let state_cache = cache::load(&harness.root, &state, &unit, &harness_remote(&harness))
        .unwrap()
        .cache
        .expect("state cache");
    let sav_cache = cache::load(&harness.root, &sav, &unit, &harness_remote(&harness))
        .unwrap()
        .cache
        .expect("sav cache");
    assert_eq!(state_cache.version, 3);
    assert_eq!(state_cache.kind, Kind::State);
    assert_eq!(sav_cache.version, 9);
    assert_eq!(sav_cache.kind, Kind::Sav);
    assert!(sav_cache.core.is_none());
    assert_ne!(
        cache::cache_path(&harness.root, &state, &unit).unwrap(),
        cache::cache_path(&harness.root, &sav, &unit).unwrap()
    );
}

fn harness_remote(harness: &Harness) -> linkslot::cache::RemoteRef {
    linkslot::cache::RemoteRef {
        url: harness.config.webdav().url.clone(),
        remote_root: harness.config.webdav().remote_root.clone(),
    }
}

// ---------------------------------------------------------------------------
// Legacy data is never read, moved or deleted
// ---------------------------------------------------------------------------

#[test]
fn legacy_directories_and_caches_are_ignored() {
    let harness = Harness::new("found-legacy");
    let unit = harness.unit();
    let legacy_dir = harness.remote_legacy_unit_dir(&unit);
    let legacy_meta = harness.remote_legacy_path(&unit, "meta.json");
    let legacy_resume = harness.remote_legacy_path(&unit, "resume.state");
    harness.dav.insert_file(&legacy_meta, b"legacy-meta");
    harness.dav.insert_file(&legacy_resume, b"legacy-save");

    // A v0.0.9 cache file (game-name key) that must not be read either.
    let legacy_cache_name = format!(
        "{}.json",
        hash::sha256_hex(format!("GBA/mgba/{}", unit.rom_sha256.clone().unwrap()).as_bytes())
    );
    let cache_dir = cache::cache_dir(&harness.root);
    std::fs::create_dir_all(&cache_dir).unwrap();
    let legacy_cache_path = cache_dir.join(&legacy_cache_name);
    std::fs::write(&legacy_cache_path, b"{}").unwrap();

    harness.seed_local(&unit, &[("resume.state", b"new-save".as_slice())]);
    let push = harness.push(PushOptions::default());
    assert_eq!(push.outcome.result(), ResultKind::Ok, "{}", push.stderr);
    let sync = harness.sync(Default::default());
    assert_eq!(sync.outcome.result(), ResultKind::Noop, "{}", sync.stderr);

    // The old cloud copy is byte-identical and was never touched.
    assert_eq!(harness.dav.file(&legacy_meta).unwrap(), b"legacy-meta");
    assert_eq!(harness.dav.file(&legacy_resume).unwrap(), b"legacy-save");
    for record in harness.dav.requests() {
        assert!(
            !record.path.starts_with(&legacy_dir),
            "legacy path touched: {} {}",
            record.method,
            record.path
        );
    }
    // The old cache file was neither read nor deleted, and the new entry uses
    // the kind-scoped name.
    assert!(
        legacy_cache_path.exists(),
        "old cache file must not be deleted"
    );
    let new_cache = cache::cache_path(
        &harness.root,
        &UnitSpec::state(&unit, harness.root.as_path()),
        &unit,
    )
    .unwrap();
    assert_ne!(new_cache, legacy_cache_path);
    assert!(new_cache.exists(), "a kind-scoped cache entry is written");
    assert!(harness.remote_unit_dir(&unit).ends_with("/state/mgba"));
    assert!(harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
}

#[test]
fn the_new_layout_never_uses_the_game_name_in_the_cloud() {
    let harness = Harness::new("found-no-name");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"save".as_slice())]);
    harness.push(PushOptions::default());

    let game_dir = harness.remote_game_dir(&unit);
    let unit_dir = harness.remote_unit_dir(&unit);
    assert_eq!(unit_dir, format!("{game_dir}/state/mgba"));
    assert!(!game_dir.contains(GAME));
    assert!(!unit_dir.contains(GAME));
    // Game-level playtime.json lives next to state/ and sav/, not inside them.
    assert_eq!(
        harness.remote_playtime_path(&unit),
        format!("{game_dir}/{}", linkslot::playtime::PLAYTIME)
    );
    assert!(!harness.remote_playtime_path(&unit).contains("/state/"));
    // And the old game-name collection was never created.
    assert!(!harness.dav.has_dir(&harness.remote_legacy_unit_dir(&unit)));
}

// ---------------------------------------------------------------------------
// Unit-level helpers that must not regress
// ---------------------------------------------------------------------------

#[test]
fn whitelists_and_stamps_stay_kind_specific() {
    assert!(unit::is_meta_file_name("resume.state"));
    assert!(unit::is_meta_file_name("2026-10-20_10-00-00.png"));
    assert!(!unit::is_meta_file_name("battery.sav"));
    assert!(!unit::is_meta_file_name("battery.sav.bak"));
    assert_eq!(
        unit::state_stamp("2026-10-20_10-00-00.state"),
        Some("2026-10-20_10-00-00")
    );
    assert_eq!(unit::png_stamp("resume.png"), None);
    assert_eq!(
        linkslot::meta::kind_whitelist(Kind::Sav).required(),
        "battery.sav"
    );
    assert_eq!(
        linkslot::meta::kind_whitelist(Kind::State).required(),
        "resume.state"
    );
}
