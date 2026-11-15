use slot_i18n::Language;

use std::path::Path;

use slot_gfx::{Draw, TexId, OUT_H, OUT_W};

use crate::art;
use crate::collection_style;
use crate::plate::UndoFace;
use crate::slot_chrome::slot_plate_mask;
use crate::switch_style as sw;
use crate::text;

const SCRIM: f32 = 0.55;
const WALLPAPER_SCRIM: f32 = 0.66;

/// Background identity of the currently selected cartridge rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    Recents,
    Favorites,
    Gba,
    Gb,
    Gbc,
    /// Famicom / NES. Appended last, like every variant before it: `index` is a
    /// slot in the app's cached overlay textures, so a row inserted in the
    /// middle would repaint the wrong shelf.
    Fc,
    /// Super Famicom / SNES, appended for the same reason.
    Sfc,
    /// Mega Drive / Genesis, appended for the same reason.
    Md,
}

impl CollectionKind {
    pub const ALL: [CollectionKind; 8] = [
        Self::Recents,
        Self::Favorites,
        Self::Gba,
        Self::Gb,
        Self::Gbc,
        Self::Fc,
        Self::Sfc,
        Self::Md,
    ];

    /// Stable slot used by the app's cached overlay textures.
    pub fn index(self) -> usize {
        match self {
            Self::Recents => 0,
            Self::Favorites => 1,
            Self::Gba => 2,
            Self::Gb => 3,
            Self::Gbc => 4,
            Self::Fc => 5,
            Self::Sfc => 6,
            Self::Md => 7,
        }
    }

    fn tint_alpha(self) -> f32 {
        match self {
            Self::Recents => 0.055,
            Self::Favorites => 0.075,
            Self::Gba => 0.080,
            Self::Gb => 0.060,
            Self::Gbc => 0.065,
            Self::Fc => 0.070,
            Self::Sfc => 0.068,
            Self::Md => 0.070,
        }
    }

    /// What the nameplate inside the slot says. Machine racks use the
    /// machine's own wordmark, exactly as the upstream card art does; the two
    /// extra racks keep their Chinese names because they are not machines.
    pub fn plate_name(self, lang: Language) -> &'static str {
        match self {
            Self::Recents => match lang {
                Language::ZhCn => "最近游玩",
                Language::EnUs => "RECENTS",
            },
            Self::Favorites => match lang {
                Language::ZhCn => "收藏",
                Language::EnUs => "FAVORITES",
            },
            Self::Gba => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.gba"),
                Language::EnUs => "GAME BOY ADVANCE",
            },
            Self::Gb => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.gb"),
                Language::EnUs => "GAME BOY",
            },
            Self::Gbc => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.gbc"),
                Language::EnUs => "GAME BOY COLOR",
            },
            Self::Fc => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.fc"),
                Language::EnUs => "FAMILY COMPUTER",
            },
            Self::Sfc => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.sfc"),
                Language::EnUs => "SUPER FAMICOM",
            },
            Self::Md => match lang {
                Language::ZhCn => slot_i18n::t(lang, "device.app.platform.md"),
                Language::EnUs => "MEGA DRIVE",
            },
        }
    }

    /// The short mark etched into the slot floor.  Machine racks use the
    /// wordmark's short form; the two non-machine racks keep their Chinese
    /// label because that is what the rack is called.
    pub fn plate_code(self, lang: Language) -> &'static str {
        match self {
            Self::Recents => match lang {
                Language::ZhCn => "最近游玩",
                Language::EnUs => "RECENTS",
            },
            Self::Favorites => match lang {
                Language::ZhCn => "收藏",
                Language::EnUs => "FAVORITES",
            },
            Self::Gba => "GBA",
            Self::Gb => "GB",
            Self::Gbc => "GBC",
            Self::Fc => "FC",
            Self::Sfc => "SFC",
            Self::Md => "MD",
        }
    }

    /// The card's optional nameplate for this rack, following upstream's
    /// `System/linkSlot/assets/carts/slot_back_<machine>.png` convention.
    pub fn slot_back_file(self) -> &'static str {
        match self {
            Self::Recents => "slot_back_recents.png",
            Self::Favorites => "slot_back_favorites.png",
            Self::Gba => "slot_back_gba.png",
            Self::Gb => "slot_back_gb.png",
            Self::Gbc => "slot_back_gbc.png",
            Self::Fc => "slot_back_fc.png",
            Self::Sfc => "slot_back_sfc.png",
            Self::Md => "slot_back_md.png",
        }
    }
}

