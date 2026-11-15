//! Visual and geometry regression tests for the three conflict pages (T21/T22/T24).

use slot_ui::conflict::{
    conflict_face, ConflictDirection, ConflictPage, ConflictUnitView, ConflictView, CARD_W,
    CONTENT_Y, CARD_X, DIRECTION_Y, SAV_ACCENT, STATE_ACCENT, TAB_H, TAB_UNDERLINE_Y, TAB_Y,
};
use slot_ui::{Language, UndoFace};

fn unit(version: u32, locked: bool, reason: &str) -> ConflictUnitView {
    ConflictUnitView {
        cloud_line: format!("云端 v{version} · RG34XX-A · 10-22 21:03 · 1 个文件"),
        local_line: format!("本地 v{} · 本机 · 10-22 21:12 · 1 个文件", version + 1),
        reason: reason.to_string(),
        locked,
    }
}

fn view(page: ConflictPage) -> ConflictView {
    let pages = if matches!(page, ConflictPage::All) {
        vec![ConflictPage::All, ConflictPage::State, ConflictPage::Sav]
    } else {
        vec![page]
    };
    ConflictView {
        pages,
        page,
        direction: ConflictDirection::Download,
        state: Some(unit(7, false, "云端有更新")),
        sav: Some(unit(9, false, "云端与本地不一致")),
        locked_state: false,
        locked_sav: false,
        error: None,
        notice: None,
        syncing: false,
    }
}

fn pixel(face: &UndoFace, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * face.w + x) * 4) as usize;
    [
        face.rgba[at],
        face.rgba[at + 1],
        face.rgba[at + 2],
        face.rgba[at + 3],
    ]
}

fn rgb(face: &UndoFace, x: u32, y: u32) -> [u8; 3] {
    let p = pixel(face, x, y);
    [p[0], p[1], p[2]]
}

fn is_colour(p: [u8; 3], colour: [u8; 4]) -> bool {
    p[0] == colour[0] && p[1] == colour[1] && p[2] == colour[2]
}

fn colour_count_in(face: &UndoFace, x0: u32, y0: u32, x1: u32, y1: u32, colour: [u8; 4]) -> usize {
    let mut count = 0;
    for y in y0..y1.min(face.h) {
        for x in x0..x1.min(face.w) {
            if is_colour(rgb(face, x, y), colour) {
                count += 1;
            }
        }
    }
    count
}

fn luminance(face: &UndoFace, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let mut total = 0.0;
    let mut samples: f32 = 0.0;
    let mut y = y0;
    while y < y1.min(face.h) {
        let mut x = x0;
        while x < x1.min(face.w) {
            let p = rgb(face, x, y);
            total += p[0] as f32 + p[1] as f32 + p[2] as f32;
            samples += 3.0;
            x += 7;
        }
        y += 7;
    }
    total / samples.max(1.0)
}

#[test]
fn the_conflict_face_is_opaque_and_full_screen() {
    for page in [ConflictPage::All, ConflictPage::State, ConflictPage::Sav] {
        let face = conflict_face(&view(page), Language::ZhCn);
        assert_eq!((face.w, face.h), (720, 480));
        assert!(
            face.rgba.chunks(4).all(|p| p[3] == 255),
            "the conflict face is not fully opaque"
        );
        assert_eq!(pixel(&face, 0, 0), [0x1b, 0x1b, 0x1f, 255]);
        assert_eq!(pixel(&face, 719, 479), [0x1b, 0x1b, 0x1f, 255]);
    }
}

#[test]
fn the_all_page_shows_both_units_and_both_accents() {
    let face = conflict_face(&view(ConflictPage::All), Language::ZhCn);
    let state = colour_count_in(
        &face,
        0,
        CONTENT_Y as u32,
        720,
        CONTENT_Y as u32 + 118,
        STATE_ACCENT,
    );
    let sav = colour_count_in(
        &face,
        0,
        (CONTENT_Y as u32) + 132,
        720,
        (CONTENT_Y as u32) + 250,
        SAV_ACCENT,
    );
    assert!(state > 300, "the state card accent is missing ({state})");
    assert!(sav > 300, "the sav card accent is missing ({sav})");
    // The mixed-direction hint is printed on the all page.
    assert_eq!(CARD_W, 648.0);
}

