//! The shelf header status bar and the shelf's bottom key legend.
//!
//! The header carries the clock (left), a Wi-Fi state icon and the battery
//! gauge (right). The bottom band carries the shelf's key hints: long-press Y
//! to sync and X for letter search on the left, long-press A to restart the
//! selected game on the right.

use slot_gfx::{Draw, TexId, OUT_H, OUT_W};
use slot_model::Battery;

use crate::battery::{draw_gauge, gauge_width, GAUGE_H};
use crate::cart::CartFace;
use crate::footer::{draw_printed, Printed};
use crate::plate::{hint_quad, HINT_EDGE, HINT_GAP, HINT_H};
use crate::slot_chrome::MOUTH_H;

/// Height of the header band at the top of the shelf.
pub const STATUS_H: f32 = 36.0;
/// Left and right case margin shared by the header and the bottom legend.
pub const STATUS_MARGIN: f32 = 24.0;
/// Gap between the Wi-Fi icon and the battery cluster.
const CLUSTER_GAP: f32 = 14.0;
/// The Wi-Fi icon's nominal size (square).
pub const WIFI_PX: f32 = 22.0;
/// A dimmed Wi-Fi icon means "not associated".
const WIFI_OFF_ALPHA: f32 = 0.28;

const BAR: [f32; 4] = [0.02, 0.02, 0.03, 0.38];
const BAR_EDGE: [f32; 4] = [1.0, 1.0, 1.0, 0.05];

/// The faces the header needs, owned by the caller (the `slot` app).
#[derive(Clone, Copy, Debug, Default)]
pub struct StatusBar {
    pub clock: Printed,
    pub battery: Option<Battery>,
    pub percent: Printed,
    pub bolt: Option<TexId>,
    pub wifi: Option<(TexId, u32, u32)>,
    pub wifi_on: bool,
}

impl StatusBar {
    pub fn draw(&self, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: STATUS_H,
            colour: BAR,
        });
        out.push(Draw::Rect {
            x: 0.0,
            y: STATUS_H - 1.0,
            w: OUT_W as f32,
            h: 1.0,
            colour: BAR_EDGE,
        });

        let text_y = (STATUS_H - HINT_H as f32) / 2.0;
        draw_printed(STATUS_MARGIN, text_y, self.clock, out);

        let gauge = gauge_width(self.percent);
        let battery_x = OUT_W as f32 - STATUS_MARGIN - gauge;
        draw_gauge(
            battery_x,
            (STATUS_H - GAUGE_H) / 2.0,
            self.battery,
            self.percent,
            self.bolt,
            out,
        );

        if let Some((tex, w, h)) = self.wifi {
            let x = battery_x - CLUSTER_GAP - w as f32;
            out.push(Draw::Tex {
                x,
                y: (STATUS_H - h as f32) / 2.0,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: if self.wifi_on { 1.0 } else { WIFI_OFF_ALPHA },
            });
        }
    }
}

/// Draw the shelf's bottom key legend: `left` hugs the left case margin and
/// `right` is right-aligned to the right one.
pub fn draw_shelf_legend(left: &[(TexId, u32)], right: &[(TexId, u32)], out: &mut Vec<Draw>) {
    let y = OUT_H as f32 - MOUTH_H + (MOUTH_H - HINT_H as f32) / 2.0;
    let seen = |w: u32| w.saturating_sub(HINT_EDGE) as f32;

    let mut x = STATUS_MARGIN;
    for (tex, w) in left {
        out.push(hint_quad(x, y, *w as f32, Some(*tex)));
        x += seen(*w) + HINT_GAP;
    }

    let total: f32 = right.iter().map(|(_, w)| seen(*w)).sum::<f32>()
        + HINT_GAP * right.len().saturating_sub(1) as f32;
    let mut x = OUT_W as f32 - STATUS_MARGIN - total;
    for (tex, w) in right {
        out.push(hint_quad(x, y, *w as f32, Some(*tex)));
        x += seen(*w) + HINT_GAP;
    }
}

/// Rasterise a Wi-Fi state icon: a dot with three arcs opening upwards, in
/// `colour`. The arcs fade out with anti-aliasing so the icon stays readable at
/// 22 px.
pub fn wifi_face(px: f32, colour: [u8; 3]) -> CartFace {
    let size = px.clamp(8.0, 64.0) as u32;
    let (w, h) = (size, size);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = h as f32 * 0.80;
    let max_r = w as f32 * 0.46;
    let band = (max_r * 0.20).max(1.0);
    let dot_r = max_r * 0.16;
    let rings = [max_r * 0.38, max_r * 0.66, max_r * 0.96];

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = cy - y as f32;
            let d = (dx * dx + dy * dy).sqrt();

            // The dot gets a one-pixel fringe; the arcs are only drawn in the
            // upward 90-degree cone so the icon reads as a Wi-Fi fan.
            let mut cover = (1.0 - (d - (dot_r - 1.0)).max(0.0)).clamp(0.0, 1.0);
            if dy > 0.0 && dx.abs() <= dy {
                for ring in rings {
                    let delta = (d - ring).abs();
                    if delta < band {
                        cover = cover.max(1.0 - delta / band);
                    }
                }
            }
            if cover <= 0.0 {
                continue;
            }
            let alpha = (cover * 255.0).round() as u8;
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = colour[0];
            rgba[i + 1] = colour[1];
            rgba[i + 2] = colour[2];
            rgba[i + 3] = alpha;
        }
    }
    CartFace { rgba, w, h }
}
