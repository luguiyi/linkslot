use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use slot_model::{MachineId, MACHINES};

use crate::atomic::atomic_write;
use crate::gb_palette::GbPalette;
use crate::Platform;

pub const BRIGHTNESS_MAX: u8 = 9;
pub const BLUE_LIGHT_MAX: u8 = 9;
pub const VOLUME_MAX: u8 = 100;

pub const UTC_OFFSET_MIN: i16 = -720;
pub const UTC_OFFSET_MAX: i16 = 840;

pub const FF_SPEEDS: [u8; 5] = [0, 2, 3, 4, 6];

pub const FF_SPEED_DEFAULT: u8 = 6;

pub const RECENTS_MAX: usize = 10;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recent {
    pub platform: Platform,
    pub stem: String,
}

/// Which automatic resume-state behaviour a switch controls.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ResumeKind {
    /// Load `resume.state` when a cart is inserted.
    Load,
    /// Write `resume.state` on autosave / lid / power / eject.
    Save,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SlotState {
    pub show_rom_title: bool,
    pub fallback_cover: bool,
    pub show_recents: bool,
    pub lid_minutes: u8,
    pub button_layout: ButtonLayout,
    pub cart: Option<String>,
    pub cart_platform: Option<Platform>,
    pub brightness: u8,
    pub blue_light: u8,
    pub volume: u8,
    pub volume_hp: u8,
    pub muted: bool,
    pub muted_hp: bool,
    pub clock_set: bool,
    pub utc_offset_min: i16,
    pub rumble: bool,
    pub ff_speed: u8,
    pub ff_sound: bool,
    pub colour_correction: bool,
    pub shader_gba: Shader,
    pub shader_gb: Shader,
    /// Per-platform "load resume.state on insert" switches.
    ///
    /// Presence means "the player set this machine explicitly"; absence means
    /// "follow [`SlotState::resume_load_default`]". 键是稳定机种 id，所以新增机种
    /// 只需要加表项，不会再出现"读不回来的 key"。
    pub resume_load: BTreeMap<MachineId, bool>,
    /// Per-platform "automatically save resume.state" switches.
    pub resume_save: BTreeMap<MachineId, bool>,
    /// The global default the platforms without their own switch follow.
    pub resume_load_default: bool,
    pub resume_save_default: bool,
    pub turbo: bool,
    pub rewind: bool,
    pub gb_palettes: bool,
    pub gb_palette: GbPalette,
    /// Screen overlays chosen from `Overlay/<machine>/*.png`, by file name;
    /// `None` means 无. An empty name is the legacy "on" (first file).
    pub fc_mask: Option<String>,
    pub gb_mask: Option<String>,
    pub gbc_mask: Option<String>,
    pub sfc_mask: Option<String>,
    pub recents: Vec<Recent>,
    pub favorites: Vec<Recent>,
    pub shelf: Option<String>,
    pub shelf_cart: Option<Recent>,
}

impl SlotState {
    pub fn played(&mut self, platform: Platform, stem: &str) {
        self.recents
            .retain(|r| !(r.platform == platform && r.stem == stem));
        self.recents.insert(
            0,
            Recent {
                platform,
                stem: stem.to_string(),
            },
        );
        self.recents.truncate(RECENTS_MAX);
    }

    pub fn is_favorite(&self, platform: Platform, stem: &str) -> bool {
        self.favorites
            .iter()
            .any(|f| f.platform == platform && f.stem == stem)
    }

    pub fn toggle_favorite(&mut self, platform: Platform, stem: &str) -> bool {
        if self.is_favorite(platform, stem) {
            self.favorites
                .retain(|f| !(f.platform == platform && f.stem == stem));
            return false;
        }
        self.favorites.push(Recent {
            platform,
            stem: stem.to_string(),
        });
        true
    }

    /// Whether `platform` may automatically load/save its resume state.
    pub fn resume(&self, platform: Platform, kind: ResumeKind) -> bool {
        self.resume_override(platform, kind)
            .unwrap_or_else(|| self.resume_default(kind))
    }

