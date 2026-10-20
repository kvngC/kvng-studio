//! KVNG Studio's built-in instruments: a polyphonic subtractive synth and
//! synthesized drum sounds. These give new projects sound without any plugins.

use serde::{Deserialize, Serialize};

use crate::sample::Sample;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wave {
    Sine,
    Triangle,
    Saw,
    Square,
    Noise,
}

impl Wave {
    pub const ALL: [Wave; 5] = [Wave::Sine, Wave::Triangle, Wave::Saw, Wave::Square, Wave::Noise];

    pub fn label(self) -> &'static str {
        match self {
            Wave::Sine => "Sine",
            Wave::Triangle => "Triangle",
            Wave::Saw => "Saw",
            Wave::Square => "Square",
            Wave::Noise => "Noise",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Oscillator {
    pub wave: Wave,
    /// 0.0..=1.0
    pub level: f32,
    /// Semitones.
    pub coarse: i32,
    /// Cents.
    pub fine: f32,
}

/// Settings of the KVNG Synth (saved in projects).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SynthParams {
    pub osc: [Oscillator; 3],
    /// Low-pass cutoff in Hz.
    pub cutoff: f32,
    /// 0.0..=1.0
    pub resonance: f32,
    /// How much the envelope opens the filter, in octaves.
    pub filter_env: f32,
    /// Amplitude envelope, seconds / level.
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Portamento time in seconds (0 = off).
    pub glide: f32,
    /// 0.0..=1.0
    pub volume: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            osc: [
                Oscillator { wave: Wave::Saw, level: 0.8, coarse: 0, fine: 0.0 },
                Oscillator { wave: Wave::Saw, level: 0.6, coarse: 0, fine: 7.0 },
                Oscillator { wave: Wave::Square, level: 0.4, coarse: -12, fine: 0.0 },
            ],
            cutoff: 2400.0,
            resonance: 0.25,
            filter_env: 1.5,
            attack: 0.005,
            decay: 0.3,
            sustain: 0.7,
            release: 0.25,
            glide: 0.0,
            volume: 0.7,
        }
    }
}

impl SynthParams {
    /// Factory presets: (name, settings).
    pub fn presets() -> Vec<(&'static str, SynthParams)> {
        let osc = |wave, level, coarse, fine| Oscillator { wave, level, coarse, fine };
        let off = osc(Wave::Sine, 0.0, 0, 0.0);
        vec![
            ("Init (Saw stack)", SynthParams::default()),
            ("808 Bass", SynthParams {
                osc: [osc(Wave::Sine, 1.0, -12, 0.0), osc(Wave::Triangle, 0.25, -12, 0.0), off],
                cutoff: 900.0, resonance: 0.0, filter_env: 0.5,
                attack: 0.002, decay: 1.2, sustain: 0.0, release: 0.4, glide: 0.06, volume: 0.9,
            }),
            ("Sub Bass", SynthParams {
                osc: [osc(Wave::Sine, 1.0, -12, 0.0), off, off],
                cutoff: 400.0, resonance: 0.0, filter_env: 0.0,
                attack: 0.005, decay: 0.2, sustain: 1.0, release: 0.12, glide: 0.0, volume: 0.9,
            }),
            ("Lead", SynthParams {
                osc: [osc(Wave::Saw, 0.8, 0, -8.0), osc(Wave::Square, 0.6, 0, 8.0), osc(Wave::Saw, 0.3, 12, 0.0)],
                cutoff: 3500.0, resonance: 0.35, filter_env: 1.0,
                attack: 0.004, decay: 0.25, sustain: 0.8, release: 0.2, glide: 0.04, volume: 0.6,
            }),
            ("Pluck", SynthParams {
                osc: [osc(Wave::Saw, 0.8, 0, 0.0), osc(Wave::Square, 0.4, 12, 0.0), off],
                cutoff: 600.0, resonance: 0.3, filter_env: 3.5,
                attack: 0.001, decay: 0.25, sustain: 0.0, release: 0.2, glide: 0.0, volume: 0.7,
            }),
            ("Pad", SynthParams {
                osc: [osc(Wave::Saw, 0.6, 0, -12.0), osc(Wave::Saw, 0.6, 0, 12.0), osc(Wave::Triangle, 0.5, -12, 0.0)],
                cutoff: 1800.0, resonance: 0.15, filter_env: 0.8,
                attack: 0.8, decay: 1.0, sustain: 0.8, release: 1.6, glide: 0.0, volume: 0.55,
            }),
            ("Keys", SynthParams {
                osc: [osc(Wave::Triangle, 0.9, 0, 0.0), osc(Wave::Sine, 0.4, 12, 3.0), osc(Wave::Sine, 0.15, 24, 0.0)],
                cutoff: 4000.0, resonance: 0.05, filter_env: 1.0,
                attack: 0.003, decay: 1.4, sustain: 0.2, release: 0.35, glide: 0.0, volume: 0.65,
            }),
            ("Brass", SynthParams {
                osc: [osc(Wave::Saw, 0.8, 0, 0.0), osc(Wave::Saw, 0.7, 0, 6.0), off],
                cutoff: 700.0, resonance: 0.2, filter_env: 2.5,
                attack: 0.06, decay: 0.4, sustain: 0.7, release: 0.25, glide: 0.0, volume: 0.6,
            }),
        ]
    }
}

