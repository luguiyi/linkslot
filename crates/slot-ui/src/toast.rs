use slot_gfx::{OUT_H, OUT_W};
use slot_i18n::Language;
use slot_store::GbPalette;

use crate::hud::{HUD_INK, PLATE_H};
use crate::quick_menu::palette_name;
use crate::icon::{haloed, HALO_PX};
use crate::text;
use crate::CartFace;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Toast {
    StateSaved,
    StateLoaded,
    NeedsGpsp,
    NoLink,
    LinkEnded,
    ColourOn,
    ColourOff,
    ShaderOff,
    ShaderLcd3x,
    ShaderGrid,
    ShaderDot,
    ShaderSimpletex,
    ShaderFourPass,
    CardUnwritable,
    FavoriteOn,
    FavoriteOff,
    RepairClean,
    RepairDone,
    RepairFailed,
    LogDebug,
    LogDebugFailed,
    Palette(GbPalette),
    Duplicate,
    UpgradeNoPackage,
    UpgradeStarted,
    UpgradeFailed,
    /// A peer joined the game this device started.
    LinkJoined,
    /// This device started a game and the peer has not joined yet.
    LinkWaiting,
    /// Every shortcut went back to its factory chord.
    ShortcutsReset,
    /// The joiner's clock was set from the host's on connect.
    ClockSynced,
    /// The joiner accepted an invite and is switching carts.
    Following,
    /// The host restarted the shared cart; the joiner jumped back with it.
    HostRestarted,
    /// Cartridge transfer mode messages.
    CartTransferWaiting,
    CartTransferReady,
    CartTransferSending,
    CartTransferReceiving,
    CartTransferDone,
    CartTransferFailed,
    CartTransferDuplicate,
    CartTransferAlreadyOwned,
    CartDeleted,
    CartDeleteFailed,
    /// The clock was corrected from an SNTP server after Wi-Fi connected.
    NetworkTimeSynced,
    /// The selected game has no readable TXT guide.
    NoGuide,
    /// A theme was saved; the palette is rebuilt on the next boot.
    ThemeSaved,
}

impl Toast {
    const FIXED: [Toast; 44] = [
        Toast::StateSaved,
        Toast::StateLoaded,
        Toast::NeedsGpsp,
        Toast::NoLink,
        Toast::LinkEnded,
        Toast::ColourOn,
        Toast::ColourOff,
        Toast::ShaderOff,
        Toast::ShaderLcd3x,
        Toast::ShaderGrid,
        Toast::ShaderDot,
        Toast::ShaderSimpletex,
        Toast::ShaderFourPass,
        Toast::CardUnwritable,
        Toast::FavoriteOn,
        Toast::FavoriteOff,
        Toast::RepairClean,
        Toast::RepairDone,
        Toast::RepairFailed,
        Toast::LogDebug,
        Toast::LogDebugFailed,
        Toast::Duplicate,
        Toast::UpgradeNoPackage,
        Toast::UpgradeStarted,
        Toast::UpgradeFailed,
        Toast::LinkJoined,
        Toast::LinkWaiting,
        Toast::ShortcutsReset,
        Toast::ClockSynced,
        Toast::Following,
        Toast::HostRestarted,
        Toast::CartTransferWaiting,
        Toast::CartTransferReady,
        Toast::CartTransferSending,
        Toast::CartTransferReceiving,
        Toast::CartTransferDone,
        Toast::CartTransferFailed,
        Toast::CartTransferDuplicate,
        Toast::CartTransferAlreadyOwned,
        Toast::CartDeleted,
        Toast::CartDeleteFailed,
        Toast::NetworkTimeSynced,
        Toast::NoGuide,
        Toast::ThemeSaved,
    ];

    pub fn all() -> Vec<Toast> {
        Self::FIXED
            .into_iter()
            .chain(GbPalette::all().map(Toast::Palette))
            .collect()
    }

    pub fn index(self) -> usize {
        match self {
            Toast::Palette(p) => Self::FIXED.len() + p.index(),
            fixed => Self::FIXED.iter().position(|t| *t == fixed).unwrap_or(0),
        }
    }

    pub fn alarm(self) -> bool {
        self == Toast::CardUnwritable
    }

    /// True when this toast draws a second line under its title.
    ///
    /// Separate from [`Toast::detail`] because the box has to be measured
    /// before a language is known: only one toast has a detail, and that does
    /// not depend on the language.
    pub fn has_detail(self) -> bool {
        matches!(self, Toast::CardUnwritable)
    }