#[test]
fn the_state_page_is_red_and_the_sav_page_is_cyan() {
    let state = conflict_face(&view(ConflictPage::State), Language::ZhCn);
    let red = colour_count_in(
        &state,
        0,
        CONTENT_Y as u32,
        720,
        CONTENT_Y as u32 + 196,
        STATE_ACCENT,
    );
    let cyan_on_state = colour_count_in(&state, 0, 0, 720, 480, SAV_ACCENT);
    assert!(red > 500, "the state page has no red accent ({red})");
    assert_eq!(
        cyan_on_state, 0,
        "the state page must not use the sav cyan"
    );
    assert_eq!(pixel(&state, 38, (CONTENT_Y + 100.0) as u32)[3], 255);

    let sav = conflict_face(&view(ConflictPage::Sav), Language::ZhCn);
    let cyan = colour_count_in(
        &sav,
        0,
        CONTENT_Y as u32,
        720,
        CONTENT_Y as u32 + 196,
        SAV_ACCENT,
    );
    assert!(cyan > 500, "the sav page has no cyan accent ({cyan})");
}

#[test]
fn a_locked_page_is_dimmed_and_differs_from_the_idle_one() {
    let idle = conflict_face(&view(ConflictPage::State), Language::ZhCn);
    let mut locked = view(ConflictPage::State);
    locked.locked_state = true;
    let face = conflict_face(&locked, Language::ZhCn);
    assert_ne!(idle.rgba, face.rgba);
    let area = (
        60,
        CONTENT_Y as u32 + 10,
        700,
        CONTENT_Y as u32 + 180,
    );
    let before = luminance(&idle, area.0, area.1, area.2, area.3);
    let after = luminance(&face, area.0, area.1, area.2, area.3);
    assert!(
        after < before * 0.85,
        "the locked page is not veiled ({before} -> {after})"
    );
}

#[test]
fn the_tab_bar_shows_the_page_number_and_underline() {
    let mut all = view(ConflictPage::All);
    all.page = ConflictPage::All;
    let face = conflict_face(&all, Language::ZhCn);
    // The current tab underline is the red accent on the all page.
    let underline = colour_count_in(
        &face,
        0,
        TAB_UNDERLINE_Y as u32,
        720,
        TAB_UNDERLINE_Y as u32 + 3,
        STATE_ACCENT,
    );
    assert!(underline > 40, "the tab underline is missing ({underline})");
    // Page number "1 / 3" sits at the top right.
    let number_ink = (480..700)
        .flat_map(|x| (TAB_Y as u32..(TAB_Y + TAB_H) as u32).map(move |y| (x, y)))
        .filter(|(x, y)| rgb(&face, *x, *y) != [0x1b, 0x1b, 0x1f])
        .count();
    assert!(number_ink > 20, "the page number is missing");

    let mut sav = view(ConflictPage::Sav);
    sav.page = ConflictPage::Sav;
    let sav_face = conflict_face(&sav, Language::ZhCn);
    assert_ne!(
        face.rgba, sav_face.rgba,
        "changing pages did not change the face"
    );
}

#[test]
fn a_single_page_has_no_tabs_and_no_page_number() {
    let mut single = view(ConflictPage::Sav);
    single.pages = vec![ConflictPage::Sav];
    let face = conflict_face(&single, Language::ZhCn);
    let top = (0..720)
        .flat_map(|x| (0..60).map(move |y| (x, y)))
        .filter(|(x, y)| rgb(&face, *x, *y) != [0x1b, 0x1b, 0x1f])
        .count();
    assert!(
        top < 120,
        "a single page must not draw the tab bar ({top} inked pixels)"
    );
}

#[test]
fn the_direction_choice_moves_the_white_chip() {
    let mut download = view(ConflictPage::State);
    download.direction = ConflictDirection::Download;
    let mut upload = download.clone();
    upload.direction = ConflictDirection::Upload;
    let a = conflict_face(&download, Language::ZhCn);
    let b = conflict_face(&upload, Language::ZhCn);
    assert_ne!(a.rgba, b.rgba, "changing direction did not change the face");
    let row = (
        0,
        DIRECTION_Y as u32,
        720,
        (DIRECTION_Y + 36.0) as u32,
    );
    let white_a = (row.0..row.2)
        .flat_map(|x| (row.1..row.3).map(move |y| (x, y)))
        .filter(|(x, y)| {
            let p = rgb(&a, *x, *y);
            p == [0xf2, 0xf2, 0xf2]
        })
        .count();
    assert!(white_a > 300, "the selected direction chip is missing");
}

