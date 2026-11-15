//! Renders the slot trough once per collection, so the nameplate can be
//! compared by eye. Opt-in: does nothing unless `SLOT_PLATE_PNG` names a file.

use slot_gfx::{Draw, TexId, OUT_H, OUT_W};
use slot_ui::{collection_plate_face, draw_empty_slot_with, CollectionKind};

const CROP_Y: usize = 408;
const CROP_H: usize = 72;
const GAP: usize = 8;

const GROUND: [u8; 3] = [0x14, 0x15, 0x1a];

fn blend(frame: &mut [u8], x: i64, y: i64, colour: [u8; 4]) {
    if x < 0 || y < 0 || x >= OUT_W as i64 || y >= OUT_H as i64 || colour[3] == 0 {
        return;
    }
    let at = ((y as usize * OUT_W as usize) + x as usize) * 4;
    let a = colour[3] as f32 / 255.0;
    for c in 0..3 {
        frame[at + c] =
            (colour[c] as f32 * a + frame[at + c] as f32 * (1.0 - a)).round() as u8;
    }
    frame[at + 3] = 0xff;
}

fn paint(frame: &mut [u8], list: &[Draw], plate: &[u8], pw: u32, ph: u32) {
    for draw in list {
        match *draw {
            Draw::Rect {
                x, y, w, h, colour,
            } => {
                let x0 = x.max(0.0).floor() as i64;
                let y0 = y.max(0.0).floor() as i64;
                let x1 = (x + w).ceil() as i64;
                let y1 = (y + h).ceil() as i64;
                let c = [
                    (colour[0] * 255.0).round() as u8,
                    (colour[1] * 255.0).round() as u8,
                    (colour[2] * 255.0).round() as u8,
                    (colour[3] * 255.0).round() as u8,
                ];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        blend(frame, xx, yy, c);
                    }
                }
            }
            Draw::Tex {
                x,
                y,
                w,
                h,
                tex,
                alpha,
            } if tex == TexId::from_raw(1) => {
                let x0 = x.max(0.0).floor() as i64;
                let y0 = y.max(0.0).floor() as i64;
                let x1 = (x + w).ceil() as i64;
                let y1 = (y + h).ceil() as i64;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let u = ((xx as f32 + 0.5 - x) / w).clamp(0.0, 0.999);
                        let v = ((yy as f32 + 0.5 - y) / h).clamp(0.0, 0.999);
                        let sx = (u * pw as f32) as u32;
                        let sy = (v * ph as f32) as u32;
                        let at = ((sy * pw + sx) * 4) as usize;
                        let c = [
                            plate[at],
                            plate[at + 1],
                            plate[at + 2],
                            (plate[at + 3] as f32 * alpha).round() as u8,
                        ];
                        blend(frame, xx, yy, c);
                    }
                }
            }
            _ => {}
        }
    }
}

#[test]
fn render_slot_plates() {
    let Ok(path) = std::env::var("SLOT_PLATE_PNG") else {
        return;
    };
    let mut sheet = Vec::new();
    let row = CROP_H + GAP;
    let sheet_h = row * CollectionKind::ALL.len();
    sheet.resize(OUT_W as usize * sheet_h * 4, 0);
    for y in 0..sheet_h {
        for x in 0..OUT_W as usize {
            let at = (y * OUT_W as usize + x) * 4;
            sheet[at..at + 3].copy_from_slice(&GROUND);
            sheet[at + 3] = 0xff;
        }
    }

    for (index, kind) in CollectionKind::ALL.iter().copied().enumerate() {
        let face = collection_plate_face(kind, slot_ui::Language::ZhCn);
        let mut frame = vec![0u8; OUT_W as usize * OUT_H as usize * 4];
        for p in frame.chunks_exact_mut(4) {
            p.copy_from_slice(&[GROUND[0], GROUND[1], GROUND[2], 0xff]);
        }
        let mut list = Vec::new();
        draw_empty_slot_with(Some((TexId::from_raw(1), face.w, face.h)), &mut list);
        paint(&mut frame, &list, &face.rgba, face.w, face.h);
        for yy in 0..CROP_H {
            let src = ((CROP_Y + yy) * OUT_W as usize) * 4;
            let dst = ((index * row + yy) * OUT_W as usize) * 4;
            sheet[dst..dst + OUT_W as usize * 4]
                .copy_from_slice(&frame[src..src + OUT_W as usize * 4]);
        }
    }

    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(file, OUT_W, sheet_h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&sheet).expect("png data");
    writer.finish().expect("png finish");
    eprintln!("wrote {path}");
}
