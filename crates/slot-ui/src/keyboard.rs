//! The on-screen keyboard used by the Wi-Fi password / network-name prompts.
//!
//! Three pages of keys are laid out on a 720×480 face: lower case, upper case
//! and symbols.  The two letter pages are full 5×10 grids (letters, digits and
//! common punctuation); the symbol page adds the remaining ASCII punctuation.
//! Navigation is pure: a [`Cursor`] knows its layout, row and column and turns
//! button presses into a new cursor, so the app can keep it in its own state and
//! only ask for a [`panel`] texture when the layout changes.
//!
//! The face itself leaves the input line area (at [`INPUT_POS`]) empty: the app
//! draws [`input_line`] on top of it so the typed text can change every frame
//! without re-rasterising the keycaps.

use slot_i18n::Language;

use crate::draw::{OUT_H, OUT_W};
use crate::plate::UndoFace;
use crate::switch_style as sw;
use crate::text;

/// Number of key pages: lower case, upper case and symbols.
pub const LAYOUT_COUNT: usize = 3;

/// A key on the on-screen keyboard.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Key {
    /// A printable character; the app appends it to the field.
    Char(char),
    /// The space key.
    Space,
    /// The backspace key.
    Backspace,
    /// Cycle to the next key page.
    Layout,
    /// Accept the field.
    Confirm,
    /// Abandon the field.
    Cancel,
}

/// Which key the selection is on.  Row and column are clamped when read, so a
/// stale cursor never panics after the page's key count changes.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Cursor {
    pub layout: usize,
    pub row: usize,
    pub col: usize,
}

impl Cursor {
    /// Move up one key row, wrapping at the top.
    pub fn up(self) -> Cursor {
        let c = self.fixed();
        let page = rows(c.layout);
        let row = (c.row + page.len() - 1) % page.len();
        let col = c.col.min(page[row].len() - 1);
        Cursor {
            layout: c.layout,
            row,
            col,
        }
    }

    /// Move down one key row, wrapping at the bottom.
    pub fn down(self) -> Cursor {
        let c = self.fixed();
        let page = rows(c.layout);
        let row = (c.row + 1) % page.len();
        let col = c.col.min(page[row].len() - 1);
        Cursor {
            layout: c.layout,
            row,
            col,
        }
    }

    /// Move one key left, wrapping at the start of the row.
    pub fn left(self) -> Cursor {
        let c = self.fixed();
        let len = rows(c.layout)[c.row].len();
        Cursor {
            layout: c.layout,
            row: c.row,
            col: (c.col + len - 1) % len,
        }
    }

    /// Move one key right, wrapping at the end of the row.
    pub fn right(self) -> Cursor {
        let c = self.fixed();
        let len = rows(c.layout)[c.row].len();
        Cursor {
            layout: c.layout,
            row: c.row,
            col: (c.col + 1) % len,
        }
    }

    /// Switch to the next key page, keeping the nearest key selected.
    pub fn cycle_layout(self) -> Cursor {
        let c = self.fixed();
        let layout = (c.layout + 1) % LAYOUT_COUNT;
        let page = rows(layout);
        let row = c.row.min(page.len() - 1);
        let col = c.col.min(page[row].len() - 1);
        Cursor { layout, row, col }
    }

    /// The key under the cursor.
    pub fn key(self) -> Key {
        let c = self.fixed();
        key_of(rows(c.layout)[c.row][c.col])
    }

    fn fixed(self) -> Cursor {
        let layout = self.layout % LAYOUT_COUNT;
        let page = rows(layout);
        let row = self.row.min(page.len() - 1);
        let col = self.col.min(page[row].len() - 1);
        Cursor { layout, row, col }
    }
}

const SPACE: &str = "空格";
const BACKSPACE: &str = "删除";
const SWITCH: &str = "切换";
const CONFIRM: &str = "确定";
const CANCEL: &str = "取消";

const LOWER: &[&[&str]] = &[
    &["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"],
    &["a", "s", "d", "f", "g", "h", "j", "k", "l", ";"],
    &["z", "x", "c", "v", "b", "n", "m", ",", ".", "/"],
    &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
    &[
        SPACE, BACKSPACE, SWITCH, CONFIRM, CANCEL, "!", "?", ":", "*", "#",
    ],
];

