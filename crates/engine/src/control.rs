//! Lock-free controls shared between the UI and the audio thread.
//!
//! The UI writes transport requests and mixer settings; the engine reads them
//! once per block and writes back the playhead position and meter peaks.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// An `f32` stored atomically as its bit pattern.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Raises the stored value to `v` if larger (used for meter peaks).
    pub fn raise(&self, v: f32) {
        let mut cur = self.0.load(Ordering::Relaxed);
        while v > f32::from_bits(cur) {
            match self.0.compare_exchange_weak(cur, v.to_bits(), Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Reads and resets to zero (the UI collects peaks this way).
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0f32.to_bits(), Ordering::Relaxed))
    }
}

/// Settings and meters for one channel or mixer insert.
#[derive(Debug)]
pub struct Strip {
    /// Linear gain.
    pub volume: AtomicF32,
    /// -1.0 (left) ..= 1.0 (right).
    pub pan: AtomicF32,
    pub mute: AtomicBool,
    pub solo: AtomicBool,
    pub peak_l: AtomicF32,
    pub peak_r: AtomicF32,
    /// For channels: the mixer insert they feed.
    pub route: AtomicU32,
    /// For inserts: the built-in compressor's gain reduction, dB.
    pub gain_reduction: AtomicF32,
    /// Peak signal level entering the built-in compressor, in dBFS.
    pub comp_input_db: AtomicF32,
    /// Output level at the same sample as `comp_input_db`, in dBFS.
    pub comp_output_db: AtomicF32,
    /// Peak level heard by the detector (which may be a sidechain), in dBFS.
    pub comp_detector_db: AtomicF32,
}

impl Strip {
    pub fn new(volume: f32, pan: f32, mute: bool) -> Self {
        Self {
            route: AtomicU32::new(0),
            gain_reduction: AtomicF32::new(0.0),
            comp_input_db: AtomicF32::new(-120.0),
            comp_output_db: AtomicF32::new(-120.0),
            comp_detector_db: AtomicF32::new(-120.0),
            volume: AtomicF32::new(volume),
            pan: AtomicF32::new(pan),
            mute: AtomicBool::new(mute),
            solo: AtomicBool::new(false),
            peak_l: AtomicF32::new(0.0),
            peak_r: AtomicF32::new(0.0),
        }
    }

    pub fn is_muted(&self) -> bool {
        self.mute.load(Ordering::Relaxed)
    }

    pub fn is_soloed(&self) -> bool {
        self.solo.load(Ordering::Relaxed)
    }

    pub fn route(&self) -> usize {
        self.route.load(Ordering::Relaxed) as usize
    }

    pub fn set_route(&self, insert: usize) {
        self.route.store(insert as u32, Ordering::Relaxed);
    }
}

const NO_SEEK: u64 = u64::MAX;

#[derive(Debug)]
pub struct Controls {
    pub sample_rate: f64,
    /// Song length in samples, including the release tail (changes as the song is edited).
    length: AtomicU64,
    playing: AtomicBool,
    seek: AtomicU64,
    position: AtomicU64,
    /// Loop region in samples; disabled when `loop_end <= loop_start`.
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    /// Share of real time the last block took to compute (1.0 = 100%).
    pub load: AtomicF32,
    /// Blocks that took longer than real time (each one is likely an audible dropout).
    pub overloads: AtomicU32,
    pub channels: Vec<Strip>,
    pub inserts: Vec<Strip>,
    /// Mute per playlist track.
    pub tracks: Vec<AtomicBool>,
    /// Click on every beat while playing.
    pub metronome: AtomicBool,
    /// Metronome level (linear, 1.0 = default).
    pub metronome_gain: AtomicF32,
    /// Beat length in samples, and beats per bar (for the metronome's accent).
    pub samples_per_beat: AtomicF32,
    pub beats_per_bar: AtomicU32,
    /// Frames of the sample preview played so far ([`NO_PREVIEW`] when none is playing).
    pub preview_position: AtomicU64,
    /// Count-in before recording: samples left, and the count-in's full length.
    count_in: AtomicU64,
    count_in_total: AtomicU64,
}

pub const NO_PREVIEW: u64 = u64::MAX;

