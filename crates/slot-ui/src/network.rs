//! The Wi-Fi network menu and the Web-service dialog, both dressed in the
//! shared Nintendo-Switch-inspired style: charcoal ground, a heading band with
//! the red accent, rounded surface cards and a white selected card.
//!
//! [`wifi_panel`] is opaque because it is opened on top of the quick menu;
//! [`web_dialog`] keeps a semi-transparent mask so the shelf stays visible
//! behind the centred dialog.  Only the drawing changed: the switch, auto-off,
//! scan, selection and connection logic all live in the app.

use slot_i18n::Language;

use crate::draw::{OUT_H, OUT_W};
use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

/// One network discovered by a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiEntry {
    pub ssid: String,
    /// Signal strength: a raw bar count of 0..=4, a dBm reading (negative,
    /// typical -30..=-90) or a percentage above 4.  See [`signal_bars`].
    pub signal: i32,
    pub secured: bool,
    /// True for a previously saved network shown above the scan results.
    pub saved: bool,
}

/// Everything the Wi-Fi panel prints.  The app owns the scan results and the
/// selection; the face is only a snapshot.
#[derive(Clone, Debug, Default)]
pub struct WifiPanelView<'a> {
    /// SSID and address of the associated network, if any.
    pub connected: Option<&'a str>,
    /// Activity line such as "正在扫描…" or "选择网络".
    pub status: &'a str,
    /// Last failure, printed in red.
    pub error: Option<&'a str>,
    /// Discovered networks, in scan order.
    pub rows: &'a [WifiEntry],
    /// Index of the first visible network row.
    pub offset: usize,
    /// Absolute selection index: `0` is the Wi-Fi switch, `1` the "auto off"
    /// setting row, `2..=rows.len() + 1` the network rows and `rows.len() + 2`
    /// the manual row.
    pub selected: usize,
    /// True when the trailing slot_i18n::t(lang, "device.quick.wifi.manual") row is what is selected.
    pub manual_row: bool,
    /// Network currently being connected to.
    pub connecting: Option<&'a str>,
    /// Wi-Fi radio currently on: the position of the switch.
    pub radio_on: bool,
    /// Whether the radio state has been read yet.  While false the panel shows
    /// slot_i18n::t(lang, "device.quick.wifi.reading") instead of the settings and the list.
    pub radio_known: bool,
    /// Value printed on the right of the slot_i18n::t(lang, "device.quick.wifi.auto_off") row, e.g. "10 分钟".
    pub auto_off_label: &'a str,
}

const HEADER_H: f32 = 88.0;
const SIDE: f32 = 48.0;
const CARD_W: f32 = OUT_W as f32 - 2.0 * SIDE;
const SWITCH_TOP: f32 = 96.0;
const SWITCH_H: f32 = 40.0;
const AUTO_OFF_TOP: f32 = 142.0;
const AUTO_OFF_H: f32 = 40.0;
const STATUS_TOP: f32 = 188.0;
const STATUS_H: f32 = 44.0;
const LIST_TOP: f32 = 236.0;
const ROW_H: f32 = 28.0;
const ROW_GAP: f32 = 4.0;
const LIST_ROWS: usize = 5;
const MANUAL_TOP: f32 = LIST_TOP + LIST_ROWS as f32 * (ROW_H + ROW_GAP);
const HINT_TOP: f32 = 434.0;
const HINT_H: f32 = 28.0;
const READING_TOP: f32 = 120.0;
const READING_H: f32 = 64.0;

const RAIL_W: f32 = 76.0;
const RAIL_H: f32 = 26.0;
const KNOB: f32 = 22.0;
const RAIL_OFF: [u8; 4] = [0x5a, 0x5a, 0x64, 0xff];

/// Bar count drawn for a signal reading.
///
/// `0..=4` is taken as an already counted number of bars, a negative value as a
/// dBm reading and anything above four as a percentage.  That keeps the panel
/// useful whichever unit the scan backend reports.
pub fn signal_bars(signal: i32) -> usize {
    if (0..=4).contains(&signal) {
        signal as usize
    } else if signal < 0 {
        (((signal + 100).clamp(0, 60)) / 15) as usize
    } else {
        ((signal.clamp(0, 100) as usize) * 4 / 100).min(4)
    }
}