pub fn wallpaper_face(path: &Path) -> Option<Vec<u8>> {
    art::cover(path, OUT_W, OUT_H)
}

pub fn bezel_face(path: &Path, w: u32, h: u32) -> Option<Vec<u8>> {
    art::cover(path, w, h)
}

/// The neutral, Nintendo-Switch-like shelf ground used when the card has no
/// wallpaper.  It is rasterised once at boot; the per-collection colour is
/// added as cheap translucent draw-list rectangles afterwards.
pub fn shelf_backdrop_face() -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (OUT_W * OUT_H * 4) as usize],
        w: OUT_W,
        h: OUT_H,
    };
    let top = [0x2a, 0x2a, 0x31];
    let bottom = [0x12, 0x12, 0x16];
    for y in 0..OUT_H {
        let t = y as f32 / (OUT_H.saturating_sub(1).max(1)) as f32;
        let colour = [
            lerp(top[0], bottom[0], t),
            lerp(top[1], bottom[1], t),
            lerp(top[2], bottom[2], t),
            0xff,
        ];
        for x in 0..OUT_W {
            let at = ((y * OUT_W + x) * 4) as usize;
            face.rgba[at..at + 4].copy_from_slice(&colour);
        }
    }

    // A quiet red edge and a bottom vignette keep the Switch accent without
    // painting the whole screen red.
    for y in 0..OUT_H {
        let t = (y as f32 / OUT_H as f32).powf(3.0);
        let alpha = (46.0 * t).round() as u8;
        for x in 0..OUT_W {
            let at = ((y * OUT_W + x) * 4) as usize;
            face.rgba[at] = face.rgba[at].saturating_sub(alpha / 2);
            face.rgba[at + 1] = face.rgba[at + 1].saturating_sub(alpha / 2);
            face.rgba[at + 2] = face.rgba[at + 2].saturating_sub(alpha / 2);
        }
    }
    face
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Sparse, low-contrast stars for the Favorites rack.  The texture is uploaded
/// once and laid over the tinted ground only when Favorites is selected.
pub fn favorite_stars_face() -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (OUT_W * OUT_H * 4) as usize],
        w: OUT_W,
        h: OUT_H,
    };
    const STAR_COLOUR: [u8; 3] = [0xff, 0xe2, 0x8a];
    let stars = [
        (70.0, 105.0, 13.0),
        (180.0, 292.0, 8.0),
        (292.0, 78.0, 9.0),
        (546.0, 112.0, 11.0),
        (646.0, 265.0, 8.0),
        (594.0, 382.0, 10.0),
        (82.0, 390.0, 10.0),
        (392.0, 414.0, 7.0),
    ];
    for (cx, cy, radius) in stars {
        star(&mut face, cx, cy, radius, radius * 0.43, STAR_COLOUR, 78);
    }
    face
}

