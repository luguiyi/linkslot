//! Overlay panels in the shared Nintendo-Switch-inspired style: the save-album
//! / utility panel, the game-title heading and the letter picker.  Only the
//! drawing changed; the pagination and A/B/left-right input logic is untouched.

use slot_i18n::Language;

use crate::draw::OUT_W;
use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

const HEADER_H: f32 = 88.0;
const SIDE: f32 = 48.0;
const CARD_W: f32 = OUT_W as f32 - 2.0 * SIDE;
const LIST_TOP: f32 = 92.0;
const LIST_H: f32 = 280.0;
const ROW_TOP: f32 = 104.0;
/// One prose card: a title line plus three wrapped lines of detail.
const NOTICE_CARD_H: f32 = 184.0;
const ROW_H: f32 = 40.0;
const ROW_GAP: f32 = 4.0;
const LIST_ROWS: usize = 6;
const EMPTY_CARD_H: f32 = 200.0;
const PAGE_Y: f32 = 392.0;
const HINT_Y: f32 = 424.0;
const HINT_H: f32 = 28.0;

const PICKER_COLS: usize = 7;
const PICKER_CELL_W: f32 = 72.0;
const PICKER_CELL_H: f32 = 56.0;
const PICKER_GAP: f32 = 10.0;
const PICKER_TOP: f32 = 110.0;

/// The game-title heading shown at the top of the navigation overlay: white
/// title type on a transparent face.  The shelf reuses this as an optional
/// title strip, so it deliberately carries no menu accent bar.
pub fn title(text: &str) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (664 * 76 * 4) as usize],
        w: 664,
        h: 76,
    };
    kb::draw_box(&mut face, text, 32.0, 16.0, 30.0, 8.0, 610.0, 60.0, sw::ink());
    face
}

/// The letter picker: a 7×4 grid of rounded cells.  Available letters are light
/// on charcoal, unavailable ones are dimmer and the selection is a white cell
/// with a red rim and dark type.
pub fn picker(index: u8, available: [bool; 26], lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, 480, sw::bg());
    heading(&mut face, slot_i18n::t(lang, "device.quick.nav.picker_title"));
    let index = (index as usize).min(available.len().saturating_sub(1));
    let grid_w = PICKER_COLS as f32 * PICKER_CELL_W + (PICKER_COLS - 1) as f32 * PICKER_GAP;
    let left = ((OUT_W as f32 - grid_w) / 2.0).round();
    for (i, &avail) in available.iter().enumerate() {
        let col = i % PICKER_COLS;
        let row = i / PICKER_COLS;
        let x = left + col as f32 * (PICKER_CELL_W + PICKER_GAP);
        let y = PICKER_TOP + row as f32 * (PICKER_CELL_H + PICKER_GAP);
        let lit = i == index;
        if lit {
            sw::fill_round(
                &mut face,
                x,
                y,
                PICKER_CELL_W,
                PICKER_CELL_H,
                sw::CARD_RADIUS,
                accent(),
            );
            sw::fill_round(
                &mut face,
                x + 2.0,
                y + 2.0,
                PICKER_CELL_W - 4.0,
                PICKER_CELL_H - 4.0,
                (sw::CARD_RADIUS - 2.0).max(0.0),
                sw::selected(),
            );
        } else if avail {
            card_at(&mut face, x, y, PICKER_CELL_W, PICKER_CELL_H, false);
        } else {
            sw::fill_round(
                &mut face,
                x,
                y,
                PICKER_CELL_W,
                PICKER_CELL_H,
                sw::CARD_RADIUS,
                [0x24, 0x24, 0x29, 0xff],
            );
        }
        let ink = if lit {
            sw::ink_selected()
        } else if avail {
            sw::ink()
        } else {
            [0x5a, 0x5a, 0x62]
        };
        let label = char::from(b'A' + i as u8).to_string();
        kb::draw_box(
            &mut face,
            &label,
            30.0,
            16.0,
            x,
            y,
            PICKER_CELL_W,
            PICKER_CELL_H,
            ink,
        );
    }
    kb::draw_box(
        &mut face,
        if available[index] {
            slot_i18n::t(lang, "device.quick.nav.picker_hint")
        } else {
            slot_i18n::t(lang, "device.quick.nav.picker_empty")
        },
        18.0,
        12.0,
        SIDE,
        HINT_Y,
        CARD_W,
        HINT_H,
        sw::ink_dim(),
    );
    face
}

