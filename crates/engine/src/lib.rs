//! KVNG Studio's audio engine.
//!
//! An [`Engine`] owns everything needed to produce audio: channels (samplers,
//! plugins, audio clips), the mixer, and a timeline of scheduled events. It is
//! built on the main thread (see [`load`]) and then driven block by block, either
//! offline ([`Engine::render`]) or from a real-time audio callback.

pub mod arp;
pub mod channel_strip;
pub mod command;
pub mod control;
pub mod edit;
pub mod effects;
pub mod gain;
pub mod instrument;
pub mod load;
pub mod midi_file;
pub mod midi_out;
pub mod monitor;
pub mod native;
pub mod sample;
pub mod song;
pub mod timeline;
pub mod view;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use control::{Controls, Strip};

use bridge::RemotePlugin;
use plughost::{MidiEvent, Plugin};
use sample::Sample;

/// The largest block processed at once; callbacks with larger buffers are split.
pub const MAX_BLOCK: usize = 512;
/// Channels a project can grow to (storage is reserved up front so adding one never
/// allocates on the audio thread).
pub const MAX_CHANNELS: usize = 512;

/// The key that plays a slicer's first slice (C5, FL's default root).
pub const SLICE_ROOT: u8 = 60;
/// Effect slots per mixer insert.
pub const MAX_EFFECTS: usize = 10;
/// Voices per sampler channel before the oldest one is stolen.
const MAX_VOICES: usize = 32;
/// Fade applied when a voice is stolen or cut, in samples, to avoid clicks.
const DECLICK: usize = 64;

#[derive(Debug, Clone, Copy)]
pub enum EventKind {
    NoteOn { key: u8, velocity: u8, expr: NoteExpr },
    NoteOff { key: u8 },
    /// Start an audio clip `offset` samples into the channel's sample, for `length` samples,
    /// at `gain` (linear) with linear fades (samples), optionally played backwards.
    Clip { offset: usize, length: usize, gain: f32, fade_in: u32, fade_out: u32, reverse: bool },
    /// Glide the channel's playing notes to `key` over `duration` samples (a slide note).
    Slide { key: u8, duration: u32 },
}

/// Per-note expression: pan and two modulation amounts, each -1.0..=1.0 (0 = neutral).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NoteExpr {
    pub pan: f32,
    pub mod_x: f32,
    pub mod_y: f32,
}

