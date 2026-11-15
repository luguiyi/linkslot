//! Inline integration tests for the Web service: HTTP framing, routing, path
//! safety, writable/header guards and the JSON shapes the page consumes.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;

use super::http::{Method, Response};
use super::test_support::{self, body_json, json_request, request, with_slot_header};
use super::{route, Shared, WebServer};
use crate::web::util;

static PORT_BASE: AtomicU16 = AtomicU16::new(18100);

fn next_ports() -> std::ops::RangeInclusive<u16> {
    let base = PORT_BASE.fetch_add(10, Ordering::SeqCst);
    base..=base + 9
}

fn start_server(root: &Path) -> WebServer {
    WebServer::bind(root.to_path_buf(), "127.0.0.1".to_string(), next_ports())
        .expect("loopback Web server starts")
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A root with one save unit and a battery save to exercise every endpoint.
fn save_fixture() -> tempfile::TempDir {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "States/GBA/mgba/宝可梦/resume.state", b"resume-state");
    write_file(root, "States/GBA/mgba/宝可梦/resume.png", b"png");
    write_file(
        root,
        "States/GBA/mgba/宝可梦/2026-10-21_10-00-00.state",
        b"snapshot",
    );
    write_file(
        root,
        "States/GBA/mgba/宝可梦/2026-10-21_10-00-00.png",
        b"thumb",
    );
    write_file(
        root,
        "States/GBA/mgba/宝可梦/playtime.json",
        br#"{"schema":1,"seconds":7200,"total_seconds":10800,"devices":{"dev-a":7200,"dev-b":3600}}"#,
    );
    write_file(root, "Saves/GBA/宝可梦.sav", b"battery");
    write_file(root, "linkSlot/Config/cache/deadbeef.json", b"{}");
    dir
}

fn png_bytes(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&pixel);
        }
        writer.write_image_data(&rgba).unwrap();
    }
    out
}

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    )
}

fn rom_fixture() -> tempfile::TempDir {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/宝可梦.gba", b"old-gba-rom");
    write_file(root, "Games/GB/Tetris.gb", b"old-gb-rom");
    write_file(
        root,
        "Labels/GBA/宝可梦.png",
        &png_bytes(4, 2, [220, 20, 20, 255]),
    );
    dir
}

fn rom_query(rom: &str) -> String {
    format!("rom={}", util::encode_query(rom))
}



fn add_query(platform: &str, name: &str) -> String {
    format!(
        "platform={}&name={}",
        util::encode_query(platform),
        util::encode_query(name)
    )
}

#[test]
fn adds_a_rom_to_the_chosen_platform() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "龙珠大冒险.gba"),
        b"new-gba-rom",
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(dir.path().join("Games/GBA/龙珠大冒险.gba").is_file());

    let games = response_json(&get(&shared, "/api/roms", ""));
    let names: Vec<String> = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|game| game["stem"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(names.iter().any(|name| name == "龙珠大冒险"), "{names:?}");
    let added = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["stem"] == "龙珠大冒险")
        .unwrap();
    assert_eq!(added["platform"], "GBA");
}

#[test]
fn renames_a_rom_and_moves_its_sidecars() {
    let dir = rom_fixture();
    let root = dir.path();
    write_file(root, "Games/GBA/OldGame.gba", b"old-gba-rom");
    write_file(root, "Games/GBA/Taken.gba", b"already-there");
    write_file(root, "Saves/GBA/OldGame.sav", b"battery");
    write_file(root, "States/GBA/mgba/OldGame/resume.state", b"state");
    write_file(root, "Info/GBA/OldGame.txt", b"intro");
    write_file(root, "Guides/GBA/OldGame.txt", b"guide");
    write_file(root, "Cheats/GBA/mgba/OldGame.cht", b"cheat");
    let shared = test_support::test_shared(root.to_path_buf());

    let response = post(
        &shared,
        "/api/rom/rename",
        serde_json::json!({
            "rom": "Games/GBA/OldGame.gba",
            "name": "New Game"
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(!root.join("Games/GBA/OldGame.gba").exists());
    assert!(root.join("Games/GBA/New Game.gba").is_file());
    assert!(root.join("Saves/GBA/New Game.sav").is_file());
    assert!(root.join("States/GBA/mgba/New Game/resume.state").is_file());
    assert!(root.join("Info/GBA/New Game.txt").is_file());
    assert!(root.join("Guides/GBA/New Game.txt").is_file());
    assert!(root.join("Cheats/GBA/mgba/New Game.cht").is_file());

    let body = body_json(&response);
    assert_eq!(body["stem"], "New Game");
    let games = response_json(&get(&shared, "/api/roms", ""));
    assert!(
        games["games"].as_array().unwrap().iter().any(|game| game["stem"] == "New Game"),
        "{games}"
    );

    // A collision leaves both the source and destination untouched.
    let collision = post(
        &shared,
        "/api/rom/rename",
        serde_json::json!({
            "rom": "Games/GBA/New Game.gba",
            "name": "Taken"
        }),
        true,
    );
    assert_eq!(collision.status, 409, "{}", body_text(&collision));
    assert!(root.join("Games/GBA/New Game.gba").is_file());
}

#[test]
fn fc_games_can_switch_between_famicom_and_nes_models() {
    let dir = rom_fixture();
    let root = dir.path();
    write_file(root, "Games/FC/Test.nes", b"nes-rom");
    let shared = test_support::test_shared(root.to_path_buf());

    let response = put_json(
        &shared,
        "/api/rom/model",
        &rom_query("Games/FC/Test.nes"),
        serde_json::json!({ "model": "nes" }),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["model"], "nes");

    let games = response_json(&get(&shared, "/api/roms", ""));
    let game = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/FC/Test.nes")
        .expect("the FC game disappeared");
    assert_eq!(game["model"], "nes");

    // GBA has one shell; the model endpoint must not pretend otherwise.
    let rejected = put_json(
        &shared,
        "/api/rom/model",
        &rom_query("Games/GB/Tetris.gb"),
        serde_json::json!({ "model": "nes" }),
    );
    assert_eq!(rejected.status, 400, "{}", body_text(&rejected));
}

#[test]
fn fronts_can_be_uploaded_selected_and_deleted() {
    let dir = rom_fixture();
    let root = dir.path();
    write_file(root, "Games/FC/Test.nes", b"nes-rom");
    let shared = test_support::test_shared(root.to_path_buf());
    let png = png_bytes(4, 4, [90, 20, 30, 255]);
    let rom = "Games/FC/Test.nes";

    let first = post_bytes(
        &shared,
        "/api/rom/fronts",
        &format!("{}&name=First&model=any", rom_query(rom)),
        &png,
        true,
    );
    assert_eq!(first.status, 200, "{}", body_text(&first));
    let first_id = body_json(&first)["id"].as_str().unwrap().to_string();

    let list = response_json(&get(&shared, "/api/rom/fronts", &rom_query(rom)));
    assert_eq!(list["faces"].as_array().unwrap().len(), 1);
    assert_eq!(list["selected"], first_id);
    let carts = slot_store::scan_quiet(root).unwrap();
    assert!(
        carts.iter().any(|cart| cart.front.is_some()),
        "the selected front did not reach the scanned cart"
    );
    let image = get(
        &shared,
        "/api/rom/front",
        &format!("{}&id={first_id}", rom_query(rom)),
    );
    assert_eq!(image.status, 200);
    assert_eq!(image.content_type, "image/png");

    let second = post_bytes(
        &shared,
        "/api/rom/fronts",
        &format!("{}&name=Second&model=any", rom_query(rom)),
        &png,
        true,
    );
    assert_eq!(second.status, 200, "{}", body_text(&second));
    let second_id = body_json(&second)["id"].as_str().unwrap().to_string();
    let selected = put(
        &shared,
        "/api/rom/fronts/select",
        &format!("{}&id={second_id}", rom_query(rom)),
        b"",
        true,
    );
    assert_eq!(selected.status, 200, "{}", body_text(&selected));
    let list = response_json(&get(&shared, "/api/rom/fronts", &rom_query(rom)));
    assert_eq!(list["selected"], second_id);

    for index in 0..3 {
        let more = post_bytes(
            &shared,
            "/api/rom/fronts",
            &format!("{}&name=More{index}&model=any", rom_query(rom)),
            &png,
            true,
        );
        assert_eq!(more.status, 200, "{}", body_text(&more));
    }
    let sixth = post_bytes(
        &shared,
        "/api/rom/fronts",
        &format!("{}&name=Sixth&model=any", rom_query(rom)),
        &png,
        true,
    );
    assert_eq!(sixth.status, 409, "{}", body_text(&sixth));

    let removed = post_query(
        &shared,
        "/api/rom/fronts/delete",
        &format!("{}&id={second_id}", rom_query(rom)),
        true,
    );
    assert_eq!(removed.status, 200, "{}", body_text(&removed));
    let list = response_json(&get(&shared, "/api/rom/fronts", &rom_query(rom)));
    assert_eq!(list["faces"].as_array().unwrap().len(), 4);
    assert!(root.join("linkSlot/Fronts/FC/Test").is_dir());
}

#[test]
fn front_projects_round_trip_with_revision_checks() {
    let dir = rom_fixture();
    let root = dir.path();
    write_file(root, "Games/FC/Test.nes", b"nes-rom");
    let shared = test_support::test_shared(root.to_path_buf());
    let (width, height) = slot_ui::front_size(
        slot_store::Platform::Fc,
        slot_store::CartModel::Famicom,
    );
    let png = png_bytes(width, height, [90, 20, 30, 255]);
    let render = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let project = serde_json::json!({
        "schema": 1,
        "canvas": {"width": width, "height": height},
        "platform": "FC",
        "model": "any",
        "name": "Projected",
        "background": "#101014",
        "layers": []
    });
    let body = serde_json::json!({
        "revision": 0,
        "project": project,
        "render": render
    });
    let rom = "Games/FC/Test.nes";
    let first = put_json(
        &shared,
        "/api/rom/fronts/project",
        &format!("{}&name=Projected&model=any", rom_query(rom)),
        body,
    );
    assert_eq!(first.status, 200, "{}", body_text(&first));
    let first_body = body_json(&first);
    let id = first_body["id"].as_str().unwrap().to_string();
    assert_eq!(first_body["revision"], 1);
    assert!(root.join(format!("linkSlot/Fronts/FC/Test/{id}.json")).is_file());
    assert!(root.join(format!("linkSlot/Fronts/FC/Test/{id}.png")).is_file());

    let loaded = get(
        &shared,
        "/api/rom/fronts/project",
        &format!("{}&id={id}", rom_query(rom)),
    );
    assert_eq!(loaded.status, 200, "{}", body_text(&loaded));
    assert_eq!(body_json(&loaded)["project"]["name"], "Projected");

    let project2 = serde_json::json!({
        "schema": 1,
        "canvas": {"width": width, "height": height},
        "platform": "FC",
        "model": "any",
        "name": "Projected v2",
        "background": "#181820",
        "layers": []
    });
    let second = put_json(
        &shared,
        "/api/rom/fronts/project",
        &format!("{}&id={id}&name=Projected%20v2&model=any", rom_query(rom)),
        serde_json::json!({
            "revision": 1,
            "project": project2,
            "render": render
        }),
    );
    assert_eq!(second.status, 200, "{}", body_text(&second));
    assert_eq!(body_json(&second)["revision"], 2);

    let stale = put_json(
        &shared,
        "/api/rom/fronts/project",
        &format!("{}&id={id}&name=Stale&model=any", rom_query(rom)),
        serde_json::json!({
            "revision": 0,
            "project": project2,
            "render": render
        }),
    );
    assert_eq!(stale.status, 409, "{}", body_text(&stale));
    let copied = post_query(
        &shared,
        "/api/rom/fronts/copy",
        &format!("{}&id={id}", rom_query(rom)),
        true,
    );
    assert_eq!(copied.status, 200, "{}", body_text(&copied));
    let list = response_json(&get(&shared, "/api/rom/fronts", &rom_query(rom)));
    assert_eq!(list["faces"].as_array().unwrap().len(), 2);
    assert_eq!(list["faces"][0]["source"], "project");
    assert_eq!(list["faces"][1]["source"], "project");

    let exported = get(
        &shared,
        "/api/rom/fronts/export",
        &format!("{}&id={id}", rom_query(rom)),
    );
    assert_eq!(exported.status, 200, "{}", body_text(&exported));
    let package = body_json(&exported);
    let imported = post_bytes(
        &shared,
        "/api/rom/fronts/import",
        &rom_query(rom),
        &serde_json::to_vec(&package).unwrap(),
        true,
    );
    assert_eq!(imported.status, 200, "{}", body_text(&imported));
    let list = response_json(&get(&shared, "/api/rom/fronts", &rom_query(rom)));
    assert_eq!(list["faces"].as_array().unwrap().len(), 3);
}

#[test]
fn front_template_exposes_the_real_cart_shape() {
    let dir = rom_fixture();
    let root = dir.path();
    write_file(root, "Games/FC/Test.nes", b"nes-rom");
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(
        &shared,
        "/api/rom/front/template",
        &rom_query("Games/FC/Test.nes"),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.content_type, "image/png");
    let decoder = png::Decoder::new(response.body.as_slice());
    let reader = decoder.read_info().unwrap();
    assert_eq!(
        (reader.info().width, reader.info().height),
        slot_ui::front_size(
            slot_store::Platform::Fc,
            slot_store::CartModel::Famicom
        )
    );
}

#[test]
fn adds_an_sfc_rom_to_the_chosen_platform() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("SFC", "超级马里奥世界.sfc"),
        b"new-sfc-rom",
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(dir.path().join("Games/SFC/超级马里奥世界.sfc").is_file());

    let games = response_json(&get(&shared, "/api/roms", ""));
    let added = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["stem"] == "超级马里奥世界")
        .expect("the SFC upload did not reach the library");
    assert_eq!(added["platform"], "SFC");

    // The SFC directory accepts exactly its own three extensions.
    let wrong = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("SFC", "Tetris.nes"),
        b"nes-rom",
        true,
    );
    assert_eq!(wrong.status, 400, "{}", body_text(&wrong));
    assert!(!dir.path().join("Games/SFC/Tetris.nes").exists());
}

