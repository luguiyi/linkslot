//! The linkSlot mod phases a Lua mod can run at.

use serde::{Deserialize, Serialize};

/// Where a mod runs relative to a state/sav unit transfer, a whole sync, or
/// device startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    BeforeUpload,
    AfterUpload,
    BeforeDownload,
    AfterDownload,
    /// Runs once after the whole `sync` command, even when no unit transferred.
    AfterSync,
    /// Runs once after system startup at the action's `boot_delay_minutes`.
    /// It has no game/unit context and never runs during a sync.
    AfterBoot,
    /// Runs when the user long-presses Y on a cartridge: `linkSlot mods run`.
    /// Manual actions never run as part of a cloud sync.
    Manual,
}

impl Timing {
    pub const ALL: [Timing; 7] = [
        Timing::BeforeUpload,
        Timing::AfterUpload,
        Timing::BeforeDownload,
        Timing::AfterDownload,
        Timing::AfterSync,
        Timing::AfterBoot,
        Timing::Manual,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Timing::BeforeUpload => "before_upload",
            Timing::AfterUpload => "after_upload",
            Timing::BeforeDownload => "before_download",
            Timing::AfterDownload => "after_download",
            Timing::AfterSync => "after_sync",
            Timing::AfterBoot => "after_boot",
            Timing::Manual => "manual",
        }
    }

    pub fn parse(text: &str) -> Option<Timing> {
        match text.trim() {
            "before_upload" => Some(Timing::BeforeUpload),
            "after_upload" => Some(Timing::AfterUpload),
            "before_download" => Some(Timing::BeforeDownload),
            "after_download" => Some(Timing::AfterDownload),
            "after_sync" => Some(Timing::AfterSync),
            "after_boot" => Some(Timing::AfterBoot),
            "manual" => Some(Timing::Manual),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Timing::BeforeUpload => "上传前",
            Timing::AfterUpload => "上传后",
            Timing::BeforeDownload => "下载前",
            Timing::AfterDownload => "下载后",
            Timing::AfterSync => "同步完成后",
            Timing::AfterBoot => "开机后",
            Timing::Manual => "手动（长按 Y）",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_timing() {
        for timing in Timing::ALL {
            assert_eq!(Timing::parse(timing.as_str()), Some(timing));
        }
        assert_eq!(Timing::parse(" upload "), None);
        assert_eq!(Timing::parse("before-upload"), None);
    }

    #[test]
    fn labels_are_chinese() {
        assert_eq!(Timing::BeforeUpload.label(), "上传前");
        assert_eq!(Timing::AfterDownload.label(), "下载后");
        assert_eq!(Timing::AfterSync.label(), "同步完成后");
        assert_eq!(Timing::AfterBoot.label(), "开机后");
        assert_eq!(Timing::Manual.label(), "手动（长按 Y）");
    }

    #[test]
    fn manual_is_parsed_and_serialized_as_manual() {
        assert_eq!(Timing::parse("manual"), Some(Timing::Manual));
        assert_eq!(Timing::Manual.as_str(), "manual");
        assert!(Timing::ALL.contains(&Timing::Manual));
    }

    #[test]
    fn after_boot_is_a_distinct_non_unit_timing() {
        assert_eq!(Timing::parse("after_boot"), Some(Timing::AfterBoot));
        assert_eq!(Timing::AfterBoot.as_str(), "after_boot");
        assert_eq!(Timing::AfterBoot.label(), "开机后");
        assert!(Timing::ALL.contains(&Timing::AfterBoot));
    }
}
