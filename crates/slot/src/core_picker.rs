//! 主板上的核心选择动画。
//!
//! 选择器是"选项驱动"的：打开时传入当前机种**实际可用**的核心列表
//! （`slot_store::available_options`），左右键在列表内切换，芯片落位到
//! 对应插槽。GBA 用它自己的两个插槽；SFC 复用 Famicom 板上的前两个插槽
//! （snes9x + 可选的 supafaust）。单核心机种列表只有 1 项：板面只作信息展示。

use slot_store::Core;
use slot_ui::{ease, Millis, Refusal, CHIP_TIP};

pub const SLIDE_MS: Millis = 160;
pub const LIFT_MS: Millis = 260;
pub const OPEN_MS: Millis = SLIDE_MS + LIFT_MS;
pub const CLOSE_MS: Millis = 320;
pub const HOP_MS: Millis = 180;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Press {
    Left,
    Right,
    Keep,
    Back,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Nothing,
    Refused,
    Write(Core),
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Chip {
    /// 芯片在插槽之间的位置：0 = 第 1 个插槽，1 = 第 2 个插槽（单核心时恒为 0.5）。
    pub across: f32,
    pub lift: f32,
    pub tip: f32,
    pub seated: Option<Core>,
    pub shake: f32,
}

#[derive(Copy, Clone)]
struct Hop {
    from: usize,
    to: usize,
    started: Millis,
}

#[derive(Copy, Clone)]
struct Close {
    started: Millis,
    from: f32,
}

#[derive(Clone)]
pub struct CorePicker {
    options: Vec<Core>,
    /// `options` 里的下标，不是 `Core::index()`。
    seat: usize,
    opened: Millis,
    started: Option<Millis>,
    hop: Option<Hop>,
    close: Option<Close>,
    refusal: Option<Refusal>,
}

impl CorePicker {
    pub fn open(seat: Core, options: Vec<Core>, now: Millis) -> Self {
        let seat = options.iter().position(|core| *core == seat).unwrap_or(0);
        CorePicker {
            options,
            seat,
            opened: now,
            started: None,
            hop: None,
            close: None,
            refusal: None,
        }
    }

    /// 可选项（与板面上的插槽一一对应）。
    pub fn options(&self) -> &[Core] {
        &self.options
    }

    pub fn start(&mut self, now: Millis) {
        self.started.get_or_insert(now);
    }

    pub fn waiting(&self) -> bool {
        self.started.is_none()
    }

    pub fn waited(&self, now: Millis) -> Millis {
        now.saturating_sub(self.opened)
    }

    pub fn seat(&self) -> Core {
        self.options[self.seat]
    }

    pub fn is_single(&self) -> bool {
        self.options.len() <= 1
    }

    pub fn closing(&self) -> bool {
        self.close.is_some()
    }

    pub fn openness(&self, now: Millis) -> f32 {
        match (self.close, self.started) {
            (Some(Close { started, from }), _) => {
                (from - now.saturating_sub(started) as f32 / CLOSE_MS as f32).max(0.0)
            }
            (None, Some(started)) => (now.saturating_sub(started) as f32 / OPEN_MS as f32).min(1.0),
            (None, None) => 0.0,
        }
    }

    pub fn finished(&self, now: Millis) -> bool {
        self.close.is_some() && self.openness(now) <= 0.0
    }

    pub fn press(&mut self, press: Press, now: Millis) -> Outcome {
        if self.close.is_some() {
            return Outcome::Nothing;
        }
        if self.is_single() {
            return match press {
                // A single-core board is information, not a choice: closing
                // it must not rewrite the core the cart already uses.
                Press::Keep | Press::Back => {
                    self.begin_close(now);
                    Outcome::Nothing
                }
                Press::Left | Press::Right => Outcome::Nothing,
            };
        }
        let target = match press {
            Press::Left => self.seat.checked_sub(1),
            Press::Right => {
                if self.seat + 1 < self.options.len() {
                    Some(self.seat + 1)
                } else {
                    None
                }
            }
            Press::Keep => {
                self.begin_close(now);
                return Outcome::Write(self.seat());
            }
            Press::Back => {
                self.begin_close(now);
                return Outcome::Nothing;
            }
        };
        if self.waiting() {
            return Outcome::Nothing;
        }
        if let Some(progress) = self.hop_progress(now) {
            // Mid-hop: pressing toward the slot we are already heading to is
            // ignored; pressing back turns the chip round from where it is.
            let Some(target) = target else {
                return Outcome::Nothing;
            };
            if target == self.seat {
                return Outcome::Nothing;
            }
            let done = ((1.0 - progress) * HOP_MS as f32) as Millis;
            self.hop = Some(Hop {
                from: self.seat,
                to: target,
                started: now.saturating_sub(done),
            });
            self.seat = target;
            self.refusal = None;
            return Outcome::Nothing;
        }
        let Some(target) = target else {
            // At the edge of the options: shake instead of moving.
            self.refusal = Some(Refusal::started(now));
            return Outcome::Refused;
        };
        self.hop = Some(Hop {
            from: self.seat,
            to: target,
            started: now,
        });
        self.seat = target;
        self.refusal = None;
        Outcome::Nothing
    }

    pub fn chip(&self, now: Millis) -> Chip {
        let shake = self.refusal.map_or(0.0, |r| r.offset(now));
        if let Some((from, to, q)) = self.hop_state(now) {
            let arc = (std::f32::consts::PI * q).sin();
            let rightward = to > from;
            let lean = if rightward { 1.0 } else { -1.0 };
            return Chip {
                across: self.across_between(from, to, q),
                lift: arc,
                tip: lean * CHIP_TIP * arc,
                seated: None,
                shake,
            };
        }
        Chip {
            across: if self.is_single() {
                0.5
            } else {
                self.seat as f32 / (self.options.len() - 1) as f32
            },
            lift: 0.0,
            tip: 0.0,
            seated: Some(self.seat()),
            shake,
        }
    }

    fn across_between(&self, from: usize, to: usize, q: f32) -> f32 {
        let denom = (self.options.len() - 1).max(1) as f32;
        let f0 = from as f32 / denom;
        let f1 = to as f32 / denom;
        // 线性插值（ease 已对称）：向右 0→1 得 ease(q)，向左 1→0 得 1-ease(q)，
        // 与旧版两条分支等价；掉头时 q_new = 1 - q_old 正好保持连续。
        f0 + (f1 - f0) * ease(q)
    }

    fn begin_close(&mut self, now: Millis) {
        self.close = Some(Close {
            started: now,
            from: self.openness(now),
        });
    }

    fn hop_state(&self, now: Millis) -> Option<(usize, usize, f32)> {
        let hop = self.hop?;
        let q = now.saturating_sub(hop.started) as f32 / HOP_MS as f32;
        (q < 1.0).then_some((hop.from, hop.to, q))
    }

    fn hop_progress(&self, now: Millis) -> Option<f32> {
        self.hop_state(now).map(|(_, _, q)| q)
    }
}
