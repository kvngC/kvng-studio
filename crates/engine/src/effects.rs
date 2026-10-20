//! KVNG Studio's built-in mixer effects: EQ, reverb, delay, compressor, chorus, flanger,
//! phaser, distortion, filter and gate.
//!
//! Each effect's continuous controls are described by [`ParamSpec`]s so the UI, automation
//! and MIDI links can treat them generically (normalised 0..1 values).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuiltinEffect {
    Eq(EqParams),
    Reverb(ReverbParams),
    Delay(DelayParams),
    Compressor(CompressorParams),
    Chorus(ModDelayParams),
    Flanger(ModDelayParams),
    Phaser(PhaserParams),
    Distortion(DistortionParams),
    Filter(FilterParams),
    Gate(GateParams),
}

/// A continuous effect control.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub lo: f32,
    pub hi: f32,
    pub default: f32,
    /// Logarithmic knob travel (frequencies, times).
    pub log: bool,
    pub unit: &'static str,
}

const fn spec(name: &'static str, lo: f32, hi: f32, default: f32, log: bool, unit: &'static str) -> ParamSpec {
    ParamSpec { name, lo, hi, default, log, unit }
}

impl ParamSpec {
    pub fn to_normalized(&self, v: f32) -> f32 {
        if self.log {
            ((v.max(self.lo) / self.lo).ln() / (self.hi / self.lo).ln()).clamp(0.0, 1.0)
        } else {
            ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
        }
    }

    pub fn from_normalized(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.log { self.lo * (self.hi / self.lo).powf(t) } else { self.lo + t * (self.hi - self.lo) }
    }

    /// The value with its unit, for knob tooltips.
    pub fn format(&self, v: f32) -> String {
        match self.unit {
            "Hz" if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
            "Hz" if v < 10.0 => format!("{v:.2} Hz"),
            "Hz" => format!("{v:.0} Hz"),
            "%" => format!("{:.0}%", v * 100.0),
            ":1" => format!("{v:.1}:1"),
            "" => format!("{v:.2}"),
            u => format!("{v:.1} {u}"),
        }
    }
}

const EQ_SPECS: [ParamSpec; 8] = [
    spec("Low cut", 20.0, 2000.0, 20.0, true, "Hz"),
    spec("Low", -18.0, 18.0, 0.0, false, "dB"),
    spec("Low freq", 30.0, 600.0, 120.0, true, "Hz"),
    spec("Mid", -18.0, 18.0, 0.0, false, "dB"),
    spec("Mid freq", 200.0, 8000.0, 1200.0, true, "Hz"),
    spec("High", -18.0, 18.0, 0.0, false, "dB"),
    spec("High freq", 2000.0, 16000.0, 8000.0, true, "Hz"),
    spec("High cut", 1000.0, 20000.0, 20000.0, true, "Hz"),
];
const REVERB_SPECS: [ParamSpec; 5] = [
    spec("Size", 0.0, 1.0, 0.6, false, "%"),
    spec("Damping", 0.0, 1.0, 0.4, false, "%"),
    spec("Width", 0.0, 1.0, 1.0, false, "%"),
    spec("Pre-delay", 0.0, 150.0, 10.0, false, "ms"),
    spec("Mix", 0.0, 1.0, 0.25, false, "%"),
];
const DELAY_SPECS: [ParamSpec; 4] = [
    spec("Time", 10.0, 2000.0, 375.0, true, "ms"),
    spec("Feedback", 0.0, 0.95, 0.4, false, "%"),
    spec("Tone", 500.0, 20000.0, 6000.0, true, "Hz"),
    spec("Mix", 0.0, 1.0, 0.3, false, "%"),
];
const COMPRESSOR_SPECS: [ParamSpec; 6] = [
    spec("Threshold", -60.0, 0.0, -18.0, false, "dB"),
    spec("Ratio", 1.0, 20.0, 4.0, true, ":1"),
    spec("Attack", 0.1, 200.0, 10.0, true, "ms"),
    spec("Release", 5.0, 2000.0, 120.0, true, "ms"),
    spec("Knee", 0.0, 24.0, 6.0, false, "dB"),
    spec("Makeup", 0.0, 24.0, 0.0, false, "dB"),
];
const MOD_DELAY_SPECS: [ParamSpec; 6] = [
    spec("Rate", 0.02, 10.0, 0.8, true, "Hz"),
    spec("Depth", 0.0, 10.0, 3.0, false, "ms"),
    spec("Delay", 0.5, 30.0, 12.0, true, "ms"),
    spec("Feedback", -0.95, 0.95, 0.0, false, "%"),
    spec("Spread", 0.0, 1.0, 0.5, false, "%"),
    spec("Mix", 0.0, 1.0, 0.5, false, "%"),
];
const FLANGER_SPECS: [ParamSpec; 6] = [
    spec("Rate", 0.02, 10.0, 0.2, true, "Hz"),
    spec("Depth", 0.0, 10.0, 2.0, false, "ms"),
    spec("Delay", 0.5, 30.0, 1.5, true, "ms"),
    spec("Feedback", -0.95, 0.95, 0.6, false, "%"),
    spec("Spread", 0.0, 1.0, 0.25, false, "%"),
    spec("Mix", 0.0, 1.0, 0.5, false, "%"),
];
const PHASER_SPECS: [ParamSpec; 5] = [
    spec("Rate", 0.02, 10.0, 0.4, true, "Hz"),
    spec("Depth", 0.0, 1.0, 0.7, false, "%"),
    spec("Center", 150.0, 5000.0, 900.0, true, "Hz"),
    spec("Feedback", 0.0, 0.95, 0.5, false, "%"),
    spec("Mix", 0.0, 1.0, 0.5, false, "%"),
];
const DISTORTION_SPECS: [ParamSpec; 4] = [
    spec("Drive", 0.0, 48.0, 12.0, false, "dB"),
    spec("Tone", 500.0, 20000.0, 12000.0, true, "Hz"),
    spec("Output", -24.0, 12.0, -6.0, false, "dB"),
    spec("Mix", 0.0, 1.0, 1.0, false, "%"),
];
const FILTER_SPECS: [ParamSpec; 5] = [
    spec("Cutoff", 20.0, 20000.0, 2000.0, true, "Hz"),
    spec("Resonance", 0.0, 1.0, 0.3, false, "%"),
    spec("LFO rate", 0.02, 10.0, 1.0, true, "Hz"),
    spec("LFO depth", 0.0, 4.0, 0.0, false, "oct"),
    spec("Drive", 0.0, 24.0, 0.0, false, "dB"),
];
const GATE_SPECS: [ParamSpec; 5] = [
    spec("Threshold", -80.0, 0.0, -40.0, false, "dB"),
    spec("Attack", 0.05, 50.0, 1.0, true, "ms"),
    spec("Hold", 0.0, 500.0, 20.0, false, "ms"),
    spec("Release", 5.0, 2000.0, 100.0, true, "ms"),
    spec("Range", -80.0, 0.0, -80.0, false, "dB"),
];