const UPPER: &[&[&str]] = &[
    &["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
    &["A", "S", "D", "F", "G", "H", "J", "K", "L", ";"],
    &["Z", "X", "C", "V", "B", "N", "M", ",", ".", "/"],
    &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
    &[
        SPACE, BACKSPACE, SWITCH, CONFIRM, CANCEL, "!", "?", ":", "*", "#",
    ],
];

const SYMBOLS: &[&[&str]] = &[
    &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
    &["!", "@", "#", "$", "%", "^", "&", "*", "(", ")"],
    &["-", "_", "=", "+", "[", "]", "{", "}", ";", ":"],
    &["'", "\"", ",", ".", "<", ">", "/", "?", "\\", "|"],
    &["~", "`", SPACE, BACKSPACE, SWITCH, CONFIRM, CANCEL],
];

/// The keys of one page, row by row; the last row always carries the function
/// keys.  `layout` wraps, so an out-of-range index still yields a page.
pub fn rows(layout: usize) -> &'static [&'static [&'static str]] {
    match layout % LAYOUT_COUNT {
        0 => LOWER,
        1 => UPPER,
        _ => SYMBOLS,
    }
}

fn key_of(label: &str) -> Key {
    match label {
        SPACE => Key::Space,
        BACKSPACE => Key::Backspace,
        SWITCH => Key::Layout,
        CONFIRM => Key::Confirm,
        CANCEL => Key::Cancel,
        other => other.chars().next().map(Key::Char).unwrap_or(Key::Space),
    }
}

fn display_label(label: &'static str, lang: Language) -> &'static str {
    match label {
        SPACE => slot_i18n::t(lang, "device.app.keyboard.space"),
        BACKSPACE => slot_i18n::t(lang, "device.app.keyboard.backspace"),
        SWITCH => slot_i18n::t(lang, "device.app.keyboard.layout"),
        CONFIRM => slot_i18n::t(lang, "device.app.keyboard.confirm"),
        CANCEL => slot_i18n::t(lang, "device.app.keyboard.cancel"),
        _ => label,
    }
}

/// Top left corner of the input line texture.  The panel leaves this band (and
/// the 640×90 texture that goes there) blank.
pub const INPUT_POS: (f32, f32) = (40.0, 96.0);

const KEY_W: f32 = 60.0;
const KEY_H: f32 = 36.0;
const KEY_GAP: f32 = 6.0;
const KEY_RADIUS: f32 = 8.0;
const KEYS_TOP: f32 = 194.0;
const KEYS_BAND_H: f32 = 204.0;
const TITLE_X: f32 = 64.0;
const TITLE_TOP: f32 = 12.0;
const TITLE_H: f32 = 40.0;
const HINT_TOP: f32 = 408.0;
const HINT_H: f32 = 28.0;
const SIDE: f32 = 48.0;
const HEADER_H: f32 = 88.0;

/// Pixel rectangle `(x, y, w, h)` of the selected key cap, for the outline the
/// app draws over the panel.
pub fn row_rect(cursor: Cursor) -> (f32, f32, f32, f32) {
    let c = cursor.fixed();
    let (x, y) = key_origin(c.layout, c.row, c.col);
    (x, y, KEY_W, KEY_H)
}

fn page_height(rows: usize) -> f32 {
    rows as f32 * KEY_H + rows.saturating_sub(1) as f32 * KEY_GAP
}

fn page_top(rows: usize) -> f32 {
    KEYS_TOP + (KEYS_BAND_H - page_height(rows)) / 2.0
}

fn key_origin(layout: usize, row: usize, col: usize) -> (f32, f32) {
    let page = rows(layout);
    let row = row.min(page.len() - 1);
    let keys = page[row];
    let col = col.min(keys.len() - 1);
    let row_w = keys.len() as f32 * KEY_W + keys.len().saturating_sub(1) as f32 * KEY_GAP;
    let x = ((OUT_W as f32 - row_w) / 2.0).round() + col as f32 * (KEY_W + KEY_GAP);
    let y = page_top(page.len()) + row as f32 * (KEY_H + KEY_GAP);
    (x, y)
}

/// The keyboard face: Switch-style dark panel, rounded key caps, a heading and
/// the bottom hint.  The selected key is not drawn; the app outlines it.
/// The input band at [`INPUT_POS`] is deliberately empty.
pub fn panel(layout: usize, title: &str, hint: &str, lang: Language) -> UndoFace {
    keyboard_face(layout, title, hint, None, lang)
}

/// Like [`panel`], but the key under `cursor` is drawn as a white Switch-style
/// key with a red outline and dark type.
pub fn panel_selected(
    layout: usize,
    title: &str,
    hint: &str,
    cursor: Cursor,
    lang: Language,
) -> UndoFace {
    keyboard_face(layout, title, hint, Some(cursor.fixed()), lang)
}

/// The highlighted key cap alone, drawn over a cached base panel.  Moving the
/// cursor only has to upload this 60×36 texture instead of the whole 720×480
/// keyboard face.
pub fn selection_face(cursor: Cursor, lang: Language) -> UndoFace {
    const W: u32 = KEY_W as u32;
    const H: u32 = KEY_H as u32;
    let c = cursor.fixed();
    let page = rows(c.layout);
    let label = display_label(page[c.row][c.col], lang);
    let mut face = UndoFace {
        rgba: vec![0u8; (W * H * 4) as usize],
        w: W,
        h: H,
    };
    draw_key(
        &mut face,
        0.0,
        0.0,
        KEY_W,
        KEY_H,
        label,
        c.row + 1 == page.len(),
        true,
    );
    face
}

fn keyboard_face(
    layout: usize,
    title: &str,
    hint: &str,
    selected: Option<Cursor>,
    lang: Language,
) -> UndoFace {
    let mut face = blank(OUT_W, OUT_H, sw::bg());
    fill(&mut face, 0.0, 0.0, OUT_W as f32, HEADER_H, sw::bg_top());
    fill(&mut face, SIDE, 16.0, 5.0, 30.0, accent());
    let text_w = OUT_W as f32 - TITLE_X - 32.0;
    draw_box(
        &mut face,
        title,
        28.0,
        15.0,
        TITLE_X,
        TITLE_TOP,
        text_w,
        TITLE_H,
        sw::ink(),
    );
    let page = rows(layout);
    for (r, keys) in page.iter().enumerate() {
        let function_row = r + 1 == page.len();
        for c in 0..keys.len() {
            let cursor = Cursor {
                layout,
                row: r,
                col: c,
            };
            let (x, y, w, h) = row_rect(cursor);
            let lit = selected == Some(cursor.fixed());
            draw_key(&mut face, x, y, w, h, display_label(keys[c], lang), function_row, lit);
        }
    }
    draw_box(
        &mut face,
        hint,
        18.0,
        12.0,
        SIDE,
        HINT_TOP,
        OUT_W as f32 - 2.0 * SIDE,
        HINT_H,
        sw::ink_dim(),
    );
    face
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    face: &mut UndoFace,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    function_row: bool,
    selected: bool,
) {
    if selected {
        sw::fill_round(face, x, y, w, h, KEY_RADIUS, accent());
        sw::fill_round(
            face,
            x + 2.0,
            y + 2.0,
            w - 4.0,
            h - 4.0,
            (KEY_RADIUS - 2.0).max(0.0),
            sw::selected(),
        );
    } else {
        let border = if function_row {
            accent_dim()
        } else {
            sw::surface_border()
        };
        sw::fill_round(face, x, y, w, h, KEY_RADIUS, border);
        sw::fill_round(
            face,
            x + 1.0,
            y + 1.0,
            w - 2.0,
            h - 2.0,
            (KEY_RADIUS - 1.0).max(0.0),
            sw::surface(),
        );
    }
    let ink = if selected {
        sw::ink_selected()
    } else if function_row {
        sw::accent()
    } else {
        sw::ink()
    };
    let px = if label.chars().count() == 1 {
        24.0
    } else {
        17.0
    };
    draw_box(
        face,
        label,
        px,
        11.0,
        x + 2.0,
        y + 2.0,
        w - 4.0,
        h - 4.0,
        ink,
    );
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

fn accent_dim() -> [u8; 4] {
    [
        sw::accent_dim()[0],
        sw::accent_dim()[1],
        sw::accent_dim()[2],
        0xff,
    ]
}

/// The typed text line as a rounded Switch-style field, transparent outside the
/// card so it can sit on top of the panel: the field itself followed by the
/// error line (red) once the field is rejected.  `masked` replaces every
/// character with a bullet for password entry.
pub fn input_line(text: &str, masked: bool, error: Option<&str>) -> UndoFace {
    const W: u32 = 640;
    const H: u32 = 90;
    let mut face = sw::rounded_face(W, H, 12.0, sw::surface(), Some(sw::surface_border()));
    let shown = if masked {
        "•".repeat(text.chars().count())
    } else {
        text.to_string()
    };
    if !shown.is_empty() {
        draw_left(
            &mut face,
            &shown,
            28.0,
            16.0,
            592.0,
            24.0,
            4.0,
            52.0,
            sw::ink(),
        );
    }
    if let Some(message) = error {
        draw_left(
            &mut face,
            message,
            18.0,
            12.0,
            592.0,
            24.0,
            58.0,
            28.0,
            sw::accent(),
        );
    }
    face
}

// ---------------------------------------------------------------------------
// Raster helpers shared with the network / navigation / clock menues.
// ---------------------------------------------------------------------------

/// A face filled with one RGBA colour.
pub(crate) fn blank(w: u32, h: u32, colour: [u8; 4]) -> UndoFace {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        rgba.extend_from_slice(&colour);
    }
    UndoFace { rgba, w, h }
}

/// Fill a rectangle, clipped to the face.
pub(crate) fn fill(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, colour: [u8; 4]) {
    let x0 = (x.round() as i32).max(0);
    let y0 = (y.round() as i32).max(0);
    let x1 = ((x + w).round() as i32).min(face.w as i32);
    let y1 = ((y + h).round() as i32).min(face.h as i32);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let at = ((yy as u32 * face.w + xx as u32) * 4) as usize;
            face.rgba[at..at + 4].copy_from_slice(&colour);
        }
    }
}