const MAX_SYNTH_VOICES: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Done,
}

struct SynthVoice {
    key: u8,
    /// Per-note pan (-1..=1) and filter offsets from the note's Mod X/Y.
    pan: f32,
    cutoff_oct: f32,
    reso_add: f32,
    velocity: f32,
    freq: f32,
    target_freq: f32,
    /// Glide coefficient for a slide note (overrides the patch's glide while set).
    slide: Option<f32>,
    phase: [f32; 3],
    stage: Stage,
    env: f32,
    /// State-variable filter memory.
    low: f32,
    band: f32,
    /// Samples to wait before starting (sub-block timing).
    delay: usize,
    age: u64,
}

/// A polyphonic three-oscillator synth with a resonant low-pass filter and ADSR.
pub struct Synth {
    pub params: SynthParams,
    sample_rate: f32,
    voices: Vec<SynthVoice>,
    noise: u32,
    clock: u64,
    last_freq: f32,
}

fn key_freq(key: u8) -> f32 {
    440.0 * 2f32.powf((key as f32 - 69.0) / 12.0)
}

impl Synth {
    pub fn new(params: SynthParams, sample_rate: f64) -> Self {
        Self { params, sample_rate: sample_rate as f32, voices: Vec::with_capacity(MAX_SYNTH_VOICES * 2), noise: 0x1234_5678, clock: 0, last_freq: 0.0 }
    }

    pub fn note_on(&mut self, key: u8, velocity: u8, offset: usize) {
        self.note_on_expr(key, velocity, offset, 0.0, 0.0, 0.0);
    }

