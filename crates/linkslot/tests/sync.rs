//! `sync` behavior: the project book v0.7 §5.1 decision matrix, early
//! `LINKSLOT_DIRECTION`, conflict information keys and the two `--resolve`
//! overwrite paths (both must back up the side they overwrite first).
//!
//! Every conflict test also asserts the "no `--resolve` changes nothing"
//! guarantee: local whitelist, cloud files and cache stay byte-identical.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::output::{Direction, ResultKind};
use linkslot::sync::{PushOptions, ResolveSpec, SyncOptions};
use linkslot::unit::Kind;
use linkslot::unit::Unit;
use support::*;

const STAMP1: &str = "2026-10-20_10-00-00";

fn meta_for(unit: &Unit, content: &[u8], version: u64, device: &str) -> Meta {
    let hashed = vec![(
        "resume.state".to_string(),
        content.len() as u64,
        hash::sha256_hex(content),
    )];
    Meta::build(unit, version, device, &hashed, 0, None)
}

fn local_meta(unit: &Unit, content: &[u8], version: u64) -> Meta {
    meta_for(unit, content, version, "rg34xxsp-deadbeef")
}

/// Recursively snapshot the files under `dir` (relative path -> bytes).
fn snapshot_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                out.insert(rel, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn remote_snapshot(harness: &Harness) -> BTreeMap<String, Vec<u8>> {
    harness
        .dav
        .file_names()
        .into_iter()
        .filter_map(|path| harness.dav.file(&path).map(|bytes| (path, bytes)))
        .collect()
}

fn line_pos(stdout: &str, needle: &str) -> usize {
    stdout
        .lines()
        .position(|line| line == needle)
        .unwrap_or_else(|| panic!("missing line {needle:?} in:\n{stdout}"))
}

fn contract_keys(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(key, _)| key.as_str()).collect()
}

fn direction_values(stdout: &str) -> Vec<String> {
    contract_values(stdout, "LINKSLOT_DIRECTION")
}

fn sync_default() -> SyncOptions {
    SyncOptions::default()
}

fn sync_resolve(direction: Direction) -> SyncOptions {
    SyncOptions {
        resolve: ResolveSpec::all(direction),
        ..SyncOptions::default()
    }
}

// ---------------------------------------------------------------------------
// Decision matrix
// ---------------------------------------------------------------------------

#[test]
fn sync_missing_cloud_and_empty_local_is_noop() {
    let harness = Harness::new("sync-empty");
    let result = harness.sync(sync_default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.exit_code(), 0);
    assert!(result.outcome.detail().contains("云端无此游戏存档"));
    assert!(result.outcome.direction().is_none());
    assert!(direction_values(&result.stdout).is_empty());
    let contract = final_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("sync"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert!(contract_value(&contract, "LINKSLOT_VERSION").is_none());
    assert!(harness.load_cache(&harness.unit()).cache.is_none());
    assert!(harness.dav.file_names().is_empty());
}

#[test]
fn sync_first_push_uploads_with_early_direction() {
    let harness = Harness::new("sync-first-push");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-v1".as_slice())]);

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.exit_code(), 0);
    assert_eq!(result.outcome.direction(), Some(Direction::Upload));
    assert_eq!(result.outcome.version(), Some(1));

    // Early direction is emitted before the actual upload, and repeated in the
    // final contract block.
    assert_eq!(direction_values(&result.stdout), ["upload", "upload"]);
    assert!(
        line_pos(&result.stdout, "LINKSLOT_DIRECTION=upload")
            < line_pos(&result.stdout, "LINKSLOT_STAGE=upload")
    );
    assert!(
        line_pos(&result.stdout, "LINKSLOT_STAGE=upload")
            < line_pos(&result.stdout, "LINKSLOT_ACTION=sync")
    );

    let contract = final_contract(&result.stdout);
    assert_eq!(
        contract_keys(&contract),
        [
            "LINKSLOT_ACTION",
            "LINKSLOT_RESULT",
            "LINKSLOT_DIRECTION",
            "LINKSLOT_VERSION",
            "LINKSLOT_ERROR",
            "LINKSLOT_DETAIL",
            "LINKSLOT_STATE_RESULT",
            "LINKSLOT_STATE_DIRECTION",
            "LINKSLOT_STATE_VERSION",
            "LINKSLOT_STATE_CLOUD_VERSION",
            "LINKSLOT_STATE_CLOUD_DEVICE",
            "LINKSLOT_STATE_CLOUD_UPDATED",
            "LINKSLOT_STATE_CLOUD_FILES",
            "LINKSLOT_STATE_LOCAL_VERSION",
            "LINKSLOT_STATE_LOCAL_DEVICE",
            "LINKSLOT_STATE_LOCAL_UPDATED",
            "LINKSLOT_STATE_LOCAL_FILES",
            "LINKSLOT_SAV_RESULT"
        ]
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_DIRECTION"),
        Some("upload")
    );
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("1"));

    let meta = Meta::parse(
        &harness
            .dav
            .file(&harness.remote_meta_path(&unit))
            .expect("meta uploaded"),
    )
    .unwrap();
    assert_eq!(meta.version, 1);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local-v1"
    );
    assert_eq!(
        harness
            .load_cache(&unit)
            .cache
            .expect("cache written")
            .version,
        1
    );
}

