//! Push behavior: first push, no-op, version bump, deletion propagation,
//! conflict matrix, `--force` meta backup, MOVE fallback and cache handling.

mod support;

use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::sync::PushOptions;
use linkslot::unit::Unit;
use support::*;

const STAMP1: &str = "2026-10-20_10-00-00";
const STAMP2: &str = "2026-10-19_09-00-00";
const RESUME_V1: &[u8] = b"resume-v1";
const RESUME_V2: &[u8] = b"resume-v2";
const STATE_V1: &[u8] = b"state-v1";
const PNG_V1: &[u8] = b"png-v1";
const STATE_V0: &[u8] = b"state-v0";

fn seed_default_local(harness: &Harness, unit: &Unit) {
    let stamp1_state = format!("{STAMP1}.state");
    let stamp1_png = format!("{STAMP1}.png");
    let stamp2_state = format!("{STAMP2}.state");
    harness.seed_local(
        unit,
        &[
            ("resume.state", RESUME_V1),
            (stamp1_state.as_str(), STATE_V1),
            (stamp1_png.as_str(), PNG_V1),
            (stamp2_state.as_str(), STATE_V0),
        ],
    );
}

#[test]
fn first_push_uploads_files_meta_and_cache() {
    let harness = Harness::new("push-first");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.exit_code(), 0);
    assert_eq!(result.outcome.version(), Some(1));
    assert!(result.outcome.conflict_path().is_none());

    let contract = parse_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("push"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("1"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("none"));
    assert!(contract_value(&contract, "LINKSLOT_DETAIL").is_some());
    let stages = stage_lines(&result.stdout);
    for expected in ["net", "connect", "upload", "commit"] {
        assert!(
            stages.iter().any(|stage| stage == expected),
            "missing stage {expected}: {stages:?}"
        );
    }

    let meta_bytes = harness
        .dav
        .file(&harness.remote_meta_path(&unit))
        .expect("meta.json uploaded last");
    let meta = Meta::parse(&meta_bytes).unwrap();
    meta.validate(&unit).unwrap();
    assert_eq!(meta.version, 1);
    assert_eq!(meta.unit.game, GAME);
    assert_eq!(meta.device_id, "rg34xxsp-deadbeef");
    let ring = meta
        .ring
        .as_ref()
        .expect("state meta carries the ring summary");
    assert_eq!(ring.count, 2);
    assert_eq!(ring.max, 10);
    assert_eq!(meta.kind, linkslot::unit::Kind::State);
    assert_eq!(meta.core.as_deref(), Some("mgba"));
    assert!(meta.aggregate_hash.starts_with("sha256:"));
    assert_eq!(meta.files.len(), 4);
    assert!(meta.previous_version.is_none());

    // Every uploaded data file matches the hashes in meta.json.
    for (name, file) in &meta.files {
        let bytes = harness
            .dav
            .file(&harness.remote_path(&unit, name))
            .unwrap_or_else(|| panic!("missing remote file {name}"));
        assert_eq!(hash::sha256_hex(&bytes), file.sha256);
        assert_eq!(bytes.len() as u64, file.size);
    }
    // No temp uploads left behind.
    for name in harness.dav.file_names() {
        assert!(!name.contains("linkslot-tmp"), "leftover temp file {name}");
    }
    // Remote collections created.
    let mkcols = harness
        .dav
        .request_methods()
        .iter()
        .filter(|method| method.as_str() == "MKCOL")
        .count();
    assert!(mkcols >= 4, "expected collection creation, got {mkcols}");

    let cache = harness.load_cache(&unit).cache.expect("cache written");
    assert_eq!(cache.version, 1);
    assert_eq!(cache.aggregate_hash, meta.aggregate_hash);
    assert_eq!(cache.unit.game, GAME);
}

