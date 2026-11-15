use std::path::Path;

use slot_gfx::OUT_W;
use slot_store::{Cart, Core, Platform};

use crate::art;
use crate::cart::{clean_label, CartFace, CART_H, CART_W};
use crate::shelf::foot_y;
use crate::shell::shell_for;
use crate::slot_chrome::ease;
use crate::text;

const BOARD_SVG: &str = include_str!("../assets/board.svg");
const BOARD_GB_SVG: &str = include_str!("../assets/board_gb.svg");
const BOARD_GBC_SVG: &str = include_str!("../assets/board_gbc.svg");
const BOARD_FC_SVG: &str = include_str!("../assets/board_fc.svg");
const BOARD_MD_SVG: &str = include_str!("../assets/board_md.svg");

pub const BOARD_W: u32 = 372;
pub const BOARD_H: u32 = 209;

/// The MD core is drawn smaller than the shared socket art: its PCB keeps a
/// large central recess for the game-name marking instead.
pub fn core_chip_scale(platform: Platform) -> f32 {
    match platform {
        Platform::Md => 0.62,
        _ => 1.0,
    }
}

/// The board canvas for each platform. GB is a portrait board, the others
/// keep the GBA-shaped landscape board.
pub fn board_size(platform: Platform) -> (u32, u32) {
    match platform {
        Platform::Gb | Platform::Gbc => (300, 342),
        Platform::Gba => (BOARD_W, BOARD_H),
        // The FC shell grew to 1.20 with its cart, so its board did too.
        Platform::Fc => (BOARD_W, 310),
        // SFC is a landscape cassette like the Famicom's.
        Platform::Sfc => (BOARD_W, 310),
        // MD is a shorter, wider cassette; its PCB keeps the same width.
        Platform::Md => (BOARD_W, 252),
    }
}

/// Board-local units, matching each SVG's viewBox. GB is 240 x 273.6.
pub fn board_units(platform: Platform) -> (f32, f32) {
    match platform {
        Platform::Gb | Platform::Gbc => (240.0, 273.6),
        Platform::Gba => (240.0, 135.0),
        Platform::Fc => (240.0, 200.0),
        Platform::Sfc => (240.0, 200.0),
        Platform::Md => (240.0, 163.0),
    }
}

pub const ROM_X: u32 = 67;
pub const ROM_Y: u32 = 48;
pub const ROM_W: u32 = 71;
pub const ROM_H: u32 = 90;

/// Where the game name sits on each board. GB keeps the name in the middle
/// of its portrait PCB; the other boards keep the GBA corner.
fn rom_marking_at(platform: Platform) -> (u32, u32) {
    match platform {
        Platform::Gb => (114, 130),
        Platform::Gba => (ROM_X, ROM_Y),
        Platform::Fc => (157, 143),
        Platform::Sfc => (157, 143),
        Platform::Md => (98, 68),
        Platform::Gbc => (114, 130),
    }
}

const PLASTIC: &str = "#ff00ff";
const FLOOR: &str = "#800080";
const DEEP: &str = "#400040";
// GB/GBC/FC board art separates the cartridge shell from the green PCB.
const SHELL: &str = "#fe01fe";
const SHELL_MID: &str = "#fe02fe";
const SHELL_DEEP: &str = "#fe03fe";

const MARK_LINE_CHARS: usize = 10;
const MARK_LINES: usize = 3;
const MARK_PAD: u32 = 4;
const MARK_PX: f32 = 10.0;
const MARK_MIN_PX: f32 = 6.0;
const MARK_INK: [u8; 3] = [0xbd, 0xbd, 0xbd];

pub fn rom_marking(stem: &str) -> Vec<String> {
    let mut title: Vec<String> = Vec::new();
    for word in clean_label(stem).to_uppercase().split_whitespace() {
        match title.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= MARK_LINE_CHARS => {
                line.push(' ');
                line.push_str(word);
            }
            _ => title.push(word.to_string()),
        }
    }
    if title.len() > MARK_LINES {
        let tail = title.split_off(MARK_LINES - 1).join(" ");
        title.push(tail);
    }
    title
}

pub fn rom_marking_face(stem: &str) -> CartFace {
    let title = rom_marking(stem);
    let mut face = CartFace {
        rgba: vec![0; (ROM_W * ROM_H * 4) as usize],
        w: ROM_W,
        h: ROM_H,
    };
    let Some(font) = text::label_font() else {
        return face;
    };
    let max_w = (ROM_W - 2 * MARK_PAD) as f32;
    let line_h = (MARK_PX * 1.25).ceil() as u32;
    let mut top = ROM_H.saturating_sub(line_h * title.len() as u32) / 2;
    for line in &title {
        let layout = text::fit(font, line, max_w, 1, MARK_PX, MARK_MIN_PX);
        ink_band(&mut face, top, line_h, &layout, MARK_INK, 1.0);
        top += line_h;
    }
    face
}