#[test]
fn adds_a_mega_drive_rom_to_the_chosen_platform() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("MD", "Sonic.md"),
        b"new-md-rom",
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(dir.path().join("Games/MD/Sonic.md").is_file());

    let games = response_json(&get(&shared, "/api/roms", ""));
    let added = games["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["stem"] == "Sonic")
        .expect("the MD upload did not reach the library");
    assert_eq!(added["platform"], "MD");

    // `.bin` is shared with other machines; inside the MD upload it is
    // still an MD ROM because the folder, not the extension, decided.
    let bin = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("MD", "Streets.bin"),
        b"new-md-bin",
        true,
    );
    assert_eq!(bin.status, 200, "{}", body_text(&bin));
    assert!(dir.path().join("Games/MD/Streets.bin").is_file());

    let wrong = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("MD", "Tetris.gb"),
        b"gb-rom",
        true,
    );
    assert_eq!(wrong.status, 400, "{}", body_text(&wrong));
    assert!(!dir.path().join("Games/MD/Tetris.gb").exists());
}

#[test]
fn adding_a_rom_checks_the_platform_extension() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let wrong = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "Tetris.gb"),
        b"gb-rom-bytes",
        true,
    );
    assert_eq!(wrong.status, 400, "{}", body_text(&wrong));
    assert!(!dir.path().join("Games/GBA/Tetris.gb").exists());

    let missing = post_bytes(&shared, "/api/rom/add", "name=x.gba", b"x", true);
    assert_eq!(missing.status, 400, "a missing platform was accepted");
    let unknown = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("PSP", "x.gba"),
        b"x",
        true,
    );
    assert_eq!(unknown.status, 400, "an unknown platform was accepted");
}

#[test]
fn adding_a_rom_refuses_a_name_or_a_body_already_present() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let same_name = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "宝可梦.gba"),
        b"something-else",
        true,
    );
    assert_eq!(same_name.status, 409, "{}", body_text(&same_name));

    // A real card has been scanned at boot, so the importer sees a metadata
    // row (size known, hash not) instead of rescanning the library.
    let existing = slot_store::Cart {
        platform: slot_store::Platform::Gba,
        stem: "宝可梦".into(),
        rom: dir.path().join("Games/GBA/宝可梦.gba"),
        label: None,
        code: String::new(),
        shell: None,
        title: String::new(),
    model: Default::default(),
    front: None,
};
    slot_store::cart_meta::ensure(dir.path(), &existing).expect("metadata row");
    let same_body = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "副本.gba"),
        b"old-gba-rom",
        true,
    );
    assert_eq!(same_body.status, 409, "{}", body_text(&same_body));
    assert!(!dir.path().join("Games/GBA/副本.gba").exists());
}

#[test]
fn adding_a_rom_cannot_escape_the_platform_directory() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "../../escape.gba"),
        b"escape-rom",
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(dir.path().join("Games/GBA/escape.gba").is_file());
    assert!(!dir.path().join("escape.gba").exists());
    assert!(!dir.path().parent().unwrap().join("escape.gba").exists());
}

#[test]
fn adding_a_rom_requires_the_web_write_header() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post_bytes(
        &shared,
        "/api/rom/add",
        &add_query("GBA", "no-header.gba"),
        b"no-header",
        false,
    );
    assert_ne!(response.status, 200, "the write guard was skipped");
    assert!(!dir.path().join("Games/GBA/no-header.gba").exists());
}

fn get(shared: &Arc<Shared>, path: &str, query: &str) -> Response {
    route(shared, &request(Method::Get, path, query))
}

fn put(shared: &Arc<Shared>, path: &str, query: &str, body: &[u8], slot_header: bool) -> Response {
    let mut builder = request(Method::Put, path, query);
    builder.body = body.to_vec();
    if slot_header {
        builder = with_slot_header(builder);
    }
    route(shared, &builder)
}

fn put_json(shared: &Arc<Shared>, path: &str, query: &str, body: Value) -> Response {
    let mut builder = request(Method::Put, path, query);
    builder.body = serde_json::to_vec(&body).unwrap();
    route(shared, &with_slot_header(builder))
}

fn post_bytes(
    shared: &Arc<Shared>,
    path: &str,
    query: &str,
    body: &[u8],
    slot_header: bool,
) -> Response {
    let mut builder = request(Method::Post, path, query);
    builder.body = body.to_vec();
    if slot_header {
        builder = with_slot_header(builder);
    }
    route(shared, &builder)
}

fn post(shared: &Arc<Shared>, path: &str, body: Value, slot_header: bool) -> Response {
    let builder = json_request(Method::Post, path, body);
    let builder = if slot_header {
        with_slot_header(builder)
    } else {
        builder
    };
    route(shared, &builder)
}

fn post_query(shared: &Arc<Shared>, path: &str, query: &str, slot_header: bool) -> Response {
    let mut builder = request(Method::Post, path, query);
    if slot_header {
        builder = with_slot_header(builder);
    }
    route(shared, &builder)
}

fn body_text(response: &Response) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn response_json(response: &Response) -> Value {
    serde_json::from_slice(&response.body).unwrap_or(Value::Null)
}

// ------------------------------------------------------------ covers + list

#[test]
fn covers_resolve_without_scanning_the_library() {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/A.gba", b"rom");
    write_file(root, "Games/GBA/子目录/B.gba", b"rom");
    write_file(root, "Labels/GBA/A.png", b"png-a");
    write_file(root, "Labels/GBA/子目录/B.png", b"png-b");
    let shared = test_support::test_shared(root.to_path_buf());

    let flat = get(&shared, "/api/rom/cover", &rom_query("Games/GBA/A.gba"));
    assert_eq!(flat.status, 200, "{}", body_text(&flat));
    assert_eq!(flat.body, b"png-a");

    let nested = get(&shared, "/api/rom/cover", &rom_query("Games/GBA/子目录/B.gba"));
    assert_eq!(nested.status, 200, "{}", body_text(&nested));
    assert_eq!(nested.body, b"png-b");

    let missing = get(&shared, "/api/rom/cover", &rom_query("Games/GBA/没有.gba"));
    assert_eq!(missing.status, 404, "a missing cover became an error page");

    // Conditional GET: the page revalidates instead of re-reading from the card.
    let etag = flat
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .map(|(_, value)| value.clone())
        .expect("the cover reply carries no ETag");
    let mut conditional = request(Method::Get, "/api/rom/cover", &rom_query("Games/GBA/A.gba"));
    conditional.headers.insert("if-none-match".into(), etag);
    let not_modified = route(&shared, &conditional);
    assert_eq!(not_modified.status, 304, "{}", body_text(&not_modified));
    assert!(not_modified.body.is_empty());
}

#[test]
fn a_cover_from_the_metadata_row_wins_over_the_conventional_path() {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/宝可梦.gba", b"rom");
    write_file(root, "Labels/GBA/宝可梦.png", b"conventional");
    write_file(root, "Labels/GBA/custom.png", b"custom");
    let cart = slot_store::Cart {
        platform: slot_store::Platform::Gba,
        stem: "宝可梦".into(),
        rom: root.join("Games/GBA/宝可梦.gba"),
        label: None,
        code: String::new(),
        shell: None,
        title: String::new(),
    model: Default::default(),
    front: None,
};
    slot_store::cart_meta::ensure(root, &cart).unwrap();
    slot_store::cart_meta::update(root, &cart, |meta| {
        meta.cover = Some("Labels/GBA/custom.png".into());
    })
    .unwrap();
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/rom/cover", &rom_query("Games/GBA/宝可梦.gba"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.body, b"custom");
}

#[test]
fn the_game_list_is_cached_until_the_library_changes() {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/A.gba", b"rom");
    let shared = test_support::test_shared(root.to_path_buf());
    let first = get(&shared, "/api/roms", "");
    let games = response_json(&first)["games"].as_array().cloned().unwrap_or_default();
    assert_eq!(games.len(), 1);

    // A file appearing on the card without the page knowing keeps the cached
    // answer until the generation changes.
    write_file(root, "Games/GBA/B.gba", b"rom");
    let cached = get(&shared, "/api/roms", "");
    let cached_games = response_json(&cached)["games"].as_array().cloned().unwrap_or_default();
    assert_eq!(cached_games.len(), 1, "the list was rebuilt although nothing invalidated it");

    shared.touch_library_changed();
    let fresh = get(&shared, "/api/roms", "");
    let fresh_games = response_json(&fresh)["games"].as_array().cloned().unwrap_or_default();
    assert_eq!(fresh_games.len(), 2, "the cached list survived an invalidation");
}

#[test]
fn the_detail_page_reads_the_intro_text() {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/A.gba", b"rom");
    write_file(root, "Games/GBA/子目录/B.gba", b"rom");
    write_file(root, "Info/GBA/A.txt", "这是一段介绍。\n第二行。".as_bytes());
    write_file(root, "Info/GBA/子目录/B.txt", b"nested intro");
    let shared = test_support::test_shared(root.to_path_buf());

    let flat = get(&shared, "/api/rom/intro", &rom_query("Games/GBA/A.gba"));
    assert_eq!(flat.status, 200, "{}", body_text(&flat));
    let text = response_json(&flat)["text"].as_str().unwrap_or_default().to_string();
    assert_eq!(text, "这是一段介绍。\n第二行。");

    let nested = get(&shared, "/api/rom/intro", &rom_query("Games/GBA/子目录/B.gba"));
    assert_eq!(nested.status, 200, "{}", body_text(&nested));
    assert_eq!(response_json(&nested)["text"], "nested intro");

    let missing = get(&shared, "/api/rom/intro", &rom_query("Games/GBA/A.gba"));
    assert_eq!(missing.status, 200);

    let none = get(&shared, "/api/rom/intro", &rom_query("Games/GBA/没有.gba"));
    assert_eq!(none.status, 404, "a game without an intro became an error page");
}

#[test]
fn an_intro_from_the_metadata_row_wins_over_the_conventional_file() {
    let dir = temp_root();
    let root = dir.path();
    write_file(root, "Games/GBA/宝可梦.gba", b"rom");
    write_file(root, "Labels/GBA/宝可梦.txt", b"conventional");
    write_file(root, "Labels/GBA/custom.txt", b"custom-intro");
    let cart = slot_store::Cart {
        platform: slot_store::Platform::Gba,
        stem: "宝可梦".into(),
        rom: root.join("Games/GBA/宝可梦.gba"),
        label: None,
        code: String::new(),
        shell: None,
        title: String::new(),
    model: Default::default(),
    front: None,
};
    slot_store::cart_meta::ensure(root, &cart).unwrap();
    slot_store::cart_meta::update(root, &cart, |meta| {
        meta.intro = Some("Labels/GBA/custom.txt".into());
    })
    .unwrap();
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/rom/intro", &rom_query("Games/GBA/宝可梦.gba"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response_json(&response)["text"], "custom-intro");
}

// ------------------------------------------------------------- browser play

fn play_fixture() -> tempfile::TempDir {
    let dir = temp_root();
    write_file(dir.path(), "Games/GBA/宝可梦.gba", b"gba-rom-bytes");
    write_file(dir.path(), "Games/GBA/整合包/火红.gba", b"nested-rom");
    write_file(dir.path(), "Games/GB/俄罗斯方块.gb", b"gb-rom");
    write_file(dir.path(), "Games/GBC/口袋妖怪.gbc", b"gbc-rom");
    write_file(dir.path(), "Games/FC/马里奥.nes", b"nes-rom");
    dir
}