#[test]
fn sync_empty_local_with_cloud_pulls() {
    let harness = Harness::new("sync-first-pull");
    let unit = harness.unit();
    let stamp_state = format!("{STAMP1}.state");
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"cloud-resume".as_slice()),
            (stamp_state.as_str(), b"cloud-state".as_slice()),
        ],
        3,
        "rg34xx-other",
    );

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Download));
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(direction_values(&result.stdout), ["download", "download"]);
    assert!(
        line_pos(&result.stdout, "LINKSLOT_DIRECTION=download")
            < line_pos(&result.stdout, "LINKSLOT_STAGE=download")
    );

    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"cloud-resume"
    );
    assert_eq!(
        harness.local_file(&unit, stamp_state.as_str()).unwrap(),
        b"cloud-state"
    );
    assert!(harness.conflict_dirs(&unit).is_empty());
    assert_eq!(
        harness
            .load_cache(&unit)
            .cache
            .expect("cache written")
            .version,
        3
    );
}

#[test]
fn sync_cloud_unchanged_and_local_clean_is_noop() {
    let harness = Harness::new("sync-noop");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same".as_slice())]);
    let cloud = harness.seed_remote(
        &unit,
        &[("resume.state", b"same".as_slice())],
        4,
        "rg34xx-other",
    );
    harness.seed_cache(&unit, &cloud);

    let result = harness.sync(sync_default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert!(result.outcome.detail().contains("已是最新"));
    assert!(result.outcome.direction().is_none());
    assert!(direction_values(&result.stdout).is_empty());
    let contract = final_contract(&result.stdout);
    assert!(contract_value(&contract, "LINKSLOT_VERSION").is_none());
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
}

#[test]
fn sync_cloud_unchanged_local_newer_pushes() {
    let harness = Harness::new("sync-push-local");
    let unit = harness.unit();
    // Cache says we last saw "base"; the cloud still has "base", the card has
    // a newer local state.
    harness.seed_local(&unit, &[("resume.state", b"local-newer".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 1));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"base".as_slice())],
        1,
        "rg34xxsp-deadbeef",
    );

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Upload));
    assert_eq!(result.outcome.version(), Some(2));
    assert_eq!(direction_values(&result.stdout), ["upload", "upload"]);
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 2);
    assert_eq!(meta.previous_version, Some(1));
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local-newer"
    );
}

#[test]
fn sync_cloud_newer_with_clean_local_pulls() {
    let harness = Harness::new("sync-pull-newer");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"old".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"old", 1));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-newer".as_slice())],
        2,
        "rg34xx-other",
    );

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Download));
    assert_eq!(result.outcome.version(), Some(2));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"cloud-newer"
    );
    assert!(harness.conflict_dirs(&unit).is_empty());
    assert_eq!(harness.load_cache(&unit).cache.unwrap().version, 2);
}

#[test]
fn sync_identical_content_updates_missing_cache_without_transfer() {
    let harness = Harness::new("sync-cache-fill");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same-content".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"same-content".as_slice())],
        5,
        "rg34xx-other",
    );

    let result = harness.sync(sync_default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert!(result.outcome.direction().is_none());
    assert!(direction_values(&result.stdout).is_empty());
    let cache = harness.load_cache(&unit).cache.expect("cache created");
    assert_eq!(cache.version, 5);
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"same-content"
    );
}

