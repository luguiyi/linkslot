use slot_store::GbPalette;

#[test]
fn the_list_is_mgba_s_48_palettes_in_its_order() {
    let names: Vec<&str> = GbPalette::all().map(GbPalette::core_name).collect();
    assert_eq!(names.len(), GbPalette::COUNT);
    assert_eq!(GbPalette::COUNT, 48);
    assert_eq!(names[0], "Grayscale");
    assert_eq!(names[1], "DMG Green");
    assert_eq!(names[14], "GBC Dark Green →A");
    assert_eq!(names[16], "SGB 1-A");
    assert_eq!(names[47], "SGB 4-H");
}

#[test]
fn labels_drop_the_boot_combo_arrows_and_stay_unique() {
    let p = GbPalette::parse("GBC Dark Green →A").unwrap();
    assert_eq!(p.label(), "GBC 深绿");
    assert_eq!(GbPalette::parse("GBC Brown ↑").unwrap().label(), "GBC 棕色");
    assert_eq!(GbPalette::parse("SGB 2-C").unwrap().label(), "SGB 2-C");
    let mut labels: Vec<&str> = GbPalette::all().map(GbPalette::label).collect();
    labels.sort();
    labels.dedup();
    assert_eq!(
        labels.len(),
        GbPalette::COUNT,
        "two palettes print the same name"
    );
    assert!(
        GbPalette::all().all(|p| !p.label().contains(['↑', '↓', '←', '→'])),
        "an arrow survived into a label"
    );
}

#[test]
fn the_menu_shows_chinese_and_the_core_hears_mgba() {
    // The core is told the palette by name, and mGBA only answers to its
    // own: a Chinese name would leave the palette the cart had.
    let p = GbPalette::parse("GBC 深绿 →A").expect("the Chinese name did not open");
    assert_eq!(p.core_name(), "GBC Dark Green →A");
    assert_eq!(p.name(), "GBC 深绿 →A");
    assert_eq!(p.label(), "GBC 深绿");

    // Both names find the same palette, so a card written by either side
    // reads back the same.
    for name in ["GBC 深绿 →A", "GBC Dark Green →A"] {
        assert_eq!(
            GbPalette::parse(name).map(GbPalette::core_name),
            Some("GBC Dark Green →A"),
            "{name} did not open the palette"
        );
    }
}

#[test]
fn next_walks_the_list_and_wraps() {
    assert_eq!(GbPalette::DEFAULT.core_name(), "DMG Green");
    assert_eq!(GbPalette::DEFAULT.next().core_name(), "GB Pocket");
    let last = GbPalette::all().last().unwrap();
    assert_eq!(last.next().core_name(), "Grayscale");
    assert_eq!(GbPalette::parse("not a palette"), None);
}

#[test]
fn prev_walks_back_and_wraps() {
    assert_eq!(GbPalette::DEFAULT.prev().core_name(), "Grayscale");
    let first = GbPalette::all().next().unwrap();
    assert_eq!(first.prev().core_name(), "SGB 4-H");
    for p in GbPalette::all() {
        assert_eq!(p.next().prev(), p);
    }
}
