use std::path::PathBuf;
use std::time::{Duration, Instant};

use slot::frontend::Frontend;
use slot::input::DeviceInput;
use slot_gfx::{Compositor, Draw, FbdevSurface, Surface};
use slot_power::{trace_first_frame, DevicePlatform};

const MIN_FRAME: Duration = Duration::from_millis(12);

const STEP_TIMEOUT: Duration = Duration::from_millis(12);

const MARGIN: Duration = Duration::from_micros(1500);

const LOG_SYNC: Duration = Duration::from_secs(1);

struct Pacer {
    last_return: Option<Instant>,
    period: Duration,
    work: Duration,
    works: [Duration; WORK_WINDOW],
    next: usize,
}

const WORK_WINDOW: usize = 32;

impl Pacer {
    fn new() -> Self {
        Pacer {
            last_return: None,
            period: Duration::from_micros(16_760),
            work: Duration::from_millis(4),
            works: [Duration::from_millis(4); WORK_WINDOW],
            next: 0,
        }
    }

    fn wait(&self) {
        let Some(last) = self.last_return else {
            return;
        };
        let delay = self.period.saturating_sub(self.work + MARGIN);
        if let Some(left) = (last + delay).checked_duration_since(Instant::now()) {
            std::thread::sleep(left);
        }
    }

    fn swapped(&mut self, work: Duration, blocked: bool) -> bool {
        let now = Instant::now();
        let mut missed = false;
        if let Some(last) = self.last_return {
            let seen = now - last;
            missed = seen > self.period.mul_f64(1.5);
            if blocked && seen > Duration::from_millis(12) && seen < Duration::from_millis(22) {
                self.period = self.period.mul_f64(0.95) + seen.mul_f64(0.05);
            }
        }
        self.works[self.next] = work;
        self.next = (self.next + 1) % WORK_WINDOW;
        self.work = self.works.iter().copied().max().unwrap_or(work);
        self.last_return = Some(now);
        missed
    }
}

pub fn run() {
    slot::boot_time::mark("start");
    let _ = std::thread::Builder::new()
        .name("slot-fonts".into())
        .spawn(|| {
            slot_ui::warm_fonts();
            slot::boot_time::mark("fonts ready");
        });
    let audio = std::thread::Builder::new()
        .name("slot-audio-open".into())
        .spawn(slot::audio::opened_sink);
    let root = slot_model::resolve_root(None, std::env::var_os("SLOT_ROOT").map(PathBuf::from));
    slot::app::App::prefetch_scan_async(root.clone());
    let mut surface = match FbdevSurface::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("slot: {e}");
            return;
        }
    };
    slot::boot_time::mark("surface");
    let mut compositor = match Compositor::new(&surface) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("slot: {e}");
            return;
        }
    };
    slot::boot_time::mark("compositor");
    let panel = surface.window_size();
    let turned = panel.0 < panel.1;
    let window = if turned { (panel.1, panel.0) } else { panel };
    let ccw = std::env::var("SLOT_ROTATE")
        .map(|v| v.trim().eq_ignore_ascii_case("ccw"))
        .unwrap_or(false);
    eprintln!(
        "slot: panel {}x{}, canvas {}x{}, turned {turned}, ccw {ccw}",
        panel.0, panel.1, window.0, window.1
    );
    if turned {
        if let Err(e) = compositor.set_panel_turn(panel, ccw) {
            eprintln!("slot: {e}");
            return;
        }
    }
    let platform = DevicePlatform::new(root.clone());
    eprintln!("slot: {}", platform.report());
    platform.trace_boot();
    slot::boot_time::mark("platform");
    // The launcher is still showing its own boot splash.  Put our copy of that
    // logo on screen as soon as the compositor exists (~1 s instead of ~2.2 s)
    // and drop the first-frame marker, so the launcher stops waiting early.
    let boot_marker = std::env::var_os("SLOT_BOOT_MARKER");
    let mut drawn = false;
    if boot_marker.is_some() && draw_boot_splash(&mut compositor, &root, window) {
        let _ = surface.swap();
        drawn = true;
        slot::boot_time::mark("first frame");
        if let Some(path) = &boot_marker {
            let _ = std::fs::write(path, b"zh-lcd4-r2 first frame\n");
        }
        trace_first_frame();
    }
    let sink = match audio.map(|h| h.join()) {
        Ok(Ok(sink)) => sink,
        _ => slot::audio::opened_sink(),
    };
    slot::boot_time::mark("audio");
    let mut frontend = Frontend::boot_with(Box::new(platform), sink);
    slot::boot_time::mark("frontend");
    frontend.upload_boot_faces(&mut compositor);
    slot::boot_time::mark("faces");
    frontend.upload_bezel(&mut compositor, window);
    slot::boot_time::mark("bezel");
    let mut input = DeviceInput::open(&root);
    slot::boot_time::mark("input");
    let card = root.clone();
    let _ = std::thread::Builder::new()
        .name("slot-bootlogo".into())
        .spawn(move || {
            slot::boot_time::mark("bootlogo start");
            slot::bootlogo::refresh(&card);
            slot::boot_time::mark("bootlogo done");
        });
    let _ = std::thread::Builder::new()
        .name("slot-logsync".into())
        .spawn(sync_log_writes);
    slot::boot_time::mark("drive emulator");
    frontend.drive_emulator();
    slot::boot_time::mark("drive emulator done");
    let mut pacer = Pacer::new();
    let (mut frames, mut missed, mut early) = (0u32, 0u32, 0u32);
    loop {
        pacer.wait();
        let began = Instant::now();
        frontend.advance(&mut input);
        if frontend.restarting() {
            frontend.restart();
        }
        if frontend.repairing() {
            frontend.repair();
        }
        if frontend.powering_off() {
            frontend.poweroff();
            frontend.shutdown();
            return;
        }
        frontend.step_emulator(pacer.period, STEP_TIMEOUT);
        frontend.render(&mut compositor, window);
        let swap = Instant::now();
        let work = swap - began;
        if let Err(e) = surface.swap() {
            eprintln!("slot: {e}");
            frontend.shutdown();
            return;
        }
        let swap_took = swap.elapsed();
        slot::latency::swapped(swap_took.as_secs_f64() * 1000.0);
        let dropped = pacer.swapped(work, swap_took > Duration::from_millis(1));
        if slot::latency::tracing() {
            frames += 1;
            missed += u32::from(dropped);
            early += u32::from(swap_took > pacer.period / 2);
            if frames == 600 {
                eprintln!(
                    "slot: pace: 600 frames, {missed} missed a latch, {early} drawn early, work {:.1} ms, period {:.2} ms",
                    pacer.work.as_secs_f64() * 1e3,
                    pacer.period.as_secs_f64() * 1e3
                );
                (frames, missed, early) = (0, 0, 0);
            }
        }
        if !drawn {
            drawn = true;
            slot::boot_time::mark("first frame");
            if let Some(path) = std::env::var_os("SLOT_BOOT_MARKER") {
                if let Err(err) = std::fs::write(path, b"zh-lcd4-r2 first frame\n") {
                    eprintln!("slot: boot marker: {err}");
                }
            }
            trace_first_frame();
        }
        if let Some(left) = MIN_FRAME.checked_sub(began.elapsed()) {
            std::thread::sleep(left);
        }
    }
}

