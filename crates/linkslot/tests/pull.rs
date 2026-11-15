//! Pull behavior: first pull, no-op matrix, conflict copies, verification
//! failures, local cleanup rules and interrupted-pull recovery.

mod support;

use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::output::ResultKind;
use linkslot::sync::PullOptions;
use linkslot::unit::Unit;
use support::*;

const STAMP1: &str = "2026-10-20_10-00-00";
const REMOTE_RESUME: &[u8] = b"remote-resume-v3";
const REMOTE_STATE: &[u8] = b"remote-state";
const REMOTE_PNG: &[u8] = b"remote-png";
const LOCAL_RESUME: &[u8] = b"local-resume";

fn seed_remote_unit(harness: &Harness, unit: &Unit) -> Meta {
    let stamp_state = format!("{STAMP1}.state");
    let stamp_png = format!("{STAMP1}.png");
    harness.seed_remote(
        unit,
        &[
            ("resume.state", REMOTE_RESUME),
            (stamp_state.as_str(), REMOTE_STATE),
            (stamp_png.as_str(), REMOTE_PNG),
        ],
        3,
        "rg34xx-other",
    )
}

fn local_dir_names(path: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn pull_missing_remote_meta_is_noop() {
    let harness = Harness::new("pull-missing");
    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.exit_code(), 0);
    assert!(result.outcome.detail().contains("云端无此游戏存档"));
    let contract = parse_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_RESULT"), Some("noop"));
    assert!(contract_value(&contract, "LINKSLOT_VERSION").is_none());
}

#[test]
fn first_pull_creates_unit_from_cloud() {
    let harness = Harness::new("pull-first");
    let unit = harness.unit();
    let meta = seed_remote_unit(&harness, &unit);

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.exit_code(), 0);
    assert_eq!(result.outcome.version(), Some(3));
    assert!(result.outcome.conflict_path().is_none());

    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    let stamp_state = format!("{STAMP1}.state");
    let stamp_png = format!("{STAMP1}.png");
    assert_eq!(
        harness.local_file(&unit, stamp_state.as_str()).unwrap(),
        REMOTE_STATE
    );
    assert_eq!(
        harness.local_file(&unit, stamp_png.as_str()).unwrap(),
        REMOTE_PNG
    );

    let stages = stage_lines(&result.stdout);
    for expected in ["net", "connect", "download", "verify", "commit"] {
        assert!(
            stages.iter().any(|stage| stage == expected),
            "missing stage {expected}: {stages:?}"
        );
    }
    let contract = parse_contract(&result.stdout);
    assert_eq!(contract_value(&contract, "LINKSLOT_ACTION"), Some("pull"));
    assert_eq!(contract_value(&contract, "LINKSLOT_VERSION"), Some("3"));
    assert_eq!(contract_value(&contract, "LINKSLOT_ERROR"), Some("none"));

    let cache = harness.load_cache(&unit).cache.expect("cache written");
    assert_eq!(cache.version, 3);
    assert_eq!(cache.aggregate_hash, meta.aggregate_hash);
    // Cloud unchanged by a pull.
    let after = Meta::parse(&harness.dav.file(&harness.remote_meta_path(&unit)).unwrap()).unwrap();
    assert_eq!(after.version, 3);
}

#[test]
fn pull_up_to_date_is_noop_without_version() {
    let harness = Harness::new("pull-noop");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert!(result.outcome.detail().contains("已是最新"));
    let contract = parse_contract(&result.stdout);
    assert!(contract_value(&contract, "LINKSLOT_VERSION").is_none());
}

#[test]
fn pull_does_not_overwrite_local_unpushed_changes() {
    let harness = Harness::new("pull-dirty");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    harness.seed_local(&unit, &[("resume.state", b"locally-dirty".as_slice())]);
    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert!(result.outcome.detail().contains("本地有未推送修改"));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"locally-dirty"
    );
    assert!(harness.conflict_dirs(&unit).is_empty());
}

