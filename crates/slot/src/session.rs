use std::path::PathBuf;
use std::time::{Duration, Instant};

use slot_input::{Action, Btn, Gestures, Millis, RawEvent};
use slot_retro::{LinkChannel, Rumble, NETPACKET_RELIABLE};
use slot_store::{Controls, Platform, Shortcut, ShortcutKeys};
use slot_ui::FfState;

pub const RUMBLE_MIN_MS: Millis = 100;
pub const RUMBLE_MAX_MS: Millis = 150;

use crate::app::{App, FollowAnswer, Phase};
use crate::audio::{opened_sink, AudioSink, Ring, Sfx};
use crate::cable::{Hello, Pairing, HELLO_VERSION};
use crate::rom_id::Digest;
use crate::core::open_core;
use crate::emu::{CoreState, EmuHandle, Speed};
use crate::frames::FrameRef;
use crate::input::Pad;
use crate::persist;
use crate::prefetch::Prefetch;

/// A re-attach of the same cart within this window is a host restart.
const RESTART_NOTE: Duration = Duration::from_secs(20);

pub struct Session {
    root: PathBuf,
    app: App,
    emu: Option<EmuHandle>,
    /// The open Famicom link between games: the socket belongs to the session
    /// and a running game borrows it. `None` while a game holds it.
    link_wire: Option<Box<dyn LinkChannel>>,
    /// A cable is attached to the game that is running.
    link_attached: bool,
    /// The last last-run.txt body written, so it is only touched on change.
    run_marker: Option<String>,
    /// Waiting for both ends to be restored before the first locked frame.
    link_handshake: Option<Instant>,
    /// The game the other side announced, and the one we announced last.
    peer_game: Option<crate::cable::GameStart>,
    announced: Option<Digest>,
    /// Keepalive: when anything was last heard, and when we last pinged.
    last_heard: Instant,
    pinged: Instant,
    /// When the session took the socket back, so an older peer that never
    /// announces still gets the old "attach and let the hello decide" path.
    wire_since: Option<Instant>,
    /// A cart this side already said no to (or could not find), so the peer's
    /// repeated announce does not reopen the question every frame.
    refused_rom: Option<Digest>,
    /// The last game that could not link, so the hint is said once per cart.
    solo_noted: Option<String>,
    /// The game this host is waiting for a peer to join, so the hint is said
    /// once rather than every frame.
    waiting_noted: Option<Digest>,
    /// The pause reason last logged, so sync_speed only speaks on change.
    speed_note: Option<&'static str>,
    /// Whether the peer has ever spoken on this socket: a session from this
    /// version pings and announces, an old one says nothing at all.
    peer_alive: bool,
    /// The game we announced last, so a new game restarts the grace period.
    game_seen: Option<Digest>,
    /// The host sends its clock once per socket and once per attach.
    time_sent: bool,
    /// A cart the player agreed to switch to: stop asking until it lands.
    following: Option<Digest>,
    /// The rom the peer last stopped, and when: a quick re-attach of the same
    /// cart means the host restarted it, which the joiner is told about.
    ended_rom: Option<Digest>,
    ended_at: Option<Instant>,
    /// Cable frames that arrived before the game attached: the hand-over to
    /// the cable must replay them, or a hello sent a moment early is lost and
    /// a Famicom/SFC/MD pair never starts.
    pending_wire: Vec<Vec<u8>>,
    /// When the cable was handed the socket, so a hello that never arrives can
    /// time out and retry instead of freezing both ends.
    attached_at: Option<Instant>,
    sink: Box<dyn AudioSink>,
    gestures: Gestures,
    pad: Pad,
    remap: crate::input::remap::Remap,
    controls: Controls,
    shortcuts: Option<ShortcutKeys>,
    select_down: bool,
    menu_down: bool,
    pressed: Vec<Btn>,
    custom_held: Vec<(Btn, Action)>,
    rewinding: bool,
    fast: bool,
    motor: u16,
    pulse: Option<(Millis, u16)>,
    prefetch: Prefetch,
    /// A requested in-place restart has dropped the old core and is waiting for
    /// the replacement to finish loading.
    restart_in_place: bool,
    reloading: bool,
    driven: bool,
}

impl Session {
    pub fn boot(root: PathBuf) -> Self {
        Self::boot_with(root, opened_sink())
    }

    pub fn boot_with(root: PathBuf, sink: Box<dyn AudioSink>) -> Self {
        let controls = slot_store::read_controls(&root);
        let shortcuts = root
            .join("linkSlot/Config/shortcuts.txt")
            .is_file()
            .then(|| slot_store::read_shortcuts(&root));
        let mut app = App::boot(&root);
        app.start_rom_index();
        Session {
            app,
            root,
            emu: None,
            link_wire: None,
            link_attached: false,
            link_handshake: None,
            run_marker: None,
            peer_game: None,
            announced: None,
            last_heard: Instant::now(),
            pinged: Instant::now(),
            wire_since: None,
            refused_rom: None,
            solo_noted: None,
            waiting_noted: None,
            speed_note: None,
            peer_alive: false,
            game_seen: None,
            time_sent: false,
            following: None,
            ended_rom: None,
            ended_at: None,
            pending_wire: Vec::new(),
            attached_at: None,
            sink,
            gestures: Gestures::new(),
            pad: Pad::default(),
            remap: crate::input::remap::Remap::default(),
            controls,
            shortcuts,
            select_down: false,
            menu_down: false,
            pressed: Vec::new(),
            custom_held: Vec::new(),
            rewinding: false,
            fast: false,
            motor: 0,
            pulse: None,
            prefetch: Prefetch::default(),
            restart_in_place: false,
            reloading: false,
            driven: false,
        }
    }

    pub fn play_sfx(&mut self, sfx: Sfx) {
        let ring = self.sink.ring();
        let rate = ring.sample_rate();
        if rate == 0 {
            return;
        }
        let mut samples = sfx.render(rate);
        crate::audio::volume::apply(&mut samples, self.app.output_volume());
        ring.mix(&samples);
    }

    pub fn audio_queued(&self) -> usize {
        self.sink.ring().queued_frames()
    }

    pub fn audio_ring(&self) -> std::sync::Arc<Ring> {
        self.sink.ring()
    }

    pub fn rumble(&mut self, strength: u16) {
        if strength == self.motor {
            return;
        }
        self.motor = strength;
        self.app.set_rumble(strength);
    }

    pub fn prefetching(&self) -> Option<&std::path::Path> {
        self.prefetch.reading()
    }