fn sync_log_writes() {
    let mut synced = 0;
    loop {
        std::thread::sleep(LOG_SYNC);
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(2, &mut st) } != 0 {
            return;
        }
        if st.st_size != synced {
            #[cfg(target_os = "linux")]
            unsafe {
                libc::syncfs(2);
            }
            synced = st.st_size;
        }
    }
}

/// Paint the boot logo (the same bitmap the firmware shows) over the whole
/// panel.  Returns false when no decodable logo exists: in that case the
/// bootloader's own splash is a better picture than a black frame.
fn draw_boot_splash(compositor: &mut Compositor, root: &std::path::Path, window: (u32, u32)) -> bool {
    let mut logo = None;
    for name in [slot::bootlogo::USER_LOGO, slot::bootlogo::CARD_LOGO] {
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            continue;
        };
        if let Some(decoded) = slot::bootlogo::decode(&bytes) {
            logo = Some(decoded);
            break;
        }
    }
    let Some(logo) = logo else {
        return false;
    };
    let (panel_w, panel_h) = window;
    if panel_w == 0 || panel_h == 0 {
        return false;
    }
    let scale = (panel_w as f32 / logo.width as f32).min(panel_h as f32 / logo.height as f32);
    let width = (logo.width as f32 * scale).round().max(1.0);
    let height = (logo.height as f32 * scale).round().max(1.0);
    let mut rgba = Vec::with_capacity(logo.rgb.len() * 4);
    for pixel in &logo.rgb {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    let tex = compositor.create_texture(logo.width as u32, logo.height as u32, &rgba);
    compositor.fit(window);
    compositor.begin_frame();
    let mut draws = vec![Draw::Rect {
        x: 0.0,
        y: 0.0,
        w: panel_w as f32,
        h: panel_h as f32,
        colour: [0.02, 0.02, 0.03, 1.0],
    }];
    draws.push(Draw::Tex {
        x: (panel_w as f32 - width) / 2.0,
        y: (panel_h as f32 - height) / 2.0,
        w: width,
        h: height,
        tex,
        alpha: 1.0,
    });
    compositor.draw_list(&draws);
    compositor.end_frame(window);
    compositor.delete_texture(tex);
    true
}
