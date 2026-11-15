//! The three-page sync-conflict screen (v0.0.10 project book §7): an opaque
//! Switch-style surface with a page tab bar, the per-unit version cards, the
//! direction choice and the locked/syncing states.
//!
//! The app owns the state machine and hands this module plain data
//! ([`ConflictView`]); [`conflict_face`] rasterises the whole 720×480 page in
//! one go so the frontend can upload and draw a single opaque texture.

use slot_gfx::{OUT_H, OUT_W};
use slot_i18n::Language;

use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

/// Green-cyan accent of the battery (`.sav`) conflict page.
pub const SAV_ACCENT: [u8; 4] = [0x00, 0xd4, 0xaa, 0xff];
/// Red accent of the instant-save (state) conflict page.
pub const STATE_ACCENT: [u8; 4] = [0xe6, 0x00, 0x12, 0xff];

/// One conflict page.  `All` is the unified two-unit page shown only when both
/// units conflict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConflictPage {
    #[default]
    All,
    State,
    Sav,
}

impl ConflictPage {
    /// Title from project book appendix A.
    pub fn title(self, lang: Language) -> &'static str {
        match self {
            ConflictPage::All => slot_i18n::t(lang, "device.quick.conflict.title_all"),
            ConflictPage::State => slot_i18n::t(lang, "device.quick.conflict.title_state"),
            ConflictPage::Sav => slot_i18n::t(lang, "device.quick.conflict.title_sav"),
        }
    }

    /// Short name used by the tab bar.
    pub fn tab(self, lang: Language) -> &'static str {
        match self {
            ConflictPage::All => slot_i18n::t(lang, "device.quick.conflict.tab_all"),
            ConflictPage::State => slot_i18n::t(lang, "device.quick.conflict.tab_state"),
            ConflictPage::Sav => slot_i18n::t(lang, "device.quick.conflict.tab_sav"),
        }
    }

    /// `true` for the two single-unit pages.
    pub fn unit_page(self) -> bool {
        !matches!(self, ConflictPage::All)
    }
}

/// Which side the page is about to overwrite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConflictDirection {
    /// `覆盖本地`: the cloud version wins.
    #[default]
    Download,
    /// `覆盖云端`: the local version wins.
    Upload,
}

impl ConflictDirection {
    /// Label from project book appendix A.
    pub fn label(self, lang: Language) -> &'static str {
        match self {
            ConflictDirection::Download => {
                slot_i18n::t(lang, "device.quick.conflict.overwrite_local")
            }
            ConflictDirection::Upload => {
                slot_i18n::t(lang, "device.quick.conflict.overwrite_cloud")
            }
        }
    }

    /// The arrow points from the cloud towards the device when downloading,
    /// and away from it when uploading.
    pub fn points_up(self) -> bool {
        matches!(self, ConflictDirection::Upload)
    }
}

/// One unit's version lines and conflict reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictUnitView {
    /// `云端 v7 · RG34XX-A · 10-22 21:03 · 1 个文件`.
    pub cloud_line: String,
    /// `本地 v8 · 本机 · 10-22 21:12 · 1 个文件`.
    pub local_line: String,
    /// Why the unit conflicts (`云端有更新`, …).
    pub reason: String,
    pub locked: bool,
}

/// Everything the conflict pages print.  The app builds this from its state
/// machine; equality is the frontend's texture-cache key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictView {
    /// Pages in tab order; a single page hides the tab bar and page number.
    pub pages: Vec<ConflictPage>,
    pub page: ConflictPage,
    pub direction: ConflictDirection,
    pub state: Option<ConflictUnitView>,
    pub sav: Option<ConflictUnitView>,
    pub locked_state: bool,
    pub locked_sav: bool,
    /// Last sync failure, shown in red above the direction row.
    pub error: Option<String>,
    /// A notice such as the changed-since reset; shown in dim type.
    pub notice: Option<String>,
    /// True while the page's A press is being synchronised.
    pub syncing: bool,
}

