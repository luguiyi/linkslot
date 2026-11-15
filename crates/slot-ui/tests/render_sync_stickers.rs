//! Render the long-press Y chooser cart faces for eyeballing. Opt-in: writes a
//! PNG only when `SYNC_STICKER_PNG` names a path.

use slot_store::Platform;
use slot_ui::{labelled_cart_face_with_sticker, CartFace, CartSticker, DEFAULT_SHELL};

const CELL_W: usize = 420;
const CELL_H: usize = 400;
const GAP: usize = 10;

enum Kind {
    Text,
    None,
    Art,
}

struct Case {
    name: &'static str,
    platform: Platform,
    label: &'static str,
    kind: Kind,
}

fn mod_art() -> Vec<u8> {
    // A stand-in for a mod sticker: a rounded amber field with a white bar.
    let (w, h) = (240u32, 120u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            let dx = x as f32 - w as f32 / 2.0;
            let dy = y as f32 - h as f32 / 2.0;
            let inside = (dx * dx / 110.0f32.powi(2) + dy * dy / 50.0f32.powi(2)) <= 1.0;
            if inside {
                rgba[at..at + 4].copy_from_slice(&[0xe8, 0xb0, 0x3c, 255]);
                if (40..80).contains(&y) && (50..190).contains(&x) {
                    rgba[at..at + 4].copy_from_slice(&[0xff, 0xff, 0xff, 255]);
                }
            }
        }
    }
    rgba
}

#[test]
fn render_sync_stickers() {
    let Ok(path) = std::env::var("SYNC_STICKER_PNG") else {
        return;
    };
    let art = mod_art();
    let cases = [
        Case {
            name: "gba text",
            platform: Platform::Gba,
            label: "同步云存档",
            kind: Kind::Text,
        },
        Case {
            name: "gba legacy",
            platform: Platform::Gba,
            label: "同步云存档",
            kind: Kind::None,
        },
        Case {
            name: "gba mod art",
            platform: Platform::Gba,
            label: "同步云存档",
            kind: Kind::Art,
        },
        Case {
            name: "fc text",
            platform: Platform::Fc,
            label: "同步云存档",
            kind: Kind::Text,
        },
        Case {
            name: "gb text long",
            platform: Platform::Gb,
            label: "同步 GBAStation 电池存档同步",
            kind: Kind::Text,
        },
        Case {
            name: "gba text long",
            platform: Platform::Gba,
            label: "同步 SaveDeck 电池存档桥 · 双向",
            kind: Kind::Text,
        },
    ];

    let cols = 3usize;
    let rows = (cases.len() + cols - 1) / cols;
    let sheet_w = cols * CELL_W + (cols + 1) * GAP;
    let sheet_h = rows * CELL_H + (rows + 1) * GAP;
    let mut sheet = vec![0u8; sheet_w * sheet_h * 4];
    for px in sheet.chunks_exact_mut(4) {
        px.copy_from_slice(&[0x24, 0x25, 0x2b, 0xff]);
    }

    for (index, case) in cases.into_iter().enumerate() {
        let sticker = match case.kind {
            Kind::Text => Some(CartSticker::Text),
            Kind::None => None,
            Kind::Art => Some(CartSticker::Art {
                rgba: &art,
                w: 240,
                h: 120,
            }),
        };
        let face = labelled_cart_face_with_sticker(
            case.platform,
            None,
            DEFAULT_SHELL,
            case.label,
            sticker,
        );
        let col = index % cols;
        let row = index / cols;
        let ox = GAP + col * (CELL_W + GAP) + (CELL_W - face.w as usize) / 2;
        let oy = GAP + row * (CELL_H + GAP) + CELL_H.saturating_sub(face.h as usize) / 2;
        paste(&mut sheet, sheet_w, &face, ox, oy);
        eprintln!("{index}: {}", case.name);
    }

    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(file, sheet_w as u32, sheet_h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&sheet)
        .expect("png data");
}

fn paste(dst: &mut [u8], dst_w: usize, face: &CartFace, ox: usize, oy: usize) {
    for y in 0..face.h as usize {
        for x in 0..face.w as usize {
            let src = (y * face.w as usize + x) * 4;
            let a = face.rgba[src + 3] as u32;
            if a == 0 {
                continue;
            }
            let to = ((oy + y) * dst_w + ox + x) * 4;
            for c in 0..3 {
                dst[to + c] = ((face.rgba[src + c] as u32 * a
                    + dst[to + c] as u32 * (255 - a)
                    + 127)
                    / 255) as u8;
            }
            dst[to + 3] = 0xff;
        }
    }
}
