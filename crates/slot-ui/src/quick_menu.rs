use slot_i18n::Language;
use slot_store::{Device, GbPalette};

use slot_store::{Platform, ResumeKind};

use crate::draw::{Draw, TexId, OUT_H, OUT_W};
use crate::plate::{arrows_hint_face, hint_face, UndoFace};
use crate::power_menu::{MENU_H, MENU_PAD, MENU_PX};
use crate::switch_style;
use crate::text;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum QuickRow {
    FastForward,
    FastForwardSound,
    Screen,
    Rumble,
    DateTime,
    Mods,
    About,
    SystemUpgrade,
    GbaShader,
    GbShader,
    ColourCorrection,
    Brightness,
    BlueLight,
    Game,
    SaveSettings,
    LoadGba,
    SaveGba,
    LoadGb,
    SaveGb,
    LoadGbc,
    SaveGbc,
    Recents,
    Turbo,
    Rewind,
    GbPalettes,
    ButtonLayout,
    RomTitle,
    FallbackCover,
    LidTimeout,
    ButtonSettings,
    SystemSettings,
    Shortcuts,
    Network,
    Wifi,
    Web,
    SyncStorage,
    ShelfKeys,
    ShelfPrev,
    ShelfNext,
    ShelfFav,
    ShortcutMap,
    ShortcutSaveState,
    ShortcutLoadState,
    ShortcutBrightnessUp,
    ShortcutBrightnessDown,
    ShortcutBlueLightDown,
    ShortcutBlueLightUp,
    ShortcutColorCorrection,
    ShortcutPaletteBack,
    ShortcutPaletteNext,
    ShortcutShaderNext,
    ShortcutShaderBack,
    ShortcutFastForward,
    ShortcutRewind,
    ControlUp,
    ControlDown,
    ControlLeft,
    ControlRight,
    ControlA,
    ControlB,
    ControlSelect,
    ControlStart,
    ControlL,
    ControlR,
    ControlTurboA,
    ControlTurboB,
    ControlPreset,
    SyncContent,
    /// 网络设置 → 卡带传输模式.
    CartTransfer,
    /// The shortcut page's last row: put every shortcut back to its default.
    ShortcutReset,
    /// 画面 → 屏幕与亮度 (a second-level page).
    DisplayPanel,
    /// 画面 → 画面效果 (a second-level page).
    DisplayEffects,
    /// 卡带架: the shelf's own settings (a top-level page).
    Shelf,
    /// 卡带架 → 外观与显示 (a second-level page).
    ShelfLook,
    /// 存档设置 top: load resume.state by default on insert.
    DefaultLoad,
    /// 存档设置 top: save resume.state by default on exit.
    DefaultSave,
    /// 画面效果 -> FC: draw the screen overlay from Overlay/FC/*.png.
    FcMask,
    /// 画面效果 -> GB: draw the screen overlay from Overlay/GB/*.png.
    GbMask,
    /// 画面效果 -> GBC: draw the screen overlay from Overlay/GBC/*.png.
    GbcMask,
    /// 画面效果 -> SFC: draw the screen overlay from Overlay/SFC/*.png.
    SfcMask,
    /// 画面 -> 遮罩: the page that holds the screen overlays.
    Masks,
    /// 外观与显示 → 主题: opens the theme list.
    Theme,
    /// 主题 → 默认深色.
    ThemeDefault,
    /// 主题 → 网络青.
    ThemeWeb,
    /// 主题 → 复古暖色.
    ThemeWarm,
    /// 系统 → 语言: the UI language, left/right to switch.
    ///
    /// Appended after every row that already existed, so no stored value or
    /// test index moves.  It sits before `SplitLink` because `SplitLink` stays
    /// the one row outside `QuickRow::ALL`, and `index()` is the enum's
    /// discriminant: keeping `SplitLink` last keeps `Language` inside the value
    /// table.
    Language,
    /// The link session page: who is on the other end, and start/stop.  This
    /// row is intentionally absent from QuickRow::ALL.
    SplitLink,
}

impl QuickRow {
    pub const ALL: [QuickRow; 86] = [
        QuickRow::FastForward,
        QuickRow::FastForwardSound,
        QuickRow::Screen,
        QuickRow::Rumble,
        QuickRow::DateTime,
        QuickRow::Mods,
        QuickRow::About,
        QuickRow::SystemUpgrade,
        QuickRow::GbaShader,
        QuickRow::GbShader,
        QuickRow::ColourCorrection,
        QuickRow::Brightness,
        QuickRow::BlueLight,
        QuickRow::Game,
        QuickRow::SaveSettings,
        QuickRow::LoadGba,
        QuickRow::SaveGba,
        QuickRow::LoadGb,
        QuickRow::SaveGb,
        QuickRow::LoadGbc,
        QuickRow::SaveGbc,
        QuickRow::Recents,
        QuickRow::Turbo,
        QuickRow::Rewind,
        QuickRow::GbPalettes,
        QuickRow::ButtonLayout,
        QuickRow::RomTitle,
        QuickRow::FallbackCover,
        QuickRow::LidTimeout,
        QuickRow::ButtonSettings,
        QuickRow::SystemSettings,
        QuickRow::Shortcuts,
        QuickRow::Network,
        QuickRow::Wifi,
        QuickRow::Web,
        QuickRow::SyncStorage,
        QuickRow::ShelfKeys,
        QuickRow::ShelfPrev,
        QuickRow::ShelfNext,
        QuickRow::ShelfFav,
        QuickRow::ShortcutMap,
        QuickRow::ShortcutSaveState,
        QuickRow::ShortcutLoadState,
        QuickRow::ShortcutBrightnessUp,
        QuickRow::ShortcutBrightnessDown,
        QuickRow::ShortcutBlueLightDown,
        QuickRow::ShortcutBlueLightUp,
        QuickRow::ShortcutColorCorrection,
        QuickRow::ShortcutPaletteBack,
        QuickRow::ShortcutPaletteNext,
        QuickRow::ShortcutShaderNext,
        QuickRow::ShortcutShaderBack,
        QuickRow::ShortcutFastForward,
        QuickRow::ShortcutRewind,
        QuickRow::ControlUp,
        QuickRow::ControlDown,
        QuickRow::ControlLeft,
        QuickRow::ControlRight,
        QuickRow::ControlA,
        QuickRow::ControlB,
        QuickRow::ControlSelect,
        QuickRow::ControlStart,
        QuickRow::ControlL,
        QuickRow::ControlR,
        QuickRow::ControlTurboA,
        QuickRow::ControlTurboB,
        QuickRow::ControlPreset,
        QuickRow::SyncContent,
        QuickRow::CartTransfer,
        QuickRow::ShortcutReset,
        QuickRow::DisplayPanel,
        QuickRow::DisplayEffects,
        QuickRow::Shelf,
        QuickRow::ShelfLook,
        QuickRow::DefaultLoad,
        QuickRow::DefaultSave,
        QuickRow::FcMask,
        QuickRow::GbMask,
        QuickRow::GbcMask,
        QuickRow::SfcMask,
        QuickRow::Masks,
        QuickRow::Theme,
        QuickRow::ThemeDefault,
        QuickRow::ThemeWeb,
        QuickRow::ThemeWarm,
        QuickRow::Language,
    ];

    pub const MAIN: [QuickRow; 6] = [
        QuickRow::Screen,
        QuickRow::Shelf,
        QuickRow::Game,
        QuickRow::ButtonSettings,
        QuickRow::Network,
        QuickRow::SystemSettings,
    ];

    pub const DISPLAY: [QuickRow; 4] = [
        QuickRow::DisplayPanel,
        QuickRow::DisplayEffects,
        QuickRow::Masks,
        // 主题 lives under 画面: it is a look, like the masks and shaders.
        QuickRow::Theme,
    ];

    pub const DISPLAY_PANEL: [QuickRow; 2] = [QuickRow::Brightness, QuickRow::BlueLight];

    pub const DISPLAY_EFFECTS: [QuickRow; 4] = [
        // Grouped by machine: GBA first, then the Game Boy family, then the
        // one row that applies everywhere.
        QuickRow::GbaShader,
        QuickRow::GbShader,
        QuickRow::GbPalettes,
        QuickRow::ColourCorrection,
    ];

    pub const MASKS: [QuickRow; 4] = [
        QuickRow::FcMask,
        QuickRow::GbMask,
        QuickRow::GbcMask,
        QuickRow::SfcMask,
    ];

    pub const THEMES: [QuickRow; 3] = [
        QuickRow::ThemeDefault,
        QuickRow::ThemeWeb,
        QuickRow::ThemeWarm,
    ];

    pub const SHELF: [QuickRow; 2] = [QuickRow::ShelfLook, QuickRow::ShelfKeys];

