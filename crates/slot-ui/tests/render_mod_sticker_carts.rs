//! Render the two shipped mod stickers on a miniature chooser cart. Opt-in:
//! `MOD_STICKER_DIR` points at the examples directory and `MOD_STICKER_PNG`
//! names the output sheet.

use std::path::Path;

use slot_store::Platform;
use slot_ui::{labelled_cart_face_with_sticker, CartFace, CartSticker, DEFAULT_SHELL};

const CELL_W: usize = 420;
const CELL_H: usize = 320;
const GAP: usize = 12;

#[test]
fn render_mod_sticker_carts() {
    let Ok(dir) = std::env::var("MOD_STICKER_DIR") else {
        return;
    };
    let Ok(out) = std::env::var("MOD_STICKER_PNG") else {
        return;
    };
    let files = [
        (
            "GBAStation",
            Path::new(&dir).join("gbastation-battery-sync/sticker.png"),
        ),
        (
            "SaveDeck",
            Path::new(&dir).join("savedeck-battery-bridge/sticker.png"),
        ),
    ];
    let mut sheet = vec![0u8; CELL_W * 2 * CELL_H * 4];
    for px in sheet.chunks_exact_mut(4) {
        px.copy_from_slice(&[0x24, 0x25, 0x2b, 0xff]);
    }
    for (index, (name, path)) in files.iter().enumerate() {
        let (rgba, w, h) = slot_ui::decode(path).unwrap_or_else(|| panic!("decode {path:?}"));
        let face = labelled_cart_face_with_sticker(
            Platform::Gba,
            None,
            DEFAULT_SHELL,
            name,
            Some(CartSticker::Art { rgba: &rgba, w, h }),
        );
        let ox = index * CELL_W + CELL_W.saturating_sub(face.w as usize) / 2;
        let oy = CELL_H.saturating_sub(face.h as usize) / 2;
        paste(&mut sheet, &face, ox, oy);
    }
    let file = std::fs::File::create(out).expect("create png");
    let mut encoder = png::Encoder::new(file, (CELL_W * 2) as u32, CELL_H as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&sheet)
        .expect("png data");
}

fn paste(dst: &mut [u8], face: &CartFace, ox: usize, oy: usize) {
    let dst_w = CELL_W * 2;
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
