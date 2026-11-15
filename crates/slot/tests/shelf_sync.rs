//! The v0.6 shelf sync overlay: long-press Y on the cartridge browser starts
//! `linkSlot sync`, the transfer animation follows the CLI, a conflict opens
//! the two-way choice, and every other key keeps its old shelf meaning.
//!
//! Everything runs on the host with a fake `SyncLauncher`, so no `/bin/sh` or
//! card is needed (project book §6.6/§6.8).

mod common;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use common::tmp_root_with_carts;
use slot::app::{
    App, ConflictPage, ConflictUi, KeyboardPurpose, Phase, ShelfSync, SyncChoice,
    SyncFace, SyncFaces,
};
use slot::sync_job::{
    ChangedSince, ConflictInfo, Direction, ResolveSpec, SyncLauncher, SyncOutcome, SyncRequest,
    SyncRequestMode, SyncResult, SyncRun, SyncStage, SyncUpdate, UnitKind, UnitOutcome,
    VersionInfo, SYNC_HOLD_MS, SYNC_RESULT_MS, SYNC_TIMEOUT_MS,
};
use slot_gfx::TexId;
use slot_input::{Action, Btn};
use slot_ui::Draw;

// ---- a fake launcher that records requests and lets a test feed progress ----

#[derive(Default)]
struct RunState {
    events: VecDeque<SyncUpdate>,
    cancelled: bool,
    exited: bool,
}

struct FakeRun {
    state: Arc<Mutex<RunState>>,
    cancels: Arc<Mutex<usize>>,
}

impl SyncRun for FakeRun {
    fn poll(&mut self) -> Option<SyncUpdate> {
        let mut state = self.state.lock().expect("fake run");
        match state.events.pop_front() {
            Some(SyncUpdate::Finished(outcome)) => {
                state.exited = true;
                Some(SyncUpdate::Finished(outcome))
            }
            update => update,
        }
    }

    fn cancel(&mut self) {
        let mut state = self.state.lock().expect("fake run");
        state.cancelled = true;
        state.exited = true;
        *self.cancels.lock().expect("cancels") += 1;
    }

    fn exited(&self) -> bool {
        self.state.lock().expect("fake run").exited
    }
}

/// Shared with the test so it can inspect the request and feed updates or a
/// final outcome into the job the app is polling.
#[derive(Clone, Default)]
struct Fake {
    requests: Arc<Mutex<Vec<SyncRequest>>>,
    run: Arc<Mutex<Option<Arc<Mutex<RunState>>>>>,
    cancels: Arc<Mutex<usize>>,
}

impl Fake {
    fn new() -> Self {
        Fake::default()
    }

    fn push(&self, update: SyncUpdate) {
        if let Some(run) = self.run.lock().expect("run slot").as_ref() {
            run.lock().expect("fake run").events.push_back(update);
        }
    }

    fn stage(&self, stage: SyncStage) {
        self.push(SyncUpdate::Stage(stage));
    }

    fn direction(&self, direction: Direction) {
        self.push(SyncUpdate::Direction(direction));
    }

    fn finish(&self, outcome: SyncOutcome) {
        self.push(SyncUpdate::Finished(outcome));
    }

    /// The process disappeared without a final `LINKSLOT_*` block.
    fn exit_without_outcome(&self) {
        if let Some(run) = self.run.lock().expect("run slot").as_ref() {
            run.lock().expect("fake run").exited = true;
        }
    }

    fn launches(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    fn last(&self) -> SyncRequest {
        self.requests
            .lock()
            .expect("requests")
            .last()
            .expect("no sync was launched")
            .clone()
    }

    fn cancels(&self) -> usize {
        *self.cancels.lock().expect("cancels")
    }

    fn cancelled(&self) -> bool {
        self.run
            .lock()
            .expect("run slot")
            .as_ref()
            .is_some_and(|run| run.lock().expect("fake run").cancelled)
    }
}

impl SyncLauncher for Fake {
    fn launch(&mut self, request: SyncRequest) -> Box<dyn SyncRun> {
        self.requests.lock().expect("requests").push(request);
        let state = Arc::new(Mutex::new(RunState::default()));
        *self.run.lock().expect("run slot") = Some(state.clone());
        Box::new(FakeRun {
            state,
            cancels: self.cancels.clone(),
        })
    }
}

// ---- rig and outcome helpers -----------------------------------------------

fn rig(root: &Path) -> (App, Fake) {
    let mut app = common::boot(root);
    let fake = Fake::new();
    app.set_sync_launcher(Box::new(fake.clone()));
    (app, fake)
}

/// Press Y and let the long-press timer fire.  The app is then in the PCB
/// opening beat, before the job is launched.
fn hold_sync(app: &mut App) {
    app.apply(Action::GbaDown(Btn::Y));
    app.update(SYNC_HOLD_MS as f32 / 1000.0);
}

/// Long-press Y and sit through the opening beat, the card swap and the
/// insert, so the sync job is actually running.
/// Long-press and run the opening beat, but accept a refusal (a read-only card,
/// a link session) instead of requiring the job to start.
fn attempt_sync(app: &mut App) {
    hold_sync(app);
    for _ in 0..300 {
        app.update(1.0 / 60.0);
        if matches!(
            app.shelf_sync(),
            ShelfSync::Running { .. } | ShelfSync::Result { .. }
        ) {
            return;
        }
    }
}

fn start_sync(app: &mut App) {
    hold_sync(app);
    for _ in 0..300 {
        app.update(1.0 / 60.0);
        if matches!(app.shelf_sync(), ShelfSync::Running { .. }) {
            return;
        }
    }
    panic!(
        "the sync job never started (active={})",
        app.shelf_sync_active()
    );
}

/// Select the network card `steps` past cloud on the PCB chip picker, then go
/// back to the shelf. Manual mods are attached this way, not by a dialog.
fn pick_network_card(app: &mut App, steps: isize) {
    app.apply(Action::GbaDown(Btn::Start));
    app.update(0.5);
    app.apply(Action::GbaDown(Btn::Y));
    for _ in 0..steps.abs() {
        let btn = if steps < 0 { Btn::Up } else { Btn::Down };
        app.apply(Action::GbaDown(btn));
        app.update(0.05);
    }
    app.apply(Action::GbaDown(Btn::A));
    app.apply(Action::GbaDown(Btn::B));
    app.update(0.5);
}

fn outcome(result: SyncResult) -> SyncOutcome {
    SyncOutcome {
        result,
        direction: None,
        version: None,
        error: None,
        detail: None,
        conflict: None,
        info: None,
        state: None,
        sav: None,
        conflict_units: Vec::new(),
        changed: None,
        choice: None,
    }
}

fn ok(direction: Direction, version: u32) -> SyncOutcome {
    SyncOutcome {
        direction: Some(direction),
        version: Some(version),
        ..outcome(SyncResult::Ok)
    }
}

fn conflict(info: ConflictInfo) -> SyncOutcome {
    SyncOutcome {
        error: Some("remote-newer".to_string()),
        info: Some(info),
        ..outcome(SyncResult::Conflict)
    }
}

fn failure(error: Option<&str>) -> SyncOutcome {
    SyncOutcome {
        error: error.map(str::to_string),
        detail: Some("linkSlot: 失败摘要".to_string()),
        ..outcome(SyncResult::Error)
    }
}

fn choice_outcome() -> SyncOutcome {
    SyncOutcome {
        result: SyncResult::Choice,
        detail: Some("需要选择存档方向".to_string()),
        choice: Some(slot_mods::ChoiceRequest {
            key: "conflict".to_string(),
            kind: slot_mods::ChoiceKind::Choice,
            title: "存档冲突".to_string(),
            body: "本地和 GBAStation 都修改了电池存档。".to_string(),
            options: vec![
                slot_mods::ChoiceOption {
                    id: "remote".to_string(),
                    label: "用 GBAStation 覆盖本地".to_string(),
                },
                slot_mods::ChoiceOption {
                    id: "local".to_string(),
                    label: "用本地覆盖 GBAStation".to_string(),
                },
            ],
            default: Some("remote".to_string()),
        }),
        ..outcome(SyncResult::Choice)
    }
}

fn running_phase(app: &App) -> &ShelfSync {
    match app.shelf_sync() {
        sync @ ShelfSync::Running { .. } => sync,
        other => panic!(
            "expected a running sync, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

fn banner(app: &App) -> String {
    app.sync_texts().banner
}

fn has_rect(out: &[Draw], w: f32, h: f32) -> bool {
    out.iter().any(|draw| match *draw {
        Draw::Rect { w: rw, h: rh, .. } => (rw - w).abs() < 0.01 && (rh - h).abs() < 0.01,
        _ => false,
    })
}

fn has_colour(out: &[Draw], colour: [f32; 4]) -> bool {
    out.iter().any(|draw| match *draw {
        Draw::Rect { colour: c, .. } => {
            (c[0] - colour[0]).abs() < 0.01
                && (c[1] - colour[1]).abs() < 0.01
                && (c[2] - colour[2]).abs() < 0.01
        }
        _ => false,
    })
}

fn drew(app: &App) -> Vec<Draw> {
    let mut out = Vec::new();
    app.draw(&mut out);
    out
}

// ---- long-press detection --------------------------------------------------

#[test]
fn holding_y_for_one_and_a_half_seconds_runs_link_slot_sync() {
    let d = tmp_root_with_carts(&["合金弹头 Advance", "Zelda"]);
    let (mut a, fake) = rig(d.path());
    assert!(!a.shelf_sync_active(), "the shelf starts with an overlay?");

    a.apply(Action::GbaDown(Btn::Y));
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Holding { .. }),
        "Y down did not arm the long-press timer"
    );

    a.update(1.0);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Holding { .. }),
        "a one-second press already fired"
    );
    assert_eq!(fake.launches(), 0);