fn tracking(px: f32) -> f32 {
    (px * 0.10).round()
}

/// Width of one line set in the label font.
pub(crate) fn ink_width(text: &str, px: f32) -> f32 {
    match text::label_font() {
        Some(font) => text::line_width(font, text, px, tracking(px)),
        None => 0.0,
    }
}

fn line_height(px: f32) -> f32 {
    text::label_font()
        .and_then(|font| font.horizontal_line_metrics(px))
        .map(|metrics| metrics.new_line_size)
        .unwrap_or(px * 1.2)
}

/// Shrink `text` to fit `max_w`, then trim it with an ellipsis if it still does
/// not fit at the minimum size.
pub(crate) fn fit_line(text: &str, max_w: f32, max_px: f32, min_px: f32) -> (String, f32) {
    let mut px = max_px;
    while px > min_px && ink_width(text, px) > max_w {
        px -= 1.0;
    }
    if ink_width(text, px) <= max_w {
        return (text.to_string(), px);
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut short: String = chars.iter().collect();
        short.push('…');
        if ink_width(&short, px) <= max_w {
            return (short, px);
        }
    }
    ("…".to_string(), px)
}

/// Draw one line left-aligned from `x`, clipped to the face; returns its width.
/// Case is preserved (unlike the menu label helper), which matters for SSIDs.
pub(crate) fn draw_text(
    face: &mut UndoFace,
    text: &str,
    px: f32,
    x: f32,
    top: f32,
    colour: [u8; 3],
) -> f32 {
    let Some(font) = text::label_font() else {
        return 0.0;
    };
    let tr = tracking(px);
    let ascent = font
        .horizontal_line_metrics(px)
        .map(|metrics| metrics.ascent)
        .unwrap_or(px * 0.8);
    let baseline = top + ascent;
    let mut pen = x;
    for ch in text.chars() {
        let (metrics, coverage) = font.rasterize(ch, px);
        let ox = pen.round() as i32 + metrics.xmin;
        let oy = (baseline - (metrics.height as f32 + metrics.ymin as f32)).round() as i32;
        for gy in 0..metrics.height {
            let dy = oy + gy as i32;
            if dy < 0 || dy >= face.h as i32 {
                continue;
            }
            for gx in 0..metrics.width {
                let dx = ox + gx as i32;
                if dx < 0 || dx >= face.w as i32 {
                    continue;
                }
                let alpha = coverage[gy * metrics.width + gx] as u32;
                if alpha == 0 {
                    continue;
                }
                let at = ((dy as u32 * face.w + dx as u32) * 4) as usize;
                let under = face.rgba[at + 3] as u32 * (255 - alpha) / 255;
                let out = alpha + under;
                if out == 0 {
                    continue;
                }
                for (offset, ink) in colour.iter().enumerate() {
                    let channel = at + offset;
                    face.rgba[channel] =
                        ((*ink as u32 * alpha + face.rgba[channel] as u32 * under + out / 2) / out)
                            as u8;
                }
                face.rgba[at + 3] = out as u8;
            }
        }
        pen += metrics.advance_width + tr;
    }
    if text.is_empty() {
        0.0
    } else {
        pen - x - tr
    }
}