    pub const SHELF_LOOK: [QuickRow; 3] = [
        QuickRow::RomTitle,
        QuickRow::FallbackCover,
        QuickRow::Recents,
    ];

    pub const BUTTON_SETTINGS: [QuickRow; 4] = [
        QuickRow::Shortcuts,
        QuickRow::ShortcutMap,
        QuickRow::ButtonLayout,
        // Turbo is a pad mapping switch, so it lives with the pad remap.
        QuickRow::Turbo,
    ];

    /// Order matters: 系统升级 sits right above 关于 at the bottom.
    /// 语言 comes second so it is one press below the date, the row people
    /// reach for first when the screen is in a language they cannot read.
    pub const SYSTEM_SETTINGS: [QuickRow; 6] = [
        QuickRow::DateTime,
        QuickRow::Language,
        QuickRow::LidTimeout,
        QuickRow::Mods,
        QuickRow::SystemUpgrade,
        QuickRow::About,
    ];

    pub const NETWORK: [QuickRow; 5] = [
        QuickRow::Wifi,
        QuickRow::Web,
        QuickRow::SyncStorage,
        QuickRow::SyncContent,
        QuickRow::CartTransfer,
    ];

    pub const SHELF_KEYS: [QuickRow; 3] = [
        QuickRow::ShelfPrev,
        QuickRow::ShelfNext,
        QuickRow::ShelfFav,
    ];

    pub const SHORTCUTS: [QuickRow; 14] = [
        QuickRow::ShortcutSaveState,
        QuickRow::ShortcutLoadState,
        QuickRow::ShortcutBrightnessUp,
        QuickRow::ShortcutBrightnessDown,
        QuickRow::ShortcutBlueLightDown,
        QuickRow::ShortcutBlueLightUp,
        QuickRow::ShortcutColorCorrection,
        QuickRow::ShortcutPaletteBack,
        QuickRow::ShortcutPaletteNext,
        QuickRow::ShortcutShaderNext,
        QuickRow::ShortcutShaderBack,
        QuickRow::ShortcutFastForward,
        QuickRow::ShortcutRewind,
        QuickRow::ShortcutReset,
    ];

    pub const CONTROLS: [QuickRow; 13] = [
        QuickRow::ControlUp,
        QuickRow::ControlDown,
        QuickRow::ControlLeft,
        QuickRow::ControlRight,
        QuickRow::ControlA,
        QuickRow::ControlB,
        QuickRow::ControlSelect,
        QuickRow::ControlStart,
        QuickRow::ControlL,
        QuickRow::ControlR,
        QuickRow::ControlTurboA,
        QuickRow::ControlTurboB,
        QuickRow::ControlPreset,
    ];

    pub const GAME: [QuickRow; 5] = [
        QuickRow::FastForward,
        QuickRow::FastForwardSound,
        QuickRow::Rewind,
        QuickRow::Rumble,
        QuickRow::SaveSettings,
    ];

    pub const SAVES: [QuickRow; 8] = [
        QuickRow::DefaultLoad,
        QuickRow::DefaultSave,
        QuickRow::LoadGba,
        QuickRow::SaveGba,
        QuickRow::LoadGb,
        QuickRow::SaveGb,
        QuickRow::LoadGbc,
        QuickRow::SaveGbc,
    ];

    /// Translation keys for the page titles, indexed by [`QuickRow::page_index`].
    ///
    /// Stored as keys rather than titles: a `&'static [&'static str]` cannot hold
    /// two languages, and one constant per language would duplicate the table the
    /// parity test exists to keep single.
    const PAGE_KEYS: [&'static str; 16] = [
    "device.quick.title.settings",
    "device.quick.title.display",
    "device.quick.title.screen_light",
    "device.quick.title.picture_effects",
    "device.quick.title.masks",
    "device.quick.title.shelf",
    "device.quick.title.shelf_look",
    "device.quick.title.shelf_keys",
    "device.quick.title.game",
    "device.quick.title.controls",
    "device.quick.title.shortcut_map",
    "device.quick.title.button_layout",
    "device.quick.title.network",
    "device.quick.title.system",
    "device.quick.title.save_settings",
    "device.quick.title.theme",
    ];
    pub const QUICK_PAGES: usize = Self::PAGE_KEYS.len();

    /// The page titles in `lang`, indexed like [`QuickRow::page_index`].
    pub fn pages(lang: Language) -> [&'static str; Self::QUICK_PAGES] {
        Self::PAGE_KEYS.map(|key| slot_i18n::t(lang, key))
    }