#[test]
fn sync_conflicts_are_reported_for_every_matrix_row() {
    // Row 4: cloud exists, no cache, local has content.
    let harness = Harness::new("sync-c-nocache");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud".as_slice())],
        2,
        "other-dev",
    );
    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Conflict);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::NoCache);
    assert_eq!(result.outcome.exit_code(), 2);

    // Row 6: cloud newer + local dirty.
    let harness = Harness::new("sync-c-newer");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-dirty".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 1));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-v2".as_slice())],
        2,
        "other-dev",
    );
    let result = harness.sync(sync_default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Conflict,
        "{}",
        result.stderr
    );
    assert_eq!(
        result.outcome.error_kind(),
        linkslot::ErrorKind::RemoteNewer
    );

    // Row 7: forked (same version, different content).
    let harness = Harness::new("sync-c-forked");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-dirty".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 2));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-fork".as_slice())],
        2,
        "other-dev",
    );
    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Forked);
    assert_eq!(result.outcome.exit_code(), 2);

    // Row 8: cloud rolled back.
    let harness = Harness::new("sync-c-rollback");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-dirty".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 3));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-old".as_slice())],
        1,
        "other-dev",
    );
    let result = harness.sync(sync_default());
    assert_eq!(
        result.outcome.error_kind(),
        linkslot::ErrorKind::RemoteRollback
    );
    assert_eq!(result.outcome.exit_code(), 2);
}

#[test]
fn sync_conflict_without_resolve_changes_nothing() {
    let harness = Harness::new("sync-conflict-readonly");
    let unit = harness.unit();
    let stamp_state = format!("{STAMP1}.state");
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"local".as_slice()),
            (stamp_state.as_str(), b"local-state".as_slice()),
        ],
    );
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud".as_slice())],
        2,
        "other-dev",
    );

    let local_before = snapshot_tree(&harness.unit_dir(&unit));
    let remote_before = remote_snapshot(&harness);
    let cache_before = harness.load_cache(&unit).cache;

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Conflict);
    assert_eq!(result.outcome.exit_code(), 2);
    assert!(result.outcome.direction().is_none());
    assert!(direction_values(&result.stdout).is_empty());
    assert!(result.outcome.conflict_path().is_none());

    // No local file (including conflict copies), cloud file or cache changed.
    assert_eq!(snapshot_tree(&harness.unit_dir(&unit)), local_before);
    assert_eq!(remote_snapshot(&harness), remote_before);
    assert_eq!(harness.load_cache(&unit).cache, cache_before);
    assert!(harness.conflict_dirs(&unit).is_empty());
    // Read-only conflict: the CLI only issued GETs.
    assert!(
        harness
            .dav
            .request_methods()
            .iter()
            .all(|method| method == "GET"),
        "unexpected cloud writes: {:?}",
        harness.dav.request_methods()
    );
}

#[test]
fn sync_conflict_reports_cloud_and_local_details() {
    let harness = Harness::new("sync-conflict-info");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-dirty".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 1));
    let cloud = harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-v2".as_slice())],
        2,
        "rg34xx-other",
    );

    let result = harness.sync(sync_default());
    assert_eq!(result.outcome.result(), ResultKind::Conflict);
    let contract = final_contract(&result.stdout);
    let keys = contract_keys(&contract);
    // Fixed order: ERROR/DETAIL then cloud then local info; no CONFLICT key.
    assert_eq!(
        keys,
        [
            "LINKSLOT_ACTION",
            "LINKSLOT_RESULT",
            "LINKSLOT_ERROR",
            "LINKSLOT_DETAIL",
            "LINKSLOT_CLOUD_VERSION",
            "LINKSLOT_CLOUD_DEVICE",
            "LINKSLOT_CLOUD_UPDATED",
            "LINKSLOT_LOCAL_VERSION",
            "LINKSLOT_LOCAL_DEVICE",
            "LINKSLOT_LOCAL_UPDATED",
            "LINKSLOT_STATE_RESULT",
            "LINKSLOT_STATE_ERROR",
            "LINKSLOT_STATE_CLOUD_VERSION",
            "LINKSLOT_STATE_CLOUD_DEVICE",
            "LINKSLOT_STATE_CLOUD_UPDATED",
            "LINKSLOT_STATE_CLOUD_FILES",
            "LINKSLOT_STATE_LOCAL_VERSION",
            "LINKSLOT_STATE_LOCAL_DEVICE",
            "LINKSLOT_STATE_LOCAL_UPDATED",
            "LINKSLOT_STATE_LOCAL_FILES",
            "LINKSLOT_SAV_RESULT",
            "LINKSLOT_CONFLICT_UNIT"
        ]
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CLOUD_VERSION"),
        Some("2")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CLOUD_DEVICE"),
        Some("rg34xx-other")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CLOUD_UPDATED"),
        Some(cloud.updated_at.as_str())
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_LOCAL_VERSION"),
        Some("1")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_LOCAL_DEVICE"),
        Some("rg34xxsp-deadbeef")
    );
    let local_updated = contract_value(&contract, "LINKSLOT_LOCAL_UPDATED").unwrap();
    assert!(local_updated.ends_with('Z'), "{local_updated}");
    assert!(contract_value(&contract, "LINKSLOT_DIRECTION").is_none());
}

