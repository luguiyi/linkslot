//! Lua mod runtime integration tests (only built with `--features mods`).
#![cfg(feature = "mods")]

mod support;

use std::fs;
use std::path::Path;

use linkslot::hash;
use linkslot::output::ResultKind;
use linkslot::sync::{PullOptions, PushOptions, SyncOptions};
use linkslot::unit::Kind;
use support::Harness;

fn write_mod(root: &Path, id: &str, timing: &str, enabled: bool, script: &str) {
    let dir = root.join("linkSlot/mods").join(id);
    let action_dir = dir.join("actions/main");
    fs::create_dir_all(&action_dir).unwrap();
    fs::write(
        dir.join("mod.json"),
        format!("{{\"schema\":2,\"name\":\"{id}\",\"enabled\":{enabled}}}"),
    )
    .unwrap();
    fs::write(
        action_dir.join("action.json"),
        format!(
            "{{\"schema\":1,\"name\":\"动作\",\"timing\":\"{timing}\",\"enabled\":true,\"order\":100}}"
        ),
    )
    .unwrap();
    fs::write(action_dir.join("main.lua"), script).unwrap();
}

fn use_mod_remote_root(harness: &mut Harness, remote_root: &str) {
    let path = harness.root.join("linkSlot/Config/config.toml");
    let text = fs::read_to_string(&path).unwrap();
    let old = "remote_root = \"/linkSlot\"";
    assert!(text.contains(old), "legacy remote_root not found");
    fs::write(
        &path,
        text.replace(old, &format!("remote_root = \"{remote_root}\"")),
    )
    .unwrap();
    harness.config = linkslot::config::Config::load(&harness.root, None).unwrap();
}

#[allow(clippy::too_many_arguments)]
fn write_action(
    root: &Path,
    mod_id: &str,
    action_id: &str,
    name: &str,
    timing: &str,
    enabled: bool,
    order: i32,
    script: &str,
) {
    let dir = root
        .join("linkSlot/mods")
        .join(mod_id)
        .join("actions")
        .join(action_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("action.json"),
        format!(
            "{{\"schema\":1,\"name\":\"{name}\",\"timing\":\"{timing}\",\"enabled\":{enabled},\"order\":{order}}}"
        ),
    )
    .unwrap();
    fs::write(dir.join("main.lua"), script).unwrap();
}

fn write_mod_enabled(root: &Path, mod_id: &str, enabled: bool) {
    let dir = root.join("linkSlot/mods").join(mod_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("mod.json"),
        format!("{{\"schema\":2,\"name\":\"{mod_id}\",\"enabled\":{enabled}}}"),
    )
    .unwrap();
}

#[test]
fn before_upload_writes_are_included_in_the_upload() {
    let harness = Harness::new("mods-before-upload");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"old-bytes")]);
    write_mod(
        &harness.root,
        "writer",
        "before_upload",
        true,
        r#"files.write(ctx.game.state_dir .. "/resume.state", "from-before-upload")"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .as_deref(),
        Some(b"from-before-upload".as_slice())
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").as_deref(),
        Some(b"from-before-upload".as_slice())
    );
}

#[test]
fn after_upload_changes_local_only() {
    let harness = Harness::new("mods-after-upload");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"uploaded")]);
    write_mod(
        &harness.root,
        "writer",
        "after_upload",
        true,
        r#"files.write(ctx.game.state_dir .. "/resume.state", "after-upload-local")"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .as_deref(),
        Some(b"uploaded".as_slice())
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").as_deref(),
        Some(b"after-upload-local".as_slice())
    );
}

#[test]
fn after_download_changes_local_only() {
    let harness = Harness::new("mods-after-download");
    let unit = harness.unit();
    harness.seed_remote(&unit, &[("resume.state", b"cloud")], 3, "device-a");
    write_mod(
        &harness.root,
        "writer",
        "after_download",
        true,
        r#"files.write(ctx.game.state_dir .. "/resume.state", "after-download-local")"#,
    );

    let result = harness.pull(PullOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .as_deref(),
        Some(b"cloud".as_slice())
    );
    assert_eq!(
        harness.local_file(&unit, "resume.state").as_deref(),
        Some(b"after-download-local".as_slice())
    );
}

