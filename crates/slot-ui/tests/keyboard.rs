use slot_ui::keyboard::{input_line, panel, row_rect, rows, Cursor, Key, INPUT_POS, LAYOUT_COUNT};

const W: u32 = 720;
const H: u32 = 480;
const FUNCTION: [&str; 5] = ["空格", "删除", "切换", "确定", "取消"];
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:'\",.<>/?\\|~`";

fn pixel(f: &slot_ui::UndoFace, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * f.w + x) * 4) as usize;
    [f.rgba[at], f.rgba[at + 1], f.rgba[at + 2], f.rgba[at + 3]]
}

fn has_ink(f: &slot_ui::UndoFace) -> bool {
    f.rgba.chunks(4).any(|p| p[3] > 0)
}

fn keys_of(layout: usize) -> Vec<&'static str> {
    rows(layout)
        .iter()
        .flat_map(|row| row.iter().copied())
        .collect()
}

#[test]
fn there_are_three_pages_with_short_rows_and_function_keys_last() {
    assert_eq!(LAYOUT_COUNT, 3);
    for layout in 0..LAYOUT_COUNT {
        let page = rows(layout);
        assert!(!page.is_empty(), "layout {layout} has no rows");
        for (r, row) in page.iter().enumerate() {
            assert!(
                (1..=10).contains(&row.len()),
                "layout {layout} row {r} has {} keys",
                row.len()
            );
            assert!(row.iter().all(|key| !key.is_empty()));
        }
        let last = page[page.len() - 1];
        let start = last
            .iter()
            .position(|key| *key == FUNCTION[0])
            .expect("the last row has no function keys");
        assert_eq!(
            &last[start..start + FUNCTION.len()],
            &FUNCTION,
            "layout {layout} does not put the function keys on the last row"
        );
        assert_eq!(
            start,
            if layout == 2 { 2 } else { 0 },
            "layout {layout} keeps stray keys before the function keys"
        );
        let labels: std::collections::HashSet<&str> = keys_of(layout).into_iter().collect();
        assert_eq!(
            labels.len(),
            keys_of(layout).len(),
            "layout {layout} repeats a key"
        );
    }
    assert_eq!(rows(LAYOUT_COUNT), rows(0), "layout index does not wrap");
}

#[test]
fn the_letter_pages_are_full_five_by_ten_grids() {
    for layout in [0, 1] {
        let page = rows(layout);
        assert_eq!(page.len(), 5, "layout {layout} should have five rows");
        for (r, row) in page.iter().enumerate() {
            assert_eq!(
                row.len(),
                10,
                "layout {layout} row {r} is not ten keys wide"
            );
        }
        // The digits are a row of their own on both letter pages, so switching
        // pages never moves a key cap.
        assert_eq!(rows(layout)[3], rows(0)[3]);
    }
}

#[test]
fn the_pages_carry_the_characters_they_promise() {
    let shared_symbols = [';', ',', '.', '/', '!', '?', ':', '*', '#'];
    let lower = keys_of(0);
    for c in 'a'..='z' {
        assert!(
            lower.contains(&c.to_string().as_str()),
            "lower page misses {c}"
        );
    }
    let upper = keys_of(1);
    for c in 'A'..='Z' {
        assert!(
            upper.contains(&c.to_string().as_str()),
            "upper page misses {c}"
        );
    }
    for page in [&lower, &upper] {
        for c in '0'..='9' {
            assert!(
                page.contains(&c.to_string().as_str()),
                "a letter page misses {c}"
            );
        }
        for c in shared_symbols {
            assert!(
                page.contains(&c.to_string().as_str()),
                "a letter page misses {c:?}"
            );
        }
    }
    let symbols = keys_of(2);
    for c in '0'..='9' {
        assert!(
            symbols.contains(&c.to_string().as_str()),
            "symbols page misses {c}"
        );
    }
    for c in SYMBOLS.chars() {
        assert!(
            symbols.contains(&c.to_string().as_str()),
            "symbols page misses {c:?}"
        );
    }
}

