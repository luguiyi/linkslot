use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use slot_gfx::{Compositor, Draw, ScreenEffect, TexId, OUT_H, OUT_W};

const PAPER: &[u8] = include_bytes!("../assets/paper.png");
use slot_i18n::Language;
use slot_input::{InputSource, Millis};
use slot_power::{Platform, Power};
use slot_store::{format_stamp, Platform as CartPlatform};
use slot_ui::{
    about_face, about_thanks_face, arrows_hint_face, badge_face, bezel_face, cart_shadow, chip_face_for,
    cart_face, chip_shadow_face, date_time_text, gb_cart_shadow, hhmm, hint_face, icon_face,
    large_word_face, menu_face,
    photo_face, quick_caret_face_state, quick_icon_face, quick_label_face_state, quick_legend_faces,
    quick_title_face, quick_value_face, rebuild_count_face, rebuild_title_face,
    set_clock_hint_face, socket_face, title_face, toast_face, wallpaper_face,
    word_face, AboutFields, AboutPage, CartFace, GbShell, Icon, LinkBadge,
    PowerChoice, QuickMenuFaces, QuickRow, QuickValue, RebuildScreen, Toast, UndoFace, ALERT_PX,
    BOLT_PX,
    HUD_ICON_PX, HUD_INK, QUICK_CARD_H, QUICK_CARD_W,
};

use crate::app::{
    App, GameIntroFaces, LinkRow, Phase, SyncFace, SyncFaces, SyncTexts, TransferFaces,
};
use crate::build_info::Build;
use crate::cart_faces::CartFaces;
use crate::face_builder::FaceBuilder;
use crate::label_cache::{self, Rebuild};
use crate::link_art_builder::LinkArtBuilder;
use crate::link_screen::{LinkSprites, Sprite};
use crate::link_start::{LinkFail, LinkStep};
use crate::session::Session;
use crate::wallpaper;

const DOZE_TIMEOUT: Duration = Duration::from_secs(180);

const ALERT_INK: [u8; 3] = [0xf0, 0xb4, 0x3c];

/// The shelf-sync overlay art, in pixels (the cloud face is a height).
const SYNC_CLOUD_PX: f32 = 88.0;
const SYNC_ARROW_PX: f32 = 42.0;
const SYNC_BANNER_PX: f32 = 26.0;
const SYNC_BANNER_MAX_W: f32 = 620.0;
const SYNC_LINE_PX: f32 = 22.0;
const SYNC_LINE_MAX_W: f32 = 600.0;
const SYNC_LABEL_PX: f32 = 24.0;
const SYNC_LABEL_MAX_W: f32 = 220.0;

/// Which language the baked [`UiFaces`] belong to, and which build is in
/// flight.
///
/// Switching the language has to re-rasterise every quick-menu label, value,
/// title, icon tile and legend hint; that takes long enough to be worth a
/// background thread, which in turn needs a rule for "a second switch arrived
/// while the first build was still running".  That rule lives here rather than
/// in the render loop so it can be tested without a GL context.
#[derive(Default)]
pub struct FacesLang {
    /// The language the textures currently on screen were built for.
    baked: Option<Language>,
    /// `(language, generation)` of the build the worker thread is running.
    building: Option<(Language, u64)>,
    /// Bumped once per scheduled build, so a late result can be recognised.
    generation: u64,
}

impl FacesLang {
    pub fn baked(&self) -> Option<Language> {
        self.baked
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_building(&self) -> bool {
        self.building.is_some()
    }

    /// Asks for a build of `lang`, returning its generation when one is needed.
    ///
    /// `None` means "nothing to do": either the textures already match, or a
    /// build is already running.  The second case is what keeps twenty quick
    /// left/right presses down to a single rebuild — the build that is running
    /// finishes in some language, and the next call notices the mismatch and
    /// schedules exactly one more.
    pub fn want(&mut self, lang: Language) -> Option<u64> {
        if self.building.is_some() || self.baked == Some(lang) {
            return None;
        }
        Some(self.begin(lang))
    }

    /// Records a build that is about to start, unconditionally.
    pub fn begin(&mut self, lang: Language) -> u64 {
        self.generation += 1;
        self.building = Some((lang, self.generation));
        self.generation
    }

    /// The worker went away without answering; allow the next frame to retry.
    pub fn abandon(&mut self) {
        self.building = None;
    }

    /// A worker finished.  Returns `false` when the result is stale and the
    /// faces should be dropped rather than shown.
    pub fn finish(&mut self, generation: u64) -> bool {
        let Some((lang, wanted)) = self.building else {
            return false;
        };
        if wanted != generation {
            return false;
        }
        self.building = None;
        self.baked = Some(lang);
        true
    }

    /// The textures were built on this thread because the worker could not be
    /// spawned, so there is nothing in flight any more.
    pub fn baked_now(&mut self, lang: Language) {
        self.building = None;
        self.baked = Some(lang);
    }
}

pub struct Frontend {
    utility_tex: Option<TexId>,
    notice_tex: Option<TexId>,
    notice_shown: Option<(String, String)>,
    utility_shown: Option<(String, Vec<String>, usize, bool)>,
    guide_tex: Option<TexId>,
    guide_shown: Option<(String, usize)>,
    /// The hold-to-leave card, built once: only the bar over it moves.
    hold_tex: Option<TexId>,
    nav_title: Option<TexId>,
    nav_title_shown: String,
    session: Session,
    start: Instant,
    last: Instant,
    draws: Vec<Draw>,
    polaroid_texes: Vec<TexId>,
    title_tex: Option<TexId>,
    faces: FaceBuilder,
    cart_faces: CartFaces,
    rebuild: Option<Rebuild>,
    stale_check: Option<std::sync::mpsc::Receiver<Vec<slot_store::Cart>>>,
    ui_faces: Option<std::sync::mpsc::Receiver<(u64, UiFaces)>>,
    /// Which language the on-screen menu textures are in.
    faces_lang: FacesLang,
    rebuild_faces: RebuildFaces,
    link_art: LinkArtBuilder,
    link_art_done: bool,
    core_asked: Option<String>,
    core_board_tex: Option<TexId>,
    core_lid_tex: Option<TexId>,
    core_built: Option<String>,
    front_deck_shown: Option<String>,
    front_deck_texs: Vec<(TexId, u32, u32)>,
    front_deck_title: Option<(TexId, u32, u32)>,
    front_deck_controls: Vec<(TexId, u32)>,
    front_deck_names: Vec<(TexId, u32, u32)>,
    front_deck_names_key: Option<String>,
    front_deck_counter: Option<(TexId, u32, u32)>,
    front_deck_counter_key: Option<String>,
    front_deck_lang: Option<Language>,
    core_network_tex: Option<TexId>,
    core_network_shown: Option<String>,
    sync_picker_tex: Option<TexId>,
    sync_picker_tex_alt: Option<TexId>,
    sync_picker_tex_lit: Option<TexId>,
    sync_picker_slot: bool,
    sync_picker_shown: Option<String>,
    /// The active machine's screen overlay, uploaded from its Overlay folder.
    fc_mask_tex: Option<TexId>,
    /// Which `dir/name` the texture holds, so a platform or selection change
    /// reloads it.
    fc_mask_tex_name: Option<String>,
    /// Set once when the overlay is missing, so the log is not spammed.
    fc_mask_missing: bool,
    /// The menu rows' value faces (file names) and the names they show.
    fc_mask_value_tex: Option<TexId>,
    fc_mask_value_shown: Option<String>,
    gb_mask_value_tex: Option<TexId>,
    gb_mask_value_shown: Option<String>,
    gbc_mask_value_tex: Option<TexId>,
    gbc_mask_value_shown: Option<String>,
    sfc_mask_value_tex: Option<TexId>,
    sfc_mask_value_shown: Option<String>,
    undo_tex: Option<TexId>,
    switcher: Switcher,
    clocks: Clocks,
    about: AboutFace,
    quick_clock: QuickClock,
    sync_texts: SyncTextCache,
    wifi_tex: Option<TexId>,
    wifi_shown: Option<String>,
    sync_tex: Option<TexId>,
    sync_shown: Option<String>,
    mods_tex: Option<TexId>,
    mods_shown: Option<String>,
    web_tex: Option<TexId>,
    web_shown: Option<(Option<String>, Option<String>)>,
    keyboard_panel_tex: Option<TexId>,
    keyboard_panel_size: (u32, u32),
    keyboard_panel_shown: Option<(usize, String, String, Language)>,
    keyboard_selection_tex: Option<TexId>,
    keyboard_selection_shown: Option<(usize, usize, usize, Language)>,
    keyboard_input_tex: Option<TexId>,
    keyboard_input_size: (u32, u32),
    keyboard_input_shown: Option<(String, bool, Option<String>)>,
    search_candidate_tex: Option<TexId>,
    search_candidate_shown: Option<(String, Vec<String>, usize, usize, usize, Language)>,
    search_results_tex: Option<TexId>,
    search_results_shown: Option<(String, usize, usize, Vec<(CartPlatform, String)>, Language)>,
    /// Opaque full-screen textures for the multi-page conflict overlay; the
    /// key is the app's conflict view (page/direction/locks/versions/error).
    conflict_tex: Option<TexId>,
    conflict_shown: Option<slot_ui::conflict::ConflictView>,
    /// Translucent full-screen modal for a manual mod's `mods.choose`.
    mod_choice_tex: Option<TexId>,
    mod_choice_shown: Option<(slot_ui::mod_choice::ModChoiceView, Language)>,
    /// Full-screen system-upgrade confirmation/progress modal.
    upgrade_tex: Option<TexId>,
    upgrade_shown: Option<(slot_ui::mod_choice::ModChoiceView, Language)>,
    intro_video: Option<IntroVideoJob>,
    /// Every GL texture owned by the current intro card.  Deleted together
    /// when the card closes, so opening videos repeatedly cannot grow the
    /// compositor's texture vector without bound.
    intro_texs: Vec<TexId>,
    transfer_texs: Vec<TexId>,
    transfer_shown: Option<u32>,
    transfer_blank_tex: Option<TexId>,
    transfer_blank_key: Option<String>,
}

struct IntroVideoJob {
    demo: PathBuf,
    events: Receiver<crate::intro_video::VideoEvent>,
    /// The one texture reused for every decoded frame.  Avoids creating a
    /// separate GL texture for all 300 frames of a clip.
    texture: Option<TexId>,
    next_frame: Option<Instant>,
}

/// The rasterised shelf-sync strings, re-uploaded only when the app's text
/// changes (the state machine rebuilds it every frame).
#[derive(Default)]
struct SyncTextCache {
    texts: Option<SyncTexts>,
    banner: Option<TexId>,
    cloud: Option<TexId>,
    local: Option<TexId>,
    upload: Option<TexId>,
    download: Option<TexId>,
    title: Option<TexId>,
    progress_left: Option<TexId>,
    progress_right: Option<TexId>,
    cancel_hint: Option<SyncFace>,
    data_label: Option<SyncFace>,
    net_label: Option<SyncFace>,
    card_label: Option<SyncFace>,
    net_slot_title: Option<SyncFace>,
    net_slot_hint: Option<SyncFace>,
    state_label: Option<SyncFace>,
    sav_label: Option<SyncFace>,
    rail_labels: [Option<SyncFace>; 5],
    check: Option<SyncFace>,
}

#[derive(Default)]
struct RebuildFaces {
    title: Option<(TexId, u32, u32)>,
    count: Option<TexId>,
    count_size: (u32, u32),
    shown: Option<usize>,
}

#[derive(Default)]
struct QuickClock {
    dim: Option<TexId>,
    lit: Option<TexId>,
    shown: String,
}

#[derive(Default)]
struct AboutFace {
    tex: Option<TexId>,
    key: Option<(String, String, bool, AboutPage, String)>,
}

#[derive(Default)]
struct Clocks {
    line: Option<TexId>,
    hint: Option<TexId>,
    shelf: Option<TexId>,
    picked: Option<String>,
    shown: String,
    battery: String,
    battery_tex: Option<TexId>,
    platform: (String, [u8; 3]),
    platform_tex: Option<TexId>,
    playtime: String,
    playtime_tex: Option<TexId>,
}

#[derive(Default)]
struct Switcher {
    open: bool,
    titled: Option<String>,
    empty: Option<TexId>,
}

impl Frontend {
    pub fn boot(platform: Box<dyn Platform>) -> Self {
        Self::boot_with(platform, crate::audio::opened_sink())
    }

    pub fn boot_with(platform: Box<dyn Platform>, sink: Box<dyn crate::audio::AudioSink>) -> Self {
        let now = Instant::now();
        let cart_faces = CartFaces::spawn(platform.root().to_path_buf());
        crate::boot_time::mark("cart workers");
        let mut session = Session::boot_with(platform.root().to_path_buf(), sink);
        crate::boot_time::mark("session");
        let stale_check =
            Self::check_labels(platform.root(), session.app().carts().cloned().collect());
        session
            .app_mut()
            .set_power(Power::new(platform, DOZE_TIMEOUT));
        let lang = session.app().language();
        Frontend {
            utility_tex: None, utility_shown: None,
            notice_tex: None, notice_shown: None,
            guide_tex: None, guide_shown: None,
            hold_tex: None,
            nav_title: None, nav_title_shown: String::new(),
            session,
            start: now,
            last: now,
            draws: Vec::new(),
            polaroid_texes: Vec::new(),
            title_tex: None,
            faces: FaceBuilder::spawn(),
            cart_faces,
            rebuild: None,
            stale_check,
            ui_faces: None,
            faces_lang: FacesLang::default(),
            rebuild_faces: RebuildFaces::default(),
            link_art: LinkArtBuilder::spawn(lang),
            link_art_done: false,
            core_asked: None,
            core_board_tex: None,
            core_lid_tex: None,
            core_built: None,
            front_deck_shown: None,
            front_deck_texs: Vec::new(),
            front_deck_title: None,
            front_deck_controls: Vec::new(),
            front_deck_names: Vec::new(),
            front_deck_names_key: None,
            front_deck_counter: None,
            front_deck_counter_key: None,
            front_deck_lang: None,
            core_network_tex: None,
            core_network_shown: None,
            sync_picker_tex: None,
            sync_picker_tex_alt: None,
            sync_picker_tex_lit: None,
            sync_picker_slot: false,
            sync_picker_shown: None,
            fc_mask_tex: None,
            fc_mask_tex_name: None,
            fc_mask_missing: false,
            fc_mask_value_tex: None,
            fc_mask_value_shown: None,
            gb_mask_value_tex: None,
            gb_mask_value_shown: None,
            gbc_mask_value_tex: None,
            gbc_mask_value_shown: None,
            sfc_mask_value_tex: None,
            sfc_mask_value_shown: None,
            undo_tex: None,
            switcher: Switcher::default(),
            clocks: Clocks::default(),
            about: AboutFace::default(),
            quick_clock: QuickClock::default(),
            sync_texts: SyncTextCache::default(),
            wifi_tex: None,
            wifi_shown: None,
            sync_tex: None,
            sync_shown: None,
            mods_tex: None,
            mods_shown: None,
            web_tex: None,
            web_shown: None,
            keyboard_panel_tex: None,
            keyboard_panel_size: (0, 0),
            keyboard_panel_shown: None,
            keyboard_selection_tex: None,
            keyboard_selection_shown: None,
            keyboard_input_tex: None,
            keyboard_input_size: (0, 0),
            keyboard_input_shown: None,
            search_candidate_tex: None,
            search_candidate_shown: None,
            search_results_tex: None,
            search_results_shown: None,
            conflict_tex: None,
            conflict_shown: None,
            mod_choice_tex: None,
            mod_choice_shown: None,
            upgrade_tex: None,
            upgrade_shown: None,
            intro_video: None,
            intro_texs: Vec::new(),
            transfer_texs: Vec::new(),
            transfer_shown: None,
            transfer_blank_tex: None,
            transfer_blank_key: None,
        }
    }