#[test]
fn before_upload_can_fetch_from_the_compat_webdav_root() {
    let mut harness = Harness::new("mods-compat");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"local-old");
    harness
        .dav
        .insert_file("/compat/GBA/battery.sav", b"fetched-from-other-frontend");
    use_mod_remote_root(&mut harness, "/compat");
    write_mod(
        &harness.root,
        "bridge",
        "before_upload",
        true,
        r#"
local data = webdav.get(ctx.webdav.active, "GBA/battery.sav")
if not data then error("compat save missing") end
files.write(ctx.game.sav_path, data)
"#,
    );

    let result = harness.sync(SyncOptions {
        only: Some(Kind::Sav),
        ..Default::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness.dav.file(&harness.remote_sav_path(&unit)).as_deref(),
        Some(b"fetched-from-other-frontend".as_slice())
    );
    assert_eq!(
        harness.local_sav(&unit).as_deref(),
        Some(b"fetched-from-other-frontend".as_slice())
    );
}

#[test]
fn disabled_and_wrong_timing_scripts_do_not_run() {
    let harness = Harness::new("mods-disabled");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state")]);
    write_mod(
        &harness.root,
        "disabled",
        "before_upload",
        false,
        r#"files.write(ctx.mod.dir .. "/ran", "disabled")"#,
    );
    write_mod(
        &harness.root,
        "wrong",
        "after_download",
        true,
        r#"files.write(ctx.mod.dir .. "/ran", "wrong")"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert!(!harness.root.join("linkSlot/mods/disabled/ran").exists());
    assert!(!harness.root.join("linkSlot/mods/wrong/ran").exists());
}

#[test]
fn sandbox_blocks_config_and_os_but_the_sync_still_succeeds() {
    let harness = Harness::new("mods-sandbox");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state")]);
    write_mod(
        &harness.root,
        "sandbox",
        "before_upload",
        true,
        r#"
assert(os == nil, "os must be nil")
assert(io == nil, "io must be nil")
assert(package == nil, "package must be nil")
assert(debug == nil, "debug must be nil")
assert(require == nil, "require must be nil")
assert(load == nil, "load must be nil")
assert(loadfile == nil, "loadfile must be nil")
assert(dofile == nil, "dofile must be nil")
local ok_read = pcall(files.read, "linkSlot/Config/config.toml")
if ok_read then error("config was readable") end
local ok_write = pcall(files.write, "linkSlot/Config/evil.txt", "x")
if ok_write then error("config was writable") end
if pcall(files.read, "States/../linkSlot/Config/config.toml") then
  error("traversal was readable")
end
if pcall(files.write, "States/evil.txt", "x") then
  error("bad state extension was writable")
end
if pcall(files.write, "linkSlot/mods/another-mod/evil", "x") then
  error("another mod was writable")
end
files.write(ctx.mod.dir .. "/sandbox-ok", "1")
"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert!(harness.root.join("linkSlot/mods/sandbox/sandbox-ok").is_file());
}

#[test]
fn script_errors_are_logged_and_do_not_change_the_sync_result() {
    let harness = Harness::new("mods-error");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state")]);
    write_mod(
        &harness.root,
        "boom",
        "before_upload",
        true,
        r#"error("boom from mod")"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert!(
        result.stderr.contains("boom from mod"),
        "stderr was {}",
        result.stderr
    );
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .as_deref(),
        Some(b"state".as_slice())
    );
}

#[test]
fn webdav_put_atomic_backs_up_and_falls_back_when_move_is_rejected() {
    let mut harness = Harness::new("mods-put-atomic-move-409");
    let old = b"old-archive";
    let new = b"new-archive";
    harness
        .dav
        .insert_file("/compat/savedeck-move-test.zip", old);
    use_mod_remote_root(&mut harness, "/compat");
    harness
        .dav
        .fail_path("MOVE", "savedeck-move-test.zip", 409, 1);
    write_mod(
        &harness.root,
        "atomic-move",
        "after_sync",
        true,
        r#"
local ok, error_message = pcall(
  webdav.put_atomic,
  ctx.webdav.active,
  "savedeck-move-test.zip",
  "new-archive"
)
if not ok then error(error_message) end
"#,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert_eq!(
        harness
            .dav
            .file("/compat/savedeck-move-test.zip")
            .as_deref(),
        Some(new.as_slice())
    );
    let state = harness.dav.state();
    let state = state.lock().unwrap();
    let backups: Vec<&String> = state
        .files
        .keys()
        .filter(|path| path.contains("savedeck-move-test.zip.linkslot-mod-bak."))
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "{:?}",
        state.files.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        state.files.get(backups[0]).map(Vec::as_slice),
        Some(old.as_slice())
    );
    assert!(
        !state
            .files
            .keys()
            .any(|path| path.contains(".linkslot-mod-tmp.")),
        "temporary file was left behind"
    );
}

