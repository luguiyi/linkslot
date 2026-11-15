//! Chinese input-method search UI in the shared Nintendo-Switch-inspired style.
//!
//! Two faces are produced for the app:
//!
//! * [`candidate_bar`] is the small translucent candidate strip that sits under
//!   the keyboard's input card (640×40 at [`CANDIDATE_POS`]).
//! * [`results_face`] is the full-screen, all-platform search result list with
//!   seven rows per page.
//!
//! Only drawing lives here; paging, focus and query state are owned by the app.

use slot_i18n::Language;
use slot_store::Platform;

use crate::collection_style;
use crate::draw::{OUT_H, OUT_W};
use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

/// Top-left corner the app draws the candidate bar at, just under the keyboard
/// input card.
pub const CANDIDATE_POS: (f32, f32) = (40.0, 146.0);
/// Pixel size of the candidate strip texture.
pub const CANDIDATE_SIZE: (f32, f32) = (BAR_W as f32, BAR_H as f32);

const BAR_W: u32 = 640;
const BAR_H: u32 = 40;
const PAD: f32 = 10.0;
const TEXT_PX: f32 = 19.0;
const MIN_PX: f32 = 13.0;
const FOCUS_PAD: f32 = 8.0;
const CHIP_H: f32 = 28.0;
const CHIP_RADIUS: f32 = 7.0;
const CANDIDATE_GAP: f32 = 10.0;
const COMPOSITION_MAX_W: f32 = 170.0;
const PAGE_PX: f32 = 14.0;
const PAGE_MAX_W: f32 = 56.0;
const ELLIPSIS: &str = "…";

const HEADER_H: f32 = 88.0;
const SIDE: f32 = 48.0;
const TITLE_PX: f32 = 28.0;
const QUERY_PX: f32 = 16.0;
const LIST_TOP: f32 = 96.0;
const ROW_X: f32 = 50.0;
const ROW_W: f32 = 620.0;
const ROW_H: f32 = 42.0;
const ROW_PITCH: f32 = 48.0;
const ROWS: usize = 7;
const ROW_NAME_PX: f32 = 19.0;
const ROW_MIN_PX: f32 = 12.0;
const TAG_PX: f32 = 13.0;
const TAG_MIN_PX: f32 = 9.0;
const TAG_MAX_W: f32 = 70.0;
const TAG_H: f32 = 22.0;
const HINT_Y: f32 = 440.0;
const HINT_H: f32 = 28.0;