#[test]
fn second_push_without_changes_is_noop() {
    let harness = Harness::new("push-noop");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    assert_eq!(
        harness.push(PushOptions::default()).outcome.version(),
        Some(1)
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.exit_code(), 0);
    assert_eq!(result.outcome.version(), None);
    let contract = parse_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("none"));
    assert!(
        contract_value(&contract, "LINKSLOT_VERSION").is_none(),
        "noop must omit VERSION"
    );
    // Content unchanged on the server.
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 1);
}

#[test]
fn push_after_local_change_increments_version() {
    let harness = Harness::new("push-v2");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());

    harness.seed_local(&unit, &[("resume.state", RESUME_V2)]);
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));

    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 2);
    assert_eq!(meta.previous_version, Some(1));
    assert_eq!(meta.previous_version, Some(1));
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        RESUME_V2
    );
    let cache = harness.load_cache(&unit).cache.unwrap();
    assert_eq!(cache.version, 2);
}

#[test]
fn push_propagates_ring_deletion_when_keep_remote_is_false() {
    let harness = Harness::new("push-delete");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    let stamp2_state = format!("{STAMP2}.state");
    assert!(harness
        .dav
        .has_file(&harness.remote_path(&unit, &stamp2_state)));

    harness.remove_local(&unit, &stamp2_state);
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert!(!meta.files.contains_key(&stamp2_state));
    assert!(
        !harness
            .dav
            .has_file(&harness.remote_path(&unit, &stamp2_state)),
        "deleted ring file should have been cleaned from the cloud"
    );
}

#[test]
fn push_keeps_remote_orphans_when_configured() {
    let harness = Harness::with_options("push-keep", 0, true);
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    let stamp2_state = format!("{STAMP2}.state");
    harness.remove_local(&unit, &stamp2_state);
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert!(!meta.files.contains_key(&stamp2_state));
    assert!(
        harness
            .dav
            .has_file(&harness.remote_path(&unit, &stamp2_state)),
        "keep_remote=true must not delete old cloud files"
    );
}

#[test]
fn push_after_local_change_uploads_only_changed_files() {
    let harness = Harness::new("push-diff");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    let puts_before = harness.dav.put_paths().len();

    harness.seed_local(&unit, &[("resume.state", RESUME_V2)]);
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));

    // Only resume.state and meta.json are sent; the two ring files and the
    // thumbnail already match the cloud meta and stay untouched.
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    assert_eq!(puts.len(), 2, "unexpected uploads: {puts:?}");
    assert!(
        puts.iter()
            .any(|path| path.contains("/resume.state.linkslot-tmp.")),
        "missing resume upload: {puts:?}"
    );
    assert!(
        puts.iter()
            .any(|path| path.contains("/meta.json.linkslot-tmp.")),
        "missing meta upload: {puts:?}"
    );
    let stamp1_state = format!("{STAMP1}.state");
    let stamp1_png = format!("{STAMP1}.png");
    assert!(
        !puts.iter().any(|path| path.contains(&stamp1_state)
            || path.contains(&stamp1_png)
            || path.contains(&format!("{STAMP2}.state"))),
        "unchanged files were re-uploaded: {puts:?}"
    );
    assert!(
        result.stderr.contains("跳过 3 个未变化文件"),
        "stderr: {}",
        result.stderr
    );
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, &stamp1_state))
            .unwrap(),
        STATE_V1
    );
    // The new cloud meta still lists the complete local whitelist.
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    let names: Vec<&str> = meta.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "2026-10-19_09-00-00.state",
            "2026-10-20_10-00-00.png",
            "2026-10-20_10-00-00.state",
            "resume.state",
        ]
    );
}

