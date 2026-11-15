//! The long-press Y choices are all NET slots.  The cloud row uses the
//! built-in generated sticker; a mod can supply a complete slot-sticker image,
//! which is placed in the same cream paper area.

use crate::cart::CartFace;
use crate::plate::UndoFace;
use crate::switch_style as sw;
use crate::text;

const W: u32 = 360;
const H: u32 = 212;
const CYAN: [u8; 4] = [86, 228, 248, 255];
const INK: [u8; 3] = [46, 41, 34];

pub enum NetSlotSticker<'a> {
    /// Built-in cloud-sync sticker.
    Cloud,
    /// A text sticker, used by mods without art.
    Text(&'a str),
    /// A complete mod sticker image, designed in the same slot-sticker layout.
    /// It fills the paper area; no generated title is layered over it.
    Art {
        rgba: &'a [u8],
        w: u32,
        h: u32,
    },
}

fn rect(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    sw::fill_round(face, x, y, w, h, 2.0, c);
}

fn blank(w: u32, h: u32) -> UndoFace {
    UndoFace {
        rgba: vec![0u8; (w * h * 4) as usize],
        w,
        h,
    }
}

fn blit(dst: &mut UndoFace, src: &UndoFace, x: u32, y: u32) {
    for sy in 0..src.h {
        let dy = y + sy;
        if dy >= dst.h {
            break;
        }
        for sx in 0..src.w {
            let dx = x + sx;
            if dx >= dst.w {
                break;
            }
            let si = ((sy * src.w + sx) * 4) as usize;
            let a = src.rgba[si + 3] as u32;
            if a == 0 {
                continue;
            }
            let di = ((dy * dst.w + dx) * 4) as usize;
            let da = dst.rgba[di + 3] as u32;
            let out = a + da * (255 - a) / 255;
            if out == 0 {
                continue;
            }
            for c in 0..3 {
                dst.rgba[di + c] = ((src.rgba[si + c] as u32 * a
                    + dst.rgba[di + c] as u32 * da * (255 - a) / 255)
                    / out) as u8;
            }
            dst.rgba[di + 3] = out as u8;
        }
    }
}

fn text_face(value: &str, w: u32, h: u32, max_w: f32, lines: usize, px: f32) -> UndoFace {
    text_face_colored(value, w, h, max_w, lines, px, INK)
}

fn text_face_colored(
    value: &str,
    w: u32,
    h: u32,
    max_w: f32,
    lines: usize,
    px: f32,
    colour: [u8; 3],
) -> UndoFace {
    let mut face = blank(w, h);
    let Some(font) = text::label_font() else {
        return face;
    };
    let layout = text::fit(font, value, max_w, lines, px, 7.0);
    text::draw_left(&mut face.rgba, face.w, face.h, &layout, colour, 0.0);
    face
}