    a.update(0.7);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Opening { .. }),
        "the hold was not accepted past the threshold"
    );
    assert_eq!(fake.launches(), 0, "the job ran before the opening beat");

    // The PCB opening, the card swap and the insert come first.
    for _ in 0..300 {
        a.update(1.0 / 60.0);
        if matches!(a.shelf_sync(), ShelfSync::Running { .. }) {
            break;
        }
    }
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Running { .. }),
        "the sync never started after the opening"
    );
    assert_eq!(fake.launches(), 1);

    let request = fake.last();
    assert_eq!(request.stem, "合金弹头 Advance");
    assert_eq!(request.platform, "GBA");
    assert_eq!(request.emulator, "mgba");
    assert_eq!(request.root, d.path());
    assert_eq!(request.resolve, ResolveSpec::default());
    let argv = request.argv();
    assert_eq!(argv.first().map(String::as_str), Some("sync"));
    assert!(argv
        .windows(2)
        .any(|pair| pair == ["--game", "合金弹头 Advance"]));
    assert!(!argv.contains(&"--resolve".to_string()));
}

#[test]
fn a_short_y_press_stays_idle_and_never_launches() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Y));
    a.update(0.3);
    a.apply(Action::GbaUp(Btn::Y));
    // The card slides back for CLOSE_MS before the overlay is gone.
    a.update(0.4);
    assert!(
        !a.shelf_sync_active(),
        "a short Y press left the overlay up"
    );
    a.update(2.0);
    assert_eq!(fake.launches(), 0, "a short Y press started a sync");
}

#[test]
fn a_press_one_hundredth_short_of_the_hold_does_not_start() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Y));
    a.update(1.49);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Holding { .. }),
        "1.49 s stopped the hold too early"
    );
    a.apply(Action::GbaUp(Btn::Y));
    a.update(0.4);
    assert!(!a.shelf_sync_active());
    assert_eq!(fake.launches(), 0, "1.49 s was treated as a full hold");
}

#[test]
fn a_release_after_the_hold_starts_the_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Y));
    // The release arrives after 1.5 s without an intervening update tick.
    a.apply_at(Action::GbaUp(Btn::Y), SYNC_HOLD_MS + 100);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Opening { .. }),
        "the release was dropped"
    );
    for _ in 0..300 {
        a.update(1.0 / 60.0);
        if matches!(a.shelf_sync(), ShelfSync::Running { .. }) {
            break;
        }
    }
    assert_eq!(fake.launches(), 1, "the release did not start the job");
    assert!(matches!(a.shelf_sync(), ShelfSync::Running { .. }));
}

#[test]
fn the_sync_mode_restricts_the_units_a_long_press_syncs() {
    use slot::linkcfg::{write_sync_mode, SyncMode};

    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_sync_mode(d.path(), SyncMode::State).expect("write sync mode");
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    assert_eq!(fake.launches(), 1, "the long press did not start a sync");
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::State));
    let argv = request.argv();
    assert!(argv.windows(2).any(|pair| pair == ["--only", "state"]));
    assert!(!argv.iter().any(|arg| arg == "sav"));

    let d2 = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_sync_mode(d2.path(), SyncMode::Sav).expect("write sync mode");
    let (mut b, fake) = rig(d2.path());
    start_sync(&mut b);
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::Sav));
    assert!(request
        .argv()
        .windows(2)
        .any(|pair| pair == ["--only", "sav"]));

    // `全部` keeps the pre-v0.0.10 argv: no `--only` at all.
    let d3 = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut c, fake) = rig(d3.path());
    start_sync(&mut c);
    assert_eq!(fake.last().only, None);
    assert!(!fake.last().argv().contains(&"--only".to_string()));
}

// ---- manual mods take over the long press ---------------------------------

fn install_manual_mod(root: &Path, mod_enabled: bool, action_enabled: bool) {
    let dir = root.join("linkSlot/mods").join("gbastation-battery-sync");
    std::fs::create_dir_all(dir.join("actions").join("sync")).expect("mod dirs");
    std::fs::write(
        dir.join("mod.json"),
        format!(
            r#"{{"schema":2,"name":"GBAStation 电池存档同步","version":"1.0.0","enabled":{mod_enabled}}}"#
        ),
    )
    .expect("mod.json");
    std::fs::write(
        dir.join("actions").join("sync").join("action.json"),
        format!(
            r#"{{"schema":1,"name":"GBAStation 电池存档同步","timing":"manual","enabled":{action_enabled},"order":100,"ui":{{"animation":"cloud_sync","title":"同步 GBAStation","units":["sav"],"show_cloud":true}}}}"#
        ),
    )
    .expect("action.json");
    std::fs::write(
        dir.join("actions").join("sync").join("main.lua"),
        "return\n",
    )
    .expect("main.lua");
}

fn install_boot_mod(root: &Path) {
    let dir = root.join("linkSlot/mods").join("boot-warmup");
    std::fs::create_dir_all(dir.join("actions").join("warmup")).expect("mod dirs");
    std::fs::write(
        dir.join("mod.json"),
        r#"{"schema":2,"name":"Boot","version":"1.0.0","enabled":true}"#,
    )
    .expect("mod.json");
    std::fs::write(
        dir.join("actions").join("warmup").join("action.json"),
        r#"{"schema":1,"name":"Warmup","timing":"after_boot","enabled":true,"order":100,"boot_delay_minutes":0}"#,
    )
    .expect("action.json");
    std::fs::write(
        dir.join("actions").join("warmup").join("main.lua"),
        "return\n",
    )
    .expect("main.lua");
    std::fs::create_dir_all(root.join("System")).expect("System");
    std::fs::write(root.join("System/linkSlot/shell/linkslot.sh"), "#!/bin/sh\n").expect("linkslot.sh");
}

#[test]
fn an_after_boot_action_launches_mods_boot_without_a_cart() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_boot_mod(d.path());
    let (mut a, fake) = rig(d.path());

    a.update(0.1);
    assert_eq!(fake.launches(), 1, "the after_boot action did not launch");
    let request = fake.last();
    assert_eq!(request.mode, SyncRequestMode::Boot);
    let argv = request.argv();
    assert_eq!(&argv[0..2], &["mods", "boot"]);
    assert!(argv.windows(2).any(|pair| pair == ["--id", "boot-warmup"]));
    assert!(argv.windows(2).any(|pair| pair == ["--action", "warmup"]));
    assert!(!argv.iter().any(|arg| arg == "--platform"));
}

