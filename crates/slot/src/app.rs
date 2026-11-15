use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use slot_gfx::{OUT_H, OUT_W};
use slot_i18n::Language;
use slot_input::{Action, Btn, MUTE_CHORD_MS};
use slot_power::{Battery, Charge, LedState, LidPolicy, Power, RepairResult};
use slot_retro::LinkChannel;
use slot_store::{
    format_stamp, read_slot_state, scan, write_slot_state, Binding, CarouselAction, CarouselKeys,
    Cart, Controls, Core, Device, GbPalette, GameButton, Modifier, Platform, Recent, ResumeKind, Shader,
    Shortcut, ShortcutKeys, SlotState, StateEntry, StateRing, Theme, ThemePreset, BLUE_LIGHT_MAX,
    BRIGHTNESS_MAX, FF_SPEEDS, RING_MAX, VOLUME_MAX,
};
use slot_ui::{
    board_from_for, board_zoom, draw_backdrop, draw_empty_slot_with, draw_shelf_legend, draw_slot_name,
    draw_about, ease, grown, lid_at, lid_from_for, lift_of, on_board_for, shelf_cart_at, ClockPicker,
    Draw, FfState, GbShell, Hud, HudKind, Icon, LinkBadge, Millis, Placed, Polaroids, PowerChoice,
    QuickMenu, QuickMenuFaces, QuickRow, QuickValue, Refusal, Shelf, SlotChrome, StatusBar, TexId,
    Toast, BOARD_W, BOARD_X, CART_W, CHIP_H, CHIP_U, CHIP_V, CHIP_W, HINT_EDGE, HINT_H, HOP_LIFT,
    SHADOW_H, SHADOW_W, SOCKET_H, SOCKET_U, SOCKET_V, SOCKET_W, TURN_PAD,
};

use crate::audio::Sfx;

/// Cartridge scan started while the GL context is still being created.
static PREFETCH_SCAN: OnceLock<Mutex<Option<(PathBuf, std::thread::JoinHandle<Vec<Cart>>)>>> =
    OnceLock::new();
use crate::boot_mods::{BootJob, BootSchedule};
use crate::cart_transfer::{Stage as TransferStage, Transfer};
use crate::core_picker::{Chip, CorePicker, Outcome, Press, CLOSE_MS, OPEN_MS};
use crate::link_kind::{link_carried, link_kind, serial_option, LinkKind};
use crate::link_radio::{radio_jobs, LinkRole, RadioJob, RadioJobs};
use crate::link_screen::LinkSprites;
use crate::link_start::{link_port, LinkFail, LinkProgress, LinkStarter, LinkStep};
use crate::linkcfg;
use crate::rom_id::{self, Digest};
use crate::persist::{self, Snapshot};
use crate::rom_index::RomIndex;
use crate::sync_job::{
    ConflictInfo, Direction, ExpectedVersions, Resolve, ResolveSpec, SyncLauncher, SyncOutcome,
    SyncRequest, SyncRequestMode, SyncResult, SyncRun, SyncStage, SyncUpdate, SystemLauncher,
    UnitKind, VersionInfo, WifiWarmup, SYNC_HOLD_MS, SYNC_RESULT_MS, SYNC_TIMEOUT_MS,
};
use crate::video_mode::{self, VideoMode};
use crate::web::WebServer;
use crate::wifi_job::{self, WifiJob, WifiMsg, WifiStatus};

/// Retry a failed automatic SNTP lookup this often while Wi-Fi stays up.
const CLOCK_SYNC_RETRY_MS: Millis = 10 * 60 * 1000;
/// Re-sync a healthy clock this often.
const CLOCK_SYNC_REFRESH_MS: Millis = 6 * 60 * 60 * 1000;

/// How often the shelf stats `linkSlot/Config/language.txt` looking for a change made
/// from the Web page.  One `metadata` call a second is invisible; a read per
/// frame would walk the card while a game runs.
const LANGUAGE_POLL_MS: Millis = 1_000;

pub const INSERT_S: f32 = 0.73;
/// Sync screen power-on/off: the same beat as a game launch.
const SYNC_FADE_S: f32 = 0.22;
/// The card swap behind the selected game card.
const SYNC_SWAP_S: f32 = 0.18;
/// The chosen network cartridge goes into the slot with the same 0.73s
/// launch beat.
const SYNC_INSERT_S: f32 = INSERT_S;
/// Open PCB, pulse the network chip, close the shell, then insert.
const SYNC_OPENING_S: f32 = 1.20;
/// Start the chip preview this long before the long-press threshold.  The PCB
/// only holds the screen for a short beat before the chip takes over.
const SYNC_PREVIEW_MS: u64 = 1_100;
/// The zoom finishes at this share of the preview window, leaving a short beat
/// with the chip fully open before the threshold lands.
const SYNC_ZOOM_SHARE: f32 = 0.75;
/// Power-on fade for the pin-1 lamp once the chip is in front.
const SYNC_MARK_FADE_S: f32 = 0.08;

/// Preview progress after the initial cartridge-opening beat.
fn sync_preview_at(elapsed: Millis) -> f32 {
    let start = SYNC_HOLD_MS.saturating_sub(SYNC_PREVIEW_MS);
    let span = SYNC_PREVIEW_MS.max(1) as f32;
    (elapsed.saturating_sub(start) as f32 / span).clamp(0.0, 1.0)
}
/// Zoom the sync chip into the foreground from the PCB page.
const SYNC_PICKER_ZOOM_S: f32 = 0.40;
/// The cartridge reaches the slot lip on the launch curve.
const SYNC_SEAT_S: f32 = SEATED_AT;
/// The sync animation pulls the game card from the slot to its left-side
/// stage position.
const INSERT_HOLD_S: f32 = 0.28;

pub const SEATED_AT: f32 = INSERT_S - INSERT_HOLD_S;

pub const EJECT_S: f32 = SEATED_AT;

/// Keep the card seated this long after the peer reports that it already owns
/// the ROM, so the player can read the reason before it is ejected.
pub const TRANSFER_DUPLICATE_HOLD_MS: Millis = 1_000;

/// Even a tiny FC ROM keeps the transfer screen up this long, so the player
/// can read the progress instead of seeing a one-frame flash.
pub const TRANSFER_MIN_VISIBLE_MS: Millis = 1_000;

const EJECT_HOLD_S: f32 = 0.35;

const POWER_ON_S: f32 = 0.22;

const POWER_OFF_S: f32 = 0.16;

const VOLUME_STEP: u8 = 5;

const AUTOSAVE_MS: Millis = 60_000;
const CARD_CHECK_MS: Millis = 2_000;

const BATTERY_CRITICAL: u8 = 5;

const BATTERY_POLL_MS: Millis = 10_000;

const CHARGE_POLL_MS: Millis = 1_000;
const HEADPHONES_POLL_MS: Millis = 500;
/// How often the shelf header re-reads the Wi-Fi carrier state.
const WIFI_POLL_MS: Millis = 2_000;

const BATTERY_LOW: u8 = 20;

pub const UNDO_GRACE_MS: Millis = 30_000;

pub const LINK_LOST_MS: Millis = 2000;

const PLAY_HOLD_MS: Millis = 500;
/// Hold B this long to commit to the intro; the bar and the animation both
/// start on the press and finish together at this mark.
const INTRO_HOLD_MS: Millis = 1200;
/// A short still beat so the press registers before the cartridge moves.
const INTRO_ANTICIPATION_MS: Millis = 120;
/// Front-to-back squeeze of the cartridge: it starts on the press itself.
const INTRO_FLIP_MS: Millis = 600;
/// A beat with the back label facing the player before the card grows.
const INTRO_BACK_MS: Millis = 60;
/// The paper tongue slides out from under the back label.
const INTRO_PULL_MS: Millis = 200;
/// The sheet stays fully out for a beat, so the player sees the paper.
const INTRO_TONGUE_HOLD_MS: Millis = 60;
/// Unfolding the pulled-out page up into the full description card.
const INTRO_ZOOM_MS: Millis = 160;
/// Flip, pause and zoom, in that order; closing mirrors it.
const INTRO_OPEN_MS: Millis = INTRO_HOLD_MS;
const _: () = assert!(
    INTRO_ANTICIPATION_MS
        + INTRO_FLIP_MS
        + INTRO_BACK_MS
        + INTRO_PULL_MS
        + INTRO_TONGUE_HOLD_MS
        + INTRO_ZOOM_MS
        == INTRO_OPEN_MS
);
/// Releasing B early plays the whole beat backwards over this span.
const INTRO_CANCEL_MS: Millis = 260;
const INTRO_CLOSE_MS: Millis = 420;
/// The description plate the card zooms out to.
const INTRO_PLATE_W: f32 = 640.0;
const INTRO_PLATE_H: f32 = 420.0;
/// One paper colour for the back label, the paper tab and the card itself.
pub const INTRO_PAPER: [u8; 4] = [234, 229, 215, 255];
pub const INTRO_PAPER_BORDER: [u8; 4] = [201, 195, 180, 255];
/// Ink on that paper.
pub const INTRO_INK: [u8; 3] = [58, 60, 66];

/// The small label on the back of each cartridge, in cart pixels.  It is much
/// smaller than the front label: the shell around it is what makes the object
/// read as a cartridge rather than a page.
pub(crate) fn intro_back_label_box(platform: Platform) -> (f32, f32, f32, f32) {
    match platform {
        Platform::Gba => (92.0, 104.0, 176.0, 74.0),
        Platform::Gb => (58.0, 150.0, 124.0, 78.0),
        Platform::Gbc => (58.0, 160.0, 124.0, 72.0),
        Platform::Fc => (105.0, 172.0, 150.0, 76.0),
        Platform::Sfc => (100.0, 120.0, 160.0, 72.0),
        Platform::Md => (100.0, 120.0, 160.0, 72.0),
    }
}

/// The in-game menu, top to bottom.
///
/// One flat list, six rows — exactly what `navigation::panel` shows on a page
/// before it starts asking the player to flip with left/right, so the whole
/// menu is one screen. The order is the safety rule: the three that only read
/// (save album, cheats, shortcut list) come first, then linking, then the two
/// that end the run. Both of those flush the battery save and the resume state
/// on their way out, so the bottom of the list never costs a lost game — but it
/// is where a stray press lands, so they sit furthest from the row M opens on.
///
/// `返回游戏` used to hold the last slot. It is gone: B and M both return and
/// the panel's own footer says so, so the slot was worth more to these three.
///
/// The panel and the A handler both walk `ALL`, so moving a row cannot leave
/// the wrong action under the cursor — the one bug this menu must not have.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum GameMenuRow {
    Album,
    Cheats,
    Guide,
    Shortcuts,
    Link,
    Restart,
    Exit,
}

impl GameMenuRow {
    const ALL: [GameMenuRow; 7] = [
        GameMenuRow::Album,
        GameMenuRow::Cheats,
        GameMenuRow::Guide,
        GameMenuRow::Link,
        GameMenuRow::Shortcuts,
        GameMenuRow::Restart,
        GameMenuRow::Exit,
    ];

    fn label(self, lang: Language) -> &'static str {
        match self {
            GameMenuRow::Album => slot_i18n::t(lang, "device.app.menu.album"),
            GameMenuRow::Cheats => slot_i18n::t(lang, "device.app.menu.cheats"),
            GameMenuRow::Guide => slot_i18n::t(lang, "device.app.menu.guide"),
            GameMenuRow::Shortcuts => slot_i18n::t(lang, "device.app.menu.shortcuts"),
            GameMenuRow::Link => slot_i18n::t(lang, "device.app.menu.link"),
            GameMenuRow::Restart => slot_i18n::t(lang, "device.app.menu.restart"),
            GameMenuRow::Exit => slot_i18n::t(lang, "device.app.menu.exit"),
        }
    }
}

/// One row of the link page (SELECT+M / 游戏菜单 → 联机).
///
/// The row set follows the session; the labels carry the live status, so the
/// page is a place to look, not a place to hunt for a setting.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum LinkAdminRow {
    Status,
    ThisGame,
    PeerGame,
    /// The joiner may re-ask to join the host's game after declining once.
    Follow,
    Start,
    Stop,
    Back,
}

/// How long the "the host started a game, switch to it?" question waits for an
/// answer before it counts as a no.
const FOLLOW_ASK_MS: Millis = 10_000;


/// How long A must be held on the About page before debug logging is enabled.
const ABOUT_DEBUG_HOLD_MS: Millis = 1_000;

/// How long a MENU press has to last before the "keep holding to leave" prompt
/// appears.
///
/// This is what keeps a plain tap of MENU clean: the menu opens at once, and
/// nothing about leaving is mentioned until the button has been down long
/// enough to not be a tap. The rest of the hold (`MENU_HOLD_MS` in
/// `slot-input`) is what the bar fills over, so the prompt only ever shows a
/// bar that is about to complete rather than one that starts at zero.
const HOLD_PROMPT_MS: Millis = 300;

/// Utility panel kinds for the shortcut mapping popups: capturing a key, and
/// the "this will set the shortcut to 无" question.
const SHORTCUT_CAPTURE: u8 = 8;
const SHORTCUT_CLEAR: u8 = 9;

/// The game-only shortcut page opened from the in-game menu.  The shelf copy
/// (`kind 2`) stays the full, static help list.
const GAME_HELP: u8 = 11;

/// A small modal shown when the game has no readable guide.
const GUIDE_MISSING: u8 = 12;

/// How long the slot_i18n::t(lang, "device.app.common.cancelled") banner stays up after a cancelled sync.
const SYNC_CANCEL_MS: Millis = 1_000;
/// How long the completed cloud-sync frame stays up before the result banner.
const SYNC_SETTLE_MS: Millis = 1_100;
const SYNC_HOLD_BAR_W: f32 = 120.0;
const SYNC_HOLD_BAR_H: f32 = 6.0;
const SYNC_CYAN: [f32; 4] = [0.34, 0.89, 0.95, 1.0];
const SYNC_PLACEHOLDER: [f32; 4] = [0.12, 0.12, 0.16, 1.0];

const SHUTDOWN_SHOW_MS: Millis = 250;

const CORE_PICKER_RECEDE: f32 = 0.26;

const SLOT_NAME_IN_MS: Millis = 200;
const SLOT_NAME_HOLD_MS: Millis = 1200;
const SLOT_NAME_OUT_MS: Millis = 800;
const SLOT_NAME_ALPHA: f32 = 0.82;
const CORE_PICKER_DIM: f32 = 0.614;
const CORE_LEGEND_Y: f32 = 386.0;
const LINK_TEXT_Y: f32 = 44.0;
const LINK_LEGEND_Y: f32 = 422.0;
const LINK_LEGEND_GAP: f32 = 40.0;
const LID_SHADOW_W: f32 = 168.0;
const LID_SHADOW_H: f32 = 18.0;
const LID_SHADOW_DROP: f32 = 29.0;
const LID_SHADOW_ALPHA: f32 = 0.8;
const FACES_WAIT_MS: Millis = 1500;

const ALERT_HOLD: f32 = 0.45;
const ALERT_GONE: f32 = 0.9;

const CLOCK_FLOOR: i64 = 1_577_836_800;

pub enum PendingUndo {
    Save {
        stamp: String,
        evicted: Option<(String, Vec<u8>, Vec<u8>)>,
    },
    Load {
        prior: Vec<u8>,
    },
}

struct LinkSession {
    client_id: u16,
    lost_at: Option<Millis>,
    /// The other device's name, from its link hello.
    peer: Option<String>,
    /// The game the other side announced it is playing, with the identity
    /// needed to find the same cart here.
    peer_game: Option<crate::cable::GameStart>,
    /// The game *this* side is playing, for the link page.
    game: Option<String>,
    /// A cable is open to the peer right now (as opposed to a live session
    /// that is merely waiting for the next game).
    paired: bool,
}

/// A game the peer started that this device could switch to.
struct FollowAsk {
    name: String,
    stem: String,
    /// The core the peer runs it on, so the joiner can match it.
    core: u8,
    since: Millis,
}

/// What the player decided about following the peer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FollowAnswer {
    /// The switch is already under way (the app started ejecting).
    Accept,
    /// The peer has to be told why this side is not coming.
    Refuse(crate::cable::FollowRefusal),
}

struct LinkStarting {
    starter: LinkStarter,
    client_id: u16,
}

struct Reload {
    stem: String,
    role: LinkRow,
    cancelled: bool,
    from: LinkKind,
    from_serial: &'static str,
    fallback: bool,
}

/// One piece of pre-rasterised text for the shelf sync overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncFace {
    pub tex: TexId,
    pub w: u32,
    pub h: u32,
}

/// The overlay text faces the frontend has rasterised; `None` while the state
/// has nothing to say on that line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncFaces {
    pub banner: Option<SyncFace>,
    pub cloud_line: Option<SyncFace>,
    pub local_line: Option<SyncFace>,
    pub upload_label: Option<SyncFace>,
    pub download_label: Option<SyncFace>,
    /// Static art for the sync animation.
    pub title: Option<SyncFace>,
    pub data_label: Option<SyncFace>,
    pub net_label: Option<SyncFace>,
    pub card_label: Option<SyncFace>,
    pub state_label: Option<SyncFace>,
    pub sav_label: Option<SyncFace>,
    pub rail_labels: [Option<SyncFace>; 5],
    pub net_slot_title: Option<SyncFace>,
    pub net_slot_hint: Option<SyncFace>,
    pub progress_left: Option<SyncFace>,
    pub progress_right: Option<SyncFace>,
    pub cancel_hint: Option<SyncFace>,
    pub check: Option<SyncFace>,
}

/// The strings behind [`SyncFaces`]; the frontend re-rasterises only when this
/// value changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncTexts {
    /// Heading above the animation; empty while no animation owns the screen.
    pub title: String,
    pub banner: String,
    pub cloud_line: String,
    pub local_line: String,
    pub upload_label: String,
    pub download_label: String,
    pub progress_left: String,
    pub progress_right: String,
}

/// The cloud and arrow textures the frontend uploads once at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncArt {
    pub cloud: SyncFace,
    pub up: SyncFace,
    pub down: SyncFace,
}

/// A screen rectangle: left, top, width, height.
type SyncRect = (f32, f32, f32, f32);

/// Which side wins a conflict.  "Overwrite the local unit" is the default: it
/// is the reversible direction (project book §6.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncChoice {
    #[default]
    Download,
    Upload,
}

impl SyncChoice {
    pub fn other(self) -> Self {
        match self {
            SyncChoice::Download => SyncChoice::Upload,
            SyncChoice::Upload => SyncChoice::Download,
        }
    }

    /// The direction label from project book appendix A.
    pub fn text(self, lang: Language) -> &'static str {
        match self {
            SyncChoice::Download => slot_i18n::t(lang, "device.app.sync.overwrite_local"),
            SyncChoice::Upload => slot_i18n::t(lang, "device.app.sync.overwrite_cloud"),
        }
    }

    /// The CLI direction this choice resolves to.
    pub fn direction(self) -> Direction {
        match self {
            SyncChoice::Download => Direction::Download,
            SyncChoice::Upload => Direction::Upload,
        }
    }

    pub fn resolve(self) -> Resolve {
        match self {
            SyncChoice::Download => Resolve::Download,
            SyncChoice::Upload => Resolve::Upload,
        }
    }
}

/// One page of the multi-page conflict overlay (project book §7.1/§7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPage {
    All,
    State,
    Sav,
}

impl ConflictPage {
    fn unit(self) -> Option<UnitKind> {
        match self {
            ConflictPage::All => None,
            ConflictPage::State => Some(UnitKind::State),
            ConflictPage::Sav => Some(UnitKind::Sav),
        }
    }
}

fn unit_index(kind: UnitKind) -> usize {
    match kind {
        UnitKind::State => 0,
        UnitKind::Sav => 1,
    }
}

fn unit_page(kind: UnitKind) -> ConflictPage {
    match kind {
        UnitKind::State => ConflictPage::State,
        UnitKind::Sav => ConflictPage::Sav,
    }
}

/// A conflicting unit and the version information the user decides against.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConflictUnit {
    kind: UnitKind,
    info: ConflictInfo,
    reason: &'static str,
}

/// What the current page's A press synchronises; carried across the CLI run so
/// the result can lock the right page or retry the failed subset.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ConflictPending {
    Both(Vec<UnitKind>),
    Unit(UnitKind),
}

/// The multi-page conflict state machine (project book §7.4).
#[derive(Clone, Debug)]
pub struct ConflictState {
    units: Vec<ConflictUnit>,
    pages: Vec<ConflictPage>,
    page: ConflictPage,
    /// `state`/`sav` locked flags, indexed by [`unit_index`].
    locked: [bool; 2],
    all_choice: SyncChoice,
    state_choice: SyncChoice,
    sav_choice: SyncChoice,
    error: Option<String>,
    notice: Option<String>,
    /// On the "全部" page: units to retry after a partial failure.
    retry: Vec<UnitKind>,
    since: Millis,
    pending: Option<ConflictPending>,
}

impl ConflictState {
    /// Build the conflict pages from the CLI's conflict outcome.  Both
    /// conflicting units produce 3 pages; one produces a single page (no tabs).
    fn from_outcome(outcome: &SyncOutcome, now: Millis) -> Option<Self> {
        if outcome.result != SyncResult::Conflict {
            return None;
        }
        let kinds: Vec<UnitKind> = if outcome.conflict_units.is_empty() {
            UnitKind::ALL
                .into_iter()
                .filter(|kind| outcome.unit_result(*kind) == Some(SyncResult::Conflict))
                .collect()
        } else {
            outcome.conflict_units.clone()
        };
        let mut units: Vec<ConflictUnit> = kinds
            .into_iter()
            .map(|kind| {
                let unit = outcome.unit(kind);
                ConflictUnit {
                    kind,
                    info: unit
                        .and_then(|unit| unit.info.clone())
                        .or_else(|| outcome.info.clone())
                        .unwrap_or_default(),
                    reason: unit
                        .map(|unit| unit.conflict_line())
                        .unwrap_or_else(|| outcome.conflict_line()),
                }
            })
            .collect();
        if units.is_empty() {
            // Legacy/malformed conflict output: keep the old single-unit page.
            units.push(ConflictUnit {
                kind: UnitKind::State,
                info: outcome.info.clone().unwrap_or_default(),
                reason: outcome.conflict_line(),
            });
        }
        units.sort_by_key(|unit| unit_index(unit.kind));
        units.dedup_by_key(|unit| unit_index(unit.kind));
        let unit_pages: Vec<ConflictPage> = units.iter().map(|unit| unit_page(unit.kind)).collect();
        let pages = if units.len() >= 2 {
            vec![ConflictPage::All, ConflictPage::State, ConflictPage::Sav]
        } else {
            unit_pages
        };
        Some(ConflictState {
            units,
            page: pages[0],
            locked: [false, false],
            all_choice: SyncChoice::Download,
            state_choice: SyncChoice::Download,
            sav_choice: SyncChoice::Download,
            error: None,
            notice: None,
            retry: Vec::new(),
            pending: None,
            pages,
            since: now,
        })
    }

    fn unit(&self, kind: UnitKind) -> Option<&ConflictUnit> {
        self.units.iter().find(|unit| unit.kind == kind)
    }

    fn has_unit(&self, kind: UnitKind) -> bool {
        self.unit(kind).is_some()
    }

    fn is_locked(&self, kind: UnitKind) -> bool {
        self.locked[unit_index(kind)]
    }

    fn lock(&mut self, kind: UnitKind) {
        self.locked[unit_index(kind)] = true;
    }

    fn all_locked(&self) -> bool {
        self.units.iter().all(|unit| self.is_locked(unit.kind))
    }

    fn locked_page(&self) -> bool {
        self.page.unit().is_some_and(|kind| self.is_locked(kind))
    }

    /// The "全部" page also becomes read-only as soon as any of its two units
    /// has been synced (audit point for T18).
    fn page_locked(&self) -> bool {
        match self.page {
            ConflictPage::All => self.units.iter().any(|unit| self.is_locked(unit.kind)),
            _ => self.locked_page(),
        }
    }

    fn direction(&self) -> SyncChoice {
        match self.page {
            ConflictPage::All => self.all_choice,
            ConflictPage::State => self.state_choice,
            ConflictPage::Sav => self.sav_choice,
        }
    }

    fn set_direction(&mut self, choice: SyncChoice) {
        match self.page {
            ConflictPage::All => self.all_choice = choice,
            ConflictPage::State => self.state_choice = choice,
            ConflictPage::Sav => self.sav_choice = choice,
        }
    }

    fn next_page(&mut self) {
        if let Some(index) = self.pages.iter().position(|page| *page == self.page) {
            if self.pages.len() > 1 {
                self.page = self.pages[(index + 1) % self.pages.len()];
            }
        }
    }

    fn prev_page(&mut self) {
        if let Some(index) = self.pages.iter().position(|page| *page == self.page) {
            if self.pages.len() > 1 {
                let len = self.pages.len();
                self.page = self.pages[(index + len - 1) % len];
            }
        }
    }

    /// The next unlocked unit page after locking `kind` (for focus switching).
    fn next_focus_page(&self) -> Option<ConflictPage> {
        [ConflictPage::State, ConflictPage::Sav]
            .into_iter()
            .find(|page| page.unit().is_some_and(|kind| self.has_unit(kind) && !self.is_locked(kind)))
    }

    /// After the CLI reported `changed-since`: keep the pages, refresh the
    /// version information the CLI sent, and reset every decision (project
    /// book §7.8).
    fn apply_changed_since(&mut self, outcome: &SyncOutcome, now: Millis, lang: Language) {
        for unit in &mut self.units {
            if let Some(fresh) = outcome.unit(unit.kind).and_then(|unit| unit.info.clone()) {
                unit.info = fresh;
            }
        }
        self.locked = [false, false];
        self.all_choice = SyncChoice::Download;
        self.state_choice = SyncChoice::Download;
        self.sav_choice = SyncChoice::Download;
        self.page = self.pages[0];
        self.error = None;
        self.retry.clear();
        self.pending = None;
        self.notice = Some(slot_i18n::t(lang, "device.app.sync.cloud_moved").to_string());
        self.since = now;
    }

    fn to_ui(&self, syncing: bool, lang: Language) -> ConflictUi {
        ConflictUi {
            pages: self.pages.clone(),
            page: self.page,
            direction: self.direction(),
            state: self.unit_ui(UnitKind::State, lang),
            sav: self.unit_ui(UnitKind::Sav, lang),
            locked_state: self.is_locked(UnitKind::State),
            locked_sav: self.is_locked(UnitKind::Sav),
            error: self.error.clone(),
            notice: self.notice.clone(),
            syncing,
        }
    }

    fn unit_ui(&self, kind: UnitKind, lang: Language) -> Option<ConflictUnitUi> {
        let unit = self.unit(kind)?;
        Some(ConflictUnitUi {
            cloud_line: version_info_line(
                slot_i18n::t(lang, "device.app.sync.side_cloud"),
                &unit.info.cloud,
                lang,
            ),
            local_line: version_info_line(
                slot_i18n::t(lang, "device.app.sync.side_local"),
                &unit.info.local,
                lang,
            ),
            reason: unit.reason.to_string(),
            locked: self.is_locked(kind),
        })
    }
}

/// Per-unit conflict information for the UI (owned so the frontend can cache
/// the rendered face by plain equality).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictUnitUi {
    pub cloud_line: String,
    pub local_line: String,
    pub reason: String,
    pub locked: bool,
}

/// Everything the conflict faces print (project book §7.2/§7.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictUi {
    pub pages: Vec<ConflictPage>,
    pub page: ConflictPage,
    pub direction: SyncChoice,
    pub state: Option<ConflictUnitUi>,
    pub sav: Option<ConflictUnitUi>,
    pub locked_state: bool,
    pub locked_sav: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// True while a per-page sync is running (A was pressed).
    pub syncing: bool,
}

/// The main-shelf sync overlay state machine (project book §6.4/§7.4).
pub enum ShelfSync {
    Idle,
    /// Y is down; holding for `SYNC_HOLD_MS` starts the sync.
    Holding {
        since: Millis,
    },
    /// The long-press flourish: open the PCB, light its network chip,
    /// close the shell, then insert the cartridge.
    Opening {
        since: Millis,
        request: SyncRequest,
    },
    /// Y was released early: put the game card back in front.
    Unholding {
        since: Millis,
        from: f32,
        preview: f32,
    },
    /// After a picker choice: swap the chosen card out from behind the game
    /// card, then insert it.
    Swapping {
        since: Millis,
        card: (TexId, u32, u32),
    },
    Running {
        run: Box<dyn SyncRun>,
        request: SyncRequest,
        started: Millis,
        stage: Option<SyncStage>,
        direction: Option<Direction>,
        /// Set when this run was launched from a conflict page; the state is
        /// resumed when the CLI finishes so locks/retries survive.
        conflict: Option<ConflictState>,
        /// `mods.progress` value, 0.0–1.0. Cloud sync uses its stage mapping.
        progress: Option<f32>,
        progress_label: Option<String>,
    },
    /// The cloud animation's completion beat.  It owns the screen briefly,
    /// then turns into the normal result banner.
    Settling {
        until: Millis,
        text: String,
        direction: Option<Direction>,
        noop: bool,
        only: Option<UnitKind>,
    },
    /// The network cartridge leaves the slot after the sync screen closes.
    Ejecting {
        since: Millis,
        text: String,
    },
    /// `linkSlot sync` answered exit 2: the user picks who wins.
    Conflict {
        state: ConflictState,
    },
    /// A manual mod asked for a choice (`LINKSLOT_RESULT=choice`).
    Choice {
        request: SyncRequest,
        state: ModChoiceState,
    },
    /// A short banner ("已拉取版本 7", slot_i18n::t(lang, "device.app.sync.offline"), …) that dismisses itself.
    Result {
        text: String,
        until: Millis,
    },
    /// The Y picker is closed but Wi-Fi is still being brought up.
    Connecting {
        since: Millis,
    },
}

/// The device-side modal state for a generic mod choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModChoiceState {
    pub request: slot_mods::ChoiceRequest,
    pub selected: usize,
}

impl ModChoiceState {
    fn new(request: slot_mods::ChoiceRequest) -> Self {
        let selected = request.default_index();
        ModChoiceState { request, selected }
    }

    fn previous(&mut self) {
        let count = self.request.options.len().max(1);
        self.selected = (self.selected + count - 1) % count;
    }

    fn next(&mut self) {
        let count = self.request.options.len().max(1);
        self.selected = (self.selected + 1) % count;
    }
}

/// The labels the frontend renders for [`ShelfSync::Choice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModChoiceUi {
    pub title: String,
    pub body: String,
    pub options: Vec<String>,
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpgradeUiState {
    Confirm,
    Applying,
    Rebooting,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeUi {
    pub target: String,
    pub state: UpgradeUiState,
}

/// What one card of the long-press Y chooser starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncPick {
    /// The built-in cloud sync, as a long press used to start directly.
    Cloud,
    /// One mod's manual action (`mods run --id --action`).
    Mod {
        mod_id: String,
        action_id: String,
        /// Optional declarative animation from the action manifest.
        ui: Option<slot_mods::ActionUi>,
    },
}

/// The animated foreground picker opened with Y on the PCB page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SyncPicker {
    original: usize,
    since: Millis,
}

/// Compact names shown on the network chip, not the full manifest title.
fn short_sync_name(mod_id: &str, full: &str) -> String {
    match mod_id {
        "gbastation-battery-sync" => "GBAStation".to_string(),
        "savedeck-battery-bridge" => "SaveDeck".to_string(),
        "ai-game-info" => "AI".to_string(),
        "metadata-migrate" => "Metadata".to_string(),
        "tianma-gba-info" => "TianmaG".to_string(),
        _ => full
            .split_whitespace()
            .next()
            .filter(|word| !word.is_empty())
            .unwrap_or(full)
            .to_string(),
    }
}

/// One cloud-sync mod exposed by the cartridge's network chip.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SyncModChoice {
    /// The built-in cloud route, always available, and the default.
    cloud: bool,
    name: String,
    mod_id: String,
    action_id: String,
    ui: Option<slot_mods::ActionUi>,
}

/// Rotation progress for the flip beat: front half, a short edge-on hold,
/// then the back half.  The hold is what makes the turn legible at speed.
fn flip_turn(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let front = 0.43;
    let back = 0.57;
    if t < front {
        0.5 * (t / front)
    } else if t <= back {
        0.5
    } else {
        0.5 + 0.5 * ((t - back) / (1.0 - back))
    }
}

/// Where the pulled-out paper tongue sits, in screen pixels: a strip hanging
/// out of the cartridge just under its back label.
fn intro_tongue_rect(
    platform: Platform,
    cart: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let cart_px = slot_ui::cart_art_box(platform);
    let (lx, ly, lw, lh) = intro_back_label_box(platform);
    let sx = cart.2 / cart_px.0 as f32;
    let sy = cart.3 / cart_px.1 as f32;
    let width = lw * 0.62 * sx;
    // The sheet is tucked behind the label: it grows upwards from the label's
    // top edge.  Its length follows the label, not the empty space above it —
    // GB/GBC/FC labels sit low, so a fraction of `ly` pulled far too long a
    // sheet out of them.
    let bottom = cart.1 + ly * sy;
    let height = (lh * 1.05).max(40.0) * sy;
    (
        cart.0 + (lx + lw * 0.5) * sx - width / 2.0,
        bottom - height,
        width,
        height,
    )
}

/// Draw the sheet at `progress` (0 = tucked behind the label, 1 = fully out).
/// It grows upwards from the label's top edge, so the paper looks like it is
/// being pulled out from behind the back label.
fn draw_intro_tongue(rect: (f32, f32, f32, f32), progress: f32, out: &mut Vec<Draw>) {
    let (x, y, w, full_h) = rect;
    let h = full_h * progress.clamp(0.0, 1.0);
    if h <= 0.5 || w <= 0.5 {
        return;
    }
    let top = y + full_h - h;
    let bottom = y + full_h;
    let paper = [
        INTRO_PAPER[0] as f32 / 255.0,
        INTRO_PAPER[1] as f32 / 255.0,
        INTRO_PAPER[2] as f32 / 255.0,
        1.0,
    ];
    let border = [
        INTRO_PAPER_BORDER[0] as f32 / 255.0,
        INTRO_PAPER_BORDER[1] as f32 / 255.0,
        INTRO_PAPER_BORDER[2] as f32 / 255.0,
        1.0,
    ];
    let edge = [
        (INTRO_PAPER[0] as f32 + 18.0).min(255.0) / 255.0,
        (INTRO_PAPER[1] as f32 + 18.0).min(255.0) / 255.0,
        (INTRO_PAPER[2] as f32 + 18.0).min(255.0) / 255.0,
        1.0,
    ];
    let ink = [
        INTRO_INK[0] as f32 / 255.0,
        INTRO_INK[1] as f32 / 255.0,
        INTRO_INK[2] as f32 / 255.0,
    ];
    let radius = 5.0f32.min(h / 2.0);
    // Body from under the rounded top down to the pocket.
    out.push(Draw::Rect {
        x,
        y: top + radius,
        w,
        h: h - radius,
        colour: paper,
    });
    // Rounded leading edge.
    let rows = radius.ceil() as u32;
    for row in 0..rows {
        let t = (row as f32 + 1.0) / rows as f32;
        let inset = radius - (radius * radius - (radius * (1.0 - t)).powi(2)).max(0.0).sqrt();
        out.push(Draw::Rect {
            x: x + inset,
            y: top + row as f32,
            w: (w - inset * 2.0).max(1.0),
            h: 1.0,
            colour: paper,
        });
    }
    // Fold crease near the pocket and the printed rules.
    out.push(Draw::Rect {
        x: x + 3.0,
        y: bottom - 11.0,
        w: (w - 6.0).max(1.0),
        h: 1.0,
        colour: border,
    });
    for i in 0..3 {
        let line_y = top + 12.0 + i as f32 * 13.0;
        if line_y + 1.5 < bottom - 14.0 {
            out.push(Draw::Rect {
                x: x + 10.0,
                y: line_y,
                w: (w - 20.0).max(2.0),
                h: 1.5,
                colour: [ink[0], ink[1], ink[2], 0.18],
            });
        }
    }
    // Light edge on the sides and a shadow line where it leaves the label.
    out.push(Draw::Rect {
        x: x - 1.0,
        y: top + radius,
        w: 1.0,
        h: (h - radius).max(1.0),
        colour: edge,
    });
    out.push(Draw::Rect {
        x: x + w,
        y: top + radius,
        w: 1.0,
        h: (h - radius).max(1.0),
        colour: edge,
    });
    out.push(Draw::Rect {
        x,
        y: bottom - 1.0,
        w,
        h: 1.0,
        colour: border,
    });
}

/// The sheet casts a soft shadow on the cartridge just above its leading edge.
fn draw_paper_shadow_on_cart(
    tongue: (f32, f32, f32, f32),
    progress: f32,
    out: &mut Vec<Draw>,
) {
    let (x, y, w, full_h) = tongue;
    let h = full_h * progress.clamp(0.0, 1.0);
    if h <= 1.0 {
        return;
    }
    let top = y + full_h - h;
    let ink = [
        INTRO_INK[0] as f32 / 255.0,
        INTRO_INK[1] as f32 / 255.0,
        INTRO_INK[2] as f32 / 255.0,
    ];
    for i in 0..3 {
        out.push(Draw::Rect {
            x: x - 1.0,
            y: top - 3.0 + i as f32,
            w: w + 2.0,
            h: 1.0,
            colour: [ink[0], ink[1], ink[2], 0.10 + i as f32 * 0.05],
        });
    }
}

/// A soft shadow plus a dark hairline around the cover/video, so bright box
/// art (often white titles) does not melt into the paper.
fn push_media_frame(out: &mut Vec<Draw>, x: f32, y: f32, w: f32, h: f32) {
    let ink = [
        INTRO_INK[0] as f32 / 255.0,
        INTRO_INK[1] as f32 / 255.0,
        INTRO_INK[2] as f32 / 255.0,
    ];
    out.push(Draw::Rect {
        x: x - 4.0,
        y: y - 4.0,
        w: w + 8.0,
        h: h + 8.0,
        colour: [ink[0], ink[1], ink[2], 0.10],
    });
    let edge = [ink[0], ink[1], ink[2], 0.34];
    out.push(Draw::Rect { x: x - 1.0, y: y - 1.0, w: w + 2.0, h: 1.0, colour: edge });
    out.push(Draw::Rect { x: x - 1.0, y: y + h, w: w + 2.0, h: 1.0, colour: edge });
    out.push(Draw::Rect { x: x - 1.0, y: y - 1.0, w: 1.0, h: h + 2.0, colour: edge });
    out.push(Draw::Rect { x: x + w, y: y - 1.0, w: 1.0, h: h + 2.0, colour: edge });
}

/// Unfold the finished card from the pulled-out tongue: the bottom edge moves
/// down to the plate, the top edge rises faster, so the sheet grows upwards
/// like a page being opened instead of a bitmap being scaled.
fn unfold_intro_draws(
    draws: Vec<Draw>,
    tongue: (f32, f32, f32, f32),
    k: f32,
) -> Vec<Draw> {
    let plate = (
        (OUT_W as f32 - INTRO_PLATE_W) / 2.0,
        (OUT_H as f32 - INTRO_PLATE_H) / 2.0,
        INTRO_PLATE_W,
        INTRO_PLATE_H,
    );
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let k_top = (k * 1.5).min(1.0);
    let top = mix(tongue.1, plate.1, k_top);
    let bottom = mix(tongue.1 + tongue.3, plate.1 + plate.3, k);
    let width = mix(tongue.2, plate.2, k);
    let centre = mix(tongue.0 + tongue.2 / 2.0, plate.0 + plate.2 / 2.0, k);
    let rect = (centre - width / 2.0, top, width, (bottom - top).max(1.0));
    let sx = rect.2 / plate.2;
    let sy = rect.3 / plate.3;
    let fade = k.clamp(0.0, 1.0);
    draws
        .into_iter()
        .map(|draw| match draw {
            Draw::Rect { x, y, w, h, mut colour } => {
                colour[3] *= fade;
                Draw::Rect {
                    x: rect.0 + (x - plate.0) * sx,
                    y: rect.1 + (y - plate.1) * sy,
                    w: w * sx,
                    h: h * sy,
                    colour,
                }
            }
            Draw::Tex { x, y, w, h, tex, alpha } => Draw::Tex {
                x: rect.0 + (x - plate.0) * sx,
                y: rect.1 + (y - plate.1) * sy,
                w: w * sx,
                h: h * sy,
                tex,
                alpha: alpha * fade,
            },
            Draw::Turned { x, y, w, h, tex, alpha, turn } => Draw::Turned {
                x: rect.0 + (x - plate.0) * sx,
                y: rect.1 + (y - plate.1) * sy,
                w: w * sx,
                h: h * sy,
                tex,
                alpha: alpha * fade,
                turn,
            },
            other => other,
        })
        .collect()
}

fn cloud_sync_choice(lang: Language) -> SyncModChoice {
    SyncModChoice {
        cloud: true,
        name: slot_i18n::t(lang, "device.app.shelf.cloud").to_string(),
        mod_id: String::new(),
        action_id: String::new(),
        ui: None,
    }
}

/// The game-intro overlay opened by holding X on the shelf. Description and
/// cover are read lazily; the frontend rasterises the text faces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameIntro {
    pub platform: Platform,
    /// The cartridge's plastic colour, so the back matches the front face.
    pub shell: [u8; 3],
    pub title: String,
    pub description: String,
    pub cover: Option<PathBuf>,
    pub demo: Option<PathBuf>,
    pub started: Millis,
    /// Set when the player asked to close; the card plays back out and is
    /// dropped after [`INTRO_CLOSE_MS`].
    pub closing: Option<Millis>,
    /// True once B has been held for the full beat.  Before that the card is
    /// only a preview: releasing B plays it back and drops it.
    pub armed: bool,
    /// Set when the preview is playing backwards after an early release.
    pub cancel: Option<Millis>,
    /// The back of the cartridge: paper label with title and a short summary.
    pub back: Option<(TexId, u32, u32)>,
    pub faces: Option<GameIntroFaces>,
    /// First visible description line.
    pub scroll: usize,
    /// Last useful scroll offset, filled in by the frontend after wrapping.
    pub max_scroll: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameIntroFaces {
    pub plate: (TexId, u32, u32),
    /// The game title printed on the paper card in ink.
    pub title: (TexId, u32, u32),
    pub body: (TexId, u32, u32),
    /// Description line currently baked into `body`.
    pub body_scroll: usize,
    pub cover: Option<(TexId, u32, u32)>,
    pub video: Option<IntroVideoFaces>,
    pub hint: (TexId, u32, u32),
    /// Separate cap face for Y so it matches the B cap instead of being plain
    /// text inside the B label.
    pub hint_delete: (TexId, u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferFaces {
    pub percent: (TexId, u32, u32),
    pub assets: Vec<(TexId, u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferView {
    pub percent: u32,
    pub assets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntroVideoFaces {
    /// One texture per kept frame, plus its display time in milliseconds.
    pub frames: Vec<(TexId, u32)>,
    pub width: u32,
    pub height: u32,
}

/// The chosen game card staying on the shelf while the module clips on, then
/// entering the native slot directly.
struct SyncInsert {
    tex: TexId,
    w: u32,
    h: u32,
    shadow: Option<TexId>,
    /// Some for the no-mod auto card: start where it was in the shelf.
    from: Option<SyncRect>,
    started: Millis,
    sounded: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GameMenu {
    Pick(LinkRow),
    Working {
        role: LinkRow,
        step: LinkStep,
        since: Millis,
    },
    Linked {
        role: LinkRow,
        worked: Millis,
        since: Millis,
        opened: bool,
    },
    Failed {
        role: LinkRow,
        fail: LinkFail,
        worked: Millis,
        since: Millis,
    },
    Unplug {
        role: LinkRow,
        since: Millis,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkRow {
    Host,
    Join,
}

impl LinkRow {
    pub const ALL: [LinkRow; 2] = [LinkRow::Host, LinkRow::Join];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn text(self, lang: Language) -> &'static str {
        match self {
            LinkRow::Host => slot_i18n::t(lang, "device.app.link.create"),
            LinkRow::Join => slot_i18n::t(lang, "device.app.link.join"),
        }
    }

    pub fn role(self) -> LinkRole {
        match self {
            LinkRow::Host => LinkRole::Host,
            LinkRow::Join => LinkRole::Join,
        }
    }

    pub fn client_id(self) -> u16 {
        match self {
            LinkRow::Host => 0,
            LinkRow::Join => 1,
        }
    }

    pub fn other(self) -> LinkRow {
        match self {
            LinkRow::Host => LinkRow::Join,
            LinkRow::Join => LinkRow::Host,
        }
    }

    pub fn from_client_id(id: u16) -> LinkRow {
        if id == 0 {
            LinkRow::Host
        } else {
            LinkRow::Join
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkLegend {
    Cancel,
    Mode,
    Swap,
    Link,
    Ok,
    Back,
    EndLink,
}

impl LinkLegend {
    pub const ALL: [LinkLegend; 7] = [
        LinkLegend::Cancel,
        LinkLegend::Mode,
        LinkLegend::Swap,
        LinkLegend::Link,
        LinkLegend::Ok,
        LinkLegend::Back,
        LinkLegend::EndLink,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

pub const LINKED_HOLD_MS: Millis = 1000;

pub const UNPLUG_HOLD_MS: Millis = 420;

#[derive(Debug)]
pub enum Phase {
    SetClock {
        picker: ClockPicker,
        seed: i64,
        from_menu: bool,
    },
    Shelf,
    QuickMenu {
        row: QuickRow,
    },
    Inserting {
        cart: String,
        t: f32,
        core_ready: bool,
        resumed: bool,
        clean: bool,
    },
    Playing {
        cart: String,
    },
    Ejecting {
        cart: String,
        t: f32,
    },
    Polaroids {
        cart: String,
    },
    About,
    Doze {
        cart: Option<String>,
    },
}

/// An image rendered as a full guide page.
#[derive(Debug)]
pub struct GuideImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
enum GuidePage {
    Text(Vec<String>),
    Image(Arc<GuideImage>),
}

/// Where the guide reader's page is stored inside a game's metadata.
const GUIDE_PAGE_KEY: &str = "guide_page";

/// A TXT/EPUB guide, opened from the in-game menu.  The game stays paused
/// underneath; B returns to the menu rather than resuming immediately.
#[derive(Debug)]
pub struct GuideView {
    title: String,
    pages: Vec<GuidePage>,
    page: usize,
}

impl GuideView {
    fn page_count(&self) -> usize {
        self.pages.len().max(1)
    }

    fn page_lines(&self) -> Vec<String> {
        match self.pages.get(self.page) {
            Some(GuidePage::Text(lines)) => lines.clone(),
            _ => Vec::new(),
        }
    }

    fn image(&self) -> Option<Arc<GuideImage>> {
        match self.pages.get(self.page) {
            Some(GuidePage::Image(image)) => Some(Arc::clone(image)),
            _ => None,
        }
    }
}

fn guide_text_pages(text: &str) -> Vec<GuidePage> {
    let lines = slot_ui::text::wrap_paragraphs(
        text,
        slot_ui::guide::TEXT_PX,
        slot_ui::guide::TEXT_W,
    );
    lines
        .chunks(slot_ui::guide::LINES_PER_PAGE)
        .map(|chunk| GuidePage::Text(chunk.to_vec()))
        .collect()
}

fn decode_guide_image(bytes: &[u8]) -> Option<GuideImage> {
    if let Some((rgba, width, height)) = slot_ui::decode_bytes(bytes) {
        return Some(GuideImage {
            rgba,
            width,
            height,
        });
    }
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (info.width as u32, info.height as u32);
    if width == 0 || height == 0 {
        return None;
    }
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            let mut rgba = vec![255u8; pixels.len() / 3 * 4];
            for (at, pixel) in pixels.chunks_exact(3).enumerate() {
                rgba[at * 4..at * 4 + 3].copy_from_slice(pixel);
            }
            rgba
        }
        jpeg_decoder::PixelFormat::L8 => {
            let mut rgba = vec![255u8; pixels.len() * 4];
            for (at, value) in pixels.iter().enumerate() {
                rgba[at * 4..at * 4 + 3].fill(*value);
            }
            rgba
        }
        _ => return None,
    };
    Some(GuideImage {
        rgba,
        width,
        height,
    })
}

fn guide_epub_pages(blocks: Vec<crate::epub::Block>, lang: Language) -> Vec<GuidePage> {
    let mut pages = Vec::new();
    for block in blocks {
        match block {
            crate::epub::Block::Text(text) => pages.extend(guide_text_pages(&text)),
            crate::epub::Block::Image { name, data } => {
                if let Some(image) = decode_guide_image(&data) {
                    pages.push(GuidePage::Image(Arc::new(image)));
                } else {
                    pages.extend(guide_text_pages(&slot_i18n::t_args(
                        lang,
                        "device.app.guide.broken_image",
                        &[("name", name.as_str())],
                    )));
                }
            }
        }
    }
    pages
}

/// What the on-screen keyboard is collecting.  The SSID is carried through
/// the password prompt so the two prompts stay one flow.
pub enum KeyboardPurpose {
    WifiPassword { ssid: String },
    WifiSsid,
    GameSearch,
}

pub struct KeyboardState {
    pub purpose: KeyboardPurpose,
    pub title: String,
    pub masked: bool,
    pub text: String,
    pub cursor: slot_ui::keyboard::Cursor,
    pub hint: String,
    pub error: Option<String>,
    pub composition: String,
    pub candidates: Vec<String>,
    pub candidate_page: usize,
    pub candidate_pages: usize,
    pub candidate_selected: usize,
    pub candidate_focus: bool,
}

/// The full-screen Wi-Fi menu, shown as an overlay while the quick menu stays
/// underneath (so `held()` keeps the game paused).
pub struct WifiUi {
    pub status: String,
    pub connected: Option<String>,
    pub error: Option<String>,
    pub rows: Vec<slot_ui::network::WifiEntry>,
    pub saved: Vec<crate::linkcfg::SavedWifi>,
    pub selected: usize,
    pub offset: usize,
    pub manual: bool,
    pub connecting: Option<String>,
    pub job: Option<WifiJob>,
    pub status_job: Option<WifiJob>,
    pub radio_on: bool,
    pub radio_known: bool,
    pub radio_job: Option<WifiJob>,
    pub radio_pending: Option<bool>,
    pub auto_off_minutes: u16,
}

pub enum WebPopup {
    Starting,
    Started { url: String },
    Failed { message: String },
}

/// A modal on top of a live link session: `EndLink` asks before a long M ends
/// the session.  Disconnects are told with a notice, not a dialog.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkPopup {
    EndLink,
}

pub struct SyncUi {
    pub profiles: Vec<linkcfg::StorageProfile>,
    pub active: String,
    pub selected: usize,
    pub tested: Option<String>,
    pub testing_id: Option<String>,
    pub job: Option<crate::storage_test::StorageTestJob>,
    pub status: String,
    pub error: Option<String>,
    pub switching: bool,
    /// First use: no profile has a URL yet.  The page offers to start the Web
    /// service instead of pretending the empty default profile is editable.
    pub needs_setup: bool,
}

/// The full-screen `mod 设置` list, shown over the quick menu.  `rows` is a
/// snapshot of `mods/`; A toggles `mod.json`'s total enabled flag.
pub struct ModsUi {
    pub rows: Vec<slot_mods::ModEntry>,
    pub selected: usize,
    pub offset: usize,
    pub error: Option<String>,
}


pub struct SearchResults {
    pub query: String,
    pub hits: Vec<crate::game_search::SearchHit>,
    pub selected: usize,
    pub offset: usize,
}

pub const SEARCH_ROWS: usize = 7;

/// The PCB overlay that lets the player choose the default shell or one of up
/// to five custom front faces.  Index 0 is always the untouched default shell,
/// so the deck is usable even when the card has no custom fronts yet.
struct FrontDeck {
    root: PathBuf,
    cart: Cart,
    faces: Vec<slot_store::fronts::FrontFace>,
    selected: usize,
    scroll: f32,
    velocity: f32,
    held: Option<(i32, Millis)>,
}

impl FrontDeck {
    fn count(&self) -> usize {
        self.faces.len() + 1
    }

    fn step(&mut self, direction: i32) {
        let count = self.count();
        self.selected = (self.selected as i32 + direction).rem_euclid(count as i32) as usize;
    }
}

fn signed_deck_offset(index: usize, scroll: f32, count: usize) -> f32 {
    let mut offset = index as f32 - scroll;
    let half = count as f32 / 2.0;
    while offset > half {
        offset -= count as f32;
    }
    while offset < -half {
        offset += count as f32;
    }
    offset
}

pub struct App {
    pub utility: Option<(u8,usize)>,
    theme_preset: ThemePreset,
    /// The UI language.
    ///
    /// Deliberately not part of [`SlotState`]: it lives in
    /// `linkSlot/Config/language.txt` so the Web page can change it without rewriting
    /// the device's state file.  The device keeps a copy because the menu must
    /// not read the card while a game is running.
    language: Language,
    /// When `linkSlot/Config/language.txt` was last read, throttling the poll that picks
    /// up a change made from the Web page.
    ///
    /// Deliberately a timestamp of *our* last look rather than the file's: the
    /// card is usually exFAT or FAT32, whose mtime has one- or two-second
    /// granularity, so a Web write can land on exactly the same stamp as the
    /// read before it and a mtime comparison would never notice.
    language_checked_at: Millis,
    pub guide: Option<GuideView>,
    pub cheat_rows: Vec<slot_store::cheats::Cheat>,
    cheat_key: String,
    pub cheat_pending: Option<Vec<String>>,
    phase: Phase,
    pub wifi_ui: Option<WifiUi>,
    pub sync_ui: Option<SyncUi>,
    upgrade_ui: Option<UpgradeUi>,
    /// Rebuild fallback covers once the eject animation returns to the shelf.
    fallback_refresh_after_eject: bool,
    sync_refresh_at: Millis,
    pub mods_ui: Option<ModsUi>,
    pub keyboard: Option<KeyboardState>,
    pub search_index: Option<crate::game_search::GameSearchIndex>,
    pub search_results: Option<SearchResults>,
    pub web_popup: Option<WebPopup>,
    pub link_popup: Option<LinkPopup>,
    /// The non-blocking notice under the status bar: text plus when it started.
    notice: Option<(String, String, Millis)>,
    web: Option<WebServer>,
    web_starting: Option<Receiver<Result<WebServer, String>>>,
    web_cancel: bool,
    wifi_pending: Option<(String, String)>,
    playtime_dir: Option<PathBuf>,
    playtime_ms: u64,
    shelves: Vec<(Rack, Shelf)>,
    shelf_at: usize,
    invalidated_faces: Vec<TexId>,
    face_generation: u64,
    carousel: CarouselKeys,
    shortcuts: ShortcutKeys,
    controls: Controls,
    controls_reload: bool,
    shortcut_reload: bool,
    play_held: Option<Millis>,
    /// A on the About page; held for [`ABOUT_DEBUG_HOLD_MS`] to enable debug logs.
    about_hold: Option<Millis>,
    about_page: slot_ui::AboutPage,
    refusal: Option<Refusal>,
    refused_from: Option<f32>,
    alert_face: Option<TexId>,
    shutdown_faces: Vec<(TexId, u32, u32)>,
    power_menu: Option<usize>,
    power_cards: Option<[(TexId, u32, u32); 2]>,
    power_labels: Vec<[(TexId, u32, u32); 2]>,
    power_icons: Vec<[(TexId, u32, u32); 2]>,
    power_title: Option<(TexId, u32, u32)>,
    core_picker: Option<CorePicker>,
    core_board_face: Option<TexId>,
    core_lid_face: Option<TexId>,
    core_faces_stem: Option<String>,
    core_socket_faces: Vec<TexId>,
    /// Chip faces keyed by platform, then core index.
    core_chip_faces: Vec<Vec<TexId>>,
    core_blank_chip_faces: Vec<TexId>,
    core_chip_shadow_face: Option<TexId>,
    front_deck: Option<FrontDeck>,
    front_deck_texs: Vec<(TexId, u32, u32)>,
    front_deck_title: Option<(TexId, u32, u32)>,
    front_deck_controls: Vec<(TexId, u32)>,
    front_deck_names: Vec<(TexId, u32, u32)>,
    front_deck_counter: Option<(TexId, u32, u32)>,
    network_face: Option<TexId>,
    network_face_off: Option<TexId>,
    sync_picker_face: Option<TexId>,
    /// The same chip with its pin-1 mark lit, used once the chip is fully open.
    sync_picker_face_lit: Option<TexId>,
    /// The face that was on screen just before a vertical switch.
    sync_picker_prev: Option<TexId>,
    sync_picker_swap: Option<Millis>,
    /// +1 when the selection moved down, -1 when it moved up.
    sync_picker_dir: f32,
    sync_mods: Vec<SyncModChoice>,
    sync_mod_index: usize,
    sync_picker: Option<SyncPicker>,
    core_legend_faces: Vec<(TexId, u32)>,
    game_menu: Option<GameMenu>,
    link_sprites: Option<LinkSprites>,
    link_hardware: LinkKind,
    last_role: LinkRow,
    link_choices: HashMap<String, LinkKind>,
    link_loaded: Option<&'static str>,
    link_reload: Option<(String, &'static str)>,
    reload: Option<Reload>,
    link_menu_faces: Vec<(TexId, u32, u32)>,
    link_linked_face: Option<(TexId, u32, u32)>,
    link_confirm_face: Option<(TexId, u32, u32)>,
    link_step_faces: Vec<(TexId, u32, u32)>,
    link_fail_faces: Vec<(TexId, u32, u32)>,
    link_legend_faces: Vec<(TexId, u32)>,
    starting: Option<LinkStarting>,
    link_transport: Option<(u16, Box<dyn LinkChannel>)>,
    /// Cartridge transfer room.  This is independent of play-link sessions.
    transfer: Option<Transfer>,
    /// Normal shelf insert animation for the card being handed over.
    transfer_insert_at: Option<Millis>,
    transfer_insert_stem: Option<String>,
    /// The card is in the slot until a rejection/duplicate/completion ejects it.
    transfer_seated: bool,
    transfer_eject_at: Option<Millis>,
    transfer_progress_started: Option<Millis>,
    transfer_done_pending: bool,
    transfer_faces: Option<TransferFaces>,
    /// A blank receiver card generated by the frontend before the real face
    /// arrives; without this the preview draws only the slot's placeholder.
    transfer_blank_face: Option<(TexId, u32, u32)>,
    restarting: bool,
    /// When the MENU press behind the open exit prompt went down, if one is
    /// running. Set on the press, cleared when it comes back up or when the
    /// hold completes, so the prompt's bar is a view of one press only.
    hold_exit: Option<Millis>,
    /// A restart was asked for in place: the session drops the old core once.
    restart_pending: bool,
    /// The replacement core must not restore the resume state written by the
    /// restart itself.  Unlike `restart_pending`, this stays set until the
    /// fresh core reports ready.
    restart_clean: bool,
    /// A cart to insert once the current one has left the slot: how the joiner
    /// follows the host into its game without the player touching the shelf.
    /// The core is the host's choice for that cart.
    insert_after_eject: Option<(String, Option<Core>)>,
    /// The peer started a game this side could switch to; waiting on the
    /// player's answer.
    follow_ask: Option<FollowAsk>,
    /// A shortcut row is waiting for the player to press its new key.
    shortcut_capture: Option<Shortcut>,
    /// The answer the session acts on (taken once).
    follow_answer: Option<FollowAnswer>,
    /// Famicom cart hashes, computed on demand for follow matching.
    rom_hashes: HashMap<String, Digest>,
    /// Content hashes of the whole card, persisted and filled in the
    /// background so a link request never reads ROMs on the UI thread.
    rom_index: RomIndex,
    repairing: bool,
    act_at: Millis,
    last_input_at: Millis,
    root: Option<PathBuf>,
    state: SlotState,
    vol_before: Vec<(u8, bool, Millis)>,
    snapshot: Option<Box<dyn Snapshot>>,
    core: Core,
    colour_pending: Option<bool>,
    palette_live: bool,
    palette_pending: Option<GbPalette>,
    link_player: Option<u8>,
    platform: Platform,
    named_core: bool,
    video_mode: VideoMode,
    link: Option<LinkSession>,
    sfx: Option<Sfx>,
    polaroids: Option<Polaroids>,
    pending: Option<(PendingUndo, Millis)>,
    legend_faces: Vec<TexId>,
    undo_face: Option<TexId>,
    clock_faces: Option<(TexId, TexId)>,
    quick_menu_faces: Option<QuickMenuFaces>,
    quick_clock_faces: Option<[(TexId, u32, u32); 2]>,
    about_face: Option<TexId>,
    wallpaper: Option<TexId>,
    /// The active machine's screen overlay, once the frontend has uploaded
    /// it. Drawn over the game, under the menus.
    fc_mask_face: Option<TexId>,
    /// Every Overlay/{FC,GB,GBC,SFC}/*.png on the card, sorted; index 0 is 无.
    fc_masks: Vec<String>,
    fc_mask_index: usize,
    gb_masks: Vec<String>,
    gb_mask_index: usize,
    gbc_masks: Vec<String>,
    gbc_mask_index: usize,
    sfc_masks: Vec<String>,
    sfc_mask_index: usize,
    /// File names drawn in the four mask rows (dynamic faces).
    fc_mask_value: Option<(TexId, u32, u32)>,
    gb_mask_value: Option<(TexId, u32, u32)>,
    gbc_mask_value: Option<(TexId, u32, u32)>,
    sfc_mask_value: Option<(TexId, u32, u32)>,
    /// The marker the previous run left behind when it died mid-game or while
    /// linked; `Some` means this boot follows an unclean exit.
    previous_run_note: Option<String>,
    shelf_backdrop: Option<TexId>,
    collection_overlays: [Option<TexId>; COLLECTION_KINDS],
    /// Per-rack nameplate drawn inside the cartridge slot (built-in plate or
    /// the card's `System/linkSlot/assets/carts/slot_back_*.png`).
    collection_slot_back: [Option<(TexId, u32, u32)>; COLLECTION_KINDS],
    battery_percent: slot_ui::Printed,
    bolt: Option<TexId>,
    shelf_platform: slot_ui::Printed,
    shelf_playtime_text: String,
    shelf_playtime_key: Option<(String, Platform)>,
    shelf_playtime_face: Option<(TexId, u32, u32)>,
    name_pending: bool,
    slot_letter: Option<char>,
    shelf_named: Option<Millis>,
    shelf_clock: slot_ui::Printed,
    hud: Hud,
    screen: f32,
    lift: f32,
    game_ready: bool,
    clock: f64,
    power: Option<Power>,
    dozed_at: Millis,
    woke_on_press: bool,
    autosave_at: Millis,
    pending_save: Option<std::thread::JoinHandle<()>>,
    card_failing: Arc<AtomicBool>,
    card_read_only: bool,
    card_check_at: Millis,
    select_held: bool,
    battery_at: Millis,
    charge_at: Millis,
    wifi_connected: bool,
    clock_sync_job: Option<Receiver<Result<i64, String>>>,
    clock_sync_retry_at: Millis,
    wifi_at: Millis,
    wifi_face: Option<(TexId, u32, u32)>,
    wifi_auto_off_minutes: u16,
    wifi_radio_on: Option<bool>,
    wifi_radio_job: Option<WifiJob>,
    wifi_radio_poll_at: Millis,
    wifi_idle: crate::wifi_power::WifiIdle,
    wifi_off_job: Option<WifiJob>,
    wifi_off_retry_at: Millis,
    shelf_legend: (Vec<(TexId, u32)>, Vec<(TexId, u32)>),
    headphones: bool,
    headphones_at: Millis,
    battery: Option<Battery>,
    last_led: Option<LedState>,
    powering_off: bool,
    radio: Box<dyn RadioJobs>,
    state_writer: crate::state_writer::StateWriter,
    sync_launcher: Box<dyn SyncLauncher>,
    /// `[sync] mode`: which units a long-press Y sync processes.
    sync_mode: linkcfg::SyncMode,
    /// The open long-press Y chooser, if any.
    sync_insert: Option<SyncInsert>,
    /// The last insert ended with the side card attached to the game
    /// card.  The sync stage keeps the card in the native slot instead of
    /// pulling it to the old left-side position.
    sync_hold_capability: bool,
    sync_pop_at: Option<Millis>,
    sync_warmup: Option<WifiWarmup>,
    sync_pending: Option<SyncRequest>,
    /// Power-on fade for the sync screen; same beat as a game launch.
    sync_open_at: Option<Millis>,
    /// Power-off fade once the sync result is ready.
    sync_close_at: Option<Millis>,
    /// Result banner fades in from black after the sync screen closes.
    sync_result_at: Option<Millis>,
    /// The static cloud network cartridge used when no manual mod is installed.
    auto_cloud_card: Option<(TexId, u32, u32)>,
    /// The card being swapped out from behind the game card.
    sync_swap_card: Option<(TexId, u32, u32)>,
    /// The chosen network cartridge, kept for the closing eject.
    sync_card: Option<(TexId, u32, u32)>,
    game_intro: Option<GameIntro>,
    shelf_sync: ShelfSync,
    sync_faces: SyncFaces,
    sync_art: Option<SyncArt>,
    /// Enabled `after_boot` actions, loaded once when the card is mounted.
    boot_mods: Option<BootSchedule>,
    boot_job: Option<BootJob>,
}

const FACE_AHEAD: i32 = 8;
const FACE_KEEP: i32 = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FaceWant {
    pub shelf: usize,
    pub index: usize,
    pub on_screen: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Rack {
    Recents,
    Favorites,
    Of(Platform),
}

impl Rack {
    fn platform(self) -> Option<Platform> {
        match self {
            Rack::Of(p) => Some(p),
            Rack::Recents | Rack::Favorites => None,
        }
    }

    fn name(self, lang: Language) -> &'static str {
        match self {
            Rack::Recents => slot_i18n::t(lang, "device.app.shelf.recents"),
            Rack::Favorites => slot_i18n::t(lang, "device.app.shelf.favorites"),
            Rack::Of(platform) => {
                let key = match platform {
                    Platform::Gba => "device.app.platform.gba",
                    Platform::Gb => "device.app.platform.gb",
                    Platform::Gbc => "device.app.platform.gbc",
                    Platform::Fc => "device.app.platform.fc",
                    Platform::Sfc => "device.app.platform.sfc",
                    Platform::Md => "device.app.platform.md",
                };
                slot_i18n::t(lang, key)
            }
        }
    }

    fn key(self) -> String {
        match self {
            Rack::Recents => "recents".into(),
            Rack::Favorites => "favorites".into(),
            Rack::Of(p) => p.dir_name().to_ascii_lowercase(),
        }
    }

    fn collection_kind(self) -> slot_ui::CollectionKind {
        match self {
            Rack::Recents => slot_ui::CollectionKind::Recents,
            Rack::Favorites => slot_ui::CollectionKind::Favorites,
            Rack::Of(Platform::Gba) => slot_ui::CollectionKind::Gba,
            Rack::Of(Platform::Gbc) => slot_ui::CollectionKind::Gbc,
            Rack::Of(Platform::Gb) => slot_ui::CollectionKind::Gb,
            Rack::Of(Platform::Fc) => slot_ui::CollectionKind::Fc,
            Rack::Of(Platform::Sfc) => slot_ui::CollectionKind::Sfc,
            Rack::Of(Platform::Md) => slot_ui::CollectionKind::Md,
        }
    }

    /// A small accent identity per collection.  The Switch UI keeps the dark
    /// neutral ground; only the collection plate and a wash use this colour.
    fn accent(self) -> [u8; 3] {
        match self {
            Rack::Recents => slot_ui::collection_style::recents().base,
            Rack::Favorites => slot_ui::collection_style::favorites().base,
            Rack::Of(platform) => slot_ui::collection_style::platform(platform).base,
        }
    }

    fn ink(self) -> [u8; 3] {
        match self {
            Rack::Recents => slot_ui::collection_style::recents().on_dark,
            Rack::Favorites => slot_ui::collection_style::favorites().on_dark,
            Rack::Of(platform) => slot_ui::collection_style::platform(platform).on_dark,
        }
    }
}

const RECENTS_AT: usize = 0;
const FAVORITES_AT: usize = 1;

/// How many collection kinds the boot face tables hold. Derived from
/// `CollectionKind::ALL` so adding a collection cannot leave these tables one
/// short — the SFC addition did exactly that, and the boot-time overlay loop
/// then panicked with an out-of-bounds index instead of reaching the shelf.
const COLLECTION_KINDS: usize = slot_ui::CollectionKind::ALL.len();

fn shelves_of(carts: Vec<Cart>, state: &SlotState) -> Vec<(Rack, Shelf)> {
    let mut rows: Vec<(Rack, Vec<Cart>)> = vec![
        (Rack::Recents, Vec::new()),
        (Rack::Favorites, Vec::new()),
    ];
    rows.extend(Platform::ALL.iter().map(|p| (Rack::Of(*p), Vec::new())));
    let original: Vec<Cart> = carts.clone();
    for cart in carts {
        if let Some((_, row)) = rows
            .iter_mut()
            .find(|(rack, _)| rack.platform() == Some(cart.platform))
        {
            row.push(cart);
        }
    }
    let recents = if state.show_recents {
        state
            .recents
            .iter()
            .filter_map(|r| {
                original
                    .iter()
                    .find(|c| c.platform == r.platform && c.stem == r.stem)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let favorites: Vec<Cart> = original
        .iter()
        .filter(|c| state.is_favorite(c.platform, &c.stem))
        .cloned()
        .collect();
    rows[RECENTS_AT].1 = recents;
    rows[FAVORITES_AT].1 = favorites;
    rows.into_iter()
        .map(|(rack, carts)| (rack, Shelf::new(carts)))
        .collect()
}

impl App {
    pub fn new(carts: Vec<Cart>) -> Self {
        let shelves = shelves_of(carts, &SlotState::default());
        let mut carousel = CarouselKeys::default();
        carousel.set(CarouselAction::Favorite, Binding::plain(Device::R2));
        let shelf_at = shelves
            .iter()
            .position(|(_, s)| !s.carts.is_empty())
            .unwrap_or(0);
        let state_writer = crate::state_writer::StateWriter::spawn();
        let card_failing = state_writer.card_failing();
        App {
            radio: radio_jobs(),
            state_writer,
            sync_launcher: Box::new(SystemLauncher::new()),
            sync_mode: linkcfg::SyncMode::default(),
            sync_insert: None,
            sync_hold_capability: false,
            sync_pop_at: None,
            sync_warmup: None,
            sync_pending: None,
            sync_open_at: None,
            sync_close_at: None,
            sync_result_at: None,
            auto_cloud_card: None,
            sync_swap_card: None,
            sync_card: None,
            game_intro: None,
            shelf_sync: ShelfSync::Idle,
            sync_faces: SyncFaces::default(),
            sync_art: None,
            boot_mods: None,
            boot_job: None,
            utility: None, guide: None,
            theme_preset: ThemePreset::Default,
            language: Language::default(),
            language_checked_at: 0,
            cheat_rows: Vec::new(), cheat_key: String::new(), cheat_pending: None,
            phase: Phase::Shelf,
            wifi_ui: None,
            sync_ui: None,
            upgrade_ui: None,
            fallback_refresh_after_eject: false,
            sync_refresh_at: 0,
            mods_ui: None,
            keyboard: None,
            search_index: None,
            search_results: None,
            web_popup: None,
            link_popup: None,
            notice: None,
            web: None,
            web_starting: None,
            web_cancel: false,
            wifi_pending: None,
            playtime_dir: None,
            playtime_ms: 0,
            shelves,
            shelf_at,
            invalidated_faces: Vec::new(),
            face_generation: 0,
            carousel,
            shortcuts: ShortcutKeys::default(),
            controls: Controls::default(),
            controls_reload: false,
            shortcut_reload: false,
            play_held: None,
            about_hold: None,
            about_page: slot_ui::AboutPage::Main,
            refusal: None,
            refused_from: None,
            alert_face: None,
            shutdown_faces: Vec::new(),
            power_menu: None,
            power_cards: None,
            power_labels: Vec::new(),
            power_icons: Vec::new(),
            power_title: None,
            core_picker: None,
            core_board_face: None,
            core_lid_face: None,
            core_faces_stem: None,
            core_socket_faces: Vec::new(),
            core_chip_faces: Vec::new(),
            core_blank_chip_faces: Vec::new(),
            core_chip_shadow_face: None,
            front_deck: None,
            front_deck_texs: Vec::new(),
            front_deck_title: None,
            front_deck_controls: Vec::new(),
            front_deck_names: Vec::new(),
            front_deck_counter: None,
            network_face: None,
            network_face_off: None,
            sync_picker_face: None,
            sync_picker_face_lit: None,
            sync_picker_prev: None,
            sync_picker_swap: None,
            sync_picker_dir: 1.0,
            sync_mods: Vec::new(),
            sync_mod_index: 0,
            sync_picker: None,
            core_legend_faces: Vec::new(),
            game_menu: None,
            link_sprites: None,
            link_hardware: LinkKind::Cable,
            last_role: LinkRow::Host,
            link_choices: HashMap::new(),
            link_loaded: None,
            link_reload: None,
            reload: None,
            link_menu_faces: Vec::new(),
            link_linked_face: None,
            link_confirm_face: None,
            link_step_faces: Vec::new(),
            link_fail_faces: Vec::new(),
            link_legend_faces: Vec::new(),
            starting: None,
            link_transport: None,
            transfer: None,
            transfer_insert_at: None,
            transfer_insert_stem: None,
            transfer_seated: false,
            transfer_eject_at: None,
            transfer_progress_started: None,
            transfer_done_pending: false,
            transfer_faces: None,
            transfer_blank_face: None,
            restarting: false,
            hold_exit: None,
            restart_pending: false,
            restart_clean: false,
            insert_after_eject: None,
            follow_ask: None,
            shortcut_capture: None,
            follow_answer: None,
            rom_hashes: HashMap::new(),
            rom_index: RomIndex::empty(),
            repairing: false,
            act_at: 0,
            last_input_at: 0,
            root: None,
            state: SlotState::default(),
            vol_before: Vec::new(),
            snapshot: None,
            core: Core::default(),
            colour_pending: None,
            palette_live: false,
            palette_pending: None,
            link_player: None,
            platform: Platform::default(),
            named_core: false,
            video_mode: VideoMode::default(),
            link: None,
            sfx: None,
            polaroids: None,
            pending: None,
            legend_faces: Vec::new(),
            undo_face: None,
            clock_faces: None,
            quick_menu_faces: None,
            quick_clock_faces: None,
            about_face: None,
            wallpaper: None,
            fc_mask_face: None,
            fc_masks: Vec::new(),
            fc_mask_index: 0,
            gb_masks: Vec::new(),
            gb_mask_index: 0,
            gbc_masks: Vec::new(),
            gbc_mask_index: 0,
            sfc_masks: Vec::new(),
            sfc_mask_index: 0,
            fc_mask_value: None,
            gb_mask_value: None,
            gbc_mask_value: None,
            sfc_mask_value: None,
            previous_run_note: None,
            shelf_backdrop: None,
            collection_overlays: [None; COLLECTION_KINDS],
            collection_slot_back: [None; COLLECTION_KINDS],
            battery_percent: slot_ui::Printed::default(),
            bolt: None,
            shelf_platform: slot_ui::Printed::default(),
            shelf_playtime_text: String::new(),
            shelf_playtime_key: None,
            shelf_playtime_face: None,
            name_pending: false,
            slot_letter: None,
            shelf_named: None,
            shelf_clock: slot_ui::Printed::default(),
            hud: Hud::new(),
            screen: 0.0,
            lift: 1.0,
            game_ready: false,
            clock: 0.0,
            power: None,
            dozed_at: 0,
            woke_on_press: false,
            autosave_at: AUTOSAVE_MS,
            pending_save: None,
            card_failing,
            card_read_only: false,
            card_check_at: 0,
            select_held: false,
            battery_at: BATTERY_POLL_MS,
            charge_at: CHARGE_POLL_MS,
            wifi_connected: false,
            clock_sync_job: None,
            clock_sync_retry_at: 0,
            wifi_at: 0,
            wifi_face: None,
            wifi_auto_off_minutes: 10,
            wifi_radio_on: None,
            wifi_radio_job: None,
            wifi_radio_poll_at: 0,
            wifi_idle: crate::wifi_power::WifiIdle::default(),
            wifi_off_job: None,
            wifi_off_retry_at: 0,
            shelf_legend: (Vec::new(), Vec::new()),
            headphones: false,
            headphones_at: 0,
            battery: None,
            last_led: None,
            powering_off: false,
        }
    }

    /// Start the cartridge scan while the EGL context is still coming up.
    /// The scan is the slowest disk walk of the boot path (~0.4 s for ~570
    /// games) and it only needs the card root, so it can run underneath
    /// `eglInitialize`/compositor setup.
    pub fn prefetch_scan_async(root: PathBuf) {
        let cell = PREFETCH_SCAN.get_or_init(|| Mutex::new(None));
        let mut slot = cell.lock().unwrap_or_else(|error| error.into_inner());
        if slot.is_some() {
            return;
        }
        let key = root.clone();
        let spawned = std::thread::Builder::new()
            .name("slot-boot-scan".into())
            .spawn(move || scan(&root).unwrap_or_default());
        if let Ok(handle) = spawned {
            *slot = Some((key, handle));
        }
    }

    /// Take (and join) the prefetched scan, if it was for this root.
    fn take_prefetched_scan(root: &Path) -> Option<Vec<Cart>> {
        let cell = PREFETCH_SCAN.get_or_init(|| Mutex::new(None));
        let pending = {
            let mut slot = cell.lock().unwrap_or_else(|error| error.into_inner());
            slot.take()
        };
        match pending {
            Some((scanned_root, handle)) if scanned_root == root => handle.join().ok(),
            Some((_, handle)) => {
                // A prefetch for another root (tests, or a remounted card):
                // let it finish quietly and scan normally below.
                let _ = handle.join();
                None
            }
            None => None,
        }
    }

    pub fn boot(root: &Path) -> Self {
        crate::root::ensure(root);
        let purged = crate::root::purge_legacy_carts(root);
        if purged > 0 {
            eprintln!("slot: carts: removed {purged} legacy stock nameplate(s)");
        }
        crate::boot_time::mark("root ready");
        // A theme pack is resolved before anything rasterises: the label font
        // and the palette are baked into cached textures for the whole run.
        let pack = crate::theme_pack::active(root);
        if let Some(font) = pack.as_ref().and_then(|pack| pack.font()) {
            match std::fs::read(&font) {
                Ok(bytes) => {
                    slot_ui::install_font(bytes);
                }
                Err(error) => {
                    eprintln!(
                        "slot: theme pack: could not read {}: {error}",
                        font.display()
                    );
                }
            }
        }
        let theme = match pack.as_ref() {
            Some(pack) => pack.theme(root),
            None => Theme::read(root),
        };
        let theme_preset = theme.preset;
        slot_ui::set_theme(theme);
        if let Some(pack) = &pack {
            eprintln!(
                "slot: theme pack: {} ({}) theme={} wallpaper={} font={}",
                pack.id,
                pack.name,
                pack.theme_file().is_some(),
                pack.wallpaper().is_some(),
                pack.font().is_some(),
            );
        }
        crate::boot_time::mark("root and theme");
        crate::boot_time::mark("scan start");
        let mut carts = Self::take_prefetched_scan(root)
            .unwrap_or_else(|| scan(root).unwrap_or_default());
        crate::boot_time::mark("scan: list");
        let rom_index = RomIndex::load(root);
        crate::boot_time::mark("scan: index");
        if let Err(error) = slot_store::cart_color::assign_missing(root, &mut carts) {
            eprintln!("slot: initial cart colours: {error}");
        }
        crate::boot_time::mark("scan: colours");
        if let Err(error) = slot_store::cart_meta::ensure_all(root, &carts) {
            eprintln!("slot: initial cart metadata: {error}");
        }
        crate::boot_time::mark("scan");
        let mut app = App::new(carts);
        crate::boot_time::mark("app constructed");
        app.root = Some(root.to_path_buf());
        app.rom_index = rom_index;
        // A marker left by the last run means it died mid-game or while linked
        // (crash, rail cut): say so once, in the same log the link uses.
        app.previous_run_note = linkcfg::read_last_run(root);
        if let Some(note) = app.previous_run_note.clone() {
            let detail = note.replace(['\n', '\r'], ", ");
            crate::link_log::warn_to(root, format!("previous run ended unexpectedly ({detail})"));
            linkcfg::clear_last_run(root);
        }
        if root.join("System/linkSlot/shell/linkslot.sh").is_file() {
            app.boot_mods = Some(BootSchedule::load(root));
        }
        app.state = read_slot_state(root);
        crate::boot_time::mark("state");
        app.language = slot_store::read_language(root);
        crate::boot_time::mark("language");
        app.theme_preset = theme_preset;
        if root.join("linkSlot/Config/carousel.txt").is_file() {
            app.carousel = slot_store::read_carousel(root);
        }
        if root.join("linkSlot/Config/shortcuts.txt").is_file() {
            app.shortcuts = slot_store::read_shortcuts(root);
        }
        if root.join("linkSlot/Config/controls.txt").is_file() {
            app.controls = slot_store::read_controls(root);
        }
        app.restock_recents();
        app.restock_favorites();
        crate::boot_time::mark("restock");
        let config = linkcfg::read(root);
        app.wifi_auto_off_minutes = config.wifi.auto_off_minutes;
        app.sync_mode = config.sync.mode;
        app.wifi_radio_poll_at = 0;
        if app.state.clock_set {
            app.start();
        } else {
            app.phase = clock_screen(system_secs(), 0, false);
        }
        crate::boot_time::mark("shelf started");
        app.refresh_masks();
        crate::boot_time::mark("masks");
        app
    }

    fn start(&mut self) {
        let seated = if self.single_cart() {
            Some((self.shelf_at, 0))
        } else {
            let stem = self.state.cart.clone();
            let platform = self.state.cart_platform;
            stem.and_then(|stem| self.seat_of(&stem, platform))
        };
        self.phase = Phase::Shelf;
        match seated {
            Some((at, i)) => {
                self.shelf_at = at;
                self.shelf_mut().select(i);
                self.insert(false);
                if let Phase::Inserting { resumed, t, .. } = &mut self.phase {
                    *resumed = true;
                    *t = INSERT_S;
                }
            }
            None => {
                if let Some((at, i)) = self.last_shelf() {
                    self.shelf_at = at;
                    self.shelves[at].1.select(i);
                }
                self.state.cart = None;
                self.state.cart_platform = None;
                self.name_pending = true;
            }
        }
    }

    fn last_shelf(&self) -> Option<(usize, usize)> {
        let key = self.state.shelf.as_deref()?;
        let cart = self.state.shelf_cart.as_ref()?;
        let at = self.shelves.iter().position(|(rack, _)| rack.key() == key)?;
        let i = self.shelves[at]
            .1
            .carts
            .iter()
            .position(|c| c.platform == cart.platform && c.stem == cart.stem)?;
        Some((at, i))
    }

    fn remember_shelf(&mut self) {
        let shelf = self.shelf();
        let Some(cart) = shelf.carts.get(shelf.index) else {
            return;
        };
        self.state.shelf_cart = Some(Recent {
            platform: cart.platform,
            stem: cart.stem.clone(),
        });
        self.state.shelf = Some(self.shelves[self.shelf_at].0.key());
        self.persist();
    }

    fn shelf(&self) -> &Shelf {
        &self.shelves[self.shelf_at].1
    }

    fn shelf_mut(&mut self) -> &mut Shelf {
        &mut self.shelves[self.shelf_at].1
    }

    fn switch_shelf(&mut self, by: i32) {
        let Some(to) = self.next_shelf(by) else {
            return;
        };
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_at = to;
        self.shelf_platform = slot_ui::Printed::default();
        self.slot_letter = None;
        self.shelf_named = None;
        self.name_pending = true;
    }

    fn jump_letter(&mut self, dir: i32) {
        let before = self.shelf().index;
        match dir > 0 {
            true => self.shelf_mut().jump_next_letter(),
            false => self.shelf_mut().jump_prev_letter(),
        }
        if self.shelf().index == before {
            return;
        }
        let Some(letter) = self.selected_stem().map(slot_store::initial) else {
            return;
        };
        if self.slot_letter != Some(letter) {
            self.shelf_platform = slot_ui::Printed::default();
        }
        self.slot_letter = Some(letter);
        self.shelf_named = None;
        self.name_pending = true;
    }

    fn slot_name_alpha(&self) -> f32 {
        let Some(at) = self.shelf_named else {
            return 0.0;
        };
        let t = self.now().saturating_sub(at);
        let level = if t < SLOT_NAME_IN_MS {
            t as f32 / SLOT_NAME_IN_MS as f32
        } else if t < SLOT_NAME_IN_MS + SLOT_NAME_HOLD_MS {
            1.0
        } else {
            let out = t - SLOT_NAME_IN_MS - SLOT_NAME_HOLD_MS;
            1.0 - (out as f32 / SLOT_NAME_OUT_MS as f32).min(1.0)
        };
        SLOT_NAME_ALPHA * ease(level)
    }

    fn next_shelf(&self, by: i32) -> Option<usize> {
        let n = self.shelves.len() as i32;
        (1..n)
            .map(|step| (self.shelf_at as i32 + by * step).rem_euclid(n) as usize)
            .find(|at| !self.shelves[*at].1.carts.is_empty())
    }

    fn restock(&mut self, at: usize, carts: Vec<Cart>) {
        let _old = std::mem::replace(&mut self.shelves[at].1, Shelf::new(carts));
        if self.shelf_at == at {
            if self.shelves[at].1.carts.is_empty() {
                self.shelf_at = self.next_shelf(1).unwrap_or(0);
            } else {
                self.shelves[at].1.select(0);
            }
        }
    }

    fn restock_recents(&mut self) {
        let carts = if self.state.show_recents {
            self.state
                .recents
                .iter()
                .filter_map(|r| {
                    self.carts()
                        .find(|c| c.platform == r.platform && c.stem == r.stem)
                        .cloned()
                })
                .collect()
        } else {
            Vec::new()
        };
        self.restock(RECENTS_AT, carts);
    }

    fn restock_favorites(&mut self) {
        let carts = self
            .carts()
            .filter(|c| self.state.is_favorite(c.platform, &c.stem))
            .cloned()
            .collect();
        self.restock(FAVORITES_AT, carts);
    }

    fn carousel_action(row: QuickRow) -> Option<CarouselAction> {
        match row {
            QuickRow::ShelfPrev => Some(CarouselAction::PreviousShelf),
            QuickRow::ShelfNext => Some(CarouselAction::NextShelf),
            QuickRow::ShelfFav => Some(CarouselAction::Favorite),
            _ => None,
        }
    }

    fn carousel_value(&self, action: CarouselAction) -> Option<QuickValue> {
        let binding = self.carousel.get(action)?;
        (binding.modifier.is_none()).then_some(QuickValue::Control(binding.button))
    }

    fn carousel_action_for(&self, physical: Btn) -> Option<CarouselAction> {
        CarouselAction::ALL.into_iter().find(|action| {
            self.carousel.get(*action).is_some_and(|binding| {
                binding.modifier.is_none() && app_device_btn(binding.button) == physical
            })
        })
    }

    fn change_carousel(&mut self, action: CarouselAction, right: bool) {
        let current = self
            .carousel
            .get(action)
            .map(|b| b.button)
            .unwrap_or(Device::L1);
        let at = Device::ALL
            .iter()
            .position(|d| *d == current)
            .unwrap_or(0);
        let to = Device::ALL[if right {
            (at + 1) % Device::ALL.len()
        } else {
            (at + Device::ALL.len() - 1) % Device::ALL.len()
        }];
        self.carousel.set(action, Binding::plain(to));
        if let Some(root) = &self.root {
            if let Err(error) = slot_store::write_carousel(root, &self.carousel) {
                eprintln!("slot: carousel keys: {error}");
            }
        }
    }

    pub fn take_shortcut_reload(&mut self) -> bool {
        std::mem::take(&mut self.shortcut_reload)
    }

    pub fn take_controls_reload(&mut self) -> bool {
        std::mem::take(&mut self.controls_reload)
    }

    fn control_row(row: QuickRow) -> Option<GameButton> {
        match row {
            QuickRow::ControlUp => Some(GameButton::Up),
            QuickRow::ControlDown => Some(GameButton::Down),
            QuickRow::ControlLeft => Some(GameButton::Left),
            QuickRow::ControlRight => Some(GameButton::Right),
            QuickRow::ControlA => Some(GameButton::A),
            QuickRow::ControlB => Some(GameButton::B),
            QuickRow::ControlSelect => Some(GameButton::Select),
            QuickRow::ControlStart => Some(GameButton::Start),
            QuickRow::ControlL => Some(GameButton::L),
            QuickRow::ControlR => Some(GameButton::R),
            QuickRow::ControlTurboA => Some(GameButton::TurboA),
            QuickRow::ControlTurboB => Some(GameButton::TurboB),
            _ => None,
        }
    }

    fn control_value(&self, button: GameButton) -> Option<QuickValue> {
        let binding = self.controls.game(button)?;
        binding
            .modifier
            .is_none()
            .then_some(QuickValue::Control(binding.button))
    }

    fn change_control(&mut self, button: GameButton, right: bool) {
        let current = self
            .controls
            .game(button)
            .map(|b| b.button)
            .unwrap_or(Device::A);
        let at = Device::ALL
            .iter()
            .position(|d| *d == current)
            .unwrap_or(0);
        let to = Device::ALL[if right {
            (at + 1) % Device::ALL.len()
        } else {
            (at + Device::ALL.len() - 1) % Device::ALL.len()
        }];
        let duplicate = GameButton::ALL.into_iter().any(|other| {
            other != button
                && self
                    .controls
                    .game(other)
                    .is_some_and(|binding| binding.button == to)
        });
        self.controls.set_game(button, Binding::plain(to));
        self.controls_reload = true;
        if duplicate {
            self.hud.toast(Toast::Duplicate, self.now());
        }
        if let Some(root) = &self.root {
            if let Err(error) = slot_store::write_controls(root, &self.controls) {
                eprintln!("slot: control bindings: {error}");
            }
        }
    }

    /// The reverse of shortcut_row, for the mapping popup's title.
    fn shortcut_row_of(shortcut: Shortcut) -> Option<QuickRow> {
        QuickRow::ALL
            .into_iter()
            .find(|row| Self::shortcut_row(*row) == Some(shortcut))
    }

    fn shortcut_row(row: QuickRow) -> Option<Shortcut> {
        match row {
            QuickRow::ShortcutSaveState => Some(Shortcut::SaveState),
            QuickRow::ShortcutLoadState => Some(Shortcut::LoadState),
            QuickRow::ShortcutBrightnessUp => Some(Shortcut::BrightnessUp),
            QuickRow::ShortcutBrightnessDown => Some(Shortcut::BrightnessDown),
            QuickRow::ShortcutBlueLightDown => Some(Shortcut::BlueLightDown),
            QuickRow::ShortcutBlueLightUp => Some(Shortcut::BlueLightUp),
            QuickRow::ShortcutColorCorrection => Some(Shortcut::ColorCorrection),
            QuickRow::ShortcutPaletteBack => Some(Shortcut::PaletteBack),
            QuickRow::ShortcutPaletteNext => Some(Shortcut::PaletteNext),
            QuickRow::ShortcutShaderNext => Some(Shortcut::ShaderNext),
            QuickRow::ShortcutShaderBack => Some(Shortcut::ShaderBack),
            QuickRow::ShortcutFastForward => Some(Shortcut::FastForward),
            QuickRow::ShortcutRewind => Some(Shortcut::Rewind),
            _ => None,
        }
    }

    fn shortcut_value(&self, shortcut: Shortcut) -> Option<QuickValue> {
        match self.shortcuts.get(shortcut) {
            Some(Binding {
                modifier: Some(Modifier::Select),
                button,
            }) => Some(QuickValue::Shortcut(button)),
            Some(Binding {
                modifier: Some(Modifier::Menu),
                button,
            }) => Some(QuickValue::MenuShortcut(button)),
            Some(Binding {
                modifier: None,
                button,
            }) => Some(QuickValue::PlainShortcut(button)),
            None => Some(QuickValue::Unbound),
            _ => Some(QuickValue::Off),
        }
    }

    fn change_shortcut(&mut self, shortcut: Shortcut, right: bool) {
        // The cycle is: 无 -> Select+key × N -> Menu+key × N -> plain key × N,
        // so a shortcut can be switched off from the menu and back on again.
        let n = Device::ALL.len();
        let (kind, at) = match self.shortcuts.get(shortcut) {
            Some(Binding {
                modifier: Some(Modifier::Select),
                button,
            }) => (0, Device::ALL.iter().position(|d| *d == button).unwrap_or(0)),
            Some(Binding {
                modifier: Some(Modifier::Menu),
                button,
            }) => (1, Device::ALL.iter().position(|d| *d == button).unwrap_or(0)),
            Some(Binding {
                modifier: None,
                button,
            }) => (2, Device::ALL.iter().position(|d| *d == button).unwrap_or(0)),
            None => (3, 0),
            _ => (0, 0),
        };
        let total = n * 3 + 1;
        let flat = (kind * n + at + if right { 1 } else { total - 1 }) % total;
        let binding = if flat == n * 3 {
            None
        } else {
            let next_kind = flat / n;
            let to = Device::ALL[flat % n];
            let modifier = match next_kind {
                1 => Modifier::Menu,
                2 => Modifier::Button(to),
                _ => Modifier::Select,
            };
            // Plain is represented by no modifier; Button would need a second key.
            if next_kind == 2 {
                Binding::plain(to)
            } else {
                Binding::with(modifier, to)
            }
        };
        let duplicate = Shortcut::ALL
            .into_iter()
            .any(|other| other != shortcut && self.shortcuts.get(other) == binding);
        self.shortcuts.set(shortcut, binding);
        self.shortcut_reload = true;
        if duplicate {
            self.hud.toast(Toast::Duplicate, self.now());
        }
        if let Some(root) = &self.root {
            if let Err(error) = slot_store::write_shortcuts(root, &self.shortcuts) {
                eprintln!("slot: shortcut keys: {error}");
            }
        }
    }

    /// Open the 「按一个键」 popup for one shortcut row.
    fn begin_shortcut_capture(&mut self, shortcut: Shortcut) {
        self.shortcut_capture = Some(shortcut);
        self.utility = Some((SHORTCUT_CAPTURE, 0));
    }

    /// Input while the popup waits for a key: a press becomes the binding
    /// (SEL held = a chord), M ends the input, a clash changes nothing.
    fn capture_shortcut_input(&mut self, action: Action) {
        let Some(shortcut) = self.shortcut_capture else {
            return;
        };
        match action {
            Action::QuickMenu => {
                // M is the way out. Nothing captured means "set to 无", which
                // asks once more before it forgets the old chord.
                self.utility = Some((SHORTCUT_CLEAR, 0));
            }
            Action::GbaDown(btn) => {
                let Some(device) = btn_device(btn) else {
                    return;
                };
                if device == Device::Select {
                    // SELECT is the chord modifier, not a key on its own.
                    return;
                }
                let binding = if self.select_held {
                    Binding::with(Modifier::Select, device)
                } else {
                    Binding::plain(device)
                };
                if Shortcut::ALL
                    .into_iter()
                    .any(|other| other != shortcut && self.shortcuts.get(other) == binding)
                {
                    self.hud.toast(Toast::Duplicate, self.now());
                    return;
                }
                self.set_shortcut(shortcut, binding);
                self.shortcut_capture = None;
                self.utility = None;
            }
            _ => {}
        }
    }

    fn confirm_clear_shortcut(&mut self) {
        if let Some(shortcut) = self.shortcut_capture.take() {
            self.set_shortcut(shortcut, None);
        }
        self.utility = None;
    }

    fn cancel_shortcut_capture(&mut self) {
        self.shortcut_capture = None;
        self.utility = None;
    }

    fn set_shortcut(&mut self, shortcut: Shortcut, binding: Option<Binding>) {
        self.shortcuts.set(shortcut, binding);
        self.shortcut_reload = true;
        if let Some(root) = &self.root {
            if let Err(error) = slot_store::write_shortcuts(root, &self.shortcuts) {
                eprintln!("slot: shortcut keys: {error}");
            }
        }
    }

    /// 「重置快捷键」: every shortcut back to the factory chord.
    fn reset_shortcuts(&mut self) {
        self.shortcuts = slot_store::ShortcutKeys::default();
        self.shortcut_reload = true;
        if let Some(root) = &self.root {
            if let Err(error) = slot_store::write_shortcuts(root, &self.shortcuts) {
                eprintln!("slot: shortcut keys: {error}");
            }
        }
        self.hud.toast(Toast::ShortcutsReset, self.now());
    }

    fn toggle_favorite(&mut self) {        let Some((platform, stem)) = self
            .shelf()
            .carts
            .get(self.shelf().index)
            .map(|c| (c.platform, c.stem.clone()))
        else {
            return;
        };
        let on = self.state.toggle_favorite(platform, &stem);
        self.restock_favorites();
        self.persist();
        let said = if on {
            Toast::FavoriteOn
        } else {
            Toast::FavoriteOff
        };
        self.hud.toast(said, self.now());
    }

    fn remember_played(&mut self, platform: Platform, stem: &str) {
        self.state.played(platform, stem);
        self.restock_recents();
        self.persist();
    }

    fn seat_of(&self, stem: &str, platform: Option<Platform>) -> Option<(usize, usize)> {
        self.shelves
            .iter()
            .enumerate()
            .filter(|(_, (rack, _))| platform.is_none_or(|want| rack.platform() == Some(want)))
            .find_map(|(at, (_, shelf))| {
                shelf
                    .carts
                    .iter()
                    .position(|c| c.stem == stem)
                    .map(|i| (at, i))
            })
    }

    pub fn confirm_clock(&mut self) {
        let Phase::SetClock {
            picker,
            seed,
            from_menu,
        } = &self.phase
        else {
            return;
        };
        let (moved, offset, from_menu) = (picker.secs() - *seed, picker.offset_min(), *from_menu);
        let utc = self.utc_secs() + moved;
        if let Some(power) = &mut self.power {
            power.set_clock(utc);
        }
        self.state.utc_offset_min = offset as i16;
        self.state.clock_set = true;
        self.persist();
        if from_menu {
            self.phase = Phase::QuickMenu {
                row: QuickRow::DateTime,
            };
        } else {
            self.start();
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn wall_secs(&self) -> i64 {
        self.utc_secs() + i64::from(self.state.utc_offset_min) * 60
    }

    pub fn utc_offset_min(&self) -> i16 {
        self.state.utc_offset_min
    }

    /// What the host shares when a link connects: the UTC clock and the offset
    /// its owner chose, so both screens show the same local time.
    pub fn clock_sync_payload(&self) -> (i64, i16) {
        (self.utc_secs(), self.state.utc_offset_min)
    }

    /// Take the host's clock after a link connects.
    pub fn apply_clock_sync(&mut self, utc: i64, offset_min: i16) {
        if let Some(power) = &mut self.power {
            power.set_clock(utc);
        }
        self.state.utc_offset_min = offset_min;
        self.state.clock_set = true;
        self.persist();
        self.hud.toast(Toast::ClockSynced, self.now());
    }

    fn utc_secs(&self) -> i64 {
        self.power.as_ref().map_or_else(system_secs, |p| p.now())
    }

    pub fn picker(&self) -> Option<&ClockPicker> {
        match &self.phase {
            Phase::SetClock { picker, .. } => Some(picker),
            _ => None,
        }
    }

    pub fn quick_menu(&self) -> Option<QuickRow> {
        match self.phase {
            Phase::QuickMenu { row } => Some(row),
            _ => None,
        }
    }

    pub fn quick_value(&self, row: QuickRow) -> Option<QuickValue> {
        match row {
            QuickRow::FastForward => QuickValue::speed(self.state.ff_speed),
            QuickRow::FastForwardSound => Some(QuickValue::flag(self.state.ff_sound)),
            QuickRow::ColourCorrection => Some(QuickValue::flag(self.state.colour_correction)),
            QuickRow::GbPalettes => Some(if self.state.gb_palettes {
                QuickValue::Palette(self.state.gb_palette)
            } else {
                QuickValue::Off
            }),
            QuickRow::ShelfPrev => self.carousel_value(CarouselAction::PreviousShelf),
            QuickRow::ShelfNext => self.carousel_value(CarouselAction::NextShelf),
            QuickRow::ShelfFav => self.carousel_value(CarouselAction::Favorite),
            QuickRow::ShortcutSaveState => self.shortcut_value(Shortcut::SaveState),
            QuickRow::ShortcutLoadState => self.shortcut_value(Shortcut::LoadState),
            QuickRow::ShortcutBrightnessUp => self.shortcut_value(Shortcut::BrightnessUp),
            QuickRow::ShortcutBrightnessDown => self.shortcut_value(Shortcut::BrightnessDown),
            QuickRow::ShortcutBlueLightDown => self.shortcut_value(Shortcut::BlueLightDown),
            QuickRow::ShortcutBlueLightUp => self.shortcut_value(Shortcut::BlueLightUp),
            QuickRow::ShortcutColorCorrection => self.shortcut_value(Shortcut::ColorCorrection),
            QuickRow::ShortcutPaletteBack => self.shortcut_value(Shortcut::PaletteBack),
            QuickRow::ShortcutPaletteNext => self.shortcut_value(Shortcut::PaletteNext),
            QuickRow::ShortcutShaderNext => self.shortcut_value(Shortcut::ShaderNext),
            QuickRow::ShortcutShaderBack => self.shortcut_value(Shortcut::ShaderBack),
            QuickRow::ShortcutFastForward => self.shortcut_value(Shortcut::FastForward),
            QuickRow::ShortcutRewind => self.shortcut_value(Shortcut::Rewind),
            QuickRow::ControlUp => self.control_value(GameButton::Up),
            QuickRow::ControlDown => self.control_value(GameButton::Down),
            QuickRow::ControlLeft => self.control_value(GameButton::Left),
            QuickRow::ControlRight => self.control_value(GameButton::Right),
            QuickRow::ControlA => self.control_value(GameButton::A),
            QuickRow::ControlB => self.control_value(GameButton::B),
            QuickRow::ControlSelect => self.control_value(GameButton::Select),
            QuickRow::ControlStart => self.control_value(GameButton::Start),
            QuickRow::ControlL => self.control_value(GameButton::L),
            QuickRow::ControlR => self.control_value(GameButton::R),
            QuickRow::ControlTurboA => self.control_value(GameButton::TurboA),
            QuickRow::ControlTurboB => self.control_value(GameButton::TurboB),
            QuickRow::ControlPreset => Some(match self.state.button_layout {
                slot_store::ButtonLayout::Off => QuickValue::Off,
                slot_store::ButtonLayout::SwapShoulders => QuickValue::SwapShoulders,
                slot_store::ButtonLayout::XyShoulders => QuickValue::XyShoulders,
                slot_store::ButtonLayout::Both => QuickValue::Both,
            }),
            QuickRow::GbaShader => Some(shader_value(self.state.shader_gba)),
            QuickRow::GbShader => Some(shader_value(self.state.shader_gb)),
            QuickRow::Rumble => Some(QuickValue::flag(self.state.rumble)),
            QuickRow::SaveSettings => None,
            QuickRow::DefaultLoad | QuickRow::DefaultSave => row
                .resume_default()
                .map(|kind| QuickValue::flag(self.state.resume_default(kind))),
            QuickRow::LoadGba
            | QuickRow::SaveGba
            | QuickRow::LoadGb
            | QuickRow::SaveGb
            | QuickRow::LoadGbc
            | QuickRow::SaveGbc => row.resume().map(|(platform, kind)| {
                match self.state.resume_override(platform, kind) {
                    Some(on) => QuickValue::flag(on),
                    None => QuickValue::Follow,
                }
            }),
            QuickRow::Brightness => Some(QuickValue::level(self.state.brightness)),
            QuickRow::BlueLight => Some(QuickValue::level(self.state.blue_light)),
            QuickRow::Recents => Some(QuickValue::flag(self.state.show_recents)),
            QuickRow::Theme => None,
            QuickRow::ThemeDefault => Some(self.theme_value(ThemePreset::Default)),
            QuickRow::ThemeWeb => Some(self.theme_value(ThemePreset::Web)),
            QuickRow::ThemeWarm => Some(self.theme_value(ThemePreset::Warm)),
            QuickRow::RomTitle => Some(QuickValue::flag(self.state.show_rom_title)),
            QuickRow::FallbackCover => Some(QuickValue::flag(self.state.fallback_cover)),
            QuickRow::LidTimeout => Some(match self.state.lid_minutes { 10=>QuickValue::Minutes10,30=>QuickValue::Minutes30,0=>QuickValue::Never,_=>QuickValue::Minutes3 }),
            QuickRow::Language => Some(match self.language {
                Language::ZhCn => QuickValue::LanguageZh,
                Language::EnUs => QuickValue::LanguageEn,
            }),
            QuickRow::ButtonLayout => None,
            QuickRow::Turbo => Some(QuickValue::flag(self.turbo())),
            QuickRow::Rewind => Some(QuickValue::flag(self.state.rewind)),
            QuickRow::SyncContent => Some(sync_mode_value(self.sync_mode)),
            QuickRow::CartTransfer => None,
            QuickRow::FcMask => Some(QuickValue::flag(self.fc_mask_index > 0)),
            QuickRow::GbMask => Some(QuickValue::flag(self.gb_mask_index > 0)),
            QuickRow::GbcMask => Some(QuickValue::flag(self.gbc_mask_index > 0)),
            QuickRow::SfcMask => Some(QuickValue::flag(self.sfc_mask_index > 0)),
            QuickRow::SplitLink => None,
            QuickRow::ShortcutReset => None,
            QuickRow::Shortcuts | QuickRow::DateTime | QuickRow::Mods | QuickRow::ButtonSettings
            | QuickRow::SystemSettings | QuickRow::About | QuickRow::SystemUpgrade | QuickRow::Screen
            | QuickRow::Game | QuickRow::Network | QuickRow::ShelfKeys | QuickRow::ShortcutMap
            | QuickRow::Wifi | QuickRow::Web | QuickRow::SyncStorage | QuickRow::Shelf
            | QuickRow::ShelfLook | QuickRow::DisplayPanel | QuickRow::DisplayEffects
            | QuickRow::Masks => None,
        }
    }

    pub fn set_quick_menu_faces(&mut self, faces: QuickMenuFaces) {
        self.quick_menu_faces = Some(faces);
    }

    /// The language the UI should be drawn in.
    ///
    /// `Frontend` compares this against the language its baked faces were built
    /// for; a mismatch is what triggers the texture rebuild.
    pub fn language(&self) -> Language {
        self.language
    }

    /// 系统 → 语言, left/right.
    ///
    /// The in-memory choice is applied first and unconditionally.  A card that
    /// refuses the write must still let someone who switched by accident get
    /// back to a menu they can read — the notice explains that the choice lasts
    /// only until the next boot.
    fn cycle_language(&mut self, right: bool) {
        let next = self.language.next(right);
        if next == self.language {
            return;
        }
        self.language = next;
        let title = slot_i18n::t(next, "device.app.language.title");
        let Some(root) = self.root.clone() else {
            return;
        };
        match slot_store::write_language(&root, next) {
            Ok(()) => {
                let detail = slot_i18n::t_args(
                    next,
                    "device.app.language.switched",
                    &[("name", next.menu_label())],
                );
                self.say_notice(title, detail);
            }
            Err(error) => {
                eprintln!("slot: language: {error}");
                let detail = slot_i18n::t(next, "device.app.language.not_written");
                self.say_notice(title, detail);
            }
        }
    }

    /// Picks up a language written by the Web page.
    ///
    /// At most one small read a second, compared by content.  Comparing the
    /// file's mtime looked cheaper but is wrong on the cards this ships on:
    /// exFAT and FAT32 stamp whole seconds (FAT32 two of them), so a Web write
    /// that lands inside the same stamp as the previous read is invisible and
    /// the menu stays in the old language until something else touches the file.
    /// Four bytes a second is not worth that bug.
    ///
    /// A missing or unreadable file leaves the current language alone rather
    /// than snapping back to Chinese mid-session.
    fn poll_language_file(&mut self) {
        if self.now().saturating_sub(self.language_checked_at) < LANGUAGE_POLL_MS {
            return;
        }
        self.language_checked_at = self.now();
        let Some(root) = self.root.as_deref() else {
            return;
        };
        let Some(code) = slot_store::read_language_code(root) else {
            return;
        };
        if code != self.language {
            self.language = code;
        }
    }

    pub fn set_quick_clock_faces(&mut self, dim: (TexId, u32, u32), lit: (TexId, u32, u32)) {
        self.quick_clock_faces = Some([dim, lit]);
    }

    pub fn set_about_face(&mut self, face: TexId) {
        self.about_face = Some(face);
    }

    pub fn set_clock_faces(&mut self, line: TexId, hint: TexId) {
        self.clock_faces = Some((line, hint));
    }

    pub fn set_cart_shadow(&mut self, face: TexId) {
        for (_, shelf) in &mut self.shelves {
            shelf.set_shadow(face);
        }
    }

    pub fn set_gb_cart_shadows(&mut self, notched: TexId, rounded: TexId) {
        for (_, shelf) in &mut self.shelves {
            shelf.set_gb_shadow(GbShell::Notched, notched);
            shelf.set_gb_shadow(GbShell::Rounded, rounded);
        }
    }

    pub fn set_wallpaper(&mut self, face: TexId) {
        self.wallpaper = Some(face);
    }

    /// The frontend owns the active mask texture; the app only decides when
    /// to draw it. Only one machine can be running, so one face is enough.
    pub fn set_mask_face(&mut self, face: Option<TexId>) {
        self.fc_mask_face = face;
    }

    pub fn mask_face(&self) -> Option<TexId> {
        self.fc_mask_face
    }

    /// Legacy FC-only name used by the FC mask tests.
    pub fn fc_mask_wanted(&self) -> bool {
        self.platform == Platform::Fc && self.mask_wanted()
    }

    /// The overlay is only drawn when the running machine has a mask and the
    /// player has selected one.
    pub fn mask_wanted(&self) -> bool {
        let Some((files, index)) = self.mask_entries(self.platform) else {
            return false;
        };
        index > 0 && !files.is_empty()
    }

    /// The mask file the running machine uses, if any.
    pub fn current_mask(&self) -> Option<&str> {
        let (files, index) = self.mask_entries(self.platform)?;
        files.get(index.checked_sub(1)?).map(String::as_str)
    }

    /// The FC mask file, for the existing FC tests and callers.
    pub fn current_fc_mask(&self) -> Option<&str> {
        self.fc_masks
            .get(self.fc_mask_index.checked_sub(1)?)
            .map(String::as_str)
    }

    /// What the menu row for one machine prints: 无 or the chosen file name.
    pub fn mask_label_for(&self, platform: Platform) -> String {
        let lang = self.language;
        self.mask_entries(platform)
            .and_then(|(files, index)| files.get(index.checked_sub(1)?))
            .cloned()
            .unwrap_or_else(|| slot_i18n::t(lang, "device.app.common.none").to_string())
    }

    pub fn fc_mask_label(&self) -> String {
        self.mask_label_for(Platform::Fc)
    }

    pub fn set_fc_mask_value(&mut self, value: Option<(TexId, u32, u32)>) {
        self.fc_mask_value = value;
    }

    pub fn set_gb_mask_value(&mut self, value: Option<(TexId, u32, u32)>) {
        self.gb_mask_value = value;
    }

    pub fn set_gbc_mask_value(&mut self, value: Option<(TexId, u32, u32)>) {
        self.gbc_mask_value = value;
    }

    pub fn set_sfc_mask_value(&mut self, value: Option<(TexId, u32, u32)>) {
        self.sfc_mask_value = value;
    }

    pub fn fc_mask_value(&self) -> Option<(TexId, u32, u32)> {
        self.fc_mask_value
    }

    pub fn gb_mask_value(&self) -> Option<(TexId, u32, u32)> {
        self.gb_mask_value
    }

    pub fn gbc_mask_value(&self) -> Option<(TexId, u32, u32)> {
        self.gbc_mask_value
    }

    pub fn sfc_mask_value(&self) -> Option<(TexId, u32, u32)> {
        self.sfc_mask_value
    }

    /// Directory below `Overlay/` for a mask-capable machine.
    pub fn mask_dir(platform: Platform) -> Option<&'static str> {
        match platform {
            Platform::Fc => Some("FC"),
            Platform::Gb => Some("GB"),
            Platform::Gbc => Some("GBC"),
            Platform::Sfc => Some("SFC"),
            Platform::Md => None,
            Platform::Gba => None,
        }
    }

    pub fn mask_dir_name(&self) -> Option<&'static str> {
        Self::mask_dir(self.platform)
    }

    fn mask_entries(&self, platform: Platform) -> Option<(&[String], usize)> {
        match platform {
            Platform::Fc => Some((&self.fc_masks, self.fc_mask_index)),
            Platform::Gb => Some((&self.gb_masks, self.gb_mask_index)),
            Platform::Gbc => Some((&self.gbc_masks, self.gbc_mask_index)),
            Platform::Sfc => Some((&self.sfc_masks, self.sfc_mask_index)),
            Platform::Md => None,
            Platform::Gba => None,
        }
    }

    /// Re-read Overlay/{FC,GB,GBC,SFC}/*.png and resolve each saved selection.
    pub fn refresh_masks(&mut self) {
        let root = self.root.clone();
        let scan = |dir: &str| {
            root.as_deref()
                .map(|root| scan_masks(root, dir))
                .unwrap_or_default()
        };
        self.fc_masks = scan("FC");
        self.gb_masks = scan("GB");
        self.gbc_masks = scan("GBC");
        self.sfc_masks = scan("SFC");
        self.fc_mask_index = resolve_mask_index(&self.fc_masks, self.state.fc_mask.as_deref());
        self.gb_mask_index = resolve_mask_index(&self.gb_masks, self.state.gb_mask.as_deref());
        self.gbc_mask_index = resolve_mask_index(&self.gbc_masks, self.state.gbc_mask.as_deref());
        self.sfc_mask_index = resolve_mask_index(&self.sfc_masks, self.state.sfc_mask.as_deref());
    }

    /// Compatibility wrapper for the FC mask tests.
    pub fn refresh_fc_masks(&mut self) {
        self.refresh_masks();
    }

    fn cycle_mask(&mut self, platform: Platform, right: bool) {
        let Some((files, index)) = self.mask_entries(platform) else {
            return;
        };
        let count = files.len();
        let next = if right {
            (index + 1) % (count + 1)
        } else {
            (index + count) % (count + 1)
        };
        let name = next.checked_sub(1).and_then(|at| files.get(at)).cloned();
        match platform {
            Platform::Fc => {
                self.fc_mask_index = next;
                self.state.fc_mask = name;
            }
            Platform::Gb => {
                self.gb_mask_index = next;
                self.state.gb_mask = name;
            }
            Platform::Gbc => {
                self.gbc_mask_index = next;
                self.state.gbc_mask = name;
            }
            Platform::Sfc => {
                self.sfc_mask_index = next;
                self.state.sfc_mask = name;
            }
            Platform::Md | Platform::Gba => {}
        }
    }

    pub fn set_shelf_backdrop(&mut self, face: TexId) {
        self.shelf_backdrop = Some(face);
    }

    pub fn set_collection_overlay(&mut self, kind: slot_ui::CollectionKind, face: TexId) {
        self.collection_overlays[kind.index()] = Some(face);
    }

    /// The rack's nameplate, shown inside the cartridge slot behind the cart.
    pub fn set_collection_slot_back(
        &mut self,
        kind: slot_ui::CollectionKind,
        face: Option<(TexId, u32, u32)>,
    ) {
        self.collection_slot_back[kind.index()] = face;
    }

    fn collection_slot_back(
        &self,
        kind: slot_ui::CollectionKind,
    ) -> Option<(TexId, u32, u32)> {
        self.collection_slot_back[kind.index()]
    }

    fn collection_overlay(&self, kind: slot_ui::CollectionKind) -> Option<TexId> {
        self.collection_overlays[kind.index()]
    }

    pub fn set_bolt_face(&mut self, bolt: TexId) {
        self.bolt = Some(bolt);
    }

    pub fn set_wifi_face(&mut self, face: TexId, w: u32, h: u32) {
        self.wifi_face = Some((face, w, h));
    }

    pub fn set_shelf_legend_faces(&mut self, left: Vec<(TexId, u32)>, right: Vec<(TexId, u32)>) {
        self.shelf_legend = (left, right);
    }

    pub fn set_shelf_platform_face(&mut self, face: TexId, w: u32) {
        self.shelf_platform = slot_ui::Printed::new(face, w);
    }

    pub fn set_shelf_platform_face_sized(&mut self, face: TexId, w: u32, h: u32) {
        self.shelf_platform = slot_ui::Printed::sized(face, w, h);
    }

    pub fn clear_shelf_platform(&mut self) {
        self.shelf_platform = slot_ui::Printed::default();
    }

    pub fn slot_text(&self) -> Option<String> {
        match self.slot_letter {
            Some(letter) => Some(letter.to_string()),
            None => self.shelf_platform_name().map(str::to_string),
        }
    }

    pub fn shelf_platform_name(&self) -> Option<&'static str> {
        Some(self.shelves[self.shelf_at].0.name(self.language))
    }

    /// Colour of the shelf's platform / collection name.  A one-letter jump
    /// keeps the neutral ink so only real collection identities are coloured.
    pub fn shelf_collection_ink(&self) -> [u8; 3] {
        if self.slot_letter.is_some() {
            slot_ui::switch_style::ink()
        } else {
            self.shelves[self.shelf_at].0.ink()
        }
    }

    pub fn set_battery_percent_face(&mut self, face: TexId, w: u32) {
        self.battery_percent = slot_ui::Printed::new(face, w);
    }

    pub fn set_shelf_clock_face(&mut self, face: TexId, w: u32) {
        self.shelf_clock = slot_ui::Printed::new(face, w);
    }

    pub fn shelf_playtime_text(&self) -> &str {
        &self.shelf_playtime_text
    }

    pub fn set_shelf_playtime_face(&mut self, face: TexId, w: u32, h: u32) {
        self.shelf_playtime_face = Some((face, w, h));
    }

    pub fn clear_shelf_playtime_face(&mut self) {
        self.shelf_playtime_face = None;
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn carts(&self) -> impl Iterator<Item = &Cart> {
        self.shelves
            .iter()
            .filter(|(rack, _)| rack.platform().is_some())
            .flat_map(|(_, shelf)| shelf.carts.iter())
    }

    pub fn seated_cart(&self) -> Option<&Cart> {
        let stem = match &self.phase {
            Phase::Inserting { cart, .. }
            | Phase::Playing { cart }
            | Phase::Ejecting { cart, .. }
            | Phase::Polaroids { cart } => cart,
            Phase::Doze { cart: Some(cart) } => cart,
            _ => return None,
        };
        self.shelf().carts.iter().find(|c| c.stem == *stem)
    }

    pub fn single_cart(&self) -> bool {
        self.carts().count() == 1
    }

    pub fn set_faces(&mut self, faces: Vec<TexId>) {
        let mut faces = faces.into_iter();
        for (_, shelf) in &mut self.shelves {
            let n = shelf.carts.len();
            shelf.set_faces(faces.by_ref().take(n).collect());
        }
    }

    pub fn face_wants(&self) -> Vec<FaceWant> {
        let here = self.shelf();
        let screen = here.on_screen();
        let mut wants: Vec<(usize, usize)> = screen.iter().map(|i| (self.shelf_at, *i)).collect();
        for at in self.neighbour_shelves() {
            wants.extend(self.shelves[at].1.on_screen().into_iter().map(|i| (at, i)));
        }
        wants.extend(
            here.around(FACE_AHEAD)
                .into_iter()
                .filter(|i| !screen.contains(i))
                .map(|i| (self.shelf_at, i)),
        );
        wants
            .into_iter()
            .filter(|(at, i)| self.shelves[*at].1.face(*i).is_none())
            .map(|(shelf, index)| FaceWant {
                shelf,
                index,
                on_screen: shelf == self.shelf_at && screen.contains(&index),
            })
            .collect()
    }

    pub(crate) fn face_generation(&self) -> u64 {
        self.face_generation
    }

    pub(crate) fn take_invalidated_faces(&mut self) -> Vec<TexId> {
        std::mem::take(&mut self.invalidated_faces)
    }

    fn invalidate_cart_faces(&mut self) {
        self.face_generation = self.face_generation.wrapping_add(1);
        let mut faces = Vec::new();
        for (_, shelf) in &mut self.shelves {
            faces.extend(shelf.take_faces());
        }
        self.invalidated_faces = faces;
        self.core_faces_stem = None;
    }

    /// Re-read cart labels after the Web page or a manual mod edits a cover,
    /// intro or guide.  ROM bytes share the same path, so only the label and
    /// resource metadata can change here.
    fn refresh_library_after_web(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let carts = match slot_store::scan_quiet(&root) {
            Ok(carts) => carts,
            Err(error) => {
                eprintln!("slot: web library refresh: {error}");
                return;
            }
        };
        let _ = slot_store::cart_meta::ensure_all(&root, &carts);
        let labels: HashMap<PathBuf, (Option<PathBuf>, Option<slot_store::ShellChoice>, slot_store::CartModel, Option<PathBuf>)> = carts
            .into_iter()
            .map(|cart| {
                (
                    cart.rom.clone(),
                    (cart.label, cart.shell, cart.model, cart.front),
                )
            })
            .collect();
        for (_, shelf) in &mut self.shelves {
            for cart in &mut shelf.carts {
                if let Some((label, shell, model, front)) = labels.get(&cart.rom) {
                    cart.label = label.clone();
                    cart.shell = *shell;
                    cart.model = *model;
                    cart.front = front.clone();
                }
            }
        }
        self.invalidate_cart_faces();
    }

    pub(crate) fn fallback_cover_enabled(&self) -> bool {
        self.state.fallback_cover
    }

    pub fn face_cart(&self, shelf: usize, index: usize) -> Option<&Cart> {
        self.shelves.get(shelf)?.1.carts.get(index)
    }

    pub fn set_cart_face(&mut self, shelf: usize, index: usize, tex: TexId) -> Option<TexId> {
        self.shelves.get_mut(shelf)?.1.set_face(index, tex)
    }

    pub fn shed_faces(&mut self) -> Vec<TexId> {
        let neighbours = self.neighbour_shelves();
        let mut shed = Vec::new();
        for (at, (_, shelf)) in self.shelves.iter_mut().enumerate() {
            let keep = if at == self.shelf_at {
                shelf.around(FACE_KEEP)
            } else if neighbours.contains(&at) {
                shelf.on_screen()
            } else {
                Vec::new()
            };
            for i in 0..shelf.carts.len() {
                if !keep.contains(&i) {
                    shed.extend(shelf.take_face(i));
                }
            }
        }
        shed
    }

    fn neighbour_shelves(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for by in [1, -1] {
            if let Some(at) = self.next_shelf(by) {
                if !out.contains(&at) {
                    out.push(at);
                }
            }
        }
        out
    }

    pub fn set_snapshot(&mut self, snapshot: Box<dyn Snapshot>) {
        self.snapshot = Some(snapshot);
    }

    pub fn set_core(&mut self, core: Core) {
        self.core = core;
        self.invalidate_shelf_playtime();
    }

    pub fn set_platform(&mut self, platform: Platform) {
        self.platform = platform;
    }

    pub fn set_named_core(&mut self, named: bool) {
        self.named_core = named;
    }

    pub fn set_video_mode(&mut self, mode: VideoMode) {
        self.video_mode = mode;
    }

    pub fn source_rect(&self) -> [f32; 4] {
        video_mode::source_rect(self.platform, self.video_mode)
    }

    /// L1's "fill the height" mode for the 8:7 machines. It keeps the picture's
    /// aspect, so SFC/FC lose the top and bottom bars and only keep side bars;
    /// GB/GBC keep their older crop-and-stretch behaviour instead.
    pub fn picture_fit(&self) -> bool {
        self.video_mode == VideoMode::Stretch
            && matches!(
                self.platform,
                Platform::Fc | Platform::Sfc | Platform::Md
            )
    }

    /// Whether the frontend owns L1/R1 outright.
    ///
    /// The Advance's L/R belong to the game. Every other machine has no
    /// shoulder of its own, so L1/R1 stay the frontend's — except the Mega
    /// Drive, whose L/R are its six-button X/Z. On MD the frontend only takes
    /// them during the SELECT+L1/R1 display chord.
    fn slot_owns_the_shoulders(&self) -> bool {
        matches!(self.phase, Phase::Playing { .. }) && self.platform != Platform::Gba
            && self.platform != Platform::Md
    }

    fn md_display_chord(&self) -> bool {
        matches!(self.phase, Phase::Playing { .. })
            && self.platform == Platform::Md
            && self.select_held
    }

    /// Whether L1/R1 select the picture mode rather than going to the game.
    fn shoulders_change_picture(&self) -> bool {
        if self.platform == Platform::Md {
            self.md_display_chord()
        } else {
            self.slot_owns_the_shoulders()
        }
    }

    pub fn taken_buttons(&self) -> &'static [Btn] {
        if self.slot_owns_the_shoulders() || self.md_display_chord() {
            &[Btn::L1, Btn::R1]
        } else {
            &[]
        }
    }

    /// The session sees SELECT before the gesture layer decides whether it is
    /// a tap, a chord or a shortcut. The MD display chord needs the physical
    /// hold state, not only the action the gesture layer eventually emits.
    pub fn set_select_held(&mut self, held: bool) {
        self.select_held = held;
    }

    pub fn takes_from_the_game(&self, action: Action) -> bool {
        match action {
            Action::GbaDown(btn) | Action::GbaUp(btn) => self.taken_buttons().contains(&btn),
            _ => false,
        }
    }

    fn set_picture(&mut self, mode: VideoMode) {
        if self.video_mode == mode {
            return;
        }
        self.video_mode = mode;
        let (Some(root), Phase::Playing { cart }) = (self.root.clone(), &self.phase) else {
            return;
        };
        let written = video_mode::write_video_mode(&root, cart, mode);
        if let Err(e) = &written {
            eprintln!("slot: video: could not write video_mode.txt: {e}");
        }
        self.card_failing.store(written.is_err(), Ordering::Relaxed);
    }

    pub fn set_link_loaded(&mut self, serial: &'static str) {
        self.link_loaded = Some(serial);
    }

    pub fn link_mode(&self, stem: &str) -> (LinkKind, &'static str) {
        let Some((cart, auto)) = self.auto_link(stem) else {
            return (LinkKind::Cable, "auto");
        };
        let chosen = self.link_choices.get(stem).copied().unwrap_or(auto);
        (chosen, serial_option(chosen, auto, &cart.code, &cart.title))
    }

    pub fn link_active(&self) -> bool {
        self.link.is_some()
    }

    /// Whether this machine is one console played by two pads (Famicom,
    /// Super Famicom or Mega Drive). Its link is a single machine, not a pair
    /// of handhelds: the cable carries one save state and two controller
    /// masks.
    pub fn single_machine_link(&self) -> bool {
        matches!(self.platform, Platform::Fc | Platform::Sfc | Platform::Md)
    }

    /// Whether this link is a cable session (one socket, many carts) rather
    /// than gpSP's per-game netpacket connection.
    ///
    /// `link_player` is what the cable path sets when a game opens the core in
    /// link mode; the single-machine consoles qualify by machine because their
    /// cable needs no core options at all.
    pub fn cable_session(&self) -> bool {
        if !self.link_active() {
            return false;
        }
        self.single_machine_link() || self.link_player.is_some()
    }

    pub fn begin_link(&mut self, client_id: u16) {
        crate::link_log::debug(format!("link active client_id={client_id}"));
        self.wifi_activity_started();
        // The card is about to be used by a game: never let a sync outlive it.
        self.cancel_shelf_sync();
        self.link = Some(LinkSession {
            client_id,
            lost_at: None,
            peer: None,
            peer_game: None,
            game: self.seated_cart().map(|cart| cart.stem.clone()),
            // A link started while a cart is in the slot is on the cable at
            // once; one started from the shelf waits for the first cart.
            paired: self.seated_cart().is_some(),
        });
        // A session knows which half of the cable it is as soon as it opens:
        // the cable path needs that before the next cart opens its core, and
        // gpSP's netpacket half has no such notion.
        if self.core == Core::Mgba || self.single_machine_link() {
            self.link_player = Some(client_id as u8);
        }
        self.sync_link_badge();
    }

    /// The other device named itself in its hello.
    pub fn set_link_peer(&mut self, name: &str) {
        if let Some(session) = &mut self.link {
            let name = name.trim();
            if !name.is_empty() {
                session.peer = Some(name.to_string());
            }
        }
    }

    /// The other side started or stopped a game (session layer).
    pub fn set_peer_game(&mut self, game: Option<crate::cable::GameStart>) {
        if let Some(session) = &mut self.link {
            session.peer_game = game;
        }
    }

    /// The peer's game as text, for the link page.
    pub fn peer_game_name(&self) -> Option<&str> {
        let game = self.link.as_ref()?.peer_game.as_ref()?;
        (!game.name.is_empty()).then_some(game.name.as_str())
    }

    pub fn peer_game(&self) -> Option<&crate::cable::GameStart> {
        self.link.as_ref()?.peer_game.as_ref()
    }

    /// This side started or stopped a game; the session itself stays.
    pub fn set_link_game(&mut self, game: Option<String>) {
        if let Some(session) = &mut self.link {
            session.game = game;
        }
    }

    /// A game ended on this side but the link stays open for the next one.
    pub fn link_game_ended(&mut self) {
        if self.link.is_some() {
            self.set_link_game(None);
            self.link_paired_now(false);
            crate::link_log::debug("link: game ended, the session stays");
        }
    }

    pub fn link_peer(&self) -> Option<&str> {
        self.link.as_ref()?.peer.as_deref()
    }

    /// The peer started a game this side is not in: ask before switching.
    ///
    /// Only ever one question at a time; the session keeps the peer's next
    /// announce out until this one is answered or times out.
    pub fn ask_follow(&mut self, name: &str, stem: &str, core: u8) {
        if self.follow_ask.is_some() {
            return;
        }
        self.follow_ask = Some(FollowAsk {
            name: name.to_string(),
            stem: stem.to_string(),
            core,
            since: self.now(),
        });
        self.utility = Some((4, 0));
    }

    /// The link page and the confirm panel read this to know whose game the
    /// question is about.
    pub fn follow_name(&self) -> Option<&str> {
        self.follow_ask.as_ref().map(|ask| ask.name.as_str())
    }

    pub fn follow_pending(&self) -> bool {
        self.follow_ask.is_some()
    }

    /// Close the question without answering it (the session's own refusal
    /// paths use this).
    pub fn clear_follow(&mut self) {
        self.follow_ask = None;
        if matches!(self.utility, Some((4, _))) {
            self.utility = None;
        }
    }

    fn answer_follow(&mut self, answer: FollowAnswer) {
        self.follow_answer = Some(answer);
        self.clear_follow();
    }

    /// A on the join question: do the switch here, and remember only the
    /// outcome for the session to relay.
    fn accept_follow(&mut self) {
        let Some(ask) = self.follow_ask.take() else {
            return;
        };
        let answer = match self.follow_game(&ask.stem, ask.core) {
            Ok(()) => {
                // Immediate feedback: the shelf may take a moment to jump to
                // the cart's own rack and run the insert.
                self.hud.toast(Toast::Following, self.now());
                FollowAnswer::Accept
            }
            Err(reason) => FollowAnswer::Refuse(reason),
        };
        self.answer_follow(answer);
    }

    /// B (or the timeout) on the join question: that is a "no".
    fn ask_declined(&mut self) {
        if self.follow_ask.is_none() {
            return;
        }
        self.answer_follow(FollowAnswer::Refuse(
            crate::cable::FollowRefusal::Declined,
        ));
    }

    /// Hand the player's answer to the session, once.
    pub fn take_follow_answer(&mut self) -> Option<FollowAnswer> {
        self.follow_answer.take()
    }

    /// Persist SHA-256 values the background index finished since the last
    /// frame.  `set_sha256` re-checks the file stamp, so a ROM replaced after
    /// the worker read it is not labelled with the old content.
    fn persist_rom_sha_updates(&mut self) {
        let updates = self.rom_index.take_sha_updates();
        if updates.is_empty() {
            return;
        }
        let Some(root) = self.root.as_deref() else {
            return;
        };
        let updates: Vec<_> = updates
            .into_iter()
            .map(|update| (update.path, update.sha256, update.size, update.mtime))
            .collect();
        let _ = slot_store::cart_meta::set_sha256_many(root, &updates);
    }

    /// Start the background content-hash index for the current shelves.
    pub fn start_rom_index(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let carts: Vec<PathBuf> = self
            .shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .map(|cart| cart.rom.clone())
            .collect();
        self.rom_index.start(&root, carts);
    }

    /// Find the local cart for a peer's announced game.
    pub fn app_find_follow(&mut self, game: &crate::cable::GameStart) -> Option<String> {
        self.find_follow_cart_named(game.platform, game.core, game.rom, &game.name)
    }

    /// A cart's SHA-256, from the persisted index when possible.
    pub fn cart_hash(&self, stem: &str, path: &Path) -> Digest {
        if let Some(hash) = self.rom_index.hash(path) {
            return hash;
        }
        if let Some(hash) = self.rom_hashes.get(stem) {
            return *hash;
        }
        rom_id::digest_file(path).unwrap_or([0u8; 32])
    }

    /// Find the cart the peer is playing: same machine, same ROM **content**.
    ///
    /// Once the background index is running this touches only memory; the
    /// fallback below exists for the desktop host and for tests that boot an
    /// `App` without a session.
    pub fn find_follow_cart(&mut self, platform: u8, _core: u8, rom: Digest) -> Option<String> {
        let want = Platform::ALL
            .into_iter()
            .find(|candidate| candidate.index() as u8 == platform)?;
        let carts: Vec<(String, PathBuf)> = self
            .shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .filter(|cart| cart.platform == want)
            .map(|cart| (cart.stem.clone(), cart.rom.clone()))
            .collect();
        for (stem, path) in carts {
            let hash = match self.rom_index.hash(&path) {
                Some(hash) => Some(hash),
                None if self.rom_hashes.contains_key(&stem) => {
                    self.rom_hashes.get(&stem).copied()
                }
                None => {
                    let hash = rom_id::digest_file(&path).unwrap_or([0u8; 32]);
                    self.rom_hashes.insert(stem.clone(), hash);
                    Some(hash)
                }
            };
            if hash == Some(rom) {
                return Some(stem);
            }
        }
        None
    }

    /// The hash match above, then a name fallback for a cart the index has not
    /// reached yet. A name is only trusted while that cart's content hash is
    /// unknown; once the index knows it, a same-name different ROM must not
    /// be silently followed.
    pub fn find_follow_cart_named(
        &mut self,
        platform: u8,
        core: u8,
        rom: Digest,
        name: &str,
    ) -> Option<String> {
        if let Some(stem) = self.find_follow_cart(platform, core, rom) {
            return Some(stem);
        }
        if !self.rom_index.started() {
            return None;
        }
        let want = Platform::ALL
            .into_iter()
            .find(|candidate| candidate.index() as u8 == platform)?;
        self.shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .find(|cart| {
                cart.platform == want
                    && cart.stem.eq_ignore_ascii_case(name)
                    && self.rom_index.hash(&cart.rom).is_none()
            })
            .map(|cart| cart.stem.clone())
    }

    /// Switch to the peer's cart: leave the current game, then insert theirs.
    ///
    /// Answers false when this card cannot do it (read-only, a write in
    /// flight, cart gone), which the session turns into a refusal.
    pub fn follow_game(
        &mut self,
        stem: &str,
        core: u8,
    ) -> Result<(), crate::cable::FollowRefusal> {
        use crate::cable::FollowRefusal;
        if self.card_read_only
            || self
                .power
                .as_ref()
                .is_some_and(|power| power.card_read_only())
        {
            return Err(FollowRefusal::Busy);
        }
        if self.web.as_ref().is_some_and(|web| web.busy()) {
            return Err(FollowRefusal::Busy);
        }
        // The cart's *own* shelf, not the one on screen: the joiner may be
        // looking at another platform (or the shelf) when the invite arrives.
        let platform = self.cart_platform(stem).ok_or(FollowRefusal::MissingRom)?;
        // The host's game is played on the host's core: a wireless cart needs
        // gpSP and a cable one needs mGBA, so the joiner adopts that choice
        // for this cart before the core is opened.
        let want = [Core::Mgba, Core::Gpsp]
            .into_iter()
            .find(|candidate| candidate.index() as u8 == core);
        match self.phase {
            Phase::Playing { .. } | Phase::Inserting { .. } => {
                self.insert_after_eject = Some((stem.to_string(), want));
                self.eject_inner(true);
                Ok(())
            }
            Phase::Shelf => {
                if self.seat_cart(stem, platform, want) {
                    Ok(())
                } else {
                    Err(FollowRefusal::Busy)
                }
            }
            _ => Err(FollowRefusal::Busy),
        }
    }

    /// Which shelf a cart lives on, whatever is on screen right now.
    pub fn cart_platform(&self, stem: &str) -> Option<Platform> {
        self.shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .find(|cart| cart.stem == stem)
            .map(|cart| cart.platform)
    }

    /// Put a specific cart in the slot, from the shelf.
    fn seat_cart(&mut self, stem: &str, platform: Platform, core: Option<Core>) -> bool {
        if !matches!(self.phase, Phase::Shelf) {
            return false;
        }
        let Some((at, index)) = self.seat_of(stem, Some(platform)) else {
            return false;
        };
        self.shelf_at = at;
        self.shelves[at].1.select(index);
        // `write_core` names the *selected* cart, so this has to come after
        // the selection and before the insert opens the core.
        if let Some(core) = core {
            self.write_core(core);
        }
        self.insert(true);
        true
    }

    /// Whether the cart in the slot can use a link at all on the core it is
    /// running on. Session bookkeeping asks this before announcing a game.
    pub fn game_can_link(&self) -> bool {
        if self.single_machine_link() {
            return true;
        }
        if self.platform != Platform::Gba {
            // Game Boy / Color serial link is always there.
            return true;
        }
        let Some(stem) = self.seated() else {
            return true;
        };
        match self.core {
            Core::Mgba => self.link_mode(stem).0 == crate::link_kind::LinkKind::Cable,
            Core::Gpsp => self
                .auto_link(stem)
                .is_some_and(|(cart, _)| crate::link_kind::link_carried(&cart.code, &cart.title)),
            _ => false,
        }
    }

    /// The game in the slot cannot link: it runs solo while the session waits.
    pub fn note_solo_game(&mut self) {
        self.hud.toast(Toast::NoLink, self.now());
    }

    /// The peer said why it will not follow.
    pub fn link_refused(&mut self, reason: crate::cable::FollowRefusal) {
        crate::link_log::warn(format!("link: the peer refused ({reason:?})"));
        self.clear_follow();
        let fail = match reason {
            crate::cable::FollowRefusal::MissingRom => crate::link_start::LinkFail::PeerMissingRom,
            crate::cable::FollowRefusal::Declined => crate::link_start::LinkFail::PeerDeclined,
            crate::cable::FollowRefusal::Busy => crate::link_start::LinkFail::PeerBusy,
        };
        let role = self
            .link_client_id()
            .map_or(self.last_role, LinkRow::from_client_id);
        let now = self.now();
        self.game_menu = Some(GameMenu::Failed {
            role,
            fail,
            worked: now,
            since: now,
        });
    }

    pub fn link_client_id(&self) -> Option<u16> {
        self.link.as_ref().map(|s| s.client_id)
    }

    /// Whether a peer is *on the cable right now*, rather than merely having a
    /// session open. Solo play inside a session keeps rewind, states and
    /// cheats: only a paired game has to stay deterministic.
    pub fn link_paired(&self) -> bool {
        self.link.as_ref().is_some_and(|session| session.paired)
    }

    /// The session itself, for the pages that show who is on the other end.
    pub fn link_paired_now(&mut self, paired: bool) {
        if let Some(session) = &mut self.link {
            session.paired = paired;
        }
    }

    /// The peer took up this game: only the host needs telling, because the
    /// joiner just answered its own question.
    pub fn link_joined(&mut self) {
        if self.link_client_id() == Some(0) {
            self.hud.toast(Toast::LinkJoined, self.now());
        }
    }

    /// The host started a game and the peer has not arrived yet.
    /// The joiner just re-attached to the host's freshly restarted cart.
    pub fn link_restarted(&mut self) {
        if self.link_client_id() != Some(0) {
            self.hud.toast(Toast::HostRestarted, self.now());
        }
    }

    pub fn link_waiting_peer(&mut self) {
        if self.link_client_id() == Some(0) {
            self.hud.toast(Toast::LinkWaiting, self.now());
        }
    }

    pub fn may_rewind(&self) -> bool {
        self.state.rewind && !self.link_paired()
    }

    pub fn button_layout(&self) -> slot_store::ButtonLayout { self.state.button_layout }

    pub fn turbo(&self) -> bool {
        !self.state.button_layout.uses_xy() && self.state.turbo
    }

    pub fn records_rewind(&self) -> bool {
        self.state.rewind
    }

    /// The canvas this machine's core hands over: the Advance/Game Boy's
    /// 240x160, or a Famicom's 256x224. Everything picture-shaped downstream is
    /// a ratio of this — the picture rect, the source rect, the texture the
    /// compositor sizes — so it is answered in one place.
    pub fn frame_size(&self) -> (u32, u32) {
        self.platform.frame()
    }

    pub fn picture_rect(&self) -> [f32; 4] {
        let (fw, fh) = self.frame_size();
        let (sw, sh) = (fw as f32, fh as f32);
        let (w, h) = self.platform.picture();
        let (x, y) = ((sw - w as f32) / 2.0, (sh - h as f32) / 2.0);
        [x / sw, y / sh, (x + w as f32) / sw, (y + h as f32) / sh]
    }

    pub fn may_load_state(&self) -> bool {
        !self.link_paired()
    }

    pub fn may_fast_forward(&self) -> bool {
        self.state.ff_speed != 0 && !self.link_paired()
    }

    pub fn end_link(&mut self) {
        crate::link_log::debug("end link requested");
        if let Some(mut starting) = self.starting.take() {
            crate::link_log::debug("end link: cancelling the pending listener");
            starting.starter.cancel();
        }
        self.link = None;
        self.link_player = None;
        self.sync_link_badge();
        self.radio.ask(RadioJob::Down);
        self.radio.ask(RadioJob::Cool);
        self.wifi_activity_finished();
    }

    pub fn peer_lost(&mut self) {
        let lang = self.language;
        let first = self
            .link
            .as_ref()
            .is_some_and(|session| session.lost_at.is_none());
        let host = self.link_client_id() == Some(0);
        if first {
            crate::link_log::warn("peer lost");
            if host {
                // A host is not ended by a dropped joiner: it stays open for
                // the next player and only the passive badge changes.
                self.say_notice(slot_i18n::t(lang, "device.app.link.waiting_title"), slot_i18n::t(lang, "device.app.link.peer_left"));
            } else {
                self.say_notice(slot_i18n::t(lang, "device.app.link.dropped_title"), slot_i18n::t(lang, "device.app.link.dropped_detail"));
            }
        }
        if matches!(self.game_menu, Some(GameMenu::Linked { .. })) {
            self.game_menu = None;
        }
        let now = self.now();
        if let Some(session) = &mut self.link {
            session.lost_at.get_or_insert(now);
        }
        self.sync_link_badge();
    }

    /// A fresh transport replaced the one that went quiet.  The host session
    /// was never torn down, so the peer can continue from the host's current
    /// game without re-creating the room.
    /// A joiner whose peer vanished keeps the session alive for as long as the
    /// notice is on screen: a cable stall may heal, and then the same notice
    /// turns into "reconnected".  After that the room is gone and the console
    /// is solo again.  A host is never ended by a missing peer.
    fn expire_lost_link(&mut self) {
        let Some(session) = &self.link else {
            return;
        };
        if session.client_id == 0 {
            return;
        }
        let Some(lost) = session.lost_at else {
            return;
        };
        if self.now().saturating_sub(lost) < slot_ui::notice::NOTICE_MS {
            return;
        }
        crate::link_log::warn("link: the peer never came back; ending the session");
        self.end_link();
    }

    pub fn link_reconnected(&mut self) {
        let lang = self.language;
        if let Some(session) = &mut self.link {
            session.lost_at = None;
        }
        self.link_popup = None;
        self.say_notice(slot_i18n::t(lang, "device.app.link.resumed_title"), slot_i18n::t(lang, "device.app.link.resumed_detail"));
        self.sync_link_badge();
    }

    pub fn bios_mismatch(&mut self) {
        let lang = self.language;
        if !self.link_active() {
            return;
        }
        crate::link_log::warn("peer BIOS mismatch");
        let role = self
            .link_client_id()
            .map_or(self.last_role, LinkRow::from_client_id);
        self.end_link();
        self.say_notice(slot_i18n::t(lang, "device.app.link.dropped_title"), slot_i18n::t(lang, "device.app.link.bios_detail"));
        self.unplug(role);
    }

    pub fn peer_ended(&mut self) {
        let lang = self.language;
        if !self.link_active() {
            return;
        }
        if self.link_client_id() == Some(0) {
            // The joiner closed its side; keep the host room alive for the
            // next join rather than ending the host's own session.
            self.peer_lost();
            return;
        }
        crate::link_log::warn("peer ended the link");
        let role = self
            .link_client_id()
            .map_or(self.last_role, LinkRow::from_client_id);
        self.end_link();
        self.say_notice(slot_i18n::t(lang, "device.app.link.dropped_title"), slot_i18n::t(lang, "device.app.link.ended_detail"));
        self.unplug(role);
    }

    /// The two ends turned out not to be the same game.
    ///
    /// Unlike a BIOS mismatch this one is worth a whole screen rather than a
    /// toast: nothing was played, and the reason — a different cart on the
    /// other device — is the one thing the player has to fix.
    pub fn game_mismatch(&mut self) {
        if !self.link_active() {
            return;
        }
        crate::link_log::warn("peer runs another game");
        let role = self
            .link_client_id()
            .map_or(self.last_role, LinkRow::from_client_id);
        // A session outlives the cart: a different game ends *this game's*
        // link, and both sides are free to pick again.
        if self.link_active() {
            self.link_game_ended();
            self.set_peer_game(None);
        } else {
            self.end_link();
        }
        let now = self.now();
        self.game_menu = Some(GameMenu::Failed {
            role,
            fail: crate::link_start::LinkFail::DifferentGame,
            worked: now,
            since: now,
        });
    }

    pub fn link_badge(&self) -> LinkBadge {
        self.hud.link()
    }

    pub fn set_link_badge_faces(&mut self, faces: Vec<TexId>) {
        self.hud.set_link_faces(faces);
    }

    fn sync_link_badge(&mut self) {
        let badge = match &self.link {
            None => LinkBadge::Off,
            Some(s) => match (s.client_id == 0, s.lost_at.is_some()) {
                (true, false) => LinkBadge::Hosting,
                (false, false) => LinkBadge::Joined,
                (true, true) => LinkBadge::HostingLost,
                (false, true) => LinkBadge::JoinedLost,
            },
        };
        self.hud.set_link(badge);
    }

    pub fn take_sfx(&mut self) -> Option<Sfx> {
        self.sfx.take()
    }

    pub fn set_power(&mut self, mut power: Power) {
        power.set_backlight(self.state.brightness);
        self.headphones = power.headphones();
        self.hud.set_headphones(self.headphones);
        let secs = power.now();
        if matches!(self.phase, Phase::SetClock { .. }) || secs < CLOCK_FLOOR {
            self.phase = clock_screen(secs, 0, false);
        }
        self.power = Some(power);
        self.battery_at = self.now();
        self.charge_at = self.now();
        self.last_led = None;
    }

    pub fn set_rumble(&mut self, strength: u16) {
        if let Some(power) = &mut self.power {
            power.set_rumble(strength);
        }
    }

    pub fn rumble_enabled(&self) -> bool {
        self.state.rumble
    }

    pub fn ff_speed(&self) -> u8 {
        self.state.ff_speed
    }

    pub fn ff_sound(&self) -> bool {
        self.state.ff_sound
    }

    pub fn screen_shader(&self) -> Shader {
        match self.platform {
            Platform::Gba => self.state.shader_gba,
            // The Famicom shares the Game Boy's preset bucket: both are
            // non-Advance pictures, and the four presets that bucket carries
            // (off/grid/simpletex/four-pass) are the ones a 2x console frame
            // can actually take. A separate `shader_fc` field would be a second
            // place to drift for no visible gain — see `Shader::GB`.
            Platform::Gb | Platform::Gbc | Platform::Fc | Platform::Sfc | Platform::Md => {
                self.state.shader_gb
            }
        }
    }

    pub fn colour_correction(&self) -> bool {
        self.state.colour_correction
    }

    pub fn powering_off(&self) -> bool {
        self.powering_off
    }

    pub fn ready_to_power_off(&self) -> bool {
        self.powering_off && self.now() >= self.act_at
    }

    pub fn ready_to_restart(&self) -> bool {
        self.restarting && self.now() >= self.act_at
    }

    pub fn shutting_down(&self) -> bool {
        self.powering_off || self.restarting || self.repairing
    }

    pub fn restarting(&self) -> bool {
        self.restarting
    }

    pub fn ready_to_repair(&self) -> bool {
        self.repairing && self.now() >= self.act_at
    }

    pub fn repairing(&self) -> bool {
        self.repairing
    }

    fn begin_repair(&mut self) {
        if self.power.is_none() {
            return;
        }
        self.power_menu = None;
        self.close_game_menu();
        self.repairing = true;
        self.act_at = self.now() + SHUTDOWN_SHOW_MS;
        self.set_led(LedState::Off);
    }

    pub fn repair(&mut self) {
        self.clear_run_marker();
        if let Some(power) = &mut self.power {
            power.repair_card();
        }
    }

    /// Forget the mid-game marker: this exit is on purpose.
    fn clear_run_marker(&mut self) {
        if let Some(root) = &self.root {
            linkcfg::clear_last_run(root);
        }
    }

    /// The marker the previous run left behind, if any (for tests and About).
    pub fn previous_run_note(&self) -> Option<&str> {
        self.previous_run_note.as_deref()
    }
    pub fn restart(&mut self) {
        self.clear_run_marker();
        self.remember_shelf();
        self.flush_playtime_now();
        self.cancel_shelf_sync();
        self.stop_web();
        self.settle_saves();
        if let Some(power) = &mut self.power {
            power.restart();
        }
    }

    pub fn power_menu(&self) -> Option<usize> {
        self.power_menu
    }

    pub fn core_picker(&self) -> Option<Core> {
        self.core_picker.as_ref().map(|p| p.seat())
    }

    pub fn core_picker_chip(&self) -> Option<Chip> {
        self.core_picker.as_ref().map(|p| p.chip(self.now()))
    }

    pub fn game_menu_open(&self) -> bool {
        self.game_menu.is_some()
    }

    pub fn game_menu(&self) -> Option<GameMenu> {
        self.game_menu
    }

    pub fn take_link_transport(&mut self) -> Option<(u16, Box<dyn LinkChannel>)> {
        self.link_transport.take()
    }

    pub fn take_link_reload(&mut self) -> Option<(String, &'static str)> {
        self.link_reload.take()
    }

    pub fn link_player(&self) -> Option<u8> {
        self.link_player
    }

    pub fn take_colour_correction(&mut self) -> Option<bool> {
        self.colour_pending.take()
    }

    pub fn gb_palette(&self) -> Option<GbPalette> {
        self.state.gb_palettes.then_some(self.state.gb_palette)
    }

    pub fn set_palette_live(&mut self, live: bool) {
        self.palette_live = live;
    }

    pub fn palette_live(&self) -> bool {
        self.palette_live
    }

    pub fn take_gb_palette(&mut self) -> Option<GbPalette> {
        self.palette_pending.take()
    }

    pub fn core(&self) -> Core {
        self.core
    }

    /// The machine the slot is holding. Read by the link path, which has to
    /// know whether two cores are being paired or one is being played by two
    /// pads.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Track the MENU press behind the "keep holding to leave" prompt.
    ///
    /// Armed only in game, and only when leaving is actually possible: on a
    /// card holding a single cart the eject is refused, so a progress bar
    /// there would be a promise the app cannot keep. The SELECT+MENU chord
    /// never arms either — that press was spent on the link screen before it
    /// could become a hold.
    fn track_hold_exit(&mut self, action: Action) {
        match action {
            Action::QuickMenu
                if matches!(self.phase, Phase::Playing { .. })
                    && !self.single_cart()
                    && !self.game_menu_open() =>
            {
                self.hold_exit = Some(self.now());
            }
            Action::MenuUp | Action::Eject => self.hold_exit = None,
            _ => {}
        }
    }

    /// How far into a held MENU the exit prompt is, once it is worth showing.
    ///
    /// `None` before the grace window — a plain tap must never flash the
    /// prompt — and outside the game or under a modal. The number is 0..=1
    /// across the hold that is left after the grace, i.e. the time the player
    /// still has to keep holding. The bar is drawn from this; the exit itself
    /// stays the gesture layer's `Eject` at one second.
    pub fn hold_exit(&self) -> Option<f32> {
        if !matches!(self.phase, Phase::Playing { .. })
            || self.power_menu.is_some()
            || self.upgrade_ui.is_some()
            || self.game_menu_open()
            || self.shutting_down()
        {
            return None;
        }
        let held = self.now().saturating_sub(self.hold_exit?);
        let span = slot_input::MENU_HOLD_MS
            .saturating_sub(HOLD_PROMPT_MS)
            .max(1);
        if held < HOLD_PROMPT_MS || span == 0 {
            return None;
        }
        Some(((held - HOLD_PROMPT_MS) as f32 / span as f32).min(1.0))
    }

    pub fn link_reload_done(&mut self) {
        let Some(reload) = self.reload.take() else {
            return;
        };
        if reload.fallback {
            self.link_choices.insert(reload.stem, reload.from);
            self.close_game_menu();
            return self.refuse();
        }
        if reload.cancelled {
            return self.close_game_menu();
        }
        let since = match self.game_menu {
            Some(GameMenu::Working { since, .. }) => since,
            _ => self.now(),
        };
        self.start_link_from(
            LinkStarter::spawn(reload.role.role(), link_port()),
            reload.role.client_id(),
            since,
        );
    }

    pub fn link_reload_failed(&mut self) {
        let Some(mut reload) = self.reload.take() else {
            return;
        };
        if !reload.fallback {
            self.link_reload = Some((reload.stem.clone(), reload.from_serial));
            reload.fallback = true;
            self.reload = Some(reload);
            return;
        }
        self.link_choices.insert(reload.stem, reload.from);
        self.refuse_seated();
    }

    fn open_cheats(&mut self) {
        if self.link_active() { self.cheat_rows.clear(); self.cheat_key.clear(); self.utility=Some((1,0)); return; }
        let Some(stem)=self.seated().map(str::to_string) else {return};
        let key=format!("{}/{}/{}",self.platform.dir_name(),self.core.as_str(),stem);
        if self.cheat_key!=key {
            let root = self.root().map(Path::to_path_buf);
            let cart = self.seated_cart().cloned();
            let path = root.as_deref().zip(cart.as_ref()).and_then(|(root, cart)| {
                slot_store::cart_meta::cheat_path(root, cart, self.core.as_str())
            });
            let text = path.and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
            self.cheat_rows=slot_store::cheats::parse(&text); self.cheat_key=key;
        }
        self.utility=Some((1,0));
    }
    fn playing_cart(&self) -> Option<&Cart> {
        let stem = self.state.cart.as_deref()?;
        let platform = self.state.cart_platform?;
        self.carts()
            .find(|cart| cart.platform == platform && cart.stem == stem)
    }

    fn game_menu_rows(&self) -> Vec<GameMenuRow> {
        GameMenuRow::ALL.to_vec()
    }

    /// The game menu lists all seven rows on one page; every other utility
    /// page keeps the six-row paging used by the shelf and link screens.
    pub fn utility_rows_per_page(&self) -> usize {
        if matches!(self.utility, Some((0, _))) {
            7
        } else {
            6
        }
    }

    /// How a game shortcut is currently bound, including a user's remapping.
    fn shortcut_text(&self, shortcut: Shortcut) -> &'static str {
        let lang = self.language;
        self.shortcut_value(shortcut)
            .map(|value| value.text(self.language))
            .unwrap_or(slot_i18n::t(lang, "device.app.common.none"))
    }

    /// The in-game shortcut page.  Only controls that act on a running game
    /// are listed, and every remappable row reads the live binding instead of
    /// the factory text in `navigation::HELP`.
    fn game_help_rows(&self) -> Vec<String> {
        let lang = self.language;
        let mut rows = vec![
            slot_i18n::t(lang, "device.app.help.game_menu").to_string(),
            slot_i18n::t(lang, "device.app.help.exit").to_string(),
            slot_i18n::t(lang, "device.app.help.link").to_string(),
            slot_i18n::t(lang, "device.app.help.album").to_string(),
            slot_i18n::t(lang, "device.app.help.cheats").to_string(),
            slot_i18n::t_args(lang, "device.app.help.state_save", &[("keys", self.shortcut_text(Shortcut::SaveState))]),
            slot_i18n::t_args(lang, "device.app.help.state_load", &[("keys", self.shortcut_text(Shortcut::LoadState))]),
            slot_i18n::t_args(lang, "device.app.help.rewind", &[("keys", self.shortcut_text(Shortcut::Rewind))]),
            slot_i18n::t_args(
                lang,
                "device.app.help.fast_forward",
                &[("keys", self.shortcut_text(Shortcut::FastForward))],
            ),
            slot_i18n::t_args(lang, "device.app.help.brightness_up", &[("keys", self.shortcut_text(Shortcut::BrightnessUp))]),
            slot_i18n::t_args(lang, "device.app.help.brightness_down", &[("keys", self.shortcut_text(Shortcut::BrightnessDown))]),
            slot_i18n::t_args(
                lang,
                "device.app.help.blue_down",
                &[("keys", self.shortcut_text(Shortcut::BlueLightDown))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.blue_up",
                &[("keys", self.shortcut_text(Shortcut::BlueLightUp))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.colour",
                &[("keys", self.shortcut_text(Shortcut::ColorCorrection))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.palette_back",
                &[("keys", self.shortcut_text(Shortcut::PaletteBack))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.palette_next",
                &[("keys", self.shortcut_text(Shortcut::PaletteNext))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.shader_next",
                &[("keys", self.shortcut_text(Shortcut::ShaderNext))],
            ),
            slot_i18n::t_args(
                lang,
                "device.app.help.shader_back",
                &[("keys", self.shortcut_text(Shortcut::ShaderBack))],
            ),
        ];
        rows.push(slot_i18n::t(lang, "device.app.help.turbo").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.gb_video").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.sfc_video").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.md_video").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.md_state").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.volume").to_string());
        rows.push(slot_i18n::t(lang, "device.app.help.chord").to_string());
        rows
    }

    pub fn guide_open(&self) -> bool {
        self.guide.is_some()
    }

    pub fn guide_page(&self) -> Option<(String, Vec<String>, usize, usize)> {
        let guide = self.guide.as_ref()?;
        Some((
            guide.title.clone(),
            guide.page_lines(),
            guide.page,
            guide.page_count(),
        ))
    }

    pub fn guide_image(&self) -> Option<Arc<GuideImage>> {
        self.guide.as_ref()?.image()
    }

    fn show_no_guide(&mut self) {
        self.utility = Some((GUIDE_MISSING, 3));
    }

    fn open_guide(&mut self) {
        let Some(cart) = self.playing_cart().cloned() else {
            return self.refuse();
        };
        let Some(root) = self.root.clone() else {
            return self.refuse();
        };
        let Some(path) = slot_store::cart_meta::guide_path(&root, &cart) else {
            self.show_no_guide();
            return;
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= crate::epub::MAX_BYTES => bytes,
            _ => {
                self.show_no_guide();
                return;
            }
        };
        let is_epub = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
            || crate::epub::is_epub(&bytes);
        let pages = if is_epub {
            match crate::epub::extract_blocks(&bytes) {
                Ok(blocks) => guide_epub_pages(blocks, self.language),
                Err(error) => {
                    eprintln!("slot: EPUB guide {}: {error}", path.display());
                    self.show_no_guide();
                    return;
                }
            }
        } else {
            guide_text_pages(&decode_guide_text(&bytes))
        };
        if pages.is_empty() {
            self.show_no_guide();
            return;
        }
        let title = if cart.title.is_empty() {
            cart.stem.clone()
        } else {
            cart.title.clone()
        };
        // Resume where this game was left: the page is written on close, and a
        // shorter guide (or a replaced one) clamps instead of overrunning.
        let start = slot_store::cart_meta::read(&root, cart.platform, &cart.rom, &cart.stem)
            .and_then(|meta| meta.extra.get(GUIDE_PAGE_KEY).and_then(|page| page.as_u64()))
            .unwrap_or(0) as usize;
        let page = start.min(pages.len().saturating_sub(1));
        self.utility = None;
        self.guide = Some(GuideView { title, pages, page });
    }

    /// Write the reader's page into this game's card metadata.  One small row
    /// per close, never per turn, so the card is not written while reading.
    fn save_guide_page(&mut self, page: usize) {
        let (Some(cart), Some(root)) = (self.playing_cart().cloned(), self.root.clone()) else {
            return;
        };
        let _ = slot_store::cart_meta::update(&root, &cart, |meta| {
            meta.extra
                .insert(GUIDE_PAGE_KEY.to_string(), serde_json::json!(page));
        });
    }

    fn guide_input(&mut self, action: Action) {
        let Some(guide) = &mut self.guide else {
            return;
        };
        match action {
            Action::GbaDown(Btn::B) | Action::GameMenu | Action::QuickMenu => {
                let page = guide.page;
                self.guide = None;
                self.utility = Some((0, 0));
                self.save_guide_page(page);
            }
            // Start jumps back to the first page without losing the bookmark:
            // closing now stores 0, exactly like reading it again from the top.
            Action::GbaDown(Btn::Start) => {
                guide.page = 0;
            }
            Action::GbaDown(Btn::Up)
            | Action::GbaDown(Btn::Left)
            | Action::GbaDown(Btn::L1) => {
                guide.page = guide.page.saturating_sub(1);
            }
            Action::GbaDown(Btn::Down)
            | Action::GbaDown(Btn::Right)
            | Action::GbaDown(Btn::R1)
            | Action::GbaDown(Btn::A) => {
                guide.page = (guide.page + 1).min(guide.page_count().saturating_sub(1));
            }
            _ => {}
        }
    }

    fn utility_input(&mut self, action: Action) {
        let Some((kind, index)) = self.utility else {
            return;
        };
        if kind == SHORTCUT_CAPTURE {
            self.capture_shortcut_input(action);
            return;
        }
        if kind == SHORTCUT_CLEAR {
            match action {
                Action::GbaDown(Btn::A) => self.confirm_clear_shortcut(),
                Action::GbaDown(Btn::B) | Action::QuickMenu | Action::Polaroids => {
                    self.cancel_shortcut_capture()
                }
                _ => {}
            }
            return;
        }
        if action == Action::Eject && matches!(self.phase, Phase::Playing { .. }) {
            self.utility = None;
            self.eject();
            return;
        }
        if matches!(
            action,
            Action::GbaDown(Btn::B) | Action::QuickMenu | Action::Polaroids
        ) {
            // Leaving the join question with B is a "no", not a dismissal:
            // the session is waiting for an answer either way.
            match kind {
                4 => {
                    self.ask_declined();
                    return;
                }
                _ => {}
            }
            self.utility =
                if kind == 1
                    || ((kind == 2 || kind == GAME_HELP || kind == GUIDE_MISSING)
                        && matches!(self.phase, Phase::Playing { .. }))
                {
                    Some((0, 0))
                } else {
                    None
                };
            return;
        }
        if self.utility_is_notice() {
            // A notice has nothing to choose: any of the usual dismiss keys
            // closes it, and the cursor never moves.
            if matches!(
                action,
                Action::GbaDown(Btn::A)
                    | Action::GbaDown(Btn::B)
                    | Action::GameMenu
                    | Action::QuickMenu
            ) {
                match kind {
                    5 => self.close_cart_transfer(),
                    _ => self.utility = Some((0, 0)),
                }
            }
            return;
        }
        let count = match kind {
            0 => self.game_menu_rows().len(),
            1 => self.cheat_rows.len().max(1),
            2 => slot_ui::navigation::help(self.language).len(),
            3 => self.link_admin().0.len().max(1),
            4 => 2,
            GAME_HELP => self.game_help_rows().len().max(1),
            GUIDE_MISSING => 4,
            _ => return,
        };
        let page = self.utility_rows_per_page();
        let next = match action {
            Action::GbaDown(Btn::Up) => (index + count - 1) % count,
            Action::GbaDown(Btn::Down) => (index + 1) % count,
            Action::GbaDown(Btn::Left) if count > page => index.saturating_sub(page),
            Action::GbaDown(Btn::Right) if count > page => (index + page).min(count - 1),
            Action::GbaDown(Btn::Left) => index,
            Action::GbaDown(Btn::Right) => index,
            Action::GbaDown(Btn::A) => {
                self.utility_press(kind, index, count);
                return;
            }
            _ => index,
        };
        self.utility = Some((kind, next.min(count - 1)));
    }

    /// A on the open utility panel, for the kinds that own a page.
    fn utility_press(&mut self, kind: u8, index: usize, count: usize) {
        let index = index.min(count.saturating_sub(1));
        match kind {
            0 => match self.game_menu_rows().get(index) {
                Some(GameMenuRow::Album) => {
                    self.utility = None;
                    self.open_polaroids();
                }
                Some(GameMenuRow::Cheats) => self.open_cheats(),
                Some(GameMenuRow::Guide) => self.open_guide(),
                Some(GameMenuRow::Shortcuts) => self.utility = Some((GAME_HELP, 0)),
                Some(GameMenuRow::Link) => self.open_link_from_menu(),
                Some(GameMenuRow::Restart) => {
                    self.utility = None;
                    self.restart_game();
                }
                Some(GameMenuRow::Exit) => {
                    self.utility = None;
                    // The same path as holding MENU for a second: flush the
                    // battery save and the resume state first, then eject. A
                    // slip here costs a trip to the shelf, not the run — the
                    // cart is one A press from resuming.
                    self.eject();
                }
                None => self.utility = None,
            },
            1 => {
                if self.link_active() || self.cheat_rows.is_empty() {
                    return;
                }
                if self.core == Core::Gpsp
                    && self.cheat_rows.iter().filter(|c| c.enabled).count() >= 20
                    && self.cheat_rows.get(index).is_some_and(|c| !c.enabled)
                {
                    self.refuse();
                    return;
                }
                if let Some(row) = self.cheat_rows.get_mut(index) {
                    row.enabled = !row.enabled;
                    self.cheat_pending = Some(
                        self.cheat_rows
                            .iter()
                            .filter(|c| c.enabled)
                            .map(|c| c.code.clone())
                            .collect(),
                    );
                }
            }
            3 => self.link_admin_press(index),
            GUIDE_MISSING => self.utility = Some((0, 0)),
            4 => {
                if index == 0 {
                    self.accept_follow();
                } else {
                    self.ask_declined();
                }
            }
            _ => {}
        }
    }

    /// The join question and the missing-cart question both expire the way a
    /// conflict popup does: the peer has been waiting, so no answer is a no.
    fn poll_follow_ask(&mut self) {
        let Some(ask) = &self.follow_ask else {
            return;
        };
        if self.now().saturating_sub(ask.since) >= FOLLOW_ASK_MS {
            self.ask_declined();
        }
    }
    /// The link page's live rows, and what pressing each one does.
    ///
    /// Everything the page says is a part of the session: who leads, who the
    /// other device is, and what each side is playing right now. The rows that
    /// act appear only while they can act, so A never lands on a dead option.
    fn link_admin(&self) -> (Vec<String>, Vec<LinkAdminRow>) {
        let lang = self.language;
        let mut rows = Vec::new();
        let mut kinds = Vec::new();
        let linked = self.link_active();
        let host = self.link_client_id() == Some(0);
        let peer = self.link_peer().map(str::to_string);
        let own = if matches!(self.phase, Phase::Playing { .. }) {
            self.seated_cart().map(|cart| cart.stem.clone())
        } else {
            None
        };
        rows.push(match (linked, host) {
            (false, _) => slot_i18n::t(lang, "device.app.link.status_off").to_string(),
            (true, true) => peer
                .map(|peer| slot_i18n::t_args(lang, "device.app.link.host_peer", &[("peer", peer.as_str())]))
                .unwrap_or_else(|| slot_i18n::t(lang, "device.app.link.host_waiting").to_string()),
            (true, false) => peer
                .map(|peer| slot_i18n::t_args(lang, "device.app.link.joiner_host", &[("peer", peer.as_str())]))
                .unwrap_or_else(|| slot_i18n::t(lang, "device.app.link.joiner_waiting").to_string()),
        });
        kinds.push(LinkAdminRow::Status);
        rows.push(own.as_deref().map_or_else(
            || slot_i18n::t(lang, "device.app.link.own_none").to_string(),
            |name| slot_i18n::t_args(lang, "device.app.link.own_game", &[("name", name)]),
        ));
        kinds.push(LinkAdminRow::ThisGame);
        if linked {
            let peer_game = self.link.as_ref().and_then(|s| s.peer_game.clone());
            rows.push(match &peer_game {
                Some(game) if !game.name.is_empty() => slot_i18n::t_args(lang, "device.app.link.peer_game", &[("name", game.name.as_str())]),
                _ => slot_i18n::t(lang, "device.app.link.peer_none").to_string(),
            });
            kinds.push(LinkAdminRow::PeerGame);
            // A netpacket link (gpSP) keeps the socket but cannot hand the
            // host's progress over: say so where the carts are chosen.
            if !self.cable_session() {
                rows.push(slot_i18n::t(lang, "device.app.link.gpsp").to_string());
                kinds.push(LinkAdminRow::Status);
                rows.push(slot_i18n::t(lang, "device.app.link.gpsp_hint").to_string());
                kinds.push(LinkAdminRow::Status);
            }
            // The joiner's door back into the host's game, after it declined
            // or after the host changed carts.
            if !host {
                if let Some(game) = &peer_game {
                    let already = own.as_deref().is_some_and(|own| own == game.name);
                    if !already {                        rows.push(slot_i18n::t_args(lang, "device.app.link.join_game", &[("name", game.name.as_str())]));
                        kinds.push(LinkAdminRow::Follow);
                    }
                }
            }
            rows.push(slot_i18n::t(lang, "device.app.link.disconnect").to_string());
            kinds.push(LinkAdminRow::Stop);
        } else {
            rows.push(slot_i18n::t(lang, "device.app.link.start").to_string());
            kinds.push(LinkAdminRow::Start);
        }
        rows.push(slot_i18n::t(lang, "device.app.common.back").to_string());
        kinds.push(LinkAdminRow::Back);
        (rows, kinds)
    }

    fn open_link_admin(&mut self) {
        self.utility = Some((3, 0));
    }

    fn link_admin_press(&mut self, index: usize) {
        let (_, kinds) = self.link_admin();
        match kinds.get(index) {
            Some(LinkAdminRow::Start) => {
                self.utility = None;
                self.open_game_menu();
            }
            Some(LinkAdminRow::Follow) => {
                let Some(peer) = self.link.as_ref().and_then(|s| s.peer_game.clone()) else {
                    return;
                };
                match self.app_find_follow(&peer) {
                    Some(stem) => {
                        let core = peer.core;
                        self.ask_follow(&peer.name, &stem, core)
                    }
                    None => {
                        crate::link_log::warn(format!(
                            "link: no local cart for {:?}",
                            peer.name
                        ));
                        self.hud.toast(Toast::NoLink, self.now());
                    }
                }
            }
            Some(LinkAdminRow::Stop) => {
                // Ask before dropping a peer: the page opens the same question
                // the chord used to.
                self.utility = None;
                self.link_popup = Some(LinkPopup::EndLink);
            }
            Some(LinkAdminRow::Back) => self.utility = None,
            _ => {}
        }
    }

    /// True when the open panel is a notice, not a list: nothing on it is
    /// selectable, because every key already does the same thing.
    pub fn utility_is_notice(&self) -> bool {
        let Some((kind, _)) = self.utility else {
            return false;
        };
        kind == GUIDE_MISSING
            || (kind == 5
                && self
                    .transfer
                    .as_ref()
                    .is_some_and(|t| t.stage == TransferStage::Failed))
    }

    pub fn utility_text(&self) -> Option<(String, Vec<String>, usize)> {
        if self.power_menu.is_some() || self.shutting_down() {
            return None;
        }
        let (kind, index) = self.utility?;
        let lang = self.language;
        Some(match kind {
            5 => {
                let (title, rows, _) = self.transfer_rows();
                let selected = index.min(rows.len().saturating_sub(1));
                (title, rows, selected)
            }
            6 => (
                slot_i18n::t(lang, "device.app.confirm.leave_transfer").into(),
                vec![slot_i18n::t(lang, "device.app.confirm.leave_transfer_yes").into(), slot_i18n::t(lang, "device.app.confirm.leave_transfer_no").into()],
                index.min(1),
            ),
            7 => {
                let (title, rows, _) = self.transfer_rows();
                let selected = index.min(rows.len().saturating_sub(1));
                (title, rows, selected)
            }
            10 => {
                let title = self
                    .shelf()
                    .carts
                    .get(self.shelf().index)
                    .map(|cart| {
                        let name = slot_ui::label_text(cart);
                        slot_i18n::t_args(lang, "device.app.confirm.delete_named", &[("name", name.as_str())])
                    })
                    .unwrap_or_else(|| slot_i18n::t(lang, "device.app.confirm.delete_cart").into());
                (
                    title,
                    vec![slot_i18n::t(lang, "device.app.confirm.delete").into(), slot_i18n::t(lang, "device.app.common.cancel").into()],
                    index.min(1),
                )
            }
            0 => {
                let rows = self.game_menu_rows();
                (
                    slot_i18n::t(lang, "device.app.menu.title").into(),
                    rows.iter()
                        .map(|row| row.label(lang).to_string())
                        .collect(),
                    index.min(rows.len().saturating_sub(1)),
                )
            }
            3 => {
                let (rows, _) = self.link_admin();
                let selected = index.min(rows.len().saturating_sub(1));
                (slot_i18n::t(lang, "device.app.link.admin").into(), rows, selected)
            }
            4 => {
                let name = self.follow_name().unwrap_or_default().to_string();
                (
                    slot_i18n::t_args(lang, "device.app.link.host_started", &[("name", name.as_str())]),
                    vec![slot_i18n::t(lang, "device.app.link.join_cart").to_string(), slot_i18n::t(lang, "device.app.common.cancel").to_string()],
                    index.min(1),
                )
            }
            SHORTCUT_CAPTURE => {
                let name = self
                    .shortcut_capture
                    .and_then(Self::shortcut_row_of)
                    .map(|row| row.label(lang))
                    .unwrap_or(slot_i18n::t(lang, "device.app.menu.shortcut_capture"));
                (
                    slot_i18n::t_args(lang, "device.app.menu.setting_name", &[("name", name)]),
                    vec![
                        slot_i18n::t(lang, "device.app.menu.press_key").to_string(),
                        slot_i18n::t(lang, "device.app.menu.press_chord").to_string(),
                        slot_i18n::t(lang, "device.app.menu.press_end").to_string(),
                    ],
                    index.min(2),
                )
            }
            SHORTCUT_CLEAR => (
                slot_i18n::t(lang, "device.app.confirm.unbind").to_string(),
                vec![
                    slot_i18n::t(lang, "device.app.confirm.unbind_yes").to_string(),
                    slot_i18n::t(lang, "device.app.common.cancel").to_string(),
                ],
                index.min(1),
            ),
            1 => {
                let rows = if self.link_active() {
                    vec![slot_i18n::t(lang, "device.app.link.no_cheats").into()]
                } else if self.cheat_rows.is_empty() {
                    vec![slot_i18n::t(lang, "device.app.cheats.none").into()]
                } else {
                    self.cheat_rows
                        .iter()
                        .map(|c| {
                            format!("[{}] {}", if c.enabled { slot_i18n::t(lang, "device.app.common.on") } else { slot_i18n::t(lang, "device.app.common.off") }, c.description)
                        })
                        .collect()
                };
                let selected = index.min(rows.len().saturating_sub(1));
                (
                    slot_i18n::t_args(
                        lang,
                        "device.app.cheats.title",
                        &[
                            ("serial", self.core.text()),
                            ("extra", if self.core == Core::Gpsp {
                                slot_i18n::t(lang, "device.app.cheats.limit")
                            } else {
                                ""
                            }),
                        ],
                    ),
                    rows,
                    selected,
                )
            }
            2 => (
                slot_i18n::t(lang, "device.app.menu.shortcuts").into(),
                slot_ui::navigation::help(lang)
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                index,
            ),
            GAME_HELP => {
                let rows = self.game_help_rows();
                let selected = index.min(rows.len().saturating_sub(1));
                (slot_i18n::t(lang, "device.app.menu.keys_title").into(), rows, selected)
            }
            GUIDE_MISSING => (
                slot_i18n::t(lang, "device.app.guide.none_title").into(),
                vec![
                    slot_i18n::t(lang, "device.app.guide.none_body").to_string(),
                    slot_i18n::t(lang, "device.app.guide.none_web").to_string(),
                    slot_i18n::t(lang, "device.app.guide.none_ai").to_string(),
                ],
                0,
            ),
            _ => return None,
        })
    }

    pub fn navigation_visible(&self) -> bool {
        matches!(self.phase, Phase::Shelf)
            && self.power_menu.is_none()
            && !self.shutting_down()
            && self.core_picker.is_none()
            && self.game_menu.is_none()
            // The intro page owns the whole screen; the shelf's navigation
            // title would otherwise print the game name in white on the paper.
            && self.game_intro.is_none()
    }
    pub fn rom_title(&self) -> Option<&str> {
        if !self.navigation_visible() || !self.state.show_rom_title {
            return None;
        }
        // The receiver's shelf selection is still the local cart, so the
        // navigation title would otherwise keep showing the wrong name while
        // the incoming card is on screen.
        if let Some(name) = self
            .transfer
            .as_ref()
            .and_then(|transfer| transfer.incoming.as_ref())
            .map(|offer| offer.name.as_str())
        {
            return Some(name);
        }
        self.selected_stem()
    }
    pub fn selected_stem(&self) -> Option<&str> {
        self.shelf()
            .carts
            .get(self.shelf().index)
            .map(|c| c.stem.as_str())
    }

    pub fn battery(&self) -> Option<Battery> {
        self.battery
    }

    pub fn poweroff(&mut self) {
        self.clear_run_marker();
        self.flush_playtime_now();
        self.stop_web();
        self.settle_saves();
        if let Some(power) = &mut self.power {
            power.poweroff();
        }
    }

    pub fn apply(&mut self, action: Action) {
        self.last_input_at = self.now();
        // A notice is not a dialog: the press that dismisses it still does its
        // usual job, so play never stalls behind the card.
        if matches!(action, Action::GbaDown(_) | Action::Insert) {
            self.clear_notice();
        }
        self.track_hold_exit(action);
        if self.power_menu.is_some() {
            match action {
                Action::LidClose => return self.doze(),
                Action::LidOpen => return self.wake(),
                _ => return self.power_menu_input(action),
            }
        }
        if self.upgrade_ui.is_some() {
            return self.upgrade_input(action);
        }
        if self.game_intro.is_some() {
            if matches!(self.utility, Some((10, _))) {
                return self.intro_delete_input(action);
            }
            let (armed, cancelling) = self
                .game_intro
                .as_ref()
                .map(|intro| (intro.armed, intro.cancel.is_some()))
                .unwrap_or((false, false));
            if !armed {
                // Holding still: only releasing B (or backing out) cancels the
                // preview; scrolling and deleting wait for the full beat.
                if !cancelling
                    && matches!(action, Action::GbaUp(Btn::B) | Action::QuickMenu)
                {
                    self.cancel_game_intro();
                }
                return;
            }
            let intro = self.game_intro.as_mut().expect("intro checked");
            let closing = intro.closing.is_some();
            match action {
                Action::GbaDown(Btn::Down) if !closing => {
                    intro.scroll = (intro.scroll + 1).min(intro.max_scroll);
                }
                Action::GbaDown(Btn::Up) if !closing => {
                    intro.scroll = intro.scroll.saturating_sub(1);
                }
                Action::GbaDown(Btn::Y) if !closing => {
                    self.utility = Some((10, 0));
                }
                Action::GbaDown(Btn::B) | Action::QuickMenu => {
                    self.close_game_intro();
                }
                _ => {}
            }
            return;
        }
        if self.sync_insert.is_some() {
            return;
        }
        if action == Action::GbaDown(Btn::Select) {
            self.select_held = true;
        }
        if action == Action::GbaUp(Btn::Select) {
            self.select_held = false;
        }
        if action == Action::GbaDown(Btn::Start) && self.select_held {
            let read_only = self.card_read_only
                || self
                    .power
                    .as_ref()
                    .is_some_and(|power| power.card_read_only());
            if read_only {
                self.begin_repair();
                return;
            }
        }
        if matches!(self.phase, Phase::Shelf) && self.core_picker.is_none() {
            let physical = match action {
                Action::GbaDown(btn) => Some(btn),
                Action::RewindStart => Some(Btn::L2),
                Action::FfStart => Some(Btn::R2),
                _ => None,
            };
            if let Some(button) = physical {
                if let Some(carousel) = self.carousel_action_for(button) {
                    match carousel {
                        CarouselAction::PreviousShelf => self.switch_shelf(-1),
                        CarouselAction::NextShelf => self.switch_shelf(1),
                        CarouselAction::Favorite => self.toggle_favorite(),
                    }
                    return;
                }
            }
        }
        match action {
            Action::LidClose => return self.doze(),
            Action::LidOpen => return self.wake(),
            Action::PowerPress => {                if self.link_active() {
                    self.link_popup = None;
                    self.end_link();
                    return;
                }
                self.woke_on_press = matches!(self.phase, Phase::Doze { .. });
                if self.woke_on_press {
                    self.wake();
                }
                return self.flush_resume();
            }
            Action::PowerTap => return self.power_press(),
            Action::PowerHold => return self.open_power_menu(),
            Action::PowerOff => return,
            _ => {}
        }
        if let Phase::SetClock {
            picker, from_menu, ..
        } = &mut self.phase
        {
            match action {
                Action::GbaDown(Btn::Left) | Action::ShelfLeft => picker.left(),
                Action::GbaDown(Btn::Right) | Action::ShelfRight => picker.right(),
                Action::GbaDown(Btn::Up) => picker.up(),
                Action::GbaDown(Btn::Down) => picker.down(),
                Action::GbaDown(Btn::A) | Action::Insert => self.confirm_clock(),
                Action::GbaDown(Btn::B) if *from_menu => {
                    self.phase = Phase::QuickMenu {
                        row: QuickRow::DateTime,
                    }
                }
                _ => {}
            }
            return;
        }
        // The shelf sync overlay owns the pad while it is up: only the keys its
        // current state handles get through (project book §6.4).
        if !matches!(self.shelf_sync, ShelfSync::Idle) {
            return self.shelf_sync_input(action);
        }
        if self.search_results.is_some() {
            return self.search_results_input(action);
        }
        if self.keyboard.is_some() {
            return self.keyboard_input(action);
        }
        if self.web_popup.is_some() {
            return self.web_popup_input(action);
        }
        if self.link_popup.is_some() {
            return self.link_popup_input(action);
        }
        if self.transfer.is_some()
            || matches!(self.utility, Some((5 | 6 | 7, _)))
        {
            return self.transfer_input(action);
        }
        if self.wifi_ui.is_some() {
            return self.wifi_input(action);
        }
        if self.sync_ui.is_some() {
            return self.sync_storage_input(action);
        }
        if self.mods_ui.is_some() {
            return self.mods_input(action);
        }
        if self.guide.is_some() {
            return self.guide_input(action);
        }
        if self.utility.is_some() { return self.utility_input(action); }
        if self.adjust(action) {
            return;
        }
        match action {
            Action::GbaUp(Btn::Left) => self.shelf_mut().release_left(),
            Action::GbaUp(Btn::Right) => self.shelf_mut().release_right(),
            _ => {}
        }
        if self.game_menu.is_some() {
            return self.game_menu_input(action);
        }
        let now = self.now();
        match self.phase {
            Phase::Shelf => match action {
                Action::GbaDown(Btn::Start) if self.core_picker.is_none() => {
                    self.open_core_picker()
                }
                _ if self.core_picker.is_some() => self.core_picker_input(action),
                Action::GbaDown(Btn::B) if self.game_intro.is_none() => {
                    self.shelf_mut().release_hold();
                    self.play_held = None;
                    if matches!(self.shelf_sync, ShelfSync::Idle) {
                        self.open_game_intro();
                    }
                }
                Action::GbaDown(Btn::X) => {
                    self.shelf_mut().release_hold();
                    self.play_held = None;
                    self.open_game_search();
                }
                Action::GbaDown(Btn::Up) => self.jump_letter(-1),
                Action::GbaDown(Btn::Down) => self.jump_letter(1),
                Action::ShelfLeft | Action::GbaDown(Btn::Left) => self.shelf_mut().hold_left(now),
                Action::ShelfRight | Action::GbaDown(Btn::Right) => {
                    self.shelf_mut().hold_right(now)
                }
                Action::QuickMenu => self.open_quick_menu(),
                Action::GbaDown(Btn::A) => self.play_held = Some(now),
                Action::GbaUp(Btn::A) => {
                    if self.play_held.take().is_some() {
                        self.insert(false);
                    }
                }
                Action::Insert => self.insert(false),
                Action::FfStart if !self.link_active() => self.toggle_favorite(),
                Action::GbaDown(Btn::Y) => self.start_sync_hold(),
                Action::GbaDown(Btn::L1) => self.switch_shelf(-1),
                Action::GbaDown(Btn::R1) => self.switch_shelf(1),
                _ => {}
            },
            Phase::Inserting { .. } if action == Action::Eject => self.eject(),
            Phase::Playing { .. } => match action {
                Action::GbaDown(Btn::L1) if self.shoulders_change_picture() => {
                    self.set_picture(VideoMode::Stretch)
                }
                Action::GbaDown(Btn::R1) if self.shoulders_change_picture() => {
                    self.set_picture(VideoMode::Actual)
                }
                Action::Eject => self.eject(),
                Action::QuickMenu => {
                    self.utility=Some((0,0));
                }
                Action::GameMenu => self.game_menu_shortcut(),
                Action::Polaroids => self.open_polaroids(),
                Action::SaveState => self.save_state(),
                Action::LoadState => self.load_newest(),
                Action::RewindStart if !self.may_rewind() => self.refuse(),
                Action::FfStart if !self.may_fast_forward() => self.refuse(),
                _ => {}
            },
            Phase::Polaroids { .. } => match action {
                Action::ShelfLeft | Action::GbaDown(Btn::Left) => self.flick(Polaroids::left),
                Action::ShelfRight | Action::GbaDown(Btn::Right) => self.flick(Polaroids::right),
                Action::GbaDown(Btn::A) => self.load_selected(),
                Action::GbaDown(Btn::B) | Action::Polaroids => self.close_polaroids(),
                Action::GbaDown(Btn::X) => self.undo(self.now()),
                Action::GbaDown(Btn::Y) => self.delete_selected(),
                _ => {}
            },
            Phase::About => self.about_input(action),
            Phase::QuickMenu { row } => self.quick_menu_input(row, action),
            _ => {}
        }
    }

    fn open_quick_menu(&mut self) {
        self.phase = Phase::QuickMenu {
            row: QuickRow::MAIN[0],
        };
    }

    /// About owns B/M (back), a short A (thanks list) and a long A (debug).
    fn about_input(&mut self, action: Action) {
        if self.about_page == slot_ui::AboutPage::Thanks {
            if matches!(action, Action::GbaDown(Btn::B) | Action::QuickMenu) {
                self.about_page = slot_ui::AboutPage::Main;
            }
            return;
        }
        match action {
            Action::GbaDown(Btn::A) => self.about_hold = Some(self.now()),
            Action::GbaUp(Btn::A) => {
                let Some(since) = self.about_hold.take() else {
                    return;
                };
                let now = self.now();
                if now.saturating_sub(since) >= ABOUT_DEBUG_HOLD_MS {
                    self.enable_debug_logs(now);
                } else {
                    self.about_page = slot_ui::AboutPage::Thanks;
                }
            }
            Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.about_hold = None;
                self.about_page = slot_ui::AboutPage::Main;
                self.phase = Phase::QuickMenu {
                    row: QuickRow::About,
                };
            }
            _ => {}
        }
    }

    /// Called every frame while A may be held on the About page.
    fn poll_about_hold(&mut self) {
        let Some(since) = self.about_hold else {
            return;
        };
        if !matches!(self.phase, Phase::About) || self.about_page != slot_ui::AboutPage::Main {
            self.about_hold = None;
            return;
        }
        let now = self.now();
        if now.saturating_sub(since) < ABOUT_DEBUG_HOLD_MS {
            return;
        }
        self.about_hold = None;
        self.enable_debug_logs(now);
    }

    fn enable_debug_logs(&mut self, now: Millis) {
        let Some(root) = self.root.as_deref() else {
            self.hud.toast(Toast::LogDebugFailed, now);
            return;
        };
        match linkcfg::write_log_level(root, linkcfg::LogLevel::Debug) {
            Ok(()) => self.hud.toast(Toast::LogDebug, now),
            Err(error) => {
                eprintln!("slot: about: could not set debug log level: {error}");
                self.hud.toast(Toast::LogDebugFailed, now);
            }
        }
    }

    pub fn about_page(&self) -> slot_ui::AboutPage {
        self.about_page
    }

    fn open_system_upgrade(&mut self) {
        let Some(root) = self.root.clone() else {
            self.hud.toast(Toast::UpgradeFailed, self.now());
            return;
        };
        if self.card_read_only
            || self.power.as_ref().is_some_and(|power| power.card_read_only())
        {
            self.hud.toast(Toast::UpgradeFailed, self.now());
            return;
        }
        match crate::upgrade::staged(&root) {
            Ok(Some(status)) => {
                self.upgrade_ui = Some(UpgradeUi {
                    target: status.target,
                    state: UpgradeUiState::Confirm,
                });
            }
            Ok(None) => self.hud.toast(Toast::UpgradeNoPackage, self.now()),
            Err(error) => {
                eprintln!("slot: system upgrade: {error}");
                self.hud.toast(Toast::UpgradeFailed, self.now());
            }
        }
    }

    fn upgrade_input(&mut self, action: Action) {
        let Some(ui) = self.upgrade_ui.as_mut() else {
            return;
        };
        match &ui.state {
            UpgradeUiState::Confirm => match action {
                Action::GbaDown(Btn::A) | Action::Insert => {
                    let Some(root) = self.root.clone() else {
                        return;
                    };
                    match crate::upgrade::apply(&root) {
                        Ok(()) => {
                            ui.state = UpgradeUiState::Applying;
                        }
                        Err(error) => {
                            eprintln!("slot: system upgrade: {error}");
                            ui.state = UpgradeUiState::Failed(error);
                        }
                    }
                }
                Action::GbaDown(Btn::B) | Action::QuickMenu => {
                    self.upgrade_ui = None;
                }
                _ => {}
            },
            UpgradeUiState::Applying | UpgradeUiState::Rebooting => {}
            UpgradeUiState::Failed(_) => match action {
                Action::GbaDown(Btn::A)
                | Action::GbaDown(Btn::B)
                | Action::Insert
                | Action::QuickMenu => self.upgrade_ui = None,
                _ => {}
            },
        }
    }

    pub fn upgrade_ui(&self) -> Option<ModChoiceUi> {
        let lang = self.language;
        let ui = self.upgrade_ui.as_ref()?;
        let (title, body, options) = match &ui.state {
            UpgradeUiState::Confirm => (
                slot_i18n::t(lang, "device.app.upgrade.title"),
                if ui.target == slot_ui::PROJECT_VERSION {
                    slot_i18n::t_args(
                        lang,
                        "device.app.upgrade.confirm_same",
                        &[("version", slot_ui::PROJECT_VERSION)],
                    )
                } else {
                    slot_i18n::t_args(
                        lang,
                        "device.app.upgrade.confirm",
                        &[
                            ("from", slot_ui::PROJECT_VERSION),
                            ("to", ui.target.to_string().as_str()),
                        ],
                    )
                },
                vec![slot_i18n::t(lang, "device.app.upgrade.start").to_string()],
            ),
            UpgradeUiState::Applying => (
                slot_i18n::t(lang, "device.app.upgrade.running_title"),
                slot_i18n::t(lang, "device.app.upgrade.running_body").to_string(),
                Vec::new(),
            ),
            UpgradeUiState::Rebooting => (
                slot_i18n::t(lang, "device.app.upgrade.done_title"),
                slot_i18n::t(lang, "device.app.upgrade.done_body").to_string(),
                Vec::new(),
            ),
            UpgradeUiState::Failed(error) => {
                let detail: String = error
                    .replace(['\n', '\r'], " ")
                    .chars()
                    .take(240)
                    .collect();
                (
                    slot_i18n::t(lang, "device.app.upgrade.failed_title"),
                    slot_i18n::t_args(
                        lang,
                        "device.app.upgrade.failed_body",
                        &[("detail", detail.as_str())],
                    ),
                    vec![slot_i18n::t(lang, "device.app.common.back").to_string()],
                )
            }
        };
        Some(ModChoiceUi {
            title: title.to_string(),
            body,
            options,
            selected: 0,
        })
    }

    fn quick_menu_input(&mut self, row: QuickRow, action: Action) {
        let row = match action {
            Action::GbaDown(Btn::Up) => row.up(),
            Action::GbaDown(Btn::Down) => row.down(),
            Action::GbaDown(Btn::Left) => return self.change_setting(row, false),
            Action::GbaDown(Btn::Right) => return self.change_setting(row, true),
            Action::GbaDown(Btn::A) => return self.open_quick_row(row),
            Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.phase = match row.parent() {
                    Some(row) => Phase::QuickMenu { row },
                    None => Phase::Shelf,
                };
                return;
            }
            _ => return,
        };
        self.phase = Phase::QuickMenu { row };
    }

    fn open_cart_transfer(&mut self) {
        if !matches!(self.phase, Phase::Shelf | Phase::QuickMenu { .. }) {
            self.hud.toast(Toast::NoLink, self.now());
            return;
        }
        self.phase = Phase::Shelf;
        self.transfer = None;
        self.utility = Some((5, 0));
    }

    fn transfer_rows(&self) -> (String, Vec<String>, usize) {
        let lang = self.language;
        let Some(transfer) = &self.transfer else {
            return (
                slot_i18n::t(lang, "device.app.transfer.title").into(),
                vec![slot_i18n::t(lang, "device.app.transfer.create").into(), slot_i18n::t(lang, "device.app.transfer.join").into(), slot_i18n::t(lang, "device.app.common.back").into()],
                0,
            );
        };
        match transfer.stage {
            TransferStage::Connecting => (
                slot_i18n::t(lang, "device.app.transfer.title").into(),
                vec![slot_i18n::t(lang, "device.app.transfer.matching").into(), slot_i18n::t(lang, "device.app.common.cancel").into()],
                0,
            ),
            TransferStage::OfferSent => (
                slot_i18n::t(lang, "device.app.transfer.waiting_title").into(),
                vec![slot_i18n::t(lang, "device.app.transfer.waiting").into(), slot_i18n::t(lang, "device.app.common.cancel").into()],
                0,
            ),
            TransferStage::Incoming => {
                let name = transfer
                    .incoming
                    .as_ref()
                    .map(|offer| offer.name.clone())
                    .unwrap_or_default();
                (
                    slot_i18n::t_args(lang, "device.app.confirm.receive", &[("name", name.as_str())]),
                    vec![slot_i18n::t(lang, "device.app.confirm.receive_yes").into(), slot_i18n::t(lang, "device.app.common.cancel").into()],
                    0,
                )
            }
            TransferStage::IncomingMeta => {
                let name = transfer
                    .incoming
                    .as_ref()
                    .map(|offer| offer.name.clone())
                    .unwrap_or_default();
                let labels: Vec<&str> = transfer
                    .metadata_missing
                    .iter()
                    .copied()
                    .map(crate::cart_transfer::asset_label)
                    .collect();
                let fields = labels.join("、");
                (
                    slot_i18n::t_args(lang, "device.app.confirm.metadata", &[("name", name.as_str())]),
                    vec![
                        slot_i18n::t_args(lang, "device.app.confirm.metadata_detail", &[("fields", fields.as_str())]),
                        slot_i18n::t(lang, "device.app.common.cancel").into(),
                    ],
                    0,
                )
            }
            TransferStage::Failed => (
                slot_i18n::t(lang, "device.app.transfer.failed").into(),
                vec![transfer.error.clone().unwrap_or_else(|| slot_i18n::t(lang, "device.app.common.unknown_error").into())],
                0,
            ),
            _ => (slot_i18n::t(lang, "device.app.transfer.title").into(), Vec::new(), 0),
        }
    }

    fn start_transfer_match(&mut self, host: bool) {
        self.wifi_activity_started();
        self.phase = Phase::Shelf;
        self.transfer = Some(Transfer::start(host, link_port()));
        self.utility = Some((5, 0));
        self.hud.toast(Toast::CartTransferWaiting, self.now());
    }

    fn begin_transfer_insert(&mut self, stem: String) {
        self.transfer_insert_at = Some(self.now());
        self.transfer_insert_stem = Some(stem);
        self.transfer_seated = true;
        self.transfer_eject_at = None;
    }

    fn begin_transfer_eject(&mut self) {
        self.begin_transfer_eject_after(0);
    }

    fn begin_transfer_eject_after(&mut self, hold: Millis) {
        if self.transfer_seated && self.transfer_eject_at.is_none() {
            let now = self.now();
            // Let the normal insert animation finish first.  The visible hold
            // starts once the card is actually seated, then the eject curve
            // takes over; duplicate detection often arrives during the insert.
            let seated_at = self
                .transfer_insert_at
                .map(|at| at.saturating_add((INSERT_S * 1000.0) as Millis))
                .filter(|at| *at > now)
                .unwrap_or(now);
            self.transfer_eject_at = Some(seated_at.saturating_add(hold));
            if hold == 0 {
                self.transfer_insert_at = None;
            }
        }
    }

    fn transfer_card_seat(&self) -> Option<f32> {
        if !self.transfer_seated {
            return None;
        }
        let now = self.now();
        if let Some(at) = self.transfer_eject_at {
            if now < at {
                // Still inserting or in the 1s duplicate hold.
                return Some(
                    self.transfer_insert_at
                        .map(|insert| {
                            (now.saturating_sub(insert) as f32 / (SEATED_AT * 1000.0))
                                .clamp(0.0, 1.0)
                        })
                        .unwrap_or(1.0),
                );
            }
            return Some(
                (1.0 - (now.saturating_sub(at) as f32 / (EJECT_S * 1000.0))).clamp(0.0, 1.0),
            );
        }
        if let Some(at) = self.transfer_insert_at {
            return Some(
                (now.saturating_sub(at) as f32 / (SEATED_AT * 1000.0)).clamp(0.0, 1.0),
            );
        }
        Some(1.0)
    }

    fn close_cart_transfer(&mut self) {
        if let Some(transfer) = &mut self.transfer {
            transfer.cancel();
        }
        self.transfer = None;
        self.utility = None;
        self.transfer_insert_at = None;
        self.transfer_insert_stem = None;
        self.transfer_seated = false;
        self.transfer_eject_at = None;
        self.transfer_progress_started = None;
        self.transfer_done_pending = false;
        self.transfer_faces = None;
        self.transfer_blank_face = None;
        self.phase = Phase::Shelf;
        self.radio.ask(RadioJob::Down);
        self.radio.ask(RadioJob::Cool);
        self.wifi_activity_finished();
    }

    fn transfer_input(&mut self, action: Action) {
        let Some((kind, index)) = self.utility else {
            // Matched shelf: A selects the cartridge to send, M asks before
            // leaving the transfer room.
            let Some(stage) = self.transfer.as_ref().map(|transfer| transfer.stage.clone()) else {
                return;
            };
            let now = self.now();
            match action {
                Action::QuickMenu | Action::GameMenu => {
                    self.utility = Some((6, 0));
                }
                Action::GbaUp(Btn::Left) => self.shelf_mut().release_left(),
                Action::GbaUp(Btn::Right) => self.shelf_mut().release_right(),
                Action::ShelfLeft | Action::GbaDown(Btn::Left)
                    if stage == TransferStage::Shelf =>
                {
                    self.shelf_mut().hold_left(now)
                }
                Action::ShelfRight | Action::GbaDown(Btn::Right)
                    if stage == TransferStage::Shelf =>
                {
                    self.shelf_mut().hold_right(now)
                }
                Action::GbaDown(Btn::L1) if stage == TransferStage::Shelf => {
                    self.switch_shelf(-1)
                }
                Action::GbaDown(Btn::R1) if stage == TransferStage::Shelf => {
                    self.switch_shelf(1)
                }
                Action::GbaDown(Btn::Up) if stage == TransferStage::Shelf => {
                    self.jump_letter(-1)
                }
                Action::GbaDown(Btn::Down) if stage == TransferStage::Shelf => {
                    self.jump_letter(1)
                }
                Action::GbaDown(Btn::A)
                    if stage == TransferStage::Shelf && matches!(self.phase, Phase::Shelf) =>
                {
                    let Some(root) = self.root.clone() else {
                        return;
                    };
                    let Some(cart) = self.shelf().carts.get(self.shelf().index).cloned() else {
                        return;
                    };
                    let hash = self.cart_hash(&cart.stem, &cart.rom);
                    let Some(transfer) = self.transfer.as_mut() else {
                        return;
                    };
                    match transfer.select(&root, &cart, hash) {
                        Ok(()) => {
                            self.begin_transfer_insert(cart.stem.clone());
                            self.utility = None;
                            self.hud.toast(Toast::CartTransferWaiting, self.now());
                        }
                        Err(error) => {
                            self.hud.toast(Toast::CartTransferFailed, self.now());
                            crate::link_log::warn(format!("transfer: {error}"));
                        }
                    }
                }
                Action::GbaDown(Btn::B) if stage == TransferStage::OfferSent => {
                    if let Some(transfer) = self.transfer.as_mut() {
                        transfer.cancel_offer();
                    }
                    self.begin_transfer_eject();
                    self.utility = None;
                }
                _ => {}
            }
            return;
        };
        match kind {
            5 => {
                let (rows, count) = if self.transfer.is_none() {
                    (3usize, 3usize)
                } else {
                    (2usize, 2usize)
                };
                let index = index.min(count.saturating_sub(1));
                match action {
                    Action::GbaDown(Btn::Up) => {
                        self.utility = Some((5, (index + rows - 1) % rows));
                    }
                    Action::GbaDown(Btn::Down) => {
                        self.utility = Some((5, (index + 1) % rows));
                    }
                    Action::GbaDown(Btn::A) => {
                        if self.transfer.is_none() {
                            match index {
                                0 => self.start_transfer_match(true),
                                1 => self.start_transfer_match(false),
                                _ => self.close_cart_transfer(),
                            }
                        } else if index == 1 {
                            self.close_cart_transfer();
                        }
                    }
                    Action::GbaDown(Btn::B) => self.close_cart_transfer(),
                    Action::QuickMenu => {
                        if self.transfer.is_some() {
                            self.utility = Some((6, 0));
                        } else {
                            self.close_cart_transfer();
                        }
                    }
                    _ => {}
                }
            }
            6 => match action {
                Action::GbaDown(Btn::Up) | Action::GbaDown(Btn::Down) => {
                    self.utility = Some((6, 1 - index.min(1)));
                }
                Action::GbaDown(Btn::A) => {
                    if index.min(1) == 0 {
                        self.close_cart_transfer();
                    } else {
                        self.utility = match self.transfer.as_ref().map(|t| t.stage.clone()) {
                            Some(TransferStage::Incoming | TransferStage::IncomingMeta) => Some((7, 0)),
                            Some(TransferStage::OfferSent) | Some(TransferStage::Connecting) => {
                                Some((5, 0))
                            }
                            _ => None,
                        };
                    }
                }
                Action::GbaDown(Btn::B) | Action::QuickMenu => {
                    self.utility = match self.transfer.as_ref().map(|t| t.stage.clone()) {
                        Some(TransferStage::Incoming | TransferStage::IncomingMeta) => Some((7, 0)),
                        Some(TransferStage::OfferSent) | Some(TransferStage::Connecting) => {
                            Some((5, 0))
                        }
                        _ => None,
                    };
                }
                _ => {}
            },
            7 => match action {
                Action::GbaDown(Btn::Up) | Action::GbaDown(Btn::Down) => {
                    self.utility = Some((7, 1 - index.min(1)));
                }
                Action::GbaDown(Btn::A) => {
                    if index.min(1) == 0 {
                        let Some(root) = self.root.clone() else {
                            return;
                        };
                        let Some(transfer) = &mut self.transfer else {
                            return;
                        };
                        let accepted = if transfer.stage == TransferStage::IncomingMeta {
                            transfer.accept_metadata(&root)
                        } else {
                            transfer.accept(&root)
                        };
                        match accepted {
                            Ok(()) => {
                                if let Some(name) = self
                                    .transfer
                                    .as_ref()
                                    .and_then(|transfer| transfer.incoming.as_ref())
                                    .map(|offer| offer.name.clone())
                                {
                                    self.begin_transfer_insert(name);
                                }
                                self.utility = None;
                                self.hud.toast(Toast::CartTransferReceiving, self.now());
                            }
                            Err(error) => {
                                transfer.error = Some(error);
                                transfer.stage = TransferStage::Failed;
                                self.utility = Some((5, 0));
                            }
                        }
                    } else if let Some(transfer) = &mut self.transfer {
                        transfer.reject();
                        self.utility = None;
                    }
                }
                Action::GbaDown(Btn::B) => {
                    if let Some(transfer) = &mut self.transfer {
                        transfer.reject();
                    }
                    self.utility = None;
                }
                Action::QuickMenu => self.utility = Some((6, 0)),
                _ => {}
            },
            _ => {}
        }
    }

    /// The receiver checks only the boot-time ROM-hash index.  It never walks
    /// the card or hashes the whole library during the handshake.
    fn cached_rom_cart(&self, platform: Platform, hash: Digest) -> Option<&Cart> {
        let root = self.root.as_deref();
        self.shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .filter(|cart| cart.platform == platform)
            .find(|cart| {
                self.rom_index
                    .hash(&cart.rom)
                    .or_else(|| self.rom_hashes.get(&cart.stem).copied())
                    .or_else(|| {
                        let root = root?;
                        let meta = slot_store::cart_meta::read(
                            root,
                            cart.platform,
                            &cart.rom,
                            &cart.stem,
                        )?;
                        if !slot_store::cart_meta::stamp_matches(root, &meta) {
                            return None;
                        }
                        meta.sha256
                            .as_deref()
                            .and_then(rom_id::digest_from_hex)
                    })
                    == Some(hash)
            })
    }

    fn finish_transfer_done(&mut self, now: Millis, received: bool) {
        let received_target = if received {
            self.transfer.as_ref().and_then(|transfer| {
                transfer.incoming.as_ref().map(|offer| {
                    (
                        offer.platform,
                        transfer
                            .target_stem()
                            .map(str::to_string)
                            .unwrap_or_else(|| offer.name.clone()),
                    )
                })
            })
        } else {
            None
        };
        if received {
            self.reload_shelves();
        }
        if let Some((platform, stem)) = received_target {
            // Keep the player on the page where the new cartridge just
            // landed, selected, instead of silently appending it to a
            // different platform shelf.
            self.transfer_insert_stem = Some(stem.clone());
            self.reveal_received_cart(platform, &stem);
        }
        self.transfer_done_pending = false;
        self.transfer_progress_started = None;
        self.hud.toast(Toast::CartTransferDone, now);
        self.utility = None;
        self.begin_transfer_eject();
    }

    fn poll_cart_transfer(&mut self) {
        let lang = self.language;
        let Some(root) = self.root.clone() else {
            return;
        };
        let now = self.now();
        let (before, mut after, done, received, mut duplicate_receiver, offer) = {
            let Some(transfer) = &mut self.transfer else {
                return;
            };
            let before = transfer.stage.clone();
            transfer.update(&root);
            let after = transfer.stage.clone();
            let done = transfer.take_done();
            (
                before,
                after,
                done,
                transfer.received,
                transfer.duplicate_receiver,
                transfer.incoming.clone(),
            )
        };
        if after == TransferStage::Incoming {
            if let Some(offer) = offer {
                let metadata = if offer.hash != [0u8; 32] {
                    self.cached_rom_cart(offer.platform, offer.hash)
                        .map(|cart| (cart.stem.clone(), offer.metadata_missing(&root, cart)))
                } else {
                    None
                };
                match metadata {
                    Some((_stem, missing)) if !missing.is_empty() => {
                        if let Some(transfer) = &mut self.transfer {
                            transfer.offer_metadata(_stem, missing);
                        }
                        after = TransferStage::IncomingMeta;
                    }
                    Some(_) => {
                        // Same ROM and every offered side asset is already
                        // present: keep the old duplicate refusal.
                        if let Some(transfer) = &mut self.transfer {
                            transfer.mark_duplicate(true);
                        }
                        after = TransferStage::Duplicate;
                        duplicate_receiver = true;
                    }
                    None => {}
                }
            }
        }
        if done {
            let started = self
                .transfer_progress_started
                .get_or_insert(now)
                .to_owned();
            if now.saturating_sub(started) < TRANSFER_MIN_VISIBLE_MS {
                self.transfer_done_pending = true;
                self.utility = None;
                return;
            }
            self.finish_transfer_done(now, received);
            return;
        }
        if before == after {
            return;
        }
        match after {
            TransferStage::Shelf => {
                self.utility = None;
                self.transfer_progress_started = None;
                self.transfer_done_pending = false;
                self.begin_transfer_eject();
                self.hud.toast(Toast::CartTransferReady, now);
            }
            TransferStage::Incoming | TransferStage::IncomingMeta => {
                self.utility = Some((7, 0));
            }
            TransferStage::Sending | TransferStage::Receiving => {
                let visible_at = self
                    .transfer_insert_at
                    .map(|at| at.saturating_add((INSERT_S * 1000.0) as Millis))
                    .filter(|at| *at > now)
                    .unwrap_or(now);
                self.transfer_progress_started.get_or_insert(visible_at);
                self.utility = None;
                self.hud.toast(
                    if after == TransferStage::Sending {
                        Toast::CartTransferSending
                    } else {
                        Toast::CartTransferReceiving
                    },
                    now,
                );
            }
            TransferStage::Duplicate => {
                self.utility = None;
                if duplicate_receiver {
                    // The receiver has no card of its own in the slot.  It
                    // must go back to Shelf immediately, or the next offer is
                    // ignored and the sender is left on "waiting for peer".
                    self.transfer_progress_started = None;
                    self.transfer_done_pending = false;
                    if let Some(transfer) = &mut self.transfer {
                        transfer.reset_after_duplicate();
                    }
                } else {
                    self.begin_transfer_eject_after(TRANSFER_DUPLICATE_HOLD_MS);
                }
                self.hud.toast(
                    if duplicate_receiver {
                        Toast::CartTransferAlreadyOwned
                    } else {
                        Toast::CartTransferDuplicate
                    },
                    now,
                );
            }
            TransferStage::Failed => {
                let reason = self
                    .transfer
                    .as_ref()
                    .and_then(|transfer| transfer.error.clone())
                    .unwrap_or_else(|| slot_i18n::t(lang, "device.app.common.unknown_error").into());
                self.utility = None;
                self.begin_transfer_eject();
                self.say_notice(slot_i18n::t(lang, "device.app.transfer.failed"), reason);
            }
            _ => {}
        }
    }

    fn reveal_received_cart(&mut self, platform: Platform, stem: &str) {
        let Some(at) = self
            .shelves
            .iter()
            .position(|(rack, _)| rack.platform() == Some(platform))
        else {
            return;
        };
        self.shelf_at = at;
        if let Some(index) = self.shelves[at]
            .1
            .carts
            .iter()
            .position(|cart| cart.stem == stem)
        {
            self.shelves[at].1.select(index);
        }
        self.shelves[at].1.release_hold();
        self.shelves[at].1.snap_to_selection();
    }

    fn reload_shelves(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let carts = slot_store::scan_quiet(&root).unwrap_or_default();
        self.shelves = shelves_of(carts, &self.state);
        self.shelf_at = self.shelf_at.min(self.shelves.len().saturating_sub(1));
        self.invalidate_cart_faces();
    }

    pub fn transfer_preview_cart(&self) -> Option<Cart> {
        let transfer = self.transfer.as_ref()?;
        let offer = transfer.incoming.as_ref()?;
        Some(Cart {
            platform: offer.platform,
            // Keep the incoming title: the preview uses a generated label
            // because the sender's cover has not arrived yet.
            stem: offer.name.clone(),
            rom: PathBuf::new(),
            label: None,
            title: String::new(),
            code: String::new(),
            shell: offer.shell,
        model: Default::default(),
        front: None,
})
    }

    pub fn set_transfer_blank_face(&mut self, face: Option<(TexId, u32, u32)>) {
        self.transfer_blank_face = face;
    }

    fn draw_transfer_preview(&self, seat: f32, out: &mut Vec<Draw>) {
        let Some(transfer) = &self.transfer else {
            return;
        };
        let Some(offer) = &transfer.incoming else {
            return;
        };
        let cart = Cart {
            platform: offer.platform,
            stem: offer.name.clone(),
            rom: PathBuf::new(),
            label: None,
            title: String::new(),
            code: String::new(),
            shell: offer.shell,
        model: Default::default(),
        front: None,
};
        let (rest, scale) = self.shelf().selected_at();
        let kind = self.shelves[self.shelf_at].0.collection_kind();
        slot_ui::SlotChrome {
            cart: &cart,
            face: self.transfer_blank_face.map(|(tex, _, _)| tex),
            rest,
            scale,
            seat,
            alert: None,
            dim: seat,
            screen: self.screen,
            game: false,
            plate: self.collection_slot_back(kind),
        }
        .draw(out);
    }

    fn draw_cart_transfer(&self, out: &mut Vec<Draw>) {
        let Some(transfer) = &self.transfer else {
            return;
        };
        if !matches!(
            transfer.stage,
            TransferStage::OfferSent
                | TransferStage::Sending
                | TransferStage::Receiving
                | TransferStage::Done
        ) {
            return;
        }
        if self.transfer_insert_at.is_some() {
            // Let the normal insert animation own the screen first.
            return;
        }
        let Some(faces) = self.transfer_faces.as_ref() else {
            return;
        };

        // The card, data arc and orbiting particles are the interface.  The
        // only textual element left is the large percentage.
        let (_, pw, ph) = faces.percent;
        if pw > 0 && ph > 0 {
            out.push(Draw::Tex {
                x: (OUT_W as f32 - pw as f32) / 2.0,
                y: 130.0,
                w: pw as f32,
                h: ph as f32,
                tex: faces.percent.0,
                alpha: 1.0,
            });
        }

        // The four cartridge assets are back; each one lights up as its
        // data reaches the receiver.
        let reached_all = transfer.stage == TransferStage::Done;
        for (index, (tex, aw, ah)) in faces.assets.iter().copied().enumerate() {
            if aw == 0 || ah == 0 {
                continue;
            }
            let x = 248.0 + index as f32 * 60.0;
            let reached = reached_all || transfer.progress + 0.02 >= (index + 1) as f32 / 4.0;
            let alpha = if reached { 1.0 } else { 0.38 };
            out.push(Draw::Rect {
                x,
                y: 208.0,
                w: 52.0,
                h: 24.0,
                colour: [0.06, 0.16, 0.21, 0.90 * alpha],
            });
            out.push(Draw::Rect {
                x: x + 6.0,
                y: 217.0,
                w: 4.0,
                h: 4.0,
                colour: [0.36, 0.92, 0.96, alpha],
            });
            let scale = 0.9;
            out.push(Draw::Tex {
                x: x + (52.0 - aw as f32 * scale) / 2.0,
                y: 212.0,
                w: aw as f32 * scale,
                h: ah as f32 * scale,
                tex,
                alpha,
            });
        }

        // The inserted card is drawn by the shelf chrome.  Draw the source
        // mini-cart and the moving data arc over the same composition.
        let source = if transfer.incoming.is_some() {
            None
        } else {
            self.transfer_insert_stem
                .as_deref()
                .and_then(|stem| self.shelf().find(stem))
        };
        let (sx, sy, sw, sh) = (44.0, 266.0, 174.0, 98.0);
        out.push(Draw::Rect {
            x: sx,
            y: sy,
            w: sw,
            h: sh,
            colour: [0.03, 0.06, 0.09, 0.70],
        });
        if let Some((_, Some(face))) = source {
            out.push(Draw::Tex {
                x: sx + 5.0,
                y: sy + 4.0,
                w: sw - 10.0,
                h: sh - 8.0,
                tex: face,
                alpha: 1.0,
            });
        } else if let Some((tex, _, _)) = self.transfer_blank_face {
            out.push(Draw::Tex {
                x: sx + 5.0,
                y: sy + 4.0,
                w: sw - 10.0,
                h: sh - 8.0,
                tex,
                alpha: 1.0,
            });
        } else {
            out.push(Draw::Rect {
                x: sx + 18.0,
                y: sy + 15.0,
                w: sw - 36.0,
                h: sh - 30.0,
                colour: [0.12, 0.30, 0.48, 0.95],
            });
            out.push(Draw::Rect {
                x: sx + 18.0,
                y: sy + sh - 27.0,
                w: sw - 36.0,
                h: 10.0,
                colour: [0.45, 0.18, 0.50, 0.95],
            });
        }
        out.push(Draw::Rect {
            x: sx + 5.0,
            y: sy,
            w: 1.0,
            h: sh,
            colour: [0.34, 0.89, 0.95, 0.55],
        });

        let (x0, y0) = (220.0, 315.0);
        let (cx, cy) = (300.0, 235.0);
        let (x1, y1) = (345.0, 300.0);
        let phase = (self.now() % 1200) as f32 / 1200.0;
        for step in 0..34 {
            let t = step as f32 / 33.0;
            let x = (1.0 - t) * (1.0 - t) * x0 + 2.0 * (1.0 - t) * t * cx + t * t * x1;
            let y = (1.0 - t) * (1.0 - t) * y0 + 2.0 * (1.0 - t) * t * cy + t * t * y1;
            let pulse = (phase - t).rem_euclid(1.0);
            let bright = if pulse < 0.14 { 1.0 } else { 0.36 };
            out.push(Draw::Rect {
                x,
                y,
                w: if pulse < 0.14 { 5.0 } else { 3.0 },
                h: if pulse < 0.14 { 5.0 } else { 3.0 },
                colour: [0.34, 0.89, 0.95, bright],
            });
        }
        let orbit = (self.now() % 1600) as f32 / 1600.0;
        for index in 0..3 {
            let t = orbit + index as f32 / 3.0;
            let x = 360.0 + (t * std::f32::consts::TAU).cos() * 138.0;
            let y = 350.0 + (t * std::f32::consts::TAU).sin() * 86.0;
            out.push(Draw::Rect {
                x,
                y,
                w: 5.0,
                h: 5.0,
                colour: [0.56, 0.96, 1.0, 0.80],
            });
        }
    }

    fn open_quick_row(&mut self, row: QuickRow) {
        // Opening 画面 itself only shows the display panel; the mask
        // directories are needed when the user actually enters 遮罩. Scanning
        // them here keeps an SD-card read off every 画面 press.
        if row == QuickRow::Masks {
            self.refresh_masks();
        }
        if let Some(shortcut) = Self::shortcut_row(row) {
            return self.begin_shortcut_capture(shortcut);
        }
        match row {
            QuickRow::DateTime => {
                self.phase = clock_screen(self.utc_secs(), self.state.utc_offset_min, true);
            }
            QuickRow::About => {
                self.phase = Phase::About;
                self.about_hold = None;
                self.about_page = slot_ui::AboutPage::Main;
            }
            QuickRow::SystemUpgrade => self.open_system_upgrade(),
            QuickRow::Mods => self.open_mods(),
            QuickRow::Shortcuts => self.utility=Some((2,0)),
            QuickRow::Network => {
                if let Some(row) = row.child() {
                    self.phase = Phase::QuickMenu { row };
                }
            }
            QuickRow::Wifi => self.open_wifi(),
            QuickRow::Web => self.open_web(),
            QuickRow::SyncStorage => self.open_sync_storage(),
            // 「云同步内容」只有左右三档，A 没有子页面（proposal §8.2）。
            QuickRow::SyncContent => {}
            QuickRow::CartTransfer => self.open_cart_transfer(),
            QuickRow::DefaultLoad
            | QuickRow::DefaultSave
            | QuickRow::FcMask
            | QuickRow::GbMask
            | QuickRow::GbcMask
            | QuickRow::SfcMask => {}
            QuickRow::SplitLink => self.open_link_admin(),
            QuickRow::ShortcutReset => self.reset_shortcuts(),
            QuickRow::Screen
            | QuickRow::Shelf
            | QuickRow::ShelfLook
            | QuickRow::Theme
            | QuickRow::DisplayPanel
            | QuickRow::DisplayEffects
            | QuickRow::Masks
            | QuickRow::Game
            | QuickRow::ShelfKeys
            | QuickRow::ShortcutMap
            | QuickRow::ButtonLayout
            | QuickRow::ButtonSettings
            | QuickRow::SystemSettings
            | QuickRow::SaveSettings => {
                if let Some(row) = row.child() {
                    self.phase = Phase::QuickMenu { row };
                }
            }
            QuickRow::Recents
            | QuickRow::Turbo
            | QuickRow::RomTitle
            | QuickRow::FallbackCover
            | QuickRow::LidTimeout
            | QuickRow::Language
            | QuickRow::Rewind
            | QuickRow::FastForward
            | QuickRow::FastForwardSound
            | QuickRow::ColourCorrection
            | QuickRow::GbaShader
            | QuickRow::GbShader
            | QuickRow::GbPalettes
            | QuickRow::ShelfPrev
            | QuickRow::ShelfNext
            | QuickRow::ShelfFav
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
            | QuickRow::ShortcutRewind
            | QuickRow::ControlUp
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
            | QuickRow::ControlPreset
            | QuickRow::Brightness
            | QuickRow::BlueLight
            | QuickRow::Rumble
            | QuickRow::LoadGba
            | QuickRow::SaveGba
            | QuickRow::LoadGb
            | QuickRow::SaveGb
            | QuickRow::LoadGbc
            | QuickRow::SaveGbc => {}
            QuickRow::ThemeDefault | QuickRow::ThemeWeb | QuickRow::ThemeWarm => {
                if let Some(preset) = Self::theme_choice(row) {
                    self.apply_theme(preset);
                }
            }
        }
    }

    const WIFI_ROWS: usize = 5;

    fn open_wifi(&mut self) {
        let lang = self.language;
        if self.link_active() {
            self.refuse();
            return;
        }
        self.keyboard = None;
        self.wifi_activity_started();
        let auto_off_minutes = self.wifi_auto_off_minutes;
        let Some(root) = self.root.clone() else {
            self.wifi_ui = Some(WifiUi {
                status: slot_i18n::t(lang, "device.app.power.no_card").into(),
                connected: None,
                error: Some(slot_i18n::t(lang, "device.app.power.no_card_root").into()),
                rows: Vec::new(),
                saved: Vec::new(),
                selected: 0,
                offset: 0,
                manual: true,
                connecting: None,
                job: None,
                status_job: None,
                radio_on: false,
                radio_known: false,
                radio_job: None,
                radio_pending: None,
                auto_off_minutes,
            });
            return;
        };
        self.wifi_auto_off_minutes = linkcfg::read(&root).wifi.auto_off_minutes;
        let auto_off_minutes = self.wifi_auto_off_minutes;
        let saved = linkcfg::read_saved_wifi(&root);
        crate::diag::write(
            &root,
            linkcfg::LogLevel::Info,
            "wifi",
            format!("page opened: saved={} auto_off={auto_off_minutes}m", saved.len()),
        );
        let rows = saved_wifi_rows(&saved);
        self.wifi_ui = Some(WifiUi {
            status: slot_i18n::t(lang, "device.app.wifi.reading").into(),
            connected: None,
            error: None,
            rows,
            saved,
            selected: 0,
            offset: 0,
            manual: true,
            connecting: None,
            // No scan until radio-status says the radio is on.
            job: None,
            status_job: Some(wifi_job::spawn_status(root.clone())),
            radio_on: false,
            radio_known: false,
            radio_job: Some(wifi_job::spawn_radio_status(root)),
            radio_pending: None,
            auto_off_minutes,
        });
    }

    fn open_sync_storage(&mut self) {
        let lang = self.language;
        if self.link_active() {
            self.refuse();
            return;
        }
        self.keyboard = None;
        let Some(root) = self.root.clone() else {
            self.sync_ui = Some(SyncUi {
                profiles: Vec::new(),
                active: String::new(),
                selected: 0,
                tested: None,
                testing_id: None,
                job: None,
                status: slot_i18n::t(lang, "device.app.power.no_card_root").into(),
                error: Some(slot_i18n::t(lang, "device.app.power.no_card_root").into()),
                switching: false,
                needs_setup: false,
            });
            return;
        };
        let config = linkcfg::read(&root);
        let configured = config
            .storages
            .iter()
            .any(|profile| !profile.url.trim().is_empty());
        let profiles = if configured {
            config.storages
        } else {
            Vec::new()
        };
        let selected = profiles
            .iter()
            .position(|profile| profile.id == config.active_storage)
            .unwrap_or(0);
        self.sync_ui = Some(SyncUi {
            profiles,
            active: config.active_storage,
            selected,
            tested: None,
            testing_id: None,
            job: None,
            status: if configured {
                slot_i18n::t(lang, "device.app.sync.pick_hint").into()
            } else {
                slot_i18n::t(lang, "device.app.sync.first_use").into()
            },
            error: None,
            switching: false,
            needs_setup: !configured,
        });
    }

    pub fn sync_test_ready(&self) -> bool {
        let Some(ui) = &self.sync_ui else {
            return false;
        };
        !ui.needs_setup
            && ui.job.is_none()
            && ui
                .profiles
                .get(ui.selected)
                .is_some_and(|profile| profile.id != ui.active)
    }

    pub fn sync_needs_setup(&self) -> bool {
        self.sync_ui
            .as_ref()
            .is_some_and(|ui| ui.needs_setup)
    }

    pub fn sync_can_switch(&self) -> bool {
        let Some(ui) = &self.sync_ui else {
            return false;
        };
        let Some(profile) = ui.profiles.get(ui.selected) else {
            return false;
        };
        !ui.needs_setup
            && ui.job.is_none()
            && profile.id != ui.active
            && ui.tested.as_deref() == Some(profile.id.as_str())
    }

    fn sync_storage_input(&mut self, action: Action) {
        let lang = self.language;
        if self.sync_needs_setup() {
            match action {
                Action::GbaDown(Btn::A) => {
                    self.sync_ui = None;
                    self.open_web();
                }
                Action::GbaDown(Btn::B) | Action::QuickMenu => {
                    self.sync_ui = None;
                }
                _ => {}
            }
            return;
        }
        let Some(ui) = self.sync_ui.as_ref() else {
            return;
        };
        if ui.job.is_some() {
            if matches!(action, Action::GbaDown(Btn::B) | Action::QuickMenu) {
                self.sync_ui = None;
            }
            return;
        }
        match action {
            Action::GbaDown(Btn::Up) if !ui.profiles.is_empty() => {
                let count = ui.profiles.len();
                if let Some(ui) = &mut self.sync_ui {
                    ui.selected = (ui.selected + count - 1) % count;
                    ui.tested = None;
                    ui.status = slot_i18n::t(lang, "device.app.sync.pick_hint").into();
                    ui.error = None;
                }
            }
            Action::GbaDown(Btn::Down) if !ui.profiles.is_empty() => {
                let count = ui.profiles.len();
                if let Some(ui) = &mut self.sync_ui {
                    ui.selected = (ui.selected + 1) % count;
                    ui.tested = None;
                    ui.status = slot_i18n::t(lang, "device.app.sync.pick_hint").into();
                    ui.error = None;
                }
            }
            Action::GbaDown(Btn::A) => self.start_sync_storage_test(),
            Action::GbaDown(Btn::X) => self.switch_sync_storage(),
            Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.sync_ui = None;
            }
            _ => {}
        }
    }

    fn start_sync_storage_test(&mut self) {
        let lang = self.language;
        if !self.sync_test_ready() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(ui) = &self.sync_ui else {
            return;
        };
        let Some(profile) = ui.profiles.get(ui.selected) else {
            return;
        };
        let id = profile.id.clone();
        let name = profile.name.clone();
        if let Some(ui) = &mut self.sync_ui {
            ui.testing_id = Some(id.clone());
            ui.tested = None;
            ui.error = None;
            ui.status = slot_i18n::t_args(
                lang,
                "device.app.sync.testing",
                &[("name", name.as_str())],
            );
            ui.job = Some(crate::storage_test::spawn(root, id));
        }
    }

    fn switch_sync_storage(&mut self) {
        let lang = self.language;
        if !self.sync_can_switch() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(ui) = &self.sync_ui else {
            return;
        };
        let Some(profile) = ui.profiles.get(ui.selected) else {
            return;
        };
        let id = profile.id.clone();
        let name = profile.name.clone();
        if let Some(ui) = &mut self.sync_ui {
            ui.switching = true;
            ui.error = None;
        }
        match linkcfg::activate_storage(&root, &id) {
            Ok(()) => {
                let config = linkcfg::read(&root);
                if let Some(ui) = &mut self.sync_ui {
                    ui.active = id;
                    ui.switching = false;
                    ui.tested = None;
                    ui.testing_id = None;
                    ui.profiles = config.storages;
                    ui.selected = ui
                        .profiles
                        .iter()
                        .position(|profile| profile.id == ui.active)
                        .unwrap_or(0);
                    ui.status = slot_i18n::t_args(
                        lang,
                        "device.app.sync.switched",
                        &[("name", name.as_str())],
                    );
                }
            }
            Err(message) => {
                if let Some(ui) = &mut self.sync_ui {
                    ui.switching = false;
                    ui.error = Some(slot_i18n::t_args(
                        lang,
                        "device.app.sync.switch_failed",
                        &[("message", message.as_str())],
                    ));
                }
            }
        }
    }

    /// The Web page writes `storages.toml`; keep an already-open device menu
    /// in sync instead of showing the profile that was active when it opened.
    fn refresh_sync_storage_from_disk(&mut self) {
        let lang = self.language;
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(ui) = self.sync_ui.as_mut() else {
            return;
        };
        if ui.switching || ui.job.is_some() {
            return;
        }
        let config = linkcfg::read(&root);
        if config.active_storage == ui.active && config.storages == ui.profiles {
            return;
        }
        let selected_id = ui.profiles.get(ui.selected).map(|profile| profile.id.clone());
        let selected_was_active = selected_id.as_deref() == Some(ui.active.as_str());
        ui.active = config.active_storage;
        ui.profiles = config.storages;
        let target = if selected_was_active {
            ui.active.clone()
        } else {
            selected_id.unwrap_or_else(|| ui.active.clone())
        };
        ui.selected = ui
            .profiles
            .iter()
            .position(|profile| profile.id == target)
            .unwrap_or(0);
        ui.tested = None;
        ui.testing_id = None;
        ui.status = slot_i18n::t(lang, "device.app.sync.updated").into();
    }

    fn poll_sync_storage(&mut self) {
        let lang = self.language;
        let Some(ui) = &mut self.sync_ui else {
            return;
        };
        let Some(job) = &ui.job else {
            return;
        };
        let Some(result) = job.try_recv() else {
            return;
        };
        let testing = ui.testing_id.take();
        ui.job = None;
        if result.ok {
            ui.error = None;
            ui.status = result.message;
            if let Some(id) = testing {
                ui.tested = Some(id);
            }
        } else {
            ui.tested = None;
            ui.error = Some(result.message);
            ui.status = slot_i18n::t(lang, "device.app.sync.test_failed").into();
        }
    }

    fn rescan_wifi(&mut self) {
        let lang = self.language;
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };
        if ui.connecting.is_some() || ui.radio_job.is_some() || !ui.radio_known {
            return;
        }
        if !ui.radio_on {
            // Refreshing must work on a card that has never joined a network:
            // turn the radio on first; the radio job automatically scans once
            // it comes up.
            self.toggle_wifi_radio();
            return;
        }
        if let Some(ui) = &mut self.wifi_ui {
            ui.status = slot_i18n::t(lang, "device.app.wifi.scanning").into();
            ui.error = None;
            ui.job = Some(wifi_job::spawn_scan(root));
        }
    }

    fn wifi_scroll(&mut self) {
        if let Some(ui) = &mut self.wifi_ui {
            if ui.selected < 2 || ui.selected > ui.rows.len() + 1 {
                return;
            }
            let index = ui.selected - 2;
            if index < ui.offset {
                ui.offset = index;
            }
            if index >= ui.offset + Self::WIFI_ROWS {
                ui.offset = index + 1 - Self::WIFI_ROWS;
            }
        }
    }

    /// Open the installed-mod list from the level-1 quick menu.
    fn open_mods(&mut self) {
        let lang = self.language;
        self.keyboard = None;
        let Some(root) = self.root.clone() else {
            self.mods_ui = Some(ModsUi {
                rows: Vec::new(),
                selected: 0,
                offset: 0,
                error: Some(slot_i18n::t(lang, "device.app.power.no_card").to_string()),
            });
            return;
        };
        match slot_mods::list(&root) {
            Ok(rows) => {
                self.mods_ui = Some(ModsUi {
                    rows,
                    selected: 0,
                    offset: 0,
                    error: None,
                });
            }
            Err(error) => {
                self.mods_ui = Some(ModsUi {
                    rows: Vec::new(),
                    selected: 0,
                    offset: 0,
                    error: Some(error.message().to_string()),
                });
            }
        }
    }

    /// Keys while the mod list is open: ↑/↓ select, ←/→ move a mod's priority,
    /// A toggles, B/M returns.
    fn mods_input(&mut self, action: Action) {
        let count = self.mods_ui.as_ref().map_or(0, |ui| ui.rows.len());
        match action {
            Action::GbaDown(Btn::Up) if count > 0 => {
                if let Some(ui) = self.mods_ui.as_mut() {
                    ui.selected = (ui.selected + count - 1) % count;
                }
                self.mods_scroll();
            }
            Action::GbaDown(Btn::Down) if count > 0 => {
                if let Some(ui) = self.mods_ui.as_mut() {
                    ui.selected = (ui.selected + 1) % count;
                }
                self.mods_scroll();
            }
            Action::GbaDown(Btn::Left) => self.mods_reorder(-1),
            Action::GbaDown(Btn::Right) => self.mods_reorder(1),
            Action::GbaDown(Btn::A) => self.mods_press(),
            Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.mods_ui = None;
            }
            _ => {}
        }
    }

    /// The root to write a mod edit to, or a reason shown in the page.
    ///
    /// Every write in this page goes through here so read-only cards and live
    /// links refuse the same way, whether the edit is a toggle or a reorder.
    fn mods_edit_root(&mut self) -> Option<PathBuf> {
        let lang = self.language;
        if self.card_read_only
            || self
                .power
                .as_ref()
                .is_some_and(|power| power.card_read_only())
        {
            if let Some(ui) = self.mods_ui.as_mut() {
                ui.error = Some(slot_i18n::t(lang, "device.app.mods.read_only").to_string());
            }
            return None;
        }
        if self.link_active() {
            if let Some(ui) = self.mods_ui.as_mut() {
                ui.error = Some(slot_i18n::t(lang, "device.app.mods.linked").to_string());
            }
            return None;
        }
        match self.root.clone() {
            Some(root) => Some(root),
            None => {
                if let Some(ui) = self.mods_ui.as_mut() {
                    ui.error = Some(slot_i18n::t(lang, "device.app.power.no_card").to_string());
                }
                None
            }
        }
    }

    /// ←/→: move the selected mod one place in the priority queue.
    ///
    /// The list *is* the queue — the shelf long-press runs the top-most mod
    /// that asks for it — so a move rewrites the whole list in steps of
    /// [`slot_mods::ORDER_STEP`] rather than juggling two numbers. That keeps
    /// the manifests readable and immune to the ties every fresh install has
    /// (they all start at the default 100).
    fn mods_reorder(&mut self, step: i32) {
        let Some(root) = self.mods_edit_root() else {
            return;
        };
        let Some(ui) = self.mods_ui.as_mut() else {
            return;
        };
        let from = ui.selected;
        let to = if step < 0 {
            from.checked_sub(1)
        } else if from + 1 < ui.rows.len() {
            Some(from + 1)
        } else {
            None
        };
        let Some(to) = to else {
            return;
        };
        ui.rows.swap(from, to);
        ui.selected = to;
        let ids: Vec<String> = ui.rows.iter().map(|entry| entry.id.clone()).collect();
        let mut failure = None;
        for (at, id) in ids.iter().enumerate() {
            let order = slot_mods::ORDER_STEP * at as i32;
            if let Err(error) = slot_mods::set_order(&root, id, order) {
                failure = Some(error.message().to_string());
                break;
            }
        }
        // Re-read rather than trusting the local list: the manifests are the
        // truth, and a partial failure would otherwise lie about the order.
        if let Ok(rows) = slot_mods::list(&root) {
            if let Some(ui) = self.mods_ui.as_mut() {
                let keep = ui.selected;
                ui.rows = rows;
                ui.selected = keep.min(ui.rows.len().saturating_sub(1));
            }
        }
        if let Some(ui) = self.mods_ui.as_mut() {
            ui.error = failure;
        }
        self.mods_scroll();
    }

    fn mods_scroll(&mut self) {
        const VISIBLE: usize = 5;
        let Some(ui) = self.mods_ui.as_mut() else {
            return;
        };
        if ui.selected < ui.offset {
            ui.offset = ui.selected;
        } else if ui.selected >= ui.offset + VISIBLE {
            ui.offset = ui.selected + 1 - VISIBLE;
        }
        ui.offset = ui.offset.min(ui.rows.len().saturating_sub(VISIBLE));
    }

    fn mods_press(&mut self) {
        let Some(ui) = self.mods_ui.as_ref() else {
            return;
        };
        let Some((id, enabled)) = ui
            .rows
            .get(ui.selected)
            .map(|entry| (entry.id.clone(), entry.enabled))
        else {
            return;
        };
        let Some(root) = self.mods_edit_root() else {
            return;
        };
        match slot_mods::toggle_mod(&root, &id, !enabled) {
            Ok(detail) => {
                if let Some(ui) = self.mods_ui.as_mut() {
                    if let Some(row) = ui.rows.iter_mut().find(|row| row.id == detail.entry.id)
                    {
                        *row = detail.entry;
                    }
                    ui.error = None;
                }
            }
            Err(error) => {
                if let Some(ui) = self.mods_ui.as_mut() {
                    ui.error = Some(error.message().to_string());
                }
            }
        }
    }

    fn wifi_input(&mut self, action: Action) {
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };
        if ui.connecting.is_some() {
            return;
        }
        let count = if !ui.radio_known {
            1
        } else {
            2 + if ui.radio_on {
                ui.rows.len() + if ui.manual { 1 } else { 0 }
            } else {
                0
            }
        };
        match action {
            Action::GbaDown(Btn::Up) if count > 0 => {
                if let Some(ui) = &mut self.wifi_ui {
                    ui.selected = (ui.selected + count - 1) % count;
                }
                self.wifi_scroll();
            }
            Action::GbaDown(Btn::Down) if count > 0 => {
                if let Some(ui) = &mut self.wifi_ui {
                    ui.selected = (ui.selected + 1) % count;
                }
                self.wifi_scroll();
            }
            Action::GbaDown(Btn::Left) if ui.radio_known && ui.selected == 1 => {
                self.adjust_wifi_auto_off(false);
            }
            Action::GbaDown(Btn::Right) if ui.radio_known && ui.selected == 1 => {
                self.adjust_wifi_auto_off(true);
            }
            Action::GbaDown(Btn::A) => self.wifi_press(),
            Action::GbaDown(Btn::X) => self.edit_saved_wifi(),
            Action::GbaDown(Btn::Y) => self.rescan_wifi(),
            Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.wifi_ui = None;
                self.keyboard = None;
                self.wifi_activity_finished();
            }
            _ => {}
        }
    }

    fn wifi_press(&mut self) {
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };        if ui.selected == 0 {
            self.toggle_wifi_radio();
            return;
        }
        if ui.selected == 1 {
            self.adjust_wifi_auto_off(true);
            return;
        }
        if !ui.radio_on {
            return;
        }
        if ui.selected <= ui.rows.len() + 1 {
            let row = ui.rows[ui.selected - 2].clone();
            if row.saved {
                let Some(root) = self.root.as_deref() else {
                    return;
                };
                let psk = linkcfg::saved_wifi_password(root, &row.ssid).unwrap_or_default();
                self.wifi_start_connect(row.ssid, psk);
            } else if row.secured {
                self.open_wifi_password(row.ssid);
            } else {
                self.wifi_start_connect(row.ssid, String::new());
            }
        } else if ui.manual && ui.selected == ui.rows.len() + 2 {
            self.open_wifi_ssid();
        }
    }

    /// Let the user replace a stale saved password without deleting the entry.
    /// A saved network normally connects immediately; if the router password
    /// changed, association times out, so X opens a fresh password prompt for
    /// the highlighted saved SSID.
    fn edit_saved_wifi(&mut self) {
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };
        if ui.selected < 2 || ui.selected > ui.rows.len() + 1 {
            return;
        }
        let row = &ui.rows[ui.selected - 2];
        if !row.saved {
            return;
        }
        let ssid = row.ssid.clone();
        self.open_wifi_password(ssid);
    }

    fn adjust_wifi_auto_off(&mut self, right: bool) {
        let lang = self.language;
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };
        let next = linkcfg::next_auto_off(ui.auto_off_minutes, right);
        if next == ui.auto_off_minutes {
            return;
        }
        let Some(root) = self.root.clone() else {
            if let Some(ui) = &mut self.wifi_ui {
                ui.error = Some(slot_i18n::t(lang, "device.app.power.no_card_root").into());
            }
            return;
        };
        match linkcfg::write_wifi_auto_off(&root, next) {
            Ok(()) => {
                self.wifi_auto_off_minutes = next;
                self.wifi_idle.reset();
                if let Some(ui) = &mut self.wifi_ui {
                    ui.auto_off_minutes = next;
                    ui.status = slot_i18n::t_args(lang, "device.app.wifi.auto_off", &[("value", linkcfg::auto_off_label(next))]);
                    ui.error = None;
                }
            }
            Err(message) => {
                if let Some(ui) = &mut self.wifi_ui {
                    ui.error = Some(slot_i18n::t_args(
                        lang,
                        "device.app.wifi.auto_off_failed",
                        &[("message", &message)],
                    ));
                }
            }
        }
    }

    fn toggle_wifi_radio(&mut self) {
        let lang = self.language;
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(ui) = &mut self.wifi_ui else {
            return;
        };
        if !ui.radio_known || ui.radio_job.is_some() || ui.connecting.is_some() {
            return;
        }
        let on = !ui.radio_on;
        ui.radio_pending = Some(on);
        ui.error = None;
        ui.status = if on {
            slot_i18n::t(lang, "device.app.wifi.turning_on").into()
        } else {
            slot_i18n::t(lang, "device.app.wifi.turning_off").into()
        };
        if !on {
            if let Some(mut job) = ui.job.take() {
                job.cancel();
            }
            ui.rows.clear();
            ui.selected = ui.selected.min(1);
            ui.offset = 0;
        }
        ui.radio_job = Some(wifi_job::spawn_radio(root, on));
    }

    fn open_game_search(&mut self) {
        let lang = self.language;
        let index = crate::game_search::GameSearchIndex::build(
            self.shelves
                .iter()
                .enumerate()
                .filter(|(_, (rack, _))| rack.platform().is_some())
                .flat_map(|(shelf, (rack, shelf_carts))| {
                    let platform = rack.platform().unwrap_or_default();
                    shelf_carts
                        .carts
                        .iter()
                        .enumerate()
                        .map(move |(number, cart)| (shelf, number, platform, cart))
                }),
        );
        self.search_index = Some(index);
        self.search_results = None;
        self.keyboard = Some(KeyboardState {
            purpose: KeyboardPurpose::GameSearch,
            title: slot_i18n::t(lang, "device.app.search.title").into(),
            masked: false,
            text: String::new(),
            cursor: slot_ui::keyboard::Cursor::default(),
            hint: search_keyboard_hint(lang),
            error: None,
            composition: String::new(),
            candidates: Vec::new(),
            candidate_page: 0,
            candidate_pages: 1,
            candidate_selected: 0,
            candidate_focus: false,
        });
    }

    fn open_wifi_password(&mut self, ssid: String) {
        let lang = self.language;
        self.keyboard = Some(KeyboardState {
            purpose: KeyboardPurpose::WifiPassword { ssid: ssid.clone() },
            title: slot_i18n::t_args(lang, "device.app.wifi.password_title", &[("ssid", ssid.as_str())]),
            masked: true,
            text: String::new(),
            cursor: slot_ui::keyboard::Cursor::default(),
            hint: slot_i18n::t(lang, "device.app.wifi.password_hint").into(),
            error: None,
            composition: String::new(),
            candidates: Vec::new(),
            candidate_page: 0,
            candidate_pages: 1,
            candidate_selected: 0,
            candidate_focus: false,
        });
    }

    fn open_wifi_ssid(&mut self) {
        let lang = self.language;
        self.keyboard = Some(KeyboardState {
            purpose: KeyboardPurpose::WifiSsid,
            title: slot_i18n::t(lang, "device.app.wifi.ssid_title").into(),
            masked: false,
            text: String::new(),
            cursor: slot_ui::keyboard::Cursor::default(),
            hint: slot_i18n::t(lang, "device.app.wifi.ssid_hint").into(),
            error: None,
            composition: String::new(),
            candidates: Vec::new(),
            candidate_page: 0,
            candidate_pages: 1,
            candidate_selected: 0,
            candidate_focus: false,
        });
    }

    fn wifi_start_connect(&mut self, ssid: String, psk: String) {
        let lang = self.language;
        if self.link_active() {
            self.refuse();
            return;
        }
        let Some(ui) = self.wifi_ui.as_ref() else {
            return;
        };
        if !ui.radio_known || !ui.radio_on || ui.connecting.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.keyboard = None;
        self.wifi_pending = Some((ssid.clone(), psk.clone()));
        if let Some(ui) = &mut self.wifi_ui {
            ui.connecting = Some(ssid.clone());
            ui.status = slot_i18n::t_args(lang, "device.app.wifi.connecting", &[("ssid", ssid.as_str())]);
            ui.error = None;
            crate::diag::write(
                &root,
                linkcfg::LogLevel::Debug,
                "wifi",
                format!("connect requested from UI: ssid={ssid:?}"),
            );
            ui.job = Some(wifi_job::spawn_connect(root, ssid, psk));
        }
    }

    fn game_search_input(&mut self, action: Action) {
        let lang = self.language;
        let Some(index) = self.search_index.as_ref() else {
            return;
        };
        let Some(kb) = self.keyboard.as_mut() else {
            return;
        };
        let mut run = false;
        match action {
            Action::GbaDown(Btn::Select) => {
                if kb.candidate_focus {
                    kb.candidate_focus = false;
                    kb.hint = search_keyboard_hint(lang);
                } else if !kb.candidates.is_empty() && !kb.composition.is_empty() {
                    kb.candidate_focus = true;
                    kb.hint = search_candidate_hint(lang);
                }
            }
            Action::GbaDown(Btn::Up) if kb.candidate_focus => {
                let pages = kb.candidate_pages.max(1);
                kb.candidate_page = (kb.candidate_page + pages - 1) % pages;
                refresh_search_candidates(kb, index, lang);
            }
            Action::GbaDown(Btn::Down) if kb.candidate_focus => {
                let pages = kb.candidate_pages.max(1);
                kb.candidate_page = (kb.candidate_page + 1) % pages;
                refresh_search_candidates(kb, index, lang);
            }
            Action::GbaDown(Btn::Left) if kb.candidate_focus => {
                if !kb.candidates.is_empty() {
                    kb.candidate_selected =
                        (kb.candidate_selected + kb.candidates.len() - 1) % kb.candidates.len();
                }
            }
            Action::GbaDown(Btn::Right) if kb.candidate_focus => {
                if !kb.candidates.is_empty() {
                    kb.candidate_selected = (kb.candidate_selected + 1) % kb.candidates.len();
                }
            }
            Action::GbaDown(Btn::A) if kb.candidate_focus => {
                commit_search_candidate(kb, lang);
            }
            Action::GbaDown(Btn::B) if kb.candidate_focus => {
                kb.candidate_focus = false;
                kb.hint = search_keyboard_hint(lang);
            }
            Action::GbaDown(Btn::Up) => kb.cursor = kb.cursor.up(),
            Action::GbaDown(Btn::Down) | Action::GbaDown(Btn::Left) | Action::GbaDown(Btn::Right) => {
                kb.cursor = match action {
                    Action::GbaDown(Btn::Down) => kb.cursor.down(),
                    Action::GbaDown(Btn::Left) => kb.cursor.left(),
                    _ => kb.cursor.right(),
                };
            }
            Action::GbaDown(Btn::Y) => kb.cursor = kb.cursor.cycle_layout(),
            Action::GbaDown(Btn::A) => match kb.cursor.key() {
                slot_ui::keyboard::Key::Char(c) => {
                    search_push(kb, c);
                    refresh_search_candidates(kb, index, lang);
                }
                slot_ui::keyboard::Key::Space => {
                    search_push(kb, ' ');
                    refresh_search_candidates(kb, index, lang);
                }
                slot_ui::keyboard::Key::Backspace => {
                    search_backspace(kb);
                    refresh_search_candidates(kb, index, lang);
                }
                slot_ui::keyboard::Key::Layout => kb.cursor = kb.cursor.cycle_layout(),
                slot_ui::keyboard::Key::Confirm => run = true,
                slot_ui::keyboard::Key::Cancel => {
                    self.keyboard = None;
                    self.search_index = None;
                    return;
                }
            },
            Action::GbaDown(Btn::B) => {
                search_backspace(kb);
                refresh_search_candidates(kb, index, lang);
            }
            Action::GbaDown(Btn::Start) => run = true,
            Action::GbaDown(Btn::X) | Action::QuickMenu => {
                self.close_game_search();
                return;
            }
            _ => {}
        }
        if run {
            self.search_confirm();
        }
    }

    fn search_confirm(&mut self) {
        let lang = self.language;
        let Some(mut kb) = self.keyboard.take() else {
            return;
        };
        if !kb.composition.is_empty() {
            if kb.candidates.is_empty() {
                kb.text.push_str(&kb.composition);
            } else {
                commit_search_candidate(&mut kb, lang);
            }
        }
        let query = kb.text.trim().to_string();
        if query.is_empty() {
            kb.error = Some(slot_i18n::t(lang, "device.app.search.prompt").into());
            kb.hint = search_keyboard_hint(lang);
            self.keyboard = Some(kb);
            return;
        }
        let Some(index) = self.search_index.as_ref() else {
            self.keyboard = Some(kb);
            return;
        };
        let hits = index.hits(&query);
        if hits.is_empty() {
            kb.error = Some(slot_i18n::t(lang, "device.app.search.none").into());
            kb.hint = search_keyboard_hint(lang);
            self.keyboard = Some(kb);
            return;
        }
        self.search_results = Some(SearchResults {
            query,
            hits,
            selected: 0,
            offset: 0,
        });
        self.keyboard = None;
    }

    fn search_results_input(&mut self, action: Action) {
        let lang = self.language;
        let Some(results) = self.search_results.as_mut() else {
            return;
        };
        match action {
            Action::GbaDown(Btn::Up) | Action::GbaDown(Btn::Down) => {
                if results.hits.is_empty() {
                    return;
                }
                let last = results.hits.len() - 1;
                results.selected = match action {
                    Action::GbaDown(Btn::Up) if results.selected == 0 => last,
                    Action::GbaDown(Btn::Up) => results.selected - 1,
                    _ if results.selected >= last => 0,
                    _ => results.selected + 1,
                };
                keep_search_result_visible(results);
            }
            Action::GbaDown(Btn::Left) | Action::GbaDown(Btn::Right) => {
                if results.hits.is_empty() {
                    return;
                }
                let page = SEARCH_ROWS;
                results.selected = match action {
                    Action::GbaDown(Btn::Left) => results.selected.saturating_sub(page),
                    _ => (results.selected + page).min(results.hits.len() - 1),
                };
                keep_search_result_visible(results);
            }
            Action::GbaDown(Btn::A) | Action::Insert => {
                let Some(hit) = results.hits.get(results.selected).cloned() else {
                    return;
                };
                self.search_results = None;
                self.search_index = None;
                self.select_search_hit(hit);
            }
            Action::GbaDown(Btn::B) => {
                let query = results.query.clone();
                self.search_results = None;
                self.search_index = None;
                self.open_game_search();
                if let Some(kb) = &mut self.keyboard {
                    kb.text = query;
                    kb.hint = search_keyboard_hint(lang);
                }
            }
            Action::GbaDown(Btn::X) | Action::QuickMenu => {
                self.close_game_search();
            }
            _ => {}
        }
    }

    /// Leave the search UI and force the shelf's faces to be rebuilt.  X used
    /// to drop only `keyboard`/`search_index`; any stale held/name state could
    /// leave the shelf with no card face for a frame (a black-looking screen).
    fn close_game_search(&mut self) {
        self.keyboard = None;
        self.search_index = None;
        self.search_results = None;
        self.play_held = None;
        self.shelf_mut().release_hold();
        self.shelf_mut().snap_to_selection();
        self.name_pending = true;
        self.shelf_named = None;
        self.shelf_platform = slot_ui::Printed::default();
    }

    fn select_search_hit(&mut self, hit: crate::game_search::SearchHit) {        if hit.shelf >= self.shelves.len() {
            return;
        }
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_at = hit.shelf;
        self.shelf_platform = slot_ui::Printed::default();
        self.slot_letter = None;
        self.shelf_named = None;
        self.name_pending = true;
        self.shelf_mut().select(hit.index);
    }

    fn keyboard_input(&mut self, action: Action) {
        let lang = self.language;
        if self
            .keyboard
            .as_ref()
            .is_some_and(|kb| matches!(kb.purpose, KeyboardPurpose::GameSearch))
        {
            return self.game_search_input(action);
        }
        let (mut confirm, mut cancel) = (false, false);
        if let Some(kb) = &mut self.keyboard {
            match action {
                Action::GbaDown(Btn::Up) => kb.cursor = kb.cursor.up(),
                Action::GbaDown(Btn::Down) => kb.cursor = kb.cursor.down(),
                Action::GbaDown(Btn::Left) => kb.cursor = kb.cursor.left(),
                Action::GbaDown(Btn::Right) => kb.cursor = kb.cursor.right(),
                Action::GbaDown(Btn::Y) => kb.cursor = kb.cursor.cycle_layout(),
                Action::GbaDown(Btn::A) => match kb.cursor.key() {
                    slot_ui::keyboard::Key::Char(c) => keyboard_push(kb, c, lang),
                    slot_ui::keyboard::Key::Space => keyboard_push(kb, ' ', lang),
                    slot_ui::keyboard::Key::Backspace => {
                        kb.text.pop();
                        kb.error = None;
                    }
                    slot_ui::keyboard::Key::Layout => kb.cursor = kb.cursor.cycle_layout(),
                    slot_ui::keyboard::Key::Confirm => confirm = true,
                    slot_ui::keyboard::Key::Cancel => cancel = true,
                },
                Action::GbaDown(Btn::B) => {
                    kb.text.pop();
                    kb.error = None;
                }
                Action::GbaDown(Btn::Start) => confirm = true,
                Action::GbaDown(Btn::X) | Action::QuickMenu => cancel = true,
                _ => {}
            }
        }
        if cancel {
            self.keyboard = None;
        }
        if confirm {
            self.keyboard_confirm();
        }
    }

    fn keyboard_confirm(&mut self) {
        let lang = self.language;
        let Some(mut kb) = self.keyboard.take() else {
            return;
        };
        match &kb.purpose {
            KeyboardPurpose::WifiSsid => {
                let ssid = kb.text.clone();
                if ssid.is_empty() {
                    kb.error = Some(slot_i18n::t(lang, "device.app.wifi.ssid_empty").into());
                    self.keyboard = Some(kb);
                    return;
                }
                if ssid.len() > 32 || ssid.chars().any(char::is_control) {
                    kb.error = Some(slot_i18n::t(lang, "device.app.wifi.ssid_length").into());
                    self.keyboard = Some(kb);
                    return;
                }
                self.open_wifi_password(ssid);
            }
            KeyboardPurpose::WifiPassword { ssid } => {
                let ssid = ssid.clone();
                let psk = kb.text.clone();
                if !psk.is_empty() && psk.chars().count() < 8 {
                    kb.error = Some(slot_i18n::t(lang, "device.app.wifi.password_short").into());
                    self.keyboard = Some(kb);
                    return;
                }
                if psk.chars().count() > 63
                    && !(psk.len() == 64 && psk.chars().all(|c| c.is_ascii_hexdigit()))
                {
                    kb.error = Some(slot_i18n::t(lang, "device.app.wifi.password_long").into());
                    self.keyboard = Some(kb);
                    return;
                }
                self.wifi_start_connect(ssid, psk);
            }
            KeyboardPurpose::GameSearch => {
                self.keyboard = Some(kb);
            }
        }
    }

    fn open_web(&mut self) {
        let lang = self.language;
        if self.web.is_some() || self.web_starting.is_some() {
            return;
        }
        self.wifi_activity_started();
        let Some(root) = self.root.clone() else {
            self.web_popup = Some(WebPopup::Failed {
                message: slot_i18n::t(lang, "device.app.power.no_card_root").into(),
            });
            return;
        };
        self.web_popup = Some(WebPopup::Starting);
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("slot-web-start".into())
            .spawn(move || {
                let _ = tx.send(WebServer::start(root));
            });
        match spawned {
            Ok(_) => self.web_starting = Some(rx),
            Err(e) => {
                let error = e.to_string();
                self.web_popup = Some(WebPopup::Failed {
                    message: slot_i18n::t_args(lang, "device.app.web.start_failed", &[("e", error.as_str())]),
                });
            }
        }
    }

    fn poll_web_start(&mut self) {
        let lang = self.language;
        let mut outcome: Option<Result<WebServer, String>> = None;
        if let Some(rx) = &self.web_starting {
            match rx.try_recv() {
                Ok(result) => outcome = Some(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    outcome = Some(Err(slot_i18n::t(lang, "device.app.web.thread_gone").into()))
                }
            }
        }
        let Some(result) = outcome else {
            return;
        };
        self.web_starting = None;
        let cancelled = std::mem::take(&mut self.web_cancel);
        if cancelled {
            if let Ok(mut server) = result {
                server.stop();
            }
            return;
        }
        match result {
            Ok(server) => {
                let url = server.url().to_string();
                server.set_writable(self.web_writable());
                self.web = Some(server);
                self.web_popup = Some(WebPopup::Started { url });
            }
            Err(message) => self.web_popup = Some(WebPopup::Failed { message }),
        }
    }

    fn web_popup_input(&mut self, action: Action) {
        if matches!(&self.web_popup, Some(WebPopup::Starting)) {
            if matches!(
                action,
                Action::GbaDown(Btn::B) | Action::GbaDown(Btn::A) | Action::QuickMenu
            ) {
                self.web_cancel = true;
                self.web_popup = None;
            }
            return;
        }
        if matches!(
            action,
            Action::GbaDown(Btn::B) | Action::GbaDown(Btn::A) | Action::QuickMenu
        ) {
            self.stop_web();
        }
    }

    pub(crate) fn stop_web(&mut self) {
        let library_changed = self
            .web
            .as_ref()
            .is_some_and(|web| web.take_library_changed());
        self.web_starting = None;
        self.web_cancel = false;
        self.web_popup = None;
        if let Some(mut server) = self.web.take() {
            server.stop();
        }
        self.wifi_activity_finished();
        self.invalidate_shelf_playtime();
        if library_changed {
            self.refresh_library_after_web();
        }
    }

    fn web_writable(&self) -> bool {
        self.state.cart.is_none()
            && !self.link_active()
            && matches!(self.shelf_sync, ShelfSync::Idle)
    }

    fn wifi_power_busy(&self) -> bool {
        self.wifi_ui.is_some()
            || self.web.is_some()
            || self.web_starting.is_some()
            || matches!(
                self.shelf_sync,
                ShelfSync::Holding { .. } | ShelfSync::Running { .. }
            )
            // Link play and link setup use the Wi-Fi/TCP transport too; an
            // active session or a Waiting/Host/Join menu must keep the radio
            // on just like cloud sync does.
            || self.link_active()
            || self.starting.is_some()
            || self.link_reload.is_some()
            || self.game_menu.is_some()
            || self.shutting_down()
    }

    fn wifi_auto_off_root(&self) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        root.join(crate::root::SYSTEM_SHELL)
            .join("slot-wifi.sh")
            .is_file()
            .then(|| root.clone())
    }

    /// Called when the user starts using Wi-Fi: cancel a pending auto-off and
    /// restart the idle countdown when the activity ends.
    fn wifi_activity_started(&mut self) {
        self.wifi_idle.reset();
        self.wifi_radio_poll_at = 0;
        self.wifi_off_retry_at = 0;
        if let Some(mut job) = self.wifi_off_job.take() {
            job.cancel();
        }
        if let Some(mut job) = self.wifi_radio_job.take() {
            job.cancel();
        }
    }

    fn wifi_activity_finished(&mut self) {
        self.wifi_idle.reset();
        self.wifi_radio_poll_at = 0;
        self.wifi_off_retry_at = 0;
    }

    /// Poll the radio state and the auto-off job. This runs from `timers`
    /// every frame; scripts are only spawned at most once per minute.
    fn poll_wifi_power(&mut self) {
        use crate::wifi_power::{IdleAction, OFF_RETRY_MS, RADIO_POLL_MS};

        let now = self.now();
        if let Some(mut job) = self.wifi_off_job.take() {
            if let Some(message) = job.try_recv() {
                match message {
                    WifiMsg::Radio(Ok(false)) => {
                        self.wifi_radio_on = Some(false);
                        self.wifi_idle.reset();
                        self.wifi_off_retry_at = 0;
                        self.wifi_radio_poll_at = now.saturating_add(RADIO_POLL_MS);
                    }
                    WifiMsg::Radio(Ok(true)) => {
                        self.wifi_radio_on = Some(true);
                        self.wifi_off_retry_at = now.saturating_add(OFF_RETRY_MS);
                    }
                    WifiMsg::Radio(Err(message)) => {
                        eprintln!("slot: wifi auto-off: {message}");
                        self.wifi_off_retry_at = now.saturating_add(OFF_RETRY_MS);
                    }
                    _ => {}
                }
            } else {
                self.wifi_off_job = Some(job);
            }
        }

        if self.wifi_auto_off_minutes == 0 {
            self.wifi_idle.reset();
            if let Some(mut job) = self.wifi_radio_job.take() {
                job.cancel();
            }
            return;
        }

        if let Some(mut job) = self.wifi_radio_job.take() {
            if let Some(message) = job.try_recv() {
                match message {
                    WifiMsg::Radio(Ok(on)) => {
                        self.wifi_radio_on = Some(on);
                        self.wifi_idle.reset();
                        self.wifi_radio_poll_at = now.saturating_add(RADIO_POLL_MS);
                    }
                    WifiMsg::Radio(Err(message)) => {
                        eprintln!("slot: wifi radio status: {message}");
                        self.wifi_radio_poll_at = now.saturating_add(RADIO_POLL_MS);
                    }
                    _ => {}
                }
            } else {
                self.wifi_radio_job = Some(job);
            }
        }

        if self.wifi_power_busy() {
            self.wifi_idle.reset();
            return;
        }

        if now >= self.wifi_radio_poll_at && self.wifi_ui.is_none() {
            if let Some(root) = self.wifi_auto_off_root() {
                if self.wifi_radio_job.is_none() {
                    self.wifi_radio_job = Some(wifi_job::spawn_radio_status(root));
                    self.wifi_radio_poll_at = now.saturating_add(RADIO_POLL_MS);
                }
            } else {
                // No device script (host build, broken card): never guess.
                self.wifi_radio_on = None;
                self.wifi_idle.reset();
                self.wifi_radio_poll_at = now.saturating_add(RADIO_POLL_MS);
                return;
            }
        }

        let action = self.wifi_idle.tick(
            now,
            false,
            self.wifi_radio_on,
            self.wifi_auto_off_minutes,
        );
        if action == IdleAction::TurnOff
            && self.wifi_off_job.is_none()
            && now >= self.wifi_off_retry_at
        {
            if let Some(root) = self.wifi_auto_off_root() {
                if let Some(mut job) = self.wifi_radio_job.take() {
                    job.cancel();
                }
                self.wifi_off_job = Some(wifi_job::spawn_radio(root, false));
            } else {
                self.wifi_radio_on = None;
                self.wifi_idle.reset();
            }
        }
    }

    fn begin_clock_sync(&mut self) {
        let now = self.now();
        if self.clock_sync_job.is_some() || now < self.clock_sync_retry_at {
            return;
        }
        // Avoid spawning a second lookup while this one is in flight.
        self.clock_sync_retry_at = now.saturating_add(CLOCK_SYNC_RETRY_MS);
        self.clock_sync_job = Some(crate::network_time::spawn());
    }

    fn poll_clock_sync(&mut self) {
        let lang = self.language;
        if self.clock_sync_job.is_none() && self.wifi_connected {
            self.begin_clock_sync();
        }
        let Some(receiver) = &self.clock_sync_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(slot_i18n::t(lang, "device.app.clock.thread_gone").into()),
        };
        self.clock_sync_job = None;
        match result {
            Ok(utc) => {
                if let Some(power) = &mut self.power {
                    power.set_clock(utc);
                }
                self.state.clock_set = true;
                self.clock_sync_retry_at = self.now().saturating_add(CLOCK_SYNC_REFRESH_MS);
                self.persist();
                self.hud.toast(Toast::NetworkTimeSynced, self.now());
            }
            Err(error) => {
                eprintln!("slot: network time sync: {error}");
                self.clock_sync_retry_at = self.now().saturating_add(CLOCK_SYNC_RETRY_MS);
            }
        }
    }

    fn poll_wifi(&mut self) {
        let lang = self.language;
        let mut persist: Option<(String, String)> = None;
        let root = self.root.clone();
        if let Some(ui) = &mut self.wifi_ui {
            if let Some(job) = &mut ui.radio_job {
                match job.try_recv() {
                    Some(WifiMsg::Radio(Ok(on))) => {
                        ui.radio_on = on;
                        ui.radio_known = true;
                        ui.radio_job = None;
                        ui.radio_pending = None;
                        ui.error = None;
                        self.wifi_radio_on = Some(on);
                        self.wifi_idle.reset();
                        self.wifi_off_retry_at = 0;
                        if on {
                            ui.status = slot_i18n::t(lang, "device.app.wifi.scanning").into();
                            if let Some(root) = root.clone() {
                                ui.job = Some(wifi_job::spawn_scan(root));
                            }
                        } else {
                            if let Some(mut job) = ui.job.take() {
                                job.cancel();
                            }
                            ui.rows.clear();
                            ui.connected = None;
                            ui.connecting = None;
                            ui.selected = ui.selected.min(1);
                            ui.offset = 0;
                            ui.status = slot_i18n::t(lang, "device.app.wifi.off").into();
                        }
                    }
                    Some(WifiMsg::Radio(Err(message))) => {
                        ui.radio_known = true;
                        ui.radio_job = None;
                        ui.radio_pending = None;
                        ui.error = Some(message);
                        ui.status = slot_i18n::t(lang, "device.app.wifi.toggle_failed").into();
                        self.wifi_radio_on = None;
                        self.wifi_idle.reset();
                    }
                    Some(_) => {}
                    None => {}
                }
            }
            if let Some(job) = &mut ui.status_job {
                match job.try_recv() {
                    Some(WifiMsg::Status(Ok(WifiStatus::Connected { ssid, ip }))) => {
                        if ui.radio_on || !ui.radio_known {
                            ui.connected = Some(format!("{ssid} · {ip}"));
                        }
                        ui.status_job = None;
                    }
                    Some(WifiMsg::Status(Ok(WifiStatus::Disconnected))) => {
                        ui.connected = None;
                        ui.status_job = None;
                    }
                    Some(WifiMsg::Status(Err(_))) => ui.status_job = None,
                    Some(_) => {}
                    None => {}
                }
            }
            if let Some(job) = &mut ui.job {
                match job.try_recv() {
                    Some(WifiMsg::Scan(Ok(rows))) => {
                        let scanned = rows
                            .into_iter()
                            .map(|n| slot_ui::network::WifiEntry {
                                ssid: n.ssid,
                                signal: n.signal,
                                secured: n.secured,
                                saved: false,
                            })
                            .collect();
                        ui.rows = merge_saved_wifi_rows(&ui.saved, scanned);
                        ui.selected = ui.selected.min(ui.rows.len() + 2);
                        if ui.selected < 2 && !ui.rows.is_empty() {
                            ui.selected = 2;
                        }
                        ui.offset = ui
                            .offset
                            .min(ui.rows.len().saturating_sub(Self::WIFI_ROWS));
                        ui.status = slot_i18n::t(lang, "device.app.wifi.pick").into();
                        ui.error = None;
                        ui.job = None;
                    }
                    Some(WifiMsg::Scan(Err(message))) => {
                        ui.status = slot_i18n::t(lang, "device.app.wifi.scan_failed").into();
                        ui.error = Some(message);
                        ui.job = None;
                    }
                    Some(WifiMsg::Connect(Ok(WifiStatus::Connected { ssid, ip }))) => {
                        ui.connected = Some(format!("{ssid} · {ip}"));
                        ui.connecting = None;
                        ui.status = slot_i18n::t(lang, "device.app.wifi.connected").into();
                        ui.error = None;
                        ui.job = None;
                        ui.radio_on = true;
                        self.wifi_radio_on = Some(true);
                        self.wifi_idle.reset();
                        if let Some((ssid, psk)) = self.wifi_pending.take() {
                            persist = Some((ssid, psk));
                        }
                        self.wifi_connected = true;
                        self.begin_clock_sync();
                    }
                    Some(WifiMsg::Connect(Ok(WifiStatus::Disconnected))) => {
                        ui.connecting = None;
                        ui.error = Some(slot_i18n::t(lang, "device.app.wifi.no_ip").into());
                        ui.job = None;
                    }
                    Some(WifiMsg::Connect(Err(message))) => {
                        ui.connecting = None;
                        ui.error = Some(slot_i18n::t_args(
                            lang,
                            "device.app.wifi.retry_password",
                            &[("message", &message)],
                        ));
                        ui.job = None;
                    }
                    Some(WifiMsg::Status(_)) | Some(WifiMsg::Radio(_)) => {}
                    None => {}
                }
            }
        }
        if let Some((ssid, psk)) = persist {
            if let Some(root) = self.root.clone() {
                match linkcfg::write_wifi(&root, &ssid, &psk) {
                    Ok(()) => {
                        if let Some(ui) = &mut self.wifi_ui {
                            ui.saved = linkcfg::read_saved_wifi(&root);
                            ui.rows = merge_saved_wifi_rows(&ui.saved, ui.rows.clone());
                        }
                    }
                    Err(message) => {
                        if let Some(ui) = &mut self.wifi_ui {
                            ui.error = Some(slot_i18n::t_args(lang, "device.app.wifi.save_failed", &[("message", message.as_str())]));
                        }
                    }
                }
            }
        }
        if let Some(web) = &self.web {
            web.set_writable(self.web_writable());
        }
    }

    /// Switch `[sync] mode` from the QuickMenu and save it immediately.
    fn change_sync_mode(&mut self, mode: linkcfg::SyncMode) {
        let Some(root) = self.root.clone() else {
            self.refuse();
            return;
        };
        match linkcfg::write_sync_mode(&root, mode) {
            Ok(()) => self.sync_mode = mode,
            Err(message) => {
                eprintln!("slot: 保存云同步内容失败：{message}");
                self.refuse();
            }
        }
    }

    fn theme_choice(row: QuickRow) -> Option<ThemePreset> {
        match row {
            QuickRow::ThemeDefault => Some(ThemePreset::Default),
            QuickRow::ThemeWeb => Some(ThemePreset::Web),
            QuickRow::ThemeWarm => Some(ThemePreset::Warm),
            _ => None,
        }
    }

    fn theme_value(&self, preset: ThemePreset) -> QuickValue {
        if self.theme_preset == preset {
            QuickValue::ThemeCurrent
        } else {
            QuickValue::ThemePick
        }
    }

    fn apply_theme(&mut self, preset: ThemePreset) {
        if preset == self.theme_preset {
            return;
        }
        self.theme_preset = preset;
        if let Some(root) = self.root.as_deref() {
            if let Err(error) = Theme::write_preset(root, preset) {
                eprintln!("slot: 保存主题失败：{error}");
            }
        }
        self.hud.toast(Toast::ThemeSaved, self.now());
    }

    fn change_setting(&mut self, row: QuickRow, right: bool) {
        if row == QuickRow::Theme {
            return;
        }
        if let Some(preset) = Self::theme_choice(row) {
            self.apply_theme(preset);
            return;
        }
        if row == QuickRow::SyncContent {
            let next = self.sync_mode.next(right);
            if next != self.sync_mode {
                self.change_sync_mode(next);
            }
            return;
        }
        if row == QuickRow::Language {
            // The language is not part of `state`, so it returns before the
            // `persist()` at the bottom of this function.
            self.cycle_language(right);
            return;
        }
        if let Some(action) = Self::carousel_action(row) {
            self.change_carousel(action, right);
            return;
        }
        if let Some(shortcut) = Self::shortcut_row(row) {
            self.change_shortcut(shortcut, right);
            return;
        }
        if let Some(button) = Self::control_row(row) {
            self.change_control(button, right);
            return;
        }
        if row == QuickRow::Brightness || row == QuickRow::BlueLight {
            let action = match (row, right) {
                (QuickRow::Brightness, true) => Action::BrightnessUp,
                (QuickRow::Brightness, false) => Action::BrightnessDown,
                (QuickRow::BlueLight, true) => Action::BlueLightUp,
                _ => Action::BlueLightDown,
            };
            self.adjust(action);
            self.persist();
            return;
        }
        if row == QuickRow::FallbackCover {
            self.state.fallback_cover = !self.state.fallback_cover;
            self.persist();
            // Existing face textures must be rebuilt with or without the
            // state screenshot fallback.
            self.invalidate_cart_faces();
            return;
        }
        let s = &mut self.state;
        match row {
            QuickRow::FastForward => {
                let to = ff_next(s.ff_speed, right);
                if to == s.ff_speed {
                    return;
                }
                s.ff_speed = to;
            }
            QuickRow::FastForwardSound => s.ff_sound = !s.ff_sound,
            QuickRow::ColourCorrection => {
                s.colour_correction = !s.colour_correction;
                self.colour_pending = Some(s.colour_correction);
            }
            QuickRow::GbaShader => {
                let to = s.shader_gba.step(&Shader::GBA, right);
                if to == s.shader_gba {
                    return;
                }
                s.shader_gba = to;
            }
            QuickRow::GbShader => {
                let to = s.shader_gb.step(&Shader::GB, right);
                if to == s.shader_gb {
                    return;
                }
                s.shader_gb = to;
            }
            QuickRow::Rumble => s.rumble = !s.rumble,
            QuickRow::SaveSettings => {}
            QuickRow::FcMask => self.cycle_mask(Platform::Fc, right),
            QuickRow::GbMask => self.cycle_mask(Platform::Gb, right),
            QuickRow::GbcMask => self.cycle_mask(Platform::Gbc, right),
            QuickRow::SfcMask => self.cycle_mask(Platform::Sfc, right),
            QuickRow::DefaultLoad => s.toggle_resume_default(ResumeKind::Load),
            QuickRow::DefaultSave => s.toggle_resume_default(ResumeKind::Save),
            QuickRow::LoadGba => s.toggle_resume(Platform::Gba, ResumeKind::Load),
            QuickRow::SaveGba => s.toggle_resume(Platform::Gba, ResumeKind::Save),
            QuickRow::LoadGb => s.toggle_resume(Platform::Gb, ResumeKind::Load),
            QuickRow::SaveGb => s.toggle_resume(Platform::Gb, ResumeKind::Save),
            QuickRow::LoadGbc => s.toggle_resume(Platform::Gbc, ResumeKind::Load),
            QuickRow::SaveGbc => s.toggle_resume(Platform::Gbc, ResumeKind::Save),
            QuickRow::Recents => {
                s.show_recents = !s.show_recents;
                self.restock_recents();
            }
            QuickRow::RomTitle => s.show_rom_title = !s.show_rom_title,
            QuickRow::LidTimeout => { let choices=[3,10,30,0]; let i=choices.iter().position(|v|*v==s.lid_minutes).unwrap_or(0); s.lid_minutes=choices[(i+if right {1}else{3})%4]; },
            QuickRow::ButtonLayout => {}
            QuickRow::ControlPreset => {
                s.button_layout = s.button_layout.step(right);
                if s.button_layout.uses_xy() {
                    s.turbo = false;
                }
            }
            QuickRow::Turbo => s.turbo = !s.button_layout.uses_xy() && !s.turbo,
            QuickRow::Rewind => s.rewind = !s.rewind,
            QuickRow::GbPalettes => {
                if !s.gb_palettes {
                    s.gb_palettes = true;
                    s.gb_palette = if right {
                        GbPalette::at(0)
                    } else {
                        GbPalette::at(GbPalette::COUNT - 1)
                    };
                } else if (right && s.gb_palette.index() + 1 >= GbPalette::COUNT)
                    || (!right && s.gb_palette.index() == 0)
                {
                    s.gb_palettes = false;
                } else {
                    s.gb_palette = if right {
                        s.gb_palette.next()
                    } else {
                        s.gb_palette.prev()
                    };
                }
            }
            QuickRow::Shortcuts | QuickRow::DateTime | QuickRow::Mods | QuickRow::ButtonSettings
            | QuickRow::SystemSettings | QuickRow::About | QuickRow::SystemUpgrade | QuickRow::Screen
            | QuickRow::Game | QuickRow::Network | QuickRow::ShelfKeys | QuickRow::ShortcutMap
            | QuickRow::Wifi | QuickRow::Web | QuickRow::SyncStorage | QuickRow::SyncContent
            | QuickRow::Brightness
            | QuickRow::BlueLight
            | QuickRow::FallbackCover
            | QuickRow::ShelfPrev | QuickRow::ShelfNext | QuickRow::ShelfFav
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
            | QuickRow::ShortcutRewind
            | QuickRow::ControlUp
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
            | QuickRow::CartTransfer
            | QuickRow::SplitLink
            | QuickRow::Shelf
            | QuickRow::ShelfLook
            | QuickRow::DisplayPanel
            | QuickRow::DisplayEffects
            | QuickRow::Masks
            | QuickRow::ShortcutReset
            | QuickRow::Theme
            | QuickRow::ThemeDefault
            | QuickRow::ThemeWeb
            | QuickRow::ThemeWarm
            // Handled above: it writes `linkSlot/Config/language.txt`, not `slot.state`.
            | QuickRow::Language => return,
        }
        self.persist();
    }
    pub fn apply_at(&mut self, action: Action, now: Millis) {
        self.clock = now as f64;
        self.apply(action);
    }

    fn step_shader(&mut self, right: bool) {
        if !matches!(self.phase, Phase::Inserting { .. } | Phase::Playing { .. }) {
            return;
        }
        let (shader, choices) = match self.platform {
            Platform::Gba => (&mut self.state.shader_gba, &Shader::GBA[..]),
            // FC rides the Game Boy bucket — see `screen_shader` for why.
            Platform::Gb | Platform::Gbc | Platform::Fc | Platform::Sfc | Platform::Md => {
                (&mut self.state.shader_gb, &Shader::GB[..])
            }
        };
        *shader = shader.step(choices, right);
        let said = match *shader {
            Shader::Off => Toast::ShaderOff,
            Shader::Lcd3x => Toast::ShaderLcd3x,
            Shader::Grid => Toast::ShaderGrid,
            Shader::Dot => Toast::ShaderDot,
            Shader::Simpletex => Toast::ShaderSimpletex,
            Shader::FourPass => Toast::ShaderFourPass,
        };
        self.persist();
        self.hud.toast(said, self.now());
    }

    fn adjust(&mut self, action: Action) -> bool {
        if action == Action::MuteToggle {
            self.mute_toggle();
            return true;
        }
        if let Action::ShaderNext | Action::ShaderPrev = action {
            self.step_shader(action == Action::ShaderNext);
            return true;
        }
        if let Action::PaletteNext | Action::PalettePrev = action {
            if self.palette_live && matches!(self.phase, Phase::Playing { .. }) {
                let to = match action {
                    Action::PalettePrev => self.state.gb_palette.prev(),
                    _ => self.state.gb_palette.next(),
                };
                self.state.gb_palette = to;
                self.palette_pending = Some(to);
                self.persist();
                self.hud.toast(Toast::Palette(to), self.now());
            }
            return true;
        }
        if action == Action::ColourCorrectionToggle {
            self.change_setting(QuickRow::ColourCorrection, true);
            let said = match self.state.colour_correction {
                true => Toast::ColourOn,
                false => Toast::ColourOff,
            };
            self.hud.toast(said, self.now());
            return true;
        }
        let s = &self.state;
        let (kind, value) = match action {
            Action::BrightnessUp => (HudKind::Brightness, up(s.brightness, 1, BRIGHTNESS_MAX)),
            Action::BrightnessDown => (HudKind::Brightness, s.brightness.saturating_sub(1)),
            Action::BlueLightUp => (HudKind::BlueLight, up(s.blue_light, 1, BLUE_LIGHT_MAX)),
            Action::BlueLightDown => (HudKind::BlueLight, s.blue_light.saturating_sub(1)),
            Action::VolumeUp => (HudKind::Volume, up(self.level(), VOLUME_STEP, VOLUME_MAX)),
            Action::VolumeDown => (HudKind::Volume, self.level().saturating_sub(VOLUME_STEP)),
            _ => return false,
        };
        if kind == HudKind::Volume {
            self.remember_volume();
        }
        let level = match kind {
            HudKind::Brightness => &mut self.state.brightness,
            HudKind::BlueLight => &mut self.state.blue_light,
            HudKind::Volume if self.headphones => &mut self.state.volume_hp,
            HudKind::Volume => &mut self.state.volume,
            HudKind::Rewind => return false,
        };
        let moved = *level != value;
        *level = value;
        let unmuted = kind == HudKind::Volume && std::mem::take(self.muted_mut());
        let (shown, now) = (self.hud_value(kind, value), self.now());
        self.hud.show(kind, shown, self.muted(), now);
        if let (HudKind::Brightness, Some(power)) = (kind, &mut self.power) {
            power.set_backlight(value);
        }
        if moved || unmuted {
            self.persist_soon();
        }
        true
    }

    fn remember_volume(&mut self) {
        if self.vol_before.len() == 2 {
            self.vol_before.remove(0);
        }
        self.vol_before
            .push((self.level(), self.muted(), self.now()));
    }

    fn mute_toggle(&mut self) {
        let now = self.now();
        if let Some((volume, muted, _)) = self
            .vol_before
            .iter()
            .find(|(_, _, at)| now.saturating_sub(*at) <= MUTE_CHORD_MS)
            .copied()
        {
            *self.level_mut() = volume;
            *self.muted_mut() = muted;
        }
        self.vol_before.clear();
        let muted = !self.muted();
        *self.muted_mut() = muted;
        self.hud
            .show(HudKind::Volume, self.output_volume(), muted, now);
        self.persist_soon();
    }

    fn hud_value(&self, kind: HudKind, value: u8) -> u8 {
        match kind {
            HudKind::Volume => self.output_volume(),
            _ => value,
        }
    }

    pub fn show_rewind(&mut self, fill: u8) {
        let now = self.now();
        self.hud.show(HudKind::Rewind, fill, false, now);
    }

    pub fn hide_rewind(&mut self) {
        self.hud.release_rewind();
    }

    pub fn set_ff(&mut self, ff: FfState) {
        self.hud.set_ff(ff);
    }

    pub fn ff_badge(&self) -> Option<Icon> {
        self.hud.badge()
    }

    pub fn blue_light(&self) -> u8 {
        self.state.blue_light
    }

    pub fn brightness(&self) -> u8 {
        self.state.brightness
    }

    pub fn volume(&self) -> u8 {
        self.level()
    }

    fn level(&self) -> u8 {
        if self.headphones {
            self.state.volume_hp
        } else {
            self.state.volume
        }
    }

    fn level_mut(&mut self) -> &mut u8 {
        if self.headphones {
            &mut self.state.volume_hp
        } else {
            &mut self.state.volume
        }
    }

    pub fn muted(&self) -> bool {
        if self.headphones {
            self.state.muted_hp
        } else {
            self.state.muted
        }
    }

    fn muted_mut(&mut self) -> &mut bool {
        if self.headphones {
            &mut self.state.muted_hp
        } else {
            &mut self.state.muted
        }
    }

    pub fn output_volume(&self) -> u8 {
        if self.muted() {
            0
        } else {
            self.level()
        }
    }

    pub fn hud_icon(&self) -> Icon {
        self.hud.glyph()
    }

    pub fn now(&self) -> Millis {
        self.clock as Millis
    }

    fn flick(&mut self, step: fn(&mut Polaroids)) {
        if let Some(p) = &mut self.polaroids {
            step(p);
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.clock += dt as f64 * 1000.0;
        if self.front_deck.is_some() && self.core_picker.is_none() {
            self.close_front_deck();
        }
        self.update_front_deck();
        self.poll_language_file();
        self.rom_index.poll();
        self.persist_rom_sha_updates();
        // Never touch the card while a game, a menu panel, a link or recent
        // input owns it: the index is allowed to lag, the session is not.
        let idle = self.now().saturating_sub(self.last_input_at) >= 1500;
        self.rom_index.set_paused(
            self.link_active() || !matches!(self.phase, Phase::Shelf) || !idle,
        );
        self.accrue_playtime(dt);
        if self.name_pending && self.shelf_platform.face.is_some() {
            self.name_pending = false;
            self.shelf_named = Some(self.now());
        }
        self.timers();
        self.poll_link();
        self.expire_lost_link();
        self.poll_cart_transfer();
        if let Some(at) = self.transfer_insert_at {
            if self.now().saturating_sub(at) as f32 >= INSERT_S * 1000.0 {
                self.transfer_insert_at = None;
            }
        }
        if self.transfer_done_pending {
            let now = self.now();
            if let Some(started) = self.transfer_progress_started {
                if now.saturating_sub(started) >= TRANSFER_MIN_VISIBLE_MS {
                    let received = self.transfer.as_ref().is_some_and(|t| t.received);
                    self.finish_transfer_done(now, received);
                }
            }
        }
        if let Some(at) = self.transfer_eject_at {
            if self.now().saturating_sub(at) as f32 >= EJECT_S * 1000.0 {
                self.transfer_eject_at = None;
                self.transfer_insert_at = None;
                self.transfer_insert_stem = None;
                self.transfer_seated = false;
                self.transfer_progress_started = None;
                self.transfer_done_pending = false;
                self.transfer_blank_face = None;
                if let Some(transfer) = &mut self.transfer {
                    if transfer.stage == TransferStage::Duplicate {
                        transfer.reset_after_duplicate();
                    } else if transfer.stage == TransferStage::Done {
                        transfer.reset_after_done();
                    }
                }
            }
        }
        self.poll_shelf_sync();
        let now = self.now();
        let mut drop_intro = false;
        let mut arm_intro = false;
        if let Some(intro) = &self.game_intro {
            if let Some(cancel) = intro.cancel {
                drop_intro = now.saturating_sub(cancel) >= INTRO_CANCEL_MS;
            } else if intro.closing.is_some_and(|since| now.saturating_sub(since) >= INTRO_CLOSE_MS) {
                drop_intro = true;
            } else if intro.closing.is_none()
                && !intro.armed
                && now.saturating_sub(intro.started) >= INTRO_OPEN_MS
            {
                arm_intro = true;
            }
        }
        if drop_intro {
            self.game_intro = None;
            self.utility = None;
        }
        if arm_intro {
            if let Some(intro) = &mut self.game_intro {
                intro.armed = true;
            }
        }
        let ready = self.core_faces_ready();
        if let Some(picker) = &mut self.core_picker {
            if picker.waiting() && (ready || picker.waited(now) >= FACES_WAIT_MS) {
                picker.start(now);
            }
        }
        if self.core_picker.as_ref().is_some_and(|p| p.finished(now)) {
            self.core_picker = None;
        }
        if !self.on_shelf() {
            self.shelf_mut().release_hold();
        }
        let mut touched = false;
        let at = self.shelf_at;
        let next = match &mut self.phase {
            Phase::Shelf => {
                let shelf = &mut self.shelves[at].1;
                shelf.tick(now);
                shelf.update(dt);
                None
            }
            Phase::Inserting {
                cart,
                t,
                core_ready,
                resumed,
                ..
            } => {
                let was = *t;
                *t += dt;
                let at = SEATED_AT - Sfx::Insert.lead();
                touched = !*resumed && was < at && *t >= at;
                (*t >= INSERT_S && *core_ready).then(|| Phase::Playing {
                    cart: std::mem::take(cart),
                })
            }
            Phase::Ejecting { t, .. } => {
                if self.screen <= 0.0 {
                    let was = *t;
                    *t += dt;
                    touched = was < 0.0 && *t >= 0.0;
                }
                (*t >= EJECT_S).then_some(Phase::Shelf)
            }
            _ => None,
        };
        if touched {
            self.sfx = Some(match self.phase {
                Phase::Ejecting { .. } => Sfx::Eject,
                _ => Sfx::Insert,
            });
        }
        if let Some(phase) = next {
            let returned_to_shelf =
                matches!(self.phase, Phase::Ejecting { .. }) && matches!(phase, Phase::Shelf);
            self.phase = phase;
            if returned_to_shelf && std::mem::take(&mut self.fallback_refresh_after_eject) {
                // The cover is virtual: Labels/ is never touched, so the next
                // generation still sees label.is_none() and may pick the newest
                // state screenshot.
                self.invalidate_cart_faces();
            }
            self.refused_from = None;
            let seated = match &self.phase {
                Phase::Playing { cart } => Some(cart.clone()),
                _ => None,
            };
            if seated.is_none() {
                self.link_loaded = None;
            }
            self.record_cart(seated);
            // Following the host: the cart that has to go in is the peer's,
            // not the one that just came out.
            if returned_to_shelf {
                if let Some((stem, core)) = std::mem::take(&mut self.insert_after_eject) {
                    let platform = self.cart_platform(&stem).unwrap_or(self.platform);
                    if !self.seat_cart(&stem, platform, core) {
                        self.hud.toast(Toast::NoLink, self.now());
                    }
                }
            }
        }
        if self
            .sync_warmup
            .as_mut()
            .is_some_and(|warmup| warmup.finished())
        {
            self.sync_warmup = None;
        }
        let mut insert_sound = false;
        let mut insert_finished = false;
        if let Some(insert) = self.sync_insert.as_mut() {
            let elapsed = now.saturating_sub(insert.started) as f32 / 1000.0;
            if !insert.sounded && elapsed >= SYNC_SEAT_S {
                insert.sounded = true;
                insert_sound = true;
            }
            if elapsed >= SYNC_INSERT_S {
                insert_finished = true;
            }
        }
        if insert_sound {
            self.sfx = Some(Sfx::Insert);
        }
        if insert_finished {
            self.sync_insert = None;
        }
        self.try_launch_pending_sync(now);
        if self.sync_pending.is_some()
            && self.sync_insert.is_none()
            && self.sync_warmup.is_some()
            && matches!(self.shelf_sync, ShelfSync::Idle)
        {
            if self.sync_open_at.is_none() {
                self.sync_open_at = Some(now);
            }
            self.shelf_sync = ShelfSync::Connecting { since: now };
        }
        self.step_screen(dt);
    }

    fn start_sync_after_opening(&mut self, request: SyncRequest, now: Millis) {
        self.sync_pending = Some(request);
        let Some((index, platform, from)) = self.selected_cart_rect() else {
            self.try_launch_pending_sync(now);
            return;
        };
        let Some(face) = self.shelf().face(index) else {
            self.try_launch_pending_sync(now);
            return;
        };
        let (w, h) = slot_ui::seated_box(platform);
        self.start_sync_card_insert((face, w, h), Some(from), now);
    }

    fn try_launch_pending_sync(&mut self, now: Millis) -> bool {
        if self.sync_warmup.is_some()
            || self.sync_insert.is_some()
            || matches!(self.shelf_sync, ShelfSync::Swapping { .. })
        {
            return false;
        }
        let Some(request) = self.sync_pending.take() else {
            return false;
        };
        self.launch_sync_request(request, now);
        self.sync_pop_at = None;
        true
    }

    fn step_screen(&mut self, dt: f32) {
        let lit = matches!(self.phase, Phase::Playing { .. } | Phase::Polaroids { .. });
        let step = if lit {
            dt / POWER_ON_S
        } else {
            -dt / POWER_OFF_S
        };
        self.screen = (self.screen + step).clamp(0.0, 1.0);
        let docked = self.power_menu.is_none()
            && matches!(
                self.phase,
                Phase::Shelf | Phase::Inserting { .. } | Phase::Ejecting { .. }
            );
        let sliding = self.power_menu.is_none()
            && matches!(
                self.phase,
                Phase::Inserting { .. }
                    | Phase::Playing { .. }
                    | Phase::Polaroids { .. }
                    | Phase::Ejecting { .. }
            );
        let reach = if sliding { dt / POWER_ON_S } else { 1.0 };
        self.lift = match docked {
            true => (self.lift - reach).max(0.0),
            false => (self.lift + reach).min(1.0),
        };
    }

    pub fn frame_split(&self) -> Option<f32> {
        match (&self.phase, self.power_menu) {
            (Phase::QuickMenu { .. }, None) => Some(slot_ui::QUICK_SPLIT),
            _ => None,
        }
    }

    pub fn frame_lift(&self) -> f32 {
        self.lift
    }

    pub fn screen_power(&self) -> f32 {
        self.screen
    }

    pub fn set_game_ready(&mut self, ready: bool) {
        self.game_ready = ready;
    }

    pub fn game_ready(&self) -> bool {
        self.game_ready
    }

    pub fn game_visible(&self) -> bool {
        self.game_ready && self.screen > 0.0
    }

    pub fn tick_ms(&mut self, now: Millis) {
        self.clock = self.clock.max(now as f64);
        self.timers();
    }

    fn poll_upgrade(&mut self) {
        let lang = self.language;
        let Some(ui) = self.upgrade_ui.as_ref() else {
            return;
        };
        if !matches!(ui.state, UpgradeUiState::Applying | UpgradeUiState::Rebooting) {
            return;
        }
        let Some(root) = self.root.as_deref() else {
            return;
        };
        let status = crate::upgrade::read_status(root);
        if status.state == "failed" {
            let message = if status.message.is_empty() {
                slot_i18n::t(lang, "device.app.upgrade.write_failed").to_string()
            } else {
                status.message
            };
            if let Some(ui) = self.upgrade_ui.as_mut() {
                ui.state = UpgradeUiState::Failed(message);
            }
            return;
        }
        // The helper only stages files; the clean restart (stop web, unload
        // WiFi, settle saves, release logs, reboot) belongs to this process.
        if status.state == "rebooting" && !self.restarting && !self.powering_off {
            if let Some(ui) = self.upgrade_ui.as_mut() {
                ui.state = UpgradeUiState::Rebooting;
            }
            self.restarting = true;
            self.act_at = self.now() + SHUTDOWN_SHOW_MS;
            self.set_led(LedState::Off);
        }
    }

    fn timers(&mut self) {
        self.poll_follow_ask();
        self.play_hold();
        self.poll_about_hold();
        self.poll_wifi();
        self.poll_clock_sync();
        self.poll_sync_storage();
        if self.sync_ui.is_some() && self.now() >= self.sync_refresh_at {
            self.sync_refresh_at = self.now().saturating_add(1000);
            self.refresh_sync_storage_from_disk();
        }
        self.poll_upgrade();
        self.poll_web_start();
        self.poll_wifi_power();
        self.poll_boot_mods();
        if let Some(result) = self.power.as_mut().and_then(|power| power.take_repair_result()) {
            let said = match result {
                RepairResult::Repaired => Toast::RepairDone,
                RepairResult::Clean => Toast::RepairClean,
                RepairResult::Failed => Toast::RepairFailed,
            };
            self.hud.toast(said, self.now());
        }
        self.refresh_shelf_playtime();
        let offer = self.undo_label();
        if let Some(p) = &mut self.polaroids {
            p.set_undo(offer);
        }
        if self.doze_expired() {
            self.on_doze_timeout();
        }
        if self.now() >= self.autosave_at {
            self.autosave();
        }
        if self.now() >= self.card_check_at {
            self.card_check_at = self.now() + CARD_CHECK_MS;
            self.card_read_only = self.power.as_ref().is_some_and(|p| p.card_read_only());
        }
        let failing = self.card_failing.load(Ordering::Relaxed) || self.card_read_only;
        if failing != self.hud.alarmed() {
            eprintln!(
                "slot: card: alarm {} (read-only {})",
                if failing { "on" } else { "off" },
                self.card_read_only
            );
        }
        self.hud.set_alarm(failing, self.now());
        if self.now() >= self.battery_at {
            self.battery_at = self.now() + BATTERY_POLL_MS;
            self.battery = self.power.as_ref().and_then(|p| p.battery());
            if let Some(b) = self.battery {
                self.on_battery(b);
            }
        }
        if self.now() >= self.headphones_at {
            self.headphones_at = self.now() + HEADPHONES_POLL_MS;
            let on = self.power.as_ref().is_some_and(|p| p.headphones());
            if on != self.headphones {
                self.headphones = on;
                self.hud.set_headphones(on);
                let now = self.now();
                self.hud
                    .show(HudKind::Volume, self.output_volume(), self.muted(), now);
            }
        }
        if self.now() >= self.charge_at {
            self.charge_at = self.now() + CHARGE_POLL_MS;
            if let (Some(power), Some(b)) = (self.power.as_ref(), self.battery.as_mut()) {
                b.charge = power.charge();
            }
            let state = self.led_state();
            self.set_led(state);
        }
        if self.now() >= self.wifi_at {
            self.wifi_at = self.now() + WIFI_POLL_MS;
            let connected = crate::wifi::connected();
            if connected && !self.wifi_connected {
                self.begin_clock_sync();
            }
            self.wifi_connected = connected;
        }
        if let Some(GameMenu::Linked {
            since,
            opened: false,
            ..
        }) = self.game_menu
        {
            if self.now().saturating_sub(since) >= LINKED_HOLD_MS {
                self.game_menu = None;
            }
        }
        if let Some(GameMenu::Unplug { since, .. }) = self.game_menu {
            if self.now().saturating_sub(since) >= UNPLUG_HOLD_MS {
                self.game_menu = None;
            }
        }
        if let Some(at) = self.link.as_ref().and_then(|s| s.lost_at) {
            if self.link_client_id() != Some(0) && self.now().saturating_sub(at) >= LINK_LOST_MS {
                self.end_link();
            }
        }
    }

    pub fn led_state(&self) -> LedState {
        let Some(b) = self.battery else {
            return LedState::Running;
        };
        match b.charge {
            Charge::Charging => LedState::Charging,
            Charge::Full => LedState::Charged,
            _ if b.percent <= BATTERY_LOW => LedState::Low,
            _ => LedState::Running,
        }
    }

    fn set_led(&mut self, state: LedState) {
        if self.shutting_down() && state != LedState::Off {
            return;
        }
        if self.last_led == Some(state) {
            return;
        }
        self.last_led = Some(state);
        if let Some(power) = self.power.as_mut() {
            power.set_led(state);
        }
    }

    fn record_cart(&mut self, cart: Option<String>) {
        let platform = self.seated_cart().map(|c| c.platform);
        if self.state.cart == cart && self.state.cart_platform == platform {
            return;
        }
        self.state.cart = cart;
        self.state.cart_platform = platform;
        self.persist();
    }

    fn playtime_counts(&self) -> bool {
        self.state.cart.is_some()
            && self.screen > 0.0
            && !matches!(
                self.phase,
                Phase::Doze { .. } | Phase::Inserting { .. } | Phase::Ejecting { .. }
            )
            && !self.powering_off
            && !self.restarting
            && !self.shutting_down()
    }

    fn playtime_target(&self) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let stem = self.state.cart.as_deref()?;
        let platform = self.state.cart_platform.unwrap_or(self.platform);
        Some(
            root.join("States")
                .join(platform.dir_name())
                .join(self.core.as_str())
                .join(stem),
        )
    }

    fn accrue_playtime(&mut self, dt: f32) {
        if self.playtime_counts() {
            if self.playtime_dir.is_none() {
                self.playtime_dir = self.playtime_target();
            }
            let millis = (dt.max(0.0).min(1.0) * 1000.0) as u64;
            self.playtime_ms = self.playtime_ms.saturating_add(millis);
            if self.playtime_ms >= 30_000 {
                self.flush_playtime(false);
            }
        } else if self.playtime_ms > 0 {
            self.flush_playtime(true);
        } else {
            self.playtime_dir = None;
        }
    }

    /// Read the selected cartridge's merged playtime lazily. The key check
    /// keeps this off the FAT32 path until the selection/invalidation changes.
    fn refresh_shelf_playtime(&mut self) {
        if !matches!(self.phase, Phase::Shelf) {
            self.shelf_playtime_text.clear();
            self.shelf_playtime_key = None;
            return;
        }
        let Some(stem) = self.selected_stem().map(str::to_string) else {
            self.shelf_playtime_text.clear();
            self.shelf_playtime_key = None;
            return;
        };
        let Some(platform) = self.shelf().carts.get(self.shelf().index).map(|c| c.platform) else {
            self.shelf_playtime_text.clear();
            self.shelf_playtime_key = None;
            return;
        };
        let key = (stem.clone(), platform);
        if self.shelf_playtime_key.as_ref() == Some(&key) {
            return;
        }
        self.shelf_playtime_key = Some(key);
        let seconds = self
            .root
            .as_ref()
            .map(|root| {
                let core = slot_store::core_for_platform(root, &stem, platform);
                let unit_dir = root
                    .join("States")
                    .join(platform.dir_name())
                    .join(core.as_str())
                    .join(&stem);
                let playtime = crate::playtime::read(&unit_dir);
                playtime.total_seconds.max(playtime.seconds)
            })
            .unwrap_or(0);
        self.shelf_playtime_text = crate::playtime::format_duration(seconds)
            .map(|duration| {
                slot_i18n::t_args(
                    self.language,
                    "device.app.power.playtime",
                    &[("duration", duration.as_str())],
                )
            })
            .unwrap_or_default();
    }

    fn invalidate_shelf_playtime(&mut self) {
        self.shelf_playtime_key = None;
    }

    /// Write the accumulated seconds to `playtime.json`. `all` also drops the
    /// sub-second remainder (used when play stops); periodic flushes keep it.
    fn flush_playtime(&mut self, all: bool) {
        let secs = self.playtime_ms / 1000;
        if secs > 0 {
            if let Some(dir) = self.playtime_dir.clone() {
                if let Err(error) = crate::playtime::add(&dir, secs) {
                    eprintln!("slot: playtime: {error}");
                }
            }
        }
        if all {
            self.playtime_ms = 0;
            self.playtime_dir = None;
        } else {
            self.playtime_ms %= 1000;
        }
    }

    pub(crate) fn flush_playtime_now(&mut self) {
        self.flush_playtime(true);
        self.invalidate_shelf_playtime();
    }

    fn persist(&self) {
        let Some(root) = &self.root else {
            return;
        };
        self.state_writer.flush();
        let written = write_slot_state(root, &self.state);
        if let Err(e) = &written {
            eprintln!("slot: slot.state: {e}");
        }
        self.card_failing.store(written.is_err(), Ordering::Relaxed);
    }

    fn persist_soon(&self) {
        if let Some(root) = &self.root {
            self.state_writer.save(root, &self.state);
        }
    }

    pub fn flush_state(&self) {
        self.state_writer.flush();
    }

    pub fn on_core_ready(&mut self) {
        let Phase::Inserting {
            cart, core_ready, ..
        } = &mut self.phase
        else {
            return;
        };
        *core_ready = true;
        let cart = cart.clone();
        self.retire_refused_resume(&cart);
    }

    fn retire_refused_resume(&mut self, stem: &str) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if snapshot.resume_trusted() {
            return;
        }
        if !self.named_core {
            return;
        }
        let Some(root) = &self.root else {
            return;
        };
        if let Some(h) = self.pending_save.take() {
            let _ = h.join();
        }
        let ring = StateRing::new(root, self.platform, self.core, stem);
        match ring.retire_resume(&format_stamp(self.wall_secs())) {
            Ok(Some(to)) => eprintln!(
                "slot: resume: {} refused this state, moved it to {}",
                self.core.as_str(),
                to.display()
            ),
            Ok(None) => {}
            Err(e) => eprintln!("slot: resume: could not move the refused state aside: {e}"),
        }
    }

    pub fn on_core_failed(&mut self) {
        let caught = self.seat();
        let Phase::Inserting { cart, .. } = &mut self.phase else {
            return;
        };
        let cart = std::mem::take(cart);
        self.refuse_out(cart, caught);
    }

    fn refuse_out(&mut self, cart: String, caught: f32) {
        let t = (1.0 - caught) * EJECT_S;
        self.phase = Phase::Ejecting { cart, t };
        self.refused_from = Some(t);
    }

    pub fn refuse(&mut self) {
        self.refusal = Some(Refusal::started(self.now()));
    }

    pub fn refusal_active(&self, now: Millis) -> bool {
        self.refusal.is_some_and(|r| r.active(now))
    }

    pub fn seat(&self) -> f32 {
        match &self.phase {
            Phase::Shelf => 0.0,
            Phase::Inserting { t, resumed, .. } => {
                if *resumed {
                    1.0
                } else {
                    (t / SEATED_AT).clamp(0.0, 1.0)
                }
            }
            Phase::Ejecting { t, .. } => 1.0 - (t / EJECT_S).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    pub fn draw(&self, out: &mut Vec<Draw>) {
        if let Some(index) = self.power_menu {
            self.draw_power_menu(index, out);
            return;
        }
        if self.game_intro.is_some() {
            self.draw_intro_scene(out);
            return;
        }
        if self.shutting_down() {
            out.push(Draw::Rect {
                x: 0.0,
                y: 0.0,
                w: OUT_W as f32,
                h: OUT_H as f32,
                colour: [0.0, 0.0, 0.0, 1.0],
            });
            let which = if self.repairing {
                PowerChoice::Repair
            } else if self.restarting {
                PowerChoice::Restart
            } else {
                PowerChoice::PowerOff
            };
            if let Some((tex, w, h)) = self.shutdown_faces.get(which.index()).copied() {
                out.push(Draw::Tex {
                    x: ((OUT_W - w) / 2) as f32,
                    y: ((OUT_H - h) / 2) as f32,
                    w: w as f32,
                    h: h as f32,
                    tex,
                    alpha: 1.0,
                });
            }
            return;
        }
        match &self.phase {
            Phase::SetClock {
                picker, from_menu, ..
            } => {
                let (line, hint) = match self.clock_faces {
                    Some((line, hint)) => (Some(line), Some(hint)),
                    None => (None, None),
                };
                let back = self
                    .quick_menu_faces
                    .as_ref()
                    .filter(|_| *from_menu)
                    .map(|f| f.legend[0]);
                picker.draw(line, hint, back, self.language, out);
                return;
            }
            Phase::QuickMenu { row } => QuickMenu {
                row: *row,
                values: QuickRow::ALL.map(|r| self.quick_value(r)),
                clock: self.quick_clock_faces,
                fc_mask_value: self.fc_mask_value,
                gb_mask_value: self.gb_mask_value,
                gbc_mask_value: self.gbc_mask_value,
                sfc_mask_value: self.sfc_mask_value,
                faces: self.quick_menu_faces.as_ref(),
            }
            .draw(out),
            Phase::Shelf => {
                let accent = self.shelves[self.shelf_at].0.accent();
                let kind = self.shelves[self.shelf_at].0.collection_kind();
                let plate = self.collection_slot_back(kind);
                draw_backdrop(
                    self.wallpaper,
                    self.shelf_backdrop,
                    accent,
                    kind,
                    self.collection_overlay(kind),
                    out,
                );
                match (self.core_picker_shown(), self.selected_stem()) {
                    (Some(picker), Some(stem)) => {
                        let open = ease(picker.openness(self.now()));
                        let dim = 1.0 + (CORE_PICKER_DIM - 1.0) * open;
                        self.shelf()
                            .draw_row(Some(stem), 0.0, CORE_PICKER_RECEDE * open, dim, out);
                        draw_empty_slot_with(plate, out);
                    }
                    _ if self.transfer_seated => {
                        // A clean, slightly dim ground keeps the card, stream
                        // and percentage readable over a busy shelf.
                        out.push(Draw::Rect {
                            x: 0.0,
                            y: 0.0,
                            w: OUT_W as f32,
                            h: OUT_H as f32,
                            colour: [0.015, 0.025, 0.045, 0.62],
                        });
                        let seat = self.transfer_card_seat().unwrap_or(1.0);
                        // `incoming` identifies the receiving side even if a
                        // state transition happens before/after `received` is
                        // set.  Once the received cart has been reloaded on
                        // Done, use the real shelf card again.
                        let preview = self.transfer.as_ref().is_some_and(|transfer| {
                            transfer.incoming.is_some()
                                && (transfer.stage != TransferStage::Done
                                    || self.transfer_done_pending)
                        });
                        if preview {
                            // Always use the offer's title and shell while the
                            // receiver is still receiving.  A same-named local
                            // cart must not be mistaken for the incoming card.
                            self.draw_transfer_preview(seat, out);
                        } else if let Some(stem) = self
                            .transfer_insert_stem
                            .as_deref()
                            .filter(|stem| self.shelf().find(stem).is_some())
                        {
                            self.shelf().draw_row(Some(stem), 0.0, seat, 0.55, out);
                            self.chrome_at(stem, seat, seat, out);
                        } else {
                            self.draw_transfer_preview(seat, out);
                        }
                    }
                    _ => {
                        let swapping = (self.sync_hold_capability
                            && self.auto_cloud_card.is_some()
                            && matches!(
                                self.shelf_sync,
                                ShelfSync::Holding { .. } | ShelfSync::Unholding { .. }
                            ))
                            || (self.sync_swap_card.is_some()
                                && matches!(self.shelf_sync, ShelfSync::Swapping { .. }));
                        let opening = matches!(
                            self.shelf_sync,
                            ShelfSync::Holding { .. }
                                | ShelfSync::Opening { .. }
                                | ShelfSync::Unholding { .. }
                        );
                        let card_away =
                            self.sync_insert.is_some() || self.sync_card.is_some();
                        let hidden = if swapping || opening || card_away {
                            self.selected_stem()
                        } else {
                            None
                        };
                        self.shelf()
                            .draw_row(hidden, self.shelf_shake(), 0.0, 1.0, out);
                        if self.sync_insert.is_some() {
                            // Same sandwich as the normal launch: plastic back,
                            // nameplate, the incoming network cartridge, then
                            // the plastic front.  Without this the slot itself
                            // disappears during the sync insert.
                            slot_ui::draw_slot_back_layer(out);
                            slot_ui::draw_slot_plate(plate, 1.0, out);
                            self.draw_sync_insert(out);
                            slot_ui::draw_slot_front_layer(out);
                        } else {
                            draw_empty_slot_with(plate, out);
                        }
                        // A nameplate carries the rack's name itself (built-in
                        // plates and upstream's slot_back art both do); fall
                        // back to the printed name only when there is none.
                        if plate.is_none() {
                            draw_slot_name(self.shelf_platform, self.slot_name_alpha(), out);
                        }
                    }
                }
                // The sync overlay owns the top edge while it is active: draw
                // the translucent header first so its cloud icon is not cut in
                // half by the status band.
                StatusBar {
                    clock: self.shelf_clock,
                    battery: self.battery,
                    percent: self.battery_percent,
                    bolt: self.bolt,
                    wifi: self.wifi_face,
                    wifi_on: self.wifi_connected,
                }
                .draw(out);
                self.draw_shelf_sync(out);
                // Legend keys only work while the shelf owns the pad; overlays
                // draw their own hints.
                let covered = self.core_picker_shown().is_some()
                    || self.game_menu.is_some()
                    || self.utility.is_some()
                    || self.keyboard.is_some()
                    || self.search_results.is_some()
                    || self.sync_insert.is_some()
                    || !matches!(self.shelf_sync, ShelfSync::Idle);
                if !covered {
                    self.draw_shelf_playtime(out);
                    draw_shelf_legend(&self.shelf_legend.0, &self.shelf_legend.1, out);
                }
            }
            Phase::About => {
                draw_about(self.about_face, out);
                self.hud.draw(self.now(), out);
                return;
            }
            Phase::Inserting { cart, resumed, .. } => {
                if !resumed {
                    let accent = self.shelves[self.shelf_at].0.accent();
                    let kind = self.shelves[self.shelf_at].0.collection_kind();
                    draw_backdrop(
                        self.wallpaper,
                        self.shelf_backdrop,
                        accent,
                        kind,
                        self.collection_overlay(kind),
                        out,
                    );
                    self.shelf()
                        .draw_row(Some(cart), 0.0, self.seat(), 1.0, out);
                }
                self.chrome(cart, self.seat(), out);
            }
            Phase::Ejecting { cart, .. } => {
                let accent = self.shelves[self.shelf_at].0.accent();
                let kind = self.shelves[self.shelf_at].0.collection_kind();
                draw_backdrop(
                    self.wallpaper,
                    self.shelf_backdrop,
                    accent,
                    kind,
                    self.collection_overlay(kind),
                    out,
                );
                self.shelf()
                    .draw_row(Some(cart), 0.0, self.seat(), 1.0, out);
                self.chrome(cart, self.seat(), out);
            }
            Phase::Playing { cart } if self.screen < 1.0 => self.chrome(cart, 0.0, out),
            Phase::Playing { .. } => self.push_game(out),
            Phase::Polaroids { .. } => {
                self.push_game(out);
                if let Some(p) = &self.polaroids {
                    p.draw(
                        self.battery,
                        self.battery_percent,
                        self.bolt,
                        self.shelf_clock,
                        out,
                    );
                }
            }
            Phase::Doze { .. } => {
                out.push(Draw::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: OUT_W as f32,
                    h: OUT_H as f32,
                    colour: [0.0, 0.0, 0.0, 1.0],
                });
                return;
            }
        }
        if let Some(picker) = self.core_picker_shown() {
            self.draw_core_picker(&picker, out);
        }
        if self.front_deck.is_some() {
            self.draw_front_deck(out);
        }
        if let Some(menu) = self.game_menu {
            self.draw_game_menu(menu, out);
        }
        self.draw_cart_transfer(out);
        self.hud.draw(self.now(), out);
        if self.link_popup_open() {
            if let Some((tex, _, _)) = self.link_confirm_face {
                out.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: OUT_W as f32,
                    h: OUT_H as f32,
                    tex,
                    alpha: 1.0,
                });
            }
        }
    }

    pub fn screen_shake(&self) -> f32 {
        self.shake_at(self.now())
    }

    pub fn shake_at(&self, now: Millis) -> f32 {
        self.shake_when(matches!(self.phase, Phase::Playing { .. }), now)
    }

    pub fn shelf_shake(&self) -> f32 {
        self.shake_when(self.on_shelf() && self.core_picker.is_none(), self.now())
    }

    fn shake_when(&self, mine: bool, now: Millis) -> f32 {
        if !mine {
            return 0.0;
        }
        self.refusal.map_or(0.0, |r| r.offset(now))
    }

    fn push_game(&self, out: &mut Vec<Draw>) {
        if self.game_visible() {
            out.push(Draw::Game);
        }
        // The selected machine's mask sits directly on the picture: menus and
        // toasts are pushed later, so they stay readable on top of it.
        if self.game_visible() && self.mask_wanted() {
            if let Some(tex) = self.fc_mask_face {
                out.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: OUT_W as f32,
                    h: OUT_H as f32,
                    tex,
                    alpha: 1.0,
                });
            }
        }
    }

    fn chrome(&self, stem: &str, dim: f32, out: &mut Vec<Draw>) {
        self.chrome_at(stem, dim, self.seat(), out);
    }

    fn chrome_at(&self, stem: &str, dim: f32, seat: f32, out: &mut Vec<Draw>) {
        let Some((cart, face)) = self.shelf().find(stem) else {
            return;
        };
        let alpha = self.alert_alpha();
        let (rest, scale) = self.shelf().selected_at();
        let kind = self.shelves[self.shelf_at].0.collection_kind();
        SlotChrome {
            cart,
            face,
            rest,
            scale,
            seat,
            alert: self.alert_face.filter(|_| alpha > 0.0).map(|t| (t, alpha)),
            dim,
            screen: self.screen,
            game: self.game_ready,
            plate: self.collection_slot_back(kind),
        }
        .draw(out);
    }

    pub fn alert_visible(&self) -> bool {
        self.alert_alpha() > 0.0
    }

    pub fn alert_alpha(&self) -> f32 {
        let (Some(from), Phase::Ejecting { t, .. }) = (self.refused_from, &self.phase) else {
            return 0.0;
        };
        let span = EJECT_S - from;
        if span <= 0.0 {
            return 0.0;
        }
        let u = ((t - from) / span).clamp(0.0, 1.0);
        ((ALERT_GONE - u) / (ALERT_GONE - ALERT_HOLD)).clamp(0.0, 1.0)
    }

    pub fn set_alert_face(&mut self, face: TexId) {
        self.alert_face = Some(face);
    }

    pub fn set_shutdown_faces(&mut self, faces: Vec<(TexId, u32, u32)>) {
        self.shutdown_faces = faces;
    }

    pub fn set_power_menu_faces(
        &mut self,
        cards: [(TexId, u32, u32); 2],
        labels: Vec<[(TexId, u32, u32); 2]>,
        icons: Vec<[(TexId, u32, u32); 2]>,
        title: (TexId, u32, u32),
    ) {
        self.power_cards = Some(cards);
        self.power_labels = labels;
        self.power_icons = icons;
        self.power_title = Some(title);
    }

    pub fn set_core_board_faces(&mut self, board: TexId, lid: TexId) {
        self.core_board_face = Some(board);
        self.core_lid_face = Some(lid);
        self.core_faces_stem = self.selected_stem().map(str::to_string);
    }

    pub fn set_network_face(&mut self, face: TexId) {
        self.network_face = Some(face);
    }

    pub fn set_network_face_off(&mut self, face: TexId) {
        self.network_face_off = Some(face);
    }

    pub fn set_sync_picker_face(&mut self, face: TexId) {
        self.sync_picker_face = Some(face);
        self.sync_picker_prev = None;
    }

    pub fn set_sync_picker_face_lit(&mut self, face: TexId) {
        self.sync_picker_face_lit = Some(face);
    }

    /// Drop any in-flight swap so reopening the picker starts from a still
    /// chip instead of sliding in from a stale face.
    pub fn clear_sync_picker_swap(&mut self) {
        self.sync_picker_prev = None;
        self.sync_picker_swap = None;
    }

    /// Publish the new picker face together with the one it replaces so the
    /// chip can slide instead of snapping.
    pub fn set_sync_picker_faces(
        &mut self,
        face: TexId,
        previous: Option<TexId>,
        lit: Option<TexId>,
    ) {
        let changed = self.sync_picker_face != Some(face);
        self.sync_picker_face = Some(face);
        if lit.is_some() {
            self.sync_picker_face_lit = lit;
        }
        if changed && previous.is_some() {
            self.sync_picker_prev = previous;
            self.sync_picker_swap = Some(self.now());
        }
    }

    /// True while the result screen is painted from black.  The eject path
    /// leaves this off so the shelf never flashes.
    pub fn sync_result_fade_active(&self) -> bool {
        self.sync_result_at.is_some()
    }

    pub fn sync_picker_open(&self) -> bool {
        self.sync_picker.is_some()
    }

    pub fn sync_highlight_open(&self) -> bool {
        matches!(
            self.shelf_sync,
            ShelfSync::Holding { .. }
                | ShelfSync::Unholding { .. }
                | ShelfSync::Opening { .. }
        )
    }

    /// Name shown on the GBA cloud-sync module.
    pub fn network_label(&self) -> String {
        let lang = self.language;
        self.sync_mods
            .get(self.sync_mod_index)
            .map(|choice| choice.name.clone())
            .unwrap_or_else(|| slot_i18n::t(lang, "device.app.shelf.cloud").to_string())
    }

    /// Optional chip cover shipped by the selected sync mod.
    pub fn sync_cover_path(&self) -> Option<PathBuf> {
        let choice = self.sync_mods.get(self.sync_mod_index)?;
        if choice.cloud {
            return None;
        }
        let root = self.root.as_ref()?;
        let path = root.join("linkSlot/mods").join(&choice.mod_id).join("sync-chip.png");
        path.is_file().then_some(path)
    }

    fn open_sync_picker(&mut self) {
        if self.sync_picker.is_some() || self.sync_mods.is_empty() {
            return;
        }
        self.sync_picker = Some(SyncPicker {
            original: self.sync_mod_index,
            since: self.now(),
        });
    }

    fn sync_picker_input(&mut self, action: Action) {
        match action {
            Action::GbaDown(Btn::Up) => self.step_sync_mod(-1),
            Action::GbaDown(Btn::Down) => self.step_sync_mod(1),
            Action::GbaDown(Btn::A) => self.sync_picker = None,
            Action::GbaDown(Btn::B) => {
                if let Some(picker) = self.sync_picker.take() {
                    self.sync_mod_index = picker.original;
                }
            }
            _ => {}
        }
    }

    fn selected_sync_pick(&self) -> Option<SyncPick> {
        let choice = self.sync_mods.get(self.sync_mod_index)?;
        if choice.cloud {
            return None;
        }
        Some(SyncPick::Mod {
            mod_id: choice.mod_id.clone(),
            action_id: choice.action_id.clone(),
            ui: choice.ui.clone(),
        })
    }

    pub fn set_core_part_faces(
        &mut self,
        sockets: Vec<TexId>,
        chips: Vec<Vec<TexId>>,
        blanks: Vec<TexId>,
        shadow: TexId,
    ) {
        self.core_socket_faces = sockets;
        self.core_chip_faces = chips;
        self.core_blank_chip_faces = blanks;
        self.core_chip_shadow_face = Some(shadow);
    }

    pub fn set_core_legend_faces(&mut self, faces: Vec<(TexId, u32)>) {
        self.core_legend_faces = faces;
    }

    pub fn set_link_menu_faces(&mut self, faces: Vec<(TexId, u32, u32)>) {
        self.link_menu_faces = faces;
    }

    pub fn set_link_linked_face(&mut self, face: (TexId, u32, u32)) {
        self.link_linked_face = Some(face);
    }

    pub fn set_link_dialog_faces(&mut self, confirm: (TexId, u32, u32)) {
        self.link_confirm_face = Some(confirm);
    }

    pub fn link_popup_open(&self) -> bool {
        self.link_popup.is_some()
    }

    /// Show the non-blocking notice.  A second one replaces the first: two
    /// stacked cards under the bar would be harder to read than either alone.
    pub fn say_notice(&mut self, title: impl Into<String>, detail: impl Into<String>) {
        self.notice = Some((title.into(), detail.into(), self.now()));
    }

    /// `(title, detail, shown_at)` while the notice is still on screen.
    pub fn notice_view(&self) -> Option<(&str, &str, Millis)> {
        let (title, detail, shown) = self.notice.as_ref()?;
        if self.now().saturating_sub(*shown) >= slot_ui::notice::NOTICE_MS {
            return None;
        }
        Some((title.as_str(), detail.as_str(), *shown))
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn set_link_step_faces(&mut self, faces: Vec<(TexId, u32, u32)>) {
        self.link_step_faces = faces;
    }

    pub fn set_link_fail_faces(&mut self, faces: Vec<(TexId, u32, u32)>) {
        self.link_fail_faces = faces;
    }

    pub fn set_link_legend_faces(&mut self, faces: Vec<(TexId, u32)>) {
        self.link_legend_faces = faces;
    }

    pub fn set_link_sprites(&mut self, sprites: LinkSprites) {
        self.link_sprites = Some(sprites);
    }

    pub fn link_sprites_ready(&self) -> bool {
        self.link_sprites.is_some()
    }

    /// The shelf behind the intro: the flip starts in place, so the rack must
    /// still be on screen (the selected cart is hidden and drawn separately by
    /// the animation).
    fn draw_intro_scene(&self, out: &mut Vec<Draw>) {
        let accent = self.shelves[self.shelf_at].0.accent();
        let kind = self.shelves[self.shelf_at].0.collection_kind();
        let plate = self.collection_slot_back(kind);
        draw_backdrop(
            self.wallpaper,
            self.shelf_backdrop,
            accent,
            kind,
            self.collection_overlay(kind),
            out,
        );
        let Some(intro) = &self.game_intro else {
            return;
        };
        let now = self.now();
        let progress = |elapsed: Millis| (elapsed as f32 / INTRO_OPEN_MS as f32).min(1.0);
        let p = if let Some(cancel) = intro.cancel {
            let at_cancel = progress(cancel.saturating_sub(intro.started));
            at_cancel
                * (1.0
                    - ease((now.saturating_sub(cancel) as f32 / INTRO_CANCEL_MS as f32).min(1.0)))
        } else if let Some(since) = intro.closing {
            // Close from wherever the opening got to, not from a full card:
            // pressing B mid-flip must not snap the card open first.
            let at_close = progress(since.saturating_sub(intro.started));
            at_close
                * (1.0 - ease((now.saturating_sub(since) as f32 / INTRO_CLOSE_MS as f32).min(1.0)))
        } else {
            progress(now.saturating_sub(intro.started))
        };
        // One timeline from the press: lift (150 ms) → flip (420) → pause on
        // the back label (110) → pull the page out (320).  The hold bar fills
        // across the same 1000 ms, so the card and the bar finish together.
        let ant_end = INTRO_ANTICIPATION_MS as f32 / INTRO_OPEN_MS as f32;
        let flip_end = (INTRO_ANTICIPATION_MS + INTRO_FLIP_MS) as f32 / INTRO_OPEN_MS as f32;
        let pause_end =
            (INTRO_ANTICIPATION_MS + INTRO_FLIP_MS + INTRO_BACK_MS) as f32 / INTRO_OPEN_MS as f32;
        let pull_end = (INTRO_ANTICIPATION_MS + INTRO_FLIP_MS + INTRO_BACK_MS + INTRO_PULL_MS)
            as f32
            / INTRO_OPEN_MS as f32;
        let tongue_end = (INTRO_ANTICIPATION_MS
            + INTRO_FLIP_MS
            + INTRO_BACK_MS
            + INTRO_PULL_MS
            + INTRO_TONGUE_HOLD_MS) as f32
            / INTRO_OPEN_MS as f32;
        let hidden = self.selected_stem();
        self.shelf()
            .draw_row(hidden.as_deref(), 0.0, 0.0, 1.0, out);
        draw_empty_slot_with(plate, out);
        let cart = self
            .selected_cart_rect()
            .map(|(_, _, rect)| rect)
            .unwrap_or((
                (OUT_W as f32 - INTRO_PLATE_W) / 2.0,
                (OUT_H as f32 - INTRO_PLATE_H) / 2.0,
                1.0,
                1.0,
            ));
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.0, 0.0, 0.0, 0.72 * ease((p / 0.62).min(1.0))],
        });
        // The bar runs while the preview is still on its way; once the beat is
        // committed it goes away and the card owns the screen.
        if !intro.armed && intro.cancel.is_none() && intro.closing.is_none() {
            let accent = self.shelves[self.shelf_at].0.accent();
            let bar_w = 150.0;
            let bar_h = 6.0;
            let bx = cart.0 + (cart.2 - bar_w) / 2.0;
            let by = cart.1 + cart.3 + 10.0;
            let fade = (p * 8.0).min(1.0);
            out.push(Draw::Rect {
                x: bx,
                y: by,
                w: bar_w,
                h: bar_h,
                colour: [0.02, 0.03, 0.05, 0.55 * fade],
            });
            out.push(Draw::Rect {
                x: bx,
                y: by,
                w: bar_w * p,
                h: bar_h,
                colour: [
                    accent[0] as f32 / 255.0,
                    accent[1] as f32 / 255.0,
                    accent[2] as f32 / 255.0,
                    fade,
                ],
            });
        }
        if p < ant_end {
            // Still frame: the press is acknowledged by the bar and the dim,
            // then the cartridge turns.  No lift or tilt, so a quick tap does
            // not look like the card wobbling.
            self.draw_intro_flip(intro, 0.0, cart, out);
            return;
        }
        if p < flip_end {
            let t = (p - ant_end) / (flip_end - ant_end);
            self.draw_intro_flip(intro, t, cart, out);
            return;
        }
        if p < pause_end {
            // The back label is fully turned but has not grown yet.
            self.draw_intro_flip(intro, 1.0, cart, out);
            return;
        }
        // The manual slides out from behind the cartridge as a narrow tongue,
        // then unfolds upward from its own top edge.  Paper first, cartridge
        // second: the shell hides the part that is still inside.
        let tongue = intro_tongue_rect(intro.platform, cart);
        if p < pull_end {
            let t = ((p - pause_end) / (pull_end - pause_end)).min(1.0);
            let pull = 1.0 - (1.0 - t) * (1.0 - t);
            self.draw_intro_flip(intro, 1.0, cart, out);
            draw_paper_shadow_on_cart(tongue, pull, out);
            draw_intro_tongue(tongue, pull, out);
            return;
        }
        self.draw_intro_flip(intro, 1.0, cart, out);
        draw_paper_shadow_on_cart(tongue, 1.0, out);
        draw_intro_tongue(tongue, 1.0, out);
        if p < tongue_end {
            return;
        }
        let k = ease(((p - tongue_end) / (1.0 - tongue_end)).min(1.0));
        if intro.faces.is_some() {
            let mut card = Vec::new();
            self.push_intro_card(intro, &mut card);
            out.extend(unfold_intro_draws(card, tongue, k));
        }
    }

    /// Squeeze the cartridge around its vertical axis: front for the first half
    /// of the beat, the paper back label for the second.
    fn draw_intro_flip(
        &self,
        intro: &GameIntro,
        t: f32,
        cart: (f32, f32, f32, f32),
        out: &mut Vec<Draw>,
    ) {
        let (x, y, w, h) = cart;
        // Constant angular speed reads as a real turn; the middle of the beat
        // holds briefly edge-on so the player sees which way it is going.
        let angle = std::f32::consts::PI * flip_turn(t);
        let scale_x = angle.cos().abs().max(0.02);
        let width = (w * scale_x).max(1.0);
        let at = x + (w - width) / 2.0;
        if t < 0.5 {
            if let Some(tex) = self.shelf().face(self.shelf().index) {
                out.push(Draw::Tex {
                    x: at,
                    y,
                    w: width,
                    h,
                    tex,
                    alpha: 1.0,
                });
            }
        } else if let Some((tex, _, _)) = intro.back {
            out.push(Draw::Tex {
                x: at,
                y,
                w: width,
                h,
                tex,
                alpha: 1.0,
            });
        } else {
            out.push(Draw::Rect {
                x: at,
                y,
                w: width,
                h,
                colour: [
                    INTRO_PAPER[0] as f32 / 255.0,
                    INTRO_PAPER[1] as f32 / 255.0,
                    INTRO_PAPER[2] as f32 / 255.0,
                    1.0,
                ],
            });
        }
    }

    /// The card contents, without the dim veil, so the zoom can transform them.
    fn push_intro_card(&self, intro: &GameIntro, out: &mut Vec<Draw>) {
        let Some(faces) = &intro.faces else {
            return;
        };
        let plate_x = (OUT_W as f32 - INTRO_PLATE_W) / 2.0;
        let plate_y = (OUT_H as f32 - INTRO_PLATE_H) / 2.0;
        let (plate, pw, ph) = faces.plate;
        out.push(Draw::Tex {
            x: plate_x,
            y: plate_y,
            w: pw as f32,
            h: ph as f32,
            tex: plate,
            alpha: 1.0,
        });
        // Title bar: the game name printed in ink, then a hairline rule.
        let (title_tex, title_w, title_h) = faces.title;
        out.push(Draw::Tex {
            x: plate_x + 32.0,
            y: plate_y + 14.0,
            w: title_w as f32,
            h: title_h as f32,
            tex: title_tex,
            alpha: 1.0,
        });
        out.push(Draw::Rect {
            x: plate_x + 32.0,
            y: plate_y + 56.0,
            w: INTRO_PLATE_W - 64.0,
            h: 2.0,
            colour: [
                INTRO_INK[0] as f32 / 255.0,
                INTRO_INK[1] as f32 / 255.0,
                INTRO_INK[2] as f32 / 255.0,
                0.22,
            ],
        });
        let top = plate_y + 72.0;
        let mut body_x = plate_x + 32.0;
        let mut has_media = false;
        if let Some(video) = &faces.video {
            let total: u64 = video.frames.iter().map(|(_, ms)| *ms as u64).sum();
            if total > 0 && video.width > 0 && video.height > 0 {
                let at = self.now().saturating_sub(intro.started) % total;
                let mut acc = 0u64;
                let mut frame = video.frames[0].0;
                for (tex, ms) in &video.frames {
                    acc += *ms as u64;
                    if at < acc {
                        frame = *tex;
                        break;
                    }
                }
                let scale = (260.0 / video.width as f32)
                    .min(260.0 / video.height as f32);
                let w = video.width as f32 * scale;
                let h = video.height as f32 * scale;
                let x = plate_x + 28.0 + (260.0 - w) / 2.0;
                let y = top + (260.0 - h) / 2.0;
                push_media_frame(out, x, y, w, h);
                out.push(Draw::Tex {
                    x,
                    y,
                    w,
                    h,
                    tex: frame,
                    alpha: 1.0,
                });
                body_x = plate_x + 312.0;
                has_media = true;
            }
        }
        if !has_media {
            if let Some((cover, cw, ch)) = faces.cover {
                let x = plate_x + 28.0;
                push_media_frame(out, x, top, cw as f32, ch as f32);
                out.push(Draw::Tex {
                    x,
                    y: top,
                    w: cw as f32,
                    h: ch as f32,
                    tex: cover,
                    alpha: 1.0,
                });
                body_x = x + cw as f32 + 24.0;
            }
        }
        let (body, bw, bh) = faces.body;
        if bw > 0 && bh > 0 {
            out.push(Draw::Tex {
                x: body_x,
                y: top,
                w: bw as f32,
                h: bh as f32,
                tex: body,
                alpha: 1.0,
            });
        }
        if intro.armed && intro.max_scroll > 0 {
            let track_x = plate_x + INTRO_PLATE_W - 16.0;
            let track_y = top;
            let track_h = 270.0;
            out.push(Draw::Rect {
                x: track_x,
                y: track_y,
                w: 3.0,
                h: track_h,
                colour: [
                    INTRO_INK[0] as f32 / 255.0,
                    INTRO_INK[1] as f32 / 255.0,
                    INTRO_INK[2] as f32 / 255.0,
                    0.28,
                ],
            });
            let thumb_h = (track_h / (intro.max_scroll as f32 + 1.0)).max(24.0);
            let progress = intro.scroll as f32 / intro.max_scroll as f32;
            let thumb_y = track_y + (track_h - thumb_h) * progress;
            out.push(Draw::Rect {
                x: track_x - 1.0,
                y: thumb_y,
                w: 5.0,
                h: thumb_h,
                colour: [
                    INTRO_INK[0] as f32 / 255.0,
                    INTRO_INK[1] as f32 / 255.0,
                    INTRO_INK[2] as f32 / 255.0,
                    0.80,
                ],
            });
        }
        if !intro.armed {
            return;
        }
        let hints = [faces.hint, faces.hint_delete];
        let total_w: f32 = hints.iter().map(|(_, w, _)| *w as f32 + 14.0).sum();
        let mut hint_x = plate_x + (INTRO_PLATE_W - total_w) / 2.0;
        for (tex, w, h) in hints {
            if w == 0 || h == 0 {
                continue;
            }
            out.push(Draw::Tex {
                x: hint_x,
                y: plate_y + INTRO_PLATE_H - 42.0,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 1.0,
            });
            hint_x += w as f32 + 14.0;
        }
    }

    /// The long-press Y chooser: a shelf-style row of network cards
    /// modules over the dimmed shelf. Left/right rolls the row; a two-choice
    /// row stops at its edges and bumps the dialog toward the rejected side.
        /// The chosen cloud card travelling from the picker into the shelf's slot.
    /// It is drawn before the slot front so the plastic swallows it at the end.
    fn cloud_insert_travel(seat: f32) -> f32 {
        const CATCH_IN: f32 = 0.42;
        const CATCH_OUT: f32 = 0.62;
        const CATCH: f32 = 0.82;
        const CREEP: f32 = 0.03;
        let seat = seat.clamp(0.0, 1.0);
        if seat < CATCH_IN {
            CATCH * ease(seat / CATCH_IN)
        } else if seat < CATCH_OUT {
            CATCH + CREEP * (seat - CATCH_IN) / (CATCH_OUT - CATCH_IN)
        } else {
            let caught = CATCH + CREEP;
            caught + (1.0 - caught) * ease((seat - CATCH_OUT) / (1.0 - CATCH_OUT))
        }
    }

    /// The network cartridge leaves the slot after the sync screen has
    /// collapsed. It travels back to its original shelf position, so the
    /// hidden shelf card can reappear exactly where it lands.
    fn draw_sync_eject(&self, started: Millis, out: &mut Vec<Draw>) {
        let Some((tex, cw, ch)) = self.sync_card else {
            return;
        };
        if cw == 0 || ch == 0 {
            return;
        }
        let Some((_, _, (x0, y0, w0, h0))) = self.selected_cart_rect() else {
            return;
        };
        let elapsed = self.now().saturating_sub(started) as f32 / 1000.0;
        let t = (elapsed / EJECT_S).clamp(0.0, 1.0);
        let e = ease(t);
        let (sw, sh) = slot_ui::seated_box(self.platform);
        let (x1, y1) = (
            (OUT_W as f32 - sw as f32) / 2.0,
            OUT_H as f32 - slot_ui::MOUTH_H + 6.0,
        );
        let x = x1 + (x0 - x1) * e;
        let y = y1 + (y0 - y1) * e;
        let w = sw as f32 + (w0 - sw as f32) * e;
        let h = sh as f32 + (h0 - sh as f32) * e;

        // The veil dims the shelf behind the slot; it has to leave with the
        // card, otherwise the frame after the eject jumps back to full
        // brightness and reads as a flash.
        let veil = (1.0 - slot_ui::ease(((t - 0.55) / 0.45).clamp(0.0, 1.0))) * 0.28;
        if veil > 0.001 {
            out.push(Draw::Rect {
                x: 0.0,
                y: 0.0,
                w: OUT_W as f32,
                h: OUT_H as f32,
                colour: [0.0, 0.0, 0.0, veil],
            });
        }
        let kind = self.shelves[self.shelf_at].0.collection_kind();
        slot_ui::draw_slot_back_layer(out);
        slot_ui::draw_slot_plate(self.collection_slot_back(kind), 1.0, out);
        out.push(Draw::Tex {
            x,
            y,
            w,
            h,
            tex,
            alpha: 1.0,
        });
        slot_ui::draw_slot_front_layer(out);
    }

    fn draw_sync_insert(&self, out: &mut Vec<Draw>) {
        let Some(insert) = &self.sync_insert else {
            return;
        };
        if insert.w == 0 || insert.h == 0 {
            return;
        }
        let elapsed = self.now().saturating_sub(insert.started) as f32 / 1000.0;
        let seat_t = (elapsed / SYNC_SEAT_S).clamp(0.0, 1.0);
        let e = Self::cloud_insert_travel(seat_t);
        let (x0, y0, w0, h0) = if let Some(from) = insert.from {
            from
        } else {
            let aspect = insert.h as f32 / insert.w as f32;
            let h0 = 196.0 - 18.0;
            let w0 = h0 / aspect;
            let x0 = (OUT_W as f32 - w0) / 2.0;
            let y0 = 70.0 + 92.0 + (196.0 - h0) / 2.0;
            (x0, y0, w0, h0)
        };
        let (seated_w, seated_h) = slot_ui::seated_box(self.platform);
        let fit = (seated_w as f32 / w0).min(seated_h as f32 / h0);
        let w1 = w0 * fit;
        let h1 = h0 * fit;
        let x1 = (OUT_W as f32 - w1) / 2.0;
        let y1 = OUT_H as f32 - slot_ui::MOUTH_H + 8.0;
        let (x, y) = (x0 + (x1 - x0) * e, y0 + (y1 - y0) * e);
        let (w, h) = (w0 + (w1 - w0) * e, h0 + (h1 - h0) * e);
        if let Some(shadow) = insert.shadow {
            out.push(Draw::Tex {
                x: x + (w - w * 0.96) / 2.0,
                y: y + 8.0,
                w: w * 0.96,
                h: h * 0.96,
                tex: shadow,
                alpha: 0.45,
            });
        }
        out.push(Draw::Tex {
            x,
            y,
            w,
            h,
            tex: insert.tex,
            alpha: 1.0,
        });
    }

    fn draw_power_menu(&self, index: usize, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: switch_colour(slot_ui::switch_style::bg()),
        });
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: 96.0,
            colour: switch_colour(slot_ui::switch_style::bg_top()),
        });
        out.push(Draw::Rect {
            x: 48.0,
            y: 22.0,
            w: 5.0,
            h: 30.0,
            colour: switch_colour([
                slot_ui::switch_style::accent()[0],
                slot_ui::switch_style::accent()[1],
                slot_ui::switch_style::accent()[2],
                255,
            ]),
        });
        if let Some((tex, w, h)) = self.power_title {
            out.push(Draw::Tex {
                x: 64.0,
                y: 18.0,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 1.0,
            });
        }
        let Some(cards) = self.power_cards else {
            return;
        };
        let (_, card_w, card_h) = cards[0];
        let card_x = (OUT_W as f32 - card_w as f32) / 2.0;
        for choice in PowerChoice::ALL {
            let selected = choice.index() == index;
            let (tex, w, h) = cards[selected as usize];
            let y = 176.0 + 68.0 * choice.index() as f32;
            out.push(Draw::Tex {
                x: card_x,
                y,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 1.0,
            });
            if let Some(&(icon, iw, ih)) = self.power_icons.get(choice.index()).map(|f| &f[selected as usize]) {
                out.push(Draw::Tex {
                    x: card_x + 24.0,
                    y: y + (card_h as f32 - ih as f32) / 2.0,
                    w: iw as f32,
                    h: ih as f32,
                    tex: icon,
                    alpha: 1.0,
                });
            }
            if let Some(&(label, lw, lh)) = self.power_labels.get(choice.index()).map(|f| &f[selected as usize]) {
                out.push(Draw::Tex {
                    x: card_x + 80.0,
                    y: y + (card_h as f32 - lh as f32) / 2.0,
                    w: lw as f32,
                    h: lh as f32,
                    tex: label,
                    alpha: 1.0,
                });
            }
        }
    }

    fn core_picker_shown(&self) -> Option<&CorePicker> {
        self.core_picker.as_ref().filter(|p| !p.waiting())
    }

    fn draw_front_deck(&self, out: &mut Vec<Draw>) {
        let Some(deck) = self.front_deck.as_ref() else {
            return;
        };
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.0, 0.0, 0.0, 190.0 / 255.0],
        });
        let accent = slot_ui::switch_style::accent();
        let accent = [
            accent[0] as f32 / 255.0,
            accent[1] as f32 / 255.0,
            accent[2] as f32 / 255.0,
        ];
        if let Some((tex, w, h)) = self.front_deck_title {
            out.push(Draw::Rect {
                x: 48.0,
                y: 22.0,
                w: 5.0,
                h: 34.0,
                colour: [accent[0], accent[1], accent[2], 1.0],
            });
            out.push(Draw::Tex {
                x: 64.0,
                y: 16.0,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 1.0,
            });
        }
        if let Some((tex, w, h)) = self.front_deck_counter {
            out.push(Draw::Tex {
                x: OUT_W as f32 - 48.0 - w as f32,
                y: 24.0,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 0.9,
            });
        }

        let count = deck.count();
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|a, b| {
            let da = signed_deck_offset(*a, deck.scroll, count).abs();
            let db = signed_deck_offset(*b, deck.scroll, count).abs();
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Keep every cartridge within the same stage box.  Native face sizes
        // differ (GBA is wide, GB is tall), and using them directly made the
        // tall machines collide with the footer.
        let stage_w = 276.0;
        let stage_h = 228.0;
        let mut selected_rect: Option<(f32, f32, f32, f32)> = None;
        for index in order {
            let Some((tex, w, h)) = self.front_deck_texs.get(index).copied() else {
                continue;
            };
            if w == 0 || h == 0 {
                continue;
            }
            let offset = signed_deck_offset(index, deck.scroll, count);
            let distance = offset.abs().min(3.0);
            let selected = index == deck.selected;
            let fit = (stage_w / w as f32).min(stage_h / h as f32);
            let scale = fit * (1.0 - 0.20 * distance);
            let alpha = 1.0 - 0.15 * distance;
            let card_w = w as f32 * scale;
            let card_h = h as f32 * scale;
            let step = if count >= 6 { 128.0 } else { 148.0 };
            let x = OUT_W as f32 / 2.0 + offset * step - card_w / 2.0;
            let y = 220.0 - card_h / 2.0 + distance * 18.0;
            if selected {
                selected_rect = Some((x, y, card_w, card_h));
            }
            out.push(Draw::Tex {
                x,
                y,
                w: card_w,
                h: card_h,
                tex,
                alpha,
            });
        }

        // The frontmost cartridge is, by definition, the selection: no extra
        // frame or underline is needed.  Its name and position counter carry
        // the information instead.
        if let Some((_, y, _, h)) = selected_rect {
            if let Some((tex, w, nh)) = self.front_deck_names.get(deck.selected).copied() {
                let name_y = (y + h + 10.0).min(OUT_H as f32 - 112.0);
                out.push(Draw::Tex {
                    x: (OUT_W as f32 - w as f32) / 2.0,
                    y: name_y,
                    w: w as f32,
                    h: nh as f32,
                    tex,
                    alpha: 1.0,
                });
            }
            // A calm page indicator sits between the name and the controls.
            let dot_y = OUT_H as f32 - 100.0;
            let dot_width = |active: bool| if active { 22.0 } else { 6.0 };
            let dots_total = (0..count).map(|index| dot_width(index == deck.selected)).sum::<f32>()
                + 6.0 * count.saturating_sub(1) as f32;
            let mut dot_x = (OUT_W as f32 - dots_total) / 2.0;
            for index in 0..count {
                let active = index == deck.selected;
                let width = dot_width(active);
                out.push(Draw::Rect {
                    x: dot_x,
                    y: dot_y,
                    w: width,
                    h: 6.0,
                    colour: if active {
                        [accent[0], accent[1], accent[2], 1.0]
                    } else {
                        [0.36, 0.36, 0.40, 1.0]
                    },
                });
                dot_x += width + 6.0;
            }
        }

        // The controls are the visual anchor at the bottom, like the shelf
        // legend, but with larger keycaps and more breathing room.
        let bar_y = OUT_H as f32 - 62.0;
        out.push(Draw::Rect {
            x: 40.0,
            y: bar_y,
            w: OUT_W as f32 - 80.0,
            h: 50.0,
            colour: [0.02, 0.02, 0.03, 0.72],
        });
        out.push(Draw::Rect {
            x: 40.0,
            y: bar_y,
            w: OUT_W as f32 - 80.0,
            h: 1.0,
            colour: [1.0, 1.0, 1.0, 0.08],
        });
        if !self.front_deck_controls.is_empty() {
            let scale = 1.45;
            let gap = 14.0;
            let total: f32 = self
                .front_deck_controls
                .iter()
                .map(|(_, w)| w.saturating_sub(4) as f32 * scale)
                .sum::<f32>()
                + gap * self.front_deck_controls.len().saturating_sub(1) as f32;
            let mut x = (OUT_W as f32 - total) / 2.0;
            let y = bar_y + (50.0 - 24.0 * scale) / 2.0;
            for (tex, w) in &self.front_deck_controls {
                let w = w.saturating_sub(4) as f32 * scale;
                out.push(Draw::Tex {
                    x: x.round(),
                    y,
                    w,
                    h: 24.0 * scale,
                    tex: *tex,
                    alpha: 0.95,
                });
                x += w + gap;
            }
        }
    }

    fn draw_core_picker(&self, picker: &CorePicker, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: switch_colour(slot_ui::switch_style::bg()),
        });
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: 96.0,
            colour: switch_colour(slot_ui::switch_style::bg_top()),
        });
        out.push(Draw::Rect {
            x: 48.0,
            y: 22.0,
            w: 5.0,
            h: 30.0,
            colour: switch_colour([
                slot_ui::switch_style::accent()[0],
                slot_ui::switch_style::accent()[1],
                slot_ui::switch_style::accent()[2],
                255,
            ]),
        });
        let platform = self
            .shelf()
            .carts
            .get(self.shelf().index)
            .map(|cart| cart.platform)
            .unwrap_or(Platform::Gba);
        let options = picker.options();
        let single = options.len() <= 1;
        let now = self.now();
        let progress = picker.openness(now);
        let lift = lift_of(progress);
        let (rest, scale) = self.shelf().selected_at();
        let shelf = self
            .selected_cart_rect()
            .map(|(_, _, (x, y, w, h))| Placed { x, y, w, h })
            .unwrap_or_else(|| shelf_cart_at(rest, scale));
        let board = board_from_for(platform, shelf, progress);
        let zoom = board_zoom(board);
        let core_zoom = zoom * slot_ui::core_chip_scale(platform);
        let ready = self.core_faces_ready();

        if ready {
            if let Some(tex) = self.core_board_face {
                out.push(Draw::Tex {
                    x: board.x,
                    y: board.y,
                    w: board.w,
                    h: board.h,
                    tex,
                    alpha: 1.0,
                });
            }
            if let Some(tex) = self.network_face {
                let (u, v) = slot_ui::network_spot(platform);
                let (x, y) = on_board_for(platform, board, u, v);
                let network_zoom =
                    board.w / slot_ui::board_size(platform).0 as f32;
                out.push(Draw::Tex {
                    x: x.round(),
                    y: y.round(),
                    w: slot_ui::NETWORK_W as f32 * network_zoom,
                    h: slot_ui::NETWORK_H as f32 * network_zoom,
                    tex,
                    alpha: 1.0,
                });
            }
            let single_spot = if single {
                Some(slot_ui::single_core_spot(platform))
            } else {
                None
            };
            for (position, core) in options.iter().enumerate() {
                let Some(tex) = self.core_socket_faces.get(core.index()).copied() else {
                    continue;
                };
                let (socket_u, socket_v) = match slot_ui::multi_core_slot(platform, position) {
                    Some((u, v, _, _)) => (u, v),
                    None => match single_spot {
                        Some((u, v, _, _)) => (u, v),
                        None => (SOCKET_U.get(position).copied().unwrap_or(SOCKET_U[0]), SOCKET_V),
                    },
                };
                let (x, y) = on_board_for(platform, board, socket_u, socket_v);
                out.push(Draw::Tex {
                    x: x.round(),
                    y: y.round(),
                    w: SOCKET_W as f32 * core_zoom,
                    h: SOCKET_H as f32 * core_zoom,
                    tex,
                    alpha: 1.0,
                });
            }

            let chip = picker.chip(now);
            let (u, chip_v) = match single_spot {
                Some((_, _, u, v)) => (u, v),
                None => slot_ui::chip_along(platform, chip.across).unwrap_or((
                    CHIP_U[0] + (CHIP_U[1] - CHIP_U[0]) * chip.across,
                    CHIP_V,
                )),
            };
            if chip.lift > 0.0 {
                if let Some(tex) = self.core_chip_shadow_face {
                    let (cx, cy) = on_board_for(platform, board, u + 19.0 * slot_ui::core_chip_scale(platform), chip_v + 29.4 * slot_ui::core_chip_scale(platform));
                    let (w, h) = (SHADOW_W as f32 * core_zoom, SHADOW_H as f32 * core_zoom);
                    out.push(Draw::Tex {
                        x: cx - w / 2.0,
                        y: cy - h / 2.0,
                        w,
                        h,
                        tex,
                        alpha: 0.6 * chip.lift * lift,
                    });
                }
            }
            let face = match chip.seated {
                Some(core) => self
                    .core_chip_faces
                    .get(platform.index())
                    .and_then(|row| row.get(core.index()))
                    .copied(),
                None => self.core_blank_chip_faces.get(platform.index()).copied(),
            };
            if let Some(tex) = face {
                let (x, y) = on_board_for(platform, board, u, chip_v - HOP_LIFT * chip.lift);
                let body = Placed {
                    x: x + chip.shake,
                    y,
                    w: CHIP_W as f32 * core_zoom,
                    h: CHIP_H as f32 * core_zoom,
                };
                let at = grown(body, TURN_PAD as f32 * zoom);
                out.push(Draw::Turned {
                    x: at.x.round(),
                    y: at.y.round(),
                    w: at.w,
                    h: at.h,
                    tex,
                    alpha: 1.0,
                    turn: chip.tip,
                });
            }
        }

        if let Some(tex) = self.core_chip_shadow_face {
            let (lid, _) = lid_from_for(platform, shelf, progress);
            let k = lid.w / lid_at(1.0).0.w;
            let (w, h) = (LID_SHADOW_W * k, LID_SHADOW_H * k);
            out.push(Draw::Tex {
                x: lid.x + (lid.w - w) / 2.0,
                y: lid.y + lid.h + LID_SHADOW_DROP * k - h / 2.0,
                w,
                h,
                tex,
                alpha: LID_SHADOW_ALPHA * lift,
            });
        }

        if ready {
            if let Some(tex) = self.core_lid_face {
                let (lid, turn) = lid_from_for(platform, shelf, progress);
                let at = grown(lid, TURN_PAD as f32 * lid.w / CART_W as f32);
                out.push(Draw::Turned {
                    x: at.x,
                    y: at.y,
                    w: at.w,
                    h: at.h,
                    tex,
                    alpha: 1.0,
                    turn,
                });
            }
        } else if let Some((_, Some(tex))) = self
            .selected_stem()
            .and_then(|stem| self.shelf().find(stem))
        {
            let (lid, turn) = lid_from_for(platform, shelf, progress);
            out.push(Draw::Turned {
                x: lid.x,
                y: lid.y,
                w: lid.w,
                h: lid.h,
                tex,
                alpha: 1.0,
                turn,
            });
        }

        let faces = &self.core_legend_faces;
        if faces.len() >= 3 {
            let cancel = faces[0];
            let swap = faces[1];
            let choose = faces[2];
            let right = BOARD_X + BOARD_W as f32;
            let seen = |w: u32| w.saturating_sub(HINT_EDGE) as f32;
            let mut hints = vec![(cancel.0, cancel.1, BOARD_X)];
            if !single {
                hints.push((swap.0, swap.1, (OUT_W as f32 - seen(swap.1)) / 2.0));
                hints.push((choose.0, choose.1, right - seen(choose.1)));
            }
            for (tex, w, x) in hints {
                out.push(Draw::Tex {
                    x: x.round(),
                    y: CORE_LEGEND_Y,
                    w: w as f32,
                    h: HINT_H as f32,
                    tex,
                    alpha: lift,
                });
            }
            // Y sync and X fronts live on their own row so the core controls
            // keep the board's corners.  Both are always present.
            let extras: Vec<(TexId, u32)> = faces.get(3..=4).unwrap_or(&[]).to_vec();
            if !extras.is_empty() {
                let total: f32 = extras.iter().map(|(_, w)| seen(*w)).sum::<f32>()
                    + 18.0 * extras.len().saturating_sub(1) as f32;
                let mut x = ((OUT_W as f32 - total) / 2.0).round();
                for (tex, w) in extras {
                    out.push(Draw::Tex {
                        x,
                        y: CORE_LEGEND_Y - 26.0,
                        w: w as f32,
                        h: HINT_H as f32,
                        tex,
                        alpha: lift,
                    });
                    x += seen(w) + 18.0;
                }
            }
        }
        if let Some(picker) = self.sync_picker {
            self.draw_sync_picker(picker, out);
        }
    }

    /// The Y-key foreground: the network chip grows to the centre and the
    /// player cycles its mod with up/down, confirming with A or cancelling
    /// with B.
    fn draw_sync_picker(&self, picker: SyncPicker, out: &mut Vec<Draw>) {
        let elapsed = self.now().saturating_sub(picker.since) as f32 / 1000.0;
        let lift = ease((elapsed / SYNC_PICKER_ZOOM_S).clamp(0.0, 1.0));
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.01, 0.02, 0.03, 0.78 * lift],
        });

        let target_w = (OUT_W as f32 - 180.0).min(360.0);
        let target_h = target_w * slot_ui::NETWORK_H as f32 / slot_ui::NETWORK_W as f32;
        let w = target_w * (0.72 + 0.28 * lift);
        let h = target_h * (0.72 + 0.28 * lift);
        let x = (OUT_W as f32 - w) / 2.0;
        let y = 188.0 - h / 2.0 + 42.0;
        // A vertical switch slides the outgoing chip out and the incoming one
        // in, both in the direction the user pressed.
        let (swap, dir) = match self.sync_picker_swap {
            Some(since) => (
                ((self.now().saturating_sub(since) as f32 / 1000.0) / SYNC_SWAP_S)
                    .clamp(0.0, 1.0),
                self.sync_picker_dir,
            ),
            None => (1.0, 1.0),
        };
        if swap < 1.0 {
            if let (Some(prev), Some(cur)) = (self.sync_picker_prev, self.sync_picker_face) {
                let give = ease(swap);
                let slide = 26.0 * (1.0 - give);
                out.push(Draw::Tex {
                    x: x.round(),
                    y: (y - dir * slide).round(),
                    w: (w * (1.0 - 0.05 * give)).round(),
                    h: (h * (1.0 - 0.05 * give)).round(),
                    tex: prev,
                    alpha: lift * (1.0 - give),
                });
                out.push(Draw::Tex {
                    x: x.round(),
                    y: (y + dir * slide * 0.6).round(),
                    w: w.round(),
                    h: h.round(),
                    tex: cur,
                    alpha: lift * give,
                });
            } else if let Some(tex) = self.sync_picker_face_lit.or(self.sync_picker_face).or(self.network_face) {
                out.push(Draw::Tex {
                    x: x.round(),
                    y: y.round(),
                    w: w.round(),
                    h: h.round(),
                    tex,
                    alpha: lift,
                });
            }
        } else if let Some(tex) = self.sync_picker_face_lit.or(self.sync_picker_face).or(self.network_face) {
            out.push(Draw::Tex {
                x: x.round(),
                y: y.round(),
                w: w.round(),
                h: h.round(),
                tex,
                alpha: lift,
            });
        }

        let faces = &self.core_legend_faces;
        if faces.len() >= 3 {
            let cancel = faces[0];
            let choose = faces[2];
            let vertical = faces.get(4).or_else(|| faces.get(1)).copied();
            let seen = |width: u32| width.saturating_sub(HINT_EDGE) as f32;
            out.push(Draw::Tex {
                x: 44.0,
                y: CORE_LEGEND_Y,
                w: cancel.1 as f32,
                h: HINT_H as f32,
                tex: cancel.0,
                alpha: lift,
            });
            if let Some((tex, width)) = vertical {
                out.push(Draw::Tex {
                    x: ((OUT_W as f32 - seen(width)) / 2.0).round(),
                    y: CORE_LEGEND_Y,
                    w: width as f32,
                    h: HINT_H as f32,
                    tex,
                    alpha: lift,
                });
            }
            out.push(Draw::Tex {
                x: (OUT_W as f32 - seen(choose.1) - 44.0).round(),
                y: CORE_LEGEND_Y,
                w: choose.1 as f32,
                h: HINT_H as f32,
                tex: choose.0,
                alpha: lift,
            });
        }
    }

    fn draw_game_menu(&self, menu: GameMenu, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: switch_colour(slot_ui::switch_style::bg()),
        });
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: 96.0,
            colour: switch_colour(slot_ui::switch_style::bg_top()),
        });
        out.push(Draw::Rect {
            x: 48.0,
            y: 22.0,
            w: 5.0,
            h: 30.0,
            colour: switch_colour([
                slot_ui::switch_style::accent()[0],
                slot_ui::switch_style::accent()[1],
                slot_ui::switch_style::accent()[2],
                255,
            ]),
        });
        if let Some(sprites) = &self.link_sprites {
            crate::link_screen::draw_link_art(menu, self.link_hardware, self.now(), sprites, out);
        }
        let line = match menu {
            GameMenu::Pick(role) => self.link_menu_faces.get(role.index()).copied(),
            GameMenu::Working { step, .. } => self
                .link_step_faces
                .get(step.shown(self.radio.warmed()).index())
                .copied(),
            GameMenu::Linked { .. } => self.link_linked_face,
            GameMenu::Failed { fail, .. } => fail
                .shown()
                .and_then(|i| self.link_fail_faces.get(i).copied()),
            GameMenu::Unplug { .. } => None,
        };
        if let Some((tex, w, h)) = line {
            out.push(Draw::Tex {
                x: ((OUT_W as f32 - w as f32) / 2.0).round(),
                y: LINK_TEXT_Y,
                w: w as f32,
                h: h as f32,
                tex,
                alpha: 1.0,
            });
        }
        let switchable = self.seated().is_some_and(|stem| self.link_switchable(stem));
        let keys: &[LinkLegend] = match menu {
            GameMenu::Pick(_) if switchable => &[
                LinkLegend::Cancel,
                LinkLegend::Mode,
                LinkLegend::Swap,
                LinkLegend::Link,
            ],
            GameMenu::Pick(_) => &[LinkLegend::Cancel, LinkLegend::Swap, LinkLegend::Link],
            GameMenu::Working { .. } => &[LinkLegend::Cancel],
            GameMenu::Linked { opened: true, .. } => &[LinkLegend::Back, LinkLegend::EndLink],
            GameMenu::Linked { .. } => &[],
            GameMenu::Failed { .. } => &[LinkLegend::Ok],
            GameMenu::Unplug { .. } => &[],
        };
        let faces: Vec<(TexId, u32)> = keys
            .iter()
            .filter_map(|k| self.link_legend_faces.get(k.index()).copied())
            .collect();
        let seen = |w: u32| w.saturating_sub(HINT_EDGE) as f32;
        let total: f32 = faces.iter().map(|(_, w)| seen(*w)).sum::<f32>()
            + LINK_LEGEND_GAP * faces.len().saturating_sub(1) as f32;
        let mut x = ((OUT_W as f32 - total) / 2.0).round();
        for (tex, w) in faces {
            out.push(Draw::Tex {
                x,
                y: LINK_LEGEND_Y,
                w: w as f32,
                h: HINT_H as f32,
                tex,
                alpha: 1.0,
            });
            x += (seen(w) + LINK_LEGEND_GAP).round();
        }
    }

    fn auto_link(&self, stem: &str) -> Option<(&Cart, LinkKind)> {
        let cart = self.shelf().carts.iter().find(|c| c.stem == stem)?;
        let auto = link_kind(&cart.code, &cart.title, slot_store::header_clean(&cart.rom));
        Some((cart, auto))
    }

    fn on_shelf(&self) -> bool {
        matches!(self.phase, Phase::Shelf)
    }

    pub fn browsed_rom(&self) -> Option<&std::path::Path> {
        if !self.on_shelf() {
            return None;
        }
        let shelf = self.shelf();
        shelf.carts.get(shelf.index).map(|c| c.rom.as_path())
    }

    fn insert(&mut self, force_clean: bool) {
        if !self.on_shelf() {
            return;
        }
        if self.web.as_ref().is_some_and(|web| web.busy()) {
            self.refuse();
            return;
        }
        self.utility=None; self.cheat_key.clear(); self.cheat_rows.clear(); self.cheat_pending=None;
        let at = self.shelf().index;
        let Some(cart) = self.shelf_mut().carts.get_mut(at).map(|c| {
            c.read_header();
            c.stem.clone()
        }) else {
            return;
        };
        eprintln!(
            "slot: launch: insert {cart} at {:.1} ms",
            crate::boot_time::ms()
        );
        self.play_held = None;
        self.refusal = None;
        self.refused_from = None;
        let platform = self.shelf().carts[at].platform;
        // A long A press means "start from the beginning" and overrides the
        // per-platform switch; a tap / boot insert follows the switch.
        let clean = force_clean || !self.state.resume(platform, ResumeKind::Load);
        self.remember_shelf();
        self.remember_played(platform, &cart);
        self.phase = Phase::Inserting {
            cart,
            t: 0.0,
            core_ready: false,
            resumed: false,
            clean,
        };
    }

    pub fn starting_clean(&self) -> bool {
        self.restart_clean || matches!(self.phase, Phase::Inserting { clean: true, .. })
    }

    pub fn on_core_restarted(&mut self) {
        self.restart_clean = false;
    }

    pub fn on_restart_failed(&mut self) {
        self.restart_clean = false;
    }

    fn play_hold(&mut self) {
        let Some(at) = self.play_held else {
            return;
        };
        // A long Y press owns the pad: never launch a cart under the overlay.
        if !self.on_shelf() || !matches!(self.shelf_sync, ShelfSync::Idle) {
            self.play_held = None;
            return;
        }
        if self.now().saturating_sub(at) >= PLAY_HOLD_MS {
            self.insert(true);
        }
    }

    fn eject(&mut self) {
        self.eject_inner(false);
    }

    /// 「重新开始游戏」: reset the running core in place, without ejecting or
    /// re-inserting the cartridge and without leaving the game page.
    fn restart_game(&mut self) {
        let Some(cart) = self.seated().map(str::to_string) else {
            return;
        };
        if !matches!(self.phase, Phase::Playing { .. }) {
            return;
        }
        crate::link_log::debug("restart the seated cart in place");
        self.utility = None;
        self.close_game_menu();
        // Persist the run exactly as an exit would, then let the session drop
        // the old core.  The phase stays Playing, so the shelf/insert animation
        // never runs; the fresh core simply takes over under the same page.
        self.flush_eject(&cart);
        self.flush_playtime_now();
        self.pending = None;
        self.refusal = None;
        self.refused_from = None;
        self.restart_clean = true;
        self.restart_pending = true;
    }

    /// One-shot: the session takes it when it drops the core of a restart.
    pub fn take_restart_pending(&mut self) -> bool {
        std::mem::take(&mut self.restart_pending)
    }

    fn eject_inner(&mut self, allow_single: bool) {
        if self.single_cart() && !allow_single {
            return self.refuse();
        }
        self.flush_playtime_now();
        self.cancel_shelf_sync();
        let cart = match &mut self.phase {
            Phase::Playing { cart } | Phase::Inserting { cart, .. } => std::mem::take(cart),
            _ => return,
        };
        self.utility=None; self.cheat_key.clear(); self.cheat_rows.clear(); self.cheat_pending=None;
        // A manual eject cancels a pending in-place restart before it can leak
        // its clean flag into the next cartridge.
        self.restart_clean = false;
        self.restart_pending = false;
        // Remember that this cart had no real cover before it leaves the screen.
        // The actual face rebuild waits for the eject animation to finish so the
        // current cart does not turn blank during the animation.
        self.fallback_refresh_after_eject = self.state.fallback_cover
            && self
                .shelf()
                .carts
                .iter()
                .find(|candidate| candidate.stem == cart)
                .is_some_and(|candidate| candidate.label.is_none());
        // A host room lives with the current game.  A follow switch is the one
        // eject that keeps it: the peer is deliberately moving this side into
        // the host's next cart.
        if self.link_active() && self.insert_after_eject.is_none() {
            self.end_link();
        }
        self.close_game_menu();
        self.flush_eject(&cart);
        self.pending = None;
        self.refusal = None;
        self.refused_from = None;
        self.phase = Phase::Ejecting {
            cart,
            t: -EJECT_HOLD_S,
        };
    }

    /// Drop the resume state when this platform opted out of automatic
    /// saving. The battery save is never filtered here.
    fn resume_wanted(&self, state: Option<Vec<u8>>) -> Option<Vec<u8>> {
        state.filter(|_| self.state.resume(self.platform, ResumeKind::Save))
    }
    /// A resume thumbnail is only useful when this ROM has no real cover.
    /// The resume state itself is still written when a cover exists.
    fn resume_thumb_wanted(&self, stem: &str) -> bool {
        self.shelf()
            .carts
            .iter()
            .find(|cart| cart.stem == stem)
            .is_some_and(|cart| cart.label.is_none())
    }

    fn flush_eject(&mut self, stem: &str) {
        self.settle_saves();
        let (Some(root), Some(snapshot)) = (&self.root, &self.snapshot) else {
            return;
        };
        let Some(state) = snapshot.state() else {
            eprintln!("slot: eject: the core gave up no state");
            return;
        };
        let (state, sav) = trusted_write(snapshot.as_ref(), state, "eject");
        let state = self.resume_wanted(state);
        let thumb = self.resume_thumb_wanted(stem).then(|| snapshot.thumb()).flatten();
        match persist::eject_with_thumb(
            root,
            self.platform,
            self.core,
            stem,
            state.as_deref(),
            sav.as_deref(),
            thumb.as_deref(),
        ) {
            Ok(()) => {
                self.state.cart = None;
                self.state.cart_platform = None;
                self.card_failing.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                eprintln!("slot: eject: {e}");
                self.card_failing.store(true, Ordering::Relaxed);
            }
        }
    }

    fn doze(&mut self) {
        self.flush_playtime_now();
        self.utility = None;
        self.stop_web();
        self.cancel_shelf_sync();
        if self.link_active() {
            self.end_link();
        }
        self.close_game_menu();
        self.core_picker = None;
        if matches!(self.phase, Phase::Doze { .. }) {
            return;
        }
        self.state_writer.flush();
        self.flush_resume();
        let cart = match &mut self.phase {
            Phase::Playing { cart } | Phase::Polaroids { cart } => Some(std::mem::take(cart)),
            _ => None,
        };
        self.polaroids = None;
        self.phase = Phase::Doze { cart };
        self.dozed_at = self.now();
        self.radio.ask(RadioJob::Cool);
        if let Some(power) = &mut self.power {
            power.on_close();
        }
    }

    fn wake(&mut self) {
        let Phase::Doze { cart } = &mut self.phase else {
            return;
        };
        self.phase = match cart.take() {
            Some(cart) => Phase::Playing { cart },
            None => Phase::Shelf,
        };
        if let Some(power) = &mut self.power {
            power.on_open();
        }
    }

    pub fn on_doze_timeout(&mut self) {
        if !matches!(self.phase, Phase::Doze { .. }) {
            return;
        }
        self.begin_power_off();
    }

    fn power_press(&mut self) {
        if std::mem::take(&mut self.woke_on_press) {
            return;
        }
        match self.phase {
            Phase::Doze { .. } => self.wake(),
            _ => self.doze(),
        }
    }

    fn open_power_menu(&mut self) {
        if self.power_menu.is_some() {
            return;
        }
        if self.link_active() {
            return self.refuse();
        }
        self.cancel_shelf_sync();
        self.flush_resume();
        self.power_menu = Some(0);
    }

    fn power_menu_input(&mut self, action: Action) {
        let Some(index) = self.power_menu else {
            return;
        };
        let last = PowerChoice::ALL.len() - 1;
        match action {
            Action::GbaDown(Btn::Up) => {
                self.power_menu = Some(if index == 0 { last } else { index - 1 })
            }
            Action::GbaDown(Btn::Down) => {
                self.power_menu = Some(if index >= last { 0 } else { index + 1 })
            }
            Action::GbaDown(Btn::B) => self.power_menu = None,
            Action::GbaDown(Btn::A) => {
                self.power_menu = None;
                self.close_game_menu();
                match PowerChoice::ALL[index] {
                    PowerChoice::Restart => {
                        self.restarting = true;
                        self.act_at = self.now() + SHUTDOWN_SHOW_MS;
                        self.set_led(LedState::Off);
                    }
                    PowerChoice::PowerOff => self.begin_power_off(),
                    PowerChoice::Repair => self.begin_repair(),
                }
            }
            _ => {}
        }
    }

    fn refresh_sync_mods(&mut self) {
        let lang = self.language;
        let Some(root) = self.root.clone() else {
            self.sync_mods = vec![cloud_sync_choice(lang)];
            self.sync_mod_index = 0;
            return;
        };
        let current = self.sync_mods.get(self.sync_mod_index).cloned();
        let mut choices = vec![cloud_sync_choice(lang)];
        if let Ok(rows) = slot_mods::list(&root) {
            for row in rows {
                if !row.enabled || row.error.is_some() {
                    continue;
                }
                let Ok(detail) = slot_mods::read_mod(&root, &row.id) else {
                    continue;
                };
                let action = detail.actions.iter().find(|action| {
                    action.enabled
                        && action.timing == Some(slot_mods::Timing::Manual)
                        && action
                            .ui
                            .as_ref()
                            .is_some_and(|ui| ui.animation == "cloud_sync")
                });
                if let Some(action) = action {
                    choices.push(SyncModChoice {
                        cloud: false,
                        name: short_sync_name(&row.id, &row.name),
                        mod_id: row.id,
                        action_id: action.id.clone(),
                        ui: action.ui.clone(),
                    });
                }
            }
        }
        self.sync_mods = choices;
        if let Some(current) = current {
            if let Some(index) = self
                .sync_mods
                .iter()
                .position(|choice| choice.name == current.name)
            {
                self.sync_mod_index = index;
            } else {
                self.sync_mod_index = 0;
            }
        } else {
            self.sync_mod_index = 0;
        }
    }

    fn step_sync_mod(&mut self, delta: isize) {
        if self.sync_mods.len() < 2 {
            return;
        }
        let len = self.sync_mods.len() as isize;
        let next = (self.sync_mod_index as isize + delta).rem_euclid(len);
        self.sync_mod_index = next as usize;
        self.sync_picker_dir = if delta < 0 { -1.0 } else { 1.0 };
    }

    fn open_front_deck(&mut self) {
        if self.front_deck.is_some() {
            return;
        }
        let (Some(root), Some(cart)) = (
            self.root.clone(),
            self.shelf().carts.get(self.shelf().index).cloned(),
        ) else {
            return;
        };
        let manifest = slot_store::fronts::load(&root, cart.platform, &cart.stem);
        let faces: Vec<_> = manifest
            .map(|manifest| {
                manifest
                    .faces
                    .into_iter()
                    .filter(|face| slot_store::fronts::compatible(&face.model, cart.model))
                    .collect()
            })
            .unwrap_or_default();
        let selected = slot_store::cart_meta::read(&root, cart.platform, &cart.rom, &cart.stem)
            .and_then(|meta| slot_store::fronts::selected_id(&meta).map(str::to_string))
            .and_then(|id| faces.iter().position(|face| face.id == id).map(|i| i + 1))
            .unwrap_or(0);
        self.front_deck = Some(FrontDeck {
            root,
            cart,
            faces,
            selected,
            scroll: selected as f32,
            velocity: 0.0,
            held: None,
        });
    }

    fn close_front_deck(&mut self) {
        self.front_deck = None;
        self.front_deck_texs.clear();
        self.front_deck_title = None;
        self.front_deck_controls.clear();
        self.front_deck_names.clear();
        self.front_deck_counter = None;
    }

    fn front_deck_input(&mut self, action: Action) {
        let now = self.now();
        let Some(deck) = &mut self.front_deck else {
            return;
        };
        match action {
            Action::GbaDown(Btn::Left) | Action::ShelfLeft => {
                deck.step(-1);
                deck.held = Some((-1, now + 400));
            }
            Action::GbaDown(Btn::Right) | Action::ShelfRight => {
                deck.step(1);
                deck.held = Some((1, now + 400));
            }
            Action::GbaUp(Btn::Left) | Action::GbaUp(Btn::Right) => {
                deck.held = None;
            }
            Action::GbaDown(Btn::A) => self.apply_front_deck(),
            Action::GbaDown(Btn::B) | Action::GbaDown(Btn::X) => self.close_front_deck(),
            _ => {}
        }
    }

    fn apply_front_deck(&mut self) {
        let Some(deck) = &self.front_deck else {
            return;
        };
        let cart = deck.cart.clone();
        let is_default = deck.selected == 0;
        let face = if is_default {
            None
        } else {
            deck.faces.get(deck.selected - 1)
        };
        if !is_default && face.is_none() {
            self.close_front_deck();
            return;
        }
        let face_path = face.map(|face| {
            slot_store::fronts::face_dir(&deck.root, deck.cart.platform, &deck.cart.stem)
                .join(&face.file)
        });
        let id = face.map(|face| face.id.clone());
        if slot_store::cart_meta::update(&deck.root, &cart, |meta| match &id {
            Some(id) => {
                meta.extra
                    .insert("front_face".to_string(), serde_json::Value::String(id.clone()));
            }
            None => {
                meta.extra.remove("front_face");
            }
        })
        .is_ok()
        {
            if let Some(selected) = self.shelves[self.shelf_at]
                .1
                .carts
                .iter_mut()
                .find(|item| item.rom == cart.rom)
            {
                selected.front = face_path;
            }
            self.invalidate_cart_faces();
        }
        self.close_front_deck();
    }

    fn update_front_deck(&mut self) {
        let now = self.now();
        let Some(deck) = &mut self.front_deck else {
            return;
        };
        if let Some((direction, next)) = deck.held {
            if now >= next {
                deck.step(direction);
                deck.held = Some((direction, now + 85));
            }
        }
        let target = deck.selected as f32;
        let dt = 1.0 / 60.0;
        let spring = 16.0;
        let damping = 2.0 * spring;
        let accel = (target - deck.scroll) * spring * spring - damping * deck.velocity;
        deck.velocity += accel * dt;
        deck.scroll += deck.velocity * dt;
        if (target - deck.scroll).abs() < 0.001 && deck.velocity.abs() < 0.01 {
            deck.scroll = target;
            deck.velocity = 0.0;
        }
    }

    /// Test/diagnostic view of the front carousel: (option count, selected).
    pub fn front_deck_status(&self) -> Option<(usize, usize)> {
        self.front_deck
            .as_ref()
            .map(|deck| (deck.count(), deck.selected))
    }

    /// Display names for the front deck, default shell first.
    pub(crate) fn front_deck_names(&self) -> Option<Vec<String>> {
        let deck = self.front_deck.as_ref()?;
        let mut names = Vec::with_capacity(deck.count());
        names.push(
            slot_i18n::t(self.language, "device.app.front_deck.default").to_string(),
        );
        names.extend(deck.faces.iter().map(|face| face.name.clone()));
        Some(names)
    }

    /// Cart clones for the frontend face builder.  The first entry is always
    /// the untouched default shell; the rest carry one custom variant each.
    pub(crate) fn front_deck_carts(&self) -> Option<Vec<Cart>> {
        let deck = self.front_deck.as_ref()?;
        let dir = slot_store::fronts::face_dir(&deck.root, deck.cart.platform, &deck.cart.stem);
        let mut carts = Vec::with_capacity(deck.count());
        let mut default = deck.cart.clone();
        default.front = None;
        carts.push(default);
        carts.extend(deck.faces.iter().map(|face| {
            let mut cart = deck.cart.clone();
            cart.front = Some(dir.join(&face.file));
            cart
        }));
        Some(carts)
    }

    pub(crate) fn set_front_deck_faces(&mut self, faces: Vec<(TexId, u32, u32)>) {
        self.front_deck_texs = faces;
    }

    pub(crate) fn set_front_deck_texts(
        &mut self,
        title: Option<(TexId, u32, u32)>,
        controls: Vec<(TexId, u32)>,
        names: Vec<(TexId, u32, u32)>,
        counter: Option<(TexId, u32, u32)>,
    ) {
        self.front_deck_title = title;
        self.front_deck_controls = controls;
        self.front_deck_names = names;
        self.front_deck_counter = counter;
    }

    fn open_core_picker(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(cart) = self.shelf().carts.get(self.shelf().index).cloned() else {
            return;
        };
        self.sync_picker = None;
        // Opening the PCB is the one moment where a disabled/uninstalled mod
        // is removed from the chip and the default cloud route is restored.
        self.refresh_sync_mods();
        let options = slot_store::available_options(&root, cart.platform);
        let now = self.now();
        let seat = slot_store::core_for_platform(&root, &cart.stem, cart.platform);
        let mut picker = CorePicker::open(seat, options, now);
        if self.core_faces_ready() {
            picker.start(now);
        }
        self.core_picker = Some(picker);
        self.shelf_mut().release_hold();
        self.play_held = None;
    }

    pub fn core_faces_ready(&self) -> bool {
        self.core_faces_stem
            .as_deref()
            .is_some_and(|stem| self.selected_stem() == Some(stem))
    }

    fn core_picker_input(&mut self, action: Action) {
        if self.front_deck.is_some() {
            self.front_deck_input(action);
            return;
        }
        if self.sync_picker.is_some() {
            self.sync_picker_input(action);
            return;
        }
        if action == Action::GbaDown(Btn::X) {
            self.open_front_deck();
            return;
        }
        if action == Action::GbaDown(Btn::Y) {
            self.open_sync_picker();
            return;
        }
        let press = match action {
            Action::GbaDown(Btn::Left) | Action::ShelfLeft => Press::Left,
            Action::GbaDown(Btn::Right) | Action::ShelfRight => Press::Right,
            Action::GbaDown(Btn::A) => Press::Keep,
            Action::GbaDown(Btn::B) => Press::Back,
            _ => return,
        };
        let now = self.now();
        let Some(picker) = &mut self.core_picker else {
            return;
        };
        let outcome = picker.press(press, now);
        if let Outcome::Write(core) = outcome {
            self.write_core(core);
        }
    }

    /// 「联机」in the in-game menu: the same door SELECT+M opens.
    ///
    /// Only the panel has to go first, so the link screen is not drawn under a
    /// menu that still owns the input. A link that is already running opens its
    /// own page: ending it is a deliberate second step there (断开联机 asks
    /// first), never the first thing a stray A does.
    fn open_link_from_menu(&mut self) {
        self.utility = None;
        if self.link_active() {
            self.open_link_admin();
            return;
        }
        self.open_game_menu();
    }

    fn open_game_menu(&mut self) {
        if self.game_menu.is_some() {
            return;
        }
        if self.link_active() {
            if self.single_machine_link() {
                self.open_link_admin();
                return;
            }
            return self.refuse();
        }
        self.wifi_activity_started();
        if self.single_machine_link() {
            // A Famicom, Super Famicom or Mega Drive has no link hardware to
            // choose between — no cable, no wireless adapter — so the hotspot
            // is the only transport and the two ends play one console with
            // two pads. `LinkKind::Cable` here only picks which of the two
            // link screens is drawn.
            //
            // Checked before the Advance/Game Boy branch on purpose: that one
            // asks the *cart* whether it carries a link, and an FC/SFC/MD cart
            // carries nothing to ask about.
            self.link_hardware = LinkKind::Cable;
            self.radio.ask(RadioJob::Warm);
            self.game_menu = Some(GameMenu::Pick(self.last_role));
            return;
        }
        if self.core == Core::Mgba {
            let wireless = self
                .seated()
                .is_some_and(|stem| self.link_mode(stem).0 == LinkKind::Wireless);
            if wireless {
                self.hud.toast(Toast::NeedsGpsp, self.now());
                return;
            }
            self.link_hardware = LinkKind::Cable;
            self.radio.ask(RadioJob::Warm);
            self.game_menu = Some(GameMenu::Pick(self.last_role));
            return;
        }
        if self.platform != Platform::Gba {
            self.hud.toast(Toast::NoLink, self.now());
            return;
        }
        let carried = self
            .seated()
            .and_then(|stem| self.auto_link(stem))
            .is_some_and(|(cart, _)| link_carried(&cart.code, &cart.title));
        if !carried {
            self.hud.toast(Toast::NoLink, self.now());
            return;
        }
        if self.core != Core::Gpsp {
            self.hud.toast(Toast::NeedsGpsp, self.now());
            return;
        }
        let hardware = self
            .seated()
            .map_or(LinkKind::Cable, |stem| self.link_mode(stem).0);
        self.link_hardware = hardware;
        self.radio.ask(RadioJob::Warm);
        self.game_menu = Some(GameMenu::Pick(self.last_role));
    }

    fn game_menu_shortcut(&mut self) {
        if self.cheat_rows.iter().any(|c|c.enabled) {for c in &mut self.cheat_rows {c.enabled=false;}self.cheat_pending=Some(Vec::new());}
        if self.link_active() {
            // Same door as the in-game 「联机」 row: the session page for a
            // Famicom link, the end-of-link question for the Advance's cable.
            return self.open_link_from_menu();
        }
        let Some(client_id) = self.link_client_id() else {
            return self.open_game_menu();
        };
        let now = self.now();
        self.game_menu = Some(GameMenu::Linked {
            role: LinkRow::from_client_id(client_id),
            worked: now,
            since: now,
            opened: true,
        });
    }

    fn end_link_from_menu(&mut self) {
        let role = self
            .link_client_id()
            .map_or(self.last_role, LinkRow::from_client_id);
        self.end_link();
        self.hud.toast(Toast::LinkEnded, self.now());
        self.unplug(role);
    }

    fn unplug(&mut self, role: LinkRow) {
        self.game_menu = Some(GameMenu::Unplug {
            role,
            since: self.now(),
        });
    }

    pub fn set_radio_jobs(&mut self, jobs: Box<dyn RadioJobs>) {
        self.radio = jobs;
    }

    // ---- main-shelf sync (project book §4.0, §5.5, §6) --------------------

    pub fn set_sync_launcher(&mut self, launcher: Box<dyn SyncLauncher>) {
        self.sync_launcher = launcher;
    }

    pub fn set_polaroid_empty_face(&mut self, tex: TexId, w: u32, h: u32) {
        if let Some(p) = &mut self.polaroids {
            p.set_empty_face(tex, w, h);
        }
    }

    /// The cloud and arrow art the overlay animates, uploaded once at boot.
    pub fn set_sync_art(&mut self, cloud: SyncFace, up: SyncFace, down: SyncFace) {
        self.sync_art = Some(SyncArt { cloud, up, down });
    }

    /// The text faces the frontend rasterised from [`App::sync_texts`].
    pub fn set_sync_text_faces(&mut self, faces: SyncFaces) {
        self.sync_faces = faces;
    }

    /// True while the shelf sync overlay owns the pad and the screen.
    pub fn shelf_sync_active(&self) -> bool {
        !matches!(self.shelf_sync, ShelfSync::Idle)
    }

    pub fn shelf_sync(&self) -> &ShelfSync {
        &self.shelf_sync
    }

    /// The strings the overlay needs, rebuilt from the state machine.  The
    /// frontend compares this value every frame and only re-rasterises text
    /// faces when it changes.
    pub fn sync_texts(&self) -> SyncTexts {
        let lang = self.language;
        match &self.shelf_sync {
            ShelfSync::Running {
                stage,
                direction,
                request,
                progress,
                progress_label,
                ..
            } => SyncTexts {
                title: match &request.ui {
                    Some(ui) if ui.animation == "cloud_sync" => {
                        let title = ui.title.trim();
                        if title.is_empty() {
                            slot_i18n::t(lang, "device.app.cheats.syncing").to_string()
                        } else {
                            title.to_string()
                        }
                    }
                    _ => slot_i18n::t(lang, "device.app.sync.title").to_string(),
                },
                banner: stage.map_or_else(
                    || slot_i18n::t(lang, "device.app.sync.preparing").to_string(),
                    |stage| stage.line().to_string(),
                ),
                progress_left: if request.mode == SyncRequestMode::Mods {
                    progress_label
                        .clone()
                        .filter(|label| !label.trim().is_empty())
                        .unwrap_or_else(|| {
                            stage.map_or_else(
                                || slot_i18n::t(lang, "device.app.sync.running").to_string(),
                                |stage| stage.line().to_string(),
                            )
                        })
                } else {
                    stage.map_or_else(
                        || slot_i18n::t(lang, "device.app.sync.preparing").to_string(),
                        |stage| stage.line().to_string(),
                    )
                },
                progress_right: if request.mode == SyncRequestMode::Mods {
                    progress
                        .map(|value| format!("{}%", (value.clamp(0.0, 1.0) * 100.0).round() as u32))
                        .unwrap_or_default()
                } else {
                    sync_stage_step(*stage)
                        .map(|step| {
                            slot_i18n::t_args(
                                lang,
                                "device.app.sync.stage",
                                &[("step", step.to_string().as_str())],
                            )
                        })
                        .unwrap_or_default()
                },
                upload_label: if direction == &Some(Direction::Upload) {
                    slot_i18n::t(lang, "device.app.sync.upload").to_string()
                } else {
                    String::new()
                },
                download_label: if direction == &Some(Direction::Download) {
                    slot_i18n::t(lang, "device.app.sync.download").to_string()
                } else {
                    String::new()
                },
                ..SyncTexts::default()
            },
            // The conflict pages are full-screen faces; they own their copy.
            ShelfSync::Conflict { .. } => SyncTexts::default(),
            ShelfSync::Settling { text, .. } => SyncTexts {
                title: slot_i18n::t(lang, "device.app.sync.title").to_string(),
                banner: text.clone(),
                progress_left: slot_i18n::t(lang, "device.app.sync.done").to_string(),
                ..SyncTexts::default()
            },
            ShelfSync::Result { text, .. } => SyncTexts {
                banner: text.clone(),
                ..SyncTexts::default()
            },
            ShelfSync::Connecting { .. } => SyncTexts {
                title: slot_i18n::t(lang, "device.app.sync.title").to_string(),
                banner: slot_i18n::t(lang, "device.app.sync.connecting").to_string(),
                progress_left: slot_i18n::t(lang, "device.app.sync.connecting").to_string(),
                ..SyncTexts::default()
            },
            ShelfSync::Ejecting { text, .. } => SyncTexts {
                banner: text.clone(),
                progress_left: slot_i18n::t(lang, "device.app.sync.done").to_string(),
                ..SyncTexts::default()
            },
            ShelfSync::Idle
            | ShelfSync::Holding { .. }
            | ShelfSync::Opening { .. }
            | ShelfSync::Unholding { .. }
            | ShelfSync::Swapping { .. }
            | ShelfSync::Choice { .. } => SyncTexts::default(),
        }
    }

    /// Y went down on the shelf: arm the long-press timer.
    fn start_sync_hold(&mut self) {
        if !matches!(self.shelf_sync, ShelfSync::Idle) || !matches!(self.phase, Phase::Shelf) {
            return;
        }
        self.wifi_activity_started();
        if self.web.as_ref().is_some_and(|web| web.busy()) {
            self.refuse();
            return;
        }
        if self.selected_stem().is_none() {
            return;
        }
        // The hold starts the PCB opening animation; only then fall back to
        // the cloud chip if the selected mod is no longer available.
        self.refresh_sync_mods();
        self.sync_hold_capability = self
            .root
            .as_deref()
            .is_some_and(|root| self.cloud_is_the_only_sync(root));
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.sync_open_at = None;
        self.sync_close_at = None;
        self.sync_result_at = None;
        self.sync_swap_card = None;
        self.sync_card = None;
        self.shelf_sync = ShelfSync::Holding {
            since: self.now(),
        };
    }

    /// Keys while the overlay is up.  Only the handles documented in §6.4 get
    /// through; everything else is frozen until the overlay is back to Idle
    /// (A/M/X/START/L1/R1/SELECT then keep their old shelf meaning).
    fn shelf_sync_input(&mut self, action: Action) {
        if let ShelfSync::Holding { since } = &self.shelf_sync {
            let since = *since;
            if action == Action::GbaUp(Btn::Y) {
                let elapsed = self.now().saturating_sub(since);
                if elapsed >= SYNC_HOLD_MS {
                    self.begin_sync(None);
                } else {
                    let from = (elapsed as f32 / OPEN_MS as f32).clamp(0.0, 1.0);
                    self.shelf_sync = ShelfSync::Unholding {
                        since: self.now(),
                        from,
                        preview: sync_preview_at(elapsed),
                    };
                    self.wifi_activity_finished();
                }
            }
            return;
        }
        if matches!(
            self.shelf_sync,
            ShelfSync::Opening { .. } | ShelfSync::Unholding { .. } | ShelfSync::Swapping { .. }
        ) {
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Choice { .. }) {
            self.choice_input(action);
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Running { .. }) {
            if action == Action::GbaDown(Btn::B) {
                self.cancel_with_banner();
            }
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Conflict { .. }) {
            self.conflict_input(action);
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Connecting { .. }) {
            if action == Action::GbaDown(Btn::B) {
                self.sync_pending = None;
                self.sync_warmup = None;
                // The network card may already be in the slot at this stage.
                self.sync_insert = None;
                self.sync_card = None;
                self.shelf_sync = ShelfSync::Idle;
                self.wifi_activity_finished();
            }
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Ejecting { .. }) {
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Settling { .. }) {
            if !matches!(action, Action::GbaUp(_)) {
                // Dismissing the settle beat early must still take the network
                // cartridge out of the slot: leaving `sync_card` behind hides
                // the game card for the rest of the session.
                let now = self.now();
                let text = match &self.shelf_sync {
                    ShelfSync::Settling { text, .. } => text.clone(),
                    _ => String::new(),
                };
                self.sync_insert = None;
                if self.sync_card.is_some() {
                    self.shelf_sync = ShelfSync::Ejecting { since: now, text };
                } else {
                    self.shelf_sync = ShelfSync::Idle;
                }
            }
            return;
        }
        if matches!(self.shelf_sync, ShelfSync::Result { .. }) {
            // Any fresh press dismisses the result banner early.
            if !matches!(action, Action::GbaUp(_)) {
                self.shelf_sync = ShelfSync::Idle;
            }
        }
    }

    /// Keys while a generic mod choice is up: ↑/↓ (and ←/→) move, A confirms,
    /// B cancels the whole manual run.
    fn choice_input(&mut self, action: Action) {
        let lang = self.language;
        let ShelfSync::Choice {
            request,
            mut state,
        } = std::mem::replace(&mut self.shelf_sync, ShelfSync::Idle)
        else {
            return;
        };
        match action {
            Action::GbaDown(Btn::Up)
            | Action::GbaDown(Btn::Left)
            | Action::ShelfLeft => {
                state.previous();
                self.shelf_sync = ShelfSync::Choice { request, state };
            }
            Action::GbaDown(Btn::Down)
            | Action::GbaDown(Btn::Right)
            | Action::ShelfRight => {
                state.next();
                self.shelf_sync = ShelfSync::Choice { request, state };
            }
            Action::GbaDown(Btn::A) | Action::Insert => {
                let Some(option) = state
                    .request
                    .options
                    .get(state.selected)
                    .map(|option| option.id.clone())
                else {
                    self.shelf_sync = ShelfSync::Choice { request, state };
                    return;
                };
                let request = request.with_choice(state.request.key.clone(), option);
                self.launch_choice(request);
            }
            Action::GbaDown(Btn::B) => {
                let fallback = match state.request.kind {
                    slot_mods::ChoiceKind::Choice => None,
                    slot_mods::ChoiceKind::Confirm => Some("no"),
                    slot_mods::ChoiceKind::Message => Some("ok"),
                };
                if let Some(option) = fallback.filter(|option| {
                    state
                        .request
                        .options
                        .iter()
                        .any(|candidate| candidate.id == *option)
                }) {
                    let request =
                        request.with_choice(state.request.key.clone(), option.to_string());
                    self.launch_choice(request);
                } else {
                    self.wifi_activity_finished();
                    let now = self.now();
                    self.show_sync_result(slot_i18n::t(lang, "device.app.common.cancelled"), now);
                }
            }
            _ => self.shelf_sync = ShelfSync::Choice { request, state },
        }
    }

    /// Relaunch `mods run` with the accumulated `--choice` answers.
    fn launch_choice(&mut self, request: SyncRequest) {
        let now = self.now();
        self.sync_pop_at = None;
        if let Some(reason) = self.sync_refusal() {
            self.wifi_activity_finished();
            self.show_sync_result(reason, now);
            return;
        }
        let run = self.sync_launcher.launch(request.clone());
        self.wifi_activity_started();
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_sync = ShelfSync::Running {
            run,
            request,
            started: now,
            stage: None,
            direction: None,
            conflict: None,
            progress: None,
            progress_label: None,
        };
    }

    /// The UI data for the generic manual-mod choice modal.
    pub fn mod_choice_ui(&self) -> Option<ModChoiceUi> {
        let ShelfSync::Choice { state, .. } = &self.shelf_sync else {
            return None;
        };
        Some(ModChoiceUi {
            title: state.request.title.clone(),
            body: state.request.body.clone(),
            options: state
                .request
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect(),
            selected: state.selected,
        })
    }

    // ---- multi-page conflict (project book §7) --------------------------

    /// Keys while the conflict pages are up: ←/→ pages, ↑/↓ direction, A
    /// confirms the page, B abandons the remaining units.
    fn conflict_input(&mut self, action: Action) {
        let lang = self.language;
        let ShelfSync::Conflict { mut state } =
            std::mem::replace(&mut self.shelf_sync, ShelfSync::Idle)
        else {
            return;
        };
        match action {
            Action::GbaDown(Btn::Left) | Action::ShelfLeft => {
                state.prev_page();
                state.notice = None;
                self.shelf_sync = ShelfSync::Conflict { state };
            }
            Action::GbaDown(Btn::Right) | Action::ShelfRight => {
                state.next_page();
                state.notice = None;
                self.shelf_sync = ShelfSync::Conflict { state };
            }
            Action::GbaDown(Btn::Up) | Action::GbaDown(Btn::Down) => {
                if !state.page_locked() {
                    let next = state.direction().other();
                    state.set_direction(next);
                    state.error = None;
                    state.notice = None;
                }
                self.shelf_sync = ShelfSync::Conflict { state };
            }
            Action::GbaDown(Btn::A) | Action::Insert => {
                if state.page_locked() {
                    state.notice = Some(slot_i18n::t(lang, "device.app.sync.check").to_string());
                    self.shelf_sync = ShelfSync::Conflict { state };
                } else {
                    self.launch_conflict_sync(state);
                }
            }
            Action::GbaDown(Btn::B) => self.abandon_conflict(state),
            _ => self.shelf_sync = ShelfSync::Conflict { state },
        }
    }

    /// Launch the CLI for the current conflict page; the conflict state rides
    /// along in the run and is resumed by [`Self::finish_conflict_stage`].
    fn launch_conflict_sync(&mut self, mut state: ConflictState) {
        let lang = self.language;
        self.sync_pop_at = None;
        let (units, direction, pending) = if state.page == ConflictPage::All {
            let targets: Vec<UnitKind> = if state.retry.is_empty() {
                state.units.iter().map(|unit| unit.kind).collect()
            } else {
                state.retry.clone()
            };
            (
                targets.clone(),
                state.all_choice,
                ConflictPending::Both(targets),
            )
        } else {
            let kind = state.page.unit().unwrap_or(UnitKind::State);
            (vec![kind], state.direction(), ConflictPending::Unit(kind))
        };
        let now = self.now();
        let Some((root, stem, platform, emulator)) = self.sync_target() else {
            state.error = Some(slot_i18n::t(lang, "device.app.sync.no_card").to_string());
            self.shelf_sync = ShelfSync::Conflict { state };
            return;
        };
        if let Some(reason) = self.sync_refusal() {
            state.error = Some(reason.to_string());
            self.shelf_sync = ShelfSync::Conflict { state };
            return;
        }
        let mut request = SyncRequest::new(platform.dir_name(), emulator.as_str(), stem, root);
        if units.len() == 1 {
            request = request.with_only(units[0]);
        }
        let mut resolve = ResolveSpec::default();
        let mut expect = ExpectedVersions::default();
        for kind in &units {
            match kind {
                UnitKind::State => resolve.state = Some(direction.direction()),
                UnitKind::Sav => resolve.sav = Some(direction.direction()),
            }
            if let Some(version) = state.unit(*kind).and_then(|unit| unit.info.cloud.version) {
                match kind {
                    UnitKind::State => expect.state = Some(version),
                    UnitKind::Sav => expect.sav = Some(version),
                }
            }
        }
        state.pending = Some(pending);
        state.error = None;
        state.notice = None;
        state.since = now;
        request = request.with_resolve(resolve).with_expect(expect);
        let run = self.sync_launcher.launch(request.clone());
        self.wifi_activity_started();
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_sync = ShelfSync::Running {
            run,
            request,
            started: now,
            stage: None,
            direction: None,
            conflict: Some(state),
            progress: None,
            progress_label: None,
        };
    }

    /// B on a conflict page: drop the remaining units, keeping the ones that
    /// were already synced (project book §7.9).
    fn abandon_conflict(&mut self, state: ConflictState) {
        let lang = self.language;
        let remaining: Vec<&str> = state
            .units
            .iter()
            .filter(|unit| !state.is_locked(unit.kind))
            .map(|unit| unit_label(lang, unit.kind))
            .collect();
        if remaining.is_empty() {
            self.shelf_sync = ShelfSync::Idle;
        } else {
            let now = self.now();
            let items = remaining.join("、");
            self.show_sync_result(
                slot_i18n::t_args(lang, "device.app.sync.item_failed", &[("item", items.as_str())]),
                now,
            );
        }
    }

    /// Fold one finished conflict CLI run back into the pages.
    fn finish_conflict_stage(&mut self, mut state: ConflictState, outcome: SyncOutcome, now: Millis) {
        let lang = self.language;
        let pending = state.pending.take();
        match outcome.result {
            SyncResult::Ok | SyncResult::Noop => {
                let mut finished = true;
                if let Some(ConflictPending::Unit(kind)) = pending {
                    state.lock(kind);
                    finished = state.all_locked();
                    if !finished {
                        if let Some(page) = state.next_focus_page() {
                            state.page = page;
                            state.notice = None;
                            state.since = now;
                        } else {
                            finished = true;
                        }
                    }
                }
                if finished {
                    self.show_sync_result(success_text(&outcome, lang), now);
                } else {
                    state.error = None;
                    self.shelf_sync = ShelfSync::Conflict { state };
                }
            }
            SyncResult::Partial => {
                state.retry = state
                    .units
                    .iter()
                    .map(|unit| unit.kind)
                    .filter(|kind| {
                        !matches!(
                            outcome.unit_result(*kind),
                            Some(SyncResult::Ok | SyncResult::Noop)
                        )
                    })
                    .collect();
                state.error = Some(partial_text(&outcome, lang));
                self.shelf_sync = ShelfSync::Conflict { state };
            }
            SyncResult::Conflict => {
                if let Some(fresh) = ConflictState::from_outcome(&outcome, now) {
                    self.shelf_sync = ShelfSync::Conflict { state: fresh };
                } else {
                    state.error = Some(outcome.conflict_line().to_string());
                    self.shelf_sync = ShelfSync::Conflict { state };
                }
            }
            SyncResult::Choice => {
                state.error = Some(slot_i18n::t(lang, "device.app.sync.unexpected_choice").to_string());
                self.shelf_sync = ShelfSync::Conflict { state };
            }
            SyncResult::Error => {
                if outcome.changed.is_some() || outcome.error.as_deref() == Some("changed-since") {
                    state.apply_changed_since(&outcome, now, lang);
                } else {
                    state.error = Some(failure_text(&outcome, lang));
                    if let Some(ConflictPending::Both(units)) = pending {
                        state.retry = units;
                    }
                }
                self.shelf_sync = ShelfSync::Conflict { state };
            }
        }
    }

    /// The UI data for the conflict pages; the frontend renders the opaque
    /// faces from it and caches by equality.
    pub fn conflict_ui(&self) -> Option<ConflictUi> {
        match &self.shelf_sync {
            ShelfSync::Conflict { state } => Some(state.to_ui(false, self.language)),
            _ => None,
        }
    }

    /// Launch `linkSlot sync`, optionally resolving a conflict direction.
    ///
    /// The network chip on the cartridge chooses the sync method. The old
    /// multi-card chooser is deliberately bypassed for the long-press gesture.
    fn begin_sync(&mut self, resolve: Option<Resolve>) {
        let lang = self.language;
        if !matches!(
            self.shelf_sync,
            ShelfSync::Holding { .. } | ShelfSync::Conflict { .. }
        ) {
            return;
        }
        let now = self.now();
        let Some((root, stem, platform, emulator)) = self.sync_target() else {
            self.refuse();
            self.show_sync_result(slot_i18n::t(lang, "device.app.sync.no_card"), now);
            return;
        };
        if let Some(reason) = self.sync_refusal() {
            self.refuse();
            self.show_sync_result(reason, now);
            return;
        }
        // A conflict resolution always means the cloud; normal long-presses
        // use whatever the cartridge's network chip currently shows.
        let request = if resolve.is_some() {
            let request = SyncRequest::new(platform.dir_name(), emulator.as_str(), stem, root);
            self.cloud_only(request, resolve)
        } else {
            match self.selected_sync_pick() {
                Some(SyncPick::Mod {
                    mod_id,
                    action_id,
                    ui,
                }) => SyncRequest::manual_for(
                    platform.dir_name(),
                    emulator.as_str(),
                    stem,
                    root,
                    mod_id,
                    action_id,
                )
                .with_ui(ui),
                _ => {
                    let request =
                        SyncRequest::new(platform.dir_name(), emulator.as_str(), stem, root);
                    self.cloud_only(request, None)
                }
            }
        };
        self.wifi_activity_started();
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_sync = ShelfSync::Opening { since: now, request };
    }

    /// `[sync] mode` restricts a long-press Y cloud sync to one unit; the CLI's
    /// `--only` flag overrides a config the CLI itself may not know yet.
    fn cloud_only(&self, mut request: SyncRequest, resolve: Option<Resolve>) -> SyncRequest {
        let only = match self.sync_mode {
            linkcfg::SyncMode::All => None,
            linkcfg::SyncMode::State => Some(UnitKind::State),
            linkcfg::SyncMode::Sav => Some(UnitKind::Sav),
        };
        if let Some(kind) = only {
            request = request.with_only(kind);
        }
        if let Some(resolve) = resolve {
            request = request.resolved(resolve);
        }
        request
    }

    /// True when the long press has no alternative but the cloud: one card on
    /// the picker means the built-in sync is the only destination.
    fn cloud_is_the_only_sync(&self, root: &Path) -> bool {
        let Ok(entries) = slot_mods::list(root) else {
            return true;
        };
        for entry in entries {
            if !entry.enabled || entry.error.is_some() {
                continue;
            }
            let Ok(detail) = slot_mods::read_mod(root, &entry.id) else {
                continue;
            };
            if detail.actions.iter().any(|action| {
                action.enabled
                    && action.error.is_none()
                    && action.timing == Some(slot_mods::Timing::Manual)
            }) {
                return false;
            }
        }
        true
    }

        pub fn auto_cloud_card(&self) -> Option<(TexId, u32, u32)> {
        self.auto_cloud_card
    }

    pub fn set_auto_cloud_card(&mut self, face: (TexId, u32, u32)) {
        if self.auto_cloud_card.is_none() {
            self.auto_cloud_card = Some(face);
        }
    }

        pub fn transfer_view(&self) -> Option<TransferView> {
        let transfer = self.transfer.as_ref()?;
        match transfer.stage {
            TransferStage::OfferSent
            | TransferStage::Sending
            | TransferStage::Receiving
            | TransferStage::Done => {}
            _ => return None,
        }
        let percent = if transfer.stage == TransferStage::Done {
            100
        } else {
            (transfer.progress.clamp(0.0, 1.0) * 100.0).round() as u32
        };
        Some(TransferView {
            percent,
            assets: transfer
                .display_assets()
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
    }

    pub fn set_transfer_faces(&mut self, faces: TransferFaces) {
        self.transfer_faces = Some(faces);
    }

    pub fn game_intro(&self) -> Option<&GameIntro> {
        self.game_intro.as_ref()
    }

    /// The frontend rasterised the back label (paper, title, summary).
    pub fn set_game_intro_back(&mut self, back: (TexId, u32, u32)) {
        if let Some(intro) = self.game_intro.as_mut() {
            intro.back = Some(back);
        }
    }

    /// B was released before the beat finished: play the preview backwards.
    fn cancel_game_intro(&mut self) {
        let now = self.now();
        if let Some(intro) = self.game_intro.as_mut() {
            if intro.cancel.is_none() && intro.closing.is_none() {
                intro.cancel = Some(now);
            }
        }
    }

    /// Start the reverse animation instead of dropping the card at once.
    fn close_game_intro(&mut self) {
        let now = self.now();
        if let Some(intro) = self.game_intro.as_mut() {
            if intro.closing.is_none() {
                intro.closing = Some(now);
            }
        }
    }

    pub fn set_game_intro_faces(&mut self, faces: GameIntroFaces) {
        if let Some(intro) = self.game_intro.as_mut() {
            intro.faces = Some(faces);
        }
    }

    /// The frontend wrapped the full description; let the D-pad clamp to it.
    pub fn set_game_intro_scroll(&mut self, max_scroll: usize) {
        if let Some(intro) = self.game_intro.as_mut() {
            intro.max_scroll = max_scroll;
            intro.scroll = intro.scroll.min(max_scroll);
        }
    }

    /// Replace the body texture after the player scrolled one line.
    pub fn set_game_intro_body(&mut self, body: (TexId, u32, u32), scroll: usize) {
        if let Some(faces) = self.game_intro.as_mut().and_then(|intro| intro.faces.as_mut()) {
            faces.body = body;
            faces.body_scroll = scroll;
        }
    }

    pub fn set_game_intro_video(&mut self, video: IntroVideoFaces) {
        let now = self.now();
        if let Some(intro) = self.game_intro.as_mut() {
            if let Some(faces) = intro.faces.as_mut() {
                faces.video = Some(video);
                intro.started = now;
            }
        }
    }

    /// Streamed decode path: the first frame starts playback immediately and
    /// later frames are appended without resetting the clock.
    pub fn append_game_intro_video_frame(&mut self, frame: (TexId, u32)) {
        if let Some(video) = self
            .game_intro
            .as_mut()
            .and_then(|intro| intro.faces.as_mut())
            .and_then(|faces| faces.video.as_mut())
        {
            video.frames.push(frame);
        }
    }

    fn intro_delete_input(&mut self, action: Action) {
        let index = self.utility.map(|(_, index)| index).unwrap_or(0).min(1);
        match action {
            Action::GbaDown(Btn::Up) | Action::GbaDown(Btn::Down) => {
                self.utility = Some((10, 1 - index));
            }
            Action::GbaDown(Btn::A) if index == 0 => self.delete_current_cart(),
            Action::GbaDown(Btn::A) | Action::GbaDown(Btn::B) | Action::QuickMenu => {
                self.utility = None;
            }
            _ => {}
        }
    }

    fn delete_current_cart(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(cart) = self.shelf().carts.get(self.shelf().index).cloned() else {
            return;
        };
        if self.card_read_only
            || self
                .power
                .as_ref()
                .is_some_and(|power| power.card_read_only())
        {
            self.hud.toast(Toast::CartDeleteFailed, self.now());
            return;
        }
        let _ = slot_store::cart_meta::remove(&root, &cart);
        let assets = [
            cart.label.clone(),
            slot_store::cart_meta::intro_path(&root, &cart),
            slot_store::cart_meta::video_path(&root, &cart),
        ];
        let mut failed = false;
        for path in assets.into_iter().flatten() {
            if let Err(error) = std::fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    failed = true;
                }
            }
        }
        if let Err(error) = std::fs::remove_file(&cart.rom) {
            if error.kind() != std::io::ErrorKind::NotFound {
                failed = true;
            }
        }
        if failed {
            self.hud.toast(Toast::CartDeleteFailed, self.now());
            return;
        }
        self.game_intro = None;
        self.utility = None;
        self.reload_shelves();
        self.hud.toast(Toast::CartDeleted, self.now());
    }

    fn open_game_intro(&mut self) {
        let lang = self.language;
        if !matches!(self.phase, Phase::Shelf) {
            return;
        }
        let Some(cart) = self.shelf().carts.get(self.shelf().index) else {
            return;
        };
        let now = self.now();
        let shell = slot_ui::shell_for(cart).colour;
        let title = slot_ui::label_text(cart);
        let cover = cart.label.clone();
        let mut description = slot_i18n::t(lang, "device.app.guide.no_intro").to_string();
        let mut demo = None;
        if let Some(root) = self.root.clone() {
            if let Some(text_path) = slot_store::cart_meta::intro_path(&root, cart) {
                if let Ok(text) = std::fs::read_to_string(text_path) {
                    let text = text.trim();
                    if !text.is_empty() {
                        description = text.chars().take(4000).collect();
                    }
                }
            }
            demo = slot_store::cart_meta::video_path(&root, cart);
        }
        self.game_intro = Some(GameIntro {
            platform: cart.platform,
            shell,
            title,
            description,
            cover,
            demo,
            started: now,
            closing: None,
            armed: false,
            cancel: None,
            back: None,
            faces: None,
            scroll: 0,
            max_scroll: 0,
        });
    }

            fn launch_sync_request(&mut self, request: SyncRequest, now: Millis) {
        self.sync_pop_at = None;
        if self.sync_open_at.is_none() {
            self.sync_open_at = Some(now);
        }
        self.sync_close_at = None;
        self.sync_result_at = None;
        let run = self.sync_launcher.launch(request.clone());
        self.shelf_mut().release_hold();
        self.play_held = None;
        self.shelf_sync = ShelfSync::Running {
            run,
            request,
            started: now,
            stage: None,
            direction: None,
            conflict: None,
            progress: None,
            progress_label: None,
        };
    }

    fn start_sync_card_insert(
        &mut self,
        card: (TexId, u32, u32),
        from: Option<SyncRect>,
        now: Millis,
    ) {
        self.sync_card = Some(card);
        self.sync_swap_card = None;
        self.sync_insert = Some(SyncInsert {
            tex: card.0,
            w: card.1,
            h: card.2,
            shadow: None,
            from,
            started: now,
            sounded: false,
        });
        self.shelf_sync = ShelfSync::Idle;
        self.shelf_mut().release_hold();
        self.play_held = None;
    }

    /// Where the highlighted cart's sync unit lives.
    fn sync_target(&self) -> Option<(PathBuf, String, Platform, Core)> {
        let cart = self.shelf().carts.get(self.shelf().index)?;
        let root = self.root.clone()?;
        let core = slot_store::core_for_platform(&root, &cart.stem, cart.platform);
        Some((root, cart.stem.clone(), cart.platform, core))
    }

    fn sync_refusal(&self) -> Option<&'static str> {
        let lang = self.language;
        if self.link_active() {
            return Some(slot_i18n::t(lang, "device.app.link.no_sync"));
        }
        if self.card_read_only || self.power.as_ref().is_some_and(|p| p.card_read_only()) {
            return Some(slot_i18n::t(lang, "device.app.error.read_only"));
        }
        if self.selected_stem().is_none() {
            return Some(slot_i18n::t(lang, "device.app.error.no_cart"));
        }
        None
    }

    fn show_sync_result(&mut self, text: impl Into<String>, now: Millis) {
        self.show_sync_result_with(text, now, true);
    }

    /// `fade` paints the screen black for one beat and fades the shelf in.
    /// After a cartridge has ejected the shelf is already on screen, so the
    /// fade would read as a flash; that path passes `false`.
    fn show_sync_result_with(&mut self, text: impl Into<String>, now: Millis, fade: bool) {
        self.sync_card = None;
        self.sync_open_at = None;
        self.sync_close_at = None;
        self.sync_result_at = fade.then_some(now);
        self.shelf_sync = ShelfSync::Result {
            text: text.into(),
            until: now + SYNC_RESULT_MS,
        };
    }

    /// B: stop the CLI and leave a short slot_i18n::t(lang, "device.app.common.cancelled") banner (§6.5).
    fn cancel_with_banner(&mut self) {
        let lang = self.language;
        let previous = std::mem::replace(&mut self.shelf_sync, ShelfSync::Idle);
        match previous {
            ShelfSync::Running { mut run, .. } => {
                run.cancel();
                let now = self.now();
                if self.sync_card.is_some() {
                    // The cartridge is already in the slot: eject it through
                    // the normal path instead of leaving the shelf card hidden.
                    self.shelf_sync = ShelfSync::Ejecting {
                        since: now,
                        text: slot_i18n::t(lang, "device.app.common.cancelled").to_string(),
                    };
                } else {
                    self.shelf_sync = ShelfSync::Result {
                        text: slot_i18n::t(lang, "device.app.common.cancelled").to_string(),
                        until: now + SYNC_CANCEL_MS,
                    };
                }
            }
            ShelfSync::Choice { .. } => {
                let now = self.now();
                self.shelf_sync = ShelfSync::Result {
                    text: slot_i18n::t(lang, "device.app.common.cancelled").to_string(),
                    until: now + SYNC_CANCEL_MS,
                };
            }
            _ => {}
        }
        self.wifi_activity_finished();
    }

    /// Kill any running sync: lid close, power menu, shutdown, restart, link.
    /// The CLI's "meta last + backups" design keeps the card intact (§5.4).
    pub fn cancel_shelf_sync(&mut self) {
        let previous = std::mem::replace(&mut self.shelf_sync, ShelfSync::Idle);
        match previous {
            ShelfSync::Running { mut run, .. } => run.cancel(),
            ShelfSync::Choice { .. } => {}
            _ => {}
        }
        self.sync_insert = None;
        self.sync_card = None;
        self.sync_pending = None;
        self.cancel_boot_mods();
        self.wifi_activity_finished();
    }

    fn cancel_boot_mods(&mut self) {
        if let Some(mut job) = self.boot_job.take() {
            job.run.cancel();
            eprintln!(
                "slot: boot mods: cancelled {}/{}",
                job.action.mod_id, job.action.action_id
            );
        }
        if let Some(schedule) = self.boot_mods.as_mut() {
            schedule.abandon_in_flight();
        }
    }

    /// The background scheduler must not touch the card while a game, sync or
    /// link session owns it.  It also waits for the first usable UI state after
    /// the boot/clock screen.
    fn boot_mods_ready(&self) -> bool {
        let Some(root) = &self.root else {
            return false;
        };
        if !root.join("System/linkSlot/shell/linkslot.sh").is_file() {
            return false;
        }
        if self.powering_off
            || self.restarting
            || self.repairing
            || self.shutting_down()
            || self.power_menu.is_some()
        {
            return false;
        }
        if self.shelf_sync_active() || self.link_active() {
            return false;
        }
        if self.card_read_only
            || self
                .power
                .as_ref()
                .is_some_and(|power| power.card_read_only())
        {
            return false;
        }
        !matches!(
            self.phase,
            Phase::Playing { .. }
                | Phase::Inserting { .. }
                | Phase::Ejecting { .. }
                | Phase::Polaroids { .. }
                | Phase::SetClock { .. }
                | Phase::Doze { .. }
        )
    }

    /// Poll one `after_boot` child at a time and launch the next due action.
    /// This never paints the sync overlay and never blocks the frame.
    fn poll_boot_mods(&mut self) {
        let lang = self.language;
        let now = self.now();
        if let Some(mut job) = self.boot_job.take() {
            let mut finished = None;
            loop {
                match job.run.poll() {
                    None => break,
                    Some(SyncUpdate::Stage(_)) | Some(SyncUpdate::Direction(_)) | Some(SyncUpdate::Progress(_)) | Some(SyncUpdate::ProgressLabel(_)) => {}
                    Some(SyncUpdate::Finished(outcome)) => {
                        finished = Some(outcome);
                        break;
                    }
                }
            }
            if finished.is_none() {
                if job.run.exited() {
                    finished = Some(SyncOutcome::error(slot_i18n::t(lang, "device.app.mods.boot_exited")));
                } else if now.saturating_sub(job.started) >= SYNC_TIMEOUT_MS {
                    job.run.cancel();
                    finished = Some(SyncOutcome::error(slot_i18n::t(lang, "device.app.mods.boot_timeout")));
                }
            }
            if let Some(outcome) = finished {
                let ok = matches!(outcome.result, SyncResult::Ok | SyncResult::Noop);
                eprintln!(
                    "slot: boot mods: {}/{} {}: {}",
                    job.action.mod_id,
                    job.action.action_id,
                    if ok { "ok" } else { "failed" },
                    outcome.detail.as_deref().unwrap_or("")
                );
                if let Some(schedule) = self.boot_mods.as_mut() {
                    schedule.finish(&job.action, ok, now);
                }
            } else {
                self.boot_job = Some(job);
                return;
            }
        }

        if !self.boot_mods_ready() {
            return;
        }
        let Some(action) = self
            .boot_mods
            .as_ref()
            .and_then(|schedule| schedule.next_due(now))
        else {
            return;
        };
        let Some(root) = self.root.clone() else {
            return;
        };
        if let Some(schedule) = self.boot_mods.as_mut() {
            schedule.start(&action);
        }
        eprintln!(
            "slot: boot mods: launching {}/{} (+{} min)",
            action.mod_id, action.action_id, action.delay_minutes
        );
        let request = SyncRequest::boot(root, action.mod_id.clone(), action.action_id.clone());
        let run = self.sync_launcher.launch(request);
        self.boot_job = Some(BootJob {
            run,
            action,
            started: now,
        });
    }

    /// Called every frame from `update`: advances the long-press timer, polls
    /// the child process and expires banners.  Never blocks.
    fn poll_shelf_sync(&mut self) {
        let lang = self.language;
        let now = self.now();
        match &self.shelf_sync {
            ShelfSync::Holding { since } if now.saturating_sub(*since) >= SYNC_HOLD_MS => {
                self.begin_sync(None);
                return;
            }
            ShelfSync::Opening { since, request }
                if now.saturating_sub(*since) >= (SYNC_OPENING_S * 1000.0) as Millis =>
            {
                let request = request.clone();
                self.start_sync_after_opening(request, now);
                return;
            }
            ShelfSync::Unholding { since, .. }
                if now.saturating_sub(*since) >= CLOSE_MS =>
            {
                self.shelf_sync = ShelfSync::Idle;
                return;
            }
            ShelfSync::Swapping { since, card } if now.saturating_sub(*since) >= (SYNC_SWAP_S * 1000.0) as Millis => {
                let card = *card;
                let from = self.selected_cart_rect().map(|(_, _, rect)| rect);
                self.start_sync_card_insert(card, from, now);
                return;
            }
            ShelfSync::Settling { until, text, .. } if now >= *until => {
                let text = text.clone();
                if self.sync_card.is_some() {
                    self.shelf_sync = ShelfSync::Ejecting { since: now, text };
                } else {
                    self.show_sync_result(text, now);
                }
                return;
            }
            ShelfSync::Ejecting { since, .. }
                if now.saturating_sub(*since) >= (EJECT_S * 1000.0) as Millis =>
            {
                let text = match &self.shelf_sync {
                    ShelfSync::Ejecting { text, .. } => text.clone(),
                    _ => String::new(),
                };
                self.sync_card = None;
                self.show_sync_result_with(text, now, false);
                return;
            }
            ShelfSync::Result { until, .. } if now >= *until => {
                self.shelf_sync = ShelfSync::Idle;
                self.sync_result_at = None;
                return;
            }
            ShelfSync::Running { .. } => {}
            _ => return,
        }
        let mut finished = None;
        let mut timed_out = false;
        let mut exited = false;
        {
            let ShelfSync::Running {
                run,
                stage,
                direction,
                progress,
                progress_label,
                started,
                ..
            } = &mut self.shelf_sync
            else {
                return;
            };
            loop {
                match run.poll() {
                    None => break,
                    Some(SyncUpdate::Stage(next)) => *stage = Some(next),
                    Some(SyncUpdate::Direction(next)) => *direction = Some(next),
                    Some(SyncUpdate::Progress(next)) => *progress = Some(next),
                    Some(SyncUpdate::ProgressLabel(next)) => *progress_label = Some(next),
                    Some(SyncUpdate::Finished(outcome)) => {
                        finished = Some(outcome);
                        break;
                    }
                }
            }
            if finished.is_none() {
                if run.exited() {
                    exited = true;
                } else if now.saturating_sub(*started) >= SYNC_TIMEOUT_MS {
                    timed_out = true;
                }
            }
        }
        if timed_out {
            if let ShelfSync::Running { run, .. } = &mut self.shelf_sync {
                run.cancel();
            }
            self.wifi_activity_finished();
            self.show_sync_result(slot_i18n::t(lang, "device.app.sync.timeout"), now);
        } else if exited {
            self.wifi_activity_finished();
            self.show_sync_result(slot_i18n::t(lang, "device.app.sync.exited"), now);
        } else if let Some(outcome) = finished {
            self.finish_shelf_sync(outcome, now);
        }
    }

    fn finish_shelf_sync(&mut self, outcome: SyncOutcome, now: Millis) {
        let lang = self.language;
        self.wifi_activity_finished();
        self.invalidate_shelf_playtime();
        // Manual mods report their own single-line reason; the cloud error
        // categories (slot_i18n::t(lang, "device.app.error.config") etc.) would hide it.
        let was_mods = matches!(
            &self.shelf_sync,
            ShelfSync::Running { request, .. } if request.mode == SyncRequestMode::Mods
        );
        let mods_wrote = was_mods && outcome.result == SyncResult::Ok;
        // A per-page conflict sync resumes its pages with locks preserved.
        let previous = std::mem::replace(&mut self.shelf_sync, ShelfSync::Idle);
        let (previous_request, previous_conflict) = match previous {
            ShelfSync::Running {
                request, conflict, ..
            } => (Some(request), conflict),
            _ => (None, None),
        };
        if let Some(state) = previous_conflict {
            self.finish_conflict_stage(state, outcome, now);
            return;
        }
        if !was_mods && previous_request.as_ref().is_some_and(|r| r.mode == SyncRequestMode::Cloud)
        {
            let settled = match outcome.result {
                SyncResult::Ok => Some(false),
                SyncResult::Noop => Some(true),
                _ => None,
            };
            if let Some(noop) = settled {
                let text = if noop {
                    slot_i18n::t(lang, "device.app.common.up_to_date").to_string()
                } else {
                    match (outcome.direction, outcome.version) {
                        (Some(Direction::Download), Some(version)) => {
                            slot_i18n::t_args(
                                lang,
                                "device.app.sync.pulled",
                                &[("version", version.to_string().as_str())],
                            )
                        }
                        (Some(Direction::Upload), Some(version)) => {
                            slot_i18n::t_args(
                                lang,
                                "device.app.sync.pushed",
                                &[("version", version.to_string().as_str())],
                            )
                        }
                        _ => outcome.banner_text(),
                    }
                };
                self.sync_close_at = Some(now);
                self.shelf_sync = ShelfSync::Settling {
                    until: now + SYNC_SETTLE_MS,
                    text,
                    direction: outcome.direction,
                    noop,
                    only: previous_request.as_ref().and_then(|request| request.only),
                };
                return;
            }
        }
        match outcome.result {
            SyncResult::Choice => {
                let (Some(request), Some(mut choice)) = (previous_request, outcome.choice.clone())
                else {
                    self.show_sync_result(slot_i18n::t(lang, "device.app.sync.bad_choice"), now);
                    return;
                };
                if request.mode != SyncRequestMode::Mods {
                    self.show_sync_result(slot_i18n::t(lang, "device.app.sync.unexpected_choice"), now);
                    return;
                }
                // Name the mod in the title: with two bridges installed, a bare
                // "存档冲突" would not say whose conflict this page resolves.
                if let (Some(root), Some(mod_id)) =
                    (self.root.as_deref(), request.boot_mod.as_deref())
                {
                    if let Ok(detail) = slot_mods::read_mod(root, mod_id) {
                        let name = detail.entry.name.trim();
                        if !name.is_empty() && !choice.title.contains(name) {
                            choice.title = format!("{name} · {}", choice.title);
                        }
                    }
                }
                self.shelf_sync = ShelfSync::Choice {
                    request,
                    state: ModChoiceState::new(choice),
                };
            }
            SyncResult::Conflict => {
                if let Some(state) = ConflictState::from_outcome(&outcome, now) {
                    self.shelf_sync = ShelfSync::Conflict { state };
                } else {
                    let text = outcome.conflict_line().to_string();
                    self.show_sync_result(text, now);
                }
            }
            SyncResult::Ok => {
                let text = if was_mods {
                    outcome
                        .detail
                        .clone()
                        .unwrap_or_else(|| outcome.banner_text())
                } else {
                    match (outcome.direction, outcome.version) {
                        (Some(Direction::Download), Some(version)) => {
                            slot_i18n::t_args(
                                lang,
                                "device.app.sync.pulled",
                                &[("version", version.to_string().as_str())],
                            )
                        }
                        (Some(Direction::Upload), Some(version)) => {
                            slot_i18n::t_args(
                                lang,
                                "device.app.sync.pushed",
                                &[("version", version.to_string().as_str())],
                            )
                        }
                        _ => outcome.banner_text(),
                    }
                };
                self.show_sync_result(text, now);
            }
            SyncResult::Noop => {
                let text = if was_mods {
                    outcome
                        .detail
                        .clone()
                        .unwrap_or_else(|| slot_i18n::t(lang, "device.app.common.up_to_date").to_string())
                } else {
                    slot_i18n::t(lang, "device.app.common.up_to_date").to_string()
                };
                self.show_sync_result(text, now);
            }
            SyncResult::Partial => {
                // One unit transferred; the CLI detail already says which one
                // failed, so show it as the banner (T18 will add the retry
                // state for the failed unit).
                let text = outcome
                    .detail
                    .clone()
                    .or_else(|| outcome.error_line().map(str::to_string))
                    .unwrap_or_else(|| slot_i18n::t(lang, "device.app.sync.partial").to_string());
                self.show_sync_result(text, now);
            }
            SyncResult::Error => {
                // `net` covers both "Wi-Fi not connected" and "Wi-Fi is up
                // but the WebDAV host is unreachable".  Show the CLI's actual
                // detail so the banner does not blame Wi-Fi for a sleeping
                // NAS; fall back to the category label when there is none.
                let text = if was_mods {
                    outcome
                        .detail
                        .clone()
                        .or_else(|| outcome.error_line().map(str::to_string))
                        .unwrap_or_else(|| slot_i18n::t(lang, "device.app.sync.failed").to_string())
                } else {
                    match outcome.error.as_deref() {
                        Some("net") => {
                            let detail = outcome.detail.clone().unwrap_or_default();
                            // LinkError appends a generic hint after the actual
                            // reason. Keep the reason (e.g. the unreachable WebDAV
                            // host) and drop the "；网络未连接，请检查..." tail.
                            let reason = detail
                                .split(slot_i18n::t(lang, "device.app.sync.offline_suffix"))
                                .next()
                                .unwrap_or(detail.as_str())
                                .trim();
                            if reason.is_empty() {
                                outcome
                                    .error_line()
                                    .unwrap_or(slot_i18n::t(lang, "device.app.sync.offline"))
                                    .to_string()
                            } else {
                                reason.to_string()
                            }
                        }
                        _ => outcome.error_line().unwrap_or(slot_i18n::t(lang, "device.app.sync.failed")).to_string(),
                    }
                };
                self.show_sync_result(text, now);
            }
        }
        if mods_wrote {
            // A manual mod may have written Labels/Guides; rebuild the shelf
            // and label caches now so its result is visible this session.
            self.refresh_library_after_web();
        }
    }

    /// Start the attach-and-insert beat using the real selected game cart.
    /// The selected cart's screen rectangle, matching `Shelf::draw_row`.
    fn selected_cart_rect(&self) -> Option<(usize, Platform, SyncRect)> {
        let cart = self.shelf().carts.get(self.shelf().index)?;
        let (x, scale) = self.shelf().selected_at();
        let (cw, ch) = slot_ui::cart_box(cart.platform);
        let (w, h) = (cw as f32 * scale, ch as f32 * scale);
        let y = slot_ui::rest_y(ch as f32) + ch as f32 - h;
        Some((self.shelf().index, cart.platform, (x, y, w, h)))
    }

    fn draw_shelf_playtime(&self, out: &mut Vec<Draw>) {
        let Some((_, _, (cart_x, cart_y, cart_w, cart_h))) = self.selected_cart_rect() else {
            return;
        };
        let Some((tex, face_w, face_h)) = self.shelf_playtime_face else {
            return;
        };
        if self.shelf_playtime_text.is_empty() || face_w == 0 || face_h == 0 {
            return;
        }
        let (x, y, w, h) = slot_ui::playtime_plate(
            (cart_x, cart_y, cart_w, cart_h),
            (face_w, face_h),
        );
        out.push(Draw::Rect {
            x,
            y,
            w,
            h,
            colour: [0.0, 0.0, 0.0, 0.66],
        });
        out.push(Draw::Rect {
            x,
            y,
            w,
            h: 1.0,
            colour: [1.0, 1.0, 1.0, 0.10],
        });
        out.push(Draw::Tex {
            x: x + (w - face_w as f32) / 2.0,
            y: y + (h - face_h as f32) / 2.0,
            w: face_w as f32,
            h: face_h as f32,
            tex,
            alpha: 1.0,
        });
    }

    fn draw_shelf_sync(&self, out: &mut Vec<Draw>) {
        match &self.shelf_sync {
            ShelfSync::Idle => {}
            ShelfSync::Holding { since } => {
                self.draw_sync_holding(*since, out);
            }
            ShelfSync::Opening { since, .. } => self.draw_sync_opening(*since, out),
            ShelfSync::Unholding {
                since,
                from,
                preview,
            } => {
                let t = (self
                    .now()
                    .saturating_sub(*since) as f32
                    / CLOSE_MS as f32)
                    .clamp(0.0, 1.0);
                let open = (*from - t).max(0.0);
                self.draw_sync_pcb(open, out);
                let zoom = *preview * (1.0 - t);
                if zoom > 0.01 {
                    self.draw_sync_highlight(zoom, 1.0, *preview, 0.0, out);
                }
            }
            ShelfSync::Swapping { since, .. } => {
                let t = (self
                    .now()
                    .saturating_sub(*since) as f32
                    / (SYNC_SWAP_S * 1000.0))
                    .clamp(0.0, 1.0);
                self.draw_hold_swap(ease(t), out);
            }
            ShelfSync::Running {
                started,
                direction,
                stage,
                request,
                progress,
                ..
            } if request.mode == SyncRequestMode::Cloud || request.ui.is_some() => {
                self.draw_sync_running(
                    *started,
                    *direction,
                    *stage,
                    request.only,
                    request.ui.as_ref(),
                    *progress,
                    out,
                )
            }
            // Mods report their own stages and have no cloud direction or save
            // units. Keep the old quiet waiting screen rather than claiming
            // they are a cloud save.
            ShelfSync::Running { .. } => self.draw_mods_running(out),
            ShelfSync::Settling {
                direction,
                noop,
                only,
                ..
            } => self.draw_sync_settled(*direction, *noop, *only, out),
            ShelfSync::Conflict { .. } => {}
            ShelfSync::Choice { .. } => {}
            ShelfSync::Connecting { since } => self.draw_sync_anim(
                *since,
                None,
                None,
                None,
                None,
                None,
                false,
                false,
                out,
            ),
            ShelfSync::Ejecting { since, .. } => self.draw_sync_eject(*since, out),
            ShelfSync::Result { .. } => {
                self.draw_sync_banner(out);
                self.draw_sync_result_fade(out);
            }
        }
    }

    /// A manual mod's quiet running state: dim the shelf and leave the stage
    /// line visible. The cloud animation's units and rail would be misleading
    /// for GBAStation/SaveDeck-style mods.
    fn draw_mods_running(&self, out: &mut Vec<Draw>) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.0, 0.0, 0.0, 0.42],
        });
        self.draw_sync_banner(out);
        self.draw_sync_open_fade(out);
    }

    /// The selected game card recedes and the cloud card comes forward.
    /// `t` is 0 at the resting stack and 1 with the cloud card in front.
    fn draw_hold_swap(&self, t: f32, out: &mut Vec<Draw>) {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.001 {
            return;
        }
        if !self.sync_hold_capability && self.sync_swap_card.is_none() {
            return;
        }
        let Some((tex, cw, ch)) = self.sync_swap_card.or(self.auto_cloud_card) else {
            return;
        };
        let Some((index, _, (x, y, w, h))) = self.selected_cart_rect() else {
            return;
        };
        // Game card recedes left/up and dims.
        if let Some(game) = self.shelf().face(index) {
            let gw = w * (1.0 - 0.05 * t);
            let gh = h * (1.0 - 0.05 * t);
            out.push(Draw::Tex {
                x: x - 14.0 * t + (w - gw) / 2.0,
                y: y - 6.0 * t + (h - gh) / 2.0,
                w: gw,
                h: gh,
                tex: game,
                alpha: 1.0 - 0.58 * t,
            });
        }
        // Cloud card comes from behind and lands on the same footprint.
        let cw_f = w * 0.97;
        let ch_f = cw_f * ch as f32 / cw as f32;
        let cx = x + (w - cw_f) / 2.0 + 10.0 * (1.0 - t);
        let cy = y + (h - ch_f) / 2.0 - 8.0 * (1.0 - t);
        out.push(Draw::Tex {
            x: cx,
            y: cy,
            w: cw_f,
            h: ch_f,
            tex,
            alpha: ease(t),
        });
    }

    /// Draw the cartridge using the same board/lid geometry as START.
    /// `open` is 0 (closed) to 1 (PCB fully out); `glow` lights the network chip.
    fn draw_sync_pcb(&self, open: f32, out: &mut Vec<Draw>) {
        let Some((index, platform, (x, y, w, h))) = self.selected_cart_rect() else {
            return;
        };
        let open = open.clamp(0.0, 1.0);
        let shelf = Placed { x, y, w, h };
        let board = slot_ui::board_from_for(platform, shelf, open);
        let (lid, turn) = slot_ui::lid_from_for(platform, shelf, open);
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.01, 0.02, 0.03, 0.52],
        });

        if self.core_faces_ready() {
            if let Some(tex) = self.core_board_face {
                out.push(Draw::Tex {
                    x: board.x.round(),
                    y: board.y.round(),
                    w: board.w.round(),
                    h: board.h.round(),
                    tex,
                    alpha: open.clamp(0.0, 1.0),
                });
            }
        } else if let Some(game) = self.shelf().face(index) {
            out.push(Draw::Tex {
                x,
                y,
                w,
                h,
                tex: game,
                alpha: 1.0,
            });
        }

        if let Some(tex) = self.network_face_off.or(self.network_face) {
            let (u, v) = slot_ui::network_spot(platform);
            let (x, y) = slot_ui::on_board_for(platform, board, u, v);
            let zoom = board.w / slot_ui::board_size(platform).0 as f32;
            let w = slot_ui::NETWORK_W as f32 * zoom;
            let h = slot_ui::NETWORK_H as f32 * zoom;
            out.push(Draw::Tex {
                x: x.round(),
                y: y.round(),
                w: w.round(),
                h: h.round(),
                tex,
                alpha: open.clamp(0.0, 1.0),
            });
        }

        if self.core_faces_ready() {
            if let Some(tex) = self.core_lid_face {
                out.push(Draw::Turned {
                    x: lid.x,
                    y: lid.y,
                    w: lid.w,
                    h: lid.h,
                    tex,
                    alpha: 1.0,
                    turn,
                });
            }
        }
    }

    /// Antenna rings that travel outward from the package edge and brighten
    /// as they go, so a held Y reads as the chip powering up.
    fn draw_signal_rings(
        &self,
        chip_x: f32,
        chip_y: f32,
        chip_w: f32,
        pulse: f32,
        strength: f32,
        out: &mut Vec<Draw>,
    ) {
        if strength <= 0.01 || pulse < 0.0 {
            return;
        }
        let cx = chip_x + chip_w / 2.0;
        let base = chip_y + 4.0;
        let min_r = 16.0;
        let max_r = chip_w * 0.30;
        for i in 0..3 {
            let t = (pulse * 1.15 + i as f32 / 3.0).fract();
            let r = min_r + (max_r - min_r) * t;
            // Farther out is brighter: the wave gains energy as it leaves.
            let bright = strength * (0.10 + 0.90 * t.powf(1.4));
            let thick = 1.3 + 1.7 * t;
            let mut angle = 202.0_f32;
            while angle <= 338.0 {
                let rad = angle.to_radians();
                let px = cx + r * rad.cos() - thick / 2.0;
                let py = base + r * rad.sin() * 0.50 - thick / 2.0;
                out.push(Draw::Rect {
                    x: px,
                    y: py,
                    w: thick,
                    h: thick,
                    colour: [0.42, 0.98, 0.72, bright.max(0.0).min(1.0)],
                });
                angle += 4.0;
            }
        }
    }

    /// Draw the enlarged sync chip. `led` is 0 when unlit; it flashes once
    /// while the preview is opening and stays lit while the chip is shown.
    fn draw_sync_highlight(
        &self,
        zoom: f32,
        led: f32,
        pulse: f32,
        strength: f32,
        out: &mut Vec<Draw>,
    ) {
        let Some((_, platform, (x, y, w, h))) = self.selected_cart_rect() else {
            return;
        };
        if zoom <= 0.01 {
            return;
        }
        let shelf = Placed { x, y, w, h };
        let board = slot_ui::board_from_for(platform, shelf, 1.0);
        let (u, v) = slot_ui::network_spot(platform);
        let (small_x, small_y) = slot_ui::on_board_for(platform, board, u, v);
        let board_zoom = board.w / slot_ui::board_size(platform).0 as f32;
        let small_w = slot_ui::NETWORK_W as f32 * board_zoom;
        let small_h = slot_ui::NETWORK_H as f32 * board_zoom;
        let target_w = 360.0;
        let target_h = target_w * slot_ui::NETWORK_H as f32 / slot_ui::NETWORK_W as f32;
        let chip_w = small_w + (target_w - small_w) * zoom;
        let chip_h = small_h + (target_h - small_h) * zoom;
        let small_cx = small_x + small_w / 2.0;
        let small_cy = small_y + small_h / 2.0;
        let cx = small_cx + (OUT_W as f32 / 2.0 - small_cx) * zoom;
        let cy = small_cy + (200.0 - small_cy) * zoom;
        let chip_x = cx - chip_w / 2.0;
        let chip_y = cy - chip_h / 2.0 - 10.0 * zoom;

        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.01, 0.02, 0.03, 0.48 * zoom],
        });
        if let Some(shadow) = self.core_chip_shadow_face {
            out.push(Draw::Tex {
                x: (chip_x + chip_w * 0.04).round(),
                y: (chip_y + chip_h * 0.58).round(),
                w: (chip_w * 0.92).round(),
                h: (chip_h * 0.52).round(),
                tex: shadow,
                alpha: 0.48 * zoom,
            });
        }
        // The pin-1 lamp switches on `SYNC_ZOOM_SHARE` into the preview and
        // fades up over `SYNC_MARK_FADE_S`, like a real indicator.
        let zoom_done = SYNC_ZOOM_SHARE * SYNC_PREVIEW_MS as f32 / 1000.0;
        let mark_level = ((pulse - zoom_done) / SYNC_MARK_FADE_S).clamp(0.0, 1.0);
        let face = self.sync_picker_face.or(self.network_face);
        if let Some(tex) = face {
            out.push(Draw::Tex {
                x: chip_x.round(),
                y: chip_y.round(),
                w: chip_w.round(),
                h: chip_h.round(),
                tex,
                alpha: zoom,
            });
            // Lamp cells on the enlarged face: pin-1 at (8.6, 15.6) and the
            // status LED at (12.6, 15.6), each 2 units across.
            let unit_w = chip_w / slot_ui::NETWORK_W as f32;
            let unit_h = chip_h / slot_ui::NETWORK_H as f32;
            let cell = (2.0 * unit_w).max(2.0);
            let lamp = |out: &mut Vec<Draw>, dx: f32, w: f32, colour: [f32; 4]| {
                out.push(Draw::Rect {
                    x: chip_x + dx * unit_w,
                    y: chip_y + 15.6 * unit_h,
                    w,
                    h: cell,
                    colour,
                });
            };
            if led > 0.01 {
                lamp(out, 11.6, cell * 3.0, [0.35, 1.0, 0.55, 0.40 * led]);
                lamp(out, 12.6, cell, [0.55, 1.0, 0.65, 0.95 * led]);
            }
            if mark_level > 0.01 {
                // Only the cell fades: a halo rectangle would read as a grey
                // patch on the face while it ramps up.
                lamp(
                    out,
                    8.6,
                    cell,
                    [0.36, 0.91, 0.61, mark_level],
                );
            }
        }

        self.draw_signal_rings(chip_x, chip_y, chip_w, pulse, strength * zoom, out);

    }

    /// After the long press is accepted: the chip preview is already open.
    /// Keep it on screen briefly, shrink it back, close the PCB, then insert.
    fn draw_sync_opening(&self, since: Millis, out: &mut Vec<Draw>) {
        let elapsed = self.now().saturating_sub(since) as f32 / 1000.0;
        let t = (elapsed / SYNC_OPENING_S).clamp(0.0, 1.0);
        let fall = ease(((t - 0.35) / 0.30).clamp(0.0, 1.0));
        let zoom = 1.0 - fall;
        let close = ((t - 0.65) / 0.32).clamp(0.0, 1.0);
        self.draw_sync_pcb(1.0 - close, out);
        if zoom > 0.01 {
            self.draw_sync_highlight(
                zoom,
                1.0,
                elapsed + SYNC_PREVIEW_MS as f32 / 1000.0,
                zoom,
                out,
            );
        }
    }

    /// The cart frame + progress bar while Y is held.
    fn draw_sync_holding(&self, since: Millis, out: &mut Vec<Draw>) {
        let Some((_, _, (_, y, _, h))) = self.selected_cart_rect() else {
            return;
        };
        let elapsed_ms = self.now().saturating_sub(since);
        let elapsed = elapsed_ms as f32;
        let progress = (elapsed / SYNC_HOLD_MS as f32).clamp(0.0, 1.0);
        let open = (elapsed / OPEN_MS as f32).clamp(0.0, 1.0);
        let preview = sync_preview_at(elapsed_ms);
        // The zoom finishes at 75% of the preview window, leaving a short beat
        // with the chip fully open before the threshold lands.
        let zoom = ease((preview / SYNC_ZOOM_SHARE).clamp(0.0, 1.0));
        // The status LED on the right is on for the whole hold; the pin-1 mark
        // on the left only lights once the chip has reached the front.
        let led = 1.0;
        self.draw_sync_pcb(open, out);
        if zoom > 0.01 {
            let since_preview =
                elapsed_ms.saturating_sub(SYNC_HOLD_MS.saturating_sub(SYNC_PREVIEW_MS));
            self.draw_sync_highlight(
                zoom,
                led,
                since_preview as f32 / 1000.0,
                zoom,
                out,
            );
        }
        let bx = (OUT_W as f32 - SYNC_HOLD_BAR_W) / 2.0;
        let by = (y + h + 14.0).min(OUT_H as f32 - 58.0 - SYNC_HOLD_BAR_H - 4.0);
        out.push(Draw::Rect {
            x: bx,
            y: by,
            w: SYNC_HOLD_BAR_W,
            h: SYNC_HOLD_BAR_H,
            colour: [0.0, 0.0, 0.0, 0.6],
        });
        if progress > 0.0 {
            out.push(Draw::Rect {
                x: bx,
                y: by,
                w: SYNC_HOLD_BAR_W * progress,
                h: SYNC_HOLD_BAR_H,
                colour: SYNC_CYAN,
            });
        }
    }

    /// The transfer animation: dim mask, cart cycling towards/away from the
    /// cloud, direction arrow and the stage line.
    fn draw_sync_running(
        &self,
        started: Millis,
        direction: Option<Direction>,
        stage: Option<SyncStage>,
        only: Option<UnitKind>,
        ui: Option<&slot_mods::ActionUi>,
        progress: Option<f32>,
        out: &mut Vec<Draw>,
    ) {
        self.draw_sync_anim(started, direction, stage, only, ui, progress, false, false, out);
    }

    fn draw_sync_settled(
        &self,
        direction: Option<Direction>,
        noop: bool,
        only: Option<UnitKind>,
        out: &mut Vec<Draw>,
    ) {
        self.draw_sync_anim(
            self.now(),
            direction,
            Some(SyncStage::Commit),
            only,
            None,
            Some(1.0),
            true,
            noop,
            out,
        );
    }

    /// The source cartridge's box on the sync stage.  The slot is a
    /// GBA-shaped footprint, so the card keeps its own aspect and its foot
    /// stays on the design's baseline instead of being stretched.
    fn sync_stage_cart(aspect: f32) -> (f32, f32, f32, f32) {
        let (x0, y0, w0, h0) = (84.0, 212.0, 188.0, 104.0);
        let h = (w0 / aspect.max(0.01)).min(150.0);
        let w = h * aspect;
        (x0 + (w0 - w) / 2.0, y0 + h0 - h, w, h)
    }

    /// The cloud-sync stage: cart on the left, cloud on the right, one lane
    /// for STATE and one for SAV. The rail replaces a false percentage because
    /// the CLI reports stages, not bytes.
    fn draw_sync_anim(
        &self,
        started: Millis,
        direction: Option<Direction>,
        stage: Option<SyncStage>,
        only: Option<UnitKind>,
        ui: Option<&slot_mods::ActionUi>,
        progress: Option<f32>,
        settled: bool,
        noop: bool,
        out: &mut Vec<Draw>,
    ) {
        out.push(Draw::Rect {
            x: 0.0,
            y: 0.0,
            w: OUT_W as f32,
            h: OUT_H as f32,
            colour: [0.016, 0.031, 0.047, 0.92],
        });
        // A very quiet horizontal wash keeps the grid readable without the
        // old full-screen disc covering the cloud and cart.
        for i in 0..12 {
            let y = 90.0 + i as f32 * 14.0;
            out.push(Draw::Rect {
                x: 0.0,
                y,
                w: OUT_W as f32,
                h: 14.0,
                colour: [0.018 + i as f32 * 0.001, 0.036 + i as f32 * 0.001, 0.048 + i as f32 * 0.001, 1.0],
            });
        }
        // Perspective floor.
        let horizon = 318.0;
        for i in -6..15 {
            let x1 = i as f32 * 72.0;
            out.push(Draw::Rect {
                x: 360.0 + (x1 - 360.0) * 0.25,
                y: horizon,
                w: 1.0,
                h: OUT_H as f32 - horizon,
                colour: [0.06, 0.17, 0.21, 1.0],
            });
        }
        for i in 1..5 {
            let y = horizon + (480.0 - horizon) * (i as f32 / 5.0).powf(1.7);
            out.push(Draw::Rect {
                x: 0.0,
                y,
                w: OUT_W as f32,
                h: 1.0,
                colour: [0.05, 0.13, 0.16, 1.0],
            });
        }

        let now = self.now();
        let wave = triangle(now.saturating_sub(started), 800);
        let faces = &self.sync_faces;
        if let Some(title) = faces.title.filter(|face| face.w > 0 && face.h > 0) {
            out.push(Draw::Tex {
                x: 28.0,
                y: 42.0,
                w: title.w as f32,
                h: title.h as f32,
                tex: title.tex,
                alpha: 1.0,
            });
        }

        // Source cartridge at the design's fixed stage position.  The slot is
        // a GBA-shaped box, so the card keeps its own aspect inside it: an FC
        // cassette is 1.20, not 1.78, and stretching it here was very visible.
        let (platform, face) = match self.selected_cart_rect() {
            Some((index, platform, _)) => (platform, self.shelf().face(index)),
            None => (self.platform, None),
        };
        let (cw, ch) = slot_ui::cart_box(platform);
        let aspect = if ch == 0 { 1.78 } else { cw as f32 / ch as f32 };
        let cart = Self::sync_stage_cart(aspect);
        match face {
            Some(tex) => out.push(Draw::Tex {
                x: cart.0,
                y: cart.1,
                w: cart.2,
                h: cart.3,
                tex,
                alpha: if settled { 0.9 } else { 1.0 },
            }),
            None => out.push(Draw::Rect {
                x: cart.0,
                y: cart.1,
                w: cart.2,
                h: cart.3,
                colour: with_alpha(SYNC_PLACEHOLDER, 0.95),
            }),
        }

        let Some(art) = self.sync_art.as_ref() else {
            self.draw_sync_banner(out);
            return;
        };
        let show_cloud = ui.map_or(true, |ui| ui.show_cloud);
        let (cx, cy, cw, ch) = (502.0, 222.0, 126.0, 82.0);
        if show_cloud {
            out.push(Draw::Tex {
                x: cx,
                y: cy,
                w: cw,
                h: ch,
                tex: art.cloud.tex,
                alpha: if noop || settled { 1.0 } else { 0.72 + 0.28 * wave },
            });
        }

        let transport = matches!(
            stage,
            Some(SyncStage::Upload)
                | Some(SyncStage::Download)
                | Some(SyncStage::Verify)
                | Some(SyncStage::Commit)
        );
        let direction = ui
            .and_then(|ui| match ui.direction.as_deref() {
                Some("upload") => Some(Direction::Upload),
                Some("download") => Some(Direction::Download),
                _ => None,
            })
            .or(direction);
        let upload = direction != Some(Direction::Download);
        let (show_state, show_sav) = match ui {
            Some(ui) if !ui.units.is_empty() => (
                ui.units.iter().any(|unit| unit == "state"),
                ui.units.iter().any(|unit| unit == "sav"),
            ),
            _ => (
                only != Some(UnitKind::Sav),
                only != Some(UnitKind::State),
            ),
        };
        if transport && !noop {
            let upper = ((274.0, 252.0), (405.0, 150.0), (504.0, 238.0));
            let lower = ((274.0, 302.0), (405.0, 340.0), (504.0, 300.0));
            let mut lanes = Vec::new();
            if show_state {
                lanes.push((upper, (0.34, 0.89, 0.96)));
            }
            if show_sav {
                lanes.push((lower, (0.49, 0.82, 0.97)));
            }
            if lanes.is_empty() && ui.is_some() {
                // Network-only mods (for example AI lookup) have no save unit
                // but still want a visible request stream.
                lanes.push((upper, (0.34, 0.89, 0.96)));
            }
            for (arc, color) in lanes {
                let points = 24;
                for step in 0..points {
                    let t0 = step as f32 / points as f32;
                    let t1 = (step + 1) as f32 / points as f32;
                    let t = if upload { t0 } else { 1.0 - t0 };
                    let tt = if upload { t1 } else { 1.0 - t1 };
                    let p0 = cubic_point(arc.0, arc.1, arc.2, t);
                    let p1 = cubic_point(arc.0, arc.1, arc.2, tt);
                    let fade = if settled { 0.5 } else { 0.65 };
                    out.push(Draw::Rect {
                        x: p0.0.min(p1.0),
                        y: p0.1.min(p1.1),
                        w: (p0.0 - p1.0).abs().max(1.0),
                        h: (p0.1 - p1.1).abs().max(1.0),
                        colour: [color.0, color.1, color.2, fade],
                    });
                }
                for bead in 0..7 {
                    let mut t = (bead as f32 / 7.0 + wave * 0.9).rem_euclid(1.0);
                    if !upload {
                        t = 1.0 - t;
                    }
                    let point = cubic_point(arc.0, arc.1, arc.2, t);
                    out.push(Draw::Rect {
                        x: point.0 - 2.0,
                        y: point.1 - 2.0,
                        w: 4.0,
                        h: 4.0,
                        colour: [color.0, color.1, color.2, 1.0],
                    });
                }
            }
        }

        if let Some(dir) = direction.filter(|_| transport && !noop) {
            let arrow = match dir {
                Direction::Upload => art.up,
                Direction::Download => art.down,
            };
            let (aw, ah) = (arrow.w as f32, arrow.h as f32);
            out.push(Draw::Tex {
                x: 455.0 - aw / 2.0,
                y: 224.0 - ah / 2.0,
                w: aw,
                h: ah,
                tex: arrow.tex,
                alpha: 0.85,
            });
            let label = match dir {
                Direction::Upload => faces.upload_label,
                Direction::Download => faces.download_label,
            };
            if let Some(label) = label.filter(|face| face.w > 0 && face.h > 0) {
                out.push(Draw::Tex {
                    x: 405.0 - label.w as f32 / 2.0,
                    y: 292.0,
                    w: label.w as f32,
                    h: label.h as f32,
                    tex: label.tex,
                    alpha: 0.9,
                });
            }
        }

        // One generic label replaces the STATE / SAV chips.  The two data
        // arcs stay, but the animation no longer names internal units.
        if let Some(label) = faces.data_label.filter(|face| face.w > 0 && face.h > 0) {
            out.push(Draw::Rect {
                x: 350.0,
                y: 236.0,
                w: 110.0,
                h: 30.0,
                colour: [0.04, 0.10, 0.13, 0.95],
            });
            out.push(Draw::Rect {
                x: 350.0,
                y: 236.0,
                w: 110.0,
                h: 1.0,
                colour: [0.21, 0.56, 0.62, 0.7],
            });
            out.push(Draw::Tex {
                x: 405.0 - label.w as f32 / 2.0,
                y: 251.0 - label.h as f32 / 2.0,
                w: label.w as f32,
                h: label.h as f32,
                tex: label.tex,
                alpha: 0.95,
            });
        }

        if settled && !noop {
            if let Some(check) = faces.check.filter(|face| face.w > 0 && face.h > 0) {
                out.push(Draw::Rect {
                    x: 381.0,
                    y: 228.0,
                    w: 48.0,
                    h: 48.0,
                    colour: [0.02, 0.10, 0.13, 0.95],
                });
                out.push(Draw::Tex {
                    x: 405.0 - check.w as f32 / 2.0,
                    y: 252.0 - check.h as f32 / 2.0,
                    w: check.w as f32,
                    h: check.h as f32,
                    tex: check.tex,
                    alpha: 1.0,
                });
            }
        }

        // Bottom progress bar: cloud follows the five stages; a mod that
        // reports mods.progress fills the same bar with its real value.
        let bar_x = 96.0;
        let bar_y = 414.0;
        let bar_w = 528.0;
        let bar_h = 10.0;
        if let Some(left) = faces.progress_left.filter(|face| face.w > 0 && face.h > 0) {
            out.push(Draw::Tex {
                x: bar_x,
                y: bar_y - 24.0,
                w: left.w as f32,
                h: left.h as f32,
                tex: left.tex,
                alpha: 0.95,
            });
        }
        if let Some(right) = faces.progress_right.filter(|face| face.w > 0 && face.h > 0) {
            out.push(Draw::Tex {
                x: bar_x + bar_w - right.w as f32,
                y: bar_y - 24.0,
                w: right.w as f32,
                h: right.h as f32,
                tex: right.tex,
                alpha: 0.95,
            });
        }
        out.push(Draw::Rect {
            x: bar_x,
            y: bar_y,
            w: bar_w,
            h: bar_h,
            colour: [0.086, 0.153, 0.180, 0.96],
        });
        let filled = if let Some(value) = progress {
            Some(value.clamp(0.0, 1.0))
        } else if settled {
            Some(1.0)
        } else {
            match stage {
                Some(SyncStage::Net) => Some(0.05),
                Some(SyncStage::Connect) => Some(0.18),
                Some(SyncStage::Upload | SyncStage::Download) => Some(0.60),
                Some(SyncStage::Verify) => Some(0.85),
                Some(SyncStage::Commit) => Some(0.95),
                _ => None,
            }
        };
        if let Some(value) = filled {
            if value > 0.0 {
                out.push(Draw::Rect {
                    x: bar_x,
                    y: bar_y,
                    w: bar_w * value,
                    h: bar_h,
                    colour: [0.337, 0.886, 0.961, 1.0],
                });
                out.push(Draw::Rect {
                    x: bar_x + 4.0,
                    y: bar_y + 2.0,
                    w: (bar_w * value - 8.0).max(0.0),
                    h: 1.0,
                    colour: [0.631, 0.957, 1.0, 0.8],
                });
            }
        } else {
            // Mod did not report progress: a moving 28% sweep, no fake value.
            let u = (self.now().saturating_sub(started) as f32 / 1400.0).rem_euclid(1.0);
            let segment = bar_w * 0.28;
            let sx = bar_x + (bar_w - segment) * u;
            out.push(Draw::Rect {
                x: sx,
                y: bar_y,
                w: segment,
                h: bar_h,
                colour: [0.337, 0.886, 0.961, 0.9],
            });
        }
        for tick in [0.25, 0.5, 0.75] {
            out.push(Draw::Rect {
                x: bar_x + bar_w * tick,
                y: bar_y - 2.0,
                w: 1.0,
                h: bar_h + 4.0,
                colour: [0.15, 0.28, 0.32, 1.0],
            });
        }
        if let Some(hint) = faces.cancel_hint.filter(|face| face.w > 0 && face.h > 0) {
            out.push(Draw::Tex {
                x: OUT_W as f32 - 36.0 - hint.w as f32,
                y: 438.0,
                w: hint.w as f32,
                h: hint.h as f32,
                tex: hint.tex,
                alpha: 0.9,
            });
        }

        // The animation's stage line sits under the title; the result banner
        // keeps the old bottom placement.
        self.draw_sync_banner_at(80.0, out);
        self.draw_sync_open_fade(out);
        if settled {
            self.draw_sync_close_fade(out);
        }
    }

    /// The rectangle the sync screen reveals, in output pixels.
    ///
    /// It borrows the cartridge power-on curves verbatim (`screen_width` widens
    /// a 2 px line first, `screen_scale` opens it up and down), so a network
    /// cartridge and a game cartridge power up with the same gesture.
    fn sync_open_rect(t: f32) -> (f32, f32, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let w = OUT_W as f32 * slot_gfx::screen_width(t);
    let h = OUT_H as f32 * slot_gfx::screen_scale(t);
    ((OUT_W as f32 - w) / 2.0, (OUT_H as f32 - h) / 2.0, w, h)
}

    /// The sync screen opening over the shelf, with the same grammar as a
    /// cartridge powering a game on: a 2 px line widens first, then the picture
    /// splits open from the middle up and down, with the same brightness flash.
    ///
    /// `t` is linear progress, exactly like the game's `screen` value; the
    /// curves live in [`slot_gfx::power`] and are shared, not re-derived here.
    fn draw_sync_window(&self, t: f32, out: &mut Vec<Draw>) {
        let t = t.clamp(0.0, 1.0);
        if t >= 0.999 {
            return;
        }
        let (x, y, w, h) = Self::sync_open_rect(t);
        let black = [0.0, 0.0, 0.0, 1.0];
        out.push(Draw::Rect { x: 0.0, y: 0.0, w: OUT_W as f32, h: y, colour: black });
        out.push(Draw::Rect { x: 0.0, y: y + h, w: OUT_W as f32, h: OUT_H as f32 - y - h, colour: black });
        out.push(Draw::Rect { x: 0.0, y, w: x, h, colour: black });
        out.push(Draw::Rect { x: x + w, y, w: OUT_W as f32 - x - w, h, colour: black });
        // The picture comes up bright and settles, the way a CRT does.
        let flash = (slot_gfx::screen_brightness(t) - 1.0).max(0.0);
        if flash > 0.001 && w >= 1.0 && h >= 1.0 {
            out.push(Draw::Rect {
                x,
                y,
                w,
                h,
                colour: [1.0, 1.0, 1.0, flash * 0.25],
            });
        }
    }

    fn draw_sync_open_fade(&self, out: &mut Vec<Draw>) {
        let Some(started) = self.sync_open_at else {
            return;
        };
        let t = (self.now().saturating_sub(started) as f32 / (SYNC_FADE_S * 1000.0))
            .clamp(0.0, 1.0);
        self.draw_sync_window(t, out);
    }

    fn draw_sync_close_fade(&self, out: &mut Vec<Draw>) {
        let Some(started) = self.sync_close_at else {
            return;
        };
        let elapsed = self.now().saturating_sub(started) as f32 / 1000.0;
        // The shrink is the game's power-off beat, not the opening one: a
        // cartridge screen breathes out faster than it comes up.
        let fade_start = (SYNC_SETTLE_MS as f32 / 1000.0 - POWER_OFF_S).max(0.0);
        let t = ((elapsed - fade_start) / POWER_OFF_S).clamp(0.0, 1.0);
        self.draw_sync_window(1.0 - t, out);
    }

    fn draw_sync_result_fade(&self, out: &mut Vec<Draw>) {
        // After a network cartridge has ejected, the shelf is already visible;
        // do not flash it back to black.
        if self.sync_card.is_some() {
            return;
        }
        let Some(started) = self.sync_result_at else {
            return;
        };
        let t = (self.now().saturating_sub(started) as f32 / (SYNC_FADE_S * 1000.0))
            .clamp(0.0, 1.0);
        let alpha = 1.0 - ease(t);
        if alpha > 0.001 {
            out.push(Draw::Rect {
                x: 0.0,
                y: 0.0,
                w: OUT_W as f32,
                h: OUT_H as f32,
                colour: [0.0, 0.0, 0.0, alpha],
            });
        }
    }

    fn draw_sync_banner(&self, out: &mut Vec<Draw>) {
        let Some(face) = self.sync_faces.banner else {
            return;
        };
        let y = (OUT_H as f32 - 58.0 - 12.0 - face.h as f32).round();
        self.draw_sync_banner_at(y, out);
    }

    fn draw_sync_banner_at(&self, y: f32, out: &mut Vec<Draw>) {
        let Some(face) = self.sync_faces.banner else {
            return;
        };
        let (w, h) = (face.w as f32, face.h as f32);
        let x = ((OUT_W as f32 - w) / 2.0).round();
        out.push(Draw::Rect {
            x: x - 12.0,
            y: y - 5.0,
            w: w + 24.0,
            h: h + 10.0,
            colour: [0.0, 0.0, 0.0, 0.55],
        });
        out.push(Draw::Tex {
            x,
            y,
            w,
            h,
            tex: face.tex,
            alpha: 1.0,
        });
    }

    fn link_popup_input(&mut self, action: Action) {
        let Some(popup) = self.link_popup else {
            return;
        };
        let LinkPopup::EndLink = popup;
        match action {
            Action::GbaDown(Btn::A) => {
                self.link_popup = None;
                self.end_link_from_menu();
            }
            Action::GbaDown(Btn::B) | Action::GameMenu | Action::QuickMenu => {
                self.link_popup = None;
            }
            _ => {}
        }
    }

    fn game_menu_input(&mut self, action: Action) {
        let Some(menu) = self.game_menu else {
            return;
        };
        match menu {
            GameMenu::Pick(role) => match action {
                Action::GbaDown(Btn::Left) | Action::GbaDown(Btn::Right) => {
                    self.last_role = role.other();
                    self.game_menu = Some(GameMenu::Pick(role.other()));
                }
                Action::GbaDown(Btn::Select) => self.switch_hardware(),
                Action::GbaDown(Btn::A) => self.pick_link(role),
                Action::GbaDown(Btn::B) | Action::GameMenu => self.close_game_menu(),
                _ => {}
            },
            GameMenu::Working { .. } => {
                if action == Action::GbaDown(Btn::B) {
                    if let Some(starting) = &mut self.starting {
                        starting.starter.cancel();
                    }
                    if let Some(reload) = &mut self.reload {
                        reload.cancelled = true;
                    }
                }
            }
            GameMenu::Linked { opened: true, .. } => match action {
                Action::GbaDown(Btn::A) => self.end_link_from_menu(),
                Action::GbaDown(Btn::B) | Action::GameMenu => self.close_game_menu(),
                _ => {}
            },
            GameMenu::Linked { .. } => {}
            GameMenu::Failed { .. } => {
                if matches!(
                    action,
                    Action::GbaDown(Btn::A) | Action::GbaDown(Btn::B) | Action::GameMenu
                ) {
                    self.close_game_menu();
                }
            }
            GameMenu::Unplug { .. } => {}
        }
    }

    pub fn start_link(&mut self, starter: LinkStarter, client_id: u16) {
        self.start_link_from(starter, client_id, self.now());
    }

    fn start_link_from(&mut self, starter: LinkStarter, client_id: u16, since: Millis) {
        crate::link_log::debug(format!(
            "begin client_id={client_id} role={:?} since={since}",
            LinkRow::from_client_id(client_id)
        ));
        self.link_popup = None;
        self.wifi_activity_started();
        // A host is a host the moment it asks to create the session, not only
        // once the first joiner's TCP handshake completes.  The listener may
        // still be warming up, but the session (and the hotspot) stays up while
        // the host plays the current game.
        if client_id == 0 && !self.link_active() {
            self.begin_link(0);
            self.link_paired_now(false);
        }
        if let Some(mut old) = self.starting.replace(LinkStarting { starter, client_id }) {
            crate::link_log::debug(format!(
                "replacing an unfinished link starter client_id={}",
                old.client_id
            ));
            old.starter.cancel();
        }
        self.game_menu = Some(GameMenu::Working {
            role: LinkRow::from_client_id(client_id),
            step: LinkStep::Radio,
            since,
        });
    }

    /// Keep a live host listening after the peer vanishes.  The session itself
    /// is never rebuilt: only the TCP listener is re-armed, so a later joiner
    /// finds the same host game.
    pub fn ensure_host_waiting(&mut self) {
        if self.link_client_id() != Some(0) || !self.link_active() || self.starting.is_some() {
            return;
        }
        crate::link_log::debug("host: re-arming the listener");
        self.starting = Some(LinkStarting {
            starter: LinkStarter::spawn(LinkRole::Host, link_port()),
            client_id: 0,
        });
    }

    fn switch_hardware(&mut self) {
        let Some(stem) = self.seated().map(str::to_string) else {
            return;
        };
        if !self.link_switchable(&stem) {
            return self.refuse();
        }
        let other = self.link_hardware.other();
        crate::link_log::debug(format!(
            "switch link hardware stem={stem:?} from={:?} to={other:?}",
            self.link_hardware
        ));
        self.link_hardware = other;
        self.link_choices.insert(stem, other);
    }

    fn link_switchable(&self, stem: &str) -> bool {
        // Only the Advance has two link hardwares to pick between. A Famicom
        // has neither, and a Game Boy's serial line is the cable whatever the
        // cart would name — so neither offers the choice.
        if self.platform != Platform::Gba {
            return false;
        }
        self.auto_link(stem).is_some_and(|(cart, auto)| {
            serial_option(self.link_hardware.other(), auto, &cart.code, &cart.title)
                != serial_option(self.link_hardware, auto, &cart.code, &cart.title)
        })
    }

    fn pick_link(&mut self, role: LinkRow) {
        self.wifi_activity_started();
        // The single-machine path (FC/SFC/MD) first: its link is a session.
        // There is no core option to seed and no serial line to name, so the
        // seated cart is not part of the decision at all. (The session is
        // opened from the in-game link menu; the shelf has no link entry any
        // more.)
        if self.single_machine_link() {
            self.link_player = Some(role.client_id() as u8);
            crate::link_log::debug(format!(
                "request role={role:?} single-machine session port={} seated={:?} wifi_connected={}",
                link_port(),
                self.seated(),
                self.wifi_connected
            ));
            return self.start_link(
                LinkStarter::spawn(role.role(), link_port()),
                role.client_id(),
            );
        }
        let Some(stem) = self.seated().map(str::to_string) else {
            // An mGBA session can open before any cart: the next insert opens
            // its core in link mode, so there is nothing to reload now. gpSP
            // needs a cart first, because its serial option is per game.
            if self.core == Core::Mgba {
                self.link_player = Some(role.client_id() as u8);
                crate::link_log::debug(format!(
                    "request role={role:?} mGBA session with no cart port={}",
                    link_port()
                ));
                return self.start_link(
                    LinkStarter::spawn(role.role(), link_port()),
                    role.client_id(),
                );
            }
            return;
        };
        let loaded = self.link_loaded.unwrap_or("auto");
        let (kind, serial) = self.link_mode(&stem);
        crate::link_log::debug(format!(
            "request role={role:?} stem={stem:?} core={:?} kind={kind:?} serial={serial:?} loaded={loaded:?} port={} wifi_connected={}",
            self.core,
            link_port(),
            self.wifi_connected
        ));
        if self.core == Core::Mgba {
            let player = role.client_id() as u8;
            if self.link_player == Some(player) {
                return self.start_link(
                    LinkStarter::spawn(role.role(), link_port()),
                    role.client_id(),
                );
            }
            if self.snapshot.as_ref().is_some_and(|s| !s.resume_trusted()) {
                return self.refuse();
            }
            self.link_player = Some(player);
            self.link_reload = Some((stem.clone(), loaded));
            self.reload = Some(Reload {
                stem,
                role,
                cancelled: false,
                from: self.link_hardware,
                from_serial: loaded,
                fallback: false,
            });
            self.game_menu = Some(GameMenu::Working {
                role,
                step: LinkStep::Radio,
                since: self.now(),
            });
            return;
        }
        if serial == loaded {
            return self.start_link(
                LinkStarter::spawn(role.role(), link_port()),
                role.client_id(),
            );
        }
        if self.snapshot.as_ref().is_some_and(|s| !s.resume_trusted()) {
            return self.refuse();
        }
        self.link_reload = Some((stem.clone(), serial));
        self.reload = Some(Reload {
            stem,
            role,
            cancelled: false,
            from: self.link_hardware.other(),
            from_serial: loaded,
            fallback: false,
        });
        self.game_menu = Some(GameMenu::Working {
            role,
            step: LinkStep::Radio,
            since: self.now(),
        });
    }

    fn refuse_seated(&mut self) {
        self.close_game_menu();
        self.pending = None;
        let caught = self.seat();
        let cart = match &mut self.phase {
            Phase::Playing { cart } => Some(std::mem::take(cart)),
            Phase::Doze { cart } => {
                *cart = None;
                None
            }
            _ => None,
        };
        if let Some(cart) = cart {
            self.refuse_out(cart, caught);
        }
    }

    fn close_game_menu(&mut self) {
        if self.game_menu.is_some() || self.starting.is_some() {
            crate::link_log::debug(format!(
                "close game menu link_active={} starting={}",
                self.link_active(),
                self.starting.is_some()
            ));
        }
        self.game_menu = None;
        if let Some(mut starting) = self.starting.take() {
            starting.starter.cancel();
        }
        if !self.link_active() {
            self.radio.ask(RadioJob::Cool);
        }
        let uncollected =
            self.link_reload.is_some() && self.reload.as_ref().is_some_and(|r| !r.fallback);
        if uncollected {
            self.link_reload = None;
            self.reload = None;
        } else if let Some(reload) = &mut self.reload {
            reload.cancelled = true;
        }
        if !self.link_active() {
            self.wifi_activity_finished();
        }
    }

    fn working_role(&self, client_id: u16) -> (LinkRow, Millis) {
        match self.game_menu {
            Some(GameMenu::Working { role, since, .. }) => (role, since),
            _ => (LinkRow::from_client_id(client_id), self.now()),
        }
    }

    fn poll_link(&mut self) {
        if self.power_menu.is_some() {
            return;
        }
        let Some(mut starting) = self.starting.take() else {
            return;
        };
        match starting.starter.poll() {
            None => self.starting = Some(starting),
            Some(LinkProgress::At(step)) => {
                crate::link_log::debug(format!("progress step={step:?}"));
                if let Some(GameMenu::Working { role, since, .. }) = self.game_menu {
                    self.game_menu = Some(GameMenu::Working { role, step, since });
                }
                self.starting = Some(starting);
            }
            Some(LinkProgress::Ready(link)) => {
                crate::link_log::debug(format!(
                    "transport established client_id={}",
                    starting.client_id
                ));
                let host_ready = starting.client_id == 0 && self.link_active();
                let (role, worked) = self.working_role(starting.client_id);
                self.game_menu = Some(GameMenu::Linked {
                    role,
                    worked,
                    since: self.now(),
                    opened: false,
                });
                if host_ready {
                    self.link_reconnected();
                } else {
                    self.begin_link(starting.client_id);
                }
                self.link_transport = Some((starting.client_id, Box::new(link)));
            }
            Some(LinkProgress::Failed(LinkFail::Cancelled)) => {
                crate::link_log::debug("progress cancelled");
                self.game_menu = None;
            }
            Some(LinkProgress::Failed(fail)) => {
                crate::link_log::warn(format!(
                    "progress failed client_id={} fail={fail:?}",
                    starting.client_id
                ));
                if starting.client_id == 0 && self.link_active() {
                    // The host did not see a joiner (or the listener hit a
                    // transient radio error).  The session is still open, so
                    // keep the host badge and arm the next listener instead of
                    // showing a dead "failed" page.
                    let (role, worked) = self.working_role(0);
                    self.game_menu = Some(GameMenu::Linked {
                        role,
                        worked,
                        since: self.now(),
                        opened: false,
                    });
                    self.ensure_host_waiting();
                    return;
                }
                let (role, worked) = self.working_role(starting.client_id);
                self.game_menu = Some(GameMenu::Failed {
                    role,
                    fail,
                    worked,
                    since: self.now(),
                });
            }
        }
    }

    fn write_core(&self, core: Core) {
        let (Some(root), Some(cart)) = (
            self.root.clone(),
            self.shelf().carts.get(self.shelf().index),
        ) else {
            return;
        };
        if let Err(e) = slot_store::write_selected_core(&root, &cart.stem, core) {
            eprintln!("slot: core: could not write selected_core.txt: {e}");
        }
    }

    fn begin_power_off(&mut self) {
        if self.powering_off {
            return;
        }
        self.remember_shelf();
        self.flush_playtime_now();
        self.cancel_shelf_sync();
        self.link_popup = None;
        if self.transfer.is_some() {
            self.close_cart_transfer();
        }
        if self.link_active() {
            self.end_link();
        }
        self.close_game_menu();
        self.powering_off = true;
        self.act_at = self.now() + SHUTDOWN_SHOW_MS;
        self.set_led(LedState::Off);
    }

    pub fn on_battery(&mut self, b: Battery) {
        if b.percent > BATTERY_CRITICAL || self.powering_off {
            return;
        }
        if matches!(b.charge, Charge::Charging | Charge::Full) {
            return;
        }
        self.flush_resume();
        self.begin_power_off();
    }

    fn doze_expired(&self) -> bool {
        if self.link_active() {
            return false;
        }
        let (Phase::Doze { .. }, Some(_power)) = (&self.phase, &self.power) else {
            return false;
        };
        slot_store::cheats::lid_expired(self.state.lid_minutes, self.now().saturating_sub(self.dozed_at))
    }

    pub fn settle_saves(&mut self) {
        self.state_writer.flush();
        if let Some(h) = self.pending_save.take() {
            let _ = h.join();
        }
    }

    fn autosave(&mut self) {
        self.autosave_at = self.now() + AUTOSAVE_MS;
        self.settle_saves();
        let (Some(root), Some(snapshot), Some(cart)) = (&self.root, &self.snapshot, self.seated())
        else {
            return;
        };
        let Some(state) = snapshot.state() else {
            eprintln!("slot: autosave: the core gave up no state");
            return;
        };
        let (state, sav) = trusted_write(snapshot.as_ref(), state, "autosave");
        let state = self.resume_wanted(state);
        let thumb = self
            .resume_thumb_wanted(cart)
            .then(|| snapshot.thumb())
            .flatten();
        let (root, platform, core, cart) =
            (root.clone(), self.platform, self.core, cart.to_owned());
        let failing = self.card_failing.clone();
        let write = move || {
            let written = persist::flush_with_thumb(
                &root,
                platform,
                core,
                &cart,
                state.as_deref(),
                sav.as_deref(),
                thumb.as_deref(),
            );
            if let Err(e) = &written {
                eprintln!("slot: autosave: {e}");
            }
            failing.store(written.is_err(), Ordering::Relaxed);
        };
        match std::thread::Builder::new()
            .name("slot-autosave".into())
            .spawn(write)
        {
            Ok(h) => self.pending_save = Some(h),
            Err(e) => eprintln!("slot: autosave: no writer thread: {e}"),
        }
    }

    pub fn flush_resume(&mut self) {
        self.settle_saves();
        self.autosave_at = self.now() + AUTOSAVE_MS;
        let (Some(root), Some(snapshot), Some(cart)) = (&self.root, &self.snapshot, self.seated())
        else {
            return;
        };
        let Some(state) = snapshot.state() else {
            eprintln!("slot: flush: the core gave up no state");
            return;
        };
        let (state, sav) = trusted_write(snapshot.as_ref(), state, "flush");
        let state = self.resume_wanted(state);
        let thumb = self
            .resume_thumb_wanted(cart)
            .then(|| snapshot.thumb())
            .flatten();
        let written = persist::flush_with_thumb(
            root,
            self.platform,
            self.core,
            cart,
            state.as_deref(),
            sav.as_deref(),
            thumb.as_deref(),
        );
        if let Err(e) = &written {
            eprintln!("slot: flush: {e}");
        }
        self.card_failing.store(written.is_err(), Ordering::Relaxed);
    }

    fn ring(&self) -> Option<StateRing> {
        let (Some(root), Some(cart)) = (&self.root, self.seated()) else {
            return None;
        };
        Some(StateRing::new(root, self.platform, self.core, cart))
    }

    fn seated(&self) -> Option<&str> {
        match &self.phase {
            Phase::Playing { cart } | Phase::Polaroids { cart } => Some(cart),
            _ => None,
        }
    }

    fn entries(&self) -> Vec<StateEntry> {
        self.ring().and_then(|r| r.list().ok()).unwrap_or_default()
    }

    fn open_polaroids(&mut self) {
        if self.link_active() {
            return self.refuse();
        }
        let entries = self.entries();
        // An empty album still opens: it shows the local empty state.
        let Phase::Playing { cart } = &mut self.phase else {
            return;
        };
        let cart = std::mem::take(cart);
        let mut p = Polaroids::new(entries, self.language);
        p.set_undo(self.undo_label());
        self.polaroids = Some(p);
        self.phase = Phase::Polaroids { cart };
        self.push_hint_faces();
    }

    fn close_polaroids(&mut self) {
        let Phase::Polaroids { cart } = &mut self.phase else {
            return;
        };
        let cart = std::mem::take(cart);
        self.polaroids = None;
        self.phase = Phase::Playing { cart };
    }

    fn load_selected(&mut self) {
        if self.polaroids.as_ref().is_some_and(|p| p.is_empty()) {
            return;
        }
        let state = self
            .polaroids
            .as_ref()
            .and_then(|p| p.selected())
            .map(|e| e.state.clone());
        let refused = state.map(|state| self.load_file(&state)) == Some(false);
        if !refused {
            self.close_polaroids();
        }
    }

    fn delete_selected(&mut self) {
        let stamp = self
            .polaroids
            .as_ref()
            .and_then(|p| p.selected())
            .map(|e| e.stamp.clone());
        let (Some(stamp), Some(ring)) = (stamp, self.ring()) else {
            return;
        };
        if let Err(e) = ring.remove(&stamp) {
            eprintln!("slot: delete: {e}");
            return;
        }
        if self.undo_targets(&stamp) {
            self.pending = None;
        }
        let Some(p) = &mut self.polaroids else {
            return;
        };
        p.remove_selected();
        if p.is_empty() {
            self.close_polaroids();
        }
    }

    fn undo_targets(&self, stamp: &str) -> bool {
        match self.pending.as_ref() {
            Some((PendingUndo::Save { stamp: pending, .. }, _)) => pending == stamp,
            _ => false,
        }
    }

    fn load_newest(&mut self) {
        let Some(newest) = self.entries().first().map(|e| e.state.clone()) else {
            return self.refuse();
        };
        self.load_file(&newest);
    }

    pub fn load_file(&mut self, state: &Path) -> bool {
        if !self.may_load_state() {
            self.refuse();
            return false;
        }
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        let bytes = match std::fs::read(state) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("slot: load: {e}");
                return false;
            }
        };
        let prior = snapshot.state();
        snapshot.load(bytes);
        self.hud.toast(Toast::StateLoaded, self.now());
        if let Some(prior) = prior {
            self.pending = Some((PendingUndo::Load { prior }, self.now()));
        }
        true
    }

    fn save_state(&mut self) {
        let (Some(ring), Some(snapshot)) = (self.ring(), &self.snapshot) else {
            return;
        };
        if !snapshot.resume_trusted() {
            eprintln!("slot: save: the core refused the resume it was given, not pushing a state");
            return self.refuse();
        }
        let Some(state) = snapshot.state() else {
            eprintln!("slot: save: the core gave up no state");
            return;
        };
        let thumb = snapshot.thumb().unwrap_or_default();
        let stamp = free_stamp(&ring, self.wall_secs());
        let evicted = doomed(&ring);
        if let Err(e) = ring.push(&state, &thumb, &stamp) {
            eprintln!("slot: save: {e}");
            return;
        }
        self.hud.toast(Toast::StateSaved, self.now());
        self.pending = Some((PendingUndo::Save { stamp, evicted }, self.now()));
    }

    pub fn undo_available(&self, now: Millis) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) <= UNDO_GRACE_MS)
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        let lang = self.language;
        if !self.undo_available(self.now()) {
            return None;
        }
        match self.pending.as_ref()?.0 {
            PendingUndo::Save { .. } => Some(slot_i18n::t(lang, "device.app.cheats.undo_save")),
            PendingUndo::Load { .. } => Some(slot_i18n::t(lang, "device.app.cheats.undo_load")),
        }
    }

    pub fn set_legend_faces(&mut self, faces: Vec<TexId>) {
        self.legend_faces = faces;
        self.push_hint_faces();
    }

    pub fn set_undo_face(&mut self, face: Option<TexId>) {
        self.undo_face = face;
        self.push_hint_faces();
    }

    fn push_hint_faces(&mut self) {
        let mut faces = self.legend_faces.clone();
        faces.extend(self.undo_face);
        if let Some(p) = &mut self.polaroids {
            p.set_hint_faces(faces);
        }
    }

    pub fn set_icon_faces(&mut self, faces: Vec<TexId>) {
        self.hud.set_icons(faces);
    }

    pub fn set_toast_faces(&mut self, faces: Vec<TexId>) {
        self.hud.set_toasts(faces);
    }

    pub fn toast(&self) -> Option<Toast> {
        self.hud.said(self.now())
    }

    pub fn card_alarm(&self) -> bool {
        self.hud.alarmed()
    }

    pub fn undo(&mut self, now: Millis) {
        if !self.undo_available(now) {
            self.pending = None;
            return;
        }
        if matches!(&self.pending, Some((PendingUndo::Load { .. }, _))) && !self.may_load_state() {
            return self.refuse();
        }
        let Some((what, _)) = self.pending.take() else {
            return;
        };
        match what {
            PendingUndo::Save { stamp, evicted } => self.undo_save(&stamp, evicted),
            PendingUndo::Load { prior } => {
                if let Some(snapshot) = &self.snapshot {
                    snapshot.load(prior);
                }
            }
        }
        self.close_polaroids();
    }

    fn undo_save(&self, stamp: &str, evicted: Option<(String, Vec<u8>, Vec<u8>)>) {
        let Some(ring) = self.ring() else {
            return;
        };
        if let Err(e) = ring.remove(stamp) {
            eprintln!("slot: undo: {e}");
            return;
        }
        let Some((stamp, state, thumb)) = evicted else {
            return;
        };
        if let Err(e) = ring.push(&state, &thumb, &stamp) {
            eprintln!("slot: undo: {e}");
        }
    }

    pub fn polaroid_entries(&self) -> &[StateEntry] {
        match &self.polaroids {
            Some(p) => &p.entries,
            None => &[],
        }
    }

    pub fn set_polaroid_faces(&mut self, faces: Vec<TexId>) {
        if let Some(p) = &mut self.polaroids {
            p.set_faces(faces);
        }
    }

    pub fn polaroid_stamp(&self) -> Option<&str> {
        self.polaroids
            .as_ref()
            .and_then(|p| p.selected())
            .map(|e| e.stamp.as_str())
    }

    pub fn polaroid_title(&self, now: &str) -> String {
        self.polaroids
            .as_ref()
            .map_or_else(String::new, |p| p.title(now))
    }

    pub fn set_polaroid_title_face(&mut self, face: TexId) {
        if let Some(p) = &mut self.polaroids {
            p.set_title_face(Some(face));
        }
    }
}

/// One conflict line in the project-book format:
/// `云端 v7 · RG34XX-A · 10-22 21:03 · 1 个文件`.
fn version_info_line(side: &str, info: &VersionInfo, lang: Language) -> String {
    let mut parts = Vec::new();
    if let Some(version) = info.version {
        parts.push(format!("v{version}"));
    }
    if let Some(device) = &info.device {
        if !device.is_empty() {
            parts.push(device.clone());
        }
    }
    if let Some(updated) = &info.updated {
        let stamp = short_stamp(updated);
        if !stamp.is_empty() {
            parts.push(stamp);
        }
    }
    if let Some(files) = info.files {
        let files = files.to_string();
        parts.push(slot_i18n::t_args(
            lang,
            "device.app.sync.file_count",
            &[("files", files.as_str())],
        ));
    }
    if parts.is_empty() {
        slot_i18n::t(lang, "device.app.sync.side_unknown").to_string()
    } else {
        format!("{side} {}", parts.join(" · "))
    }
}

/// `2026-10-20T10:00:00Z` → `10-20 10:00` (the device shows local time strings
/// only; the CLI stamps are already the reader's timezone).
fn short_stamp(stamp: &str) -> String {
    let stamp = stamp.trim();
    let Some((date, time)) = stamp.split_once('T') else {
        return stamp.to_string();
    };
    let mut fields = date.split('-');
    let (Some(_year), Some(month), Some(day)) = (fields.next(), fields.next(), fields.next()) else {
        return stamp.to_string();
    };
    let hhmm = time.get(0..5).unwrap_or(time);
    format!("{month}-{day} {hhmm}")
}

/// The success banner for one or two units (appendix A:
/// `已同步：即时 v13 / 电池 v9`).
fn success_text(outcome: &SyncOutcome, lang: Language) -> String {
    let mut parts = Vec::new();
    for kind in UnitKind::ALL {
        if let Some(unit) = outcome.unit(kind) {
            if matches!(unit.result, Some(SyncResult::Ok | SyncResult::Noop)) {
                if let Some(version) = unit.version {
                    parts.push(format!("{} v{version}", unit_short(lang, kind)));
                }
            }
        }
    }
    if parts.is_empty() {
        outcome.banner_text()
    } else {
        let items = parts.join(" / ");
        slot_i18n::t_args(
            lang,
            "device.app.sync.done_items",
            &[("items", items.as_str())],
        )
    }
}

/// `即时存档已同步，电池存档失败：<原因>` (appendix A).  Falls back to the
/// CLI's own detail when the two unit results are not both available.
fn partial_text(outcome: &SyncOutcome, lang: Language) -> String {
    let ok = UnitKind::ALL
        .into_iter()
        .find(|kind| matches!(outcome.unit_result(*kind), Some(SyncResult::Ok | SyncResult::Noop)));
    let failed = UnitKind::ALL.into_iter().find(|kind| {
        outcome.unit(*kind).is_some()
            && !matches!(
                outcome.unit_result(*kind),
                Some(SyncResult::Ok | SyncResult::Noop)
            )
    });
    match (ok, failed) {
        (Some(ok), Some(failed)) => {
            let reason = outcome
                .unit(failed)
                .and_then(|unit| unit.error.as_deref())
                .map(|error| unit_error_text(error, lang))
                .or_else(|| outcome.detail.clone())
                .unwrap_or_else(|| slot_i18n::t(lang, "device.app.common.unknown_reason").to_string());
            slot_i18n::t_args(
                lang,
                "device.app.sync.result",
                &[
                    ("ok", unit_label(lang, ok)),
                    ("failed", unit_label(lang, failed)),
                    ("reason", reason.as_str()),
                ],
            )
        }
        _ => outcome
            .detail
            .clone()
            .unwrap_or_else(|| outcome.banner_text()),
    }
}

fn unit_label(lang: Language, kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::State => slot_i18n::t(lang, "device.app.sync.unit_state"),
        UnitKind::Sav => slot_i18n::t(lang, "device.app.sync.unit_sav"),
    }
}

fn unit_short(lang: Language, kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::State => slot_i18n::t(lang, "device.app.sync.unit_state_short"),
        UnitKind::Sav => slot_i18n::t(lang, "device.app.sync.unit_sav_short"),
    }
}

/// A readable failure line for the conflict page.
fn failure_text(outcome: &SyncOutcome, lang: Language) -> String {
    outcome
        .detail
        .clone()
        .or_else(|| outcome.error_line().map(str::to_string))
        .unwrap_or_else(|| slot_i18n::t(lang, "device.app.sync.failed").to_string())
}

/// Chinese wording for a per-unit `LINKSLOT_*_ERROR` category.
fn unit_error_text(error: &str, lang: Language) -> String {
    match error {
        "net" => slot_i18n::t(lang, "device.app.sync.offline").to_string(),
        "auth" => slot_i18n::t(lang, "device.app.error.auth").to_string(),
        "cert" => slot_i18n::t(lang, "device.app.error.certificate").to_string(),
        "clock" => slot_i18n::t(lang, "device.app.error.clock").to_string(),
        "server" => slot_i18n::t(lang, "device.app.error.server").to_string(),
        "config" => slot_i18n::t(lang, "device.app.error.config").to_string(),
        "fs" => slot_i18n::t(lang, "device.app.error.card_io").to_string(),
        "verify" => slot_i18n::t(lang, "device.app.error.checksum").to_string(),
        "timeout" => slot_i18n::t(lang, "device.app.sync.timeout").to_string(),
        "busy" => slot_i18n::t(lang, "device.app.error.busy").to_string(),
        "changed-since" => slot_i18n::t(lang, "device.app.error.cloud_moved").to_string(),
        other => other.to_string(),
    }
}

#[allow(dead_code)]
fn outline(out: &mut Vec<Draw>, rect: SyncRect, t: f32, colour: [f32; 4], alpha: f32) {
    let (x, y, w, h) = rect;
    let colour = with_alpha(colour, alpha);
    out.push(Draw::Rect {
        x,
        y,
        w,
        h: t,
        colour,
    });
    out.push(Draw::Rect {
        x,
        y: y + h - t,
        w,
        h: t,
        colour,
    });
    out.push(Draw::Rect {
        x,
        y,
        w: t,
        h,
        colour,
    });
    out.push(Draw::Rect {
        x: x + w - t,
        y,
        w: t,
        h,
        colour,
    });
}

fn keyboard_push(kb: &mut KeyboardState, c: char, lang: Language) {
    if c.is_control() {
        return;
    }
    match &kb.purpose {
        KeyboardPurpose::WifiSsid => {
            if kb.text.len() + c.len_utf8() > 32 {
                kb.error = Some(slot_i18n::t(lang, "device.app.wifi.ssid_max").into());
                return;
            }
        }
        KeyboardPurpose::WifiPassword { .. } => {
            if kb.text.chars().count() >= 64 {
                kb.error = Some(slot_i18n::t(lang, "device.app.wifi.password_long").into());
                return;
            }
        }
        KeyboardPurpose::GameSearch => {
            search_push(kb, c);
            return;
        }
    }
    kb.text.push(c);
    kb.error = None;
}

fn search_keyboard_hint(lang: Language) -> String {
    slot_i18n::t(lang, "device.app.search.hint").into()
}

fn search_candidate_hint(lang: Language) -> String {
    slot_i18n::t(lang, "device.app.search.grid_hint").into()
}

fn search_push(kb: &mut KeyboardState, c: char) {
    if kb.composition.chars().count() >= 64 {
        return;
    }
    if c.is_ascii_alphanumeric() || c == ' ' {
        kb.composition.push(c.to_ascii_lowercase());
        kb.error = None;
        kb.candidate_page = 0;
        kb.candidate_focus = false;
    }
}

fn search_backspace(kb: &mut KeyboardState) {
    if !kb.composition.is_empty() {
        kb.composition.pop();
    } else {
        kb.text.pop();
    }
    kb.candidate_page = 0;
    kb.candidate_focus = false;
    kb.error = None;
}

fn refresh_search_candidates(
    kb: &mut KeyboardState,
    index: &crate::game_search::GameSearchIndex,
    lang: Language,
) {
    if kb.composition.is_empty() {
        kb.candidates.clear();
        kb.candidate_page = 0;
        kb.candidate_pages = 1;
        kb.candidate_selected = 0;
        kb.candidate_focus = false;
        kb.hint = search_keyboard_hint(lang);
        return;
    }
    let page = index.candidates(&kb.composition, kb.candidate_page, 8);
    kb.candidates = page.items;
    kb.candidate_page = page.page;
    kb.candidate_pages = page.pages;
    kb.candidate_selected = kb.candidate_selected.min(kb.candidates.len().saturating_sub(1));
    kb.error = None;
}

fn commit_search_candidate(kb: &mut KeyboardState, lang: Language) {
    let Some(candidate) = kb.candidates.get(kb.candidate_selected).cloned() else {
        return;
    };
    kb.text.push_str(&candidate);
    kb.composition.clear();
    kb.candidates.clear();
    kb.candidate_page = 0;
    kb.candidate_pages = 1;
    kb.candidate_selected = 0;
    kb.candidate_focus = false;
    kb.error = None;
    kb.hint = search_keyboard_hint(lang);
}

fn keep_search_result_visible(results: &mut SearchResults) {
    if results.selected < results.offset {
        results.offset = results.selected;
    }
    if results.selected >= results.offset + SEARCH_ROWS {
        results.offset = results.selected + 1 - SEARCH_ROWS;
    }
}

fn with_alpha(colour: [f32; 4], alpha: f32) -> [f32; 4] {
    [colour[0], colour[1], colour[2], colour[3] * alpha]
}

/// 0 → 1 → 0 every `period` ms: a cheap pulse that needs no trig.
fn sync_stage_step(stage: Option<SyncStage>) -> Option<u8> {
    match stage {
        Some(SyncStage::Net) => Some(1),
        Some(SyncStage::Connect) => Some(2),
        Some(SyncStage::Upload | SyncStage::Download) => Some(3),
        Some(SyncStage::Verify) => Some(4),
        Some(SyncStage::Commit) => Some(5),
        _ => None,
    }
}

fn triangle(elapsed: Millis, period: Millis) -> f32 {
    let period = period.max(1);
    let u = (elapsed % period) as f32 / period as f32;
    1.0 - (2.0 * u - 1.0).abs()
}

/// One point on a quadratic data arc, in screen pixels.
fn cubic_point(
    start: (f32, f32),
    control: (f32, f32),
    end: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * start.0 + 2.0 * u * t * control.0 + t * t * end.0,
        u * u * start.1 + 2.0 * u * t * control.1 + t * t * end.1,
    )
}

fn trusted_write(
    snapshot: &dyn Snapshot,
    state: Vec<u8>,
    verb: &str,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let state = if snapshot.resume_trusted() {
        Some(state)
    } else {
        eprintln!(
            "slot: {verb}: the core refused the resume it was given, not overwriting the saved one"
        );
        None
    };
    let sav = snapshot.save_ram();
    let sav = if snapshot.save_ram_trusted() {
        sav
    } else {
        if sav.is_some() {
            eprintln!(
                "slot: {verb}: the core refused the save ram it was given, not overwriting the saved one"
            );
        }
        None
    };
    (state, sav)
}

fn up(level: u8, step: u8, max: u8) -> u8 {
    level.saturating_add(step).min(max)
}

fn saved_wifi_rows(saved: &[crate::linkcfg::SavedWifi]) -> Vec<slot_ui::network::WifiEntry> {
    saved
        .iter()
        .map(|network| slot_ui::network::WifiEntry {
            ssid: network.ssid.clone(),
            signal: 0,
            secured: network.password_set,
            saved: true,
        })
        .collect()
}

fn merge_saved_wifi_rows(
    saved: &[crate::linkcfg::SavedWifi],
    scanned: Vec<slot_ui::network::WifiEntry>,
) -> Vec<slot_ui::network::WifiEntry> {
    let mut rows = saved_wifi_rows(saved);
    for row in scanned {
        if let Some(existing) = rows.iter_mut().find(|saved| saved.ssid == row.ssid) {
            existing.signal = row.signal;
            existing.secured = row.secured;
            existing.saved = true;
        } else {
            rows.push(row);
        }
    }
    rows
}

fn btn_device(btn: Btn) -> Option<Device> {
    Some(match btn {
        Btn::Up => Device::Up,
        Btn::Down => Device::Down,
        Btn::Left => Device::Left,
        Btn::Right => Device::Right,
        Btn::A => Device::A,
        Btn::B => Device::B,
        Btn::X => Device::X,
        Btn::Y => Device::Y,
        Btn::L1 => Device::L1,
        Btn::R1 => Device::R1,
        Btn::L2 => Device::L2,
        Btn::R2 => Device::R2,
        Btn::Select => Device::Select,
        Btn::Start => Device::Start,
        _ => return None,
    })
}

/// Every mask for one machine, by file name, sorted the way a person expects
/// (case-insensitive): `Overlay/FC/FC01.png`, `Overlay/GB/GB01.png`, ...
fn scan_masks(root: &std::path::Path, directory: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("Overlay").join(directory)) else {
        return Vec::new();
    };
    let mut masks: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.to_ascii_lowercase().ends_with(".png"))
        .collect();
    masks.sort_by_key(|name| name.to_ascii_lowercase());
    masks
}

/// Resolve the saved file name against the files currently on the card. The
/// legacy empty value means "on" and selects the first file.
fn resolve_mask_index(files: &[String], saved: Option<&str>) -> usize {
    match saved {
        None => 0,
        Some(name) if name.is_empty() => {
            if files.is_empty() {
                0
            } else {
                1
            }
        }
        Some(name) => files
            .iter()
            .position(|file| file.eq_ignore_ascii_case(name))
            .map_or(0, |at| at + 1),
    }
}
fn app_device_btn(device: Device) -> Btn {
    match device {
        Device::Up => Btn::Up,
        Device::Down => Btn::Down,
        Device::Left => Btn::Left,
        Device::Right => Btn::Right,
        Device::A => Btn::A,
        Device::B => Btn::B,
        Device::X => Btn::X,
        Device::Y => Btn::Y,
        Device::L1 => Btn::L1,
        Device::R1 => Btn::R1,
        Device::L2 => Btn::L2,
        Device::R2 => Btn::R2,
        Device::Select => Btn::Select,
        Device::Start => Btn::Start,
    }
}

fn shader_value(shader: Shader) -> QuickValue {    match shader {
        Shader::Off => QuickValue::Off,
        Shader::Lcd3x => QuickValue::Lcd3x,
        Shader::Grid => QuickValue::Grid,
        Shader::Dot => QuickValue::Dot,
        Shader::Simpletex => QuickValue::Simpletex,
        Shader::FourPass => QuickValue::FourPass,
    }
}

fn sync_mode_value(mode: linkcfg::SyncMode) -> QuickValue {
    match mode {
        linkcfg::SyncMode::All => QuickValue::SyncAll,
        linkcfg::SyncMode::State => QuickValue::SyncState,
        linkcfg::SyncMode::Sav => QuickValue::SyncSav,
    }
}

fn ff_next(from: u8, right: bool) -> u8 {
    let n = FF_SPEEDS.len();
    let at = FF_SPEEDS.iter().position(|&v| v == from).unwrap_or(0);
    FF_SPEEDS[if right {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    }]
}

fn clock_screen(utc: i64, offset_min: i16, from_menu: bool) -> Phase {
    Phase::SetClock {
        picker: ClockPicker::local(utc, offset_min),
        seed: utc - utc.rem_euclid(60),
        from_menu,
    }
}

fn decode_guide_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        // Guides made on Chinese Windows are commonly GBK.  GBK is a
        // superset of GB2312 and the decoder is lossy only for invalid bytes.
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    };
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "")
}

fn system_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn doomed(ring: &StateRing) -> Option<(String, Vec<u8>, Vec<u8>)> {
    let entries = ring.list().ok()?;
    let oldest = entries.get(RING_MAX - 1)?;
    let (state, thumb) = ring.read(&oldest.stamp).ok()?;
    Some((oldest.stamp.clone(), state, thumb))
}

fn free_stamp(ring: &StateRing, now: i64) -> String {
    let taken: Vec<String> = ring
        .list()
        .map(|l| l.into_iter().map(|e| e.stamp).collect())
        .unwrap_or_default();
    let mut secs = now;
    let mut stamp = format_stamp(secs);
    while taken.contains(&stamp) {
        secs += 1;
        stamp = format_stamp(secs);
    }
    stamp
}

fn switch_colour(c: [u8; 4]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        c[3] as f32 / 255.0,
    ]
}

#[cfg(test)]
mod web_refresh_tests {
    use super::*;

    #[test]
    fn gbk_guides_are_decoded_before_wrapping() {
        let (bytes, _, _) = encoding_rs::GBK.encode("中文攻略");
        assert_eq!(decode_guide_text(&bytes), "中文攻略");
    }

    #[test]
    fn a_txt_guide_opens_pages_and_returns_to_the_game_menu() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
        std::fs::create_dir_all(root.join("Guides/GBA")).unwrap();
        let rom = root.join("Games/GBA/Game.gba");
        std::fs::write(&rom, b"rom").unwrap();
        std::fs::write(
            root.join("Guides/GBA/Game.txt"),
            (0..40)
                .map(|line| format!("攻略第 {line} 行\n"))
                .collect::<String>(),
        )
        .unwrap();
        let carts = slot_store::scan_quiet(root).unwrap();
        let mut app = App::new(carts);
        app.root = Some(root.to_path_buf());
        app.state.cart = Some("Game".to_string());
        app.state.cart_platform = Some(Platform::Gba);
        app.phase = Phase::Playing {
            cart: "Game".to_string(),
        };

        app.open_guide();
        let (_, lines, page, pages) = app.guide_page().expect("guide opened");
        assert!(!lines.is_empty());
        assert_eq!(page, 0);
        assert!(pages > 1);

        app.guide_input(Action::GbaDown(Btn::Down));
        assert_eq!(app.guide_page().unwrap().2, 1);

        app.guide_input(Action::GbaDown(Btn::B));
        assert!(!app.guide_open());
        assert_eq!(app.utility, Some((0, 0)));
    }

    #[test]
    fn an_epub_guide_opens_through_the_same_reader() {
        use std::io::{Cursor, Write};

        let body = (0..40)
            .map(|line| format!("<p>第一章第 {line} 段</p>"))
            .collect::<String>();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in [
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#.to_string(),
            ),
            (
                "OEBPS/content.opf",
                r#"<?xml version="1.0"?><package><manifest><item id="one" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/></spine></package>"#.to_string(),
            ),
            (
                "OEBPS/chapter.xhtml",
                format!("<html><body><h1>第一章</h1>{body}<img src=\"pic.png\" /></body></html>"),
            ),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.start_file("OEBPS/pic.png", options).unwrap();
        zip.write_all(include_bytes!("../../slot-ui/assets/lettering_gba.png"))
            .unwrap();
        let epub = zip.finish().unwrap().into_inner();

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
        std::fs::create_dir_all(root.join("Guides/GBA")).unwrap();
        std::fs::write(root.join("Games/GBA/Game.gba"), b"rom").unwrap();
        std::fs::write(root.join("Guides/GBA/Game.epub"), epub).unwrap();
        let carts = slot_store::scan_quiet(root).unwrap();
        let mut app = App::new(carts);
        app.root = Some(root.to_path_buf());
        app.state.cart = Some("Game".to_string());
        app.state.cart_platform = Some(Platform::Gba);
        app.phase = Phase::Playing {
            cart: "Game".to_string(),
        };

        app.open_guide();
        let (_, lines, page, pages) = app.guide_page().expect("EPUB guide opened");
        assert!(lines.iter().any(|line| line.contains("第一章")));
        assert_eq!(page, 0);
        assert!(pages > 1);
        let mut found_image = false;
        for _ in 0..pages {
            if app.guide_image().is_some() {
                found_image = true;
                break;
            }
            app.guide_input(Action::GbaDown(Btn::Down));
        }
        assert!(found_image, "EPUB image page was not built");
    }

    #[test]
    fn a_web_cover_reaches_every_shelf_copy_of_the_cart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
        let rom = root.join("Games/GBA/宝可梦.gba");
        std::fs::write(&rom, b"rom").unwrap();
        let carts = slot_store::scan_quiet(root).unwrap();
        let mut app = App::new(carts);
        app.root = Some(root.to_path_buf());
        assert!(app
            .shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .all(|cart| cart.label.is_none()));

        std::fs::create_dir_all(root.join("Labels/GBA")).unwrap();
        let label = root.join("Labels/GBA/宝可梦.png");
        std::fs::write(&label, b"new label").unwrap();
        app.refresh_library_after_web();

        let copies: Vec<_> = app
            .shelves
            .iter()
            .flat_map(|(_, shelf)| shelf.carts.iter())
            .filter(|cart| cart.rom == rom)
            .collect();
        assert!(!copies.is_empty());
        for cart in copies {
            assert_eq!(cart.label.as_deref(), Some(label.as_path()));
        }
        assert_eq!(app.face_generation, 1);
    }
}

#[cfg(test)]
mod sync_stage_tests {
    use super::*;

    /// The sync stage slot is GBA-shaped; every cartridge has to keep its own
    /// aspect inside it, or an FC cassette looks stretched.
    #[test]
    fn every_cartridge_keeps_its_own_shape_on_the_sync_stage() {
        for (platform, aspect) in [
            (Platform::Gba, 360.0 / 202.0),
            (Platform::Gb, 240.0 / 276.0),
            (Platform::Gbc, 240.0 / 276.0),
            (Platform::Fc, 360.0 / 299.0),
        ] {
            let (x, y, w, h) = App::sync_stage_cart(aspect);
            assert!(
                (w / h - aspect).abs() < 0.01,
                "{platform:?} cart aspect on the sync stage is {}",
                w / h
            );
            assert!(
                (y + h - (212.0 + 104.0)).abs() < 0.5,
                "{platform:?} cart foot left the baseline"
            );
            assert!(x >= 84.0 && x + w <= 84.0 + 188.0 + 0.5, "{platform:?} cart left its slot");
        }
    }
}

#[cfg(test)]
mod sync_open_tests {
    use super::*;

    /// The first frames must be a bright line, not a small square: that is the
    /// whole difference the player sees between the two openings.
    #[test]
    fn the_sync_window_opens_like_a_cartridge_powering_a_game() {
        // A thin dash that keeps widening: at the very start it is a line, not
        // a small rectangle.
        let (_, _, early_w, early_h) = App::sync_open_rect(0.02);
        assert!(early_h < 30.0, "the picture opened too early: {early_h}");
        assert!(
            early_w > early_h * 3.0,
            "the first frame was not line-shaped: {early_w}x{early_h}"
        );

        let (x, y, mid_w, mid_h) = App::sync_open_rect(0.5);
        assert!(mid_w >= OUT_W as f32 - 1.0, "the width was still growing");
        assert!(mid_h > 200.0 && mid_h < OUT_H as f32, "the split stalled");
        assert!((x - 0.0).abs() < 1.0 && y > 0.0, "the split was not vertical");

        let (_, _, full_w, full_h) = App::sync_open_rect(1.0);
        assert_eq!((full_w, full_h), (OUT_W as f32, OUT_H as f32));
    }

    #[test]
    fn the_flash_settles_before_the_window_is_full() {
        assert!(slot_gfx::screen_brightness(0.0) > 1.4);
        assert_eq!(slot_gfx::screen_brightness(1.0), 1.0);
    }
}

/// The 系统 → 语言 row and the file behind it.
#[cfg(test)]
mod language_tests {
    use super::*;

    fn app_at(root: &Path) -> App {
        std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
        let carts = slot_store::scan_quiet(root).unwrap();
        let mut app = App::new(carts);
        app.root = Some(root.to_path_buf());
        app
    }

    #[test]
    fn the_language_row_switches_wraps_and_writes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        assert_eq!(app.language(), Language::ZhCn);
        assert_eq!(
            app.quick_value(QuickRow::Language),
            Some(QuickValue::LanguageZh)
        );

        app.change_setting(QuickRow::Language, true);
        assert_eq!(app.language(), Language::EnUs);
        assert_eq!(
            app.quick_value(QuickRow::Language),
            Some(QuickValue::LanguageEn)
        );
        assert_eq!(slot_store::read_language(dir.path()), Language::EnUs);

        // Right again wraps to the other language rather than sticking.
        app.change_setting(QuickRow::Language, true);
        assert_eq!(app.language(), Language::ZhCn);
        assert_eq!(slot_store::read_language(dir.path()), Language::ZhCn);

        app.change_setting(QuickRow::Language, false);
        assert_eq!(app.language(), Language::EnUs);
    }

    #[test]
    fn the_switch_is_announced_in_the_new_language() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        app.change_setting(QuickRow::Language, true);
        let (title, detail, _) = app.notice_view().expect("a notice confirms the change");
        assert_eq!(title, "Language");
        assert_eq!(detail, "Switched to English");
    }

    #[test]
    fn a_read_only_card_keeps_the_choice_in_memory_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        // A *directory* where the language file belongs makes the write fail.
        std::fs::create_dir_all(dir.path().join("linkSlot/Config/language.txt")).unwrap();

        app.change_setting(QuickRow::Language, true);
        assert_eq!(app.language(), Language::EnUs, "the menu still switches");
        assert_eq!(
            slot_store::read_language(dir.path()),
            Language::ZhCn,
            "nothing reached the card, so the next boot is Chinese again"
        );
        let (_, detail, _) = app.notice_view().expect("a notice explains it");
        assert!(detail.contains("Not saved"), "{detail}");
    }

    #[test]
    fn a_language_written_by_the_web_page_is_picked_up_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        assert_eq!(app.language(), Language::ZhCn);

        slot_store::write_language(dir.path(), Language::EnUs).unwrap();
        assert_eq!(app.language(), Language::ZhCn, "the device has not looked yet");

        app.clock = 5_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs);

        // A second poll with no further change must not thrash.
        app.clock = 6_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs);
    }

    #[test]
    fn the_poll_is_throttled_to_once_a_second() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        slot_store::write_language(dir.path(), Language::EnUs).unwrap();
        app.clock = 5_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs);

        // Back to Chinese on the card, but the poll refuses to look yet.
        slot_store::write_language(dir.path(), Language::ZhCn).unwrap();
        app.clock = 5_500.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs, "under a second: no look");
        app.clock = 6_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::ZhCn, "a second later: reloaded");
    }

    #[test]
    fn an_edit_that_keeps_the_timestamp_is_still_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        let path = dir.path().join("linkSlot/Config/language.txt");
        slot_store::write_language(dir.path(), Language::EnUs).unwrap();
        app.clock = 5_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs);

        // Rewrite the content and put the old stamp back.  This is exactly what
        // an exFAT/FAT32 card does when two writes land inside one tick, and it
        // is why the poll compares the code instead of the mtime.
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        slot_store::write_language(dir.path(), Language::ZhCn).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(stamp).unwrap();
        drop(file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            stamp,
            "the test needs the timestamp to actually stay put"
        );

        app.clock = 6_000.0;
        app.poll_language_file();
        assert_eq!(
            app.language(),
            Language::ZhCn,
            "a same-stamp edit must still change the menu"
        );
    }

    #[test]
    fn a_deleted_language_file_leaves_the_menu_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_at(dir.path());
        slot_store::write_language(dir.path(), Language::EnUs).unwrap();
        app.clock = 5_000.0;
        app.poll_language_file();
        assert_eq!(app.language(), Language::EnUs);

        std::fs::remove_file(dir.path().join("linkSlot/Config/language.txt")).unwrap();
        app.clock = 9_000.0;
        app.poll_language_file();
        assert_eq!(
            app.language(),
            Language::EnUs,
            "a card that lost the file must not flip the menu mid-session"
        );
    }
}