#[test]
fn a_cursor_walks_the_keys_without_leaving_the_page() {
    for layout in 0..LAYOUT_COUNT {
        let page = rows(layout);
        for (row, keys) in page.iter().enumerate() {
            let mut cursor = Cursor {
                layout,
                row,
                col: 0,
            };
            assert_eq!(cursor.left().col, keys.len() - 1);
            assert_eq!(cursor.left().row, row);
            assert_eq!(cursor.right().col, 1 % keys.len());
            for col in 0..keys.len() {
                cursor.col = col;
                assert_eq!(cursor.right().left(), cursor);
                assert_eq!(cursor.left().right(), cursor);
                assert_eq!(cursor.up().layout, layout);
                assert_eq!(cursor.down().layout, layout);
                assert!(cursor.up().row < page.len());
                assert!(cursor.down().row < page.len());
            }
        }
        assert_eq!(
            Cursor {
                layout,
                row: 0,
                col: 0
            }
            .up()
            .row,
            page.len() - 1
        );
        assert_eq!(
            Cursor {
                layout,
                row: 0,
                col: 0
            }
            .down()
            .row,
            1 % page.len()
        );
    }
}

#[test]
fn a_stale_cursor_is_clamped_instead_of_panicking() {
    let stale = Cursor {
        layout: 99,
        row: 99,
        col: 99,
    };
    let fixed = stale.up();
    assert_eq!(fixed.layout, 99 % LAYOUT_COUNT);
    let page = rows(fixed.layout);
    assert!(fixed.row < page.len());
    assert!(fixed.col < page[fixed.row].len());
    // Reading a stale cursor directly is safe too: every accessor normalises.
    let _ = stale.key();
    let (x, y, w, h) = row_rect(stale);
    assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
    for cursor in [
        stale.up(),
        stale.down(),
        stale.left(),
        stale.right(),
        stale.cycle_layout(),
    ] {
        let page = rows(cursor.layout);
        assert!(cursor.layout < LAYOUT_COUNT);
        assert!(cursor.row < page.len());
        assert!(cursor.col < page[cursor.row].len());
        let _ = cursor.key();
        let (x, y, w, h) = row_rect(cursor);
        assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
        assert!(x + w <= W as f32 && y + h <= H as f32);
    }
}

#[test]
fn cycling_the_layout_visits_every_page_and_comes_back() {
    let mut cursor = Cursor::default();
    let mut seen = Vec::new();
    for _ in 0..LAYOUT_COUNT {
        cursor = cursor.cycle_layout();
        seen.push(cursor.layout);
    }
    assert_eq!(seen, [1, 2, 0]);
    assert_eq!(cursor, Cursor::default());
    assert_eq!(
        Cursor::default()
            .cycle_layout()
            .cycle_layout()
            .cycle_layout(),
        Cursor::default()
    );
    let moved = Cursor {
        layout: 0,
        row: 2,
        col: 3,
    }
    .cycle_layout();
    assert_eq!(moved.layout, 1);
    assert_eq!(moved.key(), Key::Char('V'));
}

#[test]
fn the_key_under_the_cursor_is_the_key_its_label_names() {
    assert_eq!(Cursor::default().key(), Key::Char('q'));
    assert_eq!(
        Cursor {
            layout: 1,
            row: 0,
            col: 0
        }
        .key(),
        Key::Char('Q')
    );
    assert_eq!(
        Cursor {
            layout: 2,
            row: 0,
            col: 0
        }
        .key(),
        Key::Char('0')
    );
    // The letter pages carry their own digits and punctuation row.
    assert_eq!(
        Cursor {
            layout: 0,
            row: 3,
            col: 0
        }
        .key(),
        Key::Char('1')
    );
    assert_eq!(
        Cursor {
            layout: 0,
            row: 2,
            col: 9
        }
        .key(),
        Key::Char('/')
    );
    assert_eq!(
        Cursor {
            layout: 0,
            row: 4,
            col: 5
        }
        .key(),
        Key::Char('!')
    );
    assert_eq!(
        Cursor {
            layout: 1,
            row: 4,
            col: 9
        }
        .key(),
        Key::Char('#')
    );
    for layout in 0..LAYOUT_COUNT {
        let page = rows(layout);
        let last = page.len() - 1;
        let start = page[last]
            .iter()
            .position(|key| *key == FUNCTION[0])
            .expect("the last row has no function keys");
        for (i, expected) in [
            Key::Space,
            Key::Backspace,
            Key::Layout,
            Key::Confirm,
            Key::Cancel,
        ]
        .into_iter()
        .enumerate()
        {
            let col = start + i;
            assert_eq!(
                Cursor {
                    layout,
                    row: last,
                    col
                }
                .key(),
                expected
            );
        }
        for (row, keys) in page.iter().enumerate() {
            for col in 0..keys.len() {
                let key = Cursor { layout, row, col }.key();
                if let Key::Char(c) = key {
                    assert_ne!(c, '\0');
                }
            }
        }
    }
}

