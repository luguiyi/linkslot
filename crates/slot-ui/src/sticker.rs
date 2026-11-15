//! The Switch-style About page: linkSlot credits, the previous Chinese
//! contributor and a short introduction to the original Slot project.

use slot_gfx::{Draw, TexId, OUT_H, OUT_W};
use slot_i18n::Language;

use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

pub const ABOUT_W: u32 = OUT_W;
pub const ABOUT_H: u32 = OUT_H;

pub const AUTHOR: &str = "@bilibili 泡面loading";
pub const CONTRIBUTOR: &str = "@Bilibili仲冬临沧的十一月";
pub const CRAZY_SIX: &str = "@bilibili CrazySix";
pub const THANKS: &[&str] = &[CONTRIBUTOR, CRAZY_SIX];
pub const ORIGINAL_AUTHOR: &str = "BrandonKowalski";
pub const ORIGINAL_VERSION: &str = "1.9.0";
pub const ORIGINAL_LICENSE: &str = "GPL-3.0-or-later";
pub const ORIGINAL_URL: &str = "github.com/slot-cfw/slot";
pub const PROJECT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TEST_WARNING: &str = "测试版 · 请勿随意分发";
pub const FREE_USE: &str = "免费使用：本软件完全免费，禁止倒卖；如为付费购买请立即退款。";

/// Which face the About screen is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AboutPage {
    #[default]
    Main,
    Thanks,
}

pub struct AboutFields<'a> {
    pub serial: &'a str,
    pub date: &'a str,
    pub dirty: bool,
    pub device: &'a str,
}

pub fn about_lines(f: &AboutFields, lang: Language) -> Vec<String> {
    vec![
        slot_i18n::t_args(lang, "device.quick.about.author", &[("author", AUTHOR)]),
        slot_i18n::t_args(lang, "device.quick.about.version", &[("version", PROJECT_VERSION), ("warning", slot_i18n::t(lang, "device.quick.about.test_warning"))]),
        slot_i18n::t_args(lang, "device.quick.about.device", &[("device", &f.device)]),
        slot_i18n::t(lang, "device.quick.about.free_use").to_string(),
        slot_i18n::t_args(
            lang,
            "device.quick.about.original",
            &[
                ("author", ORIGINAL_AUTHOR),
                ("version", ORIGINAL_VERSION),
                ("license", ORIGINAL_LICENSE),
            ],
        ),
        slot_i18n::t_args(lang, "device.quick.about.github", &[("url", ORIGINAL_URL)]),
        slot_i18n::t(lang, "device.quick.about.tagline").to_string(),
        slot_i18n::t(lang, "device.quick.about.fork").to_string(),
        slot_i18n::t_args(
            lang,
            "device.quick.about.build",
            &[
                ("serial", &f.serial),
                ("dirty", if f.dirty { "*" } else { "" }),
                ("date", &f.date),
            ],
        ),
    ]
}

