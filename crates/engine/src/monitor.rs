//! Input monitoring: hearing the recording input through the outputs.
//!
//! The input stream's callback pushes frames into a lock-free single-producer /
//! single-consumer ring; the engine pops them into the master output each block. If the
//! two devices drift apart, the ring drops the oldest audio to keep latency low.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Frames held at most (power of two).
const CAPACITY: usize = 1 << 14;
/// Above this many queued frames the reader skips ahead (≈ 40 ms at 48 kHz).
const MAX_LAG: usize = 2048;

pub struct MonitorRing {
    buf: Box<[UnsafeCell<[f32; 2]>]>,
    write: AtomicUsize,
    read: AtomicUsize,
}

// One thread writes, one reads; slots are only touched by the side that owns them.
unsafe impl Sync for MonitorRing {}

impl Default for MonitorRing {
    fn default() -> Self {
        MonitorRing { buf: (0..CAPACITY).map(|_| UnsafeCell::new([0.0; 2])).collect(), write: AtomicUsize::new(0), read: AtomicUsize::new(0) }
    }
}

impl MonitorRing {
    /// Producer side: queues interleaved frames with `channels` channels (mono is doubled).
    pub fn push(&self, interleaved: &[f32], channels: usize) {
        self.push_converted(interleaved, channels, |sample| sample);
    }

    /// Also accepts integer input devices without allocating in their callback.
    pub fn push_converted<T: Copy>(&self, interleaved: &[T], channels: usize, convert: fn(T) -> f32) {
        let channels = channels.max(1);
        let mut w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        for frame in interleaved.chunks_exact(channels) {
            if w.wrapping_sub(r) >= CAPACITY - 1 {
                break; // full: the reader is far behind; drop new input
            }
            let l = convert(frame[0]);
            let rr = if channels > 1 { convert(frame[1]) } else { l };
            unsafe { *self.buf[w % CAPACITY].get() = [l, rr] };
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Consumer side (audio thread): adds queued frames to `left`/`right`.
    pub fn mix_into(&self, left: &mut [f32], right: &mut [f32], gain: f32) {
        let w = self.write.load(Ordering::Acquire);
        let mut r = self.read.load(Ordering::Relaxed);
        if w.wrapping_sub(r) > MAX_LAG + left.len() {
            r = w.wrapping_sub(left.len());
        }
        for (l, rr) in left.iter_mut().zip(right.iter_mut()) {
            if r == w {
                break;
            }
            let [a, b] = unsafe { *self.buf[r % CAPACITY].get() };
            *l += a * gain;
            *rr += b * gain;
            r = r.wrapping_add(1);
        }
        self.read.store(r, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_audio_and_bounds_latency() {
        let ring = MonitorRing::default();
        ring.push(&[0.5, -0.5, 0.25, -0.25], 2);
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        ring.mix_into(&mut l, &mut r, 1.0);
        assert_eq!(l, [0.5, 0.25, 0.0, 0.0]);
        assert_eq!(r, [-0.5, -0.25, 0.0, 0.0]);

        // A backlog is skipped so monitoring doesn't lag further and further behind.
        let backlog: Vec<f32> = (0..10_000).map(|i| i as f32).collect();
        ring.push(&backlog, 1);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        ring.mix_into(&mut l, &mut r, 1.0);
        assert_eq!(l[63], 9_999.0);
    }
}