/// The browser-play page has to work over the real TCP stack, not just
/// through `route()`: the page, its offline runtime and the ROM all come from
/// the same server, and HEAD must not hang.
#[test]
fn the_play_page_is_served_over_http() {
    let dir = play_fixture();
    let server = start_server(dir.path());
    let port = server.port();
    let page = String::from_utf8_lossy(&send_raw(
        port,
        "GET /play?rom=Games/GBA/%E5%AE%9D%E5%8F%AF%E6%A2%A6.gba HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    ))
    .into_owned();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("text/html; charset=utf-8"), "{page}");
    assert!(page.contains("/play/app.js"), "the page lost its script");

    let loader = String::from_utf8_lossy(&send_raw(
        port,
        "GET /play/data/loader.js HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    ))
    .into_owned();
    assert!(loader.starts_with("HTTP/1.1 200"), "{loader}");

    let head = String::from_utf8_lossy(&send_raw(
        port,
        "HEAD /play/data/cores/mgba-legacy-wasm.data HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    ))
    .into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    let rom = String::from_utf8_lossy(&send_raw(
        port,
        "GET /api/play/rom?rom=Games/GBA/%E5%AE%9D%E5%8F%AF%E6%A2%A6.gba HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    ))
    .into_owned();
    assert!(rom.starts_with("HTTP/1.1 200"), "{rom}");
    assert!(rom.ends_with("gba-rom-bytes"), "the ROM body is missing");
}

#[test]
fn browser_play_serves_the_rom_and_the_battery_save() {
    let dir = play_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let rom = get(&shared, "/api/play/rom", &rom_query("Games/GBA/宝可梦.gba"));
    assert_eq!(rom.status, 200, "{}", body_text(&rom));
    assert_eq!(rom.body, b"gba-rom-bytes");
    assert_eq!(rom.content_type, "application/octet-stream");
    let rom_bytes = rom
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-rom-bytes"))
        .map(|(_, value)| value.clone())
        .expect("the ROM reply carries no length header");
    assert_eq!(rom_bytes, rom.body.len().to_string());

    let missing_save = get(&shared, "/api/play/save", &rom_query("Games/GBA/宝可梦.gba"));
    assert_eq!(missing_save.status, 204, "a cart without a save is not an error");

    write_file(dir.path(), "Saves/GBA/宝可梦.sav", b"old-save");
    let save = get(&shared, "/api/play/save", &rom_query("Games/GBA/宝可梦.gba"));
    assert_eq!(save.status, 200);
    assert_eq!(save.body, b"old-save");
    let stamp = save
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-save-stamp"))
        .map(|(_, value)| value.clone())
        .expect("the save reply carries no stamp");
    assert!(!stamp.is_empty());
}

#[test]
fn browser_play_writes_the_save_back_with_a_backup_and_a_stamp_check() {
    let dir = play_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = rom_query("Games/GBA/宝可梦.gba");

    let denied = put(&shared, "/api/play/save", &query, b"new", false);
    assert_eq!(denied.status, 403, "the write guard was skipped");

    write_file(dir.path(), "Saves/GBA/宝可梦.sav", b"old-save");
    let stamp = get(&shared, "/api/play/save", &query)
        .headers
        .into_iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-save-stamp"))
        .map(|(_, value)| value)
        .expect("no stamp");

    let mut write = request(Method::Put, "/api/play/save", &query);
    write.body = b"new-save".to_vec();
    write.headers.insert("x-save-stamp".into(), stamp.clone());
    let stored = route(&shared, &with_slot_header(write));
    assert_eq!(stored.status, 200, "{}", body_text(&stored));
    assert_eq!(
        std::fs::read(dir.path().join("Saves/GBA/宝可梦.sav")).unwrap(),
        b"new-save"
    );
    assert_eq!(
        std::fs::read(dir.path().join("Saves/GBA/宝可梦.sav.bak")).unwrap(),
        b"old-save",
        "the previous save was not backed up"
    );

    let mut stale = test_support::request(Method::Put, "/api/play/save", &query);
    stale.body = b"even-newer".to_vec();
    stale.headers.insert("x-save-stamp".into(), stamp);
    let conflict = route(&shared, &with_slot_header(stale));
    assert_eq!(conflict.status, 409, "a stale stamp overwrote the card save");
    assert_eq!(
        std::fs::read(dir.path().join("Saves/GBA/宝可梦.sav")).unwrap(),
        b"new-save"
    );
}

#[test]
fn browser_play_serves_every_supported_platform() {
    let dir = play_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    for (rom, save_dir, bytes) in [
        ("Games/GB/俄罗斯方块.gb", "GB", b"gb-rom".as_slice()),
        ("Games/GBC/口袋妖怪.gbc", "GBC", b"gbc-rom".as_slice()),
        ("Games/FC/马里奥.nes", "FC", b"nes-rom".as_slice()),
    ] {
        let response = get(&shared, "/api/play/rom", &rom_query(rom));
        assert_eq!(response.status, 200, "{rom}: {}", body_text(&response));
        assert_eq!(response.body, bytes);
        let write = put(&shared, "/api/play/save", &rom_query(rom), b"save", true);
        assert_eq!(write.status, 200, "{rom}: {}", body_text(&write));
        let stem = rom.rsplit('/').next().unwrap().rsplit_once('.').unwrap().0;
        assert!(
            dir.path().join(format!("Saves/{save_dir}/{stem}.sav")).is_file(),
            "{rom} did not get a {save_dir} save"
        );
    }
}

#[test]
fn browser_play_accepts_roms_in_subfolders_with_a_flat_save_name() {
    let dir = play_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = rom_query("Games/GBA/整合包/火红.gba");
    let rom = get(&shared, "/api/play/rom", &query);
    assert_eq!(rom.status, 200, "{}", body_text(&rom));
    assert_eq!(rom.body, b"nested-rom");

    let write = put(&shared, "/api/play/save", &query, b"fire-red-save", true);
    assert_eq!(write.status, 200, "{}", body_text(&write));
    assert_eq!(
        std::fs::read(dir.path().join("Saves/GBA/火红.sav")).unwrap(),
        b"fire-red-save",
        "a nested ROM did not get the flat save path"
    );
}

#[test]
fn browser_play_rejects_paths_outside_games_or_unknown_platforms() {
    let dir = play_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    for query in [
        rom_query("Games/GBA/../secret.gba"),
        rom_query("Games/PSP/宝可梦.gba"),
        rom_query("Saves/GBA/宝可梦.sav"),
        rom_query("Games/GBA/宝可梦.gb"),
    ] {
        let response = get(&shared, "/api/play/rom", &query);
        assert_eq!(response.status, 400, "{query} was accepted");
    }
    let missing = get(&shared, "/api/play/rom", &rom_query("Games/GBA/没有.gba"));
    assert_eq!(missing.status, 404);
}

// ------------------------------------------------------------------ HTTP

fn send_raw(port: u16, raw: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}

fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let position = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response has header terminator");
    (
        String::from_utf8_lossy(&raw[..position]).into_owned(),
        raw[position + 4..].to_vec(),
    )
}

#[test]
fn serves_static_page_and_api_over_tcp() {
    let dir = save_fixture();
    let mut server = start_server(dir.path());
    assert!(server.running());
    let port = server.port();
    assert_eq!(server.url(), format!("http://127.0.0.1:{port}"));

    let raw = send_raw(port, "GET / HTTP/1.1\r\nHost: slot\r\n\r\n");
    let (header, body) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 200 OK"), "{header}");
    assert!(header.contains("Content-Type: text/html; charset=utf-8"));
    assert!(header.contains("Connection: close"));
    assert!(!header.to_ascii_lowercase().contains("access-control-allow"));
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("linkSlot"));

    let raw = send_raw(port, "GET /app.js HTTP/1.1\r\nHost: slot\r\n\r\n");
    let (header, body) = split_response(&raw);
    assert!(header.contains("application/javascript"));
    assert!(!body.is_empty());

    let raw = send_raw(port, "GET /api/status HTTP/1.1\r\nHost: slot\r\n\r\n");
    let (header, body) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 200 OK"), "{header}");
    assert!(header.contains("application/json"));
    let status: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status["web"]["port"], port);
    assert_eq!(status["storage"]["states"], 2);

    // HEAD returns headers only.
    let raw = send_raw(port, "HEAD / HTTP/1.1\r\nHost: slot\r\n\r\n");
    let (header, body) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 200 OK"));
    assert!(body.is_empty());

    server.stop();
    assert!(!server.running());
    server.stop();
    // The port must be released after stop.
    std::net::TcpListener::bind(("127.0.0.1", port)).expect("port released");
}

#[test]
fn rejects_oversized_and_malformed_requests_over_tcp() {
    let dir = temp_root();
    let server = start_server(dir.path());
    let port = server.port();

    let raw = send_raw(port, "GET / HTTP/1.1\r\nContent-Length: bad\r\n\r\n");
    let (header, _) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 400"), "{header}");

    let raw = send_raw(port, "BREW / HTTP/1.1\r\n\r\n");
    let (header, _) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 405"), "{header}");

    let raw = send_raw(port, "GET /api/nope HTTP/1.1\r\n\r\n");
    let (header, _) = split_response(&raw);
    assert!(header.starts_with("HTTP/1.1 404"), "{header}");
}

// ------------------------------------------------------------------ 概览

#[test]
fn status_json_shape() {
    let dir = save_fixture();
    write_file(dir.path(), "States/GBC/mgba/Legend/resume.state", b"x");
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = get(&shared, "/api/status", "");
    assert_eq!(response.status, 200);
    let value = body_json(&response);

    assert_eq!(value["slot"], slot_ui::PROJECT_VERSION);
    assert!(value["linkslot"].is_string());
    assert!(value["uptime_secs"].as_u64().is_some());
    assert_eq!(value["web"]["port"], 18080);
    assert!(value["web"]["url"].as_str().unwrap().starts_with("http://"));
    assert_eq!(value["network"]["connected"], false);
    assert_eq!(value["storage"]["games"], 2);
    assert_eq!(value["storage"]["states"], 3);
    assert_eq!(value["storage"]["battery"], 1);
    // Cross-device playtime of the only unit that has playtime.json.
    assert_eq!(value["storage"]["playtime_seconds"], 10800);
    assert_eq!(value["writable"], true);

    shared.writable.store(false, Ordering::Relaxed);
    let value = body_json(&get(&shared, "/api/status", ""));
    assert_eq!(value["writable"], false);
}

#[test]
fn unknown_api_and_wrong_methods_are_structured_errors() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = get(&shared, "/api/nope", "");
    assert_eq!(response.status, 404);
    assert!(response_json(&response)["error"]
        .as_str()
        .unwrap()
        .contains("接口"));
    let response = route(&shared, &request(Method::Post, "/api/status", ""));
    assert_eq!(response.status, 405);
    assert!(response_json(&response)["error"].is_string());
}

// ------------------------------------------------------------------ 存档

#[test]
fn saves_json_shape() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = get(&shared, "/api/saves", "");
    assert_eq!(response.status, 200);
    let value = body_json(&response);
    let units = value["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(unit["platform"], "GBA");
    assert_eq!(unit["emulator"], "mgba");
    assert_eq!(unit["game"], "宝可梦");
    let states = unit["states"].as_array().unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(states[0]["name"], "resume");
    assert_eq!(states[0]["resume"], true);
    assert!(states[0]["stamp"].is_null());
    assert!(states[0]["thumb"]
        .as_str()
        .unwrap()
        .starts_with("/api/file?path="));
    assert_eq!(states[1]["name"], "2026-10-21_10-00-00");
    assert_eq!(states[1]["resume"], false);
    assert_eq!(states[1]["stamp"], "2026-10-21_10-00-00");
    assert!(states[1]["size"].as_u64().unwrap() > 0);
    assert!(states[1]["mtime"].as_u64().is_some());
    assert_eq!(unit["playtime_seconds"], 7200);
    assert_eq!(unit["playtime_total_seconds"], 10800);
    assert_eq!(unit["battery"][0]["path"], "Saves/GBA/宝可梦.sav");
}

#[test]
fn saves_json_playtime_defaults_for_bad_or_missing_file() {
    let dir = save_fixture();
    let root = dir.path();
    // Corrupt playtime.json: read as 0, never fail the listing.
    write_file(root, "States/GBC/mgba/Legend/resume.state", b"x");
    write_file(root, "States/GBC/mgba/Legend/playtime.json", b"{broken");
    // Missing playtime.json: 0 as well.
    write_file(root, "States/GB/mgba/Tetris/resume.state", b"x");

    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/saves", "");
    assert_eq!(response.status, 200);
    let value = body_json(&response);
    let units = value["units"].as_array().unwrap();
    assert_eq!(units.len(), 3);
    let pokemon = units
        .iter()
        .find(|unit| unit["game"] == "宝可梦")
        .expect("宝可梦 unit");
    assert_eq!(pokemon["playtime_seconds"], 7200);
    assert_eq!(pokemon["playtime_total_seconds"], 10800);
    let legend = units
        .iter()
        .find(|unit| unit["game"] == "Legend")
        .expect("Legend unit");
    assert_eq!(legend["playtime_seconds"], 0);
    assert_eq!(legend["playtime_total_seconds"], 0);
    let tetris = units
        .iter()
        .find(|unit| unit["game"] == "Tetris")
        .expect("Tetris unit");
    assert_eq!(tetris["playtime_seconds"], 0);
    assert_eq!(tetris["playtime_total_seconds"], 0);

    let status = body_json(&get(&shared, "/api/status", ""));
    assert_eq!(status["storage"]["playtime_seconds"], 10800);
}

// ------------------------------------------------------------------ 文件

#[test]
fn downloads_state_with_attachment_and_png_inline() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let state_path = util::encode_query("States/GBA/mgba/宝可梦/resume.state");
    let response = get(&shared, "/api/file", &format!("path={state_path}"));
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "application/octet-stream");
    assert_eq!(response.body, b"resume-state");
    let disposition = response
        .headers
        .iter()
        .find(|(name, _)| name == "Content-Disposition")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert!(disposition.starts_with("attachment;"));
    assert!(disposition.contains("filename=\"resume.state\""));
    assert!(disposition.contains("filename*=UTF-8''resume.state"));

    let thumb_path = util::encode_query("States/GBA/mgba/宝可梦/resume.png");
    let response = get(&shared, "/api/file", &format!("path={thumb_path}"));
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "image/png");
    assert_eq!(response.body, b"png");
    let disposition = response
        .headers
        .iter()
        .find(|(name, _)| name == "Content-Disposition")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert!(disposition.starts_with("inline;"));

    let response = get(
        &shared,
        "/api/file",
        "path=Saves%2FGBA%2F%E5%AE%9D%E5%8F%AF%E6%A2%A6.sav",
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"battery");
    let disposition = response
        .headers
        .iter()
        .find(|(name, _)| name == "Content-Disposition")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert!(
        disposition.contains("filename*=UTF-8''%E5%AE%9D%E5%8F%AF%E6%A2%A6.sav"),
        "{disposition}"
    );
}