    /// The title of page `index` in `lang`.
    pub fn page_title_at(index: usize, lang: Language) -> &'static str {
        let index = index.min(Self::QUICK_PAGES - 1);
        slot_i18n::t(lang, Self::PAGE_KEYS[index])
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn page(self) -> &'static [QuickRow] {
        match self {
            QuickRow::DisplayPanel
            | QuickRow::DisplayEffects
            | QuickRow::Masks
            | QuickRow::Theme => &QuickRow::DISPLAY,
            QuickRow::Brightness | QuickRow::BlueLight => &QuickRow::DISPLAY_PANEL,
            QuickRow::GbaShader
            | QuickRow::GbShader
            | QuickRow::ColourCorrection
            | QuickRow::GbPalettes => &QuickRow::DISPLAY_EFFECTS,
            QuickRow::FcMask | QuickRow::GbMask | QuickRow::GbcMask | QuickRow::SfcMask => {
                &QuickRow::MASKS
            }
            QuickRow::ShelfLook | QuickRow::ShelfKeys => &QuickRow::SHELF,
            QuickRow::ThemeDefault | QuickRow::ThemeWeb | QuickRow::ThemeWarm => &QuickRow::THEMES,
            QuickRow::RomTitle | QuickRow::FallbackCover | QuickRow::Recents => {
                &QuickRow::SHELF_LOOK
            }
            QuickRow::FastForward
            | QuickRow::FastForwardSound
            | QuickRow::Rewind
            | QuickRow::Rumble
            | QuickRow::SaveSettings => &QuickRow::GAME,
            QuickRow::DefaultLoad
            | QuickRow::DefaultSave
            | QuickRow::LoadGba
            | QuickRow::SaveGba
            | QuickRow::LoadGb
            | QuickRow::SaveGb
            | QuickRow::LoadGbc
            | QuickRow::SaveGbc => &QuickRow::SAVES,
            QuickRow::Wifi
            | QuickRow::Web
            | QuickRow::SyncStorage
            | QuickRow::SyncContent
            | QuickRow::CartTransfer
            | QuickRow::SplitLink => &QuickRow::NETWORK,
            // 按键: shortcuts, the mapping page and the pad remap live together.
            QuickRow::Shortcuts
            | QuickRow::ShortcutMap
            | QuickRow::ButtonLayout
            | QuickRow::Turbo => &QuickRow::BUTTON_SETTINGS,
            QuickRow::DateTime
            | QuickRow::Language
            | QuickRow::LidTimeout
            | QuickRow::Mods
            | QuickRow::SystemUpgrade
            | QuickRow::About => &QuickRow::SYSTEM_SETTINGS,
            QuickRow::ShelfPrev | QuickRow::ShelfNext | QuickRow::ShelfFav => {
                &QuickRow::SHELF_KEYS
            }
            QuickRow::ShortcutSaveState
            | QuickRow::ShortcutLoadState
            | QuickRow::ShortcutBrightnessUp
            | QuickRow::ShortcutBrightnessDown
            | QuickRow::ShortcutBlueLightDown
            | QuickRow::ShortcutBlueLightUp
            | QuickRow::ShortcutColorCorrection
            | QuickRow::ShortcutPaletteBack
            | QuickRow::ShortcutPaletteNext
            | QuickRow::ShortcutShaderNext
            | QuickRow::ShortcutShaderBack
            | QuickRow::ShortcutFastForward
            | QuickRow::ShortcutRewind
            | QuickRow::ShortcutReset => &QuickRow::SHORTCUTS,
            QuickRow::ControlUp
            | QuickRow::ControlDown
            | QuickRow::ControlLeft
            | QuickRow::ControlRight
            | QuickRow::ControlA
            | QuickRow::ControlB
            | QuickRow::ControlSelect
            | QuickRow::ControlStart
            | QuickRow::ControlL
            | QuickRow::ControlR
            | QuickRow::ControlTurboA
            | QuickRow::ControlTurboB
            | QuickRow::ControlPreset => &QuickRow::CONTROLS,
            _ => &QuickRow::MAIN,
        }
    }

    pub fn page_title(self, lang: Language) -> &'static str {
        Self::page_title_at(self.page_index(), lang)
    }
    pub fn page_index(self) -> usize {
        match self.page()[0] {
            QuickRow::DisplayPanel => 1,
            QuickRow::Brightness => 2,
            QuickRow::GbaShader => 3,
            QuickRow::FcMask => 4,
            QuickRow::ShelfLook => 5,
            QuickRow::ThemeDefault => 15,
            QuickRow::RomTitle => 6,
            QuickRow::ShelfPrev => 7,
            QuickRow::FastForward => 8,
            QuickRow::Shortcuts => 9,
            QuickRow::ShortcutSaveState => 10,
            QuickRow::ControlUp => 11,
            QuickRow::Wifi => 12,
            QuickRow::DateTime => 13,
            QuickRow::DefaultLoad => 14,
            _ => 0,
        }
    }

    /// Short label for the fallback icon tile when the symbol font has no
    /// glyph for `glyph()`.
    ///
    /// Takes the language because a single Chinese character is meaningless in
    /// an English menu; WP1 widens this to two-letter English initials, WP0 only
    /// needs the language row itself to read correctly.
    /// Short label for the fallback icon tile when the symbol font has no
    /// glyph for `glyph()`.
    ///
    /// One character per language, so the tile keeps the same weight: a single
    /// Chinese character is meaningless in an English menu, and a full English
    /// word would not fit the tile.
    pub fn icon_text(self, lang: Language) -> &'static str {
        match lang {
            Language::ZhCn => self.icon_text_zh(),
            Language::EnUs => self.icon_text_en(),
        }
    }

    fn icon_text_zh(self) -> &'static str {
        match self {
            QuickRow::Language => "语",
            QuickRow::FastForward => "快",
            QuickRow::FastForwardSound => "音",
            QuickRow::Screen => "画",
            QuickRow::Shelf => "架",
            QuickRow::ShelfLook => "看",
            QuickRow::DisplayPanel => "亮",
            QuickRow::DisplayEffects => "效",
            QuickRow::Rumble => "震",
            QuickRow::DateTime => "时",
            QuickRow::Mods => "M",
            QuickRow::ButtonSettings => "键",
            QuickRow::SystemSettings => "系",
            QuickRow::About => "i",
            QuickRow::SystemUpgrade => "升",
            QuickRow::GbaShader => "色",
            QuickRow::GbShader => "色",
            QuickRow::ColourCorrection => "校",
            QuickRow::Brightness => "亮",
            QuickRow::BlueLight => "蓝",
            QuickRow::Game => "游",
            QuickRow::SaveSettings => "存",
            QuickRow::DefaultLoad => "读",
            QuickRow::LoadGba => "读",
            QuickRow::LoadGb => "读",
            QuickRow::LoadGbc => "读",
            QuickRow::DefaultSave => "存",
            QuickRow::FcMask => "遮",
            QuickRow::GbMask => "遮",
            QuickRow::GbcMask => "遮",
            QuickRow::SfcMask => "遮",
            QuickRow::Masks => "罩",
            QuickRow::SaveGba => "存",
            QuickRow::SaveGb => "存",
            QuickRow::SaveGbc => "存",
            QuickRow::Recents => "近",
            QuickRow::Turbo => "连",
            QuickRow::Rewind => "倒",
            QuickRow::GbPalettes => "调",
            QuickRow::ButtonLayout => "键",
            QuickRow::RomTitle => "题",
            QuickRow::FallbackCover => "图",
            QuickRow::LidTimeout => "盖",
            QuickRow::Shortcuts => "捷",
            QuickRow::Network => "网",
            QuickRow::Wifi => "W",
            QuickRow::Web => "Web",
            QuickRow::SyncStorage => "同",
            QuickRow::SyncContent => "云",
            QuickRow::CartTransfer => "传",
            QuickRow::SplitLink => "联",
            QuickRow::Theme => "主",
            QuickRow::ThemeDefault => "色",
            QuickRow::ThemeWeb => "色",
            QuickRow::ThemeWarm => "色",
            QuickRow::ShelfKeys => "架",
            QuickRow::ShelfPrev => "前",
            QuickRow::ShelfNext => "后",
            QuickRow::ShelfFav => "藏",
            QuickRow::ShortcutMap => "映",
            QuickRow::ShortcutReset => "重",
            QuickRow::ShortcutSaveState => "存",
            QuickRow::ShortcutLoadState => "读",
            QuickRow::ShortcutBrightnessUp => "亮",
            QuickRow::ShortcutBrightnessDown => "暗",
            QuickRow::ShortcutBlueLightDown => "蓝",
            QuickRow::ShortcutBlueLightUp => "蓝",
            QuickRow::ShortcutColorCorrection => "校",
            QuickRow::ShortcutPaletteBack => "调",
            QuickRow::ShortcutPaletteNext => "调",
            QuickRow::ShortcutShaderNext => "着",
            QuickRow::ShortcutShaderBack => "着",
            QuickRow::ShortcutFastForward => "快",
            QuickRow::ShortcutRewind => "倒",
            QuickRow::ControlUp => "上",
            QuickRow::ControlDown => "下",
            QuickRow::ControlLeft => "左",
            QuickRow::ControlRight => "右",
            QuickRow::ControlA => "A",
            QuickRow::ControlB => "B",
            QuickRow::ControlSelect => "S",
            QuickRow::ControlStart => "T",
            QuickRow::ControlL => "L",
            QuickRow::ControlR => "R",
            QuickRow::ControlTurboA => "X",
            QuickRow::ControlTurboB => "Y",
            QuickRow::ControlPreset => "肩",
        }
    }

    fn icon_text_en(self) -> &'static str {
        match self {
            QuickRow::Language => "L",
            QuickRow::FastForward => "F",
            QuickRow::FastForwardSound => "S",
            QuickRow::Screen => "D",
            QuickRow::Shelf => "C",
            QuickRow::ShelfLook => "L",
            QuickRow::DisplayPanel => "B",
            QuickRow::DisplayEffects => "E",
            QuickRow::Rumble => "R",
            QuickRow::DateTime => "T",
            QuickRow::Mods => "M",
            QuickRow::ButtonSettings => "K",
            QuickRow::SystemSettings => "S",
            QuickRow::About => "i",
            QuickRow::SystemUpgrade => "U",
            QuickRow::GbaShader => "S",
            QuickRow::GbShader => "S",
            QuickRow::ColourCorrection => "C",
            QuickRow::Brightness => "B",
            QuickRow::BlueLight => "B",
            QuickRow::Game => "G",
            QuickRow::SaveSettings => "S",
            QuickRow::DefaultLoad => "L",
            QuickRow::LoadGba => "L",
            QuickRow::LoadGb => "L",
            QuickRow::LoadGbc => "L",
            QuickRow::DefaultSave => "S",
            QuickRow::FcMask => "M",
            QuickRow::GbMask => "M",
            QuickRow::GbcMask => "M",
            QuickRow::SfcMask => "M",
            QuickRow::Masks => "M",
            QuickRow::SaveGba => "S",
            QuickRow::SaveGb => "S",
            QuickRow::SaveGbc => "S",
            QuickRow::Recents => "R",
            QuickRow::Turbo => "T",
            QuickRow::Rewind => "R",
            QuickRow::GbPalettes => "P",
            QuickRow::ButtonLayout => "B",
            QuickRow::RomTitle => "T",
            QuickRow::FallbackCover => "I",
            QuickRow::LidTimeout => "L",
            QuickRow::Shortcuts => "K",
            QuickRow::Network => "N",
            QuickRow::Wifi => "W",
            QuickRow::Web => "Web",
            QuickRow::SyncStorage => "M",
            QuickRow::SyncContent => "C",
            QuickRow::CartTransfer => "T",
            QuickRow::SplitLink => "L",
            QuickRow::Theme => "T",
            QuickRow::ThemeDefault => "P",
            QuickRow::ThemeWeb => "P",
            QuickRow::ThemeWarm => "P",
            QuickRow::ShelfKeys => "K",
            QuickRow::ShelfPrev => "<",
            QuickRow::ShelfNext => ">",
            QuickRow::ShelfFav => "F",
            QuickRow::ShortcutMap => "M",
            QuickRow::ShortcutReset => "R",
            QuickRow::ShortcutSaveState => "S",
            QuickRow::ShortcutLoadState => "L",
            QuickRow::ShortcutBrightnessUp => "+",
            QuickRow::ShortcutBrightnessDown => "-",
            QuickRow::ShortcutBlueLightDown => "-",
            QuickRow::ShortcutBlueLightUp => "+",
            QuickRow::ShortcutColorCorrection => "C",
            QuickRow::ShortcutPaletteBack => "P",
            QuickRow::ShortcutPaletteNext => "P",
            QuickRow::ShortcutShaderNext => "S",
            QuickRow::ShortcutShaderBack => "S",
            QuickRow::ShortcutFastForward => "F",
            QuickRow::ShortcutRewind => "R",
            QuickRow::ControlUp => "U",
            QuickRow::ControlDown => "D",
            QuickRow::ControlLeft => "L",
            QuickRow::ControlRight => "R",
            QuickRow::ControlA => "A",
            QuickRow::ControlB => "B",
            QuickRow::ControlSelect => "S",
            QuickRow::ControlStart => "T",
            QuickRow::ControlL => "L",
            QuickRow::ControlR => "R",
            QuickRow::ControlTurboA => "X",
            QuickRow::ControlTurboB => "Y",
            QuickRow::ControlPreset => "S",
        }
    }

    /// Nerd Font glyph used by the Switch-style icon tile. A neutral circle is
    /// substituted by the face builder when a font lacks the glyph.
    pub fn glyph(self) -> char {
        match self {
            QuickRow::FastForward | QuickRow::FastForwardSound => '\u{f04e}',
            QuickRow::Screen => '\u{f108}',
            QuickRow::Shelf => '\u{f07b}',
            QuickRow::ShelfLook => '\u{f03e}',
            QuickRow::DisplayPanel => '\u{f185}',
            QuickRow::DisplayEffects => '\u{f1fc}',
            QuickRow::Rumble => '\u{f0f3}',
            QuickRow::DateTime => '\u{f017}',
            QuickRow::Mods => '\u{f085}',
            QuickRow::ButtonSettings => '\u{f11c}',
            QuickRow::SystemSettings => '\u{f013}',
            QuickRow::About => '\u{f05a}',
            QuickRow::SystemUpgrade => '\u{f019}',
            QuickRow::GbaShader | QuickRow::GbShader | QuickRow::ColourCorrection => '\u{f1de}',
            QuickRow::Brightness => '\u{f185}',
            QuickRow::BlueLight => '\u{f0eb}',
            QuickRow::Game => '\u{f11b}',
            QuickRow::SaveSettings
            | QuickRow::SaveGba
            | QuickRow::SaveGb
            | QuickRow::SaveGbc => '\u{f0c7}',
            QuickRow::DefaultLoad | QuickRow::LoadGba | QuickRow::LoadGb | QuickRow::LoadGbc => '\u{f115}',
            QuickRow::DefaultSave => '\u{f0c7}',
            QuickRow::FcMask | QuickRow::GbMask | QuickRow::GbcMask | QuickRow::SfcMask => {
                '\u{f2d0}'
            }
            QuickRow::Masks => '\u{f0e0}',
            QuickRow::Recents => '\u{f1da}',
            QuickRow::Turbo => '\u{f0e7}',
            QuickRow::Rewind => '\u{f04a}',
            QuickRow::GbPalettes => '\u{f53f}',
            QuickRow::ButtonLayout => '\u{f11c}',
            QuickRow::RomTitle => '\u{f031}',
            QuickRow::FallbackCover => '\u{f03e}',
            QuickRow::LidTimeout => '\u{f186}',
            QuickRow::Shortcuts => '\u{f11c}',
            QuickRow::Network | QuickRow::Wifi => '\u{f1eb}',
            QuickRow::Web => '\u{f0ac}',
            QuickRow::SyncStorage => '\u{f0c2}',
            QuickRow::SyncContent => '\u{f0c2}',
            QuickRow::CartTransfer => '\u{f0c7}',
            QuickRow::SplitLink => '\u{f0c1}',
            QuickRow::Theme
            | QuickRow::ThemeDefault
            | QuickRow::ThemeWeb
            | QuickRow::ThemeWarm => '\u{f1fc}',
            // fa-language; the tile fallback above covers a font without it.
            QuickRow::Language => '\u{f1ab}',
            QuickRow::ShelfKeys | QuickRow::ShelfPrev | QuickRow::ShelfNext | QuickRow::ShelfFav => {
                '\u{f11c}'
            }
            QuickRow::ShortcutReset => '\u{f021}',
            QuickRow::ShortcutMap
            | QuickRow::ShortcutSaveState
            | QuickRow::ShortcutLoadState
            | QuickRow::ShortcutBrightnessUp
            | QuickRow::ShortcutBrightnessDown
            | QuickRow::ShortcutBlueLightDown
            | QuickRow::ShortcutBlueLightUp
            | QuickRow::ShortcutColorCorrection
            | QuickRow::ShortcutPaletteBack
            | QuickRow::ShortcutPaletteNext
            | QuickRow::ShortcutShaderNext
            | QuickRow::ShortcutShaderBack
            | QuickRow::ShortcutFastForward
            | QuickRow::ShortcutRewind => '\u{f11c}',
            QuickRow::ControlUp
            | QuickRow::ControlDown
            | QuickRow::ControlLeft
            | QuickRow::ControlRight
            | QuickRow::ControlA
            | QuickRow::ControlB
            | QuickRow::ControlSelect
            | QuickRow::ControlStart
            | QuickRow::ControlL
            | QuickRow::ControlR
            | QuickRow::ControlTurboA
            | QuickRow::ControlTurboB => '\u{f11c}',
            QuickRow::ControlPreset => '\u{f11c}',
        }
    }

    pub fn parent(self) -> Option<QuickRow> {
        match self {
            QuickRow::DisplayPanel
            | QuickRow::DisplayEffects
            | QuickRow::Masks
            | QuickRow::Theme => Some(QuickRow::Screen),
            QuickRow::Brightness | QuickRow::BlueLight => Some(QuickRow::DisplayPanel),
            QuickRow::GbaShader
            | QuickRow::GbShader
            | QuickRow::ColourCorrection
            | QuickRow::GbPalettes => Some(QuickRow::DisplayEffects),
            QuickRow::FcMask | QuickRow::GbMask | QuickRow::GbcMask | QuickRow::SfcMask => {
                Some(QuickRow::Masks)
            }
            QuickRow::ShelfLook | QuickRow::ShelfKeys => Some(QuickRow::Shelf),
            QuickRow::ThemeDefault | QuickRow::ThemeWeb | QuickRow::ThemeWarm => {
                Some(QuickRow::Theme)
            }
            QuickRow::RomTitle | QuickRow::FallbackCover | QuickRow::Recents => {
                Some(QuickRow::ShelfLook)
            }
            QuickRow::ShelfPrev | QuickRow::ShelfNext | QuickRow::ShelfFav => {
                Some(QuickRow::ShelfKeys)
            }
            QuickRow::FastForward
            | QuickRow::FastForwardSound
            | QuickRow::Rewind
            | QuickRow::Rumble
            | QuickRow::SaveSettings => Some(QuickRow::Game),
            QuickRow::DefaultLoad
            | QuickRow::DefaultSave
            | QuickRow::LoadGba
            | QuickRow::SaveGba
            | QuickRow::LoadGb
            | QuickRow::SaveGb
            | QuickRow::LoadGbc
            | QuickRow::SaveGbc => Some(QuickRow::SaveSettings),
            QuickRow::Shortcuts
            | QuickRow::ShortcutMap
            | QuickRow::ButtonLayout
            | QuickRow::Turbo => Some(QuickRow::ButtonSettings),
            QuickRow::ShortcutSaveState
            | QuickRow::ShortcutLoadState
            | QuickRow::ShortcutBrightnessUp
            | QuickRow::ShortcutBrightnessDown
            | QuickRow::ShortcutBlueLightDown
            | QuickRow::ShortcutBlueLightUp
            | QuickRow::ShortcutColorCorrection
            | QuickRow::ShortcutPaletteBack
            | QuickRow::ShortcutPaletteNext
            | QuickRow::ShortcutShaderNext
            | QuickRow::ShortcutShaderBack
            | QuickRow::ShortcutFastForward
            | QuickRow::ShortcutRewind
            | QuickRow::ShortcutReset => Some(QuickRow::ShortcutMap),
            QuickRow::ControlUp
            | QuickRow::ControlDown
            | QuickRow::ControlLeft
            | QuickRow::ControlRight
            | QuickRow::ControlA
            | QuickRow::ControlB
            | QuickRow::ControlSelect
            | QuickRow::ControlStart
            | QuickRow::ControlL
            | QuickRow::ControlR
            | QuickRow::ControlTurboA
            | QuickRow::ControlTurboB
            | QuickRow::ControlPreset => Some(QuickRow::ButtonLayout),
            QuickRow::Wifi
            | QuickRow::Web
            | QuickRow::SyncStorage
            | QuickRow::SyncContent
            | QuickRow::CartTransfer
            | QuickRow::SplitLink => Some(QuickRow::Network),
            QuickRow::DateTime
            | QuickRow::Language
            | QuickRow::LidTimeout
            | QuickRow::Mods
            | QuickRow::SystemUpgrade
            | QuickRow::About => Some(QuickRow::SystemSettings),
            _ => None,
        }
    }

    pub fn child(self) -> Option<QuickRow> {
        match self {
            QuickRow::Screen => Some(QuickRow::DISPLAY[0]),
            QuickRow::DisplayPanel => Some(QuickRow::DISPLAY_PANEL[0]),
            QuickRow::DisplayEffects => Some(QuickRow::DISPLAY_EFFECTS[0]),
            QuickRow::Masks => Some(QuickRow::MASKS[0]),
            QuickRow::Shelf => Some(QuickRow::SHELF[0]),
            QuickRow::ShelfLook => Some(QuickRow::SHELF_LOOK[0]),
            QuickRow::Theme => Some(QuickRow::THEMES[0]),
            QuickRow::Game => Some(QuickRow::GAME[0]),
            QuickRow::Network => Some(QuickRow::NETWORK[0]),
            QuickRow::ShelfKeys => Some(QuickRow::SHELF_KEYS[0]),
            QuickRow::ShortcutMap => Some(QuickRow::SHORTCUTS[0]),
            QuickRow::ButtonLayout => Some(QuickRow::CONTROLS[0]),
            QuickRow::ButtonSettings => Some(QuickRow::BUTTON_SETTINGS[0]),
            QuickRow::SystemSettings => Some(QuickRow::SYSTEM_SETTINGS[0]),
            QuickRow::SaveSettings => Some(QuickRow::SAVES[0]),
            _ => None,
        }
    }

    pub fn position(self) -> usize {
        self.page().iter().position(|r| *r == self).unwrap_or(0)
    }

    /// The translation key for this row's label.
    ///
    /// One match instead of a literal per language: `label` is then a single
    /// lookup, and the parity test in `slot-i18n` guarantees both files carry
    /// every key this table can return.
    pub fn key(self) -> &'static str {
        match self {
            QuickRow::FastForward => "device.quick.label.fast_forward",
            QuickRow::FastForwardSound => "device.quick.label.fast_forward_sound",
            QuickRow::Screen => "device.quick.label.screen",
            QuickRow::Rumble => "device.quick.label.rumble",
            QuickRow::DateTime => "device.quick.label.date_time",
            QuickRow::Mods => "device.quick.label.mods",
            QuickRow::About => "device.quick.label.about",
            QuickRow::SystemUpgrade => "device.quick.label.system_upgrade",
            QuickRow::GbaShader => "device.quick.label.gba_shader",
            QuickRow::GbShader => "device.quick.label.gb_shader",
            QuickRow::ColourCorrection => "device.quick.label.colour_correction",
            QuickRow::Brightness => "device.quick.label.brightness",
            QuickRow::BlueLight => "device.quick.label.blue_light",
            QuickRow::Game => "device.quick.label.game",
            QuickRow::SaveSettings => "device.quick.label.save_settings",
            QuickRow::LoadGba => "device.quick.label.load_gba",
            QuickRow::SaveGba => "device.quick.label.save_gba",
            QuickRow::LoadGb => "device.quick.label.load_gb",
            QuickRow::SaveGb => "device.quick.label.save_gb",
            QuickRow::LoadGbc => "device.quick.label.load_gbc",
            QuickRow::SaveGbc => "device.quick.label.save_gbc",
            QuickRow::Recents => "device.quick.label.recents",
            QuickRow::Turbo => "device.quick.label.turbo",
            QuickRow::Rewind => "device.quick.label.rewind",
            QuickRow::GbPalettes => "device.quick.label.gb_palettes",
            QuickRow::ButtonLayout => "device.quick.label.button_layout",
            QuickRow::RomTitle => "device.quick.label.rom_title",
            QuickRow::FallbackCover => "device.quick.label.fallback_cover",
            QuickRow::LidTimeout => "device.quick.label.lid_timeout",
            QuickRow::ButtonSettings => "device.quick.label.button_settings",
            QuickRow::SystemSettings => "device.quick.label.system_settings",
            QuickRow::Shortcuts => "device.quick.label.shortcuts",
            QuickRow::Network => "device.quick.label.network",
            QuickRow::Wifi => "device.quick.label.wifi",
            QuickRow::Web => "device.quick.label.web",
            QuickRow::SyncStorage => "device.quick.label.sync_storage",
            QuickRow::ShelfKeys => "device.quick.label.shelf_keys",
            QuickRow::ShelfPrev => "device.quick.label.shelf_prev",
            QuickRow::ShelfNext => "device.quick.label.shelf_next",
            QuickRow::ShelfFav => "device.quick.label.shelf_fav",
            QuickRow::ShortcutMap => "device.quick.label.shortcut_map",
            QuickRow::ShortcutSaveState => "device.quick.label.shortcut_save_state",
            QuickRow::ShortcutLoadState => "device.quick.label.shortcut_load_state",
            QuickRow::ShortcutBrightnessUp => "device.quick.label.shortcut_brightness_up",
            QuickRow::ShortcutBrightnessDown => "device.quick.label.shortcut_brightness_down",
            QuickRow::ShortcutBlueLightDown => "device.quick.label.shortcut_blue_light_down",
            QuickRow::ShortcutBlueLightUp => "device.quick.label.shortcut_blue_light_up",
            QuickRow::ShortcutColorCorrection => "device.quick.label.shortcut_color_correction",
            QuickRow::ShortcutPaletteBack => "device.quick.label.shortcut_palette_back",
            QuickRow::ShortcutPaletteNext => "device.quick.label.shortcut_palette_next",
            QuickRow::ShortcutShaderNext => "device.quick.label.shortcut_shader_next",
            QuickRow::ShortcutShaderBack => "device.quick.label.shortcut_shader_back",
            QuickRow::ShortcutFastForward => "device.quick.label.shortcut_fast_forward",
            QuickRow::ShortcutRewind => "device.quick.label.shortcut_rewind",
            QuickRow::ControlUp => "device.quick.label.control_up",
            QuickRow::ControlDown => "device.quick.label.control_down",
            QuickRow::ControlLeft => "device.quick.label.control_left",
            QuickRow::ControlRight => "device.quick.label.control_right",
            QuickRow::ControlA => "device.quick.label.control_a",
            QuickRow::ControlB => "device.quick.label.control_b",
            QuickRow::ControlSelect => "device.quick.label.control_select",
            QuickRow::ControlStart => "device.quick.label.control_start",
            QuickRow::ControlL => "device.quick.label.control_l",
            QuickRow::ControlR => "device.quick.label.control_r",
            QuickRow::ControlTurboA => "device.quick.label.control_turbo_a",
            QuickRow::ControlTurboB => "device.quick.label.control_turbo_b",
            QuickRow::ControlPreset => "device.quick.label.control_preset",
            QuickRow::SyncContent => "device.quick.label.sync_content",
            QuickRow::CartTransfer => "device.quick.label.cart_transfer",
            QuickRow::ShortcutReset => "device.quick.label.shortcut_reset",
            QuickRow::DisplayPanel => "device.quick.label.display_panel",
            QuickRow::DisplayEffects => "device.quick.label.display_effects",
            QuickRow::Shelf => "device.quick.label.shelf",
            QuickRow::ShelfLook => "device.quick.label.shelf_look",
            QuickRow::DefaultLoad => "device.quick.label.default_load",
            QuickRow::DefaultSave => "device.quick.label.default_save",
            QuickRow::FcMask => "device.quick.label.fc_mask",
            QuickRow::GbMask => "device.quick.label.gb_mask",
            QuickRow::GbcMask => "device.quick.label.gbc_mask",
            QuickRow::SfcMask => "device.quick.label.sfc_mask",
            QuickRow::Masks => "device.quick.label.masks",
            QuickRow::Theme => "device.quick.label.theme",
            QuickRow::ThemeDefault => "device.quick.label.theme_default",
            QuickRow::ThemeWeb => "device.quick.label.theme_web",
            QuickRow::ThemeWarm => "device.quick.label.theme_warm",
            QuickRow::Language => "device.quick.label.language",
            QuickRow::SplitLink => "device.quick.label.split_link",
        }
    }

    /// The row's label in `lang`.
    pub fn label(self, lang: Language) -> &'static str {
        slot_i18n::t(lang, self.key())
    }

    pub fn opens(self) -> bool {
        matches!(
            self,
            QuickRow::Shortcuts
                | QuickRow::DateTime
                | QuickRow::Mods
                | QuickRow::ButtonSettings
                | QuickRow::SystemSettings
                | QuickRow::About
                | QuickRow::SystemUpgrade
                | QuickRow::Screen
                | QuickRow::Shelf
                | QuickRow::ShelfLook
                | QuickRow::Theme
                | QuickRow::DisplayPanel
                | QuickRow::DisplayEffects
                | QuickRow::Masks
                | QuickRow::Game
                | QuickRow::Network
                | QuickRow::SaveSettings
                | QuickRow::ShelfKeys
                | QuickRow::ShortcutMap
                | QuickRow::ButtonLayout
                | QuickRow::Wifi
                | QuickRow::Web
                | QuickRow::SyncStorage
                | QuickRow::CartTransfer
        )
    }

    pub fn up(self) -> QuickRow {
        let page = self.page();
        page[(self.position() + page.len() - 1) % page.len()]
    }

    pub fn down(self) -> QuickRow {
        let page = self.page();
        page[(self.position() + 1) % page.len()]
    }

    pub fn resume(self) -> Option<(Platform, ResumeKind)> {
        match self {
            QuickRow::LoadGba => Some((Platform::Gba, ResumeKind::Load)),
            QuickRow::SaveGba => Some((Platform::Gba, ResumeKind::Save)),
            QuickRow::LoadGb => Some((Platform::Gb, ResumeKind::Load)),
            QuickRow::SaveGb => Some((Platform::Gb, ResumeKind::Save)),
            QuickRow::LoadGbc => Some((Platform::Gbc, ResumeKind::Load)),
            QuickRow::SaveGbc => Some((Platform::Gbc, ResumeKind::Save)),
            _ => None,
        }
    }

    /// The two global defaults at the top of the save page.
    pub fn resume_default(self) -> Option<ResumeKind> {
        match self {
            QuickRow::DefaultLoad => Some(ResumeKind::Load),
            QuickRow::DefaultSave => Some(ResumeKind::Save),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum QuickValue {
    Speed2,
    Speed3,
    Speed4,
    Speed6,
    On,
    Off,
    Level0,
    Level1,
    Level2,
    Level3,
    Level4,
    Level5,
    Level6,
    Level7,
    Level8,
    Level9,
    Palette(GbPalette),
    Control(Device),
    Shortcut(Device),
    MenuShortcut(Device),
    PlainShortcut(Device),
    Lcd3x,
    Grid,
    Dot,
    Simpletex,
    FourPass,
    SwapShoulders,
    XyShoulders,
    Both,
    Minutes3,
    Minutes10,
    Minutes30,
    Never,
    SyncAll,
    SyncState,
    SyncSav,
    /// A per-platform resume switch that follows the global default.
    Follow,
    /// A shortcut that was turned off: no chording will fire it.
    Unbound,
    /// The palette that is already selected in the theme list.
    ThemeCurrent,
    /// A palette that can be selected in the theme list.
    ThemePick,
    /// 系统 → 语言 → 中文.
    LanguageZh,
    /// 系统 → 语言 → English.
    LanguageEn,
}

impl QuickValue {
    pub const ALL: [QuickValue; 37] = [
        QuickValue::Speed2,
        QuickValue::Speed3,
        QuickValue::Speed4,
        QuickValue::Speed6,
        QuickValue::On,
        QuickValue::Off,
        QuickValue::Level0,
        QuickValue::Level1,
        QuickValue::Level2,
        QuickValue::Level3,
        QuickValue::Level4,
        QuickValue::Level5,
        QuickValue::Level6,
        QuickValue::Level7,
        QuickValue::Level8,
        QuickValue::Level9,
        QuickValue::Lcd3x,
        QuickValue::Grid,
        QuickValue::Dot,
        QuickValue::Simpletex,
        QuickValue::FourPass,
        QuickValue::SwapShoulders,
        QuickValue::XyShoulders,
        QuickValue::Both,
        QuickValue::Minutes3,
        QuickValue::Minutes10,
        QuickValue::Minutes30,
        QuickValue::Never,
        QuickValue::SyncAll,
        QuickValue::SyncState,
        QuickValue::SyncSav,
        QuickValue::Follow,
        QuickValue::Unbound,
        QuickValue::ThemeCurrent,
        QuickValue::ThemePick,
        QuickValue::LanguageZh,
        QuickValue::LanguageEn,
    ];

    pub fn index(self) -> usize {
        match self {
            QuickValue::Palette(p) => Self::ALL.len() + p.index(),
            QuickValue::Control(d) => {
                Self::ALL.len()
                    + GbPalette::COUNT
                    + Device::ALL.iter().position(|x| *x == d).unwrap_or(0)
            }
            QuickValue::Shortcut(d) => {
                Self::ALL.len()
                    + GbPalette::COUNT
                    + Device::ALL.len()
                    + Device::ALL.iter().position(|x| *x == d).unwrap_or(0)
            }
            QuickValue::MenuShortcut(d) => {
                Self::ALL.len()
                    + GbPalette::COUNT
                    + Device::ALL.len() * 2
                    + Device::ALL.iter().position(|x| *x == d).unwrap_or(0)
            }
            QuickValue::PlainShortcut(d) => {
                Self::ALL.len()
                    + GbPalette::COUNT
                    + Device::ALL.len() * 3
                    + Device::ALL.iter().position(|x| *x == d).unwrap_or(0)
            }
            other => Self::ALL.iter().position(|v| *v == other).unwrap_or(0),
        }
    }

    pub fn palette_values() -> impl Iterator<Item = QuickValue> {
        GbPalette::all().map(QuickValue::Palette)
    }

    pub fn control_values() -> impl Iterator<Item = QuickValue> {
        Device::ALL.into_iter().map(QuickValue::Control)
    }

    pub fn shortcut_values() -> impl Iterator<Item = QuickValue> {
        Device::ALL.into_iter().map(QuickValue::Shortcut)
    }

    pub fn menu_shortcut_values() -> impl Iterator<Item = QuickValue> {
        Device::ALL.into_iter().map(QuickValue::MenuShortcut)
    }

    pub fn plain_shortcut_values() -> impl Iterator<Item = QuickValue> {
        Device::ALL.into_iter().map(QuickValue::PlainShortcut)
    }

    /// The value's text in `lang`.
    ///
    /// The two language names are translated even though they read the same in
    /// both languages — that is what keeps the row legible to someone who
    /// switched by accident.
    pub fn text(self, lang: Language) -> &'static str {
        match self {
            QuickValue::LanguageZh => slot_i18n::t(lang, "device.quick.value.language_zh"),
            QuickValue::LanguageEn => slot_i18n::t(lang, "device.quick.value.language_en"),
            QuickValue::Speed2 => "2×",
            QuickValue::Speed3 => "3×",
            QuickValue::Speed4 => "4×",
            QuickValue::Speed6 => "6×",
            QuickValue::On => slot_i18n::t(lang, "device.quick.value.on"),
            QuickValue::Off => slot_i18n::t(lang, "device.quick.value.off"),
            QuickValue::Level0 => "0",
            QuickValue::Level1 => "1",
            QuickValue::Level2 => "2",
            QuickValue::Level3 => "3",
            QuickValue::Level4 => "4",
            QuickValue::Level5 => "5",
            QuickValue::Level6 => "6",
            QuickValue::Level7 => "7",
            QuickValue::Level8 => "8",
            QuickValue::Level9 => "9",
            QuickValue::Palette(p) => palette_name(p, lang),
            QuickValue::Control(d) => d.label(),
            QuickValue::Shortcut(d) => shortcut_chord(d),
            QuickValue::MenuShortcut(d) => menu_shortcut_chord(d),
            QuickValue::PlainShortcut(d) => d.label(),
            QuickValue::Lcd3x => "LCD3x",
            QuickValue::Grid => slot_i18n::t(lang, "device.quick.value.grid"),
            QuickValue::Dot => slot_i18n::t(lang, "device.quick.value.dot"),
            QuickValue::Simpletex => slot_i18n::t(lang, "device.quick.value.simpletex"),
            QuickValue::FourPass => slot_i18n::t(lang, "device.quick.value.four_pass"),
            QuickValue::SwapShoulders => slot_i18n::t(lang, "device.quick.value.swap_shoulders"),
            QuickValue::XyShoulders => slot_i18n::t(lang, "device.quick.value.xy_shoulders"),
            QuickValue::Both => slot_i18n::t(lang, "device.quick.value.both"),
            QuickValue::Minutes3 => slot_i18n::t(lang, "device.quick.value.minutes_3"),
            QuickValue::Minutes10 => slot_i18n::t(lang, "device.quick.value.minutes_10"),
            QuickValue::Minutes30 => slot_i18n::t(lang, "device.quick.value.minutes_30"),
            QuickValue::Never => slot_i18n::t(lang, "device.quick.value.never"),
            QuickValue::SyncAll => slot_i18n::t(lang, "device.quick.value.sync_all"),
            QuickValue::SyncState => slot_i18n::t(lang, "device.quick.value.sync_state"),
            QuickValue::SyncSav => slot_i18n::t(lang, "device.quick.value.sync_sav"),
            QuickValue::Follow => slot_i18n::t(lang, "device.quick.value.follow"),
            QuickValue::Unbound => slot_i18n::t(lang, "device.quick.value.unbound"),
            QuickValue::ThemeCurrent => slot_i18n::t(lang, "device.quick.value.theme_current"),
            QuickValue::ThemePick => slot_i18n::t(lang, "device.quick.value.theme_pick"),
        }
    }

    pub fn speed(frames: u8) -> Option<QuickValue> {
        match frames {
            0 => Some(QuickValue::Off),
            2 => Some(QuickValue::Speed2),
            3 => Some(QuickValue::Speed3),
            4 => Some(QuickValue::Speed4),
            6 => Some(QuickValue::Speed6),
            _ => None,
        }
    }

    pub fn flag(on: bool) -> QuickValue {
        if on {
            QuickValue::On
        } else {
            QuickValue::Off
        }
    }

    pub fn level(n: u8) -> QuickValue {
        [
            QuickValue::Level0,
            QuickValue::Level1,
            QuickValue::Level2,
            QuickValue::Level3,
            QuickValue::Level4,
            QuickValue::Level5,
            QuickValue::Level6,
            QuickValue::Level7,
            QuickValue::Level8,
            QuickValue::Level9,
        ][usize::from(n).min(9)]
    }
}

/// A palette's name in `lang`.
///
/// Chinese uses the fork's own name; English uses the mGBA name, which is
/// already English (`Grayscale`, `DMG Green`, …) and is what the state file
/// stores, so nothing new has to be translated for 48 palettes.
pub fn palette_name(palette: GbPalette, lang: Language) -> &'static str {
    match lang {
        Language::ZhCn => palette.label(),
        Language::EnUs => palette.core_name(),
    }
}

