//! A per-channel arpeggiator (like FL's channel ARP): keys held on the channel are played
//! one at a time, in order, at a tempo-synced rate, across one or more octaves.
//!
//! Real-time safe: fixed-size state, no allocation.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ArpMode {
    #[default]
    Up,
    Down,
    UpDown,
    Random,
}

impl ArpMode {
    pub const ALL: [ArpMode; 4] = [ArpMode::Up, ArpMode::Down, ArpMode::UpDown, ArpMode::Random];
    pub fn label(self) -> &'static str {
        match self {
            ArpMode::Up => "Up",
            ArpMode::Down => "Down",
            ArpMode::UpDown => "Up & down",
            ArpMode::Random => "Random",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArpSettings {
    pub on: bool,
    pub mode: ArpMode,
    /// Step length in beats (0.25 = 1/16 note).
    pub rate: f32,
    /// Note length as a share of the step (0.05..=1).
    pub gate: f32,
    /// Octaves the pattern spans (1..=4).
    pub octaves: u8,
}

impl Default for ArpSettings {
    fn default() -> Self {
        ArpSettings { on: false, mode: ArpMode::Up, rate: 0.25, gate: 0.5, octaves: 1 }
    }
}

/// Rates offered in the UI: (label, beats).
pub const RATES: [(&str, f32); 7] = [("1/4", 1.0), ("1/8", 0.5), ("1/8 triplet", 1.0 / 3.0), ("1/16", 0.25), ("1/16 triplet", 1.0 / 6.0), ("1/32", 0.125), ("1/2", 2.0)];

/// The arpeggiator's running state.
#[derive(Clone)]
pub struct Arp {
    pub settings: ArpSettings,
    /// Velocity of each held key (0 = not held).
    held: [u8; 128],
    held_count: u8,
    /// Samples until the next step.
    until_step: f64,
    /// Position in the pattern.
    index: usize,
    /// The note sounding now and samples until its release.
    sounding: Option<(u8, f64)>,
    rng: u32,
}

impl Default for Arp {
    fn default() -> Self {
        Arp { settings: ArpSettings::default(), held: [0; 128], held_count: 0, until_step: 0.0, index: 0, sounding: None, rng: 0x2545_F491 }
    }
}

/// What the arpeggiator wants played: (sample offset in the block, key, velocity; 0 = off).
pub type ArpEvent = (usize, u8, u8);

impl Arp {
    /// A key went down (velocity > 0) or up.
    pub fn key(&mut self, key: u8, velocity: u8) {
        let k = (key & 0x7F) as usize;
        let was = self.held[k] > 0;
        if velocity > 0 {
            if !was {
                self.held_count += 1;
            }
            self.held[k] = velocity;
            if self.held_count == 1 && !was {
                // First key: start right away, from the beginning of the pattern.
                self.until_step = 0.0;
                self.index = 0;
            }
        } else if was {
            self.held[k] = 0;
            self.held_count -= 1;
        }
    }

    /// Forgets held keys (transport stop/seek). Returns the note to release, if any.
    pub fn reset(&mut self) -> Option<u8> {
        self.held = [0; 128];
        self.held_count = 0;
        self.sounding.take().map(|(k, _)| k)
    }

    /// The `n`th note of the pattern (held keys sorted, across octaves), and pattern length.
    fn note_at(&mut self, n: usize) -> Option<(u8, u8, usize)> {
        let keys = self.held.iter().enumerate().filter(|(_, v)| **v > 0).map(|(k, v)| (k as u8, *v));
        let base: usize = self.held_count as usize;
        if base == 0 {
            return None;
        }
        let octaves = self.settings.octaves.clamp(1, 4) as usize;
        let len = base * octaves;
        let pick = |i: usize| -> usize { i % len };
        let idx = match self.settings.mode {
            ArpMode::Up => pick(n),
            ArpMode::Down => len - 1 - pick(n),
            ArpMode::UpDown => {
                let cycle = (len * 2).saturating_sub(2).max(1);
                let p = n % cycle;
                if p < len { p } else { cycle - p }
            }
            ArpMode::Random => {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 17;
                self.rng ^= self.rng << 5;
                self.rng as usize % len
            }
        };
        let (octave, which) = (idx / base, idx % base);
        let (key, vel) = keys.clone().nth(which)?;
        let key = key as usize + octave * 12;
        let cycle_len = if self.settings.mode == ArpMode::UpDown { (len * 2).saturating_sub(2).max(1) } else { len };
        (key <= 127).then_some((key as u8, vel, cycle_len))
    }

    /// Advances `len` samples, writing note events into `out` (returns how many).
    pub fn run(&mut self, len: usize, samples_per_beat: f64, out: &mut [ArpEvent]) -> usize {
        let mut count = 0;
        let step = (self.settings.rate.max(1.0 / 64.0) as f64 * samples_per_beat).max(16.0);
        let gate = step * self.settings.gate.clamp(0.05, 1.0) as f64;
        let mut t = 0.0f64;
        while t < len as f64 && count + 2 <= out.len() {
            // Next thing to happen: the current note's release or the next step.
            let release_at = self.sounding.map_or(f64::INFINITY, |(_, r)| r);
            let next = release_at.min(self.until_step);
            if t + next >= len as f64 {
                let dt = len as f64 - t;
                if let Some((_, r)) = &mut self.sounding {
                    *r -= dt;
                }
                self.until_step -= dt;
                break;
            }
            t += next;
            if let Some((_, r)) = &mut self.sounding {
                *r -= next;
            }
            self.until_step -= next;
            if let Some((k, r)) = self.sounding
                && r <= 1e-9
            {
                out[count] = (t as usize, k, 0);
                count += 1;
                self.sounding = None;
            }
            if self.until_step <= 1e-9 {
                self.until_step += step;
                if let Some((key, vel, cycle)) = self.note_at(self.index) {
                    if let Some((k, _)) = self.sounding.take() {
                        out[count] = (t as usize, k, 0);
                        count += 1;
                    }
                    out[count] = (t as usize, key, vel);
                    count += 1;
                    self.sounding = Some((key, gate));
                    self.index = (self.index + 1) % cycle.max(1);
                } else {
                    self.index = 0;
                }
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(arp: &mut Arp, blocks: usize, spb: f64) -> Vec<(usize, u8, u8)> {
        let mut all = Vec::new();
        let mut buf = [(0, 0, 0); 64];
        for b in 0..blocks {
            let n = arp.run(512, spb, &mut buf);
            all.extend(buf[..n].iter().map(|&(o, k, v)| (b * 512 + o, k, v)));
        }
        all
    }

    #[test]
    fn plays_held_keys_in_order_on_the_grid() {
        let mut arp = Arp { settings: ArpSettings { on: true, rate: 0.25, gate: 0.5, ..Default::default() }, ..Default::default() };
        for k in [64, 60, 67] {
            arp.key(k, 100);
        }
        // 1/16 at 2048 samples per beat = 512 samples per step.
        let ons: Vec<(usize, u8)> = collect(&mut arp, 8, 2048.0).into_iter().filter(|e| e.2 > 0).map(|e| (e.0, e.1)).collect();
        assert_eq!(ons[..6], [(0, 60), (512, 64), (1024, 67), (1536, 60), (2048, 64), (2560, 67)]);
        let offs = collect(&mut arp, 1, 2048.0).into_iter().filter(|e| e.2 == 0).count();
        assert!(offs >= 1, "gated notes are released");
    }

    #[test]
    fn modes_and_octaves() {
        let mut arp = Arp { settings: ArpSettings { on: true, mode: ArpMode::Down, octaves: 2, ..Default::default() }, ..Default::default() };
        arp.key(60, 90);
        arp.key(62, 90);
        let keys: Vec<u8> = collect(&mut arp, 8, 2048.0).into_iter().filter(|e| e.2 > 0).map(|e| e.1).take(4).collect();
        assert_eq!(keys, [74, 72, 62, 60]);

        let mut arp = Arp { settings: ArpSettings { on: true, mode: ArpMode::UpDown, ..Default::default() }, ..Default::default() };
        for k in [60, 64, 67] {
            arp.key(k, 90);
        }
        let keys: Vec<u8> = collect(&mut arp, 8, 2048.0).into_iter().filter(|e| e.2 > 0).map(|e| e.1).take(6).collect();
        assert_eq!(keys, [60, 64, 67, 64, 60, 64]);
    }

    #[test]
    fn releasing_all_keys_stops_and_reset_releases() {
        let mut arp = Arp { settings: ArpSettings { on: true, gate: 1.0, ..Default::default() }, ..Default::default() };
        arp.key(60, 100);
        collect(&mut arp, 1, 2048.0);
        arp.key(60, 0);
        let later: Vec<_> = collect(&mut arp, 4, 2048.0).into_iter().filter(|e| e.2 > 0).collect();
        assert!(later.is_empty());
        arp.key(60, 100);
        collect(&mut arp, 1, 2048.0);
        assert_eq!(arp.reset(), Some(60));
    }
}
