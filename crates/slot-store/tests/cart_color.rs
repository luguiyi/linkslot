use slot_store::cart_color;
use slot_store::{scan_quiet, Platform};

fn write_rom(root: &std::path::Path, platform: &str, stem: &str) {
    let dir = root.join("Games").join(platform);
    std::fs::create_dir_all(&dir).unwrap();
    let extension = if platform == "GBA" { "gba" } else { "gb" };
    std::fs::write(dir.join(format!("{stem}.{extension}")), b"rom").unwrap();
}

#[test]
fn first_scan_assigns_and_persists_distinct_colours() {
    let d = tempfile::tempdir().unwrap();
    write_rom(d.path(), "GBA", "Alpha");
    write_rom(d.path(), "GBA", "Beta");
    let mut carts = scan_quiet(d.path()).unwrap();
    assert!(carts.iter().all(|cart| cart.shell.is_none()));

    assert!(cart_color::assign_missing(d.path(), &mut carts).unwrap());
    let alpha = carts.iter().find(|cart| cart.stem == "Alpha").unwrap().shell.unwrap();
    let beta = carts.iter().find(|cart| cart.stem == "Beta").unwrap().shell.unwrap();
    assert_ne!(alpha.colour, beta.colour, "two roms got the same initial colour");
    let text = std::fs::read_to_string(d.path().join(cart_color::CART_COLOR_FILE)).unwrap();
    assert!(text.contains("GBA/Alpha"));
    assert!(text.contains(&cart_color::to_hex(alpha.colour)));

    // The next scan reads the saved values and does not assign again.
    let mut again = scan_quiet(d.path()).unwrap();
    assert!(again.iter().all(|cart| cart.shell.is_some()));
    assert!(!cart_color::assign_missing(d.path(), &mut again).unwrap());
    let again_alpha = again
        .iter()
        .find(|cart| cart.stem == "Alpha")
        .unwrap()
        .shell
        .unwrap();
    assert_eq!(again_alpha.colour, alpha.colour);
}

#[test]
fn explicit_shell_choice_is_not_overwritten() {
    let d = tempfile::tempdir().unwrap();
    write_rom(d.path(), "GB", "Tetris");
    std::fs::create_dir_all(d.path().join("linkSlot/Config")).unwrap();
    std::fs::write(
        d.path().join("linkSlot/Config/cart_shell.txt"),
        "Tetris = rounded c2332e clear\n",
    )
    .unwrap();
    let mut carts = scan_quiet(d.path()).unwrap();
    assert_eq!(carts[0].shell.unwrap().colour, [0xc2, 0x33, 0x2e]);
    assert_eq!(carts[0].shell.unwrap().outline, slot_store::Outline::Rounded);
    assert!(!cart_color::assign_missing(d.path(), &mut carts).unwrap());
    assert_eq!(carts[0].shell.unwrap().colour, [0xc2, 0x33, 0x2e]);
}

#[test]
fn platform_qualified_keys_do_not_collide() {
    let d = tempfile::tempdir().unwrap();
    write_rom(d.path(), "GBA", "Same");
    std::fs::create_dir_all(d.path().join("Games/GB")).unwrap();
    std::fs::write(d.path().join("Games/GB/Same.gb"), b"rom").unwrap();
    let mut carts = scan_quiet(d.path()).unwrap();
    cart_color::assign_missing(d.path(), &mut carts).unwrap();
    let colours: Vec<_> = carts.iter().map(|cart| cart.shell.unwrap().colour).collect();
    assert_eq!(colours.len(), 2);
    assert_ne!(colours[0], colours[1]);
}

#[test]
fn keys_are_per_platform_and_parse_hex() {
    assert_ne!(
        cart_color::key(Platform::Gba, "Same"),
        cart_color::key(Platform::Gb, "Same")
    );
    assert_eq!(cart_color::parse_hex("#a1B2c3"), Some([0xa1, 0xb2, 0xc3]));
    assert_eq!(cart_color::parse_hex("a1b2c3"), Some([0xa1, 0xb2, 0xc3]));
    assert_eq!(cart_color::parse_hex("red"), None);
}