#[test]
fn file_paths_are_locked_to_states_and_saves() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    for query in [
        "path=%2Fetc%2Fpasswd",
        "path=States%2F..%2Fetc%2Fpasswd",
        "path=States%2FGBA%2F..%2F..%2FConfig%2Fx.sav",
        "path=linkSlot%2FConfig%2Fcache%2Fdeadbeef.json",
        "path=States%2FGBA%2Fmgba%2F%E5%AE%9D%E5%8F%AF%E6%A2%A6%2Fnotes.txt",
        "path=",
        "",
    ] {
        let response = get(&shared, "/api/file", query);
        assert!(
            response.status == 400 || response.status == 403,
            "{query}: {}",
            body_text(&response)
        );
        assert!(response_json(&response)["error"].is_string());
    }
    let response = get(
        &shared,
        "/api/file",
        "path=States%2FGBA%2Fmgba%2F%E5%AE%9D%E5%8F%AF%E6%A2%A6%2Fmissing.state",
    );
    assert_eq!(response.status, 404);
}

// ------------------------------------------------------------------ 上传

#[test]
fn upload_requires_header_and_writable() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = format!(
        "path={}",
        util::encode_query("States/GBA/mgba/宝可梦/resume.state")
    );

    let response = put(&shared, "/api/file", &query, b"new", false);
    assert_eq!(response.status, 403);
    assert!(body_text(&response).contains("X-Slot-Web"));

    shared.writable.store(false, Ordering::Relaxed);
    let response = put(&shared, "/api/file", &query, b"new", true);
    assert_eq!(response.status, 409);
    assert!(body_text(&response).contains("设备正在游戏中或同步中"));

    shared.writable.store(true, Ordering::Relaxed);
    let response = put(&shared, "/api/file", &query, b"new-state", true);
    assert_eq!(response.status, 200);
    let value = response_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["size"], 9);
    assert_eq!(
        std::fs::read(dir.path().join("States/GBA/mgba/宝可梦/resume.state")).unwrap(),
        b"new-state"
    );
    // The response header is still required when the body is empty.
    let response = put(&shared, "/api/file", &query, b"", true);
    assert_eq!(response.status, 200);
}

#[test]
fn upload_creates_missing_directories_and_leaves_no_temp_files() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = format!(
        "path={}",
        util::encode_query("States/GBA/mgba/New Game/resume.state")
    );
    let response = put(&shared, "/api/file", &query, b"fresh", true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let target = dir.path().join("States/GBA/mgba/New Game/resume.state");
    assert_eq!(std::fs::read(&target).unwrap(), b"fresh");
    let leftovers: Vec<String> = std::fs::read_dir(target.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp") || name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn upload_rejects_traversal_and_foreign_extensions() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    for query in [
        format!("path={}", util::encode_query("States/../../etc/passwd")),
        format!("path={}", util::encode_query("/etc/passwd.state")),
        format!("path={}", util::encode_query("linkSlot/Config/config.toml")),
        format!("path={}", util::encode_query("States/GBA/mgba/x.jpg")),
    ] {
        let response = put(&shared, "/api/file", &query, b"x", true);
        assert!(
            response.status == 400 || response.status == 403,
            "{query}: {}",
            body_text(&response)
        );
    }
}

#[test]
fn delete_save_requires_header_writable_and_removes_state_thumbnail() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let root = dir.path();
    let state = "States/GBA/mgba/宝可梦/resume.state";
    let query = format!("path={}", util::encode_query(state));

    let response = post_query(&shared, "/api/saves/delete", &query, false);
    assert_eq!(response.status, 403);
    assert!(root.join(state).exists());

    shared.writable.store(false, Ordering::Relaxed);
    let response = post_query(&shared, "/api/saves/delete", &query, true);
    assert_eq!(response.status, 409);
    assert!(root.join(state).exists());

    shared.writable.store(true, Ordering::Relaxed);
    let response = post_query(&shared, "/api/saves/delete", &query, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = response_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["thumbnail_removed"], true);
    assert!(!root.join(state).exists());
    assert!(!root.join("States/GBA/mgba/宝可梦/resume.png").exists());
    assert!(root
        .join("States/GBA/mgba/宝可梦/2026-10-21_10-00-00.state")
        .exists());

    let battery = "Saves/GBA/宝可梦.sav";
    let query = format!("path={}", util::encode_query(battery));
    let response = post_query(&shared, "/api/saves/delete", &query, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(!root.join(battery).exists());

    let response = post_query(
        &shared,
        "/api/saves/delete",
        &format!("path={}", util::encode_query("linkSlot/Config/config.toml")),
        true,
    );
    assert!(response.status == 400 || response.status == 403);
    assert!(root.join("linkSlot/Config/cache/deadbeef.json").exists());
}

// ------------------------------------------------------------------ 云同步

#[test]
fn sync_info_shape() {
    let dir = save_fixture();
    let root = dir.path();
    write_file(
        root,
        "linkSlot/Config/cache/deadbeef.json",
        r#"{"unit":{"platform":"GBA","emulator":"mgba","game":"宝可梦"},
             "version":2,"synced_at":"2026-10-21T10:00:00Z",
             "remote":{"url":"https://nas/dav","remote_root":"/linkSlot"}}"#
            .as_bytes(),
    );
    write_file(root, "linkSlot/Config/cache/corrupt.json", b"{not json");
    let mut log = String::new();
    for index in 0..300 {
        log.push_str(&format!("log-line-{index}\n"));
    }
    write_file(root, "linkSlot/Logs/linkSlot.log", log.as_bytes());

    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/sync", "");
    assert_eq!(response.status, 200);
    let value = body_json(&response);
    assert!(value["webdav"]["url"].is_string());
    assert!(value["webdav"]["remote_root"].is_string());
    assert!(value["webdav"]["username"].is_string());
    assert!(value["webdav"]["password_set"].is_boolean());
    assert!(value["active_storage"].is_string());
    assert!(value["storages"].is_array());
    assert_eq!(value["storages"].as_array().unwrap().len(), 1);
    assert_eq!(value["storages"][0]["id"], "default");
    assert!(value["webdav"]["ca_cert"].is_string());
    assert!(value["webdav"]["insecure"].is_boolean());
    assert!(value["keep_remote"].is_boolean());
    assert_eq!(value["log_level"], "info");
    let caches = value["caches"].as_array().unwrap();
    assert_eq!(caches.len(), 1);
    assert_eq!(caches[0]["game"], "宝可梦");
    assert_eq!(caches[0]["version"], 2);
    let log_tail = value["log_tail"].as_array().unwrap();
    assert_eq!(log_tail.len(), 200);
    assert_eq!(log_tail[199], "log-line-299");
}

#[test]
fn web_upload_limit_defaults_to_32_and_is_saved_per_game_card() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let value = body_json(&get(&shared, "/api/settings", ""));
    assert_eq!(value["max_upload_mb"], 32);
    assert_eq!(value["min_upload_mb"], 1);
    assert_eq!(value["max_limit_mb"], 256);
    assert_eq!(shared.upload_limit(), 32 * 1024 * 1024);

    let refused = post(&shared, "/api/settings", serde_json::json!({"max_upload_mb": 48}), false);
    assert_eq!(refused.status, 403);
    let saved = post(&shared, "/api/settings", serde_json::json!({"max_upload_mb": 48}), true);
    assert_eq!(saved.status, 200, "{}", body_text(&saved));
    assert!(dir.path().join("linkSlot/Config/web.toml").is_file());
    assert_eq!(shared.upload_limit(), 48 * 1024 * 1024);
    assert_eq!(body_json(&get(&shared, "/api/settings", ""))["max_upload_mb"], 48);

    for bad in [serde_json::json!({"max_upload_mb": 0}), serde_json::json!({"max_upload_mb": 257})] {
        assert_eq!(post(&shared, "/api/settings", bad, true).status, 400);
    }
}

#[test]
fn the_i18n_endpoint_follows_the_language_file() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let value = body_json(&get(&shared, "/api/i18n", ""));
    assert_eq!(value["language"], "zh-CN", "a card without the file is Chinese");
    assert_eq!(value["strings"]["device.quick.label.language"], "语言");

    // The device writes the file; the page must follow without a restart.
    slot_store::write_language(dir.path(), slot_i18n::Language::EnUs).unwrap();
    let value = body_json(&get(&shared, "/api/i18n", ""));
    assert_eq!(value["language"], "en-US");
    assert_eq!(value["strings"]["device.quick.label.language"], "Language");
    assert_eq!(
        value["strings"].as_object().unwrap().len(),
        slot_i18n::len(slot_i18n::Language::EnUs),
        "the page gets the whole table, not a per-page slice"
    );
    // The management page needs its own domain, or every label falls back.
    let strings = value["strings"].as_object().unwrap();
    let web = strings.keys().filter(|key| key.starts_with("web.")).count();
    assert!(web > 100, "only {web} web.* keys reached the page");
}

#[test]
fn the_i18n_endpoint_can_preview_a_language_and_rejects_unknown_codes() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let value = body_json(&get(&shared, "/api/i18n", "lang=en_US"));
    assert_eq!(value["language"], "en-US");
    assert_eq!(value["strings"]["device.quick.value.language_en"], "English");

    let refused = get(&shared, "/api/i18n", "lang=fr-FR");
    assert_eq!(refused.status, 400);
    let message = body_json(&refused)["error"].as_str().unwrap().to_string();
    assert!(message.contains("fr-FR"), "{message}");
}

#[test]
fn api_errors_follow_the_language_file() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let missing = get(&shared, "/api/does-not-exist", "");
    assert_eq!(missing.status, 404);
    assert!(body_text(&missing).contains("接口不存在"));

    slot_store::write_language(dir.path(), slot_i18n::Language::EnUs).unwrap();

    let missing = get(&shared, "/api/does-not-exist", "");
    assert_eq!(missing.status, 404);
    assert!(
        body_text(&missing).contains("Unknown API route"),
        "{}",
        body_text(&missing)
    );

    let settings = post(
        &shared,
        "/api/settings",
        serde_json::json!({ "language": "en-US" }),
        false,
    );
    assert_eq!(settings.status, 403);
    assert!(
        body_text(&settings).contains("X-Slot-Web: 1"),
        "{}",
        body_text(&settings)
    );

    let bad_rom = get(&shared, "/api/play/rom", "rom=not-a-game");
    assert_eq!(bad_rom.status, 400);
    assert!(
        body_text(&bad_rom).contains("Only ROMs under Games/"),
        "{}",
        body_text(&bad_rom)
    );

    // A deep file-browser error still has a Chinese detail internally; English
    // mode gets the generic translated error instead of leaking it.
    let deep = get(&shared, "/api/files", "path=..");
    assert_eq!(deep.status, 400, "{}", body_text(&deep));
    let deep_error = body_json(&deep)["error"].as_str().unwrap_or("").to_string();
    assert!(!deep_error.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "{deep_error}");
    assert!(deep_error.contains("Request failed"), "{deep_error}");
}

#[test]
fn the_web_page_can_read_and_set_the_language() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let value = body_json(&get(&shared, "/api/settings", ""));
    assert_eq!(value["language"], "zh-CN");
    let codes: Vec<&str> = value["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["zh-CN", "en-US"]);

    // Writes need the page's header, like every other mutating endpoint.
    let refused = post(
        &shared,
        "/api/settings",
        serde_json::json!({ "language": "en-US" }),
        false,
    );
    assert_eq!(refused.status, 403);
    assert_eq!(
        slot_store::read_language(dir.path()),
        slot_i18n::Language::ZhCn
    );

    let saved = post(
        &shared,
        "/api/settings",
        serde_json::json!({ "language": "en-US" }),
        true,
    );
    assert_eq!(saved.status, 200, "{}", body_text(&saved));
    assert_eq!(body_json(&saved)["language"], "en-US");
    assert!(dir.path().join("linkSlot/Config/language.txt").is_file());
    assert_eq!(
        slot_store::read_language(dir.path()),
        slot_i18n::Language::EnUs
    );

    // Changing the language must not disturb the upload ceiling.
    assert_eq!(body_json(&get(&shared, "/api/settings", ""))["max_upload_mb"], 32);
}

#[test]
fn an_unknown_language_from_the_page_is_refused() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let refused = post(
        &shared,
        "/api/settings",
        serde_json::json!({ "language": "fr-FR" }),
        true,
    );
    assert_eq!(refused.status, 400);
    let message = body_json(&refused)["error"].as_str().unwrap().to_string();
    assert!(message.contains("fr-FR"), "{message}");
    assert!(!dir.path().join("linkSlot/Config/language.txt").exists());
}

#[test]
fn an_empty_settings_body_is_refused_and_writes_nothing() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    for body in [serde_json::json!({}), serde_json::json!({ "unrelated": 1 })] {
        let refused = post(&shared, "/api/settings", body.clone(), true);
        assert_eq!(refused.status, 400, "{body}");
        // The message must say "nothing to save", not "language is missing":
        // an empty body never mentioned a language.
        let message = body_json(&refused)["error"].as_str().unwrap().to_string();
        assert!(
            message.contains("没有任何可保存"),
            "wrong message for {body}: {message}"
        );
    }
    assert!(!dir.path().join("linkSlot/Config/language.txt").exists());
    assert!(!dir.path().join("linkSlot/Config/web.toml").exists());

    // A `language` field of the wrong JSON type is still a client error, but a
    // different one: the value is missing rather than unrecognised.
    let refused = post(
        &shared,
        "/api/settings",
        serde_json::json!({ "language": 5 }),
        true,
    );
    assert_eq!(refused.status, 400);
    let message = body_json(&refused)["error"].as_str().unwrap().to_string();
    assert!(message.contains("language"), "{message}");
}

