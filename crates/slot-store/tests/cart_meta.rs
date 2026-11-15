use std::path::Path;

use slot_store::cart_meta::{self, CartMeta};
use slot_store::{Cart, Outline, Platform, ShellChoice, ShellFinish};
use tempfile::tempdir;

fn cart(root: &Path) -> Cart {
    let dir = root.join("Games/GBA/Sub");
    std::fs::create_dir_all(&dir).unwrap();
    let rom = dir.join("Game.gba");
    std::fs::write(&rom, b"rom").unwrap();
    std::fs::create_dir_all(root.join("Labels/GBA/Sub")).unwrap();
    std::fs::write(root.join("Labels/GBA/Sub/Game.png"), b"png").unwrap();
    std::fs::write(root.join("Labels/GBA/Sub/Game.txt"), b"intro").unwrap();
    Cart {
        platform: Platform::Gba,
        stem: "Game".into(),
        rom,
        label: Some(root.join("Labels/GBA/Sub/Game.png")),
        title: String::new(),
        code: String::new(),
        shell: Some(ShellChoice {
            outline: Outline::Rounded,
            colour: [0x12, 0x34, 0x56],
            finish: ShellFinish::Clear,
        }),
    model: Default::default(),
    front: None,
}
}

#[test]
fn metadata_is_database_backed_and_drives_the_scan() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);

    assert!(cart_meta::ensure(root, &cart).unwrap());
    assert!(root.join(cart_meta::DB_FILE).is_file());
    assert!(!root.join("linkSlot/Metadata/GBA/Sub/Game.json").exists());
    let meta: CartMeta = cart_meta::read(root, cart.platform, &cart.rom, &cart.stem).unwrap();
    assert_eq!(meta.rom, "Games/GBA/Sub/Game.gba");
    assert_eq!(meta.cover.as_deref(), Some("Labels/GBA/Sub/Game.png"));
    assert_eq!(meta.color.as_deref(), Some("#123456"));
    assert_eq!(meta.shell.as_ref().unwrap().outline, "rounded");
    assert_eq!(meta.shell.as_ref().unwrap().finish, "clear");

    cart_meta::update(root, &cart, |meta| {
        meta.color = Some("#abcdef".into());
    })
    .unwrap();
    let carts = slot_store::scan_quiet(root).unwrap();
    assert_eq!(carts.len(), 1);
    assert_eq!(carts[0].shell.unwrap().colour, [0xab, 0xcd, 0xef]);
}

#[test]
fn a_cheat_path_is_database_backed_with_a_conventional_fallback() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    let conventional = root.join("Cheats/GBA/mgba/Game.cht");
    std::fs::create_dir_all(conventional.parent().unwrap()).unwrap();
    std::fs::write(&conventional, b"cheat0_code=00").unwrap();
    assert_eq!(
        cart_meta::cheat_path(root, &cart, "mgba"),
        Some(conventional.clone())
    );

    let custom = root.join("Cheats/custom/Game.cht");
    std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
    std::fs::write(&custom, b"cheat0_code=11").unwrap();
    cart_meta::ensure(root, &cart).unwrap();
    cart_meta::update(root, &cart, |meta| {
        meta.cheat = Some("Cheats/custom/Game.cht".into());
    })
    .unwrap();
    assert_eq!(cart_meta::cheat_path(root, &cart, "mgba"), Some(custom.clone()));

    std::fs::remove_file(&custom).unwrap();
    assert_eq!(
        cart_meta::cheat_path(root, &cart, "mgba"),
        Some(conventional),
        "a missing custom path should not hide the conventional file"
    );
}

#[cfg(not(windows))]
#[test]
fn conventional_guide_and_cheat_extensions_are_case_insensitive() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    let guide = root.join("Guides/GBA/Game.TXT");
    let cheat = root.join("Cheats/GBA/mgba/Game.CHT");
    std::fs::create_dir_all(guide.parent().unwrap()).unwrap();
    std::fs::create_dir_all(cheat.parent().unwrap()).unwrap();
    std::fs::write(&guide, "攻略").unwrap();
    std::fs::write(&cheat, "cheat0_code=00").unwrap();
    assert_eq!(cart_meta::guide_path(root, &cart), Some(guide));
    assert_eq!(cart_meta::cheat_path(root, &cart, "mgba"), Some(cheat));
}

#[test]
fn an_epub_guide_is_a_conventional_fallback() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    let epub = root.join("Guides/GBA/Game.epub");
    std::fs::create_dir_all(epub.parent().unwrap()).unwrap();
    std::fs::write(&epub, b"epub").unwrap();
    assert_eq!(cart_meta::guide_path(root, &cart), Some(epub));
}

#[test]
fn a_guide_path_is_database_backed_with_a_conventional_fallback() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    let conventional = root.join("Guides/GBA/Game.txt");
    std::fs::create_dir_all(conventional.parent().unwrap()).unwrap();
    std::fs::write(&conventional, "攻略".as_bytes()).unwrap();
    assert_eq!(
        cart_meta::guide_path(root, &cart),
        Some(conventional.clone())
    );

    let custom = root.join("Guides/custom.txt");
    std::fs::write(&custom, "自定义攻略".as_bytes()).unwrap();
    cart_meta::ensure(root, &cart).unwrap();
    cart_meta::update(root, &cart, |meta| {
        meta.guide = Some("Guides/custom.txt".into());
    })
    .unwrap();
    assert_eq!(cart_meta::guide_path(root, &cart), Some(custom.clone()));
    assert_eq!(
        cart_meta::read(root, cart.platform, &cart.rom, &cart.stem)
            .unwrap()
            .guide
            .as_deref(),
        Some("Guides/custom.txt")
    );

    std::fs::remove_file(&custom).unwrap();
    assert_eq!(
        cart_meta::guide_path(root, &cart),
        Some(conventional),
        "a missing custom guide should fall back to Guides/<platform>/<stem>.txt"
    );
}