    pub fn detail(self, lang: Language) -> Option<&'static str> {
        match self {
            Toast::CardUnwritable => Some(slot_i18n::t(
                lang,
                "device.toast.card_unwritable.detail",
            )),
            _ => None,
        }
    }

    pub fn text(self, lang: Language) -> &'static str {
        match self {
            Toast::Palette(p) => palette_name(p, lang),
            Toast::StateSaved => slot_i18n::t(lang, "device.toast.state_saved"),
            Toast::StateLoaded => slot_i18n::t(lang, "device.toast.state_loaded"),
            Toast::NeedsGpsp => slot_i18n::t(lang, "device.toast.needs_gpsp"),
            Toast::NoLink => slot_i18n::t(lang, "device.toast.no_link"),
            Toast::LinkEnded => slot_i18n::t(lang, "device.toast.link_ended"),
            Toast::ColourOn => slot_i18n::t(lang, "device.toast.colour_on"),
            Toast::ColourOff => slot_i18n::t(lang, "device.toast.colour_off"),
            Toast::ShaderOff => slot_i18n::t(lang, "device.toast.shader_off"),
            Toast::ShaderLcd3x => slot_i18n::t(lang, "device.toast.shader_lcd3x"),
            Toast::ShaderGrid => slot_i18n::t(lang, "device.toast.shader_grid"),
            Toast::ShaderDot => slot_i18n::t(lang, "device.toast.shader_dot"),
            Toast::ShaderSimpletex => slot_i18n::t(lang, "device.toast.shader_simpletex"),
            Toast::ShaderFourPass => slot_i18n::t(lang, "device.toast.shader_four_pass"),
            Toast::CardUnwritable => slot_i18n::t(lang, "device.toast.card_unwritable"),
            Toast::FavoriteOn => slot_i18n::t(lang, "device.toast.favorite_on"),
            Toast::FavoriteOff => slot_i18n::t(lang, "device.toast.favorite_off"),
            Toast::RepairClean => slot_i18n::t(lang, "device.toast.repair_clean"),
            Toast::RepairDone => slot_i18n::t(lang, "device.toast.repair_done"),
            Toast::RepairFailed => slot_i18n::t(lang, "device.toast.repair_failed"),
            Toast::LogDebug => slot_i18n::t(lang, "device.toast.log_debug"),
            Toast::LogDebugFailed => slot_i18n::t(lang, "device.toast.log_debug_failed"),
            Toast::Duplicate => slot_i18n::t(lang, "device.toast.duplicate"),
            Toast::UpgradeNoPackage => slot_i18n::t(lang, "device.toast.upgrade_no_package"),
            Toast::UpgradeStarted => slot_i18n::t(lang, "device.toast.upgrade_started"),
            Toast::UpgradeFailed => slot_i18n::t(lang, "device.toast.upgrade_failed"),
            Toast::LinkJoined => slot_i18n::t(lang, "device.toast.link_joined"),
            Toast::LinkWaiting => slot_i18n::t(lang, "device.toast.link_waiting"),
            Toast::ShortcutsReset => slot_i18n::t(lang, "device.toast.shortcuts_reset"),
            Toast::ClockSynced => slot_i18n::t(lang, "device.toast.clock_synced"),
            Toast::Following => slot_i18n::t(lang, "device.toast.following"),
            Toast::HostRestarted => slot_i18n::t(lang, "device.toast.host_restarted"),
            Toast::CartTransferWaiting => slot_i18n::t(lang, "device.toast.cart_transfer_waiting"),
            Toast::CartTransferReady => slot_i18n::t(lang, "device.toast.cart_transfer_ready"),
            Toast::CartTransferSending => slot_i18n::t(lang, "device.toast.cart_transfer_sending"),
            Toast::CartTransferReceiving => slot_i18n::t(lang, "device.toast.cart_transfer_receiving"),
            Toast::CartTransferDone => slot_i18n::t(lang, "device.toast.cart_transfer_done"),
            Toast::CartTransferFailed => slot_i18n::t(lang, "device.toast.cart_transfer_failed"),
            Toast::CartTransferDuplicate => slot_i18n::t(lang, "device.toast.cart_transfer_duplicate"),
            Toast::CartTransferAlreadyOwned => slot_i18n::t(lang, "device.toast.cart_transfer_already_owned"),
            Toast::CartDeleted => slot_i18n::t(lang, "device.toast.cart_deleted"),
            Toast::CartDeleteFailed => slot_i18n::t(lang, "device.toast.cart_delete_failed"),
            Toast::NetworkTimeSynced => slot_i18n::t(lang, "device.toast.network_time_synced"),
            Toast::NoGuide => slot_i18n::t(lang, "device.toast.no_guide"),
            Toast::ThemeSaved => slot_i18n::t(lang, "device.toast.theme_saved"),
        }
    }
}

const TOAST_W: u32 = 240;
const TOAST_H: u32 = 22;
const TOAST_PX: f32 = 16.0;
const TOAST_MIN_PX: f32 = 12.0;

const ALARM_W: u32 = 560;
const ALARM_H: u32 = 44;
const ALARM_PX: f32 = 32.0;
const DETAIL_H: u32 = 30;
const DETAIL_PX: f32 = 20.0;

struct Setting {
    w: u32,
    h: u32,
    px: f32,
    min_px: f32,
    middle: f32,
}