#[test]
fn log_level_can_be_changed_from_the_web_page() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post(
        &shared,
        "/api/log/level",
        serde_json::json!({ "level": "debug" }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        crate::linkcfg::read_log_level(dir.path()),
        crate::linkcfg::LogLevel::Debug
    );
    let value = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(value["log_level"], "debug");

    let response = post(
        &shared,
        "/api/log/level",
        serde_json::json!({ "level": "verbose" }),
        true,
    );
    assert_eq!(response.status, 400);
}

#[test]
fn sync_test_is_guarded_like_the_other_write_endpoints() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post(&shared, "/api/sync/test", serde_json::json!({}), false);
    assert_eq!(response.status, 403);
    shared.writable.store(false, Ordering::Relaxed);
    let response = post(&shared, "/api/sync/test", serde_json::json!({}), true);
    assert_eq!(response.status, 409);
}

#[test]
fn sync_config_and_run_are_guarded() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let body = serde_json::json!({ "url": "https://nas/dav", "remote_root": "/linkSlot" });

    let response = post(&shared, "/api/sync/config", body.clone(), false);
    assert_eq!(response.status, 403);
    shared.writable.store(false, Ordering::Relaxed);
    let response = post(&shared, "/api/sync/config", body.clone(), true);
    assert_eq!(response.status, 409);
    shared.writable.store(true, Ordering::Relaxed);
    // Validation happens before linkcfg is touched.
    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({ "url": "ftp://bad" }),
        true,
    );
    assert_eq!(response.status, 400);
    assert!(body_text(&response).contains("http://"));

    let response = post(&shared, "/api/sync/run", serde_json::json!({}), true);
    assert_eq!(response.status, 400);
    let response = post(
        &shared,
        "/api/sync/run",
        serde_json::json!({ "platform": "GBA", "emulator": "mgba", "game": "a/b" }),
        true,
    );
    assert_eq!(response.status, 400);
}

#[cfg(unix)]
#[test]
fn sync_run_reports_conflict_exit_code() {
    let dir = save_fixture();
    let root = dir.path();
    write_file(
        root,
        "System/linkSlot/shell/linkslot.sh",
        b"#!/bin/sh\necho 'LINKSLOT_CONFLICT=1'\necho 'conflict detected' >&2\nexit 2\n",
    );
    let shared = test_support::test_shared(root.to_path_buf());
    let response = post(
        &shared,
        "/api/sync/run",
        serde_json::json!({ "platform": "GBA", "emulator": "mgba", "game": "宝可梦" }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], 2);
    assert!(value["hint"].as_str().unwrap().contains("冲突"));

    write_file(
        root,
        "System/linkSlot/shell/linkslot.sh",
        b"#!/bin/sh\necho RESULT=ok\nexit 0\n",
    );
    let response = post(
        &shared,
        "/api/sync/run",
        serde_json::json!({ "platform": "GBA", "emulator": "mgba", "game": "宝可梦" }),
        true,
    );
    let value = body_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["code"], 0);
}

// ------------------------------------------------------------------ 网络

#[test]
fn network_writes_are_guarded() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post(
        &shared,
        "/api/network/connect",
        serde_json::json!({ "ssid": "home", "psk": "pw" }),
        false,
    );
    assert_eq!(response.status, 403);
    shared.writable.store(false, Ordering::Relaxed);
    let response = post(
        &shared,
        "/api/network/connect",
        serde_json::json!({ "ssid": "home", "psk": "pw" }),
        true,
    );
    assert_eq!(response.status, 409);
    shared.writable.store(true, Ordering::Relaxed);
    let response = post(
        &shared,
        "/api/network/connect",
        serde_json::json!({ "psk": "pw" }),
        true,
    );
    assert_eq!(response.status, 400);
    // Without a slot-wifi.sh in the temp root the network reads fail cleanly.
    let response = get(&shared, "/api/network", "");
    assert!(response.status == 200 || response.status == 500);
    assert!(response_json(&response).is_object());
}

// ------------------------------------------------- 真实 linkcfg / wifi_job 集成

#[test]
fn sync_config_roundtrips_through_linkcfg() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "url": "https://nas.example.com/dav",
            "remote_root": "/linkSlot",
            "username": "bob",
            "password": "secret",
            "ca_cert": "",
            "insecure": false,
            "keep_remote": true,
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let info = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(info["webdav"]["url"], "https://nas.example.com/dav");
    assert_eq!(info["webdav"]["username"], "bob");
    assert_eq!(info["webdav"]["password_set"], true);
    assert_eq!(info["keep_remote"], true);

    // Omitting the password keeps the stored one.
    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "url": "https://nas.example.com/dav",
            "remote_root": "/linkSlot",
            "username": "bob",
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let info = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(info["webdav"]["password_set"], true);

    // clear_password erases it (linkcfg stores `password = ""`).
    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "url": "https://nas.example.com/dav",
            "remote_root": "/linkSlot",
            "username": "bob",
            "clear_password": true,
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let info = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(info["webdav"]["password_set"], false);
}

#[test]
fn sync_config_saves_named_storages_and_one_active() {
    let dir = save_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "active": "work",
            "keep_remote": true,
            "profiles": [
                {
                    "id": "home", "name": "家里", "kind": "webdav",
                    "url": "https://home.example/dav", "remote_root": "/home",
                    "username": "u1", "password": "p1", "ca_cert": "", "insecure": false
                },
                {
                    "id": "work", "name": "公司", "kind": "webdav",
                    "url": "https://work.example/dav", "remote_root": "/work",
                    "username": "u2", "password": "p2", "ca_cert": "", "insecure": false
                }
            ]
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let info = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(info["active_storage"], "work");
    assert_eq!(info["storages"].as_array().unwrap().len(), 2);
    assert_eq!(info["webdav"]["url"], "https://work.example/dav");
    assert_eq!(info["webdav"]["username"], "u2");
    assert_eq!(info["webdav"]["password_set"], true);
    assert!(dir.path().join("linkSlot/Config/storages.toml").is_file());

    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "active": "home",
            "profiles": [
                {
                    "id": "home", "name": "家里", "kind": "webdav",
                    "url": "https://home.example/dav", "remote_root": "/home",
                    "username": "u1", "ca_cert": "", "insecure": false
                },
                {
                    "id": "work", "name": "公司", "kind": "webdav",
                    "url": "https://work.example/dav", "remote_root": "/work",
                    "username": "u2", "ca_cert": "", "insecure": false
                }
            ]
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let info = body_json(&get(&shared, "/api/sync", ""));
    assert_eq!(info["active_storage"], "home");
    assert_eq!(info["webdav"]["url"], "https://home.example/dav");

    let response = post(
        &shared,
        "/api/sync/config",
        serde_json::json!({
            "active": "missing",
            "profiles": [{
                "id": "home", "name": "家里", "kind": "webdav",
                "url": "https://home.example/dav", "remote_root": "/home"
            }]
        }),
        true,
    );
    assert_eq!(response.status, 400);
    assert!(body_text(&response).contains("生效"));
}

#[test]
fn start_without_wifi_reports_chinese_hint() {
    let dir = temp_root();
    let error = match WebServer::start(dir.path().to_path_buf()) {
        Ok(_) => panic!("start must fail without a connected Wi-Fi"),
        Err(error) => error,
    };
    assert_eq!(error, "请先在 网络设置 → WiFi 连接 中连接 WiFi");
}

#[cfg(unix)]
fn install_wifi_script(root: &Path) {
    // status / scan / connect fake implementing net-core's slot-wifi.sh contract.
    write_file(
        root,
        "System/linkSlot/shell/slot-wifi.sh",
        b"#!/bin/sh\n\
          dir=$(dirname \"$0\")\n\
          case \"$1\" in\n\
          status)\n\
            if [ -f \"$dir/wifi-up\" ]; then\n\
              printf 'state=connected\\nssid=home\\nip=192.168.1.42\\n'\n\
            else\n\
              printf 'state=disconnected\\nssid=\\nip=\\n'\n\
            fi\n\
            ;;\n\
          scan)\n\
            printf 'HomeNet\\t-52\\t1\\nOpenNet\\t-78\\t0\\nHomeNet\\t-70\\t1\\n'\n\
            ;;\n\
          connect)\n\
            : > \"$dir/wifi-up\"\n\
            ;;\n\
          esac\n",
    );
}

#[cfg(unix)]
#[test]
fn network_status_scan_and_connect_use_the_real_script() {
    let dir = save_fixture();
    let root = dir.path();
    install_wifi_script(root);
    let shared = test_support::test_shared(root.to_path_buf());

    let response = get(&shared, "/api/network", "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["connected"], false);

    let response = get(&shared, "/api/network/scan", "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let networks = body_json(&response)["networks"].as_array().unwrap().clone();
    assert_eq!(
        networks.len(),
        2,
        "duplicate SSIDs are merged: {networks:?}"
    );
    assert_eq!(networks[0]["ssid"], "HomeNet");
    assert_eq!(networks[0]["signal"], -52);
    assert_eq!(networks[0]["secured"], true);
    assert_eq!(networks[1]["ssid"], "OpenNet");

    let response = post(
        &shared,
        "/api/network/connect",
        serde_json::json!({ "ssid": "home", "psk": "secret-pass" }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["connected"], true);
    assert_eq!(value["ip"], "192.168.1.42");

    // The Wi-Fi settings are persisted through linkcfg.
    let config = crate::linkcfg::read(root);
    assert_eq!(config.wifi.ssid, "home");
    assert!(config.wifi.psk_set);
    assert!(config.wifi.manage);
}

#[cfg(unix)]
#[test]
fn status_reads_and_caches_the_link_slot_cli_version() {
    let dir = save_fixture();
    let root = dir.path();
    write_file(
        root,
        "System/linkSlot/bin/linkslot_sync",
        b"#!/bin/sh\nprintf 'linkSlot 0.0.2\\n'\n",
    );
    let shared = test_support::test_shared(root.to_path_buf());
    let value = body_json(&get(&shared, "/api/status", ""));
    assert_eq!(value["linkslot"], "0.0.2");
    // Cached: a second request still reports the version even after removal.
    std::fs::remove_file(root.join("System/linkSlot/bin/linkslot_sync")).unwrap();
    let value = body_json(&get(&shared, "/api/status", ""));
    assert_eq!(value["linkslot"], "0.0.2");
}

#[test]
fn uploaded_state_shows_up_in_the_saves_listing() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = format!(
        "path={}",
        util::encode_query("States/GBC/mgba/Legend/resume.state")
    );
    let response = put(&shared, "/api/file", &query, b"fresh-state", true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&get(&shared, "/api/saves", ""));
    let units = value["units"].as_array().unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0]["platform"], "GBC");
    assert_eq!(units[0]["game"], "Legend");
    assert_eq!(units[0]["states"][0]["resume"], true);
    assert_eq!(units[0]["states"][0]["size"], 11);
}

#[test]
fn rom_library_lists_relative_paths_and_label_geometry() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/roms", "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.content_type, "application/json; charset=utf-8");
    let value = body_json(&response);
    let games = value["games"].as_array().expect("games array");
    assert_eq!(games.len(), 2);
    let emerald = games
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .expect("GBA game");
    assert_eq!(emerald["platform"], "GBA");
    assert_eq!(emerald["file"], "宝可梦.gba");
    assert_eq!(emerald["size"], 11);
    assert_eq!(emerald["has_cover"], true);
    assert_eq!(emerald["cover_width"], 492);
    assert_eq!(emerald["cover_height"], 258);
    assert!(emerald["cover"]
        .as_str()
        .unwrap()
        .starts_with("/api/rom/cover?rom="));
    assert!(!body_text(&response).contains(root.to_string_lossy().as_ref()));
}

#[test]
fn setting_a_cart_colour_writes_the_per_game_metadata() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let query = rom_query("Games/GBA/宝可梦.gba");

    let response = put_json(&shared, "/api/rom/color", &query, serde_json::json!({"color": "#12ab34"}));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response_json(&response)["color"], "#12ab34");
    let saved = slot_store::cart_meta::read(
        root,
        slot_store::Platform::Gba,
        &root.join("Games/GBA/宝可梦.gba"),
        "宝可梦",
    )
    .unwrap();
    assert_eq!(saved.color.as_deref(), Some("#12ab34"));

    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["color"], "#12ab34");

    let bad = put_json(&shared, "/api/rom/color", &query, serde_json::json!({"color": "red"}));
    assert_eq!(bad.status, 400);
    let missing = put_json(&shared, "/api/rom/color", &query, serde_json::json!({}));
    assert_eq!(missing.status, 400);
    let guarded = {
        let mut builder = request(Method::Put, "/api/rom/color", &query);
        builder.body = serde_json::to_vec(&serde_json::json!({"color": "#ffffff"})).unwrap();
        route(&shared, &builder)
    };
    assert_eq!(guarded.status, 403);
}

#[test]
fn the_mod_migration_endpoint_imports_legacy_sidecars() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    std::fs::create_dir_all(root.join("linkSlot/Metadata/GBA")).unwrap();
    std::fs::write(
        root.join("linkSlot/Metadata/GBA/宝可梦.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "platform": "GBA",
            "rom": "Games/GBA/宝可梦.gba",
            "color": "#12ab34",
            "title": "宝可梦"
        }))
        .unwrap(),
    )
    .unwrap();

    let response = post(&shared, "/api/library/migrate", serde_json::json!({}), true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response_json(&response)["imported"], 1);
    let info = body_json(&get(&shared, "/api/roms", ""));
    let game = info["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["color"], "#12ab34");
}