/// A notice: the same heading and card family as [`panel`], but nothing is
/// selectable and there is one thing to press.
///
/// Use this wherever a dialog would ask the player to "confirm" something whose
/// every key already did the same thing — a menu there is just a keypress tax.
pub fn notice(title: &str, lines: &[String], lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, 480, sw::bg());
    heading(&mut face, title);
    let height = NOTICE_CARD_H.min(ROW_TOP + 200.0);
    card_at(&mut face, SIDE, ROW_TOP, CARD_W, height, false);
    let mut y = ROW_TOP + 18.0;
    for (i, line) in lines.iter().enumerate() {
        let ink = if i == 0 { sw::ink() } else { sw::ink_dim() };
        kb::draw_wrapped(
            &mut face,
            line,
            20.0,
            12.0,
            SIDE + 28.0,
            y,
            CARD_W - 56.0,
            34.0,
            2,
            ink,
        );
        y += 40.0;
    }
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.nav.notice_hint"),
        16.0,
        11.0,
        SIDE,
        HINT_Y,
        CARD_W,
        HINT_H,
        sw::ink_dim(),
    );
    face
}

/// The save-album / utility panel: a heading band, a rounded list area with one
/// white card for the selected row and the page / key hints at the bottom.
/// Six rows per page by default; the in-game menu asks for seven so it fits on
/// one screen.
pub fn panel(title: &str, rows: &[String], selected: usize, lang: Language) -> UndoFace {
    panel_with_limit(title, rows, selected, LIST_ROWS, lang)
}

/// [`panel`] with an explicit page size.  Seven rows use a slightly shorter
/// row pitch so the last row still clears the page label and the key hints.
pub fn panel_with_limit(
    title: &str,
    rows: &[String],
    selected: usize,
    rows_per_page: usize,
    lang: Language,
) -> UndoFace {
    let mut face = kb::blank(OUT_W, 480, sw::bg());
    heading(&mut face, title);
    if rows.is_empty() {
        card_at(&mut face, SIDE, ROW_TOP, CARD_W, EMPTY_CARD_H, false);
        kb::draw_box(
            &mut face,
            slot_i18n::t(lang, "device.quick.nav.empty_states"),
            24.0,
            14.0,
            SIDE,
            ROW_TOP,
            CARD_W,
            EMPTY_CARD_H,
            sw::ink_dim(),
        );
        kb::draw_box(
            &mut face,
            slot_i18n::t(lang, "device.quick.nav.panel_back"),
            18.0,
            12.0,
            SIDE,
            HINT_Y,
            CARD_W,
            HINT_H,
            sw::ink_dim(),
        );
        return face;
    }
    let rows_per_page = rows_per_page.max(1);
    let compact = rows_per_page > LIST_ROWS;
    let (row_h, row_gap) = if compact {
        (34.0, 4.0)
    } else {
        (ROW_H, ROW_GAP)
    };
    // One rounded content area behind the rows.
    sw::fill_round(&mut face, SIDE, LIST_TOP, CARD_W, LIST_H, 14.0, sw::surface());
    let start = selected / rows_per_page * rows_per_page;
    for (i, text) in rows.iter().enumerate().skip(start).take(rows_per_page) {
        let row = i - start;
        let y = ROW_TOP + row as f32 * (row_h + row_gap);
        let lit = i == selected;
        if lit {
            card_at(&mut face, SIDE + 12.0, y, CARD_W - 24.0, row_h - 4.0, true);
        }
        // A row whose label says it is unavailable (SFC's link row today) is
        // drawn dim even though it can still be focused, so the player sees the
        // state before pressing it.
        // The suffix is the row's own marker, so it follows the language too.
        // WP2 supplies the English marker; until then an English row simply
        // keeps the normal ink, which is a shade, not a bug.
        let disabled = text.ends_with("（暂不支持）") || text.ends_with("(not supported)");
        let ink = if lit && !disabled {
            sw::ink_selected()
        } else if disabled {
            sw::ink_dim()
        } else {
            sw::ink()
        };
        kb::draw_left(
            &mut face,
            text,
            20.0,
            12.0,
            CARD_W - 72.0,
            SIDE + 36.0,
            y,
            row_h,
            ink,
        );
    }
    let pages = rows.len().max(1).div_ceil(rows_per_page);
    let page_label = if pages <= 1 {
        slot_i18n::t_args(
            lang,
            "device.quick.nav.panel_count",
            &[("count", &rows.len().to_string())],
        )
    } else {
        slot_i18n::t_args(
            lang,
            "device.quick.nav.panel_page",
            &[
                ("page", &(start / rows_per_page + 1).to_string()),
                ("pages", &pages.to_string()),
            ],
        )
    };
    kb::draw_box(
        &mut face,
        &page_label,
        16.0,
        11.0,
        SIDE,
        PAGE_Y,
        CARD_W,
        24.0,
        sw::ink_dim(),
    );
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.nav.panel_hint"),
        16.0,
        11.0,
        SIDE,
        HINT_Y,
        CARD_W,
        HINT_H,
        sw::ink_dim(),
    );
    face
}