    /// Starts a note with per-note pan and Mod X (±2 octaves of cutoff) / Mod Y (resonance).
    pub fn note_on_expr(&mut self, key: u8, velocity: u8, offset: usize, pan: f32, mod_x: f32, mod_y: f32) {
        let target = key_freq(key);
        let start = if self.params.glide > 0.0 && self.last_freq > 0.0 { self.last_freq } else { target };
        self.last_freq = target;
        if self.voices.len() >= MAX_SYNTH_VOICES {
            // Steal the oldest voice.
            if let Some(i) = self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i) {
                self.voices.swap_remove(i);
            }
        }
        self.clock += 1;
        self.voices.push(SynthVoice {
            key,
            pan: pan.clamp(-1.0, 1.0),
            cutoff_oct: mod_x.clamp(-1.0, 1.0) * 2.0,
            reso_add: mod_y.clamp(-1.0, 1.0) * 0.5,
            velocity: (velocity as f32 / 127.0).clamp(0.0, 1.0),
            freq: start,
            target_freq: target,
            slide: None,
            phase: [0.0; 3],
            stage: Stage::Attack,
            env: 0.0,
            low: 0.0,
            band: 0.0,
            delay: offset,
            age: self.clock,
        });
    }

    /// Glides every sounding voice to `key` over about `duration` samples (slide notes).
    pub fn slide_to(&mut self, key: u8, duration: u32) {
        let coeff = (-1.0 / (duration.max(64) as f32 / 4.0)).exp();
        let target = key_freq(key);
        for v in self.voices.iter_mut().filter(|v| !matches!(v.stage, Stage::Release | Stage::Done)) {
            v.target_freq = target;
            v.slide = Some(coeff);
        }
        self.last_freq = target;
    }

    pub fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.stage != Stage::Release) {
            v.stage = Stage::Release;
        }
    }

    pub fn all_notes_off(&mut self) {
        self.voices.clear();
    }

    /// Adds the synth's output into `left`/`right`.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let sr = self.sample_rate;
        let p = &self.params;
        let rate = |secs: f32| 1.0 / (secs.max(0.001) * sr);
        let (a_rate, d_rate, r_rate) = (rate(p.attack), rate(p.decay), rate(p.release));
        let glide_coeff = if p.glide > 0.0 { (-1.0 / (p.glide * sr)).exp() } else { 0.0 };
        let base_reso = p.resonance;
        let osc_ratio: [f32; 3] = std::array::from_fn(|i| 2f32.powf((p.osc[i].coarse as f32 + p.osc[i].fine / 100.0) / 12.0));
        let gain = p.volume * 0.25;
        let noise = &mut self.noise;

        for v in self.voices.iter_mut() {
            let q = 1.0 - (base_reso + v.reso_add).clamp(0.0, 0.97);
            let (pan_l, pan_r) = ((1.0 - v.pan).min(1.0), (1.0 + v.pan).min(1.0));
            let start = v.delay.min(left.len());
            v.delay -= start;
            for i in start..left.len() {
                match v.stage {
                    Stage::Attack => {
                        v.env += a_rate;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        v.env -= d_rate * (1.0 - p.sustain).max(0.001);
                        if v.env <= p.sustain {
                            v.env = p.sustain;
                            v.stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => v.env = p.sustain,
                    Stage::Release => {
                        v.env -= r_rate * v.env.max(0.001) * 4.0 + r_rate * 0.01;
                        if v.env <= 0.0005 {
                            v.env = 0.0;
                            v.stage = Stage::Done;
                        }
                    }
                    Stage::Done => break,
                }
                v.freq = v.target_freq + (v.freq - v.target_freq) * v.slide.unwrap_or(glide_coeff);

                let mut s = 0.0;
                for (o, osc) in p.osc.iter().enumerate() {
                    if osc.level <= 0.0 {
                        continue;
                    }
                    let ph = v.phase[o];
                    let x = match osc.wave {
                        Wave::Sine => (ph * std::f32::consts::TAU).sin(),
                        Wave::Triangle => 4.0 * (ph - (ph + 0.5).floor()).abs() - 1.0,
                        Wave::Saw => 2.0 * ph - 1.0,
                        Wave::Square => if ph < 0.5 { 1.0 } else { -1.0 },
                        Wave::Noise => {
                            *noise ^= *noise << 13;
                            *noise ^= *noise >> 17;
                            *noise ^= *noise << 5;
                            (*noise as f32 / u32::MAX as f32) * 2.0 - 1.0
                        }
                    };
                    s += x * osc.level;
                    v.phase[o] = (ph + v.freq * osc_ratio[o] / sr).fract();
                }

                // Chamberlin state-variable low-pass, opened by the envelope.
                let cutoff = (p.cutoff * 2f32.powf(p.filter_env * v.env + v.cutoff_oct)).clamp(20.0, sr * 0.45);
                let f = 2.0 * (std::f32::consts::PI * cutoff / sr).sin();
                let high = s - v.low - q * v.band;
                v.band += f * high;
                v.low += f * v.band;
                let out = v.low * v.env * v.velocity * gain;
                left[i] += out * pan_l;
                right[i] += out * pan_r;
            }
        }
        self.voices.retain(|v| v.stage != Stage::Done);
    }
}

