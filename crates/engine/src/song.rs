//! KVNG Studio's project document.
//!
//! Everything needed to rebuild a session: channels and their sound sources,
//! the mixer, patterns, playlist clips and tracks. FL Studio projects are
//! converted into this form on import; `.kvng` files are this, as JSON.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::view::{ClipKind, ClipView, NoteView, PatternView, TrackView};

/// Bumped when the file layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;
pub const EXTENSION: &str = "kvng";

#[derive(Clone, Serialize, Deserialize)]
pub struct Song {
    pub format: u32,
    pub name: String,
    pub tempo: f64,
    pub ppq: u32,
    pub beats_per_bar: u32,
    pub channels: Vec<ChannelSpec>,
    /// Index 0 is the master.
    pub inserts: Vec<InsertSpec>,
    pub patterns: Vec<PatternView>,
    pub clips: Vec<ClipView>,
    pub tracks: Vec<TrackView>,
    #[serde(default)]
    pub automations: Vec<crate::view::AutomationView>,
    #[serde(default)]
    pub swing: f32,
    #[serde(default)]
    pub markers: Vec<crate::view::Marker>,
    #[serde(default)]
    pub tempo_changes: Vec<crate::view::TempoChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meter_changes: Vec<crate::view::MeterChange>,
    #[serde(default)]
    pub controller_links: Vec<crate::view::ControllerLink>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub name: String,
    pub source: SourceSpec,
    /// Linear gain.
    pub volume: f32,
    pub pan: f32,
    /// Mixer insert this channel feeds.
    pub route: usize,
    pub muted: bool,
    pub root_note: u8,
    /// Sampler settings (pitch, envelope, cut itself...), for sample-based channels.
    #[serde(default)]
    pub sampler: crate::instrument::SamplerSettings,
    /// Colour chosen by the user (shared with the channel's mixer track display).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// The channel's arpeggiator.
    #[serde(default, skip_serializing_if = "is_default_arp")]
    pub arp: crate::arp::ArpSettings,
    /// Channel rack filter group (FL's channel groups); None = unsorted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceSpec {
    /// A one-shot sample. Paths may use FL Studio placeholders such as `%ILSharedData%`.
    Sampler { path: String },
    AudioClip { path: String },
    Plugin(PluginSpec),
    /// One of FL Studio's own instruments, which KVNG Studio can't play.
    FlNative { internal_name: String },
    /// KVNG Studio's built-in synth.
    Synth { params: crate::instrument::SynthParams },
    /// A built-in, synthesized drum sound.
    Drum { drum: crate::instrument::Drum },
    /// A loop cut into slices (start of each, as a fraction of the file's length), played
    /// from key C5 upwards.
    Slicer { path: String, slices: Vec<f64> },
    /// Sends notes to an external MIDI device (port name; empty = first device).
    MidiOut { port: String, midi_channel: u8 },
    Empty,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PluginSpec {
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    pub format: PluginFormatSpec,
    #[serde(default)]
    pub state: Option<StateSpec>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginFormatSpec {
    Vst3 { path: String, name: Option<String> },
    AudioUnit { kind: String, subtype: String, manufacturer: String },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateSpec {
    /// Raw VST3 component/controller state (as FL Studio stores it).
    Vst3 {
        #[serde(with = "b64")]
        component: Vec<u8>,
        #[serde(with = "b64_opt", default)]
        controller: Option<Vec<u8>>,
    },
    /// An Audio Unit ClassInfo property list.
    AudioUnit {
        #[serde(with = "b64")]
        class_info: Vec<u8>,
    },
    /// State in the plugin host's own format (what KVNG Studio saves).
    Host {
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InsertSpec {
    pub name: String,
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub routes_to: Vec<usize>,
    pub effects: Vec<EffectSpec>,
    /// The insert's built-in EQ and compressor.
    #[serde(default)]
    pub strip: crate::channel_strip::StripSettings,
    /// Send level (linear) per entry of `routes_to`; missing entries mean 1.0 (0 dB).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_levels: Vec<f32>,
    /// Inserts whose compressor this insert keys (sidechain sends).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidechain_to: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EffectSpec {
    pub name: String,
    pub source: EffectSource,
    /// 0.0 = dry, 1.0 = fully wet.
    pub mix: f32,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Loaded but skipped (the slot's bypass switch).
    #[serde(default)]
    pub bypassed: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectSource {
    Plugin(PluginSpec),
    /// One of KVNG Studio's built-in effects.
    Builtin { effect: crate::effects::BuiltinEffect },
    /// One of FL Studio's effects, played through a KVNG adapter when one exists.
    FlNative {
        internal_name: String,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
}

/// Mixer inserts besides the master (FL Studio's count). Projects with fewer are padded
/// on load so there's always a free track for new channels.
pub const MAX_INSERTS: usize = 125;

impl InsertSpec {
    /// An unused insert `index`, routed to the master.
    pub fn blank(index: usize) -> InsertSpec {
        InsertSpec { name: format!("Insert {index}"), volume: 1.0, pan: 0.0, muted: false, routes_to: vec![0], effects: vec![], strip: Default::default(), route_levels: vec![], sidechain_to: vec![], color: None }
    }
}

impl Song {
    /// A new project like FL Studio's default template: drums and a synth, each on its
    /// own mixer insert, master plus 16 inserts, one empty pattern.
    pub fn empty(name: &str) -> Song {
        use crate::instrument::{Drum, SynthParams};
        let mut channels: Vec<ChannelSpec> = Drum::ALL
            .iter()
            .enumerate()
            .map(|(i, d)| ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(), name: d.label().into(), source: SourceSpec::Drum { drum: *d }, volume: 1.0, pan: 0.0, route: i + 1, muted: false, root_note: 60 })
            .collect();
        channels.push(ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(),
            name: "KVNG Synth".into(),
            source: SourceSpec::Synth { params: SynthParams::default() },
            volume: 1.0,
            pan: 0.0,
            route: 5,
            muted: false,
            root_note: 60,
        });
        let mut inserts = vec![InsertSpec { name: "Master".into(), routes_to: vec![], ..InsertSpec::blank(0) }];
        for i in 1..=MAX_INSERTS {
            let mut ins = InsertSpec::blank(i);
            if let Some(c) = channels.get(i - 1) {
                ins.name = c.name.clone();
            }
            inserts.push(ins);
        }
        Song {
            format: FORMAT_VERSION,
            name: name.into(),
            tempo: 130.0,
            ppq: 96,
            beats_per_bar: 4,
            channels,
            inserts,
            patterns: vec![PatternView { color: None, name: "Pattern 1".into(), notes: vec![], length: 384 }],
            clips: vec![],
            tracks: (1..=16).map(|i| TrackView { name: format!("Track {i}"), muted: false, color: None, folder: false, collapsed: false, grouped: false }).collect(),
            automations: vec![],
            swing: 0.0,
            markers: vec![],
            tempo_changes: vec![],
            meter_changes: vec![],
            controller_links: vec![],
        }
    }

    pub fn read(path: &Path) -> Result<Song, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let song: Song = serde_json::from_str(&text).map_err(|e| format!("{}: not a valid KVNG project ({e})", path.display()))?;
        if song.format > FORMAT_VERSION {
            return Err(format!("{} was saved by a newer KVNG Studio (format {})", path.display(), song.format));
        }
        Ok(song)
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a half-written project.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("kvng.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Channels the arrangement plays (on unmuted tracks), by index.
    pub fn used_channels(&self) -> HashSet<usize> {
        let mut used = HashSet::new();
        for clip in &self.clips {
            if self.tracks.get(clip.track).is_some_and(|t| t.muted) {
                continue;
            }
            match clip.kind {
                ClipKind::Automation => {}
                ClipKind::Audio => used.extend(clip.channel),
                ClipKind::Pattern => {
                    if let Some(p) = clip.pattern.and_then(|p| self.patterns.get(p)) {
                        used.extend(p.notes.iter().map(|n| n.channel));
                    }
                }
            }
        }
        // Notes in patterns that aren't placed yet are still played in pattern mode.
        for p in &self.patterns {
            used.extend(p.notes.iter().map(|n| n.channel));
        }
        used
    }
}

/// The current session as a document: `base` updated with the edited view, the live
/// mixer settings, and each helper-hosted plugin's current settings. Plugins whose
/// helper has stopped keep the settings they were loaded with (reported as problems).
pub fn capture(base: &Song, view: &crate::view::ProjectView, controls: &crate::control::Controls) -> (Song, Vec<String>) {
    let mut problems = Vec::new();
    let mut song = base.clone();
    song.name = view.name.clone();
    song.tempo = view.tempo;
    song.beats_per_bar = view.beats_per_bar;
    song.patterns = view.patterns.clone();
    song.clips = view.clips.clone();
    song.tracks = view.tracks.clone();
    song.automations = view.automations.clone();
    song.swing = view.swing;
    song.markers = view.markers.clone();
    song.tempo_changes = view.tempo_changes.clone();
    song.meter_changes = view.meter_changes.clone();
    song.controller_links = view.controller_links.clone();

    for (i, spec) in song.channels.iter_mut().enumerate() {
        if let Some(v) = view.channels.get(i) {
            spec.name = v.name.clone();
            spec.color = v.color;
        }
        if let Some(strip) = controls.channels.get(i) {
            spec.volume = strip.volume.load();
            spec.pan = strip.pan.load();
            spec.muted = strip.is_muted();
            spec.route = strip.route();
        }
        if let (SourceSpec::Plugin(plugin), Some(handle)) = (&mut spec.source, view.channels.get(i).and_then(|c| c.editor.as_ref())) {
            match handle.save_state() {
                Ok(data) => plugin.state = Some(StateSpec::Host { data }),
                Err(e) => problems.push(format!("{}: settings not saved ({e})", spec.name)),
            }
        }
    }

    for (i, spec) in song.inserts.iter_mut().enumerate() {
        if let Some(strip) = controls.inserts.get(i) {
            spec.volume = strip.volume.load();
            spec.pan = strip.pan.load();
            spec.muted = strip.is_muted();
        }
        if let Some(v) = view.inserts.get(i) {
            spec.name = v.name.clone();
            if i > 0 {
                spec.routes_to = v.routes_to.clone();
                spec.route_levels = if v.route_levels.iter().all(|&l| l == 1.0) { vec![] } else { v.route_levels.clone() };
            }
            spec.sidechain_to = v.sidechain_to.clone();
            spec.color = v.color;
        }
        // Loaded effects appear in the view in document order (ones that failed to load are absent).
        let Some(insert_view) = view.inserts.get(i) else { continue };
        let mut next = 0;
        for fx_view in &insert_view.effects {
            let Some(offset) = spec.effects[next..].iter().position(|e| e.enabled && e.name == fx_view.name) else { continue };
            let fx = &mut spec.effects[next + offset];
            next += offset + 1;
            fx.bypassed = fx_view.bypass;
            fx.mix = fx_view.mix;
            if let (EffectSource::Plugin(plugin), Some(handle)) = (&mut fx.source, &fx_view.editor) {
                match handle.save_state() {
                    Ok(data) => plugin.state = Some(StateSpec::Host { data }),
                    Err(e) => problems.push(format!("{} on {}: settings not saved ({e})", fx.name, spec.name)),
                }
            }
        }
    }
    compact_deleted(&mut song, view);
    (song, problems)
}

/// Drops channels the user deleted, renumbering note and clip references.
fn compact_deleted(song: &mut Song, view: &crate::view::ProjectView) {
    let deleted: Vec<bool> = (0..song.channels.len()).map(|i| view.channels.get(i).is_some_and(|c| c.deleted)).collect();
    if !deleted.iter().any(|d| *d) {
        return;
    }
    let mut remap = vec![None; deleted.len()];
    let mut next = 0;
    for (i, d) in deleted.iter().enumerate() {
        if !d {
            remap[i] = Some(next);
            next += 1;
        }
    }
    let mut i = 0;
    song.channels.retain(|_| {
        i += 1;
        !deleted[i - 1]
    });
    for p in &mut song.patterns {
        p.notes.retain_mut(|n| match remap.get(n.channel).copied().flatten() {
            Some(c) => {
                n.channel = c;
                true
            }
            None => false,
        });
    }
    song.clips.retain_mut(|c| match c.channel {
        Some(ch) => match remap.get(ch).copied().flatten() {
            Some(n) => {
                c.channel = Some(n);
                true
            }
            None => false,
        },
        None => true,
    });
}

/// Converts an FL Studio project into a [`Song`].
pub fn from_flp(project: &flp::Project, path: &Path) -> Song {
    use flp::{ChannelKind, ItemSource, PluginFormat, PluginState};
    let iid_to_index: std::collections::HashMap<u16, usize> = project.channels.iter().enumerate().map(|(i, c)| (c.iid, i)).collect();
    let ppq = project.ppq.max(1) as u32;
    let beats_per_bar = project.time_signature.0.max(1) as u32;
    let ticks_per_bar = (ppq * beats_per_bar) as u64;

    let plugin_spec = |p: &flp::Plugin| -> Option<PluginSpec> {
        let w = p.wrapped.as_ref()?;
        let format = match w.format() {
            PluginFormat::Vst3 { .. } => PluginFormatSpec::Vst3 { path: w.path.clone()?, name: w.name.clone() },
            PluginFormat::AudioUnit { kind, subtype, manufacturer } => PluginFormatSpec::AudioUnit { kind, subtype, manufacturer },
            PluginFormat::Other => return None,
        };
        let state = match w.decoded_state() {
            Some(PluginState::Vst3 { component, controller }) => {
                Some(StateSpec::Vst3 { component: component.to_vec(), controller: controller.map(<[u8]>::to_vec) })
            }
            Some(PluginState::AudioUnit { class_info }) => Some(StateSpec::AudioUnit { class_info: class_info.to_vec() }),
            _ => None,
        };
        Some(PluginSpec {
            name: w.name.clone().unwrap_or_else(|| p.display_name.clone()),
            vendor: w.vendor.clone().unwrap_or_default(),
            format,
            state,
        })
    };

    let channels = project
        .channels
        .iter()
        .map(|ch| {
            let source = match (ch.kind, &ch.sample_path, &ch.plugin) {
                (_, _, _) if !ch.enabled => SourceSpec::Empty,
                (ChannelKind::Sampler, Some(p), _) => SourceSpec::Sampler { path: p.clone() },
                (ChannelKind::AudioClip, Some(p), _) => SourceSpec::AudioClip { path: p.clone() },
                (ChannelKind::Generator, _, Some(pl)) => match plugin_spec(pl) {
                    Some(spec) => SourceSpec::Plugin(spec),
                    None if !pl.internal_name.is_empty() => SourceSpec::FlNative { internal_name: pl.internal_name.clone() },
                    None => SourceSpec::Empty,
                },
                _ => SourceSpec::Empty,
            };
            ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(),
                name: ch.name.clone(),
                source,
                volume: crate::gain::channel_volume(ch.volume.unwrap_or(0.781_25)),
                pan: ch.pan.unwrap_or(0.0),
                route: ch.insert.map_or(0, |i| i.max(0) as usize),
                muted: false,
                root_note: ch.root_note,
            }
        })
        .collect();

    let mut inserts: Vec<InsertSpec> = project
        .inserts
        .iter()
        .map(|ins| InsertSpec {
            strip: Default::default(),
            route_levels: vec![],
            sidechain_to: vec![],
            color: None,
            name: ins.name.clone().unwrap_or_else(|| if ins.index == 0 { "Master".into() } else { format!("Insert {}", ins.index) }),
            volume: crate::gain::insert_volume(ins.volume),
            pan: ins.pan as f32 / 6400.0,
            muted: !ins.enabled,
            routes_to: if ins.index == 0 {
                vec![]
            } else if ins.routes_to.is_empty() {
                vec![0]
            } else {
                ins.routes_to.clone()
            },
            effects: ins
                .slots
                .iter()
                .filter(|_| ins.effects_enabled)
                .filter_map(|slot| {
                    let source = match plugin_spec(&slot.plugin) {
                        Some(spec) => EffectSource::Plugin(spec),
                        None if !slot.plugin.internal_name.is_empty() && !slot.plugin.is_third_party() => {
                            EffectSource::FlNative { internal_name: slot.plugin.internal_name.clone(), data: slot.plugin.data.clone() }
                        }
                        None => return None,
                    };
                    let name = if slot.plugin.display_name.is_empty() { slot.plugin.internal_name.clone() } else { slot.plugin.display_name.clone() };
                    Some(EffectSpec { name, source, mix: slot.mix as f32 / 12800.0, enabled: slot.enabled, bypassed: false })
                })
                .collect(),
        })
        .collect();
    if inserts.is_empty() {
        inserts = Song::empty("").inserts;
    }

    let pattern_index: std::collections::HashMap<u16, usize> = project.patterns.iter().enumerate().map(|(i, p)| (p.iid, i)).collect();
    let patterns = project
        .patterns
        .iter()
        .map(|p| {
            let notes: Vec<NoteView> = p
                .notes
                .iter()
                .filter_map(|n| {
                    Some(NoteView {
                        start: n.position as u64,
                        length: n.length.max(1) as u64,
                        key: n.key.min(127) as u8,
                        velocity: n.velocity,
                        channel: *iid_to_index.get(&n.channel)?,
                        pan: n.pan.min(128),
                        mod_x: n.mod_x,
                        mod_y: n.mod_y,
                        // FL note flag bit 3 marks slide notes.
                        slide: n.flags & 0x08 != 0,
                    })
                })
                .collect();
            let end = notes.iter().map(|n| n.start + n.length).max().unwrap_or(ticks_per_bar);
            PatternView {
                color: None,
                name: if p.name.is_empty() { format!("Pattern {}", p.iid) } else { p.name.clone() },
                notes,
                length: end.div_ceil(ticks_per_bar).max(1) * ticks_per_bar,
            }
        })
        .collect();

    let (mut clips, mut tracks) = (Vec::new(), Vec::new());
    if let Some(arr) = project.arrangements.first() {
        for item in &arr.items {
            let offset = if item.start_offset.is_finite() && item.start_offset > 0.0 { item.start_offset as f64 } else { 0.0 };
            let clip = match item.source {
                ItemSource::Pattern(iid) => {
                    let Some(&pi) = pattern_index.get(&iid) else { continue };
                    ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false,
                        track: item.track as usize,
                        start: item.position as u64,
                        length: item.length as u64,
                        label: project.patterns[pi].name.clone(),
                        kind: ClipKind::Pattern,
                        pattern: Some(pi),
                        channel: None,
                        source_offset: offset,
                    }
                }
                ItemSource::Channel(iid) => {
                    let Some(&ci) = iid_to_index.get(&iid) else { continue };
                    ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false,
                        track: item.track as usize,
                        start: item.position as u64,
                        length: item.length as u64,
                        label: project.channels[ci].name.clone(),
                        kind: ClipKind::Audio,
                        pattern: None,
                        channel: Some(ci),
                        // FL stores audio clip offsets in milliseconds.
                        source_offset: offset / 1000.0,
                    }
                }
            };
            clips.push(clip);
        }
        let last_track = clips.iter().map(|c| c.track).max().unwrap_or(0);
        let shown = (last_track + 4).max(12).min(arr.tracks.len().max(last_track + 1));
        for i in 0..shown {
            let t = arr.tracks.get(i);
            let name = t.map(|t| t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Track {}", i + 1));
            tracks.push(TrackView { name, muted: t.is_some_and(|t| !t.enabled), color: None, folder: false, collapsed: false, grouped: false });
        }
    }
    if tracks.is_empty() {
        tracks = Song::empty("").tracks;
    }

    Song {
        format: FORMAT_VERSION,
        name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        tempo: project.tempo,
        ppq,
        beats_per_bar,
        channels,
        inserts,
        patterns,
        clips,
        tracks,
        automations: vec![],
        swing: 0.0,
        markers: vec![],
        tempo_changes: vec![],
        meter_changes: vec![],
        controller_links: vec![],
    }
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

mod b64_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(b) => super::b64::serialize(b, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        s.map(|s| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
        })
        .transpose()
    }
}

fn is_default_arp(a: &crate::arp::ArpSettings) -> bool {
    *a == crate::arp::ArpSettings::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_and_strip_settings_survive_save_and_load() {
        let mut song = Song::empty("t");
        song.inserts[1].routes_to = vec![0, 3];
        song.inserts[1].route_levels = vec![1.0, 0.5];
        song.inserts[2].sidechain_to = vec![1];
        song.inserts[1].strip.comp.on = true;
        song.inserts[1].strip.comp.sidechain = true;
        song.inserts[1].strip.eq.bands[2].gain = 4.5;
        let back: Song = serde_json::from_str(&serde_json::to_string(&song).unwrap()).unwrap();
        assert_eq!(back.inserts[1].route_levels, vec![1.0, 0.5]);
        assert_eq!(back.inserts[2].sidechain_to, vec![1]);
        assert_eq!(back.inserts[1].strip, song.inserts[1].strip);
        // Older files without these fields load with neutral defaults.
        let mut old = serde_json::to_value(&song).unwrap();
        for ins in old["inserts"].as_array_mut().unwrap() {
            let o = ins.as_object_mut().unwrap();
            o.remove("strip");
            o.remove("route_levels");
            o.remove("sidechain_to");
        }
        let old: Song = serde_json::from_value(old).unwrap();
        assert!(old.inserts[1].route_levels.is_empty() && !old.inserts[1].strip.comp.on);
    }

    #[test]
    fn round_trips_through_json() {
        let mut song = Song::empty("test");
        song.channels.push(ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(),
            name: "Synth".into(),
            source: SourceSpec::Plugin(PluginSpec {
                name: "Kia".into(),
                vendor: "AudioFB".into(),
                format: PluginFormatSpec::AudioUnit { kind: "aumu".into(), subtype: "k105".into(), manufacturer: "Aufb".into() },
                state: Some(StateSpec::Host { data: vec![0, 1, 2, 255] }),
            }),
            volume: 0.5,
            pan: -0.25,
            route: 3,
            muted: true,
            root_note: 60,
        });
        song.patterns[0].notes.push(NoteView { start: 96, length: 24, key: 64, velocity: 90, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        let json = serde_json::to_string(&song).unwrap();
        let back: Song = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        assert_eq!(back.channels.len(), 6, "template channels plus the added one");
        match &back.channels.last().unwrap().source {
            SourceSpec::Plugin(PluginSpec { state: Some(StateSpec::Host { data }), .. }) => assert_eq!(data, &vec![0, 1, 2, 255]),
            _ => panic!("plugin state lost"),
        }
    }
}
