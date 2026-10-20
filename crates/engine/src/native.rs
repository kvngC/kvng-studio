//! KVNG Studio's own implementations of FL Studio's built-in effects.
//!
//! Each adapter reads the parameter block FL saved for the plugin and maps it
//! onto our DSP. The DSP is ours; only the stored settings are interpreted.
//! Parameter layouts come from PyFLP's documentation and from comparing FL's
//! factory presets; time constants marked "approx." are not yet calibrated.

use crate::gain;

pub trait NativeEffect: Send {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Processing delay in samples.
    fn latency(&self) -> usize {
        0
    }
}

/// Builds an adapter for FL's `internal_name`. `None` means we have no adapter.
pub fn effect(internal_name: &str, data: &[u8], sample_rate: f64) -> Option<Result<Box<dyn NativeEffect>, String>> {
    let p = Params::new(data);
    let fx: Result<Box<dyn NativeEffect>, String> = match internal_name {
        "Fruity Limiter" => FruityLimiter::new(&p, sample_rate).map(|f| Box::new(f) as _),
        "Fruity Balance" => Balance::new(&p).map(|f| Box::new(f) as _),
        "Fruity Soft Clipper" => SoftClipper::new(&p).map(|f| Box::new(f) as _),
        "Fruity Fast Dist" => FastDist::new(&p).map(|f| Box::new(f) as _),
        "Fruity Stereo Enhancer" => StereoEnhancer::new(&p, sample_rate).map(|f| Box::new(f) as _),
        _ => return None,
    };
    Some(fx)
}

/// Names of FL plugins we have adapters for.
pub const SUPPORTED: &[&str] = &["Fruity Limiter", "Fruity Balance", "Fruity Soft Clipper", "Fruity Fast Dist", "Fruity Stereo Enhancer"];

/// Little-endian 32-bit fields of a plugin's data block.
struct Params<'a>(&'a [u8]);

impl<'a> Params<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    fn i32(&self, index: usize) -> Result<i32, String> {
        self.0
            .get(index * 4..index * 4 + 4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| format!("settings block too short ({} bytes)", self.0.len()))
    }

    fn f(&self, index: usize) -> Result<f32, String> {
        self.i32(index).map(|v| v as f32)
    }
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn gain_to_db(g: f32) -> f32 {
    20.0 * g.max(1e-9).log10()
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn coeff(ms: f32, sample_rate: f64) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sample_rate as f32)).exp()
    }
}

/// Maps a 0..=10000 knob onto `min..max` milliseconds on a log scale (approx.).
fn knob_ms(value: f32, min: f32, max: f32) -> f32 {
    let x = (value / 10000.0).clamp(0.0, 1.0);
    min * (max / min).powf(x)
}

// ---------------------------------------------------------------------------
// Fruity Limiter

/// Layout (u32 fields after a version word), from FL's factory presets:
/// limiter gain, saturation, ceiling (all ×1000 linear), attack, attack curve,
/// release, release curve, sustain; compressor threshold (×1000), knee, ratio
/// (−1000..1000, negative = expansion), attack, release, curve, sustain;
/// noise gate gain (×1000), threshold (×1000), release. A second block of the
/// same shape follows, then global settings.
struct FruityLimiter {
    gain: f32,
    saturation: f32,
    ceiling: f32,
    lim_attack: f32,
    lim_release: f32,
    lim_hold: usize,
    comp_threshold_db: f32,
    comp_knee_db: f32,
    comp_ratio: f32,
    comp_attack: f32,
    comp_release: f32,
    gate_floor: f32,
    gate_threshold: f32,
    gate_release: f32,
    // state
    lim_env: f32,
    lim_hold_left: usize,
    comp_env_db: f32,
    gate_gain: f32,
}

impl FruityLimiter {
    fn new(p: &Params, sample_rate: f64) -> Result<Self, String> {
        let at = |i: usize| p.f(i + 1); // skip the version word
        let comp_threshold = at(8)? / 1000.0;
        Ok(Self {
            gain: at(0)? / 1000.0,
            saturation: at(1)? / 1000.0,
            ceiling: (at(2)? / 1000.0).max(0.001),
            lim_attack: coeff(knob_ms(at(3)?, 0.05, 50.0), sample_rate),
            lim_release: coeff(knob_ms(at(5)?, 1.0, 2000.0), sample_rate),
            lim_hold: (knob_ms(at(7)?, 0.1, 1000.0) * 0.001 * sample_rate as f32) as usize,
            comp_threshold_db: gain_to_db(comp_threshold.max(0.001)),
            comp_knee_db: at(9)? / 1000.0 * 12.0,
            comp_ratio: (at(10)? / 1000.0).clamp(-1.0, 1.0),
            comp_attack: coeff(knob_ms(at(11)?.max(1.0), 0.05, 200.0), sample_rate),
            comp_release: coeff(knob_ms(at(12)?, 1.0, 2000.0), sample_rate),
            gate_floor: at(15)? / 1000.0,
            gate_threshold: at(16)? / 1000.0,
            gate_release: coeff(knob_ms(at(17)?, 1.0, 2000.0), sample_rate),
            lim_env: 0.0,
            lim_hold_left: 0,
            comp_env_db: -120.0,
            gate_gain: 1.0,
        })
    }