/// The per-collection motif layer.  It is rasterised once, kept transparent in
/// the centre so cartridge labels stay clear, and composited over the dark
/// shelf ground.  Favorites keeps the original stars; the other collections
/// use one shared visual language with a different geometric motif.
pub fn collection_overlay_face(kind: CollectionKind) -> UndoFace {
    if kind == CollectionKind::Favorites {
        return favorite_stars_face();
    }
    let mut face = UndoFace {
        rgba: vec![0u8; (OUT_W * OUT_H * 4) as usize],
        w: OUT_W,
        h: OUT_H,
    };
    let style = match kind {
        CollectionKind::Recents => collection_style::recents(),
        CollectionKind::Favorites => collection_style::favorites(),
        CollectionKind::Gba => collection_style::platform(slot_store::Platform::Gba),
        CollectionKind::Gb => collection_style::platform(slot_store::Platform::Gb),
        CollectionKind::Gbc => collection_style::platform(slot_store::Platform::Gbc),
        CollectionKind::Fc => collection_style::platform(slot_store::Platform::Fc),
        CollectionKind::Sfc => collection_style::platform(slot_store::Platform::Sfc),
        CollectionKind::Md => collection_style::platform(slot_store::Platform::Md),
    };
    let base = style.base;
    let ink = style.on_dark;
    let tint = |alpha: u8| [base[0], base[1], base[2], alpha];
    let node = |alpha: u8| [ink[0], ink[1], ink[2], alpha];

    // One shared, very quiet Switch-like light band so the five collections
    // read as a family even when their motifs differ.
    polygon(
        &mut face,
        &[(-80.0, 140.0), (130.0, -20.0), (220.0, -20.0), (20.0, 210.0)],
        [255, 255, 255, 8],
    );

    match kind {
        CollectionKind::Gba => {
            for (x0, y0) in [(-45.0, 54.0), (510.0, 370.0)] {
                for offset in [0.0, 30.0, 60.0] {
                    polygon(
                        &mut face,
                        &[
                            (x0 + offset, y0),
                            (x0 + offset + 190.0, y0 - 54.0),
                            (x0 + offset + 230.0, y0 - 54.0),
                            (x0 + offset + 40.0, y0),
                        ],
                        tint(48),
                    );
                }
            }
            for (cx, cy) in [(102.0, 110.0), (614.0, 100.0), (650.0, 390.0), (134.0, 404.0)] {
                polygon(
                    &mut face,
                    &[(cx, cy - 14.0), (cx + 14.0, cy), (cx, cy + 14.0), (cx - 14.0, cy)],
                    node(90),
                );
            }
        }
        CollectionKind::Gb => {
            for (gx, gy) in [(38.0, 76.0), (520.0, 66.0), (566.0, 330.0), (64.0, 344.0)] {
                for i in 0..5 {
                    for j in 0..5 {
                        if (i + j) % 3 == 0 {
                            fill_rgba(
                                &mut face,
                                gx + i as f32 * 17.0,
                                gy + j as f32 * 17.0,
                                11.0,
                                11.0,
                                tint(58),
                            );
                        }
                    }
                }
            }
            for i in 0..6 {
                for j in 0..5 {
                    fill_rgba(
                        &mut face,
                        470.0 + i as f32 * 20.0,
                        170.0 + j as f32 * 20.0,
                        11.0,
                        11.0,
                        tint(18),
                    );
                }
            }
        }
        CollectionKind::Gbc => {
            for (cx, cy, r) in [(600.0, 90.0, 92.0), (115.0, 390.0, 78.0)] {
                ring(&mut face, cx, cy, r, 3.0, tint(58));
                ring(
                    &mut face,
                    cx,
                    cy,
                    r - 24.0,
                    3.0,
                    [0x00, 0xc8, 0xff, 38],
                );
                ring(
                    &mut face,
                    cx,
                    cy,
                    r - 48.0,
                    3.0,
                    [0xff, 0x78, 0xc8, 34],
                );
            }
            for (cx, cy) in [(100.0, 90.0), (520.0, 390.0)] {
                circle(&mut face, cx, cy, 5.0, node(90));
            }
        }
        CollectionKind::Recents => {
            for (cx, cy, r) in [(610.0, 98.0, 92.0), (112.0, 382.0, 76.0)] {
                ring(&mut face, cx, cy, r, 4.0, tint(70));
                ring(&mut face, cx, cy, r - 18.0, 2.0, [255, 255, 255, 32]);
            }
            for (x, y, w, h) in [
                (42.0, 92.0, 32.0, 78.0),
                (620.0, 300.0, 28.0, 68.0),
                (96.0, 294.0, 24.0, 54.0),
            ] {
                sw::fill_round(&mut face, x, y, w, h, 8.0, tint(44));
            }
        }
        CollectionKind::Sfc => {
            // The Super Famicom's violet-grey switch panel: three quiet bands
            // with the same clearance for the selected cart as the FC motif.
            for (i, y) in [0.0f32, 34.0, 68.0].into_iter().enumerate() {
                let alpha = 60 - i as u8 * 14;
                sw::fill_round(&mut face, 40.0, 150.0 + y, 150.0, 14.0, 7.0, tint(alpha));
                sw::fill_round(&mut face, 530.0, 330.0 - y, 150.0, 14.0, 7.0, tint(alpha));
            }
            for (cx, cy) in [(120.0, 360.0), (600.0, 120.0)] {
                circle(&mut face, cx, cy, 46.0, node(46));
                circle(&mut face, cx, cy, 30.0, tint(60));
            }
        }
        CollectionKind::Md => {
            // Mega Drive: a charcoal console strip with the red power accent.
            sw::fill_round(&mut face, 36.0, 150.0, 170.0, 20.0, 6.0, tint(58));
            sw::fill_round(&mut face, 36.0, 182.0, 92.0, 8.0, 4.0, tint(42));
            sw::fill_round(&mut face, 520.0, 300.0, 170.0, 20.0, 6.0, tint(52));
            sw::fill_round(&mut face, 598.0, 332.0, 92.0, 8.0, 4.0, tint(38));
            circle(&mut face, 600.0, 120.0, 42.0, node(42));
            circle(&mut face, 600.0, 120.0, 27.0, tint(58));
        }
        CollectionKind::Fc => {
            // A Famicom cassette's face: three moulded ribs across the top of a
            // horizontal cart, with a scatter of 8-bit pixels under them. It
            // has to read as a different machine beside the Advance's slanted
            // bands and the Game Boy's dot grid, not as a fifth shade of them.
            //
            // ⚠️ Nothing may enter x 220..450, y 140..330: that is where the
            // selected cart is drawn (`every_collection_has_a_sparse_overlay_
            // with_a_clear_centre` is the gate), so the ribs stop short of it.
            for offset in [0.0, 26.0, 52.0] {
                sw::fill_round(&mut face, 40.0, 96.0 + offset, 150.0, 12.0, 6.0, tint(40));
                sw::fill_round(&mut face, 508.0, 300.0 + offset, 178.0, 12.0, 6.0, tint(30));
            }
            for (px, py) in [
                (96.0, 250.0),
                (124.0, 250.0),
                (152.0, 250.0),
                (124.0, 278.0),
                (572.0, 150.0),
                (600.0, 150.0),
                (600.0, 178.0),
            ] {
                fill_rgba(&mut face, px, py, 16.0, 16.0, node(80));
            }
        }
        CollectionKind::Favorites => unreachable!(),
    }
    face
}

