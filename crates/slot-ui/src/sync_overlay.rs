//! Procedural art for the v0.6 shelf sync overlay: the cloud the cartridge
//! talks to, the transfer arrows and one-line status text. Everything is
//! rastered here, so the overlay needs no extra image assets.

use crate::hud::HUD_INK;
use crate::plate::UndoFace;
use crate::text;
use crate::CartFace;

/// Where a transfer arrow points: up to the cloud (upload) or down to the
/// cartridge (download).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Arrow {
    Up,
    Down,
}

/// The shape of a cloud: roughly one and a half times as wide as it is tall.
const CLOUD_ASPECT: f32 = 1.5;
/// Supersampling grid per pixel, so the round outlines stay smooth.
const SAMPLES: u32 = 4;
/// Status lines never shrink below this, however little room they are given.
const MIN_PX: f32 = 12.0;

/// The cloud the cartridge syncs with. `px` is the cloud's height; the face
/// comes out [`CLOUD_ASPECT`] times as wide.
pub fn cloud_face(px: f32) -> CartFace {
    let h = px.round().max(1.0) as u32;
    let w = (px * CLOUD_ASPECT).round().max(1.0) as u32;
    let rgba = raster(w, h, cloud);
    CartFace { rgba, w, h }
}

/// A transfer arrow in a square box `px` on a side. Up points at the cloud,
/// down at the cartridge.
pub fn arrow_face(dir: Arrow, px: f32) -> CartFace {
    let side = px.round().max(1.0) as u32;
    let rgba = raster(side, side, |u, v| arrow(dir, u, v));
    CartFace {
        rgba,
        w: side,
        h: side,
    }
}

/// One line of overlay text, shrunk to fit `max_w` (never below 12 px) and
/// trimmed to the ink so the caller can centre it by the face's own size.
pub fn line_face(text: &str, max_w: f32, px: f32) -> UndoFace {
    let Some(font) = text::label_font() else {
        return empty();
    };
    if text.is_empty() || max_w < 1.0 {
        return empty();
    }
    let box_w = max_w.floor() as u32;
    let layout = text::fit(font, text, box_w as f32, 1, px.max(MIN_PX), MIN_PX);
    let box_h = font
        .horizontal_line_metrics(layout.px)
        .map_or((layout.px * 1.4).ceil(), |m| m.new_line_size.ceil())
        .max(1.0) as u32;
    let mut rgba = vec![0u8; (box_w * box_h * 4) as usize];
    text::draw_centred(&mut rgba, box_w, box_h, &layout, HUD_INK);
    trim(rgba, box_w, box_h)
}

fn empty() -> UndoFace {
    UndoFace {
        rgba: Vec::new(),
        w: 0,
        h: 0,
    }
}

/// A cloud built from overlapping round bumps over an elliptical base, in
/// unit coordinates where the shape should fill the box.
fn cloud(u: f32, v: f32) -> bool {
    let round = |cx: f32, cy: f32, rx: f32, ry: f32| {
        let dx = (u - cx) / rx;
        let dy = (v - cy) / ry;
        dx * dx + dy * dy <= 1.0
    };
    round(0.50, 0.76, 0.47, 0.24)
        || round(0.50, 0.28, 0.28, 0.28)
        || round(0.27, 0.52, 0.20, 0.20)
        || round(0.75, 0.54, 0.17, 0.17)
}

/// A triangular head on a straight shaft, mirrored for the two directions.
fn arrow(dir: Arrow, u: f32, v: f32) -> bool {
    let v = match dir {
        Arrow::Up => v,
        Arrow::Down => 1.0 - v,
    };
    let (apex, base, half) = (0.06, 0.46, 0.38);
    let head = v >= apex && v <= base && (u - 0.5).abs() <= half * (v - apex) / (base - apex);
    let shaft = (0.40..=0.94).contains(&v) && (u - 0.5).abs() <= 0.115;
    head || shaft
}

/// Fill `w` by `h` with `ink`, testing `inside` on a supersampled grid so the
/// curved outlines are antialiased.
fn raster(w: u32, h: u32, inside: impl Fn(f32, f32) -> bool) -> Vec<u8> {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let step = 1.0 / SAMPLES as f32;
    let samples = SAMPLES * SAMPLES;
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                let v = (y as f32 + (sy as f32 + 0.5) * step) / h as f32;
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) * step) / w as f32;
                    if inside(u, v) {
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                continue;
            }
            let at = ((y * w + x) * 4) as usize;
            rgba[at..at + 3].copy_from_slice(&HUD_INK);
            rgba[at + 3] = (hits * 255 / samples) as u8;
        }
    }
    rgba
}

/// Crop a face down to its inked pixels, the way `rebuild` trims its lines.
fn trim(rgba: Vec<u8>, w: u32, h: u32) -> UndoFace {
    let inked = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize] > 0;
    let rows: Vec<u32> = (0..h).filter(|&y| (0..w).any(|x| inked(x, y))).collect();
    let cols: Vec<u32> = (0..w).filter(|&x| (0..h).any(|y| inked(x, y))).collect();
    let (Some(&top), Some(&bottom), Some(&left), Some(&right)) =
        (rows.first(), rows.last(), cols.first(), cols.last())
    else {
        return empty();
    };
    let (tw, th) = (right - left + 1, bottom - top + 1);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in top..=bottom {
        let at = ((y * w + left) * 4) as usize;
        out.extend_from_slice(&rgba[at..at + (tw * 4) as usize]);
    }
    UndoFace {
        rgba: out,
        w: tw,
        h: th,
    }
}
