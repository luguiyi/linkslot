//! End-to-end `linkSlot mods run` test against a fake GBAStation HTTP server.
//!
//! The core pieces are the `manual` timing, the scoped `http.*` bridge,
//! `ctx.mod.config`, `files.write` and `mods.report`; the real mod package is
//! exercised separately when it is present in the workdoc example directory.

#![cfg(feature = "mods")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn unique_root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "linkslot-mods-run-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn install_mod(root: &Path, base_url: &str, lua: &str) {
    write_file(
        root,
        "linkSlot/mods/gbastation-battery-sync/mod.json",
        r#"{"schema":2,"name":"GBAStation 电池存档同步","version":"1.0.0","enabled":true}"#
            .as_bytes(),
    );
    write_file(
        root,
        "linkSlot/mods/gbastation-battery-sync/config.json",
        format!(r#"{{"schema":1,"base_url":"{base_url}"}}"#).as_bytes(),
    );
    write_file(
        root,
        "linkSlot/mods/gbastation-battery-sync/actions/sync/action.json",
        r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,"order":100}"#.as_bytes(),
    );
    write_file(
        root,
        "linkSlot/mods/gbastation-battery-sync/actions/sync/main.lua",
        lua.as_bytes(),
    );
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            break;
        }
    }
    if buffer.is_empty() {
        return None;
    }
    String::from_utf8(buffer)
        .ok()
        .and_then(|text| text.lines().next().map(str::to_string))
}

fn reply(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).ok();
    stream.write_all(body).ok();
    stream.flush().ok();
}

/// Serve the three GETs used by the download test, plus the connectivity probe.
fn spawn_gbastation(
    save: Vec<u8>,
    expected: usize,
) -> (u16, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let handle = std::thread::spawn(move || {
        for _ in 0..expected {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let Some(line) = read_request(&mut stream) else {
                // TCP reachability probe: connect + close, no HTTP request.
                continue;
            };
            seen.fetch_add(1, Ordering::SeqCst);
            if line.starts_with("GET /api/games ") {
                let body = br#"{"ok":true,"games":[{"id":"game1","title":"Test Game","path":"sdmc:/roms/Test Game.gba","platform":1}]}"#;
                reply(&mut stream, "application/json", body);
            } else if line.starts_with("GET /api/game/game1/saves ") {
                let body = format!(
                    r#"{{"ok":true,"saveDir":"sdmc:/saves/Test Game","battery":[{{"type":"battery","name":"Test Game.sav","path":"sdmc:/saves/Test Game/Test Game.sav","size":{},"modified":"2026-10-25 12:00:00"}}],"states":[]}}"#,
                    save.len()
                );
                reply(&mut stream, "application/json", body.as_bytes());
            } else if line.starts_with("GET /api/game/game1/save/export") {
                reply(&mut stream, "application/octet-stream", &save);
            } else {
                let body = b"{\"ok\":false}";
                reply(&mut stream, "application/json", body);
            }
        }
    });
    (port, count, handle)
}

const DOWNLOAD_LUA: &str = r#"
local function fail(kind, detail)
  mods.report{ status = "error", error = kind, detail = detail }
  return
end

local games = http.get({"api", "games"})
if games.status ~= 200 then return fail("server", "games HTTP " .. tostring(games.status)) end
local list = json.decode(games.body)
if not list.games or #list.games == 0 then return fail("config", "没有游戏") end
local game = list.games[1]

local saves = http.get({"api", "game", "game1", "saves"})
if saves.status ~= 200 then return fail("server", "saves HTTP " .. tostring(saves.status)) end
local parsed = json.decode(saves.body)
if not parsed.battery or #parsed.battery == 0 then return fail("config", "没有电池存档") end

local downloaded = http.get({"api", "game", "game1", "save", "export"}, { path = parsed.battery[1].path })
if downloaded.status ~= 200 then return fail("server", "export HTTP " .. tostring(downloaded.status)) end
if #downloaded.body ~= parsed.battery[1].size then return fail("verify", "大小不一致") end
local written, error = pcall(files.write, ctx.game.sav_path, downloaded.body)
if not written then return fail("fs", tostring(error)) end
mods.report{ status = "ok", direction = "download", detail = "已拉取测试存档" }
"#;