#[test]
fn setting_a_cheat_path_writes_the_per_game_metadata() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let query = rom_query("Games/GBA/宝可梦.gba");
    let cheat = root.join("Cheats/GBA/mgba/宝可梦.cht");
    std::fs::create_dir_all(cheat.parent().unwrap()).unwrap();
    std::fs::write(&cheat, b"cheat0_code=00").unwrap();

    let conventional_value = body_json(&get(&shared, "/api/roms", ""));
    let conventional_game = conventional_value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(conventional_game["cheat"], "Cheats/GBA/mgba/宝可梦.cht");
    assert_eq!(conventional_game["has_cheat"], true);

    let response = put_json(
        &shared,
        "/api/rom/cheat",
        &query,
        serde_json::json!({"cheat": "Cheats/GBA/mgba/宝可梦.cht"}),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        response_json(&response)["cheat"],
        "Cheats/GBA/mgba/宝可梦.cht"
    );
    let saved = slot_store::cart_meta::read(
        root,
        slot_store::Platform::Gba,
        &root.join("Games/GBA/宝可梦.gba"),
        "宝可梦",
    )
    .unwrap();
    assert_eq!(saved.cheat.as_deref(), Some("Cheats/GBA/mgba/宝可梦.cht"));

    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["cheat"], "Cheats/GBA/mgba/宝可梦.cht");
    assert_eq!(game["has_cheat"], true);

    let bad = put_json(
        &shared,
        "/api/rom/cheat",
        &query,
        serde_json::json!({"cheat": "Cheats/GBA/mgba/宝可梦.txt"}),
    );
    assert_eq!(bad.status, 400);
    let missing = put_json(
        &shared,
        "/api/rom/cheat",
        &query,
        serde_json::json!({"cheat": "Cheats/GBA/mgba/nope.cht"}),
    );
    assert_eq!(missing.status, 404);

    let cleared = put_json(
        &shared,
        "/api/rom/cheat",
        &query,
        serde_json::json!({"cheat": ""}),
    );
    assert_eq!(cleared.status, 200, "{}", body_text(&cleared));
    assert!(response_json(&cleared)["cheat"].is_null());
}

#[test]
fn conventional_guides_and_cheats_are_found_regardless_of_case() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    write_file(root, "Cheats/GBA/gpsp/宝可梦.CHT", b"cheat0_code=00");
    write_file(root, "Guides/GBA/攻略/宝可梦.TXT", b"guide text");
    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["cheat"], "Cheats/GBA/gpsp/宝可梦.CHT");
    assert_eq!(game["has_cheat"], true);
    assert_eq!(game["guide"], "Guides/GBA/攻略/宝可梦.TXT");
    assert_eq!(game["has_guide"], true);
}

#[test]
fn an_epub_guide_uploads_with_its_own_extension() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let query = format!("{}&ext=epub", rom_query("Games/GBA/宝可梦.gba"));
    let response = put(
        &shared,
        "/api/rom/guide",
        &query,
        b"PK\x03\x04epub",
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        response_json(&response)["guide"],
        "Guides/GBA/宝可梦.epub"
    );
    assert!(root.join("Guides/GBA/宝可梦.epub").is_file());
    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["guide"], "Guides/GBA/宝可梦.epub");
    assert_eq!(game["has_guide"], true);
}

#[test]
fn a_txt_guide_uploads_to_the_conventional_path_and_is_searchable() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let query = rom_query("Games/GBA/宝可梦.gba");

    let response = put(
        &shared,
        "/api/rom/guide",
        &query,
        "攻略正文".as_bytes(),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        std::fs::read_to_string(root.join("Guides/GBA/宝可梦.txt")).unwrap(),
        "攻略正文"
    );
    assert_eq!(
        response_json(&response)["guide"],
        "Guides/GBA/宝可梦.txt"
    );

    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["guide"], "Guides/GBA/宝可梦.txt");
    assert_eq!(game["has_guide"], true);

    let custom = root.join("Guides/GBA/custom.txt");
    std::fs::write(&custom, "自定义".as_bytes()).unwrap();
    let mut guide_request = request(Method::Post, "/api/rom/guide", &query);
    guide_request.body = serde_json::to_vec(&serde_json::json!({
        "guide": "Guides/GBA/custom.txt"
    }))
    .unwrap();
    guide_request
        .headers
        .insert("content-type".to_string(), "application/json".to_string());
    let response = route(&shared, &with_slot_header(guide_request));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let saved = slot_store::cart_meta::read(
        root,
        slot_store::Platform::Gba,
        &root.join("Games/GBA/宝可梦.gba"),
        "宝可梦",
    )
    .unwrap();
    assert_eq!(saved.guide.as_deref(), Some("Guides/GBA/custom.txt"));
}

#[test]
fn uploading_intro_and_video_uses_the_rom_stem() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let query = rom_query("Games/GBA/宝可梦.gba");

    let response = put(&shared, "/api/rom/intro", &query, "一段介绍".as_bytes(), true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Info/GBA/宝可梦.txt")).unwrap(),
        "一段介绍"
    );

    let mut mp4 = b"\0\0\0\x18ftypisom".to_vec();
    mp4.extend_from_slice(&[0u8; 16]);
    let response = put(&shared, "/api/rom/video", &query, &mp4, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        std::fs::read(dir.path().join("Info/GBA/宝可梦.mp4")).unwrap(),
        mp4
    );

    let value = body_json(&get(&shared, "/api/roms", ""));
    let game = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GBA/宝可梦.gba")
        .unwrap();
    assert_eq!(game["has_intro"], true);
    assert_eq!(game["has_video"], true);

    let response = put(&shared, "/api/rom/video", &query, b"not an mp4", true);
    assert_eq!(response.status, 400);
    let response = put(&shared, "/api/rom/intro", &query, &[0xff, 0xfe], true);
    assert_eq!(response.status, 400);

    let meta = slot_store::cart_meta::read(
        dir.path(),
        slot_store::Platform::Gba,
        &dir.path().join("Games/GBA/宝可梦.gba"),
        "宝可梦",
    )
    .unwrap();
    assert_eq!(meta.intro.as_deref(), Some("Info/GBA/宝可梦.txt"));
    assert_eq!(meta.video.as_deref(), Some("Info/GBA/宝可梦.mp4"));
}

#[test]
fn a_rom_cover_reads_as_png_and_missing_covers_are_404() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let response = get(
        &shared,
        "/api/rom/cover",
        &format!("{}&t=1", rom_query("Games/GBA/宝可梦.gba")),
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "image/png");
    assert_eq!(
        response.body,
        std::fs::read(dir.path().join("Labels/GBA/宝可梦.png")).unwrap()
    );
    let response = get(&shared, "/api/rom/cover", &rom_query("Games/GB/Tetris.gb"));
    assert_eq!(response.status, 404);
}

#[test]
fn replacing_a_cover_writes_a_normalized_png_and_drops_the_cache() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let cache = root.join("System/Cache/Labels/GBA/宝可梦.rgba");
    write_file(root, "System/Cache/Labels/GBA/宝可梦.rgba", b"old-cache");

    let query = rom_query("Games/GBA/宝可梦.gba");
    let response = put(
        &shared,
        "/api/rom/cover",
        &query,
        &png_bytes(8, 8, [10, 20, 30, 255]),
        false,
    );
    assert_eq!(response.status, 403);
    let response = put(
        &shared,
        "/api/rom/cover",
        &query,
        &png_bytes(8, 8, [10, 20, 30, 255]),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert!(
        !cache.exists(),
        "the old label cache survived a replacement"
    );
    let stored = std::fs::read(root.join("Labels/GBA/宝可梦.png")).unwrap();
    assert_eq!(png_dimensions(&stored), (492, 258));
    assert!(shared.library_changed.load(Ordering::Acquire));

    let before = stored;
    let response = put(&shared, "/api/rom/cover", &query, b"not a png", true);
    assert_eq!(response.status, 400);
    assert_eq!(
        std::fs::read(root.join("Labels/GBA/宝可梦.png")).unwrap(),
        before
    );
}

#[test]
fn adding_a_cover_to_a_game_without_one_creates_the_label_file() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let query = rom_query("Games/GB/Tetris.gb");
    let response = put(
        &shared,
        "/api/rom/cover",
        &query,
        &png_bytes(6, 6, [40, 50, 60, 255]),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let label = root.join("Labels/GB/Tetris.png");
    assert!(label.is_file(), "the first cover was not created");
    let stored = std::fs::read(label).unwrap();
    assert_eq!(png_dimensions(&stored), (372, 332));
    let value = body_json(&get(&shared, "/api/roms", ""));
    let tetris = value["games"]
        .as_array()
        .unwrap()
        .iter()
        .find(|game| game["rom"] == "Games/GB/Tetris.gb")
        .unwrap();
    assert_eq!(tetris["has_cover"], true);
}

#[test]
fn replacing_a_rom_keeps_the_same_path_and_rejects_bad_requests() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let rom = root.join("Games/GBA/宝可梦.gba");
    let query = rom_query("Games/GBA/宝可梦.gba");

    let response = put(&shared, "/api/rom", &query, b"new-rom", false);
    assert_eq!(response.status, 403);
    let response = put(&shared, "/api/rom", &query, b"", true);
    assert_eq!(response.status, 400);
    let response = put(
        &shared,
        "/api/rom",
        &rom_query("Games/GBA/does-not-exist.gba"),
        b"new-rom",
        true,
    );
    assert_eq!(response.status, 404);
    let response = put(
        &shared,
        "/api/rom",
        &rom_query("Games/../Games/GBA/宝可梦.gba"),
        b"new-rom",
        true,
    );
    assert_eq!(response.status, 404);

    let response = put(&shared, "/api/rom", &query, b"new-rom", true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(std::fs::read(&rom).unwrap(), b"new-rom");
    assert!(rom.exists(), "the ROM path changed");
    assert!(shared.library_changed.load(Ordering::Acquire));
    let meta = slot_store::cart_meta::read(
        root,
        slot_store::Platform::Gba,
        &rom,
        "宝可梦",
    )
    .expect("replacement metadata");
    assert_eq!(
        meta.sha256.as_deref(),
        Some(crate::rom_id::sha256_hex(b"new-rom").as_str())
    );
}

#[test]
fn replacing_a_rom_with_another_cards_content_is_refused() {
    let dir = rom_fixture();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let source = root.join("Games/GBA/宝可梦.gba");
    let other = root.join("Games/GBA/Other.gba");
    std::fs::write(&other, b"other-rom").unwrap();
    let carts = slot_store::scan_quiet(root).unwrap();
    let cart = carts
        .iter()
        .find(|cart| cart.rom == other)
        .expect("other cart");
    slot_store::cart_meta::ensure(root, cart).unwrap();
    let digest = crate::rom_id::sha256_file(&other).unwrap();
    let metadata = std::fs::metadata(&other).unwrap();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap();
    slot_store::cart_meta::set_sha256(root, &other, &digest, metadata.len(), mtime).unwrap();

    let response = put(
        &shared,
        "/api/rom",
        &rom_query("Games/GBA/宝可梦.gba"),
        b"other-rom",
        true,
    );
    assert_eq!(response.status, 409, "{}", body_text(&response));
    assert_eq!(std::fs::read(&source).unwrap(), b"old-gba-rom");
}

#[test]
fn rom_writes_are_refused_while_the_card_is_read_only() {
    let dir = rom_fixture();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    shared.writable.store(false, Ordering::Relaxed);
    let response = put(
        &shared,
        "/api/rom/cover",
        &rom_query("Games/GBA/宝可梦.gba"),
        &png_bytes(2, 2, [1, 2, 3, 255]),
        true,
    );
    assert_eq!(response.status, 409);
    let response = put(
        &shared,
        "/api/rom",
        &rom_query("Games/GBA/宝可梦.gba"),
        b"new-rom",
        true,
    );
    assert_eq!(response.status, 409);
}

#[cfg(unix)]
#[test]
fn a_cover_cannot_escape_the_card_through_a_labels_symlink() {
    let dir = rom_fixture();
    let root = dir.path();
    let outside = temp_root();
    std::fs::remove_dir_all(root.join("Labels")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("Labels")).unwrap();
    let shared = test_support::test_shared(root.to_path_buf());
    let response = put(
        &shared,
        "/api/rom/cover",
        &rom_query("Games/GBA/宝可梦.gba"),
        &png_bytes(4, 4, [1, 2, 3, 255]),
        true,
    );
    assert_eq!(response.status, 403);
    assert!(
        !outside.path().join("GBA/宝可梦.png").exists(),
        "a cover escaped through the Labels symlink"
    );
}

// ------------------------------------------------------- Lua 脚本（mods v2）

fn mod_payload(name: &str, enabled: bool) -> Value {
    serde_json::json!({ "name": name, "version": "1.2.3", "enabled": enabled })
}

fn action_payload(name: &str, timing: &str, content: &str) -> Value {
    serde_json::json!({
        "name": name,
        "timing": timing,
        "enabled": true,
        "order": 100,
        "boot_delay_minutes": 0,
        "content": content,
    })
}

fn create_mod(shared: &Arc<Shared>, name: &str, enabled: bool) -> Response {
    post(shared, "/api/mods/item", mod_payload(name, enabled), true)
}

