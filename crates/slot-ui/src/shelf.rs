use slot_gfx::{Draw, TexId, OUT_H, OUT_W};
use slot_store::Cart;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::cart::{cart_box, gb_shell_of, label_colour, label_text, CART_H, CART_W};
use crate::hud::Millis;
use crate::silhouette::GbShell;
use crate::slot_chrome::{draw_empty_slot, MOUTH_H};
use crate::status_bar::{STATUS_H, STATUS_MARGIN};

const PLAYTIME_GAP: f32 = 6.0;
const PLAYTIME_PAD: f32 = 6.0;
const PLAYTIME_MOUTH_GAP: f32 = 4.0;

/// Plate for the selected cartridge's playtime label, positioned just below
/// the cartridge and kept clear of the slot mouth, status bar and screen edges.
pub fn playtime_plate(cart: (f32, f32, f32, f32), face: (u32, u32)) -> (f32, f32, f32, f32) {
    let (cart_x, cart_y, cart_w, cart_h) = cart;
    let w = face.0 as f32 + 2.0 * PLAYTIME_PAD;
    let h = face.1 as f32 + 2.0 * PLAYTIME_PAD;
    let min_x = STATUS_MARGIN;
    let max_x = (OUT_W as f32 - STATUS_MARGIN - w).max(min_x);
    let x = (cart_x + cart_w / 2.0 - w / 2.0).clamp(min_x, max_x);
    let mouth_top = OUT_H as f32 - MOUTH_H;
    let max_y = (mouth_top - PLAYTIME_MOUTH_GAP - h).max(STATUS_H + 4.0);
    let y = (cart_y + cart_h + PLAYTIME_GAP).min(max_y);
    (x, y, w, h)
}


const SIDE_SCALE: f32 = 0.78;
const SIDE_ALPHA: f32 = 0.55;

static SLACK: AtomicU32 = AtomicU32::new(0);

pub fn set_shelf_slack(px: f32) {
    SLACK.store(px.max(0.0).to_bits(), Ordering::Relaxed);
}

fn raise() -> f32 {
    f32::from_bits(SLACK.load(Ordering::Relaxed)) / 2.0
}

pub fn panel_middle_y() -> f32 {
    OUT_H as f32 / 2.0 + raise()
}

pub fn rest_y(h: f32) -> f32 {
    let y = if h > CART_H as f32 {
        (OUT_H as f32 - MOUTH_H - h) / 2.0
    } else {
        (OUT_H + CART_H) as f32 / 2.0 - h
    };
    y - raise()
}

pub fn foot_y(h: f32) -> f32 {
    rest_y(h) + h
}
const OMEGA: f32 = 16.0;
const PART: f32 = 130.0;

const SLOTS: i32 = 3;

const REPEAT_DELAY_MS: Millis = 400;
const REPEAT_MS: [Millis; 4] = [110, 85, 65, 50];

pub struct Shelf {
    pub carts: Vec<Cart>,
    pub index: usize,
    pub scroll: f32,
    faces: Vec<Option<TexId>>,
    shadow: Option<TexId>,
    gb_shadow: Option<TexId>,
    gbc_shadow: Option<TexId>,
    shells: Vec<Option<GbShell>>,
    ride: f32,
    vel: f32,
    held: Option<(i32, Millis, usize)>,
}

impl Shelf {
    pub fn new(carts: Vec<Cart>) -> Self {
        Shelf {
            shells: carts.iter().map(gb_shell_of).collect(),
            faces: vec![None; carts.len()],
            carts,
            index: 0,
            scroll: 0.0,
            shadow: None,
            gb_shadow: None,
            gbc_shadow: None,
            ride: 0.0,
            vel: 0.0,
            held: None,
        }
    }

    pub fn select(&mut self, i: usize) {
        self.index = i;
        self.scroll = i as f32;
        self.ride = i as f32;
        self.vel = 0.0;
    }

    pub fn set_shadow(&mut self, face: TexId) {
        self.shadow = Some(face);
    }

    pub fn set_gb_shadow(&mut self, shell: GbShell, face: TexId) {
        match shell {
            GbShell::Notched => self.gb_shadow = Some(face),
            GbShell::Rounded => self.gbc_shadow = Some(face),
        }
    }

    pub fn set_faces(&mut self, faces: Vec<TexId>) {
        let mut faces = faces.into_iter();
        for face in &mut self.faces {
            *face = faces.next();
        }
    }

    pub fn face(&self, i: usize) -> Option<TexId> {
        self.faces.get(i).copied().flatten()
    }

    pub fn set_face(&mut self, i: usize, tex: TexId) -> Option<TexId> {
        self.faces.get_mut(i).and_then(|f| f.replace(tex))
    }

    pub fn take_face(&mut self, i: usize) -> Option<TexId> {
        self.faces.get_mut(i).and_then(Option::take)
    }

    pub fn take_faces(&mut self) -> Vec<TexId> {
        self.faces.iter_mut().filter_map(Option::take).collect()
    }