pub fn board_face(cart: &Cart) -> CartFace {
    let shell = shell_for(cart);
    let board_svg = match cart.platform {
        Platform::Gba => BOARD_SVG,
        Platform::Gb => BOARD_GB_SVG,
        Platform::Gbc => BOARD_GBC_SVG,
        Platform::Fc => BOARD_FC_SVG,
        Platform::Sfc => BOARD_FC_SVG,
        Platform::Md => BOARD_MD_SVG,
    };
    // GBA keeps the cart shell colour. The GB/GBC/FC boards are always the
    // deep service-board green, so a red/yellow FC shell cannot turn the PCB
    // into a yellow slab.
    let pcb = match cart.platform {
        Platform::Gba => shell.colour,
        Platform::Gb | Platform::Gbc | Platform::Fc | Platform::Sfc | Platform::Md => {
            [0x2f, 0x7f, 0x31]
        }
    };
    let svg = board_svg
        .replace(DEEP, &hex(shade(pcb, 0.35)))
        .replace(FLOOR, &hex(shade(pcb, 0.62)))
        .replace(PLASTIC, &hex(pcb))
        .replace(SHELL, &hex(shell.colour))
        .replace(SHELL_MID, &hex(shade(shell.colour, 0.78)))
        .replace(SHELL_DEEP, &hex(shade(shell.colour, 0.55)));
    let (bw, bh) = board_size(cart.platform);
    let rgba = art::render_svg(&svg, bw, bh)
        .unwrap_or_else(|| vec![0; (bw * bh * 4) as usize]);
    let mut face = CartFace {
        rgba,
        w: bw,
        h: bh,
    };
    let (mx, my) = rom_marking_at(cart.platform);
    let mark = rom_marking_face(&cart.stem);
    let mark = if cart.platform == Platform::Fc {
        scaled_face(&mark, 58, 64)
    } else {
        mark
    };
    over(&mut face, &mark, mx, my);
    face
}

fn scaled_face(src: &CartFace, w: u32, h: u32) -> CartFace {
    let mut out = CartFace {
        rgba: vec![0; (w * h * 4) as usize],
        w,
        h,
    };
    for y in 0..h {
        let sy = (y * src.h / h).min(src.h.saturating_sub(1));
        for x in 0..w {
            let sx = (x * src.w / w).min(src.w.saturating_sub(1));
            let s = ((sy * src.w + sx) * 4) as usize;
            let d = ((y * w + x) * 4) as usize;
            out.rgba[d..d + 4].copy_from_slice(&src.rgba[s..s + 4]);
        }
    }
    out
}

fn ink_band(
    face: &mut CartFace,
    top: u32,
    band: u32,
    layout: &text::Layout,
    ink: [u8; 3],
    strength: f32,
) {
    let band = band.min(face.h.saturating_sub(top));
    for (i, a) in text::coverage(face.w, band, layout).into_iter().enumerate() {
        let a = (a as f32 * strength).round() as u8;
        if a == 0 {
            continue;
        }
        let at = ((top * face.w) as usize + i) * 4;
        let (a, d_a) = (a as f32 / 255.0, face.rgba[at + 3] as f32 / 255.0);
        let out_a = a + d_a * (1.0 - a);
        for (k, &c) in ink.iter().enumerate() {
            let d_rgb = face.rgba[at + k] as f32 / 255.0;
            let out_rgb = (c as f32 / 255.0 * a + d_rgb * d_a * (1.0 - a)) / out_a;
            face.rgba[at + k] = (out_rgb * 255.0).round() as u8;
        }
        face.rgba[at + 3] = (out_a * 255.0).round() as u8;
    }
}