    /// The platform's own switch: `None` means "follow the default".
    pub fn resume_override(&self, platform: Platform, kind: ResumeKind) -> Option<bool> {
        let table = match kind {
            ResumeKind::Load => &self.resume_load,
            ResumeKind::Save => &self.resume_save,
        };
        table.get(&platform.machine().id).copied()
    }

    pub fn resume_default(&self, kind: ResumeKind) -> bool {
        match kind {
            ResumeKind::Load => self.resume_load_default,
            ResumeKind::Save => self.resume_save_default,
        }
    }

    pub fn set_resume_default(&mut self, kind: ResumeKind, on: bool) {
        match kind {
            ResumeKind::Load => self.resume_load_default = on,
            ResumeKind::Save => self.resume_save_default = on,
        }
    }

    pub fn set_resume(&mut self, platform: Platform, kind: ResumeKind, on: Option<bool>) {
        let table = match kind {
            ResumeKind::Load => &mut self.resume_load,
            ResumeKind::Save => &mut self.resume_save,
        };
        match on {
            Some(on) => {
                table.insert(platform.machine().id, on);
            }
            None => {
                table.remove(&platform.machine().id);
            }
        }
    }

    /// Cycle one platform's switch: follow the default, on, off, back.
    pub fn toggle_resume(&mut self, platform: Platform, kind: ResumeKind) {
        let next = match self.resume_override(platform, kind) {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.set_resume(platform, kind, next);
    }

    pub fn toggle_resume_default(&mut self, kind: ResumeKind) {
        self.set_resume_default(kind, !self.resume_default(kind));
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ButtonLayout {
    #[default]
    Off,
    SwapShoulders,
    XyShoulders,
    Both,
}
impl ButtonLayout {
    pub fn name(self) -> &'static str {
        match self { Self::Off => "off", Self::SwapShoulders => "swap-shoulders", Self::XyShoulders => "xy-shoulders", Self::Both => "both" }
    }
    pub fn parse(s: &str) -> Self {
        match s { "swap-shoulders" => Self::SwapShoulders, "xy-shoulders" => Self::XyShoulders, "both" => Self::Both, _ => Self::Off }
    }
    pub fn uses_xy(self) -> bool { matches!(self, Self::XyShoulders | Self::Both) }
    pub fn step(self, right: bool) -> Self {
        let all = [Self::Off, Self::SwapShoulders, Self::XyShoulders, Self::Both];
        all[(self as usize + if right { 1 } else { all.len() - 1 }) % all.len()]
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shader {
    Off,
    Lcd3x,
    Grid,
    Dot,
    Simpletex,
    FourPass,
}

impl Shader {
    pub const GBA: [Shader; 5] = [Shader::Off, Shader::Lcd3x, Shader::Grid, Shader::Dot, Shader::FourPass];
    pub const GB: [Shader; 4] = [Shader::Off, Shader::Grid, Shader::Simpletex, Shader::FourPass];

    pub fn name(self) -> &'static str {
        match self {
            Shader::Off => "off",
            Shader::Lcd3x => "lcd3x",
            Shader::Grid => "grid",
            Shader::Dot => "dot",
            Shader::Simpletex => "simpletex",
            Shader::FourPass => "four-pass",
        }
    }

    pub fn parse(name: &str) -> Option<Shader> {
        [
            Shader::Off,
            Shader::Lcd3x,
            Shader::Grid,
            Shader::Dot,
            Shader::Simpletex,
            Shader::FourPass,
        ]
        .into_iter()
        .find(|s| s.name() == name)
    }

    pub fn step(self, choices: &[Shader], right: bool) -> Shader {
        let n = choices.len();
        let at = choices.iter().position(|&s| s == self).unwrap_or(0);
        choices[if right {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }]
    }
}

impl Default for SlotState {
    fn default() -> Self {
        SlotState {
            show_rom_title: true,
            fallback_cover: false,
            show_recents: false,
            lid_minutes: 3,
            button_layout: ButtonLayout::Off,
            cart: None,
            cart_platform: None,
            brightness: 5,
            blue_light: 0,
            volume: 60,
            volume_hp: 60,
            muted: false,
            muted_hp: false,
            clock_set: false,
            utc_offset_min: 480,
            rumble: true,
            ff_speed: FF_SPEED_DEFAULT,
            ff_sound: false,
            colour_correction: false,
            shader_gba: Shader::Grid,
            shader_gb: Shader::Simpletex,
            resume_load: BTreeMap::new(),
            resume_save: BTreeMap::new(),
            resume_load_default: true,
            resume_save_default: true,
            turbo: false,
            rewind: true,
            gb_palettes: false,
            gb_palette: GbPalette::DEFAULT,
            fc_mask: None,
            gb_mask: None,
            gbc_mask: None,
            sfc_mask: None,
            recents: Vec::new(),
            favorites: Vec::new(),
            shelf: None,
            shelf_cart: None,
        }
    }
}

fn state_path(root: &Path) -> PathBuf {
    root.join("linkSlot/Config").join("slot.state")
}

const RECENTS_FILE: &str = "recents.txt";
const FAVORITES_FILE: &str = "favorites.txt";

fn list_path(root: &Path, name: &str) -> PathBuf {
    root.join("linkSlot/Config").join(name)
}

pub fn read_slot_state(root: &Path) -> SlotState {
    let mut s: SlotState = std::fs::read(state_path(root))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| parse(&s))
        .unwrap_or_default();
    s.recents = read_list(root, RECENTS_FILE);
    s.recents.truncate(RECENTS_MAX);
    s.favorites = read_list(root, FAVORITES_FILE);
    s
}

fn read_list(root: &Path, name: &str) -> Vec<Recent> {
    let text = std::fs::read_to_string(list_path(root, name)).unwrap_or_default();
    let mut list: Vec<Recent> = Vec::new();
    for r in text.lines().filter_map(recent_value) {
        if !list.contains(&r) {
            list.push(r);
        }
    }
    list
}

fn write_list(root: &Path, name: &str, list: &[Recent]) -> std::io::Result<()> {
    let path = list_path(root, name);
    if list.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
    }
    let text: String = list
        .iter()
        .map(|r| format!("{}/{}\n", platform_key(r.platform), r.stem))
        .collect();
    if std::fs::read(&path).is_ok_and(|old| old == text.as_bytes()) {
        return Ok(());
    }
    atomic_write(&path, text.as_bytes())
}

pub fn write_slot_state(root: &Path, s: &SlotState) -> std::io::Result<()> {
    let mut text = format!(
        "cart={}\ncart_platform={}\nbrightness={}\nblue_light={}\nvolume={}\nvolume_hp={}\nmuted={}\nmuted_hp={}\nclock_set={}\nutc_offset_min={}\nutc_offset_initialized=1\nrumble={}\nff_speed={}\nff_sound={}\ncolour_correction={}\nshader_gba={}\nshader_gb={}\nturbo_enabled={}\nrewind={}\ngb_palettes={}\ngb_palette={}\nfc_mask={}\ngb_mask={}\ngbc_mask={}\nsfc_mask={}\nbutton_layout={}\nshow_rom_title={}\nfallback_cover={}\nlid_minutes={}\nresume_load_default={}\nresume_save_default={}\n",
        s.cart.as_deref().unwrap_or(""),
        s.cart_platform.map_or(String::new(), platform_key),
        s.brightness,
        s.blue_light,
        s.volume,
        s.volume_hp,
        s.muted as u8,
        s.muted_hp as u8,
        s.clock_set as u8,
        s.utc_offset_min,
        s.rumble as u8,
        s.ff_speed,
        s.ff_sound as u8,
        s.colour_correction as u8,
        s.shader_gba.name(),
        s.shader_gb.name(),
        s.turbo as u8,
        s.rewind as u8,
        s.gb_palettes as u8,
        // The card keeps the name the menu shows; the core is told mGBA's
        // own name for the same palette.
        s.gb_palette.name(),
        fc_mask_line(s.fc_mask.as_deref()),
        fc_mask_line(s.gb_mask.as_deref()),
        fc_mask_line(s.gbc_mask.as_deref()),
        fc_mask_line(s.sfc_mask.as_deref()),
        s.button_layout.name(),
        s.show_rom_title as u8,
        s.fallback_cover as u8,
        s.lid_minutes,
        s.resume_load_default as u8,
        s.resume_save_default as u8
    );
    // 每机种开关写成一行 CSV（`gba:1,fc:0`），只有玩家显式设置过的机种才会出现；
    // 没有出现的机种跟随 `resume_*_default`。旧版本的前端读不懂这两个 key，
    // 会忽略它们并沿用默认值（与旧格式的 `default` 语义一致）。
    text.push_str(&format!("auto_load={}\n", resume_csv(&s.resume_load)));
    text.push_str(&format!("auto_save={}\n", resume_csv(&s.resume_save)));
    text.push_str(&format!("show_recents={}\n", s.show_recents as u8));
    text.push_str(&format!(
        "shelf={}\n",
        s.shelf.as_deref().unwrap_or("")
    ));
    text.push_str(&format!(
        "shelf_cart={}\n",
        s.shelf_cart.as_ref().map_or(String::new(), |r| format!(
            "{}/{}",
            platform_key(r.platform),
            r.stem
        ))
    ));
    atomic_write(&state_path(root), text.as_bytes())?;
    write_list(root, RECENTS_FILE, &s.recents)?;
    write_list(root, FAVORITES_FILE, &s.favorites)
}

fn parse(text: &str) -> Option<SlotState> {
    let mut cart = None;
    let mut cart_platform = None;
    let mut brightness = None;
    let mut blue_light = None;
    let mut volume = None;
    let mut volume_hp = None;
    let mut muted = None;
    let mut muted_hp = None;
    let mut clock_set = None;
    let mut utc_offset_min = None;
    let mut utc_offset_initialized = false;
    let mut rumble = None;
    let mut ff_speed = None;
    let mut ff_sound = None;
    let mut colour_correction = None;
    let mut shader_gba = None;
    let mut shader_gb = None;
    let mut resume_load: BTreeMap<MachineId, bool> = BTreeMap::new();
    let mut resume_save: BTreeMap<MachineId, bool> = BTreeMap::new();
    let mut resume_load_default: Option<bool> = None;
    let mut resume_save_default: Option<bool> = None;
    let mut turbo = None;
    let mut button_layout = ButtonLayout::Off;
    let mut show_rom_title = true;
    let mut fallback_cover = false;
    let mut show_recents = None;
    let mut shelf = None;
    let mut shelf_cart = None;
    let mut lid_minutes = 3;
    let mut rewind = None;
    let mut gb_palettes = None;
    let mut gb_palette = None;
    let mut fc_mask: Option<Option<String>> = None;
    let mut gb_mask: Option<Option<String>> = None;
    let mut gbc_mask: Option<Option<String>> = None;
    let mut sfc_mask: Option<Option<String>> = None;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // 新格式：`auto_load=gba:1,fc:0`（只写显式设置过的机种）。
        if key == "auto_load" {
            resume_load = parse_resume(value);
            continue;
        }
        if key == "auto_save" {
            resume_save = parse_resume(value);
            continue;
        }
        // 旧格式：`auto_load_gba=1` / `auto_save_gba=0`，逐机种读取以兼容老卡。
        if let Some(rest) = key.strip_prefix("auto_load_") {
            if let Some(machine) = machine_by_key(rest) {
                if let Some(on) = flag(value) {
                    resume_load.insert(machine.id, on);
                }
            }
            continue;
        }
        if let Some(rest) = key.strip_prefix("auto_save_") {
            if let Some(machine) = machine_by_key(rest) {
                if let Some(on) = flag(value) {
                    resume_save.insert(machine.id, on);
                }
            }
            continue;
        }
        match key {
            "cart" => cart = Some(value.to_string()),
            "cart_platform" => cart_platform = platform_value(value),
            "brightness" => brightness = Some(level(value, BRIGHTNESS_MAX)?),
            "blue_light" => blue_light = Some(level(value, BLUE_LIGHT_MAX)?),
            "volume" => volume = Some(level(value, VOLUME_MAX)?),
            "volume_hp" => volume_hp = Some(level(value, VOLUME_MAX)?),
            "muted" => muted = Some(level(value, 1)? == 1),
            "muted_hp" => muted_hp = Some(level(value, 1)? == 1),
            "clock_set" => clock_set = Some(level(value, 1)? == 1),
            "utc_offset_min" => utc_offset_min = Some(offset(value)?),
            "utc_offset_initialized" => utc_offset_initialized = flag(value).unwrap_or(false),
            "rumble" => rumble = flag(value),
            "ff_speed" => ff_speed = ff_speed_value(value),
            "ff_sound" => ff_sound = flag(value),
            "colour_correction" => colour_correction = flag(value),
            "shader_gba" => shader_gba = Shader::parse(value).filter(|s| Shader::GBA.contains(s)),
            "resume_load_default" => resume_load_default = flag(value),
            "resume_save_default" => resume_save_default = flag(value),
            "turbo_enabled" => turbo = flag(value),
            // Older state files wrote `turbo=1` by default.  Do not read that
            // key any more: v0.0.7 starts with button turbo off for everyone.
            "turbo" => {}
            "lid_minutes" => lid_minutes = value.parse::<u8>().ok().filter(|v| [0,3,10,30].contains(v)).unwrap_or(3),
            "show_rom_title" => show_rom_title = flag(value).unwrap_or(true),
            "fallback_cover" => fallback_cover = flag(value).unwrap_or(false),
            "show_recents" => show_recents = flag(value),
            "shelf" => shelf = Some(value.to_string()).filter(|v| !v.is_empty()),
            "shelf_cart" => shelf_cart = recent_value(value),
            "button_layout" => button_layout = ButtonLayout::parse(value),
            "rewind" => rewind = flag(value),
            "gb_palettes" => gb_palettes = flag(value),
            "gb_palette" => gb_palette = GbPalette::parse(value),
            "fc_mask" => fc_mask = fc_mask_value(value),
            "gb_mask" => gb_mask = fc_mask_value(value),
            "gbc_mask" => gbc_mask = fc_mask_value(value),
            "sfc_mask" => sfc_mask = fc_mask_value(value),
            "shader_gb" => shader_gb = Shader::parse(value).filter(|s| Shader::GB.contains(s)),
            _ => {}
        }
    }
    let cart = cart?;
    let fallback = SlotState::default();
    let new_format = resume_load_default.is_some() || resume_save_default.is_some();
    let default_load = resume_load_default.unwrap_or(true);
    let default_save = resume_save_default.unwrap_or(true);
    // A file from before the defaults existed stores `1` for the old hard
    // default (on): that is "follow the default", not an explicit choice, so a
    // later default change can reach it. An explicit `0` is an off the player
    // asked for and is preserved.
    let migrate = |table: BTreeMap<MachineId, bool>| -> BTreeMap<MachineId, bool> {
        if new_format {
            return table;
        }
        table.into_iter().filter(|(_, on)| !*on).collect()
    };
    let mut utc_offset_min = utc_offset_min?;
    if !utc_offset_initialized && utc_offset_min == 0 {
        // State written before the Beijing default existed did not record
        // whether UTC+0 was intentional.  Treat the old implicit zero as the
        // new default; a later write adds the marker, so an explicit UTC+0
        // survives from then on.
        utc_offset_min = 480;
    }
    Some(SlotState {
        show_rom_title,
        fallback_cover,
        show_recents: show_recents.unwrap_or(fallback.show_recents),
        lid_minutes,
        button_layout,
        cart: (!cart.is_empty()).then_some(cart),
        cart_platform,
        brightness: brightness?,
        blue_light: blue_light?,
        volume: volume?,
        volume_hp: volume_hp.or(volume)?,
        muted: muted?,
        muted_hp: muted_hp.or(muted)?,
        clock_set: clock_set?,
        utc_offset_min,
        rumble: rumble.unwrap_or(fallback.rumble),
        ff_speed: ff_speed.unwrap_or(fallback.ff_speed),
        ff_sound: ff_sound.unwrap_or(fallback.ff_sound),
        colour_correction: colour_correction.unwrap_or(fallback.colour_correction),
        shader_gba: shader_gba.unwrap_or(fallback.shader_gba),
        shader_gb: shader_gb.unwrap_or(fallback.shader_gb),
        resume_load: migrate(resume_load),
        resume_save: migrate(resume_save),
        resume_load_default: default_load,
        resume_save_default: default_save,
        turbo: !button_layout.uses_xy() && turbo.unwrap_or(fallback.turbo),
        rewind: rewind.unwrap_or(fallback.rewind),
        gb_palettes: gb_palettes.unwrap_or(fallback.gb_palettes),
        gb_palette: gb_palette.unwrap_or(fallback.gb_palette),
        fc_mask: fc_mask.unwrap_or(None),
        gb_mask: gb_mask.unwrap_or(None),
        gbc_mask: gbc_mask.unwrap_or(None),
        sfc_mask: sfc_mask.unwrap_or(None),
        recents: Vec::new(),
        favorites: Vec::new(),
        shelf,
        shelf_cart,
    })
}

