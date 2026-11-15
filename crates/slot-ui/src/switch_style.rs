//! Shared Nintendo-Switch-inspired menu surface: charcoal background, rounded
//! cards, a white selected card and the red accent used by the device menus.

use crate::plate::UndoFace;
use crate::text;

pub fn bg() -> [u8; 4] {
    crate::slot_chrome::theme().bg
}
pub fn bg_top() -> [u8; 4] {
    crate::slot_chrome::theme().bg_top
}
pub fn surface() -> [u8; 4] {
    crate::slot_chrome::theme().surface
}
pub fn surface_border() -> [u8; 4] {
    crate::slot_chrome::theme().surface_border
}
pub fn selected() -> [u8; 4] {
    crate::slot_chrome::theme().selected
}
pub fn selected_border() -> [u8; 4] {
    crate::slot_chrome::theme().selected_border
}
pub fn ink() -> [u8; 3] {
    crate::slot_chrome::theme().ink
}
pub fn ink_selected() -> [u8; 3] {
    crate::slot_chrome::theme().ink_selected
}
pub fn ink_dim() -> [u8; 3] {
    crate::slot_chrome::theme().ink_dim
}
pub fn accent() -> [u8; 3] {
    crate::slot_chrome::theme().accent
}
pub fn accent_dim() -> [u8; 3] {
    crate::slot_chrome::theme().accent_dim
}

pub const CARD_RADIUS: f32 = 11.0;
pub const DIALOG_RADIUS: f32 = 16.0;

/// Blend `colour` over the face at one pixel, respecting the destination alpha.
fn blend_px(face: &mut UndoFace, x: u32, y: u32, colour: [u8; 4], coverage: f32) {
    if x >= face.w || y >= face.h || coverage <= 0.0 {
        return;
    }
    let i = ((y * face.w + x) * 4) as usize;
    let src = (colour[3] as f32 * coverage.clamp(0.0, 1.0) / 255.0).clamp(0.0, 1.0);
    if src <= 0.0 {
        return;
    }
    let dst = face.rgba[i + 3] as f32 / 255.0;
    let out = src + dst * (1.0 - src);
    if out <= 0.0 {
        return;
    }
    for (c, channel) in colour.iter().enumerate().take(3) {
        let a = *channel as f32 / 255.0;
        let b = face.rgba[i + c] as f32 / 255.0;
        face.rgba[i + c] = (((a * src + b * dst * (1.0 - src)) / out) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    face.rgba[i + 3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn rounded_coverage(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    if w <= 0.0 || h <= 0.0 {
        return 0.0;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if px < x || py < y || px >= x + w || py >= y + h {
        return 0.0;
    }
    let cx = px.clamp(x + r, x + w - r);
    let cy = py.clamp(y + r, y + h - r);
    let dx = px - cx;
    let dy = py - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    (r - dist + 0.5).clamp(0.0, 1.0)
}

/// Draw a rounded rectangle into an existing RGBA face.
pub fn fill_round(
    face: &mut UndoFace,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    colour: [u8; 4],
) {
    let x0 = x.floor().max(0.0) as u32;
    let y0 = y.floor().max(0.0) as u32;
    let x1 = (x + w).ceil().min(face.w as f32) as u32;
    let y1 = (y + h).ceil().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let coverage = rounded_coverage(
                px as f32 + 0.5,
                py as f32 + 0.5,
                x,
                y,
                w,
                h,
                radius,
            );
            blend_px(face, px, py, colour, coverage);
        }
    }
}

/// A transparent rounded card face, optionally with a 1 px border.
pub fn rounded_face(
    w: u32,
    h: u32,
    radius: f32,
    fill: [u8; 4],
    border: Option<[u8; 4]>,
) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (w * h * 4) as usize],
        w,
        h,
    };
    if w == 0 || h == 0 {
        return face;
    }
    if let Some(border) = border {
        fill_round(
            &mut face,
            0.5,
            0.5,
            w as f32 - 1.0,
            h as f32 - 1.0,
            radius,
            border,
        );
        fill_round(
            &mut face,
            1.5,
            1.5,
            w as f32 - 3.0,
            h as f32 - 3.0,
            (radius - 1.0).max(0.0),
            fill,
        );
    } else {
        fill_round(&mut face, 0.0, 0.0, w as f32, h as f32, radius, fill);
    }
    face
}

pub fn card_face(w: u32, h: u32, selected: bool) -> UndoFace {
    if selected {
        rounded_face(
            w,
            h,
            CARD_RADIUS,
            crate::switch_style::selected(),
            Some(selected_border()),
        )
    } else {
        rounded_face(w, h, CARD_RADIUS, surface(), Some(surface_border()))
    }
}

/// A small rounded icon tile with one or two characters, used as a reliable
/// fallback when the bundled symbol font lacks a glyph.
pub fn tile_face(label: &str, fill: [u8; 4], ink: [u8; 3], size: u32) -> UndoFace {
    let mut face = rounded_face(size, size, 7.0, fill, None);
    let Some(font) = text::label_font() else {
        return face;
    };
    let layout = text::fit(font, label, size as f32 - 4.0, 1, 18.0, 12.0);
    text::draw_centred(&mut face.rgba, size, size, &layout, ink);
    face
}

/// White title/heading type on a transparent background.
pub fn title_face(text_value: &str, px: f32) -> UndoFace {
    let Some(font) = text::label_font() else {
        return UndoFace {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    };
    let layout = text::fit(font, text_value, 600.0, 1, px, 14.0);
    let ink = layout
        .lines
        .iter()
        .map(|line| text::line_width(font, line, layout.px, layout.tracking))
        .fold(0.0, f32::max)
        .ceil() as u32;
    let w = (ink + 8).max(1);
    let h = (layout.px * 1.45).ceil().max(1.0) as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    text::draw_centred(&mut rgba, w, h, &layout, crate::switch_style::ink());
    UndoFace { rgba, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(face: &UndoFace, x: u32, y: u32) -> u8 {
        face.rgba[((y * face.w + x) * 4 + 3) as usize]
    }

    fn rgb(face: &UndoFace, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
    }

    #[test]
    fn a_card_has_transparent_corners_and_an_opaque_centre() {
        let card = card_face(120, 40, false);
        assert_eq!(alpha(&card, 0, 0), 0);
        assert!(alpha(&card, 60, 20) > 250);
        assert_eq!(rgb(&card, 60, 20), [surface()[0], surface()[1], surface()[2]]);
    }

    #[test]
    fn the_selected_card_is_white() {
        let card = card_face(120, 40, true);
        assert_eq!(rgb(&card, 60, 20), [0xf2, 0xf2, 0xf2]);
    }

    #[test]
    fn a_rounded_fill_stays_inside_its_bounds() {
        let mut face = UndoFace {
            rgba: vec![0u8; 40 * 40 * 4],
            w: 40,
            h: 40,
        };
        fill_round(&mut face, 4.0, 4.0, 32.0, 32.0, 8.0, [255, 0, 0, 255]);
        assert_eq!(alpha(&face, 0, 0), 0);
        assert_eq!(alpha(&face, 20, 20), 255);
        assert_eq!(alpha(&face, 39, 39), 0);
    }
}
