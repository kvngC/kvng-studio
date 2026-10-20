//! The EQ and compressor built into every mixer insert (like a console channel strip).
//!
//! Signal order inside an insert: input → EQ → compressor → plugin/effect slots → fader.
//! Both modules start switched off, and once a switch-off fade has finished they're skipped
//! entirely, so projects without these settings render bit-identically.
//!
//! The EQ is five RBJ "Audio EQ Cookbook" biquads (low shelf, three bells, high shelf) plus
//! optional 12 dB/oct high- and low-pass filters, run in f64 transposed direct form II with
//! separate left/right state. The compressor is a feed-forward, peak-detecting, stereo-linked
//! design with a soft knee, optional detector high-pass, makeup and dry/wet mix.

use serde::{Deserialize, Serialize};

use crate::effects::ParamSpec;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Band {
    pub on: bool,
    pub freq: f32,
    /// dB
    pub gain: f32,
    /// Bells only (shelves use slope S = 1).
    pub q: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub on: bool,
    /// Low shelf, three bells, high shelf.
    pub bands: [Band; 5],
    pub hp_on: bool,
    pub hp_freq: f32,
    pub lp_on: bool,
    pub lp_freq: f32,
}

impl Default for Band {
    fn default() -> Self {
        Band { on: true, freq: 1000.0, gain: 0.0, q: 0.707 }
    }
}

impl Default for EqSettings {
    fn default() -> Self {
        let band = |freq| Band { freq, ..Band::default() };
        EqSettings { on: false, bands: [band(100.0), band(300.0), band(1000.0), band(3000.0), band(10_000.0)], hp_on: false, hp_freq: 40.0, lp_on: false, lp_freq: 16_000.0 }
    }
}