    fn upload_paper(compositor: &mut Compositor) {
        let mut dec = png::Decoder::new(std::io::Cursor::new(PAPER));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let Ok(mut reader) = dec.read_info() else {
            return;
        };
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let Ok(info) = reader.next_frame(&mut buf) else {
            return;
        };
        if info.color_type != png::ColorType::Grayscale || info.width != info.height {
            return;
        }
        compositor.set_paper(info.width, &buf[..info.buffer_size()]);
    }

    fn draw_guide(&mut self, compositor: &mut Compositor) {
        let Some((title, lines, page, pages)) = self.session.app().guide_page() else {
            self.guide_shown = None;
            return;
        };
        let key = (title.clone(), page);
        if self.guide_shown.as_ref() != Some(&key) {
            let lang = self.session.app().language();
            let face = if let Some(image) = self.session.app().guide_image() {
                slot_ui::guide::image_face(
                    &title,
                    &image.rgba,
                    image.width,
                    image.height,
                    page,
                    pages,
                    lang,
                )
            } else {
                slot_ui::guide::page_face(&title, &lines, page, pages, lang)
            };
            upload(compositor, &mut self.guide_tex, face);
            self.guide_shown = Some(key);
        }
        if let Some(tex) = self.guide_tex {
            self.draws.push(Draw::Tex {
                x: 0.0,
                y: 0.0,
                w: 720.0,
                h: 480.0,
                tex,
                alpha: 1.0,
            });
        }
    }

    fn sync_front_deck(&mut self, compositor: &mut Compositor) {
        let lang = self.session.app().language();
        let carts = self.session.app().front_deck_carts();
        let Some(carts) = carts else {
            if self.front_deck_shown.take().is_some() {
                for tex in self.front_deck_texs.drain(..) {
                    compositor.delete_texture(tex.0);
                }
                if let Some((tex, _, _)) = self.front_deck_title.take() {
                    compositor.delete_texture(tex);
                }
                for (tex, _) in self.front_deck_controls.drain(..) {
                    compositor.delete_texture(tex);
                }
                for (tex, _, _) in self.front_deck_names.drain(..) {
                    compositor.delete_texture(tex);
                }
                if let Some((tex, _, _)) = self.front_deck_counter.take() {
                    compositor.delete_texture(tex);
                }
                self.session.app_mut().set_front_deck_faces(Vec::new());
                self.session
                    .app_mut()
                    .set_front_deck_texts(None, Vec::new(), Vec::new(), None);
                self.front_deck_lang = None;
                self.front_deck_names_key = None;
                self.front_deck_counter_key = None;
            }
            return;
        };
        let mut key = String::new();
        for cart in &carts {
            key.push_str(&cart.stem);
            key.push('|');
            if let Some(front) = &cart.front {
                key.push_str(&front.to_string_lossy());
            }
            key.push(';');
        }
        if self.front_deck_shown.as_deref() != Some(key.as_str()) {
            for tex in self.front_deck_texs.drain(..) {
                compositor.delete_texture(tex.0);
            }
            let mut texs = Vec::new();
            for cart in &carts {
                let face = slot_ui::cart_face(cart);
                let tex = compositor.create_texture(face.w, face.h, &face.rgba);
                texs.push((tex, face.w, face.h));
            }
            self.front_deck_texs = texs;
            self.session
                .app_mut()
                .set_front_deck_faces(self.front_deck_texs.clone());
            self.front_deck_shown = Some(key);
        }

        if self.front_deck_lang != Some(lang) {
            if let Some((old, _, _)) = self.front_deck_title.take() {
                compositor.delete_texture(old);
            }
            for (old, _) in self.front_deck_controls.drain(..) {
                compositor.delete_texture(old);
            }
            let ink = [0xf6, 0xe9, 0xcf];
            let title = large_word_face(
                slot_i18n::t(lang, "device.app.front_deck.title"),
                30.0,
                ink,
            );
            let title_size = (title.w, title.h);
            let title = compositor.create_texture(title.w, title.h, &title.rgba);
            self.front_deck_title = Some((title, title_size.0, title_size.1));
            let controls = [
                ("B", "device.app.common.back"),
                ("← →", "device.app.core.switch"),
                ("A", "device.app.front_deck.apply"),
                ("X", "device.app.front_deck.close"),
            ]
            .into_iter()
            .map(|(key, text)| {
                let face = hint_face(key, slot_i18n::t(lang, text));
                let tex = compositor.create_texture(face.w, face.h, &face.rgba);
                (tex, face.w)
            })
            .collect();
            self.front_deck_controls = controls;
            self.front_deck_lang = Some(lang);
            self.front_deck_names_key = None;
        }

        let names = self
            .session
            .app()
            .front_deck_names()
            .unwrap_or_default();
        let names_key = format!("{lang:?}|{}", names.join("\u{1f}"));
        if self.front_deck_names_key.as_deref() != Some(names_key.as_str()) {
            for (old, _, _) in self.front_deck_names.drain(..) {
                compositor.delete_texture(old);
            }
            for name in &names {
                let face = large_word_face(name, 21.0, [0xf6, 0xe9, 0xcf]);
                let tex = compositor.create_texture(face.w, face.h, &face.rgba);
                self.front_deck_names.push((tex, face.w, face.h));
            }
            self.front_deck_names_key = Some(names_key);
        }

        if let Some((count, selected)) = self.session.app().front_deck_status() {
            let counter = format!("{} / {}", selected + 1, count);
            if self.front_deck_counter_key.as_deref() != Some(counter.as_str()) {
                if let Some((old, _, _)) = self.front_deck_counter.take() {
                    compositor.delete_texture(old);
                }
                let face = large_word_face(&counter, 22.0, [0xf6, 0xe9, 0xcf]);
                let size = (face.w, face.h);
                let tex = compositor.create_texture(face.w, face.h, &face.rgba);
                self.front_deck_counter = Some((tex, size.0, size.1));
                self.front_deck_counter_key = Some(counter);
            }
        }

        self.session.app_mut().set_front_deck_texts(
            self.front_deck_title,
            self.front_deck_controls.clone(),
            self.front_deck_names.clone(),
            self.front_deck_counter,
        );
    }

    pub fn upload_faces(&mut self, compositor: &mut Compositor) {
        self.upload_boot_faces(compositor);
        if let Some((generation, faces)) = self.ui_faces.take().and_then(|rx| rx.recv().ok()) {
            self.faces_lang.finish(generation);
            self.place_ui_faces(compositor, faces);
        }
    }

    pub fn upload_boot_faces(&mut self, compositor: &mut Compositor) {
        crate::boot_time::mark("boot faces start");
        Self::upload_paper(compositor);
        crate::boot_time::mark("paper");
        if self.rebuild.is_none() {
            crate::boot_time::mark("cart sync start");
            self.cart_faces.sync(self.session.app_mut(), compositor);
        }
        crate::boot_time::mark("cart faces");
        let lang = self.session.app().language();
        let generation = self.faces_lang.begin(lang);
        match self.spawn_ui_faces(lang, generation) {
            Some(rx) => self.ui_faces = Some(rx),
            None => {
                // No worker thread: build here so the menu still gets labels.
                let faces = UiFaces::build(lang);
                self.faces_lang.baked_now(lang);
                self.place_ui_faces(compositor, faces);
            }
        }
        crate::boot_time::mark("ui face build started");
        let icons = Icon::ALL
            .iter()
            .map(|i| {
                let f = icon_face(*i, HUD_ICON_PX, HUD_INK);
                compositor.create_texture(f.w, f.h, &f.rgba)
            })
            .collect();
        self.session.app_mut().set_icon_faces(icons);
        let alert = icon_face(Icon::Alert, ALERT_PX, ALERT_INK);
        let alert = compositor.create_texture(alert.w, alert.h, &alert.rgba);
        self.session.app_mut().set_alert_face(alert);
        let legend = legend_faces(
            compositor,
            &[
                ("B", slot_i18n::t(lang, "device.app.common.back")),
                ("Y", slot_i18n::t(lang, "device.app.common.delete")),
                ("A", slot_i18n::t(lang, "device.app.common.load")),
            ],
        );
        self.session.app_mut().set_legend_faces(legend);
        let hint = set_clock_hint_face(lang);
        self.clocks.hint = Some(compositor.create_texture(hint.w, hint.h, &hint.rgba));
        let shadow = cart_shadow();
        let id = compositor.create_texture(shadow.w, shadow.h, &shadow.rgba);
        self.session.app_mut().set_cart_shadow(id);
        let notched = gb_cart_shadow(GbShell::Notched);
        let notched = compositor.create_texture(notched.w, notched.h, &notched.rgba);
        let rounded = gb_cart_shadow(GbShell::Rounded);
        let rounded = compositor.create_texture(rounded.w, rounded.h, &rounded.rgba);
        self.session.app_mut().set_gb_cart_shadows(notched, rounded);
        let bolt = icon_face(Icon::Charging, BOLT_PX, HUD_INK);
        let bolt_id = compositor.create_texture(bolt.w, bolt.h, &bolt.rgba);
        self.session.app_mut().set_bolt_face(bolt_id);
        // Shelf header: Wi-Fi state icon and the bottom key legend. These are
        // static faces uploaded once at boot.
        let wifi = slot_ui::wifi_face(slot_ui::WIFI_PX, HUD_INK);
        let (wifi_w, wifi_h) = (wifi.w, wifi.h);
        let wifi_id = compositor.create_texture(wifi_w, wifi_h, &wifi.rgba);
        self.session
            .app_mut()
            .set_wifi_face(wifi_id, wifi_w, wifi_h);
        let shelf_left = sized_legend_faces(
            compositor,
            &[
                ("Y", slot_i18n::t(lang, "device.app.shelf.hold_sync")),
                ("X", slot_i18n::t(lang, "device.app.shelf.search")),
            ],
        );
        let shelf_right = sized_legend_faces(
            compositor,
            &[("B", slot_i18n::t(lang, "device.app.shelf.hold_intro"))],
        );
        self.session
            .app_mut()
            .set_shelf_legend_faces(shelf_left, shelf_right);
        let cloud = sync_face(
            compositor,
            slot_ui::sync_overlay::cloud_face(SYNC_CLOUD_PX),
        );
        let up = sync_face(
            compositor,
            slot_ui::sync_overlay::arrow_face(slot_ui::sync_overlay::Arrow::Up, SYNC_ARROW_PX),
        );
        let down = sync_face(
            compositor,
            slot_ui::sync_overlay::arrow_face(slot_ui::sync_overlay::Arrow::Down, SYNC_ARROW_PX),
        );
        self.session.app_mut().set_sync_art(cloud, up, down);
        crate::boot_time::mark("shelf faces");
        let backdrop = slot_ui::shelf_backdrop_face();
        let backdrop = compositor.create_texture(backdrop.w, backdrop.h, &backdrop.rgba);
        self.session.app_mut().set_shelf_backdrop(backdrop);
        for kind in slot_ui::CollectionKind::ALL {
            let overlay = slot_ui::collection_overlay_face(kind);
            let overlay = compositor.create_texture(overlay.w, overlay.h, &overlay.rgba);
            self.session.app_mut().set_collection_overlay(kind, overlay);
        }
        // The rack's nameplate, upstream's `System/linkSlot/assets/carts/slot_back_<machine>.png`
        // when the card supplies it, and a built-in plate otherwise. The plate
        // sits behind the cart inside the slot; the full-screen wall stays the
        // card's wallpaper.
        let root = self.session.app().root().map(Path::to_path_buf);
        let carts_dir = root.as_ref().map(|root| root.join("System/linkSlot/assets/carts"));
        // A "plate pack" mod can ship `slot_back_<machine>.png` in its own
        // directory. Card art wins, then the lowest-`order` enabled mod that
        // has the file, then the built-in engraved plate.
        let plate_mods: Vec<PathBuf> = root
            .as_deref()
            .map(|root| {
                let mut mods = slot_mods::list(root).unwrap_or_default();
                mods.retain(|entry| entry.enabled && entry.error.is_none());
                mods.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
                mods.into_iter()
                    .map(|entry| root.join("linkSlot/mods").join(entry.id))
                    .collect()
            })
            .unwrap_or_default();
        for kind in slot_ui::CollectionKind::ALL {
            let file = kind.slot_back_file();
            let card_art = carts_dir
                .as_deref()
                .and_then(|dir| slot_ui::decode(&dir.join(file)))
                .or_else(|| {
                    plate_mods
                        .iter()
                        .find_map(|dir| slot_ui::decode(&dir.join(file)))
                });
            let (rgba, w, h) = match card_art {
                Some((rgba, w, h)) => {
                    let face = slot_ui::slot_plate_from_art(&rgba, w, h);
                    (face.rgba, face.w, face.h)
                }
                None => {
                    let face = slot_ui::collection_plate_face(kind, lang);
                    (face.rgba, face.w, face.h)
                }
            };
            let tex = compositor.create_texture(w, h, &rgba);
            self.session
                .app_mut()
                .set_collection_slot_back(kind, Some((tex, w, h)));
        }
        crate::boot_time::mark("plates");
        crate::boot_time::mark("wallpaper start");
        self.upload_wallpaper(compositor);
        crate::boot_time::mark("wallpaper");
    }

    fn poll_ui_faces(&mut self, compositor: &mut Compositor) {
        if let Some(rx) = &self.ui_faces {
            match rx.try_recv() {
                Ok((generation, faces)) => {
                    self.ui_faces = None;
                    // A stale result is dropped: a newer build is already on its
                    // way and would paint over it a frame later anyway.
                    if self.faces_lang.finish(generation) {
                        self.place_ui_faces(compositor, faces);
                        crate::boot_time::mark("ui faces");
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // The worker vanished without answering: forget the request
                    // so the next frame can try again instead of waiting for a
                    // result that will never arrive.
                    self.ui_faces = None;
                    self.faces_lang.abandon();
                }
            }
        }
        // The language can change while this runs — from 系统 → 语言 or from the
        // Web page, which the app notices through the file's mtime.  Either way
        // the menu textures were rasterised in the old language and must be
        // rebuilt before the next frame is drawn.
        let lang = self.session.app().language();
        if let Some(generation) = self.faces_lang.want(lang) {
            match self.spawn_ui_faces(lang, generation) {
                Some(rx) => self.ui_faces = Some(rx),
                None => {
                    let faces = UiFaces::build(lang);
                    self.faces_lang.baked_now(lang);
                    self.place_ui_faces(compositor, faces);
                }
            }
        }
    }

    /// Starts the whole-menu texture build on its own thread.
    ///
    /// `None` means the thread could not be spawned; the caller builds inline
    /// rather than leaving the menu without labels.
    fn spawn_ui_faces(
        &mut self,
        lang: Language,
        generation: u64,
    ) -> Option<std::sync::mpsc::Receiver<(u64, UiFaces)>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("slot-ui-faces".into())
            .spawn(move || {
                let _ = tx.send((generation, UiFaces::build(lang)));
            });
        match spawned {
            Ok(_) => Some(rx),
            Err(error) => {
                eprintln!("slot: ui faces thread: {error}");
                None
            }
        }
    }