#[test]
fn an_enabled_manual_mod_takes_over_the_long_press() {
    use slot::linkcfg::{write_sync_mode, SyncMode};

    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_sync_mode(d.path(), SyncMode::State).expect("write sync mode");
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());

    // The mod shows up as a second network card, and the chip says so.
    pick_network_card(&mut a, 1);
    assert!(
        a.network_label().contains("GBAStation"),
        "{}",
        a.network_label()
    );

    start_sync(&mut a);
    assert_eq!(fake.launches(), 1, "the long press did not run the mod");
    assert_eq!(fake.last().mode, SyncRequestMode::Mods);
    let argv = fake.last().argv();
    assert!(argv
        .windows(2)
        .any(|pair| pair == ["--id", "gbastation-battery-sync"]));
}

#[test]
fn a_manual_mod_can_bring_its_cloud_animation_declaration() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let action = d
        .path()
        .join("linkSlot/mods/gbastation-battery-sync/actions/sync/action.json");
    std::fs::write(
        &action,
        r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,"order":100,
            "ui":{"animation":"cloud_sync","title":"同步电池存档","units":["sav"],
                  "show_cloud":true,"direction":"upload"}}"#,
    )
    .unwrap();
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);

    let request = fake.last();
    let ui = request.ui.expect("the action ui did not reach the request");
    assert_eq!(ui.animation, "cloud_sync");
    assert_eq!(ui.title, "同步电池存档");
    assert_eq!(ui.units, vec!["sav"]);
    assert!(ui.show_cloud);
    assert_eq!(ui.direction.as_deref(), Some("upload"));
    assert_eq!(a.sync_texts().title, "同步电池存档");
}

#[test]
fn a_mod_card_runs_only_that_action() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);

    let request = fake.last();
    assert_eq!(request.mode, SyncRequestMode::Mods);
    let argv = request.argv();
    assert_eq!(&argv[0..2], &["mods", "run"]);
    assert!(argv
        .windows(2)
        .any(|pair| pair == ["--id", "gbastation-battery-sync"]));
    assert!(argv.windows(2).any(|pair| pair == ["--action", "sync"]));
    assert!(!argv.iter().any(|arg| arg == "--only"));
}

#[test]
fn the_insert_plays_before_the_job_starts() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    let tex = |raw: usize| card_tex(TexId::from_raw(raw));
    set_all_cart_faces(&mut a, tex(940));

    // Long press: the PCB opens, then the network card goes into the slot.
    hold_sync(&mut a);
    assert!(matches!(a.shelf_sync(), ShelfSync::Opening { .. }));
    a.update(1.2);
    a.update(1.0 / 60.0);
    assert_eq!(fake.launches(), 0, "the job started before the insert");

    let mut draws = Vec::new();
    a.draw(&mut draws);
    assert!(
        draws
            .iter()
            .any(|draw| matches!(*draw, Draw::Tex { .. })),
        "nothing is drawn during the insert"
    );

    for _ in 0..120 {
        a.update(1.0 / 60.0);
        if fake.launches() > 0 {
            break;
        }
    }
    assert_eq!(fake.launches(), 1, "the insert never handed over to the job");
    assert!(matches!(a.shelf_sync(), ShelfSync::Running { .. }));
}

#[test]
fn without_mods_the_game_card_steps_aside_and_comes_back() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    let tex = |raw: usize| card_tex(TexId::from_raw(raw));
    set_all_cart_faces(&mut a, tex(940));
    a.set_auto_cloud_card((tex(950), 360, 202));
    // The open PCB has its own board and lid textures; without them the lift
    // is only visible as a different draw order, which is too weak to assert.
    a.set_core_board_faces(tex(960), tex(961));

    // Which boxes the row drew, in screen order.
    let cart_boxes = |app: &mut App| {
        let mut draws = Vec::new();
        app.draw(&mut draws);
        let mut boxes: Vec<(i32, i32)> = draws
            .iter()
            .filter_map(|draw| match *draw {
                Draw::Tex { tex: found, x, w, .. } if found == tex(940) => {
                    Some((x.round() as i32, w.round() as i32))
                }
                _ => None,
            })
            .collect();
        boxes.sort();
        boxes
    };
    let board_drawn = |app: &mut App| {
        let mut draws = Vec::new();
        app.draw(&mut draws);
        draws
            .iter()
            .any(|draw| matches!(*draw, Draw::Tex { tex: found, .. } if found == tex(960)))
    };

    let before = cart_boxes(&mut a);
    assert!(!before.is_empty(), "the shelf drew no carts to begin with");
    assert!(!board_drawn(&mut a), "a closed shelf drew an open PCB");

    // Y down: the card lifts towards the PCB, which is its own drawing.
    a.apply(Action::GbaDown(Btn::Y));
    a.update(0.10);
    assert!(matches!(a.shelf_sync(), ShelfSync::Holding { .. }));
    assert!(board_drawn(&mut a), "the card did not start lifting");

    // Releasing before the threshold puts it back exactly where it was.
    a.apply(Action::GbaUp(Btn::Y));
    assert!(matches!(a.shelf_sync(), ShelfSync::Unholding { .. }));
    a.update(0.4);
    assert!(matches!(a.shelf_sync(), ShelfSync::Idle));
    assert!(!board_drawn(&mut a), "the open PCB stayed after the release");
    assert_eq!(cart_boxes(&mut a), before, "the game card did not come back");
    assert_eq!(fake.launches(), 0);

    // Holding through the threshold opens the PCB and starts the job.
    let (mut b, fake_b) = rig(d.path());
    set_all_cart_faces(&mut b, tex(940));
    b.set_auto_cloud_card((tex(950), 360, 202));
    b.apply(Action::GbaDown(Btn::Y));
    b.update(SYNC_HOLD_MS as f32 / 1000.0);
    assert!(matches!(b.shelf_sync(), ShelfSync::Opening { .. }));
    assert_eq!(fake_b.launches(), 0, "the job started during the opening");
    for _ in 0..120 {
        b.update(1.0 / 60.0);
        if fake_b.launches() > 0 {
            break;
        }
    }
    assert_eq!(fake_b.launches(), 1);
    assert!(matches!(b.shelf_sync(), ShelfSync::Running { .. }));
}

#[test]
fn without_mods_the_selected_game_card_still_starts_the_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    let tex = |raw: usize| card_tex(TexId::from_raw(raw));
    set_all_cart_faces(&mut a, tex(940));
    start_sync(&mut a);

    assert_eq!(fake.launches(), 1, "the direct cloud sync did not start");
    assert_eq!(fake.last().mode, SyncRequestMode::Cloud);
    let mut draws = Vec::new();
    a.draw(&mut draws);
    assert!(
        draws.iter().any(|draw| matches!(
            *draw,
            Draw::Tex { tex: found, .. } if found == tex(940)
        )),
        "the syncing game card is not visible"
    );
}

#[test]
fn a_mod_progress_update_reaches_the_sync_bar() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let action = d
        .path()
        .join("linkSlot/mods/gbastation-battery-sync/actions/sync/action.json");
    std::fs::write(
        &action,
        r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,"order":100,
            "ui":{"animation":"cloud_sync","title":"同步测试","units":[]}}"#,
    )
    .unwrap();
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.push(SyncUpdate::Progress(0.42));
    fake.push(SyncUpdate::ProgressLabel("正在生成攻略".to_string()));
    a.update(1.0 / 60.0);

    let texts = a.sync_texts();
    assert_eq!(texts.progress_left, "正在生成攻略");
    assert_eq!(texts.progress_right, "42%");
}

