//! Lock-freier SPSC-Ring.
//!
//! **Phasentrennung:** Der Producer (cpal-Callback) schreibt nur den
//! Write-Cursor. Der Consumer (Worker nach Stream-Drop) schreibt nur den
//! Read-Cursor. Bei vollem Ring verwirft der Producer das *neueste* Frame und
//! zählt Overflow — kein producerseitiges Überschreiben des Read-Cursors
//! (codex H5 / agy B2). `pop`/`drain`/`reset` nur, wenn der Producer steht.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Interleaved Samples. Overflow zählt verworfene **Frames**.
pub struct OverwriteSpsc<T: Copy> {
    buf: Box<[UnsafeCell<T>]>,
    cap: usize,
    channels: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    overflow: AtomicU64,
}

unsafe impl<T: Copy + Send> Send for OverwriteSpsc<T> {}
unsafe impl<T: Copy + Send> Sync for OverwriteSpsc<T> {}

impl<T: Copy + Default> OverwriteSpsc<T> {
    pub fn new(min_samples: usize, channels: usize) -> Self {
        let channels = channels.max(1);
        let frames = min_samples.div_ceil(channels).max(2);
        let cap = frames.saturating_mul(channels);
        let mut buf = Vec::with_capacity(cap);
        for _ in 0..cap {
            buf.push(UnsafeCell::new(T::default()));
        }
        Self {
            buf: buf.into_boxed_slice(),
            cap,
            channels,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn capacity_samples(&self) -> usize {
        self.cap
    }

    pub fn overflow(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }

    /// Stand des Write-Cursors. Nur zum **Beobachten** gedacht: der Consumer
    /// erkennt daran, ob der Producer nach dem Pausieren des Streams noch
    /// schreibt, bevor er `drain`/`reset` aufruft (codex H5).
    pub fn write_pos(&self) -> usize {
        self.write.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        self.write.store(0, Ordering::Relaxed);
        self.read.store(0, Ordering::Relaxed);
        self.overflow.store(0, Ordering::Relaxed);
    }

    /// Producer: ein interleaved Frame. Niemals blockieren.
    /// Bei vollem Ring: Frame verwerfen, Overflow++, Read-Cursor unangetastet.
    pub fn push_frame(&self, frame: &[T]) {
        let n = self.channels;
        if frame.len() < n || n == 0 || self.cap < n {
            return;
        }
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let used = w.wrapping_sub(r);
        if used + n > self.cap {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return;
        }
        for (i, sample) in frame.iter().copied().take(n).enumerate() {
            let idx = w.wrapping_add(i) % self.cap;
            unsafe {
                *self.buf[idx].get() = sample;
            }
        }
        self.write.store(w.wrapping_add(n), Ordering::Release);
    }

    /// Consumer: ein Sample. Nur der Consumer schreibt `read`.
    pub fn pop(&self) -> Option<T> {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let idx = r % self.cap;
        let value = unsafe { *self.buf[idx].get() };
        self.read.store(r.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    pub fn drain(&self, out: &mut Vec<T>) {
        while let Some(v) = self.pop() {
            out.push(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_drops_newest_and_counts() {
        let rb = OverwriteSpsc::<i16>::new(4, 1);
        assert_eq!(rb.capacity_samples(), 4);
        for v in [1, 2, 3, 4] {
            rb.push_frame(&[v]);
        }
        assert_eq!(rb.overflow(), 0);
        rb.push_frame(&[5]);
        rb.push_frame(&[6]);
        assert_eq!(rb.overflow(), 2);
        let mut got = Vec::new();
        rb.drain(&mut got);
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[test]
    fn stereo_overflow_keeps_frame_alignment() {
        let rb = OverwriteSpsc::<i16>::new(4, 2);
        rb.push_frame(&[1, 2]);
        rb.push_frame(&[3, 4]);
        rb.push_frame(&[5, 6]);
        assert_eq!(rb.overflow(), 1);
        let mut got = Vec::new();
        rb.drain(&mut got);
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[test]
    fn overflow_does_not_panic_when_full() {
        let rb = OverwriteSpsc::<f32>::new(2, 1);
        for i in 0..100 {
            rb.push_frame(&[i as f32]);
        }
        assert!(rb.overflow() >= 98);
        let mut got = Vec::new();
        rb.drain(&mut got);
        assert_eq!(got.len(), rb.capacity_samples());
        assert_eq!(got, vec![0.0, 1.0]);
    }
}
