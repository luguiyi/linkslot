//! The on-device "mod 设置" page: one card per installed mod with the same
//! track-and-knob switch as the Wi-Fi page.  A toggles the selected mod's
//! total `enabled` flag; B returns to the quick menu.
//!
//! The app owns the list; this module only rasterises the snapshot.

use slot_i18n::Language;

use slot_gfx::{OUT_H, OUT_W};

use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsMenuRow<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub action_count: usize,
    pub enabled: bool,
    pub error: Option<&'a str>,
    /// 1-based priority, matching the row's place in the list: 1 takes the
    /// shelf long-press when two mods ask for it.
    pub rank: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsMenuView<'a> {
    pub rows: &'a [ModsMenuRow<'a>],
    /// Absolute selected index; clamped when the list shrinks.
    pub selected: usize,
    /// First visible row.
    pub offset: usize,
    /// A list-wide error (read failure, toggle failure, read-only card).
    pub error: Option<&'a str>,
}

const HEADER_H: f32 = 88.0;
const SIDE: f32 = 48.0;
const CARD_W: f32 = OUT_W as f32 - 2.0 * SIDE;
const LIST_TOP: f32 = 96.0;
const ROW_H: f32 = 54.0;
const ROW_GAP: f32 = 6.0;
const MAX_ROWS: usize = 5;
const ERROR_TOP: f32 = 398.0;
const HINT_TOP: f32 = 444.0;
const HINT_H: f32 = 28.0;
const RAIL_W: f32 = 76.0;
const RAIL_H: f32 = 26.0;
const KNOB: f32 = 22.0;
const RAIL_OFF: [u8; 4] = [0x5a, 0x5a, 0x64, 0xff];

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

pub fn mods_menu(view: &ModsMenuView<'_>, lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    heading(&mut face, view.rows.len(), lang);

    if view.rows.is_empty() {
        let (message, colour) = match view.error {
            Some(error) => (error, accent_ink()),
            None => (slot_i18n::t(lang, "device.app.mods.none"), sw::ink_dim()),
        };
        card_at(
            &mut face,
            SIDE,
            LIST_TOP + 24.0,
            CARD_W,
            76.0,
            false,
        );
        kb::draw_box(
            &mut face,
            message,
            22.0,
            14.0,
            SIDE,
            LIST_TOP + 24.0,
            CARD_W,
            76.0,
            colour,
        );
        hint(&mut face, lang);
        return face;
    }

    let selected = view.selected.min(view.rows.len() - 1);
    let max_offset = view.rows.len().saturating_sub(MAX_ROWS);
    let start = view.offset.min(max_offset);
    for (index, row) in view
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(MAX_ROWS)
    {
        let y = LIST_TOP + (index - start) as f32 * (ROW_H + ROW_GAP);
        mod_row(&mut face, y, row, index == selected, lang);
    }

    if let Some(error) = view.error {
        kb::draw_left(
            &mut face,
            error,
            14.0,
            10.0,
            CARD_W,
            SIDE,
            ERROR_TOP,
            24.0,
            accent_ink(),
        );
    }
    hint(&mut face, lang);
    face
}

fn heading(face: &mut UndoFace, count: usize, lang: Language) {
    kb::fill(face, 0.0, 0.0, OUT_W as f32, HEADER_H, sw::bg_top());
    kb::fill(face, SIDE, 16.0, 5.0, 30.0, accent());
    kb::draw_box(
        face,
        slot_i18n::t(lang, "device.app.mods.settings"),
        28.0,
        15.0,
        64.0,
        12.0,
        OUT_W as f32 - 200.0,
        40.0,
        sw::ink(),
    );
    let count_text = slot_i18n::t_args(
        lang,
        "device.app.mods.count",
        &[("count", count.to_string().as_str())],
    );
    kb::draw_left(
        face,
        &count_text,
        16.0,
        11.0,
        120.0,
        OUT_W as f32 - SIDE - 120.0,
        20.0,
        40.0,
        sw::ink_dim(),
    );
}

fn hint(face: &mut UndoFace, lang: Language) {
    kb::draw_box(
        face,
        slot_i18n::t(lang, "device.app.mods.hint"),
        16.0,
        11.0,
        SIDE,
        HINT_TOP,
        CARD_W,
        HINT_H,
        sw::ink_dim(),
    );
}

