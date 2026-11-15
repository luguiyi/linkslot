//! `playtime.json` end-to-end behavior: after every successful (`ok`/`noop`)
//! push/pull/sync the best-effort playtime step merges the local device counter
//! with the cloud device table. Playtime is NOT part of the save whitelist, so
//! it must never affect the save result, the meta, deletion propagation, the
//! diff plan, conflicts or backup copies.

mod support;

use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::playtime::Playtime;
use linkslot::sync::PushOptions;
use linkslot::unit::Unit;
use support::*;

/// Build a `playtime.json` value. `total_seconds` is only a placeholder here:
/// LinkSlot recomputes it as the sum over `devices` during the merge.
fn playtime(seconds: u64, devices: &[(&str, u64)]) -> Playtime {
    Playtime {
        schema: 1,
        seconds,
        total_seconds: seconds,
        devices: devices
            .iter()
            .map(|(device, seconds)| ((*device).to_string(), *seconds))
            .collect(),
    }
}

fn local_playtime(harness: &Harness, unit: &Unit) -> Playtime {
    let bytes = harness
        .local_playtime(unit)
        .unwrap_or_else(|| panic!("local playtime.json missing for {:?}", unit.game));
    Playtime::parse(&bytes).expect("local playtime.json parses")
}

fn cloud_playtime(harness: &Harness, unit: &Unit) -> Playtime {
    let bytes = harness
        .remote_playtime(unit)
        .unwrap_or_else(|| panic!("cloud playtime.json missing for {:?}", unit.game));
    Playtime::parse(&bytes).expect("cloud playtime.json parses")
}

#[test]
fn push_merges_and_uploads_the_device_table() {
    let harness = Harness::new("playtime-push");
    let unit = harness.unit();
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"playtime-resume".as_slice()),
            ("2026-10-20_10-00-00.state", b"playtime-state".as_slice()),
        ],
    );
    harness.write_local_playtime(&unit, &playtime(120, &[]).to_bytes().unwrap());

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);

    // Cloud copy: merged table, no local `seconds` counter.
    let cloud = cloud_playtime(&harness, &unit);
    assert_eq!(cloud.schema, 1);
    assert_eq!(cloud.devices.get("rg34xxsp-deadbeef"), Some(&120));
    assert_eq!(cloud.total_seconds, 120);
    assert_eq!(
        cloud.seconds, 0,
        "the cloud copy must not carry the local seconds counter"
    );

    // Local copy: seconds/total_seconds/devices written back.
    let local = local_playtime(&harness, &unit);
    assert_eq!(local.seconds, 120);
    assert_eq!(local.total_seconds, 120);
    assert_eq!(local.devices.get("rg34xxsp-deadbeef"), Some(&120));

    // Playtime never enters the save whitelist/meta.
    let meta = Meta::parse(
        &harness
            .dav
            .file(&harness.remote_meta_path(&unit))
            .expect("meta uploaded"),
    )
    .unwrap();
    assert!(!meta.files.contains_key("playtime.json"));
}

#[test]
fn noop_push_merges_other_devices_and_keeps_the_save_result() {
    let harness = Harness::new("playtime-noop");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same-resume".as_slice())]);
    // The cloud already knows another device's play time.
    harness.seed_remote_playtime(&unit, &playtime(0, &[("dev-a", 300)]).to_bytes().unwrap());
    harness.write_local_playtime(&unit, &playtime(100, &[]).to_bytes().unwrap());

    let first = harness.push(PushOptions::default());
    assert_eq!(first.outcome.result(), ResultKind::Ok, "{}", first.stderr);
    let cloud = cloud_playtime(&harness, &unit);
    assert_eq!(cloud.devices.get("dev-a"), Some(&300));
    assert_eq!(cloud.devices.get("rg34xxsp-deadbeef"), Some(&100));
    assert_eq!(
        cloud.total_seconds, 400,
        "total_seconds is the sum over all devices"
    );

    // Only the local play time advances: the save push must stay a noop, while
    // the merged cloud table still gets updated.
    harness.write_local_playtime(&unit, &playtime(150, &[]).to_bytes().unwrap());
    let second = harness.push(PushOptions::default());
    assert_eq!(
        second.outcome.result(),
        ResultKind::Noop,
        "playtime must not change the save result: {}",
        second.stderr
    );
    let cloud = cloud_playtime(&harness, &unit);
    assert_eq!(cloud.devices.get("rg34xxsp-deadbeef"), Some(&150));
    assert_eq!(cloud.total_seconds, 450);
    let local = local_playtime(&harness, &unit);
    assert_eq!(local.seconds, 150);
    assert_eq!(local.total_seconds, 450);
}