/// Playlist tracks the engine can mute (FL Studio's maximum).
pub const MAX_TRACKS: usize = 500;

impl Controls {
    pub fn new(sample_rate: f64, length: u64, channels: Vec<Strip>, inserts: Vec<Strip>) -> Self {
        Self {
            sample_rate,
            length: AtomicU64::new(length),
            playing: AtomicBool::new(false),
            seek: AtomicU64::new(NO_SEEK),
            position: AtomicU64::new(0),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            load: AtomicF32::new(0.0),
            overloads: AtomicU32::new(0),
            channels,
            inserts,
            tracks: (0..MAX_TRACKS).map(|_| AtomicBool::new(false)).collect(),
            metronome: AtomicBool::new(false),
            metronome_gain: AtomicF32::new(1.0),
            samples_per_beat: AtomicF32::new((sample_rate * 0.5) as f32),
            beats_per_bar: AtomicU32::new(4),
            preview_position: AtomicU64::new(NO_PREVIEW),
            count_in: AtomicU64::new(0),
            count_in_total: AtomicU64::new(0),
        }
    }

    pub fn length(&self) -> u64 {
        self.length.load(Ordering::Relaxed)
    }

    pub(crate) fn set_length(&self, length: u64) {
        self.length.store(length, Ordering::Relaxed);
    }

    pub fn set_track_muted(&self, track: usize, muted: bool) {
        if let Some(t) = self.tracks.get(track) {
            t.store(muted, Ordering::Relaxed);
        }
    }

    pub fn is_track_muted(&self, track: usize) -> bool {
        self.tracks.get(track).is_some_and(|t| t.load(Ordering::Relaxed))
    }

    pub fn play(&self) {
        self.playing.store(true, Ordering::Release);
    }

    /// Plays `samples` of count-in clicks before the song moves (call before `play`).
    pub fn start_count_in(&self, samples: u64) {
        self.count_in_total.store(samples, Ordering::Relaxed);
        self.count_in.store(samples, Ordering::Release);
    }

    pub fn count_in_left(&self) -> u64 {
        self.count_in.load(Ordering::Acquire)
    }

    pub fn count_in_total(&self) -> u64 {
        self.count_in_total.load(Ordering::Relaxed)
    }

    pub(crate) fn consume_count_in(&self, samples: u64) {
        let left = self.count_in.load(Ordering::Acquire);
        self.count_in.store(left.saturating_sub(samples), Ordering::Release);
    }

    /// Stores the last block's load and counts overloads.
    pub(crate) fn record_load(&self, load: f32) {
        self.load.store(load);
        if load > 1.0 {
            self.overloads.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn stop(&self) {
        self.count_in.store(0, Ordering::Release);
        self.playing.store(false, Ordering::Release);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    /// Requests a jump; the engine applies it at the next block.
    pub fn seek(&self, sample: u64) {
        let sample = sample.min(self.length());
        self.seek.store(sample, Ordering::Release);
        // Show the new position immediately, even while stopped.
        self.position.store(sample, Ordering::Release);
    }

    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    pub fn set_loop(&self, range: Option<(u64, u64)>) {
        let (s, e) = range.unwrap_or((0, 0));
        self.loop_start.store(s, Ordering::Relaxed);
        self.loop_end.store(e, Ordering::Relaxed);
    }

    pub fn loop_range(&self) -> Option<(u64, u64)> {
        let (s, e) = (self.loop_start.load(Ordering::Relaxed), self.loop_end.load(Ordering::Relaxed));
        (e > s).then_some((s, e))
    }

    pub(crate) fn take_seek(&self) -> Option<u64> {
        let s = self.seek.swap(NO_SEEK, Ordering::AcqRel);
        (s != NO_SEEK).then_some(s)
    }

    pub(crate) fn set_position(&self, sample: u64) {
        self.position.store(sample, Ordering::Release);
    }

    pub fn any_insert_soloed(&self) -> bool {
        self.inserts.iter().skip(1).any(Strip::is_soloed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_raise_and_take() {
        let p = AtomicF32::new(0.0);
        p.raise(0.5);
        p.raise(0.25);
        assert_eq!(p.take(), 0.5);
        assert_eq!(p.load(), 0.0);
    }
}
