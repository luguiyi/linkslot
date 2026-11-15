//! G3 integration coverage: the dual-unit decision matrix driven end to end
//! through the real CLI, plus `--only` / `--resolve` / `--expect`, partial
//! outcomes, changed-since aborts, power-loss safety and the global lock.

mod support;

use support::*;

fn sync_args<'a>(extra: &'a [&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "sync",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ];
    args.extend_from_slice(extra);
    args
}

fn dual_conflict_harness(tag: &str) -> Harness {
    let harness = Harness::new(tag);
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"state-cloud".as_slice())],
        2,
        "other-dev",
    );
    harness.write_local_sav(&unit, "sav", b"sav-local");
    harness.seed_remote_sav(&unit, b"sav-cloud", 3, "other-dev");
    harness
}

#[test]
fn cli_dual_conflict_exits_2_and_lists_both_units() {
    let harness = dual_conflict_harness("g3-dual-conflict");

    let (code, stdout, _stderr) = harness.cli(&sync_args(&[]));
    assert_eq!(code, 2, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_RESULT"),
        Some("conflict")
    );
    // CONFLICT_UNIT is emitted once per conflicting unit, state first.
    let conflict_units: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("LINKSLOT_CONFLICT_UNIT="))
        .map(str::to_string)
        .collect();
    assert_eq!(conflict_units, ["state", "sav"]);
    // Neither side changed.
    assert_eq!(
        harness.local_file(&harness.unit(), "resume.state").unwrap(),
        b"state-local"
    );
    assert_eq!(harness.local_sav(&harness.unit()).unwrap(), b"sav-local");
    assert_eq!(harness.remote_sav(&harness.unit()).unwrap(), b"sav-cloud");
}

#[test]
fn cli_resolve_per_unit_wins_each_side() {
    let harness = dual_conflict_harness("g3-resolve-per-unit");
    let unit = harness.unit();

    let (code, stdout, _stderr) =
        harness.cli(&sync_args(&["--resolve", "state=upload,sav=download"]));
    assert_eq!(code, 0, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_STATE_RESULT"),
        Some("ok")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_STATE_DIRECTION"),
        Some("upload")
    );
    assert_eq!(contract_value(&contract, "LINKSLOT_SAV_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_SAV_DIRECTION"),
        Some("download")
    );

    // State overwrote the cloud; battery overwrote the local file.
    assert_eq!(
        harness.remote_path(&unit, "resume.state"),
        harness.remote_path(&unit, "resume.state")
    );
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"state-local"
    );
    assert_eq!(harness.local_sav(&unit).unwrap(), b"sav-cloud");
}

#[test]
fn cli_single_conflict_lists_one_unit_only() {
    let harness = Harness::new("g3-single-conflict");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"state-cloud".as_slice())],
        2,
        "other-dev",
    );
    // No battery anywhere: it must not become a second conflict.

    let (code, stdout, _stderr) = harness.cli(&sync_args(&[]));
    assert_eq!(code, 2);
    let conflict_units: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("LINKSLOT_CONFLICT_UNIT="))
        .map(str::to_string)
        .collect();
    assert_eq!(conflict_units, ["state"]);
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_SAV_RESULT"),
        Some("noop")
    );
}

#[test]
fn cli_expect_changed_since_aborts_before_any_transfer() {
    let harness = Harness::new("g3-changed-since");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"state-cloud".as_slice())],
        3,
        "other-dev",
    );

    let puts_before = harness.dav.put_paths().len();
    let (code, stdout, _stderr) = harness.cli(&sync_args(&[
        "--resolve",
        "download",
        "--expect",
        "state=99",
    ]));
    assert_eq!(code, 2, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_ERROR"),
        Some("changed-since")
    );
    assert!(contract_value(&contract, "LINKSLOT_DETAIL")
        .unwrap()
        .contains("期望 v99"));
    // Nothing was transferred or backed up.
    assert_eq!(harness.dav.put_paths().len(), puts_before);
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"state-local"
    );
}

#[test]
fn cli_expect_matching_proceeds() {
    let harness = Harness::new("g3-expect-ok");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"state-cloud".as_slice())],
        3,
        "other-dev",
    );

    let (code, stdout, _stderr) = harness.cli(&sync_args(&[
        "--resolve",
        "download",
        "--expect",
        "state=3",
    ]));
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"state-cloud"
    );
}