fn setting(toast: Toast) -> Setting {
    match toast.alarm() {
        true => Setting {
            w: ALARM_W,
            h: ALARM_H,
            px: ALARM_PX,
            min_px: ALARM_PX,
            middle: OUT_H as f32 / 2.0,
        },
        false => Setting {
            w: TOAST_W,
            h: TOAST_H,
            px: TOAST_PX,
            min_px: TOAST_MIN_PX,
            middle: PLATE_H / 2.0,
        },
    }
}

pub fn toast_rect(toast: Toast) -> (f32, f32, f32, f32) {
    let (w, h) = toast_box(toast);
    let (w, h) = (w as f32, h as f32);
    let middle = setting(toast).middle;
    ((OUT_W as f32 - w) / 2.0, middle - h / 2.0, w, h)
}

pub fn toast_box(toast: Toast) -> (u32, u32) {
    let s = setting(toast);
    (s.w + 2 * HALO_PX, s.h + detail_h(toast) + 2 * HALO_PX)
}

fn detail_h(toast: Toast) -> u32 {
    if toast.has_detail() {
        DETAIL_H
    } else {
        0
    }
}

pub fn toast_face(toast: Toast, lang: Language) -> CartFace {
    let Some(font) = text::label_font() else {
        return CartFace {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    };
    let s = setting(toast);
    let layout = text::fit(font, toast.text(lang), s.w as f32, 1, s.px, s.min_px);
    let mut cov = text::coverage(s.w, s.h, &layout);
    if let Some(detail) = toast.detail(lang) {
        let layout = text::fit(font, detail, s.w as f32, 1, DETAIL_PX, DETAIL_PX);
        cov.extend(text::coverage(s.w, DETAIL_H, &layout));
    }
    haloed(&cov, s.w, s.h + detail_h(toast), HUD_INK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink_width(toast: Toast) -> u32 {
        let font = text::label_font().expect("label font");
        let s = setting(toast);
        let layout = text::fit(font, toast.text(Language::ZhCn), s.w as f32, 1, s.px, s.min_px);
        let cov = text::coverage(s.w, s.h, &layout);
        let cols: Vec<u32> = (0..s.w)
            .filter(|x| (0..s.h).any(|y| cov[(y * s.w + x) as usize] > 0))
            .collect();
        match (cols.first(), cols.last()) {
            (Some(a), Some(b)) => b - a + 1,
            _ => 0,
        }
    }

    fn ink_rows(face: &CartFace) -> (u32, u32) {
        let mut first = None;
        let mut last = 0;
        for y in 0..face.h {
            let inked = (0..face.w).any(|x| face.rgba[((y * face.w + x) * 4 + 3) as usize] > 0);
            if inked {
                first.get_or_insert(y);
                last = y;
            }
        }
        (first.unwrap_or(0), last)
    }

    fn ink_height_at(toast: Toast, px: f32) -> u32 {
        let font = text::label_font().expect("label font");
        let layout = text::fit(font, toast.text(Language::ZhCn), TOAST_W as f32, 1, px, px);
        let cov = text::coverage(TOAST_W, TOAST_H, &layout);
        let rows: Vec<u32> = (0..TOAST_H)
            .filter(|y| (0..TOAST_W).any(|x| cov[(y * TOAST_W + x) as usize] > 0))
            .collect();
        match (rows.first(), rows.last()) {
            (Some(a), Some(b)) => b - a + 1,
            _ => 0,
        }
    }

    #[test]
    fn the_ended_line_is_rastered_the_size_the_others_are() {
        let ended = toast_face(Toast::LinkEnded, Language::ZhCn);
        let saved = toast_face(Toast::StateSaved, Language::ZhCn);
        assert_eq!(
            (ended.w, ended.h),
            (saved.w, saved.h),
            "one box holds every banner"
        );
        let (top, bottom) = ink_rows(&ended);
        assert!(bottom > top, "the line rastered to nothing at all");

        let shipped = ink_height_at(Toast::LinkEnded, TOAST_PX);
        let shrunk = ink_height_at(Toast::LinkEnded, TOAST_MIN_PX);
        assert!(
            shipped > shrunk,
            "the shipped line is no taller than the {TOAST_MIN_PX} px fallback: {shipped} against {shrunk}"
        );
        let reference = ink_height_at(Toast::StateSaved, TOAST_PX);
        assert!(
            shipped.abs_diff(reference) <= 1,
            "LINK ENDED is {shipped} px of ink where STATE SAVED is {reference}"
        );
    }

    #[test]
    fn every_toast_is_set_at_full_size() {
        let font = text::label_font().expect("label font");
        for t in Toast::all() {
            let s = setting(t);
            let layout = text::fit(font, t.text(Language::ZhCn), s.w as f32, 1, s.px, s.min_px);
            assert_eq!(
                layout.px,
                s.px,
                "{:?} ({:?}) was shrunk to {} px to fit {}",
                t,
                t.text(Language::ZhCn),
                layout.px,
                s.w
            );
            assert_eq!(layout.lines.len(), 1, "{t:?} wrapped onto a second line");
            let w = ink_width(t);
            assert!(w <= s.w, "{:?} is {w} px wide in a {} px box", t, s.w);
        }
    }
}