    fn place_ui_faces(&mut self, compositor: &mut Compositor, faces: UiFaces) {
        let mut cart = |f: CartFace| compositor.create_texture(f.w, f.h, &f.rgba);
        let link_badges = faces.link_badges.into_iter().map(&mut cart).collect();
        let toasts = faces.toasts.into_iter().map(&mut cart).collect();
        let sockets = faces.sockets.into_iter().map(&mut cart).collect();
        let chips: Vec<Vec<TexId>> = faces
            .chips
            .into_iter()
            .map(|row| row.into_iter().map(&mut cart).collect())
            .collect();
        let blanks: Vec<TexId> = faces.blank_chips.into_iter().map(&mut cart).collect();
        let shadow = cart(faces.chip_shadow);
        let mut up = |f: UndoFace| (compositor.create_texture(f.w, f.h, &f.rgba), f.w, f.h);
        let lines = faces.shutdown.into_iter().map(&mut up).collect();
        let cards = faces.quick_cards.map(&mut up);
        let power_cards = faces.power_cards.map(&mut up);
        let power_labels = faces
            .power_labels
            .into_iter()
            .map(|v| v.map(&mut up))
            .collect();
        let power_icons = faces
            .power_icons
            .into_iter()
            .map(|v| {
                v.map(|f| {
                    up(UndoFace {
                        rgba: f.rgba,
                        w: f.w,
                        h: f.h,
                    })
                })
            })
            .collect();
        let power_title = up(faces.power_title);
        let labels = faces
            .quick_labels
            .into_iter()
            .map(|v| v.map(&mut up))
            .collect();
        let values = faces
            .quick_values
            .into_iter()
            .map(|v| v.map(&mut up))
            .collect();
        let carets = faces.quick_carets.map(|side| side.map(&mut up));
        let icons = faces
            .quick_icons
            .into_iter()
            .map(|v| {
                v.map(|f| {
                    up(UndoFace {
                        rgba: f.rgba,
                        w: f.w,
                        h: f.h,
                    })
                })
            })
            .collect();
        let titles = faces.quick_titles.map(&mut up);
        let legend = faces.quick_legend.map(|f| {
            let (tex, w, _) = up(f);
            (tex, w)
        });
        let core_legend = faces
            .core_legend
            .into_iter()
            .map(|f| {
                let (tex, w, _) = up(f);
                (tex, w)
            })
            .collect();
        let roles = faces.link_roles.into_iter().map(&mut up).collect();
        let linked = up(faces.linked);
        let link_confirm = up(faces.link_confirm);
        let link_legend = faces
            .link_legend
            .into_iter()
            .map(|f| {
                let (tex, w, _) = up(f);
                (tex, w)
            })
            .collect();
        let steps = faces.link_steps.into_iter().map(&mut up).collect();
        let fails = faces.link_fails.into_iter().map(&mut up).collect();
        let app = self.session.app_mut();
        app.set_link_badge_faces(link_badges);
        app.set_shutdown_faces(lines);
        app.set_power_menu_faces(power_cards, power_labels, power_icons, power_title);
        app.set_quick_menu_faces(QuickMenuFaces {
            cards,
            labels,
            values,
            carets,
            icons,
            titles,
            legend,
        });
        app.set_core_part_faces(sockets, chips, blanks, shadow);
        app.set_core_legend_faces(core_legend);
        app.set_link_menu_faces(roles);
        app.set_link_linked_face(linked);
        app.set_link_dialog_faces(link_confirm);
        app.set_link_legend_faces(link_legend);
        app.set_link_step_faces(steps);
        app.set_link_fail_faces(fails);
        app.set_toast_faces(toasts);

        // The shelf legend lives outside UiFaces, so it was uploaded only at
        // boot and kept the old language after a switch.  Rebuild it with the
        // same language as the freshly rebuilt menu.
        let lang = self.session.app().language();
        let shelf_left = sized_legend_faces(
            compositor,
            &[
                ("Y", slot_i18n::t(lang, "device.app.shelf.hold_sync")),
                ("X", slot_i18n::t(lang, "device.app.shelf.search")),
            ],
        );
        let shelf_right = sized_legend_faces(
            compositor,
            &[("B", slot_i18n::t(lang, "device.app.shelf.hold_intro"))],
        );
        self.session
            .app_mut()
            .set_shelf_legend_faces(shelf_left, shelf_right);
    }

    fn upload_wallpaper(&mut self, compositor: &mut Compositor) {
        let app = self.session.app();
        let seed = app.wall_secs().unsigned_abs();
        let root = app.root().map(Path::to_path_buf);
        // A theme pack's own wallpaper replaces the `Wallpapers/` rotation.
        let picked = root
            .as_deref()
            .and_then(|root| crate::theme_pack::active(root).and_then(|pack| pack.wallpaper()))
            .or_else(|| root.as_deref().and_then(|root| wallpaper::pick(root, seed)));
        crate::boot_time::mark("wallpaper picked");
        let Some(rgba) = picked.as_deref().and_then(wallpaper_face) else {
            return;
        };
        crate::boot_time::mark("wallpaper decoded");
        let id = compositor.create_texture(OUT_W, OUT_H, &rgba);
        self.session.app_mut().set_wallpaper(id);
    }

    pub fn upload_bezel(&mut self, compositor: &mut Compositor, panel: (u32, u32)) {
        if !slot_gfx::framed(panel) {
            return;
        }
        slot_ui::set_shelf_slack((OUT_W * panel.1 / panel.0).saturating_sub(OUT_H) as f32);
        let Some(path) = self
            .session
            .app()
            .root()
            .and_then(|root| crate::bezel::pick(root, panel))
        else {
            return;
        };
        if let Some(rgba) = bezel_face(&path, panel.0, panel.1) {
            compositor.set_bezel(panel.0, panel.1, &rgba);
            eprintln!("slot: bezel {}", path.display());
        }
    }

    fn sync_transfer(&mut self, compositor: &mut Compositor) {
        let Some(view) = self.session.app().transfer_view() else {
            if !self.transfer_texs.is_empty() {
                for texture in self.transfer_texs.drain(..) {
                    compositor.delete_texture(texture);
                }
                self.transfer_shown = None;
            }
            self.release_transfer_blank(compositor);
            return;
        };
        self.sync_transfer_blank(compositor);
        let key = view.percent;
        if self.transfer_shown.as_ref() == Some(&key) {
            return;
        }
        let percent = large_word_face(&format!("{}%", view.percent), 54.0, [89, 231, 244]);
        let assets: Vec<_> = view.assets.iter().map(|asset| word_face(asset)).collect();

        for texture in self.transfer_texs.drain(..) {
            compositor.delete_texture(texture);
        }
        let percent_tex = compositor.create_texture(percent.w, percent.h, &percent.rgba);
        self.transfer_texs.push(percent_tex);
        let mut assets_out = Vec::with_capacity(assets.len());
        for asset in assets {
            let tex = compositor.create_texture(asset.w, asset.h, &asset.rgba);
            assets_out.push((tex, asset.w, asset.h));
            self.transfer_texs.push(tex);
        }
        self.session.app_mut().set_transfer_faces(TransferFaces {
            percent: (percent_tex, percent.w, percent.h),
            assets: assets_out,
        });
        self.transfer_shown = Some(key);
    }

    fn sync_transfer_blank(&mut self, compositor: &mut Compositor) {
        let Some(cart) = self.session.app().transfer_preview_cart() else {
            self.release_transfer_blank(compositor);
            return;
        };
        let key = format!("{:?}|{:?}|{:?}", cart.platform, cart.stem, cart.shell);
        if self.transfer_blank_key.as_deref() == Some(&key) {
            return;
        }
        let face = cart_face(&cart);
        let tex = compositor.create_texture(face.w, face.h, &face.rgba);
        if let Some(old) = self.transfer_blank_tex.take() {
            compositor.delete_texture(old);
        }
        self.transfer_blank_tex = Some(tex);
        self.transfer_blank_key = Some(key);
        self.session
            .app_mut()
            .set_transfer_blank_face(Some((tex, face.w, face.h)));
    }

    fn release_transfer_blank(&mut self, compositor: &mut Compositor) {
        if let Some(texture) = self.transfer_blank_tex.take() {
            compositor.delete_texture(texture);
        }
        if self.transfer_blank_key.take().is_some() {
            self.session.app_mut().set_transfer_blank_face(None);
        }
    }

    /// Rasterise the X-hold game intro once: plate, title, description and an
    /// uncropped cover thumbnail.
    fn sync_game_intro(&mut self, compositor: &mut Compositor) {
        let lang = self.session.app().language();
        if self.session.app().game_intro().is_none() {
            self.release_intro(compositor);
            return;
        }
        self.poll_intro_video(compositor);
        let Some(intro) = self.session.app().game_intro() else {
            self.release_intro(compositor);
            return;
        };
        let scroll = intro.scroll;
        if let Some(faces) = intro.faces.as_ref() {
            if faces.body_scroll != scroll {
                let description = intro.description.clone();
                let width = faces.body.1;
                let old_body = faces.body.0;
                let (body_face, max_scroll) = intro_scrolling_face(
                    &description,
                    width,
                    270,
                    scroll,
                    crate::app::INTRO_INK,
                );
                let body_tex =
                    compositor.create_texture(body_face.w, body_face.h, &body_face.rgba);
                compositor.delete_texture(old_body);
                self.intro_texs.retain(|texture| *texture != old_body);
                self.intro_texs.push(body_tex);
                self.session.app_mut().set_game_intro_body(
                    (body_tex, body_face.w, body_face.h),
                    scroll,
                );
                self.session.app_mut().set_game_intro_scroll(max_scroll);
            }
            return;
        }
        let description = intro.description.clone();
        let demo = intro.demo.clone();
        let platform = intro.platform;
        let shell = intro.shell;
        let title = intro.title.clone();
        let plate = slot_ui::switch_style::rounded_face(
            640,
            420,
            18.0,
            crate::app::INTRO_PAPER,
            Some(crate::app::INTRO_PAPER_BORDER),
        );
        // The card no longer repeats the cartridge label: its white title art
        // was the "shelf title" showing through on the paper. Only a demo video
        // keeps a side column.
        let (body_w, body_h) = if demo.is_some() {
            (300, 270)
        } else {
            (576, 270)
        };
        let (body_face, max_scroll) =
            intro_scrolling_face(&description, body_w, body_h, 0, crate::app::INTRO_INK);
        let paper_rgb = [
            crate::app::INTRO_PAPER[0],
            crate::app::INTRO_PAPER[1],
            crate::app::INTRO_PAPER[2],
        ];
        let hint = if max_scroll > 0 {
            slot_ui::hint_face_ink(
                "B",
                slot_i18n::t(lang, "device.app.intro.back_scroll"),
                crate::app::INTRO_INK,
                paper_rgb,
                crate::app::INTRO_INK,
            )
        } else {
            slot_ui::hint_face_ink(
                "B",
                slot_i18n::t(lang, "device.app.common.back"),
                crate::app::INTRO_INK,
                paper_rgb,
                crate::app::INTRO_INK,
            )
        };
        let hint_delete = slot_ui::hint_face_ink(
            "Y",
            slot_i18n::t(lang, "device.app.intro.delete_cart"),
            crate::app::INTRO_INK,
            paper_rgb,
            crate::app::INTRO_INK,
        );
        // A previous intro should already have released these; drain anything
        // left so reopening cannot silently accumulate GL textures.
        self.release_intro(compositor);
        let title_face = intro_title_face(&title);
        let title_tex = compositor.create_texture(title_face.w, title_face.h, &title_face.rgba);
        self.intro_texs.push(title_tex);
        let plate_tex = compositor.create_texture(plate.w, plate.h, &plate.rgba);
        let back_face = intro_back_face(platform, shell, &title, lang);
        let back_tex = compositor.create_texture(back_face.w, back_face.h, &back_face.rgba);
        self.intro_texs.push(back_tex);
        self.session
            .app_mut()
            .set_game_intro_back((back_tex, back_face.w, back_face.h));
        let body_tex = compositor.create_texture(body_face.w, body_face.h, &body_face.rgba);
        let hint_tex = compositor.create_texture(hint.w, hint.h, &hint.rgba);
        let hint_delete_tex =
            compositor.create_texture(hint_delete.w, hint_delete.h, &hint_delete.rgba);
        self.intro_texs
            .extend([plate_tex, title_tex, body_tex, hint_tex, hint_delete_tex]);
        self.session
            .app_mut()
            .set_game_intro_faces(GameIntroFaces {
                plate: (plate_tex, plate.w, plate.h),
                title: (title_tex, title_face.w, title_face.h),
                body: (body_tex, body_face.w, body_face.h),
                body_scroll: 0,
                cover: None,
                video: None,
                hint: (hint_tex, hint.w, hint.h),
                hint_delete: (hint_delete_tex, hint_delete.w, hint_delete.h),
            });
        self.session.app_mut().set_game_intro_scroll(max_scroll);
        self.start_intro_video(demo);
    }

    fn start_intro_video(&mut self, demo: Option<PathBuf>) {
        let Some(demo) = demo else {
            self.intro_video = None;
            return;
        };
        if self
            .intro_video
            .as_ref()
            .is_some_and(|job| job.demo == demo)
        {
            return;
        }
        // A two-frame queue gives the decoder backpressure: if texture upload
        // falls behind, it waits instead of buffering hundreds of RGBA frames.
        let (tx, events) = mpsc::sync_channel(2);
        let path = demo.clone();
        let spawned = std::thread::Builder::new()
            .name("slot-intro-video".into())
            .spawn(move || {
                lower_intro_priority();
                let _ = crate::intro_video::decode_stream(
                    &path,
                    |rgba, width, height, duration_ms| {
                        tx.send(crate::intro_video::VideoEvent::Frame {
                            rgba,
                            width,
                            height,
                            duration_ms,
                        })
                        .is_ok()
                    },
                );
                let _ = tx.send(crate::intro_video::VideoEvent::Done);
            });
        self.intro_video = spawned.ok().map(|_| IntroVideoJob {
            demo,
            events,
            texture: None,
            next_frame: None,
        });
    }

    fn poll_intro_video(&mut self, compositor: &mut Compositor) {
        let Some(job) = self.intro_video.as_mut() else {
            return;
        };
        let now = Instant::now();
        if job.next_frame.is_some_and(|at| now < at) {
            return;
        }
        let mut finished = false;
        match job.events.try_recv() {
            Ok(crate::intro_video::VideoEvent::Frame {
                rgba,
                width,
                height,
                duration_ms,
            }) => {
                let duration = std::time::Duration::from_millis(u64::from(duration_ms.max(16)));
                if let Some(tex) = job.texture {
                    // One texture updated in place: no per-frame GL allocation
                    // and no 65MB of retained video textures.
                    compositor.update_texture(tex, width, height, &rgba);
                } else {
                    let tex = compositor.create_texture(width, height, &rgba);
                    job.texture = Some(tex);
                    self.intro_texs.push(tex);
                    self.session
                        .app_mut()
                        .set_game_intro_video(crate::app::IntroVideoFaces {
                            frames: vec![(tex, duration_ms)],
                            width,
                            height,
                        });
                }
                job.next_frame = Some(now + duration);
            }
            Ok(crate::intro_video::VideoEvent::Done) | Err(TryRecvError::Disconnected) => {
                finished = true;
            }
            Err(TryRecvError::Empty) => {}
        }
        if finished {
            self.intro_video = None;
        }
    }