fn fill_rgba(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, colour: [u8; 4]) {
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let x1 = (x + w).round().min(face.w as f32) as u32;
    let y1 = (y + h).round().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            blend(face, px, py, colour);
        }
    }
}

fn circle(face: &mut UndoFace, cx: f32, cy: f32, r: f32, colour: [u8; 4]) {
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = (cx + r).ceil().min(face.w as f32) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = (cy + r).ceil().min(face.h as f32) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r * r {
                blend(face, x, y, colour);
            }
        }
    }
}

fn ring(face: &mut UndoFace, cx: f32, cy: f32, r: f32, width: f32, colour: [u8; 4]) {
    let x0 = (cx - r - width).floor().max(0.0) as u32;
    let x1 = (cx + r + width).ceil().min(face.w as f32) as u32;
    let y0 = (cy - r - width).floor().max(0.0) as u32;
    let y1 = (cy + r + width).ceil().min(face.h as f32) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            if (distance - r).abs() <= width / 2.0 {
                blend(face, x, y, colour);
            }
        }
    }
}

/// The fallback for a card with no `slot_back_*.png`: an etched lockup on the
/// dark slot floor, not a plaque.  Keep the canvas at the trough aspect so the
/// text does not letterbox when the front plastic masks it.
pub const SLOT_PLATE_W: u32 = 508;
pub const SLOT_PLATE_H: u32 = 74;

pub fn collection_plate_face(kind: CollectionKind, lang: Language) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (SLOT_PLATE_W * SLOT_PLATE_H * 4) as usize],
        w: SLOT_PLATE_W,
        h: SLOT_PLATE_H,
    };

    // One large mark, centred in the trough.  There is deliberately no small
    // second line: the rack name is already the shelf's context, and the
    // trough is only 37px high on the device.
    let main = text_mask(
        SLOT_PLATE_W,
        66,
        kind.plate_code(lang),
        SLOT_PLATE_W as f32 - 30.0,
        54.0,
        22.0,
    );
    paint_engraved(
        &mut face,
        &main,
        0,
        4,
        [102, 130, 142],
        [148, 177, 187],
        [3, 6, 8],
    );

    mask_plate(&mut face);
    face
}

fn text_mask(w: u32, h: u32, value: &str, max_w: f32, px: f32, min_px: f32) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (w * h * 4) as usize],
        w,
        h,
    };
    let Some(font) = text::label_font() else {
        return face;
    };
    let layout = text::fit(font, value, max_w, 1, px, min_px);
    text::draw_centred(&mut face.rgba, w, h, &layout, [255, 255, 255]);
    face
}

/// Paint a white text coverage mask as an engraved bevel: a dark lower-right
/// edge, a cool upper-left highlight, then the mid-tone face.
fn paint_engraved(
    dst: &mut UndoFace,
    mask: &UndoFace,
    x: u32,
    y: u32,
    main: [u8; 3],
    highlight: [u8; 3],
    shadow: [u8; 3],
) {
    paint_mask(dst, mask, x as i32 + 1, y as i32 + 1, shadow, 210);
    paint_mask(dst, mask, x as i32 - 1, y as i32 - 1, highlight, 130);
    paint_mask(dst, mask, x as i32, y as i32, main, 255);
}