#[test]
fn a_mod_card_also_plays_the_insert_before_starting() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);

    hold_sync(&mut a);
    assert!(matches!(a.shelf_sync(), ShelfSync::Opening { .. }));
    assert_eq!(fake.launches(), 0, "the mod started before the opening");
    for _ in 0..120 {
        a.update(1.0 / 60.0);
        if fake.launches() > 0 {
            break;
        }
    }
    assert_eq!(fake.launches(), 1);
    assert_eq!(fake.last().mode, SyncRequestMode::Mods);
    assert!(matches!(a.shelf_sync(), ShelfSync::Running { .. }));
}

/// Install one manual mod under an arbitrary id, to stand in for SaveDeck.
fn install_manual_mod_as(root: &Path, id: &str, name: &str, order: i32) {
    let dir = root.join("linkSlot/mods").join(id);
    std::fs::create_dir_all(dir.join("actions").join("sync")).expect("mod dirs");
    std::fs::write(
        dir.join("mod.json"),
        format!(
            r#"{{"schema":2,"name":"{name}","version":"1.0.0","enabled":true,"order":{order}}}"#
        ),
    )
    .expect("mod.json");
    std::fs::write(
        dir.join("actions").join("sync").join("action.json"),
        r#"{"schema":1,"name":"同步","timing":"manual","enabled":true,"order":100,"ui":{"animation":"cloud_sync","title":"同步","units":["sav"],"show_cloud":true}}"#,
    )
    .expect("action.json");
    std::fs::write(dir.join("actions").join("sync").join("main.lua"), "return\n")
        .expect("main.lua");
}

#[test]
fn both_bridges_take_the_long_press() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod_as(d.path(), "savedeck-battery-bridge", "SaveDeck bridge", 100);
    install_manual_mod_as(d.path(), "gbastation-battery-sync", "GBAStation bridge", 100);

    // Card order follows the mod id; each card must run its own action.
    for (steps, id) in [
        (1isize, "gbastation-battery-sync"),
        (2, "savedeck-battery-bridge"),
    ] {
        let (mut a, fake) = rig(d.path());
        pick_network_card(&mut a, steps);
        start_sync(&mut a);
        let argv = fake.last().argv();
        assert_eq!(&argv[0..2], &["mods", "run"]);
        assert!(
            argv.windows(2).any(|pair| pair == ["--id", id]),
            "card {steps} ran the wrong mod: {argv:?}"
        );
    }
}

#[test]
fn a_disabled_mod_or_action_leaves_cloud_sync_alone() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), false, true);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    assert_eq!(fake.last().mode, SyncRequestMode::Cloud);

    let d2 = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d2.path(), true, false);
    let (mut b, fake) = rig(d2.path());
    start_sync(&mut b);
    assert_eq!(fake.last().mode, SyncRequestMode::Cloud);
}

#[test]
fn a_manual_choice_opens_a_modal_and_confirming_relaunches_with_choice() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.finish(choice_outcome());
    a.update(0.1);

    let ui = a.mod_choice_ui().expect("choice modal");
    // The mod's own title, prefixed with its name so two bridges stay apart.
    assert!(ui.title.contains("存档冲突"), "{}", ui.title);
    assert!(ui.title.contains("GBAStation"), "{}", ui.title);
    assert_eq!(ui.selected, 0);
    assert_eq!(ui.options.len(), 2);

    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(a.mod_choice_ui().expect("choice modal").selected, 1);
    a.apply(Action::GbaDown(Btn::A));

    assert_eq!(fake.launches(), 2);
    let request = fake.last();
    assert_eq!(request.mode, SyncRequestMode::Mods);
    assert_eq!(
        request.choices,
        vec![("conflict".to_string(), "local".to_string())]
    );
    // The retry still targets the bridge that was picked, not the cloud and
    // not whichever mod happens to sort first.
    assert!(request
        .argv()
        .windows(2)
        .any(|pair| pair == ["--id", "gbastation-battery-sync"]));
    assert!(request
        .argv()
        .windows(2)
        .any(|pair| pair == ["--action", "sync"]));
    assert!(request.argv().windows(2).any(|pair| {
        pair == ["--choice".to_string(), "conflict=local".to_string()]
    }));

    fake.finish(ok(Direction::Upload, 1));
    a.update(0.1);
    assert_eq!(banner(&a), "已推送版本 1");
}

#[test]
fn b_on_a_confirm_dialog_relaunches_with_no_instead_of_cancelling() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);

    fake.finish(SyncOutcome {
        result: SyncResult::Choice,
        choice: Some(slot_mods::ChoiceRequest {
            key: "ui_confirm_1".to_string(),
            kind: slot_mods::ChoiceKind::Confirm,
            title: "继续？".to_string(),
            body: "测试".to_string(),
            options: vec![
                slot_mods::ChoiceOption {
                    id: "yes".to_string(),
                    label: "继续".to_string(),
                },
                slot_mods::ChoiceOption {
                    id: "no".to_string(),
                    label: "停止".to_string(),
                },
            ],
            default: Some("no".to_string()),
        }),
        ..outcome(SyncResult::Choice)
    });
    a.update(1.0 / 60.0);
    assert_eq!(
        a.mod_choice_ui().map(|ui| ui.options.len()),
        Some(2),
        "the confirmation modal did not open"
    );

    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(fake.launches(), 2, "B did not resume the mod with no");
    assert_eq!(
        fake.last().choices,
        vec![("ui_confirm_1".to_string(), "no".to_string())]
    );
}

#[test]
fn b_on_a_manual_choice_cancels_without_relaunching() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.finish(choice_outcome());
    a.update(0.1);
    assert!(a.mod_choice_ui().is_some());

    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(banner(&a), "已取消");
    assert_eq!(fake.launches(), 1);
    assert_eq!(fake.cancels(), 0);
}

#[test]
fn manual_mods_show_their_own_stage_and_error_detail() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.stage(SyncStage::Mods);
    a.update(0.1);
    assert_eq!(banner(&a), "正在同步");

    fake.finish(SyncOutcome {
        result: SyncResult::Error,
        error: Some("config".to_string()),
        detail: Some("请先在 mod 管理页填写 GBAStation 地址".to_string()),
        ..outcome(SyncResult::Error)
    });
    a.update(0.1);
    assert_eq!(banner(&a), "请先在 mod 管理页填写 GBAStation 地址");
}

#[test]
fn manual_mods_show_their_own_noop_detail() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.finish(SyncOutcome {
        result: SyncResult::Noop,
        detail: Some("两边都有存档但内容不同，请先选择策略".to_string()),
        ..outcome(SyncResult::Noop)
    });
    a.update(0.1);
    assert_eq!(banner(&a), "两边都有存档但内容不同，请先选择策略");
}

#[test]
fn manual_mods_show_their_own_success_detail() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, fake) = rig(d.path());
    pick_network_card(&mut a, 1);
    start_sync(&mut a);
    fake.finish(SyncOutcome {
        result: SyncResult::Ok,
        direction: Some(Direction::Download),
        detail: Some("已从 GBAStation 拉取电池存档".to_string()),
        ..outcome(SyncResult::Ok)
    });
    a.update(0.1);
    assert_eq!(banner(&a), "已从 GBAStation 拉取电池存档");
}

// ---- the other shelf keys keep working ------------------------------------

#[test]
fn a_short_a_press_still_starts_the_cart() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::A));
    a.apply(Action::GbaUp(Btn::A));
    assert!(matches!(a.phase(), Phase::Inserting { .. }));
    assert_eq!(fake.launches(), 0);
}

#[test]
fn holding_a_still_starts_the_cart_from_the_beginning() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::A));
    a.update(0.51);
    assert!(matches!(a.phase(), Phase::Inserting { .. }));
    assert!(a.starting_clean(), "a held A must start a clean session");
    assert_eq!(fake.launches(), 0);
}