fn card_at(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, selected: bool) {
    let (rim, fill) = if selected {
        (sw::selected_border(), sw::selected())
    } else {
        (sw::surface_border(), sw::surface())
    };
    sw::fill_round(face, x, y, w, h, sw::CARD_RADIUS, rim);
    sw::fill_round(
        face,
        x + 1.0,
        y + 1.0,
        w - 2.0,
        h - 2.0,
        (sw::CARD_RADIUS - 1.0).max(0.0),
        fill,
    );
}

fn mod_row(face: &mut UndoFace, y: f32, row: &ModsMenuRow<'_>, selected: bool, lang: Language) {
    card_at(face, SIDE, y, CARD_W, ROW_H, selected);
    let ink = if selected { sw::ink_selected() } else { sw::ink() };
    let dim = if selected {
        [0x4a, 0x4a, 0x54]
    } else {
        sw::ink_dim()
    };
    kb::draw_left(
        face,
        row.name,
        18.0,
        12.0,
        CARD_W - 160.0,
        SIDE + 24.0,
        y + 4.0,
        30.0,
        ink,
    );
    let status = match row.error {
        Some(error) => error.to_string(),
        None => slot_i18n::t_args(
            lang,
            "device.app.mods.status",
            &[
                ("rank", row.rank.to_string().as_str()),
                ("version", row.version),
                ("count", row.action_count.to_string().as_str()),
            ],
        ),
    };
    let status_colour = if row.error.is_some() { accent_ink() } else { dim };
    kb::draw_left(
        face,
        &status,
        12.0,
        9.0,
        CARD_W - 160.0,
        SIDE + 24.0,
        y + 30.0,
        18.0,
        status_colour,
    );

    let rail_x = OUT_W as f32 - SIDE - 24.0 - RAIL_W;
    let rail_y = y + (ROW_H - RAIL_H) / 2.0;
    let track = if row.enabled { accent() } else { RAIL_OFF };
    sw::fill_round(face, rail_x, rail_y, RAIL_W, RAIL_H, RAIL_H / 2.0, track);
    let knob_y = rail_y + (RAIL_H - KNOB) / 2.0;
    let knob_x = if row.enabled {
        rail_x + RAIL_W - 3.0 - KNOB
    } else {
        rail_x + 3.0
    };
    sw::fill_round(
        face,
        knob_x,
        knob_y,
        KNOB,
        KNOB,
        KNOB / 2.0,
        [0xff, 0xff, 0xff, 0xff],
    );
}

fn accent_ink() -> [u8; 3] {
    sw::accent()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &'static str, enabled: bool) -> ModsMenuRow<'static> {
        ModsMenuRow {
            name,
            version: "1.1.0",
            action_count: 1,
            enabled,
            error: None,
            rank: 1,
        }
    }

    #[test]
    fn face_is_full_screen_and_toggles_are_visible() {
        let rows = [row("GBAStation 电池存档同步", true), row("SaveDeck 桥接", false)];
        let view = ModsMenuView {
            rows: &rows,
            selected: 0,
            offset: 0,
            error: None,
        };
        let face = mods_menu(&view, Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert!(face.rgba.chunks(4).any(|p| p[0] > 48 && p[1] > 48));

        let other = ModsMenuView {
            selected: 1,
            ..view
        };
        assert_ne!(face.rgba, mods_menu(&other, Language::ZhCn).rgba);

        let off = [ModsMenuRow {
            enabled: false,
            ..row("Only", true)
        }];
        let on = [ModsMenuRow {
            enabled: true,
            ..row("Only", true)
        }];
        let base = ModsMenuView {
            rows: &off,
            selected: 0,
            offset: 0,
            error: None,
        };
        let on_view = ModsMenuView { rows: &on, ..base.clone() };
        assert_ne!(
            mods_menu(&base, Language::ZhCn).rgba,
            mods_menu(&on_view, Language::ZhCn).rgba
        );
    }

    #[test]
    fn empty_and_error_states_render() {
        let empty = ModsMenuView {
            rows: &[],
            selected: 0,
            offset: 0,
            error: Some("无法读取 mods 目录"),
        };
        let face = mods_menu(&empty, Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert!(face.rgba.chunks(4).any(|p| p[0] > 32));
    }
}