#[test]
fn every_key_has_a_cap_inside_the_screen_and_caps_do_not_overlap() {
    for layout in 0..LAYOUT_COUNT {
        let page = rows(layout);
        let mut previous_bottom: Option<f32> = None;
        for (row, keys) in page.iter().enumerate() {
            let mut previous_right: Option<f32> = None;
            let mut row_y = None;
            for col in 0..keys.len() {
                let (x, y, w, h) = row_rect(Cursor { layout, row, col });
                assert!(x >= 0.0, "{layout}/{row}/{col} starts left of the screen");
                assert!(y >= 0.0, "{layout}/{row}/{col} starts above the screen");
                assert!(x + w <= W as f32, "{layout}/{row}/{col} ends off-screen");
                assert!(y + h <= H as f32, "{layout}/{row}/{col} ends off-screen");
                assert!(
                    y >= INPUT_POS.1 + 90.0,
                    "{layout}/{row}/{col} overlaps the input band"
                );
                if let Some(right) = previous_right {
                    assert!(x >= right, "{layout}/{row}/{col} overlaps its neighbour");
                }
                previous_right = Some(x + w);
                assert_eq!(*row_y.get_or_insert(y), y, "row {row} is not level");
            }
            let y = row_y.expect("a row without keys");
            if let Some(bottom) = previous_bottom {
                assert!(
                    y >= bottom,
                    "layout {layout} row {row} overlaps the row above"
                );
            }
            previous_bottom = Some(
                y + row_rect(Cursor {
                    layout,
                    row,
                    col: 0,
                })
                .3,
            );
        }
    }
}

#[test]
fn a_panel_is_a_full_screen_face_with_a_blank_input_band() {
    for layout in 0..LAYOUT_COUNT {
        let f = panel(
            layout,
            "WiFi 密码 · Home",
            "A 输入   B 删除   Y 切换   START 确定   X 取消",
            slot_ui::Language::ZhCn,
        );
        assert_eq!((f.w, f.h), (W, H));
        assert!(has_ink(&f), "layout {layout} panel is empty");
        assert_eq!(
            pixel(&f, 0, 0)[3],
            255,
            "layout {layout} panel is translucent"
        );
        assert_eq!(pixel(&f, W - 1, H - 1)[3], 255);
        let x0 = INPUT_POS.0 as u32;
        let y0 = INPUT_POS.1 as u32;
        for y in y0..y0 + 90 {
            for x in x0..x0 + 640 {
                assert_eq!(
                    pixel(&f, x, y),
                    [0x1b, 0x1b, 0x1f, 255],
                    "layout {layout} draws in the input band at ({x}, {y})"
                );
            }
        }
    }
    let one = panel(0, "WiFi 密码", "A 输入", slot_ui::Language::ZhCn);
    let two = panel(1, "WiFi 密码", "A 输入", slot_ui::Language::ZhCn);
    assert_ne!(one.rgba, two.rgba, "the layout argument is ignored");
    let other_title = panel(0, "输入 WiFi 名称", "A 输入", slot_ui::Language::ZhCn);
    assert_ne!(one.rgba, other_title.rgba, "the title is not drawn");
    let other_hint = panel(0, "WiFi 密码", "START 确定", slot_ui::Language::ZhCn);
    assert_ne!(one.rgba, other_hint.rgba, "the hint is not drawn");
}