#[test]
fn mods_run_pulls_a_battery_save_through_the_scoped_http_bridge() {
    let root = unique_root("pull");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    std::fs::create_dir_all(root.join("Saves/GBA")).unwrap();
    let save = b"battery-save-bytes".to_vec();
    let (port, seen, server) = spawn_gbastation(save.clone(), 4);
    let base_url = format!("http://127.0.0.1:{port}");
    install_mod(&root, &base_url, DOWNLOAD_LUA);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("LINKSLOT_ACTION=mods"), "{stdout}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    assert!(stdout.contains("已拉取测试存档"), "{stdout}");
    assert_eq!(
        std::fs::read(root.join("Saves/GBA/Test Game.sav")).unwrap(),
        save
    );
    assert_eq!(seen.load(Ordering::SeqCst), 3);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_real_gbastation_mod_pulls_a_save_against_a_mock_server() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gbastation-battery-sync/actions/sync/main.lua");
    let Ok(source) = std::fs::read_to_string(&script) else {
        // The example is checked in under tests/fixtures so a clean clone runs it too.
        return;
    };
    let root = unique_root("real-mod");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    std::fs::create_dir_all(root.join("Saves/GBA")).unwrap();
    let save = b"real-mod-battery-save".to_vec();
    let (port, seen, server) = spawn_gbastation(save.clone(), 4);
    let base_url = format!("http://127.0.0.1:{port}");
    install_mod(&root, &base_url, &source);

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("LINKSLOT_ACTION=mods"), "{stdout}");
    assert!(stdout.contains("LINKSLOT_RESULT=ok"), "{stdout}");
    assert!(stdout.contains("LINKSLOT_STAGE=mods"), "{stdout}");
    assert!(
        stdout.contains("LINKSLOT_SAV_DIRECTION=download"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read(root.join("Saves/GBA/Test Game.sav")).unwrap(),
        save
    );
    assert_eq!(seen.load(Ordering::SeqCst), 3);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ui_confirm_returns_a_boolean_after_the_user_answers() {
    let root = unique_root("ui-confirm");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    let script = r#"
        local yes = ui.confirm{
          title = "继续？",
          body = "测试确认弹窗",
          yes = "继续",
          no = "停止",
        }
        files.write("linkSlot/mods/gbastation-battery-sync/answer.txt", tostring(yes))
        mods.report{ status = "ok", detail = "confirm=" .. tostring(yes) }
    "#;
    install_mod(&root, "", script);
    let base_args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&base_args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=choice"), "{text}");
    assert!(text.contains("\"kind\":\"confirm\""), "{text}");
    assert!(text.contains("继续？"), "{text}");
    assert!(!root.join("linkSlot/mods/gbastation-battery-sync/answer.txt").exists());

    let mut args = base_args;
    args.push("--choice".to_string());
    args.push("ui_confirm_1=yes".to_string());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=ok"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join("linkSlot/mods/gbastation-battery-sync/answer.txt")).unwrap(),
        "true"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ui_message_returns_true_after_the_user_closes_it() {
    let root = unique_root("ui-message");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    let script = r#"
        local ok = ui.message{
          title = "提示",
          body = "这是一段文字弹窗。",
          button = "知道了",
        }
        files.write("linkSlot/mods/gbastation-battery-sync/answer.txt", tostring(ok))
        mods.report{ status = "ok", detail = "message=" .. tostring(ok) }
    "#;
    install_mod(&root, "", script);
    let base_args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&base_args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=choice"), "{text}");
    assert!(text.contains("\"kind\":\"message\""), "{text}");
    assert!(!root.join("linkSlot/mods/gbastation-battery-sync/answer.txt").exists());

    let mut args = base_args;
    args.push("--choice".to_string());
    args.push("ui_message_1=ok".to_string());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=ok"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join("linkSlot/mods/gbastation-battery-sync/answer.txt")).unwrap(),
        "true"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mods_choose_requests_a_choice_and_resumes_with_it() {
    let root = unique_root("choose");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    let script = r#"
        local choice = mods.choose{
          key = "conflict",
          title = "存档冲突",
          body = "测试选择",
          default = "remote",
          options = {
            { id = "remote", label = "用远端覆盖本地" },
            { id = "local", label = "用本地覆盖远端" },
          },
        }
        files.write("linkSlot/mods/gbastation-battery-sync/answer.txt", choice)
        mods.report{ status = "ok", detail = "choice=" .. choice }
    "#;
    install_mod(&root, "", script);
    let base_args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&base_args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=choice"), "{text}");
    assert!(text.contains("LINKSLOT_CHOICE="), "{text}");
    assert!(text.contains("存档冲突"), "{text}");
    assert!(!root.join("linkSlot/mods/gbastation-battery-sync/answer.txt").exists());

    let mut args = base_args;
    args.push("--choice".to_string());
    args.push("conflict=local".to_string());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=ok"), "{text}");
    assert!(text.contains("choice=local"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join("linkSlot/mods/gbastation-battery-sync/answer.txt")).unwrap(),
        "local"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_real_mod_asks_before_overwriting_a_conflicted_save() {
    use sha2::{Digest, Sha256};

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gbastation-battery-sync/actions/sync/main.lua");
    let Ok(source) = std::fs::read_to_string(&script) else {
        return;
    };
    let root = unique_root("conflict-choice");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    std::fs::create_dir_all(root.join("Saves/GBA")).unwrap();
    let local = b"old-local-save".to_vec();
    let remote = b"new-remote-save".to_vec();
    write_file(&root, "Saves/GBA/Test Game.sav", &local);

    let key = {
        let mut digest = Sha256::new();
        digest.update(b"GBA\nTest Game");
        format!("{:x}", digest.finalize())[..16].to_string()
    };
    let old_local = {
        let mut digest = Sha256::new();
        digest.update(b"state-local");
        format!("{:x}", digest.finalize())
    };
    let old_remote = {
        let mut digest = Sha256::new();
        digest.update(b"state-remote");
        format!("{:x}", digest.finalize())
    };
    write_file(
        &root,
        &format!("linkSlot/mods/gbastation-battery-sync/state/GBA/{key}.json"),
        format!(
            r#"{{"local_hash":"{old_local}","remote_hash":"{old_remote}","local_path":"Saves/GBA/Test Game.sav","remote_path":"sdmc:/saves/Test Game/Test Game.sav","remote_name":"Test Game.sav"}}"#
        )
        .as_bytes(),
    );

    let (port, _seen, server) = spawn_gbastation(remote.clone(), 8);
    let base_url = format!("http://127.0.0.1:{port}");
    install_mod(&root, &base_url, &source);
    write_file(
        &root,
        "linkSlot/mods/gbastation-battery-sync/config.json",
        format!(
            r#"{{"schema":1,"base_url":"{base_url}","mode":"two-way","first_sync":"ask","on_conflict":"ask","backup":true,"verify":true}}"#
        )
        .as_bytes(),
    );
    let base_args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&base_args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=choice"), "{text}");
    assert!(text.contains("存档冲突"), "{text}");
    assert_eq!(std::fs::read(root.join("Saves/GBA/Test Game.sav")).unwrap(), local);

    let mut args = base_args;
    args.push("--choice".to_string());
    args.push("conflict=remote".to_string());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, 0, "stdout: {text}\nstderr: {}", String::from_utf8_lossy(&stderr));
    assert!(text.contains("LINKSLOT_RESULT=ok"), "{text}");
    assert_eq!(
        std::fs::read(root.join("Saves/GBA/Test Game.sav")).unwrap(),
        remote
    );
    assert_eq!(
        std::fs::read(root.join(format!(
            "linkSlot/mods/gbastation-battery-sync/backups/GBA/{key}-local.sav"
        )))
        .unwrap(),
        local
    );
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn mods_run_without_config_reports_a_readable_configuration_error() {
    let root = unique_root("config");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    install_mod(
        &root,
        "",
        "mods.report{ status='error', error='config', detail='缺少地址' }",
    );
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 3, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("LINKSLOT_RESULT=error"), "{stdout}");
    assert!(stdout.contains("LINKSLOT_ERROR=config"), "{stdout}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Install one mod with a manual action and an explicit priority.
fn install_ordered_mod(root: &Path, id: &str, name: &str, order: i32, lua: &str) {
    write_file(
        root,
        &format!("linkSlot/mods/{id}/mod.json"),
        format!(
            r#"{{"schema":2,"name":"{name}","version":"1.0.0","enabled":true,"order":{order}}}"#
        )
        .as_bytes(),
    );
    write_file(
        root,
        &format!("linkSlot/mods/{id}/actions/sync/action.json"),
        r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,"order":100}"#.as_bytes(),
    );
    write_file(
        root,
        &format!("linkSlot/mods/{id}/actions/sync/main.lua"),
        lua.as_bytes(),
    );
}

/// Two bridges both ask for the shelf long-press. Only one may run — the other
/// would decide against a save file the first had just rewritten — and the
/// winner is the one with the smaller priority number, i.e. the one the device
/// list shows at the top.
#[test]
fn only_the_highest_priority_manual_mod_runs() {
    let root = unique_root("priority");
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    std::fs::create_dir_all(root.join("Saves/GBA")).unwrap();
    install_ordered_mod(
        &root,
        "lower",
        "Lower",
        20,
        r#"mods.report{ status = "error", error = "server", detail = "较低优先级也跑了" }"#,
    );
    install_ordered_mod(
        &root,
        "higher",
        "Higher",
        10,
        r#"mods.report{ status = "ok", detail = "优先级最高的跑完了" }"#,
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let args = vec![
        "mods".to_string(),
        "run".to_string(),
        "--platform".to_string(),
        "GBA".to_string(),
        "--emulator".to_string(),
        "mgba".to_string(),
        "--game".to_string(),
        "Test Game".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    let code = linkslot::cli::execute(&args, &mut stdout, &mut stderr);
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains("优先级最高的跑完了"),
        "the top-priority mod did not run: {stdout}"
    );
    assert!(
        !stdout.contains("较低优先级也跑了"),
        "the lower-priority mod ran too: {stdout}"
    );
    assert!(
        stderr.contains("优先级"),
        "the skipped mod was not named: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