/// The state line for the FC mask: "none", the legacy "1" (first file), or
/// the chosen file name.
fn fc_mask_line(value: Option<&str>) -> String {
    match value {
        None => "none".to_string(),
        Some(name) if name.is_empty() => "1".to_string(),
        Some(name) => name.to_string(),
    }
}

/// Parse the FC mask line; the outer Option is "the key was there".
fn fc_mask_value(value: &str) -> Option<Option<String>> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    if value == "1" || value.eq_ignore_ascii_case("true") {
        return Some(Some(String::new()));
    }
    Some(Some(value.to_string()))
}
fn platform_key(platform: Platform) -> String {
    platform.dir_name().to_ascii_lowercase()
}

/// 按目录名查机种（大小写无关），用于解析 `auto_load_<dir>` / CSV 项。
fn machine_by_key(key: &str) -> Option<&'static slot_model::Machine> {
    MACHINES.iter().find(|m| m.dir.eq_ignore_ascii_case(key))
}

/// 把每机种开关写成 `gba:1,fc:0`；未出现的机种跟随全局默认。
fn resume_csv(table: &BTreeMap<MachineId, bool>) -> String {
    table
        .iter()
        .map(|(id, on)| format!("{}:{}", id.0.to_ascii_lowercase(), u8::from(*on)))
        .collect::<Vec<_>>()
        .join(",")
}