impl ConflictView {
    fn tabbed(&self) -> bool {
        self.pages.len() > 1
    }

    fn index(&self) -> usize {
        self.pages
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0)
    }

    fn page_locked(&self) -> bool {
        match self.page {
            ConflictPage::All => self.locked_state || self.locked_sav,
            ConflictPage::State => self.locked_state,
            ConflictPage::Sav => self.locked_sav,
        }
    }

    fn accent(&self) -> [u8; 4] {
        match self.page {
            ConflictPage::All => sw::surface_border(),
            ConflictPage::State => STATE_ACCENT,
            ConflictPage::Sav => SAV_ACCENT,
        }
    }

    fn unit(&self, page: ConflictPage) -> Option<&ConflictUnitView> {
        match page {
            ConflictPage::State => self.state.as_ref(),
            ConflictPage::Sav => self.sav.as_ref(),
            ConflictPage::All => None,
        }
    }

    /// The unit printed on this page (single-unit pages only).
    fn page_unit(&self) -> Option<&ConflictUnitView> {
        self.unit(self.page)
    }
}

// ---------------------------------------------------------------------------
// Layout constants (tests assert against these)
// ---------------------------------------------------------------------------

pub const TAB_Y: f32 = 18.0;
pub const TAB_H: f32 = 34.0;
pub const TAB_UNDERLINE_Y: f32 = 52.0;
pub const TITLE_Y: f32 = 64.0;
pub const CONTENT_Y: f32 = 118.0;
pub const CARD_X: f32 = 36.0;
pub const CARD_W: f32 = OUT_W as f32 - 2.0 * CARD_X;
pub const DIRECTION_Y: f32 = 402.0;
pub const DIRECTION_H: f32 = 36.0;
pub const DIRECTION_CHIP_W: f32 = 132.0;
const DIRECTION_GAP: f32 = 16.0;
/// Mixed-direction hint, always visible on the "全部" page (§7.7).
pub fn mixed_hint(lang: Language) -> &'static str {
    slot_i18n::t(lang, "device.quick.conflict.mixed_hint")
}
pub fn locked_text(lang: Language) -> &'static str {
    slot_i18n::t(lang, "device.quick.conflict.locked_text")
}
pub fn locked_label(lang: Language) -> &'static str {
    slot_i18n::t(lang, "device.quick.conflict.locked_label")
}
const SIDE: f32 = CARD_X;

/// Rasterise the whole conflict screen.  The face is opaque: the shelf behind
/// must never show through a conflict page.
pub fn conflict_face(view: &ConflictView, lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    if view.tabbed() {
        draw_tabs(&mut face, view, lang);
    }
    draw_title(&mut face, view, lang);
    if matches!(view.page, ConflictPage::All) {
        draw_all_page(&mut face, view, lang);
    } else if matches!(view.page, ConflictPage::State) {
        draw_state_page(&mut face, view, lang);
    } else {
        draw_sav_page(&mut face, view, lang);
    }
    let locked = view.page_locked();
    if locked {
        let bottom = if matches!(view.page, ConflictPage::All) {
            CONTENT_Y + 250.0
        } else {
            CONTENT_Y + 196.0
        };
        veil(&mut face, SIDE, CONTENT_Y, CARD_W, bottom - CONTENT_Y);
        let panel_w = 246.0;
        let panel_h = 50.0;
        let panel_x = (OUT_W as f32 - panel_w) / 2.0;
        let panel_y = CONTENT_Y + (bottom - CONTENT_Y - panel_h) / 2.0;
        sw::fill_round(
            &mut face,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            11.0,
            [0x18, 0x18, 0x1d, 0xff],
        );
        check_mark(&mut face, panel_x + 32.0, panel_y + 15.0, 20.0, sw::ink());
        kb::draw_left(
            &mut face,
            locked_label(lang),
            20.0,
            13.0,
            160.0,
            panel_x + 64.0,
            panel_y,
            panel_h,
            sw::ink(),
        );
    }
    draw_message(&mut face, view, lang);
    draw_direction_row(&mut face, view, lang);
    draw_hint(&mut face, view, lang);
    face
}