#[test]
fn dismissing_the_settle_beat_puts_the_game_card_back() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    // With a cart face the flow inserts the network cartridge into the slot,
    // which is what keeps the game card hidden while the sync overlay is up.
    for shelf in 0..8 {
        for index in 0..8 {
            a.set_cart_face(shelf, index, TexId::from_raw(9));
        }
    }
    start_sync(&mut a);
    fake.finish(outcome(SyncResult::Noop));
    for _ in 0..240 {
        a.update(1.0 / 60.0);
        if matches!(a.shelf_sync(), ShelfSync::Settling { .. }) {
            break;
        }
    }
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Settling { .. }),
        "the sync never reached its settle beat"
    );

    // Pressing a key during the settle beat used to jump straight to Idle and
    // strand the network card, leaving the selected cartridge undrawn.
    a.apply(Action::GbaDown(Btn::A));
    for _ in 0..240 {
        a.update(1.0 / 60.0);
    }
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Idle),
        "the sync overlay never finished"
    );
    let mut out = Vec::new();
    a.draw(&mut out);
    // Faces are invalidated by a finished sync, so the shelf may draw the
    // selected cart as a fallback rectangle; both count as "the card is back".
    let carts = out
        .iter()
        .filter(|draw| match **draw {
            Draw::Rect { w, .. } | Draw::Tex { w, .. } | Draw::Turned { w, .. } => {
                (w - 360.0).abs() < 1.0
            }
            Draw::Game | Draw::Shot { .. } => false,
        })
        .count();
    assert!(
        carts >= 1,
        "the game card stayed hidden after dismissing the settle beat"
    );
}

#[test]
fn m_opens_the_settings_menu_and_is_not_a_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::QuickMenu);
    assert!(matches!(a.phase(), Phase::QuickMenu { .. }));
    assert_eq!(fake.launches(), 0);
}

#[test]
fn y_in_the_game_intro_deletes_after_confirmation() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, _fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::B));
    a.update(1.3);
    assert!(a.game_intro().is_some(), "intro did not open");

    a.apply(Action::GbaDown(Btn::Y));
    let (title, rows, selected) = a.utility_text().expect("no delete confirmation");
    assert!(title.contains("Emerald"), "{title}");
    assert_eq!(rows, vec!["删除卡带".to_string(), "取消".to_string()]);
    assert_eq!(selected, 0);

    a.apply(Action::GbaDown(Btn::A));
    assert!(!d.path().join("Games/GBA/Emerald.gba").exists());
    assert!(d.path().join("Games/GBA/Fusion.gba").exists());
    assert!(a.game_intro().is_none(), "intro stayed after delete");
    assert!(a.utility_text().is_none(), "confirmation stayed after delete");
}

#[test]
fn x_opens_the_chinese_search_keyboard() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::X));
    assert!(
        matches!(
            a.keyboard.as_ref().map(|kb| &kb.purpose),
            Some(KeyboardPurpose::GameSearch)
        ),
        "a short X no longer opens the game search"
    );
    assert_eq!(fake.launches(), 0);
}

#[test]
fn holding_b_opens_the_game_intro() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    std::fs::create_dir_all(d.path().join("Labels/GBA")).unwrap();
    std::fs::write(
        d.path().join("Labels/GBA/Emerald.txt"),
        "这是一段游戏介绍。",
    )
    .unwrap();
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::B));
    a.update(1.3);
    let intro = a.game_intro().expect("no game intro");
    assert_eq!(intro.title, "Emerald");
    assert!(intro.description.contains("游戏介绍"));
    assert!(a.keyboard.is_none(), "holding B opened search");
    a.set_game_intro_scroll(2);
    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(a.game_intro().unwrap().scroll, 1);
    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(a.game_intro().unwrap().scroll, 2);
    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(a.game_intro().unwrap().scroll, 2, "scroll passed the end");
    a.apply(Action::GbaDown(Btn::Up));
    assert_eq!(a.game_intro().unwrap().scroll, 1);
    a.apply(Action::GbaUp(Btn::B));
    assert!(a.game_intro().is_some(), "releasing B closed the intro");
    a.apply(Action::GbaDown(Btn::B));
    assert!(
        a.game_intro().is_some(),
        "closing dropped the card instead of playing out"
    );
    a.update(0.45);
    assert!(a.game_intro().is_none(), "the card never finished closing");
    assert_eq!(fake.launches(), 0);
}

#[test]
fn start_still_opens_the_core_picker() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Start));
    assert!(
        a.core_picker().is_some(),
        "START no longer opens the picker"
    );
    assert_eq!(fake.launches(), 0);
}

#[test]
fn y_never_starts_a_sync_from_the_game_or_the_album() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = common::app_playing_in(d.path(), "Emerald");
    let fake = Fake::new();
    a.set_sync_launcher(Box::new(fake.clone()));
    a.apply(Action::GbaDown(Btn::Y));
    a.update(SYNC_HOLD_MS as f32 / 1000.0);
    assert!(matches!(a.phase(), Phase::Playing { .. }));
    assert_eq!(fake.launches(), 0, "Y started a sync from the game");

    a.apply(Action::Polaroids);
    assert!(matches!(a.phase(), Phase::Polaroids { .. }));
    a.apply(Action::GbaDown(Btn::Y));
    a.apply(Action::GbaUp(Btn::Y));
    a.update(SYNC_HOLD_MS as f32 / 1000.0);
    assert_eq!(fake.launches(), 0, "Y started a sync from the album");
    assert!(!a.shelf_sync_active());
}

// ---- the overlay freezes the rest of the pad -------------------------------

#[test]
fn a_running_sync_swallows_every_key_but_b() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    running_phase(&a);
    for action in [
        Action::GbaDown(Btn::A),
        Action::GbaUp(Btn::A),
        Action::GbaDown(Btn::X),
        Action::GbaDown(Btn::Start),
        Action::GbaDown(Btn::L1),
        Action::GbaDown(Btn::R1),
        Action::GbaDown(Btn::Y),
        Action::QuickMenu,
    ] {
        a.apply(action);
    }
    assert!(matches!(a.shelf_sync(), ShelfSync::Running { .. }));
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(fake.launches(), 1);
    assert_eq!(fake.cancels(), 0);
}

#[test]
fn a_held_but_not_yet_triggered_press_ignores_other_keys() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Y));
    a.apply(Action::GbaDown(Btn::A));
    a.apply(Action::GbaUp(Btn::A));
    a.apply(Action::GbaDown(Btn::Start));
    assert!(matches!(a.shelf_sync(), ShelfSync::Holding { .. }));
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(fake.launches(), 0);
}

// ---- progress, direction and result text ----------------------------------

#[test]
fn the_stage_and_direction_reach_the_overlay_texts() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);

    fake.stage(SyncStage::Net);
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "正在连接网络");

    fake.stage(SyncStage::Download);
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "正在下载存档");

    fake.direction(Direction::Download);
    a.update(1.0 / 60.0);
    match running_phase(&a) {
        ShelfSync::Running { direction, .. } => {
            assert_eq!(*direction, Some(Direction::Download));
        }
        _ => unreachable!(),
    }
}

#[test]
fn a_pull_success_shows_the_pulled_version_then_fades() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(ok(Direction::Download, 7));
    a.update(1.0 / 60.0);

    assert!(
        matches!(a.shelf_sync(), ShelfSync::Settling { .. }),
        "the completed cloud animation did not appear"
    );
    assert_eq!(banner(&a), "已拉取版本 7");

    a.update(1.2);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Result { .. }),
        "the completion animation did not hand over to the result banner"
    );
    a.update(SYNC_RESULT_MS as f32 / 1000.0 + 0.1);
    assert!(!a.shelf_sync_active(), "the result banner never faded");
}