#[test]
fn pull_cloud_update_with_local_dirty_makes_conflict_copy() {
    let harness = Harness::new("pull-conflict");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    harness.seed_local(&unit, &[("resume.state", b"locally-dirty".as_slice())]);
    let stamp_state = format!("{STAMP1}.state");
    let stamp_png = format!("{STAMP1}.png");
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"remote-v4".as_slice()),
            (stamp_state.as_str(), REMOTE_STATE),
            (stamp_png.as_str(), REMOTE_PNG),
        ],
        4,
        "rg34xx-other",
    );

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(4));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"remote-v4"
    );
    let conflict = result
        .outcome
        .conflict_path()
        .expect("conflict copy path reported");
    let dirs = harness.conflict_dirs(&unit);
    assert_eq!(dirs.len(), 1);
    assert_eq!(conflict, dirs[0]);
    assert_eq!(
        std::fs::read(conflict.join("resume.state")).unwrap(),
        b"locally-dirty"
    );
    let contract = parse_contract(&result.stdout);
    assert_eq!(
        contract_value(&contract, "LINKSLOT_CONFLICT"),
        Some(conflict.to_string_lossy().as_ref())
    );
}

#[test]
fn pull_without_cache_backs_up_local_before_replacing() {
    let harness = Harness::new("pull-nocache");
    let unit = harness.unit();
    let stamp_state = format!("{STAMP1}.state");
    harness.seed_local(
        &unit,
        &[
            ("resume.state", LOCAL_RESUME),
            (stamp_state.as_str(), b"local-state".as_slice()),
        ],
    );
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"remote-new".as_slice()),
            (stamp_state.as_str(), b"remote-state".as_slice()),
        ],
        2,
        "rg34xx-other",
    );

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    let conflict = result.outcome.conflict_path().expect("conflict copy");
    assert_eq!(
        std::fs::read(conflict.join("resume.state")).unwrap(),
        LOCAL_RESUME
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"remote-new"
    );
    let cache = harness.load_cache(&unit).cache.expect("cache created");
    assert_eq!(cache.version, 2);
}

#[test]
fn pull_verification_failure_leaves_local_untouched() {
    let harness = Harness::new("pull-verify");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", LOCAL_RESUME)]);
    // Craft a meta whose digest does not match the uploaded bytes.
    let files: Vec<(&str, &[u8])> = vec![("resume.state", b"corrupt-payload".as_slice())];
    let hashed: Vec<(String, u64, String)> = files
        .iter()
        .map(|(name, bytes)| {
            (
                (*name).to_string(),
                bytes.len() as u64,
                hash::sha256_hex(bytes),
            )
        })
        .collect();
    let mut meta = Meta::build(&unit, 2, "other", &hashed, 0, None);
    meta.files.get_mut("resume.state").unwrap().sha256 = "0".repeat(64);
    meta.aggregate_hash = hash::aggregate_hash(
        meta.files
            .iter()
            .map(|(name, file)| (name.as_str(), file.sha256.as_str())),
    );
    harness.seed_remote_meta(&unit, &meta, &files);

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Error,
        "{}",
        result.stderr
    );
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Verify);
    assert_eq!(result.outcome.exit_code(), 4);
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        LOCAL_RESUME
    );
    assert!(harness.conflict_dirs(&unit).is_empty());
    let unit_dir = harness.unit_dir(&unit);
    let leftovers: Vec<String> = local_dir_names(&unit_dir)
        .into_iter()
        .filter(|name| name.starts_with(".linkslot-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp dirs left behind: {leftovers:?}");
    assert!(harness.load_cache(&unit).cache.is_none());
}

#[test]
fn pull_removes_extra_ring_files_but_keeps_unknown_files() {
    let harness = Harness::new("pull-cleanup");
    let unit = harness.unit();
    let stale_state = "2026-10-18_08-00-00.state";
    let stale_png = "2026-10-18_08-00-00.png";
    let refused = "resume-refused-2026-10-19_09-00-00.state";
    harness.seed_local(
        &unit,
        &[
            ("resume.state", LOCAL_RESUME),
            (stale_state, b"stale-state".as_slice()),
            (stale_png, b"stale-png".as_slice()),
            (refused, b"refused".as_slice()),
            ("notes.txt", b"keep-me".as_slice()),
        ],
    );
    harness.seed_remote(
        &unit,
        &[("resume.state", b"remote-only".as_slice())],
        2,
        "rg34xx-other",
    );

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"remote-only"
    );
    assert!(harness.local_file(&unit, stale_state).is_none());
    assert!(harness.local_file(&unit, stale_png).is_none());
    assert_eq!(harness.local_file(&unit, refused).unwrap(), b"refused");
    assert_eq!(harness.local_file(&unit, "notes.txt").unwrap(), b"keep-me");
}

