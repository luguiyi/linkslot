use slot_ui::network::{signal_bars, web_dialog, wifi_panel, WifiEntry, WifiPanelView};
use slot_ui::{Language, QuickRow};

const W: u32 = 720;
const H: u32 = 480;
/// Switch-style ground: the charcoal page and its slightly lighter heading band.
const BACKGROUND: [u8; 4] = [0x1b, 0x1b, 0x1f, 255];
const TOP_BACKGROUND: [u8; 4] = [0x24, 0x24, 0x29, 255];
/// The charcoal card fill used by dialogs and unselected rows.
const SURFACE: [u8; 4] = [0x2d, 0x2d, 0x33, 255];
const ACCENT: [u8; 3] = [0xe6, 0x00, 0x12];
const SELECTED: [u8; 3] = [0xf2, 0xf2, 0xf2];

fn pixel(f: &slot_ui::UndoFace, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * f.w + x) * 4) as usize;
    [f.rgba[at], f.rgba[at + 1], f.rgba[at + 2], f.rgba[at + 3]]
}

fn rgb(p: [u8; 4]) -> [u8; 3] {
    [p[0], p[1], p[2]]
}

fn white_pixels_between(f: &slot_ui::UndoFace, y0: u32, y1: u32) -> usize {
    let mut count = 0;
    for y in y0..y1.min(f.h) {
        for x in 0..f.w {
            let p = pixel(f, x, y);
            if p[3] == 255 && rgb(p) == SELECTED {
                count += 1;
            }
        }
    }
    count
}

fn has_ink(f: &slot_ui::UndoFace) -> bool {
    f.rgba.chunks(4).any(|p| p[3] > 0)
}

fn entry(ssid: &str, signal: i32, secured: bool) -> WifiEntry {
    WifiEntry {
        ssid: ssid.to_string(),
        signal,
        secured,
        saved: false,
    }
}

fn networks(count: usize) -> Vec<WifiEntry> {
    (0..count)
        .map(|i| entry(&format!("Home-{i:02}"), (i % 5) as i32, i % 2 == 0))
        .collect()
}

/// A panel with the radio read and switched on.  `selected` is the absolute
/// index: 0 = switch, 1 = auto-off setting, 2 = first network row,
/// `rows.len() + 2` = manual row.
fn view<'a>(rows: &'a [WifiEntry], selected: usize) -> WifiPanelView<'a> {
    WifiPanelView {
        status: "选择网络",
        rows,
        selected,
        auto_off_label: "10 分钟",
        radio_on: true,
        radio_known: true,
        ..WifiPanelView::default()
    }
}

