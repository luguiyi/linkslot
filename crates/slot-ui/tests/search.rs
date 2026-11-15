use slot_store::Platform;
use slot_ui::collection_style;
use slot_ui::search::{
    candidate_bar as candidate_bar_with_lang, results_face as results_face_with_lang, CANDIDATE_POS,
};
use slot_ui::{Language, UndoFace};

fn candidate_bar(
    composition: &str,
    candidates: &[String],
    selected: usize,
    page: usize,
    pages: usize,
) -> UndoFace {
    candidate_bar_with_lang(
        composition,
        candidates,
        selected,
        page,
        pages,
        Language::ZhCn,
    )
}

fn results_face(
    query: &str,
    rows: &[(Platform, String)],
    selected: usize,
    offset: usize,
) -> UndoFace {
    results_face_with_lang(query, rows, selected, offset, Language::ZhCn)
}

const W: u32 = 720;
const H: u32 = 480;
const BAR_W: u32 = 640;
const BAR_H: u32 = 40;
const BG: [u8; 3] = [0x1b, 0x1b, 0x1f];
const BG_TOP: [u8; 3] = [0x24, 0x24, 0x29];
const SURFACE: [u8; 3] = [0x2d, 0x2d, 0x33];
const SELECTED: [u8; 3] = [0xf2, 0xf2, 0xf2];
const INK: [u8; 3] = [0xf4, 0xf4, 0xf4];
const ACCENT: [u8; 3] = [0xe6, 0x00, 0x12];
const LIST_TOP: u32 = 96;
const ROW_PITCH: u32 = 48;
const ROW_H: u32 = 42;
const HINT_Y: u32 = 440;

fn pixel(f: &UndoFace, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * f.w + x) * 4) as usize;
    [f.rgba[at], f.rgba[at + 1], f.rgba[at + 2], f.rgba[at + 3]]
}

fn rgb(p: [u8; 4]) -> [u8; 3] {
    [p[0], p[1], p[2]]
}

fn has_ink(f: &UndoFace) -> bool {
    f.rgba.chunks(4).any(|p| p[3] > 0)
}

fn opaque_count(f: &UndoFace, colour: [u8; 3]) -> usize {
    f.rgba
        .chunks(4)
        .filter(|p| p[3] == 255 && [p[0], p[1], p[2]] == colour)
        .count()
}

fn colour_in_band(f: &UndoFace, y0: u32, y1: u32, colour: [u8; 3]) -> usize {
    let mut count = 0;
    for y in y0..y1.min(f.h) {
        for x in 0..f.w {
            let p = pixel(f, x, y);
            if p[3] == 255 && rgb(p) == colour {
                count += 1;
            }
        }
    }
    count
}

/// Pixels that are not the page ground, i.e. text/cards drawn in a band.
fn painted_in_band(f: &UndoFace, y0: u32, y1: u32) -> usize {
    let mut count = 0;
    for y in y0..y1.min(f.h) {
        for x in 0..f.w {
            let p = pixel(f, x, y);
            if p[3] > 0 && rgb(p) != BG {
                count += 1;
            }
        }
    }
    count
}

/// Light text pixels, with a small tolerance for antialiasing at small sizes.
fn light_ink_count(f: &UndoFace) -> usize {
    f.rgba
        .chunks(4)
        .filter(|p| {
            p[3] == 255
                && (p[0] as i32 - INK[0] as i32).abs() < 24
                && (p[1] as i32 - INK[1] as i32).abs() < 24
                && (p[2] as i32 - INK[2] as i32).abs() < 24
        })
        .count()
}

fn rows(count: usize) -> Vec<(Platform, String)> {
    const TAGS: [Platform; 3] = [Platform::Gba, Platform::Gb, Platform::Gbc];
    (0..count)
        .map(|i| (TAGS[i % TAGS.len()], format!("游戏标题 {i}")))
        .collect()
}

#[test]
fn the_candidate_bar_is_a_640_by_40_dark_strip() {
    let bar = candidate_bar(
        "baokemeng",
        &["宝可梦".into(), "宝可梦绿宝石".into(), "宝".into()],
        0,
        0,
        1,
    );
    assert_eq!((bar.w, bar.h), (BAR_W, BAR_H));
    assert_eq!(CANDIDATE_POS, (40.0, 146.0));
    assert!(has_ink(&bar));
    assert_eq!(
        pixel(&bar, 0, 0)[3],
        0,
        "the rounded corner is not transparent"
    );
    assert_eq!(pixel(&bar, BAR_W - 1, BAR_H - 1)[3], 0);
    assert!(
        pixel(&bar, BAR_W / 2, BAR_H / 2)[3] > 200,
        "the bar interior is not an opaque dark card"
    );
}

#[test]
fn the_focused_candidate_is_white_with_a_red_rim() {
    let candidates = [
        "宝可梦".to_string(),
        "皮卡丘".to_string(),
        "伊布".to_string(),
    ];
    let first = candidate_bar("bao", &candidates, 0, 0, 1);
    let second = candidate_bar("bao", &candidates, 1, 0, 1);
    assert_ne!(first.rgba, second.rgba, "the focus did not move");
    assert!(
        opaque_count(&first, SELECTED) > 100,
        "the focused candidate is not a white chip"
    );
    assert!(
        opaque_count(&first, ACCENT) > 40,
        "the focused candidate has no red rim"
    );
    assert!(
        light_ink_count(&first) > 100,
        "the other candidates are not drawn in light ink"
    );
}