    pub fn core_rumble(&self) -> Option<&Rumble> {
        self.emu.as_ref().map(EmuHandle::rumble)
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    pub fn emu(&self) -> Option<&EmuHandle> {
        self.emu.as_ref()
    }

    pub fn set_driven(&mut self, driven: bool) {
        self.driven = driven;
        if let Some(emu) = &self.emu {
            emu.set_driven(driven);
        }
    }

    pub fn step_emulator(&self, present: Duration, timeout: Duration) -> bool {
        self.emu.as_ref().is_some_and(|emu| {
            emu.tick(present);
            emu.wait_frame(timeout)
        })
    }

    pub fn frame(&self) -> Option<FrameRef> {
        self.emu.as_ref().and_then(|e| e.latest_frame())
    }

    pub fn core_settling(&self) -> bool {
        self.emu
            .as_ref()
            .is_some_and(|e| e.state() == CoreState::Loading)
    }

    pub fn has_core(&self) -> bool {
        self.emu.is_some()
    }

    pub fn frame_ready(&self) -> bool {
        self.emu.as_ref().is_some_and(EmuHandle::frame_ready)
    }

    pub fn frames_published(&self) -> u64 {
        self.emu.as_ref().map_or(0, EmuHandle::published_count)
    }

    pub fn observed_speed(&self) -> Option<Speed> {
        self.emu.as_ref().map(EmuHandle::observed_speed)
    }

    pub fn frames_taken(&self) -> u64 {
        self.emu.as_ref().map_or(0, EmuHandle::frames_taken)
    }

    pub fn game_visible(&self) -> bool {
        self.app.game_visible()
    }

    pub fn feed(&mut self, events: impl IntoIterator<Item = RawEvent>, now: Millis) {
        self.app.set_select_held(self.select_down);
        let mut actions = Vec::new();
        for ev in events {
            if let RawEvent::Down(Btn::Select) = ev {
                self.app.set_select_held(true);
            }
            if let RawEvent::Up(Btn::Select) = ev {
                self.app.set_select_held(false);
            }
            // MD's six-button X/Z are plain L/R, so SELECT+L/R is the display
            // chord. Handle it before the shortcut table: SELECT+L1/R1 are
            // Load/Save-State by default on every other machine.
            let md_display_chord = self.app.platform() == Platform::Md
                && matches!(self.app.phase(), Phase::Playing { .. })
                && self.select_down;
            if md_display_chord {
                match ev {
                    RawEvent::Down(btn @ (Btn::L1 | Btn::R1)) => {
                        self.app.set_select_held(true);
                        self.app.apply(Action::GbaDown(btn));
                        self.pad.apply(Action::GbaUp(btn));
                        continue;
                    }
                    RawEvent::Up(btn @ (Btn::L1 | Btn::R1)) => {
                        self.app.apply(Action::GbaUp(btn));
                        self.pad.apply(Action::GbaUp(btn));
                        continue;
                    }
                    _ => {}
                }
            }
            if let RawEvent::Up(btn) = ev {
                let mut consumed = false;
                if let Some(at) = self.custom_held.iter().position(|(held, _)| *held == btn) {
                    let (_, action) = self.custom_held.remove(at);
                    consumed = true;
                    if let Some(release) = custom_release(action) {
                        actions.push(release);
                    }
                }
                self.pressed.retain(|held| *held != btn);
                if btn == Btn::Select {
                    self.select_down = false;
                }
                if btn == Btn::Menu {
                    self.menu_down = false;
                }
                if consumed {
                    continue;
                }
            }
            if let RawEvent::Down(btn) = ev {
                let was_held = self.pressed.contains(&btn);
                if !was_held {
                    self.pressed.push(btn);
                }
                if btn == Btn::Select {
                    self.select_down = true;
                } else if btn == Btn::Menu {
                    self.menu_down = true;
                } else if !was_held {
                    if let Some(action) = self.shortcuts.as_ref().and_then(|keys| {
                        custom_shortcut(
                            keys,
                            btn,
                            self.select_down,
                            self.menu_down,
                            &self.pressed,
                        )
                    }) {
                        self.custom_held.push((btn, action));
                        actions.push(action);
                        continue;
                    }
                }
            }
            let layout = if self.playing() && !self.showing_polaroids() {
                self.app.button_layout()
            } else { slot_store::ButtonLayout::Off };
            if let Some(ev) = self.remap.feed_with(ev, layout, &self.controls) {
                actions.extend(self.gestures.feed(ev, now));
            }
        }
        actions.extend(self.gestures.tick(now));
        for action in actions {
            self.act(action);
        }
        // Gesture actions can be emitted one feed late (a SELECT hold becomes a
        // chord): re-align the app with the physical key before the pad is
        // rebuilt, or a stale synthetic SELECT-down keeps splitting L/R.
        self.app.set_select_held(self.select_down);
        if self.app.take_shortcut_reload() {
            self.shortcuts = Some(slot_store::read_shortcuts(&self.root));
        }
        if self.app.take_controls_reload() {
            self.controls = slot_store::read_controls(&self.root);
        }
        self.sync_pad();
    }

    fn sync_pad(&mut self) {
        for btn in self.app.taken_buttons() {
            self.pad.apply(Action::GbaUp(*btn));
        }
        if let Some(emu) = &self.emu {
            emu.set_input(self.pad.mask());
        }
    }

    fn bridge_link(&mut self, f: impl FnOnce(&mut App)) {
        let had_link = self.app.link_active();
        f(&mut self.app);
        if had_link && !self.app.link_active() {
            if let Some(emu) = &self.emu {
                emu.end_link();
            }
        }
    }

    fn act(&mut self, action: Action) {
        if trace() {
            eprintln!("slot: {action:?} in {:?}", self.app.phase());
        }
        match action {
            Action::RewindStart => self.rewinding = true,
            Action::RewindStop => self.rewinding = false,
            Action::FfStart => self.fast = true,
            Action::FfStop => self.fast = false,
            _ => {}
        }
        let menu = self.overlaid();
        let select_held = self.select_down;
        self.bridge_link(|app| {
            // The gesture layer may emit a synthetic SELECT-up when it turns
            // the hold into a chord. The physical key is still down: keep the
            // MD display chord armed until the real release.
            if select_held {
                app.set_select_held(true);
            }
            app.apply(action)
        });
        if matches!(
            action,
            Action::VolumeUp | Action::VolumeDown | Action::MuteToggle
        ) {
            if let Some(emu) = &self.emu {
                emu.set_volume(self.app.output_volume());
            }
        }
        if menu || self.overlaid() {
            self.pad.clear();
        } else if self.app.takes_from_the_game(action) {
            if let Action::GbaDown(btn) | Action::GbaUp(btn) = action {
                self.pad.apply(Action::GbaUp(btn));
            }
        } else {
            self.pad.apply(action);
        }
        self.sync_rumble();
    }

    pub fn update(&mut self, dt: f32) {
        self.bridge_link(|app| app.update(dt));
        if let Some(emu) = &self.emu {
            emu.set_volume(self.app.output_volume());
        }
        if let Some((client_id, transport)) = self.app.take_link_transport() {
            // Every link is a session now: the socket is kept between games, so
            // it starts life here rather than inside a worker; `sync_link`
            // hands it to whichever game is running.
            self.link_wire = Some(transport);
            self.link_attached = false;
            self.link_handshake = None;
            self.peer_game = None;
            self.app.set_peer_game(None);
            self.announced = None;
            self.peer_alive = false;
            self.game_seen = None;
            self.time_sent = false;
            self.following = None;
            self.ended_rom = None;
            self.ended_at = None;
            self.pending_wire.clear();
            self.attached_at = None;
            self.wire_since = Some(Instant::now());
            // The keepalive clock starts when there is a socket to keep
            // alive, not when the app booted: a device that has been on the
            // shelf for a minute must not call a fresh peer silent.
            self.last_heard = Instant::now();
            self.pinged = Instant::now();
            self.refused_rom = None;
            crate::link_log::debug(format!(
                "link: session socket open ({} client_id {client_id} platform={:?} core={:?} seated={:?} phase={:?})",
                if self.cable_link() { "cable" } else { "netpacket" },
                self.app.platform(),
                self.app.core(),
                self.app.seated_cart().map(|cart| cart.stem.clone()),
                self.app.phase(),
            ));
            // "Linked" is the moment the joiner's clock should match the host.
            self.send_time_sync();
        }
        if let Some(codes)=self.app.cheat_pending.take() { if let Some(emu)=&self.emu { emu.set_cheats(codes); } }
        if let Some(p) = self.app.take_gb_palette() {
            if let Some(emu) = &self.emu {
                emu.set_option("mgba_gb_colors", p.core_name());
            }
        }
        if let Some(on) = self.app.take_colour_correction() {
            if let Some((key, value)) = crate::core::colour_option(self.app.core(), on) {
                if let Some(emu) = &self.emu {
                    emu.set_option(key, value);
                }
            }
        }
        if let Some((stem, serial)) = self.app.take_link_reload() {
            self.reload_for_link(&stem, serial);
        }
        if self.app.link_active() {
            if self.emu.as_ref().is_some_and(EmuHandle::peer_mismatch) {
                if self.session_link() {
                    // Session: the cart differs, the pair does not. Drop the
                    // cable, keep the socket, and say so once.
                    self.detach_link(true);
                    self.bridge_link(|app| app.game_mismatch());
                } else {
                    self.bridge_link(|app| app.game_mismatch());
                }
            } else if self.emu.as_ref().is_some_and(EmuHandle::bios_mismatch) {
                self.bridge_link(|app| app.bios_mismatch());
            } else if self.emu.as_ref().is_some_and(EmuHandle::peer_ended) {
                if self.session_link() {
                    // The cable is done even when the room is not: hand the
                    // socket back so a live host can re-arm its listener.
                    self.detach_link(true);
                }
                self.bridge_link(|app| app.peer_ended());
            } else if self.emu.as_ref().is_some_and(EmuHandle::link_lost) {
                if self.session_link() && self.link_attached {
                    // A cable that stalls is not necessarily a dead peer: the
                    // other end may be loading, paused or between carts. Drop
                    // the cable and keep the session; keepalive (five seconds
                    // of silence) is what ends a session.
                    crate::link_log::warn(
                        "link: the cable stalled; detaching, the session stays",
                    );
                    self.detach_link(true);
                } else {
                    self.app.peer_lost();
                }
            }
        }
        if let Some(sfx) = self.app.take_sfx() {
            self.play_sfx(sfx);
        }
        self.sync_link();
        self.sync_core();
        self.sync_reload();
        self.sync_run_marker();
        if !self.has_core() {
            for action in self.gestures.drop_ff_latch() {
                self.act(action);
            }
        }
        let ready = self.emu.as_ref().is_some_and(EmuHandle::has_published);
        if ready && !self.app.game_ready() {
            launch("first frame shown");
        }
        self.app.set_game_ready(ready);
        self.sync_speed();
        self.sync_rewind_hud();
        self.sync_ff_hud();
        self.sync_rumble();
        self.sync_pad();
        let now = self.app.now();
        self.prefetch.browse(self.app.browsed_rom(), now);
    }

    fn sync_rumble(&mut self) {
        let asked = match &self.emu {
            Some(emu) if self.playing() && self.app.rumble_enabled() => emu.rumble().strength(),
            _ => {
                self.pulse = None;
                return self.rumble(0);
            }
        };
        let now = self.app.now();
        let want = match (asked, self.pulse) {
            (0, Some((at, strength))) if now.saturating_sub(at) < RUMBLE_MIN_MS => strength,
            (0, _) => {
                self.pulse = None;
                0
            }
            (asked, Some((at, _))) => {
                self.pulse = Some((at, asked));
                if now.saturating_sub(at) < RUMBLE_MAX_MS {
                    asked
                } else {
                    0
                }
            }
            (asked, None) => {
                self.pulse = Some((now, asked));
                asked
            }
        };
        self.rumble(want);
    }

    fn sync_ff_hud(&mut self) {
        let ff = match (self.actually_fast_forwarding(), self.gestures.ff_latched()) {
            (false, _) => FfState::Off,
            (true, false) => FfState::Held,
            (true, true) => FfState::Latched,
        };
        self.app.set_ff(ff);
    }

    fn sync_rewind_hud(&mut self) {
        let fill = self
            .actually_rewinding()
            .then(|| self.emu.as_ref().map(EmuHandle::rewind_fill))
            .flatten();
        match fill {
            Some(fill) => self.app.show_rewind(fill),
            None => self.app.hide_rewind(),
        }
    }

    fn actually_rewinding(&self) -> bool {
        self.rewinding && self.playing() && self.app.may_rewind()
    }

    fn actually_fast_forwarding(&self) -> bool {
        self.fast && self.playing() && self.app.may_fast_forward()
    }

    fn inserting(&self) -> bool {
        matches!(self.app.phase(), Phase::Inserting { .. })
    }

    fn showing_polaroids(&self) -> bool {
        matches!(self.app.phase(), Phase::Polaroids { .. })
    }

    fn overlaid(&self) -> bool {
        self.showing_polaroids() || self.held()
    }

    fn playing(&self) -> bool {
        matches!(self.app.phase(), Phase::Playing { .. }) && !self.held()
    }

    fn held(&self) -> bool {
        self.app.utility.is_some() || self.app.guide_open() || self.app.power_menu().is_some() || self.app.game_menu_open() || self.app.shutting_down()
    }

    fn dozing(&self) -> bool {
        matches!(self.app.phase(), Phase::Doze { .. })
    }

    fn ejecting(&self) -> bool {
        matches!(self.app.phase(), Phase::Ejecting { .. })
    }

    fn sync_speed(&mut self) {
        if let Some(emu) = &self.emu {
            emu.set_fast_steps(u32::from(self.app.ff_speed()));
            emu.set_ff_sound(self.app.ff_sound());
            // One pause reason wins, and it is logged when it changes: "why is
            // the linked game not stepping" is the first question a link bug
            // asks.
            let pause = if self.inserting() {
                Some("inserting")
            } else if self.ejecting() {
                Some("ejecting")
            } else if self.showing_polaroids() {
                Some("polaroids")
            } else if self.dozing() {
                Some("doze")
            } else if self.link_handshake.is_some() {
                Some("handshake")
            } else if self.held() && !self.app.link_paired() {
                Some("menu")
            } else {
                None
            };
            if pause != self.speed_note {
                match pause {
                    Some(reason) => crate::link_log::debug(format!(
                        "link: pausing the core ({reason}) attached={} paired={} phase={:?}",
                        self.link_attached,
                        self.app.link_paired(),
                        self.app.phase()
                    )),
                    None => crate::link_log::debug(format!(
                        "link: core running attached={} paired={} handshake={}",
                        self.link_attached,
                        self.app.link_paired(),
                        self.link_handshake.is_some()
                    )),
                }
                self.speed_note = pause;
            }
            emu.set_speed(if pause.is_some() {
                Speed::Paused
            } else if self.actually_fast_forwarding() {
                Speed::Fast
            } else {
                Speed::Normal
            });
            emu.set_rewinding(self.actually_rewinding());
            emu.set_turbo(self.app.turbo());
            emu.set_rewind_recording(self.app.records_rewind());
        }
    }

    fn no_core() -> bool {
        std::env::var_os("SLOT_NO_CORE").is_some_and(|v| v != "0")
    }

/// The `last-run.txt` body for this state; `None` means no marker at all (the
/// shelf with no link is the only fully clean state).
fn run_marker_text(cart: Option<&str>, linked: bool) -> Option<String> {
    match (cart, linked) {
        (Some(stem), true) => Some(format!("cart={stem}\nlinked=1\n")),
        (Some(stem), false) => Some(format!("cart={stem}\nlinked=0\n")),
        (None, true) => Some("linked=1\n".to_string()),
        (None, false) => None,
    }
}
    /// Keep linkSlot/Config/last-run.txt in step with the game and the link.
    /// The file exists only while a game runs or a link is open, so one left
    /// behind at the next boot means this run died instead of exiting.
    fn sync_run_marker(&mut self) {
        // Shutdown already removed the marker; do not recreate it for the one
        // or two frames the game may still run while the screen fades.
        if self.app.shutting_down() {
            if self.run_marker.take().is_some() {
                crate::linkcfg::clear_last_run(&self.root);
            }
            return;
        }
        let in_game = matches!(
            self.app.phase(),
            Phase::Playing { .. } | Phase::Inserting { .. } | Phase::Ejecting { .. }
        );
        let cart = self
            .app
            .seated_cart()
            .filter(|_| in_game)
            .map(|cart| cart.stem.clone());
        let linked = self.app.link_active();
        let text = Self::run_marker_text(cart.as_deref(), linked);
        if text != self.run_marker {
            match &text {
                Some(text) => crate::linkcfg::write_last_run(&self.root, text),
                None => crate::linkcfg::clear_last_run(&self.root),
            }
            self.run_marker = text;
        }
    }
    fn sync_core(&mut self) {
        if Self::no_core() {
            return;
        }
        // An in-place restart drops the old worker and opens a fresh one while
        // the app stays in `Playing`; no eject/insert animation is involved.
        if matches!(self.app.phase(), Phase::Playing { .. }) && self.app.take_restart_pending() {
            if self.link_attached {
                self.detach_link(true);
            }
            self.emu = None;
            self.restart_in_place = true;
        }
        let stem = match self.app.phase() {
            Phase::Shelf => {
                self.emu = None;
                self.restart_in_place = false;
                return;
            }
            Phase::Inserting { cart, .. } => cart.clone(),
            Phase::Playing { .. } if self.restart_in_place => {
                let Some(cart) = self.app.seated_cart() else {
                    self.restart_in_place = false;
                    return;
                };
                cart.stem.clone()
            }
            _ => return,
        };
        if self.emu.is_none() {
            let (_, serial) = self.app.link_mode(&stem);
            self.spawn_core(&stem, serial);
        }
        match self.emu.as_ref().map(EmuHandle::state) {
            Some(CoreState::Loading) => {}
            Some(CoreState::Ready) => {
                if self.restart_in_place {
                    self.restart_in_place = false;
                    self.app.on_core_restarted();
                }
                if let (true, Some(p), Some(emu)) =
                    (self.app.palette_live(), self.app.gb_palette(), &self.emu)
                {
                    emu.set_option("mgba_gb_colors", p.core_name());
                }
                self.app.on_core_ready()
            }
            Some(CoreState::Failed) | None => {
                self.emu = None;
                if self.restart_in_place {
                    self.restart_in_place = false;
                    self.app.on_restart_failed();
                } else {
                    self.app.on_core_failed();
                }
            }
        }
    }

    fn spawn_core(&mut self, stem: &str, serial: &'static str) {
        let Some((rom, platform)) = self
            .app
            .seated_cart()
            .filter(|c| c.stem == stem)
            .map(|c| (c.rom.clone(), c.platform))
        else {
            return;
        };
        let core = slot_store::core_for_platform(&self.root, stem, platform);
        launch(&format!("opening {stem} on {}", core.as_str()));
        self.app.set_core(core);
        self.app.set_platform(platform);
        self.app
            .set_video_mode(crate::video_mode::video_mode_for(&self.root, stem));
        self.app.set_link_loaded(serial);
        let resume = (!self.app.starting_clean())
            .then(|| persist::read_resume(&self.root, platform, core, stem))
            .flatten();
        launch("resume state read");
        let player = self.app.link_player();
        crate::link_log::debug(format!(
            "link: opening {stem:?} core={} platform={platform:?} serial={serial} player={player:?} clean={}",
            core.as_str(),
            self.app.starting_clean()
        ));
        let palette = crate::core::palette_for(self.app.gb_palette(), platform, &rom, player);
        self.app.set_palette_live(palette.is_some());
        let opened = open_core(
            &self.root,
            core,
            platform,
            serial,
            self.app.colour_correction(),
            palette,
            player,
        );
        self.app.set_named_core(opened.named);
        launch("core opened");
        let sav = persist::read_sav(&self.root, platform, stem);
        launch("save read");
        let ring = self.sink.ring();
        let emu = match player.filter(|_| platform == Platform::Gba && core == slot_store::Core::Mgba) {
            Some(p) => EmuHandle::spawn_linked(opened.core, rom, ring, sav, resume, p),
            None => EmuHandle::spawn(opened.core, rom, ring, sav, resume),
        };
        emu.set_volume(self.app.output_volume());
        emu.set_direct_xy(platform == Platform::Md);
        emu.set_driven(self.driven);
        self.app.set_snapshot(Box::new(emu.snapshot()));
        self.emu = Some(emu);
    }

    /// How the cable for the cart in the slot is shaped.
    ///
    /// Everything with a paired core keeps mGBA's two-GBA link. A Famicom,
    /// Super Famicom or Mega Drive is one machine played by two pads, so its
    /// cable carries the game's identity instead: the ROM hash is what tells
    /// two carts apart, and it is read once, here, on the frame the transport
    /// arrives.
    fn link_pairing(&self) -> Pairing {
        if !self.app.single_machine_link() {
            return Pairing::PairedGba;
        }
        let cart = self.app.seated_cart();
        let device = self
            .app
            .root()
            .map(crate::linkcfg::read_device_name)
            .unwrap_or_else(crate::linkcfg::default_device_name);
        let hello = Hello {
            version: HELLO_VERSION,
            platform: self.app.platform().index() as u8,
            core: self.app.core().index() as u8,
            rom: cart.map_or([0u8; 32], |cart| {
                crate::rom_id::digest_file(&cart.rom).unwrap_or([0u8; 32])
            }),
            device: [0; crate::cable::HELLO_DEVICE],
        }
        .named(&device);
        Pairing::SingleMachine(hello)
    }

    /// Whether the open link is a session: one socket that outlives the game,
    /// rather than a connection opened per cart.
    ///
    /// Every machine qualifies now: the cable covers the Famicom, the Super
    /// Famicom and mGBA, and gpSP's netpacket keeps the same socket while its
    /// per-game link state is rebuilt on each insert.
    fn session_link(&self) -> bool {
        self.app.link_active()
    }

    /// Whether this session runs on the lockstep cable (state transfer and a
    /// joint start) rather than gpSP's netpacket (each side's own save).
    ///
    /// Follows the core the current game runs on, not the platform: the host's
    /// choice can move a session from mGBA to gpSP between carts. A Famicom,
    /// Super Famicom or Mega Drive always runs on the cable: its core has no
    /// netpacket hooks, and one console with two pads is exactly what the
    /// cable models.
    fn cable_link(&self) -> bool {
        self.app.single_machine_link() || self.app.core() == slot_store::Core::Mgba
    }

    /// The game this side is in right now, as the other end has to see it.
    /// A game that cannot link is not a session game: it runs solo and the
    /// peer is told nothing about it.
    fn current_game(&self) -> Option<crate::cable::GameStart> {
        if !matches!(self.app.phase(), Phase::Playing { .. }) {
            return None;
        }
        if !self.app.game_can_link() {
            return None;
        }
        let cart = self.app.seated_cart()?;
        // Mid-switch the app still reports the previous machine while the new
        // cart is already seated (an FC cart announced as GBA made the peer
        // answer "different game" and never pair). Say nothing until the core
        // that is actually running belongs to this cart.
        if self.app.platform() != cart.platform {
            return None;
        }
        Some(crate::cable::GameStart {
            platform: cart.platform.index() as u8,
            core: self.app.core().index() as u8,
            // Cached: a GBA cart is up to 32 MB and this runs every frame.
            rom: self.app.cart_hash(&cart.stem, &cart.rom),
            name: cart.stem.clone(),
        })
    }

    /// One frame of session upkeep: listen, announce, attach, detach.
    ///
    /// Runs before `sync_core`, because that is what drops the worker the
    /// socket would otherwise die with.
    fn sync_link(&mut self) {
        if !self.app.link_active() {
            // No session: whatever socket is still around is stale.
            if self.link_attached {
                if let Some(emu) = &self.emu {
                    let _ = emu.release_transport();
                }
            }
            self.link_attached = false;
            self.link_handshake = None;
            self.peer_game = None;
            self.announced = None;
            self.link_wire = None;
            return;
        }
        if !self.session_link() {
            return;
        }
        // A host keeps its room open across a dropped peer.  The app owns the
        // listener because it also owns the UI/session identity; the session
        // only asks it to re-arm when no transport is live.
        if self.app.link_client_id() == Some(0) && self.link_wire.is_none() && !self.link_attached {
            self.app.ensure_host_waiting();
        }
        if self.link_attached && !matches!(self.app.phase(), Phase::Playing { .. }) {
            self.detach_link(true);
        }
        // A cart that cannot link runs solo, and the player is told once.
        if matches!(self.app.phase(), Phase::Playing { .. }) && !self.app.game_can_link() {
            let stem = self.app.seated_cart().map(|cart| cart.stem.clone());
            if stem.is_some() && self.solo_noted != stem {
                self.solo_noted = stem;
                self.app.note_solo_game();
            }
        }
        self.pump_link();
        if let Some(emu) = &self.emu {
            if let Some(peer) = emu.peer_hello() {
                self.app.set_link_peer(&peer.device_name());
            }
            for frame in emu.take_link_controls() {
                self.link_control(&frame);
            }
        }
        // P2: act on the answer the player gave to "join the host's game?".
        if let Some(answer) = self.app.take_follow_answer() {
            match answer {
                FollowAnswer::Accept => {
                    crate::link_log::debug("link: following the host into its game");
                    self.following = self.peer_game.as_ref().map(|peer| peer.rom);
                }
                FollowAnswer::Refuse(reason) => {
                    if let Some(peer) = self.peer_game.clone() {
                        self.refused_rom = Some(peer.rom);
                    }
                    self.refuse_follow(reason);
                }
            }
        }
        // P3: a session with no game still has to notice a dead peer, and a
        // silent one has to be asked. Both directions only happen while this
        // side is the one holding the socket.
        self.keepalive();
        if self.link_attached {
            // A Famicom/SFC/MD pair cannot step until each end has the other's
            // hello. If it never arrives (it can be eaten while the peer is
            // still in the session window) retry the attach instead of
            // sitting frozen until the stall kills the session.
            if self.hello_overdue() {
                crate::link_log::warn(
                    "link: attached but the peer's hello never arrived; re-attaching",
                );
                self.detach_link(true);
                return;
            }
            self.poll_handshake();
            return;
        }
        match self.current_game() {
            Some(game) => {
                self.announce(&game);
                let agreed = self
                    .peer_game
                    .as_ref()
                    .is_some_and(|peer| game.agrees(peer));
                if agreed {
                    self.attach_link(&game);
                } else if self.app.link_client_id() == Some(0)
                    && self.waiting_noted != Some(game.rom)
                {
                    // The host started a game; say once that it is waiting
                    // rather than looking stuck.
                    self.waiting_noted = Some(game.rom);
                    self.app.link_waiting_peer();
                }
            }
            None => self.announced = None,
        }
        self.consider_follow();
        self.attach_without_announce();
    }

    /// An older peer never says which game it started: after a short grace
    /// period the socket is handed over anyway, exactly as before this
    /// session existed. The cable's own hello and the BIOS/state checks still
    /// decide whether the pair can run.
    fn attach_without_announce(&mut self) {
        if self.link_attached || self.link_wire.is_none() {
            return;
        }
        // A peer from this version pings and will announce its game: wait for
        // it rather than guessing. Only a silent peer (an older build) gets
        // the blind hand-over, after a grace period measured from *this*
        // game's start, not from when the socket opened.
        if self.peer_alive {
            return;
        }
        let Some(game) = self.current_game() else {
            self.game_seen = None;
            return;
        };
        if self.game_seen != Some(game.rom) {
            self.game_seen = Some(game.rom);
            self.wire_since = Some(Instant::now());
            crate::link_log::debug(format!(
                "link: waiting for the peer to announce {:?} (old-peer grace)",
                game.name
            ));
            return;
        }
        if self.peer_game.is_some() {
            return;
        }
        let Some(since) = self.wire_since else {
            self.wire_since = Some(Instant::now());
            return;
        };
        if since.elapsed() < Duration::from_millis(1500) {
            return;
        }
        crate::link_log::warn("link: the peer never announced, attaching anyway");
        self.attach_link(&game);
    }

    /// The host shares its clock; the joiner takes it.
    fn send_time_sync(&mut self) {
        if self.app.link_client_id() != Some(0) {
            return;
        }
        if self.time_sent {
            return;
        }
        let (utc, offset_min) = self.app.clock_sync_payload();
        let Some(wire) = self.link_wire.as_mut() else {
            return;
        };
        wire.send(
            NETPACKET_RELIABLE,
            &crate::cable::time_sync_packet(utc, offset_min),
        );
        self.time_sent = true;
        crate::link_log::debug(format!(
            "link: shared the clock utc={utc} offset={offset_min}"
        ));
    }

    /// Answer the peer's invitation with one of the refusals.
    fn refuse_follow(&mut self, reason: crate::cable::FollowRefusal) {        self.app.clear_follow();
        if let Some(wire) = self.link_wire.as_mut() {
            wire.send(NETPACKET_RELIABLE, &crate::cable::refuse_packet(reason));
        }
        crate::link_log::debug(format!("link: refused to follow ({reason:?})"));
    }

    /// The peer started a game this side is not in: offer to switch, or say
    /// why we cannot.
    fn consider_follow(&mut self) {
        let Some(peer) = self.peer_game.clone() else {
            return;
        };
        if self.app.follow_pending() {
            return;
        }
        // A switch already agreed to is in flight: asking again would flash a
        // second question under the insert.
        if self.following == Some(peer.rom) {
            return;
        }
        if self.current_game().is_some_and(|own| own.agrees(&peer)) {
            return;
        }
        if self.refused_rom == Some(peer.rom) {
            return;
        }
        match self
            .app
            .find_follow_cart_named(peer.platform, peer.core, peer.rom, &peer.name)
        {
            Some(stem) => {
                crate::link_log::debug(format!(
                    "link: offering to switch to {:?} (cart {stem:?})",
                    peer.name
                ));
                self.app.ask_follow(&peer.name, &stem, peer.core)
            }
            None => {
                // No cart here and no cart transfer any more: tell the host
                // this side cannot follow, instead of stalling the session.
                crate::link_log::warn(format!(
                    "link: no local cart for {:?} (rom={})",
                    peer.name, crate::rom_id::digest_hex(&peer.rom)
                ));
                self.refused_rom = Some(peer.rom);
                self.refuse_follow(crate::cable::FollowRefusal::MissingRom);
            }
        }
    }

    /// One ping a second while no game holds the socket; five seconds of
    /// silence means the peer is gone even though no frame was ever owed.
    fn keepalive(&mut self) {
        const PING_EVERY: Duration = Duration::from_secs(1);
        const QUIET: Duration = Duration::from_secs(5);
        if self.link_attached {
            return;
        }
        // Nothing to keep alive: no socket yet, or one the app already gave
        // up on. Without this the quiet check would fire every frame forever.
        if self.link_wire.is_none() {
            return;
        }
        if self.last_heard.elapsed() >= QUIET {
            crate::link_log::warn(format!(
                "link: the peer went quiet after {} ms",
                self.last_heard.elapsed().as_millis()
            ));
            self.link_wire = None;
            self.peer_game = None;
            self.announced = None;
            self.app.peer_lost();
            return;
        }
        if self.pinged.elapsed() < PING_EVERY {
            return;
        }
        if let Some(wire) = self.link_wire.as_mut() {
            wire.send(NETPACKET_RELIABLE, &crate::cable::ping_packet());
            self.pinged = Instant::now();
        }
    }

    /// Tell the peer which cart this side is running.
    fn announce(&mut self, game: &crate::cable::GameStart) {
        let Some(wire) = self.link_wire.as_mut() else {
            return;
        };
        if self.announced == Some(game.rom) {
            return;
        }
        wire.send(NETPACKET_RELIABLE, &crate::cable::game_start_packet(game));
        self.announced = Some(game.rom);
        crate::link_log::debug(format!(
            "link: announced game {:?} platform={} core={} rom={}",
            game.name, game.platform, game.core, crate::rom_id::digest_hex(&game.rom)
        ));
    }

    /// Both ends are on the same cart: open the cable across the session's
    /// socket and hold both sides still until the pair is armed.
    fn attach_link(&mut self, game: &crate::cable::GameStart) {
        // The joiner may have started its clock from a sleep since the session
        // opened; re-share it with every attach while the socket is still here.
        self.send_time_sync();
        let Some(wire) = self.link_wire.take() else {
            return;
        };
        let Some(emu) = &self.emu else {
            self.link_wire = Some(wire);
            return;
        };
        // Frames the peer sent while we were still in the session window (its
        // hello, its ready, the first masks) go in front of the live socket.
        let pending = std::mem::take(&mut self.pending_wire);
        let wire: Box<dyn LinkChannel> = if pending.is_empty() {
            wire
        } else {
            crate::link_log::debug(format!(
                "link: replaying {} cable frames that arrived before the attach",
                pending.len()
            ));
            Box::new(ReplayLink::new(pending, wire))
        };
        let client_id = self.app.link_client_id().unwrap_or(0);
        if self.cable_link() {
            let pairing = self.link_pairing();
            emu.begin_cable(client_id as u8, wire, pairing);
            // Both ends hold still until the pair is armed; running earlier
            // would leave the joiner a few frames behind.
            self.link_handshake = Some(Instant::now());
        } else {
            // netpacket: gpSP's own handshake owns the timing, and each side
            // keeps its own save — the session only keeps the socket.
            emu.begin_link(client_id, wire);
            self.link_handshake = None;
        }
        self.link_attached = true;
        self.attached_at = Some(Instant::now());
        self.following = None;
        self.app.clear_follow();
        self.app.set_link_game(Some(game.name.clone()));
        self.app.link_paired_now(true);
        self.app.link_joined();
        // The host stopped this very cart a moment ago: this attach is its
        // restart, and the joiner's screen is about to jump back to the start.
        if client_id != 0
            && self.ended_rom == Some(game.rom)
            && self
                .ended_at
                .is_some_and(|at| at.elapsed() < RESTART_NOTE)
        {
            crate::link_log::debug("link: the host restarted the cart; telling the player");
            self.app.link_restarted();
        }
        self.ended_rom = None;
        self.ended_at = None;
        self.waiting_noted = None;
        crate::link_log::debug(format!(
            "link: attached to {:?} ({} player={client_id} rom={})",
            game.name,
            if self.cable_link() { "cable" } else { "netpacket" },
            crate::rom_id::digest_hex(&game.rom)
        ));
    }

    /// Close the cable, keep the session.
    fn detach_link(&mut self, tell_peer: bool) {
        if self.link_attached {
            if let Some(emu) = &self.emu {
                if let Some(wire) = emu.release_transport() {
                    self.link_wire = Some(wire);
                }
            }
            self.link_attached = false;
            crate::link_log::debug("link: detached from the game, session kept");
        }
        self.link_handshake = None;
        self.attached_at = None;
        self.pending_wire.clear();
        self.wire_since = Some(Instant::now());
        self.peer_game = None;
        self.announced = None;
        self.app.set_link_game(None);
        self.app.set_peer_game(None);
        self.app.link_paired_now(false);
        if tell_peer {
            if let Some(wire) = self.link_wire.as_mut() {
                wire.send(NETPACKET_RELIABLE, &crate::cable::game_end_packet());
            }
        }
    }

    /// Read whatever the peer sent while no game held the socket.
    fn pump_link(&mut self) {
        let mut frames = Vec::new();
        let mut closed = false;
        if let Some(wire) = self.link_wire.as_mut() {
            while let Some(frame) = wire.try_recv() {
                frames.push(frame);
            }
            closed = wire.is_closed();
        }
        if !frames.is_empty() {
            self.last_heard = Instant::now();
        }
        for frame in &frames {
            if crate::cable::is_control(frame) {
                self.link_control(frame);
            } else {
                // A cable frame that beat the attach: keep it so the hand-over
                // can replay it. Losing a one-shot hello used to freeze a
                // Famicom/SFC/MD pair until the stall made it disconnect.
                self.keep_cable_frame(frame);
            }
        }
        if closed {
            self.link_wire = None;
            self.announced = None;
            self.app.peer_lost();
        }
    }

    /// Buffer a cable frame from the session window, with a bound so a peer
    /// that screams masks cannot grow the session without limit.
    fn keep_cable_frame(&mut self, frame: &[u8]) {
        const MAX_FRAMES: usize = 256;
        const MAX_BYTES: usize = 256 * 1024;
        let bytes: usize = self.pending_wire.iter().map(Vec::len).sum();
        if self.pending_wire.len() >= MAX_FRAMES || bytes + frame.len() > MAX_BYTES {
            return;
        }
        self.pending_wire.push(frame.to_vec());
    }

    fn link_control(&mut self, frame: &[u8]) {
        self.last_heard = Instant::now();
        if !self.peer_alive {
            self.peer_alive = true;
            crate::link_log::debug(format!(
                "link: the peer is a session too (first frame {} bytes)",
                frame.len()
            ));
        }
        if let Some(start) = crate::cable::decode_game_start(frame) {
            // A refusal sticks to that cart only: a new game reopens the
            // question, the same one does not nag on every announce.
            if self.refused_rom != Some(start.rom) {
                self.refused_rom = None;
            }
            let agrees = self
                .current_game()
                .is_some_and(|own| own.agrees(&start));
            self.peer_game = Some(start.clone());
            self.app.set_peer_game(Some(start.clone()));
            // Answer with our own cart: the other side may have joined after
            // this one announced, and it needs to know what to compare against.
            if let Some(game) = self.current_game() {
                self.announce(&game);
            }
            crate::link_log::debug(format!(
                "link: peer plays {:?} platform={} core={} rom={} agrees={agrees}",
                start.name, start.platform, start.core, crate::rom_id::digest_hex(&start.rom)
            ));
        } else if crate::cable::is_ping(frame) {
            if let Some(wire) = self.link_wire.as_mut() {
                wire.send(NETPACKET_RELIABLE, &crate::cable::pong_packet());
            }
        } else if crate::cable::is_pong(frame) {
        } else if let Some(reason) = crate::cable::decode_refuse(frame) {
            crate::link_log::debug(format!("link: peer refused a follow ({reason:?})"));
            self.app.link_refused(reason);
        } else if let Some((utc, offset_min)) = crate::cable::decode_time_sync(frame) {
            if self.app.link_client_id() != Some(0) {
                crate::link_log::debug(format!(
                    "link: clock taken from the host utc={utc} offset={offset_min}"
                ));
                self.app.apply_clock_sync(utc, offset_min);
            }
        } else if crate::cable::is_control(frame) {
            crate::link_log::debug("link: peer ended its game");
            self.ended_rom = self.peer_game.as_ref().map(|peer| peer.rom);
            self.ended_at = Some(Instant::now());
            self.following = None;
            self.peer_game = None;
            self.app.set_peer_game(None);
            if self.link_attached {
                self.detach_link(false);
            }
        }
    }

    /// Hold both ends until the cable is armed, then let them run.
    ///
    /// Running before the peer has restored would step the host's machine on
    /// its own and leave the joiner a few frames behind — the pair would start
    /// out desynced. `armed` means "the joiner has my state and I have its
    /// ready", which is exactly the moment both may step together.
    /// A single-machine cable (FC/SFC/MD) is gated on the peer's hello: if the
    /// cable has been attached for a while and no hello arrived, hand it back
    /// and retry.
    fn hello_overdue(&self) -> bool {
        const HELLO_WAIT: Duration = Duration::from_secs(2);
        if !self.app.single_machine_link() {
            return false;
        }
        let Some(at) = self.attached_at else {
            return false;
        };
        let missing = self
            .emu
            .as_ref()
            .is_some_and(|emu| emu.peer_hello().is_none());
        missing && at.elapsed() >= HELLO_WAIT
    }

    fn poll_handshake(&mut self) {
        let Some(since) = self.link_handshake else {
            return;
        };
        let Some(emu) = &self.emu else {
            self.link_handshake = None;
            return;
        };
        if emu.link_armed() {
            self.link_handshake = None;
            crate::link_log::debug(format!(
                "link: pair armed after {} ms",
                since.elapsed().as_millis()
            ));
            return;
        }
        if since.elapsed() >= Duration::from_secs(3) {
            crate::link_log::warn(format!(
                "link: the pair never armed after {} ms, back to solo",
                since.elapsed().as_millis()
            ));
            self.detach_link(true);
        }
    }

    fn reload_for_link(&mut self, stem: &str, serial: &'static str) {
        eprintln!("slot: link: loading {stem} again with gpsp_serial={serial}");
        if self.emu.is_some() {
            self.app.flush_resume();
        }
        self.emu = None;
        self.spawn_core(stem, serial);
        self.reloading = true;
    }

    fn sync_reload(&mut self) {
        if !self.reloading {
            return;
        }
        match self.emu.as_ref().map(EmuHandle::state) {
            Some(CoreState::Loading) => {}
            Some(CoreState::Ready) => {
                self.reloading = false;
                self.app.link_reload_done();
            }
            Some(CoreState::Failed) | None => {
                self.reloading = false;
                self.emu = None;
                self.app.link_reload_failed();
                if let Some((stem, serial)) = self.app.take_link_reload() {
                    self.reload_for_link(&stem, serial);
                }
            }
        }
    }
}

pub(crate) fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SLOT_TRACE").is_some())
}

