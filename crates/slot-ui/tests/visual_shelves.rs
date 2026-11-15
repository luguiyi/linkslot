use slot_gfx::{Draw, TexId};
use slot_ui::sync_menu::{SyncMenuRow, SyncMenuView};
use slot_ui::Language;
use slot_ui::{collection_overlay_face, shelf_backdrop_face, CollectionKind};

#[test]
fn the_generated_shelf_ground_is_a_full_opaque_frame() {
    let face = shelf_backdrop_face();
    assert_eq!((face.w, face.h), (720, 480));
    assert_eq!(face.rgba.len(), (720 * 480 * 4) as usize);
    assert!(face.rgba.chunks(4).all(|pixel| pixel[3] == 255));
    // The generated face must not be a flat black rectangle.
    assert!(
        face.rgba
            .chunks(4)
            .any(|pixel| pixel[0] > 32 && pixel[1] > 32 && pixel[2] > 32)
    );
}

#[test]
fn the_sync_menu_gate_is_visible_as_grey_and_white_states() {
    let rows = [
        SyncMenuRow {
            name: "家里",
            detail: "https://home.example/dav · /home",
            active: true,
            password_set: true,
        },
        SyncMenuRow {
            name: "公司",
            detail: "https://work.example/dav · /work",
            active: false,
            password_set: true,
        },
    ];
    let view = SyncMenuView {
        rows: &rows,
        selected: 1,
        test_ready: true,
        testing: false,
        active: false,
        can_switch: false,
        status: Some("按 A 测试当前同步方式"),
        error: None,
        needs_setup: false,
    };
    let locked = slot_ui::sync_menu::sync_menu_face(&view, Language::ZhCn);
    assert_eq!((locked.w, locked.h), (720, 480));
    assert!(
        locked.rgba.chunks(4).all(|pixel| pixel[3] == 255),
        "sync page must be opaque so the previous screen cannot bleed through"
    );
    assert!(locked.rgba.chunks(4).any(|pixel| pixel[3] > 0));

    let unlocked = slot_ui::sync_menu::sync_menu_face(
        &SyncMenuView {
            can_switch: true,
            ..view
        },
        Language::ZhCn,
    );
    assert_eq!(unlocked.rgba.len(), locked.rgba.len());
    assert_ne!(unlocked.rgba, locked.rgba, "switch state is not rendered");

    let setup = slot_ui::sync_menu::sync_menu_face(&SyncMenuView {
        rows: &[],
        selected: 0,
        test_ready: false,
        testing: false,
        active: false,
        can_switch: false,
        status: Some("首次使用：请先启动 Web 服务配置同步方式"),
        error: None,
        needs_setup: true,
    }, Language::ZhCn);
    assert_eq!((setup.w, setup.h), (720, 480));
    assert!(
        setup.rgba.chunks(4).all(|pixel| pixel[3] == 255),
        "the first-use page must be opaque"
    );
    assert_ne!(
        setup.rgba,
        locked.rgba,
        "first-use guidance is not rendered"
    );
}

#[test]
fn the_backdrop_fallback_draws_a_ground_and_collection_wash() {
    let mut out = Vec::new();
    slot_ui::draw_backdrop(
        None,
        None,
        [0x4f, 0x6b, 0xff],
        slot_ui::CollectionKind::Gba,
        None,
        &mut out,
    );
    assert!(matches!(out[0], Draw::Rect { .. }));
    assert!(out.len() >= 2);
}

#[test]
fn every_collection_has_a_sparse_overlay_with_a_clear_centre() {
    for kind in CollectionKind::ALL {
        let face = collection_overlay_face(kind);
        assert_eq!((face.w, face.h), (720, 480));
        assert!(
            face.rgba.chunks(4).any(|pixel| pixel[3] > 0),
            "{kind:?} overlay is blank"
        );
        assert!(
            face.rgba.chunks(4).any(|pixel| pixel[3] == 0),
            "{kind:?} overlay has no transparent areas"
        );
        for y in 140..330 {
            for x in 220..450 {
                let at = ((y * face.w + x) * 4) as usize;
                assert_eq!(
                    face.rgba[at + 3], 0,
                    "{kind:?} motif enters the cartridge centre at ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn the_backdrop_uses_the_current_collection_overlay() {
    let overlay = TexId::from_raw(7);
    let mut out = Vec::new();
    slot_ui::draw_backdrop(
        None,
        None,
        [0x20, 0x6b, 0xc4],
        CollectionKind::Gba,
        Some(overlay),
        &mut out,
    );
    assert!(out.iter().any(|draw| matches!(
        draw,
        Draw::Tex { tex, .. } if *tex == overlay
    )));

    let wallpaper = TexId::from_raw(8);
    let mut wallpapered = Vec::new();
    slot_ui::draw_backdrop(
        Some(wallpaper),
        None,
        [0x20, 0x6b, 0xc4],
        CollectionKind::Gba,
        Some(overlay),
        &mut wallpapered,
    );
    assert!(!wallpapered.iter().any(|draw| matches!(
        draw,
        Draw::Tex { tex, .. } if *tex == overlay
    )));
}

#[test]
fn favorites_keep_their_star_overlay() {
    let stars = slot_ui::favorite_stars_face();
    assert_eq!((stars.w, stars.h), (720, 480));
    assert!(stars.rgba.chunks(4).any(|pixel| pixel[3] > 0));
    assert!(stars.rgba.chunks(4).any(|pixel| pixel[3] == 0));
    assert_eq!(
        collection_overlay_face(CollectionKind::Favorites).rgba,
        stars.rgba
    );
}

#[test]
fn the_backdrop_fallback_without_an_overlay_has_no_texture_draw() {
    let mut out = Vec::new();
    slot_ui::draw_backdrop(
        None,
        None,
        [0x4f, 0x6b, 0xff],
        CollectionKind::Gba,
        None,
        &mut out,
    );
    assert!(matches!(out[0], Draw::Rect { .. }));
    assert!(!out.iter().any(|draw| matches!(draw, Draw::Tex { .. })));
}