#[test]
fn sync_no_cache_conflict_omits_unknown_local_version() {
    let harness = Harness::new("sync-conflict-nocache-info");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud".as_slice())],
        2,
        "other-dev",
    );

    let result = harness.sync(sync_default());
    let contract = final_contract(&result.stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CLOUD_VERSION"),
        Some("2")
    );
    assert!(contract_value(&contract, "LINKSLOT_LOCAL_VERSION").is_none());
    assert_eq!(
        contract_value(&contract, "LINKSLOT_LOCAL_DEVICE"),
        Some("rg34xxsp-deadbeef")
    );
    // Local mtime is known because the whitelist has files.
    assert!(contract_value(&contract, "LINKSLOT_LOCAL_UPDATED").is_some());
}

// ---------------------------------------------------------------------------
// --resolve
// ---------------------------------------------------------------------------

#[test]
fn sync_resolve_upload_backs_up_cloud_then_overwrites() {
    let harness = Harness::new("sync-resolve-up");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-v1".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-old".as_slice())],
        2,
        "other-dev",
    );

    let result = harness.sync(sync_resolve(Direction::Upload));
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.exit_code(), 0);
    assert_eq!(result.outcome.direction(), Some(Direction::Upload));
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(direction_values(&result.stdout), ["upload", "upload"]);

    // Cloud backup directory: old meta.json + every old data file.
    let backup_dirs = harness.remote_conflict_dirs(&unit);
    assert_eq!(backup_dirs.len(), 1, "backups: {backup_dirs:?}");
    let backup_dir = &backup_dirs[0];
    let backup_files = harness.remote_files_in_dir(backup_dir);
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
    let backup_meta = Meta::parse(
        &harness
            .dav
            .file(&format!("{backup_dir}/meta.json"))
            .expect("old meta backed up"),
    )
    .unwrap();
    assert_eq!(backup_meta.version, 2);
    assert_eq!(
        harness
            .dav
            .file(&format!("{backup_dir}/resume.state"))
            .unwrap(),
        b"cloud-old"
    );

    // The overwrite actually happened.
    let meta = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(meta.version, 3);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local-v1"
    );

    // LINKSLOT_CONFLICT points at the cloud backup directory URL.
    let conflict = result
        .outcome
        .conflict_path()
        .expect("backup directory reported")
        .to_string_lossy()
        .to_string();
    assert!(
        conflict.contains("/.conflict."),
        "conflict path: {conflict}"
    );
    // Hash-identified units are per-segment encoded plain ASCII in the URL.
    let hash = unit.rom_sha256.clone().unwrap();
    assert!(
        conflict.contains(&format!("/GBA/{hash}/state/mgba/.conflict.")),
        "backup URL should point below the hash unit dir: {conflict}"
    );
    assert!(
        !conflict.contains(GAME),
        "game name must not appear in the cloud URL: {conflict}"
    );
    let contract = final_contract(&result.stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CONFLICT"),
        Some(conflict.as_str())
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_LOCAL_DEVICE"),
        None,
        "successful resolve upload is not a conflict outcome"
    );
    // The MKCOL + backup PUTs happened before the new data upload.
    let mkcols = harness.dav.request_paths("MKCOL");
    assert!(
        mkcols.iter().any(|path| path.contains("/.conflict.")),
        "MKCOL paths: {mkcols:?}"
    );
    assert!(harness.load_cache(&unit).cache.unwrap().version == 3);
}