// ---------------------------------------------------------------------------
// Chrome
// ---------------------------------------------------------------------------

fn draw_tabs(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    let mut x = SIDE;
    for page in &view.pages {
        let locked = match page {
            ConflictPage::All => view.locked_state || view.locked_sav,
            ConflictPage::State => view.locked_state,
            ConflictPage::Sav => view.locked_sav,
        };
        let text = page.tab(lang);
        let check = if locked { 20.0 } else { 0.0 };
        let w = kb::ink_width(text, 18.0) + 26.0 + check;
        let ink = if *page == view.page && !locked {
            sw::ink()
        } else {
            sw::ink_dim()
        };
        if locked {
            check_mark(face, x + 8.0, TAB_Y + 10.0, 16.0, sw::ink_dim());
        }
        kb::draw_box(face, text, 18.0, 12.0, x + check, TAB_Y, w - check, TAB_H, ink);
        if *page == view.page {
            let colour = if locked { sw::surface_border() } else { STATE_ACCENT };
            sw::fill_round(face, x + 8.0, TAB_UNDERLINE_Y, w - 16.0, 3.0, 1.5, colour);
        }
        x += w + 4.0;
    }
    let total = view.pages.len();
    let position = format!("{} / {total}", view.index() + 1);
    let px = 16.0;
    let ink = kb::ink_width(&position, px);
    kb::draw_left(
        face,
        &position,
        px,
        11.0,
        ink.max(1.0),
        OUT_W as f32 - SIDE - ink,
        TAB_Y,
        TAB_H,
        sw::ink_dim(),
    );
}