fn over(dst: &mut CartFace, src: &CartFace, x: u32, y: u32) {
    for row in 0..src.h {
        for col in 0..src.w {
            let s = ((row * src.w + col) * 4) as usize;
            let a = src.rgba[s + 3] as u32;
            let (dx, dy) = (x + col, y + row);
            if a == 0 || dx >= dst.w || dy >= dst.h {
                continue;
            }
            let d = ((dy * dst.w + dx) * 4) as usize;
            for k in 0..3 {
                dst.rgba[d + k] =
                    ((src.rgba[s + k] as u32 * a + dst.rgba[d + k] as u32 * (255 - a)) / 255) as u8;
            }
            dst.rgba[d + 3] = dst.rgba[d + 3].max(a as u8);
        }
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn shade(c: [u8; 3], f: f32) -> [u8; 3] {
    c.map(|v| (v as f32 * f).round().clamp(0.0, 255.0) as u8)
}

const SOCKET_SVG: &str = include_str!("../assets/socket.svg");
const CHIP_SVG: &str = include_str!("../assets/chip.svg");
const CHIP_GB_SVG: &str = include_str!("../assets/chip_gb.svg");
const CHIP_GBC_SVG: &str = include_str!("../assets/chip_gbc.svg");
const CHIP_FC_SVG: &str = include_str!("../assets/chip_fc.svg");

pub const BOARD_X: f32 = 174.0;
pub const BOARD_Y: f32 = 150.0;

pub const TURN_PAD: u32 = 2;

pub const LID_TURN: f32 = -5.0 * std::f32::consts::PI / 180.0;
pub const CHIP_TIP: f32 = 4.0 * std::f32::consts::PI / 180.0;
pub const HOP_LIFT: f32 = 10.0;

pub const SLIDE_SHARE: f32 = 160.0 / 420.0;
pub const SLIDE_UP: f32 = CART_H as f32 / 3.0;

/// How far the lid slides before it lifts: a taller cartridge travels farther.
pub fn slide_up_for(platform: Platform) -> f32 {
    match platform {
        Platform::Fc => crate::cart::FC_CART_H as f32 / 3.0,
        Platform::Sfc => crate::cart::SFC_CART_H as f32 / 3.0,
        _ => SLIDE_UP,
    }
}

pub fn slide_of(progress: f32) -> f32 {
    ease((progress / SLIDE_SHARE).clamp(0.0, 1.0))
}

pub fn lift_of(progress: f32) -> f32 {
    ease(((progress - SLIDE_SHARE) / (1.0 - SLIDE_SHARE)).clamp(0.0, 1.0))
}

pub const SOCKET_U: [f32; 2] = [99.5, 171.5];
pub const SOCKET_V: f32 = 59.1;
pub const SOCKET_W: u32 = 64;
pub const SOCKET_H: u32 = 46;

pub const CHIP_U: [f32; 2] = [101.0, 173.0];
pub const CHIP_V: f32 = 60.6;
pub const CHIP_W: u32 = 59;
pub const CHIP_H: u32 = 41;

/// The chip cavity each single-core board draws for the core to live in, in
/// board units: (left, top, right, bottom).
///
/// These mirror the dark recess in `board_gb.svg`, `board_gbc.svg` and
/// `board_fc.svg`. The socket art and the chip are placed inside them, so a
/// redrawn board has to move its cavity and this table with it. SFC reuses the
/// Famicom board art and therefore the Famicom cavity.
pub const CAVITIES: [(Platform, (f32, f32, f32, f32)); 5] = [
    (Platform::Gb, (76.0, 28.0, 164.0, 104.0)),
    (Platform::Gbc, (96.0, 50.0, 144.0, 86.0)),
    (Platform::Fc, (23.0, 34.0, 79.0, 78.0)),
    (Platform::Sfc, (23.0, 34.0, 79.0, 78.0)),
    (Platform::Md, (16.0, 28.0, 64.0, 80.0)),
];

/// The socket art and the chip inside a single-core board's cavity, in board
/// units: (socket_u, socket_v, chip_u, chip_v).
///
/// The chip is the socket art's middle, the way the GBA board's two chips sit
/// in their sockets.
/// Famicom 主板（SFC 复用）上相邻核心插槽的间距，单位是板坐标。
///
/// `board_fc.svg` 画了 3 个核心插槽：x = 23.2 / 92 / 160.8，宽 56、高 44；
/// 第 1、2 个槽的间距正好 69。
pub const SFC_SLOT_PITCH: f32 = 69.0;

/// 多核心机种在 PCB 上第 `index` 个插槽的 (socket_u, socket_v, chip_u, chip_v)。
///
/// * GBA 用它自己的两个插槽（mGBA / gpSP）；
/// * SFC 用 Famicom 板上前两个插槽（snes9x / 可选的 supafaust）；
/// * 单核心机种没有"选择槽"，返回 `None`，调用方回退到 [`single_core_spot`]。
pub fn multi_core_slot(platform: Platform, index: usize) -> Option<(f32, f32, f32, f32)> {
    match platform {
        Platform::Gba => Some((
            *SOCKET_U.get(index)?,
            SOCKET_V,
            *CHIP_U.get(index)?,
            CHIP_V,
        )),
        Platform::Sfc => {
            if index >= 2 {
                return None;
            }
            let (socket_u, socket_v, chip_u, chip_v) = single_core_spot(platform);
            let shift = SFC_SLOT_PITCH * index as f32;
            Some((socket_u + shift, socket_v, chip_u + shift, chip_v))
        }
        _ => None,
    }
}

/// 芯片在两个插槽之间（`across` ∈ 0..=1）的 (chip_u, chip_v)。
pub fn chip_along(platform: Platform, across: f32) -> Option<(f32, f32)> {
    let first = multi_core_slot(platform, 0)?;
    let second = multi_core_slot(platform, 1)?;
    Some((
        first.2 + (second.2 - first.2) * across,
        first.3 + (second.3 - first.3) * across,
    ))
}
pub fn single_core_spot(platform: Platform) -> (f32, f32, f32, f32) {
    let (l, t, r, b) = CAVITIES
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, cavity)| *cavity)
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    let (socket_w, socket_h, chip_w, chip_h) = socket_units(platform);
    let socket_u = (l + r) / 2.0 - socket_w / 2.0;
    let socket_v = (t + b) / 2.0 - socket_h / 2.0;
    let chip_u = socket_u + (socket_w - chip_w) / 2.0;
    let chip_v = socket_v + (socket_h - chip_h) / 2.0;
    (socket_u, socket_v, chip_u, chip_v)
}

