mod common;

use tempfile::tempdir;

#[test]
fn ensure_creates_every_folder_dirs_names_and_nothing_else() {
    let d = tempdir().unwrap();
    let out = d.path().join("dist");
    slot::root::ensure(&out);

    for name in slot::root::DIRS {
        assert!(out.join(name).is_dir(), "{name} was not created");
    }

    let mut got: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    got.sort();
    let mut want: Vec<String> = slot::root::DIRS
        .iter()
        .map(|s| s.split('/').next().unwrap().to_string())
        .collect();
    want.sort();
    want.dedup();
    assert_eq!(got, want);
}

#[test]
fn ensure_leaves_existing_content_alone() {
    let d = tempdir().unwrap();
    let out = d.path().join("dist");
    std::fs::create_dir_all(out.join("Games")).unwrap();
    std::fs::write(out.join("Games/Emerald.gba"), b"rom").unwrap();
    slot::root::ensure(&out);
    assert_eq!(
        std::fs::read(out.join("Games/Emerald.gba")).unwrap(),
        b"rom"
    );
}

#[test]
fn a_booted_app_root_has_the_same_folders() {
    let d = common::tmp_root_with_carts(&["Emerald"]);
    for name in slot::root::DIRS {
        assert!(d.path().join(name).is_dir(), "app root is missing {name}");
    }
}

/// `linkslot-v0.8.3` shipped this exact factory nameplate and v0.8.5 shipped
/// the unused `cart_sfc.png`. Upgrading a card leaves both in `System/linkSlot/assets/carts`.
#[test]
fn legacy_stock_carts_are_removed_but_user_plates_stay() {
    const STOCK_PLATE: &[u8] = include_bytes!("fixtures/slot_back_recents.png");
    const OLD_CART: &[u8] = include_bytes!("fixtures/cart_sfc.png");
    let d = tempdir().unwrap();
    let root = d.path().join("dist");
    let carts = root.join("System/linkSlot/assets/carts");
    std::fs::create_dir_all(&carts).unwrap();
    std::fs::write(carts.join("slot_back_recents.png"), STOCK_PLATE).unwrap();
    std::fs::write(carts.join("cart_sfc.png"), OLD_CART).unwrap();
    std::fs::write(carts.join("slot_back_gba.png"), b"my own plate").unwrap();
    std::fs::write(carts.join("slot_back_md.png"), b"md plate").unwrap();

    assert_eq!(slot::root::purge_legacy_carts(&root), 2);
    assert!(!carts.join("slot_back_recents.png").exists());
    assert!(!carts.join("cart_sfc.png").exists());
    assert!(carts.join("slot_back_gba.png").is_file());
    assert!(carts.join("slot_back_md.png").is_file());
}