/// Where the hold-to-leave prompt sits, in output pixels: `[x, y, w, h]`.
///
/// Low on the screen on purpose: it replaces the panel's footer hint instead of
/// covering a menu row, so the list the player is looking at stays readable
/// while the bar fills.
const HOLD_CARD: [f32; 4] = [140.0, 392.0, 440.0, 76.0];

/// The fill area of the prompt's bar, in output pixels: `[x, y, w, h]`.
pub const HOLD_BAR: [f32; 4] = [164.0, 450.0, 392.0, 10.0];

/// The colour the bar fills with.
pub fn hold_fill() -> [f32; 4] {
    let accent = sw::accent();
    [
        accent[0] as f32 / 255.0,
        accent[1] as f32 / 255.0,
        accent[2] as f32 / 255.0,
        1.0,
    ]
}

/// The "keep holding MENU to leave" prompt.
///
/// A full-screen face that is transparent everywhere except one card: the game
/// menu stays readable underneath. The bar is *not* baked in — the caller draws
/// it with [`HOLD_BAR`] and the progress it has — so this face is built once
/// and never rebuilt, and the bar moves at frame rate for free.
pub fn hold_card(lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, 480, [0, 0, 0, 0]);
    let [x, y, w, h] = HOLD_CARD;
    sw::fill_round(&mut face, x, y, w, h, 16.0, sw::surface());
    // An accent spine on the left, the way the page headings carry one: this
    // card is an action in progress, not another list.
    kb::fill(
        &mut face,
        x + 4.0,
        y + 14.0,
        4.0,
        h - 28.0,
        [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff],
    );
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.nav.hold_title"),
        19.0,
        13.0,
        x + 24.0,
        y + 12.0,
        w - 48.0,
        24.0,
        sw::ink(),
    );
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.nav.hold_hint"),
        12.0,
        9.0,
        x + 24.0,
        y + 36.0,
        w - 48.0,
        18.0,
        sw::ink_dim(),
    );
    let [bx, by, bw, bh] = HOLD_BAR;
    sw::fill_round(&mut face, bx, by, bw, bh, bh / 2.0, sw::bg_top());
    face
}