#[test]
fn pull_force_redownloads_even_when_up_to_date() {
    let harness = Harness::new("pull-force");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());
    harness.seed_local(&unit, &[("resume.state", b"dirty".as_slice())]);

    let result = harness.pull(PullOptions {
        force: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    assert_eq!(harness.conflict_dirs(&unit).len(), 1);
}

#[test]
fn pull_force_makes_conflict_copy_even_when_local_is_clean() {
    let harness = Harness::new("pull-force-clean");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    // First pull leaves a clean local side (local == cache).
    harness.pull(PullOptions::default());

    let result = harness.pull(PullOptions {
        force: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    let dirs = harness.conflict_dirs(&unit);
    assert_eq!(
        dirs.len(),
        1,
        "--force must always back up the local side before overwriting"
    );
    assert_eq!(
        std::fs::read(dirs[0].join("resume.state")).unwrap(),
        REMOTE_RESUME
    );
    let reported = result
        .outcome
        .conflict_path()
        .expect("conflict copy path reported");
    assert_eq!(reported, dirs[0].as_path());
}

#[test]
fn pull_only_downloads_changed_files() {
    let harness = Harness::new("pull-diff");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    assert_eq!(
        harness.pull(PullOptions::default()).outcome.version(),
        Some(3)
    );

    let stamp1_state = format!("{STAMP1}.state");
    let stamp1_png = format!("{STAMP1}.png");
    let stamp2 = "2026-10-19_09-00-00.state".to_string();
    let gets_before = harness.dav.get_paths().len();
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"remote-v4".as_slice()),
            (stamp1_state.as_str(), REMOTE_STATE),
            (stamp1_png.as_str(), REMOTE_PNG),
            (stamp2.as_str(), b"remote-state-2".as_slice()),
        ],
        4,
        "rg34xx-other",
    );

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(4));
    assert!(
        result.stderr.contains("跳过 2 个未变化文件"),
        "stderr: {}",
        result.stderr
    );

    // meta.json + layout.json + the changed resume.state + the new ring state;
    // the two unchanged files are never fetched again. The best-effort
    // playtime merge adds one GET for the (missing) playtime.json.
    let gets: Vec<String> = harness
        .dav
        .get_paths()
        .into_iter()
        .skip(gets_before)
        .collect();
    assert_eq!(gets.len(), 5, "GETs: {gets:?}");
    assert!(gets.iter().any(|path| path.ends_with("/meta.json")));
    assert!(gets.iter().any(|path| path.ends_with("/layout.json")));
    assert!(gets.iter().any(|path| path.ends_with("/resume.state")));
    assert!(gets.iter().any(|path| path.ends_with(&stamp2)));
    assert!(gets.iter().any(|path| path.ends_with("/playtime.json")));
    assert!(!gets.iter().any(|path| path.ends_with(&stamp1_state)));
    assert!(!gets.iter().any(|path| path.ends_with(&stamp1_png)));

    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        b"remote-v4"
    );
    assert_eq!(
        harness.local_file(&unit, &stamp2).unwrap(),
        b"remote-state-2"
    );
    assert_eq!(
        harness.local_file(&unit, &stamp1_state).unwrap(),
        REMOTE_STATE
    );
    assert_eq!(
        harness.local_file(&unit, &stamp1_png).unwrap(),
        REMOTE_PNG,
        "the unchanged thumbnail must be kept byte-identical"
    );
    assert_eq!(harness.load_cache(&unit).cache.unwrap().version, 4);

    // The crash-safety backup and temp directories were dropped on commit.
    let leftovers: Vec<String> = local_dir_names(&harness.unit_dir(&unit))
        .into_iter()
        .filter(|name| name.starts_with(".linkslot"))
        .collect();
    assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
}