    /// Stop the decoder and release every texture owned by the closed intro.
    fn release_intro(&mut self, compositor: &mut Compositor) {
        self.intro_video = None;
        for texture in self.intro_texs.drain(..) {
            compositor.delete_texture(texture);
        }
    }

    /// Upload the active machine's chosen mask once and tell the app whether
    /// to draw it. The art lives on the card, like the reference project's
    /// overlay ring: the package ships only the README that explains it.
    fn sync_masks(&mut self, compositor: &mut Compositor) {
        let app = self.session.app();
        let wanted = app.mask_wanted();
        let name = app.current_mask().map(str::to_string);
        let dir = app.mask_dir_name().unwrap_or("FC");
        if wanted {
            if let Some(name) = name {
                let key = format!("{dir}/{name}");
                if self.fc_mask_tex.is_none()
                    || self.fc_mask_tex_name.as_deref() != Some(key.as_str())
                {
                    let root = self
                        .session
                        .app()
                        .root()
                        .map(std::path::Path::to_path_buf);
                    let loaded = root
                        .map(|root| root.join("Overlay").join(dir).join(&name))
                        .filter(|path| path.is_file())
                        .and_then(|path| bezel_face(&path, OUT_W, OUT_H).map(|rgba| (path, rgba)));
                    match loaded {
                        Some((path, rgba)) => {
                            self.fc_mask_tex =
                                Some(compositor.create_texture(OUT_W, OUT_H, &rgba));
                            self.fc_mask_tex_name = Some(key);
                            self.fc_mask_missing = false;
                            eprintln!("slot: {dir} mask {}", path.display());
                        }
                        None => {
                            self.fc_mask_tex = None;
                            self.fc_mask_tex_name = None;
                            if !self.fc_mask_missing {
                                self.fc_mask_missing = true;
                                eprintln!("slot: no {dir} mask at Overlay/{dir}/{name}");
                            }
                        }
                    }
                }
                self.session.app_mut().set_mask_face(self.fc_mask_tex);
            }
        } else if self.session.app().mask_face().is_some() {
            self.session.app_mut().set_mask_face(None);
        }

        // Each mask row prints its chosen file name, so its value is a dynamic
        // face rather than one of the fixed QuickValues.
        let label = self.session.app().mask_label_for(CartPlatform::Fc);
        if self.fc_mask_value_shown.as_deref() != Some(label.as_str()) {
            let face = quick_value_face(&label, false);
            let tex = compositor.create_texture(face.w, face.h, &face.rgba);
            self.fc_mask_value_tex = Some(tex);
            self.fc_mask_value_shown = Some(label);
            self.session
                .app_mut()
                .set_fc_mask_value(Some((tex, face.w, face.h)));
        }
        let label = self.session.app().mask_label_for(CartPlatform::Gb);
        if self.gb_mask_value_shown.as_deref() != Some(label.as_str()) {
            let face = quick_value_face(&label, false);
            let tex = compositor.create_texture(face.w, face.h, &face.rgba);
            self.gb_mask_value_tex = Some(tex);
            self.gb_mask_value_shown = Some(label);
            self.session
                .app_mut()
                .set_gb_mask_value(Some((tex, face.w, face.h)));
        }
        let label = self.session.app().mask_label_for(CartPlatform::Gbc);
        if self.gbc_mask_value_shown.as_deref() != Some(label.as_str()) {
            let face = quick_value_face(&label, false);
            let tex = compositor.create_texture(face.w, face.h, &face.rgba);
            self.gbc_mask_value_tex = Some(tex);
            self.gbc_mask_value_shown = Some(label);
            self.session
                .app_mut()
                .set_gbc_mask_value(Some((tex, face.w, face.h)));
        }
        let label = self.session.app().mask_label_for(CartPlatform::Sfc);
        if self.sfc_mask_value_shown.as_deref() != Some(label.as_str()) {
            let face = quick_value_face(&label, false);
            let tex = compositor.create_texture(face.w, face.h, &face.rgba);
            self.sfc_mask_value_tex = Some(tex);
            self.sfc_mask_value_shown = Some(label);
            self.session
                .app_mut()
                .set_sfc_mask_value(Some((tex, face.w, face.h)));
        }
    }

    pub fn render(&mut self, compositor: &mut Compositor, window: (u32, u32)) {
        compositor.fit(window);
        self.compose(compositor);
        compositor.end_frame(window);
    }

    pub fn compose(&mut self, compositor: &mut Compositor) {
        // One language for every panel this call re-rasterises, so a switch
        // mid-frame cannot mix the two.
        let lang = self.session.app().language();
        self.sync_front_deck(compositor);
        // The canvas the picture is composited into: the Advance's 240x160, or a
        // Famicom's 256x224. It has to be told before the picture/source rects
        // are, because those are ratios of *this* canvas and the pipeline sizes
        // the game texture from it.
        let (frame_w, frame_h) = self.session.app().frame_size();
        compositor.set_frame_size(frame_w, frame_h);
        compositor.set_blue_light(self.session.app().blue_light());
        compositor.set_picture(self.session.app().picture_rect());
        compositor.set_screen_fit(self.session.app().picture_fit());
        compositor.set_screen_effect(match self.session.app().screen_shader() {
            slot_store::Shader::Off => ScreenEffect::None,
            slot_store::Shader::Lcd3x => ScreenEffect::Lcd3x,
            slot_store::Shader::Grid => ScreenEffect::Grid,
            slot_store::Shader::Dot => ScreenEffect::Dot,
            slot_store::Shader::Simpletex => ScreenEffect::Simpletex,
            slot_store::Shader::FourPass => ScreenEffect::FourPass,
        });
        compositor.set_shake(self.session.app().screen_shake());
        compositor.set_screen_power(self.session.app().screen_power());
        compositor.set_frame_lift(self.session.app().frame_lift());
        compositor.set_frame_split(self.session.app().frame_split());
        compositor.set_game_source_rect(self.session.app().source_rect());
        compositor.begin_frame();
        if self.draw_rebuild(compositor) {
            return;
        }
        if let Some(frame) = self.session.frame() {
            let (w, h) = (frame.width(), frame.height());
            compositor.upload_game(&frame, w, h);
            crate::latency::taken();
        }
        self.cart_faces.sync(self.session.app_mut(), compositor);
        sync_clock(self.session.app_mut(), compositor, &mut self.clocks);
        sync_about(self.session.app_mut(), compositor, &mut self.about);
        sync_quick_clock(self.session.app_mut(), compositor, &mut self.quick_clock);
        sync_core_picker(
            self.session.app_mut(),
            compositor,
            &self.faces,
            &mut self.core_asked,
            &mut self.core_board_tex,
            &mut self.core_lid_tex,
            &mut self.core_built,
        );
        sync_network_chip(
            self.session.app_mut(),
            compositor,
            &mut self.core_network_tex,
            &mut self.core_network_shown,
        );
        sync_sync_picker_face(
            self.session.app_mut(),
            compositor,
            &mut self.sync_picker_tex,
            &mut self.sync_picker_tex_alt,
            &mut self.sync_picker_tex_lit,
            &mut self.sync_picker_slot,
            &mut self.sync_picker_shown,
        );
        if !self.link_art_done {
            if let Some(art) = self.link_art.take() {
                let mut up = |f: &slot_ui::CartFace| Sprite {
                    tex: compositor.create_texture(f.w, f.h, &f.rgba),
                    w: f.w,
                    h: f.h,
                };
                let sprites = LinkSprites {
                    port: up(&art.port),
                    plug_host: up(&art.plug_host),
                    plug_join: up(&art.plug_join),
                    adapter: up(&art.adapter),
                    arcs_right: [
                        up(&art.arcs_right[0]),
                        up(&art.arcs_right[1]),
                        up(&art.arcs_right[2]),
                    ],
                    arcs_left: [
                        up(&art.arcs_left[0]),
                        up(&art.arcs_left[1]),
                        up(&art.arcs_left[2]),
                    ],
                    clicks: up(&art.clicks),
                    arrow_left: up(&art.arrow_left),
                    arrow_right: up(&art.arrow_right),
                };
                self.session.app_mut().set_link_sprites(sprites);
                self.link_art_done = true;
            }
        }
        sync_switcher(
            self.session.app_mut(),
            compositor,
            Faces {
                pool: &mut self.polaroid_texes,
                title: &mut self.title_tex,
                undo: &mut self.undo_tex,
            },
            &mut self.switcher,
        );
        sync_shelf_sync(self.session.app_mut(), compositor, &mut self.sync_texts);
        self.sync_masks(compositor);
        self.sync_game_intro(compositor);
        self.sync_transfer(compositor);
        self.draws.clear();
        self.session.app().draw(&mut self.draws);
        if self.session.app().navigation_visible() {
            if let Some(text) = self.session.app().rom_title() {
                if self.nav_title_shown != text || self.nav_title.is_none() {
                    let face = slot_ui::navigation::title(text);
                    upload(compositor, &mut self.nav_title, face);
                    self.nav_title_shown = text.to_string();
                }
                if let Some(tex) = self.nav_title {
                    self.draws.push(Draw::Tex { x: 28.0, y: 14.0, w: 664.0, h: 76.0, tex, alpha: 1.0 });
                }
            }
        }
        if let Some(content) = self.session.app().utility_text() {
            let notice = self.session.app().utility_is_notice();
            let key = (content.0.clone(), content.1.clone(), content.2, notice);
            if self.utility_shown.as_ref() != Some(&key) {
                let face = if notice {
                    slot_ui::navigation::notice(&content.0, &content.1, lang)
                } else {
                    slot_ui::navigation::panel_with_limit(
                        &content.0,
                        &content.1,
                        content.2,
                        self.session.app().utility_rows_per_page(),
                        lang,
                    )
                };
                upload(compositor, &mut self.utility_tex, face);
                self.utility_shown = Some(key);
            }
            if let Some(tex) = self.utility_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }
        self.draw_guide(compositor);
        // The exit prompt sits over the menu it was opened from. Only the bar
        // is per-frame: the card behind it is one texture for the whole session.
        if let Some(progress) = self.session.app().hold_exit() {
            if self.hold_tex.is_none() {
                upload(
                    compositor,
                    &mut self.hold_tex,
                    slot_ui::navigation::hold_card(lang),
                );
            }
            if let Some(tex) = self.hold_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
                let [bx, by, bw, bh] = slot_ui::navigation::HOLD_BAR;
                let filled = bw * progress.clamp(0.0, 1.0);
                if filled > 0.0 {
                    self.draws.push(Draw::Rect {
                        x: bx,
                        y: by,
                        w: filled,
                        h: bh,
                        colour: slot_ui::navigation::hold_fill(),
                    });
                }
            }
        }
        self.draw_notice(compositor);
        self.draw_network_overlays(compositor);
        // The conflict pages are opaque; draw them last so the shelf and every
        // other overlay can never show through them.
        self.draw_conflict_page(compositor);
        self.draw_mod_choice(compositor);
        self.draw_upgrade(compositor);
        compositor.draw_list(&self.draws);
    }

    /// Render (or re-use) the three-page conflict texture and blit it
    /// full-screen.  The cache key is the whole [`slot_ui::conflict::ConflictView`]
    /// so paging, direction changes, locks, versions and errors all refresh the
    /// texture; the same [`TexId`] is updated in place, which avoids a flash.
    /// The non-blocking link / transfer notice: one card under the status bar
    /// that fades out on its own.  It never takes input and never covers the
    /// game, so a dropped peer no longer freezes play.
    fn draw_notice(&mut self, compositor: &mut Compositor) {
        let Some((title, detail, shown)) = self.session.app().notice_view() else {
            self.notice_shown = None;
            return;
        };
        let key = (title.to_string(), detail.to_string());
        if self.notice_shown.as_ref() != Some(&key) {
            let face = slot_ui::notice::notice_face(&key.0, &key.1);
            upload(compositor, &mut self.notice_tex, face);
            self.notice_shown = Some(key);
        }
        let alpha = slot_ui::notice::notice_alpha(shown, self.now());
        if alpha <= 0.0 {
            return;
        }
        if let Some(tex) = self.notice_tex {
            let (x, y, w, h) = slot_ui::notice::notice_rect();
            self.draws.push(Draw::Tex {
                x: x.round(),
                y: y.round(),
                w,
                h,
                tex,
                alpha,
            });
        }
    }

    fn draw_conflict_page(&mut self, compositor: &mut Compositor) {
        // One language for every panel this call re-rasterises, so a switch
        // mid-frame cannot mix the two.
        let lang = self.session.app().language();
        let Some(ui) = self.session.app().conflict_ui() else {
            self.conflict_shown = None;
            return;
        };
        let view = conflict_view(&ui);
        if self.conflict_shown.as_ref() != Some(&view) {
            let face = slot_ui::conflict::conflict_face(&view, lang);
            match self.conflict_tex {
                Some(id) => compositor.update_texture(id, face.w, face.h, &face.rgba),
                None => {
                    self.conflict_tex =
                        Some(compositor.create_texture(face.w, face.h, &face.rgba));
                }
            }
            self.conflict_shown = Some(view);
        }
        if let Some(tex) = self.conflict_tex {
            self.draws.push(Draw::Tex {
                x: 0.0,
                y: 0.0,
                w: 720.0,
                h: 480.0,
                tex,
                alpha: 1.0,
            });
        }
    }

    /// Render (or re-use) the generic manual-mod choice modal.  It is drawn
    /// after the conflict page and network overlays so it is always on top.
    fn draw_mod_choice(&mut self, compositor: &mut Compositor) {
        let Some(ui) = self.session.app().mod_choice_ui() else {
            self.mod_choice_shown = None;
            return;
        };
        let view = slot_ui::mod_choice::ModChoiceView {
            title: ui.title,
            body: ui.body,
            options: ui.options,
            selected: ui.selected,
        };
        if self.mod_choice_shown.as_ref() != Some(&(view.clone(), self.session.app().language())) {
            let face = slot_ui::mod_choice::mod_choice_face(
                &view,
                self.session.app().language(),
            );
            match self.mod_choice_tex {
                Some(id) => compositor.update_texture(id, face.w, face.h, &face.rgba),
                None => {
                    self.mod_choice_tex =
                        Some(compositor.create_texture(face.w, face.h, &face.rgba));
                }
            }
            self.mod_choice_shown = Some((view, self.session.app().language()));
        }
        if let Some(tex) = self.mod_choice_tex {
            self.draws.push(Draw::Tex {
                x: 0.0,
                y: 0.0,
                w: 720.0,
                h: 480.0,
                tex,
                alpha: 1.0,
            });
        }
    }

    /// The system upgrade modal is drawn above every other overlay.
    fn draw_upgrade(&mut self, compositor: &mut Compositor) {
        let Some(ui) = self.session.app().upgrade_ui() else {
            self.upgrade_shown = None;
            return;
        };
        let view = slot_ui::mod_choice::ModChoiceView {
            title: ui.title,
            body: ui.body,
            options: ui.options,
            selected: ui.selected,
        };
        if self.upgrade_shown.as_ref() != Some(&(view.clone(), self.session.app().language())) {
            let face = slot_ui::mod_choice::mod_choice_face(
                &view,
                self.session.app().language(),
            );
            match self.upgrade_tex {
                Some(id) => compositor.update_texture(id, face.w, face.h, &face.rgba),
                None => {
                    self.upgrade_tex =
                        Some(compositor.create_texture(face.w, face.h, &face.rgba));
                }
            }
            self.upgrade_shown = Some((view, self.session.app().language()));
        }
        if let Some(tex) = self.upgrade_tex {
            self.draws.push(Draw::Tex {
                x: 0.0,
                y: 0.0,
                w: 720.0,
                h: 480.0,
                tex,
                alpha: 1.0,
            });
        }
    }