/// The translucent candidate strip shown under the keyboard input card.
///
/// The left side echoes the current composition (`拼: baokemeng`), the right
/// side lists the candidates and the page indicator.  The focused candidate is
/// a white chip with a red rim; when the list does not fit, a window around the
/// focus is shown with ellipses and long labels are trimmed.
pub fn candidate_bar(
    composition: &str,
    candidates: &[String],
    selected: usize,
    page: usize,
    pages: usize,
    lang: Language,
) -> UndoFace {
    let mut face = sw::rounded_face(
        BAR_W,
        BAR_H,
        10.0,
        [sw::bg()[0], sw::bg()[1], sw::bg()[2], 0xe6],
        Some([
            sw::surface_border()[0],
            sw::surface_border()[1],
            sw::surface_border()[2],
            0xd0,
        ]),
    );
    let centre = BAR_H as f32 / 2.0;

    let label = if composition.is_empty() {
        slot_i18n::t(lang, "device.app.search.compose_empty").to_string()
    } else {
        slot_i18n::t_args(
            lang,
            "device.app.search.compose",
            &[("text", composition)],
        )
    };
    let (label, label_px) = kb::fit_line(&label, COMPOSITION_MAX_W, TEXT_PX, MIN_PX);
    let label_w = kb::ink_width(&label, label_px);
    kb::draw_left(
        &mut face,
        &label,
        label_px,
        label_px,
        COMPOSITION_MAX_W,
        PAD,
        0.0,
        BAR_H as f32,
        sw::ink(),
    );
    let separator_x = (PAD + label_w + 10.0).min(COMPOSITION_MAX_W + PAD + 10.0);
    kb::fill(
        &mut face,
        separator_x,
        7.0,
        1.0,
        BAR_H as f32 - 14.0,
        [
            sw::surface_border()[0],
            sw::surface_border()[1],
            sw::surface_border()[2],
            0x90,
        ],
    );

    let pages = pages.max(1);
    let page = page.min(pages - 1);
    let page_label = format!("{}/{}", page + 1, pages);
    right_text(
        &mut face,
        &page_label,
        PAGE_PX,
        OUT_W.min(BAR_W) as f32 - PAD,
        0.0,
        BAR_H as f32,
        sw::ink_dim(),
    );
    let page_w = kb::ink_width(&page_label, PAGE_PX).min(PAGE_MAX_W);

    let region_x = separator_x + 10.0;
    let region_right = BAR_W as f32 - PAD - page_w - 8.0;
    let region_w = (region_right - region_x).max(40.0);
    let value_y = (centre - CHIP_H / 2.0).max(0.0);

    if candidates.is_empty() {
        kb::draw_left(
            &mut face,
            slot_i18n::t(lang, "device.app.search.no_candidates"),
            TEXT_PX,
            MIN_PX,
            region_w,
            region_x,
            0.0,
            BAR_H as f32,
            sw::ink_dim(),
        );
        return face;
    }

    let focus = selected.min(candidates.len() - 1);
    let mut px = TEXT_PX;
    while px > MIN_PX && all_width(candidates, px) > region_w {
        px -= 1.0;
    }
    let (start, end) = if all_width(candidates, px) <= region_w {
        (0, candidates.len())
    } else {
        window_around(candidates, focus, px, region_w)
    };
    let ell_w = kb::ink_width(ELLIPSIS, px);
    let tail = if end < candidates.len() {
        ell_w + CANDIDATE_GAP
    } else {
        0.0
    };

    let mut x = region_x;
    if start > 0 {
        kb::draw_left(
            &mut face,
            ELLIPSIS,
            px,
            px,
            ell_w.max(1.0),
            x,
            0.0,
            BAR_H as f32,
            sw::ink_dim(),
        );
        x += ell_w + CANDIDATE_GAP;
    }
    for (i, candidate) in candidates.iter().enumerate().skip(start).take(end - start) {
        let focused = i == focus;
        let remaining = (region_right - x - tail - CANDIDATE_GAP).max(16.0);
        let text_limit = if focused {
            (remaining - 2.0 * FOCUS_PAD).max(8.0)
        } else {
            remaining
        };
        let (text, text_px) = kb::fit_line(candidate, text_limit, px, MIN_PX);
        let ink = kb::ink_width(&text, text_px);
        if focused {
            let chip_w = ink + 2.0 * FOCUS_PAD;
            let chip_x = x;
            sw::fill_round(
                &mut face,
                chip_x,
                value_y,
                chip_w,
                CHIP_H,
                CHIP_RADIUS,
                accent(),
            );
            sw::fill_round(
                &mut face,
                chip_x + 2.0,
                value_y + 2.0,
                chip_w - 4.0,
                CHIP_H - 4.0,
                (CHIP_RADIUS - 2.0).max(0.0),
                sw::selected(),
            );
            kb::draw_left(
                &mut face,
                &text,
                text_px,
                text_px,
                ink.max(1.0),
                chip_x + FOCUS_PAD,
                value_y,
                CHIP_H,
                sw::ink_selected(),
            );
            x += chip_w + CANDIDATE_GAP;
        } else {
            kb::draw_left(
                &mut face,
                &text,
                text_px,
                text_px,
                ink.max(1.0),
                x,
                0.0,
                BAR_H as f32,
                sw::ink(),
            );
            x += ink + CANDIDATE_GAP;
        }
    }
    if end < candidates.len() {
        let ex = x.min((region_right - ell_w).max(region_x));
        kb::draw_left(
            &mut face,
            ELLIPSIS,
            px,
            px,
            ell_w.max(1.0),
            ex,
            0.0,
            BAR_H as f32,
            sw::ink_dim(),
        );
    }
    face
}

fn all_width(candidates: &[String], px: f32) -> f32 {
    let chips: f32 = candidates
        .iter()
        .map(|text| kb::ink_width(text, px) + 2.0 * FOCUS_PAD)
        .sum();
    chips + CANDIDATE_GAP * candidates.len().saturating_sub(1) as f32
}

fn window_cost(candidates: &[String], start: usize, end: usize, px: f32) -> f32 {
    let mut width = 0.0;
    for text in &candidates[start..end] {
        width += kb::ink_width(text, px) + 2.0 * FOCUS_PAD;
    }
    width += CANDIDATE_GAP * (end - start).saturating_sub(1) as f32;
    let ell = kb::ink_width(ELLIPSIS, px);
    if start > 0 {
        width += ell + CANDIDATE_GAP;
    }
    if end < candidates.len() {
        width += ell + CANDIDATE_GAP;
    }
    width
}