#[test]
fn push_mixed_add_modify_delete_transfers_only_the_diff() {
    let harness = Harness::new("push-mixed");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());

    let stamp3 = "2026-10-21_11-00-00";
    let stamp3_state = format!("{stamp3}.state");
    let stamp3_png = format!("{stamp3}.png");
    let stamp2_state = format!("{STAMP2}.state");
    harness.seed_local(
        &unit,
        &[
            ("resume.state", RESUME_V2),
            (stamp3_state.as_str(), b"new-state".as_slice()),
            (stamp3_png.as_str(), b"new-png".as_slice()),
        ],
    );
    harness.remove_local(&unit, &stamp2_state);

    let puts_before = harness.dav.put_paths().len();
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));

    // Changed resume + the added state/thumbnail + meta; the unchanged ring
    // state and thumbnail are not re-uploaded.
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    assert_eq!(puts.len(), 4, "unexpected uploads: {puts:?}");
    for name in ["resume.state", stamp3_state.as_str(), stamp3_png.as_str()] {
        assert!(
            puts.iter()
                .any(|path| path.contains(&format!("/{name}.linkslot-tmp."))),
            "missing upload of {name}: {puts:?}"
        );
    }
    assert!(!puts
        .iter()
        .any(|path| path.contains(&format!("{STAMP1}.state"))
            || path.contains(&format!("{STAMP1}.png"))));

    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    let names: Vec<&str> = meta.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "2026-10-20_10-00-00.png",
            "2026-10-20_10-00-00.state",
            "2026-10-21_11-00-00.png",
            "2026-10-21_11-00-00.state",
            "resume.state",
        ]
    );
    assert!(!harness
        .dav
        .has_file(&harness.remote_path(&unit, &stamp2_state)));
}

#[test]
fn push_deletion_only_uploads_meta_and_deletes_old_files() {
    let harness = Harness::new("push-delete-only");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    let stamp2_state = format!("{STAMP2}.state");
    harness.remove_local(&unit, &stamp2_state);

    let puts_before = harness.dav.put_paths().len();
    let methods_before = harness.dav.request_methods().len();
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));

    // No data file changed, so only the new meta.json is uploaded (as a temp
    // file first); the removed ring file is deleted after the commit.
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    assert_eq!(puts.len(), 1, "unexpected uploads: {puts:?}");
    assert!(puts[0].contains("/meta.json.linkslot-tmp."), "{puts:?}");
    let methods: Vec<String> = harness
        .dav
        .request_methods()
        .into_iter()
        .skip(methods_before)
        .collect();
    assert!(
        methods.iter().any(|method| method == "DELETE"),
        "expected a cloud delete: {methods:?}"
    );
    // Deletion propagation must happen only after the new meta.json commit.
    let records = harness.dav.requests();
    let records = &records[methods_before..];
    let meta_put = records
        .iter()
        .position(|record| {
            record.method == "PUT" && record.path.contains("/meta.json.linkslot-tmp.")
        })
        .expect("meta.json upload missing");
    let delete = records
        .iter()
        .position(|record| record.method == "DELETE")
        .expect("DELETE missing");
    assert!(
        delete > meta_put,
        "DELETE must follow the meta commit: {:?}",
        records
            .iter()
            .map(|record| format!("{} {}", record.method, record.path))
            .collect::<Vec<_>>()
    );
    assert!(records[delete].path.ends_with(&stamp2_state));
    assert!(
        !harness
            .dav
            .has_file(&harness.remote_path(&unit, &stamp2_state)),
        "removed ring file should be deleted from the cloud"
    );
}