fn paint_mask(dst: &mut UndoFace, mask: &UndoFace, x: i32, y: i32, colour: [u8; 3], alpha: u32) {
    for my in 0..mask.h {
        let dy = y + my as i32;
        if dy < 0 || dy >= dst.h as i32 {
            continue;
        }
        for mx in 0..mask.w {
            let dx = x + mx as i32;
            if dx < 0 || dx >= dst.w as i32 {
                continue;
            }
            let ma = mask.rgba[((my * mask.w + mx) * 4 + 3) as usize] as u32;
            if ma == 0 {
                continue;
            }
            let a = ma * alpha / 255;
            if a == 0 {
                continue;
            }
            let at = ((dy as u32 * dst.w + dx as u32) * 4) as usize;
            let da = dst.rgba[at + 3] as u32;
            let out = a + da * (255 - a) / 255;
            if out == 0 {
                continue;
            }
            for c in 0..3 {
                dst.rgba[at + c] = ((colour[c] as u32 * a
                    + dst.rgba[at + c] as u32 * da * (255 - a) / 255)
                    / out) as u8;
            }
            dst.rgba[at + 3] = out as u8;
        }
    }
}

fn mask_plate(face: &mut UndoFace) {
    let mask = slot_plate_mask(face.w, face.h);
    for (px, m) in face.rgba.chunks_exact_mut(4).zip(mask) {
        px[3] = (px[3] as u32 * m as u32 / 255) as u8;
    }
}

/// Fit a card's slot_back_*.png into the nameplate canvas and mask it to the
/// trough. contain, not cover: a machine wordmark must not lose its ends.
pub fn slot_plate_from_art(art: &[u8], aw: u32, ah: u32) -> UndoFace {
    slot_plate_from_art_inset(art, aw, ah, 0.0)
}

/// Like [`slot_plate_from_art`], but leaves `inset` transparent pixels on each
/// side before fitting.  The Recents/Favorites plaques carry their icon in the
/// left edge; at full width that icon falls under the slot mask's curved
/// corners.  Insets pull the artwork inward without touching the machine
/// wordmarks, which have no left-edge icon.
pub fn slot_plate_from_art_inset(art: &[u8], aw: u32, ah: u32, inset: f32) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (SLOT_PLATE_W * SLOT_PLATE_H * 4) as usize],
        w: SLOT_PLATE_W,
        h: SLOT_PLATE_H,
    };
    if aw == 0 || ah == 0 || art.len() < (aw as usize * ah as usize * 4) {
        return face;
    }
    let inner_w = (SLOT_PLATE_W as f32 - inset * 2.0).max(1.0);
    let scale = (inner_w / aw as f32).min(SLOT_PLATE_H as f32 / ah as f32);
    let dw = (aw as f32 * scale).round().max(1.0) as u32;
    let dh = (ah as f32 * scale).round().max(1.0) as u32;
    let ox = (SLOT_PLATE_W - dw) / 2;
    let oy = (SLOT_PLATE_H - dh) / 2;
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * ah as f32 / dh as f32) as u32;
        let sy = sy.min(ah - 1);
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * aw as f32 / dw as f32) as u32;
            let sx = sx.min(aw - 1);
            let src = ((sy * aw + sx) * 4) as usize;
            let dst = (((oy + y) * SLOT_PLATE_W + ox + x) * 4) as usize;
            face.rgba[dst..dst + 4].copy_from_slice(&art[src..src + 4]);
        }
    }
    mask_plate(&mut face);
    face
}

fn polygon(face: &mut UndoFace, points: &[(f32, f32)], colour: [u8; 4]) {
    let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min).max(0.0) as u32;
    let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max).min(face.w as f32) as u32;
    let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min).max(0.0) as u32;
    let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max).min(face.h as f32) as u32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            if inside_any(x as f32 + 0.5, y as f32 + 0.5, points) {
                blend(face, x, y, colour);
            }
        }
    }
}