    /// Gain change in dB the compressor applies to a signal at `level_db`.
    fn comp_gain_db(&self, level_db: f32) -> f32 {
        let over = level_db - self.comp_threshold_db;
        if self.comp_ratio > 0.0 {
            // Soft knee: blend into the slope across the knee width.
            let k = self.comp_knee_db.max(0.01);
            let slope = self.comp_ratio; // 1.0 = infinite ratio
            if over <= -k / 2.0 {
                0.0
            } else if over >= k / 2.0 {
                -over * slope
            } else {
                let x = over + k / 2.0;
                -slope * x * x / (2.0 * k)
            }
        } else if self.comp_ratio < 0.0 && over < 0.0 {
            // Downward expansion below the threshold.
            over * -self.comp_ratio
        } else {
            0.0
        }
    }

    fn saturate(&self, x: f32) -> f32 {
        let (s, c) = (self.saturation, self.ceiling);
        let a = x.abs();
        if s >= c || a <= s {
            return x;
        }
        let y = s + (c - s) * ((a - s) / (c - s)).tanh();
        y.copysign(x)
    }
}

impl NativeEffect for FruityLimiter {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let comp_active = self.comp_ratio != 0.0;
        let gate_active = self.gate_floor < 0.999 && self.gate_threshold > 0.0;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let peak = l.abs().max(r.abs());

            if gate_active {
                let target = if peak >= self.gate_threshold { 1.0 } else { self.gate_floor };
                self.gate_gain = if target > self.gate_gain { target } else { target + (self.gate_gain - target) * self.gate_release };
            }

            let mut g = self.gate_gain;
            if comp_active {
                let level_db = gain_to_db(peak);
                let c = if level_db > self.comp_env_db { self.comp_attack } else { self.comp_release };
                self.comp_env_db = level_db + (self.comp_env_db - level_db) * c;
                g *= db_to_gain(self.comp_gain_db(self.comp_env_db));
            }
            g *= self.gain;

            let (mut sl, mut sr) = (self.saturate(*l * g), self.saturate(*r * g));

            // Peak limiter with hold, then a hard ceiling so nothing escapes.
            let p = sl.abs().max(sr.abs());
            if p > self.lim_env {
                self.lim_env = p + (self.lim_env - p) * self.lim_attack;
                self.lim_hold_left = self.lim_hold;
            } else if self.lim_hold_left > 0 {
                self.lim_hold_left -= 1;
            } else {
                self.lim_env = p + (self.lim_env - p) * self.lim_release;
            }
            let lg = if self.lim_env > self.ceiling { self.ceiling / self.lim_env } else { 1.0 };
            sl = (sl * lg).clamp(-self.ceiling, self.ceiling);
            sr = (sr * lg).clamp(-self.ceiling, self.ceiling);
            *l = sl;
            *r = sr;
        }
    }
}

// ---------------------------------------------------------------------------
// Simple utilities (layouts documented by PyFLP)

/// Fruity Balance: `i32 pan (-128..127)`, `u32 volume (0..320, 256 = 0 dB)`.
struct Balance {
    gl: f32,
    gr: f32,
}

impl Balance {
    fn new(p: &Params) -> Result<Self, String> {
        let pan = p.f(0)? / 128.0;
        let vol = gain::fl_volume(p.f(1)? / 320.0);
        let (gl, gr) = gain::pan_gains(vol, pan);
        Ok(Self { gl, gr })
    }
}

impl NativeEffect for Balance {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.iter_mut().for_each(|s| *s *= self.gl);
        right.iter_mut().for_each(|s| *s *= self.gr);
    }
}

/// Fruity Soft Clipper: `u32 threshold (1..127, 127 = 0 dB)`, `u32 post (0..160, 128 = unity)`.
struct SoftClipper {
    threshold: f32,
    post: f32,
}

impl SoftClipper {
    fn new(p: &Params) -> Result<Self, String> {
        let t = (p.f(0)? / 127.0).clamp(0.0, 1.0);
        Ok(Self { threshold: t * t, post: p.f(1)? / 128.0 })
    }

    fn clip(&self, x: f32) -> f32 {
        let a = x.abs();
        let t = self.threshold;
        if a <= t || t >= 1.0 {
            x.clamp(-1.0, 1.0)
        } else {
            (t + (1.0 - t) * ((a - t) / (1.0 - t)).tanh()).copysign(x)
        }
    }
}

impl NativeEffect for SoftClipper {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s = self.clip(*s) * self.post;
        }
    }
}