#[test]
fn renamed_rom_restores_playtime_into_the_new_local_dir() {
    let harness = Harness::new("playtime-rename");
    let name_a = "Playtime A";
    let name_b = "Playtime B";
    let unit_a = harness.unit_named(name_a);
    harness.seed_local(&unit_a, &[("resume.state", b"renamed-playtime".as_slice())]);
    harness.write_local_playtime(&unit_a, &playtime(90, &[]).to_bytes().unwrap());

    let (code, _stdout, stderr) = harness.cli(&[
        "push",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        name_a,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        cloud_playtime(&harness, &unit_a).devices.get("rg34xxsp-deadbeef"),
        Some(&90)
    );

    // Same ROM bytes under a new stem -> same hash unit, empty local dir.
    let unit_b = harness.unit_named(name_b);
    assert_eq!(unit_a.id().unwrap(), unit_b.id().unwrap());
    assert!(harness.local_playtime(&unit_b).is_none());
    let (code, stdout, stderr) = harness.cli(&[
        "sync",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        name_b,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("LINKSLOT_DIRECTION=download"),
        "stdout: {stdout}"
    );

    // The cloud device table is merged back into the new local directory.
    let local = local_playtime(&harness, &unit_b);
    assert_eq!(local.seconds, 90);
    assert_eq!(local.devices.get("rg34xxsp-deadbeef"), Some(&90));
    assert_eq!(local.total_seconds, 90);
    // The old game-name cloud directory is never touched or created.
    assert!(!harness
        .dav
        .has_dir(&harness.remote_legacy_unit_dir(&unit_b)));
}

#[test]
fn corrupt_cloud_playtime_is_skipped_without_changing_the_result() {
    let harness = Harness::new("playtime-corrupt");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"corrupt-playtime".as_slice())]);
    harness.dav.insert_file(
        &harness.remote_playtime_path(&unit),
        b"{not json".as_slice(),
    );
    harness.write_local_playtime(&unit, &playtime(42, &[]).to_bytes().unwrap());

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert!(
        result.stderr.contains("损坏"),
        "expected a playtime warning: {}",
        result.stderr
    );
    // A cloud file we cannot parse is never overwritten, and the local counter
    // is left as Slot wrote it.
    assert_eq!(
        harness.remote_playtime(&unit).unwrap(),
        b"{not json".to_vec()
    );
    assert_eq!(local_playtime(&harness, &unit).seconds, 42);
}

#[test]
fn playtime_upload_failure_only_warns() {
    let harness = Harness::new("playtime-put-fail");
    let unit = harness.unit();
    harness.seed_local(
        &unit,
        &[("resume.state", b"playtime-fail-resume".as_slice())],
    );
    harness.write_local_playtime(&unit, &playtime(5, &[]).to_bytes().unwrap());
    // Only the best-effort playtime PUT fails; the save transfer succeeded.
    harness.dav.fail_path("PUT", "playtime.json", 500, 1);

    let result = harness.push(PushOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Ok,
        "a playtime failure must not change the save result: {}",
        result.stderr
    );
    assert_eq!(result.outcome.version(), Some(1));
    assert!(
        result.stderr.contains("游戏时长"),
        "expected a playtime warning: {}",
        result.stderr
    );
    assert!(harness.remote_playtime(&unit).is_none());
}

