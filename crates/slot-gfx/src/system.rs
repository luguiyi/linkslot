//! 机种画布：帧多大、在 720x480 合成画布上放大几倍、摆在哪。
use crate::surface::{OUT_H, OUT_W};

/// 能塞进合成画布的最大整数倍数。非整数缩放会让每个源像素的宽度不一样，
/// 所以宁可留黑边也不做分数缩放。
pub fn scale_for(src_w: u32, src_h: u32) -> u32 {
    // 空画布在调用点已经被挡掉；这里再兜一层，免得除零。
    if src_w == 0 || src_h == 0 {
        return 1;
    }
    (OUT_W / src_w).min(OUT_H / src_h).max(1)
}

/// 能塞进合成画布的最大分数倍数，保持源画面比例。
///
/// 与 [`scale_for`] 的整数策略相对：SFC/FC 的 "L1 铺满高度" 用这个，把上下
/// 黑边换成左右黑边，同时不拉伸、不裁切画面。
pub fn fit_scale(src_w: u32, src_h: u32) -> f32 {
    if src_w == 0 || src_h == 0 {
        return 1.0;
    }
    (OUT_W as f32 / src_w as f32).min(OUT_H as f32 / src_h as f32)
}

/// 居中放置的整数缩放矩形，坐标与 720x480 的合成画布同一坐标系。
pub fn base_rect(src_w: u32, src_h: u32) -> (f32, f32, f32, f32) {
    let scale = scale_for(src_w, src_h) as f32;
    let w = src_w as f32 * scale;
    let h = src_h as f32 * scale;
    ((OUT_W as f32 - w) / 2.0, (OUT_H as f32 - h) / 2.0, w, h)
}

/// 居中放置的分数缩放矩形；保持比例，通常会在左右留下黑边。
pub fn base_rect_fit(src_w: u32, src_h: u32) -> (f32, f32, f32, f32) {
    let scale = fit_scale(src_w, src_h);
    let w = src_w as f32 * scale;
    let h = src_h as f32 * scale;
    ((OUT_W as f32 - w) / 2.0, (OUT_H as f32 - h) / 2.0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_is_the_largest_integer_that_fits_the_output() {
        assert_eq!(scale_for(240, 160), 3);
        assert_eq!(scale_for(160, 144), 3);
        assert_eq!(scale_for(256, 224), 2);
        assert_eq!(scale_for(640, 480), 1);
    }

    #[test]
    fn the_base_rect_centres_the_scaled_canvas() {
        assert_eq!(base_rect(240, 160), (0.0, 0.0, 720.0, 480.0));
        assert_eq!(base_rect(256, 224), (104.0, 16.0, 512.0, 448.0));
    }

    #[test]
    fn the_fit_rect_fills_the_height_and_keeps_the_aspect() {
        let (x, y, w, h) = base_rect_fit(256, 224);
        assert!((y - 0.0).abs() < 1e-4, "top bar survived: {y}");
        assert!((h - 480.0).abs() < 1e-4, "height is {h}");
        assert!(
            (w / h - 256.0 / 224.0).abs() < 1e-5,
            "fit distorted the SFC aspect: {w}x{h}"
        );
        assert!((x - (720.0 - w) / 2.0).abs() < 1e-4, "fit is not centred");
        assert!(x > 80.0 && x < 90.0, "unexpected SFC side margin: {x}");

        let (x, y, w, h) = base_rect_fit(320, 224);
        assert!((y - 0.0).abs() < 1e-4, "MD top bar survived: {y}");
        assert!((h - 480.0).abs() < 1e-4, "MD height is {h}");
        assert!(
            (w / h - 320.0 / 224.0).abs() < 1e-5,
            "fit distorted the MD aspect: {w}x{h}"
        );
        assert!((x - (720.0 - w) / 2.0).abs() < 1e-4, "MD fit is not centred");
    }

    #[test]
    fn the_fit_rect_for_an_exact_canvas_still_fills_it() {
        assert_eq!(base_rect_fit(240, 160), (0.0, 0.0, 720.0, 480.0));
        assert_eq!(base_rect_fit(0, 0), (360.0, 240.0, 0.0, 0.0));
    }
}
