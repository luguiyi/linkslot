use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The machine's picture size, carried with the buffer.
///
/// It used to be one constant — the Advance's 240x160 — because the buffer was
/// always that one frame. A Famicom hands over 256x224, so the size has to
/// travel with the bytes: nothing downstream (the game texture, the save-state
/// thumbnail) can cut the picture at the right stride without it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameSize {
    pub w: u32,
    pub h: u32,
}

pub struct Frames {
    inner: Mutex<Inner>,
    size: usize,
    taken: AtomicU64,
}

struct Inner {
    ready: Option<(Vec<u8>, FrameSize)>,
    spare: Vec<Vec<u8>>,
    allocated: usize,
}

impl Frames {
    pub fn new(size: usize) -> Arc<Self> {
        Arc::new(Frames {
            inner: Mutex::new(Inner {
                ready: None,
                spare: Vec::new(),
                allocated: 0,
            }),
            size,
            taken: AtomicU64::new(0),
        })
    }

    pub fn take_write(&self) -> Vec<u8> {
        let mut i = self.lock();
        match i.spare.pop() {
            Some(buf) => buf,
            None => {
                i.allocated += 1;
                Vec::with_capacity(self.size)
            }
        }
    }

    /// Hand over one finished frame, with the size its own core says it is.
    pub fn publish(&self, buf: Vec<u8>, size: FrameSize) {
        let mut i = self.lock();
        if let Some((dropped, _)) = i.ready.replace((buf, size)) {
            i.spare.push(dropped);
        }
    }

    pub fn latest(self: &Arc<Self>) -> Option<FrameRef> {
        let (buf, size) = self.lock().ready.take()?;
        self.taken.fetch_add(1, Ordering::Relaxed);
        Some(FrameRef {
            frames: self.clone(),
            buf,
            size,
        })
    }

    pub fn is_ready(&self) -> bool {
        self.lock().ready.is_some()
    }

    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    pub fn allocated(&self) -> usize {
        self.lock().allocated
    }

    fn recycle(&self, buf: Vec<u8>) {
        self.lock().spare.push(buf);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct FrameRef {
    frames: Arc<Frames>,
    buf: Vec<u8>,
    size: FrameSize,
}

impl FrameRef {
    pub fn width(&self) -> u32 {
        self.size.w
    }

    pub fn height(&self) -> u32 {
        self.size.h
    }

    pub fn size(&self) -> FrameSize {
        self.size
    }
}

impl Deref for FrameRef {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl Drop for FrameRef {
    fn drop(&mut self) {
        self.frames.recycle(std::mem::take(&mut self.buf));
    }
}
