use std::collections::HashSet;

use slot_store::{parse_stamp, Device, GbPalette, Platform, ResumeKind, Shortcut, FF_SPEEDS};
use slot_ui::{
    date_time_text, quick_caret_face, quick_icon_face, quick_label_face, quick_title_face,
    quick_value_face, row_top, ClockPicker, Language, QuickRow, QuickValue, UndoFace, MENU_PAD,
    QUICK_PITCH, QUICK_TOP,
};

fn inked(f: &UndoFace, x: u32, y: u32) -> bool {
    f.rgba[((y * f.w + x) * 4 + 3) as usize] > 128
}

fn ink_columns(f: &UndoFace) -> (u32, u32) {
    let cols: Vec<u32> = (0..f.w)
        .filter(|&x| (0..f.h).any(|y| inked(f, x, y)))
        .collect();
    (
        *cols.first().expect("an empty face"),
        *cols.last().expect("an empty face"),
    )
}

#[test]
fn the_type_sits_exactly_menu_pad_in_from_both_sides_of_its_face() {
    for f in [
        quick_label_face(QuickRow::FastForwardSound, Language::ZhCn),
        quick_label_face(QuickRow::Rumble, Language::ZhCn),
        quick_label_face(QuickRow::Network, Language::ZhCn),
        quick_label_face(QuickRow::Wifi, Language::ZhCn),
        quick_label_face(QuickRow::Web, Language::ZhCn),
        quick_value_face("Off", false),
        quick_value_face(QuickValue::On.text(Language::ZhCn), true),
        quick_value_face("SEP 15 16:35", true),
    ] {
        let (first, last) = ink_columns(&f);
        let right = f.w - 1 - MENU_PAD;
        assert!(
            (MENU_PAD..MENU_PAD + 4).contains(&first),
            "ink starts at {first} of {}",
            f.w
        );
        assert!(
            (right - 4..=right).contains(&last),
            "ink ends at {last} of {}",
            f.w
        );
    }
}

#[test]
fn a_long_label_is_set_as_large_as_a_short_one() {
    let tall = |f: &UndoFace| {
        (0..f.h)
            .filter(|&y| (0..f.w).any(|x| inked(f, x, y)))
            .count()
    };
    let long = tall(&quick_label_face(QuickRow::FastForwardSound, Language::ZhCn));
    let short = tall(&quick_label_face(QuickRow::Rumble, Language::ZhCn));
    assert!(long + 1 >= short, "{long} rows of ink against {short}");
}

#[test]
fn the_fast_forward_row_offers_off_and_four_ceilings() {
    assert_eq!(FF_SPEEDS, [0, 2, 3, 4, 6]);
    assert_eq!(
        FF_SPEEDS.map(QuickValue::speed),
        [
            Some(QuickValue::Off),
            Some(QuickValue::Speed2),
            Some(QuickValue::Speed3),
            Some(QuickValue::Speed4),
            Some(QuickValue::Speed6),
        ]
    );
    for other in [1, 5, 7, 9, 16, 28, 255] {
        assert_eq!(
            QuickValue::speed(other),
            None,
            "{other}x is not a value the row has"
        );
    }
}

