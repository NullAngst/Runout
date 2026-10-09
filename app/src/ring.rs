//! Single-producer single-consumer ring of audio frames. The audio callback
//! only ever pops from it; it never locks or allocates.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Floats per frame: original L/R, processed L/R, removed L/R.
pub const FRAME: usize = 6;

pub struct Ring {
    buf: Box<[UnsafeCell<f32>]>,
    cap: usize,
    head: AtomicUsize, // next write (producer)
    tail: AtomicUsize, // next read (consumer)
    /// Producer asks the consumer to drop everything queued.
    pub flush: AtomicBool,
}

unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Ring {
    pub fn new(frames: usize) -> Self {
        let cap = frames + 1;
        let buf: Vec<UnsafeCell<f32>> = (0..cap * FRAME).map(|_| UnsafeCell::new(0.0)).collect();
        Ring { buf: buf.into_boxed_slice(), cap, head: AtomicUsize::new(0), tail: AtomicUsize::new(0), flush: AtomicBool::new(false) }
    }

    pub fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        (h + self.cap - t) % self.cap
    }

    pub fn free(&self) -> usize {
        self.cap - 1 - self.len()
    }

    /// Producer: push one frame. Returns false when full.
    pub fn push(&self, f: &[f32; FRAME]) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        let next = (h + 1) % self.cap;
        if next == t {
            return false;
        }
        for (i, v) in f.iter().enumerate() {
            // SAFETY: only the producer writes slot h, and the consumer does not
            // read it until head moves past it.
            unsafe { *self.buf[h * FRAME + i].get() = *v };
        }
        self.head.store(next, Ordering::Release);
        true
    }

    /// Consumer: pop one frame.
    pub fn pop(&self) -> Option<[f32; FRAME]> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let mut f = [0.0f32; FRAME];
        for (i, v) in f.iter_mut().enumerate() {
            // SAFETY: slot t was published by the producer's Release store.
            *v = unsafe { *self.buf[t * FRAME + i].get() };
        }
        self.tail.store((t + 1) % self.cap, Ordering::Release);
        Some(f)
    }

    /// Consumer: drop everything queued.
    pub fn clear_from_consumer(&self) {
        let h = self.head.load(Ordering::Acquire);
        self.tail.store(h, Ordering::Release);
    }
}