#[test]
fn push_full_reuploads_every_whitelist_file() {
    let harness = Harness::new("push-full");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    harness.seed_local(&unit, &[("resume.state", RESUME_V2)]);

    let puts_before = harness.dav.put_paths().len();
    let result = harness.push(PushOptions {
        full: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    assert!(
        result.stderr.contains("全量模式"),
        "stderr: {}",
        result.stderr
    );
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    // Four whitelist files plus meta.json, even though three were unchanged.
    assert_eq!(puts.len(), 5, "unexpected uploads: {puts:?}");
    let names = [
        "resume.state".to_string(),
        format!("{STAMP1}.state"),
        format!("{STAMP1}.png"),
        format!("{STAMP2}.state"),
    ];
    for name in &names {
        assert!(
            puts.iter()
                .any(|path| path.contains(&format!("/{name}.linkslot-tmp."))),
            "missing upload of {name}: {puts:?}"
        );
    }
}

#[test]
fn push_full_repairs_even_when_up_to_date() {
    let harness = Harness::new("push-full-noop");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());

    let puts_before = harness.dav.put_paths().len();
    let result = harness.push(PushOptions {
        full: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    assert!(
        result.stderr.contains("忽略“已是最新”"),
        "stderr: {}",
        result.stderr
    );
    let puts: Vec<String> = harness
        .dav
        .put_paths()
        .into_iter()
        .skip(puts_before)
        .collect();
    assert_eq!(puts.len(), 5, "unexpected uploads: {puts:?}");
}

#[test]
fn push_force_full_backs_up_cloud_even_when_up_to_date() {
    let harness = Harness::new("push-force-full");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());

    let result = harness.push(PushOptions {
        force: true,
        full: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    // A concurrent --force must back the cloud up before the full re-upload.
    assert_eq!(harness.remote_conflict_dirs(&unit).len(), 1);
    assert!(result.outcome.conflict_path().is_some());
}

#[test]
fn push_requires_resume_state() {
    let harness = Harness::new("push-noresume");
    let unit = harness.unit();
    let stamp1_state = format!("{STAMP1}.state");
    harness.seed_local(&unit, &[(stamp1_state.as_str(), STATE_V1)]);
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Error);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Fs);
    assert_eq!(result.outcome.exit_code(), 4);
    assert!(
        result.outcome.detail().contains("先玩一下"),
        "detail was {}",
        result.outcome.detail()
    );
    assert!(harness.dav.file(&harness.remote_meta_path(&unit)).is_none());
}

#[test]
fn push_without_cache_refuses_to_overwrite_cloud() {
    let harness = Harness::new("push-nocache");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", RESUME_V1)]);
    let remote_meta = harness.seed_remote(
        &unit,
        &[("resume.state", b"other-device".as_slice())],
        2,
        "other-dev",
    );
    let result = harness.push(PushOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Conflict,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::NoCache);
    assert_eq!(result.outcome.exit_code(), 2);
    let contract = parse_contract(&result.stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_RESULT"),
        Some("conflict")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_ERROR"),
        Some("no-cache")
    );
    // Cloud untouched.
    let current =
        Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(current.version, remote_meta.version);
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        RESUME_V1
    );
}

#[test]
fn push_force_overwrites_cloud_and_backs_up_old_meta() {
    let harness = Harness::new("push-force");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", RESUME_V1)]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other-device".as_slice())],
        2,
        "other-dev",
    );

    let result = harness.push(PushOptions {
        force: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    let conflict = result
        .outcome
        .conflict_path()
        .expect("--force must report the cloud backup directory")
        .to_string_lossy()
        .to_string();
    assert!(
        conflict.contains("/.conflict."),
        "conflict path should be the backup directory URL: {conflict}"
    );
    let contract = parse_contract(&result.stdout);
    assert!(contract_value(&contract, "LINKSLOT_CONFLICT").is_some());

    // The old cloud meta.json and the old data files it listed are copied to
    // `<unit>/.conflict.<stamp>/` before the overwrite.
    let backup_dirs = harness.remote_conflict_dirs(&unit);
    assert_eq!(backup_dirs.len(), 1, "backups: {backup_dirs:?}");
    let backup_meta_path = format!("{}/meta.json", backup_dirs[0]);
    let backup_meta = Meta::parse(&harness.dav.file(&backup_meta_path).unwrap()).unwrap();
    assert_eq!(backup_meta.version, 2);
    assert_eq!(
        harness
            .dav
            .file(&format!("{}/resume.state", backup_dirs[0]))
            .unwrap(),
        b"other-device"
    );
    let backup_files = harness.remote_files_in_dir(&backup_dirs[0]);
    assert!(
        backup_files.iter().any(|path| path.ends_with("/meta.json")),
        "backup files: {backup_files:?}"
    );
    assert!(
        backup_files
            .iter()
            .any(|path| path.ends_with("/resume.state")),
        "backup files: {backup_files:?}"
    );
    let puts = harness.dav.put_paths();
    assert!(
        puts.iter().any(|path| path.contains("/.conflict.")),
        "expected backup PUTs, got {puts:?}"
    );

    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        RESUME_V1
    );
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 3);
    assert_eq!(meta.previous_version, Some(2));
}

