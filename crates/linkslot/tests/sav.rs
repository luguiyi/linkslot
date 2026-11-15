//! Battery save (`.sav` / `.srm`) push: upload protocol, noop, conflicts,
//! forced backup and failure safety.
//!
//! The battery unit is a single file in the shared `Saves/<platform>/`
//! directory; these tests pin that a push only ever touches its own cloud
//! `sav/` directory and never the save-state unit.

mod support;

use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::sync::PushOptions;
use linkslot::unit::{Kind, BATTERY};
use support::*;

const SRAM_V1: &[u8] = b"SRAM-v1";
const SRAM_V2: &[u8] = b"SRAM-v2-longer";
const REMOTE_SRAM: &[u8] = b"remote-sram";

#[test]
fn first_push_uploads_battery_and_sav_meta() {
    let harness = Harness::new("sav-push-first");
    let unit = harness.unit();
    let local = harness.write_local_sav(&unit, "sav", SRAM_V1);

    let result = harness.push_sav(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(1));

    // Only `battery.sav` + `meta.json` in the battery cloud directory.
    assert_eq!(harness.remote_sav(&unit).unwrap(), SRAM_V1);
    let meta = harness.remote_sav_meta(&unit).expect("sav meta uploaded");
    assert_eq!(meta.schema, 2);
    assert_eq!(meta.kind, Kind::Sav);
    assert!(meta.core.is_none());
    assert!(meta.ring.is_none());
    assert_eq!(meta.version, 1);
    assert_eq!(meta.unit.rom_sha256, unit.rom_sha256.clone().unwrap());
    assert_eq!(meta.files.len(), 1);
    assert_eq!(
        meta.files.get(BATTERY).unwrap().sha256,
        linkslot::hash::sha256_hex(SRAM_V1)
    );
    meta.validate_sav(&unit).unwrap();
    assert_eq!(
        harness
            .dav
            .file_names_under(&harness.remote_sav_unit_dir(&unit))
            .len(),
        2,
        "no temp files may remain: {:?}",
        harness.dav.file_names()
    );

    // The local file is untouched and the battery cache is written.
    assert_eq!(std::fs::read(&local).unwrap(), SRAM_V1);
    let cache = harness
        .load_sav_cache(&unit)
        .cache
        .expect("battery cache written");
    assert_eq!(cache.version, 1);
    assert_eq!(cache.kind, Kind::Sav);
    assert!(cache.core.is_none());

    // The save-state unit is not involved at all.
    assert!(
        !harness.unit_dir(&unit).exists(),
        "state unit must stay absent"
    );
    assert!(
        harness.load_cache(&unit).cache.is_none(),
        "state cache must not be written by a battery push"
    );
    assert!(
        !harness
            .dav
            .has_file(&harness.remote_path(&unit, "resume.state")),
        "state cloud unit must stay untouched"
    );
}