#[test]
fn sync_resolve_upload_works_for_forked_conflict() {
    let harness = Harness::new("sync-resolve-up-forked");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 2));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-fork".as_slice())],
        2,
        "other-dev",
    );

    let conflict = harness.sync(sync_default());
    assert_eq!(conflict.outcome.error_kind(), linkslot::ErrorKind::Forked);
    let result = harness.sync(sync_resolve(Direction::Upload));
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(harness.remote_conflict_dirs(&unit).len(), 1);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local"
    );
}

#[test]
fn sync_resolve_download_always_makes_local_conflict_copy() {
    // A forked conflict (both sides moved at the same version): `--resolve
    // download` must copy the local side aside before overwriting it.
    let harness = Harness::new("sync-resolve-down");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-changed".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"local-base", 2));
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"cloud-fork".as_slice()),
            ("2026-10-19_09-00-00.state", b"cloud-state".as_slice()),
        ],
        2,
        "other-dev",
    );

    let before = harness.sync(sync_default());
    assert_eq!(before.outcome.result(), ResultKind::Conflict);
    assert_eq!(before.outcome.error_kind(), linkslot::ErrorKind::Forked);
    assert!(harness.conflict_dirs(&unit).is_empty());

    let requests_before = harness.dav.request_methods().len();
    let result = harness.sync(sync_resolve(Direction::Download));
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Download));
    assert_eq!(result.outcome.version(), Some(2));
    assert_eq!(direction_values(&result.stdout), ["download", "download"]);

    // Local conflict copy with the pre-overwrite files.
    let dirs = harness.conflict_dirs(&unit);
    assert_eq!(dirs.len(), 1, "local conflict copies: {dirs:?}");
    let copy = &dirs[0];
    assert_eq!(
        std::fs::read(copy.join("resume.state")).unwrap(),
        b"local-changed"
    );
    let reported = result
        .outcome
        .conflict_path()
        .expect("local conflict copy reported");
    assert_eq!(reported, copy.as_path());
    let contract = final_contract(&result.stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CONFLICT"),
        Some(copy.to_string_lossy().as_ref())
    );

    // Cloud content replaced the local unit.
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"cloud-fork"
    );
    assert_eq!(
        harness
            .local_file(&unit, "2026-10-19_09-00-00.state")
            .unwrap(),
        b"cloud-state"
    );
    assert_eq!(harness.load_cache(&unit).cache.unwrap().version, 2);

    // A download-only resolve never writes to the cloud.
    let methods: Vec<String> = harness
        .dav
        .request_methods()
        .into_iter()
        .skip(requests_before)
        .collect();
    assert!(
        methods.iter().all(|method| method == "GET"),
        "unexpected cloud writes: {methods:?}"
    );
}

#[test]
fn sync_resolve_download_replaces_dirty_local_and_keeps_copy() {
    let harness = Harness::new("sync-resolve-down-dirty");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-dirty".as_slice())]);
    harness.seed_cache(&unit, &local_meta(&unit, b"base", 1));
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud-v2".as_slice())],
        2,
        "other-dev",
    );

    let before = harness.sync(sync_default());
    assert_eq!(
        before.outcome.error_kind(),
        linkslot::ErrorKind::RemoteNewer
    );

    let result = harness.sync(sync_resolve(Direction::Download));
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    let dirs = harness.conflict_dirs(&unit);
    assert_eq!(dirs.len(), 1);
    assert_eq!(
        std::fs::read(dirs[0].join("resume.state")).unwrap(),
        b"local-dirty"
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"cloud-v2"
    );
}

// ---------------------------------------------------------------------------
// CLI wiring
// ---------------------------------------------------------------------------

#[test]
fn cli_sync_first_push_and_noop_roundtrip() {
    let harness = Harness::new("sync-cli-roundtrip");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"cli-sync".as_slice())]);

    let (code, stdout, stderr) = harness.cli(&[
        "sync",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let contract = final_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("sync"));
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_DIRECTION"),
        Some("upload")
    );
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("1"));
    assert_eq!(direction_values(&stdout), ["upload", "upload"]);

    // Nothing changed since: smart sync reports noop without a direction.
    let (code, stdout, _stderr) = harness.cli(&[
        "sync",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
    ]);
    assert_eq!(code, 0);
    let contract = final_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert!(direction_values(&stdout).is_empty());
}