fn inside_any(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = points.len() - 1;
    for current in 0..points.len() {
        let (x1, y1) = points[current];
        let (x2, y2) = points[previous];
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn blend(face: &mut UndoFace, x: u32, y: u32, colour: [u8; 4]) {
    if x >= face.w || y >= face.h || colour[3] == 0 {
        return;
    }
    let at = ((y * face.w + x) * 4) as usize;
    let src = colour[3] as f32 / 255.0;
    let dst = face.rgba[at + 3] as f32 / 255.0;
    let out = src + dst * (1.0 - src);
    if out <= 0.0 {
        return;
    }
    for (channel, &src_byte) in colour.iter().take(3).enumerate() {
        let src_c = src_byte as f32 / 255.0;
        let dst_c = face.rgba[at + channel] as f32 / 255.0;
        face.rgba[at + channel] =
            (((src_c * src + dst_c * dst * (1.0 - src)) / out) * 255.0).round() as u8;
    }
    face.rgba[at + 3] = (out * 255.0).round() as u8;
}

fn star(
    face: &mut UndoFace,
    cx: f32,
    cy: f32,
    outer: f32,
    inner: f32,
    colour: [u8; 3],
    alpha: u8,
) {
    let mut points = [(0.0f32, 0.0f32); 10];
    for (index, point) in points.iter_mut().enumerate() {
        let radius = if index % 2 == 0 { outer } else { inner };
        let angle = -std::f32::consts::FRAC_PI_2
            + index as f32 * std::f32::consts::PI / 5.0;
        *point = (
            cx + radius * angle.cos(),
            cy + radius * angle.sin(),
        );
    }
    let min_x = (cx - outer).floor().max(0.0) as u32;
    let max_x = (cx + outer).ceil().min(face.w as f32) as u32;
    let min_y = (cy - outer).floor().max(0.0) as u32;
    let max_y = (cy + outer).ceil().min(face.h as f32) as u32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            if !inside_polygon(x as f32 + 0.5, y as f32 + 0.5, &points) {
                continue;
            }
            let at = ((y * face.w + x) * 4) as usize;
            face.rgba[at] = colour[0];
            face.rgba[at + 1] = colour[1];
            face.rgba[at + 2] = colour[2];
            face.rgba[at + 3] = alpha;
        }
    }
}

fn inside_polygon(x: f32, y: f32, points: &[(f32, f32); 10]) -> bool {
    let mut inside = false;
    let mut previous = 9;
    for current in 0..10 {
        let (x1, y1) = points[current];
        let (x2, y2) = points[previous];
        if (y1 > y) != (y2 > y)
            && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

/// Draw the shelf ground and the current collection's subtle colour wash.
///
/// Wallpapers remain supported: when one is supplied it replaces the generated
/// ground but still sits under a dark Switch-style scrim and the same accent
/// bars, so the shelf never reverts to a bright photo background.
pub fn draw_backdrop(
    wallpaper: Option<TexId>,
    base: Option<TexId>,
    accent: [u8; 3],
    kind: CollectionKind,
    overlay: Option<TexId>,
    out: &mut Vec<Draw>,
) {
    match wallpaper.or(base) {
        Some(tex) => out.push(Draw::Tex {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            tex,
            alpha: 1.0,
        }),
        None => out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.07, 0.07, 0.09, 1.0],
        }),
    }
    out.push(Draw::Rect {
        x: 0.0,
        y: 0.0,
        w: OUT_W as f32,
        h: OUT_H as f32,
        colour: [
            0.0,
            0.0,
            0.0,
            if wallpaper.is_some() {
                WALLPAPER_SCRIM
            } else {
                SCRIM
            },
        ],
    });

    let accent = [
        accent[0] as f32 / 255.0,
        accent[1] as f32 / 255.0,
        accent[2] as f32 / 255.0,
    ];
    // Very light full-screen identity wash.  It keeps the black Switch ground
    // readable and never turns the whole screen into a saturated theme.
    out.push(Draw::Rect {
        x: 0.0,
        y: 0.0,
        w: OUT_W as f32,
        h: OUT_H as f32,
        colour: [
            accent[0],
            accent[1],
            accent[2],
            kind.tint_alpha() * if wallpaper.is_some() { 0.65 } else { 1.0 },
        ],
    });
    // A slightly stronger edge glow gives the collection an ambient identity
    // without covering the cartridge labels in the centre.
    out.push(Draw::Rect {
        x: 0.0,
        y: OUT_H as f32 - 92.0,
        w: OUT_W as f32,
        h: 92.0,
        colour: [accent[0], accent[1], accent[2], 0.045],
    });
    if wallpaper.is_none() {
        if let Some(tex) = overlay {
            out.push(Draw::Tex {
                x: 0.0,
                y: 0.0,
                w: OUT_W as f32,
                h: OUT_H as f32,
                tex,
                alpha: 0.90,
            });
        }
    }
}
