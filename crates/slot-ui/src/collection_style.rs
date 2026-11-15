//! Shared colour identities for the three cartridge platforms and the two
//! system collections.  The base hues follow the GBAStation web platform
//! badges; the foreground tones are adapted for the dark Switch-style device
//! UI and its white selected cards.

use slot_store::Platform;

use crate::switch_style as sw;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentStyle {
    pub base: [u8; 3],
    pub on_dark: [u8; 3],
    /// Foreground colour for the light chip on a selected white card.  System
    /// collections only use the dark shelf ground, so this stays `None`.
    pub on_light: Option<[u8; 3]>,
}

pub const GBA: AccentStyle = AccentStyle {
    base: [0x20, 0x6b, 0xc4],
    on_dark: [0x6a, 0xa8, 0xff],
    on_light: Some([0x17, 0x56, 0xa3]),
};

pub const GB: AccentStyle = AccentStyle {
    base: [0x2f, 0xb3, 0x44],
    on_dark: [0x5b, 0xd9, 0x7a],
    on_light: Some([0x1a, 0x73, 0x2e]),
};

pub const GBC: AccentStyle = AccentStyle {
    base: [0x8b, 0x5c, 0xf6],
    on_dark: [0xb9, 0xa3, 0xff],
    on_light: Some([0x6d, 0x34, 0xd1]),
};

pub const RECENTS: AccentStyle = AccentStyle {
    base: [0xf2, 0x84, 0x5c],
    on_dark: [0xff, 0x9e, 0x7d],
    on_light: None,
};

pub const FAVORITES: AccentStyle = AccentStyle {
    base: [0xf2, 0xc1, 0x4e],
    on_dark: [0xff, 0xd7, 0x6a],
    on_light: None,
};

/// Famicom / NES. The one machine here whose badge everyone already knows, so
/// the hue is Nintendo's own red rather than a fourth invented one — it also
/// keeps the shelf readable beside the Advance's blue, the Game Boy's green and
/// the Color's violet.
pub const FC: AccentStyle = AccentStyle {
    base: [0xb8, 0x1f, 0x2c],
    on_dark: [0xff, 0x6b, 0x74],
    on_light: Some([0x8d, 0x12, 0x1e]),
};

/// Super Famicom. Kept violet-grey, the colour of the console's switches and
/// of the cartridge's recessed label well.
pub const SFC: AccentStyle = AccentStyle {
    base: [0x6b, 0x5c, 0x8f],
    on_dark: [0xbb, 0xa8, 0xf0],
    on_light: Some([0x4a, 0x3d, 0x6b]),
};

/// Mega Drive / Genesis: charcoal plastic with a cool blue highlight.
pub const MD: AccentStyle = AccentStyle {
    base: [0x2b, 0x3b, 0x5f],
    on_dark: [0x8f, 0xb8, 0xff],
    on_light: Some([0x1f, 0x2d, 0x4f]),
};

pub fn platform(platform: Platform) -> AccentStyle {
    match platform {
        Platform::Gba => GBA,
        Platform::Gb => GB,
        Platform::Gbc => GBC,
        Platform::Fc => FC,
        Platform::Sfc => SFC,
        Platform::Md => MD,
    }
}

pub fn recents() -> AccentStyle {
    RECENTS
}

pub fn favorites() -> AccentStyle {
    FAVORITES
}

/// Text colour for a platform badge or collection label.
pub fn ink(style: AccentStyle, selected: bool) -> [u8; 3] {
    if selected {
        style.on_light.unwrap_or(style.on_dark)
    } else {
        style.on_dark
    }
}

/// Fill (first) and border (second) for a badge.  Dark cards get a dark tinted
/// chip; the selected white card gets a light tinted chip, mirroring the web
/// badge while keeping this project's card surfaces.
pub fn chip(style: AccentStyle, selected: bool) -> ([u8; 4], [u8; 4]) {
    let ground = if selected { sw::selected() } else { sw::surface() };
    let fill = blend(rgb(ground), style.base, if selected { 0.10 } else { 0.18 });
    ([fill[0], fill[1], fill[2], 0xff], [
        style.base[0],
        style.base[1],
        style.base[2],
        0xff,
    ])
}

fn rgb(colour: [u8; 4]) -> [u8; 3] {
    [colour[0], colour[1], colour[2]]
}

fn blend(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (a[i] as f32 * (1.0 - t) + b[i] as f32 * t).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
        fn luminance(c: [u8; 3]) -> f32 {
            let channel = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
        }
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn platform_and_collection_hues_follow_the_approved_palette() {
        assert_eq!(GBA.base, [0x20, 0x6b, 0xc4]);
        assert_eq!(GB.base, [0x2f, 0xb3, 0x44]);
        assert_eq!(GBC.base, [0x8b, 0x5c, 0xf6]);
        assert_eq!(RECENTS.base, [0xf2, 0x84, 0x5c]);
        assert_eq!(FAVORITES.base, [0xf2, 0xc1, 0x4e]);
    }

    #[test]
    fn platform_badge_text_is_readable_on_both_card_states() {
        for style in [GBA, GB, GBC] {
            let (dark_fill, _) = chip(style, false);
            let (light_fill, _) = chip(style, true);
            assert!(
                contrast(style.on_dark, rgb(dark_fill)) >= 4.5,
                "dark badge ink is too faint for {:?}",
                style.base
            );
            assert!(
                contrast(style.on_light.unwrap(), rgb(light_fill)) >= 4.5,
                "light badge ink is too faint for {:?}",
                style.base
            );
        }
    }

    #[test]
    fn system_collection_labels_are_readable_on_the_shelf() {
        for style in [RECENTS, FAVORITES] {
            assert!(
                contrast(style.on_dark, rgb(sw::surface())) >= 4.5,
                "collection shelf ink is too faint for {:?}",
                style.base
            );
        }
    }
}