#[test]
fn the_quick_menu_hangs_the_network_page_off_the_main_menu() {
    assert_eq!(
        QuickRow::MAIN.map(|row| row.label(Language::ZhCn)),
        [
            "画面",
            "卡带架",
            "游戏",
            "按键",
            "网络设置",
            "系统",
        ]
    );
    assert_eq!(
        QuickRow::NETWORK,
        [
            QuickRow::Wifi,
            QuickRow::Web,
            QuickRow::SyncStorage,
            QuickRow::SyncContent,
            QuickRow::CartTransfer,
        ]
    );
    assert_eq!(
        QuickRow::NETWORK.map(|row| row.label(Language::ZhCn)),
        [
            "WiFi 连接",
            "启动 Web 服务",
            "同步方式",
            "云同步内容",
            "卡带传输模式",
        ]
    );
    for &row in &QuickRow::MAIN {
        assert_eq!(row.page(), &QuickRow::MAIN[..], "{row:?}");
    }
    for &row in &QuickRow::NETWORK {
        assert_eq!(row.page(), &QuickRow::NETWORK[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::Network), "{row:?}");
        assert_eq!(row.child(), None, "{row:?}");
        assert_eq!(row == QuickRow::SyncContent, !row.opens(), "{row:?}");
    }
    assert_eq!(QuickRow::Network.page(), &QuickRow::MAIN[..]);
    assert_eq!(QuickRow::Network.parent(), None);
    assert_eq!(QuickRow::Network.child(), Some(QuickRow::Wifi));
    assert!(QuickRow::Network.opens());
    assert_eq!(QuickRow::Network.position(), 4);
    assert_eq!(QuickRow::Wifi.position(), 0);
    assert_eq!(QuickRow::Web.position(), 1);
    assert_eq!(QuickRow::SyncContent.position(), 3);

    // The shelf deliberately has no link page any more: linking lives in the
    // in-game menu only.
    assert!(!QuickRow::SplitLink.opens());

    // The 画面 tree and the game sub-page keep working the same way.
    for &row in &QuickRow::DISPLAY {
        assert_eq!(row.page(), &QuickRow::DISPLAY[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::Screen), "{row:?}");
    }
    for &row in &QuickRow::DISPLAY_PANEL {
        assert_eq!(row.parent(), Some(QuickRow::DisplayPanel), "{row:?}");
    }
    for &row in &QuickRow::DISPLAY_EFFECTS {
        assert_eq!(row.page(), &QuickRow::DISPLAY_EFFECTS[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::DisplayEffects), "{row:?}");
    }
    for &row in &QuickRow::SHELF_LOOK {
        assert_eq!(row.page(), &QuickRow::SHELF_LOOK[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::ShelfLook), "{row:?}");
    }
    for &row in &QuickRow::GAME {
        assert_eq!(row.page(), &QuickRow::GAME[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::Game), "{row:?}");
    }
    assert_eq!(QuickRow::Screen.parent(), None);
    assert_eq!(QuickRow::Game.parent(), None);
    for &row in &QuickRow::MAIN {
        if !matches!(
            row,
            QuickRow::Screen
                | QuickRow::Shelf
                | QuickRow::Game
                | QuickRow::ButtonSettings
                | QuickRow::SystemSettings
                | QuickRow::Network
                | QuickRow::ShelfKeys
                | QuickRow::ShortcutMap
        ) {
            assert_eq!(row.child(), None, "{row:?}");
        }
    }
    for &row in &[
        QuickRow::Shortcuts,
        QuickRow::DateTime,
        QuickRow::About,
        QuickRow::Screen,
        QuickRow::Game,
        QuickRow::ButtonSettings,
        QuickRow::SystemSettings,
        QuickRow::Network,
        QuickRow::ShelfKeys,
        QuickRow::ShortcutMap,
    ] {
        assert!(row.opens(), "{row:?} should open a screen");
    }

    // Moving around the main menu and the network page stays inside the page.
    assert_eq!(QuickRow::Network.up(), QuickRow::ButtonSettings);
    assert_eq!(QuickRow::Network.down(), QuickRow::SystemSettings);
    assert_eq!(QuickRow::SystemSettings.down(), QuickRow::Screen);
    assert_eq!(QuickRow::Shelf.up(), QuickRow::Screen);
    assert_eq!(QuickRow::Shelf.down(), QuickRow::Game);
    assert_eq!(QuickRow::Wifi.down(), QuickRow::Web);
    assert_eq!(QuickRow::Web.down(), QuickRow::SyncStorage);
    assert_eq!(QuickRow::SyncStorage.down(), QuickRow::SyncContent);
    assert_eq!(QuickRow::SyncContent.down(), QuickRow::CartTransfer);
    assert_eq!(QuickRow::CartTransfer.down(), QuickRow::Wifi);
    assert_eq!(QuickRow::Wifi.up(), QuickRow::CartTransfer);
    assert_eq!(QuickRow::Web.up(), QuickRow::Wifi);
    assert_eq!(QuickRow::SyncStorage.up(), QuickRow::Web);
    assert_eq!(QuickRow::SyncContent.up(), QuickRow::SyncStorage);
}

#[test]
fn a_wifi_panel_is_a_full_screen_opaque_face() {
    let rows = networks(3);
    let f = wifi_panel(&view(&rows, 2), Language::ZhCn);
    assert_eq!((f.w, f.h), (W, H));
    assert!(has_ink(&f), "the panel is empty");
    assert_eq!(pixel(&f, 0, 0), TOP_BACKGROUND);
    assert_eq!(pixel(&f, W - 1, 0), TOP_BACKGROUND);
    assert_eq!(pixel(&f, 0, H - 1), BACKGROUND);
    assert_eq!(pixel(&f, W - 1, H - 1), BACKGROUND);
    assert!(
        f.rgba.chunks(4).all(|p| p[3] == 255),
        "the panel is translucent"
    );
}