impl NoteExpr {
    /// From FL-style note values (pan 0..=128 centred on 64, mod 0..=255 centred on 128).
    pub fn from_note(pan: u8, mod_x: u8, mod_y: u8) -> Self {
        Self { pan: (pan as f32 - 64.0) / 64.0, mod_x: (mod_x as f32 - 128.0) / 128.0, mod_y: (mod_y as f32 - 128.0) / 128.0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Event {
    /// Absolute position in samples.
    pub time: u64,
    pub channel: usize,
    /// Playlist track the event comes from ([`timeline::NO_TRACK`] for none).
    pub track: u16,
    pub kind: EventKind,
}

/// A third-party plugin, hosted either in a helper process (the default) or in-process.
pub enum PluginInstance {
    Local(Plugin),
    Remote(RemotePlugin),
}

impl PluginInstance {
    pub fn process(&mut self, channels: &mut [&mut [f32]], midi: &[MidiEvent]) {
        match self {
            PluginInstance::Local(p) => p.process(channels, midi),
            PluginInstance::Remote(p) => p.process(channels, midi),
        }
    }

    pub fn latency_samples(&self) -> usize {
        match self {
            PluginInstance::Local(p) => p.latency_samples(),
            PluginInstance::Remote(p) => p.latency_samples(),
        }
    }

    /// Sets a parameter (normalised 0..1) for the next block. Real-time safe.
    pub fn set_param(&mut self, index: u32, value: f32) {
        match self {
            PluginInstance::Local(p) => p.set_param(index, value),
            PluginInstance::Remote(p) => p.set_param(index, value),
        }
    }

    pub fn name(&self) -> String {
        match self {
            PluginInstance::Local(p) => p.name(),
            PluginInstance::Remote(p) => p.info.name.clone(),
        }
    }

    pub fn num_outputs(&self) -> usize {
        match self {
            PluginInstance::Local(p) => p.num_outputs(),
            PluginInstance::Remote(p) => p.info.outputs,
        }
    }

    pub fn accepts_midi(&self) -> bool {
        match self {
            PluginInstance::Local(p) => p.accepts_midi(),
            PluginInstance::Remote(p) => p.info.accepts_midi,
        }
    }

    pub fn open_editor(&mut self) -> bool {
        match self {
            PluginInstance::Local(p) => p.open_editor(),
            PluginInstance::Remote(p) => p.open_editor().is_ok(),
        }
    }

    /// Where the plugin runs, for display: "in-process", "helper (arm64)", "helper (x86_64)".
    pub fn location(&self) -> String {
        match self {
            PluginInstance::Local(_) => "in-process".into(),
            PluginInstance::Remote(p) => format!("helper ({})", p.arch),
        }
    }

    /// False once a helper-hosted plugin has crashed or hung.
    pub fn is_alive(&self) -> bool {
        match self {
            PluginInstance::Local(_) => true,
            PluginInstance::Remote(p) => p.is_alive(),
        }
    }
}

pub enum Source {
    Sampler { sample: Arc<Sample>, root_note: u8, voices: Vec<Voice> },
    AudioClip { sample: Arc<Sample>, voices: Vec<Voice> },
    /// A loop cut into slices: key C5 + n plays slice n (like FL's Slicex).
    Slicer { sample: Arc<Sample>, slices: Vec<(usize, usize)>, voices: Vec<Voice> },
    Plugin { plugin: Box<PluginInstance>, midi: Vec<MidiEvent> },
    /// KVNG Studio's built-in synth.
    Synth(Box<instrument::Synth>),
    /// Notes sent to an external MIDI device, on MIDI channel `midi_channel` (0..=15).
    MidiOut { out: Box<midi_out::MidiOut>, midi_channel: u8 },
    /// A channel we can't play yet (missing plugin, FL-native generator, ...).
    Silent,
}

pub struct Voice {
    /// The note that started it (for note-offs).
    key: u8,
    /// Per-note pan, -1..=1.
    pan: f32,
    /// Envelope level and stage (0 attack, 1 decay, 2 sustain, 3 release).
    env: f32,
    stage: u8,
    /// Read position in the sample, in (fractional) samples.
    pos: f64,
    /// Read increment per output sample (pitch ratio).
    step: f64,
    /// Sample index at which the voice ends.
    end: f64,
    gain: f32,
    /// Output samples to wait before starting (sub-block scheduling).
    delay: usize,
    /// Audio clip fades (None for notes).
    clip_fade: Option<ClipFade>,
    /// Pitch at note-on, and a slide target the step glides to (coeff 0 = no slide).
    base_step: f64,
    step_target: f64,
    step_coeff: f64,
    /// Remaining samples of a fade-out, when the voice is being cut.
    fade: Option<usize>,
}

/// Fixed delay used for plugin delay compensation. A zero-length line passes audio through.
#[derive(Default)]
struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    fn new(samples: usize) -> Self {
        Self { buf: vec![0.0; samples], pos: 0 }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }

    /// Delays `x` in place.
    fn process(&mut self, x: &mut [f32]) {
        if self.buf.is_empty() {
            return;
        }
        for s in x {
            std::mem::swap(&mut self.buf[self.pos], &mut *s);
            self.pos = (self.pos + 1) % self.buf.len();
        }
    }

    /// Adds a delayed copy of `src` into `dst`.
    /// Adds `src` (delayed, times `gain`) into `dst`. Unity gain adds exactly.
    fn mix_into(&mut self, src: &[f32], dst: &mut [f32], gain: f32) {
        let unity = gain == 1.0;
        if self.buf.is_empty() {
            if unity {
                dst.iter_mut().zip(src).for_each(|(d, s)| *d += s);
            } else {
                dst.iter_mut().zip(src).for_each(|(d, s)| *d += s * gain);
            }
            return;
        }
        for (d, s) in dst.iter_mut().zip(src) {
            *d += if unity { self.buf[self.pos] } else { self.buf[self.pos] * gain };
            self.buf[self.pos] = *s;
            self.pos = (self.pos + 1) % self.buf.len();
        }
    }
}

pub struct Channel {
    pub name: String,
    pub source: Source,
    /// Linear gain and pan the channel starts with (live values live in [`Controls`]).
    pub initial_volume: f32,
    pub initial_pan: f32,
    /// Gains applied at the end of the previous block, for click-free ramps.
    prev_gain: [f32; 2],
    /// Mixer insert index this channel feeds.
    pub insert: usize,
    buf: [Vec<f32>; 2],
    /// Delay compensation applied before the channel reaches its insert.
    delay: [DelayLine; 2],
    /// Keys currently held on a plugin, so they can be released when the timeline changes.
    held: [u64; 2],
    /// Settings for sampler sources.
    pub sampler: instrument::SamplerSettings,
    /// Engine sample rate (for envelope times).
    sample_rate: f32,
    /// Arpeggiator (off unless switched on in channel settings).
    pub arp: arp::Arp,
    /// Sampler channel filter state.
    filter: instrument::ChannelFilter,
    /// Plugin channels: the last key started, the current pitch bend (semitones) and a bend
    /// ramp in progress (from, to, samples done, total) for slide notes.
    last_key: u8,
    bend_now: f32,
    bend_ramp: Option<(f32, f32, u32, u32)>,
}

impl Channel {
    /// `volume` is FL's channel knob position (0.0..=1.0).
    pub fn new(name: String, source: Source, volume: f32, pan: f32, insert: usize) -> Self {
        Self::with_gain(name, source, gain::channel_volume(volume), pan, insert)
    }

    /// `initial_volume` is a linear gain.
    pub fn with_gain(name: String, source: Source, initial_volume: f32, pan: f32, insert: usize) -> Self {
        Self {
            name,
            source,
            initial_volume,
            initial_pan: pan,
            prev_gain: gain::pan_gains_tuple(initial_volume, pan),
            insert,
            buf: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            delay: Default::default(),
            held: [0; 2],
            sampler: Default::default(),
            sample_rate: 48_000.0,
            arp: Default::default(),
            filter: Default::default(),
            last_key: 60,
            bend_now: 0.0,
            bend_ramp: None,
        }
    }

    /// Plugin slide notes: pitch-bend messages along a ramp (about every 64 samples).
    fn tick_bend(&mut self, len: usize) {
        let Some((from, to, done, total)) = self.bend_ramp else { return };
        let Source::Plugin { midi, .. } = &mut self.source else {
            self.bend_ramp = None;
            return;
        };
        let range = self.sampler.bend_range.max(0.1);
        let mut t = 0usize;
        let mut d = done;
        while t < len {
            d = (d + 64).min(total);
            let semis = from + (to - from) * (d as f32 / total.max(1) as f32);
            self.bend_now = semis;
            let value = (8192.0 + (semis / range).clamp(-1.0, 1.0) * 8191.0).round() as u16;
            midi.push(MidiEvent::pitch_bend(t as u32, 0, value));
            if d >= total {
                self.bend_ramp = None;
                return;
            }
            t += 64;
        }
        self.bend_ramp = Some((from, to, d, total));
    }

    /// A note for this channel: goes through the arpeggiator when it's on.
    fn event(&mut self, kind: EventKind, offset: usize) {
        if self.arp.settings.on {
            match kind {
                EventKind::NoteOn { key, velocity, .. } => return self.arp.key(key, velocity.max(1)),
                EventKind::NoteOff { key } => return self.arp.key(key, 0),
                EventKind::Clip { .. } | EventKind::Slide { .. } => {}
            }
        }
        self.handle(kind, offset);
    }

    /// Plays this block's arpeggiator notes (before the source renders).
    fn tick_arp(&mut self, len: usize, samples_per_beat: f64) {
        if !self.arp.settings.on {
            return;
        }
        let mut out = [(0usize, 0u8, 0u8); 64];
        let n = self.arp.run(len, samples_per_beat, &mut out);
        for &(offset, key, velocity) in &out[..n] {
            let kind = if velocity > 0 { EventKind::NoteOn { key, velocity, expr: NoteExpr::default() } } else { EventKind::NoteOff { key } };
            self.handle(kind, offset.min(len.saturating_sub(1)));
        }
    }

    /// Stops the arpeggiator (transport stop, seek, or switched off).
    fn reset_arp(&mut self) {
        if let Some(key) = self.arp.reset() {
            self.handle(EventKind::NoteOff { key }, 0);
        }
    }

    /// Sends note-offs for every key still held on a plugin.
    fn release_held(&mut self) {
        if let Source::Plugin { midi, .. } = &mut self.source {
            for key in 0..128u8 {
                if self.held[(key / 64) as usize] & (1 << (key % 64)) != 0 {
                    midi.push(MidiEvent::note_off(0, 0, key));
                }
            }
        }
        self.held = [0; 2];
    }

    /// Processing delay of this channel's sound source.
    pub fn latency(&self) -> usize {
        match &self.source {
            Source::Plugin { plugin, .. } => plugin.latency_samples(),
            _ => 0,
        }
    }

    fn handle(&mut self, kind: EventKind, offset: usize) {
        match (&mut self.source, kind) {
            (Source::Synth(s), EventKind::NoteOn { key, velocity, expr }) => s.note_on_expr(key, gain::midi_velocity(velocity), offset, expr.pan, expr.mod_x, expr.mod_y),
            (Source::Synth(s), EventKind::NoteOff { key }) => s.note_off(key),
            (Source::Synth(s), EventKind::Slide { key, duration }) => s.slide_to(key, duration),
            (Source::Plugin { .. }, EventKind::Slide { key, duration }) => {
                let target = key as f32 - self.last_key as f32;
                self.bend_ramp = Some((self.bend_now, target, 0, duration.max(64)));
            }
            (Source::Sampler { voices, .. }, EventKind::Slide { key, duration }) => {
                let coeff = (-1.0 / (duration.max(64) as f64 / 4.0)).exp();
                for v in voices.iter_mut().filter(|v| v.fade.is_none() && v.stage != 3) {
                    v.step_target = v.base_step * 2f64.powf((key as f64 - v.key as f64) / 12.0);
                    v.step_coeff = coeff;
                }
            }
            (Source::MidiOut { out, midi_channel }, EventKind::NoteOn { key, velocity, .. }) => {
                out.send([0x90 | (*midi_channel & 0x0F), key & 0x7F, gain::midi_velocity(velocity)]);
            }
            (Source::MidiOut { out, midi_channel }, EventKind::NoteOff { key }) => out.send([0x80 | (*midi_channel & 0x0F), key & 0x7F, 0]),
            (Source::Sampler { sample, root_note, voices }, EventKind::NoteOn { key, velocity, expr }) => {
                let st = &self.sampler;
                if st.cut_itself {
                    for v in voices.iter_mut().filter(|v| v.fade.is_none()) {
                        v.fade = Some(DECLICK);
                    }
                }
                let step = 2f64.powf((key as f64 - *root_note as f64 + st.pitch as f64) / 12.0);
                let len = sample.len() as f64;
                let (pos, end, step) = if st.reverse { (len - 1.0, 0.0, -step) } else { (0.0, len, step) };
                start_voice(voices, key, pos, end, step, gain::velocity(velocity), offset, st.envelope);
                if let Some(v) = voices.last_mut() {
                    v.pan = expr.pan;
                }
            }
            (Source::Slicer { sample, slices, voices }, EventKind::NoteOn { key, velocity, expr }) => {
                let Some(&(start, end)) = (key as usize).checked_sub(SLICE_ROOT as usize).and_then(|i| slices.get(i)) else { return };
                let st = &self.sampler;
                if st.cut_itself {
                    for v in voices.iter_mut().filter(|v| v.fade.is_none()) {
                        v.fade = Some(DECLICK);
                    }
                }
                let step = 2f64.powf(st.pitch as f64 / 12.0);
                let (start, end) = (start.min(sample.len()) as f64, end.min(sample.len()) as f64);
                let (pos, end, step) = if st.reverse { (end - 1.0, start - 0.5, -step) } else { (start, end, step) };
                start_voice(voices, key, pos, end, step, gain::velocity(velocity), offset, st.envelope);
                if let Some(v) = voices.last_mut() {
                    v.pan = expr.pan;
                }
            }
            (Source::Sampler { voices, .. } | Source::Slicer { voices, .. }, EventKind::NoteOff { key }) if self.sampler.follow_note || self.sampler.envelope => {
                for v in voices.iter_mut().filter(|v| v.key == key && v.stage != 3) {
                    if self.sampler.envelope {
                        v.stage = 3;
                    } else if v.fade.is_none() {
                        v.fade = Some(DECLICK);
                    }
                }
            }
            (Source::AudioClip { sample, voices }, EventKind::Clip { offset: start, length, gain, fade_in, fade_out, reverse }) => {
                let end = (start + length).min(sample.len());
                if reverse {
                    // Reversed: the clip's region of the sample, read from its end to its start.
                    let (from, to) = (end as f64 - 1.0, start.min(end) as f64 - 0.5);
                    start_voice(voices, 0, from, to, -1.0, gain, offset, false);
                } else {
                    start_voice(voices, 0, start as f64, end as f64, 1.0, gain, offset, false);
                }
                if let Some(v) = voices.last_mut() {
                    v.clip_fade = Some(ClipFade { fade_in, fade_out, elapsed: 0, length: end.saturating_sub(start) as u32 });
                }
            }
            (Source::Plugin { midi, .. }, EventKind::NoteOn { key, velocity, .. }) => {
                // A new note starts unbent.
                if self.bend_now != 0.0 || self.bend_ramp.is_some() {
                    midi.push(MidiEvent::pitch_bend(offset as u32, 0, 8192));
                    self.bend_now = 0.0;
                    self.bend_ramp = None;
                }
                self.last_key = key;
                midi.push(MidiEvent::note_on(offset as u32, 0, key, gain::midi_velocity(velocity)));
                self.held[(key / 64) as usize & 1] |= 1 << (key % 64);
            }
            (Source::Plugin { midi, .. }, EventKind::NoteOff { key }) => {
                midi.push(MidiEvent::note_off(offset as u32, 0, key));
                self.held[(key / 64) as usize & 1] &= !(1 << (key % 64));
            }
            // Sampler channels play one-shot: the whole sample regardless of note length.
            _ => {}
        }
    }

    /// Hands this block to a helper-hosted plugin so it can work in parallel
    /// with everything else; [`Channel::render`] collects the result.
    fn begin(&mut self, len: usize) {
        if let Source::Plugin { plugin, midi } = &mut self.source
            && let PluginInstance::Remote(p) = plugin.as_mut() {
                p.begin(len, |_| None, midi);
                midi.clear();
            }
    }

    /// Renders `len` samples of this channel (pre-fader) into its buffer.
    fn render(&mut self, len: usize) {
        let [l, r] = &mut self.buf;
        l[..len].fill(0.0);
        r[..len].fill(0.0);
        match &mut self.source {
            Source::Sampler { sample, voices, .. } | Source::Slicer { sample, voices, .. } => {
                let (st, sr) = (self.sampler, self.sample_rate);
                voices.retain_mut(|v| render_voice(v, sample, &mut l[..len], &mut r[..len], &st, sr));
            }
            Source::AudioClip { sample, voices } => {
                let (st, sr) = (instrument::SamplerSettings::default(), self.sample_rate);
                voices.retain_mut(|v| render_voice(v, sample, &mut l[..len], &mut r[..len], &st, sr));
            }
            Source::Plugin { plugin, midi } => {
                let mut chans: [&mut [f32]; 2] = [&mut l[..len], &mut r[..len]];
                match plugin.as_mut() {
                    PluginInstance::Remote(p) => p.finish(&mut chans),
                    PluginInstance::Local(p) => {
                        p.process(&mut chans, midi);
                        midi.clear();
                    }
                }
            }
            Source::Synth(s) => s.render(&mut l[..len], &mut r[..len]),
            Source::MidiOut { .. } | Source::Silent => {}
        }
        if matches!(self.source, Source::Sampler { .. } | Source::Slicer { .. }) && self.sampler.filter != 0 {
            let [l, r] = &mut self.buf;
            self.filter.process(&self.sampler, &mut l[..len], &mut r[..len], self.sample_rate);
        }
        self.delay[0].process(&mut self.buf[0][..len]);
        self.delay[1].process(&mut self.buf[1][..len]);
    }
}

/// Equal-power fade-in/out of an audio clip voice, in output samples (smooth at both ends,
/// and overlapping fades crossfade without a dip in loudness).
#[derive(Clone, Copy)]
struct ClipFade {
    fade_in: u32,
    fade_out: u32,
    elapsed: u32,
    length: u32,
}

impl ClipFade {
    #[inline]
    fn next(&mut self) -> f32 {
        let t = self.elapsed;
        self.elapsed = self.elapsed.saturating_add(1);
        let a = if self.fade_in > 0 && t < self.fade_in { fade_curve(t as f32 / self.fade_in as f32) } else { 1.0 };
        let left = self.length.saturating_sub(t);
        let b = if self.fade_out > 0 && left < self.fade_out { fade_curve(left as f32 / self.fade_out as f32) } else { 1.0 };
        a * b
    }
}

/// The fade shape for 0..=1 progress: equal-power (sin), so fades sound and look smooth.
pub fn fade_curve(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

#[allow(clippy::too_many_arguments)]
fn start_voice(voices: &mut Vec<Voice>, key: u8, pos: f64, end: f64, step: f64, gain: f32, delay: usize, envelope: bool) {
    if voices.len() >= MAX_VOICES {
        // Steal the oldest voice that isn't already fading out.
        if let Some(v) = voices.iter_mut().find(|v| v.fade.is_none()) {
            v.fade = Some(DECLICK);
        }
        if voices.len() >= MAX_VOICES * 2 {
            voices.remove(0);
        }
    }
    voices.push(Voice { key, pan: 0.0, env: if envelope { 0.0 } else { 1.0 }, stage: if envelope { 0 } else { 2 }, pos, step, end, gain, delay, fade: None, clip_fade: None, base_step: step, step_target: step, step_coeff: 0.0 });
}

/// Mixes one voice into `l`/`r`. Returns false once the voice has finished.
/// Voices with a negative step play backwards (towards `end`, which is then 0).
fn render_voice(v: &mut Voice, s: &Sample, l: &mut [f32], r: &mut [f32], settings: &instrument::SamplerSettings, sample_rate: f32) -> bool {
    if s.is_empty() {
        return false;
    }
    let rate = |secs: f32| 1.0 / (secs.max(0.0005) * sample_rate);
    let start = v.delay.min(l.len());
    v.delay -= start;
    for i in start..l.len() {
        let finished = if v.step >= 0.0 { v.pos >= v.end } else { v.pos < v.end || v.pos < 0.0 };
        if finished {
            return false;
        }
        if settings.envelope {
            match v.stage {
                0 => {
                    v.env += rate(settings.attack);
                    if v.env >= 1.0 {
                        v.env = 1.0;
                        v.stage = 1;
                    }
                }
                1 => {
                    v.env -= rate(settings.decay) * (1.0 - settings.sustain).max(0.001);
                    if v.env <= settings.sustain {
                        v.env = settings.sustain;
                        v.stage = 2;
                    }
                }
                3 => {
                    v.env -= rate(settings.release);
                    if v.env <= 0.0 {
                        return false;
                    }
                }
                _ => {}
            }
        }
        if v.step_coeff > 0.0 {
            v.step = v.step_target + (v.step - v.step_target) * v.step_coeff;
        }
        let mut g = v.gain * v.env;
        if let Some(cf) = v.clip_fade.as_mut() {
            g *= cf.next();
        }
        if let Some(remaining) = v.fade.as_mut() {
            if *remaining == 0 {
                return false;
            }
            g *= *remaining as f32 / DECLICK as f32;
            *remaining -= 1;
        }
        let idx = (v.pos as usize).min(s.len() - 1);
        let frac = (v.pos - idx as f64).clamp(0.0, 1.0) as f32;
        let next = (idx + 1).min(s.len() - 1);
        let (gl, gr) = ((1.0 - v.pan).min(1.0), (1.0 + v.pan).min(1.0));
        l[i] += (s.left[idx] + (s.left[next] - s.left[idx]) * frac) * g * gl;
        r[i] += (s.right[idx] + (s.right[next] - s.right[idx]) * frac) * g * gr;
        v.pos += v.step;
    }
    true
}

pub enum Processor {
    Plugin(Box<PluginInstance>),
    /// One of KVNG Studio's adapters for FL's built-in effects.
    Native(Box<dyn native::NativeEffect>),
    /// One of KVNG Studio's own effects (EQ, reverb, delay, compressor).
    Builtin(Box<effects::Builtin>),
}

impl Processor {
    pub fn latency(&self) -> usize {
        match self {
            Processor::Plugin(p) => p.latency_samples(),
            Processor::Native(n) => n.latency(),
            Processor::Builtin(_) => 0,
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            Processor::Plugin(p) => {
                let mut chans: [&mut [f32]; 2] = [left, right];
                p.process(&mut chans, &[]);
            }
            Processor::Native(n) => n.process(left, right),
            Processor::Builtin(b) => b.process(left, right),
        }
    }
}

pub struct Effect {
    pub name: String,
    pub processor: Processor,
    /// 0.0 = dry, 1.0 = fully wet.
    pub mix: f32,
    /// Skipped entirely while bypassed.
    pub bypass: bool,
}

pub struct Insert {
    pub name: String,
    pub enabled: bool,
    pub effects: Vec<Effect>,
    /// Linear gain and pan the insert starts with (live values live in [`Controls`]).
    pub initial_volume: f32,
    pub initial_pan: f32,
    prev_gain: [f32; 2],
    /// Destination inserts; the master has none.
    pub routes_to: Vec<usize>,
    buf: [Vec<f32>; 2],
    dry: [Vec<f32>; 2],
    /// Send level (linear) per entry of `routes_to`; missing entries are 1.0.
    pub route_levels: Vec<f32>,
    /// Inserts whose compressor listens to this one (sidechain sends, post-fader).
    pub sidechain_to: Vec<usize>,
    /// Sidechain (key) input summed from other inserts this block.
    sc: [Vec<f32>; 2],
    sc_fed: bool,
    /// Delay compensation per entry of `routes_to`.
    route_delays: Vec<[DelayLine; 2]>,
    /// Built-in EQ and compressor (after the effect slots, before the fader).
    pub strip: channel_strip::ChannelStrip,
}

impl Insert {
    /// `volume` is an FL fader position (0..=16000) and `pan` FL's -6400..=6400.
    pub fn new(name: String, enabled: bool, volume: u32, pan: i32, routes_to: Vec<usize>) -> Self {
        Self::with_gain(name, enabled, gain::insert_volume(volume), pan as f32 / 6400.0, routes_to)
    }

    pub fn with_gain(name: String, enabled: bool, initial_volume: f32, initial_pan: f32, routes_to: Vec<usize>) -> Self {
        Self {
            name,
            enabled,
            effects: Vec::new(),
            initial_volume,
            initial_pan,
            prev_gain: gain::pan_gains_tuple(initial_volume, initial_pan),
            routes_to,
            buf: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            dry: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            route_levels: Vec::new(),
            sidechain_to: Vec::new(),
            sc: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            sc_fed: false,
            route_delays: Vec::new(),
            strip: channel_strip::ChannelStrip::new(channel_strip::StripSettings::default(), 44_100.0),
        }
    }

    pub fn latency(&self) -> usize {
        self.effects.iter().map(|fx| fx.processor.latency()).sum()
    }

    /// Runs the effect chain, then ramps to `gain` (left, right).
    fn process(&mut self, len: usize, gain: [f32; 2]) {
        {
            // Built-in EQ → compressor (keyed by sidechain sends, if any), then the effect slots.
            let [l, r] = &mut self.buf;
            // The key buffer is silent when nothing sends to it, so a compressor set to
            // listen to its sidechain then leaves the audio alone.
            let key = Some((&self.sc[0][..len], &self.sc[1][..len]));
            self.strip.process(&mut l[..len], &mut r[..len], key);
        }
        for fx in self.effects.iter_mut().filter(|fx| !fx.bypass) {
            let wet_only = fx.mix >= 0.999;
            if !wet_only {
                for c in 0..2 {
                    self.dry[c][..len].copy_from_slice(&self.buf[c][..len]);
                }
            }
            let [l, r] = &mut self.buf;
            fx.processor.process(&mut l[..len], &mut r[..len]);
            if !wet_only {
                for c in 0..2 {
                    for (w, d) in self.buf[c][..len].iter_mut().zip(&self.dry[c][..len]) {
                        *w = *d + (*w - *d) * fx.mix;
                    }
                }
            }
        }
        for (c, g) in gain.iter().enumerate() {
            ramp_gain(&mut self.buf[c][..len], self.prev_gain[c], *g);
        }
        self.prev_gain = gain;
    }
}

pub struct Engine {
    pub sample_rate: f64,
    pub channels: Vec<Channel>,
    /// Index 0 is the master.
    pub inserts: Vec<Insert>,
    /// Sorted by time.
    events: Vec<Event>,
    next_event: usize,
    /// Insert processing order: every insert comes before the inserts it feeds.
    order: Vec<usize>,
    pos: u64,
    /// Where the song ends, including a tail for releases.
    pub length: u64,
    /// Total delay from the timeline to the master output, in samples.
    pub latency: usize,
    /// Safety limiter state on the master output.
    limiter_gain: f32,
    controls: Arc<Controls>,
    /// For each insert, every insert that feeds it directly or indirectly (and vice versa);
    /// used so soloing an insert keeps its whole signal path audible.
    upstream: Vec<Vec<usize>>,
    downstream: Vec<Vec<usize>>,
    /// Transport state seen at the previous block.
    was_playing: bool,
    timeline_rx: timeline::TimelineReceiver,
    timeline_tx: Option<timeline::TimelineSender>,
    command_rx: command::CommandReceiver,
    command_tx: Option<command::CommandSender>,
    /// Browser preview: a sample and its playback position.
    preview: Option<(Arc<Sample>, usize)>,
    /// Metronome click: samples left, phase increment (pitch), current phase.
    click: Option<(usize, f32, f32)>,
    /// Input being monitored (recording input heard through the outputs).
    monitor: Option<Arc<monitor::MonitorRing>>,
    /// The mixer insert monitoring plays through (0 = straight to the output).
    monitor_insert: usize,
    /// Automation segments, sorted by start.
    automation: Vec<timeline::AutoSegment>,
    /// Metronome beat positions from the tempo map (empty: constant beat length).
    beats: Vec<(u64, bool)>,
    /// Stem export: each tapped insert's post-fader output, accumulated block by block.
    stems: Vec<(usize, [Vec<f32>; 2])>,
}

impl Engine {
    pub fn new(sample_rate: f64, mut channels: Vec<Channel>, mut inserts: Vec<Insert>, mut events: Vec<Event>, length: u64) -> Self {
        timeline::sort(&mut events);
        let (timeline_tx, timeline_rx) = timeline::link();
        let (command_tx, command_rx) = command::link();
        channels.reserve(MAX_CHANNELS.saturating_sub(channels.len()));
        for ch in &mut channels {
            ch.sample_rate = sample_rate as f32;
        }
        let n_inserts = inserts.len();
        for ins in &mut inserts {
            ins.effects.reserve(MAX_EFFECTS.saturating_sub(ins.effects.len()));
            // Routing edits on the audio thread reuse these allocations.
            ins.routes_to.reserve(n_inserts.saturating_sub(ins.routes_to.len()));
            ins.route_levels.resize(ins.routes_to.len(), 1.0);
            ins.route_levels.reserve(n_inserts.saturating_sub(ins.route_levels.len()));
            ins.sidechain_to.reserve(n_inserts.saturating_sub(ins.sidechain_to.len()));
            ins.strip = channel_strip::ChannelStrip::new(ins.strip.settings, sample_rate);
        }
        let order = processing_order(&inserts);
        let controls = Arc::new(Controls::new(
            sample_rate,
            length,
            (0..MAX_CHANNELS.max(channels.len()))
                .map(|i| match channels.get(i) {
                    Some(c) => {
                        let strip = Strip::new(c.initial_volume, c.initial_pan, false);
                        strip.set_route(c.insert);
                        strip
                    }
                    None => Strip::new(1.0, 0.0, false),
                })
                .collect(),
            inserts.iter().map(|i| Strip::new(i.initial_volume, i.initial_pan, !i.enabled)).collect(),
        ));
        let (upstream, downstream) = signal_paths(&inserts);
        let mut engine = Self {
            sample_rate,
            channels,
            inserts,
            events,
            next_event: 0,
            order,
            pos: 0,
            length,
            latency: 0,
            limiter_gain: 1.0,
            controls,
            upstream,
            downstream,
            was_playing: false,
            timeline_rx,
            timeline_tx: Some(timeline_tx),
            command_rx,
            command_tx: Some(command_tx),
            preview: None,
            click: None,
            monitor: None,
            monitor_insert: 0,
            automation: Vec::new(),
            stems: Vec::new(),
            beats: Vec::new(),
        };
        engine.compensate_latency();
        engine
    }

    /// Plugin delay compensation: every input to an insert is delayed to match its
    /// slowest input, so everything arrives at the master in time with everything else.
    fn compensate_latency(&mut self) {
        let n = self.inserts.len();
        let mut input = vec![0usize; n];
        for ch in &self.channels {
            if ch.insert < n {
                input[ch.insert] = input[ch.insert].max(ch.latency());
            }
        }
        let mut output = vec![0usize; n];
        for &i in &self.order {
            output[i] = input[i] + self.inserts[i].latency();
            for &d in &self.inserts[i].routes_to {
                if d < n && d != i {
                    input[d] = input[d].max(output[i]);
                }
            }
        }
        for ch in &mut self.channels {
            if ch.insert < n {
                let d = input[ch.insert] - ch.latency();
                ch.delay = [DelayLine::new(d), DelayLine::new(d)];
            }
        }
        for (i, insert) in self.inserts.iter_mut().enumerate().take(n) {
            let delays = insert
                .routes_to
                .iter()
                .map(|&d| {
                    let samples = if d < n && d != i { input[d].saturating_sub(output[i]) } else { 0 };
                    [DelayLine::new(samples), DelayLine::new(samples)]
                })
                .collect();
            insert.route_delays = delays;
        }
        self.latency = output[0];
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.length
    }

    /// Jumps to `pos` samples, silencing anything playing.
    pub fn seek(&mut self, pos: u64) {
        self.pos = pos;
        self.limiter_gain = 1.0;
        for ch in &mut self.channels {
            ch.delay.iter_mut().for_each(DelayLine::clear);
            ch.held = [0; 2];
            ch.arp.reset();
            ch.bend_ramp = None;
            if ch.bend_now != 0.0 {
                ch.bend_now = 0.0;
                if let Source::Plugin { midi, .. } = &mut ch.source {
                    midi.push(MidiEvent::pitch_bend(0, 0, 8192));
                }
            }
        }
        for ins in &mut self.inserts {
            ins.route_delays.iter_mut().flatten().for_each(DelayLine::clear);
        }
        self.next_event = self.events.partition_point(|e| e.time < pos);
        for ch in &mut self.channels {
            match &mut ch.source {
                Source::Sampler { voices, .. } | Source::AudioClip { voices, .. } | Source::Slicer { voices, .. } => voices.clear(),
                Source::Plugin { midi, .. } => {
                    midi.clear();
                    midi.push(MidiEvent { sample_offset: 0, bytes: [0xB0, 123, 0], len: 3 }); // all notes off
                }
                Source::Synth(s) => s.all_notes_off(),
                Source::MidiOut { out, midi_channel } => out.send([0xB0 | (*midi_channel & 0x0F), 123, 0]),
                Source::Silent => {}
            }
        }
    }

    /// The sender the UI uses to replace the timeline while playing (available once).
    pub fn take_timeline_sender(&mut self) -> Option<timeline::TimelineSender> {
        self.timeline_tx.take()
    }

    /// The sender the UI uses to add channels and effects while running (available once).
    pub fn take_command_sender(&mut self) -> Option<command::CommandSender> {
        self.command_tx.take()
    }

    /// Applies queued structural changes. Returns true if latency compensation must be redone.
    fn apply_commands(&mut self) -> bool {
        use command::{Command, Garbage};
        let mut structural = false;
        let mut reroute = false;
        while let Ok(cmd) = self.command_rx.rx.try_recv() {
            let garbage = match cmd {
                Command::AddChannel(mut ch) => {
                    if self.channels.len() < self.channels.capacity() {
                        ch.sample_rate = self.sample_rate as f32;
                        self.channels.push(*ch);
                        structural = true;
                        None
                    } else {
                        Some(Garbage::Channel(ch))
                    }
                }
                Command::ReplaceSource { channel, source } => match self.channels.get_mut(channel) {
                    Some(ch) => {
                        let old = std::mem::replace(&mut ch.source, *source);
                        ch.held = [0; 2];
                        structural = true;
                        Some(Garbage::Source(Box::new(old)))
                    }
                    None => Some(Garbage::Source(source)),
                },
                Command::InsertEffect { insert, index, effect } => match self.inserts.get_mut(insert) {
                    Some(ins) if ins.effects.len() < ins.effects.capacity() => {
                        let at = index.min(ins.effects.len());
                        ins.effects.insert(at, *effect);
                        structural = true;
                        None
                    }
                    _ => Some(Garbage::Effect(effect)),
                },
                Command::RemoveEffect { insert, index } => self.inserts.get_mut(insert).filter(|i| index < i.effects.len()).map(|ins| {
                    structural = true;
                    Garbage::Effect(Box::new(ins.effects.remove(index)))
                }),
                Command::Preview { channel, key, velocity } => {
                    if let Some(ch) = self.channels.get_mut(channel) {
                        let kind = if velocity > 0 { EventKind::NoteOn { key, velocity, expr: NoteExpr::default() } } else { EventKind::NoteOff { key } };
                        ch.event(kind, 0);
                    }
                    None
                }
                Command::PreviewSample(sample) => {
                    let position = if sample.is_some() { 0 } else { control::NO_PREVIEW };
                    self.controls.preview_position.store(position, Ordering::Relaxed);
                    let old = std::mem::replace(&mut self.preview, sample.map(|s| (s, 0)));
                    old.map(|(s, _)| Garbage::Sample(s))
                }
                Command::SetMonitor(ring, insert) => {
                    self.monitor_insert = insert;
                    std::mem::replace(&mut self.monitor, ring).map(Garbage::Monitor)
                }
                Command::SetArp { channel, settings } => {
                    if let Some(ch) = self.channels.get_mut(channel) {
                        let was_on = ch.arp.settings.on;
                        ch.arp.settings = settings;
                        if was_on && !settings.on {
                            ch.reset_arp();
                        }
                    }
                    None
                }
                Command::SetStrip { insert, settings } => {
                    if let Some(ins) = self.inserts.get_mut(insert) {
                        ins.strip.set(*settings);
                    }
                    None
                }
                Command::SetBuiltinEffect { insert, index, params } => {
                    if let Some(Processor::Builtin(b)) = self.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(index)).map(|e| &mut e.processor) {
                        b.set_params(*params);
                    }
                    None
                }
                Command::SetControl { target, value } => {
                    self.apply_target(target, value.clamp(0.0, 1.0));
                    None
                }
                Command::SetEffectBypass { insert, index, bypass } => {
                    if let Some(fx) = self.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(index)) {
                        fx.bypass = bypass;
                    }
                    None
                }
                Command::SetRoutes { insert, routes, levels } => {
                    if insert > 0 && let Some(ins) = self.inserts.get_mut(insert) {
                        // Reuse the existing allocations (reserved at startup).
                        ins.routes_to.clear();
                        ins.route_levels.clear();
                        for (k, &d) in routes.iter().enumerate() {
                            if d != insert {
                                ins.routes_to.push(d);
                                ins.route_levels.push(levels.get(k).copied().unwrap_or(1.0));
                            }
                        }
                        reroute = true;
                    }
                    None
                }
                Command::SetSidechains { insert, targets } => {
                    if let Some(ins) = self.inserts.get_mut(insert) {
                        ins.sidechain_to.clear();
                        ins.sidechain_to.extend(targets.iter().copied().filter(|&d| d != insert));
                        reroute = true;
                    }
                    None
                }
                Command::SetSampler { channel, settings } => {
                    if let Some(ch) = self.channels.get_mut(channel) {
                        ch.sampler = settings;
                    }
                    None
                }
                Command::SetSynth { channel, params } => {
                    if let Some(Source::Synth(s)) = self.channels.get_mut(channel).map(|c| &mut c.source) {
                        s.params = *params;
                    }
                    None
                }
                Command::SetEffectMix { insert, index, mix } => {
                    if let Some(fx) = self.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(index)) {
                        fx.mix = mix.clamp(0.0, 1.0);
                    }
                    None
                }
            };
            if let Some(g) = garbage {
                let _ = self.command_rx.garbage.send(g);
            }
        }
        if reroute {
            self.order = processing_order(&self.inserts);
            let (up, down) = signal_paths(&self.inserts);
            self.upstream = up;
            self.downstream = down;
        }
        structural || reroute
    }