/// The names shown after choosing `感谢名单` on the About screen.
pub fn thanks_lines(lang: Language) -> Vec<String> {
    vec![
        slot_i18n::t_args(lang, "device.quick.about.author", &[("author", AUTHOR)]),
        slot_i18n::t_args(lang, "device.quick.about.thanks_line", &[("name", CONTRIBUTOR)]),
        slot_i18n::t_args(lang, "device.quick.about.thanks_line", &[("name", CRAZY_SIX)]),
        slot_i18n::t(lang, "device.quick.about.thanks_footer").to_string(),
    ]
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

fn fill(face: &mut UndoFace, x: f32, y: f32, w: f32, h: f32, colour: [u8; 4]) {
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let x1 = (x + w).round().min(face.w as f32) as u32;
    let y1 = (y + h).round().min(face.h as f32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let at = ((py * face.w + px) * 4) as usize;
            face.rgba[at..at + 4].copy_from_slice(&colour);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn line(
    face: &mut UndoFace,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    px: f32,
    min_px: f32,
    colour: [u8; 3],
) {
    kb::draw_left(face, text, px, min_px, width, x, y, px * 1.5, colour);
}

fn blit(dst: &mut UndoFace, src: &UndoFace, x: f32, y: f32) {
    let (x, y) = (x.round() as u32, y.round() as u32);
    for row in 0..src.h {
        for col in 0..src.w {
            let dx = x + col;
            let dy = y + row;
            if dx >= dst.w || dy >= dst.h {
                continue;
            }
            let from = ((row * src.w + col) * 4) as usize;
            let to = ((dy * dst.w + dx) * 4) as usize;
            dst.rgba[to..to + 4].copy_from_slice(&src.rgba[from..from + 4]);
        }
    }
}

fn draw_card(face: &mut UndoFace, x: f32, y: f32, w: u32, h: u32) {
    let card = sw::rounded_face(w, h, 12.0, sw::surface(), Some(sw::surface_border()));
    blit(face, &card, x, y);
}

pub fn about_face(f: &AboutFields, lang: Language) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (ABOUT_W * ABOUT_H * 4) as usize],
        w: ABOUT_W,
        h: ABOUT_H,
    };
    fill(&mut face, 0.0, 0.0, ABOUT_W as f32, ABOUT_H as f32, sw::bg());
    fill(&mut face, 0.0, 0.0, ABOUT_W as f32, 88.0, sw::bg_top());
    sw::fill_round(&mut face, 48.0, 22.0, 5.0, 30.0, 2.0, accent());
    line(&mut face, slot_i18n::t(lang, "device.quick.about.title"), 64.0, 16.0, 600.0, 30.0, 20.0, sw::ink());

    // Brand and product framing.
    draw_card(&mut face, 40.0, 72.0, 640, 96);
    line(&mut face, "linkSlot", 64.0, 78.0, 592.0, 28.0, 18.0, sw::ink());
    line(
        &mut face,
        &slot_i18n::t_args(lang, "device.quick.about.current_version", &[("version", PROJECT_VERSION)]),
        64.0,
        106.0,
        592.0,
        13.0,
        10.0,
        sw::ink_dim(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.test_warning"),
        64.0,
        130.0,
        592.0,
        11.0,
        9.0,
        [sw::accent()[0], sw::accent()[1], sw::accent()[2]],
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.free_use"),
        64.0,
        148.0,
        592.0,
        11.0,
        9.0,
        sw::ink_dim(),
    );

    // Author and the actual machine name; contributors live on the thanks page.
    draw_card(&mut face, 40.0, 176.0, 640, 64);
    line(&mut face, slot_i18n::t(lang, "device.quick.about.label_author"), 64.0, 190.0, 80.0, 14.0, 11.0, sw::ink_dim());
    line(&mut face, AUTHOR, 150.0, 182.0, 506.0, 22.0, 16.0, sw::ink());
    line(&mut face, slot_i18n::t(lang, "device.quick.about.label_device"), 64.0, 214.0, 80.0, 13.0, 10.0, sw::ink_dim());
    line(&mut face, f.device, 150.0, 208.0, 506.0, 14.0, 10.0, sw::ink());

    // Thanks list entry.
    draw_card(&mut face, 40.0, 248.0, 640, 48);
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.thanks_list"),
        64.0,
        260.0,
        300.0,
        18.0,
        13.0,
        sw::ink(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.tap_a"),
        500.0,
        262.0,
        156.0,
        12.0,
        9.0,
        sw::ink_dim(),
    );

    // Original Slot project.
    draw_card(&mut face, 40.0, 304.0, 640, 96);
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.original_title"),
        64.0,
        314.0,
        592.0,
        20.0,
        14.0,
        sw::ink(),
    );
    line(
        &mut face,
        &slot_i18n::t_args(
            lang,
            "device.quick.about.original_line",
            &[
                ("author", ORIGINAL_AUTHOR),
                ("version", ORIGINAL_VERSION),
                ("license", ORIGINAL_LICENSE),
            ],
        ),
        64.0,
        338.0,
        592.0,
        15.0,
        11.0,
        sw::ink_dim(),
    );
    line(
        &mut face,
        &slot_i18n::t_args(lang, "device.quick.about.github", &[("url", ORIGINAL_URL)]),
        64.0,
        358.0,
        592.0,
        14.0,
        10.0,
        sw::ink_dim(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.merged"),
        64.0,
        378.0,
        592.0,
        13.0,
        10.0,
        sw::ink(),
    );

    // Build footer and key hints.
    line(
        &mut face,
        &slot_i18n::t_args(
            lang,
            "device.quick.about.build_short",
            &[
                ("serial", &f.serial),
                ("dirty", if f.dirty { "*" } else { "" }),
                ("date", &f.date),
            ],
        ),
        48.0,
        404.0,
        500.0,
        13.0,
        9.0,
        sw::ink_dim(),
    );
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.back"),
        14.0,
        10.0,
        540.0,
        428.0,
        132.0,
        24.0,
        sw::ink_dim(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.hint"),
        170.0,
        430.0,
        380.0,
        11.0,
        9.0,
        sw::ink_dim(),
    );

    face
}

pub fn about_thanks_face(f: &AboutFields, lang: Language) -> UndoFace {
    let mut face = UndoFace {
        rgba: vec![0u8; (ABOUT_W * ABOUT_H * 4) as usize],
        w: ABOUT_W,
        h: ABOUT_H,
    };
    fill(&mut face, 0.0, 0.0, ABOUT_W as f32, ABOUT_H as f32, sw::bg());
    fill(&mut face, 0.0, 0.0, ABOUT_W as f32, 88.0, sw::bg_top());
    sw::fill_round(&mut face, 48.0, 22.0, 5.0, 30.0, 2.0, accent());
    line(&mut face, slot_i18n::t(lang, "device.quick.about.thanks_list"), 64.0, 16.0, 600.0, 30.0, 20.0, sw::ink());
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.thanks_note"),
        64.0,
        52.0,
        600.0,
        12.0,
        9.0,
        sw::ink_dim(),
    );

    draw_card(&mut face, 40.0, 72.0, 640, 76);
    line(&mut face, slot_i18n::t(lang, "device.quick.about.label_author"), 64.0, 86.0, 80.0, 14.0, 11.0, sw::ink_dim());
    line(&mut face, AUTHOR, 150.0, 80.0, 506.0, 22.0, 16.0, sw::ink());

    draw_card(&mut face, 40.0, 160.0, 640, 184);
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.label_thanks"),
        64.0,
        172.0,
        592.0,
        22.0,
        16.0,
        sw::ink(),
    );
    line(
        &mut face,
        CONTRIBUTOR,
        64.0,
        208.0,
        592.0,
        20.0,
        14.0,
        sw::ink(),
    );
    line(
        &mut face,
        CRAZY_SIX,
        64.0,
        244.0,
        592.0,
        20.0,
        14.0,
        sw::ink(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.thanks_footer"),
        64.0,
        292.0,
        592.0,
        13.0,
        10.0,
        sw::ink_dim(),
    );

    line(
        &mut face,
        &slot_i18n::t_args(
            lang,
            "device.quick.about.build_short",
            &[
                ("serial", &f.serial),
                ("dirty", if f.dirty { "*" } else { "" }),
                ("date", &f.date),
            ],
        ),
        48.0,
        400.0,
        500.0,
        13.0,
        9.0,
        sw::ink_dim(),
    );
    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.back"),
        14.0,
        10.0,
        540.0,
        428.0,
        132.0,
        24.0,
        sw::ink_dim(),
    );
    line(
        &mut face,
        slot_i18n::t(lang, "device.quick.about.thanks_brand"),
        170.0,
        430.0,
        380.0,
        11.0,
        9.0,
        sw::ink_dim(),
    );

    face
}

