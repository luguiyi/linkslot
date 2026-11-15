mod atomic;
pub mod bindings;
pub mod cart_color;
pub mod cart_meta;
pub mod cart_shell;
mod cart_model;
pub mod fronts;
mod config;
mod core;
pub mod gb;
mod gb_palette;
mod gba;
pub mod ini;
pub mod language;
pub mod pinyin;
mod ring;
mod scan;
mod slot_state;
mod stamp;
mod theme;
pub mod title_search;

pub use atomic::atomic_write;
pub use bindings::{
    read_carousel, read_controls, read_shortcuts, write_carousel, write_controls, write_shortcuts,
    Binding, CarouselAction, CarouselKeys, Controls, Device, GameButton, Modifier, Shortcut,
    ShortcutKeys,
};
pub use cart_shell::{Outline, ShellChoice, ShellFinish, CART_SHELL_FILE, LABELS_SHELL_FILE};
pub use cart_model::CartModel;
pub use config::{move_config, CONFIG_DIR};
pub use core::{
    available_options, core_for, core_for_platform, read_selected_cores, write_selected_core, Core, SELECTED_CORE_FILE,
};
pub use gb_palette::GbPalette;
pub use gba::{header_clean, header_code, header_title, header_title_code};
pub use language::{
    language_mtime, language_path, read_language, read_language_code, write_language, LANGUAGE_FILE,
};
pub use slot_model::{Machine, MachineId, Platform};
pub use ring::{StateEntry, StateRing, RING_MAX};
pub use scan::{initial, is_hidden, listing, scan, scan_quiet, sort_key, Cart, StoreError};
pub use slot_state::{
    read_slot_state, write_slot_state, ButtonLayout, Recent, ResumeKind, Shader, SlotState,
    BLUE_LIGHT_MAX, BRIGHTNESS_MAX, FF_SPEEDS, FF_SPEED_DEFAULT, RECENTS_MAX, UTC_OFFSET_MAX,
    UTC_OFFSET_MIN, VOLUME_MAX,
};
pub use stamp::{
    civil_from_days, days_from_civil, days_in_month, format_stamp, parse_stamp, stamp_now,
};
pub use theme::{Theme, ThemePreset, THEME_FILE};

pub mod cheats;