/// The Switch-style face for a Select chord, e.g. `SEL+UP`.
fn menu_shortcut_chord(device: Device) -> &'static str {
    match device {
        Device::Up => "MENU+UP",
        Device::Down => "MENU+DOWN",
        Device::Left => "MENU+LEFT",
        Device::Right => "MENU+RIGHT",
        Device::A => "MENU+A",
        Device::B => "MENU+B",
        Device::X => "MENU+X",
        Device::Y => "MENU+Y",
        Device::L1 => "MENU+L1",
        Device::R1 => "MENU+R1",
        Device::L2 => "MENU+L2",
        Device::R2 => "MENU+R2",
        Device::Select => "MENU+SELECT",
        Device::Start => "MENU+START",
    }
}

fn shortcut_chord(device: Device) -> &'static str {
    match device {
        Device::Up => "SEL+UP",
        Device::Down => "SEL+DOWN",
        Device::Left => "SEL+LEFT",
        Device::Right => "SEL+RIGHT",
        Device::A => "SEL+A",
        Device::B => "SEL+B",
        Device::X => "SEL+X",
        Device::Y => "SEL+Y",
        Device::L1 => "SEL+L1",
        Device::R1 => "SEL+R1",
        Device::L2 => "SEL+L2",
        Device::R2 => "SEL+R2",
        Device::Select => "SEL+SELECT",
        Device::Start => "SEL+START",
    }
}

