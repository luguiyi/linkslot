use std::path::Path;

use slot_gfx::WHOLE_TEXTURE;
use slot_store::{ini, Platform};

pub const VIDEO_MODE_FILE: &str = "linkSlot/Config/video_mode.txt";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VideoMode {
    #[default]
    Actual,
    /// GB/GBC: crop the padded canvas to the picture and stretch it to fill.
    /// FC/SFC/MD: fill the height with the source aspect, keeping side bars
    /// (L1).
    Stretch,
}

impl VideoMode {
    pub fn as_str(self) -> &'static str {
        match self {
            VideoMode::Actual => "actual",
            VideoMode::Stretch => "stretch",
        }
    }

    pub fn parse(s: &str) -> Option<VideoMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "actual" => Some(VideoMode::Actual),
            "stretch" => Some(VideoMode::Stretch),
            _ => None,
        }
    }
}

pub fn video_mode_for(root: &Path, stem: &str) -> VideoMode {
    ini::value(root, VIDEO_MODE_FILE, stem)
        .as_deref()
        .and_then(VideoMode::parse)
        .unwrap_or_default()
}

pub fn write_video_mode(root: &Path, stem: &str, mode: VideoMode) -> std::io::Result<()> {
    ini::write(root, VIDEO_MODE_FILE, stem, mode.as_str())
}

pub fn source_rect(platform: Platform, mode: VideoMode) -> [f32; 4] {
    // The canvas the core hands over, not a fixed 240x160: an FC frame is
    // 256x224 and its picture fills it, so the two rects coincide there and
    // "stretch" selects the same source; the fit is done by the destination
    // rect instead (see `App::picture_fit`).
    let (fw, fh) = platform.frame();
    let (w, h) = platform.picture();
    if mode == VideoMode::Actual || (w, h) == (fw, fh) {
        return WHOLE_TEXTURE;
    }
    let x = fw.saturating_sub(w) / 2;
    let y = fh.saturating_sub(h) / 2;
    [
        x as f32 / fw as f32,
        y as f32 / fh as f32,
        w as f32 / fw as f32,
        h as f32 / fh as f32,
    ]
}
