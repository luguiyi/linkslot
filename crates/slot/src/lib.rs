pub mod app;
pub mod collection;
pub mod intro_video;
pub mod tianma;
pub mod theme_pack;
pub mod audio;
pub mod bezel;
pub mod boot_mods;
pub mod boot_time;
pub mod bootlogo;
pub mod build_info;
pub mod cable;
pub mod cart_faces;
pub mod cart_transfer;
pub mod core;
pub mod core_picker;
pub mod diag;
pub mod epub;
#[cfg(unix)]
pub mod crash;
#[cfg(not(unix))]
pub mod crash {
    /// Crash handler is a Unix signal trampoline; other hosts have none.
    pub fn install() {}
}
pub mod drc;
pub mod emu;
pub mod face_builder;
pub mod frames;
pub mod frontend;
pub mod game_search;
pub mod gbastation;
pub mod input;
pub mod label_cache;
pub mod latency;
pub mod link_art_builder;
pub mod link_kind;
mod link_log;
pub mod link_net;
pub mod link_radio;
pub mod network_time;
pub mod link_screen;
pub mod link_start;
pub mod link_state;
pub mod linkcfg;
pub mod persist;
pub mod playtime;
pub mod prefetch;
pub mod resample;
pub mod rewind;
pub mod root;
mod rom_index;
pub mod rom_id;
pub mod session;
pub mod state_writer;
pub mod storage_test;
pub mod sync_job;
pub mod thumb;
pub mod upgrade;
pub mod video_mode;
pub mod wallpaper;
pub mod web;
pub mod wifi;
pub mod wifi_job;
pub mod wifi_power;