pub const QUICK_PITCH: f32 = 48.0;
pub const QUICK_TOP: f32 = 88.0;
pub const QUICK_EDGE: f32 = 48.0;
pub const QUICK_CARD_W: u32 = 624;
pub const QUICK_CARD_H: u32 = 42;
const CARD_Y: f32 = 88.0;
const ICON_PX: f32 = 24.0;
const ICON_X: f32 = 64.0;
const LABEL_X: f32 = 104.0;
const RIGHT_PAD: f32 = 24.0;
const CARET_GAP: f32 = 14.0;
const CARET_PX: f32 = 24.0;
const LEGEND_Y: f32 = 430.0;
const HEADER_Y: f32 = 22.0;
const HEADER_H: f32 = 30.0;
pub const QUICK_SPLIT: f32 = LEGEND_Y - 2.0;

pub fn quick_label_face(row: QuickRow, lang: Language) -> UndoFace {
    quick_label_face_state(row, false, lang)
}

pub fn quick_label_face_state(row: QuickRow, lit: bool, lang: Language) -> UndoFace {
    quick_text_face(
        row.label(lang),
        if lit {
            switch_style::ink_selected()
        } else {
            switch_style::ink()
        },
    )
}

/// `text` is already resolved: the clock, a mask file name and a menu value all
/// arrive as strings, so this builder takes the string rather than a value.
pub fn quick_value_face(text: &str, lit: bool) -> UndoFace {
    quick_text_face(
        text,
        if lit {
            switch_style::ink_selected()
        } else {
            switch_style::ink_dim()
        },
    )
}