fn draw_title(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    let accent = view.accent();
    sw::fill_round(face, SIDE, TITLE_Y + 4.0, 5.0, 24.0, 2.5, accent);
    kb::draw_left(
        face,
        view.page.title(lang),
        26.0,
        16.0,
        OUT_W as f32 - 2.0 * SIDE - 180.0,
        SIDE + 16.0,
        TITLE_Y,
        32.0,
        sw::ink(),
    );
    let subtitle = match view.page {
        ConflictPage::All => slot_i18n::t(lang, "device.quick.conflict.same_both").to_string(),
        _ => view
            .page_unit()
            .map(|unit| unit.reason.clone())
            .unwrap_or_default(),
    };
    if !subtitle.is_empty() {
        kb::draw_left(
            face,
            &subtitle,
            16.0,
            11.0,
            OUT_W as f32 - 2.0 * SIDE,
            SIDE + 16.0,
            TITLE_Y + 32.0,
            20.0,
            sw::ink_dim(),
        );
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn card(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, accent: [u8; 4], frame: bool) {
    let border = if frame { accent } else { sw::surface_border() };
    sw::fill_round(face, x, y, w, h, 12.0, border);
    sw::fill_round(face, x + 1.0, y + 1.0, w - 2.0, h - 2.0, 11.0, sw::surface());
    sw::fill_round(face, x + 2.0, y + 14.0, 4.0, h - 28.0, 2.0, accent);
}

fn draw_all_page(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    for (index, page) in [ConflictPage::State, ConflictPage::Sav].into_iter().enumerate() {
        let y = CONTENT_Y + index as f32 * 132.0;
        let accent = if matches!(page, ConflictPage::State) {
            STATE_ACCENT
        } else {
            SAV_ACCENT
        };
        card(face, SIDE, y, CARD_W, 118.0, accent, false);
        let Some(unit) = view.unit(page) else {
            kb::draw_box(
                face,
                slot_i18n::t(lang, "device.quick.conflict.off"),
                18.0,
                12.0,
                SIDE + 20.0,
                y,
                200.0,
                118.0,
                sw::ink_dim(),
            );
            continue;
        };
        kb::draw_left(
            face,
            page.tab(lang),
            20.0,
            14.0,
            220.0,
            SIDE + 22.0,
            y + 10.0,
            26.0,
            sw::ink(),
        );
        let text_x = SIDE + 22.0;
        let text_w = CARD_W - 250.0;
        kb::draw_left(
            face,
            &unit.cloud_line,
            15.0,
            10.0,
            text_w,
            text_x,
            y + 40.0,
            22.0,
            sw::ink(),
        );
        kb::draw_left(
            face,
            &unit.local_line,
            15.0,
            10.0,
            text_w,
            text_x,
            y + 64.0,
            22.0,
            sw::ink_dim(),
        );
        direction_art(face, SIDE + CARD_W - 150.0, y + 16.0, view.direction, accent);
        let label_x = SIDE + CARD_W - 190.0;
        kb::draw_box(
            face,
            view.direction.label(lang),
            14.0,
            10.0,
            label_x,
            y + 86.0,
            150.0,
            20.0,
            sw::ink_dim(),
        );
    }
}

fn draw_state_page(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    card(face, SIDE, CONTENT_Y, CARD_W, 196.0, STATE_ACCENT, true);
    let y = CONTENT_Y;
    kb::draw_left(
        face,
        slot_i18n::t(lang, "device.quick.conflict.tab_state"),
        22.0,
        14.0,
        260.0,
        SIDE + 22.0,
        y + 14.0,
        30.0,
        sw::ink(),
    );
    if let Some(unit) = &view.state {
        let text_w = CARD_W - 250.0;
        kb::draw_left(
            face,
            &unit.cloud_line,
            17.0,
            11.0,
            text_w,
            SIDE + 22.0,
            y + 58.0,
            26.0,
            sw::ink(),
        );
        kb::draw_left(
            face,
            &unit.local_line,
            17.0,
            11.0,
            text_w,
            SIDE + 22.0,
            y + 92.0,
            26.0,
            sw::ink_dim(),
        );
    } else {
        kb::draw_box(
            face,
            slot_i18n::t(lang, "device.quick.conflict.state_off"),
            17.0,
            11.0,
            SIDE,
            y,
            CARD_W,
            196.0,
            sw::ink_dim(),
        );
    }
    kb::draw_left(
        face,
        slot_i18n::t(lang, "device.quick.conflict.sync_unit"),
        16.0,
        11.0,
        240.0,
        SIDE + 22.0,
        y + 148.0,
        26.0,
        sw::ink_dim(),
    );
    direction_art(
        face,
        SIDE + CARD_W - 150.0,
        y + 30.0,
        view.direction,
        STATE_ACCENT,
    );
    kb::draw_box(
        face,
        view.direction.label(lang),
        15.0,
        10.0,
        SIDE + CARD_W - 200.0,
        y + 154.0,
        160.0,
        24.0,
        sw::ink_dim(),
    );
}

fn draw_sav_page(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    card(face, SIDE, CONTENT_Y, CARD_W, 196.0, SAV_ACCENT, true);
    let y = CONTENT_Y;
    draw_sav_icon(face, SIDE + 24.0, y + 36.0);
    let text_x = SIDE + 172.0;
    let text_w = CARD_W - 350.0;
    kb::draw_left(
        face,
        slot_i18n::t(lang, "device.quick.conflict.tab_sav"),
        22.0,
        14.0,
        260.0,
        text_x,
        y + 14.0,
        30.0,
        sw::ink(),
    );
    if let Some(unit) = &view.sav {
        kb::draw_left(
            face,
            &unit.cloud_line,
            16.0,
            11.0,
            text_w,
            text_x,
            y + 58.0,
            24.0,
            sw::ink(),
        );
        kb::draw_left(
            face,
            &unit.local_line,
            16.0,
            11.0,
            text_w,
            text_x,
            y + 88.0,
            24.0,
            sw::ink_dim(),
        );
    } else {
        kb::draw_left(
            face,
            slot_i18n::t(lang, "device.quick.conflict.sav_off"),
            16.0,
            11.0,
            text_w,
            text_x,
            y + 74.0,
            26.0,
            sw::ink_dim(),
        );
    }
    kb::draw_left(
        face,
        slot_i18n::t(lang, "device.quick.conflict.sync_unit"),
        16.0,
        11.0,
        240.0,
        text_x,
        y + 148.0,
        26.0,
        sw::ink_dim(),
    );
    direction_art(
        face,
        SIDE + CARD_W - 150.0,
        y + 30.0,
        view.direction,
        SAV_ACCENT,
    );
    kb::draw_box(
        face,
        view.direction.label(lang),
        15.0,
        10.0,
        SIDE + CARD_W - 200.0,
        y + 154.0,
        160.0,
        24.0,
        sw::ink_dim(),
    );
}

/// A rounded `.sav` cartridge tile: a battery icon, `.sav` type and the file
/// count line (the CLI reports files, not bytes).
fn draw_sav_icon(face: &mut UndoFace, x: f32, y: f32) {
    sw::fill_round(face, x, y, 118.0, 124.0, 12.0, [0x0b, 0x3b, 0x34, 0xff]);
    sw::fill_round(face, x + 1.0, y + 1.0, 116.0, 122.0, 11.0, [0x08, 0x2a, 0x26, 0xff]);
    // Battery shell.
    sw::fill_round(face, x + 22.0, y + 18.0, 54.0, 28.0, 5.0, SAV_ACCENT);
    sw::fill_round(face, x + 25.0, y + 21.0, 48.0, 22.0, 4.0, [0x08, 0x2a, 0x26, 0xff]);
    sw::fill_round(face, x + 78.0, y + 26.0, 7.0, 12.0, 2.0, SAV_ACCENT);
    // Charge bars.
    for i in 0..3 {
        sw::fill_round(
            face,
            x + 30.0 + i as f32 * 14.0,
            y + 25.0,
            10.0,
            14.0,
            2.0,
            SAV_ACCENT,
        );
    }
    kb::draw_box(
        face,
        ".sav",
        20.0,
        13.0,
        x,
        y + 66.0,
        118.0,
        26.0,
        sw::ink(),
    );
    kb::draw_box(
        face,
        "SRAM",
        12.0,
        9.0,
        x,
        y + 92.0,
        118.0,
        20.0,
        sw::ink_dim(),
    );
}

// ---------------------------------------------------------------------------
// Message area, direction row and hint
// ---------------------------------------------------------------------------

fn draw_message(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    // §7.7: the mixed-direction warning is permanent on every conflict page.
    let hint_top = DIRECTION_Y - 20.0;
    kb::draw_left(
        face,
        mixed_hint(lang),
        13.0,
        10.0,
        CARD_W,
        SIDE,
        hint_top,
        20.0,
        sw::ink_dim(),
    );
    if view.syncing {
        kb::draw_box(
            face,
            slot_i18n::t(lang, "device.quick.conflict.syncing"),
            16.0,
            12.0,
            SIDE,
            DIRECTION_Y - 52.0,
            CARD_W,
            24.0,
            sw::ink(),
        );
        return;
    }
    let (text, colour) = if let Some(error) = &view.error {
        (error.as_str(), [0xff, 0x6b, 0x6b])
    } else if let Some(notice) = &view.notice {
        (notice.as_str(), sw::ink())
    } else {
        return;
    };
    if matches!(view.page, ConflictPage::All) {
        // The two-unit cards end at 368; a single truncated line sits between
        // them and the permanent hint.
        kb::draw_left(
            face,
            text,
            12.0,
            10.0,
            CARD_W,
            SIDE,
            CONTENT_Y + 250.0 + 2.0,
            18.0,
            colour,
        );
    } else {
        let top = CONTENT_Y + 202.0;
        let height = (hint_top - 2.0 - top).max(14.0);
        kb::draw_wrapped(
            face,
            text,
            13.0,
            10.0,
            SIDE,
            top,
            CARD_W,
            height,
            2,
            colour,
        );
    }
}

fn draw_direction_row(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    let locked = view.page_locked();
    let mut x = SIDE;
    for direction in [ConflictDirection::Download, ConflictDirection::Upload] {
        let selected = view.direction == direction;
        if selected && !locked {
            sw::fill_round(
                face,
                x,
                DIRECTION_Y,
                DIRECTION_CHIP_W,
                DIRECTION_H,
                9.0,
                sw::selected(),
            );
            kb::draw_box(
                face,
                direction.label(lang),
                18.0,
                12.0,
                x,
                DIRECTION_Y,
                DIRECTION_CHIP_W,
                DIRECTION_H,
                sw::ink_selected(),
            );
        } else {
            let fill = if selected {
                [0x3a, 0x3a, 0x42, 0xff]
            } else {
                sw::surface()
            };
            sw::fill_round(
                face,
                x,
                DIRECTION_Y,
                DIRECTION_CHIP_W,
                DIRECTION_H,
                9.0,
                fill,
            );
            let ink = if selected && !locked {
                sw::ink()
            } else {
                sw::ink_dim()
            };
            kb::draw_box(
                face,
                direction.label(lang),
                18.0,
                12.0,
                x,
                DIRECTION_Y,
                DIRECTION_CHIP_W,
                DIRECTION_H,
                ink,
            );
        }
        x += DIRECTION_CHIP_W + DIRECTION_GAP;
    }
}

fn draw_hint(face: &mut UndoFace, view: &ConflictView, lang: Language) {
    let hint = if view.tabbed() {
        slot_i18n::t(lang, "device.quick.conflict.hint_paged")
    } else {
        slot_i18n::t(lang, "device.quick.conflict.hint")
    };
    let px = 14.0;
    let ink = kb::ink_width(hint, px);
    kb::draw_left(
        face,
        hint,
        px,
        10.0,
        ink.max(1.0),
        OUT_W as f32 - SIDE - ink,
        OUT_H as f32 - 42.0,
        26.0,
        sw::ink_dim(),
    );
}

// ---------------------------------------------------------------------------
// Small procedural art: cloud, transfer arrow, battery tile
// ---------------------------------------------------------------------------

fn direction_art(
    face: &mut UndoFace,
    x: f32,
    y: f32,
    direction: ConflictDirection,
    accent: [u8; 4],
) {
    let cx = x + 60.0;
    cloud(face, cx, y + 26.0, 46.0, accent);
    arrow(
        face,
        cx,
        y + 62.0,
        30.0,
        direction.points_up(),
        [sw::ink()[0], sw::ink()[1], sw::ink()[2], 0xff],
    );
}

fn cloud(face: &mut UndoFace, cx: f32, cy: f32, size: f32, colour: [u8; 4]) {
    let r = size * 0.5;
    sw::fill_round(face, cx - r, cy, size, r * 0.85, r * 0.42, colour);
    sw::fill_round(
        face,
        cx - r * 0.78,
        cy - r * 0.55,
        r * 1.05,
        r * 1.0,
        r * 0.5,
        colour,
    );
    sw::fill_round(
        face,
        cx - r * 0.05,
        cy - r * 0.9,
        r * 1.1,
        r * 1.05,
        r * 0.52,
        colour,
    );
    sw::fill_round(
        face,
        cx + r * 0.35,
        cy - r * 0.45,
        r * 0.8,
        r * 0.9,
        r * 0.42,
        colour,
    );
}

/// A straight transfer arrow with a triangular head.  `points_up` flips it.
fn arrow(face: &mut UndoFace, cx: f32, cy: f32, size: f32, points_up: bool, colour: [u8; 4]) {
    let half = size * 0.5;
    let head = size * 0.34;
    let shaft_w = size * 0.18;
    // The transfer travels from the cloud to the cart (download) or back up.
    let dir = if points_up { -1.0 } else { 1.0 };
    let tip_y = cy + dir * half;
    let base_y = tip_y - dir * head;
    let tip = (cx, tip_y);
    let left = (cx - half * 0.72, base_y);
    let right = (cx + half * 0.72, base_y);
    triangle(face, [tip, left, right], colour);
    // The shaft, starting at the cloud side.
    let start_y = cy - dir * half;
    let top = start_y.min(base_y);
    let bottom = start_y.max(base_y);
    sw::fill_round(
        face,
        cx - shaft_w / 2.0,
        top,
        shaft_w,
        bottom - top,
        shaft_w / 2.0,
        colour,
    );
}

fn triangle(face: &mut UndoFace, points: [(f32, f32); 3], colour: [u8; 4]) {
    let min_x = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let max_x = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let min_y = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let max_y = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    let x0 = min_x.floor().max(0.0) as u32;
    let y0 = min_y.floor().max(0.0) as u32;
    let x1 = max_x.ceil().min(face.w as f32) as u32;
    let y1 = max_y.ceil().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let mut hits = 0;
            for sy in 0..2 {
                for sx in 0..2 {
                    let u = px as f32 + (sx as f32 + 0.5) / 2.0;
                    let v = py as f32 + (sy as f32 + 0.5) / 2.0;
                    if inside(points, u, v) {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                blend(face, px, py, colour, hits as f32 / 4.0);
            }
        }
    }
}

fn inside(points: [(f32, f32); 3], px: f32, py: f32) -> bool {
    let sign = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let d1 = sign((px, py), points[0], points[1]);
    let d2 = sign((px, py), points[1], points[2]);
    let d3 = sign((px, py), points[2], points[0]);
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Darken a content rectangle (the locked-page veil, §7.5).
fn veil(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32) {
    let x0 = x.floor().max(0.0) as u32;
    let y0 = y.floor().max(0.0) as u32;
    let x1 = (x + w).ceil().min(face.w as f32) as u32;
    let y1 = (y + h).ceil().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            blend(face, px, py, [0, 0, 0, 255], 0.55);
        }
    }
}

/// A tick mark drawn from two thick segments (the label font has no ✓ glyph).
fn check_mark(face: &mut UndoFace, x: f32, y: f32, size: f32, colour: [u8; 3]) {
    let colour = [colour[0], colour[1], colour[2], 0xff];
    let a = (x, y + size * 0.52);
    let b = (x + size * 0.38, y + size * 0.86);
    let c = (x + size, y + size * 0.16);
    thick_line(face, a, b, size * 0.18, colour);
    thick_line(face, b, c, size * 0.18, colour);
}

fn thick_line(
    face: &mut UndoFace,
    a: (f32, f32),
    b: (f32, f32),
    thickness: f32,
    colour: [u8; 4],
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = (dx * dx + dy * dy).max(0.0001);
    let min_x = a.0.min(b.0) - thickness;
    let min_y = a.1.min(b.1) - thickness;
    let max_x = a.0.max(b.0) + thickness;
    let max_y = a.1.max(b.1) + thickness;
    let x0 = min_x.floor().max(0.0) as u32;
    let y0 = min_y.floor().max(0.0) as u32;
    let x1 = max_x.ceil().min(face.w as f32) as u32;
    let y1 = max_y.ceil().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sx = px as f32 + 0.5;
            let sy = py as f32 + 0.5;
            let t = (((sx - a.0) * dx + (sy - a.1) * dy) / len_sq).clamp(0.0, 1.0);
            let cx = a.0 + t * dx;
            let cy = a.1 + t * dy;
            let dist = ((sx - cx) * (sx - cx) + (sy - cy) * (sy - cy)).sqrt();
            let coverage = (thickness * 0.5 - dist + 0.5).clamp(0.0, 1.0);
            blend(face, px, py, colour, coverage);
        }
    }
}