#[test]
fn cli_partial_reports_one_ok_one_error() {
    let harness = Harness::new("g3-partial");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.write_local_sav(&unit, "sav", b"sav-local");
    // The battery upload fails; the state unit must still commit.
    harness
        .dav
        .fail_path("PUT", "battery.sav.linkslot-tmp", 500, 1);

    let (code, stdout, _stderr) = harness.cli(&sync_args(&[]));
    assert_eq!(code, 1, "{stdout}"); // failing unit's category (server/net)
    let contract = parse_contract(&stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_RESULT"),
        Some("partial")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_STATE_RESULT"),
        Some("ok")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_SAV_RESULT"),
        Some("error")
    );
    // The state side is committed and is never rolled back.
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"state-local"
    );
    assert!(harness.load_cache(&unit).cache.is_some());
}

#[test]
fn cli_only_sav_skips_the_state_unit() {
    let harness = Harness::new("g3-only-sav");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-local".as_slice())]);
    harness.write_local_sav(&unit, "sav", b"sav-local");

    let (code, stdout, _stderr) = harness.cli(&sync_args(&["--only", "sav"]));
    assert_eq!(code, 0, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_SAV_RESULT"), Some("ok"));
    assert!(contract_value(&contract, "LINKSLOT_STATE_RESULT").is_none());
    // The save-state unit was never uploaded.
    assert!(!harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
    assert!(harness.dav.has_file(&harness.remote_sav_path(&unit)));
}

#[test]
fn cli_pull_download_failure_keeps_the_local_battery() {
    let harness = Harness::new("g3-pull-fail");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"sav-local");
    harness.seed_remote_sav(&unit, b"sav-cloud", 2, "other-dev");
    harness.dav.fail_path("GET", "battery.sav", 500, 1);

    // Battery-only sync; a forced download fails mid-transfer.
    let (code, stdout, _stderr) =
        harness.cli(&sync_args(&["--only", "sav", "--resolve", "download"]));
    assert_eq!(code, 1, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("error"));
    // The local battery save survived the failed download.
    assert_eq!(harness.local_sav(&unit).unwrap(), b"sav-local");
    assert!(harness.load_sav_cache(&unit).cache.is_none());
}

#[test]
fn cli_sync_is_serialized_by_the_global_lock() {
    let harness = Harness::new("g3-lock");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"sav-local");

    // Hold the lock as if another sync were in flight.
    let _lock = linkslot::store::UnitLock::acquire(&harness.root).unwrap();
    let (code, stdout, _stderr) = harness.cli(&sync_args(&[]));
    assert_eq!(code, 4, "{stdout}");
    let contract = parse_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("busy"));
    // Nothing was transferred while the lock was held.
    assert!(!harness.dav.has_file(&harness.remote_sav_path(&unit)));
    drop(_lock);
}

#[test]
fn cli_never_reads_old_directories_or_old_caches() {
    let harness = Harness::new("g3-legacy");
    let unit = harness.unit();
    // Pre-v0.0.10 cloud directory and an old cache file.
    let legacy_dir = harness.remote_legacy_unit_dir(&unit);
    harness
        .dav
        .insert_file(&format!("{legacy_dir}/meta.json"), b"legacy-meta");
    harness
        .dav
        .insert_file(&format!("{legacy_dir}/resume.state"), b"legacy-save");
    let old_cache_name = format!(
        "{}.json",
        linkslot::hash::sha256_hex(
            format!("GBA/mgba/{}", unit.rom_sha256.clone().unwrap()).as_bytes()
        )
    );
    let cache_dir = linkslot::cache::cache_dir(&harness.root);
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join(&old_cache_name), b"{}").unwrap();

    harness.seed_local(&unit, &[("resume.state", b"new-save".as_slice())]);
    let (code, _stdout, stderr) = harness.cli(&sync_args(&[]));
    assert_eq!(code, 0, "stderr: {stderr}");

    // The old data is byte-identical and no request touched it.
    assert_eq!(
        harness
            .dav
            .file(&format!("{legacy_dir}/meta.json"))
            .unwrap(),
        b"legacy-meta"
    );
    assert_eq!(
        harness
            .dav
            .file(&format!("{legacy_dir}/resume.state"))
            .unwrap(),
        b"legacy-save"
    );
    for record in harness.dav.requests() {
        assert!(!record.path.starts_with(&legacy_dir), "legacy path touched");
    }
    assert!(
        cache_dir.join(&old_cache_name).exists(),
        "old cache not deleted"
    );
    // New data went to the kind-scoped layout.
    assert!(harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
}