    /// Switches to a new timeline at the current position without stopping.
    fn swap_timeline(&mut self, mut new: timeline::Timeline) {
        let old = std::mem::replace(&mut self.events, new.events);
        std::mem::swap(&mut self.automation, &mut new.automation);
        let new_beats = std::mem::take(&mut new.beats);
        self.length = new.length;
        self.controls.set_length(new.length);
        self.next_event = self.events.partition_point(|e| e.time < self.pos);
        for ch in &mut self.channels {
            ch.release_held();
        }
        // Free the old list on the UI thread, not here.
        let old_beats = std::mem::replace(&mut self.beats, new_beats);
        let _ = self.timeline_rx.garbage.send(timeline::Timeline { events: old, length: 0, automation: new.automation, beats: old_beats });
    }

    /// Shared transport, mixer and meter state for the UI.
    pub fn controls(&self) -> Arc<Controls> {
        self.controls.clone()
    }

    /// Fills `left`/`right` with the next samples, following the transport in [`Controls`].
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let started = Instant::now();
        while let Ok(t) = self.timeline_rx.rx.try_recv() {
            self.swap_timeline(t);
        }
        if self.apply_commands() {
            self.compensate_latency();
        }
        if let Some(pos) = self.controls.take_seek() {
            self.seek(pos);
        }
        let playing = self.controls.is_playing();
        if self.was_playing && !playing {
            // Stopping: cut everything so nothing hangs when playback resumes.
            self.seek(self.pos);
        }
        self.was_playing = playing;
        let counting_in = playing && self.controls.count_in_left() > 0;
        if !playing || counting_in {
            // Stopped: the timeline stays put, but channels and effects keep running so
            // note previews and effect tails are heard (as in FL Studio).
            let mut done = 0;
            while done < left.len() {
                let len = MAX_BLOCK.min(left.len() - done);
                self.process_block(&mut left[done..done + len], &mut right[done..done + len], false);
                done += len;
            }
            if counting_in {
                self.count_in(left, right);
            }
            let budget = left.len() as f64 / self.sample_rate;
            self.controls.record_load((started.elapsed().as_secs_f64() / budget) as f32);
            return;
        }