#[test]
fn cli_sync_conflict_exits_2_and_resolve_upload_works() {
    let harness = Harness::new("sync-cli-resolve");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local".as_slice())]);
    harness.seed_remote(
        &unit,
        &[("resume.state", b"cloud".as_slice())],
        2,
        "other-dev",
    );

    let (code, stdout, _stderr) = harness.cli(&[
        "sync",
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

    let (code, stdout, stderr) = harness.cli(&[
        "sync",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        GAME,
        "--resolve",
        "upload",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let contract = final_contract(&stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_DIRECTION"),
        Some("upload")
    );
    assert!(contract_value(&contract, "LINKSLOT_CONFLICT")
        .unwrap()
        .contains("/.conflict."));
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local"
    );
}

#[test]
fn cli_sync_argument_validation() {
    let harness = Harness::new("sync-cli-args");

    for args in [
        vec![
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            GAME,
            "--resolve",
            "sideways",
        ],
        vec![
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            GAME,
            "--force",
        ],
        vec![
            "push",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            GAME,
            "--resolve",
            "upload",
        ],
    ] {
        let (code, stdout, _stderr) = harness.cli(&args);
        assert_eq!(code, 3, "args {args:?} should be a config error");
        let contract = parse_contract(&stdout);
        assert_eq!(
            contract_value(&contract, "LINKSLOT_ERROR"),
            Some("config"),
            "args {args:?}"
        );
    }

    // Missing --resolve value is also a config error (exit 3).
    let (code, _stdout, _stderr) = harness.cli(&["sync", "--resolve"]);
    assert_eq!(code, 3);

    // Usage mentions the new command and option.
    let (code, stdout, _stderr) = harness.cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("linkSlot sync"));
    assert!(stdout.contains("--resolve"));
    assert!(stdout.contains("--full"));
}

#[test]
fn sync_resolve_ignored_when_there_is_no_conflict() {
    let harness = Harness::new("sync-resolve-noconflict");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"local-only".as_slice())]);

    // `--resolve download` only overrides *conflicts*; a first push still pushes.
    let result = harness.sync(sync_resolve(Direction::Download));
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Upload));
    assert_eq!(result.outcome.version(), Some(1));
    assert!(harness.conflict_dirs(&unit).is_empty());
}

#[test]
fn legacy_game_name_cloud_dir_is_never_read_or_written() {
    // Old linkSlot identified units by game name. Those cloud dirs are not
    // migrated: a hash sync must leave them byte-identical and never address
    // them over WebDAV.
    let harness = Harness::new("sync-legacy-dir");
    let unit = harness.unit();
    let legacy_dir = harness.remote_legacy_unit_dir(&unit);
    harness.dav.insert_file(
        &format!("{legacy_dir}/meta.json"),
        b"legacy-meta".as_slice(),
    );
    harness
        .dav
        .insert_file(&format!("{legacy_dir}/resume.state"), b"legacy".as_slice());
    let legacy_before = harness.dav.file_names_under(&format!("{legacy_dir}/"));

    harness.seed_local(&unit, &[("resume.state", b"local-v1".as_slice())]);
    assert_eq!(
        harness.push(PushOptions::default()).outcome.result(),
        ResultKind::Ok
    );
    assert_eq!(
        harness.sync(sync_default()).outcome.result(),
        ResultKind::Noop
    );

    // The old data is untouched and no request ever mentioned its path.
    assert_eq!(
        harness.dav.file_names_under(&format!("{legacy_dir}/")),
        legacy_before
    );
    assert_eq!(
        harness
            .dav
            .file(&format!("{legacy_dir}/meta.json"))
            .unwrap(),
        b"legacy-meta"
    );
    for record in harness.dav.requests() {
        assert!(
            !record.path.starts_with(&legacy_dir),
            "legacy cloud dir touched: {} {}",
            record.method,
            record.path
        );
        if let Some(destination) = &record.destination {
            assert!(
                !destination.starts_with(&legacy_dir),
                "legacy cloud dir used as MOVE destination: {} {}",
                record.method,
                destination
            );
        }
    }
    // The new unit lives under the hash path.
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .unwrap(),
        b"local-v1"
    );
}

