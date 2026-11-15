//! A non-blocking notice: one card under the status bar that fades away.
//!
//! Used where a dialog would only ask the player to press a key: a dropped
//! peer, a failed cartridge transfer.  It never takes input and never covers
//! the game — five seconds and it is gone.

use slot_gfx::{OUT_H, OUT_W};

use crate::keyboard as kb;
use crate::switch_style as sw;
use crate::plate::UndoFace;

pub const NOTICE_W: f32 = 420.0;
pub const NOTICE_H: f32 = 62.0;
/// Just under the 40 px status bar.
pub const NOTICE_TOP: f32 = 52.0;
/// How long a notice stays on screen, counting the fades.
pub const NOTICE_MS: u64 = 5000;
/// Fade in at the start and out at the end.
pub const NOTICE_FADE_MS: u64 = 300;

pub fn notice_rect() -> (f32, f32, f32, f32) {
    ((OUT_W as f32 - NOTICE_W) / 2.0, NOTICE_TOP, NOTICE_W, NOTICE_H)
}

/// 0..1 visibility for a notice that appeared at `shown` milliseconds.
pub fn notice_alpha(shown: u64, now: u64) -> f32 {
    let age = now.saturating_sub(shown);
    if age >= NOTICE_MS {
        return 0.0;
    }
    let fade_in = (age as f32 / NOTICE_FADE_MS as f32).min(1.0);
    let fade_out = ((NOTICE_MS - age) as f32 / NOTICE_FADE_MS as f32).min(1.0);
    fade_in.min(fade_out)
}

/// The card itself: transparent everywhere except the plate, so the caller can
/// blit it straight over the picture.
pub fn notice_face(title: &str, detail: &str) -> UndoFace {
    let w = NOTICE_W as u32;
    let h = NOTICE_H as u32;
    let mut face = kb::blank(w, h, [0, 0, 0, 0]);
    let radius = 12.0;
    // Soft drop shadow, then the plate in the dialog's own colours.
    kb::fill(&mut face, 0.0, 3.0, NOTICE_W, NOTICE_H, [0, 0, 0, 80]);
    sw::fill_round(&mut face, 0.0, 0.0, NOTICE_W, NOTICE_H, radius, sw::surface_border());
    sw::fill_round(
        &mut face,
        1.0,
        1.0,
        NOTICE_W - 2.0,
        NOTICE_H - 2.0,
        (radius - 1.0).max(0.0),
        sw::surface(),
    );
    // The same accent spine the modal dialogs carry.
    let accent = sw::accent();
    kb::fill(
        &mut face,
        14.0,
        13.0,
        4.0,
        NOTICE_H - 26.0,
        [accent[0], accent[1], accent[2], 0xff],
    );
    kb::draw_left(
        &mut face,
        title,
        17.0,
        12.0,
        NOTICE_W - 48.0,
        28.0,
        9.0,
        22.0,
        sw::ink(),
    );
    kb::draw_left(
        &mut face,
        detail,
        12.0,
        9.0,
        NOTICE_W - 48.0,
        28.0,
        34.0,
        20.0,
        sw::ink_dim(),
    );
    face
}

/// Clamp the card to the screen even if a future panel size changes.
pub fn notice_bottom() -> f32 {
    (NOTICE_TOP + NOTICE_H).min(OUT_H as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_starts_and_ends_invisible() {
        assert_eq!(notice_alpha(0, 0), 0.0, "it appeared at full alpha");
        assert_eq!(notice_alpha(0, NOTICE_MS), 0.0, "it outlived five seconds");
        assert_eq!(notice_alpha(0, 10_000), 0.0);
    }

    #[test]
    fn the_card_is_opaque_in_the_middle_of_its_life() {
        assert_eq!(notice_alpha(0, NOTICE_FADE_MS), 1.0);
        assert_eq!(notice_alpha(0, 2_500), 1.0);
        assert!(
            notice_alpha(0, NOTICE_MS - NOTICE_FADE_MS / 2) < 1.0,
            "the tail did not fade"
        );
    }

    #[test]
    fn the_face_is_a_rounded_card_with_clean_corners() {
        let face = notice_face("联机已断开", "对方已断开连接 · 可继续游戏");
        assert_eq!((face.w, face.h), (NOTICE_W as u32, NOTICE_H as u32));
        let at = |x: u32, y: u32| face.rgba[((y * face.w + x) * 4 + 3) as usize];
        assert_eq!(at(0, 0), 0, "the card corner is square");
        assert_eq!(at(face.w - 1, 0), 0, "the card corner is square");
        assert!(at(face.w / 2, face.h / 2) > 200, "the card has no plate");
        let opaque = face.rgba.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 5_000, "the card is nearly empty: {opaque}");
    }

    #[test]
    fn the_card_sits_under_the_status_bar() {
        let (_, y, _, h) = notice_rect();
        assert!(y >= 40.0, "the notice overlapped the status bar: {y}");
        assert!(y + h <= 480.0, "the notice ran off the screen");
    }
}