/// Cover-fit a mod sticker into the paper area.  New mod stickers use the
/// 512×145 slot-sticker aspect, so this is normally an exact fit.
fn art_face(rgba: &[u8], w: u32, h: u32, out_w: u32, out_h: u32) -> UndoFace {
    let mut face = blank(out_w, out_h);
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return face;
    }
    let scale = (w as f32 / out_w as f32).max(h as f32 / out_h as f32);
    let ox = (w as f32 - out_w as f32 * scale) / 2.0;
    let oy = (h as f32 - out_h as f32 * scale) / 2.0;
    for y in 0..out_h {
        for x in 0..out_w {
            let sx = (ox + x as f32 * scale).clamp(0.0, w as f32 - 1.0) as u32;
            let sy = (oy + y as f32 * scale).clamp(0.0, h as f32 - 1.0) as u32;
            let si = ((sy * w + sx) * 4) as usize;
            let di = ((y * out_w + x) * 4) as usize;
            face.rgba[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    face
}

/// 360×212 transparent face, matching the chooser card slot.
pub fn net_slot_face_with_sticker(sticker: NetSlotSticker<'_>) -> CartFace {
    let mut face = blank(W, H);
    let body = sw::surface();
    let border = sw::surface_border();
    let cream = [232, 226, 205, 255];

    // Main silicon body, with a light edge and a darker inset.
    sw::fill_round(&mut face, 44.0, 20.0, 272.0, 168.0, 18.0, border);
    sw::fill_round(&mut face, 45.0, 21.0, 270.0, 166.0, 17.0, body);
    sw::fill_round(&mut face, 54.0, 30.0, 252.0, 148.0, 13.0, [8, 20, 26, 255]);

    // The upper mouth that receives the game card.
    sw::fill_round(&mut face, 64.0, 40.0, 232.0, 28.0, 6.0, [5, 10, 15, 255]);
    sw::fill_round(&mut face, 65.0, 41.0, 230.0, 26.0, 5.0, [10, 23, 28, 255]);
    for i in 0..7 {
        let x = 82.0 + i as f32 * 28.0;
        rect(&mut face, x, 52.0, 11.0, 8.0, [18, 51, 56, 255]);
        rect(&mut face, x + 2.0, 54.0, 7.0, 1.0, CYAN);
    }
    rect(&mut face, 48.0, 66.0, 10.0, 76.0, [10, 31, 36, 255]);
    rect(&mut face, 302.0, 66.0, 10.0, 76.0, [10, 31, 36, 255]);
    rect(&mut face, 60.0, 72.0, 240.0, 2.0, CYAN);

    // The sticker paper is always present; a custom mod image replaces the
    // generated paper entirely.
    sw::fill_round(&mut face, 74.0, 84.0, 212.0, 60.0, 4.0, [126, 121, 103, 255]);
    match sticker {
        NetSlotSticker::Art { rgba, w, h } => {
            let art = art_face(rgba, w, h, 210, 58);
            blit(&mut face, &art, 75, 85);
        }
        other => {
            sw::fill_round(&mut face, 75.0, 85.0, 210.0, 58.0, 3.0, cream);
            rect(&mut face, 82.0, 90.0, 196.0, 3.0, CYAN);
            let top = text_face("NET SLOT", 190, 12, 184.0, 1, 9.0);
            blit(&mut face, &top, 82, 96);
            let label = match other {
                NetSlotSticker::Cloud => "联网卡槽",
                NetSlotSticker::Text(label) => label,
                NetSlotSticker::Art { .. } => unreachable!(),
            };
            let title = text_face(label, 194, 26, 192.0, 2, 12.0);
            blit(&mut face, &title, 82, 108);
            let hint = text_face("插卡同步", 190, 10, 184.0, 1, 7.0);
            blit(&mut face, &hint, 82, 132);
        }
    }

    // Bottom connector tongue.
    sw::fill_round(&mut face, 66.0, 178.0, 228.0, 14.0, 3.0, [5, 10, 15, 255]);
    for i in 0..8 {
        let x = 78.0 + i as f32 * 26.0;
        rect(&mut face, x, 181.0, 12.0, 7.0, [15, 51, 56, 255]);
        rect(&mut face, x + 2.0, 182.0, 8.0, 1.0, CYAN);
    }

    CartFace {
        rgba: face.rgba,
        w: face.w,
        h: face.h,
    }
}

pub fn net_slot_face() -> CartFace {
    net_slot_face_with_sticker(NetSlotSticker::Cloud)
}

/// The chooser's attachable capability module.  This is not a slot: no card
/// goes inside it.  It is a small clip that sits on the cartridge's top edge.
pub fn net_module_face(label: &str) -> CartFace {
    let mut face = blank(W, H);
    let body = sw::surface();
    let border = sw::surface_border();

    // A large module shown alone in the chooser row.
    sw::fill_round(&mut face, 62.0, 64.0, 236.0, 84.0, 12.0, border);
    sw::fill_round(&mut face, 63.0, 65.0, 234.0, 82.0, 11.0, body);
    sw::fill_round(&mut face, 72.0, 72.0, 216.0, 65.0, 8.0, [8, 18, 23, 255]);
    // clip arms
    rect(&mut face, 50.0, 82.0, 12.0, 48.0, [12, 29, 35, 255]);
    rect(&mut face, 298.0, 82.0, 12.0, 48.0, [12, 29, 35, 255]);
    // pogo contacts under the clip
    for i in 0..4 {
        let x = 104.0 + i as f32 * 42.0;
        rect(&mut face, x, 138.0, 20.0, 9.0, [9, 52, 60, 255]);
        rect(&mut face, x + 3.0, 139.0, 14.0, 1.0, CYAN);
    }
    rect(&mut face, 72.0, 82.0, 216.0, 1.5, CYAN);
    let tag = if label == "NET" { "LINK" } else { "MOD" };
    let top = text_face_colored(tag, 80, 12, 76.0, 1, 8.0, [86, 228, 248]);
    blit(&mut face, &top, 90, 90);
    let light = text_face_colored(label, 190, 30, 184.0, 1, 20.0, [194, 214, 222]);
    blit(&mut face, &light, 86, 102);
    sw::fill_round(&mut face, 289.0, 99.0, 9.0, 9.0, 4.5, CYAN);
    // top antenna nub
    sw::fill_round(&mut face, 166.0, 48.0, 28.0, 17.0, 4.0, border);
    sw::fill_round(&mut face, 167.0, 49.0, 26.0, 15.0, 3.0, [13, 27, 32, 255]);

    CartFace {
        rgba: face.rgba,
        w: face.w,
        h: face.h,
    }
}

/// A small transparent label for the module attached to the live card.
pub fn module_label_face(label: &str) -> CartFace {
    let face = text_face_colored(label, 120, 20, 112.0, 1, 14.0, [210, 228, 234]);
    CartFace {
        rgba: face.rgba,
        w: face.w,
        h: face.h,
    }
}

/// The chooser row: a green side card with a red-framed ability sticker.
pub fn net_card_face(label: &str) -> CartFace {
    let mut face = blank(W, H);
    let edge = [60, 214, 156, 255];
    let red = [219, 58, 52, 255];
    let cream = [232, 226, 205, 255];
    sw::fill_round(&mut face, 70.0, 28.0, 220.0, 154.0, 10.0, [20, 50, 38, 255]);
    rect(&mut face, 70.0, 28.0, 220.0, 1.5, edge);
    rect(&mut face, 70.0, 180.0, 220.0, 1.5, edge);
    rect(&mut face, 72.0, 44.0, 14.0, 122.0, [219, 175, 51, 255]);
    for i in 0..7 {
        rect(&mut face, 75.0, 53.0 + i as f32 * 15.0, 8.0, 8.0, [255, 226, 104, 255]);
    }
    sw::fill_round(&mut face, 96.0, 38.0, 178.0, 48.0, 5.0, red);
    sw::fill_round(&mut face, 99.0, 41.0, 172.0, 42.0, 3.0, cream);
    let top = text_face_colored("SYNC CARD", 150, 10, 140.0, 1, 5.0, [65, 66, 54]);
    blit(&mut face, &top, 108, 43);
    let light = text_face_colored(label, 162, 25, 158.0, 1, 14.0, [45, 45, 42]);
    blit(&mut face, &light, 104, 54);
    sw::fill_round(&mut face, 230.0, 112.0, 26.0, 20.0, 3.0, [12, 29, 23, 255]);
    let chip = text_face_colored("SYNC", 38, 8, 34.0, 1, 3.8, [112, 214, 184]);
    blit(&mut face, &chip, 234, 117);
    sw::fill_round(&mut face, 258.0, 142.0, 7.0, 7.0, 3.5, CYAN);
    let bay = text_face_colored("SIDE BAY", 80, 8, 76.0, 1, 4.0, [140, 190, 170]);
    blit(&mut face, &bay, 112, 158);

    CartFace {
        rgba: face.rgba,
        w: face.w,
        h: face.h,
    }
}

/// The dark label baked into the red sticker on the live side card.
pub fn net_card_label_face(label: &str) -> CartFace {
    let face = text_face_colored(label, 120, 22, 112.0, 1, 14.0, [45, 45, 42]);
    CartFace {
        rgba: face.rgba,
        w: face.w,
        h: face.h,
    }
}