    /// WiFi screen, Web address dialog and the on-screen keyboard are overlay
    /// faces drawn on top of whatever the app has already composed.
    fn draw_network_overlays(&mut self, compositor: &mut Compositor) {
        // One language for every panel this call re-rasterises, so a switch
        // mid-frame cannot mix the two.
        let lang = self.session.app().language();
        let wifi = self.session.app().wifi_ui.as_ref().map(|ui| {
            let mut key = format!(
                "{}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{:?}|{}",
                ui.status,
                ui.connected,
                ui.error,
                ui.selected,
                ui.offset,
                ui.manual,
                ui.connecting,
                ui.radio_on,
                ui.radio_known,
                ui.radio_pending,
                ui.auto_off_minutes
            );
            for row in &ui.rows {
                key.push_str(&format!(
                    "|{}:{}:{}:{}",
                    row.ssid, row.signal, row.secured, row.saved
                ));
            }
            let view = slot_ui::network::WifiPanelView {
                connected: ui.connected.as_deref(),
                status: &ui.status,
                error: ui.error.as_deref(),
                rows: &ui.rows,
                offset: ui.offset,
                selected: ui.selected,
                manual_row: ui.radio_on && ui.selected == ui.rows.len() + 2,
                connecting: ui.connecting.as_deref(),
                radio_on: ui.radio_on,
                radio_known: ui.radio_known,
                auto_off_label: crate::linkcfg::auto_off_label(ui.auto_off_minutes),
            };
            (key, slot_ui::network::wifi_panel(&view, lang))
        });
        if let Some((key, face)) = wifi {
            if self.wifi_shown.as_ref() != Some(&key) {
                upload(compositor, &mut self.wifi_tex, face);
                self.wifi_shown = Some(key);
            }
            if let Some(tex) = self.wifi_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }

        let sync = self.session.app().sync_ui.as_ref().map(|ui| {
            let mut key = format!(
                "{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{}|{:?}",
                ui.active,
                ui.selected,
                ui.status,
                ui.tested,
                ui.testing_id,
                ui.switching,
                ui.error,
                ui.profiles.len(),
                ui.needs_setup,
                lang
            );
            let mut rows = Vec::with_capacity(ui.profiles.len());
            let mut details = Vec::with_capacity(ui.profiles.len());
            for profile in &ui.profiles {
                let detail = if profile.remote_root.is_empty() {
                    profile.url.clone()
                } else {
                    format!("{} · {}", profile.url, profile.remote_root)
                };
                key.push_str(&format!(
                    "|{}:{}:{}",
                    profile.id, profile.password_set, detail
                ));
                details.push(detail);
                rows.push(slot_ui::sync_menu::SyncMenuRow {
                    name: &profile.name,
                    detail: "",
                    active: profile.id == ui.active,
                    password_set: profile.password_set,
                });
            }
            for (row, detail) in rows.iter_mut().zip(details.iter()) {
                row.detail = detail;
            }
            let view = slot_ui::sync_menu::SyncMenuView {
                rows: &rows,
                selected: ui.selected,
                test_ready: self.session.app().sync_test_ready(),
                testing: ui.job.is_some(),
                active: ui
                    .profiles
                    .get(ui.selected)
                    .is_some_and(|profile| profile.id == ui.active),
                can_switch: self.session.app().sync_can_switch(),
                status: Some(&ui.status),
                error: ui.error.as_deref(),
                needs_setup: ui.needs_setup,
            };
            (key, slot_ui::sync_menu::sync_menu_face(&view, lang))
        });
        if let Some((key, face)) = sync {
            if self.sync_shown.as_ref() != Some(&key) {
                upload(compositor, &mut self.sync_tex, face);
                self.sync_shown = Some(key);
            }
            if let Some(tex) = self.sync_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }

        let mods = self.session.app().mods_ui.as_ref().map(|ui| {
            let mut key = format!("{}|{}|{:?}|{:?}", ui.selected, ui.offset, ui.error, lang);
            let mut rows = Vec::with_capacity(ui.rows.len());
            for (index, entry) in ui.rows.iter().enumerate() {
                key.push_str(&format!(
                    "|{}:{}:{}:{}:{}:{}:{:?}",
                    entry.id,
                    entry.name,
                    entry.version,
                    entry.action_count,
                    entry.enabled,
                    entry.order,
                    entry.error
                ));
                rows.push(slot_ui::mods_menu::ModsMenuRow {
                    name: &entry.name,
                    version: &entry.version,
                    action_count: entry.action_count,
                    enabled: entry.enabled,
                    error: entry.error.as_deref(),
                    rank: index + 1,
                });
            }
            let view = slot_ui::mods_menu::ModsMenuView {
                rows: &rows,
                selected: ui.selected,
                offset: ui.offset,
                error: ui.error.as_deref(),
            };
            (key, slot_ui::mods_menu::mods_menu(&view, self.session.app().language()))
        });
        if let Some((key, face)) = mods {
            if self.mods_shown.as_ref() != Some(&key) {
                upload(compositor, &mut self.mods_tex, face);
                self.mods_shown = Some(key);
            }
            if let Some(tex) = self.mods_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }

        let web = self.session.app().web_popup.as_ref().map(|popup| match popup {
            crate::app::WebPopup::Starting => (None, None),
            crate::app::WebPopup::Started { url } => (Some(url.clone()), None),
            crate::app::WebPopup::Failed { message } => (None, Some(message.clone())),
        });
        if let Some((url, error)) = web {
            if self.web_shown.as_ref() != Some(&(url.clone(), error.clone())) {
                upload(
                    compositor,
                    &mut self.web_tex,
                    slot_ui::network::web_dialog(url.as_deref(), error.as_deref(), lang),
                );
                self.web_shown = Some((url, error));
            }
            if let Some(tex) = self.web_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }

        let keyboard = self.session.app().keyboard.as_ref().map(|kb| {
            let lang = self.session.app().language();
            let panel_key = (
                kb.cursor.layout,
                kb.title.clone(),
                kb.hint.clone(),
                lang,
            );
            let selection_key = (kb.cursor.layout, kb.cursor.row, kb.cursor.col, lang);
            let shown_text = if matches!(kb.purpose, crate::app::KeyboardPurpose::GameSearch) {
                format!("{}{}", kb.text, kb.composition)
            } else {
                kb.text.clone()
            };
            let input_key = (shown_text.clone(), kb.masked, kb.error.clone());
            let candidate_key = if matches!(kb.purpose, crate::app::KeyboardPurpose::GameSearch)
                && !kb.composition.is_empty()
            {
                Some((
                    kb.composition.clone(),
                    kb.candidates.clone(),
                    kb.candidate_selected,
                    kb.candidate_page,
                    kb.candidate_pages,
                    lang,
                ))
            } else {
                None
            };
            (
                panel_key,
                selection_key,
                kb.cursor,
                input_key,
                shown_text,
                kb.masked,
                kb.error.clone(),
                candidate_key,
            )
        });
        if let Some((
            panel_key,
            selection_key,
            cursor,
            input_key,
            shown_text,
            masked,
            error,
            candidate_key,
        )) = keyboard
        {
            // Build faces only when their cache key actually changed.  The old
            // path rebuilt the full keyboard and input faces every frame even
            // when nothing had moved, which made key input feel laggy on the
            // H700.
            if self.keyboard_panel_shown.as_ref() != Some(&panel_key) {
                let panel = slot_ui::keyboard::panel(
                    panel_key.0,
                    &panel_key.1,
                    &panel_key.2,
                    panel_key.3,
                );
                upload(compositor, &mut self.keyboard_panel_tex, panel);
                self.keyboard_panel_size = (720, 480);
                self.keyboard_panel_shown = Some(panel_key);
            }
            if self.keyboard_selection_shown.as_ref() != Some(&selection_key) {
                let face = slot_ui::keyboard::selection_face(
                    cursor,
                    selection_key.3,
                );
                upload(compositor, &mut self.keyboard_selection_tex, face);
                self.keyboard_selection_shown = Some(selection_key);
            }
            if self.keyboard_input_shown.as_ref() != Some(&input_key) {
                let input =
                    slot_ui::keyboard::input_line(&shown_text, masked, error.as_deref());
                let size = (input.w, input.h);
                upload(compositor, &mut self.keyboard_input_tex, input);
                self.keyboard_input_size = size;
                self.keyboard_input_shown = Some(input_key);
            }
            if let Some(tex) = self.keyboard_panel_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: self.keyboard_panel_size.0 as f32,
                    h: self.keyboard_panel_size.1 as f32,
                    tex,
                    alpha: 1.0,
                });
            }
            if let Some(tex) = self.keyboard_selection_tex {
                let (x, y, w, h) = slot_ui::keyboard::row_rect(cursor);
                self.draws.push(Draw::Tex {
                    x,
                    y,
                    w,
                    h,
                    tex,
                    alpha: 1.0,
                });
            }
            if let Some(tex) = self.keyboard_input_tex {
                let (ix, iy) = slot_ui::keyboard::INPUT_POS;
                self.draws.push(Draw::Tex {
                    x: ix,
                    y: iy,
                    w: self.keyboard_input_size.0 as f32,
                    h: self.keyboard_input_size.1 as f32,
                    tex,
                    alpha: 1.0,
                });
            }
            if let Some(candidate_key) = candidate_key {
                if self.search_candidate_shown.as_ref() != Some(&candidate_key) {
                    let face = slot_ui::search::candidate_bar(
                        &candidate_key.0,
                        &candidate_key.1,
                        candidate_key.2,
                        candidate_key.3,
                        candidate_key.4,
                        candidate_key.5,
                    );
                    upload(compositor, &mut self.search_candidate_tex, face);
                    self.search_candidate_shown = Some(candidate_key);
                }
                if let Some(tex) = self.search_candidate_tex {
                    let (x, y) = slot_ui::search::CANDIDATE_POS;
                    let (w, h) = slot_ui::search::CANDIDATE_SIZE;
                    self.draws.push(Draw::Tex {
                        x,
                        y,
                        w,
                        h,
                        tex,
                        alpha: 1.0,
                    });
                }
            }
        }

        let search_view = self.session.app().search_results.as_ref().map(|results| {
            let rows: Vec<(CartPlatform, String)> = results
                .hits
                .iter()
                .map(|hit| (hit.platform, hit.label.clone()))
                .collect();
            (
                results.query.clone(),
                results.selected,
                results.offset,
                rows,
                self.session.app().language(),
            )
        });
        if let Some((query, selected, offset, rows, lang)) = search_view {
            let key = (query.clone(), selected, offset, rows.clone(), lang);
            if self.search_results_shown.as_ref() != Some(&key) {
                let face = slot_ui::search::results_face(&query, &rows, selected, offset, lang);
                upload(compositor, &mut self.search_results_tex, face);
                self.search_results_shown = Some(key);
            }
            if let Some(tex) = self.search_results_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }
    }

    pub fn drive_emulator(&mut self) {
        self.session.set_driven(true);
    }

    /// Called once before the process leaves the main loop.
    pub fn shutdown(&mut self) {
        self.session.app_mut().flush_playtime_now();
        self.session.app_mut().stop_web();
    }

    pub fn step_emulator(&self, present: Duration, timeout: Duration) -> bool {
        self.session.step_emulator(present, timeout)
    }

    pub fn rebuilding_labels(&self) -> bool {
        self.rebuild.is_some()
    }

    fn check_labels(
        root: &std::path::Path,
        carts: Vec<slot_store::Cart>,
    ) -> Option<std::sync::mpsc::Receiver<Vec<slot_store::Cart>>> {
        crate::boot_time::mark("labels: check start");
        let (tx, rx) = std::sync::mpsc::channel();
        let root = root.to_path_buf();
        std::thread::Builder::new()
            .name("slot-label-check".into())
            .spawn(move || {
                let _ = tx.send(label_cache::stale(&root, carts.iter()));
                crate::boot_time::mark("labels: check done");
            })
            .ok()
            .map(|_| rx)
    }

    fn poll_label_check(&mut self) {
        let Some(rx) = &self.stale_check else {
            return;
        };
        let stale = match rx.try_recv() {
            Ok(stale) => stale,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        self.stale_check = None;
        let Some(root) = self.session.app().root().map(|r| r.to_path_buf()) else {
            return;
        };
        if !stale.is_empty() {
            eprintln!("slot: label cache: {} new labels", stale.len());
            self.rebuild = Some(Rebuild::start(&root, stale));
        }
    }

    fn draw_rebuild(&mut self, compositor: &mut Compositor) -> bool {
        // One language for every panel this call re-rasterises, so a switch
        // mid-frame cannot mix the two.
        let lang = self.session.app().language();
        self.poll_label_check();
        self.poll_ui_faces(compositor);
        let Some(rebuild) = &mut self.rebuild else {
            return false;
        };
        if rebuild.stalled() && !rebuild.finished() {
            eprintln!(
                "slot: label cache: no label finished for a while, going on at {} / {}",
                rebuild.done(),
                rebuild.total()
            );
            self.rebuild = None;
            self.last = Instant::now();
            return false;
        }
        if rebuild.finished() {
            self.rebuild = None;
            self.last = Instant::now();
            return false;
        }
        let faces = &mut self.rebuild_faces;
        let title = *faces.title.get_or_insert_with(|| {
            let f = rebuild_title_face(slot_i18n::t(lang, "device.app.rebuild.caching_labels"));
            (compositor.create_texture(f.w, f.h, &f.rgba), f.w, f.h)
        });
        let done = rebuild.done();
        if faces.shown != Some(done) {
            let f = rebuild_count_face(&format!("{done} / {}", rebuild.total()));
            faces.count_size = (f.w, f.h);
            upload(compositor, &mut faces.count, f);
            faces.shown = Some(done);
        }
        let screen = RebuildScreen {
            title: Some(title),
            count: faces
                .count
                .map(|tex| (tex, faces.count_size.0, faces.count_size.1)),
            fraction: done as f32 / rebuild.total().max(1) as f32,
        };
        self.draws.clear();
        screen.draw(&mut self.draws);
        self.draw_guide(compositor);
        if let Some(content) = self.session.app().utility_text() {
            let notice = self.session.app().utility_is_notice();
            let key = (content.0.clone(), content.1.clone(), content.2, notice);
            if self.utility_shown.as_ref() != Some(&key) {
                let face = if notice {
                    slot_ui::navigation::notice(&content.0, &content.1, lang)
                } else {
                    slot_ui::navigation::panel_with_limit(
                        &content.0,
                        &content.1,
                        content.2,
                        self.session.app().utility_rows_per_page(),
                        lang,
                    )
                };
                upload(compositor, &mut self.utility_tex, face);
                self.utility_shown = Some(key);
            }
            if let Some(tex) = self.utility_tex {
                self.draws.push(Draw::Tex {
                    x: 0.0,
                    y: 0.0,
                    w: 720.0,
                    h: 480.0,
                    tex,
                    alpha: 1.0,
                });
            }
        }
        compositor.draw_list(&self.draws);
        true
    }

    pub fn advance(&mut self, input: &mut dyn InputSource) {
        let now = self.now();
        let events = input.poll(now);
        if self.rebuild.is_some() {
            return;
        }
        self.session.feed(events, now);
        let dt = self.last.elapsed().as_secs_f32().min(0.05);
        self.last = Instant::now();
        self.session.update(dt);
    }

    pub fn app(&self) -> &crate::app::App {
        self.session.app()
    }

    pub fn core_settling(&self) -> bool {
        self.session.core_settling()
    }

    pub fn advance_at(&mut self, input: &mut dyn InputSource, now: Millis, dt: f32) {
        let events = input.poll(now);
        self.session.feed(events, now);
        self.session.update(dt.clamp(0.0, 0.05));
    }

    fn now(&self) -> Millis {
        self.start.elapsed().as_millis() as Millis
    }

    pub fn powering_off(&self) -> bool {
        self.session.app().ready_to_power_off()
    }

    pub fn restarting(&self) -> bool {
        self.session.app().ready_to_restart()
    }

    pub fn repairing(&self) -> bool {
        self.session.app().ready_to_repair()
    }

    pub fn repair(&mut self) {
        self.session.app_mut().repair();
    }

    pub fn restart(&mut self) {
        self.session.app_mut().restart();
    }

    pub fn poweroff(&mut self) {
        self.session.app_mut().poweroff();
    }
}

