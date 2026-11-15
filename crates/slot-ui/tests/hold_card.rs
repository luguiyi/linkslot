//! The hold-to-leave prompt: a see-through screen with one card, and a bar the
//! caller fills in.
//!
//! What matters here is that the card never covers the menu it floats over
//! (everything outside it stays transparent) and that the bar the frontend
//! paints sits inside the card it was built for — the two are drawn by
//! different pieces of code, so the geometry has to be pinned somewhere.

use slot_ui::navigation::{hold_card, hold_fill, HOLD_BAR};
use slot_ui::switch_style::{bg_top, surface};
use slot_ui::{Language, UndoFace};

fn alpha(face: &UndoFace, x: u32, y: u32) -> u8 {
    face.rgba[((y * face.w + x) * 4 + 3) as usize]
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

#[test]
fn the_prompt_is_a_card_on_a_see_through_screen() {
    let face = hold_card(Language::ZhCn);
    assert_eq!((face.w, face.h), (720, 480));

    for (x, y) in [(0, 0), (719, 0), (0, 479), (719, 479), (360, 40)] {
        assert_eq!(
            alpha(&face, x, y),
            0,
            "the prompt is opaque at ({x}, {y}), so the menu behind it is hidden"
        );
    }
    assert_eq!(
        alpha(&face, 360, 420),
        255,
        "the card itself is not opaque"
    );
}

#[test]
fn the_bar_is_inside_the_card_and_its_track_is_drawn() {
    let face = hold_card(Language::ZhCn);
    let [x, y, w, h] = HOLD_BAR;
    // The card is at 140..580 by 392..468; the fill must stay well inside it,
    // or the frontend would paint a bar over the game instead of the card.
    assert!(x > 140.0 && x + w < 580.0, "the bar leaves the card sideways");
    assert!(y > 392.0 && y + h < 468.0, "the bar leaves the card vertically");

    let mid = pixel(&face, (x + w / 2.0) as u32, (y + h / 2.0) as u32);
    assert_ne!(mid, surface(), "the bar's track was not drawn");
    assert_eq!(mid, bg_top(), "the bar's track is the wrong colour");
}

#[test]
fn the_fill_is_the_accent_colour_and_covers_the_track() {
    let [r, g, b, a] = hold_fill();
    assert!(r > g && r > b, "the fill is not the red accent");
    assert_eq!(a, 1.0);
    // A full bar must be exactly the bar's area, which is why it is a plain
    // rectangle over a rounded track rather than a second face.
    assert_eq!(HOLD_BAR[3], 10.0);
}