/// Runs `$body` with `$it` bound to an iterator over the effect's control values, in
/// [`BuiltinEffect::specs`] order.
macro_rules! with_fields {
    ($e:expr, |$it:ident| $body:expr) => {
        match $e {
            BuiltinEffect::Eq(p) => {
                let $it = [&mut p.low_cut, &mut p.low_gain, &mut p.low_freq, &mut p.mid_gain, &mut p.mid_freq, &mut p.high_gain, &mut p.high_freq, &mut p.high_cut].into_iter();
                $body
            }
            BuiltinEffect::Reverb(p) => {
                let $it = [&mut p.size, &mut p.damping, &mut p.width, &mut p.predelay, &mut p.mix].into_iter();
                $body
            }
            BuiltinEffect::Delay(p) => {
                let $it = [&mut p.ms, &mut p.feedback, &mut p.tone, &mut p.mix].into_iter();
                $body
            }
            BuiltinEffect::Compressor(p) => {
                let $it = [&mut p.threshold, &mut p.ratio, &mut p.attack, &mut p.release, &mut p.knee, &mut p.makeup].into_iter();
                $body
            }
            BuiltinEffect::Chorus(p) | BuiltinEffect::Flanger(p) => {
                let $it = [&mut p.rate, &mut p.depth, &mut p.delay, &mut p.feedback, &mut p.spread, &mut p.mix].into_iter();
                $body
            }
            BuiltinEffect::Phaser(p) => {
                let $it = [&mut p.rate, &mut p.depth, &mut p.center, &mut p.feedback, &mut p.mix].into_iter();
                $body
            }
            BuiltinEffect::Distortion(p) => {
                let $it = [&mut p.drive, &mut p.tone, &mut p.output, &mut p.mix].into_iter();
                $body
            }
            BuiltinEffect::Filter(p) => {
                let $it = [&mut p.cutoff, &mut p.resonance, &mut p.lfo_rate, &mut p.lfo_depth, &mut p.drive].into_iter();
                $body
            }
            BuiltinEffect::Gate(p) => {
                let $it = [&mut p.threshold, &mut p.attack, &mut p.hold, &mut p.release, &mut p.range].into_iter();
                $body
            }
        }
    };
}