fn launch(step: &str) {
    eprintln!("slot: launch: {step} at {:.1} ms", crate::boot_time::ms());
}

fn device_btn(device: slot_store::Device) -> Btn {
    match device {
        slot_store::Device::Up => Btn::Up,
        slot_store::Device::Down => Btn::Down,
        slot_store::Device::Left => Btn::Left,
        slot_store::Device::Right => Btn::Right,
        slot_store::Device::A => Btn::A,
        slot_store::Device::B => Btn::B,
        slot_store::Device::X => Btn::X,
        slot_store::Device::Y => Btn::Y,
        slot_store::Device::L1 => Btn::L1,
        slot_store::Device::R1 => Btn::R1,
        slot_store::Device::L2 => Btn::L2,
        slot_store::Device::R2 => Btn::R2,
        slot_store::Device::Select => Btn::Select,
        slot_store::Device::Start => Btn::Start,
    }
}

/// Only Select-chords are translated here. Menu and plain-button shortcuts
/// remain on the legacy gesture path until the settings UI lands.
fn custom_shortcut(
    keys: &ShortcutKeys,
    physical: Btn,
    select: bool,
    menu: bool,
    pressed: &[Btn],
) -> Option<Action> {
    Shortcut::ALL.into_iter().find_map(|shortcut| {
        let binding = keys.get(shortcut)?;
        if device_btn(binding.button) != physical {
            return None;
        }
        let matches = match binding.modifier {
            Some(slot_store::Modifier::Select) => select,
            Some(slot_store::Modifier::Menu) => menu,
            Some(slot_store::Modifier::Button(first)) => {
                let first = device_btn(first);
                first != physical && pressed.contains(&first)
            }
            None => {
                !select
                    && !menu
                    && !(physical == Btn::R2 && shortcut == Shortcut::FastForward)
                    && !(physical == Btn::L2 && shortcut == Shortcut::Rewind)
            }
        };
        matches.then(|| match shortcut {
            Shortcut::SaveState => Action::SaveState,
            Shortcut::LoadState => Action::LoadState,
            Shortcut::BrightnessUp => Action::BrightnessUp,
            Shortcut::BrightnessDown => Action::BrightnessDown,
            Shortcut::BlueLightDown => Action::BlueLightDown,
            Shortcut::BlueLightUp => Action::BlueLightUp,
            Shortcut::ColorCorrection => Action::ColourCorrectionToggle,
            Shortcut::PaletteBack => Action::PalettePrev,
            Shortcut::PaletteNext => Action::PaletteNext,
            Shortcut::ShaderNext => Action::ShaderNext,
            Shortcut::ShaderBack => Action::ShaderPrev,
            Shortcut::FastForward => Action::FfStart,
            Shortcut::Rewind => Action::RewindStart,
        })
    })
}