/// Per-channel sampler settings (FL's channel settings: pitch, envelope, cut itself...).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplerSettings {
    /// Semitones.
    pub pitch: f32,
    pub reverse: bool,
    /// A new note stops the previous one (808s, open/closed hats).
    pub cut_itself: bool,
    /// Stop at the end of the note instead of playing the whole sample.
    pub follow_note: bool,
    /// Apply the ADSR envelope below.
    pub envelope: bool,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Channel filter: 0 = off, 1 = low-pass, 2 = high-pass, 3 = band-pass.
    pub filter: u8,
    /// Plugin channels: the plugin's pitch-bend range in semitones (slide notes use it).
    pub bend_range: f32,
    /// Hz
    pub cutoff: f32,
    /// 0..1
    pub resonance: f32,
}

impl Default for SamplerSettings {
    fn default() -> Self {
        Self { pitch: 0.0, reverse: false, cut_itself: false, follow_note: false, envelope: false, attack: 0.002, decay: 0.3, sustain: 1.0, release: 0.08, filter: 0, bend_range: 2.0, cutoff: 2000.0, resonance: 0.2 }
    }
}

/// The channel filter's running state (TPT state-variable filter, per side).
#[derive(Clone, Copy, Default)]
pub struct ChannelFilter {
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl ChannelFilter {
    /// Filters a block in place with `settings` (does nothing when the filter is off).
    pub fn process(&mut self, settings: &SamplerSettings, left: &mut [f32], right: &mut [f32], sample_rate: f32) {
        if settings.filter == 0 {
            self.ic1 = [0.0; 2];
            self.ic2 = [0.0; 2];
            return;
        }
        let g = (std::f32::consts::PI * settings.cutoff.clamp(20.0, sample_rate * 0.45) / sample_rate).tan();
        let k = 2.0 - 2.0 * settings.resonance.clamp(0.0, 1.0) * 0.97;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let (a2, a3) = (g * a1, g * g * a1);
        for (c, buf) in [left, right].into_iter().enumerate() {
            for x in buf.iter_mut() {
                let v3 = *x - self.ic2[c];
                let v1 = a1 * self.ic1[c] + a2 * v3;
                let v2 = self.ic2[c] + a2 * self.ic1[c] + a3 * v3;
                self.ic1[c] = 2.0 * v1 - self.ic1[c];
                self.ic2[c] = 2.0 * v2 - self.ic2[c];
                *x = match settings.filter {
                    2 => *x - k * v1 - v2,
                    3 => v1,
                    _ => v2,
                };
            }
            if !(self.ic1[c].is_finite() && self.ic2[c].is_finite()) {
                self.ic1[c] = 0.0;
                self.ic2[c] = 0.0;
            }
        }
    }
}

/// Built-in drum sounds, synthesized when a project loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drum {
    Kick,
    Clap,
    Hat,
    Snare,
}

impl Drum {
    pub const ALL: [Drum; 4] = [Drum::Kick, Drum::Clap, Drum::Hat, Drum::Snare];