fn legend_faces(compositor: &mut Compositor, legend: &[(&str, &str)]) -> Vec<TexId> {
    legend
        .iter()
        .map(|(key, label)| {
            let f = hint_face(key, label);
            compositor.create_texture(f.w, f.h, &f.rgba)
        })
        .collect()
}

/// The shelf legend needs each face's width as well as its texture.
fn lower_intro_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid);
        if tid > 0 {
            let _ = libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, 10);
        }
    }
}

fn intro_scrolling_face(
    text: &str,
    w: u32,
    h: u32,
    scroll: usize,
    ink: [u8; 3],
) -> (UndoFace, usize) {
    let px = 20.0;
    let line_h = slot_ui::text::line_height(px);
    let visible = ((h as f32 / line_h).floor() as usize).max(1);
    let lines = slot_ui::text::wrap_lines(text, px, w as f32 - 10.0);
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let mut shown = Vec::with_capacity(visible);
    for index in 0..visible {
        shown.push(lines.get(scroll + index).cloned().unwrap_or_default());
    }
    let layout = slot_ui::text::Layout {
        lines: shown,
        px,
        tracking: slot_ui::text::tracking(px),
    };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    slot_ui::text::draw_left(&mut rgba, w, h, &layout, ink, 4.0);
    (UndoFace { rgba, w, h }, max_scroll)
}

/// Text-only face in a chosen ink colour, used for the printed label.
fn intro_label_face(text: &str, w: u32, h: u32, px: f32, colour: [u8; 3]) -> UndoFace {
    let line_h = slot_ui::text::line_height(px).max(1.0);
    let visible = ((h as f32 / line_h).floor() as usize).max(1);
    let lines: Vec<String> = slot_ui::text::wrap_lines(text, px, w as f32 - 20.0)
        .into_iter()
        .take(visible)
        .collect();
    let layout = slot_ui::text::Layout {
        lines,
        px,
        tracking: slot_ui::text::tracking(px),
    };
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    slot_ui::text::draw_left(&mut rgba, w, h, &layout, colour, 10.0);
    UndoFace { rgba, w, h }
}

/// The card's title, printed in ink on the paper. One line, shrunk to fit.
fn intro_title_face(title: &str) -> UndoFace {
    let (w, h) = (560u32, 34u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    if let Some(font) = slot_ui::text::label_font() {
        let layout = slot_ui::text::fit(font, title, w as f32 - 4.0, 1, 22.0, 13.0);
        slot_ui::text::draw_left(&mut rgba, w, h, &layout, crate::app::INTRO_INK, 1.0);
    }
    UndoFace { rgba, w, h }
}

fn inside_rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) -> bool {
    let cx = x.clamp(x0 + radius, x1 - radius);
    let cy = y.clamp(y0 + radius, y1 - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

fn fill_rounded(rgba: &mut [u8], w: u32, h: u32, inset: f32, radius: f32, colour: [u8; 4]) {
    let (hw, hh) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            if inside_rounded(x as f32 + 0.5, y as f32 + 0.5, inset, inset, hw - inset, hh - inset, radius) {
                let at = ((y * w + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&colour);
            }
        }
    }
}

fn blend_at(dst: &mut [u8], dw: u32, dh: u32, src: &[u8], sw: u32, sh: u32, at_x: u32, at_y: u32) {
    for y in 0..sh {
        for x in 0..sw {
            let dx = at_x + x;
            let dy = at_y + y;
            if dx >= dw || dy >= dh {
                continue;
            }
            let si = ((y * sw + x) * 4) as usize;
            let alpha = src[si + 3] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let di = ((dy * dw + dx) * 4) as usize;
            for channel in 0..3 {
                let under = dst[di + channel] as f32;
                dst[di + channel] =
                    (under * (1.0 - alpha) + src[si + channel] as f32 * alpha).round() as u8;
            }
            dst[di + 3] = dst[di + 3].max(src[si + 3]);
        }
    }
}

/// The back of the cartridge: a gold shell edge around a printed paper label
/// with the heading and a short summary of the description.
fn fill_rect(rgba: &mut [u8], w: u32, h: u32, x: f32, y: f32, rw: f32, rh: f32, colour: [u8; 4]) {
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + rw) as u32).min(w);
    let y1 = ((y + rh) as u32).min(h);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let at = ((yy * w + xx) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&colour);
        }
    }
}

fn fill_circle(rgba: &mut [u8], w: u32, h: u32, cx: f32, cy: f32, radius: f32, colour: [u8; 3]) {
    let r2 = radius * radius;
    for yy in 0..h {
        for xx in 0..w {
            let dx = xx as f32 + 0.5 - cx;
            let dy = yy as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                let at = ((yy * w + xx) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
            }
        }
    }
}

/// The back of the cartridge: the same shell as the front, plus a small
/// printed label (title, barcode, serial, region) and the paper tab of the
/// manual tucked under it.  The description itself stays in the card.
fn intro_back_face(
    platform: slot_store::Platform,
    shell: [u8; 3],
    title: &str,
    lang: Language,
) -> UndoFace {
    use slot_store::Platform;
    // Art pixels, not the 85% display size: the label box is in art space and
    // the compositor scales the whole texture down to the cart rect anyway.
    let (w, h) = slot_ui::cart_art_box(platform);
    let code = match platform {
        Platform::Gba => "AGB",
        Platform::Gb => "DMG",
        Platform::Gbc => "CGB",
        Platform::Fc => "HVC",
        Platform::Sfc => "SHVC",
        Platform::Md => "MD",
    };
    let scale = |factor: f32| {
        [
            (shell[0] as f32 * factor).clamp(0.0, 255.0) as u8,
            (shell[1] as f32 * factor).clamp(0.0, 255.0) as u8,
            (shell[2] as f32 * factor).clamp(0.0, 255.0) as u8,
        ]
    };
    let shell_dark = scale(0.60);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    fill_rounded(
        &mut rgba,
        w,
        h,
        0.0,
        12.0,
        [shell_dark[0], shell_dark[1], shell_dark[2], 255],
    );
    fill_rounded(&mut rgba, w, h, 2.0, 10.0, [shell[0], shell[1], shell[2], 255]);
    // Top-to-bottom moulded shade so the coloured plastic still reads as a
    // solid object rather than a flat swatch.
    for y in 0..h {
        let t = y as f32 / (h.max(2) - 1) as f32;
        let f = 1.10 - 0.20 * t;
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            if rgba[at + 3] == 0 {
                continue;
            }
            for c in 0..3 {
                rgba[at + c] = (rgba[at + c] as f32 * f).clamp(0.0, 255.0) as u8;
            }
        }
    }
    // screw dimples
    for (cx, cy) in [
        (16.0, 16.0),
        (w as f32 - 16.0, 16.0),
        (16.0, h as f32 - 16.0),
        (w as f32 - 16.0, h as f32 - 16.0),
    ] {
        fill_circle(&mut rgba, w, h, cx, cy, 4.0, shell_dark);
    }
    // side grip ridges
    for offset in [52.0f32, 58.0, 64.0] {
        let top = offset;
        let bottom = h as f32 - offset;
        if bottom > top + 20.0 {
            fill_rect(&mut rgba, w, h, 7.0, top, 1.0, bottom - top, [shell_dark[0], shell_dark[1], shell_dark[2], 255]);
            fill_rect(&mut rgba, w, h, w as f32 - 8.0, top, 1.0, bottom - top, [shell_dark[0], shell_dark[1], shell_dark[2], 255]);
        }
    }

    let (lx, ly, lw, lh) = crate::app::intro_back_label_box(platform);
    let paper = crate::app::INTRO_PAPER;
    let border = crate::app::INTRO_PAPER_BORDER;
    fill_rect(&mut rgba, w, h, lx - 1.5, ly - 1.5, lw + 3.0, lh + 3.0, border);
    fill_rect(
        &mut rgba,
        w,
        h,
        lx,
        ly,
        lw,
        lh,
        [paper[0], paper[1], paper[2], 255],
    );

    let ink = crate::app::INTRO_INK;
    let title_text: String = title.trim().chars().take(22).collect();
    let title_face = intro_label_face(&title_text, (lw - 16.0).max(20.0) as u32, 14, 9.0, ink);
    blend_at(
        &mut rgba,
        w,
        h,
        &title_face.rgba,
        title_face.w,
        title_face.h,
        (lx + 8.0) as u32,
        (ly + 4.0) as u32,
    );

    // barcode
    let bar_top = ly + lh * 0.42;
    let bar_h = (lh * 0.26).max(9.0);
    let mut bar_x = lx + 8.0;
    let pattern = [2u32, 1, 3, 1, 2, 4, 1, 2, 1, 3, 2, 1, 4, 2];
    for (index, bar) in pattern.iter().enumerate() {
        if index % 2 == 0 {
            fill_rect(
                &mut rgba,
                w,
                h,
                bar_x,
                bar_top,
                *bar as f32 * 1.2,
                bar_h,
                [ink[0], ink[1], ink[2], 255],
            );
        }
        bar_x += (*bar as f32 + 1.0) * 1.2;
    }

    let slug: String = title
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(10)
        .collect::<String>()
        .to_uppercase();
    let serial = format!("{code}-{}", if slug.is_empty() { "SLOT" } else { &slug });
    let serial_face = intro_label_face(
        &serial,
        (lw - 16.0).max(20.0) as u32,
        10,
        6.5,
        ink,
    );
    blend_at(
        &mut rgba,
        w,
        h,
        &serial_face.rgba,
        serial_face.w,
        serial_face.h,
        (lx + 8.0) as u32,
        (bar_top + bar_h + 1.0) as u32,
    );
    let note_face = intro_label_face(
        slot_i18n::t(lang, "device.app.intro.has_manual"),
        (lw - 16.0).max(20.0) as u32,
        9,
        6.0,
        [110, 110, 114],
    );
    blend_at(
        &mut rgba,
        w,
        h,
        &note_face.rgba,
        note_face.w,
        note_face.h,
        (lx + 8.0) as u32,
        (bar_top + bar_h + 12.0) as u32,
    );

    // The paper tab now peeks out from the TOP edge of the label: that is
    // where the sheet is pulled out from in the intro animation.
    let tab_w = (lw * 0.34).max(18.0);
    fill_rect(
        &mut rgba,
        w,
        h,
        lx + lw / 2.0 - tab_w / 2.0,
        ly - 5.0,
        tab_w,
        6.0,
        [246, 242, 228, 255],
    );
    UndoFace { rgba, w, h }
}

fn sized_legend_faces(compositor: &mut Compositor, legend: &[(&str, &str)]) -> Vec<(TexId, u32)> {
    legend
        .iter()
        .map(|(key, label)| {
            let f = hint_face(key, label);
            let tex = compositor.create_texture(f.w, f.h, &f.rgba);
            (tex, f.w)
        })
        .collect()
}

struct UiFaces {
    link_badges: Vec<CartFace>,
    shutdown: Vec<UndoFace>,
    power_cards: [UndoFace; 2],
    power_labels: Vec<[UndoFace; 2]>,
    power_icons: Vec<[CartFace; 2]>,
    power_title: UndoFace,
    quick_cards: [UndoFace; 2],
    quick_labels: Vec<[UndoFace; 2]>,
    quick_values: Vec<[UndoFace; 2]>,
    quick_carets: [[UndoFace; 2]; 2],
    quick_icons: Vec<[CartFace; 2]>,
    quick_titles: [UndoFace; QuickRow::QUICK_PAGES],
    quick_legend: [UndoFace; 3],
    sockets: Vec<CartFace>,
    chips: Vec<Vec<CartFace>>,
    blank_chips: Vec<CartFace>,
    chip_shadow: CartFace,
    core_legend: Vec<UndoFace>,
    link_roles: Vec<UndoFace>,
    linked: UndoFace,
    link_confirm: UndoFace,
    link_legend: Vec<UndoFace>,
    link_steps: Vec<UndoFace>,
    link_fails: Vec<UndoFace>,
    toasts: Vec<CartFace>,
}

impl UiFaces {
    /// Rasterises every menu texture in `lang`.
    ///
    /// The language is a parameter rather than read from `App` inside: this runs
    /// on a worker thread, and a value captured at spawn time is the only way to
    /// be sure the whole batch comes out in one language.
    fn build(lang: Language) -> Self {
        UiFaces {
            link_badges: LinkBadge::FACES
                .iter()
                .map(|b| {
                    let (badge, ink) = (
                        b.badge().expect("a face has a glyph"),
                        b.colour().expect("and a colour"),
                    );
                    badge_face(badge, HUD_ICON_PX, ink)
                })
                .collect(),
            shutdown: PowerChoice::ALL
                .iter()
                .map(|c| {
                    menu_face(match c {
                        PowerChoice::Restart => slot_i18n::t(lang, "device.app.power.restarting"),
                        PowerChoice::PowerOff => slot_i18n::t(lang, "device.app.power.powering_off"),
                        PowerChoice::Repair => slot_i18n::t(lang, "device.app.power.repairing"),
                    })
                })
                .collect(),
            power_cards: [false, true].map(slot_ui::power_card_face),
            power_labels: PowerChoice::ALL
                .iter()
                .map(|c| [false, true].map(|lit| slot_ui::power_label_face(*c, lit, lang)))
                .collect(),
            power_icons: PowerChoice::ALL
                .iter()
                .map(|c| [false, true].map(|lit| slot_ui::power_icon_face(*c, lit, lang)))
                .collect(),
            power_title: slot_ui::power_title_face(lang),
            quick_cards: [false, true].map(|selected| {
                slot_ui::switch_style::card_face(QUICK_CARD_W, QUICK_CARD_H, selected)
            }),
            quick_labels: QuickRow::ALL
                .iter()
                .map(|r| [false, true].map(|lit| quick_label_face_state(*r, lit, lang)))
                .collect(),
            quick_values: QuickValue::ALL
                .iter()
                .copied()
                .chain(QuickValue::palette_values())
                .chain(QuickValue::control_values())
                .chain(QuickValue::shortcut_values())
                .chain(QuickValue::menu_shortcut_values())
                .chain(QuickValue::plain_shortcut_values())
                .map(|v| [false, true].map(|lit| quick_value_face(v.text(lang), lit)))
                .collect(),
            quick_carets: [false, true]
                .map(|right| [false, true].map(|lit| quick_caret_face_state(right, lit))),
            quick_icons: QuickRow::ALL
                .iter()
                .map(|r| [false, true].map(|lit| quick_icon_face(*r, lit, lang)))
                .collect(),
            quick_titles: std::array::from_fn(|index| quick_title_face(index, lang)),
            quick_legend: quick_legend_faces(lang),
            sockets: slot_store::Core::EVERY
                .iter()
                .map(|c| socket_face(*c))
                .collect(),
            chips: slot_store::Platform::ALL
                .iter()
                .map(|p| slot_store::Core::EVERY.iter().map(|c| chip_face_for(*p, Some(*c))).collect())
                .collect(),
            blank_chips: slot_store::Platform::ALL
                .iter()
                .map(|p| chip_face_for(*p, None))
                .collect(),
            chip_shadow: chip_shadow_face(),
            core_legend: vec![
                hint_face("B", slot_i18n::t(lang, "device.app.core.cancel")),
                arrows_hint_face(slot_i18n::t(lang, "device.app.core.switch")),
                hint_face("A", slot_i18n::t(lang, "device.app.core.confirm")),
                hint_face("Y", slot_i18n::t(lang, "device.app.core.switch_net_chip")),
                hint_face("X", slot_i18n::t(lang, "device.app.core.front_cover")),
                hint_face(slot_i18n::t(lang, "device.app.core.up_down_keys"), slot_i18n::t(lang, "device.app.core.up_down_switch")),
            ],
            link_roles: LinkRow::ALL.iter().map(|r| menu_face(r.text(lang))).collect(),
            linked: menu_face(slot_i18n::t(lang, "device.app.link.connected")),
            link_confirm: slot_ui::link_dialog::end_link(lang),
            link_legend: vec![
                hint_face("B", slot_i18n::t(lang, "device.app.core.cancel")),
                hint_face("SELECT", slot_i18n::t(lang, "device.app.link.mode")),
                arrows_hint_face(slot_i18n::t(lang, "device.app.core.switch")),
                hint_face("A", slot_i18n::t(lang, "device.app.link.action")),
                hint_face("A", slot_i18n::t(lang, "device.app.common.back")),
                hint_face("B", slot_i18n::t(lang, "device.app.common.back")),
                hint_face("A", slot_i18n::t(lang, "device.app.link.disconnect")),
            ],
            link_steps: LinkStep::ALL.iter().map(|s| menu_face(s.line())).collect(),
            link_fails: LinkFail::SHOWN
                .iter()
                .map(|f| menu_face(f.line()))
                .collect(),
            toasts: Toast::all().into_iter().map(|toast| toast_face(toast, lang)).collect(),
        }
    }
}