#[test]
fn webdav_put_atomic_keeps_original_when_backup_fails() {
    let mut harness = Harness::new("mods-put-atomic-backup-fail");
    let old = b"old-archive";
    harness
        .dav
        .insert_file("/compat/savedeck-move-test.zip", old);
    use_mod_remote_root(&mut harness, "/compat");
    harness
        .dav
        .fail_path("MOVE", "savedeck-move-test.zip", 409, 1);
    harness.dav.fail_path("PUT", "linkslot-mod-bak", 500, 1);
    write_mod(
        &harness.root,
        "atomic-backup-fail",
        "after_sync",
        true,
        r#"
local ok, error_message = pcall(
  webdav.put_atomic,
  ctx.webdav.active,
  "savedeck-move-test.zip",
  "new-archive"
)
if ok then error("put_atomic unexpectedly succeeded") end
"#,
    );

    let _ = harness.sync(SyncOptions::default());
    assert_eq!(
        harness
            .dav
            .file("/compat/savedeck-move-test.zip")
            .as_deref(),
        Some(old.as_slice())
    );
    let state = harness.dav.state();
    let state = state.lock().unwrap();
    assert!(
        !state
            .files
            .keys()
            .any(|path| path.contains(".linkslot-mod-tmp.")),
        "temporary file was left behind"
    );
    assert!(
        !state
            .files
            .keys()
            .any(|path| path.contains(".linkslot-mod-bak.")),
        "failed backup was left behind"
    );
}

#[test]
fn unknown_webdav_profile_is_rejected_without_failing_the_sync() {
    let harness = Harness::new("mods-unknown-profile");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"state")]);
    write_mod(
        &harness.root,
        "unknown-profile",
        "before_upload",
        true,
        r#"
local ok = pcall(webdav.get, "missing", "anything")
if ok then error("unknown profile was usable") end
files.write(ctx.game.state_dir .. "/resume.state", "local-still-works")
"#,
    );

    let result = harness.push(PushOptions::default());
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness
            .dav
            .file(&harness.remote_path(&unit, "resume.state"))
            .as_deref(),
        Some(b"local-still-works".as_slice())
    );
}

#[test]
fn cli_mods_check_list_and_version() {
    let dir = support::TestDir::new("mods-cli-list");
    write_mod(dir.path(), "good", "after_download", true, "return 1");
    write_mod(
        dir.path(),
        "bad",
        "after_download",
        true,
        "this is not valid lua !!!",
    );

    let mut check_ok = Vec::new();
    let mut check_err = Vec::new();
    let code = linkslot::cli::execute(
        &[
            "mods".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "good".to_string(),
            "--root".to_string(),
            dir.path().display().to_string(),
        ],
        &mut check_ok,
        &mut check_err,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&check_err));

    let mut check_action = Vec::new();
    let mut check_action_err = Vec::new();
    let code = linkslot::cli::execute(
        &[
            "mods".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "good".to_string(),
            "--action".to_string(),
            "main".to_string(),
            "--root".to_string(),
            dir.path().display().to_string(),
        ],
        &mut check_action,
        &mut check_action_err,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&check_action_err));

    let mut check_bad = Vec::new();
    let mut check_bad_err = Vec::new();
    let code = linkslot::cli::execute(
        &[
            "mods".to_string(),
            "check".to_string(),
            "--id".to_string(),
            "bad".to_string(),
            "--root".to_string(),
            dir.path().display().to_string(),
        ],
        &mut check_bad,
        &mut check_bad_err,
    );
    assert_eq!(code, 3);
    assert!(String::from_utf8_lossy(&check_bad_err).contains("语法错误"));

    let mut list = Vec::new();
    let mut list_err = Vec::new();
    let code = linkslot::cli::execute(
        &[
            "mods".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--root".to_string(),
            dir.path().display().to_string(),
        ],
        &mut list,
        &mut list_err,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&list_err));
    let text = String::from_utf8_lossy(&list);
    assert!(text.contains("\"id\":\"good\""), "{text}");
    assert!(text.contains("\"id\":\"bad\""), "{text}");
    assert!(text.contains("\"action_count\":1"), "{text}");
    assert!(text.contains("\"version\":\"1.0.0\""), "{text}");
    assert!(text.contains("\"timing\":\"after_download\""), "{text}");

    let mut version = Vec::new();
    let mut version_err = Vec::new();
    let code = linkslot::cli::execute(&["version".to_string()], &mut version, &mut version_err);
    assert_eq!(code, 0);
    assert!(String::from_utf8_lossy(&version).contains("mods=1"));
}