#[test]
fn playtime_is_not_part_of_meta_or_deletion_and_legacy_dirs_stay_untouched() {
    let harness = Harness::new("playtime-meta");
    let unit = harness.unit();
    let stamp = "2026-10-20_10-00-00.state";
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"keep-resume".as_slice()),
            (stamp, b"ring-state".as_slice()),
        ],
    );
    harness.write_local_playtime(&unit, &playtime(33, &[]).to_bytes().unwrap());

    // Old game-name cloud data (playtime included) must never be touched.
    let legacy_dir = harness.remote_legacy_unit_dir(&unit);
    let legacy_playtime = harness.remote_legacy_path(&unit, "playtime.json");
    harness
        .dav
        .insert_file(&legacy_playtime, b"legacy-playtime".as_slice());

    let first = harness.push(PushOptions::default());
    assert_eq!(first.outcome.result(), ResultKind::Ok, "{}", first.stderr);

    // Dropping the ring state propagates to the save files only: playtime is
    // not in meta.json, so it is never part of the deletion list.
    harness.remove_local(&unit, stamp);
    let second = harness.push(PushOptions::default());
    assert_eq!(second.outcome.result(), ResultKind::Ok, "{}", second.stderr);
    assert!(!harness.dav.has_file(&harness.remote_path(&unit, stamp)));

    let meta = Meta::parse(
        &harness
            .dav
            .file(&harness.remote_meta_path(&unit))
            .expect("meta uploaded"),
    )
    .unwrap();
    assert!(!meta.files.contains_key("playtime.json"));
    assert!(
        harness.remote_playtime(&unit).is_some(),
        "playtime must survive the save deletion propagation"
    );

    // The legacy directory stays byte-identical and is never addressed.
    assert_eq!(
        harness.dav.file(&legacy_playtime).unwrap(),
        b"legacy-playtime".to_vec()
    );
    for record in harness.dav.requests() {
        assert!(
            !record.path.starts_with(&legacy_dir),
            "legacy cloud dir touched: {} {}",
            record.method,
            record.path
        );
    }
}

// ---------------------------------------------------------------------------
// v0.0.10 hotfix: a hashless unit has no playtime cloud identity
// ---------------------------------------------------------------------------

#[test]
fn hashless_unit_never_touches_the_cloud() {
    let harness = Harness::new("playtime-hashless");
    // A unit built from game name only: local directory yes, cloud identity no.
    let unit = Unit::new("GBA", "mgba", GAME).unwrap();
    assert!(unit.rom_sha256.is_none());

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    {
        let mut reporter = linkslot::output::Reporter::new(&harness.root, true, &mut out, &mut err);
        linkslot::playtime::sync(&harness.ctx, &harness.config, &unit, &mut reporter);
    }

    // No request at all: not even a GET on the remote root, and no file is
    // created at `<remote_root>/playtime.json`.
    assert!(
        harness.dav.requests().is_empty(),
        "unexpected requests: {:?}",
        harness.dav.request_methods()
    );
    assert!(harness.dav.file_names().is_empty());
    assert!(!harness.dav.has_file(&format!(
        "{}/{}",
        harness.config.webdav().remote_root.trim_end_matches('/'),
        linkslot::playtime::PLAYTIME
    )));

    // The failure is surfaced (as a best-effort warning) and names the cause.
    let stderr = String::from_utf8(err).unwrap();
    assert!(stderr.contains("游戏时长同步失败"), "{stderr}");
    assert!(stderr.contains("SHA-256"), "{stderr}");
}

#[test]
fn hashed_unit_keeps_the_game_level_playtime_path() {
    let harness = Harness::new("playtime-hashed-path");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"playtime-path".as_slice())]);
    harness.write_local_playtime(
        &unit,
        &playtime(60, &[("rg34xxsp-deadbeef", 60)]).to_bytes().unwrap(),
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);

    let digest = unit.rom_sha256.clone().unwrap();
    let expected = format!("/linkSlot/GBA/{digest}/{}", linkslot::playtime::PLAYTIME);
    assert!(
        harness.dav.has_file(&expected),
        "playtime should be at the game level: {:?}",
        harness.dav.file_names()
    );
    // It must not be nested inside the state unit directory.
    assert!(!harness
        .dav
        .has_file(&harness.remote_path(&unit, linkslot::playtime::PLAYTIME)));
}