/// Draw one fitted line centred in a rectangle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_box(
    face: &mut UndoFace,
    text: &str,
    max_px: f32,
    min_px: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    colour: [u8; 3],
) {
    if text.is_empty() {
        return;
    }
    let (line, px) = fit_line(text, w, max_px, min_px);
    let ink = ink_width(&line, px);
    let top = y + ((h - line_height(px)) / 2.0).max(0.0);
    draw_text(face, &line, px, x + ((w - ink) / 2.0).max(0.0), top, colour);
}

/// Draw one fitted line left-aligned in a rectangle (vertically centred).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_left(
    face: &mut UndoFace,
    text: &str,
    max_px: f32,
    min_px: f32,
    max_w: f32,
    x: f32,
    y: f32,
    h: f32,
    colour: [u8; 3],
) {
    if text.is_empty() {
        return;
    }
    let mut line = text.to_string();
    let mut px = max_px;
    while px > min_px && ink_width(&line, px) > max_w {
        px -= 1.0;
    }
    if ink_width(&line, px) > max_w {
        // Keep the end of the field visible: drop characters from the front.
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        while start < chars.len() {
            start += 1;
            let mut short: String = chars[start..].iter().collect();
            short.insert(0, '…');
            if ink_width(&short, px) <= max_w {
                line = short;
                break;
            }
        }
        if ink_width(&line, px) > max_w {
            line = "…".to_string();
        }
    }
    let top = y + ((h - line_height(px)) / 2.0).max(0.0);
    draw_text(face, &line, px, x, top, colour);
}

