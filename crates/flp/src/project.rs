//! Builds a structured [`Project`] from the raw event stream.
//!
//! FLP is a flat, order-dependent stream: "new channel", "new pattern",
//! "new arrangement" and the mixer insert/slot markers switch which object
//! the following events describe.

use crate::event::{self, Event, Events, Reader, DATA, DWORD, TEXT, WORD};
use crate::Error;

// Project-wide
const FL_VERSION: u8 = TEXT + 7;
const TEMPO: u8 = DWORD + 28;
const TITLE: u8 = TEXT + 2;
const ARTISTS: u8 = TEXT + 15;
const GENRE: u8 = TEXT + 14;

// Channels
const CHANNEL_NEW: u8 = WORD;
const CHANNEL_TYPE: u8 = 21;
const CHANNEL_ENABLED: u8 = 0;
const CHANNEL_ROUTED_TO: u8 = 22;
const CHANNEL_NAME_LEGACY: u8 = TEXT;
const CHANNEL_SAMPLE_PATH: u8 = TEXT + 4;
const CHANNEL_LEVELS: u8 = DATA + 11;
const CHANNEL_ROOT_NOTE: u8 = DWORD + 7;

// Plugins (shared by channels and mixer slots)
const PLUGIN_INTERNAL_NAME: u8 = TEXT + 9;
const PLUGIN_NAME: u8 = TEXT + 11;
const PLUGIN_DATA: u8 = DATA + 5;

// Patterns
const PATTERN_NEW: u8 = WORD + 1;
const PATTERN_NAME: u8 = TEXT + 1;
const PATTERN_LENGTH: u8 = DWORD + 36;
const PATTERN_NOTES: u8 = DATA + 16;

// Arrangements / playlist
const ARRANGEMENT_NEW: u8 = WORD + 35;
const ARRANGEMENT_NAME: u8 = TEXT + 49;
const ARRANGEMENT_PLAYLIST: u8 = DATA + 25;
const TRACK_DATA: u8 = DATA + 30;
const TRACK_NAME: u8 = TEXT + 47;
const TIME_SIG_NUM: u8 = 17;
const TIME_SIG_BEAT: u8 = 18;

// Mixer
const INSERT_NAME: u8 = TEXT + 12;
const INSERT_FLAGS: u8 = DATA + 28;
const INSERT_OUTPUT: u8 = DWORD + 19;
const SLOT_INDEX: u8 = WORD + 34;
const INSERT_ROUTING: u8 = DATA + 27;
const MIXER_PARAMS: u8 = DATA + 17;

/// Playlist item indices at or above this refer to patterns; below it, channels.
const PATTERN_BASE: u16 = 20480;
/// Playlist track indices are stored counting down from this value.
const TRACK_INDEX_TOP: u16 = 499;

#[derive(Debug, Clone, Default)]
pub struct Project {
    pub version: String,
    pub format: i16,
    pub ppq: u16,
    pub tempo: f64,
    pub title: String,
    pub artists: String,
    pub genre: String,
    pub time_signature: (u8, u8),
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub arrangements: Vec<Arrangement>,
    /// Index 0 is the master insert.
    pub inserts: Vec<Insert>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// FL's built-in Sampler.
    Sampler,
    /// A generator plugin: FL's own (FLEX, Sytrus...) or a wrapped VST/AU.
    Generator,
    Layer,
    /// An audio clip channel (placed directly on the playlist).
    AudioClip,
    Automation,
    Unknown(u8),
}

impl From<u8> for ChannelKind {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Sampler,
            2 => Self::Generator,
            3 => Self::Layer,
            4 => Self::AudioClip,
            5 => Self::Automation,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub iid: u16,
    pub kind: ChannelKind,
    pub name: String,
    pub enabled: bool,
    pub sample_path: Option<String>,
    /// Mixer insert this channel feeds (0 = master).
    pub insert: Option<i8>,
    /// 0.0..=1.0, where FL's default is 0.78125.
    pub volume: Option<f32>,
    /// -1.0 (left) ..= 1.0 (right).
    pub pan: Option<f32>,
    /// The key at which a sample plays at its original pitch (FL default: 60, C5).
    pub root_note: u8,
    pub plugin: Option<Plugin>,
}