/// The cloud card is only inserted when the shelf has a face for it, so this
/// test gives every cart one: that is the path a device takes, and the one
/// where the eject used to flash the screen black.
#[test]
fn the_result_after_the_card_ejects_does_not_flash_the_screen() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    let n = a.carts().count();
    a.set_faces((0..n).map(|i| TexId::from_raw(700 + i)).collect());

    start_sync(&mut a);
    fake.finish(ok(Direction::Download, 7));
    a.update(1.0 / 60.0);
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Settling { .. }),
        "the sync did not settle"
    );

    let mut ejected = false;
    for _ in 0..400 {
        a.update(1.0 / 60.0);
        match a.shelf_sync() {
            ShelfSync::Ejecting { .. } => ejected = true,
            ShelfSync::Result { .. } => break,
            _ => {}
        }
    }
    assert!(ejected, "the cloud card never ejected");
    assert!(
        matches!(a.shelf_sync(), ShelfSync::Result { .. }),
        "the eject never handed over to the result banner"
    );
    assert!(
        !a.sync_result_fade_active(),
        "the result painted the shelf black again after the eject"
    );
}

#[test]
fn a_push_success_shows_the_new_version() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(ok(Direction::Upload, 4));
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "已推送版本 4");
}

#[test]
fn a_noop_says_up_to_date() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(outcome(SyncResult::Noop));
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "已是最新");
}

#[test]
fn network_errors_show_the_cli_detail_and_other_errors_show_a_short_line() {
    for (category, line) in [
        (Some("cert"), "云端证书校验失败"),
        (Some("verify"), "存档校验失败"),
        (None, "同步失败"),
    ] {
        let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
        let (mut a, fake) = rig(d.path());
        start_sync(&mut a);
        fake.finish(failure(category));
        a.update(1.0 / 60.0);
        assert_eq!(banner(&a), line, "wrong short line for {category:?}");
        assert!(
            !banner(&a).contains("linkSlot"),
            "the raw CLI detail leaked into the banner"
        );
    }

    // A network failure is exactly where the generic "网络未连接" used to
    // hide the real cause (Wi-Fi up, WebDAV unreachable); the CLI detail is
    // now the banner text.
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(failure(Some("net")));
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "linkSlot: 失败摘要");
}

#[test]
fn a_process_that_vanishes_without_a_block_reports_an_error() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.exit_without_outcome();
    a.update(1.0 / 60.0);
    assert_eq!(banner(&a), "同步进程意外退出");
}

#[test]
fn any_key_dismisses_a_result_banner_without_starting_anything() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(ok(Direction::Download, 2));
    a.update(1.0 / 60.0);
    a.apply(Action::GbaDown(Btn::A));
    a.apply(Action::GbaUp(Btn::A));
    assert!(!a.shelf_sync_active(), "a key did not dismiss the banner");
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(fake.launches(), 1);
}

// ---- cancel and timeout ----------------------------------------------------

#[test]
fn b_cancels_the_running_job_and_shows_cancelled_then_idles() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    a.apply(Action::GbaDown(Btn::B));
    assert_eq!(fake.cancels(), 1, "the CLI was never told to stop");
    assert!(
        fake.cancelled(),
        "the fake run did not see the cancellation"
    );
    assert_eq!(banner(&a), "已取消");
    a.update(1.3);
    assert!(!a.shelf_sync_active(), "the cancelled banner stayed up");
    assert!(matches!(a.phase(), Phase::Shelf));
    assert!(!matches!(a.phase(), Phase::Playing { .. }));
}

#[test]
fn a_job_running_past_two_minutes_is_cancelled_with_a_timeout_line() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    a.update(SYNC_TIMEOUT_MS as f32 / 1000.0 + 1.0);

    assert_eq!(fake.cancels(), 1, "the timed out job was not terminated");
    assert_eq!(banner(&a), "同步超时");
    a.update(SYNC_RESULT_MS as f32 / 1000.0 + 0.1);
    assert!(!a.shelf_sync_active());
}

#[test]
fn closing_the_lid_cancels_a_running_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    a.apply(Action::LidClose);
    assert!(matches!(a.phase(), Phase::Doze { .. }));
    assert_eq!(fake.cancels(), 1, "the CLI kept running under the lid");
    assert!(!a.shelf_sync_active(), "the overlay survived the lid close");
}

#[test]
fn the_power_menu_cancels_a_running_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    a.apply(Action::PowerHold);
    assert!(a.power_menu().is_some(), "the power menu did not open");
    assert_eq!(fake.cancels(), 1, "the CLI kept running behind the menu");
    assert!(!a.shelf_sync_active());
}

// ---- conflicts -------------------------------------------------------------

fn both_sides() -> ConflictInfo {
    ConflictInfo {
        cloud: VersionInfo {
            version: Some(7),
            device: Some("rg34xx-a1b2c3d4".to_string()),
            updated: Some("2026-10-20T10:00:00Z".to_string()),
            files: Some(3),
        },
        local: VersionInfo {
            version: Some(6),
            device: None,
            updated: None,
            files: None,
        },
    }
}

fn unit_conflict(kind: UnitKind) -> UnitOutcome {
    UnitOutcome {
        kind,
        result: Some(SyncResult::Conflict),
        direction: None,
        version: None,
        error: Some("remote-newer".to_string()),
        info: Some(both_sides()),
    }
}

fn unit_ok(kind: UnitKind, version: u32) -> UnitOutcome {
    UnitOutcome {
        kind,
        result: Some(SyncResult::Ok),
        direction: Some(Direction::Download),
        version: Some(version),
        error: None,
        info: None,
    }
}

fn unit_failed(kind: UnitKind, error: &str) -> UnitOutcome {
    UnitOutcome {
        kind,
        result: Some(SyncResult::Error),
        direction: None,
        version: None,
        error: Some(error.to_string()),
        info: None,
    }
}

fn dual_conflict() -> SyncOutcome {
    SyncOutcome {
        error: Some("remote-newer".to_string()),
        conflict_units: vec![UnitKind::State, UnitKind::Sav],
        state: Some(unit_conflict(UnitKind::State)),
        sav: Some(unit_conflict(UnitKind::Sav)),
        ..outcome(SyncResult::Conflict)
    }
}

fn single_conflict(kind: UnitKind) -> SyncOutcome {
    let mut result = outcome(SyncResult::Conflict);
    result.error = Some("remote-newer".to_string());
    result.conflict_units = vec![kind];
    match kind {
        UnitKind::State => result.state = Some(unit_conflict(kind)),
        UnitKind::Sav => result.sav = Some(unit_conflict(kind)),
    }
    result
}

fn units_outcome(result: SyncResult) -> SyncOutcome {
    let mut out = outcome(result);
    for kind in UnitKind::ALL {
        if kind == UnitKind::State {
            out.state = Some(unit_ok(kind, 8));
        } else {
            out.sav = Some(unit_ok(kind, 9));
        }
    }
    out
}

fn open_conflict(app: &mut App, fake: &Fake, result: SyncOutcome) {
    start_sync(app);
    fake.finish(result);
    app.update(1.0 / 60.0);
}

fn ui(app: &App) -> ConflictUi {
    app.conflict_ui()
        .unwrap_or_else(|| panic!("expected the conflict UI to be up"))
}

#[test]
fn two_conflicting_units_open_three_pages_with_local_wins_default() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());

    let view = ui(&a);
    assert_eq!(
        view.pages,
        vec![ConflictPage::All, ConflictPage::State, ConflictPage::Sav]
    );
    assert_eq!(view.page, ConflictPage::All);
    assert_eq!(view.direction, SyncChoice::Download);
    assert!(!view.locked_state && !view.locked_sav);
    assert!(!view.syncing);
    let state = view.state.expect("the state unit is missing");
    let sav = view.sav.expect("the sav unit is missing");
    assert!(
        state.cloud_line.contains("v7") && state.cloud_line.contains("rg34xx-a1b2c3d4"),
        "cloud line was {:?}",
        state.cloud_line
    );
    assert!(
        state.cloud_line.contains("3 个文件"),
        "the file count is missing: {:?}",
        state.cloud_line
    );
    assert!(
        state.local_line.contains("v6"),
        "local line was {:?}",
        state.local_line
    );
    assert!(sav.cloud_line.contains("v7"));
}