        let mut done = 0;
        while done < left.len() {
            let mut len = MAX_BLOCK.min(left.len() - done);
            if let Some((start, end)) = self.controls.loop_range() {
                if self.pos >= end || self.pos < start.min(end) && self.pos + len as u64 > end {
                    self.seek(start);
                } else if self.pos + len as u64 > end {
                    len = (end - self.pos) as usize;
                }
            }
            if self.pos >= self.length {
                left[done..].fill(0.0);
                right[done..].fill(0.0);
                self.controls.stop();
                break;
            }
            self.process_block(&mut left[done..done + len], &mut right[done..done + len], true);
            done += len;
        }
        self.controls.set_position(self.pos);
        let budget = left.len() as f64 / self.sample_rate;
        self.controls.record_load((started.elapsed().as_secs_f64() / budget) as f32);
    }

    /// Starts capturing the post-fader output of `inserts` (offline stem export).
    /// Capturing allocates, so it's only for offline rendering.
    pub fn capture_stems(&mut self, inserts: &[usize]) {
        self.stems = inserts.iter().filter(|&&i| i < self.inserts.len()).map(|&i| (i, [Vec::new(), Vec::new()])).collect();
    }

    /// Takes the captured stems: (insert index, left, right).
    pub fn take_stems(&mut self) -> Vec<(usize, [Vec<f32>; 2])> {
        std::mem::take(&mut self.stems)
    }

    /// Moves automated controls to their values at the current position.
    fn apply_automation(&mut self) {
        let pos = self.pos;
        // Segments are sorted by start, so for each target the latest one that has
        // begun wins; after a clip ends its last value holds (as in FL Studio).
        for i in 0..self.automation.len() {
            let seg = self.automation[i];
            if seg.start > pos {
                break;
            }
            self.apply_target(seg.target, seg.value_at(pos).clamp(0.0, 1.0));
        }
    }

    /// Sets an automatable control to `v` (0..=1 on its knob/fader).
    fn apply_target(&mut self, target: view::AutomationTarget, v: f32) {
        use view::AutomationTarget::*;
        match target {
            ChannelVolume { channel } => {
                if let Some(s) = self.controls.channels.get(channel) {
                    s.volume.store(gain::channel_volume(v));
                }
            }
            ChannelPan { channel } => {
                if let Some(s) = self.controls.channels.get(channel) {
                    s.pan.store(v * 2.0 - 1.0);
                }
            }
            InsertVolume { insert } => {
                if let Some(s) = self.controls.inserts.get(insert) {
                    s.volume.store(gain::fl_volume(v));
                }
            }
            InsertPan { insert } => {
                if let Some(s) = self.controls.inserts.get(insert) {
                    s.pan.store(v * 2.0 - 1.0);
                }
            }
            SynthCutoff { channel } => {
                if let Some(Source::Synth(s)) = self.channels.get_mut(channel).map(|c| &mut c.source) {
                    s.params.cutoff = 20.0 * 1000f32.powf(v);
                }
            }
            SynthResonance { channel } => {
                if let Some(Source::Synth(s)) = self.channels.get_mut(channel).map(|c| &mut c.source) {
                    s.params.resonance = v;
                }
            }
            ChannelPluginParam { channel, param } => {
                if let Some(Source::Plugin { plugin, .. }) = self.channels.get_mut(channel).map(|c| &mut c.source) {
                    plugin.set_param(param, v);
                }
            }
            InsertStrip { insert, param } => {
                if let Some(ins) = self.inserts.get_mut(insert) {
                    let mut settings = ins.strip.settings;
                    settings.set_normalized(param as usize, v);
                    ins.strip.set(settings);
                }
            }
            EffectParam { insert, slot, param } => match self.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(slot)).map(|e| &mut e.processor) {
                Some(Processor::Plugin(p)) => p.set_param(param, v),
                Some(Processor::Builtin(b)) => {
                    let mut params = b.params.clone();
                    params.set_normalized(param as usize, v);
                    b.set_params(params);
                }
                _ => {}
            },
        }
    }

    /// Renders one block. With `advance` false (stopped), no timeline events fire and the
    /// playhead doesn't move.
    fn process_block(&mut self, left: &mut [f32], right: &mut [f32], advance: bool) {
        let len = left.len();
        if advance && !self.automation.is_empty() {
            self.apply_automation();
        }
        let spb = self.controls.samples_per_beat.load();
        for ins in &mut self.inserts {
            for fx in &mut ins.effects {
                if let Processor::Builtin(b) = &mut fx.processor {
                    b.samples_per_beat = spb;
                }
            }
        }
        let block_end = self.pos + len as u64;

        while advance && let Some(ev) = self.events.get(self.next_event).filter(|e| e.time < block_end) {
            let offset = ev.time.saturating_sub(self.pos) as usize;
            // Note-offs always go through so muting a track never leaves notes hanging.
            let muted = self.controls.is_track_muted(ev.track as usize) && !matches!(ev.kind, EventKind::NoteOff { .. });
            if !muted && let Some(ch) = self.channels.get_mut(ev.channel) {
                ch.event(ev.kind, offset);
            }
            self.next_event += 1;
        }

        for ins in &mut self.inserts {
            for c in 0..2 {
                ins.buf[c][..len].fill(0.0);
            }
            if ins.sc_fed {
                for c in 0..2 {
                    ins.sc[c][..len].fill(0.0);
                }
                ins.sc_fed = false;
            }
        }

        // Arpeggiators add their notes, then helper-hosted plugins start (in parallel).
        let spb = self.controls.samples_per_beat.load().max(1.0) as f64;
        for ch in &mut self.channels {
            ch.tick_arp(len, spb);
            ch.tick_bend(len);
        }
        for ch in &mut self.channels {
            ch.begin(len);
        }
        for (idx, ch) in self.channels.iter_mut().enumerate() {
            ch.render(len);
            let strip = &self.controls.channels[idx];
            let gain = if strip.is_muted() { [0.0; 2] } else { gain::pan_gains_tuple(strip.volume.load(), strip.pan.load()) };
            for (c, g) in gain.iter().enumerate() {
                ramp_gain(&mut ch.buf[c][..len], ch.prev_gain[c], *g);
            }
            ch.prev_gain = gain;
            strip.peak_l.raise(peak(&ch.buf[0][..len]));
            strip.peak_r.raise(peak(&ch.buf[1][..len]));
            if let Some(ins) = self.inserts.get_mut(strip.route()) {
                for c in 0..2 {
                    for (o, s) in ins.buf[c][..len].iter_mut().zip(&ch.buf[c][..len]) {
                        *o += s;
                    }
                }
            }
        }

        // Input monitoring through a mixer track: the input joins that track like a channel.
        if self.monitor_insert > 0
            && let (Some(ring), Some(ins)) = (&self.monitor, self.inserts.get_mut(self.monitor_insert))
        {
            let [l, r] = &mut ins.buf;
            ring.mix_into(&mut l[..len], &mut r[..len], 1.0);
        }
        let any_solo = self.controls.any_insert_soloed();
        for oi in 0..self.order.len() {
            let i = self.order[oi];
            let strip = &self.controls.inserts[i];
            let audible = !strip.is_muted() && (!any_solo || i == 0 || self.on_soloed_path(i));
            let gain = if audible { gain::pan_gains_tuple(strip.volume.load(), strip.pan.load()) } else { [0.0; 2] };
            self.inserts[i].process(len, gain);
            strip.gain_reduction.store(self.inserts[i].strip.gain_reduction);
            strip.comp_input_db.store(self.inserts[i].strip.comp_input_db);
            strip.comp_output_db.store(self.inserts[i].strip.comp_output_db);
            strip.comp_detector_db.store(self.inserts[i].strip.comp_detector_db);
            strip.peak_l.raise(peak(&self.inserts[i].buf[0][..len]));
            strip.peak_r.raise(peak(&self.inserts[i].buf[1][..len]));
            for d in 0..self.inserts[i].routes_to.len() {
                let dest = self.inserts[i].routes_to[d];
                if dest == i || dest >= self.inserts.len() {
                    continue;
                }
                let level = self.inserts[i].route_levels.get(d).copied().unwrap_or(1.0);
                let (src, dst) = two_mut(&mut self.inserts, i, dest);
                for c in 0..2 {
                    src.route_delays[d][c].mix_into(&src.buf[c][..len], &mut dst.buf[c][..len], level);
                }
            }
            for s in 0..self.inserts[i].sidechain_to.len() {
                let dest = self.inserts[i].sidechain_to[s];
                if dest == i || dest >= self.inserts.len() {
                    continue;
                }
                let (src, dst) = two_mut(&mut self.inserts, i, dest);
                for c in 0..2 {
                    dst.sc[c][..len].iter_mut().zip(&src.buf[c][..len]).for_each(|(k, x)| *k += x);
                }
                dst.sc_fed = true;
            }
        }

        if advance {
            for (i, buf) in &mut self.stems {
                buf[0].extend_from_slice(&self.inserts[*i].buf[0][..len]);
                buf[1].extend_from_slice(&self.inserts[*i].buf[1][..len]);
            }
        }
        let master = &self.inserts[0];
        left.copy_from_slice(&master.buf[0][..len]);
        right.copy_from_slice(&master.buf[1][..len]);
        if self.monitor_insert == 0
            && let Some(ring) = &self.monitor
        {
            ring.mix_into(left, right, 1.0);
        }
        if let Some((sample, pos)) = &mut self.preview {
            let n = len.min(sample.len().saturating_sub(*pos));
            for i in 0..n {
                left[i] += sample.left[*pos + i];
                right[i] += sample.right[*pos + i];
            }
            *pos += n;
            self.controls.preview_position.store(*pos as u64, Ordering::Relaxed);
            if *pos >= sample.len() {
                self.controls.preview_position.store(control::NO_PREVIEW, Ordering::Relaxed);
                let (s, _) = self.preview.take().unwrap();
                let _ = self.command_rx.garbage.send(command::Garbage::Sample(s));
            }
        }
        if advance && self.controls.metronome.load(Ordering::Relaxed) {
            self.metronome(left, right);
        }
        self.limit(left, right);
        if advance {
            self.pos = block_end;
        }
    }

    /// Adds a click on each beat in this block (higher pitch on the bar's first beat).
    /// Count-in clicks before recording: the song waits while these play.
    fn count_in(&mut self, left: &mut [f32], right: &mut [f32]) {
        let spb = self.controls.samples_per_beat.load().max(1.0) as u64;
        let per_bar = self.controls.beats_per_bar.load(Ordering::Relaxed).max(1) as u64;
        let (left_samples, total) = (self.controls.count_in_left(), self.controls.count_in_total());
        let elapsed = total.saturating_sub(left_samples);
        for i in 0..left.len() as u64 {
            let t = elapsed + i;
            if t < total && t % spb == 0 {
                let accent = (t / spb).is_multiple_of(per_bar);
                let freq = if accent { 1760.0 } else { 1320.0 };
                self.click = Some(((0.03 * self.sample_rate as f32) as usize, freq / self.sample_rate as f32, 0.0));
            }
            self.render_click(left, right, i as usize);
        }
        self.controls.consume_count_in(left.len() as u64);
    }

    fn render_click(&mut self, left: &mut [f32], right: &mut [f32], i: usize) {
        if let Some((left_n, inc, phase)) = &mut self.click {
            let total = 0.03 * self.sample_rate as f32;
            let env = *left_n as f32 / total;
            let v = (*phase * std::f32::consts::TAU).sin() * env * env * 0.35 * self.controls.metronome_gain.load();
            left[i] += v;
            right[i] += v;
            *phase = (*phase + *inc).fract();
            *left_n -= 1;
            if *left_n == 0 {
                self.click = None;
            }
        }
    }

    fn metronome(&mut self, left: &mut [f32], right: &mut [f32]) {
        const CLICK_LEN: f32 = 0.03;
        // With tempo or meter changes the timeline lists every beat (and which are accents).
        if !self.beats.is_empty() {
            let mut k = self.beats.partition_point(|b| b.0 < self.pos);
            for i in 0..left.len() {
                let t = self.pos + i as u64;
                while let Some(&(at, accent)) = self.beats.get(k).filter(|b| b.0 <= t) {
                    if at == t {
                        let freq = if accent { 1760.0 } else { 1320.0 };
                        self.click = Some(((CLICK_LEN * self.sample_rate as f32) as usize, freq / self.sample_rate as f32, 0.0));
                    }
                    k += 1;
                }
                self.render_click(left, right, i);
            }
            return;
        }
        let spb = self.controls.samples_per_beat.load().max(1.0) as f64;
        let per_bar = self.controls.beats_per_bar.load(Ordering::Relaxed).max(1) as u64;
        let len = left.len();
        for i in 0..len {
            let t = (self.pos + i as u64) as f64;
            let beat = (t / spb).floor();
            if (t - beat * spb) < 1.0 {
                let accent = (beat as u64).is_multiple_of(per_bar);
                let freq = if accent { 1760.0 } else { 1320.0 };
                self.click = Some(((CLICK_LEN * self.sample_rate as f32) as usize, freq / self.sample_rate as f32, 0.0));
            }
            self.render_click(left, right, i);
        }
    }

    /// True when insert `i` is soloed or carries/receives the signal of a soloed insert.
    fn on_soloed_path(&self, i: usize) -> bool {
        let soloed = |j: &usize| self.controls.inserts[*j].is_soloed();
        self.controls.inserts[i].is_soloed() || self.upstream[i].iter().any(soloed) || self.downstream[i].iter().any(soloed)
    }

    /// Fast-attack, slow-release peak limiter at -0.3 dBFS so nothing clips.
    /// (FL-native effects such as Fruity Limiter aren't implemented yet.)
    fn limit(&mut self, left: &mut [f32], right: &mut [f32]) {
        const CEILING: f32 = 0.966;
        let release = (-1.0 / (0.1 * self.sample_rate as f32)).exp();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let peak = l.abs().max(r.abs());
            let target = if peak * self.limiter_gain > CEILING { CEILING / peak } else { 1.0 };
            self.limiter_gain = if target < self.limiter_gain {
                target
            } else {
                target + (self.limiter_gain - target) * release
            };
            *l = (*l * self.limiter_gain).clamp(-1.0, 1.0);
            *r = (*r * self.limiter_gain).clamp(-1.0, 1.0);
        }
    }

    /// Renders the whole song offline into stereo buffers.
    /// The overall output latency is trimmed so the result lines up with the timeline.
    pub fn render(&mut self) -> [Vec<f32>; 2] {
        self.controls.play();
        self.was_playing = true;
        let total = self.length.saturating_sub(self.pos) as usize;
        let mut out = [vec![0.0; total + self.latency], vec![0.0; total + self.latency]];
        let [l, r] = &mut out;
        self.process(l, r);
        for ch in &mut out {
            ch.drain(..self.latency);
        }
        out
    }
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Multiplies `buf` by a gain ramping linearly from `from` to `to` (no zipper noise).
fn ramp_gain(buf: &mut [f32], from: f32, to: f32) {
    if (from - to).abs() < 1e-6 {
        if (to - 1.0).abs() > 1e-6 {
            buf.iter_mut().for_each(|s| *s *= to);
        }
        return;
    }
    let step = (to - from) / buf.len().max(1) as f32;
    for (i, s) in buf.iter_mut().enumerate() {
        *s *= from + step * (i + 1) as f32;
    }
}