#[derive(Debug, Clone, Default)]
pub struct Plugin {
    /// FL's internal identifier, e.g. "Fruity Wrapper" for third-party plugins, "FLEX", "Fruity Limiter".
    pub internal_name: String,
    /// The user-visible name, e.g. "TRITON_Extreme [64bit]".
    pub display_name: String,
    /// Present when the plugin is a third-party plugin hosted through FL's wrapper.
    pub wrapped: Option<WrappedPlugin>,
    /// The raw plugin data block. For FL's own plugins this is their private parameter format.
    pub data: Vec<u8>,
}

impl Plugin {
    pub fn is_third_party(&self) -> bool {
        self.wrapped.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct WrappedPlugin {
    pub name: Option<String>,
    pub vendor: Option<String>,
    /// Bundle path; FL stores this for VST/VST3 but not for Audio Units.
    pub path: Option<String>,
    pub fourcc: Option<String>,
    pub guid: Option<Vec<u8>>,
    /// FL's state block exactly as stored. See [`WrappedPlugin::decoded_state`].
    pub state: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginFormat {
    Vst3 { class_id: [u8; 16] },
    AudioUnit { kind: String, subtype: String, manufacturer: String },
    /// VST2 or anything not recognised.
    Other,
}

/// A plugin's saved state, unwrapped from FL's container into what the plugin format expects.
#[derive(Debug, Clone)]
pub enum PluginState<'a> {
    /// Bytes for `IComponent::setState` (and `IEditController::setComponentState`).
    Vst3 { component: &'a [u8], controller: Option<&'a [u8]> },
    /// An `AUPreset`/ClassInfo property list for `kAudioUnitProperty_ClassInfo`.
    AudioUnit { class_info: &'a [u8] },
    Unknown(&'a [u8]),
}

impl WrappedPlugin {
    pub fn format(&self) -> PluginFormat {
        let Some(guid) = self.guid.as_deref() else { return PluginFormat::Other };
        if let Some(path) = &self.path {
            if path.ends_with(".vst3") && guid.len() == 16 {
                return PluginFormat::Vst3 { class_id: guid.try_into().unwrap() };
            }
            return PluginFormat::Other;
        }
        // Audio Units: manufacturer, type, subtype as little-endian four-char codes.
        if guid.len() >= 12 {
            let code = |i: usize| guid[i..i + 4].iter().rev().map(|&b| b as char).collect::<String>();
            return PluginFormat::AudioUnit { manufacturer: code(0), kind: code(4), subtype: code(8) };
        }
        PluginFormat::Other
    }

    pub fn decoded_state(&self) -> Option<PluginState<'_>> {
        const VST3_COMPONENT: u32 = 3;
        const VST3_CONTROLLER: u32 = 4;

        let state = self.state.as_deref()?;
        match self.format() {
            PluginFormat::Vst3 { .. } => {
                // u32 version, then (u32 id, u64 length, bytes) records.
                let mut r = Reader::new(state, 0);
                let mut component = None;
                let mut controller = None;
                if r.u32().ok()? == 1 {
                    while let Ok(id) = r.u32() {
                        let len = r.u64().ok()? as usize;
                        let bytes = r.take(len).ok()?;
                        match id {
                            VST3_COMPONENT => component = Some(bytes),
                            VST3_CONTROLLER => controller = Some(bytes),
                            _ => {}
                        }
                    }
                }
                Some(match component {
                    Some(component) => PluginState::Vst3 { component, controller },
                    None => PluginState::Unknown(state),
                })
            }
            PluginFormat::AudioUnit { .. } => {
                // u32 0, u32 plist length, u32 0, then the property list.
                let mut r = Reader::new(state, 0);
                let (_, len, _) = (r.u32().ok()?, r.u32().ok()? as usize, r.u32().ok()?);
                Some(match r.take(len) {
                    Ok(class_info) => PluginState::AudioUnit { class_info },
                    Err(_) => PluginState::Unknown(state),
                })
            }
            PluginFormat::Other => Some(PluginState::Unknown(state)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub iid: u16,
    pub name: String,
    pub length: Option<u32>,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, Copy)]
pub struct Note {
    /// In ticks (see [`Project::ppq`]).
    pub position: u32,
    pub length: u32,
    /// The channel (by `iid`) that plays this note.
    pub channel: u16,
    /// MIDI-style key number; 60 = C5 in FL's naming.
    pub key: u16,
    pub velocity: u8,
    pub pan: u8,
    pub release: u8,
    pub fine_pitch: u8,
    pub mod_x: u8,
    pub mod_y: u8,
    pub flags: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Arrangement {
    pub iid: u16,
    pub name: String,
    pub items: Vec<PlaylistItem>,
    /// Playlist tracks in order; index matches [`PlaylistItem::track`].
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Default)]
pub struct Track {
    pub name: String,
    /// False when the track is muted.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSource {
    Pattern(u16),
    /// An audio clip or automation clip, by channel `iid`.
    Channel(u16),
}

#[derive(Debug, Clone, Copy)]
pub struct PlaylistItem {
    pub position: u32,
    pub length: u32,
    /// 0-based playlist track.
    pub track: u16,
    pub source: ItemSource,
    pub group: u16,
    /// Offsets into the source, in ticks for patterns; -1 when unset.
    pub start_offset: f32,
    pub end_offset: f32,
}

#[derive(Debug, Clone)]
pub struct Insert {
    pub index: usize,
    pub name: Option<String>,
    pub slots: Vec<Slot>,
    /// False when the insert is muted.
    pub enabled: bool,
    /// False when the insert's effect slots are bypassed.
    pub effects_enabled: bool,
    /// Inserts this one sends to. Empty for the master.
    pub routes_to: Vec<usize>,
    /// Fader position on FL's 0..=16000 scale (12800 = 0 dB).
    pub volume: u32,
    /// -6400 (left) ..= 6400 (right).
    pub pan: i32,
}

impl Default for Insert {
    fn default() -> Self {
        Self {
            index: 0,
            name: None,
            slots: Vec::new(),
            enabled: true,
            effects_enabled: true,
            routes_to: Vec::new(),
            volume: 12800,
            pan: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Slot {
    pub index: u16,
    pub plugin: Plugin,
    pub enabled: bool,
    /// Dry/wet mix, 0..=12800 (12800 = fully wet).
    pub mix: u32,
}

enum Ctx {
    None,
    Channel(usize),
    Pattern(usize),
    Arrangement(usize),
    Slot,
}

#[derive(Default)]
struct MixerState {
    started: bool,
    current: Insert,
    pending_slot: Option<(u16, Plugin)>,
}

impl MixerState {
    fn flush_slot(&mut self) {
        if let Some((index, plugin)) = self.pending_slot.take()
            && !plugin.internal_name.is_empty() {
                self.current.slots.push(Slot { index, plugin, enabled: true, mix: 12800 });
            }
    }
}

pub fn parse(file: &[u8]) -> Result<Project, Error> {
    let (header, data, base) = event::split(file)?;
    let mut p = Project {
        format: header.format,
        ppq: header.ppq,
        time_signature: (4, 4),
        ..Default::default()
    };
    let mut utf16 = true;
    let mut ctx = Ctx::None;
    let mut mixer = MixerState::default();

    for ev in Events::new(data, base) {
        let ev = ev?;
        let text = |ev: &Event| decode_text(ev.as_bytes(), utf16);

        match ev.id {
            FL_VERSION => {
                p.version = decode_text(ev.as_bytes(), false);
                utf16 = version_at_least(&p.version, 11, 5);
            }
            TEMPO => p.tempo = ev.as_u32().unwrap_or(0) as f64 / 1000.0,
            TITLE => p.title = text(&ev),
            ARTISTS => p.artists = text(&ev),
            GENRE => p.genre = text(&ev),
            TIME_SIG_NUM => p.time_signature.0 = ev.as_u32().unwrap_or(4) as u8,
            TIME_SIG_BEAT => p.time_signature.1 = ev.as_u32().unwrap_or(4) as u8,

            CHANNEL_NEW if !mixer.started => {
                let iid = ev.as_u32().unwrap_or(0) as u16;
                p.channels.push(Channel {
                    iid,
                    kind: ChannelKind::Unknown(255),
                    name: String::new(),
                    enabled: true,
                    sample_path: None,
                    insert: None,
                    volume: None,
                    pan: None,
                    root_note: 60,
                    plugin: None,
                });
                ctx = Ctx::Channel(p.channels.len() - 1);
            }

            PATTERN_NEW => {
                // Each pattern appears twice: once with notes, once with name/colour.
                let iid = ev.as_u32().unwrap_or(0) as u16;
                let idx = match p.patterns.iter().position(|pat| pat.iid == iid) {
                    Some(i) => i,
                    None => {
                        p.patterns.push(Pattern { iid, name: String::new(), length: None, notes: Vec::new() });
                        p.patterns.len() - 1
                    }
                };
                ctx = Ctx::Pattern(idx);
            }

            ARRANGEMENT_NEW => {
                let iid = ev.as_u32().unwrap_or(0) as u16;
                p.arrangements.push(Arrangement { iid, ..Default::default() });
                ctx = Ctx::Arrangement(p.arrangements.len() - 1);
            }

            INSERT_FLAGS | SLOT_INDEX | INSERT_OUTPUT | INSERT_NAME | INSERT_ROUTING => {
                mixer.started = true;
                match ev.id {
                    INSERT_FLAGS => {
                        const EFFECTS_ENABLED: u32 = 1 << 2;
                        const ENABLED: u32 = 1 << 3;
                        let b = ev.as_bytes();
                        if b.len() >= 8 {
                            let flags = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                            mixer.current.enabled = flags & ENABLED != 0;
                            mixer.current.effects_enabled = flags & EFFECTS_ENABLED != 0;
                        }
                    }
                    INSERT_ROUTING => {
                        mixer.current.routes_to =
                            ev.as_bytes().iter().enumerate().filter(|(_, b)| **b != 0).map(|(i, _)| i).collect();
                    }
                    SLOT_INDEX => {
                        mixer.flush_slot();
                        mixer.pending_slot = Some((ev.as_u32().unwrap_or(0) as u16, Plugin::default()));
                        ctx = Ctx::Slot;
                    }
                    INSERT_NAME => mixer.current.name = Some(text(&ev)),
                    INSERT_OUTPUT => {
                        mixer.flush_slot();
                        let mut done = std::mem::take(&mut mixer.current);
                        done.index = p.inserts.len();
                        p.inserts.push(done);
                        ctx = Ctx::None;
                    }
                    _ => {}
                }
            }

            MIXER_PARAMS => apply_mixer_params(&mut p.inserts, ev.as_bytes()),

            _ => apply_contextual(&mut p, &mut mixer, &ctx, &ev, utf16)?,
        }
    }

    Ok(p)
}

fn apply_contextual(p: &mut Project, mixer: &mut MixerState, ctx: &Ctx, ev: &Event, utf16: bool) -> Result<(), Error> {
    let text = || decode_text(ev.as_bytes(), utf16);

    match ctx {
        Ctx::Channel(i) => {
            let ch = &mut p.channels[*i];
            match ev.id {
                CHANNEL_TYPE => ch.kind = ChannelKind::from(ev.as_u32().unwrap_or(255) as u8),
                CHANNEL_ENABLED => ch.enabled = ev.as_u32() != Some(0),
                CHANNEL_ROUTED_TO => ch.insert = Some(ev.as_u32().unwrap_or(0) as u8 as i8),
                CHANNEL_SAMPLE_PATH => {
                    let path = text();
                    ch.sample_path = (!path.is_empty()).then_some(path);
                }
                CHANNEL_NAME_LEGACY if ch.name.is_empty() => ch.name = text(),
                CHANNEL_ROOT_NOTE => ch.root_note = ev.as_u32().unwrap_or(60).min(127) as u8,
                CHANNEL_LEVELS => {
                    let mut r = Reader::new(ev.as_bytes(), ev.offset);
                    if let (Ok(pan), Ok(vol)) = (r.u32(), r.u32()) {
                        ch.pan = Some((pan as i32 as f32 - 6400.0) / 6400.0);
                        ch.volume = Some(vol as f32 / 12800.0);
                    }
                }
                PLUGIN_INTERNAL_NAME | PLUGIN_NAME | PLUGIN_DATA => {
                    let plugin = ch.plugin.get_or_insert_with(Plugin::default);
                    apply_plugin_event(plugin, ev, utf16)?;
                    if ev.id == PLUGIN_NAME {
                        ch.name = plugin.display_name.clone();
                    }
                }
                _ => {}
            }
        }
        Ctx::Pattern(i) => {
            let pat = &mut p.patterns[*i];
            match ev.id {
                PATTERN_NAME => pat.name = text(),
                PATTERN_LENGTH => pat.length = ev.as_u32(),
                PATTERN_NOTES => pat.notes.extend(parse_notes(ev.as_bytes())),
                _ => {}
            }
        }
        Ctx::Arrangement(i) => {
            let arr = &mut p.arrangements[*i];
            match ev.id {
                ARRANGEMENT_NAME => arr.name = text(),
                ARRANGEMENT_PLAYLIST => arr.items.extend(parse_playlist(ev.as_bytes())),
                TRACK_DATA => {
                    // u32 iid, u32 colour, u32 icon, u8 enabled, ...; names follow their track.
                    let enabled = ev.as_bytes().get(12).is_none_or(|&b| b != 0);
                    arr.tracks.push(Track { name: String::new(), enabled });
                }
                TRACK_NAME => {
                    if let Some(last) = arr.tracks.last_mut() {
                        last.name = text();
                    }
                }
                _ => {}
            }
        }
        Ctx::Slot => {
            if let Some((_, plugin)) = mixer.pending_slot.as_mut()
                && matches!(ev.id, PLUGIN_INTERNAL_NAME | PLUGIN_NAME | PLUGIN_DATA) {
                    apply_plugin_event(plugin, ev, utf16)?;
                }
        }
        Ctx::None => {}
    }
    Ok(())
}

/// Mixer parameters are a list of 12-byte records:
/// `[4 unknown][u8 id][u8 unknown][u16 insert<<6 | slot][i32 value]`.
fn apply_mixer_params(inserts: &mut [Insert], data: &[u8]) {
    const SLOT_ENABLED: u8 = 0;
    const SLOT_MIX: u8 = 1;
    const VOLUME: u8 = 192;
    const PAN: u8 = 193;

    for rec in data.as_chunks::<12>().0 {
        let id = rec[4];
        let target = u16::from_le_bytes([rec[6], rec[7]]);
        let value = i32::from_le_bytes([rec[8], rec[9], rec[10], rec[11]]);
        let (insert, slot) = (((target >> 6) & 0x7f) as usize, (target & 0x3f));
        let Some(ins) = inserts.get_mut(insert) else { continue };
        match id {
            VOLUME => ins.volume = value.max(0) as u32,
            PAN => ins.pan = value,
            SLOT_ENABLED | SLOT_MIX => {
                if let Some(s) = ins.slots.iter_mut().find(|s| s.index == slot) {
                    if id == SLOT_ENABLED {
                        s.enabled = value != 0;
                    } else {
                        s.mix = value.max(0) as u32;
                    }
                }
            }
            _ => {}
        }
    }
}

fn apply_plugin_event(plugin: &mut Plugin, ev: &Event, utf16: bool) -> Result<(), Error> {
    match ev.id {
        PLUGIN_INTERNAL_NAME => plugin.internal_name = decode_text(ev.as_bytes(), utf16),
        PLUGIN_NAME => plugin.display_name = decode_text(ev.as_bytes(), utf16),
        PLUGIN_DATA => {
            plugin.data = ev.as_bytes().to_vec();
            if plugin.internal_name == "Fruity Wrapper" {
                plugin.wrapped = Some(parse_wrapper(ev.as_bytes(), ev.offset)?);
            }
        }
        _ => {}
    }
    Ok(())
}

/// FL's third-party plugin wrapper stores a `u32` kind followed by
/// `(u32 id, u64 length, bytes)` records.
fn parse_wrapper(data: &[u8], offset: usize) -> Result<WrappedPlugin, Error> {
    const NAME: u32 = 54;
    const PATH: u32 = 55;
    const VENDOR: u32 = 56;
    const FOURCC: u32 = 51;
    const GUID: u32 = 52;
    const STATE: u32 = 53;

    let mut out = WrappedPlugin::default();
    let mut r = Reader::new(data, offset);
    let _kind = r.u32()?;
    while let Ok(id) = r.u32() {
        let len = r.u64()? as usize;
        let bytes = r.take(len)?;
        let utf8 = || String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string();
        match id {
            NAME => out.name = Some(utf8()),
            PATH => out.path = Some(utf8()),
            VENDOR => out.vendor = Some(utf8()),
            FOURCC => out.fourcc = Some(utf8()),
            GUID => out.guid = Some(bytes.to_vec()),
            STATE => out.state = Some(bytes.to_vec()),
            _ => {}
        }
    }
    Ok(out)
}

fn parse_notes(data: &[u8]) -> impl Iterator<Item = Note> + '_ {
    data.as_chunks::<24>().0.iter().map(|n| {
        let u16_at = |i: usize| u16::from_le_bytes([n[i], n[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([n[i], n[i + 1], n[i + 2], n[i + 3]]);
        Note {
            position: u32_at(0),
            flags: u16_at(4),
            channel: u16_at(6),
            length: u32_at(8),
            key: u16_at(12),
            fine_pitch: n[16],
            release: n[18],
            pan: n[20],
            velocity: n[21],
            mod_x: n[22],
            mod_y: n[23],
        }
    })
}

fn parse_playlist(data: &[u8]) -> Vec<PlaylistItem> {
    // FL 21 grew items from 32 to 60 bytes; only a 32-byte layout divides evenly by 32 but not 60.
    let size = if data.len().is_multiple_of(32) && !data.len().is_multiple_of(60) { 32 } else { 60 };
    data.chunks_exact(size)
        .map(|it| {
            let u16_at = |i: usize| u16::from_le_bytes([it[i], it[i + 1]]);
            let u32_at = |i: usize| u32::from_le_bytes([it[i], it[i + 1], it[i + 2], it[i + 3]]);
            let f32_at = |i: usize| f32::from_le_bytes([it[i], it[i + 1], it[i + 2], it[i + 3]]);
            let pattern_base = u16_at(4);
            let item_index = u16_at(6);
            let base = if pattern_base == 0 { PATTERN_BASE } else { pattern_base };
            let source = if item_index >= base {
                ItemSource::Pattern(item_index - base)
            } else {
                ItemSource::Channel(item_index)
            };
            PlaylistItem {
                position: u32_at(0),
                length: u32_at(8),
                track: TRACK_INDEX_TOP.saturating_sub(u16_at(12)),
                source,
                group: u16_at(14),
                start_offset: f32_at(24),
                end_offset: f32_at(28),
            }
        })
        .collect()
}

fn decode_text(bytes: &[u8], utf16: bool) -> String {
    let s = if utf16 {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    s.trim_end_matches('\0').to_string()
}

fn version_at_least(version: &str, major: u32, minor: u32) -> bool {
    let mut parts = version.split('.').map(|s| s.parse::<u32>().unwrap_or(0));
    let v = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    v >= (major, minor)
}