pub const SHADOW_W: u32 = 66;
pub const SHADOW_H: u32 = 12;

const NAME_PX: f32 = 17.0;
const NAME_MIN_PX: f32 = 8.0;
const SOCKET_INK: [u8; 3] = [0xee, 0xf5, 0xe6];
const CHIP_INK: [u8; 3] = [0xf2, 0xf2, 0xf2];


/// The GBA top-right cloud-sync module, in screen pixels.
pub const NETWORK_W: u32 = 60;
pub const NETWORK_H: u32 = 46;
/// Where that module sits in the GBA board's 240 x 135 units.
pub const NETWORK_U: f32 = 152.0;
pub const NETWORK_V: f32 = 24.0;

/// Where each platform's network module replaces its stock small chip.
pub fn network_spot(platform: Platform) -> (f32, f32) {
    match platform {
        Platform::Gba => (NETWORK_U, NETWORK_V),
        Platform::Gb => (28.0, 46.0),
        Platform::Gbc => (28.0, 178.0),
        Platform::Fc => (182.0, 108.0),
        Platform::Sfc => (182.0, 108.0),
        Platform::Md => (182.0, 36.0),
    }
}

/// A network module face with a live cloud-sync mod name. The picker can
/// request a higher-resolution face so zooming it up does not become blurry.
pub fn network_chip_face(label: Option<&str>) -> CartFace {
    network_chip_face_scaled_cover(label, 1, true, None)
}

pub fn network_chip_face_scaled(label: Option<&str>, scale: u32) -> CartFace {
    network_chip_face_scaled_cover(label, scale, true, None)
}

pub fn network_chip_face_scaled_unlit(label: Option<&str>, scale: u32) -> CartFace {
    network_chip_face_scaled_cover(label, scale, false, None)
}

pub fn network_chip_face_scaled_cover(
    label: Option<&str>,
    scale: u32,
    lit: bool,
    cover: Option<&Path>,
) -> CartFace {
    network_chip_face_scaled_impl(label, scale, lit, true, cover)
}

/// Same face with both lamps addressable: `led_lit` is the status LED on the
/// right, `mark_lit` the pin-1 mark on the left.
pub fn network_chip_face_scaled_marked(
    label: Option<&str>,
    scale: u32,
    led_lit: bool,
    mark_lit: bool,
    cover: Option<&Path>,
) -> CartFace {
    network_chip_face_scaled_impl(label, scale, led_lit, mark_lit, cover)
}