/// 解析 `gba:1,fc:0`；未知机种或非法值整项跳过（不毁掉其余设置）。
fn parse_resume(value: &str) -> BTreeMap<MachineId, bool> {
    let mut table = BTreeMap::new();
    for item in value.split(',').filter(|s| !s.is_empty()) {
        let Some((dir, raw)) = item.split_once(':') else {
            continue;
        };
        let Some(machine) = machine_by_key(dir) else {
            continue;
        };
        if let Some(on) = flag(raw) {
            table.insert(machine.id, on);
        }
    }
    table
}

fn recent_value(value: &str) -> Option<Recent> {
    let (platform, stem) = value.split_once('/')?;
    Some(Recent {
        platform: platform_value(platform)?,
        stem: stem.to_string(),
    })
    .filter(|r| !r.stem.is_empty())
}

fn platform_value(value: &str) -> Option<Platform> {
    Platform::ALL
        .into_iter()
        .find(|p| value.eq_ignore_ascii_case(p.dir_name()))
}

fn offset(value: &str) -> Option<i16> {
    value
        .parse()
        .ok()
        .filter(|n| (UTC_OFFSET_MIN..=UTC_OFFSET_MAX).contains(n))
}

fn ff_speed_value(value: &str) -> Option<u8> {
    value.parse().ok().filter(|n| FF_SPEEDS.contains(n))
}

fn level(value: &str, max: u8) -> Option<u8> {
    value.parse().ok().filter(|n| *n <= max)
}

fn flag(value: &str) -> Option<bool> {
    level(value, 1).map(|n| n == 1)
}