pub fn draw_about(face: Option<TexId>, out: &mut Vec<Draw>) {
    let Some(tex) = face else {
        return;
    };
    out.push(Draw::Tex {
        x: 0.0,
        y: 0.0,
        w: ABOUT_W as f32,
        h: ABOUT_H as f32,
        tex,
        alpha: 1.0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> AboutFields<'static> {
        AboutFields {
            serial: "ABCDEF0",
            date: "2026-10-22 14:00",
            dirty: false,
            device: "Anbernic RG34XX SP",
        }
    }

    fn rgb(face: &UndoFace, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * face.w + x) * 4) as usize;
        [face.rgba[at], face.rgba[at + 1], face.rgba[at + 2]]
    }

    #[test]
    fn the_about_page_is_full_screen_and_has_the_switch_header() {
        let face = about_face(&fields(), Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert_eq!(rgb(&face, 50, 30), [0xe6, 0x00, 0x12]);
        assert_eq!(rgb(&face, 10, 10), [0x24, 0x24, 0x29]);
    }

    #[test]
    fn the_required_credits_and_original_project_are_present() {
        let all = about_lines(&fields(), Language::ZhCn).join("\n");
        for expected in [
            AUTHOR,
            ORIGINAL_AUTHOR,
            ORIGINAL_VERSION,
            ORIGINAL_LICENSE,
            ORIGINAL_URL,
            PROJECT_VERSION,
            slot_i18n::t(Language::ZhCn, "device.quick.about.test_warning"),
            slot_i18n::t(Language::ZhCn, "device.quick.about.free_use"),
            "Anbernic RG34XX SP",
            slot_i18n::t(Language::ZhCn, "device.quick.about.merged_badge"),
            slot_i18n::t(Language::ZhCn, "device.quick.about.fork_badge"),
        ] {
            assert!(all.contains(expected), "missing {expected:?} in {all}");
        }
    }

    #[test]
    fn the_current_version_and_test_warning_are_shown() {
        let f = fields();
        let all = about_lines(&f, Language::ZhCn).join("\n");
        assert!(
            all.contains(&format!("当前项目版本：v{PROJECT_VERSION}")),
            "missing current project version in {all}"
        );
        assert!(
            all.contains(TEST_WARNING),
            "missing test-build warning in {all}"
        );
        assert!(
            all.contains(&format!("v{ORIGINAL_VERSION}")),
            "the upstream baseline version must stay visible"
        );
        assert!(
            all.contains("本机名称：Anbernic RG34XX SP"),
            "the actual device name is missing from {all}"
        );
    }

    #[test]
    fn the_free_use_notice_is_shown() {
        let all = about_lines(&fields(), Language::ZhCn).join("\n");
        assert!(all.contains(FREE_USE), "missing free-use notice in {all}");
    }

    #[test]
    fn the_thanks_page_lists_the_author_and_contributors() {
        let all = thanks_lines(Language::ZhCn).join("\n");
        for expected in [AUTHOR, CONTRIBUTOR, CRAZY_SIX] {
            assert!(all.contains(expected), "missing {expected:?} in {all}");
        }
        assert!(
            !about_lines(&fields(), Language::ZhCn).join("\n").contains(CONTRIBUTOR),
            "the main About page should only keep the author"
        );
        assert!(
            !all.contains("代码贡献") && !all.contains("特别感谢："),
            "the thanks list should not separate contributor roles"
        );
        let face = about_thanks_face(&fields(), Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
        assert_eq!(rgb(&face, 50, 30), [0xe6, 0x00, 0x12]);
    }

    #[test]
    fn no_barcode_or_old_sticker_content_remains() {
        let all = about_lines(&fields(), Language::ZhCn).join("\n");
        for forbidden in ["AGS-102", "CODE39", "S/LOT-USA", "伊萨卡制造"] {
            assert!(!all.contains(forbidden), "stale sticker text {forbidden:?}");
        }
    }

    #[test]
    fn a_missing_build_date_does_not_panic() {
        let face = about_face(&AboutFields {
            serial: "",
            date: "",
            dirty: true,
            device: "未知设备",
        }, Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
    }
}