fn network_chip_face_scaled_impl(
    label: Option<&str>,
    scale: u32,
    lit: bool,
    mark_lit: bool,
    cover: Option<&Path>,
) -> CartFace {
    let scale = scale.max(1);
    let s = scale as f32;
    let w = NETWORK_W * scale;
    let h = NETWORK_H * scale;
    let mut face = CartFace {
        rgba: vec![0; (w * h * 4) as usize],
        w,
        h,
    };
    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return;
        }
        let at = ((y as u32 * w + x as u32) * 4) as usize;
        face.rgba[at..at + 4].copy_from_slice(&c);
    };

    // antenna arcs above the package
    let cx = w as f32 / 2.0;
    let antenna = if lit {
        [104, 224, 178, 255]
    } else {
        [72, 86, 80, 255]
    };
    for (radius, ink) in [
        (4.0 * s, antenna),
        (7.0 * s, [antenna[0], antenna[1], antenna[2], 210]),
        (10.0 * s, [antenna[0], antenna[1], antenna[2], 150]),
    ] {
        let mut a = 200.0_f32;
        while a <= 340.0 {
            let rad = a.to_radians();
            let x = cx + radius * rad.cos();
            let y = 10.0 * s + radius * rad.sin() * 0.55;
            put(x.round() as i32, y.round() as i32, ink);
            a += 5.0;
        }
    }
    put(w as i32 / 2 - 1, (3.0 * s) as i32, antenna);
    put(w as i32 / 2, (3.0 * s) as i32, antenna);

    // black package with a thin border
    let left = (4.0 * s) as i32;
    let right = w as i32 - left;
    let top = (9.0 * s) as i32;
    // The enlarged face is a package, not a connector: round it more and
    // drop the pin rows entirely (see below).
    let corner = if scale >= 4 { 11.0 * s } else { 7.0 * s };
    if scale >= 4 {
        // Enlarged face: a constant-width border, measured from the rounded
        // silhouette, so the four corners do not bulge out like the old
        // axis-aligned bands did.
        let edge_w = 2.0 * s;
        let cx0 = (left + right) as f32 * 0.5;
        let cy0 = (top + h as i32 - 1) as f32 * 0.5;
        let hw = (right - left) as f32 * 0.5 - corner;
        let hh = (h as i32 - 1 - top) as f32 * 0.5 - corner;
        for y in top..h as i32 {
            for x in left..right {
                let qx = (x as f32 - cx0).abs() - hw;
                let qy = (y as f32 - cy0).abs() - hh;
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                    + qx.max(qy).min(0.0)
                    - corner;
                if d > 0.0 {
                    continue;
                }
                put(
                    x,
                    y,
                    if d > -edge_w {
                        [55, 55, 58, 255]
                    } else {
                        [20, 20, 21, 255]
                    },
                );
            }
        }
    } else {
        for y in top..h as i32 {
            for x in left..right {
                let nx = x - left;
                let ny = y - top;
                let ex = (right - 1 - x).min(nx) as f32;
                let ey = (h as i32 - 1 - y).min(ny) as f32;
                if ex < corner && ey < corner {
                    let dx = corner - ex;
                    let dy = corner - ey;
                    if dx * dx + dy * dy > corner * corner {
                        continue;
                    }
                }
                let edge = x < left + (2.0 * s) as i32
                    || x > right - (2.0 * s) as i32
                    || y < top + (2.0 * s) as i32
                    || y > h as i32 - (2.0 * s) as i32;
                put(x, y, if edge { [55, 55, 58, 255] } else { [20, 20, 21, 255] });
            }
        }
    }
    // pin rows: only the small board chip wears them; the enlarged face is
    // shown as a bare package.
    if scale < 4 {
        for k in 0..=6 {
            let x = (8.0 + 7.0 * k as f32) * s;
            let x = x.round() as i32;
            for step in 0..3 {
                put(x, (6.0 * s) as i32 + step, [176, 176, 179, 255]);
                put(x, h as i32 - (3.0 * s) as i32 + step, [176, 176, 179, 255]);
            }
        }
    }
    // The sticker keeps a uniform margin inside the package edge; the package
    // border itself is the only ring drawn.
    let ring = 3.2 * s;
    drop(put);
    if let Some(cover) = cover {
        // Enlarged faces keep a margin and round the cover to match; the small
        // board chip keeps its old inset and silhouette exactly.
        let zoomed = scale >= 4;
        let (x0, y0, iw, ih) = if zoomed {
            let inset = ring as u32;
            (
                left as u32 + inset,
                top as u32 + inset,
                (right as u32 - left as u32).saturating_sub(2 * inset),
                (h as u32 - top as u32).saturating_sub(2 * inset),
            )
        } else {
            let ix = (11.0 * s) as u32;
            let iy = (12.0 * s) as u32;
            (
                ix,
                iy,
                w.saturating_sub(2 * ix),
                h.saturating_sub(iy + (10.0 * s) as u32),
            )
        };
        if let Some(src) = crate::art::cover(cover, iw, ih) {
            // A rectangular cover would poke out of the rounded corners, so
            // clear everything outside the silhouette before compositing.
            let (l, r, t, b, rad) = if zoomed {
                let l = x0 as f32;
                let r = (x0 + iw) as f32 - 1.0;
                let t = y0 as f32;
                let b = (y0 + ih) as f32 - 1.0;
                let inset = (x0 as f32 - left as f32).max(0.0);
                (l, r, t, b, (corner - inset).max(2.0))
            } else {
                (
                    left as f32,
                    right as f32,
                    top as f32,
                    h as f32 - 1.0,
                    corner,
                )
            };
            let half_w = (r - l) * 0.5 - rad;
            let half_h = (b - t) * 0.5 - rad;
            let cx = (l + r) * 0.5;
            let cy = (t + b) * 0.5;
            for py in 0..ih {
                for px in 0..iw {
                    // Skip everything outside the rounded cover instead of
                    // clearing it: that area must keep showing the package.
                    let x = (x0 + px) as f32 - cx;
                    let y = (y0 + py) as f32 - cy;
                    let qx = x.abs() - half_w;
                    let qy = y.abs() - half_h;
                    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                        + qx.max(qy).min(0.0)
                        - rad;
                    if outside > 0.0 {
                        continue;
                    }
                    let si = ((py * iw + px) * 4) as usize;
                    let a = src[si + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    let di = (((y0 + py) * w + x0 + px) * 4) as usize;
                    for k in 0..3 {
                        face.rgba[di + k] = ((src[si + k] as u32 * a
                            + face.rgba[di + k] as u32 * (255 - a))
                            / 255) as u8;
                    }
                    face.rgba[di + 3] = 255;
                }
            }
            let mut mark = |x: i32, y: i32, c: [u8; 4]| {
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    return;
                }
                let at = ((y as u32 * w + x as u32) * 4) as usize;
                face.rgba[at..at + 4].copy_from_slice(&c);
            };
            let dot = (2.0 * s).max(2.0) as i32;
            for yy in 0..dot {
                for xx in 0..dot {
                    mark(
                        (8.6 * s) as i32 + xx,
                        (15.6 * s) as i32 + yy,
                        if mark_lit {
                            [91, 231, 155, 255]
                        } else {
                            [40, 42, 44, 255]
                        },
                    );
                    mark(
                        (12.6 * s) as i32 + xx,
                        (15.6 * s) as i32 + yy,
                        if lit { [91, 231, 155, 255] } else { [38, 45, 42, 255] },
                    );
                }
            }
            return face;
        }
    }

    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return;
        }
        let at = ((y as u32 * w + x as u32) * 4) as usize;
        face.rgba[at..at + 4].copy_from_slice(&c);
    };
    let name = label.filter(|s| !s.trim().is_empty()).unwrap_or("云存档");
    if scale >= 4 {
        // Chip-style top marking: pin-1 dot, LED, model, capability icon,
        // laser-etched mod name, revision/date and a tiny 2D trace code.
        // The face keeps a margin: a thin moulding line runs just inside the
        // package edge, and every marking sits inside it.
        // A faint top bevel, clipped to the straight run of the top edge.
        let bevel = (1.0 * s) as i32;
        let bx0 = (left as f32 + corner) as i32;
        let bx1 = (right as f32 - corner) as i32;
        for y in (top as f32 + 0.8 * s) as i32..((top as f32 + 0.8 * s) as i32 + bevel) {
            for x in bx0..bx1 {
                put(x, y, [62, 62, 66, 255]);
            }
        }

        let dot = (2.0 * s).max(2.0) as i32;
        for yy in 0..dot {
            for xx in 0..dot {
                put(
                    (8.6 * s) as i32 + xx,
                    (15.6 * s) as i32 + yy,
                    if mark_lit {
                        [91, 231, 155, 255]
                    } else {
                        [40, 42, 44, 255]
                    },
                );
                put(
                    (12.6 * s) as i32 + xx,
                    (15.6 * s) as i32 + yy,
                    if lit { [91, 231, 155, 255] } else { [38, 45, 42, 255] },
                );
            }
        }

        // capability icon: cloud + sync arrows. It sits on the top-right
        // corner arc, pulled inward by the corner radius so neither the cloud
        // nor its arc crosses the package edge.
        // 5.2 units covers the widest part (the arrow arc), and the icon is
        // drawn at 0.72 of the small-chip size so it stays compact.
        let icon_half = 5.2 * s;
        let icon_k = 0.72;
        let gap = 2.2 * s;
        let pull = ((corner - ring) - icon_half - gap).max(0.0)
            * std::f32::consts::FRAC_1_SQRT_2;
        let icx = (right as f32 - corner) + pull;
        let icy = (top as f32 + corner) - pull;
        for (dx, dy, rr) in [(-2.0, 0.0, 2.4), (0.0, -1.8, 3.2), (2.4, 0.0, 2.2)] {
            let ccx = icx + dx * icon_k * s;
            let ccy = icy + dy * icon_k * s;
            let cr = rr * icon_k * s;
            for yy in (ccy - cr) as i32..=(ccy + cr) as i32 {
                for xx in (ccx - cr) as i32..=(ccx + cr) as i32 {
                    let dx = xx as f32 - ccx;
                    let dy = yy as f32 - ccy;
                    if dx * dx + dy * dy <= cr * cr {
                        put(xx, yy, [132, 218, 188, 255]);
                    }
                }
            }
        }
        let mut angle = 200.0_f32;
        while angle <= 340.0 {
            let rad = angle.to_radians();
            put(
                (icx + 7.0 * icon_k * s * rad.cos()) as i32,
                (icy + 7.0 * icon_k * s * rad.sin() * 0.55) as i32,
                [104, 224, 178, 235],
            );
            angle += 6.0;
        }

        // 2D trace code: sits beside the revision/date block, inside the
        // sticker, rather than jammed into the corner arc.
        let code = 6.4 * s;
        let bx = (right as f32 - ring) - 2.2 * s - code;
        let by = 33.0 * s;
        for yy in 0..7 {
            for xx in 0..7 {
                if (xx * 7 + yy * 3) % 5 != 0 {
                    let cell = code / 7.0;
                    for py in 0..cell as i32 {
                        for px in 0..cell as i32 {
                            put(
                                (bx + xx as f32 * cell) as i32 + px,
                                (by + yy as f32 * cell) as i32 + py,
                                [210, 214, 205, 255],
                            );
                        }
                    }
                }
            }
        }

        if let Some(font) = text::label_font() {
            let ls = text::fit(font, "LS-SYNC", 16.0 * s, 1, 4.0 * s, 2.2 * s);
            ink_band(&mut face, (13.0 * s) as u32, (5.0 * s) as u32, &ls, NETWORK_INK, 1.0);
            let label_layout = text::fit(font, name, w as f32 - 16.0 * s, 1, 7.0 * s, 3.0 * s);
            ink_band(
                &mut face,
                (25.0 * s) as u32,
                (8.0 * s) as u32,
                &label_layout,
                NETWORK_INK,
                1.0,
            );
            let rev = text::fit(font, "REV C", 20.0 * s, 1, 3.0 * s, 2.0 * s);
            ink_band(&mut face, (33.0 * s) as u32, (3.0 * s) as u32, &rev, [165, 190, 180], 1.0);
            let date = text::fit(font, "25A1", 20.0 * s, 1, 3.0 * s, 2.0 * s);
            ink_band(&mut face, (36.0 * s) as u32, (3.0 * s) as u32, &date, [126, 155, 146], 1.0);
        }

    } else {
        // Small board chip: only the status LED and name are legible.
        let led = (2.0 * s).max(1.0) as i32;
        for yy in 0..led {
            for xx in 0..led {
                put(
                    (9.0 * s) as i32 + xx,
                    (14.0 * s) as i32 + yy,
                    if lit { [91, 231, 155, 255] } else { [38, 45, 42, 255] },
                );
            }
        }
        if let Some(font) = text::label_font() {
            let layout = text::fit(font, name, w as f32 - 9.0 * s, 1, 8.0 * s, 3.0 * s);
            ink_band(
                &mut face,
                (25.0 * s) as u32,
                (9.0 * s) as u32,
                &layout,
                NETWORK_INK,
                1.0,
            );
        }
    }
    face
}

