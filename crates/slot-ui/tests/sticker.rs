use slot_ui::{Language, 
    about_face, about_lines, thanks_lines, AboutFields, ABOUT_H, ABOUT_W, AUTHOR, CONTRIBUTOR,
    ORIGINAL_AUTHOR, ORIGINAL_LICENSE, ORIGINAL_URL, ORIGINAL_VERSION,
};

fn fields() -> AboutFields<'static> {
    AboutFields {
        serial: "ABCDEF0",
        date: "2026-10-22 14:00",
        dirty: false,
        device: "Anbernic RG34XX SP",
    }
}

#[test]
fn the_about_page_has_the_requested_credits() {
    let all = about_lines(&fields(), Language::ZhCn).join("\n");
    assert!(all.contains(AUTHOR), "{all}");
    assert!(
        !all.contains(CONTRIBUTOR),
        "the main About page should only keep the author"
    );
    assert_eq!(AUTHOR, "@bilibili 泡面loading");
    assert_eq!(CONTRIBUTOR, "@Bilibili仲冬临沧的十一月");
    let thanks = thanks_lines(Language::ZhCn).join("\n");
    assert!(thanks.contains(CONTRIBUTOR), "{thanks}");
}

#[test]
fn the_about_page_introduces_the_original_slot_project() {
    let all = about_lines(&fields(), Language::ZhCn).join("\n");
    for expected in [
        ORIGINAL_AUTHOR,
        ORIGINAL_VERSION,
        ORIGINAL_LICENSE,
        ORIGINAL_URL,
        "中文定制 fork",
    ] {
        assert!(all.contains(expected), "missing {expected:?} in {all}");
    }
    assert!(all.contains("GBA / GB / GBC"));
}

#[test]
fn the_old_sticker_and_barcode_content_is_gone() {
    let all = about_lines(&fields(), Language::ZhCn).join("\n");
    for forbidden in [
        "AGS-102",
        "5V",
        "1.5A",
        "S/LOT-USA",
        "伊萨卡制造",
        "CODE39",
    ] {
        assert!(!all.contains(forbidden), "stale sticker text {forbidden:?}");
    }
}

#[test]
fn the_about_face_is_full_screen_and_has_the_switch_header() {
    let face = about_face(&fields(), Language::ZhCn);
    assert_eq!((face.w, face.h), (ABOUT_W, ABOUT_H));
    let pixel = |x: u32, y: u32| -> [u8; 3] {
        let at = ((y * face.w + x) * 4) as usize;
        [face.rgba[at], face.rgba[at + 1], face.rgba[at + 2]]
    };
    assert_eq!(pixel(50, 30), [0xe6, 0x00, 0x12], "the red accent bar");
    assert_eq!(pixel(10, 10), [0x24, 0x24, 0x29], "the top band");
    assert!(
        face.rgba.chunks(4).any(|pixel| pixel[3] > 0),
        "the page is empty"
    );
}

#[test]
fn a_missing_build_value_does_not_panic() {
    let face = about_face(&AboutFields {
        serial: "",
        date: "",
        dirty: true,
        device: "未知设备",
    }, Language::ZhCn);
    assert_eq!((face.w, face.h), (ABOUT_W, ABOUT_H));
}