struct Faces<'a> {
    pool: &'a mut Vec<TexId>,
    title: &'a mut Option<TexId>,
    undo: &'a mut Option<TexId>,
}

fn sync_switcher(app: &mut App, compositor: &mut Compositor, texes: Faces, state: &mut Switcher) {
    if !matches!(app.phase(), Phase::Polaroids { .. }) {
        state.open = false;
        return;
    }

    if !state.open {
        state.open = true;
        state.titled = None;
        let entries = app.polaroid_entries().to_vec();
        let faces: Vec<_> = entries.iter().map(photo_face).collect();
        let ids = faces
            .iter()
            .enumerate()
            .map(|(i, f)| match texes.pool.get(i) {
                Some(id) => {
                    compositor.update_texture(*id, f.w, f.h, &f.rgba);
                    *id
                }
                None => {
                    let id = compositor.create_texture_nearest(f.w, f.h, &f.rgba);
                    texes.pool.push(id);
                    id
                }
            })
            .collect();
        app.set_polaroid_faces(ids);
        if entries.is_empty() && state.empty.is_none() {
            let face = word_face(slot_i18n::t(
                app.language(),
                "device.app.saves.no_states",
            ));
            let (w, h) = (face.w, face.h);
            let id = upload(compositor, &mut state.empty, face);
            app.set_polaroid_empty_face(id, w, h);
        }

        let label = app
            .undo_label()
            .map(|l| upload(compositor, texes.undo, hint_face("X", l)));
        app.set_undo_face(label);
    }
    if state.titled.as_deref() != app.polaroid_stamp() {
        state.titled = app.polaroid_stamp().map(str::to_string);
        let face = title_face(&app.polaroid_title(&format_stamp(app.wall_secs())));
        let id = upload(compositor, texes.title, face);
        app.set_polaroid_title_face(id);
    }
}

fn sync_clock(app: &mut App, compositor: &mut Compositor, clocks: &mut Clocks) {
    let picked = app.picker().map(|p| p.text());
    if picked != clocks.picked {
        clocks.picked = picked;
        if let (Some(face), Some(hint)) = (app.picker().map(|p| p.face()), clocks.hint) {
            let line = upload(compositor, &mut clocks.line, face);
            app.set_clock_faces(line, hint);
        }
    }
    let shown = hhmm(app.wall_secs());
    if shown != clocks.shown {
        let face = word_face(&shown);
        clocks.shown = shown;
        let w = face.w;
        let id = upload(compositor, &mut clocks.shelf, face);
        app.set_shelf_clock_face(id, w);
    }
    let platform_shown = app.slot_text().unwrap_or_default();
    let platform_key = (platform_shown, app.shelf_collection_ink());
    if platform_key != clocks.platform {
        clocks.platform = platform_key.clone();
        if platform_key.0.is_empty() {
            app.clear_shelf_platform();
        } else {
            let face = slot_ui::slot_name_face(&platform_key.0, platform_key.1);
            let (w, h) = (face.w, face.h);
            let id = upload(compositor, &mut clocks.platform_tex, face);
            app.set_shelf_platform_face_sized(id, w, h);
        }
    }
    let battery_shown = app
        .battery()
        .map(|b| format!("{}%", b.percent))
        .unwrap_or_default();
    if battery_shown != clocks.battery {
        clocks.battery = battery_shown.clone();
        if !battery_shown.is_empty() {
            let face = word_face(&battery_shown);
            let w = face.w;
            let id = upload(compositor, &mut clocks.battery_tex, face);
            app.set_battery_percent_face(id, w);
        }
    }

    let playtime_shown = app.shelf_playtime_text().to_string();
    if playtime_shown != clocks.playtime {
        clocks.playtime = playtime_shown.clone();
        if playtime_shown.is_empty() {
            clocks.playtime_tex = None;
            app.clear_shelf_playtime_face();
        } else {
            let face = slot_ui::sync_overlay::line_face(&playtime_shown, 360.0, 20.0);
            if face.w == 0 || face.h == 0 {
                clocks.playtime_tex = None;
                app.clear_shelf_playtime_face();
            } else {
                let (w, h) = (face.w, face.h);
                let id = upload(compositor, &mut clocks.playtime_tex, face);
                app.set_shelf_playtime_face(id, w, h);
            }
        }
    }
}

fn sync_quick_clock(app: &mut App, compositor: &mut Compositor, state: &mut QuickClock) {
    if app.quick_menu().is_none() {
        return;
    }
    let text = date_time_text(app.wall_secs(), app.language());
    if text == state.shown {
        return;
    }
    let (dim, lit) = (
        quick_value_face(&text, false),
        quick_value_face(&text, true),
    );
    let (dim_size, lit_size) = ((dim.w, dim.h), (lit.w, lit.h));
    let dim = upload(compositor, &mut state.dim, dim);
    let lit = upload(compositor, &mut state.lit, lit);
    app.set_quick_clock_faces((dim, dim_size.0, dim_size.1), (lit, lit_size.0, lit_size.1));
    state.shown = text;
}

fn sync_about(app: &mut App, compositor: &mut Compositor, state: &mut AboutFace) {
    if !matches!(app.phase(), Phase::About) {
        return;
    }
    let build = Build::current();
    let serial = build.serial();
    let page = app.about_page();
    let lang = app.language();
    let device = slot_device::DeviceInfo::current().model().to_string();
    let key = (
        serial.clone(),
        build.date.to_string(),
        build.dirty,
        page,
        device.clone(),
    );
    if state.tex.is_some() && state.key.as_ref() == Some(&key) {
        return;
    }
    state.key = Some(key);
    let fields = AboutFields {
        serial: &serial,
        date: build.date,
        dirty: build.dirty,
        device: &device,
    };
    let face = match page {
        AboutPage::Main => about_face(&fields, lang),
        AboutPage::Thanks => about_thanks_face(&fields, lang),
    };
    let id = upload(compositor, &mut state.tex, face);
    app.set_about_face(id);
}

fn sync_core_picker(
    app: &mut App,
    compositor: &mut Compositor,
    builder: &FaceBuilder,
    asked: &mut Option<String>,
    board: &mut Option<TexId>,
    lid: &mut Option<TexId>,
    built: &mut Option<String>,
) {
    let current = app
        .selected_stem()
        .and_then(|stem| app.carts().find(|cart| cart.stem == stem).cloned());
    let Some(current) = current else {
        *asked = None;
        return;
    };
    let key = core_face_key(&current.stem, current.front.as_deref());
    let ready = app.core_faces_ready();
    if *asked != Some(key.clone()) || !ready {
        builder.request(current.clone());
        *asked = Some(key.clone());
        if !ready {
            *built = None;
        }
    }
    let Some(faces) = builder.take() else {
        return;
    };
    if faces.stem != current.stem || faces.front != current.front {
        // A stale build for the previous front finished after the selection
        // changed.  Ask for the current one again rather than painting it.
        builder.request(current.clone());
        *asked = Some(key);
        *built = None;
        return;
    }
    if *built == Some(key.clone()) {
        return;
    }
    let board_id = upload_rgba(
        compositor,
        board,
        faces.board.w,
        faces.board.h,
        &faces.board.rgba,
    );
    let lid_id = upload_rgba(compositor, lid, faces.lid.w, faces.lid.h, &faces.lid.rgba);
    app.set_core_board_faces(board_id, lid_id);
    *built = Some(key);
}