/// The page header, drawn from the page's index rather than a title string so
/// the language decides the text.
pub fn quick_title_face(index: usize, lang: Language) -> UndoFace {
    switch_style::title_face(QuickRow::page_title_at(index, lang), 30.0)
}

pub fn quick_icon_face(row: QuickRow, lit: bool, lang: Language) -> crate::cart::CartFace {
    let colour = if lit {
        switch_style::accent()
    } else {
        switch_style::ink()
    };
    let face = crate::icon::glyph_face(row.glyph(), ICON_PX, colour);
    if face.w > 0 {
        face
    } else {
        let fill = if lit {
            [
                switch_style::accent()[0],
                switch_style::accent()[1],
                switch_style::accent()[2],
                255,
            ]
        } else {
            switch_style::surface_border()
        };
        let face = switch_style::tile_face(row.icon_text(lang), fill, switch_style::ink(), 28);
        crate::cart::CartFace {
            rgba: face.rgba,
            w: face.w,
            h: face.h,
        }
    }
}

/// Smallest label size `text::fit` may fall back to when a translation is
/// wider than the card.
const LABEL_MIN_PX: f32 = 18.0;

fn quick_text_face(label: &str, colour: [u8; 3]) -> UndoFace {
    let Some(font) = text::label_font() else {
        return UndoFace {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    };
    // Long labels shrink instead of being special-cased per language: English
    // strings are routinely wider than the Chinese they replace.
    let px = MENU_PX;
    let layout = text::fit(font, label, OUT_W as f32, 1, px, LABEL_MIN_PX);
    let set = layout
        .lines
        .iter()
        .map(|l| text::line_width(font, l, layout.px, layout.tracking))
        .fold(0.0, f32::max);
    let w = set.ceil() as u32 + 2 * MENU_PAD;
    let mut rgba = vec![0u8; (w * MENU_H * 4) as usize];
    text::draw_centred(&mut rgba, w, MENU_H, &layout, colour);
    UndoFace { rgba, w, h: MENU_H }
}

pub fn quick_caret_face(right: bool) -> UndoFace {
    quick_caret_face_state(right, false)
}

pub fn quick_caret_face_state(right: bool, lit: bool) -> UndoFace {
    let glyph = if right { '\u{f0da}' } else { '\u{f0d9}' };
    let ink = if lit {
        switch_style::ink_selected()
    } else {
        switch_style::ink()
    };
    let (Some(symbols), Some(label)) = (crate::icon::symbols_font(), text::label_font()) else {
        return UndoFace {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    };
    let (m, cov) = symbols.rasterize(glyph, CARET_PX);
    let (w, h) = (m.width as u32, MENU_H);
    let centre = match label.horizontal_line_metrics(MENU_PX) {
        Some(v) => {
            (h as f32 - v.new_line_size) / 2.0 + v.ascent
                - label.metrics('H', MENU_PX).height as f32 / 2.0
        }
        None => h as f32 / 2.0,
    };
    let top = (centre - m.height as f32 / 2.0).round() as i32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for gy in 0..m.height {
        let y = top + gy as i32;
        if y < 0 || y >= h as i32 {
            continue;
        }
        for gx in 0..m.width {
            let at = ((y as u32 * w + gx as u32) * 4) as usize;
            let a = cov[gy * m.width + gx];
            rgba[at..at + 4].copy_from_slice(&[ink[0], ink[1], ink[2], a]);
        }
    }
    UndoFace { rgba, w, h }
}

pub fn quick_legend_faces(lang: Language) -> [UndoFace; 3] {
    [
        hint_face("B", slot_i18n::t(lang, "device.quick.legend.back")),
        arrows_hint_face(slot_i18n::t(lang, "device.quick.legend.adjust")),
        hint_face("A", slot_i18n::t(lang, "device.quick.legend.open")),
    ]
}

pub struct QuickMenuFaces {
    pub cards: [(TexId, u32, u32); 2],
    pub labels: Vec<[(TexId, u32, u32); 2]>,
    pub values: Vec<[(TexId, u32, u32); 2]>,
    /// [left, right] × [unselected, selected]
    pub carets: [[(TexId, u32, u32); 2]; 2],
    pub icons: Vec<[(TexId, u32, u32); 2]>,
    /// One title per page, indexed by `QuickRow::page_index`.
    pub titles: [(TexId, u32, u32); QuickRow::QUICK_PAGES],
    pub legend: [(TexId, u32); 3],
}

pub struct QuickMenu<'a> {
    pub row: QuickRow,
    pub values: [Option<QuickValue>; QuickRow::ALL.len()],
    pub clock: Option<[(TexId, u32, u32); 2]>,
    /// Mask values are file names, so they arrive as ready faces.
    pub fc_mask_value: Option<(TexId, u32, u32)>,
    pub gb_mask_value: Option<(TexId, u32, u32)>,
    pub gbc_mask_value: Option<(TexId, u32, u32)>,
    pub sfc_mask_value: Option<(TexId, u32, u32)>,
    pub faces: Option<&'a QuickMenuFaces>,
}

impl QuickMenu<'_> {
    pub fn draw(&self, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: colour(switch_style::bg()),
        });
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: 96.0,
            colour: colour(switch_style::bg_top()),
        });
        out.push(Draw::Rect {
            x: QUICK_EDGE,
            y: HEADER_Y,
            w: 5.0,
            h: HEADER_H,
            colour: colour([switch_style::accent()[0], switch_style::accent()[1], switch_style::accent()[2], 255]),
        });
        let Some(faces) = self.faces else {
            return;
        };
        let page = self.row.page_index();
        if let Some(&(tex, w, h)) = faces.titles.get(page) {
            push(out, tex, 64.0, 18.0, w, h);
        }
        let dot_x = OUT_W as f32 - QUICK_EDGE - (faces.titles.len() as f32 - 1.0) * 14.0;
        for i in 0..faces.titles.len() {
            let on = i == page;
            out.push(Draw::Rect {
                x: dot_x + i as f32 * 14.0,
                y: 34.0,
                w: 6.0,
                h: 6.0,
                colour: if on {
                    colour([switch_style::accent()[0], switch_style::accent()[1], switch_style::accent()[2], 255])
                } else {
                    [0.36, 0.36, 0.4, 1.0]
                },
            });
        }
        let card_x = (OUT_W as f32 - faces.cards[0].1 as f32) / 2.0;
        let card_right = card_x + faces.cards[0].1 as f32;
        let page = self.row.page();
        let (offset, take) = page_window(page, self.row.position());
        for (at, &row) in page.iter().enumerate().skip(offset).take(take) {
            let y = CARD_Y + QUICK_PITCH * (at - offset) as f32;
            let lit = row == self.row;
            let state = lit as usize;
            let (card_tex, card_w, card_h) = faces.cards[state];
            push(out, card_tex, card_x, y, card_w, card_h);
            if let Some(&(tex, w, h)) = faces.icons.get(row.index()).map(|f| &f[state]) {
                push(out, tex, ICON_X, y + (card_h as f32 - h as f32) / 2.0, w, h);
            }
            if let Some(&(tex, w, h)) = faces.labels.get(row.index()).map(|f| &f[state]) {
                push(out, tex, LABEL_X, y + (card_h as f32 - h as f32) / 2.0, w, h);
            }
            let value = match row {
                QuickRow::DateTime => self.clock.map(|c| c[state]),
                QuickRow::FcMask => self.fc_mask_value,
                QuickRow::GbMask => self.gb_mask_value,
                QuickRow::GbcMask => self.gbc_mask_value,
                QuickRow::SfcMask => self.sfc_mask_value,
                _ => self.values[row.index()]
                    .and_then(|v| faces.values.get(v.index()))
                    .map(|v| v[state]),
            };
            if row.opens() {
                let (chevron_tex, cw, ch) = faces.carets[1][state];
                let cx = card_right - RIGHT_PAD - cw as f32;
                if let Some((tex, w, h)) = value {
                    push(
                        out,
                        tex,
                        cx - 8.0 - w as f32,
                        y + (card_h as f32 - h as f32) / 2.0,
                        w,
                        h,
                    );
                }
                push(
                    out,
                    chevron_tex,
                    cx,
                    y + (card_h as f32 - ch as f32) / 2.0,
                    cw,
                    ch,
                );
                continue;
            }
            let Some((tex, w, h)) = value else {
                continue;
            };
            let ty = y + (card_h as f32 - h as f32) / 2.0;
            if !lit {
                push(out, tex, card_right - RIGHT_PAD - w as f32, ty, w, h);
                continue;
            }
            let (right_tex, rw, rh) = faces.carets[1][state];
            let rx = card_right - RIGHT_PAD - rw as f32;
            push(out, right_tex, rx, ty, rw, rh);
            let vx = rx - CARET_GAP + MENU_PAD as f32 - w as f32;
            push(out, tex, vx, ty, w, h);
            let (left_tex, lw, lh) = faces.carets[0][state];
            push(out, left_tex, vx + MENU_PAD as f32 - CARET_GAP - lw as f32, ty, lw, lh);
        }
    }
}