/// Fruity Fast Dist: `pre (64..192, 128 = unity)`, `threshold (1..10)`,
/// `kind (A = 0, B = 1)`, `mix (0..128)`, `post (0..128)`.
struct FastDist {
    pre: f32,
    threshold: f32,
    soft: bool,
    mix: f32,
    post: f32,
}

impl FastDist {
    fn new(p: &Params) -> Result<Self, String> {
        Ok(Self {
            pre: p.f(0)? / 128.0,
            threshold: (p.f(1)? / 10.0).clamp(0.1, 1.0),
            soft: p.i32(2)? == 1,
            mix: (p.f(3)? / 128.0).clamp(0.0, 1.0),
            post: p.f(4)? / 128.0,
        })
    }
}

impl NativeEffect for FastDist {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let t = self.threshold;
        for s in left.iter_mut().chain(right.iter_mut()) {
            let x = *s * self.pre;
            // A clips hard at the threshold; B saturates smoothly (approx. of FL's two modes).
            let wet = if self.soft { (x / t).tanh() * t } else { x.clamp(-t, t) } / t;
            *s = (x + (wet - x) * self.mix) * self.post;
        }
    }
}

/// Fruity Stereo Enhancer: `pan`, `volume (256 = 0 dB)`, `separation (-96..96)`,
/// `phase offset (-512..512 = ±500 ms)`, `position (pre/post)`, `phase inversion`.
struct StereoEnhancer {
    gl: f32,
    gr: f32,
    /// -1 = fully widened, 0 = unchanged, 1 = mono.
    separation: f32,
    invert: (f32, f32),
    delay_left: bool,
    delay: Vec<f32>,
    delay_pos: usize,
}

impl StereoEnhancer {
    fn new(p: &Params, sample_rate: f64) -> Result<Self, String> {
        let pan = p.f(0)? / 128.0;
        let vol = gain::fl_volume(p.f(1)? / 320.0);
        let (gl, gr) = gain::pan_gains(vol, pan);
        let offset = p.f(3)?;
        let delay_samples = (offset.abs() / 512.0 * 0.5 * sample_rate as f32) as usize;
        let invert = match p.i32(5)? {
            1 => (-1.0, 1.0),
            2 => (1.0, -1.0),
            _ => (1.0, 1.0),
        };
        Ok(Self {
            gl,
            gr,
            separation: (p.f(2)? / 96.0).clamp(-1.0, 1.0),
            invert,
            delay_left: offset < 0.0,
            delay: vec![0.0; delay_samples.max(1)],
            delay_pos: 0,
        })
    }
}

impl NativeEffect for StereoEnhancer {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let use_delay = self.delay.len() > 1;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (mut a, mut b) = (*l * self.invert.0, *r * self.invert.1);
            if use_delay {
                let target = if self.delay_left { &mut a } else { &mut b };
                let delayed = self.delay[self.delay_pos];
                self.delay[self.delay_pos] = *target;
                self.delay_pos = (self.delay_pos + 1) % self.delay.len();
                *target = delayed;
            }
            // Mid/side: positive separation narrows, negative widens.
            let mid = (a + b) * 0.5;
            let side = (a - b) * 0.5 * (1.0 - self.separation);
            *l = (mid + side) * self.gl;
            *r = (mid - side) * self.gr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// FL's factory "Default" Fruity Limiter settings.
    const LIMITER_DEFAULT: [i32; 42] = [
        5, 1000, 1000, 1000, 1115, 3, 5000, 3, 2882, 1000, 0, 0, 0, 6666, 1, 650, 1000, 0, 4187, 1000, 1000, 1000,
        1115, 3, 5000, 3, 2882, 1000, 0, 0, 0, 6666, 1, 650, 1000, 0, 4187, 50, 0, 16843009, 0, 0,
    ];

    #[test]
    fn default_limiter_is_transparent_below_ceiling_and_caps_peaks() {
        let mut fx = effect("Fruity Limiter", &le(&LIMITER_DEFAULT), 48_000.0).unwrap().unwrap();
        let mut l: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut r = l.clone();
        let before = l.clone();
        fx.process(&mut l, &mut r);
        let max_diff = l.iter().zip(&before).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_diff < 1e-6, "quiet signal changed by {max_diff}");

        let mut l: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 3.0).collect();
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|s| s.abs() <= 1.0 + 1e-6));
    }

    #[test]
    fn balance_default_is_unity() {
        let mut fx = effect("Fruity Balance", &le(&[0, 256]), 48_000.0).unwrap().unwrap();
        let (mut l, mut r) = (vec![0.5f32; 8], vec![-0.25f32; 8]);
        fx.process(&mut l, &mut r);
        assert!((l[0] - 0.5).abs() < 1e-4 && (r[0] + 0.25).abs() < 1e-4);
    }

    #[test]
    fn unknown_plugin_has_no_adapter() {
        assert!(effect("Fruity Reeverb 2", &[], 48_000.0).is_none());
    }
}