const NETWORK_INK: [u8; 3] = [0xee, 0xf5, 0xe6];

const LID_REST: Placed = Placed {
    x: 288.0,
    y: 30.0,
    w: 144.0,
    h: 81.0,
};

const FC_LID_REST: Placed = Placed {
    x: 288.0,
    y: 30.0,
    w: 144.0,
    h: 120.0,
};

const MD_LID_REST: Placed = Placed {
    x: 288.0,
    y: 30.0,
    w: 144.0,
    h: 97.6,
};

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub fn shelf_cart() -> Placed {
    shelf_cart_at((OUT_W - CART_W) as f32 / 2.0, 1.0)
}

pub fn shelf_cart_at(x: f32, scale: f32) -> Placed {
    let (w, h) = (CART_W as f32 * scale, CART_H as f32 * scale);
    Placed {
        x,
        y: foot_y(CART_H as f32) - h,
        w,
        h,
    }
}

pub fn board_at(progress: f32) -> Placed {
    board_from(shelf_cart(), progress)
}

pub fn board_from(shelf: Placed, progress: f32) -> Placed {
    board_from_for(Platform::Gba, shelf, progress)
}

/// The board lift, with a portrait rest position for the GB board.
pub fn board_from_for(platform: Platform, shelf: Placed, progress: f32) -> Placed {
    let (w, h) = board_size(platform);
    let (x, y) = match platform {
        Platform::Gb | Platform::Gbc => (210.0, 90.0),
        Platform::Gba | Platform::Fc | Platform::Sfc => (BOARD_X, BOARD_Y),
        Platform::Md => (BOARD_X, 174.0),
    };
    let rest = Placed {
        x,
        y,
        w: w as f32,
        h: h as f32,
    };
    lerp(shelf, rest, lift_of(progress))
}