fn colour(c: [u8; 4]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

pub fn row_top(row: QuickRow) -> f32 {
    CARD_Y + QUICK_PITCH * row.position() as f32
}

const MAX_ROWS: usize = 8;

fn page_window(page: &[QuickRow], selected: usize) -> (usize, usize) {
    let take = page.len().min(MAX_ROWS);
    if take == 0 {
        return (0, 0);
    }
    let offset = selected
        .saturating_sub(MAX_ROWS - 1)
        .min(page.len().saturating_sub(take));
    (offset, take)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thirteen_row_page_scrolls_to_keep_the_selection_on_screen() {
        assert_eq!(page_window(&QuickRow::SHORTCUTS, 0), (0, 8));
        assert_eq!(page_window(&QuickRow::SHORTCUTS, 7), (0, 8));
        assert_eq!(page_window(&QuickRow::SHORTCUTS, 8), (1, 8));
        assert_eq!(page_window(&QuickRow::SHORTCUTS, 12), (5, 8));
    }

    #[test]
    fn every_quick_row_indexes_into_the_value_table() {
        for row in QuickRow::ALL {
            assert!(
                row.index() < QuickRow::ALL.len(),
                "{row:?} is outside the value table"
            );
        }
    }

    #[test]
    fn the_main_page_never_scrolls() {
        assert_eq!(page_window(&QuickRow::MAIN, 5), (0, 6));
        // 主题 moved under 画面, so that page must still fit without scrolling.
        assert_eq!(page_window(&QuickRow::DISPLAY, 3), (0, 4));
    }

    #[test]
    fn the_language_row_is_appended_so_no_earlier_index_moves() {
        assert_eq!(*QuickRow::ALL.last().expect("ALL is not empty"), QuickRow::Language);
        // `index()` is the enum discriminant and the key into the value table:
        // the new row must stay inside it, which is why `Language` is declared
        // before `SplitLink` even though it is last in `ALL`.
        assert!(QuickRow::Language.index() < QuickRow::ALL.len());
        assert_eq!(QuickRow::Language.page(), &QuickRow::SYSTEM_SETTINGS[..]);
        assert!(QuickRow::SYSTEM_SETTINGS.contains(&QuickRow::Language));
        // 日期与时间 must stay first: `page_index()` reads `page()[0]`.
        assert_eq!(QuickRow::SYSTEM_SETTINGS[0], QuickRow::DateTime);
        assert_eq!(
            QuickRow::Language.page_index(),
            QuickRow::DateTime.page_index()
        );
        assert_eq!(QuickRow::Language.parent(), Some(QuickRow::SystemSettings));
        // A value row, not a page: it must not steal the A button.
        assert!(!QuickRow::Language.opens());
    }

    #[test]
    fn the_language_values_are_last_and_read_in_the_requested_language() {
        assert_eq!(*QuickValue::ALL.last().expect("ALL is not empty"), QuickValue::LanguageEn);
        assert_eq!(QuickValue::LanguageZh.text(Language::ZhCn), "中文");
        assert_eq!(QuickValue::LanguageZh.text(Language::EnUs), "中文");
        assert_eq!(QuickValue::LanguageEn.text(Language::ZhCn), "English");
        assert_eq!(QuickValue::LanguageEn.text(Language::EnUs), "English");
        assert_eq!(QuickRow::Language.label(Language::ZhCn), "语言");
        assert_eq!(QuickRow::Language.label(Language::EnUs), "Language");
    }

    #[test]
    fn page_titles_follow_the_language() {
        for (index, key) in QuickRow::PAGE_KEYS.iter().enumerate() {
            let zh = QuickRow::page_title_at(index, Language::ZhCn);
            let en = QuickRow::page_title_at(index, Language::EnUs);
            assert_eq!(zh, slot_i18n::t(Language::ZhCn, key));
            assert_eq!(en, slot_i18n::t(Language::EnUs, key));
            assert_ne!(zh, en, "page title {key} did not switch language");
        }
    }
}

fn push(out: &mut Vec<Draw>, tex: TexId, x: f32, y: f32, w: u32, h: u32) {
    out.push(Draw::Tex {
        x: x.round(),
        y: y.round(),
        w: w as f32,
        h: h as f32,
        tex,
        alpha: 1.0,
    });
}