#[test]
fn pull_full_redownloads_unchanged_files() {
    let harness = Harness::new("pull-full");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());
    let stamp1_state = format!("{STAMP1}.state");
    let stamp1_png = format!("{STAMP1}.png");

    // Dirty local side + a cloud update: the pull is allowed to replace local.
    harness.seed_local(&unit, &[("resume.state", b"locally-dirty".as_slice())]);
    let gets_before = harness.dav.get_paths().len();
    harness.seed_remote(
        &unit,
        &[
            ("resume.state", b"remote-v4".as_slice()),
            (stamp1_state.as_str(), REMOTE_STATE),
            (stamp1_png.as_str(), REMOTE_PNG),
        ],
        4,
        "rg34xx-other",
    );

    let result = harness.pull(PullOptions {
        full: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(4));
    assert!(
        result.stderr.contains("全量模式"),
        "stderr: {}",
        result.stderr
    );
    let gets: Vec<String> = harness
        .dav
        .get_paths()
        .into_iter()
        .skip(gets_before)
        .collect();
    // meta.json + layout.json + all three cloud files, including the two that
    // were unchanged, plus the best-effort playtime.json GET.
    assert_eq!(gets.len(), 6, "GETs: {gets:?}");
}

#[test]
fn pull_full_refreshes_even_when_up_to_date() {
    let harness = Harness::new("pull-full-noop");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    let gets_before = harness.dav.get_paths().len();
    let result = harness.pull(PullOptions {
        full: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert!(
        result.stderr.contains("全量模式"),
        "stderr: {}",
        result.stderr
    );
    let gets: Vec<String> = harness
        .dav
        .get_paths()
        .into_iter()
        .skip(gets_before)
        .collect();
    // meta.json + layout.json + all three cloud files even though the local
    // side matched, plus the best-effort playtime.json GET.
    assert_eq!(gets.len(), 6, "GETs: {gets:?}");
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
}

#[test]
fn pull_full_overwrites_dirty_local_and_keeps_conflict_copy() {
    let harness = Harness::new("pull-full-dirty");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());
    harness.seed_local(&unit, &[("resume.state", b"locally-dirty".as_slice())]);

    // The cloud has no update; `--full` is a deliberate repair and may replace
    // the unsynced local file, but only after a conflict copy was made.
    let result = harness.pull(PullOptions {
        full: true,
        ..PullOptions::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    let dirs = harness.conflict_dirs(&unit);
    assert_eq!(dirs.len(), 1, "conflict copies: {dirs:?}");
    assert_eq!(
        std::fs::read(dirs[0].join("resume.state")).unwrap(),
        b"locally-dirty"
    );
}

#[test]
fn pull_recovers_partially_committed_diff_pull() {
    // A diff pull that crashes after installing the changed file and moving the
    // unchanged ones back must come back to the pre-pull content on the next
    // pull (the journal distinguishes new files from moved-back originals).
    let harness = Harness::new("pull-diff-recover");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());
    let unit_dir = harness.unit_dir(&unit);
    let stamp1_state = format!("{STAMP1}.state");
    let stamp1_png = format!("{STAMP1}.png");

    {
        let mut backup = linkslot::store::BackupDir::create(&unit_dir).unwrap();
        for (name, _) in linkslot::unit::local_replaceable_files(&unit_dir).unwrap() {
            backup.move_in(&name).unwrap();
        }
        backup.write_journal(&["resume.state".to_string()]).unwrap();
        std::fs::write(unit_dir.join("resume.state"), b"half-pulled").unwrap();
        for name in [&stamp1_state, &stamp1_png] {
            std::fs::rename(backup.path().join(name), unit_dir.join(name)).unwrap();
        }
        std::mem::forget(backup); // "power loss": Drop must not restore here
    }

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    assert_eq!(
        harness.local_file(&unit, &stamp1_state).unwrap(),
        REMOTE_STATE
    );
    assert_eq!(harness.local_file(&unit, &stamp1_png).unwrap(), REMOTE_PNG);
    assert!(harness.conflict_dirs(&unit).is_empty());
    let leftovers: Vec<String> = local_dir_names(&unit_dir)
        .into_iter()
        .filter(|name| name.starts_with(".linkslot"))
        .collect();
    assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
}

#[test]
fn pull_removes_local_extras_without_fetching_unchanged_files() {
    let harness = Harness::new("pull-extras");
    let unit = harness.unit();
    let stale_state = "2026-10-18_08-00-00.state";
    let stale_png = "2026-10-18_08-00-00.png";
    harness.seed_local(
        &unit,
        &[
            ("resume.state", b"same".as_slice()),
            (stale_state, b"stale".as_slice()),
            (stale_png, b"stale-png".as_slice()),
        ],
    );
    harness.seed_remote(
        &unit,
        &[("resume.state", b"same".as_slice())],
        2,
        "rg34xx-other",
    );

    let gets_before = harness.dav.get_paths().len();
    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(2));
    let gets: Vec<String> = harness
        .dav
        .get_paths()
        .into_iter()
        .skip(gets_before)
        .collect();
    // only meta.json, the layout check and the best-effort playtime probe are
    // fetched; no cloud data file is downloaded.
    assert_eq!(
        gets.len(),
        3,
        "only meta.json/layout.json/playtime should be fetched: {gets:?}"
    );
    assert!(gets[0].ends_with("/meta.json"));
    assert!(gets[1].ends_with("/layout.json"));
    assert!(gets[2].ends_with("/playtime.json"));
    assert_eq!(harness.local_file(&unit, "resume.state").unwrap(), b"same");
    assert!(harness.local_file(&unit, stale_state).is_none());
    assert!(harness.local_file(&unit, stale_png).is_none());
    assert!(
        result.stderr.contains("已校验 0 个文件"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn pull_recovers_interrupted_backup_before_acting() {
    let harness = Harness::new("pull-recover");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    // Simulate a pull that died after moving the old resume into the backup.
    let unit_dir = harness.unit_dir(&unit);
    let backup = unit_dir.join(".linkslot-backup-dead");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::rename(unit_dir.join("resume.state"), backup.join("resume.state")).unwrap();
    std::fs::write(unit_dir.join("resume.state"), b"half-pulled").unwrap();

    let result = harness.pull(PullOptions::default());
    assert_eq!(
        result.outcome.result(),
        ResultKind::Noop,
        "{}",
        result.stderr
    );
    // Old resume restored; the half-pulled file was dropped.
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    assert!(!backup.exists());
}

#[test]
fn pull_restores_a_missing_unit_dir_instead_of_noop() {
    // The cache is keyed by the ROM hash, so it can outlive a renamed/deleted
    // local unit directory. A missing directory means "no local history", not
    // "the local side deleted its saves": an explicit pull must restore it.
    let harness = Harness::new("pull-missing-dir");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.pull(PullOptions::default());

    let unit_dir = harness.unit_dir(&unit);
    std::fs::remove_dir_all(&unit_dir).unwrap();
    assert!(!unit_dir.exists());

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(result.outcome.version(), Some(3));
    assert_eq!(
        harness.local_file(&unit, "resume.state").unwrap(),
        REMOTE_RESUME
    );
    let cache = harness.load_cache(&unit).cache.expect("cache rewritten");
    assert_eq!(cache.version, 3);
}

// ---------------------------------------------------------------------------
// v0.0.10 cloud layout marker
// ---------------------------------------------------------------------------

#[test]
fn pull_refuses_a_cloud_that_has_data_but_no_layout_marker() {
    let harness = Harness::new("pull-no-layout");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.remove_remote_file(&harness.remote_layout_path());

    let result = harness.pull(PullOptions::default());
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
    // Nothing was written locally and the cloud copy is untouched.
    assert!(harness.local_file(&unit, "resume.state").is_none());
    assert!(harness.load_cache(&unit).cache.is_none());
    assert!(!harness.dav.has_file(&harness.remote_layout_path()));
}

#[test]
fn pull_rejects_a_layout_from_another_app() {
    let harness = Harness::new("pull-layout-app");
    let unit = harness.unit();
    seed_remote_unit(&harness, &unit);
    harness.seed_layout_bytes(br#"{"layout":1,"app":"otherApp","units":["state","sav"]}"#);

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Error);
    assert_eq!(result.outcome.error_kind(), linkslot::ErrorKind::Verify);
    assert!(harness.local_file(&unit, "resume.state").is_none());
}