#[test]
fn push_falls_back_to_the_srm_and_keeps_it() {
    let harness = Harness::new("sav-push-srm");
    let unit = harness.unit();
    let srm = harness.write_local_sav(&unit, "srm", SRAM_V1);

    let result = harness.push_sav(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(harness.remote_sav(&unit).unwrap(), SRAM_V1);
    // The emulator's `.srm` is read, never renamed or deleted.
    assert_eq!(std::fs::read(&srm).unwrap(), SRAM_V1);
    assert!(!harness.sav_path(&unit).exists());
    let meta = harness.remote_sav_meta(&unit).unwrap();
    assert_eq!(meta.files.get(BATTERY).unwrap().size, SRAM_V1.len() as u64);
}

#[test]
fn push_accepts_an_empty_battery_file() {
    let harness = Harness::new("sav-push-empty");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"");

    let result = harness.push_sav(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(harness.remote_sav(&unit).unwrap(), b"");
    let meta = harness.remote_sav_meta(&unit).unwrap();
    assert_eq!(meta.files.get(BATTERY).unwrap().size, 0);
    meta.validate_sav(&unit).unwrap();
}

#[test]
fn push_without_a_local_battery_is_a_silent_noop() {
    let harness = Harness::new("sav-push-none");
    let unit = harness.unit();

    let result = harness.push_sav(PushOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.detail(), "本地没有电池存档");
    // Nothing was uploaded, no meta was created and no network round was even
    // needed (the card is checked before connecting).
    assert!(
        harness.dav.requests().is_empty(),
        "{:?}",
        harness.dav.request_methods()
    );
    assert!(harness.dav.file_names().is_empty());
    assert!(harness.load_sav_cache(&unit).cache.is_none());
}

#[test]
fn second_push_is_noop_and_a_change_bumps_the_version() {
    let harness = Harness::new("sav-push-version");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    assert_eq!(
        harness.push_sav(PushOptions::default()).outcome.version(),
        Some(1)
    );

    let puts_before = harness.dav.put_paths().len();
    let again = harness.push_sav(PushOptions::default());
    assert_eq!(again.outcome.result(), ResultKind::Noop, "{}", again.stderr);
    assert_eq!(
        harness.dav.put_paths().len(),
        puts_before,
        "an up-to-date push must not upload"
    );

    harness.write_local_sav(&unit, "sav", SRAM_V2);
    let changed = harness.push_sav(PushOptions::default());
    assert_eq!(
        changed.outcome.result(),
        ResultKind::Ok,
        "{}",
        changed.stderr
    );
    assert_eq!(changed.outcome.version(), Some(2));
    let meta = harness.remote_sav_meta(&unit).unwrap();
    assert_eq!(meta.version, 2);
    assert_eq!(meta.previous_version, Some(1));
    assert_eq!(harness.remote_sav(&unit).unwrap(), SRAM_V2);
    assert_eq!(harness.load_sav_cache(&unit).cache.unwrap().version, 2);
}

#[test]
fn push_full_reuploads_even_when_up_to_date() {
    let harness = Harness::new("sav-push-full");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.push_sav(PushOptions::default());

    let result = harness.push_sav(PushOptions {
        full: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    assert!(result.stderr.contains("全量模式"), "{}", result.stderr);
}

#[test]
fn push_without_cache_refuses_to_overwrite_the_cloud() {
    let harness = Harness::new("sav-push-nocache");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");

    let result = harness.push_sav(PushOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Conflict,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::NoCache);
    assert_eq!(result.outcome.exit_code(), 2);
    // Cloud untouched, local untouched, no cache written.
    assert_eq!(harness.remote_sav(&unit).unwrap(), REMOTE_SRAM);
    assert_eq!(harness.remote_sav_meta(&unit).unwrap().version, 2);
    assert_eq!(harness.local_sav(&unit).unwrap(), SRAM_V1);
    assert!(harness.load_sav_cache(&unit).cache.is_none());
}

#[test]
fn force_push_backs_up_the_cloud_battery_first() {
    let harness = Harness::new("sav-push-force");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");

    let result = harness.push_sav(PushOptions {
        force: true,
        ..PushOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert!(result.outcome.conflict_path().is_some());
    let dir = harness
        .dav
        .dir_names_under(&format!(
            "{}/.conflict.",
            harness.remote_sav_unit_dir(&unit)
        ))
        .into_iter()
        .find(|path| path.contains("/.conflict."))
        .expect("cloud backup directory");
    assert_eq!(
        harness.dav.file(&format!("{dir}/battery.sav")).unwrap(),
        REMOTE_SRAM
    );
    assert_eq!(
        Meta::parse(&harness.dav.file(&format!("{dir}/meta.json")).unwrap())
            .unwrap()
            .version,
        2
    );
    assert_eq!(harness.remote_sav(&unit).unwrap(), SRAM_V1);
}

#[test]
fn failed_commit_removes_the_temp_file_and_keeps_the_old_meta() {
    let harness = Harness::new("sav-push-fail");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");
    // The battery upload succeeds, but committing it (MOVE) fails.
    harness
        .dav
        .fail_path("MOVE", "battery.sav.linkslot-tmp", 500, 1);

    let result = harness.push_sav(PushOptions {
        force: true,
        ..PushOptions::default()
    });
    assert_eq!(
        result.outcome.result(),
        ResultKind::Error,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Server);

    // The old data file and meta are untouched, and the temp upload is gone.
    assert_eq!(harness.remote_sav(&unit).unwrap(), REMOTE_SRAM);
    assert_eq!(harness.remote_sav_meta(&unit).unwrap().version, 2);
    let leftovers: Vec<String> = harness
        .dav
        .file_names_under(&harness.remote_sav_unit_dir(&unit))
        .into_iter()
        .filter(|path| path.contains("linkslot-tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp uploads left behind: {leftovers:?}"
    );
    // No cache entry: the transfer did not commit.
    assert!(harness.load_sav_cache(&unit).cache.is_none());
}

#[test]
fn a_sav_meta_found_where_a_state_meta_belongs_is_rejected() {
    let harness = Harness::new("sav-push-kind");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    // A battery meta planted in the save-state cloud directory.
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");
    let meta_bytes = harness
        .dav
        .file(&harness.remote_sav_meta_path(&unit))
        .unwrap();
    harness
        .dav
        .insert_file(&harness.remote_meta_path(&unit), &meta_bytes);

    // A state read must reject it instead of treating it as a state unit.
    let state = harness.pull(Default::default());
    assert_eq!(
        state.outcome.result(),
        ResultKind::Error,
        "{}",
        state.stderr
    );
    assert_eq!(state.outcome.error_kind(), linkslot::ErrorKind::Server);
    assert!(state.outcome.detail().contains("单元类型不匹配"));
    // The planted meta is still there: nothing was overwritten or deleted.
    assert!(harness.dav.has_file(&harness.remote_meta_path(&unit)));
    assert!(harness.local_file(&unit, "resume.state").is_none());
}

// ---------------------------------------------------------------------------
// Battery save pull
// ---------------------------------------------------------------------------

use linkslot::sync::PullOptions;

#[test]
fn pull_creates_the_saves_directory_and_the_local_file() {
    let harness = Harness::new("sav-pull-first");
    let unit = harness.unit();
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 3, "other-dev");
    assert!(!harness.sav_path(&unit).exists());

    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));

    // `Saves/GBA/` was created and only this game's file was written.
    assert_eq!(harness.local_sav(&unit).unwrap(), REMOTE_SRAM);
    assert!(
        harness.sav_conflict_files(&unit).is_empty(),
        "no backup on a new card"
    );
    let cache = harness
        .load_sav_cache(&unit)
        .cache
        .expect("battery cache written");
    assert_eq!(cache.version, 3);
    assert_eq!(cache.kind, Kind::Sav);
    // The state side is untouched.
    assert!(harness.load_cache(&unit).cache.is_none());
    assert!(!harness.unit_dir(&unit).exists());
    // The downloaded work file was consumed by the install.
    let work = linkslot::store::sav_work_dir(&harness.root, unit.rom_sha256.as_deref().unwrap());
    assert!(!work.join(linkslot::store::SAV_INCOMING).exists());
}

#[test]
fn pull_overwrites_a_dirty_local_file_after_backing_it_up() {
    let harness = Harness::new("sav-pull-dirty");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");

    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(harness.local_sav(&unit).unwrap(), REMOTE_SRAM);

    // The unsynced local save is preserved next to it.
    let backups = harness.sav_conflict_files(&unit);
    assert_eq!(backups.len(), 1, "backups: {backups:?}");
    let name = backups[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(name.starts_with(&format!("{GAME}.conflict.")), "{name}");
    assert!(name.ends_with(".sav"), "{name}");
    assert_eq!(std::fs::read(&backups[0]).unwrap(), SRAM_V1);
    assert_eq!(
        result
            .outcome
            .conflict_path()
            .map(|path| path.to_path_buf()),
        Some(backups[0].clone())
    );
}

#[test]
fn pull_keeps_the_srm_and_writes_a_sav() {
    let harness = Harness::new("sav-pull-srm");
    let unit = harness.unit();
    let srm = harness.write_local_sav(&unit, "srm", b"old-srm-bytes");
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");

    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    // The pull always writes `.sav`, and never deletes the emulator's `.srm`.
    assert_eq!(harness.local_sav(&unit).unwrap(), REMOTE_SRAM);
    assert_eq!(std::fs::read(&srm).unwrap(), b"old-srm-bytes");
    // The old `.srm` was backed up under the `.sav` conflict name.
    let backup = harness
        .sav_conflict_files(&unit)
        .into_iter()
        .next()
        .expect("srm backup");
    assert_eq!(std::fs::read(backup).unwrap(), b"old-srm-bytes");
}

#[test]
fn pull_up_to_date_is_noop_and_downloads_no_data() {
    let harness = Harness::new("sav-pull-noop");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.push_sav(PushOptions::default());

    let gets_before = harness.dav.get_paths().len();
    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.detail(), "已是最新");
    let gets: Vec<String> = harness
        .dav
        .get_paths()
        .into_iter()
        .skip(gets_before)
        .collect();
    assert!(
        !gets.iter().any(|path| path.ends_with("/battery.sav")),
        "up-to-date pull must not download the battery: {gets:?}"
    );
}

#[test]
fn pull_hash_mismatch_leaves_the_local_file_untouched() {
    let harness = Harness::new("sav-pull-corrupt");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.seed_remote_sav(&unit, REMOTE_SRAM, 2, "other-dev");
    // The cloud body no longer matches the hash in meta.json.
    harness.dav.corrupt(&harness.remote_sav_path(&unit));

    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Error,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Verify);
    assert_eq!(
        harness.local_sav(&unit).unwrap(),
        SRAM_V1,
        "verification must fail before anything local changes"
    );
    assert!(harness.sav_conflict_files(&unit).is_empty());
    assert!(harness.load_sav_cache(&unit).cache.is_none());
    // The work file is not installed; the next pull cleans it up.
    let work = linkslot::store::sav_work_dir(&harness.root, unit.rom_sha256.as_deref().unwrap());
    assert!(!harness.sav_path(&unit).with_extension("tmp").exists());
    assert!(!work.join(linkslot::store::SAV_INCOMING).exists());
}

#[test]
fn pull_without_a_cloud_battery_is_a_noop() {
    let harness = Harness::new("sav-pull-none");
    let unit = harness.unit();

    let result = harness.pull_sav(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.detail(), "云端无此游戏存档");
    assert!(!harness.sav_path(&unit).exists());
    assert!(harness.dav.file_names().is_empty());
}

#[test]
fn force_pull_replaces_and_backs_up_even_when_up_to_date() {
    let harness = Harness::new("sav-pull-force");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", SRAM_V1);
    harness.push_sav(PushOptions::default());

    let result = harness.pull_sav(PullOptions {
        force: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(1));
    assert_eq!(harness.local_sav(&unit).unwrap(), SRAM_V1);
    let backup = harness
        .sav_conflict_files(&unit)
        .into_iter()
        .next()
        .expect("--force backs the local save up");
    assert_eq!(std::fs::read(backup).unwrap(), SRAM_V1);
}
