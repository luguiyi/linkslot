use slot_gfx::{Draw, TexId, OUT_H, OUT_W};
use slot_ui::{Language, about_face, draw_about, AboutFields};

#[test]
fn render_about() {
    let Ok(out) = std::env::var("SCRATCH_PNG") else {
        return;
    };
    let face = about_face(&AboutFields {
        serial: "0473885",
        date: "2026-10-22 14:00",
        dirty: false,
        device: "Anbernic RG34XX SP",
    }, Language::ZhCn);
    let mut list = Vec::new();
    draw_about(Some(TexId::from_raw(1)), &mut list);

    let (w, h) = (OUT_W as usize, OUT_H as usize);
    let mut px = vec![0u8; w * h * 4];
    for p in px.chunks_mut(4) {
        p.copy_from_slice(&[0, 0, 0, 255]);
    }
    for draw in list {
        let Draw::Tex {
            x,
            y,
            w: tw,
            h: th,
            alpha,
            ..
        } = draw
        else {
            continue;
        };
        for row in 0..th as usize {
            for col in 0..tw as usize {
                let (sx, sy) = (
                    col * face.w as usize / tw as usize,
                    row * face.h as usize / th as usize,
                );
                let s = (sy * face.w as usize + sx) * 4;
                let a = face.rgba[s + 3] as f32 / 255.0 * alpha;
                let (dx, dy) = (x as usize + col, y as usize + row);
                if a > 0.0 && dx < w && dy < h {
                    let at = (dy * w + dx) * 4;
                    for c in 0..3 {
                        px[at + c] = (face.rgba[s + c] as f32 * a
                            + px[at + c] as f32 * (1.0 - a)) as u8;
                    }
                }
            }
        }
    }

    let f = std::fs::File::create(&out).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&px).unwrap();
    println!("wrote {out}");
}
