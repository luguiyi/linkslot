use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::UNIX_EPOCH;

use slot_gfx::{Compositor, TexId};
use slot_store::Cart;
use slot_ui::{cart_face_with, CartFace};

use crate::app::{App, FaceWant};
use crate::label_cache::label_art;

const WORKERS: usize = 2;

type Key = (usize, usize);

#[derive(Default)]
struct Queue {
    jobs: VecDeque<(Key, Cart, u64)>,
    closed: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

pub struct CartFaces {
    root: PathBuf,
    queue: Shared,
    built: Receiver<(Key, u64, CartFace)>,
    pending: HashSet<Key>,
    asked: Vec<FaceWant>,
    free: Vec<TexId>,
}

impl CartFaces {
    pub fn spawn(root: PathBuf) -> Self {
        let queue: Shared = Arc::default();
        let (outbox, built) = mpsc::channel();
        for n in 0..WORKERS {
            let (queue, outbox, root) = (queue.clone(), outbox.clone(), root.clone());
            let spawned = thread::Builder::new()
                .name(format!("slot-carts-{n}"))
                .spawn(move || work(&queue, &outbox, &root));
            if let Err(e) = spawned {
                eprintln!("slot: cart faces: worker thread failed to start: {e}");
            }
        }
        CartFaces {
            root,
            queue,
            built,
            pending: HashSet::new(),
            asked: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn sync(&mut self, app: &mut App, compositor: &mut Compositor) {
        self.free.extend(app.shed_faces());
        let invalidated = app.take_invalidated_faces();
        if !invalidated.is_empty() {
            self.free.extend(invalidated);
            // Drop both queued work and the "asked" snapshot: every face must
            // be rebuilt from the new label, and a worker already inside the
            // old job must not satisfy the new generation.
            self.pending.clear();
            self.asked.clear();
        }
        while let Ok((key, generation, face)) = self.built.try_recv() {
            self.place(app, compositor, key, generation, face);
        }
        let wants = app.face_wants();
        if wants != self.asked {
            self.requeue(app, &wants);
            self.asked = wants;
        }
        let mut waiting: Vec<Key> = self
            .asked
            .iter()
            .filter(|w| w.on_screen)
            .map(|w| (w.shelf, w.index))
            .collect();
        while !waiting.is_empty() {
            let Ok((key, generation, face)) = self.built.recv() else {
                return;
            };
            waiting.retain(|k| *k != key);
            self.place(app, compositor, key, generation, face);
        }
    }

    fn requeue(&mut self, app: &App, wants: &[FaceWant]) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        for (key, _, _) in queue.jobs.drain(..) {
            self.pending.remove(&key);
        }
        let generation = app.face_generation();
        for w in wants {
            let key = (w.shelf, w.index);
            if self.pending.contains(&key) {
                continue;
            }
            if let Some(cart) = app.face_cart(w.shelf, w.index) {
                let mut cart = cart.clone();
                if cart.label.is_none() && app.fallback_cover_enabled() {
                    cart.label = latest_state_screenshot(&self.root, &cart);
                }
                queue.jobs.push_back((key, cart, generation));
                self.pending.insert(key);
            }
        }
        wake.notify_all();
    }

    fn place(
        &mut self,
        app: &mut App,
        compositor: &mut Compositor,
        key: Key,
        generation: u64,
        face: CartFace,
    ) {
        if generation != app.face_generation() {
            return;
        }
        self.pending.remove(&key);
        self.asked.retain(|w| (w.shelf, w.index) != key);
        let tex = match self.free.pop() {
            Some(tex) => {
                compositor.update_texture(tex, face.w, face.h, &face.rgba);
                tex
            }
            None => compositor.create_texture(face.w, face.h, &face.rgba),
        };
        self.free.extend(app.set_cart_face(key.0, key.1, tex));
    }
}

impl Drop for CartFaces {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        wake.notify_all();
    }
}

/// Newest screenshot produced by an in-game save state for a cartridge.
/// `resume.png` is written next to the automatic resume state on exit, so a
/// file with the same FAT timestamp as a timestamped snapshot wins.
/// Covers only the no-label case; an explicit Labels/<name>.png always wins.
fn latest_state_screenshot(root: &Path, cart: &Cart) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    // Every core the app can load, not a hand-written pair: the state
    // directory is `States/<platform>/<core.as_str()>/<stem>`, so a core left
    // out here is a machine whose carts never get a fallback cover — which is
    // exactly what happened to the Famicom while this read `["mgba", "gpsp"]`.
    for core in slot_store::Core::EVERY {
        let dir = root
            .join("States")
            .join(cart.platform.dir_name())
            .join(core.as_str())
            .join(&cart.stem);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            let prefer_resume = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("resume.png"));
            let wins = best.as_ref().is_none_or(|(stamp, current)| {
                modified > *stamp
                    || (modified == *stamp
                        && prefer_resume
                        && current
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_none_or(|name| !name.eq_ignore_ascii_case("resume.png")))
            });
            if wins {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn work(queue: &Shared, outbox: &Sender<(Key, u64, CartFace)>, root: &Path) {
    let (lock, wake) = &**queue;
    loop {
        let (key, cart, generation) = {
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.closed {
                    return;
                }
                if let Some(job) = q.jobs.pop_front() {
                    break job;
                }
                q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let face = cart_face_with(&cart, label_art(root, &cart));
        if outbox.send((key, generation, face)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slot_store::{Cart, Platform};

    fn cart(platform: Platform, stem: &str) -> Cart {
        Cart {
            platform,
            stem: stem.to_string(),
            rom: PathBuf::from(format!("Games/{}/{stem}.rom", platform.dir_name())),
            label: None,
            title: String::new(),
            code: String::new(),
            shell: None,
        model: Default::default(),
        front: None,
}
    }

    fn shot(root: &Path, platform: Platform, core: &str, stem: &str, name: &str) {
        let dir = root
            .join("States")
            .join(platform.dir_name())
            .join(core)
            .join(stem);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"png").unwrap();
    }

    /// The fallback cover is "the newest screenshot of this cart", and a cart's
    /// screenshots live under whichever core ran it. A hand-written core list
    /// here is a machine that silently never gets a cover — the Famicom's case.
    #[test]
    fn the_fallback_cover_finds_a_famicom_carts_screenshot() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let fc = cart(Platform::Fc, "Mario");
        shot(root, Platform::Fc, "fceumm", "Mario", "resume.png");

        assert_eq!(
            latest_state_screenshot(root, &fc),
            Some(root.join("States/FC/fceumm/Mario/resume.png")),
            "an FC cart's own screenshot was not seen"
        );
    }

    /// Same cart name on two machines is two carts, and the cover must come
    /// from the one being drawn — `States/GBA/mgba/Emerald` is not `States/FC/…`.
    #[test]
    fn one_cart_never_wears_another_machines_screenshot() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        shot(root, Platform::Gba, "mgba", "Emerald", "resume.png");

        assert_eq!(latest_state_screenshot(root, &cart(Platform::Fc, "Emerald")), None);
        assert_eq!(
            latest_state_screenshot(root, &cart(Platform::Gba, "Emerald")),
            Some(root.join("States/GBA/mgba/Emerald/resume.png"))
        );
    }

    /// Two cores can leave a screenshot for the same cart (a card whose
    /// `selected_core.txt` changed machines) — the newest file wins regardless
    /// of which core wrote it.
    #[test]
    fn the_newest_screenshot_wins_across_cores() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        shot(root, Platform::Gba, "mgba", "Emerald", "2026-01-01_00-00-00.png");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        shot(root, Platform::Gba, "gpsp", "Emerald", "2026-01-02_00-00-00.png");

        let found = latest_state_screenshot(root, &cart(Platform::Gba, "Emerald")).unwrap();
        assert!(
            found.ends_with(Path::new("gpsp").join("Emerald").join("2026-01-02_00-00-00.png")),
            "the older screenshot won: {found:?}"
        );
    }
}