/// Pick the widest run of candidates around `focus` that fits `available`.
fn window_around(candidates: &[String], focus: usize, px: f32, available: f32) -> (usize, usize) {
    let (mut start, mut end) = (focus, (focus + 1).min(candidates.len()));
    loop {
        let mut changed = false;
        if end < candidates.len() && window_cost(candidates, start, end + 1, px) <= available {
            end += 1;
            changed = true;
        }
        if start > 0 && window_cost(candidates, start - 1, end, px) <= available {
            start -= 1;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    (start, end)
}

/// The full-screen all-platform search result list.
///
/// Seven rounded rows per page from y = 96 (pitch 48), the selected row drawn as
/// a white card with dark type, a platform tag on the left and the game name to
/// its right.  The header carries the title, the query and the page number; the
/// key hints sit at the bottom.
pub fn results_face(
    query: &str,
    rows: &[(Platform, String)],
    selected: usize,
    offset: usize,
    lang: Language,
) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    kb::fill(&mut face, 0.0, 0.0, OUT_W as f32, HEADER_H, sw::bg_top());
    kb::fill(&mut face, SIDE, 16.0, 5.0, 30.0, accent());
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.app.search.results_title"),
        TITLE_PX,
        15.0,
        64.0,
        8.0,
        OUT_W as f32 - 2.0 * 64.0,
        44.0,
        sw::ink(),
    );

    let count = rows.len().to_string();
    let query_label = if query.is_empty() {
        slot_i18n::t_args(
            lang,
            "device.app.search.total",
            &[("count", count.as_str())],
        )
    } else {
        slot_i18n::t_args(
            lang,
            "device.app.search.query",
            &[("query", query), ("count", count.as_str())],
        )
    };
    let (query_line, query_px) = kb::fit_line(&query_label, 430.0, QUERY_PX, 11.0);
    kb::draw_left(
        &mut face,
        &query_line,
        query_px,
        query_px,
        430.0,
        64.0,
        52.0,
        24.0,
        sw::ink_dim(),
    );

    let first = if rows.is_empty() {
        0
    } else {
        offset.min(rows.len() - 1)
    };
    if !rows.is_empty() {
        let pages = rows.len().div_ceil(ROWS);
        let page = first / ROWS + 1;
        right_text(
            &mut face,
            &slot_i18n::t_args(
                lang,
                "device.app.search.page",
                &[
                    ("page", page.to_string().as_str()),
                    ("pages", pages.to_string().as_str()),
                ],
            ),
            14.0,
            OUT_W as f32 - SIDE,
            52.0,
            24.0,
            sw::ink_dim(),
        );
    }

    if rows.is_empty() {
        kb::draw_box(
            &mut face,
            slot_i18n::t(lang, "device.app.search.empty"),
            22.0,
            14.0,
            SIDE,
            180.0,
            OUT_W as f32 - 2.0 * SIDE,
            60.0,
            sw::ink_dim(),
        );
    } else {
        for i in 0..ROWS {
            let index = first + i;
            if index >= rows.len() {
                break;
            }
            let y = LIST_TOP + i as f32 * ROW_PITCH;
            let (platform, name) = &rows[index];
            result_row(&mut face, y, *platform, name, selected == index);
        }
    }

    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.app.search.results_hint"),
        16.0,
        11.0,
        SIDE,
        HINT_Y,
        OUT_W as f32 - 2.0 * SIDE,
        HINT_H,
        sw::ink_dim(),
    );
    face
}

fn result_row(face: &mut UndoFace, y: f32, platform: Platform, name: &str, selected: bool) {
    card_at(face, ROW_X, y, ROW_W, ROW_H, selected);
    let style = collection_style::platform(platform);
    let tag = {
        let (text, px) = kb::fit_line(platform.dir_name(), TAG_MAX_W, TAG_PX, TAG_MIN_PX);
        let ink = kb::ink_width(&text, px);
        (text, px, ink)
    };
    let tag_w = tag.2 + 14.0;
    let tag_x = ROW_X + 16.0;
    let tag_y = y + (ROW_H - TAG_H) / 2.0;
    let (fill, border) = collection_style::chip(style, selected);
    sw::fill_round(face, tag_x, tag_y, tag_w, TAG_H, 6.0, border);
    sw::fill_round(
        face,
        tag_x + 1.0,
        tag_y + 1.0,
        tag_w - 2.0,
        TAG_H - 2.0,
        5.0,
        fill,
    );
    kb::draw_left(
        face,
        &tag.0,
        tag.1,
        tag.1,
        tag.2.max(1.0),
        tag_x + 7.0,
        tag_y,
        TAG_H,
        collection_style::ink(style, selected),
    );
    let name_x = tag_x + tag_w + 16.0;
    let name_w = (ROW_X + ROW_W - 16.0 - name_x).max(40.0);
    let ink = if selected { sw::ink_selected() } else { sw::ink() };
    kb::draw_left(
        face,
        name,
        ROW_NAME_PX,
        ROW_MIN_PX,
        name_w,
        name_x,
        y,
        ROW_H,
        ink,
    );
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

/// A Switch-style rounded card: white when selected, charcoal surface when not.
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

/// Draw one fitted line right-aligned to `right`.
fn right_text(
    face: &mut UndoFace,
    text: &str,
    px: f32,
    right: f32,
    y: f32,
    h: f32,
    colour: [u8; 3],
) {
    let (line, px) = kb::fit_line(text, PAGE_MAX_W.max(64.0), px, 8.0);
    let ink = kb::ink_width(&line, px);
    kb::draw_left(
        face,
        &line,
        px,
        px,
        ink.max(1.0),
        (right - ink).max(0.0),
        y,
        h,
        colour,
    );
}