#[test]
fn the_wifi_panel_uses_switch_cards_and_accent_marks() {
    let rows = networks(3);
    let on = wifi_panel(&view(&rows, 2), Language::ZhCn);
    assert_eq!(rgb(pixel(&on, 50, 20)), ACCENT, "the heading accent bar");
    assert!(
        white_pixels_between(&on, 234, 390) > 5000,
        "the selected network row is not a white card"
    );
    let on_switch = wifi_panel(&view(&rows, 0), Language::ZhCn);
    assert!(
        white_pixels_between(&on_switch, 234, 390) < 200,
        "a network row is white while the switch is selected"
    );
    assert!(
        white_pixels_between(&on_switch, 96, 140) > 5000,
        "the selected switch is not a white card"
    );
    assert_eq!(
        rgb(pixel(&on_switch, 600, 116)),
        ACCENT,
        "the on switch track is not accent coloured"
    );
    let off = WifiPanelView {
        radio_on: false,
        ..view(&rows, 2)
    };
    assert_ne!(
        rgb(pixel(&wifi_panel(&off, Language::ZhCn), 600, 116)),
        ACCENT,
        "the off switch track is accent coloured"
    );
}

#[test]
fn the_wifi_panel_says_so_when_nothing_was_found() {
    let empty: [WifiEntry; 0] = [];
    let found_none = view(&empty, 2);
    assert!(has_ink(&wifi_panel(&found_none, Language::ZhCn)));
    let blank = slot_ui::network::wifi_panel(&WifiPanelView::default(), Language::ZhCn);
    assert!(has_ink(&blank), "an empty panel has no label at all");
    let closed = WifiPanelView {
        radio_on: false,
        ..found_none.clone()
    };
    assert_ne!(
        wifi_panel(&found_none, Language::ZhCn).rgba,
        wifi_panel(&closed, Language::ZhCn).rgba,
        "an empty scan and a closed radio read the same"
    );
}

#[test]
fn the_wifi_panel_shows_at_most_five_rows_and_scrolls_with_offset() {
    let five = networks(5);
    let six = networks(6);
    let baseline = wifi_panel(&view(&five, 2), Language::ZhCn);
    let hidden = wifi_panel(&view(&six, 2), Language::ZhCn);
    assert_eq!(
        baseline.rgba, hidden.rgba,
        "a sixth row is drawn inside the five-row window"
    );
    let scrolled = WifiPanelView {
        offset: 1,
        ..view(&six, 2)
    };
    assert_ne!(
        baseline.rgba,
        wifi_panel(&scrolled, Language::ZhCn).rgba,
        "offset does not move the window"
    );
    let far = WifiPanelView {
        offset: 100,
        ..view(&six, 2)
    };
    assert_eq!(
        wifi_panel(&scrolled, Language::ZhCn).rgba,
        wifi_panel(&far, Language::ZhCn).rgba,
        "an out-of-range offset is not clamped"
    );
}

#[test]
fn the_auto_off_row_shows_its_value_and_can_be_selected() {
    let rows = networks(3);
    let switch = wifi_panel(&view(&rows, 0), Language::ZhCn);
    let auto = wifi_panel(&view(&rows, 1), Language::ZhCn);
    let network = wifi_panel(&view(&rows, 2), Language::ZhCn);
    assert_ne!(switch.rgba, auto.rgba, "the switch row is not highlighted");
    assert_ne!(
        auto.rgba, network.rgba,
        "the auto-off row is not highlighted"
    );

    let other_value = WifiPanelView {
        auto_off_label: "永不",
        ..view(&rows, 1)
    };
    assert_ne!(
        auto.rgba,
        wifi_panel(&other_value, Language::ZhCn).rgba,
        "the auto-off value is not drawn"
    );
    let no_value = WifiPanelView {
        auto_off_label: "",
        ..view(&rows, 1)
    };
    assert_ne!(
        auto.rgba,
        wifi_panel(&no_value, Language::ZhCn).rgba,
        "an empty auto-off value prints nothing"
    );

    // The setting row can still be changed while the radio is off.
    let off = WifiPanelView {
        radio_on: false,
        ..view(&rows, 2)
    };
    let off_auto = WifiPanelView {
        selected: 1,
        ..off.clone()
    };
    assert_ne!(
        wifi_panel(&off, Language::ZhCn).rgba,
        wifi_panel(&off_auto, Language::ZhCn).rgba,
        "the auto-off row cannot be selected while the radio is off"
    );
    let off_other = WifiPanelView {
        auto_off_label: "永不",
        ..off.clone()
    };
    assert_ne!(
        wifi_panel(&off, Language::ZhCn).rgba,
        wifi_panel(&off_other, Language::ZhCn).rgba,
        "the auto-off value is hidden while the radio is off"
    );
}