impl EqSettings {
    /// Whether anything besides LOW/MID/HIGH is shaping the sound (shown on the strip).
    pub fn hidden_processing(&self) -> bool {
        self.hp_on || self.lp_on || [1, 3].iter().any(|&b| self.bands[b].on && self.bands[b].gain.abs() > 0.01)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompSettings {
    pub on: bool,
    /// dBFS
    pub threshold: f32,
    pub ratio: f32,
    /// ms
    pub attack: f32,
    pub release: f32,
    /// dB
    pub knee: f32,
    pub makeup: f32,
    /// 0..1
    pub mix: f32,
    /// High-pass on the detector only, so bass doesn't pump the compressor.
    pub hpf_on: bool,
    pub hpf: f32,
    /// Listen to the sidechain input (other inserts' sidechain sends) instead of this
    /// channel's own signal, e.g. ducking a bass under the kick.
    pub sidechain: bool,
}

impl Default for CompSettings {
    fn default() -> Self {
        CompSettings { on: false, threshold: -18.0, ratio: 4.0, attack: 10.0, release: 100.0, knee: 6.0, makeup: 0.0, mix: 1.0, hpf_on: false, hpf: 100.0, sidechain: false }
    }
}

/// Stereo controls (FL's insert stereo separation, swap and polarity).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StereoSettings {
    /// 0 = mono, 1 = unchanged, 2 = twice as wide.
    pub width: f32,
    pub swap: bool,
    pub invert_left: bool,
    pub invert_right: bool,
}

impl Default for StereoSettings {
    fn default() -> Self {
        StereoSettings { width: 1.0, swap: false, invert_left: false, invert_right: false }
    }
}

impl StereoSettings {
    pub fn is_neutral(&self) -> bool {
        *self == StereoSettings::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StripSettings {
    pub eq: EqSettings,
    pub comp: CompSettings,
    pub stereo: StereoSettings,
}

const fn spec(name: &'static str, lo: f32, hi: f32, default: f32, log: bool, unit: &'static str) -> ParamSpec {
    ParamSpec { name, lo, hi, default, log, unit }
}

/// Every automatable strip control, with its section. The index is the stable id used by
/// automation (`AutomationTarget::InsertStrip`), so only append to this list.
pub const PARAMS: [(&str, ParamSpec); 24] = [
    ("EQ", spec("Low", -12.0, 12.0, 0.0, false, "dB")),
    ("EQ", spec("Low freq", 20.0, 1000.0, 100.0, true, "Hz")),
    ("EQ", spec("Low-mid", -12.0, 12.0, 0.0, false, "dB")),
    ("EQ", spec("Low-mid freq", 20.0, 5000.0, 300.0, true, "Hz")),
    ("EQ", spec("Low-mid Q", 0.3, 15.0, 0.707, true, "")),
    ("EQ", spec("Mid", -12.0, 12.0, 0.0, false, "dB")),
    ("EQ", spec("Mid freq", 20.0, 20000.0, 1000.0, true, "Hz")),
    ("EQ", spec("Mid Q", 0.3, 15.0, 0.707, true, "")),
    ("EQ", spec("High-mid", -12.0, 12.0, 0.0, false, "dB")),
    ("EQ", spec("High-mid freq", 200.0, 20000.0, 3000.0, true, "Hz")),
    ("EQ", spec("High-mid Q", 0.3, 15.0, 0.707, true, "")),
    ("EQ", spec("High", -12.0, 12.0, 0.0, false, "dB")),
    ("EQ", spec("High freq", 1000.0, 20000.0, 10000.0, true, "Hz")),
    ("EQ", spec("High-pass", 20.0, 2000.0, 40.0, true, "Hz")),
    ("EQ", spec("Low-pass", 1000.0, 20000.0, 16000.0, true, "Hz")),
    ("Comp", spec("Threshold", -60.0, 0.0, -18.0, false, "dB")),
    ("Comp", spec("Ratio", 1.0, 20.0, 4.0, true, ":1")),
    ("Comp", spec("Attack", 0.1, 100.0, 10.0, true, "ms")),
    ("Comp", spec("Release", 10.0, 2000.0, 100.0, true, "ms")),
    ("Comp", spec("Knee", 0.0, 12.0, 6.0, false, "dB")),
    ("Comp", spec("Makeup", 0.0, 24.0, 0.0, false, "dB")),
    ("Comp", spec("Mix", 0.0, 1.0, 1.0, false, "%")),
    ("Comp", spec("Detector HPF", 20.0, 666.0, 100.0, true, "Hz")),
    ("Stereo", spec("Width", 0.0, 2.0, 1.0, false, "%")),
];

/// Indices into [`PARAMS`] for the controls shown on the mixer strip itself.
pub const LOW: usize = 0;
pub const MID: usize = 5;
pub const HIGH: usize = 11;
pub const THRESHOLD: usize = 15;
pub const RATIO: usize = 16;

impl StripSettings {
    pub fn value_mut(&mut self, index: usize) -> Option<&mut f32> {
        let (e, c, st) = (&mut self.eq, &mut self.comp, &mut self.stereo);
        let [b0, b1, b2, b3, b4] = &mut e.bands;
        let all: [&mut f32; 24] = [
            &mut b0.gain,
            &mut b0.freq,
            &mut b1.gain,
            &mut b1.freq,
            &mut b1.q,
            &mut b2.gain,
            &mut b2.freq,
            &mut b2.q,
            &mut b3.gain,
            &mut b3.freq,
            &mut b3.q,
            &mut b4.gain,
            &mut b4.freq,
            &mut e.hp_freq,
            &mut e.lp_freq,
            &mut c.threshold,
            &mut c.ratio,
            &mut c.attack,
            &mut c.release,
            &mut c.knee,
            &mut c.makeup,
            &mut c.mix,
            &mut c.hpf,
            &mut st.width,
        ];
        all.into_iter().nth(index)
    }

    pub fn value(&self, index: usize) -> Option<f32> {
        let mut copy = *self;
        copy.value_mut(index).map(|v| *v)
    }

    pub fn normalized(&self, index: usize) -> f32 {
        match (PARAMS.get(index), self.value(index)) {
            (Some((_, spec)), Some(v)) => spec.to_normalized(v),
            _ => 0.5,
        }
    }

    /// Sets a control from 0..1 (automation, MIDI links). Allocation-free.
    pub fn set_normalized(&mut self, index: usize, t: f32) {
        if let (Some((_, spec)), Some(v)) = (PARAMS.get(index), self.value_mut(index)) {
            *v = spec.from_normalized(t);
        }
    }

    /// Clamps every value into range and replaces non-finite ones with defaults.
    pub fn sanitized(mut self) -> Self {
        for (i, (_, spec)) in PARAMS.iter().enumerate() {
            if let Some(v) = self.value_mut(i) {
                *v = if v.is_finite() { v.clamp(spec.lo.min(spec.hi), spec.hi.max(spec.lo)) } else { spec.default };
            }
        }
        // The low shelf's own range is narrower than its spec allows elsewhere.
        self.eq.bands[0].q = 0.707;
        self.eq.bands[4].q = 0.707;
        self
    }

    /// The EQ's response in dB at `freq` (for drawing; uses the playback coefficients).
    pub fn eq_response_db(&self, freq: f32, sample_rate: f32) -> f32 {
        let sr = sample_rate as f64;
        eq_filters(&self.eq, sr).iter().flatten().map(|c| c.response_db(freq as f64, sr)).sum::<f64>() as f32
    }

    /// The compressor's static output level for an input level (both dB), for its graph.
    pub fn comp_curve_db(&self, input_db: f32) -> f32 {
        input_db - target_reduction(input_db - self.comp.threshold, self.comp.ratio, self.comp.knee) + self.comp.makeup
    }
}

/// Target gain reduction (positive dB) for a level `d` dB above threshold.
fn target_reduction(d: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee <= 0.0 {
        d.max(0.0) * slope
    } else if d <= -knee / 2.0 {
        0.0
    } else if d < knee / 2.0 {
        slope * (d + knee / 2.0).powi(2) / (2.0 * knee)
    } else {
        slope * d
    }
}

/// Normalised biquad coefficients.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

enum Kind {
    LowShelf(f64),
    HighShelf(f64),
    Bell(f64, f64),
    HighPass,
    LowPass,
}

impl Coeffs {
    /// RBJ Audio EQ Cookbook designs (shelves with slope S = 1, pass filters with Q = 1/√2).
    fn design(kind: Kind, freq: f64, sr: f64) -> Coeffs {
        let w = std::f64::consts::TAU * (freq.clamp(10.0, sr * 0.45) / sr);
        let (cw, sw) = (w.cos(), w.sin());
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::LowShelf(db) | Kind::HighShelf(db) => {
                let a = 10f64.powf(db / 40.0);
                let alpha = sw / 2.0 * 2f64.sqrt();
                let k = 2.0 * a.sqrt() * alpha;
                if matches!(kind, Kind::LowShelf(_)) {
                    (a * ((a + 1.0) - (a - 1.0) * cw + k), 2.0 * a * ((a - 1.0) - (a + 1.0) * cw), a * ((a + 1.0) - (a - 1.0) * cw - k), (a + 1.0) + (a - 1.0) * cw + k, -2.0 * ((a - 1.0) + (a + 1.0) * cw), (a + 1.0) + (a - 1.0) * cw - k)
                } else {
                    (a * ((a + 1.0) + (a - 1.0) * cw + k), -2.0 * a * ((a - 1.0) + (a + 1.0) * cw), a * ((a + 1.0) + (a - 1.0) * cw - k), (a + 1.0) - (a - 1.0) * cw + k, 2.0 * ((a - 1.0) - (a + 1.0) * cw), (a + 1.0) - (a - 1.0) * cw - k)
                }
            }
            Kind::Bell(db, q) => {
                let a = 10f64.powf(db / 40.0);
                let alpha = sw / (2.0 * q.max(0.05));
                (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a)
            }
            Kind::HighPass => {
                let alpha = sw / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
                ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            Kind::LowPass => {
                let alpha = sw / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
                ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
        };
        let c = Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
        // Stable second-order poles: |a2| < 1 and |a1| < 1 + a2. Fall back to identity.
        let stable = c.a2.abs() < 1.0 && c.a1.abs() < 1.0 + c.a2;
        if stable && [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()) { c } else { Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 } }
    }

    fn response_db(&self, freq: f64, sr: f64) -> f64 {
        let w = std::f64::consts::TAU * freq / sr;
        let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
        let num = ((self.b0 + self.b1 * c1 + self.b2 * c2).powi(2) + (self.b1 * s1 + self.b2 * s2).powi(2)).sqrt();
        let den = ((1.0 + self.a1 * c1 + self.a2 * c2).powi(2) + (self.a1 * s1 + self.a2 * s2).powi(2)).sqrt();
        20.0 * (num / den.max(1e-300)).log10()
    }
}

/// Filters the EQ settings call for, in processing order (None = identity, skipped).
fn eq_filters(eq: &EqSettings, sr: f64) -> [Option<Coeffs>; 7] {
    let b = &eq.bands;
    let gain = |i: usize| if b[i].on && b[i].gain.abs() > 1e-4 { Some(b[i].gain as f64) } else { None };
    [
        eq.hp_on.then(|| Coeffs::design(Kind::HighPass, eq.hp_freq as f64, sr)),
        gain(0).map(|g| Coeffs::design(Kind::LowShelf(g), b[0].freq as f64, sr)),
        gain(1).map(|g| Coeffs::design(Kind::Bell(g, b[1].q as f64), b[1].freq as f64, sr)),
        gain(2).map(|g| Coeffs::design(Kind::Bell(g, b[2].q as f64), b[2].freq as f64, sr)),
        gain(3).map(|g| Coeffs::design(Kind::Bell(g, b[3].q as f64), b[3].freq as f64, sr)),
        gain(4).map(|g| Coeffs::design(Kind::HighShelf(g), b[4].freq as f64, sr)),
        eq.lp_on.then(|| Coeffs::design(Kind::LowPass, eq.lp_freq as f64, sr)),
    ]
}

/// One biquad with its own left/right history (transposed direct form II).
#[derive(Clone, Copy, Default)]
struct Filter {
    c: Option<Coeffs>,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl Filter {
    #[inline]
    fn run(&mut self, ch: usize, x: f64) -> f64 {
        let Some(c) = self.c else { return x };
        let y = c.b0 * x + self.z1[ch];
        self.z1[ch] = c.b1 * x - c.a1 * y + self.z2[ch];
        self.z2[ch] = c.b2 * x - c.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }

    /// Changes coefficients, keeping history (filters switching in or out start clean).
    fn retune(&mut self, c: Option<Coeffs>) {
        if self.c.is_none() != c.is_none() {
            self.reset();
        }
        self.c = c;
    }

    /// Recovers from NaN/inf input so it can't poison the state forever.
    fn sanitize(&mut self) {
        if !(self.z1.iter().chain(&self.z2).all(|v| v.is_finite())) {
            self.reset();
        }
    }
}

/// Samples between coefficient updates while parameters glide.
const CONTROL_BLOCK: usize = 32;
/// Parameter glide time and on/off crossfade time.
const SMOOTH_MS: f32 = 10.0;
const FADE_MS: f32 = 5.0;

/// The running strip of one insert. All state is preallocated; `process` never allocates.
pub struct ChannelStrip {
    /// What the user set.
    pub settings: StripSettings,
    /// What the filters currently use (glides toward `settings`).
    current: EqSettings,
    sr: f64,
    filters: [Filter; 7],
    /// On/off crossfades, 0 (off) ..= 1 (on).
    eq_fade: f32,
    comp_fade: f32,
    fade_step: f32,
    glide: f32,
    detector: [Filter; 1],
    /// Smoothed gain reduction, dB (≥ 0).
    gr: f32,
    /// Deepest gain reduction in the last block, dB (≥ 0), for the meter.
    pub gain_reduction: f32,
    /// Last block's actual pre/post-compressor signal and detector levels.
    pub comp_input_db: f32,
    pub comp_output_db: f32,
    pub comp_detector_db: f32,
    dry: [Vec<f32>; 2],
}

impl ChannelStrip {
    pub fn new(settings: StripSettings, sample_rate: f64) -> Self {
        let settings = settings.sanitized();
        let sr = sample_rate.max(1.0);
        let mut strip = ChannelStrip {
            settings,
            current: settings.eq,
            sr,
            filters: [Filter::default(); 7],
            eq_fade: if settings.eq.on { 1.0 } else { 0.0 },
            comp_fade: if settings.comp.on { 1.0 } else { 0.0 },
            fade_step: 1.0 / (FADE_MS * 0.001 * sr as f32).max(1.0),
            glide: 1.0 - (-(CONTROL_BLOCK as f32) / (SMOOTH_MS * 0.001 * sr as f32)).exp(),
            detector: [Filter::default()],
            gr: 0.0,
            gain_reduction: 0.0,
            comp_input_db: -120.0,
            comp_output_db: -120.0,
            comp_detector_db: -120.0,
            dry: [vec![0.0; crate::MAX_BLOCK], vec![0.0; crate::MAX_BLOCK]],
        };
        strip.retune();
        strip
    }

    pub fn set(&mut self, settings: StripSettings) {
        self.settings = settings.sanitized();
        // Switches and filter types change at once (their filters start clean); continuous
        // values glide.
        let target = self.settings.eq;
        self.current.on = target.on;
        self.current.hp_on = target.hp_on;
        self.current.lp_on = target.lp_on;
        for (cur, t) in self.current.bands.iter_mut().zip(&target.bands) {
            cur.on = t.on;
            if cur.gain.abs() < 1e-4 && t.gain.abs() >= 1e-4 {
                // A band switching in starts at its frequency/Q so it doesn't sweep.
                cur.freq = t.freq;
                cur.q = t.q;
            }
        }
        if self.settings.comp.hpf_on {
            self.detector[0].retune(Some(Coeffs::design(Kind::HighPass, self.settings.comp.hpf as f64, self.sr)));
        } else {
            self.detector[0].retune(None);
        }
    }

    fn retune(&mut self) {
        let coeffs = eq_filters(&self.current, self.sr);
        for (f, c) in self.filters.iter_mut().zip(coeffs) {
            f.retune(c);
        }
    }

    /// Moves the playing EQ values one control step toward the targets. Returns true while
    /// still gliding.
    fn glide(&mut self) -> bool {
        let t = self.settings.eq;
        let k = self.glide;
        let mut moving = false;
        let mut step = |cur: &mut f32, target: f32, log: bool| {
            if (*cur - target).abs() <= target.abs().max(1.0) * 1e-4 {
                *cur = target;
                return;
            }
            moving = true;
            *cur = if log { (cur.ln() + (target.ln() - cur.ln()) * k).exp() } else { *cur + (target - *cur) * k };
        };
        for (cur, tb) in self.current.bands.iter_mut().zip(&t.bands) {
            step(&mut cur.gain, tb.gain, false);
            step(&mut cur.freq, tb.freq, true);
            step(&mut cur.q, tb.q, true);
        }
        step(&mut self.current.hp_freq, t.hp_freq, true);
        step(&mut self.current.lp_freq, t.lp_freq, true);
        moving
    }

    /// Processes a block. `key` is the sidechain input, when other inserts send to it.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let len = left.len().min(right.len()).min(self.dry[0].len());
        self.gain_reduction = 0.0;
        self.comp_input_db = -120.0;
        self.comp_output_db = -120.0;
        self.comp_detector_db = -120.0;
        let (eq_on, comp_on) = (self.settings.eq.on, self.settings.comp.on);
        if self.eq_fade > 0.0 || eq_on {
            if self.eq_fade == 0.0 {
                // Fresh start: no stale history from before it was switched off.
                self.filters.iter_mut().for_each(Filter::reset);
            }
            self.process_eq(&mut left[..len], &mut right[..len], eq_on);
        }
        if self.comp_fade > 0.0 || comp_on {
            if self.comp_fade == 0.0 {
                self.gr = 0.0;
                self.detector[0].reset();
            }
            let key = key.filter(|_| self.settings.comp.sidechain).map(|(kl, kr)| (&kl[..len.min(kl.len())], &kr[..len.min(kr.len())]));
            self.process_comp(&mut left[..len], &mut right[..len], comp_on, key);
        } else {
            self.gr = 0.0;
        }
        let st = self.settings.stereo;
        if !st.is_neutral() {
            let w = st.width.clamp(0.0, 2.0);
            for (l, r) in left[..len].iter_mut().zip(right[..len].iter_mut()) {
                let (mut a, mut b) = if st.swap { (*r, *l) } else { (*l, *r) };
                if st.invert_left {
                    a = -a;
                }
                if st.invert_right {
                    b = -b;
                }
                let (mid, side) = ((a + b) * 0.5, (a - b) * 0.5 * w);
                *l = mid + side;
                *r = mid - side;
            }
        }
    }

    fn process_eq(&mut self, left: &mut [f32], right: &mut [f32], on: bool) {
        let target_fade = if on { 1.0 } else { 0.0 };
        let mut start = 0;
        while start < left.len() {
            let end = (start + CONTROL_BLOCK).min(left.len());
            if self.glide() || self.filters.iter().zip(eq_filters(&self.current, self.sr)).any(|(f, c)| f.c != c) {
                self.retune();
            }
            for i in start..end {
                let (dl, dr) = (left[i], right[i]);
                let (mut l, mut r) = (dl as f64, dr as f64);
                for f in &mut self.filters {
                    l = f.run(0, l);
                    r = f.run(1, r);
                }
                if self.eq_fade != target_fade {
                    self.eq_fade = if on { (self.eq_fade + self.fade_step).min(1.0) } else { (self.eq_fade - self.fade_step).max(0.0) };
                }
                let m = self.eq_fade as f64;
                left[i] = (dl as f64 + (l - dl as f64) * m) as f32;
                right[i] = (dr as f64 + (r - dr as f64) * m) as f32;
            }
            start = end;
        }
        self.filters.iter_mut().for_each(Filter::sanitize);
    }

    fn process_comp(&mut self, left: &mut [f32], right: &mut [f32], on: bool, key: Option<(&[f32], &[f32])>) {
        let c = self.settings.comp;
        let sr = self.sr as f32;
        let att = (-1.0 / (sr * c.attack.max(0.01) * 0.001)).exp();
        let rel = (-1.0 / (sr * c.release.max(1.0) * 0.001)).exp();
        let target_fade = if on { 1.0 } else { 0.0 };
        let mut deepest = 0.0f32;
        let (mut peak_input, mut paired_output, mut peak_detector) = (0.0f32, 0.0f32, 0.0f32);
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let (dl, dr) = (*l, *r);
            let (kl, kr) = match key {
                Some((kl, kr)) => (kl.get(i).copied().unwrap_or(0.0), kr.get(i).copied().unwrap_or(0.0)),
                None => (dl, dr),
            };
            let (detl, detr) = (self.detector[0].run(0, kl as f64) as f32, self.detector[0].run(1, kr as f64) as f32);
            let detector_level = detl.abs().max(detr.abs());
            peak_detector = peak_detector.max(detector_level);
            let level = 20.0 * detector_level.max(1e-12).log10();
            let target = target_reduction(level - c.threshold, c.ratio, c.knee);
            let a = if target > self.gr { att } else { rel };
            self.gr = a * self.gr + (1.0 - a) * target;
            if !self.gr.is_finite() {
                self.gr = 0.0;
            }
            deepest = deepest.max(self.gr);
            let g = 10f32.powf((c.makeup - self.gr) / 20.0);
            let (wl, wr) = (dl * g, dr * g);
            let (ol, or) = (dl + c.mix * (wl - dl), dr + c.mix * (wr - dr));
            if self.comp_fade != target_fade {
                self.comp_fade = if on { (self.comp_fade + self.fade_step).min(1.0) } else { (self.comp_fade - self.fade_step).max(0.0) };
            }
            *l = dl + (ol - dl) * self.comp_fade;
            *r = dr + (or - dr) * self.comp_fade;
            let input = dl.abs().max(dr.abs());
            if input > 0.0 && input >= peak_input {
                peak_input = input;
                paired_output = l.abs().max(r.abs());
            }
        }
        self.detector[0].sanitize();
        self.gain_reduction = if on { deepest } else { 0.0 };
        let db = |v: f32| 20.0 * v.max(1e-6).log10();
        self.comp_input_db = db(peak_input);
        self.comp_output_db = db(paired_output);
        self.comp_detector_db = db(peak_detector);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn sine(freq: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (std::f64::consts::TAU * freq * i as f64 / SR).sin() as f32 * amp).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn run(strip: &mut ChannelStrip, l: &mut [f32], r: &mut [f32]) {
        for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            strip.process(cl, cr, None);
        }
    }

    #[test]
    fn off_is_exact_pass_through() {
        let mut strip = ChannelStrip::new(StripSettings::default(), SR);
        let mut l = sine(440.0, 0.8, 4096);
        let mut r = sine(220.0, 0.8, 4096);
        let (bl, br) = (l.clone(), r.clone());
        run(&mut strip, &mut l, &mut r);
        assert_eq!((l, r), (bl, br));
    }

    #[test]
    fn neutral_eq_switched_on_is_identity() {
        let mut s = StripSettings::default();
        s.eq.on = true;
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = sine(440.0, 0.8, 4096);
        let mut r = l.clone();
        let before = l.clone();
        run(&mut strip, &mut l, &mut r);
        assert_eq!(l, before);
    }

    #[test]
    fn bell_and_shelf_match_their_curves() {
        let mut s = StripSettings::default();
        s.eq.on = true;
        s.eq.bands[2].gain = 6.0; // 1 kHz bell
        s.eq.bands[0].gain = -9.0; // 100 Hz low shelf
        assert!((s.eq_response_db(1000.0, SR as f32) - 6.0).abs() < 0.1);
        assert!((s.eq_response_db(15.0, SR as f32) + 9.0).abs() < 0.2, "shelf asymptote");
        for (freq, expect) in [(1000.0, s.eq_response_db(1000.0, SR as f32)), (5000.0, s.eq_response_db(5000.0, SR as f32)), (60.0, s.eq_response_db(60.0, SR as f32))] {
            let mut strip = ChannelStrip::new(s, SR);
            let mut l = sine(freq, 0.25, 48_000);
            let mut r = l.clone();
            run(&mut strip, &mut l, &mut r);
            let measured = 20.0 * (rms(&l[24_000..]) / (0.25 / 2f32.sqrt())).log10();
            assert!((measured - expect).abs() < 0.1, "{freq} Hz: measured {measured:.2} dB, curve {expect:.2} dB");
            assert_eq!(l, r, "left and right are processed identically but separately");
        }
    }

    #[test]
    fn compressor_static_transfer_with_dc_fixture() {
        let s = StripSettings { comp: CompSettings { on: true, threshold: -18.0, ratio: 4.0, knee: 0.0, attack: 1.0, release: 50.0, ..Default::default() }, ..Default::default() };
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = vec![0.5f32; 48_000]; // −6.02 dBFS
        let mut r = l.clone();
        run(&mut strip, &mut l, &mut r);
        let expected_gr = (20.0 * 0.5f32.log10() + 18.0) * 0.75;
        let out_db = 20.0 * l[47_999].log10();
        assert!((out_db - (20.0 * 0.5f32.log10() - expected_gr)).abs() < 0.05, "out {out_db}");
        assert!((strip.gain_reduction - expected_gr).abs() < 0.05);
        assert!((s.comp_curve_db(-6.02) - (-6.02 - expected_gr)).abs() < 0.05);
        assert!((strip.comp_input_db - -6.0206).abs() < 0.01);
        assert!((strip.comp_detector_db - -6.0206).abs() < 0.01);
        assert!((strip.comp_output_db - out_db).abs() < 0.05);
    }

    #[test]
    fn compressor_meter_distinguishes_sidechain_from_program_audio() {
        let s = StripSettings { comp: CompSettings { on: true, sidechain: true, threshold: -30.0, attack: 0.1, ..Default::default() }, ..Default::default() };
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = vec![0.1; 512];
        let mut r = l.clone();
        let key = vec![0.8; 512];
        strip.process(&mut l, &mut r, Some((&key, &key)));
        assert!((strip.comp_input_db - -20.0).abs() < 0.01);
        assert!((strip.comp_detector_db - -1.938).abs() < 0.01);
        assert!(strip.comp_output_db < strip.comp_input_db);
    }

    #[test]
    fn compressor_attack_reaches_one_time_constant() {
        let s = StripSettings { comp: CompSettings { on: true, threshold: -30.0, ratio: 10.0, knee: 0.0, attack: 10.0, ..Default::default() }, ..Default::default() };
        let mut strip = ChannelStrip::new(s, SR);
        let n = (SR * 0.010) as usize;
        let mut l = vec![1.0f32; n];
        let mut r = l.clone();
        run(&mut strip, &mut l, &mut r);
        let target = 30.0 * 0.9;
        assert!((strip.gr / target - 0.632).abs() < 0.01, "gr after one attack time: {}", strip.gr / target);
    }

    #[test]
    fn unity_ratio_and_stereo_link() {
        let s = StripSettings { comp: CompSettings { on: true, ratio: 1.0, knee: 0.0, ..Default::default() }, ..Default::default() };
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = sine(100.0, 0.9, 4800);
        let mut r = l.clone();
        let before = l.clone();
        run(&mut strip, &mut l, &mut r);
        assert!(l.iter().zip(&before).all(|(a, b)| (a - b).abs() < 1e-6));

        let s = StripSettings { comp: CompSettings { on: true, knee: 0.0, attack: 0.1, ..Default::default() }, ..Default::default() };
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = vec![0.9f32; 4800];
        let mut r = vec![0.01f32; 4800];
        run(&mut strip, &mut l, &mut r);
        assert!((l[4799] / 0.9 - r[4799] / 0.01).abs() < 1e-4, "both sides get the same gain");
    }

    #[test]
    fn toggling_is_click_free_and_finite() {
        let mut s = StripSettings::default();
        s.eq.bands[2].gain = 12.0;
        s.eq.bands[2].q = 15.0;
        s.comp.threshold = -40.0;
        let mut strip = ChannelStrip::new(s, SR);
        let mut l = sine(1000.0, 0.5, 48_000);
        let mut r = l.clone();
        for (i, (cl, cr)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
            s.eq.on = i % 3 != 0;
            s.comp.on = i % 5 < 2;
            strip.set(s);
            strip.process(cl, cr, None);
        }
        assert!(l.iter().all(|v| v.is_finite()));
        let biggest_jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(biggest_jump < 1.0, "jump {biggest_jump}");

        // NaN input doesn't poison later audio.
        let mut nan = vec![f32::NAN; 64];
        let mut nan_r = nan.clone();
        strip.process(&mut nan, &mut nan_r, None);
        let mut l = sine(1000.0, 0.5, 2048);
        let mut r = l.clone();
        strip.process(&mut l[..1024], &mut r[..1024], None);
        strip.process(&mut l[1024..], &mut r[1024..], None);
        assert!(l[1024..].iter().all(|v| v.is_finite()));
    }

    #[test]
    fn stereo_width_swap_and_invert() {
        let mut s = StripSettings::default();
        s.stereo.width = 0.0;
        let mut strip = ChannelStrip::new(s, SR);
        let (mut l, mut r) = (vec![1.0f32; 64], vec![0.0f32; 64]);
        strip.process(&mut l, &mut r, None);
        assert!((l[10] - 0.5).abs() < 1e-6 && (r[10] - 0.5).abs() < 1e-6, "width 0 = mono");
        s.stereo = StereoSettings { swap: true, invert_left: true, ..Default::default() };
        strip.set(s);
        let (mut l, mut r) = (vec![1.0f32; 64], vec![0.25f32; 64]);
        strip.process(&mut l, &mut r, None);
        assert!((l[10] + 0.25).abs() < 1e-6 && (r[10] - 1.0).abs() < 1e-6, "swapped, then left inverted");
    }

    #[test]
    fn params_round_trip() {
        let mut s = StripSettings::default();
        for (i, (_, spec)) in PARAMS.iter().enumerate() {
            assert!((s.value(i).unwrap() - spec.default).abs() < 1e-3, "{} default", spec.name);
            s.set_normalized(i, 0.3);
            assert!((s.normalized(i) - 0.3).abs() < 1e-4, "{}", spec.name);
        }
        assert!(s.value(PARAMS.len()).is_none());
    }
}
