//! Full-screen guide pages: text and EPUB images.

use slot_i18n::Language;

use crate::draw::{OUT_H, OUT_W};
use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;
use crate::text;

pub const TEXT_PX: f32 = 18.0;
pub const TEXT_W: f32 = 624.0;
pub const LINES_PER_PAGE: usize = 13;

const IMAGE_X: f32 = 48.0;
const IMAGE_Y: f32 = 88.0;
const IMAGE_W: f32 = 624.0;
const IMAGE_H: f32 = 342.0;

pub fn page_face(
    title: &str,
    lines: &[String],
    page: usize,
    pages: usize,
    lang: Language,
) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    kb::fill(&mut face, 0.0, 0.0, OUT_W as f32, 70.0, sw::bg_top());
    kb::fill(&mut face, 48.0, 72.0, OUT_W as f32 - 96.0, 1.0, [0x3a, 0x3a, 0x42, 0xff]);

    let (heading, px) = kb::fit_line(title, 624.0, 22.0, 14.0);
    kb::draw_text(&mut face, &heading, px, 48.0, 24.0, sw::ink());

    let line_h = text::line_height(TEXT_PX) + 5.0;
    for (index, line) in lines.iter().enumerate() {
        let top = 92.0 + index as f32 * line_h;
        kb::draw_text(&mut face, line, TEXT_PX, 48.0, top, sw::ink());
    }

    let footer = slot_i18n::t_args(
        lang,
        "device.app.guide.footer",
        &[
            ("page", page.saturating_add(1).to_string().as_str()),
            ("pages", pages.max(1).to_string().as_str()),
        ],
    );
    kb::draw_text(&mut face, &footer, 14.0, 48.0, OUT_H as f32 - 30.0, sw::ink_dim());
    face
}

/// Full-screen image page used by EPUB guides.  The image is contained inside
/// the content area; it is never cropped or stretched.
pub fn image_face(
    title: &str,
    rgba: &[u8],
    width: u32,
    height: u32,
    page: usize,
    pages: usize,
    lang: Language,
) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    kb::fill(&mut face, 0.0, 0.0, OUT_W as f32, 70.0, sw::bg_top());
    kb::fill(&mut face, 48.0, 72.0, OUT_W as f32 - 96.0, 1.0, [0x3a, 0x3a, 0x42, 0xff]);
    kb::fill(&mut face, IMAGE_X, IMAGE_Y, IMAGE_W, IMAGE_H, [0x0b, 0x0b, 0x10, 0xff]);

    let (heading, px) = kb::fit_line(title, 624.0, 22.0, 14.0);
    kb::draw_text(&mut face, &heading, px, 48.0, 24.0, sw::ink());

    if width > 0 && height > 0 && rgba.len() >= (width as usize * height as usize * 4) {
        let scale = (IMAGE_W / width as f32).min(IMAGE_H / height as f32);
        let draw_w = ((width as f32 * scale).round() as u32).clamp(1, IMAGE_W as u32);
        let draw_h = ((height as f32 * scale).round() as u32).clamp(1, IMAGE_H as u32);
        let left = IMAGE_X + (IMAGE_W - draw_w as f32) / 2.0;
        let top = IMAGE_Y + (IMAGE_H - draw_h as f32) / 2.0;
        kb::fill(&mut face, left - 2.0, top - 2.0, draw_w as f32 + 4.0, 2.0, [0x3a, 0x3a, 0x42, 0xff]);
        kb::fill(&mut face, left - 2.0, top + draw_h as f32, draw_w as f32 + 4.0, 2.0, [0x3a, 0x3a, 0x42, 0xff]);
        for y in 0..draw_h {
            let sy = ((y as f32 + 0.5) / scale).min(height as f32 - 1.0) as u32;
            for x in 0..draw_w {
                let sx = ((x as f32 + 0.5) / scale).min(width as f32 - 1.0) as u32;
                let src = ((sy * width + sx) * 4) as usize;
                let dst = (((top as u32 + y) * OUT_W + left as u32 + x) * 4) as usize;
                face.rgba[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
    }

    let footer = slot_i18n::t_args(
        lang,
        "device.app.guide.image_footer",
        &[
            ("page", page.saturating_add(1).to_string().as_str()),
            ("pages", pages.max(1).to_string().as_str()),
        ],
    );
    kb::draw_text(&mut face, &footer, 14.0, 48.0, OUT_H as f32 - 30.0, sw::ink_dim());
    face
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_page_fits_the_picture_inside_the_content_box() {
        let rgba = vec![255u8; 4 * 4];
        let face = image_face("测试图片", &rgba, 2, 2, 0, 2, Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert!(face.rgba.chunks(4).any(|pixel| pixel[0] > 0x80));
    }

    #[test]
    fn a_guide_page_is_full_screen_and_has_ink() {
        let face = page_face(
            "测试攻略",
            &["第一行".to_string(), "第二行".to_string()],
            0,
            3,
            Language::ZhCn,
        );
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert!(face.rgba.chunks(4).any(|pixel| pixel[0] > 0x80));
    }
}