#[test]
fn the_absolute_selection_marks_the_settings_a_row_or_the_manual_row() {
    let rows = networks(4);
    let switch = wifi_panel(&view(&rows, 0), Language::ZhCn);
    let auto = wifi_panel(&view(&rows, 1), Language::ZhCn);
    let first = wifi_panel(&view(&rows, 2), Language::ZhCn);
    let second = wifi_panel(&view(&rows, 3), Language::ZhCn);
    assert_ne!(switch.rgba, auto.rgba, "the switch row is not highlighted");
    assert_ne!(auto.rgba, first.rgba, "the auto-off row is not highlighted");
    assert_ne!(
        first.rgba, second.rgba,
        "the network row is not highlighted"
    );

    // `rows.len() + 2` is the manual row; `manual_row` says it is selected.
    let manual = WifiPanelView {
        manual_row: true,
        ..view(&rows, rows.len() + 2)
    };
    let no_manual = WifiPanelView {
        manual_row: false,
        ..view(&rows, rows.len() + 2)
    };
    assert_ne!(
        wifi_panel(&manual, Language::ZhCn).rgba,
        wifi_panel(&no_manual, Language::ZhCn).rgba,
        "the manual row cannot be selected"
    );
    assert_ne!(wifi_panel(&manual, Language::ZhCn).rgba, first.rgba);
}

#[test]
fn the_switch_reads_known_and_closed_states() {
    let rows = networks(4);
    let on = view(&rows, 2);
    let off = WifiPanelView {
        radio_on: false,
        ..on.clone()
    };
    let reading = WifiPanelView {
        radio_known: false,
        ..on.clone()
    };
    let reading_off = WifiPanelView {
        radio_known: false,
        radio_on: false,
        ..on.clone()
    };

    assert_ne!(
        wifi_panel(&on, Language::ZhCn).rgba,
        wifi_panel(&off, Language::ZhCn).rgba,
        "the switch position is not drawn"
    );
    assert_ne!(
        wifi_panel(&on, Language::ZhCn).rgba,
        wifi_panel(&reading, Language::ZhCn).rgba,
        "the unknown radio state is not shown"
    );
    assert_eq!(
        wifi_panel(&reading, Language::ZhCn).rgba,
        wifi_panel(&reading_off, Language::ZhCn).rgba,
        "an unknown radio state draws a switch position"
    );

    // Unknown state: the setting rows are withheld and the selection does not
    // matter until the radio state has been read.
    let reading_auto = WifiPanelView {
        selected: 1,
        ..reading.clone()
    };
    assert_eq!(
        wifi_panel(&reading, Language::ZhCn).rgba,
        wifi_panel(&reading_auto, Language::ZhCn).rgba,
        "the setting rows are drawn while the radio state is unknown"
    );
    let reading_value = WifiPanelView {
        auto_off_label: "永不",
        ..reading.clone()
    };
    assert_eq!(
        wifi_panel(&reading, Language::ZhCn).rgba,
        wifi_panel(&reading_value, Language::ZhCn).rgba,
        "the auto-off value is drawn while the radio state is unknown"
    );

    // Closed: the scan list and the manual row disappear, but both setting
    // rows stay visible.
    let empty: [WifiEntry; 0] = [];
    let off_empty = WifiPanelView {
        radio_on: false,
        ..view(&empty, 2)
    };
    let off_rows = WifiPanelView {
        radio_on: false,
        ..view(&rows, 2)
    };
    assert_eq!(
        wifi_panel(&off_empty, Language::ZhCn).rgba,
        wifi_panel(&off_rows, Language::ZhCn).rgba,
        "the list is drawn while the radio is off"
    );
    let off_manual = WifiPanelView {
        manual_row: true,
        selected: rows.len() + 2,
        ..off.clone()
    };
    assert_eq!(
        wifi_panel(&off, Language::ZhCn).rgba,
        wifi_panel(&off_manual, Language::ZhCn).rgba,
        "the manual row is drawn while the radio is off"
    );

    // The switch itself stays selectable while the radio is off.
    let off_switch = WifiPanelView {
        selected: 0,
        ..off.clone()
    };
    assert_ne!(
        wifi_panel(&off, Language::ZhCn).rgba,
        wifi_panel(&off_switch, Language::ZhCn).rgba,
        "the switch row cannot be selected while the radio is off"
    );
}