pub fn lid_at(progress: f32) -> (Placed, f32) {
    lid_from(shelf_cart(), progress)
}

pub fn lid_from(shelf: Placed, progress: f32) -> (Placed, f32) {
    lid_from_for(Platform::Gba, shelf, progress)
}

/// The opened cart lid, in the platform's own portrait/landscape shape.
pub fn lid_from_for(platform: Platform, shelf: Placed, progress: f32) -> (Placed, f32) {
    let rest = match platform {
        Platform::Gb | Platform::Gbc => Placed {
            x: 320.0,
            y: 20.0,
            w: 104.0,
            h: 120.0,
        },
        Platform::Gba => LID_REST,
        Platform::Fc => FC_LID_REST,
        Platform::Sfc => FC_LID_REST,
        Platform::Md => MD_LID_REST,
    };
    let slid = Placed {
        y: shelf.y - slide_up_for(platform) * slide_of(progress),
        ..shelf
    };
    let lift = lift_of(progress);
    (lerp(slid, rest, lift), LID_TURN * lift)
}

const UNITS_W: f32 = 240.0;
const UNITS_H: f32 = 135.0;

/// The socket and the chip measured in board units rather than pixels.
fn socket_units(platform: Platform) -> (f32, f32, f32, f32) {
    let (bw, bh) = board_size(platform);
    let (uw, uh) = board_units(platform);
    let zoom = bw as f32 / BOARD_W as f32;
    let core = core_chip_scale(platform);
    let sw = SOCKET_W as f32 * zoom * uw / bw as f32 * core;
    let sh = SOCKET_H as f32 * zoom * uh / bh as f32 * core;
    let cw = CHIP_W as f32 * zoom * uw / bw as f32 * core;
    let ch = CHIP_H as f32 * zoom * uh / bh as f32 * core;
    (sw, sh, cw, ch)
}