/// Wrap one paragraph on character boundaries and vertically centre the block
/// inside a rectangle.  Every line is drawn centred; the last line is trimmed
/// with an ellipsis when `max_lines` would be exceeded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_wrapped(
    face: &mut UndoFace,
    text: &str,
    max_px: f32,
    min_px: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    max_lines: usize,
    colour: [u8; 3],
) {
    let mut px = max_px;
    let mut lines = wrap(text, px, w);
    while px > min_px && (lines.len() > max_lines || lines.iter().any(|l| ink_width(l, px) > w)) {
        px -= 1.0;
        lines = wrap(text, px, w);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let mut marked = last.clone();
            marked.push('…');
            *last = fit_line(&marked, w, px, px).0;
        }
    }
    let lines: Vec<String> = lines.iter().map(|l| fit_line(l, w, px, px).0).collect();
    let line_h = line_height(px);
    let block = line_h * lines.len() as f32;
    let mut top = y + ((h - block) / 2.0).max(0.0);
    for line in &lines {
        let ink = ink_width(line, px);
        draw_text(face, line, px, x + ((w - ink) / 2.0).max(0.0), top, colour);
        top += line_h;
    }
}

fn wrap(text: &str, px: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut line));
            continue;
        }
        let mut candidate = line.clone();
        candidate.push(ch);
        if !line.is_empty() && ink_width(&candidate, px) > max_w {
            lines.push(std::mem::take(&mut line));
        }
        line.push(ch);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selected_key_overlay_is_only_key_sized() {
        let face = selection_face(Cursor::default(), Language::ZhCn);
        assert_eq!((face.w, face.h), (KEY_W as u32, KEY_H as u32));
        assert!(
            face.rgba.chunks(4).any(|p| p[3] > 0),
            "the selected key overlay is blank"
        );
    }
}