    pub fn label(self) -> &'static str {
        match self {
            Drum::Kick => "Kick",
            Drum::Clap => "Clap",
            Drum::Hat => "Hat",
            Drum::Snare => "Snare",
        }
    }

    /// Renders the drum at `sample_rate`.
    pub fn render(self, sample_rate: f64) -> Sample {
        let sr = sample_rate as f32;
        let mut noise = 0x9E37_79B9u32;
        let mut rnd = move || {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            (noise as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let (secs, mut voice): (f32, Box<dyn FnMut(f32) -> f32>) = match self {
            Drum::Kick => {
                let mut phase = 0.0f32;
                (0.55, Box::new(move |t: f32| {
                    let f = 45.0 + 140.0 * (-t * 28.0).exp();
                    phase += f / sr;
                    let body = (phase * std::f32::consts::TAU).sin() * (-t * 7.0).exp();
                    let click = if t < 0.004 { (1.0 - t / 0.004) * 0.6 } else { 0.0 };
                    (body + click).tanh() * 0.95
                }))
            }
            Drum::Snare => {
                let mut phase = 0.0f32;
                let mut lp = 0.0f32;
                (0.3, Box::new(move |t: f32| {
                    phase += 190.0 / sr;
                    let tone = (phase * std::f32::consts::TAU).sin() * (-t * 30.0).exp() * 0.5;
                    let n = rnd();
                    lp += (n - lp) * 0.55;
                    let rattle = (n - lp * 0.4) * (-t * 14.0).exp() * 0.7;
                    (tone + rattle).clamp(-1.0, 1.0)
                }))
            }
            Drum::Clap => {
                let mut bp = (0.0f32, 0.0f32);
                (0.35, Box::new(move |t: f32| {
                    // Three quick bursts then a short tail, band-passed around 1.2 kHz.
                    let bursts = [0.0f32, 0.011, 0.022];
                    let mut env = bursts.iter().map(|b| if t >= *b { (-(t - b) * 160.0).exp() } else { 0.0 }).fold(0.0, f32::max);
                    if t >= 0.03 {
                        env = env.max(0.55 * (-(t - 0.03) * 18.0).exp());
                    }
                    let f = 2.0 * (std::f32::consts::PI * 1200.0 / sr).sin();
                    let n = rnd();
                    let high = n - bp.0 - 0.6 * bp.1;
                    bp.1 += f * high;
                    bp.0 += f * bp.1;
                    (bp.1 * env * 2.2).clamp(-1.0, 1.0)
                }))
            }
            Drum::Hat => {
                let mut hp_prev = 0.0f32;
                let mut hp = 0.0f32;
                (0.12, Box::new(move |t: f32| {
                    let n = rnd();
                    hp = 0.92 * (hp + n - hp_prev);
                    hp_prev = n;
                    hp * (-t * 45.0).exp() * 0.6
                }))
            }
        };
        let n = (secs * sr) as usize;
        let mono: Vec<f32> = (0..n).map(|i| voice(i as f32 / sr)).collect();
        Sample { left: mono.clone(), right: mono }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_plays_and_releases() {
        let mut s = Synth::new(SynthParams::default(), 48_000.0);
        s.note_on(60, 100, 0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        s.render(&mut l, &mut r);
        assert!(l.iter().any(|x| x.abs() > 0.01), "note is audible");
        s.note_off(60);
        for _ in 0..40 {
            l.fill(0.0);
            r.fill(0.0);
            s.render(&mut l, &mut r);
        }
        assert!(s.voices.is_empty(), "voice ends after release");
    }

    #[test]
    fn synth_slide_reaches_the_new_pitch() {
        let mut synth = Synth::new(SynthParams::default(), 48_000.0);
        synth.note_on_expr(57, 100, 0, 0.0, 0.0, 0.0); // A4 = 220 Hz
        synth.slide_to(69, 2400); // A5
        let (mut l, mut r) = (vec![0.0f32; 24_000], vec![0.0f32; 24_000]);
        synth.render(&mut l, &mut r);
        assert!(synth.voices.iter().all(|v| (v.freq - key_freq(69)).abs() < 1.0), "voices glided to the slide's pitch");
    }

    #[test]
    fn channel_filter_low_pass_removes_highs() {
        let st = SamplerSettings { filter: 1, cutoff: 300.0, resonance: 0.0, ..Default::default() };
        let mut f = ChannelFilter::default();
        let mut l: Vec<f32> = (0..48_000).map(|i| (std::f32::consts::TAU * 6000.0 * i as f32 / 48_000.0).sin()).collect();
        let mut r = l.clone();
        f.process(&st, &mut l, &mut r, 48_000.0);
        let rms = (l[2000..].iter().map(|v| v * v).sum::<f32>() / (l.len() - 2000) as f32).sqrt();
        assert!(rms < 0.01, "rms {rms}");
        let off = SamplerSettings::default();
        let mut l2 = vec![0.5f32; 64];
        let mut r2 = l2.clone();
        f.process(&off, &mut l2, &mut r2, 48_000.0);
        assert!(l2.iter().all(|v| *v == 0.5), "off is pass-through");
    }

    #[test]
    fn every_preset_plays() {
        for (name, p) in SynthParams::presets() {
            let mut s = Synth::new(p, 48_000.0);
            s.note_on(48, 100, 0);
            let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
            s.render(&mut l, &mut r);
            let peak = l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.005 && peak.is_finite() && peak < 4.0, "{name}: peak {peak}");
        }
    }

    #[test]
    fn drums_render_within_range() {
        for d in Drum::ALL {
            let smp = d.render(48_000.0);
            let peak = smp.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.1 && peak <= 1.0, "{d:?} peak {peak}");
        }
    }
}