#[test]
fn push_remote_newer_aborts_without_force() {
    let harness = Harness::new("push-newer");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    let cache_meta = harness.meta_from_local(&unit, 1);
    harness.seed_cache(&unit, &cache_meta);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other-device".as_slice())],
        2,
        "other-dev",
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Conflict);
    assert_eq!(
        result.outcome.error_kind(),
        linkslot::ErrorKind::RemoteNewer
    );
    assert_eq!(result.outcome.exit_code(), 2);
    assert!(result.outcome.detail().contains("先拉取"));
}

#[test]
fn push_forked_and_rollback_are_detected() {
    // Forked: same version, different content.
    let harness = Harness::new("push-forked");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    let cache_meta = harness.meta_from_local(&unit, 2);
    harness.seed_cache(&unit, &cache_meta);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other-device".as_slice())],
        2,
        "other-dev",
    );
    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Forked);
    assert_eq!(result.outcome.exit_code(), 2);

    // Rollback: cloud version lower than cache.
    let harness = Harness::new("push-rollback");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    let cache_meta = harness.meta_from_local(&unit, 3);
    harness.seed_cache(&unit, &cache_meta);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"older".as_slice())],
        1,
        "other-dev",
    );
    let result = harness.push(PushOptions::default());
    assert_eq!(
        result.outcome.error_kind(),
        linkslot::ErrorKind::RemoteRollback
    );
    assert_eq!(result.outcome.exit_code(), 2);
}

#[test]
fn push_force_without_cache_is_allowed() {
    let harness = Harness::new("push-force-nocache");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", RESUME_V1)]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other".as_slice())],
        4,
        "other-dev",
    );
    let result = harness.push(PushOptions {
        force: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(5));
    assert!(result.outcome.conflict_path().is_some());
}

#[test]
fn push_falls_back_to_plain_put_when_move_is_unsupported() {
    let harness = Harness::new("push-nomove");
    harness.dav.set_move_supported(false);
    let unit = harness.unit();
    seed_default_local(&harness, &unit);

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(1));
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    for (name, file) in &meta.files {
        let bytes = harness.dav.file(&harness.remote_path(&unit, name)).unwrap();
        assert_eq!(hash::sha256_hex(&bytes), file.sha256);
    }
    for name in harness.dav.file_names() {
        assert!(!name.contains("linkslot-tmp"), "leftover temp file {name}");
    }
}

#[test]
fn push_sees_conflict_copy_residue_and_refuses() {
    let harness = Harness::new("push-residue");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    let backup_dir = harness.unit_dir(&unit).join(".linkslot-backup-dead");
    std::fs::create_dir_all(&backup_dir).unwrap();
    std::fs::write(backup_dir.join("resume.state"), b"old").unwrap();

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Error);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Fs);
    // Recovery restored the backed-up resume.
    assert_eq!(harness.local_file(&unit, "resume.state").unwrap(), b"old");
}

// ---------------------------------------------------------------------------
// v0.0.10 cloud layout marker
// ---------------------------------------------------------------------------

#[test]
fn first_push_writes_the_layout_marker_before_the_unit() {
    let harness = Harness::new("push-layout");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);

    let bytes = harness
        .dav
        .file(&harness.remote_layout_path())
        .expect("layout.json written by the first push");
    let layout = linkslot::unit::Layout::parse(&bytes).unwrap();
    assert_eq!(layout, linkslot::unit::Layout::current());
    layout.validate().unwrap();

    // The marker is committed before any unit data (a reader must never see
    // unit data under an unmarked root).
    let requests = harness.dav.requests();
    let layout_put = requests
        .iter()
        .position(|record| {
            record.method == "PUT" && record.path.contains("/layout.json.linkslot-tmp.")
        })
        .or_else(|| {
            requests
                .iter()
                .position(|record| record.method == "PUT" && record.path.ends_with("/layout.json"))
        })
        .expect("layout PUT missing");
    let first_unit_put = requests
        .iter()
        .position(|record| record.method == "PUT" && record.path.contains("/state/mgba/"))
        .expect("unit upload missing");
    assert!(layout_put < first_unit_put, "layout must be written first");
}