#[test]
fn the_wifi_panel_prints_status_errors_and_the_current_network() {
    let rows = networks(2);
    let base = view(&rows, 2);
    let connected = WifiPanelView {
        connected: Some("Home-00 · 192.168.1.5"),
        ..base.clone()
    };
    assert_ne!(
        wifi_panel(&base, Language::ZhCn).rgba,
        wifi_panel(&connected, Language::ZhCn).rgba,
        "the current network is not marked"
    );
    let elsewhere = WifiPanelView {
        connected: Some("Home-01 · 192.168.1.6"),
        ..base.clone()
    };
    assert_ne!(
        wifi_panel(&connected, Language::ZhCn).rgba,
        wifi_panel(&elsewhere, Language::ZhCn).rgba,
        "the marker does not follow the current network"
    );
    let connecting = WifiPanelView {
        connecting: Some("Home-01"),
        ..base.clone()
    };
    assert_ne!(
        wifi_panel(&base, Language::ZhCn).rgba,
        wifi_panel(&connecting, Language::ZhCn).rgba,
        "the connecting row is not shown"
    );
    let failed = WifiPanelView {
        error: Some("密码错误"),
        ..base.clone()
    };
    assert_ne!(
        wifi_panel(&base, Language::ZhCn).rgba,
        wifi_panel(&failed, Language::ZhCn).rgba,
        "the error is not shown"
    );
    let other_status = WifiPanelView {
        status: "正在扫描…",
        ..base.clone()
    };
    assert_ne!(
        wifi_panel(&base, Language::ZhCn).rgba,
        wifi_panel(&other_status, Language::ZhCn).rgba,
        "the status is not shown"
    );
}

#[test]
fn signal_readings_become_zero_to_four_bars() {
    assert_eq!(signal_bars(0), 0);
    assert_eq!(signal_bars(4), 4);
    assert_eq!(signal_bars(-40), 4);
    assert_eq!(signal_bars(-55), 3);
    assert_eq!(signal_bars(-70), 2);
    assert_eq!(signal_bars(-85), 1);
    assert_eq!(signal_bars(-100), 0);
    assert_eq!(signal_bars(-120), 0);
    assert_eq!(signal_bars(100), 4);
    for signal in -120..=120 {
        assert!(signal_bars(signal) <= 4, "{signal}");
    }
    for signal in [0, 1, 2, 3, 4] {
        assert_eq!(signal_bars(signal), signal as usize);
    }
}

#[test]
fn wifi_entries_compare_and_clone_by_their_fields() {
    let a = entry("Home", -50, true);
    let b = a.clone();
    assert_eq!(a, b);
    assert_eq!(a.ssid, "Home");
    assert_eq!(a.signal, -50);
    assert!(a.secured);
    assert_ne!(a, entry("Home", -50, false));
    assert!(format!("{a:?}").contains("Home"));
}

#[test]
fn a_web_dialog_is_a_dark_card_on_a_translucent_mask() {
    let success = web_dialog(Some("http://192.168.1.5:8080/"), None, Language::ZhCn);
    assert_eq!((success.w, success.h), (W, H));
    assert!(has_ink(&success), "the dialog is empty");
    let mask = pixel(&success, 0, 0);
    assert!(
        (1..255).contains(&mask[3]),
        "the mask is not semi-transparent: {mask:?}"
    );
    assert_eq!((mask[0], mask[1], mask[2]), (0, 0, 0));
    assert_eq!(
        pixel(&success, W / 2, 350),
        SURFACE,
        "the rounded dialog card is not an opaque surface"
    );
    assert_eq!(
        rgb(pixel(&success, 114, 150)),
        ACCENT,
        "the dialog has no accent bar"
    );

    let failure = web_dialog(None, Some("端口 8080 已被占用"), Language::ZhCn);
    assert_eq!((failure.w, failure.h), (W, H));
    assert_ne!(success.rgba, failure.rgba, "failure looks like success");
    assert!(
        failure.rgba.chunks(4).any(|p| {
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            r > 150 && r > g + 30 && r > b + 30
        }),
        "the failure message is not red"
    );

    let starting = web_dialog(None, None, Language::ZhCn);
    assert_ne!(starting.rgba, success.rgba, "starting looks like success");
    assert_ne!(starting.rgba, failure.rgba, "starting looks like failure");
    assert_ne!(
        web_dialog(Some("http://10.0.0.2:8080/"), None, Language::ZhCn).rgba,
        success.rgba,
        "the address is not drawn"
    );
}