/// The Switch heading band: charcoal ground, red accent bar and white title.
fn heading(face: &mut UndoFace, title: &str) {
    kb::fill(face, 0.0, 0.0, OUT_W as f32, HEADER_H, sw::bg_top());
    kb::fill(face, SIDE, 16.0, 5.0, 30.0, accent());
    kb::draw_box(
        face,
        title,
        28.0,
        15.0,
        64.0,
        12.0,
        OUT_W as f32 - 96.0,
        40.0,
        sw::ink(),
    );
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

/// A Switch-style rounded card: white with a white rim when selected, charcoal
/// surface with a light rim otherwise.
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

/// Translation keys for the help page's lines, in the order it shows them.
const HELP_KEYS: [&'static str; 36] = [
    "device.quick.help.shelf_pick",
    "device.quick.help.shelf_search",
    "device.quick.help.shelf_settings",
    "device.quick.help.shelf_network",
    "device.quick.help.shelf_core",
    "device.quick.help.shelf_restart",
    "device.quick.help.shelf_sync",
    "device.quick.help.sync_conflict",
    "device.quick.help.game_menu",
    "device.quick.help.game_exit",
    "device.quick.help.album_open",
    "device.quick.help.state_shortcuts",
    "device.quick.help.album_browse",
    "device.quick.help.album_delete",
    "device.quick.help.rewind",
    "device.quick.help.fast_forward",
    "device.quick.help.brightness",
    "device.quick.help.blue_light",
    "device.quick.help.shader",
    "device.quick.help.colour",
    "device.quick.help.palette",
    "device.quick.help.volume",
    "device.quick.help.mute",
    "device.quick.help.link",
    "device.quick.help.turbo",
    "device.quick.help.gb_video",
    "device.quick.help.sfc_video",
    "device.quick.help.power",
    "device.quick.help.lid",
    "device.quick.help.power_menu",
    "device.quick.help.chord",
    "device.quick.help.swap_shoulders",
    "device.quick.help.xy_shoulders",
    "device.quick.help.both_shoulders",
    "device.quick.help.cheats",
    "device.quick.help.cheats_default",
];

/// Actual input bindings in `lang`; shoulder combinations are stated in the
/// default layout.
///
/// Returned by value: an array of `&'static str` is `Copy`, so this costs the
/// same as handing out a slice while letting the language pick the text.
pub fn help(lang: Language) -> [&'static str; HELP_KEYS.len()] {
    HELP_KEYS.map(|key| slot_i18n::t(lang, key))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(face: &UndoFace, x: u32, y: u32) -> u8 {
        face.rgba[((y * face.w + x) * 4 + 3) as usize]
    }

    fn rgb(face: &UndoFace, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
    }

    #[test]
    fn the_shelf_title_has_no_menu_accent_bar() {
        let face = title("宝可梦");
        assert_eq!(alpha(&face, 8, 30), 0, "the old red shelf bar came back");
        assert!(
            face.rgba.chunks(4).any(|pixel| pixel[3] > 0),
            "the title type disappeared"
        );
    }

    fn white_pixels_below(face: &UndoFace, y0: u32) -> usize {
        let mut count = 0;
        for y in y0..face.h {
            for x in 0..face.w {
                if rgb(face, x, y) == [0xf2, 0xf2, 0xf2] {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn the_letter_picker_is_a_switch_face_with_one_white_selection() {
        let available = [true; 26];
        let face = picker(4, available, Language::ZhCn);
        assert_eq!((face.w, face.h), (720, 480));
        assert_eq!(
            (rgb(&face, 0, 0), alpha(&face, 0, 0)),
            ([0x24, 0x24, 0x29], 255)
        );
        assert_eq!(rgb(&face, 50, 20), [0xe6, 0x00, 0x12], "the accent bar");
        assert!(white_pixels_below(&face, PICKER_TOP as u32) > 0);
        let dim = [false; 26];
        assert!(
            white_pixels_below(&picker(0, dim, Language::ZhCn), PICKER_TOP as u32) > 0,
            "the selected cell is not white even when unavailable"
        );
    }

    #[test]
    fn the_panel_puts_the_selected_row_on_a_white_card() {
        let rows: Vec<String> = (0..8).map(|i| format!("即时存档 {i}")).collect();
        let top = panel("存档相册", &rows, 0, Language::ZhCn);
        let lower = panel("存档相册", &rows, 7, Language::ZhCn);
        assert_eq!((top.w, top.h), (720, 480));
        assert_eq!(rgb(&top, 0, 0), [0x24, 0x24, 0x29]);
        let card_row = |face: &UndoFace| white_pixels_below(face, ROW_TOP as u32);
        assert!(card_row(&top) > 0);
        assert!(card_row(&lower) > 0);
        assert_ne!(top.rgba, lower.rgba, "the paged selection did not move");
        let empty: [String; 0] = [];
        let blank = panel("存档相册", &empty, 0, Language::ZhCn);
        assert_eq!(blank.rgba.len(), (720 * 480 * 4) as usize);
    }
}