impl BuiltinEffect {
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinEffect::Eq(_) => "KVNG EQ",
            BuiltinEffect::Reverb(_) => "KVNG Reverb",
            BuiltinEffect::Delay(_) => "KVNG Delay",
            BuiltinEffect::Compressor(_) => "KVNG Compressor",
            BuiltinEffect::Chorus(_) => "KVNG Chorus",
            BuiltinEffect::Flanger(_) => "KVNG Flanger",
            BuiltinEffect::Phaser(_) => "KVNG Phaser",
            BuiltinEffect::Distortion(_) => "KVNG Distortion",
            BuiltinEffect::Filter(_) => "KVNG Filter",
            BuiltinEffect::Gate(_) => "KVNG Gate",
        }
    }

    pub fn all() -> Vec<BuiltinEffect> {
        vec![
            BuiltinEffect::Eq(EqParams::default()),
            BuiltinEffect::Reverb(ReverbParams::default()),
            BuiltinEffect::Delay(DelayParams::default()),
            BuiltinEffect::Compressor(CompressorParams::default()),
            BuiltinEffect::Chorus(ModDelayParams::chorus()),
            BuiltinEffect::Flanger(ModDelayParams::flanger()),
            BuiltinEffect::Phaser(PhaserParams::default()),
            BuiltinEffect::Distortion(DistortionParams::default()),
            BuiltinEffect::Filter(FilterParams::default()),
            BuiltinEffect::Gate(GateParams::default()),
        ]
    }

    pub fn description(&self) -> &'static str {
        match self {
            BuiltinEffect::Eq(_) => "3-band EQ with low/high cut",
            BuiltinEffect::Reverb(_) => "Room/hall reverb",
            BuiltinEffect::Delay(_) => "Tempo-synced stereo delay",
            BuiltinEffect::Compressor(_) => "Compressor with makeup gain",
            BuiltinEffect::Chorus(_) => "Stereo chorus",
            BuiltinEffect::Flanger(_) => "Flanger with feedback",
            BuiltinEffect::Phaser(_) => "6-stage phaser",
            BuiltinEffect::Distortion(_) => "Saturation, clipping, folding, bitcrush",
            BuiltinEffect::Filter(_) => "Resonant filter with LFO",
            BuiltinEffect::Gate(_) => "Noise gate",
        }
    }

    /// The effect's continuous controls, in the order [`BuiltinEffect::values_mut`] returns them.
    pub fn specs(&self) -> &'static [ParamSpec] {
        match self {
            BuiltinEffect::Eq(_) => &EQ_SPECS,
            BuiltinEffect::Reverb(_) => &REVERB_SPECS,
            BuiltinEffect::Delay(_) => &DELAY_SPECS,
            BuiltinEffect::Compressor(_) => &COMPRESSOR_SPECS,
            BuiltinEffect::Chorus(_) => &MOD_DELAY_SPECS,
            BuiltinEffect::Flanger(_) => &FLANGER_SPECS,
            BuiltinEffect::Phaser(_) => &PHASER_SPECS,
            BuiltinEffect::Distortion(_) => &DISTORTION_SPECS,
            BuiltinEffect::Filter(_) => &FILTER_SPECS,
            BuiltinEffect::Gate(_) => &GATE_SPECS,
        }
    }

    pub fn values_mut(&mut self) -> Vec<&mut f32> {
        with_fields!(self, |it| it.collect())
    }

    /// One control's value (no allocation: safe on the audio thread).
    pub fn value_mut(&mut self, index: usize) -> Option<&mut f32> {
        with_fields!(self, |it| it.into_iter().nth(index))
    }

    /// Control `index` as 0..1 (0.5 if there is no such control).
    pub fn normalized(&self, index: usize) -> f32 {
        let mut copy = self.clone();
        match (self.specs().get(index), copy.value_mut(index)) {
            (Some(spec), Some(v)) => spec.to_normalized(*v),
            _ => 0.5,
        }
    }

    /// Sets control `index` from 0..1 (automation, MIDI links).
    pub fn set_normalized(&mut self, index: usize, t: f32) {
        let specs = self.specs();
        if let (Some(spec), Some(v)) = (specs.get(index), self.value_mut(index)) {
            *v = spec.from_normalized(t);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqParams {
    /// Shelf/peak gains in dB.
    pub low_gain: f32,
    pub mid_gain: f32,
    pub high_gain: f32,
    pub low_freq: f32,
    pub mid_freq: f32,
    pub high_freq: f32,
    /// High-pass and low-pass cutoffs in Hz (20 / 20000 = off).
    pub low_cut: f32,
    pub high_cut: f32,
}

impl Default for EqParams {
    fn default() -> Self {
        Self { low_gain: 0.0, mid_gain: 0.0, high_gain: 0.0, low_freq: 120.0, mid_freq: 1200.0, high_freq: 8000.0, low_cut: 20.0, high_cut: 20000.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    /// 0..1
    pub size: f32,
    pub damping: f32,
    pub width: f32,
    pub mix: f32,
    /// Pre-delay in ms.
    pub predelay: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self { size: 0.6, damping: 0.4, width: 1.0, mix: 0.25, predelay: 10.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DelayParams {
    /// Delay time in beats (e.g. 0.75 = dotted eighth) when synced, else ms.
    pub beats: f32,
    pub sync: bool,
    pub ms: f32,
    pub feedback: f32,
    pub mix: f32,
    pub ping_pong: bool,
    /// Low-pass on the repeats, Hz.
    pub tone: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self { beats: 0.75, sync: true, ms: 375.0, feedback: 0.4, mix: 0.3, ping_pong: true, tone: 6000.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorParams {
    /// dB
    pub threshold: f32,
    pub ratio: f32,
    /// ms
    pub attack: f32,
    pub release: f32,
    /// dB
    pub makeup: f32,
    /// Soft knee width, dB.
    pub knee: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self { threshold: -18.0, ratio: 4.0, attack: 10.0, release: 120.0, makeup: 0.0, knee: 6.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModDelayParams {
    /// LFO rate, Hz.
    pub rate: f32,
    /// Modulation depth and base delay, ms.
    pub depth: f32,
    pub delay: f32,
    /// −0.95..0.95
    pub feedback: f32,
    /// LFO phase offset between left and right (1 = opposite).
    pub spread: f32,
    pub mix: f32,
}

impl ModDelayParams {
    pub fn chorus() -> Self {
        Self { rate: 0.8, depth: 3.0, delay: 12.0, feedback: 0.0, spread: 0.5, mix: 0.5 }
    }
    pub fn flanger() -> Self {
        Self { rate: 0.2, depth: 2.0, delay: 1.5, feedback: 0.6, spread: 0.25, mix: 0.5 }
    }
}

impl Default for ModDelayParams {
    fn default() -> Self {
        Self::chorus()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhaserParams {
    pub rate: f32,
    /// Sweep width, 0..1 (1 = ±2 octaves around `center`).
    pub depth: f32,
    pub center: f32,
    pub feedback: f32,
    pub mix: f32,
}

impl Default for PhaserParams {
    fn default() -> Self {
        Self { rate: 0.4, depth: 0.7, center: 900.0, feedback: 0.5, mix: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DistortionMode {
    /// tanh saturation.
    #[default]
    Soft,
    /// Hard clipping.
    Hard,
    /// Wavefolding.
    Fold,
    /// Bit depth reduction (drive lowers the bit depth).
    Bitcrush,
}

impl DistortionMode {
    pub const ALL: [DistortionMode; 4] = [DistortionMode::Soft, DistortionMode::Hard, DistortionMode::Fold, DistortionMode::Bitcrush];
    pub fn label(self) -> &'static str {
        match self {
            DistortionMode::Soft => "Soft clip",
            DistortionMode::Hard => "Hard clip",
            DistortionMode::Fold => "Wavefold",
            DistortionMode::Bitcrush => "Bitcrush",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DistortionParams {
    pub mode: DistortionMode,
    /// dB of gain into the shaper.
    pub drive: f32,
    /// Low-pass after the shaper, Hz.
    pub tone: f32,
    /// Output level, dB.
    pub output: f32,
    pub mix: f32,
}

impl Default for DistortionParams {
    fn default() -> Self {
        Self { mode: DistortionMode::Soft, drive: 12.0, tone: 12000.0, output: -6.0, mix: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    #[default]
    LowPass,
    HighPass,
    BandPass,
    Notch,
}

impl FilterMode {
    pub const ALL: [FilterMode; 4] = [FilterMode::LowPass, FilterMode::HighPass, FilterMode::BandPass, FilterMode::Notch];
    pub fn label(self) -> &'static str {
        match self {
            FilterMode::LowPass => "Low-pass",
            FilterMode::HighPass => "High-pass",
            FilterMode::BandPass => "Band-pass",
            FilterMode::Notch => "Notch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterParams {
    pub mode: FilterMode,
    pub cutoff: f32,
    /// 0..1 (near 1 = self-oscillation edge).
    pub resonance: f32,
    pub lfo_rate: f32,
    /// Cutoff sweep, octaves (0 = LFO off).
    pub lfo_depth: f32,
    /// Input gain, dB (soft-saturated).
    pub drive: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self { mode: FilterMode::LowPass, cutoff: 2000.0, resonance: 0.3, lfo_rate: 1.0, lfo_depth: 0.0, drive: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateParams {
    /// Opens above this level, dBFS.
    pub threshold: f32,
    pub attack: f32,
    pub hold: f32,
    pub release: f32,
    /// Attenuation when closed, dB.
    pub range: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self { threshold: -40.0, attack: 1.0, hold: 20.0, release: 100.0, range: -80.0 }
    }
}

/// RBJ biquad.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

enum Shape {
    LowShelf(f32),
    HighShelf(f32),
    Peak(f32, f32),
    HighPass,
    LowPass,
}

impl Biquad {
    fn new(shape: Shape, freq: f32, sr: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * (freq.clamp(10.0, sr * 0.49) / sr);
        let (cw, sw) = (w.cos(), w.sin());
        let (b0, b1, b2, a0, a1, a2) = match shape {
            Shape::LowShelf(db) | Shape::HighShelf(db) => {
                let a = 10f32.powf(db / 40.0);
                let alpha = sw / 2.0 * 2f32.sqrt();
                let k = 2.0 * a.sqrt() * alpha;
                if matches!(shape, Shape::LowShelf(_)) {
                    (a * ((a + 1.0) - (a - 1.0) * cw + k), 2.0 * a * ((a - 1.0) - (a + 1.0) * cw), a * ((a + 1.0) - (a - 1.0) * cw - k), (a + 1.0) + (a - 1.0) * cw + k, -2.0 * ((a - 1.0) + (a + 1.0) * cw), (a + 1.0) + (a - 1.0) * cw - k)
                } else {
                    (a * ((a + 1.0) + (a - 1.0) * cw + k), -2.0 * a * ((a - 1.0) + (a + 1.0) * cw), a * ((a + 1.0) + (a - 1.0) * cw - k), (a + 1.0) - (a - 1.0) * cw + k, 2.0 * ((a - 1.0) - (a + 1.0) * cw), (a + 1.0) - (a - 1.0) * cw - k)
                }
            }
            Shape::Peak(db, q) => {
                let a = 10f32.powf(db / 40.0);
                let alpha = sw / (2.0 * q);
                (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a)
            }
            Shape::HighPass => {
                let alpha = sw / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
                ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            Shape::LowPass => {
                let alpha = sw / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
                ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
        };
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: [0.0; 2], z2: [0.0; 2] }
    }

    /// Keeps the filter state while changing coefficients (no clicks on knob moves).
    fn retune(&mut self, shape: Shape, freq: f32, sr: f32) {
        let n = Biquad::new(shape, freq, sr);
        (self.b0, self.b1, self.b2, self.a1, self.a2) = (n.b0, n.b1, n.b2, n.a1, n.a2);
    }

    #[inline]
    fn run(&mut self, c: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z1[c];
        self.z1[c] = self.b1 * x - self.a1 * y + self.z2[c];
        self.z2[c] = self.b2 * x - self.a2 * y;
        y
    }
}

/// A running built-in effect.
pub struct Builtin {
    pub params: BuiltinEffect,
    sr: f32,
    /// Beat length in samples (for delay sync), updated by the engine.
    pub samples_per_beat: f32,
    /// Compressor only: deepest gain reduction in the last block, dB (≥ 0).
    pub gain_reduction: f32,
    state: State,
}

enum State {
    Eq([Biquad; 5]),
    Reverb(Box<Reverb>),
    Delay { buf: [Vec<f32>; 2], pos: usize, lp: [f32; 2] },
    Compressor { env_db: f32 },
    ModDelay { buf: [Vec<f32>; 2], pos: usize, phase: f32 },
    Phaser { z: [[f32; PHASER_STAGES]; 2], fb: [f32; 2], phase: f32 },
    Distortion { lp: [f32; 2] },
    Filter { ic1: [f32; 2], ic2: [f32; 2], phase: f32 },
    Gate { gain: f32, hold_left: usize },
}

const PHASER_STAGES: usize = 6;
const TAU: f32 = std::f32::consts::TAU;

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Triangle wavefolder: identity within ±1, folding back beyond.
fn fold(v: f32) -> f32 {
    let t = (v + 1.0) * 0.25;
    1.0 - 4.0 * (t - t.floor() - 0.5).abs()
}

impl Builtin {
    pub fn new(params: BuiltinEffect, sample_rate: f64) -> Self {
        let sr = sample_rate as f32;
        let state = match &params {
            BuiltinEffect::Eq(_) => State::Eq([Biquad::default(); 5]),
            BuiltinEffect::Reverb(_) => State::Reverb(Box::new(Reverb::new(sr))),
            BuiltinEffect::Delay(_) => State::Delay { buf: [vec![0.0; (sr * 4.0) as usize], vec![0.0; (sr * 4.0) as usize]], pos: 0, lp: [0.0; 2] },
            BuiltinEffect::Compressor(_) => State::Compressor { env_db: -120.0 },
            BuiltinEffect::Chorus(_) | BuiltinEffect::Flanger(_) => State::ModDelay { buf: [vec![0.0; (sr * 0.1) as usize], vec![0.0; (sr * 0.1) as usize]], pos: 0, phase: 0.0 },
            BuiltinEffect::Phaser(_) => State::Phaser { z: [[0.0; PHASER_STAGES]; 2], fb: [0.0; 2], phase: 0.0 },
            BuiltinEffect::Distortion(_) => State::Distortion { lp: [0.0; 2] },
            BuiltinEffect::Filter(_) => State::Filter { ic1: [0.0; 2], ic2: [0.0; 2], phase: 0.0 },
            BuiltinEffect::Gate(_) => State::Gate { gain: 0.0, hold_left: 0 },
        };
        let mut b = Self { params, sr, samples_per_beat: sr * 0.5, gain_reduction: 0.0, state };
        b.retune();
        b
    }

    /// Applies new settings (same effect type) without resetting its sound.
    pub fn set_params(&mut self, params: BuiltinEffect) {
        if std::mem::discriminant(&params) == std::mem::discriminant(&self.params) {
            self.params = params;
            self.retune();
        }
    }

    fn retune(&mut self) {
        let sr = self.sr;
        if let (BuiltinEffect::Eq(p), State::Eq(f)) = (&self.params, &mut self.state) {
            f[0].retune(Shape::LowShelf(p.low_gain), p.low_freq, sr);
            f[1].retune(Shape::Peak(p.mid_gain, 0.8), p.mid_freq, sr);
            f[2].retune(Shape::HighShelf(p.high_gain), p.high_freq, sr);
            f[3].retune(Shape::HighPass, p.low_cut, sr);
            f[4].retune(Shape::LowPass, p.high_cut, sr);
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let sr = self.sr;
        match (&self.params, &mut self.state) {
            (BuiltinEffect::Eq(p), State::Eq(f)) => {
                let use_hp = p.low_cut > 21.0;
                let use_lp = p.high_cut < 19_999.0;
                for (c, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                    for x in buf.iter_mut() {
                        let low = f[0].run(c, *x);
                        let mid = f[1].run(c, low);
                        let mut y = f[2].run(c, mid);
                        if use_hp {
                            y = f[3].run(c, y);
                        }
                        if use_lp {
                            y = f[4].run(c, y);
                        }
                        *x = y;
                    }
                }
            }
            (BuiltinEffect::Reverb(p), State::Reverb(r)) => r.process(p, left, right),
            (BuiltinEffect::Delay(p), State::Delay { buf, pos, lp }) => {
                let len = buf[0].len();
                let time = if p.sync { p.beats * self.samples_per_beat } else { p.ms * 0.001 * sr };
                let d = (time as usize).clamp(1, len - 1);
                let tone = 1.0 - (-2.0 * std::f32::consts::PI * p.tone / sr).exp();
                for i in 0..left.len() {
                    let read = (*pos + len - d) % len;
                    let (dl, dr) = (buf[0][read], buf[1][read]);
                    lp[0] += (dl - lp[0]) * tone;
                    lp[1] += (dr - lp[1]) * tone;
                    let (inl, inr) = (left[i], right[i]);
                    if p.ping_pong {
                        // Mono input feeds the left line; repeats bounce between sides.
                        buf[0][*pos] = (inl + inr) * 0.5 + lp[1] * p.feedback;
                        buf[1][*pos] = lp[0] * p.feedback;
                    } else {
                        buf[0][*pos] = inl + lp[0] * p.feedback;
                        buf[1][*pos] = inr + lp[1] * p.feedback;
                    }
                    left[i] = inl + lp[0] * p.mix;
                    right[i] = inr + lp[1] * p.mix;
                    *pos = (*pos + 1) % len;
                }
            }
            (BuiltinEffect::Compressor(p), State::Compressor { env_db }) => {
                let att = (-1.0 / (p.attack.max(0.1) * 0.001 * sr)).exp();
                let rel = (-1.0 / (p.release.max(1.0) * 0.001 * sr)).exp();
                let makeup = 10f32.powf(p.makeup / 20.0);
                let slope = 1.0 - 1.0 / p.ratio.max(1.0);
                let k = p.knee.max(0.01);
                let mut deepest = 0.0f32;
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    let level = 20.0 * l.abs().max(r.abs()).max(1e-9).log10();
                    let c = if level > *env_db { att } else { rel };
                    *env_db = level + (*env_db - level) * c;
                    let over = *env_db - p.threshold;
                    let gr = if over <= -k / 2.0 {
                        0.0
                    } else if over >= k / 2.0 {
                        -over * slope
                    } else {
                        let x = over + k / 2.0;
                        -slope * x * x / (2.0 * k)
                    };
                    deepest = deepest.min(gr);
                    let g = 10f32.powf(gr / 20.0) * makeup;
                    *l *= g;
                    *r *= g;
                }
                self.gain_reduction = -deepest;
            }
            (BuiltinEffect::Chorus(p) | BuiltinEffect::Flanger(p), State::ModDelay { buf, pos, phase }) => {
                let len = buf[0].len();
                let inc = p.rate / sr;
                let fb = p.feedback.clamp(-0.95, 0.95);
                for i in 0..left.len() {
                    for (c, line) in buf.iter_mut().enumerate() {
                        let lfo = 0.5 + 0.5 * (TAU * (*phase + c as f32 * p.spread * 0.5)).sin();
                        let d = ((p.delay + p.depth * lfo) * 0.001 * sr).clamp(1.0, (len - 2) as f32);
                        let rp = *pos as f32 - d + len as f32;
                        let i0 = rp as usize % len;
                        let frac = rp.fract();
                        let wet = line[i0] * (1.0 - frac) + line[(i0 + 1) % len] * frac;
                        let x = if c == 0 { &mut left[i] } else { &mut right[i] };
                        line[*pos] = *x + wet * fb;
                        *x = *x * (1.0 - p.mix) + wet * p.mix;
                    }
                    *pos = (*pos + 1) % len;
                    *phase = (*phase + inc).fract();
                }
            }
            (BuiltinEffect::Phaser(p), State::Phaser { z, fb, phase }) => {
                let inc = p.rate / sr;
                for i in 0..left.len() {
                    for c in 0..2 {
                        let f = (p.center * 2f32.powf(2.0 * p.depth * (TAU * (*phase + c as f32 * 0.25)).sin())).clamp(20.0, sr * 0.45);
                        let t = (std::f32::consts::PI * f / sr).tan();
                        let a = (t - 1.0) / (t + 1.0);
                        let x = if c == 0 { &mut left[i] } else { &mut right[i] };
                        let mut y = *x + fb[c] * p.feedback.clamp(0.0, 0.95);
                        for s in z[c].iter_mut() {
                            let o = a * y + *s;
                            *s = y - a * o;
                            y = o;
                        }
                        fb[c] = y;
                        *x = *x * (1.0 - p.mix) + y * p.mix;
                    }
                    *phase = (*phase + inc).fract();
                }
            }
            (BuiltinEffect::Distortion(p), State::Distortion { lp }) => {
                let g = db_to_gain(p.drive);
                let out = db_to_gain(p.output);
                let tone = 1.0 - (-TAU * p.tone / sr).exp();
                let levels = 2f32.powf((16.0 - p.drive / 48.0 * 14.0).max(2.0) - 1.0);
                for (c, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                    for x in buf.iter_mut() {
                        let v = *x * g;
                        let y = match p.mode {
                            DistortionMode::Soft => v.tanh(),
                            DistortionMode::Hard => v.clamp(-1.0, 1.0),
                            DistortionMode::Fold => fold(v),
                            DistortionMode::Bitcrush => (x.clamp(-1.0, 1.0) * levels).round() / levels,
                        };
                        lp[c] += (y - lp[c]) * tone;
                        *x = *x * (1.0 - p.mix) + lp[c] * out * p.mix;
                    }
                }
            }
            (BuiltinEffect::Filter(p), State::Filter { ic1, ic2, phase }) => {
                // Topology-preserving state-variable filter (stable under fast modulation).
                let k = 2.0 - 2.0 * p.resonance.clamp(0.0, 1.0) * 0.98;
                let drive = (p.drive > 0.01).then(|| db_to_gain(p.drive));
                let coeffs = |f: f32| {
                    let g = (std::f32::consts::PI * f.clamp(20.0, sr * 0.45) / sr).tan();
                    let a1 = 1.0 / (1.0 + g * (g + k));
                    (a1, g * a1, g * g * a1)
                };
                let mut c0 = coeffs(p.cutoff);
                let inc = p.lfo_rate / sr;
                for i in 0..left.len() {
                    if p.lfo_depth > 0.0 {
                        c0 = coeffs(p.cutoff * 2f32.powf(p.lfo_depth * (TAU * *phase).sin()));
                        *phase = (*phase + inc).fract();
                    }
                    let (a1, a2, a3) = c0;
                    for c in 0..2 {
                        let x = if c == 0 { &mut left[i] } else { &mut right[i] };
                        let input = drive.map_or(*x, |d| (*x * d).tanh());
                        let v3 = input - ic2[c];
                        let v1 = a1 * ic1[c] + a2 * v3;
                        let v2 = ic2[c] + a2 * ic1[c] + a3 * v3;
                        ic1[c] = 2.0 * v1 - ic1[c];
                        ic2[c] = 2.0 * v2 - ic2[c];
                        *x = match p.mode {
                            FilterMode::LowPass => v2,
                            FilterMode::BandPass => v1,
                            FilterMode::HighPass => input - k * v1 - v2,
                            FilterMode::Notch => input - k * v1,
                        };
                    }
                }
            }
            (BuiltinEffect::Gate(p), State::Gate { gain, hold_left }) => {
                let thr = db_to_gain(p.threshold);
                let floor = db_to_gain(p.range);
                let att = (-1.0 / (p.attack.max(0.01) * 0.001 * sr)).exp();
                let rel = (-1.0 / (p.release.max(1.0) * 0.001 * sr)).exp();
                let hold = (p.hold.max(0.0) * 0.001 * sr) as usize;
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    let target = if l.abs().max(r.abs()) > thr {
                        *hold_left = hold;
                        1.0
                    } else if *hold_left > 0 {
                        *hold_left -= 1;
                        1.0
                    } else {
                        floor
                    };
                    let coef = if target > *gain { att } else { rel };
                    *gain = target + (*gain - target) * coef;
                    *l *= *gain;
                    *r *= *gain;
                }
            }
            _ => {}
        }
    }
}

/// The EQ's magnitude response in dB at `freq` (for drawing its curve).
pub fn eq_response_db(p: &EqParams, freq: f32, sr: f32) -> f32 {
    let mut bands = vec![
        Biquad::new(Shape::LowShelf(p.low_gain), p.low_freq, sr),
        Biquad::new(Shape::Peak(p.mid_gain, 0.8), p.mid_freq, sr),
        Biquad::new(Shape::HighShelf(p.high_gain), p.high_freq, sr),
    ];
    if p.low_cut > 21.0 {
        bands.push(Biquad::new(Shape::HighPass, p.low_cut, sr));
    }
    if p.high_cut < 19_999.0 {
        bands.push(Biquad::new(Shape::LowPass, p.high_cut, sr));
    }
    let w = std::f64::consts::TAU * (freq as f64 / sr as f64);
    let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
    let mut db = 0.0;
    for b in &bands {
        // |H(e^jw)| = |b0 + b1 z^-1 + b2 z^-2| / |1 + a1 z^-1 + a2 z^-2|
        let (b0, b1, b2, a1, a2) = (b.b0 as f64, b.b1 as f64, b.b2 as f64, b.a1 as f64, b.a2 as f64);
        let num = ((b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2)).sqrt();
        let den = ((1.0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2)).sqrt();
        db += 20.0 * (num / den.max(1e-12)).log10();
    }
    db as f32
}

/// Freeverb-style reverb: 8 parallel combs and 4 series allpasses per side.
struct Reverb {
    combs: [[Comb; 8]; 2],
    allpasses: [[Allpass; 4]; 2],
    pre: Vec<f32>,
    pre_pos: usize,
    sr: f32,
}

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Reverb {
    fn new(sr: f32) -> Self {
        let scale = sr / 44_100.0;
        const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASS: [usize; 4] = [556, 441, 341, 225];
        const SPREAD: usize = 23;
        let comb = |n: usize| Comb { buf: vec![0.0; ((n as f32) * scale) as usize], pos: 0, store: 0.0 };
        let ap = |n: usize| Allpass { buf: vec![0.0; ((n as f32) * scale) as usize], pos: 0 };
        Self {
            combs: [COMBS.map(comb), COMBS.map(|n| comb(n + SPREAD))],
            allpasses: [ALLPASS.map(ap), ALLPASS.map(|n| ap(n + SPREAD))],
            pre: vec![0.0; (sr * 0.2) as usize],
            pre_pos: 0,
            sr,
        }
    }

    fn process(&mut self, p: &ReverbParams, left: &mut [f32], right: &mut [f32]) {
        let feedback = 0.7 + 0.28 * p.size.clamp(0.0, 1.0);
        let damp = p.damping.clamp(0.0, 1.0) * 0.4;
        let pre_len = self.pre.len();
        let pre_d = ((p.predelay * 0.001 * self.sr) as usize).min(pre_len - 1);
        let (wet1, wet2) = (p.mix * (p.width / 2.0 + 0.5), p.mix * ((1.0 - p.width) / 2.0));
        for i in 0..left.len() {
            let input = (left[i] + right[i]) * 0.015;
            self.pre[self.pre_pos] = input;
            let delayed = self.pre[(self.pre_pos + pre_len - pre_d) % pre_len];
            self.pre_pos = (self.pre_pos + 1) % pre_len;
            let mut out = [0.0f32; 2];
            for (side, output) in out.iter_mut().enumerate() {
                for c in self.combs[side].iter_mut() {
                    let y = c.buf[c.pos];
                    c.store = y * (1.0 - damp) + c.store * damp;
                    c.buf[c.pos] = delayed + c.store * feedback;
                    c.pos = (c.pos + 1) % c.buf.len();
                    *output += y;
                }
                for a in self.allpasses[side].iter_mut() {
                    let b = a.buf[a.pos];
                    let y = -*output + b;
                    a.buf[a.pos] = *output + b * 0.5;
                    a.pos = (a.pos + 1) % a.buf.len();
                    *output = y;
                }
            }
            left[i] = left[i] * (1.0 - p.mix * 0.5) + out[0] * wet1 + out[1] * wet2;
            right[i] = right[i] * (1.0 - p.mix * 0.5) + out[1] * wet1 + out[0] * wet2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse() -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0; 48_000];
        l[0] = 1.0;
        (l.clone(), l)
    }

    #[test]
    fn flat_eq_passes_audio_unchanged() {
        let mut fx = Builtin::new(BuiltinEffect::Eq(EqParams::default()), 48_000.0);
        let (mut l, mut r) = ((0..1000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect::<Vec<_>>(), vec![0.25; 1000]);
        let before = l.clone();
        fx.process(&mut l, &mut r);
        let err = l.iter().zip(&before).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-3, "error {err}");
    }

    #[test]
    fn reverb_and_delay_leave_tails() {
        for fx in [BuiltinEffect::Reverb(ReverbParams::default()), BuiltinEffect::Delay(DelayParams::default())] {
            let mut b = Builtin::new(fx.clone(), 48_000.0);
            let (mut l, mut r) = impulse();
            b.process(&mut l, &mut r);
            let tail: f32 = l[4000..].iter().chain(&r[4000..]).map(|x| x.abs()).sum();
            assert!(tail > 0.01, "{} has a tail", fx.name());
            assert!(l.iter().all(|x| x.is_finite()));
        }
    }

    fn sine(freq: f32, amp: f32) -> Vec<f32> {
        (0..48_000).map(|i| (TAU * freq * i as f32 / 48_000.0).sin() * amp).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn every_effect_stays_finite_and_bounded() {
        for fx in BuiltinEffect::all() {
            let mut b = Builtin::new(fx.clone(), 48_000.0);
            let (mut l, mut r) = (sine(220.0, 0.8), sine(330.0, 0.8));
            for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                b.process(cl, cr);
            }
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 8.0), "{}", fx.name());
        }
    }

    #[test]
    fn params_round_trip_through_normalized_values() {
        for mut fx in BuiltinEffect::all() {
            assert_eq!(fx.specs().len(), fx.values_mut().len(), "{}", fx.name());
            for (i, spec) in fx.specs().iter().enumerate() {
                assert!((spec.to_normalized(spec.default) - fx.normalized(i)).abs() < 1e-4, "{} {} default", fx.name(), spec.name);
                fx.set_normalized(i, 0.25);
                assert!((fx.normalized(i) - 0.25).abs() < 1e-4, "{} {}", fx.name(), spec.name);
            }
        }
    }

    #[test]
    fn low_pass_filter_removes_highs_and_gate_closes_on_quiet_input() {
        let mut f = Builtin::new(BuiltinEffect::Filter(FilterParams { cutoff: 300.0, ..Default::default() }), 48_000.0);
        let (mut l, mut r) = (sine(5000.0, 0.5), sine(5000.0, 0.5));
        f.process(&mut l, &mut r);
        assert!(rms(&l[1000..]) < 0.01, "rms {}", rms(&l[1000..]));

        let mut g = Builtin::new(BuiltinEffect::Gate(GateParams::default()), 48_000.0);
        let (mut l, mut r) = (sine(100.0, 0.001), sine(100.0, 0.001));
        g.process(&mut l, &mut r);
        assert!(rms(&l[24_000..]) < 1e-6);
        let (mut l, mut r) = (sine(100.0, 0.5), sine(100.0, 0.5));
        g.process(&mut l, &mut r);
        assert!(rms(&l[24_000..]) > 0.3);
    }

    #[test]
    fn distortion_limits_and_folds() {
        let mut d = Builtin::new(BuiltinEffect::Distortion(DistortionParams { drive: 30.0, output: 0.0, tone: 20000.0, ..Default::default() }), 48_000.0);
        let (mut l, mut r) = (sine(100.0, 0.9), sine(100.0, 0.9));
        d.process(&mut l, &mut r);
        assert!(l.iter().all(|v| v.abs() <= 1.01));
        assert!(rms(&l) > 0.8, "a hard-driven sine becomes nearly square");
        assert_eq!(fold(0.5), 0.5);
        assert!((fold(1.5) - 0.5).abs() < 1e-6);
        assert!((fold(-1.5) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn compressor_reduces_loud_signals() {
        let mut b = Builtin::new(BuiltinEffect::Compressor(CompressorParams::default()), 48_000.0);
        let mut l: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.9).collect();
        let mut r = l.clone();
        b.process(&mut l, &mut r);
        let peak = l[24_000..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 0.5, "peak {peak}");
    }
}