#[test]
fn a_version_one_database_gains_the_guide_column() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("linkSlot/Metadata")).unwrap();
    let conn = rusqlite::Connection::open(root.join(cart_meta::DB_FILE)).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE carts (
            rom TEXT PRIMARY KEY, platform TEXT NOT NULL, stem TEXT NOT NULL,
            title TEXT, code TEXT, hash_hex TEXT, size INTEGER, mtime INTEGER,
            cover TEXT, intro TEXT, video TEXT, cheat TEXT, color TEXT,
            shell_outline TEXT, shell_finish TEXT, core TEXT, extra_json TEXT,
            updated_at INTEGER NOT NULL DEFAULT 0
        );
        PRAGMA user_version = 1;
        "#,
    )
    .unwrap();
    drop(conn);

    let cart = cart(root);
    cart_meta::ensure(root, &cart).unwrap();
    cart_meta::update(root, &cart, |meta| {
        meta.guide = Some("Guides/GBA/Game.txt".into());
    })
    .unwrap();
    assert_eq!(
        cart_meta::read(root, cart.platform, &cart.rom, &cart.stem)
            .unwrap()
            .guide
            .as_deref(),
        Some("Guides/GBA/Game.txt")
    );
    let conn = rusqlite::Connection::open(root.join(cart_meta::DB_FILE)).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, cart_meta::SCHEMA);
    let has_sha: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('carts') WHERE name='sha256')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(has_sha);
}

#[test]
fn sha256_round_trips_and_finds_content() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    cart_meta::ensure(root, &cart).unwrap();
    let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    cart_meta::update(root, &cart, |meta| {
        meta.sha256 = Some(digest.into());
    })
    .unwrap();
    let found = cart_meta::read_by_sha(root, cart.platform, digest).unwrap();
    assert_eq!(found.rom, "Games/GBA/Sub/Game.gba");
    assert_eq!(
        cart_meta::read(root, cart.platform, &cart.rom, &cart.stem)
            .unwrap()
            .sha256
            .as_deref(),
        Some(digest)
    );

    let metadata = std::fs::metadata(&cart.rom).unwrap();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap();
    assert!(cart_meta::candidates_by_size(root, cart.platform, metadata.len()).is_empty());
    cart_meta::update(root, &cart, |meta| meta.sha256 = None).unwrap();
    assert_eq!(
        cart_meta::candidates_by_size(root, cart.platform, metadata.len()).len(),
        1
    );
    assert!(cart_meta::set_sha256(root, &cart.rom, digest, metadata.len(), mtime).unwrap());
    assert!(cart_meta::read_by_sha(root, cart.platform, digest).is_some());
}

#[test]
fn legacy_sidecars_can_be_migrated_explicitly() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    std::fs::create_dir_all(root.join("linkSlot/Metadata/GBA")).unwrap();
    let meta = CartMeta {
        schema: 1,
        platform: "GBA".into(),
        rom: "Games/GBA/Sub/Game.gba".into(),
        title: Some("Migrated".into()),
        code: Some("AXV".into()),
        sha256: None,
        size: Some(3),
        mtime: None,
        cover: Some("Labels/GBA/Sub/Game.png".into()),
        intro: None,
        video: None,
        cheat: Some("Cheats/GBA/mgba/Game.cht".into()),
        guide: Some("Guides/GBA/Game.txt".into()),
        color: Some("#123456".into()),
        shell: Some(cart_meta::ShellMeta {
            outline: "rounded".into(),
            finish: "clear".into(),
        }),
        core: None,
        extra: std::collections::HashMap::new(),
    };
    std::fs::write(
        root.join("linkSlot/Metadata/GBA/Game.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("linkSlot/Metadata/GBA/bad.json"),
        r#"{"schema":1,"platform":"GBA","rom":"../outside.gba"}"#,
    )
    .unwrap();

    let report = cart_meta::migrate_sidecars(root).unwrap();
    assert_eq!(report.imported, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(
        cart_meta::read(root, cart.platform, &cart.rom, &cart.stem)
            .unwrap()
            .cheat
            .as_deref(),
        Some("Cheats/GBA/mgba/Game.cht")
    );
}

#[test]
fn read_all_returns_the_database_rows_once() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let cart = cart(root);
    cart_meta::ensure(root, &cart).unwrap();
    let all = cart_meta::read_all(root);
    assert_eq!(all.len(), 1);
    assert_eq!(
        all["Games/GBA/Sub/Game.gba"].cover.as_deref(),
        Some("Labels/GBA/Sub/Game.png")
    );

    cart_meta::update(root, &cart, |meta| meta.color = Some("#010203".into())).unwrap();
    let all = cart_meta::read_all(root);
    assert_eq!(all["Games/GBA/Sub/Game.gba"].color.as_deref(), Some("#010203"));
}