#[test]
fn a_focus_beyond_the_candidates_is_clamped() {
    let candidates = ["宝可梦".to_string(), "皮卡丘".to_string()];
    let far = candidate_bar("bao", &candidates, 99, 0, 1);
    assert_eq!((far.w, far.h), (BAR_W, BAR_H));
    assert!(has_ink(&far));
}

#[test]
fn an_empty_candidate_list_says_so() {
    let empty = candidate_bar("", &[], 0, 0, 1);
    assert!(has_ink(&empty), "the empty bar draws nothing");
    assert_eq!(opaque_count(&empty, SELECTED), 0);
    let with = candidate_bar("", &["宝".into()], 0, 0, 1);
    assert_ne!(empty.rgba, with.rgba);
}

#[test]
fn the_candidate_page_and_long_labels_stay_inside_the_bar() {
    let short = candidate_bar("bao", &["宝".into()], 0, 0, 2);
    let next = candidate_bar("bao", &["宝".into()], 0, 1, 2);
    assert_ne!(short.rgba, next.rgba, "the page number is not drawn");

    let long_composition = "baokemeng".repeat(8);
    let many: Vec<String> = (0..8)
        .map(|i| format!("一个非常非常长的候选游戏名字{i}"))
        .collect();
    let bar = candidate_bar(&long_composition, &many, 7, 1, 3);
    assert_eq!((bar.w, bar.h), (BAR_W, BAR_H));
    assert!(has_ink(&bar));

    let small = candidate_bar(&long_composition, &many[..2], 1, 0, 1);
    assert_eq!((small.w, small.h), (BAR_W, BAR_H));
    // pages == 0 is treated as one page.
    let zero = candidate_bar("", &[], 0, 0, 0);
    assert_eq!((zero.w, zero.h), (BAR_W, BAR_H));
}

#[test]
fn the_results_face_is_a_full_screen_switch_page() {
    let face = results_face("宝可梦", &rows(5), 0, 0);
    assert_eq!((face.w, face.h), (W, H));
    assert!(has_ink(&face));
    assert_eq!(rgb(pixel(&face, 0, 0)), BG_TOP);
    assert_eq!(rgb(pixel(&face, 50, 20)), ACCENT, "the heading accent bar");
    assert_eq!(rgb(pixel(&face, 0, H - 1)), BG);
    assert!(
        painted_in_band(&face, HINT_Y, H) > 20,
        "the key hint line is missing"
    );
    assert!(
        painted_in_band(&face, 52, 76) > 20,
        "the query/page line is missing"
    );
}

#[test]
fn the_selected_result_is_a_white_card() {
    let rows = rows(5);
    let selected = results_face("游戏", &rows, 2, 0);
    let blank = results_face("游戏", &rows, 99, 0);
    let second_top = LIST_TOP + 2 * ROW_PITCH;
    assert!(
        colour_in_band(&selected, second_top, second_top + ROW_H, SELECTED) > 5000,
        "the selected row is not a white card"
    );
    assert!(
        colour_in_band(&selected, LIST_TOP, LIST_TOP + ROW_H, SELECTED) < 500,
        "an unselected row is white"
    );
    assert!(
        colour_in_band(&selected, LIST_TOP, LIST_TOP + ROW_H, SURFACE) > 5000,
        "the unselected row is not a surface card"
    );
    assert!(
        colour_in_band(&blank, LIST_TOP, HINT_Y, SELECTED) < 500,
        "a white selection is drawn beyond the list"
    );
}

#[test]
fn platform_tags_use_their_identity_colours() {
    let rows = vec![
        (Platform::Gba, "Selected GBA".to_string()),
        (Platform::Gb, "Unselected GB".to_string()),
        (Platform::Gbc, "Unselected GBC".to_string()),
    ];
    let face = results_face("game", &rows, 0, 0);
    assert!(
        opaque_count(
            &face,
            collection_style::GBA.on_light.expect("GBA light ink")
        ) > 0,
        "the selected GBA tag is not using its light identity ink"
    );
    assert!(
        opaque_count(&face, collection_style::GB.on_dark) > 0,
        "the GB tag is not using its identity ink"
    );
    assert!(
        opaque_count(&face, collection_style::GBC.on_dark) > 0,
        "the GBC tag is not using its identity ink"
    );
}

#[test]
fn the_results_face_handles_empty_pages_and_long_text() {
    let empty = results_face("", &[], 0, 0);
    assert_eq!((empty.w, empty.h), (W, H));
    assert!(has_ink(&empty), "the empty result page draws nothing");
    let far = results_face("", &[], 99, 99);
    assert_eq!((far.w, far.h), (W, H));

    let many = rows(20);
    let paged = results_face("游戏", &many, 15, 14);
    assert_eq!((paged.w, paged.h), (W, H));
    assert_ne!(
        results_face("游戏", &many, 0, 0).rgba,
        paged.rgba,
        "the offset does not page the list"
    );

    let long_query = "这是一个特别长的搜索查询".repeat(6);
    let long_rows: Vec<(Platform, String)> = (0..7)
        .map(|i| (Platform::Gba, format!("很长的游戏名称{i}").repeat(10)))
        .collect();
    let long = results_face(&long_query, &long_rows, 3, 0);
    assert_eq!((long.w, long.h), (W, H));
    assert!(has_ink(&long));
}
