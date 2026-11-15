use slot_ui::{
    collection_plate_face, slot_plate_from_art, slot_plate_from_art_inset, CollectionKind,
    Language, SLOT_PLATE_H, SLOT_PLATE_W,
};

#[test]
fn every_rack_gets_an_etched_slot_mark() {
    let mut seen = Vec::new();
    let total = (SLOT_PLATE_W * SLOT_PLATE_H) as usize;
    for kind in CollectionKind::ALL {
        let face = collection_plate_face(kind, Language::ZhCn);
        assert_eq!(
            (face.w, face.h),
            (SLOT_PLATE_W, SLOT_PLATE_H),
            "{kind:?} has the wrong plate size"
        );
        let opaque = face.rgba.chunks_exact(4).filter(|px| px[3] == 0xff).count();
        let painted = face.rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(painted > 0, "{kind:?} has no etched text");
        assert!(
            opaque < total / 4,
            "{kind:?} still paints a solid plate instead of etching the slot"
        );
        // Masked to the slot's trough: the mark must not spill onto the rim.
        assert_eq!(face.rgba[3], 0, "{kind:?} plate corner is not masked");
        let middle_rows = (SLOT_PLATE_H / 3..SLOT_PLATE_H * 2 / 3)
            .flat_map(|y| (0..SLOT_PLATE_W).map(move |x| (y * SLOT_PLATE_W + x) * 4))
            .filter(|at| face.rgba[*at as usize + 3] > 0)
            .count();
        assert!(middle_rows > 0, "{kind:?} mark is not visible in the trough");
        assert!(!seen.contains(&face.rgba), "{kind:?} repeats another plate");
        seen.push(face.rgba);
    }
}

#[test]
fn a_card_nameplate_is_fitted_and_masked() {
    // A 380x44 card nameplate, like upstream's; the fitter normalises it to
    // the app's plate canvas and clips it to the trough.
    let art = vec![0x7fu8; 380 * 44 * 4];
    let face = slot_plate_from_art(&art, 380, 44);
    assert_eq!((face.w, face.h), (SLOT_PLATE_W, SLOT_PLATE_H));
    assert_eq!(face.rgba[3], 0, "the art was not masked");
    let middle = (SLOT_PLATE_H / 2 * SLOT_PLATE_W + SLOT_PLATE_W / 2) as usize * 4;
    assert!(face.rgba[middle + 3] > 0, "the art is not visible in the slot");
}

#[test]
fn collection_icons_can_be_inset_away_from_the_slot_corners() {
    let art = vec![0x7fu8; 380 * 44 * 4];
    let face = slot_plate_from_art_inset(&art, 380, 44, 28.0);
    let left_middle = (SLOT_PLATE_H / 2 * SLOT_PLATE_W + 2) as usize * 4;
    assert_eq!(
        face.rgba[left_middle + 3], 0,
        "the inset plate must not paint the left icon gutter"
    );
    let middle = (SLOT_PLATE_H / 2 * SLOT_PLATE_W + SLOT_PLATE_W / 2) as usize * 4;
    assert!(face.rgba[middle + 3] > 0, "the inset plate is empty");
}

#[test]
fn names_and_card_files_follow_upstream() {
    let zh = Language::ZhCn;
    assert_eq!(CollectionKind::Gba.plate_name(zh), "GBA");
    assert_eq!(CollectionKind::Gb.plate_name(zh), "GB");
    assert_eq!(CollectionKind::Gbc.plate_name(zh), "GBC");
    assert_eq!(CollectionKind::Fc.plate_name(zh), "FC");
    assert_eq!(CollectionKind::Gba.plate_name(Language::EnUs), "GAME BOY ADVANCE");
    assert_eq!(CollectionKind::Gb.plate_name(Language::EnUs), "GAME BOY");
    assert_eq!(CollectionKind::Recents.plate_name(zh), "最近游玩");
    assert_eq!(CollectionKind::Favorites.plate_name(zh), "收藏");
    assert_eq!(CollectionKind::Gba.plate_code(zh), "GBA");
    assert_eq!(CollectionKind::Gb.plate_code(zh), "GB");
    assert_eq!(CollectionKind::Gbc.plate_code(zh), "GBC");
    assert_eq!(CollectionKind::Fc.plate_code(zh), "FC");
    assert_eq!(CollectionKind::Recents.plate_code(zh), "最近游玩");
    assert_eq!(CollectionKind::Favorites.plate_code(zh), "收藏");

    assert_eq!(CollectionKind::Gba.slot_back_file(), "slot_back_gba.png");
    assert_eq!(CollectionKind::Gb.slot_back_file(), "slot_back_gb.png");
    assert_eq!(CollectionKind::Gbc.slot_back_file(), "slot_back_gbc.png");
    assert_eq!(CollectionKind::Fc.slot_back_file(), "slot_back_fc.png");
    assert_eq!(
        CollectionKind::Recents.slot_back_file(),
        "slot_back_recents.png"
    );
    assert_eq!(
        CollectionKind::Favorites.slot_back_file(),
        "slot_back_favorites.png"
    );

    // The two special racks must not silently borrow a machine's wordmark.
    let mut names: Vec<&str> = CollectionKind::ALL
        .iter()
        .map(|k| k.plate_name(Language::ZhCn))
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CollectionKind::ALL.len());
}