#[test]
fn one_conflicting_unit_opens_a_single_page_without_tabs() {
    for kind in UnitKind::ALL {
        let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
        let (mut a, fake) = rig(d.path());
        open_conflict(&mut a, &fake, single_conflict(kind));
        let view = ui(&a);
        assert_eq!(view.pages.len(), 1, "a single conflict must not show tabs");
        assert_eq!(
            view.page,
            match kind {
                UnitKind::State => ConflictPage::State,
                UnitKind::Sav => ConflictPage::Sav,
            }
        );
        assert!(a.shelf_sync_active());
    }
}

#[test]
fn legacy_single_unit_conflict_falls_back_to_the_state_page() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, conflict(both_sides()));
    let view = ui(&a);
    assert_eq!(view.pages, vec![ConflictPage::State]);
    assert!(view
        .state
        .expect("the legacy state info is missing")
        .cloud_line
        .contains("v7"));
}

#[test]
fn a_conflict_never_times_out_on_its_own() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.update(SYNC_TIMEOUT_MS as f32 / 1000.0 + 60.0);
    assert!(matches!(a.shelf_sync(), ShelfSync::Conflict { .. }));
    assert_eq!(fake.cancels(), 0);
}

#[test]
fn left_and_right_cycle_the_three_pages() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());

    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::State);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::Sav);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::All, "pages do not wrap");
    a.apply(Action::GbaDown(Btn::Left));
    assert_eq!(ui(&a).page, ConflictPage::Sav);
    a.apply(Action::ShelfLeft);
    assert_eq!(ui(&a).page, ConflictPage::State);
}

#[test]
fn left_and_right_do_nothing_on_a_single_page() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, single_conflict(UnitKind::Sav));
    let before = ui(&a);
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::Left));
    assert_eq!(ui(&a).pages, before.pages);
    assert_eq!(ui(&a).page, before.page);
}

#[test]
fn up_and_down_flip_the_direction_of_the_current_page() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());

    a.apply(Action::GbaDown(Btn::Up));
    assert_eq!(ui(&a).direction, SyncChoice::Upload);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::State);
    assert_eq!(
        ui(&a).direction,
        SyncChoice::Download,
        "every page has its own direction"
    );
    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(ui(&a).direction, SyncChoice::Upload);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).direction, SyncChoice::Download);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::All);
    assert_eq!(
        ui(&a).direction,
        SyncChoice::Upload,
        "the all-page choice was not kept"
    );
}

#[test]
fn a_on_the_all_page_resolves_both_units_in_one_call() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    assert_eq!(fake.launches(), 1);

    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(fake.launches(), 2, "A did not launch the conflict sync");
    let request = fake.last();
    assert_eq!(request.only, None);
    assert_eq!(request.resolve, ResolveSpec::all(Direction::Download));
    assert_eq!(request.expect.state, Some(7));
    assert_eq!(request.expect.sav, Some(7));
    assert!(matches!(a.shelf_sync(), ShelfSync::Running { .. }));
    assert!(
        a.conflict_ui().is_none(),
        "the conflict pages must hand over to the sync animation"
    );
    assert_eq!(a.sync_texts().title, "云存档同步");
}

#[test]
fn a_on_a_unit_page_syncs_only_that_unit() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));

    a.apply(Action::GbaDown(Btn::A));
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::State));
    assert_eq!(
        request.resolve,
        ResolveSpec {
            state: Some(Direction::Download),
            sav: None,
        }
    );
    assert_eq!(request.expect.state, Some(7));
    assert_eq!(request.expect.sav, None);
    let argv = request.argv();
    assert!(argv.windows(2).any(|pair| pair == ["--only", "state"]));
    assert!(argv
        .windows(2)
        .any(|pair| pair == ["--resolve", "state=download"]));
}

#[test]
fn a_successful_unit_page_locks_it_and_moves_the_focus() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::A));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);

    let view = ui(&a);
    assert!(view.locked_state, "the synced state page was not locked");
    assert!(!view.locked_sav);
    assert_eq!(view.page, ConflictPage::Sav, "focus did not move");
    assert!(view.error.is_none());

    a.apply(Action::GbaDown(Btn::A));
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::Sav));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    assert!(
        a.conflict_ui().is_none(),
        "the last locked unit must close the pages"
    );
    assert!(banner(&a).contains("已同步"));
}

#[test]
fn the_all_page_is_read_only_after_a_unit_locks() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::A));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::All);

    let direction = ui(&a).direction;
    a.apply(Action::GbaDown(Btn::Up));
    assert_eq!(
        ui(&a).direction,
        direction,
        "the all page direction changed after a lock"
    );
    let launches = fake.launches();
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(
        fake.launches(),
        launches,
        "A on the locked all page launched"
    );
    assert_eq!(ui(&a).notice.as_deref(), Some("✓ 已同步"));

    // The locked unit page is read-only too.
    a.apply(Action::GbaDown(Btn::Left));
    assert_eq!(ui(&a).page, ConflictPage::Sav);
    a.apply(Action::GbaDown(Btn::Left));
    assert_eq!(ui(&a).page, ConflictPage::State);
    let direction = ui(&a).direction;
    a.apply(Action::GbaDown(Btn::Down));
    assert_eq!(ui(&a).direction, direction);
    a.apply(Action::GbaDown(Btn::A));
    assert_eq!(fake.launches(), launches);
}

#[test]
fn b_abandons_the_remaining_unit_with_a_banner() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::A));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    assert_eq!(ui(&a).page, ConflictPage::Sav);

    a.apply(Action::GbaDown(Btn::B));
    assert!(a.conflict_ui().is_none(), "B did not close the pages");
    assert!(
        banner(&a).contains("电池存档未同步"),
        "the abandon banner was {:?}",
        banner(&a)
    );
    assert_eq!(fake.launches(), 2, "B launched another sync");
}

#[test]
fn b_on_a_fresh_conflict_closes_without_launching() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::B));
    assert!(a.conflict_ui().is_none(), "B did not close the pages");
    assert!(matches!(a.phase(), Phase::Shelf));
    assert_eq!(fake.launches(), 1, "B confirmed something");
    assert!(banner(&a).contains("未同步"));
}

#[test]
fn a_unit_page_failure_stays_and_retries_the_same_page() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::A));
    let mut failed = outcome(SyncResult::Error);
    failed.error = Some("net".to_string());
    failed.detail = Some("无法连接 WebDAV".to_string());
    failed.state = Some(unit_failed(UnitKind::State, "net"));
    fake.finish(failed);
    a.update(1.0 / 60.0);

    let view = ui(&a);
    assert_eq!(view.page, ConflictPage::State, "the failed page moved away");
    assert!(!view.locked_state, "a failed page must not lock");
    assert!(
        view.error
            .as_deref()
            .unwrap_or("")
            .contains("无法连接 WebDAV"),
        "error was {:?}",
        view.error
    );
    a.apply(Action::GbaDown(Btn::Right));
    assert_eq!(ui(&a).page, ConflictPage::Sav, "pages stay navigable");
    a.apply(Action::GbaDown(Btn::Left));
    a.apply(Action::GbaDown(Btn::A));
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::State));
}

#[test]
fn a_partial_all_page_retry_touches_only_the_failed_unit() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::A));

    let mut partial = outcome(SyncResult::Partial);
    partial.error = Some("net".to_string());
    partial.state = Some(unit_ok(UnitKind::State, 8));
    partial.sav = Some(unit_failed(UnitKind::Sav, "net"));
    fake.finish(partial);
    a.update(1.0 / 60.0);
    let view = ui(&a);
    assert_eq!(view.page, ConflictPage::All);
    assert!(!view.locked_state && !view.locked_sav);
    assert!(
        view.error.as_deref().unwrap_or("").contains("电池存档失败"),
        "partial error was {:?}",
        view.error
    );

    a.apply(Action::GbaDown(Btn::A));
    let request = fake.last();
    assert_eq!(request.only, Some(UnitKind::Sav));
    assert_eq!(
        request.resolve,
        ResolveSpec {
            state: None,
            sav: Some(Direction::Download),
        }
    );
    assert_eq!(request.expect.state, None);
    assert_eq!(request.expect.sav, Some(7));

    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    assert!(
        a.conflict_ui().is_none(),
        "the retry did not close the pages"
    );
}

