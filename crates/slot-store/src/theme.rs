use std::path::Path;

pub const THEME_FILE: &str = "theme.txt";

/// Three built-in palettes.  The old four slot colours are still accepted as
/// per-file overrides for compatibility.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ThemePreset {
    #[default]
    Default,
    Web,
    Warm,
}

impl ThemePreset {
    pub const ALL: [ThemePreset; 3] = [Self::Default, Self::Web, Self::Warm];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "默认深色",
            Self::Web => "网络青",
            Self::Warm => "复古暖色",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Web => "web",
            Self::Warm => "warm",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "dark" => Some(Self::Default),
            "web" | "teal" | "cyan" => Some(Self::Web),
            "warm" | "retro" | "amber" => Some(Self::Warm),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Default => 0,
            Self::Web => 1,
            Self::Warm => 2,
        }
    }

    pub fn at(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    pub fn next(self) -> Self {
        Self::at((self.index() + 1) % Self::ALL.len())
    }

    pub fn prev(self) -> Self {
        Self::at((self.index() + Self::ALL.len() - 1) % Self::ALL.len())
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Theme {
    pub preset: ThemePreset,
    // The cartridge slot / chassis (existing fields).
    pub housing: [u8; 3],
    pub recess: [u8; 3],
    pub opening: [u8; 3],
    pub edge: [u8; 3],
    // Shared UI palette (v0.6.13).
    pub bg: [u8; 4],
    pub bg_top: [u8; 4],
    pub surface: [u8; 4],
    pub surface_border: [u8; 4],
    pub selected: [u8; 4],
    pub selected_border: [u8; 4],
    pub ink: [u8; 3],
    pub ink_selected: [u8; 3],
    pub ink_dim: [u8; 3],
    pub accent: [u8; 3],
    pub accent_dim: [u8; 3],
}

impl Default for Theme {
    fn default() -> Self {
        Self::preset(ThemePreset::Default)
    }
}

impl Theme {
    pub fn preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::Default => Theme {
                preset,
                housing: [0x24, 0x24, 0x29],
                recess: [0x1a, 0x1a, 0x1d],
                opening: [0x05, 0x05, 0x08],
                edge: [0x4d, 0x4d, 0x57],
                bg: [0x1b, 0x1b, 0x1f, 0xff],
                bg_top: [0x24, 0x24, 0x29, 0xff],
                surface: [0x2d, 0x2d, 0x33, 0xff],
                surface_border: [0x4a, 0x4a, 0x54, 0xff],
                selected: [0xf2, 0xf2, 0xf2, 0xff],
                selected_border: [0xff, 0xff, 0xff, 0xff],
                ink: [0xf4, 0xf4, 0xf4],
                ink_selected: [0x1a, 0x1a, 0x1a],
                ink_dim: [0x9a, 0x9a, 0xa4],
                accent: [0xe6, 0x00, 0x12],
                accent_dim: [0x8a, 0x12, 0x1a],
            },
            ThemePreset::Web => Theme {
                preset,
                housing: [0x24, 0x24, 0x38],
                recess: [0x1a, 0x1a, 0x2e],
                opening: [0x0b, 0x0b, 0x18],
                edge: [0x00, 0x8f, 0x7a],
                bg: [0x1a, 0x1a, 0x2e, 0xff],
                bg_top: [0x24, 0x24, 0x38, 0xff],
                surface: [0x2a, 0x2a, 0x40, 0xff],
                surface_border: [0x3a, 0x3a, 0x55, 0xff],
                selected: [0x00, 0xd4, 0xaa, 0xff],
                selected_border: [0x78, 0xff, 0xe6, 0xff],
                ink: [0xe8, 0xe8, 0xf0],
                ink_selected: [0x10, 0x1c, 0x26],
                ink_dim: [0x88, 0x88, 0xa8],
                accent: [0x00, 0xd4, 0xaa],
                accent_dim: [0x00, 0x8f, 0x7a],
            },
            ThemePreset::Warm => Theme {
                preset,
                housing: [0x3b, 0x2b, 0x20],
                recess: [0x24, 0x18, 0x0f],
                opening: [0x0a, 0x06, 0x03],
                edge: [0x9a, 0x73, 0x52],
                bg: [0x1f, 0x17, 0x12, 0xff],
                bg_top: [0x2b, 0x21, 0x19, 0xff],
                surface: [0x34, 0x27, 0x1d, 0xff],
                surface_border: [0x5d, 0x49, 0x37, 0xff],
                selected: [0xf4, 0xe5, 0xd2, 0xff],
                selected_border: [0xff, 0xf1, 0xdc, 0xff],
                ink: [0xf6, 0xea, 0xd9],
                ink_selected: [0x26, 0x18, 0x0f],
                ink_dim: [0xb3, 0x9a, 0x80],
                accent: [0xe0, 0x8a, 0x4a],
                accent_dim: [0x8a, 0x4c, 0x2d],
            },
        }
    }

    pub fn read(root: &Path) -> Self {
        match std::fs::read_to_string(root.join("linkSlot/Config").join(THEME_FILE)) {
            Ok(text) => Self::parse(&text),
            Err(_) => Theme::default(),
        }
    }

    pub fn write_preset(root: &Path, preset: ThemePreset) -> std::io::Result<()> {
        let dir = root.join("linkSlot/Config");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(THEME_FILE),
            format!("preset {}\n", preset.as_str()),
        )
    }

    pub fn parse(text: &str) -> Self {
        let mut preset = ThemePreset::Default;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                if parts.next().is_none()
                    && matches!(name.to_ascii_lowercase().as_str(), "preset" | "theme")
                {
                    if let Some(found) = ThemePreset::parse(value) {
                        preset = found;
                    }
                }
            }
        }
        let mut theme = Theme::preset(preset);
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
                continue;
            };
            if parts.next().is_some() {
                continue;
            }
            let Some(rgb) = hex(value) else {
                continue;
            };
            match name.to_ascii_lowercase().as_str() {
                "housing" => theme.housing = rgb,
                "recess" => theme.recess = rgb,
                "opening" => theme.opening = rgb,
                "edge" => theme.edge = rgb,
                "bg" => theme.bg = rgba(rgb),
                "bg_top" => theme.bg_top = rgba(rgb),
                "surface" => theme.surface = rgba(rgb),
                "surface_border" => theme.surface_border = rgba(rgb),
                "selected" => theme.selected = rgba(rgb),
                "selected_border" => theme.selected_border = rgba(rgb),
                "ink" => theme.ink = rgb,
                "ink_selected" => theme.ink_selected = rgb,
                "ink_dim" => theme.ink_dim = rgb,
                "accent" => theme.accent = rgb,
                "accent_dim" => theme.accent_dim = rgb,
                _ => {}
            }
        }
        theme
    }
}

fn rgba(rgb: [u8; 3]) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], 0xff]
}

fn hex(value: &str) -> Option<[u8; 3]> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}