#[test]
fn the_input_line_shows_the_field_and_masks_passwords() {
    assert!(INPUT_POS.0 + 640.0 <= W as f32);
    let plain = input_line("hunter2", false, None);
    assert_eq!((plain.w, plain.h), (640, 90));
    assert!(has_ink(&plain), "the text is not drawn");
    assert_eq!(
        pixel(&plain, 0, 0)[3],
        0,
        "the background is not transparent"
    );
    assert_eq!(pixel(&plain, 639, 89)[3], 0);

    let masked = input_line("hunter2", true, None);
    assert!(has_ink(&masked));
    assert_ne!(masked.rgba, plain.rgba, "the password is not masked");
    assert_eq!(
        masked.rgba,
        input_line("•••••••", false, None).rgba,
        "masking does not turn the characters into bullets"
    );
    assert_ne!(
        input_line("hunter2", true, None).rgba,
        input_line("hunter22", true, None).rgba
    );

    let empty = input_line("", false, None);
    let ink = |f: &slot_ui::UndoFace| {
        f.rgba
            .chunks(4)
            .filter(|p| p[3] == 255 && [p[0], p[1], p[2]] == [0xf4, 0xf4, 0xf4])
            .count()
    };
    assert_eq!(ink(&empty), 0, "empty input prints text");
    assert!(ink(&plain) > 0, "the typed text is not inked");
}

#[test]
fn the_error_line_is_red_and_only_appears_with_an_error() {
    let clean = input_line("abc", false, None);
    let broken = input_line("abc", false, Some("密码至少 8 位（留空表示开放网络）"));
    assert_ne!(clean.rgba, broken.rgba);
    let red = |f: &slot_ui::UndoFace| {
        f.rgba
            .chunks(4)
            .any(|p| p[3] > 128 && p[0] == 0xe6 && p[1] == 0x00 && p[2] == 0x12)
    };
    assert!(red(&broken), "the error line is not red");
    assert!(!red(&clean), "clean input shows a red line");
}

#[test]
fn the_keys_are_switch_cards_and_the_selected_one_is_white_with_a_red_rim() {
    use slot_ui::keyboard::panel_selected;

    let plain = panel(0, "标题", "提示", slot_ui::Language::ZhCn);
    let picked = panel_selected(
        0,
        "标题",
        "提示",
        Cursor::default(),
        slot_ui::Language::ZhCn,
    );
    assert_ne!(plain.rgba, picked.rgba, "the selection is not drawn");

    let (x, y, _w, h) = row_rect(Cursor::default());
    let (xi, yi) = (x as u32, y as u32);
    let surface = pixel(&plain, xi + 4, yi + 4);
    assert_eq!(
        [surface[0], surface[1], surface[2]],
        [0x2d, 0x2d, 0x33],
        "an unselected key is not a charcoal card"
    );
    let white = pixel(&picked, xi + 4, yi + 4);
    assert_eq!(
        [white[0], white[1], white[2]],
        [0xf2, 0xf2, 0xf2],
        "the selected key is not white"
    );
    let rim = pixel(&picked, xi + 1, yi + h as u32 / 2);
    assert_eq!(
        [rim[0], rim[1], rim[2]],
        [0xe6, 0x00, 0x12],
        "the selected key has no red rim"
    );

    for layout in 0..LAYOUT_COUNT {
        let f = panel_selected(
            layout,
            "标题",
            "提示",
            Cursor {
                layout,
                row: 0,
                col: 0,
            },
            slot_ui::Language::ZhCn,
        );
        assert_eq!((f.w, f.h), (W, H));
        assert!(has_ink(&f));
    }
}

#[test]
fn the_input_line_is_a_rounded_switch_field() {
    let plain = input_line("abc", false, None);
    assert_eq!((plain.w, plain.h), (640, 90));
    assert_eq!(
        pixel(&plain, 0, 0)[3],
        0,
        "the rounded corner is not transparent"
    );
    let mut surface = 0;
    for y in 6..84 {
        for x in 6..634 {
            let p = pixel(&plain, x, y);
            if p[3] == 255 && [p[0], p[1], p[2]] == [0x2d, 0x2d, 0x33] {
                surface += 1;
            }
        }
    }
    assert!(
        surface > 1000,
        "no surface card behind the field: {surface}"
    );
}
