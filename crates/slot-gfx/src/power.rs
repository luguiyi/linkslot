use crate::surface::{OUT_H, OUT_W};

const LINE_PX: f32 = 2.0;

const DOT_T: f32 = 0.12;

const OVERSHOOT: f32 = 0.6;

pub fn screen_scale(t: f32) -> f32 {
    let left = 1.0 - t.clamp(0.0, 1.0);
    1.0 - (1.0 - LINE_PX / OUT_H as f32) * left * left
}

pub fn screen_width(t: f32) -> f32 {
    let left = 1.0 - (t.clamp(0.0, 1.0) / DOT_T).min(1.0);
    1.0 - (1.0 - LINE_PX / OUT_W as f32) * left
}

pub fn screen_brightness(t: f32) -> f32 {
    let left = 1.0 - t.clamp(0.0, 1.0);
    1.0 + OVERSHOOT * left * left
}

/// 开机动画的当前矩形：终态就是该机种的 base_rect，收起时收向合成画布中心。
pub fn screen_rect(src_w: u32, src_h: u32, t: f32) -> (f32, f32, f32, f32) {
    let (_, _, base_w, base_h) = crate::system::base_rect(src_w, src_h);
    screen_rect_from(base_w, base_h, t)
}

/// 同 [`screen_rect`]，但终态是保持比例的分数缩放矩形（铺满高度）。
pub fn screen_rect_fit(src_w: u32, src_h: u32, t: f32) -> (f32, f32, f32, f32) {
    let (_, _, base_w, base_h) = crate::system::base_rect_fit(src_w, src_h);
    screen_rect_from(base_w, base_h, t)
}

fn screen_rect_from(base_w: f32, base_h: f32, t: f32) -> (f32, f32, f32, f32) {
    let w = base_w * screen_width(t);
    let h = base_h * screen_scale(t);
    ((OUT_W as f32 - w) / 2.0, (OUT_H as f32 - h) / 2.0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_animation_settles_on_the_canvas_base_rect() {
        // GBA 收尾必须还是从前的整块 720x480，否则老机种会掉像素。
        assert_eq!(screen_rect(240, 160, 1.0), (0.0, 0.0, 720.0, 480.0));
        assert_eq!(
            screen_rect(256, 224, 1.0),
            crate::system::base_rect(256, 224)
        );
    }

    #[test]
    fn the_fit_animation_settles_on_the_fractional_rect() {
        let (x, y, w, h) = screen_rect_fit(256, 224, 1.0);
        assert!((y - 0.0).abs() < 1e-4, "top bar at rest: {y}");
        assert!((h - 480.0).abs() < 1e-4, "height at rest: {h}");
        assert!((w - 548.5714).abs() < 0.01, "width at rest: {w}");
        assert!((x - 85.7143).abs() < 0.01, "left margin at rest: {x}");
    }
}