#[test]
fn the_rows_run_in_the_order_the_user_chose() {
    assert_eq!(
        QuickRow::MAIN.map(|row| row.label(Language::ZhCn)),
        ["画面", "卡带架", "游戏", "按键", "网络设置", "系统"]
    );
    assert_eq!(
        QuickRow::DISPLAY.map(|row| row.label(Language::ZhCn)),
        ["屏幕与亮度", "画面效果", "遮罩", "主题"]
    );
    assert_eq!(
        QuickRow::DISPLAY_PANEL.map(|row| row.label(Language::ZhCn)),
        ["亮度", "蓝光"]
    );
    assert_eq!(
        QuickRow::DISPLAY_EFFECTS.map(|row| row.label(Language::ZhCn)),
        [
            "GBA 着色器",
            "GB / GBC 着色器",
            "GB / GBC 调色板",
            "色彩校正（全机种）",
        ]
    );
    assert_eq!(
        QuickRow::MASKS.map(|row| row.label(Language::ZhCn)),
        ["FC 遮罩", "GB 遮罩", "GBC 遮罩", "SFC 遮罩"]
    );
    assert_eq!(
        QuickRow::SHELF.map(|row| row.label(Language::ZhCn)),
        ["外观与显示", "卡带界面按键"]
    );
    assert_eq!(
        QuickRow::SHELF_LOOK.map(|row| row.label(Language::ZhCn)),
        ["游戏标题", QuickRow::FallbackCover.label(Language::ZhCn), "显示最近游玩"]
    );
    assert_eq!(
        QuickRow::GAME.map(|row| row.label(Language::ZhCn)),
        ["快进", "快进声音", "倒带", "震动", "存档设置"]
    );
    assert_eq!(
        QuickRow::BUTTON_SETTINGS.map(|row| row.label(Language::ZhCn)),
        ["快捷键一览", "快捷键映射", "按键映射", "连发按键（GBA）"]
    );
    assert_eq!(
        QuickRow::SYSTEM_SETTINGS.map(|row| row.label(Language::ZhCn)),
        [
            "日期与时间",
            "语言",
            "合盖自动关机",
            "mod 管理",
            "系统升级",
            "关于",
        ],
        "系统升级 must sit right above 关于"
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
    assert_eq!(
        QuickRow::SAVES.map(|row| row.label(Language::ZhCn)),
        [
            "默认插入时加载",
            "默认退出时保存",
            "GBA 插入时加载",
            "GBA 自动保存",
            "GB 插入时加载",
            "GB 自动保存",
            "GBC 插入时加载",
            "GBC 自动保存",
        ]
    );
    let opens: Vec<QuickRow> = QuickRow::ALL.into_iter().filter(|r| r.opens()).collect();
    assert_eq!(
        opens,
        [
            QuickRow::Screen,
            QuickRow::DateTime,
            QuickRow::Mods,
            QuickRow::About,
            QuickRow::SystemUpgrade,
            QuickRow::Game,
            QuickRow::SaveSettings,
            QuickRow::ButtonLayout,
            QuickRow::ButtonSettings,
            QuickRow::SystemSettings,
            QuickRow::Shortcuts,
            QuickRow::Network,
            QuickRow::Wifi,
            QuickRow::Web,
            QuickRow::SyncStorage,
            QuickRow::ShelfKeys,
            QuickRow::ShortcutMap,
            QuickRow::CartTransfer,
            QuickRow::DisplayPanel,
            QuickRow::DisplayEffects,
            QuickRow::Shelf,
            QuickRow::ShelfLook,
            QuickRow::Masks,
            QuickRow::Theme,
        ]
    );
    for page in [
        &QuickRow::MAIN[..],
        &QuickRow::DISPLAY[..],
        &QuickRow::DISPLAY_PANEL[..],
        &QuickRow::DISPLAY_EFFECTS[..],
        &QuickRow::MASKS[..],
        &QuickRow::SHELF[..],
        &QuickRow::SHELF_LOOK[..],
        &QuickRow::THEMES[..],
        &QuickRow::GAME[..],
        &QuickRow::SAVES[..],
        &QuickRow::NETWORK[..],
        &QuickRow::SHELF_KEYS[..],
        &QuickRow::SHORTCUTS[..],
        &QuickRow::CONTROLS[..],
        &QuickRow::BUTTON_SETTINGS[..],
        &QuickRow::SYSTEM_SETTINGS[..],
    ] {
        for &row in page {
            assert_eq!(row.page(), page, "{row:?}");
            assert_eq!(row.up().down(), row, "{row:?}");
            assert_eq!(row.down().up(), row, "{row:?}");
        }
    }
}

#[test]
fn the_pages_partition_the_rows_without_overlap() {
    let mut seen: Vec<QuickRow> = Vec::new();
    for page in [
        &QuickRow::MAIN[..],
        &QuickRow::DISPLAY[..],
        &QuickRow::DISPLAY_PANEL[..],
        &QuickRow::DISPLAY_EFFECTS[..],
        &QuickRow::MASKS[..],
        &QuickRow::SHELF[..],
        &QuickRow::SHELF_LOOK[..],
        &QuickRow::THEMES[..],
        &QuickRow::GAME[..],
        &QuickRow::SAVES[..],
        &QuickRow::NETWORK[..],
        &QuickRow::SHELF_KEYS[..],
        &QuickRow::SHORTCUTS[..],
        &QuickRow::CONTROLS[..],
        &QuickRow::BUTTON_SETTINGS[..],
        &QuickRow::SYSTEM_SETTINGS[..],
    ] {
        for &row in page {
            assert!(!seen.contains(&row), "{row:?} appears on two pages");
            seen.push(row);
        }
    }
    assert_eq!(seen.len(), QuickRow::ALL.len());
    for row in QuickRow::ALL {
        assert!(seen.contains(&row), "{row:?} is on no page");
    }

    let labels: HashSet<&str> = QuickRow::ALL.iter().map(|r| r.label(Language::ZhCn)).collect();
    assert_eq!(labels.len(), QuickRow::ALL.len(), "two rows share a label");
    assert!(QuickRow::ALL.iter().all(|r| !r.label(Language::ZhCn).is_empty()));
}

#[test]
fn the_shortcut_map_page_hangs_the_thirteen_shortcuts_off_the_main_menu() {
    assert_eq!(
        QuickRow::SHORTCUTS.len(),
        Shortcut::ALL.len() + 1,
        "the shortcuts plus 重置快捷键"
    );
    assert_eq!(
        QuickRow::SHORTCUTS.map(|row| row.label(Language::ZhCn)),
        [
            "保存状态",
            "读取状态",
            "提高亮度",
            "降低亮度",
            "降低蓝光",
            "提高蓝光",
            "切换色彩校正",
            "上一个调色板",
            "下一个调色板",
            "下一个着色器",
            "上一个着色器",
            "按住快进",
            "按住倒带",
            "重置快捷键",
        ]
    );
    assert_eq!(
        QuickRow::ShortcutMap.page(),
        &QuickRow::BUTTON_SETTINGS[..]
    );
    assert_eq!(QuickRow::ShortcutMap.parent(), Some(QuickRow::ButtonSettings));
    assert_eq!(
        QuickRow::ShortcutMap.child(),
        Some(QuickRow::ShortcutSaveState)
    );
    assert!(QuickRow::ShortcutMap.opens());
    for &row in &QuickRow::SHORTCUTS {
        assert_eq!(row.page(), &QuickRow::SHORTCUTS[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::ShortcutMap), "{row:?}");
        assert_eq!(row.child(), None, "{row:?}");
        assert!(!row.opens(), "{row:?} should change the chord, not open a page");
    }
}

#[test]
fn a_shortcut_value_sits_after_the_controls_and_prints_its_chord() {
    let values: Vec<QuickValue> = QuickValue::shortcut_values().collect();
    assert_eq!(values.len(), Device::ALL.len());
    assert!(values.iter().all(|v| matches!(v, QuickValue::Shortcut(_))));
    assert_eq!(QuickValue::Shortcut(Device::Up).text(Language::ZhCn), "SEL+UP");
    assert_eq!(QuickValue::Shortcut(Device::Down).text(Language::ZhCn), "SEL+DOWN");
    assert_eq!(QuickValue::Shortcut(Device::R2).text(Language::ZhCn), "SEL+R2");
    assert_eq!(QuickValue::Shortcut(Device::Select).text(Language::ZhCn), "SEL+SELECT");
    assert_eq!(QuickValue::Shortcut(Device::Start).text(Language::ZhCn), "SEL+START");

    let palette_last = QuickValue::palette_values()
        .last()
        .expect("no palettes")
        .index();
    let control_first = QuickValue::control_values().next().expect("no controls").index();
    let shortcut_first = values[0].index();
    assert_eq!(control_first, palette_last + 1);
    assert_eq!(shortcut_first, control_first + Device::ALL.len());
    assert_eq!(
        shortcut_first,
        QuickValue::ALL.len() + GbPalette::COUNT + Device::ALL.len(),
        "a shortcut value must come after every fixed, palette and control value"
    );
    for (i, value) in values.iter().enumerate() {
        assert_eq!(value.index(), shortcut_first + i, "{value:?}");
    }
}

#[test]
fn the_network_page_starts_at_the_same_top_as_every_other_page() {
    assert_eq!(
        row_top(QuickRow::Wifi),
        row_top(QuickRow::Screen),
        "the network page does not start level with the main menu"
    );
    assert_eq!(
        row_top(QuickRow::Web) - row_top(QuickRow::Wifi),
        QUICK_PITCH,
        "the two network rows are not one pitch apart"
    );
    assert_eq!(QUICK_TOP, row_top(QuickRow::Screen));
    assert_eq!(row_top(QuickRow::Web), QUICK_TOP + QUICK_PITCH);
    assert_eq!(row_top(QuickRow::GbaShader), QUICK_TOP);
}

#[test]
fn every_row_has_an_icon_and_every_page_a_title_in_both_languages() {
    for lang in Language::ALL {
        for row in QuickRow::ALL {
            let face = quick_icon_face(row, false, lang);
            assert!(
                face.w > 0 && face.rgba.chunks(4).any(|p| p[3] > 0),
                "no icon for {row:?} in {lang:?}"
            );
            // The tile fallback has to read in both languages: a single Chinese
            // character is not an English badge.
            assert!(
                !row.icon_text(lang).is_empty(),
                "{row:?} has no fallback tile text in {lang:?}"
            );
        }
        for index in 0..QuickRow::QUICK_PAGES {
            assert!(
                !QuickRow::page_title_at(index, lang).is_empty(),
                "page {index} has no title in {lang:?}"
            );
            let face = quick_title_face(index, lang);
            assert!(face.w > 0, "no title face for page {index} in {lang:?}");
        }
    }
}

/// True for a character a Chinese-only label would use.  A handful of rows are
/// single ASCII letters in both languages, so this only asks that nothing CJK
/// survives in English.
fn is_cjk(c: char) -> bool {
    ('\u{3000}'..='\u{303f}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)
}

#[test]
fn every_label_reads_in_both_languages_and_english_is_not_chinese() {
    for lang in Language::ALL {
        for row in QuickRow::ALL {
            assert!(!row.label(lang).is_empty(), "{row:?} has no label in {lang:?}");
            assert!(
                !row.page_title(lang).is_empty(),
                "{row:?} sits on a page with no title"
            );
        }
    }
    for row in QuickRow::ALL {
        assert!(
            !row.label(Language::EnUs).chars().any(is_cjk),
            "{row:?} is still Chinese in English"
        );
        assert!(
            !row.icon_text(Language::EnUs).chars().any(is_cjk),
            "{row:?} keeps a Chinese badge in English"
        );
    }
    for index in 0..QuickRow::QUICK_PAGES {
        assert!(
            !QuickRow::page_title_at(index, Language::EnUs)
                .chars()
                .any(is_cjk),
            "page {index} is still Chinese in English"
        );
    }
}

#[test]
fn the_values_read_in_both_languages() {
    for value in QuickValue::ALL {
        assert!(!value.text(Language::ZhCn).is_empty(), "{value:?}");
        assert!(!value.text(Language::EnUs).is_empty(), "{value:?}");
        // The two language names are deliberately native in both tables.
        if matches!(value, QuickValue::LanguageZh | QuickValue::LanguageEn) {
            continue;
        }
        assert!(
            !value.text(Language::EnUs).chars().any(is_cjk),
            "{value:?} is still Chinese in English: {}",
            value.text(Language::EnUs)
        );
    }
    // The words people read on every visit.
    assert_eq!(QuickValue::On.text(Language::ZhCn), "开启");
    assert_eq!(QuickValue::On.text(Language::EnUs), "On");
    assert_eq!(QuickValue::Off.text(Language::EnUs), "Off");
    assert_eq!(QuickValue::Never.text(Language::EnUs), "Never");
    assert_eq!(QuickValue::SyncState.text(Language::EnUs), "States only");
    assert_ne!(
        QuickValue::On.text(Language::ZhCn),
        QuickValue::On.text(Language::EnUs)
    );
}

#[test]
fn the_values_read_as_the_menu_prints_them() {
    assert_eq!(
        QuickValue::ALL.map(|value| value.text(Language::ZhCn)),
        [
            "2×",
            "3×",
            "4×",
            "6×",
            "开启",
            "关闭",
            "0",
            "1",
            "2",
            "3",
            "4",
            "5",
            "6",
            "7",
            "8",
            "9",
            "LCD3x",
            "网格",
            "点阵",
            "纸感",
            "四级 LCD",
            "交换R1L1和R2L2",
            "XY替代LR",
            "同时启用",
            "3分钟",
            "10分钟",
            "30分钟",
            "永不",
            "全部",
            "仅即时存档",
            "仅电池存档",
            "默认",
            "无",
            "当前",
            "选择",
            "中文",
            "English",
        ]
    );
    assert_eq!(QuickValue::flag(true), QuickValue::On);
    assert_eq!(QuickValue::flag(false), QuickValue::Off);
}

#[test]
fn sync_content_is_the_fourth_network_row_and_cycles_three_modes() {
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
    let row = QuickRow::SyncContent;
    assert_eq!(row.page(), &QuickRow::NETWORK[..]);
    assert_eq!(row.position(), 3, "云同步内容 should be the 4th network row");
    assert_eq!(row.parent(), Some(QuickRow::Network));
    assert_eq!(row.child(), None);
    assert!(
        !row.opens(),
        "云同步内容只由左右键切换，A 没有子页面"
    );
    let values = [
        QuickValue::SyncAll,
        QuickValue::SyncState,
        QuickValue::SyncSav,
    ];
    assert_eq!(
        values.map(|value| value.text(Language::ZhCn)),
        ["全部", "仅即时存档", "仅电池存档"]
    );
    for (at, value) in values.iter().enumerate() {
        assert_eq!(value.index(), QuickValue::SyncAll.index() + at);
    }
}

#[test]
fn the_date_and_time_read_as_a_month_a_day_and_the_carousels_24_hour_clock() {
    let at = |stamp: &str| date_time_text(parse_stamp(stamp).expect("a stamp"), Language::ZhCn);
    assert_eq!(at("2026-09-15_16-35-00"), "9月15日 16:35");
    assert_eq!(at("2027-01-05_04-07-59"), "1月5日 04:07");
}

#[test]
fn a_picker_for_a_set_clock_starts_at_the_local_time_and_its_offset() {
    let utc = parse_stamp("2026-09-15_21-35-42").expect("a stamp");
    let p = ClockPicker::local(utc, -300);
    assert_eq!(p.offset_min(), -300);
    assert_eq!(
        p.secs(),
        utc - 42,
        "the picker does not show the minute it was opened in"
    );
    assert!(p.text().starts_with("2026-09-15 16:35"), "{}", p.text());
}

#[test]
fn a_value_is_grey_until_its_row_is_in_hand() {
    let inkiest = |lit| {
        let f = quick_value_face("4×", lit);
        f.rgba
            .chunks(4)
            .max_by_key(|p| p[3])
            .map(|p| [p[0], p[1], p[2]])
            .expect("an empty face")
    };
    assert_eq!(inkiest(true), [0x1a, 0x1a, 0x1a]);
    assert_eq!(inkiest(false), [0x9a, 0x9a, 0xa4]);
}

#[test]
fn the_arrows_are_faces_the_height_of_a_value() {
    for right in [false, true] {
        let caret = quick_caret_face(right);
        assert!(
            caret.w > 0 && caret.rgba.chunks(4).any(|p| p[3] > 0),
            "an empty arrow"
        );
        assert_eq!(caret.h, quick_value_face("On", true).h);
    }
}

#[test]
fn the_two_carets_point_different_ways() {
    assert_ne!(
        slot_ui::quick_caret_face(false).rgba,
        slot_ui::quick_caret_face(true).rgba,
        "a caret glyph is missing from the symbols font"
    );
}

#[test]
fn the_save_page_hangs_off_the_game_page() {
    assert_eq!(QuickRow::SaveSettings.page(), &QuickRow::GAME[..]);
    assert_eq!(QuickRow::SaveSettings.parent(), Some(QuickRow::Game));
    assert!(QuickRow::SaveSettings.opens());
    assert_eq!(QuickRow::SaveSettings.child(), Some(QuickRow::DefaultLoad));
    assert_eq!(QuickRow::SaveSettings.resume(), None);

    for row in QuickRow::SAVES {
        assert_eq!(row.page(), &QuickRow::SAVES[..], "{row:?}");
        assert_eq!(row.parent(), Some(QuickRow::SaveSettings), "{row:?}");
        assert!(!row.opens(), "{row:?}");
        assert!(
            row.resume().is_some() || row.resume_default().is_some(),
            "{row:?}"
        );
    }
    assert_eq!(
        QuickRow::SAVES.map(|row| row.resume()),
        [
            None,
            None,
            Some((Platform::Gba, ResumeKind::Load)),
            Some((Platform::Gba, ResumeKind::Save)),
            Some((Platform::Gb, ResumeKind::Load)),
            Some((Platform::Gb, ResumeKind::Save)),
            Some((Platform::Gbc, ResumeKind::Load)),
            Some((Platform::Gbc, ResumeKind::Save)),
        ]
    );
}

#[test]
fn every_page_has_its_own_title_and_index() {
    // Order must match `page_index`; the old 6-title array had three pages
    // sharing/borrowing a title and one with none, so this pins the mapping.
    let pages = [
        &QuickRow::MAIN[..],
        &QuickRow::DISPLAY[..],
        &QuickRow::DISPLAY_PANEL[..],
        &QuickRow::DISPLAY_EFFECTS[..],
        &QuickRow::MASKS[..],
        &QuickRow::SHELF[..],
        &QuickRow::SHELF_LOOK[..],
        &QuickRow::SHELF_KEYS[..],
        &QuickRow::GAME[..],
        &QuickRow::BUTTON_SETTINGS[..],
        &QuickRow::SHORTCUTS[..],
        &QuickRow::CONTROLS[..],
        &QuickRow::NETWORK[..],
        &QuickRow::SYSTEM_SETTINGS[..],
        &QuickRow::SAVES[..],
        &QuickRow::THEMES[..],
    ];
    assert_eq!(pages.len(), QuickRow::QUICK_PAGES);
    for (i, page) in pages.iter().enumerate() {
        let row = page[0];
        assert_eq!(row.page_index(), i, "{row:?}");
        assert_eq!(row.page_title(Language::ZhCn), QuickRow::pages(Language::ZhCn)[i], "{row:?}");
    }
}

#[test]
fn the_game_page_fits_the_window_without_scrolling() {
    assert!(
        QuickRow::GAME.len() <= 8,
        "GAME would scroll; move a row to a subpage instead"
    );
}