/// Transitive feeders (`upstream`) and destinations (`downstream`) of each insert.
fn signal_paths(inserts: &[Insert]) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let n = inserts.len();
    let reach = |start: usize, next: &dyn Fn(usize) -> Vec<usize>| {
        let mut seen = vec![false; n];
        let mut stack = next(start);
        let mut out = Vec::new();
        while let Some(j) = stack.pop() {
            if j < n && j != start && !seen[j] {
                seen[j] = true;
                out.push(j);
                stack.extend(next(j));
            }
        }
        out
    };
    let feeds = |j: usize| inserts[j].routes_to.clone();
    let fed_by = |j: usize| (0..n).filter(|&k| inserts[k].routes_to.contains(&j)).collect::<Vec<_>>();
    let downstream = (0..n).map(|i| reach(i, &feeds)).collect();
    let upstream = (0..n).map(|i| reach(i, &fed_by)).collect();
    (upstream, downstream)
}

fn two_mut<T>(v: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    assert_ne!(a, b);
    if a < b {
        let (x, y) = v.split_at_mut(b);
        (&mut x[a], &mut y[0])
    } else {
        let (x, y) = v.split_at_mut(a);
        (&mut y[0], &mut x[b])
    }
}

/// Orders inserts so each is processed before every insert it sends to.
/// Inserts caught in a routing loop are appended at the end (their loop is broken).
fn processing_order(inserts: &[Insert]) -> Vec<usize> {
    let n = inserts.len();
    let mut incoming = vec![0usize; n];
    // Sidechain sends count too: a key source must be processed before its target.
    let edges = |ins: &Insert| ins.routes_to.iter().chain(&ins.sidechain_to).copied().collect::<Vec<_>>();
    for ins in inserts {
        for d in edges(ins) {
            if d < n {
                incoming[d] += 1;
            }
        }
    }
    let mut ready: Vec<usize> = (0..n).filter(|&i| incoming[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = ready.pop() {
        order.push(i);
        for d in edges(&inserts[i]) {
            if d < n && d != i {
                incoming[d] -= 1;
                if incoming[d] == 0 {
                    ready.push(d);
                }
            }
        }
    }
    for i in 0..n {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Delays its input by a fixed amount and reports that as latency.
    struct Delay(DelayLine, DelayLine, usize);

    impl native::NativeEffect for Delay {
        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            self.0.process(left);
            self.1.process(right);
        }
        fn latency(&self) -> usize {
            self.2
        }
    }

    fn click_channel(insert: usize) -> Channel {
        let mut left = vec![0.0; 64];
        left[0] = 1.0;
        let sample = Arc::new(Sample { right: left.clone(), left });
        Channel::new("click".into(), Source::Sampler { sample, root_note: 60, voices: Vec::new() }, 0.781_25, 0.0, insert)
    }

    #[test]
    fn channels_and_effects_can_be_added_while_running() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let one = Insert::new("1".into(), true, 12800, 0, vec![0]);
        let mut engine = Engine::new(48_000.0, vec![], vec![master, one], vec![], 48_000);
        let commands = engine.take_command_sender().unwrap();
        let timeline = engine.take_timeline_sender().unwrap();
        let controls = engine.controls();
        controls.play();

        // New channel 0 routed to insert 1, plus a note for it.
        controls.channels[0].set_route(1);
        commands.send(command::Command::AddChannel(Box::new(click_channel(1)))).unwrap();
        let note = Event { time: 100, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        timeline.send(timeline::Timeline { events: vec![note], length: 48_000, automation: vec![], beats: vec![] });
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        assert_eq!(l.iter().position(|s| s.abs() > 0.01), Some(100), "the new channel plays");

        // An effect that adds latency is picked up by delay compensation.
        let fx = Effect { name: "delay".into(), processor: Processor::Native(Box::new(Delay(DelayLine::new(64), DelayLine::new(64), 64))), mix: 1.0, bypass: false };
        commands.send(command::Command::InsertEffect { insert: 1, index: 0, effect: Box::new(fx) }).unwrap();
        engine.process(&mut l, &mut r);
        assert_eq!(engine.latency, 64);
        commands.send(command::Command::RemoveEffect { insert: 1, index: 0 }).unwrap();
        engine.process(&mut l, &mut r);
        assert_eq!(engine.latency, 0);
    }

    #[test]
    fn previews_play_while_stopped_without_moving_the_playhead() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let one = Insert::new("1".into(), true, 12800, 0, vec![0]);
        let mut engine = Engine::new(48_000.0, vec![click_channel(1)], vec![master, one], vec![], 48_000);
        let commands = engine.take_command_sender().unwrap();
        commands.send(command::Command::Preview { channel: 0, key: 60, velocity: 100 }).unwrap();
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        assert!(l[0].abs() > 0.01, "preview is heard while stopped");
        assert_eq!(engine.position(), 0);
    }

    fn dc_channel(level: f32, insert: usize) -> Channel {
        let sample = Arc::new(Sample { left: vec![level; 48_000], right: vec![level; 48_000] });
        Channel::new("dc".into(), Source::Sampler { sample, root_note: 60, voices: Vec::new() }, 1.0, 0.0, insert)
    }

    fn note_at(time: u64, channel: usize) -> Event {
        Event { time, channel, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 128, expr: NoteExpr::default() } }
    }

    #[test]
    fn metronome_follows_the_beat_list() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let mut engine = Engine::new(48_000.0, vec![], vec![master], vec![], 48_000);
        let sender = engine.take_timeline_sender().unwrap();
        let controls = engine.controls();
        controls.metronome.store(true, Ordering::Relaxed);
        // Accent at 0, plain clicks at 3000 and 6500, accent at 10 000 (clicks last 1440 samples).
        sender.send(timeline::Timeline { events: vec![], length: 48_000, automation: vec![], beats: vec![(0, true), (3000, false), (6500, false), (10_000, true)] });
        controls.play();
        let (mut l, mut r) = (vec![0.0; 12_000], vec![0.0; 12_000]);
        engine.process(&mut l, &mut r);
        let onsets: Vec<usize> = (0..l.len()).filter(|&i| l[i].abs() > 1e-4 && (i == 0 || l[i - 1].abs() <= 1e-4) && (i < 3 || l[i - 3..i].iter().all(|v| v.abs() <= 1e-4))).collect();
        for at in [3000, 6500, 10_000] {
            assert!(onsets.iter().any(|&o| (at..at + 4).contains(&o)), "click near {at}: {onsets:?}");
        }
        assert!(!onsets.iter().any(|&o| (4500..6400).contains(&o)), "no clicks between beats: {onsets:?}");
    }

    #[test]
    fn monitoring_can_play_through_a_mixer_track() {
        let run = |muted: bool| {
            let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
            let track = Insert::new("vox".into(), true, 12800, 0, vec![0]);
            let mut engine = Engine::new(48_000.0, vec![], vec![master, track], vec![], 48_000);
            let commands = engine.take_command_sender().unwrap();
            let controls = engine.controls();
            controls.inserts[1].mute.store(muted, Ordering::Relaxed);
            let ring = Arc::new(monitor::MonitorRing::default());
            commands.send(command::Command::SetMonitor(Some(ring.clone()), 1)).unwrap();
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            let mut peak = 0.0f32;
            for block in 0..6 {
                ring.push(&[0.2; 1024], 2);
                engine.process(&mut l, &mut r);
                // Skip the first blocks: mute changes fade in over a block (click-free).
                if block >= 2 {
                    peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
                }
            }
            peak
        };
        assert!(run(false) > 0.1, "input heard through the track");
        assert!(run(true) < 1e-3, "muting the track mutes the monitored input");
    }

    #[test]
    fn slide_notes_glide_pitch_up_an_octave() {
        // A long 500 Hz sine played at its root, then a slide note an octave up.
        let n = 96_000;
        let sine: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 500.0 * i as f32 / 48_000.0).sin() * 0.3).collect();
        let sample = Arc::new(Sample { left: sine.clone(), right: sine });
        let ch = Channel::new("s".into(), Source::Sampler { sample, root_note: 60, voices: Vec::new() }, 0.781_25, 0.0, 0);
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let events = vec![
            Event { time: 0, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } },
            Event { time: 4800, channel: 0, track: timeline::NO_TRACK, kind: EventKind::Slide { key: 72, duration: 2400 } },
        ];
        let mut engine = Engine::new(48_000.0, vec![ch], vec![master], events, 48_000);
        engine.controls().play();
        let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
        engine.process(&mut l, &mut r);
        let freq = |x: &[f32]| x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 / (x.len() as f32 / 48_000.0);
        let before = freq(&l[0..4800]);
        let after = freq(&l[14_000..24_000]);
        assert!((before - 500.0).abs() < 15.0, "before {before}");
        assert!((after - 1000.0).abs() < 25.0, "after the slide {after}");
    }

    #[test]
    fn slicer_keys_play_their_slices() {
        // Slice 1 is silence, slice 2 is a constant level.
        let mut left = vec![0.0f32; 2000];
        left[1000..].fill(0.25);
        let sample = Arc::new(Sample { right: left.clone(), left });
        let source = load::slicer_source(sample, &[0.0, 0.5]);
        let ch = Channel::new("slices".into(), source, 0.781_25, 0.0, 0);
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let play = |key: u8| {
            let ev = Event { time: 0, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key, velocity: 128, expr: NoteExpr::default() } };
            let mut engine = Engine::new(48_000.0, vec![Channel::new("slices".into(), match &ch.source { Source::Slicer { sample, slices, .. } => Source::Slicer { sample: sample.clone(), slices: slices.clone(), voices: Vec::new() }, _ => unreachable!() }, 0.781_25, 0.0, 0)], vec![Insert::new("Master".into(), true, 12800, 0, vec![])], vec![ev], 48_000);
            engine.controls().play();
            let (mut l, mut r) = (vec![0.0; 1500], vec![0.0; 1500]);
            engine.process(&mut l, &mut r);
            l
        };
        let _ = master;
        let first = play(SLICE_ROOT);
        assert!(first.iter().all(|v| v.abs() < 1e-6), "slice 1 is the silent half");
        let second = play(SLICE_ROOT + 1);
        assert!(second[10..990].iter().all(|v| *v > 0.05), "slice 2 plays the loud half, for its 1000 frames");
        assert!(second[1100..].iter().all(|v| v.abs() < 1e-3), "and then stops");
        assert!(play(SLICE_ROOT + 5).iter().all(|v| v.abs() < 1e-6), "keys past the last slice are silent");
    }

    #[test]
    fn arpeggiator_retriggers_a_held_note_on_the_grid() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let mut ch = click_channel(0);
        ch.arp.settings = arp::ArpSettings { on: true, rate: 0.25, gate: 0.5, ..Default::default() };
        let hold = Event { time: 0, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        let mut engine = Engine::new(48_000.0, vec![ch], vec![master], vec![hold], 96_000);
        engine.controls().play();
        // Default tempo: 24 000 samples per beat, so a 1/16 step is 6 000 samples.
        let mut out = Vec::new();
        let (mut l, mut r) = (vec![0.0; 1000], vec![0.0; 1000]);
        for _ in 0..24 {
            engine.process(&mut l, &mut r);
            out.extend_from_slice(&l);
        }
        let onsets: Vec<usize> = (0..out.len()).filter(|&i| out[i].abs() > 0.1 && (i == 0 || out[i - 1].abs() <= 0.1)).collect();
        assert_eq!(onsets.len(), 4, "onsets at {onsets:?}");
        assert!(onsets.windows(2).all(|w| (w[1] - w[0]) == 6000), "{onsets:?}");
    }

    #[test]
    fn audio_clips_apply_gain_fades_and_reverse() {
        let render = |gain: f32, fade_in: u32, fade_out: u32, reverse: bool| {
            let ramp: Vec<f32> = (0..1000).map(|i| i as f32 / 4000.0).collect();
            let sample = Arc::new(Sample { left: ramp.clone(), right: ramp });
            let ch = Channel::new("clip".into(), Source::AudioClip { sample, voices: Vec::new() }, 0.781_25, 0.0, 0);
            let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
            let clip = Event { time: 0, channel: 0, track: timeline::NO_TRACK, kind: EventKind::Clip { offset: 0, length: 1000, gain, fade_in, fade_out, reverse } };
            let mut engine = Engine::new(48_000.0, vec![ch], vec![master], vec![clip], 48_000);
            engine.controls().play();
            let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
            engine.process(&mut l, &mut r);
            l
        };
        let plain = render(1.0, 0, 0, false);
        let half = render(0.5, 0, 0, false);
        assert!((half[500] / plain[500] - 0.5).abs() < 1e-4, "clip gain");
        let faded = render(1.0, 200, 200, false);
        let half_power = std::f32::consts::FRAC_1_SQRT_2;
        assert!(faded[0].abs() < 1e-6 && (faded[100] / plain[100] - half_power).abs() < 0.02, "fade in");
        assert!((faded[500] - plain[500]).abs() < 1e-6, "untouched middle");
        assert!((faded[900] / plain[900] - half_power).abs() < 0.02, "fade out");
        let rev = render(1.0, 0, 0, true);
        assert!(rev[0] > rev[500] && rev[500] > rev[990], "reversed ramp falls");
        assert!((rev[0] - plain[999]).abs() < 0.01);
    }

    #[test]
    fn send_levels_scale_the_routed_signal() {
        let level_out = |send: f32| {
            let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
            let mut one = Insert::new("1".into(), true, 12800, 0, vec![0]);
            one.route_levels = vec![send];
            let mut engine = Engine::new(48_000.0, vec![dc_channel(0.1, 1)], vec![master, one], vec![note_at(0, 0)], 48_000);
            engine.controls().play();
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            for _ in 0..4 {
                engine.process(&mut l, &mut r);
            }
            l[511]
        };
        let (full, half) = (level_out(1.0), level_out(0.5));
        assert!(full > 0.05);
        assert!((half / full - 0.5).abs() < 1e-3, "half-level send: {half} vs {full}");
    }

    #[test]
    fn sidechain_send_keys_another_inserts_compressor() {
        let gr_with_key = |keyed: bool| {
            let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
            let mut bass = Insert::new("bass".into(), true, 12800, 0, vec![0]);
            let settings = channel_strip::StripSettings {
                comp: channel_strip::CompSettings { on: true, sidechain: true, threshold: -10.0, ratio: 10.0, attack: 0.1, knee: 0.0, ..Default::default() },
                ..Default::default()
            };
            bass.strip.settings = settings;
            let mut kick = Insert::new("kick".into(), true, 12800, 0, vec![0]);
            if keyed {
                kick.sidechain_to = vec![1];
            }
            let channels = vec![dc_channel(0.25, 1), dc_channel(0.9, 2)];
            let mut engine = Engine::new(48_000.0, channels, vec![master, bass, kick], vec![note_at(0, 0), note_at(0, 1)], 48_000);
            let controls = engine.controls();
            controls.play();
            let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
            for _ in 0..8 {
                engine.process(&mut l, &mut r);
            }
            controls.inserts[1].gain_reduction.load()
        };
        assert_eq!(gr_with_key(false), 0.0, "no key: the compressor hears nothing");
        assert!(gr_with_key(true) > 6.0, "the kick ducks the bass");
    }

    #[test]
    fn inserts_can_be_rerouted_while_running() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let one = Insert::new("1".into(), true, 12800, 0, vec![0]);
        let bus = Insert::new("bus".into(), true, 12800, 0, vec![0]);
        // The note comes after the first block, once the mute's click-free fade has finished.
        let note = Event { time: 700, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        let mut engine = Engine::new(48_000.0, vec![click_channel(1)], vec![master, one, bus], vec![note], 48_000);
        let commands = engine.take_command_sender().unwrap();
        let controls = engine.controls();
        controls.play();
        // Send insert 1 to the bus instead of the master, then mute the bus.
        commands.send(command::Command::SetRoutes { insert: 1, routes: vec![2], levels: vec![1.0] }).unwrap();
        controls.inserts[2].mute.store(true, Ordering::Relaxed);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        engine.process(&mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.0), "audio now goes through the muted bus");
        assert_eq!(engine.inserts[1].routes_to, vec![2]);
    }

    #[test]
    fn automation_moves_controls_during_playback() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let mut engine = Engine::new(48_000.0, vec![click_channel(0)], vec![master], vec![], 48_000);
        let sender = engine.take_timeline_sender().unwrap();
        let controls = engine.controls();
        let seg = timeline::AutoSegment { target: view::AutomationTarget::ChannelPan { channel: 0 }, start: 0, end: 4096, from: 0.0, to: 1.0 };
        sender.send(timeline::Timeline { events: vec![], length: 48_000, automation: vec![seg], beats: vec![] });
        controls.play();
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        assert!((controls.channels[0].pan.load() + 1.0).abs() < 0.01, "starts hard left");
        for _ in 0..7 {
            engine.process(&mut l, &mut r);
        }
        let pan = controls.channels[0].pan.load();
        assert!(pan > 0.7, "moved toward the right ({pan})");
        for _ in 0..4 {
            engine.process(&mut l, &mut r);
        }
        assert!((controls.channels[0].pan.load() - 1.0).abs() < 0.01, "holds the last value");
    }

    #[test]
    fn timeline_swaps_in_while_playing() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let one = Insert::new("1".into(), true, 12800, 0, vec![0]);
        let mut engine = Engine::new(48_000.0, vec![click_channel(1)], vec![master, one], vec![], 48_000);
        let sender = engine.take_timeline_sender().unwrap();
        let controls = engine.controls();
        controls.play();

        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        assert!(l.iter().all(|s| *s == 0.0), "empty timeline is silent");

        // An edit arrives mid-playback: a note just ahead of the playhead.
        let note = Event { time: 600, channel: 0, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        sender.send(timeline::Timeline { events: vec![note], length: 48_000, automation: vec![], beats: vec![] });
        engine.process(&mut l, &mut r);
        let hit = l.iter().position(|s| s.abs() > 0.01);
        assert_eq!(hit, Some(600 - 512), "note plays at its time without a restart");
        assert!(controls.is_playing());
    }

    #[test]
    fn muted_track_is_silent_and_unmuting_restores_it() {
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let one = Insert::new("1".into(), true, 12800, 0, vec![0]);
        let note = |t| Event { time: t, channel: 0, track: 3, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        let mut engine = Engine::new(48_000.0, vec![click_channel(1)], vec![master, one], vec![note(100), note(700)], 48_000);
        let controls = engine.controls();
        controls.set_track_muted(3, true);
        controls.play();
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        engine.process(&mut l, &mut r);
        assert!(l.iter().all(|s| *s == 0.0));
        controls.set_track_muted(3, false);
        engine.process(&mut l, &mut r);
        assert_eq!(l.iter().position(|s| s.abs() > 0.01), Some(700 - 512));
    }

    #[test]
    fn delay_compensation_aligns_paths_and_trims_output() {
        let mut slow = Insert::new("slow".into(), true, 12800, 0, vec![0]);
        slow.effects.push(Effect {
            name: "delay".into(),
            processor: Processor::Native(Box::new(Delay(DelayLine::new(100), DelayLine::new(100), 100))),
            mix: 1.0,
            bypass: false,
        });
        let fast = Insert::new("fast".into(), true, 12800, 0, vec![0]);
        let master = Insert::new("Master".into(), true, 12800, 0, vec![]);
        let channels = vec![click_channel(1), click_channel(2)];
        let at = |t, channel| Event { time: t, channel, track: timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } };
        let events = vec![at(1000, 0), at(1000, 1)];

        let mut engine = Engine::new(48_000.0, channels, vec![master, slow, fast], events, 4000);
        assert_eq!(engine.latency, 100);
        let [l, _] = engine.render();
        let peaks: Vec<usize> = l.iter().enumerate().filter(|(_, s)| s.abs() > 0.01).map(|(i, _)| i).collect();
        // Both clicks sum into a single peak at the scheduled time (limited to the ceiling).
        assert_eq!(peaks, vec![1000], "peaks at {peaks:?}");
    }
}