/// The whole Wi-Fi menu: the radio switch, the "auto off" setting, current
/// state, the scan list and the manual-row entry.  The list is hidden while the
/// radio is off or its state is still unknown; the two setting cards stay
/// reachable while the radio is off.
pub fn wifi_panel(view: &WifiPanelView, lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, sw::bg());
    heading(&mut face, slot_i18n::t(lang, "device.quick.wifi.title"));

    if !view.radio_known {
        card_at(&mut face, SIDE, READING_TOP, CARD_W, READING_H, false);
        kb::draw_box(
            &mut face,
            slot_i18n::t(lang, "device.quick.wifi.reading"),
            22.0,
            14.0,
            SIDE,
            READING_TOP,
            CARD_W,
            READING_H,
            sw::ink_dim(),
        );
        hint(&mut face, lang);
        return face;
    }

    switch_row(&mut face, view.radio_on, view.selected == 0);
    auto_off_row(&mut face, view.auto_off_label, view.selected == 1, lang);
    status_card(&mut face, view, lang);

    if !view.radio_on {
        centred_message(&mut face, slot_i18n::t(lang, "device.quick.wifi.off"));
        hint(&mut face, lang);
        return face;
    }
    if view.rows.is_empty() {
        centred_message(&mut face, slot_i18n::t(lang, "device.quick.wifi.none"));
    }
    let start = if view.rows.len() > LIST_ROWS {
        view.offset.min(view.rows.len() - LIST_ROWS)
    } else {
        0
    };
    for (i, entry) in view.rows.iter().enumerate().skip(start).take(LIST_ROWS) {
        let row = i - start;
        let y = LIST_TOP + row as f32 * (ROW_H + ROW_GAP);
        let selected = !view.manual_row && view.selected == i + 2;
        wifi_row(&mut face, y, entry, selected, view, lang);
    }
    card_at(&mut face, SIDE, MANUAL_TOP, CARD_W, ROW_H, view.manual_row);
    let manual_ink = if view.manual_row {
        sw::ink_selected()
    } else {
        sw::ink_dim()
    };
    kb::draw_left(
        &mut face,
        slot_i18n::t(lang, "device.quick.wifi.manual"),
        18.0,
        12.0,
        CARD_W - 48.0,
        SIDE + 24.0,
        MANUAL_TOP,
        ROW_H,
        manual_ink,
    );
    hint(&mut face, lang);
    face
}

/// The Switch-style heading band with the red accent bar and the page title.
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