#[test]
fn second_push_does_not_rewrite_the_layout_marker() {
    let harness = Harness::new("push-layout-once");
    let unit = harness.unit();
    seed_default_local(&harness, &unit);
    harness.push(PushOptions::default());
    let layout_puts_before = harness
        .dav
        .put_paths()
        .iter()
        .filter(|path| {
            path.ends_with("/layout.json") || path.contains("/layout.json.linkslot-tmp.")
        })
        .count();

    harness.seed_local(&unit, &[("resume.state", RESUME_V2)]);
    harness.push(PushOptions::default());
    let layout_puts_after = harness
        .dav
        .put_paths()
        .iter()
        .filter(|path| {
            path.ends_with("/layout.json") || path.contains("/layout.json.linkslot-tmp.")
        })
        .count();
    assert_eq!(
        layout_puts_before, layout_puts_after,
        "an existing marker must not be rewritten"
    );
}

#[test]
fn push_refuses_a_cloud_that_has_data_but_no_layout_marker() {
    let harness = Harness::new("push-no-layout");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", RESUME_V1)]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"other-device".as_slice())],
        2,
        "other-dev",
    );
    harness.remove_remote_file(&harness.remote_layout_path());
    let puts_before = harness.dav.put_paths().len();

    let result = harness.push(PushOptions {
        force: true,
        ..PushOptions::default()
    });
    assert_eq!(
        result.outcome.result(),
        ResultKind::Error,
        "{}",
        result.stderr
    );
    assert_eq!(
        result.outcome.error_kind(),
        linkslot::ErrorKind::Verify,
        "{}",
        result.outcome.detail()
    );
    assert!(
        result.outcome.detail().contains("layout.json"),
        "{}",
        result.outcome.detail()
    );
    // Nothing was uploaded and the old cloud copy is byte-identical.
    assert_eq!(harness.dav.put_paths().len(), puts_before);
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 2);
    assert!(!harness.dav.has_file(&harness.remote_layout_path()));
}

#[test]
fn push_refuses_an_incompatible_layout_marker() {
    for (tag, bytes) in [
        (
            "push-layout-version",
            br#"{"layout":2,"app":"linkSlot","units":["state","sav"]}"#.as_slice(),
        ),
        (
            "push-layout-app",
            br#"{"layout":1,"app":"otherApp","units":["state","sav"]}"#.as_slice(),
        ),
        (
            "push-layout-units",
            br#"{"layout":1,"app":"linkSlot","units":["state"]}"#.as_slice(),
        ),
        ("push-layout-corrupt", b"{not json".as_slice()),
    ] {
        let harness = Harness::new(tag);
        let unit = harness.unit();
        harness.seed_local(&unit, &[("resume.state", RESUME_V1)]);
        harness.seed_remote(
            &unit,
            &[("resume.state", b"other-device".as_slice())],
            2,
            "other-dev",
        );
        harness.seed_layout_bytes(bytes);
        let puts_before = harness.dav.put_paths().len();

        let result = harness.push(PushOptions::default());
        assert_eq!(
            result.outcome.result(),
            ResultKind::Error,
            "{tag}: {}",
            result.stderr
        );
        assert_eq!(
            result.outcome.error_kind(),
            linkslot::ErrorKind::Verify,
            "{tag}"
        );
        assert_eq!(
            harness.dav.put_paths().len(),
            puts_before,
            "{tag}: nothing may be uploaded"
        );
    }
}
