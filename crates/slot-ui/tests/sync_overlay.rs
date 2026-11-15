use slot_ui::sync_overlay::{arrow_face, cloud_face, line_face, Arrow};
use slot_ui::{CartFace, UndoFace};

fn inked(face: &[u8]) -> usize {
    face.chunks_exact(4).filter(|p| p[3] > 0).count()
}

fn ink_mass(face: &CartFace, top_half: bool) -> u64 {
    let from = if top_half { 0 } else { face.h as usize / 2 };
    let to = if top_half {
        face.h as usize / 2
    } else {
        face.h as usize
    };
    face.rgba[from * face.w as usize * 4..to * face.w as usize * 4]
        .chunks_exact(4)
        .map(|p| p[3] as u64)
        .sum()
}

fn face_ok(w: u32, h: u32, rgba: &[u8]) -> bool {
    w > 0 && h > 0 && rgba.len() == (w * h * 4) as usize
}

#[test]
fn the_cloud_and_the_arrows_raster_into_faces() {
    for px in [16.0, 24.0, 48.0] {
        let cloud = cloud_face(px);
        assert!(
            face_ok(cloud.w, cloud.h, &cloud.rgba),
            "the cloud at {px} px is {0}x{1} with {2} bytes",
            cloud.w,
            cloud.h,
            cloud.rgba.len()
        );
        assert!(inked(&cloud.rgba) > 0, "the cloud at {px} px is blank");
        for dir in [Arrow::Up, Arrow::Down] {
            let arrow = arrow_face(dir, px);
            assert!(
                face_ok(arrow.w, arrow.h, &arrow.rgba),
                "the {dir:?} arrow at {px} px is malformed"
            );
            assert!(
                inked(&arrow.rgba) > 0,
                "the {dir:?} arrow at {px} px is blank"
            );
        }
    }
}

#[test]
fn a_bigger_icon_rasteres_bigger() {
    let small = cloud_face(20.0);
    let large = cloud_face(40.0);
    assert!(large.w > small.w && large.h > small.h);
    assert_eq!(large.h, 40, "px is the cloud's height");
    assert_eq!(large.w, 60, "the cloud is one and a half times as wide");
    assert_eq!((arrow_face(Arrow::Up, 33.4).w, arrow_face(Arrow::Up, 33.4).h), (33, 33));
}

#[test]
fn the_arrows_point_opposite_ways() {
    let up = arrow_face(Arrow::Up, 48.0);
    let down = arrow_face(Arrow::Down, 48.0);
    assert_ne!(up.rgba, down.rgba, "the two arrows are the same art");
    assert_eq!((up.w, up.h), (down.w, down.h));
    let (up_top, up_bottom) = (ink_mass(&up, true), ink_mass(&up, false));
    let (down_top, down_bottom) = (ink_mass(&down, true), ink_mass(&down, false));
    assert!(
        up_top > up_bottom,
        "the up arrow's head is not on top ({up_top} above, {up_bottom} below)"
    );
    assert!(
        down_bottom > down_top,
        "the down arrow's head is not at the bottom ({down_top} above, {down_bottom} below)"
    );
}

#[test]
fn a_line_face_is_inked_and_no_wider_than_asked() {
    for (text, max_w) in [
        ("正在同步…", 320.0),
        ("已拉取版本 12", 320.0),
        ("覆盖本地", 140.0),
    ] {
        let face: UndoFace = line_face(text, max_w, 28.0);
        assert!(
            face_ok(face.w, face.h, &face.rgba),
            "{text:?} rastered to {0}x{1}",
            face.w,
            face.h
        );
        assert!(inked(&face.rgba) > 0, "{text:?} rastered to nothing");
        assert!(
            face.w as f32 <= max_w,
            "{text:?} is {} px wide in a {max_w} px box",
            face.w
        );
    }
}

#[test]
fn a_long_line_shrinks_instead_of_overflowing() {
    let short = line_face("覆盖本地", 560.0, 28.0);
    let long = line_face(
        "云端版本 12 · rg34xx-a1b2c3d4 · 2026-10-21 09:30 · 已是最新",
        220.0,
        28.0,
    );
    assert!(long.w as f32 <= 220.0, "the long line overran its box");
    assert!(inked(&long.rgba) > 0, "the long line rastered to nothing");
    assert!(
        long.h < short.h,
        "the long line ({} px tall) did not shrink below the short one ({} px)",
        long.h,
        short.h
    );
}

#[test]
fn a_line_face_keeps_a_transparent_background() {
    let face = line_face("云端版本 12 · 已是最新", 480.0, 26.0);
    assert!(
        face.rgba.chunks_exact(4).any(|p| p[3] == 0),
        "the line has an opaque background"
    );
    let col_inked = |x: usize| {
        (0..face.h as usize).any(|y| face.rgba[(y * face.w as usize + x) * 4 + 3] > 0)
    };
    assert!(
        col_inked(0) && col_inked(face.w as usize - 1),
        "the line was not trimmed to its ink"
    );
    assert!(
        (0..face.w as usize).any(|x| !col_inked(x)),
        "the face is solid from edge to edge"
    );
}

#[test]
fn an_impossible_box_still_produces_a_face_inside_it() {
    let face = line_face(
        "一段非常非常长的中文提示文字，用来确认最小字号时也不会溢出给定的宽度",
        80.0,
        32.0,
    );
    assert!(face.w > 0 && face.h > 0, "the shrunken line vanished");
    assert!(
        face.w as f32 <= 80.0,
        "the shrunken line is {} px wide",
        face.w
    );
}

#[test]
fn pin_one_mark_follows_its_own_lamp() {
    let off = slot_ui::network_chip_face_scaled_marked(None, 6, true, false, None);
    let on = slot_ui::network_chip_face_scaled_marked(None, 6, true, true, None);
    let at = |face: &slot_ui::CartFace, x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
    };
    // (8.6, 15.6) scaled by 6, plus one pixel into the 2-unit cell.
    let (x, y) = (8.6f32 as u32 * 6 + 3, 15.6f32 as u32 * 6 + 3);
    assert!(at(&on, x, y)[1] > 180, "pin-1 should be lit");
    assert!(at(&off, x, y)[1] < 80, "pin-1 should be dark");
    // The status LED keeps burning on both faces.
    let (lx, ly) = (12.6f32 as u32 * 6 + 3, 15.6f32 as u32 * 6 + 3);
    assert!(at(&off, lx, ly)[1] > 180, "status LED stays lit");
    assert_eq!(at(&on, lx, ly), at(&off, lx, ly));
}