#[test]
fn a_mod_import_can_create_and_upgrade_a_stable_id() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let payload = serde_json::json!({
        "id": "gbastation-battery-sync",
        "allow_create": true,
        "name": "GBAStation",
        "version": "1.0.0",
        "author": "linkSlot",
        "enabled": false,
    });
    let response = post(&shared, "/api/mods/item", payload.clone(), true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["id"], "gbastation-battery-sync");

    let state = root.join("linkSlot/mods/gbastation-battery-sync/state/playtime/GBA/key.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, br#"{"merged":42}"#).unwrap();
    let response = post(&shared, "/api/mods/item", payload, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(std::fs::read(&state).unwrap(), br#"{"merged":42}"#);
}

#[test]
fn mods_expose_author_with_default() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());

    let response = post(
        &shared,
        "/api/mods/item",
        serde_json::json!({
            "name": "有作者",
            "version": "1.0.0",
            "author": "作者 A",
            "enabled": false,
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let created = body_json(&response);
    assert_eq!(created["author"], "作者 A");
    let id = created["id"].as_str().unwrap().to_string();
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(format!("linkSlot/mods/{id}/mod.json"))).unwrap())
            .unwrap();
    assert_eq!(meta["author"], "作者 A");

    let response = create_mod(&shared, "无作者", false);
    assert_eq!(response.status, 200);
    assert_eq!(body_json(&response)["author"], "无作者信息");
    let list = body_json(&get(&shared, "/api/mods", ""));
    let mods = list["mods"].as_array().unwrap();
    assert!(mods.iter().any(|entry| entry["author"] == "作者 A"));
    assert!(mods.iter().any(|entry| entry["author"] == "无作者信息"));
}

#[test]
fn mods_list_exposes_only_public_profile_fields() {
    let dir = temp_root();
    let root = dir.path();
    // A real sidecar with credentials: the script page must never echo them.
    write_file(
        root,
        "linkSlot/Config/storages.toml",
        br#"active = "home"

[[storage]]
id = "home"
name = "nas"
kind = "webdav"
url = "https://nas.example/dav"
remote_root = "/linkSlot"
username = "saver"
password = "storage-secret"
ca_cert = ""
insecure = false
"#,
    );
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/mods", "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    let text = body_text(&response);
    assert!(!text.contains("storage-secret"), "password leaked: {text}");

    let storages = value["storages"].as_array().expect("storages");
    assert_eq!(storages.len(), 1);
    let storage = &storages[0];
    assert_eq!(storage["id"], "home");
    assert_eq!(storage["name"], "nas");
    assert_eq!(storage["kind"], "webdav");
    assert_eq!(storage["url"], "https://nas.example/dav");
    assert_eq!(storage["remote_root"], "/linkSlot");
    assert_eq!(storage["active"], true);
    assert_eq!(storage["password_set"], true);
    assert!(storage.get("username").is_none(), "{storage}");
    assert!(storage.get("password").is_none(), "{storage}");
    assert!(storage.get("ca_cert").is_none(), "{storage}");

    assert_eq!(value["mods"].as_array().unwrap().len(), 0);
    let timings = value["timings"].as_array().unwrap();
    assert_eq!(timings.len(), 7);
    assert!(timings
        .iter()
        .any(|timing| timing["value"].as_str() == Some("after_sync")));
    assert!(timings
        .iter()
        .any(|timing| timing["value"].as_str() == Some("after_boot")));
    assert!(timings
        .iter()
        .any(|timing| timing["value"].as_str() == Some("manual")));
    assert_eq!(value["runtime_supported"], false);
    assert!(value["runtime_hint"].is_string());
    assert!(value.get("compat_roots").is_none());
    assert!(value.get("compat_error").is_none());
    assert_eq!(value["max_script_bytes"], 262144);
    assert_eq!(value["max_mods"], 64);
}

#[test]
fn mods_mod_action_crud_toggle_delete_and_uninstall() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());

    // Mod create is guarded like every other write endpoint.
    let response = post(
        &shared,
        "/api/mods/item",
        mod_payload("演示 mod", true),
        false,
    );
    assert_eq!(response.status, 403);
    assert!(body_text(&response).contains("X-Slot-Web"));
    shared.writable.store(false, Ordering::Relaxed);
    let response = post(
        &shared,
        "/api/mods/item",
        mod_payload("演示 mod", true),
        true,
    );
    assert_eq!(response.status, 409);
    shared.writable.store(true, Ordering::Relaxed);

    let response = create_mod(&shared, "演示 mod", true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let created = body_json(&response);
    let mod_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["name"], "演示 mod");
    assert_eq!(created["version"], "1.2.3");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["action_count"], 0);
    assert_eq!(created["actions"].as_array().unwrap().len(), 0);
    assert_eq!(created["revision"].as_str().unwrap().len(), 64);
    assert!(created.get("timing").is_none(), "{created}");

    let response = get(&shared, "/api/mods/item", &format!("id={mod_id}"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let detail = body_json(&response);
    assert_eq!(detail["name"], "演示 mod");
    assert_eq!(detail["actions"].as_array().unwrap().len(), 0);
    assert_eq!(detail["meta_present"], true);

    // Stale mod revision -> 409 and no write.
    let mut stale = mod_payload("改名", false);
    stale["id"] = serde_json::json!(mod_id);
    stale["revision"] = serde_json::json!("deadbeef");
    let response = post(&shared, "/api/mods/item", stale, true);
    assert_eq!(response.status, 409, "{}", body_text(&response));

    // Matching revision -> 200.
    let mut update = mod_payload("演示 mod v2", false);
    update["id"] = serde_json::json!(mod_id);
    update["revision"] = created["revision"].clone();
    let response = post(&shared, "/api/mods/item", update, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let updated = body_json(&response);
    assert_eq!(updated["name"], "演示 mod v2");
    assert_eq!(updated["version"], "1.2.3");
    assert_eq!(updated["enabled"], false);
    let mod_revision = updated["revision"].clone();

    // Action create.
    let payload = action_payload("下载后动作", "after_download", "log.info('hi')");
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": mod_id,
            "name": payload["name"],
            "timing": payload["timing"],
            "enabled": payload["enabled"],
            "order": payload["order"],
            "content": payload["content"],
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let action = body_json(&response);
    let action_id = action["id"].as_str().expect("action id").to_string();
    assert_eq!(action["name"], "下载后动作");
    assert_eq!(action["timing"], "after_download");
    assert_eq!(action["timing_label"], "下载后");
    assert_eq!(action["content"], "log.info('hi')");
    assert_eq!(action["revision"].as_str().unwrap().len(), 64);

    let response = get(
        &shared,
        "/api/mods/action",
        &format!("id={mod_id}&action={action_id}"),
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let action_detail = body_json(&response);
    assert_eq!(action_detail["content"], "log.info('hi')");
    assert_eq!(action_detail["meta_present"], true);
    assert_eq!(action_detail["script_present"], true);

    // Stale action revision and bad fields.
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": mod_id,
            "action_id": action_id,
            "name": "改",
            "timing": "after_sync",
            "enabled": true,
            "order": 100,
            "content": "return",
            "revision": "deadbeef",
        }),
        true,
    );
    assert_eq!(response.status, 409, "{}", body_text(&response));
    for bad in [
        serde_json::json!({
            "id": mod_id, "action_id": action_id, "name": "x",
            "timing": "sometimes", "enabled": true, "order": 100, "content": "return",
        }),
        serde_json::json!({
            "id": mod_id, "action_id": action_id, "name": "x",
            "timing": "after_sync", "enabled": true, "order": 10000, "content": "return",
        }),
        serde_json::json!({
            "id": mod_id, "action_id": action_id, "name": "x".repeat(65),
            "timing": "after_sync", "enabled": true, "order": 100, "content": "return",
        }),
        serde_json::json!({
            "id": mod_id, "action_id": action_id, "name": "x",
            "timing": "after_sync", "enabled": true, "order": 100,
            "content": "x".repeat(slot_mods::MAX_SCRIPT_BYTES + 1),
        }),
    ] {
        let response = post(&shared, "/api/mods/action", bad, true);
        assert_eq!(response.status, 400, "{}", body_text(&response));
    }

    // Matching revision -> action update.
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": mod_id,
            "action_id": action_id,
            "name": "同步后动作",
            "timing": "after_sync",
            "enabled": false,
            "order": -5,
            "content": "log.info('v2')",
            "revision": action["revision"],
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let updated_action = body_json(&response);
    assert_eq!(updated_action["name"], "同步后动作");
    assert_eq!(updated_action["timing"], "after_sync");
    assert_eq!(updated_action["enabled"], false);
    assert_eq!(updated_action["order"], -5);
    assert_eq!(updated_action["content"], "log.info('v2')");

    // Action and mod toggles are independent.
    let response = post(
        &shared,
        "/api/mods/action/toggle",
        serde_json::json!({ "id": mod_id, "action": action_id, "enabled": true }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["enabled"], true);
    let response = post(
        &shared,
        "/api/mods/toggle",
        serde_json::json!({ "id": mod_id, "enabled": true }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["enabled"], true);

    // Invalid/empty routes.
    assert_eq!(get(&shared, "/api/mods/item", "").status, 400);
    assert_eq!(get(&shared, "/api/mods/item", "id=missing").status, 404);
    assert_eq!(get(&shared, "/api/mods/item", "id=..%2Fescape").status, 400);
    let response = post(
        &shared,
        "/api/mods/item",
        serde_json::json!({
            "id": mod_id,
            "name": "坏版本",
            "version": "../escape",
            "enabled": true,
        }),
        true,
    );
    assert_eq!(response.status, 400, "{}", body_text(&response));
    assert!(body_text(&response).contains("版本"));
    assert_eq!(get(&shared, "/api/mods/toggle", "").status, 405);
    assert_eq!(get(&shared, "/api/mods/action", "").status, 400);
    assert_eq!(
        get(
            &shared,
            "/api/mods/action",
            &format!("id={mod_id}&action=missing"),
        )
        .status,
        404
    );
    assert_eq!(get(&shared, "/api/mods/uninstall", "").status, 405);
    assert_eq!(get(&shared, "/api/mods/nope", "").status, 404);

    // Action delete goes to the mod's trash.
    let response = post(
        &shared,
        "/api/mods/action/delete",
        serde_json::json!({ "id": mod_id, "action": action_id }),
        false,
    );
    assert_eq!(response.status, 403);
    let response = post(
        &shared,
        "/api/mods/action/delete",
        serde_json::json!({ "id": mod_id, "action": action_id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let deleted = body_json(&response);
    assert_eq!(deleted["ok"], true);
    assert!(
        deleted["trash"]
            .as_str()
            .unwrap()
            .starts_with(&format!("linkSlot/mods/{mod_id}/.trash/actions/")),
        "{deleted}"
    );
    assert_eq!(
        get(
            &shared,
            "/api/mods/action",
            &format!("id={mod_id}&action={action_id}"),
        )
        .status,
        404
    );

    // Recreate an action and uninstall the whole mod permanently.
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": mod_id,
            "name": "再次创建",
            "timing": "after_sync",
            "enabled": true,
            "order": 100,
            "content": "return",
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let response = post(
        &shared,
        "/api/mods/uninstall",
        serde_json::json!({ "id": mod_id }),
        false,
    );
    assert_eq!(response.status, 403);
    let response = post(
        &shared,
        "/api/mods/uninstall",
        serde_json::json!({ "id": mod_id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let removed = body_json(&response);
    assert_eq!(removed["ok"], true);
    assert_eq!(
        get(&shared, "/api/mods/item", &format!("id={mod_id}")).status,
        404
    );
    assert!(!root.join("linkSlot/mods").join(&mod_id).exists());
    assert!(!root.join("linkSlot/mods/.trash").exists());
    assert_eq!(mod_revision.as_str().unwrap().len(), 64);
}

#[test]
fn tianma_status_advances_after_start() {
    let dir = temp_root();
    let root = dir.path();
    std::fs::create_dir_all(root.join("Imports/TianmaG")).unwrap();
    std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
    let shared = test_support::test_shared(root.to_path_buf());
    let response = post(
        &shared,
        "/api/tianma/start",
        serde_json::json!({ "path": "Imports/TianmaG" }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let mut finished = false;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(20));
        let value = body_json(&get(&shared, "/api/tianma/status", ""));
        if value["finished"] == true {
            finished = true;
            break;
        }
    }
    assert!(finished, "the importer status never left running: {:?}", body_json(&get(&shared, "/api/tianma/status", "")));
}

#[test]
fn a_mod_sticker_can_be_uploaded_as_a_raw_file() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    std::fs::create_dir_all(root.join("linkSlot/mods/demo")).unwrap();
    let png = png_bytes(4, 4, [10, 20, 30, 255]);
    let path = util::encode_query("linkSlot/mods/demo/sticker.png");
    let mut request = request(Method::Post, "/api/files/upload", &format!("path={path}"));
    request.body = png.clone();
    let response = route(&shared, &with_slot_header(request));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        std::fs::read(root.join("linkSlot/mods/demo/sticker.png")).unwrap(),
        png
    );
}

#[test]
fn a_mod_sticker_can_be_written_through_the_raw_mod_endpoint() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let created = body_json(&create_mod(&shared, "贴纸测试", true));
    let id = created["id"].as_str().unwrap().to_string();
    let png = png_bytes(4, 4, [11, 22, 33, 255]);
    let mut request = request(
        Method::Post,
        "/api/mods/file",
        &format!("id={id}&path=sticker.png"),
    );
    request.body = png.clone();
    request
        .headers
        .insert("content-type".to_string(), "image/png".to_string());
    let response = route(&shared, &with_slot_header(request));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(std::fs::read(root.join("linkSlot/mods").join(&id).join("sticker.png")).unwrap(), png);
}

#[test]
fn mods_file_api_is_scoped_and_serves_mod_ui() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let created = body_json(&create_mod(&shared, "文件测试", true));
    let id = created["id"].as_str().unwrap().to_string();

    let response = get(&shared, "/api/mods/files", &format!("id={id}"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["has_ui"], false);
    assert_eq!(value["files"].as_array().unwrap().len(), 1);
    assert_eq!(value["files"][0]["path"], "mod.json");
    assert_eq!(value["files"][0]["editable"], false);

    // Every write endpoint uses the same guard.
    let file_body = serde_json::json!({
        "id": id,
        "path": "web/index.html",
        "content": "<!doctype html><title>ui</title>",
    });
    let response = post(&shared, "/api/mods/file", file_body.clone(), false);
    assert_eq!(response.status, 403);
    shared.writable.store(false, Ordering::Relaxed);
    let response = post(&shared, "/api/mods/file", file_body.clone(), true);
    assert_eq!(response.status, 409);
    shared.writable.store(true, Ordering::Relaxed);

    let response = post(&shared, "/api/mods/file", file_body, true);
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let saved = body_json(&response);
    let revision = saved["revision"].as_str().unwrap().to_string();
    assert_eq!(saved["path"], "web/index.html");

    let response = get(&shared, "/api/mods/files", &format!("id={id}"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["has_ui"], true);
    assert!(value["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "web/index.html" && file["editable"] == true));

    let path = crate::web::util::encode_query("web/index.html");
    let response = get(&shared, "/api/mods/file", &format!("id={id}&path={path}"));
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(
        body_json(&response)["content"],
        "<!doctype html><title>ui</title>"
    );

    // Stale revision and invalid JSON never overwrite.
    let stale = serde_json::json!({
        "id": id,
        "path": "web/index.html",
        "content": "new",
        "revision": "deadbeef",
    });
    assert_eq!(post(&shared, "/api/mods/file", stale, true).status, 409);
    let invalid_json = serde_json::json!({
        "id": id,
        "path": "config.json",
        "content": "{",
    });
    assert_eq!(
        post(&shared, "/api/mods/file", invalid_json, true).status,
        400
    );

    // mod.json and paths outside this mod are not writable through the API.
    let response = post(
        &shared,
        "/api/mods/file",
        serde_json::json!({
            "id": id,
            "path": "mod.json",
            "content": "{}",
        }),
        true,
    );
    assert_eq!(response.status, 400);
    let traversal = crate::web::util::encode_query("../mod.json");
    assert_eq!(
        get(
            &shared,
            "/api/mods/file",
            &format!("id={id}&path={traversal}")
        )
        .status,
        400
    );

    // Mod UI assets are outside /api, sandboxed by the parent, and cannot
    // escape the current mod's web/ directory.
    let response = get(&shared, &format!("/mod-ui/{id}/web/index.html"), "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(response.content_type, "text/html; charset=utf-8");
    assert!(response
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("Content-Security-Policy")));
    let response = get(&shared, "/mod-ui/sdk.js", "");
    assert_eq!(response.status, 200);
    assert!(body_text(&response).contains("LinkSlotMod"));
    assert_eq!(
        get(&shared, &format!("/mod-ui/{id}/web/../mod.json"), "").status,
        400
    );
    assert_eq!(get(&shared, "/mod-ui/nope/web/index.html", "").status, 404);

    assert_eq!(get(&shared, "/api/mods/file", "").status, 400);
    assert_eq!(
        get(
            &shared,
            "/api/mods/file",
            &format!("id={id}&path=missing.md")
        )
        .status,
        404
    );
    let response = post(
        &shared,
        "/api/mods/check",
        serde_json::json!({ "id": id }),
        true,
    );
    assert_eq!(response.status, 200);
    assert!(revision.len() == 64);
}

#[test]
fn mods_write_endpoints_are_guarded() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    for path in [
        "/api/mods/item",
        "/api/mods/file",
        "/api/mods/toggle",
        "/api/mods/delete",
        "/api/mods/uninstall",
        "/api/mods/action",
        "/api/mods/action/toggle",
        "/api/mods/action/delete",
        "/api/mods/test",
        "/api/mods/check",
    ] {
        let response = post(&shared, path, serde_json::json!({}), false);
        assert_eq!(response.status, 403, "{path}");
        assert!(body_text(&response).contains("X-Slot-Web"), "{path}");
    }
    shared.writable.store(false, Ordering::Relaxed);
    for path in [
        "/api/mods/item",
        "/api/mods/file",
        "/api/mods/toggle",
        "/api/mods/delete",
        "/api/mods/uninstall",
        "/api/mods/action",
        "/api/mods/action/toggle",
        "/api/mods/action/delete",
        "/api/mods/test",
        "/api/mods/check",
    ] {
        let response = post(&shared, path, serde_json::json!({}), true);
        assert_eq!(response.status, 409, "{path}");
    }
    shared.writable.store(true, Ordering::Relaxed);
    assert_eq!(get(&shared, "/api/mods/delete", "").status, 405);
    assert_eq!(get(&shared, "/api/mods/nope", "").status, 404);
}

#[test]
fn old_single_script_mod_is_rejected() {
    let dir = temp_root();
    let root = dir.path();
    write_file(
        root,
        "linkSlot/mods/legacy/mod.json",
        r#"{"schema":1,"name":"旧 mod","timing":"after_download","enabled":true,"order":100}"#
            .as_bytes(),
    );
    write_file(root, "linkSlot/mods/legacy/main.lua", b"return");
    let shared = test_support::test_shared(root.to_path_buf());
    let response = get(&shared, "/api/mods", "");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    let mods = value["mods"].as_array().unwrap();
    assert_eq!(mods.len(), 1);
    assert!(mods[0]["error"].as_str().unwrap().contains("旧版"));
    assert_eq!(mods[0]["action_count"], 0);

    let response = get(&shared, "/api/mods/item", "id=legacy");
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let detail = body_json(&response);
    assert_eq!(detail["actions"].as_array().unwrap().len(), 0);
    assert!(detail["error"].as_str().unwrap().contains("旧版"));

    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": "legacy",
            "name": "x",
            "timing": "after_sync",
            "enabled": true,
            "order": 100,
            "content": "return",
        }),
        true,
    );
    assert_eq!(response.status, 400, "{}", body_text(&response));
}

#[cfg(unix)]
#[test]
fn mods_runtime_flag_is_read_from_the_version_and_cached() {
    let dir = temp_root();
    let root = dir.path();
    write_file(
        root,
        "System/linkSlot/bin/linkslot_sync",
        b"#!/bin/sh\nprintf 'linkSlot 0.0.2 mods=1\\n'\n",
    );
    let shared = test_support::test_shared(root.to_path_buf());
    let value = body_json(&get(&shared, "/api/mods", ""));
    assert_eq!(value["runtime_supported"], true);
    assert!(value["runtime_hint"].is_null());
    // Cached: deleting the CLI leaves the flag stable for this session.
    std::fs::remove_file(root.join("System/linkSlot/bin/linkslot_sync")).unwrap();
    let value = body_json(&get(&shared, "/api/mods", ""));
    assert_eq!(value["runtime_supported"], true);
}

#[cfg(unix)]
fn install_fake_mods_cli(root: &Path, script: &str) {
    write_file(root, "System/linkSlot/shell/linkslot.sh", script.as_bytes());
}

#[cfg(unix)]
#[test]
fn mods_check_and_test_call_the_cli_with_slot_root() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let created = body_json(&create_mod(&shared, "检查 mod", true));
    let id = created["id"].as_str().expect("id").to_string();
    let action = body_json(&post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": id,
            "name": "检查动作",
            "timing": "before_upload",
            "enabled": true,
            "order": 100,
            "content": "return 1",
        }),
        true,
    ));
    let action_id = action["id"].as_str().expect("action id").to_string();
    install_fake_mods_cli(
        root,
        r#"#!/bin/sh
dir=$(dirname "$0")
{
  printf '%s\n' "$@"
  printf 'SLOT_ROOT=%s\n' "$SLOT_ROOT"
} > "$dir/args.log"
printf 'LINKSLOT_ACTION=mods\nLINKSLOT_RESULT=ok\nLINKSLOT_DETAIL=命令成功\n'
exit 0
"#,
    );

    let response = post(
        &shared,
        "/api/mods/check",
        serde_json::json!({ "id": id, "action": action_id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["code"], 0);
    assert!(value["detail"].as_str().unwrap().contains("命令成功"));
    let args = std::fs::read_to_string(root.join("System/args.log")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(
        &lines[0..6],
        &[
            "mods",
            "check",
            "--id",
            id.as_str(),
            "--action",
            action_id.as_str()
        ]
    );
    assert_eq!(lines[6], format!("SLOT_ROOT={}", root.display()));

    // Without --action the CLI checks every action of the mod.
    let response = post(
        &shared,
        "/api/mods/check",
        serde_json::json!({ "id": id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let args = std::fs::read_to_string(root.join("System/args.log")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(&lines[0..4], &["mods", "check", "--id", id.as_str()]);
    assert_eq!(lines[4], format!("SLOT_ROOT={}", root.display()));

    let response = post(
        &shared,
        "/api/mods/test",
        serde_json::json!({ "profile": "default" }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], true);
    assert_eq!(value["code"], 0);
    let args = std::fs::read_to_string(root.join("System/args.log")).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(&lines[0..4], &["mods", "test", "--profile", "default"]);
    assert_eq!(lines[4], format!("SLOT_ROOT={}", root.display()));

    // An old CLI that has no `mods` subcommand is a readable 200 result, never
    // a Web 500, and the hint points at the upgrade.
    install_fake_mods_cli(
        root,
        "#!/bin/sh\nprintf 'linkSlot: 未知命令 mods\\n' >&2\nexit 2\n",
    );
    let response = post(
        &shared,
        "/api/mods/check",
        serde_json::json!({ "id": id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], 2);
    assert!(
        value["hint"].as_str().unwrap().contains("不支持"),
        "{value}"
    );
}

#[cfg(not(unix))]
#[test]
fn mods_cli_spawn_failure_is_reported_without_500() {
    let dir = temp_root();
    let root = dir.path();
    let shared = test_support::test_shared(root.to_path_buf());
    let created = body_json(&create_mod(&shared, "检查 mod", true));
    let id = created["id"].as_str().expect("id").to_string();
    let response = post(
        &shared,
        "/api/mods/check",
        serde_json::json!({ "id": id }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let value = body_json(&response);
    assert_eq!(value["ok"], false);
    assert!(value["hint"].as_str().unwrap().contains("linkSlot CLI"));
}

#[test]
fn action_api_roundtrips_after_boot_delay_and_rejects_out_of_range() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());
    let created = body_json(&create_mod(&shared, "开机 mod", true));
    let id = created["id"].as_str().expect("id").to_string();
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": id,
            "name": "开机动作",
            "timing": "after_boot",
            "enabled": true,
            "order": 100,
            "boot_delay_minutes": 15,
            "content": "log.info('boot')",
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    let action = body_json(&response);
    assert_eq!(action["boot_delay_minutes"], 15);
    let action_id = action["id"].as_str().expect("action id").to_string();
    let detail = body_json(&get(
        &shared,
        "/api/mods/action",
        &format!("id={id}&action={action_id}"),
    ));
    assert_eq!(detail["boot_delay_minutes"], 15);

    // Older mod pages do not send the new field while editing an action; the
    // existing delay must be preserved, not reset to 0.
    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": id,
            "action_id": action_id,
            "name": "开机动作改名",
            "timing": "after_boot",
            "enabled": true,
            "order": 100,
            "content": "return",
            "revision": detail["revision"],
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", body_text(&response));
    assert_eq!(body_json(&response)["boot_delay_minutes"], 15);

    let response = post(
        &shared,
        "/api/mods/action",
        serde_json::json!({
            "id": id,
            "name": "开机动作",
            "timing": "after_boot",
            "enabled": true,
            "order": 100,
            "boot_delay_minutes": 1441,
            "content": "return",
        }),
        true,
    );
    assert_eq!(response.status, 400, "{}", body_text(&response));
}

#[test]
fn upgrade_upload_is_stage_only_and_requires_a_valid_zip() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let status = body_json(&get(&shared, "/api/upgrade/status", ""));
    assert_eq!(status["state"], "idle");

    let mut missing = request(Method::Post, "/api/upgrade", "");
    missing.body = b"not a zip".to_vec();
    assert_eq!(route(&shared, &missing).status, 403);

    let mut invalid = with_slot_header(request(Method::Post, "/api/upgrade", ""));
    invalid.body = b"not a zip".to_vec();
    let response = route(&shared, &invalid);
    assert_eq!(response.status, 400, "{}", body_text(&response));

    let mut apply = with_slot_header(request(Method::Post, "/api/upgrade/apply", ""));
    apply.body = b"{}".to_vec();
    assert_eq!(route(&shared, &apply).status, 404);
}

#[test]
fn gbastation_endpoints_validate_the_origin_and_guard_writes() {
    let dir = temp_root();
    let shared = test_support::test_shared(dir.path().to_path_buf());

    let status = body_json(&get(&shared, "/api/gbastation/status", ""));
    assert_eq!(status["running"], false);

    let missing = post_bytes(&shared, "/api/gbastation/games", "", b"{}", true);
    assert_eq!(missing.status, 400, "{}", body_text(&missing));

    let https = post_bytes(
        &shared,
        "/api/gbastation/games",
        "",
        br#"{"base_url":"https://192.168.1.23:8080"}"#,
        true,
    );
    assert_eq!(https.status, 502, "{}", body_text(&https));

    let unguarded = post_bytes(
        &shared,
        "/api/gbastation/start",
        "",
        br#"{"base_url":"https://192.168.1.23:8080","keys":["x"]}"#,
        false,
    );
    assert_eq!(unguarded.status, 403, "{}", body_text(&unguarded));

    let bad_origin = post_bytes(
        &shared,
        "/api/gbastation/start",
        "",
        br#"{"base_url":"https://192.168.1.23:8080","keys":["x"]}"#,
        true,
    );
    assert_eq!(bad_origin.status, 400, "{}", body_text(&bad_origin));
}