fn custom_release(action: Action) -> Option<Action> {
    match action {
        Action::FfStart => Some(Action::FfStop),
        Action::RewindStart => Some(Action::RewindStop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slot_store::{Binding, Device, Modifier};

    #[test]
    fn a_custom_select_shortcut_wins() {
        let mut keys = ShortcutKeys::default();
        keys.set(
            Shortcut::BrightnessUp,
            Binding::with(Modifier::Select, Device::X),
        );
        assert_eq!(
            custom_shortcut(&keys, Btn::X, true, false, &[]),
            Some(Action::BrightnessUp)
        );
        assert_eq!(custom_shortcut(&keys, Btn::Up, true, false, &[]), None);
    }

    #[test]
    fn a_button_pair_shortcut_waits_for_both_buttons() {
        let mut keys = ShortcutKeys::default();
        keys.set(
            Shortcut::SaveState,
            Binding::with(Modifier::Button(Device::L1), Device::R1),
        );
        assert_eq!(
            custom_shortcut(&keys, Btn::R1, false, false, &[Btn::L1]),
            Some(Action::SaveState)
        );
        assert_eq!(
            custom_shortcut(&keys, Btn::R1, false, false, &[]),
            None
        );
    }

    #[test]
    fn menu_and_plain_shortcuts_are_matched_by_their_modifier() {
        let mut keys = ShortcutKeys::default();
        keys.set(
            Shortcut::SaveState,
            Binding::with(Modifier::Menu, Device::X),
        );
        keys.set(
            Shortcut::FastForward,
            Binding::plain(Device::X),
        );
        assert_eq!(
            custom_shortcut(&keys, Btn::X, false, true, &[]),
            Some(Action::SaveState)
        );
        assert_eq!(
            custom_shortcut(&keys, Btn::X, false, false, &[]),
            Some(Action::FfStart)
        );
        assert_eq!(custom_shortcut(&keys, Btn::X, true, false, &[]), None);
    }
}

/// A socket whose first frames come from a queue: used when the peer sent
/// cable traffic before this side attached to the game.
struct ReplayLink {
    first: std::collections::VecDeque<Vec<u8>>,
    inner: Box<dyn LinkChannel>,
}

impl ReplayLink {
    fn new(first: Vec<Vec<u8>>, inner: Box<dyn LinkChannel>) -> Self {
        Self {
            first: first.into(),
            inner,
        }
    }
}

impl LinkChannel for ReplayLink {
    fn send(&mut self, flags: i32, buf: &[u8]) {
        self.inner.send(flags, buf);
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        self.first.pop_front().or_else(|| self.inner.try_recv())
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    fn send_end(&mut self) {
        self.inner.send_end();
    }

    fn peer_ended(&self) -> bool {
        self.inner.peer_ended()
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;
    use slot_retro::LoopbackLink;

    #[test]
    fn cable_frames_buffered_before_the_attach_are_replayed_first() {
        let mut inner = LoopbackLink::default();
        inner.send(NETPACKET_RELIABLE, b"live-1");
        inner.send(NETPACKET_RELIABLE, b"live-2");
        let mut link =
            ReplayLink::new(vec![b"hello".to_vec(), b"ready".to_vec()], Box::new(inner));
        assert_eq!(link.try_recv().as_deref(), Some(&b"hello"[..]));
        assert_eq!(link.try_recv().as_deref(), Some(&b"ready"[..]));
        assert_eq!(link.try_recv().as_deref(), Some(&b"live-1"[..]));
        assert_eq!(link.try_recv().as_deref(), Some(&b"live-2"[..]));
        assert_eq!(link.try_recv(), None);
        // Sending still reaches the live peer.
        link.send(NETPACKET_RELIABLE, b"out");
        assert!(!link.is_closed());
    }
}
#[cfg(test)]
mod run_marker_tests {
    use super::Session;

    #[test]
    fn the_marker_only_exists_for_a_game_or_a_link() {
        assert_eq!(Session::run_marker_text(None, false), None);
        assert_eq!(
            Session::run_marker_text(None, true).as_deref(),
            Some("linked=1\n")
        );
        assert_eq!(
            Session::run_marker_text(Some("Emerald"), false).as_deref(),
            Some("cart=Emerald\nlinked=0\n")
        );
        assert_eq!(
            Session::run_marker_text(Some("Emerald"), true).as_deref(),
            Some("cart=Emerald\nlinked=1\n")
        );
    }
}