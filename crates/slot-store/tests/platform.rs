use slot_store::Platform;

#[test]
fn every_platform_has_a_directory() {
    let names: Vec<&str> = Platform::ALL.iter().map(|p| p.dir_name()).collect();
    assert_eq!(names, vec!["GBA", "GB", "GBC", "FC", "SFC", "MD"]);
}

#[test]
fn every_platform_is_a_shelf_of_its_own() {
    assert_eq!(
        Platform::ALL,
        [
            Platform::Gba,
            Platform::Gb,
            Platform::Gbc,
            Platform::Fc,
            Platform::Sfc,
            Platform::Md
        ]
    );
    for (i, p) in Platform::ALL.iter().enumerate() {
        assert!(
            !Platform::ALL[..i].contains(p),
            "{p:?} is named twice in the ring, so two shelves would hold one platform"
        );
    }
}

#[test]
fn each_platform_takes_only_its_own_extensions() {
    assert!(Platform::Gba.accepts(std::path::Path::new("Metroid Fusion.gba")));
    assert!(!Platform::Gba.accepts(std::path::Path::new("Tetris.gb")));
    assert!(Platform::Gb.accepts(std::path::Path::new("Tetris.gb")));
    assert!(Platform::Gb.accepts(std::path::Path::new("Tetris.gbc")));
    assert!(!Platform::Gb.accepts(std::path::Path::new("Metroid Fusion.gba")));
    assert!(Platform::Gba.accepts(std::path::Path::new("Shrek.GBA")));
    assert!(Platform::Fc.accepts(std::path::Path::new("Contra.nes")));
    assert!(Platform::Fc.accepts(std::path::Path::new("Contra.NES")));
    assert!(!Platform::Fc.accepts(std::path::Path::new("Contra.gba")));
    assert!(!Platform::Gb.accepts(std::path::Path::new("Contra.nes")));
    assert!(Platform::Sfc.accepts(std::path::Path::new("Mario.sfc")));
    assert!(Platform::Sfc.accepts(std::path::Path::new("Mario.smc")));
    assert!(Platform::Sfc.accepts(std::path::Path::new("Mario.fig")));
    assert!(!Platform::Sfc.accepts(std::path::Path::new("Contra.nes")));
}

#[test]
fn platform_index_matches_all() {
    // `SlotState` stores per-platform switches in `[T; 4]`; a reordered
    // `Platform::ALL` would silently map GBC settings onto GB.
    for (i, p) in Platform::ALL.iter().enumerate() {
        assert_eq!(p.index(), i, "{p:?} is out of order");
    }
    assert_eq!(
        Platform::Fc.index(),
        3,
        "Famicom must stay the appended fourth slot"
    );
    // `index()` is documented as the enum discriminant; a future explicit
    // `#[repr]` or reordering must keep the slot state arrays readable.
    assert_eq!(Platform::Fc as usize, 3);
}

#[test]
fn each_platform_has_a_fixed_core_frame_canvas() {
    // The canvas is what the renderer scales from; it must not follow the
    // ROM's native resolution (GB is 160x144, yet its canvas is 240x160).
    assert_eq!(Platform::Gba.frame(), (240, 160));
    assert_eq!(Platform::Gb.frame(), (240, 160));
    assert_eq!(Platform::Gbc.frame(), (240, 160));
    assert_eq!(Platform::Fc.frame(), (256, 224));
    assert_eq!(Platform::Sfc.frame(), (256, 224));
    assert_eq!(Platform::Md.frame(), (320, 224));
    assert_eq!(Platform::Md.picture(), (320, 224));
    assert_eq!(Platform::Md.name(), "Mega Drive");
    assert_eq!(Platform::Md.dir_name(), "MD");
    assert!(Platform::Md.accepts(std::path::Path::new("Sonic.md")));
    assert!(Platform::Md.accepts(std::path::Path::new("Sonic.gen")));
    assert!(Platform::Md.accepts(std::path::Path::new("Sonic.smd")));
    assert!(Platform::Md.accepts(std::path::Path::new("Sonic.bin")));
    assert!(!Platform::Md.accepts(std::path::Path::new("Sonic.sfc")));
}

#[test]
fn famicom_names_itself() {
    assert_eq!(Platform::Fc.name(), "Famicom");
    assert_eq!(Platform::Fc.dir_name(), "FC");
}

#[test]
fn super_famicom_names_itself() {
    assert_eq!(Platform::Sfc.name(), "Super Famicom");
    assert_eq!(Platform::Sfc.dir_name(), "SFC");
}