pub fn on_board(board: Placed, u: f32, v: f32) -> (f32, f32) {
    (
        board.x + u * board.w / UNITS_W,
        board.y + v * board.h / UNITS_H,
    )
}

pub fn on_board_for(platform: Platform, board: Placed, u: f32, v: f32) -> (f32, f32) {
    let (uw, uh) = board_units(platform);
    (
        board.x + u * board.w / uw,
        board.y + v * board.h / uh,
    )
}

pub fn board_zoom(board: Placed) -> f32 {
    board.w / BOARD_W as f32
}

pub fn grown(p: Placed, by: f32) -> Placed {
    Placed {
        x: p.x - by,
        y: p.y - by,
        w: p.w + 2.0 * by,
        h: p.h + 2.0 * by,
    }
}

fn lerp(a: Placed, b: Placed, t: f32) -> Placed {
    Placed {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        w: a.w + (b.w - a.w) * t,
        h: a.h + (b.h - a.h) * t,
    }
}

pub fn padded(face: &CartFace, pad: u32) -> CartFace {
    let (w, h) = (face.w + 2 * pad, face.h + 2 * pad);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let stride = (face.w * 4) as usize;
    for row in 0..face.h {
        let from = (row * face.w * 4) as usize;
        let to = (((row + pad) * w + pad) * 4) as usize;
        rgba[to..to + stride].copy_from_slice(&face.rgba[from..from + stride]);
    }
    CartFace { rgba, w, h }
}

pub fn socket_face(core: Core) -> CartFace {
    let rgba = art::render_svg(SOCKET_SVG, SOCKET_W, SOCKET_H)
        .unwrap_or_else(|| vec![0; (SOCKET_W * SOCKET_H * 4) as usize]);
    let mut face = CartFace {
        rgba,
        w: SOCKET_W,
        h: SOCKET_H,
    };
    if let Some(layout) = name_layout(core, (SOCKET_W - 12) as f32) {
        ink_band(&mut face, 4, 37, &layout, SOCKET_INK, 0.5);
    }
    face
}

pub fn chip_face(core: Option<Core>) -> CartFace {
    chip_face_for(Platform::Gba, core)
}

pub fn chip_face_for(platform: Platform, core: Option<Core>) -> CartFace {
    let svg = match platform {
        Platform::Gba => CHIP_SVG,
        Platform::Gb => CHIP_GB_SVG,
        Platform::Gbc => CHIP_GBC_SVG,
        Platform::Fc => CHIP_FC_SVG,
        Platform::Sfc => CHIP_FC_SVG,
        Platform::Md => CHIP_FC_SVG,
    };
    let rgba = art::render_svg(svg, CHIP_W, CHIP_H)
        .unwrap_or_else(|| vec![0; (CHIP_W * CHIP_H * 4) as usize]);
    let mut face = CartFace {
        rgba,
        w: CHIP_W,
        h: CHIP_H,
    };
    if let Some(layout) = core.and_then(|c| name_layout(c, (CHIP_W - 8) as f32)) {
        ink_band(&mut face, 4, 34, &layout, CHIP_INK, 1.0);
    }
    padded(&face, TURN_PAD)
}

pub fn chip_shadow_face() -> CartFace {
    let (w, h) = (SHADOW_W, SHADOW_H);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = (x as f32 + 0.5 - w as f32 / 2.0) / (w as f32 / 2.0);
            let dy = (y as f32 + 0.5 - h as f32 / 2.0) / (h as f32 / 2.0);
            let fall = (1.0 - (dx * dx + dy * dy)).max(0.0);
            rgba.extend_from_slice(&[0, 0, 0, (fall * fall * 255.0) as u8]);
        }
    }
    CartFace { rgba, w, h }
}

fn name_layout(core: Core, max_w: f32) -> Option<text::Layout> {
    let font = text::label_font()?;
    let tracking = |px: f32| (px * 0.10).round();
    let mut px = NAME_PX;
    while px > NAME_MIN_PX && text::line_width(font, core.text(), px, tracking(px)) > max_w {
        px -= 1.0;
    }
    Some(text::Layout {
        lines: vec![core.chip_text().to_string()],
        px,
        tracking: tracking(px),
    })
}