#[test]
fn sync_full_reuploads_whole_unit_when_pushing() {
    let harness = Harness::new("sync-full-push");
    let unit = harness.unit();
    let stamp1_state = format!("{STAMP1}.state");
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"base".as_slice()),
            (stamp1_state.as_str(), b"state".as_slice()),
        ],
    );
    let cloud = harness.meta_from_local(&unit, 1);
    harness.seed_cache(&unit, &cloud);
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"base".as_slice()),
            (stamp1_state.as_str(), b"state".as_slice()),
        ],
        1,
        "rg34xxsp-deadbeef",
    );
    // Only resume.state changed: a plain sync pushes just that, --full pushes
    // every whitelist file.
    harness.seed_local(&unit, &[("resume.state", b"changed".as_slice())]);

    let puts_before = harness.dav.put_paths().len();
    let result = harness.sync(SyncOptions {
        full: true,
        ..SyncOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.direction(), Some(Direction::Upload));
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
    assert_eq!(puts.len(), 3, "resume + unchanged state + meta: {puts:?}");
    assert!(puts
        .iter()
        .any(|path| path.contains(&format!("/{stamp1_state}.linkslot-tmp."))));
}

// ---------------------------------------------------------------------------
// v0.0.10 dual-unit status / stdout contract
// ---------------------------------------------------------------------------

#[test]
fn status_reports_both_units_versions() {
    let harness = Harness::new("status-both");
    let unit = harness.unit();
    harness.seed_remote(&unit, &[("resume.state", b"state".as_slice())], 3, "dev-a");
    harness.seed_remote_sav(&unit, b"SRAM", 5, "dev-b");

    let result = harness.status();
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    let contract = final_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("status"));
    // New per-unit keys.
    assert_eq!(
        contract_value(&contract, "LINKSLOT_STATE_RESULT"),
        Some("ok")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_STATE_CLOUD_VERSION"),
        Some("3")
    );
    assert_eq!(contract_value(&contract, "LINKSLOT_SAV_RESULT"), Some("ok"));
    assert_eq!(
        contract_value(&contract, "LINKSLOT_SAV_CLOUD_VERSION"),
        Some("5")
    );
    assert_eq!(
        contract_value(&contract, "LINKSLOT_SAV_CLOUD_FILES"),
        Some("1")
    );
    // Old single-unit keys are still there for compatibility.
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("3"));
    assert!(
        result.outcome.detail().contains("本地"),
        "{}",
        result.outcome.detail()
    );
}

#[test]
fn sync_only_state_ignores_the_battery_unit() {
    let harness = Harness::new("only-state");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state-v1".as_slice())]);
    // A battery conflict would normally surface; `--only state` must skip it.
    harness.write_local_sav(&unit, "sav", b"local-sram");
    harness.seed_remote_sav(&unit, b"remote-sram", 2, "other-dev");

    let result = harness.sync(SyncOptions {
        only: Some(Kind::State),
        ..SyncOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    // The battery unit was not read, not transferred, not conflicted.
    let contract = final_contract(&result.stdout);
    assert!(contract_value(&contract, "LINKSLOT_STATE_RESULT").is_some());
    assert!(
        contract_value(&contract, "LINKSLOT_SAV_RESULT").is_none(),
        "battery unit must be absent from an --only state run: {stdout}",
        stdout = result.stdout
    );
    assert!(!result.stdout.contains("LINKSLOT_CONFLICT_UNIT"));
    // The battery cloud copy is untouched.
    assert_eq!(harness.remote_sav(&unit).unwrap(), b"remote-sram");
}

#[test]
fn sync_only_sav_ignores_the_state_unit() {
    let harness = Harness::new("only-sav");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"local-sram");

    let result = harness.sync(SyncOptions {
        only: Some(Kind::Sav),
        ..SyncOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    let contract = final_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_SAV_RESULT"), Some("ok"));
    assert!(
        contract_value(&contract, "LINKSLOT_STATE_RESULT").is_none(),
        "state unit must be absent from an --only sav run"
    );
    assert!(!harness
        .dav
        .has_file(&harness.remote_path(&unit, "resume.state")));
}