    pub fn on_screen(&self) -> Vec<usize> {
        self.around(SLOTS)
    }

    pub fn around(&self, radius: i32) -> Vec<usize> {
        let mut out = Vec::new();
        for step in 0..=radius {
            for off in [step, -step] {
                if let Some(i) = self.cart_at_offset(off) {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    pub fn find(&self, stem: &str) -> Option<(&Cart, Option<TexId>)> {
        let i = self.carts.iter().position(|c| c.stem == stem)?;
        Some((&self.carts[i], self.face(i)))
    }

    pub fn left(&mut self) {
        self.step(-1);
    }

    pub fn right(&mut self) {
        self.step(1);
    }

    pub fn jump_next_letter(&mut self) {
        self.jump(1);
    }

    pub fn jump_prev_letter(&mut self) {
        self.jump(-1);
    }

    fn start_of_letter(&self, from: usize) -> usize {
        let n = self.carts.len();
        let letter = slot_store::initial(&self.carts[from].stem);
        let mut at = from;
        for _ in 0..n {
            let before = (at as i32 - 1).rem_euclid(n as i32) as usize;
            if slot_store::initial(&self.carts[before].stem) != letter {
                break;
            }
            at = before;
        }
        at
    }

    fn jump(&mut self, dir: i32) {
        let n = self.carts.len();
        if n < 2 {
            return;
        }
        let wrap = |i: i32| i.rem_euclid(n as i32) as usize;
        let here = slot_store::initial(&self.carts[self.index].stem);
        if self
            .carts
            .iter()
            .all(|c| slot_store::initial(&c.stem) == here)
        {
            return;
        }
        let target = match dir > 0 {
            true => {
                let mut at = self.index;
                for _ in 0..n {
                    at = wrap(at as i32 + 1);
                    if slot_store::initial(&self.carts[at].stem) != here {
                        break;
                    }
                }
                at
            }
            false => {
                let start = self.start_of_letter(self.index);
                match start == self.index {
                    true => self.start_of_letter(wrap(start as i32 - 1)),
                    false => start,
                }
            }
        };
        let ahead = (target as i32 - self.index as i32).rem_euclid(n as i32);
        let delta = match dir > 0 {
            true => ahead,
            false => ahead - n as i32,
        };
        self.index = target;
        self.ride += delta as f32;
        let to = self.scroll_target();
        self.scroll = to - (to - self.scroll).clamp(-1.0, 1.0);
    }

    pub fn hold_left(&mut self, now: Millis) {
        self.hold(-1, now);
    }

    pub fn hold_right(&mut self, now: Millis) {
        self.hold(1, now);
    }

    fn hold(&mut self, by: i32, now: Millis) {
        self.step(by);
        self.held = Some((by, now + REPEAT_DELAY_MS, 0));
    }

    pub fn release_left(&mut self) {
        self.release(-1);
    }

    pub fn release_right(&mut self) {
        self.release(1);
    }

    fn release(&mut self, by: i32) {
        if matches!(self.held, Some((held, _, _)) if held == by) {
            self.held = None;
        }
    }

    pub fn release_hold(&mut self) {
        self.held = None;
    }

    /// Put the selected cartridge back at the centre immediately.  Used when a
    /// full-screen overlay closes so a stale/off-screen spring cannot leave the
    /// shelf looking empty after the overlay disappears.
    pub fn snap_to_selection(&mut self) {
        self.scroll = self.index as f32;
        self.ride = self.index as f32;
        self.vel = 0.0;
        self.held = None;
    }

    pub fn tick(&mut self, now: Millis) {
        let Some((by, due, fired)) = self.held else {
            return;
        };
        if now < due {
            return;
        }
        self.step(by);
        let rate = REPEAT_MS[fired.min(REPEAT_MS.len() - 1)];
        self.held = Some((by, now + rate, fired + 1));
    }

    fn step(&mut self, by: i32) {
        let n = self.carts.len();
        if n < 2 {
            return;
        }
        self.index = (self.index as i32 + by).rem_euclid(n as i32) as usize;
        self.ride += by as f32;
    }

    pub fn scroll_target(&self) -> f32 {
        let n = self.carts.len();
        if n == 0 {
            return 0.0;
        }
        let from = self.ride;
        let n = n as f32;
        from + (self.index as f32 - from + n / 2.0).rem_euclid(n) - n / 2.0
    }

    pub fn cart_at_offset(&self, off: i32) -> Option<usize> {
        let n = self.carts.len() as i32;
        if n == 0 {
            return None;
        }
        if n == 1 {
            return (off == 0).then_some(self.index);
        }
        Some((self.index as i32 + off).rem_euclid(n) as usize)
    }

    pub fn selected_at(&self) -> (f32, f32) {
        let w = self
            .carts
            .get(self.index)
            .map_or(CART_W, |c| cart_box(c.platform).0) as f32;
        let target = self.scroll_target();
        let scroll = if self.scroll.is_finite() {
            self.scroll
        } else {
            target
        };
        let offset = target - scroll;
        let scale = shrink(offset);
        (OUT_W as f32 / 2.0 + offset * w - w * scale / 2.0, scale)
    }

    pub fn update(&mut self, dt: f32) {
        // Search overlays can upload large textures on the render thread; a
        // single very long frame must not let the shelf spring diverge into
        // NaN/Infinity (which hides every cartridge on the next draw).
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let dt = dt.min(0.05);
        if !self.ride.is_finite() {
            self.ride = self.index as f32;
        }
        if !self.scroll.is_finite() {
            self.scroll = self.scroll_target();
            self.vel = 0.0;
        }
        let accel = -2.0 * OMEGA * self.vel - OMEGA * OMEGA * (self.scroll - self.scroll_target());
        self.vel += accel * dt;
        self.scroll += self.vel * dt;
        if !self.scroll.is_finite() || !self.vel.is_finite() {
            self.scroll = self.scroll_target();
            self.vel = 0.0;
        }
    }

    pub fn draw(&self, shake: f32, out: &mut Vec<Draw>) {
        self.draw_row(None, shake, 0.0, 1.0, out);
        draw_empty_slot(out);
    }

    pub fn draw_row(
        &self,
        hidden: Option<&str>,
        shake: f32,
        recede: f32,
        dim: f32,
        out: &mut Vec<Draw>,
    ) {
        let recede = recede.clamp(0.0, 1.0);
        let dim = dim.clamp(0.0, 1.0);
        let target = self.scroll_target();
        let scroll = if self.scroll.is_finite() {
            self.scroll
        } else {
            target
        };
        // Farthest carts first, the selected one last: neighbours of different
        // widths overlap (a GB cart beside an Advance one sits wide), and the
        // whole point of the row is that the current cartridge is in front.
        let mut order: Vec<(usize, f32)> = (-SLOTS..=SLOTS)
            .filter_map(|slot| {
                self.cart_at_offset(slot)
                    .map(|i| (i, target + slot as f32 - scroll))
            })
            .collect();
        order.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
        for (i, offset) in order {
            let cart = &self.carts[i];
            if hidden == Some(cart.stem.as_str()) {
                continue;
            }
            let t = offset.abs().min(1.0);
            let scale = shrink(offset);
            let alpha = (1.0 + (SIDE_ALPHA - 1.0) * t) * (1.0 - recede);
            let (cw, ch) = cart_box(cart.platform);
            let (w, h) = (cw as f32 * scale, ch as f32 * scale);
            let away = offset.signum() * (1.0 + offset.abs());
            let x = OUT_W as f32 / 2.0 + offset * cw as f32 - w / 2.0 + away * PART * recede;
            if x + w <= 0.0 || x >= OUT_W as f32 || alpha <= 0.0 {
                continue;
            }
            let x = x + shake;
            let y = foot_y(ch as f32) - h;
            if alpha < 1.0 {
                let backing = match self.shells.get(i).copied().flatten() {
                    None => self.shadow,
                    Some(GbShell::Notched) => self.gb_shadow,
                    Some(GbShell::Rounded) => self.gbc_shadow,
                };
                if let Some(tex) = backing {
                    out.push(Draw::Tex {
                        x,
                        y,
                        w,
                        h,
                        tex,
                        alpha: recede_alpha(alpha),
                    });
                }
            }
            out.push(match self.face(i) {
                Some(tex) => Draw::Tex {
                    x,
                    y,
                    w,
                    h,
                    tex,
                    alpha: alpha * dim,
                },
                None => {
                    let c = label_colour(&label_text(cart));
                    Draw::Rect {
                        x,
                        y,
                        w,
                        h,
                        colour: [
                            c[0] as f32 / 255.0,
                            c[1] as f32 / 255.0,
                            c[2] as f32 / 255.0,
                            alpha * dim,
                        ],
                    }
                }
            });
        }
    }
}

fn shrink(offset: f32) -> f32 {
    1.0 + (SIDE_SCALE - 1.0) * offset.abs().min(1.0)
}

fn recede_alpha(face_alpha: f32) -> f32 {
    (face_alpha / SIDE_ALPHA).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_finite_spring_state_is_recovered_instead_of_hiding_the_shelf() {
        let mut shelf = Shelf::new(Vec::new());
        shelf.scroll = f32::NAN;
        shelf.vel = f32::INFINITY;
        shelf.ride = f32::NAN;
        shelf.update(1.0);
        assert!(shelf.scroll.is_finite());
        assert!(shelf.vel.is_finite());
        assert!(shelf.ride.is_finite());
    }

    #[test]
    fn snapping_back_to_selection_recovers_an_off_screen_spring() {
        let mut shelf = Shelf::new(Vec::new());
        shelf.scroll = 1.0e30;
        shelf.ride = 1.0e30;
        shelf.vel = 1.0e30;
        shelf.snap_to_selection();
        assert_eq!(shelf.scroll, shelf.index as f32);
        assert_eq!(shelf.ride, shelf.index as f32);
        assert_eq!(shelf.vel, 0.0);
    }
}