fn hint(face: &mut UndoFace, lang: Language) {
    kb::draw_box(
        face,
        slot_i18n::t(lang, "device.quick.wifi.hint"),
        16.0,
        11.0,
        SIDE,
        HINT_TOP,
        CARD_W,
        HINT_H,
        sw::ink_dim(),
    );
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

fn accent_ink() -> [u8; 3] {
    sw::accent()
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

/// The Wi-Fi switch card: label on the left, a track and a white round knob on
/// the right.  The accent track means "on"; the grey track means "off".
fn switch_row(face: &mut UndoFace, on: bool, selected: bool) {
    card_at(face, SIDE, SWITCH_TOP, CARD_W, SWITCH_H, selected);
    let ink = if selected { sw::ink_selected() } else { sw::ink() };
    kb::draw_left(
        face,
        "WiFi",
        24.0,
        14.0,
        300.0,
        SIDE + 24.0,
        SWITCH_TOP,
        SWITCH_H,
        ink,
    );
    let rail_x = OUT_W as f32 - SIDE - 24.0 - RAIL_W;
    let rail_y = SWITCH_TOP + (SWITCH_H - RAIL_H) / 2.0;
    let track = if on { accent() } else { RAIL_OFF };
    sw::fill_round(face, rail_x, rail_y, RAIL_W, RAIL_H, RAIL_H / 2.0, track);
    let knob_y = rail_y + (RAIL_H - KNOB) / 2.0;
    let knob_x = if on {
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

/// The slot_i18n::t(lang, "device.quick.wifi.auto_off") setting card: label on the left, the current choice on
/// the right.  It stays reachable while the radio is off.
fn auto_off_row(face: &mut UndoFace, value: &str, selected: bool, lang: Language) {
    card_at(face, SIDE, AUTO_OFF_TOP, CARD_W, AUTO_OFF_H, selected);
    let ink = if selected { sw::ink_selected() } else { sw::ink() };
    kb::draw_left(
        face,
        slot_i18n::t(lang, "device.quick.wifi.auto_off"),
        22.0,
        14.0,
        320.0,
        SIDE + 24.0,
        AUTO_OFF_TOP,
        AUTO_OFF_H,
        ink,
    );
    right_text(
        face,
        value,
        20.0,
        12.0,
        280.0,
        OUT_W as f32 - SIDE - 24.0,
        AUTO_OFF_TOP,
        AUTO_OFF_H,
        ink,
    );
}

/// Draw one fitted line right-aligned to `right`, clipped to the face.
#[allow(clippy::too_many_arguments)]
fn right_text(
    face: &mut UndoFace,
    text: &str,
    max_px: f32,
    min_px: f32,
    max_w: f32,
    right: f32,
    y: f32,
    h: f32,
    colour: [u8; 3],
) {
    let (line, px) = kb::fit_line(text, max_w, max_px, min_px);
    let ink = kb::ink_width(&line, px);
    let x = (right - ink).max(0.0);
    kb::draw_left(face, &line, px, px, max_w, x, y, h, colour);
}

/// The compressed connection state card: the current network on the first line
/// and the most pressing status (error, connecting or activity) below it.
fn status_card(face: &mut UndoFace, view: &WifiPanelView, lang: Language) {
    card_at(face, SIDE, STATUS_TOP, CARD_W, STATUS_H, false);
    let line1 = match view.connected {
        Some(name) => slot_i18n::t_args(lang, "device.quick.wifi.connected", &[("name", name)]),
        None => slot_i18n::t(lang, "device.quick.wifi.not_connected").to_string(),
    };
    let ink1 = if view.connected.is_some() {
        sw::ink()
    } else {
        sw::ink_dim()
    };
    kb::draw_left(
        face,
        &line1,
        15.0,
        10.0,
        CARD_W - 48.0,
        SIDE + 24.0,
        STATUS_TOP + 1.0,
        20.0,
        ink1,
    );
    let line2 = if let Some(message) = view.error {
        Some((slot_i18n::t_args(lang, "device.quick.wifi.error", &[("message", message)]), accent_ink()))
    } else if let Some(name) = view.connecting {
        Some((slot_i18n::t_args(lang, "device.quick.wifi.connecting_to", &[("name", name)]), sw::ink()))
    } else if view.status.is_empty() {
        None
    } else {
        Some((slot_i18n::t_args(lang, "device.quick.wifi.status", &[("status", &view.status)]), sw::ink_dim()))
    };
    if let Some((text, ink)) = line2 {
        kb::draw_left(
            face,
            &text,
            13.0,
            10.0,
            CARD_W - 48.0,
            SIDE + 24.0,
            STATUS_TOP + 21.0,
            22.0,
            ink,
        );
    }
}

/// One row message centred over the list area.
fn centred_message(face: &mut UndoFace, text: &str) {
    kb::draw_box(
        face,
        text,
        22.0,
        14.0,
        SIDE,
        LIST_TOP,
        CARD_W,
        140.0,
        sw::ink_dim(),
    );
}

fn wifi_row(
    face: &mut UndoFace,
    y: f32,
    entry: &WifiEntry,
    selected: bool,
    view: &WifiPanelView,
    lang: Language,
) {
    card_at(face, SIDE, y, CARD_W, ROW_H, selected);
    let ink = if selected { sw::ink_selected() } else { sw::ink() };
    let dim = if selected {
        [0x55, 0x55, 0x5d]
    } else {
        sw::ink_dim()
    };
    signal_icon(face, SIDE + 16.0, y + 7.0, entry.signal, selected);
    kb::draw_left(
        face,
        &entry.ssid,
        17.0,
        11.0,
        300.0,
        SIDE + 48.0,
        y,
        ROW_H,
        ink,
    );
    if view.connecting == Some(entry.ssid.as_str()) {
        kb::draw_left(
            face,
            slot_i18n::t(lang, "device.quick.wifi.connecting"),
            13.0,
            10.0,
            100.0,
            SIDE + 360.0,
            y,
            ROW_H,
            ink,
        );
    }
    if is_current(view.connected, &entry.ssid) {
        kb::draw_left(face, slot_i18n::t(lang, "device.quick.wifi.current"), 13.0, 10.0, 56.0, SIDE + 470.0, y, ROW_H, ink);
    }
    let lock = if entry.saved {
        slot_i18n::t(lang, "device.quick.wifi.saved")
    } else if entry.secured {
        slot_i18n::t(lang, "device.quick.wifi.secured")
    } else {
        slot_i18n::t(lang, "device.quick.wifi.open")
    };
    kb::draw_left(face, lock, 13.0, 10.0, 80.0, SIDE + 540.0, y, ROW_H, dim);
}

/// Whether `connected` (an SSID, optionally followed by " · <address>") names
/// this network.
fn is_current(connected: Option<&str>, ssid: &str) -> bool {
    match connected {
        Some(name) => {
            name == ssid
                || name
                    .strip_prefix(ssid)
                    .map(|rest| rest.starts_with(" ·"))
                    .unwrap_or(false)
        }
        None => false,
    }
}

fn signal_icon(face: &mut UndoFace, x: f32, top: f32, signal: i32, selected: bool) {
    let bars = signal_bars(signal);
    let lit = if selected {
        [
            sw::ink_selected()[0],
            sw::ink_selected()[1],
            sw::ink_selected()[2],
            0xff,
        ]
    } else {
        [sw::ink()[0], sw::ink()[1], sw::ink()[2], 0xff]
    };
    let dim = if selected {
        [0x9a, 0x9a, 0x9a, 0xff]
    } else {
        [0x4c, 0x51, 0x60, 0xff]
    };
    for (i, height) in [5.0_f32, 8.0, 11.0, 14.0].into_iter().enumerate() {
        let colour = if i < bars { lit } else { dim };
        kb::fill(
            face,
            x + i as f32 * 7.0,
            top + 14.0 - height,
            4.0,
            height,
            colour,
        );
    }
}

const DIALOG_W: f32 = 560.0;
const DIALOG_H: f32 = 260.0;

/// The Web-service dialog over a semi-transparent mask: a rounded Switch-style
/// card with the slot_i18n::t(lang, "device.quick.web.title") heading, the service state, the address and the
/// hint.  `error` wins over `url`; both `None` draws the "starting…" state.
pub fn web_dialog(url: Option<&str>, error: Option<&str>, lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, [0, 0, 0, 0]);
    kb::fill(
        &mut face,
        0.0,
        0.0,
        OUT_W as f32,
        OUT_H as f32,
        [0, 0, 0, 168],
    );

    let x = ((OUT_W as f32 - DIALOG_W) / 2.0).round();
    let y = ((OUT_H as f32 - DIALOG_H) / 2.0).round();
    kb::fill(
        &mut face,
        x + 8.0,
        y + 10.0,
        DIALOG_W,
        DIALOG_H,
        [0, 0, 0, 96],
    );
    sw::fill_round(
        &mut face,
        x,
        y,
        DIALOG_W,
        DIALOG_H,
        sw::DIALOG_RADIUS,
        sw::surface_border(),
    );
    sw::fill_round(
        &mut face,
        x + 1.0,
        y + 1.0,
        DIALOG_W - 2.0,
        DIALOG_H - 2.0,
        (sw::DIALOG_RADIUS - 1.0).max(0.0),
        sw::surface(),
    );
    kb::fill(&mut face, x + 32.0, y + 30.0, 5.0, 28.0, accent());
    kb::draw_left(
        &mut face,
        slot_i18n::t(lang, "device.quick.web.title"),
        26.0,
        14.0,
        DIALOG_W - 96.0,
        x + 48.0,
        y + 20.0,
        44.0,
        sw::ink(),
    );

    match (url, error) {
        (_, Some(message)) => {
            kb::draw_left(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.failed"),
                18.0,
                12.0,
                DIALOG_W - 96.0,
                x + 48.0,
                y + 74.0,
                24.0,
                sw::accent(),
            );
            kb::draw_wrapped(
                &mut face,
                message,
                20.0,
                12.0,
                x + 48.0,
                y + 104.0,
                DIALOG_W - 96.0,
                80.0,
                3,
                sw::ink(),
            );
            kb::draw_box(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.back"),
                16.0,
                11.0,
                x + 48.0,
                y + 206.0,
                DIALOG_W - 96.0,
                30.0,
                sw::ink_dim(),
            );
        }
        (Some(url), None) => {
            kb::draw_left(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.running"),
                18.0,
                12.0,
                DIALOG_W - 96.0,
                x + 48.0,
                y + 74.0,
                24.0,
                sw::ink_dim(),
            );
            kb::draw_box(
                &mut face,
                url,
                30.0,
                14.0,
                x + 32.0,
                y + 102.0,
                DIALOG_W - 64.0,
                48.0,
                sw::ink(),
            );
            kb::draw_wrapped(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.address_hint"),
                16.0,
                11.0,
                x + 40.0,
                y + 158.0,
                DIALOG_W - 80.0,
                62.0,
                3,
                sw::ink_dim(),
            );
        }
        (None, None) => {
            kb::draw_left(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.starting"),
                18.0,
                12.0,
                DIALOG_W - 96.0,
                x + 48.0,
                y + 84.0,
                28.0,
                sw::ink_dim(),
            );
            kb::draw_box(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.wait"),
                20.0,
                12.0,
                x + 48.0,
                y + 130.0,
                DIALOG_W - 96.0,
                40.0,
                sw::ink_dim(),
            );
            kb::draw_box(
                &mut face,
                slot_i18n::t(lang, "device.quick.web.cancel"),
                16.0,
                11.0,
                x + 48.0,
                y + 206.0,
                DIALOG_W - 96.0,
                30.0,
                sw::ink_dim(),
            );
        }
    }
    face
}