#[test]
fn changed_since_resets_the_conflict_pages() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, dual_conflict());
    a.apply(Action::GbaDown(Btn::Right));
    a.apply(Action::GbaDown(Btn::A));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    assert!(ui(&a).locked_state);

    a.apply(Action::GbaDown(Btn::A));
    let mut changed = outcome(SyncResult::Error);
    changed.error = Some("changed-since".to_string());
    changed.changed = Some(ChangedSince {
        unit: Some(UnitKind::Sav),
        expected: Some(7),
        actual: Some(9),
    });
    let mut fresh_sav = unit_conflict(UnitKind::Sav);
    fresh_sav.info = Some(ConflictInfo {
        cloud: VersionInfo {
            version: Some(9),
            device: Some("other-device".to_string()),
            ..VersionInfo::default()
        },
        local: both_sides().local,
    });
    changed.sav = Some(fresh_sav);
    fake.finish(changed);
    a.update(1.0 / 60.0);

    let view = ui(&a);
    assert_eq!(view.page, ConflictPage::All, "the pages were not reset");
    assert!(!view.locked_state && !view.locked_sav);
    assert_eq!(view.direction, SyncChoice::Download);
    assert!(view.error.is_none());
    assert!(
        view.notice
            .as_deref()
            .unwrap_or("")
            .contains("云端已被其它设备更新"),
        "notice was {:?}",
        view.notice
    );
    assert!(
        view.sav
            .expect("the sav page is missing")
            .cloud_line
            .contains("v9"),
        "the refreshed version is not shown"
    );
}

#[test]
fn the_last_locked_unit_closes_the_conflict() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    open_conflict(&mut a, &fake, single_conflict(UnitKind::Sav));
    a.apply(Action::GbaDown(Btn::A));
    fake.finish(units_outcome(SyncResult::Ok));
    a.update(1.0 / 60.0);
    assert!(a.conflict_ui().is_none());
    assert!(banner(&a).contains("已同步"));
}

// ---- drawing ---------------------------------------------------------------

#[test]
fn the_hold_draws_a_cyan_frame_and_a_progress_bar() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, _fake) = rig(d.path());
    a.apply(Action::GbaDown(Btn::Y));
    a.update(0.75);
    let out = drew(&a);
    assert!(
        has_colour(&out, [0.34, 0.89, 0.95, 1.0]),
        "the held cart has no cyan frame"
    );
    assert!(
        has_rect(&out, 120.0, 6.0),
        "the hold progress bar (120x6) is missing"
    );
}

#[test]
fn a_running_sync_dims_the_shelf_and_stays_up_until_the_result() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    let out = drew(&a);
    assert!(
        out.iter().any(|draw| matches!(
            draw,
            Draw::Rect { w, h, colour, .. }
                if (*w - 720.0).abs() < 0.01 && (*h - 480.0).abs() < 0.01 && colour[3] > 0.2
        )),
        "the running overlay does not dim the shelf"
    );
    fake.finish(ok(Direction::Download, 1));
    a.update(1.0 / 60.0);
    assert!(matches!(
        a.shelf_sync(),
        ShelfSync::Settling { .. } | ShelfSync::Result { .. }
    ));
}

fn face(tex: usize, w: u32, h: u32) -> SyncFace {
    SyncFace {
        tex: TexId::from_raw(tex),
        w,
        h,
    }
}

#[test]
fn the_sync_cloud_is_drawn_over_the_translucent_header() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, _fake) = rig(d.path());
    let (cloud, up, down) = (face(31, 132, 88), face(32, 42, 42), face(33, 42, 42));
    a.set_sync_art(cloud, up, down);
    start_sync(&mut a);
    let out = drew(&a);

    let header = out.iter().position(|draw| {
        matches!(
            *draw,
            Draw::Rect { w, h, colour, .. }
                if (w - 720.0).abs() < 0.01
                    && (h - 36.0).abs() < 0.01
                    && colour[3] < 0.5
        )
    });
    let cloud = out
        .iter()
        .position(|draw| matches!(*draw, Draw::Tex { tex, .. } if tex == TexId::from_raw(31)));
    let (header, cloud) = (
        header.expect("the translucent status band was not drawn"),
        cloud.expect("the sync cloud was not drawn"),
    );
    assert!(
        cloud > header,
        "the status band is painted over the sync cloud"
    );
}

#[test]
fn the_result_banner_is_drawn_with_the_rasterised_text() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    fake.finish(ok(Direction::Download, 7));
    a.update(1.0 / 60.0);
    a.set_sync_text_faces(SyncFaces {
        banner: Some(face(21, 300, 28)),
        ..SyncFaces::default()
    });
    let out = drew(&a);
    assert!(
        out.iter()
            .any(|draw| matches!(*draw, Draw::Tex { tex, .. } if tex == TexId::from_raw(21))),
        "the banner text was not drawn"
    );
    assert!(
        out.iter()
            .any(|draw| matches!(*draw, Draw::Rect { colour, .. } if colour[3] > 0.3)),
        "the banner has no backing band"
    );
}

// ---- gating ----------------------------------------------------------------

#[test]
fn a_linked_session_refuses_to_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, fake) = rig(d.path());
    a.begin_link(0);
    attempt_sync(&mut a);
    assert_eq!(fake.launches(), 0, "a sync ran during a link session");
    assert!(
        banner(&a).contains("联机"),
        "the refusal line was {:?}",
        banner(&a)
    );
}

#[test]
fn a_read_only_card_refuses_to_sync() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, read_only) = common::app_playing_with_card(d.path(), "Emerald");
    read_only.store(true, Ordering::Relaxed);
    a.apply(Action::Eject);
    for _ in 0..90 {
        a.update(1.0 / 60.0);
    }
    assert!(
        matches!(a.phase(), Phase::Shelf),
        "the cart did not eject to the shelf"
    );
    let fake = Fake::new();
    a.set_sync_launcher(Box::new(fake.clone()));

    attempt_sync(&mut a);
    assert_eq!(fake.launches(), 0, "a sync ran with a read-only card");
    assert!(
        banner(&a).contains("只读"),
        "the refusal line was {:?}",
        banner(&a)
    );
}

#[test]
fn the_sync_request_uses_the_selected_carts_platform_and_core() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    slot_store::write_selected_core(d.path(), "Emerald", slot_store::Core::Gpsp).expect("core");
    let (mut a, fake) = rig(d.path());
    start_sync(&mut a);
    let request = fake.last();
    assert_eq!(request.platform, "GBA");
    assert_eq!(
        request.emulator, "gpsp",
        "the selected core is not passed to the CLI"
    );
}

// ---- the long-press Y chooser's miniature cartridges ----------------------

fn card_tex(tex: TexId) -> TexId {
    tex
}

fn set_all_cart_faces(app: &mut App, tex: TexId) {
    for shelf in 0..16 {
        for index in 0..16 {
            if app.face_cart(shelf, index).is_none() {
                break;
            }
            app.set_cart_face(shelf, index, tex);
        }
    }
}

#[test]
fn a_manual_mod_can_ship_a_cover_for_its_network_chip() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    install_manual_mod(d.path(), true, true);
    let (mut a, _fake) = rig(d.path());

    // Without a cover the chip paints the default markings.
    pick_network_card(&mut a, 1);
    assert!(a.sync_cover_path().is_none());

    // A `sync-chip.png` in the mod directory skins the chip.
    let cover = d.path().join("linkSlot/mods/gbastation-battery-sync/sync-chip.png");
    std::fs::write(&cover, b"png").expect("sync chip");
    let (mut b, _fake) = rig(d.path());
    pick_network_card(&mut b, 1);
    let found = b.sync_cover_path().expect("no cover");
    assert!(
        found.ends_with("linkSlot/mods/gbastation-battery-sync/sync-chip.png"),
        "wrong cover path: {}",
        found.display()
    );
}

