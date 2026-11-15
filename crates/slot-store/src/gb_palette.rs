/// The names the menu shows.
const NAMES: [&str; 48] = [
    "灰阶",
    "DMG 经典绿",
    "GB 口袋版",
    "GB 背光版",
    "GBC 棕色 ↑",
    "GBC 红色 ↑A",
    "GBC 深棕 ↑B",
    "GBC 淡黄 ↓",
    "GBC 橙色 ↓A",
    "GBC 黄色 ↓B",
    "GBC 蓝色 ←",
    "GBC 深蓝 ←A",
    "GBC 灰色 ←B",
    "GBC 绿色 →",
    "GBC 深绿 →A",
    "GBC 反色 →B",
    "SGB 1-A",
    "SGB 1-B",
    "SGB 1-C",
    "SGB 1-D",
    "SGB 1-E",
    "SGB 1-F",
    "SGB 1-G",
    "SGB 1-H",
    "SGB 2-A",
    "SGB 2-B",
    "SGB 2-C",
    "SGB 2-D",
    "SGB 2-E",
    "SGB 2-F",
    "SGB 2-G",
    "SGB 2-H",
    "SGB 3-A",
    "SGB 3-B",
    "SGB 3-C",
    "SGB 3-D",
    "SGB 3-E",
    "SGB 3-F",
    "SGB 3-G",
    "SGB 3-H",
    "SGB 4-A",
    "SGB 4-B",
    "SGB 4-C",
    "SGB 4-D",
    "SGB 4-E",
    "SGB 4-F",
    "SGB 4-G",
    "SGB 4-H",
];

/// The names mGBA knows the same palettes by.
///
/// The core is told one of these, so they cannot be the Chinese labels
/// above: mGBA only answers to its own names, and a name it does not know
/// leaves the palette it had. The order is mGBA's: the sixteen Game Boy and
/// Game Boy Color palettes, then the thirty-two Super Game Boy borders.
const CORE_NAMES: [&str; 48] = [
    "Grayscale",
    "DMG Green",
    "GB Pocket",
    "GB Backlight",
    "GBC Brown ↑",
    "GBC Red ↑A",
    "GBC Dark Brown ↑B",
    "GBC Light Yellow ↓",
    "GBC Orange ↓A",
    "GBC Yellow ↓B",
    "GBC Blue ←",
    "GBC Dark Blue ←A",
    "GBC Gray ←B",
    "GBC Green →",
    "GBC Dark Green →A",
    "GBC Inverted →B",
    "SGB 1-A",
    "SGB 1-B",
    "SGB 1-C",
    "SGB 1-D",
    "SGB 1-E",
    "SGB 1-F",
    "SGB 1-G",
    "SGB 1-H",
    "SGB 2-A",
    "SGB 2-B",
    "SGB 2-C",
    "SGB 2-D",
    "SGB 2-E",
    "SGB 2-F",
    "SGB 2-G",
    "SGB 2-H",
    "SGB 3-A",
    "SGB 3-B",
    "SGB 3-C",
    "SGB 3-D",
    "SGB 3-E",
    "SGB 3-F",
    "SGB 3-G",
    "SGB 3-H",
    "SGB 4-A",
    "SGB 4-B",
    "SGB 4-C",
    "SGB 4-D",
    "SGB 4-E",
    "SGB 4-F",
    "SGB 4-G",
    "SGB 4-H",
];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct GbPalette(u8);

impl GbPalette {
    pub const COUNT: usize = NAMES.len();
    pub const DEFAULT: GbPalette = GbPalette(1);

    pub fn all() -> impl Iterator<Item = GbPalette> {
        (0..Self::COUNT as u8).map(GbPalette)
    }

    pub fn at(index: usize) -> GbPalette {
        GbPalette((index % Self::COUNT) as u8)
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The name mGBA knows this palette by.
    ///
    /// This is the name the core is told, and the one the state file keeps,
    /// so a card written by upstream reads back the same.
    pub fn core_name(self) -> &'static str {
        CORE_NAMES[self.index()]
    }

    /// The name the menu shows, arrows and all.
    pub fn name(self) -> &'static str {
        NAMES[self.index()]
    }

    /// The name the menu shows, without the arrows that mark which Game Boy
    /// Color palette a name stands for.
    pub fn label(self) -> &'static str {
        let name = self.name();
        match name.find(['↑', '↓', '←', '→']) {
            Some(at) => name[..at].trim_end(),
            None => name,
        }
    }

    pub fn next(self) -> GbPalette {
        GbPalette(((self.index() + 1) % Self::COUNT) as u8)
    }

    pub fn prev(self) -> GbPalette {
        GbPalette(((self.index() + Self::COUNT - 1) % Self::COUNT) as u8)
    }

    /// The palette with this name.
    ///
    /// Both names open a palette: the Chinese one this fork shows and the
    /// mGBA one the state file keeps.
    pub fn parse(name: &str) -> Option<GbPalette> {
        let name = name.trim();
        Self::all().find(|p| p.core_name() == name || p.name() == name || p.label() == name)
    }
}