/// Core/cart faces are keyed by the selected front as well as the ROM stem:
/// the async builder may return the previous front after a selection change.
fn core_face_key(stem: &str, front: Option<&std::path::Path>) -> String {
    format!(
        "{}|{}",
        stem,
        front
            .map(|front| front.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

fn sync_network_chip(
    app: &mut App,
    compositor: &mut Compositor,
    tex: &mut Option<TexId>,
    shown: &mut Option<String>,
) {
    let label = app.network_label();
    let cover = app.sync_cover_path();
    let key = format!(
        "{}|{}",
        label,
        cover
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    if shown.as_deref() == Some(key.as_str()) {
        return;
    }
    let face = slot_ui::network_chip_face_scaled_cover(
        Some(&label),
        1,
        true,
        cover.as_deref(),
    );
    let id = compositor.create_texture(face.w, face.h, &face.rgba);
    app.set_network_face(id);
    let off = slot_ui::network_chip_face_scaled_cover(
        Some(&label),
        1,
        false,
        cover.as_deref(),
    );
    app.set_network_face_off(compositor.create_texture(off.w, off.h, &off.rgba));
    *tex = Some(id);
    *shown = Some(key);
}

fn sync_sync_picker_face(
    app: &mut App,
    compositor: &mut Compositor,
    tex: &mut Option<TexId>,
    tex_alt: &mut Option<TexId>,
    tex_lit: &mut Option<TexId>,
    alt: &mut bool,
    shown: &mut Option<String>,
) {
    if !app.sync_picker_open() && !app.sync_highlight_open() {
        *shown = None;
        app.clear_sync_picker_swap();
        return;
    }
    let label = app.network_label();
    let cover = app.sync_cover_path();
    let key = format!(
        "{}|{}",
        label,
        cover
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    if shown.as_deref() == Some(key.as_str()) {
        return;
    }
    let face = slot_ui::network_chip_face_scaled_marked(
        Some(&label),
        6,
        true,
        false,
        cover.as_deref(),
    );
    // Two alternating slots keep the outgoing face alive long enough for the
    // swap animation, and never allocate a third texture.
    let previous = if *alt { *tex } else { *tex_alt };
    let slot = if *alt { tex_alt } else { tex };
    let id = upload_rgba(compositor, slot, face.w, face.h, &face.rgba);
    // The same chip with its pin-1 mark lit; the highlight swaps to it once the
    // chip is fully open.
    let lit_face = slot_ui::network_chip_face_scaled_marked(
        Some(&label),
        6,
        true,
        true,
        cover.as_deref(),
    );
    let lit = upload_rgba(compositor, tex_lit, lit_face.w, lit_face.h, &lit_face.rgba);
    *alt = !*alt;
    app.set_sync_picker_faces(id, previous, Some(lit));
    *shown = Some(key);
}

fn sync_face(compositor: &mut Compositor, face: slot_ui::CartFace) -> SyncFace {
    let tex = compositor.create_texture(face.w, face.h, &face.rgba);
    SyncFace {
        tex,
        w: face.w,
        h: face.h,
    }
}

/// Build one of the animation's fixed labels once and reuse its texture.
fn static_sync_face(
    compositor: &mut Compositor,
    slot: &mut Option<SyncFace>,
    text: &str,
    max_w: f32,
    px: f32,
) -> Option<SyncFace> {
    if slot.is_none() {
        let mut tex = None;
        *slot = upload_line(compositor, &mut tex, text, max_w, px);
    }
    *slot
}

/// Rasterise the app's overlay strings when they change.  The app owns the
/// state machine; this only turns its text into textures.
fn sync_shelf_sync(app: &mut App, compositor: &mut Compositor, cache: &mut SyncTextCache) {
    let lang = app.language();
    if app.auto_cloud_card().is_none() {
        let face = slot_ui::sync_cart_face_with_sticker(
            slot_ui::SYNC_SHELL,
            slot_i18n::t(lang, "device.app.sync.auto_card"),
            Some(slot_ui::CartSticker::Text),
        );
        let tex = compositor.create_texture(face.w, face.h, &face.rgba);
        app.set_auto_cloud_card((tex, face.w, face.h));
    }
    let data_label = static_sync_face(
        compositor,
        &mut cache.data_label,
        slot_i18n::t(lang, "device.app.sync.data"),
        110.0,
        12.0,
    );
    let net_label = static_sync_face(compositor, &mut cache.net_label, "NET", 40.0, 9.0);
    if cache.card_label.is_none() {
        let face = slot_ui::net_card_label_face(slot_i18n::t(lang, "device.app.sync.cloud_label"));
        let tex = compositor.create_texture(face.w, face.h, &face.rgba);
        cache.card_label = Some(SyncFace { tex, w: face.w, h: face.h });
    }
    let card_label = cache.card_label;
    let net_slot_title = static_sync_face(compositor, &mut cache.net_slot_title, "NET SLOT", 90.0, 10.0);
    let net_slot_hint = static_sync_face(
        compositor,
        &mut cache.net_slot_hint,
        slot_i18n::t(lang, "device.app.sync.net_slot"),
        90.0,
        10.0,
    );
    let state_label = static_sync_face(compositor, &mut cache.state_label, "STATE", 100.0, 14.0);
    let sav_label = static_sync_face(compositor, &mut cache.sav_label, "SAV", 100.0, 14.0);
    let rail_labels = [
        static_sync_face(compositor, &mut cache.rail_labels[0], slot_i18n::t(lang, "device.app.sync.rail.network"), 60.0, 10.0),
        static_sync_face(compositor, &mut cache.rail_labels[1], slot_i18n::t(lang, "device.app.sync.rail.cloud"), 60.0, 10.0),
        static_sync_face(compositor, &mut cache.rail_labels[2], slot_i18n::t(lang, "device.app.sync.rail.transfer"), 60.0, 10.0),
        static_sync_face(compositor, &mut cache.rail_labels[3], slot_i18n::t(lang, "device.app.sync.rail.verify"), 60.0, 10.0),
        static_sync_face(compositor, &mut cache.rail_labels[4], slot_i18n::t(lang, "device.app.sync.rail.done"), 60.0, 10.0),
    ];
    let check = static_sync_face(compositor, &mut cache.check, "✓", 48.0, 28.0);
    if cache.cancel_hint.is_none() {
        let face = hint_face("B", slot_i18n::t(lang, "device.app.common.cancel"));
        let tex = compositor.create_texture(face.w, face.h, &face.rgba);
        cache.cancel_hint = Some(SyncFace { tex, w: face.w, h: face.h });
    }
    let cancel_hint = cache.cancel_hint;
    let texts = app.sync_texts();
    if cache.texts.as_ref() == Some(&texts) {
        return;
    }
    let title = upload_line(compositor, &mut cache.title, &texts.title, 360.0, 22.0);
    let progress_left = upload_line(
        compositor,
        &mut cache.progress_left,
        &texts.progress_left,
        320.0,
        13.0,
    );
    let progress_right = upload_line(
        compositor,
        &mut cache.progress_right,
        &texts.progress_right,
        180.0,
        13.0,
    );
    let banner = upload_line(
        compositor,
        &mut cache.banner,
        &texts.banner,
        SYNC_BANNER_MAX_W,
        SYNC_BANNER_PX,
    );
    let cloud_line = upload_line(
        compositor,
        &mut cache.cloud,
        &texts.cloud_line,
        SYNC_LINE_MAX_W,
        SYNC_LINE_PX,
    );
    let local_line = upload_line(
        compositor,
        &mut cache.local,
        &texts.local_line,
        SYNC_LINE_MAX_W,
        SYNC_LINE_PX,
    );
    let upload_label = upload_line(
        compositor,
        &mut cache.upload,
        &texts.upload_label,
        SYNC_LABEL_MAX_W,
        SYNC_LABEL_PX,
    );
    let download_label = upload_line(
        compositor,
        &mut cache.download,
        &texts.download_label,
        SYNC_LABEL_MAX_W,
        SYNC_LABEL_PX,
    );
    app.set_sync_text_faces(SyncFaces {
        banner,
        cloud_line,
        local_line,
        upload_label,
        download_label,
        title,
        data_label,
        net_label,
        card_label,
        net_slot_title,
        net_slot_hint,
        state_label,
        sav_label,
        rail_labels,
        progress_left,
        progress_right,
        cancel_hint,
        check,
    });
    cache.texts = Some(texts);
}

fn upload_line(
    compositor: &mut Compositor,
    slot: &mut Option<TexId>,
    text: &str,
    max_w: f32,
    px: f32,
) -> Option<SyncFace> {
    if text.is_empty() {
        *slot = None;
        return None;
    }
    let face = slot_ui::sync_overlay::line_face(text, max_w, px);
    if face.w == 0 || face.h == 0 {
        return None;
    }
    let (w, h) = (face.w, face.h);
    let tex = upload(compositor, slot, face);
    Some(SyncFace { tex, w, h })
}

fn upload(compositor: &mut Compositor, slot: &mut Option<TexId>, face: slot_ui::UndoFace) -> TexId {
    upload_rgba(compositor, slot, face.w, face.h, &face.rgba)
}

fn upload_rgba(
    compositor: &mut Compositor,
    slot: &mut Option<TexId>,
    w: u32,
    h: u32,
    rgba: &[u8],
) -> TexId {
    match *slot {
        Some(id) => {
            compositor.update_texture(id, w, h, rgba);
            id
        }
        None => {
            let id = compositor.create_texture(w, h, rgba);
            *slot = Some(id);
            id
        }
    }
}
fn conflict_view(ui: &crate::app::ConflictUi) -> slot_ui::conflict::ConflictView {
    use slot_ui::conflict::{
        ConflictDirection, ConflictPage, ConflictUnitView, ConflictView,
    };
    let page = |page: crate::app::ConflictPage| match page {
        crate::app::ConflictPage::All => ConflictPage::All,
        crate::app::ConflictPage::State => ConflictPage::State,
        crate::app::ConflictPage::Sav => ConflictPage::Sav,
    };
    let unit = |unit: &crate::app::ConflictUnitUi| ConflictUnitView {
        cloud_line: unit.cloud_line.clone(),
        local_line: unit.local_line.clone(),
        reason: unit.reason.clone(),
        locked: unit.locked,
    };
    ConflictView {
        pages: ui.pages.iter().copied().map(page).collect(),
        page: page(ui.page),
        direction: match ui.direction {
            crate::app::SyncChoice::Download => ConflictDirection::Download,
            crate::app::SyncChoice::Upload => ConflictDirection::Upload,
        },
        state: ui.state.as_ref().map(unit),
        sav: ui.sav.as_ref().map(unit),
        locked_state: ui.locked_state,
        locked_sav: ui.locked_sav,
        error: ui.error.clone(),
        notice: ui.notice.clone(),
        syncing: ui.syncing,
    }
}


#[cfg(test)]
mod faces_lang_tests {
    use super::*;

    #[test]
    fn the_first_build_is_requested_and_lands() {
        let mut state = FacesLang::default();
        assert_eq!(state.baked(), None);
        let generation = state.want(Language::EnUs).expect("the first build");
        assert!(state.is_building());
        assert!(state.finish(generation));
        assert_eq!(state.baked(), Some(Language::EnUs));
        assert!(!state.is_building());
    }

    #[test]
    fn a_matching_language_never_rebuilds() {
        let mut state = FacesLang::default();
        state.baked_now(Language::ZhCn);
        assert_eq!(state.want(Language::ZhCn), None);
        assert_eq!(state.generation(), 0, "no build means no new generation");
    }

    #[test]
    fn a_second_switch_while_building_waits_and_then_rebuilds_once() {
        let mut state = FacesLang::default();
        let first = state.want(Language::EnUs).expect("the first build");
        // Twenty more presses during the build must not queue twenty builds.
        for _ in 0..20 {
            assert_eq!(state.want(Language::ZhCn), None);
            assert_eq!(state.want(Language::EnUs), None);
        }
        assert!(state.finish(first));
        assert_eq!(state.baked(), Some(Language::EnUs));
        // Now that the language has moved on, exactly one more build follows.
        let second = state.want(Language::ZhCn).expect("the late build");
        assert_ne!(second, first);
        assert!(state.finish(second));
        assert_eq!(state.baked(), Some(Language::ZhCn));
        assert_eq!(state.generation(), 2);
    }

    #[test]
    fn a_stale_result_is_dropped_instead_of_painted() {
        let mut state = FacesLang::default();
        let first = state.want(Language::EnUs).expect("the first build");
        // Pretend the worker answered after we gave up on it.
        assert!(state.finish(first));
        // A duplicate answer for the same generation finds nothing in flight.
        assert!(!state.finish(first));
    }

    #[test]
    fn abandoning_a_dead_worker_allows_a_retry() {
        let mut state = FacesLang::default();
        let _ = state.want(Language::EnUs).expect("the first build");
        state.abandon();
        assert!(!state.is_building());
        assert!(state.want(Language::EnUs).is_some(), "a retry must be possible");
    }

    #[test]
    fn both_languages_build_the_whole_menu_batch() {
        // `build` only rasterises; it never touches GL, so it is safe here.
        let zh = UiFaces::build(Language::ZhCn);
        let en = UiFaces::build(Language::EnUs);

        for faces in [&zh, &en] {
            assert_eq!(faces.quick_labels.len(), QuickRow::ALL.len());
            assert_eq!(faces.quick_icons.len(), QuickRow::ALL.len());
            assert_eq!(faces.quick_titles.len(), QuickRow::QUICK_PAGES);
            assert!(
                faces.core_legend.len() >= 5,
                "the PCB legend lost the front-cover hint"
            );
            assert!(
                faces.quick_values.len() >= QuickValue::ALL.len(),
                "the value table lost entries"
            );
            assert!(!faces.toasts.is_empty());
            // Every row must have ink in both languages: a missing font or an
            // empty label would show up as a zero-width face.
            for row in QuickRow::ALL {
                assert!(
                    faces.quick_labels[row.index()][0].w > 0,
                    "{row:?} has no label texture"
                );
                assert!(
                    faces.quick_icons[row.index()][0].w > 0,
                    "{row:?} has no icon texture"
                );
            }
        }

        // The language reaches the rasteriser, not just the value table: the
        // same row is drawn with different pixels in the two languages.
        let at = QuickRow::Language.index();
        assert_ne!(
            zh.quick_labels[at][0].rgba, en.quick_labels[at][0].rgba,
            "语言/Language produced the same texture"
        );
        assert_ne!(
            zh.core_legend[4].rgba, en.core_legend[4].rgba,
            "the X front-cover hint did not follow the language"
        );
    }

    #[test]
    fn the_menu_titles_exist_in_both_languages() {
        // `UiFaces::build` needs a GL-free path to be tested; this is the part
        // that would panic if a language had no table at all.
        for lang in Language::ALL {
            for row in QuickRow::ALL {
                assert!(!row.label(lang).is_empty(), "{row:?} has no label");
                assert!(!row.icon_text(lang).is_empty(), "{row:?} has no icon text");
            }
            for value in QuickValue::ALL {
                assert!(!value.text(lang).is_empty(), "{value:?} has no text");
            }
            for title in QuickRow::pages(lang) {
                assert!(!title.is_empty(), "a page has no title");
            }
        }
    }

    #[test]
    fn shelf_legend_follows_language() {
        for key in [
            "device.app.shelf.hold_sync",
            "device.app.shelf.search",
            "device.app.shelf.hold_intro",
        ] {
            let zh = slot_i18n::t(Language::ZhCn, key);
            let en = slot_i18n::t(Language::EnUs, key);
            assert_ne!(zh, en, "{key} did not switch language");
            assert!(!zh.is_empty() && !en.is_empty(), "{key} is empty");
            assert!(
                !en.chars().any(|c| {
                    ('\u{3000}'..='\u{303f}').contains(&c)
                        || ('\u{4e00}'..='\u{9fff}').contains(&c)
                }),
                "{key} is still Chinese in English: {en}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_face_keys_include_the_front_path() {
        let stem = "Emerald";
        let custom = std::path::Path::new("linkSlot/Fronts/GBA/Emerald/custom.png");
        assert_ne!(
            core_face_key(stem, None),
            core_face_key(stem, Some(custom)),
            "the default shell and a custom front shared a core-face cache key"
        );
        assert_ne!(
            core_face_key(stem, Some(custom)),
            core_face_key(stem, Some(std::path::Path::new("linkSlot/Fronts/GBA/Emerald/other.png"))),
            "two custom fronts shared a core-face cache key"
        );
    }
    use crate::app::{ConflictPage, ConflictUnitUi, ConflictUi, SyncChoice};

    fn unit(locked: bool) -> ConflictUnitUi {
        ConflictUnitUi {
            cloud_line: "云端 v7 · RG34XX-A".to_string(),
            local_line: "本地 v8 · 本机".to_string(),
            reason: "云端有更新".to_string(),
            locked,
        }
    }

    fn ui() -> ConflictUi {
        ConflictUi {
            pages: vec![
                ConflictPage::All,
                ConflictPage::State,
                ConflictPage::Sav,
            ],
            page: ConflictPage::All,
            direction: SyncChoice::Download,
            state: Some(unit(false)),
            sav: Some(unit(false)),
            locked_state: false,
            locked_sav: false,
            error: None,
            notice: None,
            syncing: false,
        }
    }

    #[test]
    fn the_conflict_view_maps_every_page_and_direction() {
        let mapped = conflict_view(&ui());
        assert_eq!(mapped.pages.len(), 3);
        assert_eq!(mapped.page, slot_ui::conflict::ConflictPage::All);
        assert_eq!(
            mapped.direction,
            slot_ui::conflict::ConflictDirection::Download
        );
        let mut upload = ui();
        upload.direction = SyncChoice::Upload;
        assert_eq!(
            conflict_view(&upload).direction,
            slot_ui::conflict::ConflictDirection::Upload
        );
        let mut sav_page = ui();
        sav_page.page = ConflictPage::Sav;
        assert_eq!(
            conflict_view(&sav_page).page,
            slot_ui::conflict::ConflictPage::Sav
        );
    }

    #[test]
    fn the_conflict_cache_key_changes_with_page_locks_and_errors() {
        let base = conflict_view(&ui());
        let mut page = ui();
        page.page = ConflictPage::State;
        assert_ne!(base, conflict_view(&page), "the page is not in the key");
        let mut locked = ui();
        locked.locked_state = true;
        assert_ne!(base, conflict_view(&locked), "the lock is not in the key");
        let mut errored = ui();
        errored.error = Some("网络未连接".to_string());
        assert_ne!(base, conflict_view(&errored), "the error is not in the key");
        let mut syncing = ui();
        syncing.syncing = true;
        assert_ne!(
            base,
            conflict_view(&syncing),
            "the syncing state is not in the key"
        );
        let mut version = ui();
        version.state = Some(ConflictUnitUi {
            cloud_line: "云端 v9 · RG34XX-A".to_string(),
            ..unit(false)
        });
        assert_ne!(
            base,
            conflict_view(&version),
            "the version lines are not in the key"
        );
        assert_eq!(base, conflict_view(&ui()), "the key is not stable");
    }
}

#[cfg(test)]
mod intro_back_tests {
    use super::*;

    /// The redesign: the label is a small printed sticker on the shell, not a
    /// page covering the back of the cartridge.
    #[test]
    fn the_back_label_stays_small_and_papered() {
        slot_ui::warm_fonts();
        for platform in [
            slot_store::Platform::Gba,
            slot_store::Platform::Gb,
            slot_store::Platform::Gbc,
            slot_store::Platform::Fc,
        ] {
            // A red shell proves the back follows the cartridge's own colour.
            let face = intro_back_face(platform, [0xc2, 0x33, 0x2e], "CONTRA", Language::ZhCn);
            let (lx, ly, lw, lh) = crate::app::intro_back_label_box(platform);
            assert!(lx + lw <= face.w as f32 && ly + lh <= face.h as f32);
            let label = lw * lh / (face.w * face.h) as f32;
            assert!(
                (0.05..0.45).contains(&label),
                "{platform:?} label covers {label:.2} of the back"
            );
            let paper_at = |x: f32, y: f32| {
                let at = ((y as u32 * face.w + x as u32) * 4) as usize;
                face.rgba[at] > 200 && face.rgba[at + 1] > 190 && face.rgba[at + 2] > 170
            };
            assert!(
                paper_at(lx + lw / 2.0, ly + lh / 2.0),
                "{platform:?} has no paper label"
            );
            assert!(
                !paper_at(20.0, face.h as f32 / 2.0),
                "{platform:?} still paints the whole back as paper"
            );
            let shell_at = |face: &UndoFace| {
                let at = (((face.h / 2) * face.w + 20) * 4) as usize;
                face.rgba[at..at + 3].to_vec()
            };
            let other = intro_back_face(platform, [0x2f, 0x5c, 0xc0], "CONTRA", Language::ZhCn);
            assert_ne!(
                shell_at(&face),
                shell_at(&other),
                "{platform:?} ignores the cartridge colour on its back"
            );
            if std::env::var_os("SLOT_DUMP_INTRO_BACKS").is_some() {
                let path = std::env::temp_dir().join(format!("intro-back-{platform:?}.png"));
                let file = std::fs::File::create(&path).unwrap();
                let mut encoder = png::Encoder::new(
                    std::io::BufWriter::new(file),
                    face.w,
                    face.h,
                );
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(&face.rgba).unwrap();
                eprintln!("wrote {}", path.display());
            }
        }
    }
}