#[test]
fn the_error_and_syncing_states_have_their_own_lines() {
    let mut error = view(ConflictPage::Sav);
    error.error = Some("电池存档失败：网络未连接".to_string());
    let face = conflict_face(&error, Language::ZhCn);
    let mut red = 0;
    for y in (CONTENT_Y as u32) + 190..DIRECTION_Y as u32 {
        for x in 0..720 {
            let p = rgb(&face, x, y);
            if p[0] > 180 && p[1] < 140 && p[2] < 140 {
                red += 1;
            }
        }
    }
    assert!(red > 20, "the failure line is not drawn in red ({red})");

    // §7.7: the mixed-direction hint is permanent, even without an error.
    let plain = conflict_face(&view(ConflictPage::State), Language::ZhCn);
    let hint_ink = ((DIRECTION_Y as u32 - 18)..(DIRECTION_Y as u32 - 2))
        .flat_map(|y| (0..720).map(move |x| (x, y)))
        .filter(|(x, y)| rgb(&plain, *x, *y) != [0x1b, 0x1b, 0x1f])
        .count();
    assert!(
        hint_ink > 20,
        "the permanent mixed-direction hint is missing ({hint_ink})"
    );

    let mut syncing = view(ConflictPage::State);
    syncing.syncing = true;
    let face = conflict_face(&syncing, Language::ZhCn);
    assert_ne!(conflict_face(&view(ConflictPage::State), Language::ZhCn).rgba, face.rgba);
}

#[test]
fn the_tab_underline_moves_with_the_page() {
    let mut all = view(ConflictPage::All);
    all.page = ConflictPage::All;
    let mut sav = view(ConflictPage::All);
    sav.page = ConflictPage::Sav;
    let underline_left = |face: &UndoFace| {
        (0..720)
            .find(|x| {
                (TAB_UNDERLINE_Y as u32..(TAB_UNDERLINE_Y as u32 + 3))
                    .any(|y| is_colour(rgb(face, *x, y), STATE_ACCENT))
            })
            .expect("no tab underline")
    };
    let a = conflict_face(&all, Language::ZhCn);
    let b = conflict_face(&sav, Language::ZhCn);
    assert!(
        underline_left(&b) > underline_left(&a),
        "the tab underline did not move with the page"
    );
}

#[test]
fn a_locked_tab_shows_a_check_mark_in_the_tab_bar() {
    let mut locked = view(ConflictPage::All);
    locked.page = ConflictPage::State;
    locked.locked_state = true;
    let locked_face = conflict_face(&locked, Language::ZhCn);
    let mut idle = view(ConflictPage::All);
    idle.page = ConflictPage::State;
    let idle_face = conflict_face(&idle, Language::ZhCn);
    // The locked tab adds a procedural check mark in the tab band.
    let mut changed = 0;
    for x in 96..220 {
        for y in TAB_Y as u32..(TAB_Y + TAB_H) as u32 {
            if pixel(&idle_face, x, y) != pixel(&locked_face, x, y) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 30,
        "no check mark was added to the locked tab ({changed} changed pixels)"
    );
}

#[test]
fn the_all_page_cards_sit_at_the_exported_geometry() {
    let face = conflict_face(&view(ConflictPage::All), Language::ZhCn);
    let surface = [0x2d, 0x2d, 0x33, 0xff];
    let x = CARD_X as u32 + 12;
    for top in [CONTENT_Y, CONTENT_Y + 132.0] {
        let y = top as u32 + 24;
        assert_eq!(
            pixel(&face, x, y),
            surface,
            "the card interior is not at {top}"
        );
    }
    // The gap between the two cards is the page background.
    assert_eq!(
        pixel(&face, x, (CONTENT_Y + 124.0) as u32),
        [0x1b, 0x1b, 0x1f, 255]
    );
}

#[test]
fn the_selected_direction_chip_starts_at_the_card_edge() {
    let face = conflict_face(&view(ConflictPage::State), Language::ZhCn);
    let white = [0xf2, 0xf2, 0xf2, 0xff];
    assert_eq!(
        pixel(&face, CARD_X as u32 + 8, DIRECTION_Y as u32 + 8),
        white,
        "the download chip is not at the card edge"
    );
    let mut upload = view(ConflictPage::State);
    upload.direction = ConflictDirection::Upload;
    let face = conflict_face(&upload, Language::ZhCn);
    assert_ne!(
        pixel(&face, CARD_X as u32 + 8, DIRECTION_Y as u32 + 8),
        white,
        "the download chip stayed white after switching to upload"
    );
    assert_eq!(
        pixel(
            &face,
            (CARD_X + 132.0 + 16.0) as u32 + 8,
            DIRECTION_Y as u32 + 8
        ),
        white,
        "the upload chip is not the second chip"
    );
}