fn blend(face: &mut UndoFace, x: u32, y: u32, colour: [u8; 4], coverage: f32) {
    if x >= face.w || y >= face.h || coverage <= 0.0 {
        return;
    }
    let at = ((y * face.w + x) * 4) as usize;
    let src = (colour[3] as f32 * coverage.clamp(0.0, 1.0) / 255.0).clamp(0.0, 1.0);
    if src <= 0.0 {
        return;
    }
    let dst = face.rgba[at + 3] as f32 / 255.0;
    let out = src + dst * (1.0 - src);
    if out <= 0.0 {
        return;
    }
    for (offset, channel) in colour.iter().enumerate().take(3) {
        let a = *channel as f32 / 255.0;
        let b = face.rgba[at + offset] as f32 / 255.0;
        face.rgba[at + offset] =
            (((a * src + b * dst * (1.0 - src)) / out) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    face.rgba[at + 3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(page: ConflictPage) -> ConflictView {
        let unit = |locked: bool| ConflictUnitView {
            cloud_line: "云端 v7 · RG34XX-A · 10-22 21:03 · 1 个文件".to_string(),
            local_line: "本地 v8 · 本机 · 10-22 21:12 · 1 个文件".to_string(),
            reason: "云端有更新".to_string(),
            locked,
        };
        let n = if matches!(page, ConflictPage::All) { 3 } else { 1 };
        ConflictView {
            pages: match n {
                3 => vec![
                    ConflictPage::All,
                    ConflictPage::State,
                    ConflictPage::Sav,
                ],
                _ => vec![page],
            },
            page,
            direction: ConflictDirection::Download,
            state: Some(unit(false)),
            sav: Some(unit(false)),
            locked_state: false,
            locked_sav: false,
            error: None,
            notice: None,
            syncing: false,
        }
    }

    #[test]
    fn the_page_copy_comes_from_the_project_book() {
        assert_eq!(ConflictPage::All.title(Language::ZhCn), "同步冲突 · 全部");
        assert_eq!(ConflictPage::State.title(Language::ZhCn), "同步冲突 · 即时存档");
        assert_eq!(ConflictPage::Sav.title(Language::ZhCn), "同步冲突 · 电池存档");
        assert_eq!(ConflictPage::State.tab(Language::ZhCn), "即时存档");
        assert_eq!(ConflictDirection::Download.label(Language::ZhCn), "覆盖本地");
        assert_eq!(ConflictDirection::Upload.label(Language::ZhCn), "覆盖云端");
        assert_eq!(locked_text(Language::ZhCn), "✓ 已同步");
        assert!(mixed_hint(Language::ZhCn).contains("即时存档含电池内容"));
    }

    #[test]
    fn a_single_page_hides_the_tab_bar_and_page_number() {
        let single = view(ConflictPage::Sav);
        assert!(!single.tabbed());
        let triple = view(ConflictPage::All);
        assert!(triple.tabbed());
        assert_eq!(triple.index(), 0);
        let mut state_page = triple.clone();
        state_page.page = ConflictPage::State;
        assert_eq!(state_page.index(), 1);
    }

    #[test]
    fn the_all_page_is_locked_when_either_unit_is_locked() {
        let mut all = view(ConflictPage::All);
        assert!(!all.page_locked());
        all.locked_state = true;
        assert!(all.page_locked());
        all.locked_state = false;
        all.locked_sav = true;
        assert!(all.page_locked());
        let mut state = view(ConflictPage::State);
        state.locked_sav = true;
        assert!(
            !state.page_locked(),
            "the state page ignores the sav lock"
        );
        state.locked_state = true;
        assert!(state.page_locked());
    }

    #[test]
    fn the_direction_points_down_to_overwrite_local() {
        assert!(!ConflictDirection::Download.points_up());
        assert!(ConflictDirection::Upload.points_up());
    }
}