#[test]
fn cli_mods_test_probes_the_configured_sync_root() {
    let mut harness = Harness::new("mods-cli-test");
    harness
        .dav
        .insert_file("/compat/GBA/battery.sav", b"compat");
    use_mod_remote_root(&mut harness, "/compat");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(
        &[
            "mods".to_string(),
            "test".to_string(),
            "--profile".to_string(),
            "default".to_string(),
            "--root".to_string(),
            harness.root.display().to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
    assert!(
        String::from_utf8_lossy(&stderr).contains("同步方式可访问")
            || String::from_utf8_lossy(&stdout).contains("同步方式可访问"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
}

#[test]
fn after_boot_runs_without_a_game_context_from_mods_boot() {
    let harness = Harness::new("mods-after-boot");
    write_mod_enabled(&harness.root, "boot", true);
    write_action(
        &harness.root,
        "boot",
        "main",
        "boot-action",
        "after_boot",
        true,
        100,
        r#"
if ctx.timing ~= "after_boot" then error("wrong timing: " .. tostring(ctx.timing)) end
if ctx.unit ~= "boot" then error("wrong unit: " .. tostring(ctx.unit)) end
if ctx.game ~= nil or ctx.unit_path ~= nil or ctx.transfer ~= nil then
    error("after_boot must not expose game/unit paths")
end
files.write(ctx.mod.dir .. "/ran", "boot")
"#,
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let _ = harness.sync(SyncOptions::default());
    assert!(
        !harness.root.join("linkSlot/mods/boot/ran").exists(),
        "after_boot must not run during cloud sync"
    );
    let args = vec![
        "mods".to_string(),
        "boot".to_string(),
        "--id".to_string(),
        "boot".to_string(),
        "--action".to_string(),
        "main".to_string(),
        "--root".to_string(),
        harness.root.display().to_string(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("LINKSLOT_ACTION=mods"), "{stdout}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    assert_eq!(
        fs::read_to_string(harness.root.join("linkSlot/mods/boot/ran"))
            .ok()
            .as_deref(),
        Some("boot")
    );
}

#[test]
fn after_sync_runs_even_when_the_sync_is_noop() {
    let harness = Harness::new("mods-after-sync-noop");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same")]);
    harness.seed_remote(&unit, &[("resume.state", b"same")], 1, "device-a");
    write_mod(
        &harness.root,
        "after-sync",
        "after_sync",
        true,
        r#"
if ctx.unit ~= nil or ctx.game ~= nil or ctx.unit_path ~= nil then
    error("after_sync must not expose a unit context")
end
files.write(ctx.mod.dir .. "/ran", "after-sync")
"#,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert_eq!(
        fs::read_to_string(harness.root.join("linkSlot/mods/after-sync/ran"))
            .ok()
            .as_deref(),
        Some("after-sync")
    );
}

#[test]
fn after_sync_runs_without_any_local_units() {
    let harness = Harness::new("mods-after-sync-empty");
    write_mod(
        &harness.root,
        "after-sync-empty",
        "after_sync",
        true,
        r#"files.write(ctx.mod.dir .. "/ran", "empty")"#,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert_eq!(
        fs::read_to_string(harness.root.join("linkSlot/mods/after-sync-empty/ran"))
            .ok()
            .as_deref(),
        Some("empty")
    );
}

#[test]
fn mod_can_use_a_non_active_named_webdav_remote_root() {
    let mut harness = Harness::new("mods-named-profile");
    let unit = harness.unit();
    harness.write_local_sav(&unit, "sav", b"local-old");
    harness
        .dav
        .insert_file("/SaveDeck/GBA/battery.sav", b"named-profile-data");
    let dav_url = harness.config.webdav().url.clone();
    let path = harness.root.join("linkSlot/Config/storages.toml");
    fs::write(
        &path,
        format!(
            "active = \"default\"\n\n\
             [[storage]]\nid = \"default\"\nname = \"Default\"\nkind = \"webdav\"\nurl = \"{dav_url}\"\nremote_root = \"/linkSlot\"\nusername = \"user\"\npassword = \"pass\"\n\n\
             [[storage]]\nid = \"save\"\nname = \"SaveDeck\"\nkind = \"webdav\"\nurl = \"{dav_url}\"\nremote_root = \"/SaveDeck\"\nusername = \"user\"\npassword = \"pass\"\n"
        ),
    )
    .unwrap();
    harness.config = linkslot::config::Config::load(&harness.root, None).unwrap();
    write_mod(
        &harness.root,
        "named-profile",
        "before_upload",
        true,
        r#"
local data = webdav.get("save", "GBA/battery.sav")
if not data then error("named profile save missing") end
files.write(ctx.game.sav_path, data)
"#,
    );

    let result = harness.sync(SyncOptions {
        only: Some(Kind::Sav),
        ..Default::default()
    });
    assert_eq!(result.outcome.result(), ResultKind::Ok, "{}", result.stderr);
    assert_eq!(
        harness.dav.file(&harness.remote_sav_path(&unit)).as_deref(),
        Some(b"named-profile-data".as_slice())
    );
    assert_eq!(
        harness.local_sav(&unit).as_deref(),
        Some(b"named-profile-data".as_slice())
    );
}

#[test]
fn multiple_actions_have_own_enabled_state_and_context() {
    let harness = Harness::new("mods-multiple-actions");
    write_mod_enabled(&harness.root, "demo", true);
    write_action(
        &harness.root,
        "demo",
        "first",
        "先执行",
        "after_sync",
        true,
        10,
        r#"files.write(ctx.mod.dir .. "/first", ctx.action.id .. "|" .. ctx.action.name .. "|" .. ctx.action.dir)"#,
    );
    write_action(
        &harness.root,
        "demo",
        "second",
        "停用动作",
        "after_sync",
        false,
        20,
        r#"files.write(ctx.mod.dir .. "/second", "must-not-run")"#,
    );
    write_action(
        &harness.root,
        "demo",
        "early",
        "先排序",
        "after_sync",
        true,
        5,
        r#"local prev = files.read(ctx.mod.dir .. "/order") or ""; files.write(ctx.mod.dir .. "/order", prev .. "early,")"#,
    );
    write_action(
        &harness.root,
        "demo",
        "late",
        "后排序",
        "after_sync",
        true,
        30,
        r#"local prev = files.read(ctx.mod.dir .. "/order") or ""; files.write(ctx.mod.dir .. "/order", prev .. "late,")"#,
    );
    write_mod_enabled(&harness.root, "disabled-mod", false);
    write_action(
        &harness.root,
        "disabled-mod",
        "action",
        "mod 停用",
        "after_sync",
        true,
        10,
        r#"files.write(ctx.mod.dir .. "/must-not-run", "must-not-run")"#,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert_eq!(
        fs::read_to_string(harness.root.join("linkSlot/mods/demo/first"))
            .ok()
            .as_deref(),
        Some("first|先执行|linkSlot/mods/demo/actions/first")
    );
    assert!(!harness.root.join("linkSlot/mods/demo/second").exists());
    assert!(!harness.root.join("linkSlot/mods/disabled-mod/must-not-run").exists());
    assert_eq!(
        fs::read_to_string(harness.root.join("linkSlot/mods/demo/order"))
            .ok()
            .as_deref(),
        Some("early,late,")
    );
}

#[test]
fn zip_api_round_trips_binary_entries_inside_a_mod() {
    let harness = Harness::new("mods-zip-api");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same")]);
    harness.seed_remote(&unit, &[("resume.state", b"same")], 1, "device-a");
    write_mod(
        &harness.root,
        "zip-api",
        "after_sync",
        true,
        r#"
local bytes = zip.build({
    { name = "_savedeck/game.json", data = "{}" },
    { name = "龙珠大冒险.srm", data = "\0\1\255" },
})
local entries = zip.list(bytes)
local save = zip.read(bytes, entries[2].index)
files.write(ctx.mod.dir .. "/result", entries[2].name .. ":" .. save .. ":" .. tostring(#entries))
"#,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let mut expected = "龙珠大冒险.srm:".as_bytes().to_vec();
    expected.extend_from_slice(&[0, 1, 255]);
    expected.extend_from_slice(b":2");
    assert_eq!(
        fs::read(harness.root.join("linkSlot/mods/zip-api/result"))
            .ok()
            .as_deref(),
        Some(expected.as_slice())
    );
}

#[test]
fn savedeck_example_pulls_a_remote_battery_save() {
    let mut harness = Harness::new("mods-savedeck-pull");
    let unit = harness.unit();
    harness.seed_local(&unit, &[("resume.state", b"same")]);
    harness.seed_remote(&unit, &[("resume.state", b"same")], 1, "device-a");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(
        &harness.root,
        "savedeck-bridge",
        "after_sync",
        true,
        &script,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let expected: Vec<u8> = (0..512).map(|index| ((index * 7) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Game.sav")).ok(),
        Some(expected.clone()),
        "{}",
        result.stderr
    );

    let local_path = harness.root.join("Saves/GBA/Test Game.sav");
    let changed = b"changed-local-save";
    fs::write(&local_path, changed).unwrap();
    let before = harness
        .dav
        .file("/compat/savedeck-test.zip")
        .expect("fixture present");
    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let after = harness
        .dav
        .file("/compat/savedeck-test.zip")
        .expect("updated archive present");
    assert_ne!(before, after, "push should rewrite the SaveDeck archive");
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(harness.root.join("linkSlot/mods/savedeck-bridge/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["games"]["GBA/Test Game"]["remote_hash"].as_str(),
        Some(hash::sha256_hex(changed).as_str()),
        "{}",
        result.stderr
    );
    assert!(
        harness
            .root
            .join("linkSlot/mods/savedeck-bridge/backup/GBA-Test_Game.local.sav")
            .exists(),
        "push should back up the local save"
    );
}

#[test]
fn savedeck_manual_action_does_not_run_during_cloud_sync() {
    let mut harness = Harness::new("mods-savedeck-manual-not-auto");
    let unit = harness.unit_named("Test Game");
    harness.write_local_sav(&unit, "sav", b"local-save");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    let before = harness
        .dav
        .file("/compat/savedeck-test.zip")
        .expect("fixture");

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert_eq!(
        harness.dav.file("/compat/savedeck-test.zip").unwrap(),
        before,
        "manual action must not run after a normal cloud sync"
    );
}

#[test]
fn savedeck_manual_pull_creates_the_local_save() {
    let mut harness = Harness::new("mods-savedeck-manual-pull");
    let _unit = harness.unit_named("Test Game");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    let args = vec![
        "mods",
        "run",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        "Test Game",
    ];
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    assert!(
        stdout.contains("LINKSLOT_SAV_DIRECTION=download"),
        "{stdout}"
    );
    let expected: Vec<u8> = (0..512).map(|index| ((index * 7) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Game.sav")).unwrap(),
        expected
    );
}

/// 在 linkSlot 里玩过、SaveDeck 里还没有这个游戏时，长按 Y 要告诉用户"先去
/// SaveDeck 保存一次"，而不是只回一句没同步。这条提示完全在 mod 里，用
/// `mods.choose` 的可恢复检查点实现，所以这里也顺带钉住"弹窗那一次不写文件"。
#[test]
fn savedeck_manual_run_asks_to_save_in_savedeck_first() {
    let mut harness = Harness::new("mods-savedeck-missing-archive");
    let unit = harness.unit_named("Other Game");
    harness.write_local_sav(&unit, "sav", b"played-in-linkslot");
    // The fixture archive belongs to another game, so this one has no counterpart.
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    let original_archive = harness
        .dav
        .file("/compat/savedeck-test.zip")
        .expect("fixture");
    let local_save = harness.root.join("Saves/GBA/Other Game.sav");

    let mut args = vec![
        "mods",
        "run",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        "Other Game",
    ];
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=choice"), "{stdout}");
    assert!(stdout.contains("savedeck_missing_archive"), "{stdout}");
    assert!(
        stdout.contains("SaveDeck 里还没有这个游戏"),
        "the dialog does not name the problem: {stdout}"
    );
    // The prompt is a checkpoint, so it must not have written anything yet.
    assert_eq!(fs::read(&local_save).unwrap(), b"played-in-linkslot");
    assert_eq!(
        harness.dav.file("/compat/savedeck-test.zip").unwrap(),
        original_archive
    );
    assert!(!harness
        .root
        .join("linkSlot/mods/savedeck-manual/state.json")
        .exists());

    args.push("--choice");
    args.push("savedeck_missing_archive=ok");
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=noop"), "{stdout}");
    assert!(
        stdout.contains("先在 SaveDeck"),
        "the banner does not say what to do next: {stdout}"
    );
    assert_eq!(fs::read(&local_save).unwrap(), b"played-in-linkslot");
    assert_eq!(
        harness.dav.file("/compat/savedeck-test.zip").unwrap(),
        original_archive
    );
}

/// 诊断模式只读，不该被弹窗打断：同样的「SaveDeck 里还没有」只写在横幅里。
#[test]
fn savedeck_diagnose_reports_the_missing_archive_without_a_dialog() {
    let mut harness = Harness::new("mods-savedeck-missing-diagnose");
    let unit = harness.unit_named("Other Game");
    harness.write_local_sav(&unit, "sav", b"played-in-linkslot");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    fs::write(
        harness.root.join("linkSlot/mods/savedeck-manual/config.json"),
        r#"{"schema":1,"kind":"linkSlot.savedeck","profile":"default","mode":"diagnose"}"#,
    )
    .unwrap();

    let args = vec![
        "mods",
        "run",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        "Other Game",
    ];
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        !stdout.contains("LINKSLOT_RESULT=choice"),
        "diagnose mode popped a dialog: {stdout}"
    );
    assert!(stdout.contains("LINKSLOT_RESULT=noop"), "{stdout}");
    assert!(stdout.contains("先在 SaveDeck"), "{stdout}");
}

#[test]
fn savedeck_manual_conflict_local_choice_pushes_without_writing_before_choice() {
    let mut harness = Harness::new("mods-savedeck-manual-local");
    let unit = harness.unit_named("Test Game");
    harness.write_local_sav(&unit, "sav", b"local-save");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    let original_archive = harness
        .dav
        .file("/compat/savedeck-test.zip")
        .expect("fixture");

    let mut args = vec![
        "mods",
        "run",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        "Test Game",
    ];
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=choice"), "{stdout}");
    assert!(stdout.contains("savedeck_conflict"), "{stdout}");
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Game.sav")).unwrap(),
        b"local-save"
    );
    assert_eq!(
        harness.dav.file("/compat/savedeck-test.zip").unwrap(),
        original_archive
    );
    assert!(!harness
        .root
        .join("linkSlot/mods/savedeck-manual/state.json")
        .exists());

    args.push("--choice");
    args.push("savedeck_conflict=local");
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Game.sav")).unwrap(),
        b"local-save"
    );
    assert_ne!(
        harness.dav.file("/compat/savedeck-test.zip").unwrap(),
        original_archive
    );
}

#[test]
fn savedeck_manual_conflict_remote_choice_pulls() {
    let mut harness = Harness::new("mods-savedeck-manual-remote");
    let unit = harness.unit_named("Test Game");
    harness.write_local_sav(&unit, "sav", b"local-save");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(&harness.root, "savedeck-manual", "manual", true, &script);
    let mut args = vec![
        "mods",
        "run",
        "--platform",
        "GBA",
        "--emulator",
        "mgba",
        "--game",
        "Test Game",
    ];
    let (_, first_stdout, _) = harness.cli(&args);
    assert!(
        first_stdout.contains("LINKSLOT_RESULT=choice"),
        "{first_stdout}"
    );
    args.push("--choice");
    args.push("savedeck_conflict=remote");
    let (code, stdout, stderr) = harness.cli(&args);
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    let expected: Vec<u8> = (0..512).map(|index| ((index * 7) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Game.sav")).unwrap(),
        expected
    );
}

#[test]
fn savedeck_maps_gba_hack_platform_to_local_gba() {
    let mut harness = Harness::new("mods-savedeck-gba-hack");
    harness.dav.insert_file(
        "/compat/savedeck-gba-hack.zip",
        include_bytes!("fixtures/savedeck-gba-hack.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    write_mod(
        &harness.root,
        "savedeck-gba-hack",
        "after_sync",
        true,
        &script,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let expected: Vec<u8> = (0..512).map(|index| ((index * 11) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(harness.root.join("Saves/GBA/Test Hack.sav")).ok(),
        Some(expected),
        "{}",
        result.stderr
    );
}

#[test]
fn savedeck_matches_chinese_name_when_romstem_differs() {
    let mut harness = Harness::new("mods-savedeck-name-fallback");
    harness.dav.insert_file(
        "/compat/savedeck-name-match.zip",
        include_bytes!("fixtures/savedeck-name-match.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"")
            .replace("on_conflict = \"ask\"", "on_conflict = \"remote\"");
    let local_path = harness.root.join("Saves/GBA/龙珠大冒险.sav");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    fs::write(&local_path, b"old-local-save").unwrap();
    write_mod(
        &harness.root,
        "savedeck-name-fallback",
        "after_sync",
        true,
        &script,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("拉取 1"), "{}", result.stderr);
    let expected: Vec<u8> = (0..512).map(|index| ((index * 13) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(&local_path).ok(),
        Some(expected),
        "{}",
        result.stderr
    );
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(harness.root.join("linkSlot/mods/savedeck-name-fallback/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["games"]["GBA/龙珠大冒险"]["archive"], "savedeck-name-match.zip",
        "{}",
        result.stderr
    );

    let changed = b"changed-chinese-local-save";
    fs::write(&local_path, changed).unwrap();
    let before = harness
        .dav
        .file("/compat/savedeck-name-match.zip")
        .expect("fixture present");
    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let after = harness
        .dav
        .file("/compat/savedeck-name-match.zip")
        .expect("updated archive present");
    assert_ne!(
        before, after,
        "push should rewrite the name-matched archive"
    );
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(harness.root.join("linkSlot/mods/savedeck-name-fallback/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["games"]["GBA/龙珠大冒险"]["remote_hash"].as_str(),
        Some(hash::sha256_hex(changed).as_str()),
        "{}",
        result.stderr
    );
}

#[test]
fn savedeck_alias_maps_local_name_to_remote_stem() {
    let mut harness = Harness::new("mods-savedeck-alias");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"")
            .replace("on_conflict = \"ask\"", "on_conflict = \"remote\"")
            .replace(
                "aliases = {}",
                "aliases = { [\"GBA/本地龙珠\"] = \"Test Game\" }",
            );
    let local_path = harness.root.join("Saves/GBA/本地龙珠.sav");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    fs::write(&local_path, b"old-alias-save").unwrap();
    write_mod(&harness.root, "savedeck-alias", "after_sync", true, &script);

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let expected: Vec<u8> = (0..512).map(|index| ((index * 7) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(&local_path).ok(),
        Some(expected),
        "{}",
        result.stderr
    );
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(harness.root.join("linkSlot/mods/savedeck-alias/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["games"]["GBA/本地龙珠"]["archive"],
        "savedeck-test.zip"
    );
}

#[test]
fn savedeck_loads_aliases_and_mode_from_config_json() {
    let mut harness = Harness::new("mods-savedeck-config");
    harness.dav.insert_file(
        "/compat/savedeck-test.zip",
        include_bytes!("fixtures/savedeck-test.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"");
    let local_path = harness.root.join("Saves/GBA/配置龙珠.sav");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    fs::write(&local_path, b"old-config-save").unwrap();
    write_mod(
        &harness.root,
        "savedeck-config",
        "after_sync",
        true,
        &script,
    );
    fs::write(
        harness.root.join("linkSlot/mods/savedeck-config/config.json"),
        r#"{
          "schema": 1,
          "profile": "default",
          "mode": "pull",
          "on_conflict": "remote",
          "match_by_name": true,
          "allow_create": false,
          "batch": 8,
          "platforms": ["GBA"],
          "aliases": { "GBA/配置龙珠": "Test Game" }
        }"#,
    )
    .unwrap();

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    let expected: Vec<u8> = (0..512).map(|index| ((index * 7) & 0xff) as u8).collect();
    assert_eq!(
        fs::read(&local_path).ok(),
        Some(expected),
        "{}",
        result.stderr
    );
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(harness.root.join("linkSlot/mods/savedeck-config/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["games"]["GBA/配置龙珠"]["archive"],
        "savedeck-test.zip"
    );
}

#[test]
fn savedeck_diagnose_mode_is_read_only() {
    let mut harness = Harness::new("mods-savedeck-diagnose");
    harness.dav.insert_file(
        "/compat/savedeck-name-match.zip",
        include_bytes!("fixtures/savedeck-name-match.zip"),
    );
    use_mod_remote_root(&mut harness, "/compat");
    let local_path = harness.root.join("Saves/GBA/龙珠大冒险.sav");
    fs::create_dir_all(local_path.parent().unwrap()).unwrap();
    fs::write(&local_path, b"local-diagnose-save").unwrap();
    let before_remote = harness
        .dav
        .file("/compat/savedeck-name-match.zip")
        .expect("fixture present");
    let script =
        include_str!("fixtures/savedeck-battery-bridge/actions/sync/main.lua")
            .replace("profile = \"\"", "profile = \"default\"")
            .replace("mode = \"two-way\"", "mode = \"diagnose\"");
    write_mod(
        &harness.root,
        "savedeck-diagnose",
        "after_sync",
        true,
        &script,
    );

    let result = harness.sync(SyncOptions::default());
    assert!(
        matches!(result.outcome.result(), ResultKind::Ok | ResultKind::Noop),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("诊断"), "{}", result.stderr);
    assert_eq!(
        fs::read(&local_path).unwrap(),
        b"local-diagnose-save",
        "diagnose must not overwrite local save"
    );
    let after_remote = harness
        .dav
        .file("/compat/savedeck-name-match.zip")
        .expect("fixture still present");
    assert_eq!(
        before_remote, after_remote,
        "diagnose must not rewrite the remote archive"
    );
    assert!(
        !harness
            .root
            .join("linkSlot/mods/savedeck-diagnose/state.json")
            .exists(),
        "diagnose must not persist state"
    );
}
