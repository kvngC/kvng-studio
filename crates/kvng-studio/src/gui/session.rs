//! An open project: the editable song, the running engine's controls, and UI state.

use std::path::PathBuf;
use std::sync::Arc;

use engine::control::Controls;
use engine::command::{Command, CommandSender};
use engine::song::{ChannelSpec, EffectSource, EffectSpec, PluginSpec, Song, SourceSpec};
use engine::view::ChannelStatus;
use engine::{Channel, Effect, PluginInstance, Processor, Source};
use engine::timeline::{self, TimelineSender};
use engine::view::{AutoPoint, AutomationTarget, AutomationView, ClipKind, ClipView, PatternView, ProjectView};

use super::audio;
use super::widgets::MeterState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Loop the selected pattern (FL's PAT).
    Pattern,
    /// Play the playlist (FL's SONG).
    Song,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    None,
    Sixteenth,
    Eighth,
    Beat,
    Bar,
}

impl Snap {
    pub const ALL: [Snap; 5] = [Snap::Sixteenth, Snap::Eighth, Snap::Beat, Snap::Bar, Snap::None];

    pub fn label(self) -> &'static str {
        match self {
            Snap::None => "None",
            Snap::Sixteenth => "1/4 beat",
            Snap::Eighth => "1/2 beat",
            Snap::Beat => "Beat",
            Snap::Bar => "Bar",
        }
    }

    pub fn ticks(self, ppq: u64, beats_per_bar: u64) -> u64 {
        match self {
            Snap::None => 1,
            Snap::Sixteenth => (ppq / 4).max(1),
            Snap::Eighth => (ppq / 2).max(1),
            Snap::Beat => ppq,
            Snap::Bar => ppq * beats_per_bar,
        }
    }
}

/// What undo restores: patterns, playlist clips and automation curves.
/// Everything undo can bring back.
#[derive(Clone)]
struct Snapshot {
    patterns: Vec<PatternView>,
    clips: Vec<ClipView>,
    automations: Vec<AutomationView>,
    strips: Vec<StripSettings>,
    tracks: Vec<engine::view::TrackView>,
    markers: Vec<engine::view::Marker>,
    tempo_changes: Vec<engine::view::TempoChange>,
    meter_changes: Vec<engine::view::MeterChange>,
}

use engine::channel_strip::StripSettings;

pub struct Session {
    pub view: ProjectView,
    pub controls: Arc<Controls>,
    timeline: TimelineSender,
    commands: CommandSender,
    is_audio_clip: Vec<bool>,
    _stream: audio::Stream,
    pub device_name: String,

    pub mode: Mode,
    pub pattern: usize,
    /// Channel shown in the piano roll and highlighted in the rack.
    pub channel: usize,
    pub snap: Snap,
    /// Loop region set on the playlist ruler, in ticks.
    pub song_loop: Option<(u64, u64)>,

    pub channel_meters: Vec<MeterState>,
    pub insert_meters: Vec<MeterState>,
    undo: Vec<Snapshot>,
    strip_gesture: Option<(usize, std::time::Instant)>,
    edit_gesture: Option<(u64, std::time::Instant)>,
    /// Automation being recorded: target → (automation index, last written song tick).
    auto_recording: Vec<(AutomationTarget, usize, u64)>,
    /// When plugins were last asked for parameter changes (None = not recording).
    plugin_poll: Option<std::time::Instant>,
    /// Recording input (from Settings; None = system default).
    pub input_device: Option<String>,
    /// Recording options from Settings.
    pub count_in_bars: u32,
    pub metronome_when_recording: bool,
    pub monitor_input: bool,
    /// Recording latency compensation in ms (None = automatic).
    pub record_offset_ms: Option<f32>,
    /// Output buffer size, for automatic latency compensation.
    pub buffer_frames: Option<u32>,
    /// Mixer track armed for audio recording (FL's mixer record arm): takes are routed there.
    pub record_insert: Option<usize>,
    /// The metronome setting to restore after recording turned it on.
    metronome_before_recording: Option<bool>,
    /// Identifies this session's autosave when it has no file yet.
    pub session_id: u64,
    /// Channels skipped at load that something now plays (loaded by the app in the background).
    load_requests: Vec<usize>,
    loading_channels: std::collections::HashSet<usize>,
    redo: Vec<Snapshot>,
    /// Set by edits; the timeline is recompiled once per frame.
    dirty: bool,
    /// Last note length used in the piano roll (FL reuses it for new notes).
    pub last_note_length: u64,

    /// The document this session was built from (sources, plugin identities).
    song: Song,
    /// Where the project is saved as `.kvng`; `None` until first saved (e.g. an FL import).
    pub path: Option<PathBuf>,
    /// Changed since the last save.
    pub modified: bool,
    /// Note being auditioned: channel, key, and when to release it automatically.
    preview: Option<(usize, u8, Option<std::time::Instant>)>,
    /// MIDI keyboards and the computer keyboard.
    pub live: super::midi::LiveInput,
    /// Record live notes into the current pattern while playing.
    pub recording: bool,
    /// Record audio from the input device while playing.
    pub audio_armed: bool,
    /// Playlist track selected for the next audio take; None creates a new lane.
    pub record_track: Option<usize>,
    /// One playlist track listened to in isolation.
    pub solo_track: Option<usize>,
    recorder: Option<super::recorder::Recorder>,
    /// Keys held during recording: key → (start tick in the pattern, velocity).
    held: std::collections::HashMap<u8, (u64, u8)>,
    /// One undo step per recording take.
    modified_by_recording: bool,
    /// Waiting for a MIDI controller to link to this control.
    pub learning: Option<AutomationTarget>,
}

impl Session {
    pub fn new(mut loaded: engine::load::Loaded, output: &audio::Output, path: Option<PathBuf>) -> Result<Session, String> {
        let controls = loaded.engine.controls();
        let timeline = loaded.engine.take_timeline_sender().ok_or("engine timeline already taken")?;
        let commands = loaded.engine.take_command_sender().ok_or("engine commands already taken")?;
        let prefs = super::prefs::Prefs::load();
        let live = super::midi::LiveInput::connect(commands.note_sender(), &prefs.disabled_midi_inputs);
        let view = loaded.view;
        let is_audio_clip = view.channels.iter().map(|c| c.kind == "Audio clip").collect();
        let stream = audio::start(output, loaded.engine)?;

        // Start where FL users expect: the first pattern that has notes, first channel it uses.
        let pattern = view.patterns.iter().position(|p| !p.notes.is_empty()).unwrap_or(0);
        let channel = view.patterns.get(pattern).and_then(|p| p.notes.first()).map_or(0, |n| n.channel);
        let mode = if view.clips.is_empty() { Mode::Pattern } else { Mode::Song };

        let mut session = Session {
            channel_meters: vec![MeterState::default(); view.channels.len()],
            insert_meters: vec![MeterState::default(); view.inserts.len()],
            last_note_length: view.ppq as u64,
            view,
            controls,
            timeline,
            commands,
            is_audio_clip,
            _stream: stream,
            device_name: output.device_name.clone(),
            mode,
            pattern,
            channel,
            snap: Snap::Sixteenth,
            song_loop: None,
            undo: Vec::new(),
            strip_gesture: None,
            edit_gesture: None,
            auto_recording: Vec::new(),
            plugin_poll: None,
            input_device: prefs.input_device.clone(),
            count_in_bars: prefs.count_in_bars,
            metronome_when_recording: prefs.metronome_when_recording,
            monitor_input: prefs.monitor_input,
            record_offset_ms: prefs.record_offset_ms,
            buffer_frames: None,
            record_insert: None,
            metronome_before_recording: None,
            load_requests: Vec::new(),
            loading_channels: Default::default(),
            session_id: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64),
            redo: Vec::new(),
            dirty: false,
            song: loaded.song,
            path,
            modified: false,
            preview: None,
            live,
            recording: false,
            audio_armed: false,
            record_track: None,
            solo_track: None,
            recorder: None,
            held: std::collections::HashMap::new(),
            modified_by_recording: false,
            learning: None,
        };
        session.sync_metronome();
        session.recompile();
        Ok(session)
    }

    pub fn ppq(&self) -> u64 {
        self.view.ppq as u64
    }

    pub fn ticks_per_bar(&self) -> u64 {
        self.view.ticks_per_bar()
    }

    pub fn snap_ticks(&self) -> u64 {
        self.snap.ticks(self.ppq(), self.view.beats_per_bar as u64)
    }

    /// Current playhead in ticks.
    pub fn position_ticks(&self) -> f64 {
        self.view.sample_to_tick(self.controls.position())
    }

    pub fn is_playing(&self) -> bool {
        self.controls.is_playing()
    }

    pub fn toggle_play(&self) {
        if self.is_playing() {
            self.controls.stop();
        } else {
            if (self.recording || self.audio_armed) && self.count_in_bars > 0 {
                let spb = self.controls.samples_per_beat.load().max(1.0) as u64;
                self.controls.start_count_in(spb * u64::from(self.view.beats_per_bar.max(1)) * u64::from(self.count_in_bars));
            }
            self.controls.play();
        }
    }

    /// Takes the Settings window's recording options.
    pub fn apply_prefs(&mut self, prefs: &super::prefs::Prefs) {
        self.input_device = prefs.input_device.clone();
        self.count_in_bars = prefs.count_in_bars;
        self.metronome_when_recording = prefs.metronome_when_recording;
        self.monitor_input = prefs.monitor_input;
        self.controls.metronome_gain.store(prefs.metronome_volume.clamp(0.0, 2.0));
        self.record_offset_ms = prefs.record_offset_ms;
    }

    /// Whether the count-in before recording is playing.
    pub fn counting_in(&self) -> bool {
        self.is_playing() && self.controls.count_in_left() > 0
    }

    /// FL's stop: halt and return to the start.
    pub fn stop(&self) {
        self.controls.stop();
        self.controls.seek(0);
    }

    pub fn seek_ticks(&self, ticks: f64) {
        self.controls.seek(self.view.tick_to_sample(ticks));
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.controls.seek(0);
            self.recompile();
        }
    }

    pub fn select_pattern(&mut self, pattern: usize) {
        if pattern < self.view.patterns.len() && pattern != self.pattern {
            self.pattern = pattern;
            if self.mode == Mode::Pattern {
                self.controls.seek(0);
                self.recompile();
            }
        }
    }

    pub fn set_song_loop(&mut self, range: Option<(u64, u64)>) {
        self.song_loop = range.filter(|(a, b)| b > a);
        if self.mode == Mode::Song {
            self.apply_loop(None);
        }
    }

    pub fn set_tempo(&mut self, tempo: f64) {
        let tempo = tempo.clamp(10.0, 522.0);
        if (tempo - self.view.tempo).abs() < 1e-6 {
            return;
        }
        let tick = self.position_ticks();
        self.modified = true;
        self.view.tempo = tempo;
        self.view.samples_per_tick = self.view.sample_rate * 60.0 / (tempo * self.view.ppq as f64);
        self.sync_metronome();
        self.recompile();
        self.seek_ticks(tick);
    }

    /// Notes that something changed that should be saved (mixer moves, mutes, ...).
    pub fn touch(&mut self) {
        self.modified = true;
    }

    /// The current state as a document, including each plugin's current settings.
    pub fn to_song(&self) -> (Song, Vec<String>) {
        engine::song::capture(&self.song, &self.view, &self.controls)
    }

    /// A cheap, owned snapshot for background autosave. Plugin state is fetched
    /// from helpers on the worker, after the UI frame has returned.
    pub fn autosave_snapshot(&self) -> (Song, engine::view::ProjectView, Arc<engine::control::Controls>) {
        (self.song.clone(), self.view.clone(), self.controls.clone())
    }

    /// Adds an instrument channel playing `plugin` (already loaded) and selects it.
    /// Returns the new channel's index.
    pub fn add_instrument(&mut self, spec: PluginSpec, plugin: PluginInstance) -> Result<usize, String> {
        let index = self.view.channels.len();
        if index >= engine::MAX_CHANNELS {
            return Err(format!("a project can have at most {} channels", engine::MAX_CHANNELS));
        }
        let name = unique_name(&spec.name, self.view.channels.iter().filter(|c| !c.deleted).map(|c| c.name.as_str()));
        let channel = Channel::with_gain(name.clone(), Source::Plugin { plugin: Box::new(plugin), midi: Vec::with_capacity(256) }, 1.0, 0.0, 0);
        let channel_spec = ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(), name, source: SourceSpec::Plugin(spec), volume: 1.0, pan: 0.0, route: 0, muted: false, root_note: 60 };
        let view = engine::load::channel_view(&channel, &channel_spec, ChannelStatus::Ok);

        let strip = &self.controls.channels[index];
        strip.volume.store(1.0);
        strip.pan.store(0.0);
        strip.set_route(0);
        strip.mute.store(false, std::sync::atomic::Ordering::Relaxed);
        self.commands.send(Command::AddChannel(Box::new(channel)))?;

        self.view.channels.push(view);
        self.song.channels.push(channel_spec);
        self.is_audio_clip.push(false);
        self.channel_meters.push(MeterState::default());
        self.channel = index;
        self.modified = true;
        // FL gives each new channel its own mixer track.
        self.route_to_free_insert(index);
        Ok(index)
    }

    /// Appends a plugin effect to mixer insert `insert`.
    pub fn add_effect(&mut self, insert: usize, spec: PluginSpec, plugin: PluginInstance) -> Result<(), String> {
        let Some(ins_view) = self.view.inserts.get(insert) else { return Err("no such insert".into()) };
        if ins_view.effects.len() >= engine::MAX_EFFECTS {
            return Err(format!("an insert holds at most {} effects", engine::MAX_EFFECTS));
        }
        let effect = Effect { name: spec.name.clone(), processor: Processor::Plugin(Box::new(plugin)), mix: 1.0, bypass: false };
        let view = engine::load::effect_view(&effect);
        let index = ins_view.effects.len();
        self.commands.send(Command::InsertEffect { insert, index, effect: Box::new(effect) })?;
        self.view.inserts[insert].effects.push(view);
        self.view.inserts[insert].used = true;
        if let Some(song_ins) = self.song.inserts.get_mut(insert) {
            song_ins.effects.push(EffectSpec { name: spec.name.clone(), source: EffectSource::Plugin(spec), mix: 1.0, enabled: true, bypassed: false });
        }
        self.modified = true;
        Ok(())
    }

    /// The document entry for the `index`-th loaded effect of `insert` (effects that
    /// failed to load are in the document but not the view).
    fn song_effect_index(&self, insert: usize, index: usize) -> Option<usize> {
        let name = &self.view.inserts.get(insert)?.effects.get(index)?.name;
        let preceding = self.view.inserts[insert].effects[..index].iter().filter(|e| &e.name == name).count();
        self.song.inserts.get(insert)?.effects.iter().enumerate().filter(|(_, e)| e.enabled && &e.name == name).nth(preceding).map(|(i, _)| i)
    }

    /// The plugin a channel plays (for presets), if it's a third-party plugin.
    pub fn channel_plugin_name(&self, channel: usize) -> Option<String> {
        match &self.song.channels.get(channel)?.source {
            SourceSpec::Plugin(p) => Some(p.name.clone()),
            _ => None,
        }
    }

    /// The plugin in an effect slot (for presets), if it's a third-party plugin.
    pub fn effect_plugin_name(&self, insert: usize, index: usize) -> Option<String> {
        let i = self.song_effect_index(insert, index)?;
        match &self.song.inserts[insert].effects[i].source {
            EffectSource::Plugin(p) => Some(p.name.clone()),
            _ => None,
        }
    }

    /// An insert's effects with their current plugin settings (for saving as a chain).
    pub fn effect_chain(&self, insert: usize) -> Vec<engine::song::EffectSpec> {
        let (song, _) = self.to_song();
        song.inserts.get(insert).map(|i| i.effects.iter().filter(|e| e.enabled).cloned().collect()).unwrap_or_default()
    }

    /// Adds one of KVNG Studio's effects to the end of `insert`'s chain.
    pub fn add_builtin_effect(&mut self, insert: usize, effect: engine::effects::BuiltinEffect) -> Result<usize, String> {
        let Some(ins_view) = self.view.inserts.get(insert) else { return Err("no such insert".into()) };
        if ins_view.effects.len() >= engine::MAX_EFFECTS {
            return Err(format!("an insert holds at most {} effects", engine::MAX_EFFECTS));
        }
        let name = effect.name().to_string();
        let fx = Effect {
            name: name.clone(),
            processor: Processor::Builtin(Box::new(engine::effects::Builtin::new(effect.clone(), self.view.sample_rate))),
            mix: 1.0,
            bypass: false,
        };
        let view = engine::load::effect_view(&fx);
        let index = ins_view.effects.len();
        self.commands.send(Command::InsertEffect { insert, index, effect: Box::new(fx) })?;
        self.view.inserts[insert].effects.push(view);
        self.view.inserts[insert].used = true;
        if let Some(song_ins) = self.song.inserts.get_mut(insert) {
            song_ins.effects.push(EffectSpec { name, source: EffectSource::Builtin { effect }, mix: 1.0, enabled: true, bypassed: false });
        }
        self.modified = true;
        Ok(index)
    }

    /// Settings of a built-in effect in a mixer slot.
    pub fn builtin_effect(&self, insert: usize, index: usize) -> Option<engine::effects::BuiltinEffect> {
        let i = self.song_effect_index(insert, index)?;
        match &self.song.inserts[insert].effects[i].source {
            EffectSource::Builtin { effect } => Some(effect.clone()),
            _ => None,
        }
    }

    pub fn set_builtin_effect(&mut self, insert: usize, index: usize, effect: engine::effects::BuiltinEffect) {
        let Some(i) = self.song_effect_index(insert, index) else { return };
        if self.commands.send(Command::SetBuiltinEffect { insert, index, params: Box::new(effect.clone()) }).is_ok() {
            self.song.inserts[insert].effects[i].source = EffectSource::Builtin { effect };
            self.modified = true;
        }
    }

    /// Removes the `index`-th loaded effect from `insert`.
    pub fn remove_effect(&mut self, insert: usize, index: usize) -> Result<(), String> {
        let song_index = self.song_effect_index(insert, index);
        self.commands.send(Command::RemoveEffect { insert, index })?;
        if let (Some(pos), Some(song_ins)) = (song_index, self.song.inserts.get_mut(insert)) {
            song_ins.effects.remove(pos);
        }
        self.view.inserts[insert].effects.remove(index);
        self.modified = true;
        Ok(())
    }

    /// Sends insert `insert` to `routes` (0 = master).
    pub fn set_routes(&mut self, insert: usize, routes: Vec<usize>) {
        if insert == 0 || insert >= self.view.inserts.len() {
            return;
        }
        // Keep each destination's existing send level.
        let ins = &self.view.inserts[insert];
        let levels: Vec<f32> = routes.iter().map(|d| ins.routes_to.iter().position(|x| x == d).and_then(|k| ins.route_levels.get(k).copied()).unwrap_or(1.0)).collect();
        self.set_sends(insert, routes, levels);
    }

    /// Sets an insert's destinations and send levels (linear). Destinations that would
    /// feed back into this insert are dropped.
    pub fn set_sends(&mut self, insert: usize, routes: Vec<usize>, levels: Vec<f32>) {
        if insert == 0 || insert >= self.view.inserts.len() {
            return;
        }
        let (routes, levels): (Vec<usize>, Vec<f32>) = routes.into_iter().zip(levels).filter(|(d, _)| *d != insert && !self.reaches(*d, insert)).unzip();
        if self.commands.send(Command::SetRoutes { insert, routes: routes.clone(), levels: levels.clone() }).is_ok() {
            for &d in &routes {
                self.view.inserts[d].used = true;
            }
            self.view.inserts[insert].routes_to = routes;
            self.view.inserts[insert].route_levels = levels;
            self.modified = true;
        }
    }

    /// Changes the level of the send from `insert` to `dest` (linear).
    pub fn set_send_level(&mut self, insert: usize, dest: usize, level: f32) {
        let Some(ins) = self.view.inserts.get(insert) else { return };
        let Some(k) = ins.routes_to.iter().position(|&d| d == dest) else { return };
        let (routes, mut levels) = (ins.routes_to.clone(), ins.route_levels.clone());
        levels.resize(routes.len(), 1.0);
        levels[k] = level.clamp(0.0, 2.0);
        self.set_sends(insert, routes, levels);
    }

    /// Whether audio (or a sidechain key) from `from` can reach `to` through the routing.
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        let mut stack = vec![from];
        let mut seen = vec![false; self.view.inserts.len()];
        while let Some(i) = stack.pop() {
            if i == to {
                return true;
            }
            if i >= seen.len() || std::mem::replace(&mut seen[i], true) {
                continue;
            }
            stack.extend(self.view.inserts[i].routes_to.iter().chain(&self.view.inserts[i].sidechain_to).copied());
        }
        false
    }

    /// Makes `insert` key the compressor of each of `targets` (sidechain sends).
    pub fn set_sidechains(&mut self, insert: usize, targets: Vec<usize>) {
        if insert >= self.view.inserts.len() {
            return;
        }
        let targets: Vec<usize> = targets.into_iter().filter(|&t| t != insert && t < self.view.inserts.len() && !self.reaches(t, insert)).collect();
        if self.commands.send(Command::SetSidechains { insert, targets: targets.clone() }).is_ok() {
            self.view.inserts[insert].sidechain_to = targets;
            self.modified = true;
        }
    }

    /// Inserts that send a sidechain key to `insert`.
    pub fn sidechain_sources(&self, insert: usize) -> Vec<usize> {
        self.view.inserts.iter().filter(|i| i.sidechain_to.contains(&insert)).map(|i| i.index).collect()
    }

    pub fn set_effect_bypass(&mut self, insert: usize, index: usize, bypass: bool) {
        if self.commands.send(Command::SetEffectBypass { insert, index, bypass }).is_ok()
            && let Some(fx) = self.view.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(index))
        {
            fx.bypass = bypass;
            self.modified = true;
        }
    }

    pub fn set_effect_mix(&mut self, insert: usize, index: usize, mix: f32) {
        if self.commands.send(Command::SetEffectMix { insert, index, mix }).is_ok()
            && let Some(fx) = self.view.inserts.get_mut(insert).and_then(|i| i.effects.get_mut(index))
        {
            fx.mix = mix.clamp(0.0, 1.0);
            self.modified = true;
        }
    }

    pub fn rename_insert(&mut self, insert: usize, name: &str) {
        let name = name.trim();
        if let Some(i) = self.view.inserts.get_mut(insert)
            && !name.is_empty()
        {
            i.name = name.to_string();
            self.modified = true;
        }
    }

    pub fn rename_channel(&mut self, channel: usize, name: String) {
        let name = name.trim().to_string();
        if let Some(c) = self.view.channels.get_mut(channel)
            && !name.is_empty()
            && c.name != name
        {
            c.name = name;
            self.modified = true;
        }
    }

    /// Deletes a channel and its notes (undo restores the notes, not the channel).
    pub fn delete_channel(&mut self, channel: usize) -> Result<(), String> {
        if channel >= self.view.channels.len() {
            return Err("no such channel".into());
        }
        self.commands.send(Command::ReplaceSource { channel, source: Box::new(Source::Silent) })?;
        self.checkpoint();
        for p in &mut self.view.patterns {
            p.notes.retain(|n| n.channel != channel);
        }
        self.view.clips.retain(|c| c.channel != Some(channel));
        let c = &mut self.view.channels[channel];
        c.deleted = true;
        c.editor = None;
        if self.channel == channel {
            self.channel = self.view.channels.iter().position(|c| !c.deleted).unwrap_or(0);
        }
        self.mark_dirty();
        Ok(())
    }

    /// Starts auditioning `key` on `channel` (call [`Session::preview_off`] to release).
    /// The first mixer insert nothing uses (no channel feeds it, no effects, default routing),
    /// FL-style, so each new channel gets its own track.
    pub fn free_insert(&self) -> Option<usize> {
        (1..self.view.inserts.len()).find(|&i| {
            let ins = &self.view.inserts[i];
            !ins.used
                && ins.effects.is_empty()
                && ins.routes_to == [0]
                && !self.view.channels.iter().enumerate().any(|(c, ch)| !ch.deleted && self.controls.channels.get(c).is_some_and(|s| s.route() == i))
                && self.view.inserts.iter().all(|other| !other.routes_to.contains(&i))
        })
    }

    /// Gives `channel` its own free mixer track named after it (stays on its current
    /// insert if none is free). Returns the insert.
    pub fn route_to_free_insert(&mut self, channel: usize) -> usize {
        let Some(i) = self.free_insert() else { return self.controls.channels.get(channel).map_or(0, |s| s.route()) };
        let name = self.view.channels.get(channel).map(|c| c.name.clone()).unwrap_or_default();
        self.rename_insert(i, &name);
        self.route_channel(channel, i);
        i
    }

    /// Sends `channel` to mixer insert `insert`.
    pub fn route_channel(&mut self, channel: usize, insert: usize) {
        if insert >= self.view.inserts.len() {
            return;
        }
        if let Some(strip) = self.controls.channels.get(channel) {
            strip.set_route(insert);
        }
        if let Some(c) = self.view.channels.get_mut(channel) {
            c.insert = insert;
        }
        if let Some(c) = self.song.channels.get_mut(channel) {
            c.route = insert;
        }
        self.view.inserts[insert].used = true;
        self.modified = true;
    }

    /// Adds the next free mixer track to the mixer view. Returns it.
    pub fn add_mixer_track(&mut self) -> Option<usize> {
        let i = self.free_insert()?;
        self.view.inserts[i].used = true;
        self.modified = true;
        Some(i)
    }

    /// Swaps what a channel plays, keeping its notes, mixer routing, volume and pan.
    /// The channel takes the new sound's name.
    pub fn replace_channel(&mut self, channel: usize, base_name: &str, spec: SourceSpec, source: Source) -> Result<(), String> {
        if self.view.channels.get(channel).is_none_or(|c| c.deleted) {
            return Err("no such channel".into());
        }
        // Close the old plugin's window first; the helper goes away with its source.
        if let Some(h) = &self.view.channels[channel].editor {
            let _ = h.close();
        }
        let name = unique_name(base_name, self.view.channels.iter().enumerate().filter(|(i, c)| *i != channel && !c.deleted).map(|(_, c)| c.name.as_str()));
        let audio_clip = matches!(spec, SourceSpec::AudioClip { .. });
        let mut new_spec = self.song.channels[channel].clone();
        new_spec.source = spec;
        new_spec.name = name.clone();
        let temp = Channel::with_gain(name.clone(), source, 1.0, 0.0, 0);
        let fresh = engine::load::channel_view(&temp, &new_spec, ChannelStatus::Ok);
        self.commands.send(Command::ReplaceSource { channel, source: Box::new(temp.source) })?;
        // Keep the mixer track's name in step when it was named after the channel.
        let insert = self.controls.channels.get(channel).map_or(0, |s| s.route());
        if insert > 0 && self.view.inserts.get(insert).is_some_and(|i| i.name == self.view.channels[channel].name) {
            self.rename_insert(insert, &name);
        }
        let v = &mut self.view.channels[channel];
        (v.name, v.kind, v.detail, v.waveform, v.status, v.editor) = (name, fresh.kind, fresh.detail, fresh.waveform, ChannelStatus::Ok, fresh.editor);
        self.song.channels[channel] = new_spec;
        if let Some(flag) = self.is_audio_clip.get_mut(channel) {
            *flag = audio_clip;
        }
        self.loading_channels.remove(&channel);
        self.modified = true;
        Ok(())
    }

    /// Replaces a channel's sound with a sample (an audio clip channel stays an audio clip).
    pub fn replace_with_sample(&mut self, channel: usize, path: &std::path::Path, sample: Arc<engine::sample::Sample>) -> Result<(), String> {
        let raw = path.to_string_lossy().into_owned();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Sample".into());
        let root = self.song.channels.get(channel).map_or(60, |c| c.root_note);
        let (spec, source) = if self.is_audio_clip.get(channel).copied().unwrap_or(false) {
            (SourceSpec::AudioClip { path: raw }, Source::AudioClip { sample, voices: Vec::with_capacity(32) })
        } else {
            (SourceSpec::Sampler { path: raw }, Source::Sampler { sample, root_note: root, voices: Vec::with_capacity(64) })
        };
        self.replace_channel(channel, &stem, spec, source)
    }

    /// Replaces a channel's sound with a built-in instrument.
    pub fn replace_with_builtin(&mut self, channel: usize, spec: SourceSpec) -> Result<(), String> {
        let sr = self.view.sample_rate;
        let root = self.song.channels.get(channel).map_or(60, |c| c.root_note);
        let (name, source) = match &spec {
            SourceSpec::Synth { params } => ("KVNG Synth".to_string(), Source::Synth(Box::new(engine::instrument::Synth::new(params.clone(), sr)))),
            SourceSpec::Drum { drum } => (drum.label().to_string(), Source::Sampler { sample: Arc::new(drum.render(sr)), root_note: root, voices: Vec::with_capacity(64) }),
            SourceSpec::MidiOut { port, midi_channel } => ("MIDI Out".to_string(), Source::MidiOut { out: Box::new(engine::midi_out::MidiOut::open(port)?), midi_channel: *midi_channel }),
            _ => return Err("not a built-in instrument".into()),
        };
        self.replace_channel(channel, &name, spec, source)
    }

    /// Replaces a channel's sound with a loaded plugin.
    pub fn replace_with_plugin(&mut self, channel: usize, spec: PluginSpec, plugin: PluginInstance) -> Result<(), String> {
        let name = spec.name.clone();
        self.replace_channel(channel, &name, SourceSpec::Plugin(spec), Source::Plugin { plugin: Box::new(plugin), midi: Vec::with_capacity(256) })
    }

    /// Asks for a not-yet-loaded channel to be loaded (no-op otherwise).
    pub fn request_load(&mut self, channel: usize) {
        if self.view.channels.get(channel).is_some_and(|c| c.status == ChannelStatus::Unused && !c.deleted) && self.loading_channels.insert(channel) {
            self.load_requests.push(channel);
        }
    }

    /// Channels to load now: (index, spec, project folder, sample rate).
    pub fn take_load_requests(&mut self) -> Vec<(usize, ChannelSpec, Option<PathBuf>, f64)> {
        let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        std::mem::take(&mut self.load_requests)
            .into_iter()
            .filter_map(|c| self.song.channels.get(c).map(|spec| (c, spec.clone(), dir.clone(), self.view.sample_rate)))
            .collect()
    }

    /// Installs a channel loaded in the background.
    pub fn finish_load(&mut self, channel: usize, source: Source, status: ChannelStatus) -> Result<(), String> {
        self.loading_channels.remove(&channel);
        let Some(spec) = self.song.channels.get(channel) else { return Err("no such channel".into()) };
        let temp = Channel::with_gain(spec.name.clone(), source, 1.0, 0.0, 0);
        let fresh = engine::load::channel_view(&temp, spec, status);
        self.commands.send(Command::ReplaceSource { channel, source: Box::new(temp.source) })?;
        if let Some(v) = self.view.channels.get_mut(channel) {
            (v.kind, v.detail, v.waveform, v.status, v.editor) = (fresh.kind, fresh.detail, fresh.waveform, status, fresh.editor);
        }
        Ok(())
    }

    pub fn preview_on(&mut self, channel: usize, key: u8, velocity: u8) {
        self.request_load(channel);
        if let Some((c, k, _)) = self.preview.take() {
            let _ = self.commands.send(Command::Preview { channel: c, key: k, velocity: 0 });
        }
        if self.commands.send(Command::Preview { channel, key, velocity: velocity.max(1) }).is_ok() {
            self.preview = Some((channel, key, None));
        }
    }

    /// Auditions a short note that releases itself (step sequencer clicks).
    pub fn preview_blip(&mut self, channel: usize, key: u8) {
        self.request_load(channel);
        self.preview_on(channel, key, 100);
        if let Some(p) = &mut self.preview {
            p.2 = Some(std::time::Instant::now() + std::time::Duration::from_millis(180));
        }
    }

    pub fn preview_off(&mut self) {
        if let Some((channel, key, _)) = self.preview.take() {
            let _ = self.commands.send(Command::Preview { channel, key, velocity: 0 });
        }
    }

    /// Releases a timed preview once it's due (call every frame).
    pub fn tick_preview(&mut self) {
        if let Some((_, _, Some(due))) = self.preview
            && std::time::Instant::now() >= due
        {
            self.preview_off();
        }
    }

    /// The settings of a KVNG Synth channel.
    pub fn synth_params(&self, channel: usize) -> Option<engine::instrument::SynthParams> {
        match self.song.channels.get(channel).map(|c| &c.source) {
            Some(SourceSpec::Synth { params }) => Some(params.clone()),
            _ => None,
        }
    }

    /// Changes a KVNG Synth channel's settings (heard immediately, saved with the project).
    pub fn set_synth_params(&mut self, channel: usize, params: engine::instrument::SynthParams) {
        if let Some(ChannelSpec { source: SourceSpec::Synth { params: p }, .. }) = self.song.channels.get_mut(channel) {
            *p = params.clone();
            let _ = self.commands.send(Command::SetSynth { channel, params: Box::new(params) });
            self.modified = true;
        }
    }

    /// A MIDI Out channel's port and MIDI channel (0..=15).
    pub fn midi_out(&self, channel: usize) -> Option<(String, u8)> {
        match self.song.channels.get(channel).map(|c| &c.source) {
            Some(SourceSpec::MidiOut { port, midi_channel }) => Some((port.clone(), *midi_channel)),
            _ => None,
        }
    }

    /// Points a MIDI Out channel at another device or MIDI channel.
    pub fn set_midi_out(&mut self, channel: usize, port: String, midi_channel: u8) -> Result<(), String> {
        let Some(spec) = self.song.channels.get_mut(channel) else { return Err("no such channel".into()) };
        if !matches!(spec.source, SourceSpec::MidiOut { .. }) {
            return Err("not a MIDI Out channel".into());
        }
        let out = engine::midi_out::MidiOut::open(&port)?;
        let detail = format!("{} · channel {}", out.port, midi_channel + 1);
        spec.source = SourceSpec::MidiOut { port: out.port.clone(), midi_channel };
        self.commands.send(Command::ReplaceSource { channel, source: Box::new(Source::MidiOut { out: Box::new(out), midi_channel }) })?;
        if let Some(v) = self.view.channels.get_mut(channel) {
            v.detail = detail;
        }
        self.modified = true;
        Ok(())
    }

    /// A channel's sampler settings, if it plays samples.
    pub fn sampler_settings(&self, channel: usize) -> Option<engine::instrument::SamplerSettings> {
        let spec = self.song.channels.get(channel)?;
        matches!(spec.source, SourceSpec::Sampler { .. } | SourceSpec::Drum { .. } | SourceSpec::FlNative { .. } | SourceSpec::Slicer { .. })
            .then_some(spec.sampler)
            .filter(|_| matches!(self.view.channels.get(channel).map(|c| c.kind.as_str()), Some("Sampler" | "KVNG Drum" | "Replacement" | "Slicer")))
    }

    pub fn set_sampler_settings(&mut self, channel: usize, settings: engine::instrument::SamplerSettings) {
        if let Some(spec) = self.song.channels.get_mut(channel) {
            spec.sampler = settings;
            let _ = self.commands.send(Command::SetSampler { channel, settings });
            self.modified = true;
        }
    }

    /// Adds a built-in instrument (KVNG Synth or a drum) as a new channel.
    pub fn add_builtin(&mut self, source: SourceSpec) -> Result<usize, String> {
        let index = self.view.channels.len();
        if index >= engine::MAX_CHANNELS {
            return Err(format!("a project can have at most {} channels", engine::MAX_CHANNELS));
        }
        let sr = self.view.sample_rate;
        let (base, engine_source) = match &source {
            SourceSpec::Synth { params } => ("KVNG Synth".to_string(), Source::Synth(Box::new(engine::instrument::Synth::new(params.clone(), sr)))),
            SourceSpec::Drum { drum } => (drum.label().to_string(), Source::Sampler { sample: Arc::new(drum.render(sr)), root_note: 60, voices: Vec::with_capacity(64) }),
            SourceSpec::MidiOut { port, midi_channel } => {
                let out = engine::midi_out::MidiOut::open(port)?;
                ("MIDI Out".to_string(), Source::MidiOut { out: Box::new(out), midi_channel: *midi_channel })
            }
            _ => return Err("not a built-in instrument".into()),
        };
        let name = unique_name(&base, self.view.channels.iter().filter(|c| !c.deleted).map(|c| c.name.as_str()));
        let channel = Channel::with_gain(name.clone(), engine_source, 1.0, 0.0, 0);
        let channel_spec = ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(), name, source, volume: 1.0, pan: 0.0, route: 0, muted: false, root_note: 60 };
        let view = engine::load::channel_view(&channel, &channel_spec, ChannelStatus::Ok);
        let strip = &self.controls.channels[index];
        strip.volume.store(1.0);
        strip.pan.store(0.0);
        strip.set_route(0);
        strip.mute.store(false, std::sync::atomic::Ordering::Relaxed);
        self.commands.send(Command::AddChannel(Box::new(channel)))?;
        self.view.channels.push(view);
        self.song.channels.push(channel_spec);
        self.is_audio_clip.push(false);
        self.channel_meters.push(MeterState::default());
        self.channel = index;
        self.modified = true;
        // FL gives each new channel its own mixer track.
        self.route_to_free_insert(index);
        Ok(index)
    }

    /// Reconnects MIDI inputs (after plugging devices in or switching some off).
    pub fn reconnect_midi(&mut self, disabled: &[String]) {
        let target = self.channel;
        self.live = super::midi::LiveInput::connect(self.commands.note_sender(), disabled);
        self.live.set_target(target);
    }

    /// Copies every sample the project uses into `Samples/` next to the project and points
    /// the project at those copies with relative paths, so the folder can be moved or shared.
    /// Returns (files copied, problems).
    pub fn collect_samples(&mut self) -> Result<(usize, Vec<String>), String> {
        let Some(project) = self.path.clone() else { return Err("Save the project first, so there's a folder to collect into".into()) };
        let dir = project.parent().map(|d| d.to_path_buf()).ok_or("project has no folder")?;
        let target = dir.join(engine::sample::SAMPLES_DIR);
        let (mut copied, mut problems) = (0, Vec::new());
        for spec in &mut self.song.channels {
            let (SourceSpec::Sampler { path } | SourceSpec::AudioClip { path } | SourceSpec::Slicer { path, .. }) = &mut spec.source else { continue };
            let Some(file) = engine::sample::resolve_path(path, Some(&dir)) else {
                problems.push(format!("{}: sample not found ({path})", spec.name));
                continue;
            };
            let name = file.file_name().map(|n| n.to_owned()).unwrap_or_default();
            let dest = target.join(&name);
            let relative = format!("{}/{}", engine::sample::SAMPLES_DIR, name.to_string_lossy());
            if file.canonicalize().ok() != dest.canonicalize().ok() {
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                // Different files with the same name don't overwrite each other.
                let dest = if dest.exists() && std::fs::metadata(&dest).map(|m| m.len()).ok() != std::fs::metadata(&file).map(|m| m.len()).ok() {
                    let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let ext = dest.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "wav".into());
                    (2..).map(|i| target.join(format!("{stem} {i}.{ext}"))).find(|p| !p.exists()).unwrap()
                } else {
                    dest
                };
                if !dest.exists() {
                    std::fs::copy(&file, &dest).map_err(|e| format!("{}: {e}", file.display()))?;
                    copied += 1;
                }
                *path = format!("{}/{}", engine::sample::SAMPLES_DIR, dest.file_name().unwrap_or_default().to_string_lossy());
            } else {
                *path = relative;
            }
        }
        self.modified = true;
        Ok((copied, problems))
    }

    /// Channels whose sample file can't be found: (channel, file name).
    pub fn missing_samples(&self) -> Vec<(usize, String)> {
        let dir = self.path.as_ref().and_then(|p| p.parent());
        self.song
            .channels
            .iter()
            .enumerate()
            .filter(|(i, _)| self.view.channels.get(*i).is_some_and(|c| !c.deleted))
            .filter_map(|(i, spec)| match &spec.source {
                SourceSpec::Sampler { path } | SourceSpec::AudioClip { path } | SourceSpec::Slicer { path, .. } if engine::sample::resolve_path(path, dir).is_none() => {
                    Some((i, std::path::Path::new(&path.replace('\\', "/")).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
                }
                _ => None,
            })
            .collect()
    }

    /// Looks for the missing samples (by file name) anywhere under `folder` and relinks
    /// those it finds. Returns how many were relinked and the names still missing.
    pub fn relink_samples(&mut self, folder: &std::path::Path) -> (usize, Vec<String>) {
        let missing = self.missing_samples();
        if missing.is_empty() {
            return (0, vec![]);
        }
        let mut found: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
        let mut stack = vec![folder.to_path_buf()];
        let mut visited = 0;
        while let Some(d) = stack.pop() {
            visited += 1;
            if visited > 20_000 {
                break;
            }
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Some(n) = p.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
                    found.entry(n).or_insert(p);
                }
            }
        }
        let (mut relinked, mut still) = (0, Vec::new());
        for (channel, name) in missing {
            match found.get(&name.to_lowercase()) {
                Some(file) => match engine::sample::load_audio(file, self.view.sample_rate) {
                    Ok(sample) => {
                        let keep_name = self.view.channels[channel].name.clone();
                        if self.replace_with_sample(channel, file, Arc::new(sample)).is_ok() {
                            // Relinking shouldn't rename the channel.
                            self.view.channels[channel].name = keep_name.clone();
                            self.song.channels[channel].name = keep_name;
                            relinked += 1;
                        }
                    }
                    Err(e) => still.push(format!("{name} ({e})")),
                },
                None => still.push(name),
            }
        }
        (relinked, still)
    }

    /// A channel's colour: the user's choice, else one derived from its name.
    pub fn channel_color(&self, channel: usize) -> eframe::egui::Color32 {
        match self.view.channels.get(channel) {
            Some(c) => c.color.map_or_else(|| super::theme::clip_color(&c.name), |[r, g, b]| eframe::egui::Color32::from_rgb(r, g, b)),
            None => super::theme::TEXT_DIM,
        }
    }

    /// A mixer track's colour: its own, else that of a coloured channel feeding it, else
    /// one derived from its name.
    pub fn insert_color(&self, insert: usize) -> eframe::egui::Color32 {
        let Some(ins) = self.view.inserts.get(insert) else { return super::theme::TEXT_DIM };
        if let Some([r, g, b]) = ins.color {
            return eframe::egui::Color32::from_rgb(r, g, b);
        }
        let fed_by = self.view.channels.iter().enumerate().find(|(c, ch)| !ch.deleted && ch.color.is_some() && self.controls.channels.get(*c).is_some_and(|s| s.route() == insert));
        match fed_by {
            Some((c, _)) => self.channel_color(c),
            None => super::theme::clip_color(&ins.name),
        }
    }

    /// Colours a channel (and its own mixer track, when nothing else feeds that track).
    pub fn set_channel_color(&mut self, channel: usize, color: Option<[u8; 3]>) {
        let Some(c) = self.view.channels.get_mut(channel) else { return };
        c.color = color;
        let insert = self.controls.channels.get(channel).map_or(0, |s| s.route());
        let shared = self.view.channels.iter().enumerate().any(|(i, ch)| i != channel && !ch.deleted && self.controls.channels.get(i).is_some_and(|s| s.route() == insert));
        if insert > 0 && !shared {
            self.view.inserts[insert].color = color;
        }
        self.modified = true;
    }

    pub fn set_insert_color(&mut self, insert: usize, color: Option<[u8; 3]>) {
        if let Some(i) = self.view.inserts.get_mut(insert) {
            i.color = color;
            self.modified = true;
        }
    }

    /// Like [`Session::checkpoint`], but repeated edits of the same thing within half a
    /// second (one drag) share an undo step.
    pub fn checkpoint_gesture(&mut self, key: u64) {
        let now = std::time::Instant::now();
        if !self.edit_gesture.is_some_and(|(k, at)| k == key && now.duration_since(at).as_secs_f32() < 0.5) {
            self.checkpoint();
        }
        self.edit_gesture = Some((key, now));
    }

    /// The saved description of a channel (for settings that live there).
    pub fn song_channel_mut(&mut self, channel: usize) -> Option<&mut ChannelSpec> {
        self.modified = true;
        self.song.channels.get_mut(channel)
    }

    /// Imports a MIDI file: each MIDI track becomes a new KVNG Synth channel, and all the
    /// notes go into a new pattern named after the file (which becomes the current one).
    pub fn import_midi(&mut self, path: &std::path::Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imported = engine::midi_file::import(&bytes, self.view.ppq)?;
        self.checkpoint();
        let pattern = self.view.new_pattern();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "MIDI".into());
        self.view.patterns[pattern].name = stem.clone();
        let mut channels = 0;
        let mut notes = 0;
        for track in &imported.tracks {
            let channel = self.add_builtin(SourceSpec::Synth { params: engine::instrument::SynthParams::default() })?;
            self.rename_channel(channel, track.name.clone());
            channels += 1;
            for &(start, length, key, velocity) in &track.notes {
                self.view.add_note(pattern, engine::view::NoteView { start, length, key, velocity, channel, pan: 64, mod_x: 128, mod_y: 128, slide: false });
                notes += 1;
            }
        }
        let bar = self.ticks_per_bar();
        self.view.patterns[pattern].fit_length(bar);
        self.select_pattern(pattern);
        self.mark_dirty();
        let tempo = imported.tempo.map_or(String::new(), |t| format!(" (file tempo {t:.1} BPM)"));
        Ok(format!("Imported {notes} notes on {channels} new channel(s) into pattern “{stem}”{tempo}"))
    }

    /// The current pattern as a MIDI file, one track per channel.
    pub fn export_pattern_midi(&self) -> Vec<u8> {
        let Some(p) = self.view.patterns.get(self.pattern) else { return Vec::new() };
        let mut by_channel: std::collections::BTreeMap<usize, Vec<engine::view::NoteView>> = Default::default();
        for n in &p.notes {
            by_channel.entry(n.channel).or_default().push(*n);
        }
        let groups: Vec<(String, Vec<engine::view::NoteView>)> = by_channel.into_iter().map(|(c, notes)| (self.view.channels.get(c).map_or(format!("Channel {}", c + 1), |ch| ch.name.clone()), notes)).collect();
        engine::midi_file::export(&groups, self.view.ppq, self.view.tempo)
    }

    /// A channel's rack filter group.
    pub fn channel_group(&self, channel: usize) -> Option<String> {
        self.song.channels.get(channel).and_then(|c| c.group.clone())
    }

    pub fn set_channel_group(&mut self, channel: usize, group: Option<String>) {
        if let Some(c) = self.song.channels.get_mut(channel) {
            c.group = group.map(|g| g.trim().to_string()).filter(|g| !g.is_empty());
            self.modified = true;
        }
    }

    /// Group names in use, sorted.
    pub fn channel_groups(&self) -> Vec<String> {
        let mut g: Vec<String> = self.song.channels.iter().enumerate().filter(|(i, _)| self.view.channels.get(*i).is_some_and(|c| !c.deleted)).filter_map(|(_, c)| c.group.clone()).collect();
        g.sort_by_key(|n| n.to_lowercase());
        g.dedup();
        g
    }

    /// Plugin channels: the pitch-bend range slide notes assume (semitones).
    pub fn bend_range(&self, channel: usize) -> Option<f32> {
        self.song.channels.get(channel).filter(|c| matches!(c.source, SourceSpec::Plugin(_))).map(|c| c.sampler.bend_range)
    }

    pub fn set_bend_range(&mut self, channel: usize, semitones: f32) {
        if let Some(spec) = self.song.channels.get_mut(channel) {
            spec.sampler.bend_range = semitones.clamp(1.0, 48.0);
            let settings = spec.sampler;
            let _ = self.commands.send(Command::SetSampler { channel, settings });
            self.modified = true;
        }
    }

    /// A channel's arpeggiator settings.
    pub fn arp(&self, channel: usize) -> Option<engine::arp::ArpSettings> {
        self.song.channels.get(channel).map(|c| c.arp)
    }

    pub fn set_arp(&mut self, channel: usize, settings: engine::arp::ArpSettings) {
        if let Some(c) = self.song.channels.get_mut(channel)
            && c.arp != settings
        {
            c.arp = settings;
            let _ = self.commands.send(Command::SetArp { channel, settings });
            self.modified = true;
        }
    }

    /// Creates a slicer channel from a sample cut at `starts` (frames), plus a new pattern that
    /// plays the slices in order at the project tempo (so the loop sounds as before, but can now
    /// be rearranged in the piano roll). Returns the new channel.
    pub fn create_slicer(&mut self, path: &std::path::Path, sample: Arc<engine::sample::Sample>, starts: &[usize], name: &str) -> Result<usize, String> {
        let len = sample.len().max(1);
        let fractions: Vec<f64> = starts.iter().map(|&f| f as f64 / len as f64).collect();
        let index = self.view.channels.len();
        if index >= engine::MAX_CHANNELS {
            return Err(format!("a project can have at most {} channels", engine::MAX_CHANNELS));
        }
        self.checkpoint();
        let source = engine::load::slicer_source(sample, &fractions);
        let slice_count = match &source {
            Source::Slicer { slices, .. } => slices.len(),
            _ => 0,
        };
        let spec_source = SourceSpec::Slicer { path: path.to_string_lossy().into_owned(), slices: fractions };
        let name = unique_name(&format!("{name} slices"), self.view.channels.iter().filter(|c| !c.deleted).map(|c| c.name.as_str()));
        let channel = Channel::with_gain(name.clone(), source, 1.0, 0.0, 0);
        let channel_spec = ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(), name, source: spec_source, volume: 1.0, pan: 0.0, route: 0, muted: false, root_note: engine::SLICE_ROOT };
        let view = engine::load::channel_view(&channel, &channel_spec, ChannelStatus::Ok);
        let strip = &self.controls.channels[index];
        strip.volume.store(1.0);
        strip.pan.store(0.0);
        strip.set_route(0);
        strip.mute.store(false, std::sync::atomic::Ordering::Relaxed);
        self.commands.send(Command::AddChannel(Box::new(channel)))?;
        self.view.channels.push(view);
        self.song.channels.push(channel_spec);
        self.is_audio_clip.push(false);
        self.channel_meters.push(MeterState::default());
        self.route_to_free_insert(index);
        // A pattern replaying the slices where they fall in the original.
        let pattern = self.view.new_pattern();
        self.view.patterns[pattern].name = self.view.channels[index].name.clone();
        let mut sorted: Vec<usize> = starts.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let spt = self.view.samples_per_tick.max(1e-9);
        for (k, &start) in sorted.iter().enumerate().take(slice_count) {
            let end = sorted.get(k + 1).copied().unwrap_or(len);
            let tick = (start as f64 / spt).round() as u64;
            let length = (((end - start) as f64 / spt).round() as u64).max(1);
            self.view.add_note(pattern, engine::view::NoteView { start: tick, length, key: engine::SLICE_ROOT + k as u8, velocity: 100, channel: index, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        }
        let bar = self.ticks_per_bar();
        self.view.patterns[pattern].fit_length(bar);
        self.channel = index;
        self.select_pattern(pattern);
        self.modified = true;
        Ok(index)
    }

    /// Writes the moved controls' values into automation while recording (FL records knob
    /// moves into automation clips when recording is armed). Call every frame.
    pub fn record_automation(&mut self, touched: Vec<AutomationTarget>) -> Option<String> {
        if !(self.recording && self.is_playing() && self.mode == Mode::Song) {
            self.auto_recording.clear();
            self.plugin_poll = None;
            return None;
        }
        let tick = self.position_ticks().max(0.0) as u64;
        let mut message = None;
        for target in touched {
            // Plugin parameters come from the plugins themselves (below).
            if matches!(target, AutomationTarget::ChannelPluginParam { .. }) || (matches!(target, AutomationTarget::EffectParam { insert, slot, .. } if self.builtin_effect(insert, slot).is_none())) {
                continue;
            }
            let value = self.control_value(target);
            if let Some(m) = self.record_point(target, value, tick, None) {
                message = Some(m);
            }
        }
        // Knobs turned in plugin windows: ask each plugin what changed (about 25 times a
        // second). Changes made before recording started are thrown away.
        let now = std::time::Instant::now();
        let first_poll = self.plugin_poll.is_none();
        if self.plugin_poll.is_none_or(|t| now.duration_since(t).as_millis() >= 40) {
            self.plugin_poll = Some(now);
            let mut handles: Vec<(AutomationTarget, bridge::EditorHandle, u32)> = Vec::new();
            for (c, ch) in self.view.channels.iter().enumerate() {
                if let (false, Some(h)) = (ch.deleted, &ch.editor) {
                    handles.push((AutomationTarget::ChannelPluginParam { channel: c, param: 0 }, h.clone(), 0));
                }
            }
            for ins in &self.view.inserts {
                for (slot, fx) in ins.effects.iter().enumerate() {
                    if let Some(h) = &fx.editor {
                        handles.push((AutomationTarget::EffectParam { insert: ins.index, slot, param: 0 }, h.clone(), 0));
                    }
                }
            }
            for (base, handle, _) in handles {
                let Ok(changes) = handle.take_param_changes() else { continue };
                if first_poll {
                    continue;
                }
                for (param, value) in changes {
                    let target = match base {
                        AutomationTarget::ChannelPluginParam { channel, .. } => AutomationTarget::ChannelPluginParam { channel, param },
                        AutomationTarget::EffectParam { insert, slot, .. } => AutomationTarget::EffectParam { insert, slot, param },
                        other => other,
                    };
                    if let Some(m) = self.record_point(target, value, tick, Some(&handle)) {
                        message = Some(m);
                    }
                }
            }
        }
        message
    }

    /// Writes one recorded value (punch-in), creating the target's automation clip if needed.
    fn record_point(&mut self, target: AutomationTarget, value: f32, tick: u64, plugin: Option<&bridge::EditorHandle>) -> Option<String> {
        let mut message = None;
        let entry = self.auto_recording.iter().position(|(t, _, _)| *t == target);
        let (auto, last) = match entry {
            Some(i) => (self.auto_recording[i].1, self.auto_recording[i].2),
            None => {
                let existing = self.view.automations.iter().position(|a| a.target == target);
                let a = match existing {
                    Some(a) => {
                        self.checkpoint();
                        a
                    }
                    None => {
                        // Plugin parameters are named after the plugin's own parameter name.
                        let name = match (target, plugin) {
                            (AutomationTarget::ChannelPluginParam { channel, param }, Some(h)) => h
                                .parameters()
                                .ok()
                                .and_then(|p| p.get(param as usize).map(|(n, _)| n.clone()))
                                .map(|n| format!("{}: {n}", self.view.channels.get(channel).map_or("Plugin", |c| c.name.as_str()))),
                            (AutomationTarget::EffectParam { insert, slot, param }, Some(h)) => h
                                .parameters()
                                .ok()
                                .and_then(|p| p.get(param as usize).map(|(n, _)| n.clone()))
                                .map(|n| format!("{}: {n}", self.view.inserts.get(insert).and_then(|i| i.effects.get(slot)).map_or("Effect", |e| e.name.as_str()))),
                            _ => None,
                        };
                        let name = name.unwrap_or_else(|| target.label(&self.view));
                        message = Some(format!("Recording automation for {name}"));
                        self.create_automation_named(target, name, value)
                    }
                };
                self.auto_recording.push((target, a, tick));
                (a, tick)
            }
        };
        // Song tick → the automation's own ticks, via the clip that plays it.
        let offset = self.view.clips.iter().find(|c| c.automation == Some(auto)).map_or(0.0, |c| c.source_offset - c.start as f64);
        let at = (tick as f64 + offset).max(0.0) as u64;
        let from = (last as f64 + offset).max(0.0) as u64;
        let bar = self.ticks_per_bar();
        if let Some(a) = self.view.automations.get_mut(auto) {
            // Punch in: what was recorded before now is kept; everything after the previous
            // write is replaced (so old points can't pull the control back).
            a.points.retain(|p| p.tick <= from);
            if a.points.last().is_none_or(|p| at >= p.tick + 3 || (p.value - value).abs() > 0.004) {
                a.points.push(engine::view::AutoPoint { tick: at, value, tension: 0.0 });
                a.points.sort_by_key(|p| p.tick);
            }
            if let Some(clip) = self.view.clips.iter_mut().find(|c| c.automation == Some(auto))
                && clip.start + clip.length < tick
            {
                clip.length = tick - clip.start + bar;
            }
        }
        if let Some(e) = self.auto_recording.iter_mut().find(|(t, _, _)| *t == target) {
            e.2 = tick;
        }
        self.mark_dirty();
        message
    }

    /// Time-stretches an audio clip's sample by `ratio` (2 = twice as long) without changing its
    /// pitch, saves it as a new file, and scales the clip to match (FL's clip stretch).
    pub fn stretch_audio_clip(&mut self, clip: usize, ratio: f64) -> Result<String, String> {
        let Some(c) = self.view.clips.get(clip).cloned() else { return Err("no such clip".into()) };
        let Some(channel) = c.channel.filter(|_| c.kind == ClipKind::Audio) else { return Err("not an audio clip".into()) };
        let ratio = ratio.clamp(0.25, 4.0);
        let sample = self.channel_sample(channel)?;
        let stretched = engine::edit::time_stretch(&sample, ratio, self.view.sample_rate);
        self.checkpoint();
        let file = self.replace_sample(channel, stretched)?;
        for other in self.view.clips.iter_mut().filter(|o| o.channel == Some(channel)) {
            other.source_offset *= ratio;
            other.length = ((other.length as f64) * ratio).round().max(1.0) as u64;
            other.fade_in = ((other.fade_in as f64) * ratio).round() as u64;
            other.fade_out = ((other.fade_out as f64) * ratio).round() as u64;
        }
        self.mark_dirty();
        Ok(format!("Stretched to {:.0}% ({})", ratio * 100.0, file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
    }

    /// Tells the engine which playlist tracks are muted (after tracks move or change).
    pub fn sync_track_mutes(&self) {
        // A muted folder silences the tracks inside it.
        let solo = self.solo_track.filter(|&i| i < self.view.tracks.len());
        for (i, muted) in self.view.effective_mutes().into_iter().enumerate() {
            self.controls.set_track_muted(i, muted || solo.is_some_and(|selected| selected != i));
        }
        for i in self.view.tracks.len()..engine::control::MAX_TRACKS {
            self.controls.set_track_muted(i, false);
        }
    }

    /// An insert's built-in EQ/compressor settings.
    pub fn strip(&self, insert: usize) -> Option<StripSettings> {
        self.song.inserts.get(insert).map(|i| i.strip)
    }

    /// Changes an insert's built-in EQ/compressor. Changes within half a second on the same
    /// insert (one knob drag) share an undo step.
    pub fn set_strip(&mut self, insert: usize, settings: StripSettings) {
        let Some(current) = self.strip(insert) else { return };
        if current == settings {
            return;
        }
        let now = std::time::Instant::now();
        if !self.strip_gesture.is_some_and(|(i, at)| i == insert && now.duration_since(at).as_secs_f32() < 0.5) {
            self.checkpoint();
        }
        self.strip_gesture = Some((insert, now));
        self.song.inserts[insert].strip = settings;
        let _ = self.commands.send(Command::SetStrip { insert, settings: Box::new(settings) });
        self.modified = true;
    }

    /// Whether the channel plays a sample the audio editor can open.
    pub fn has_sample(&self, channel: usize) -> bool {
        let deleted = self.view.channels.get(channel).is_none_or(|c| c.deleted);
        !deleted && matches!(self.song.channels.get(channel).map(|c| &c.source), Some(SourceSpec::Sampler { .. } | SourceSpec::AudioClip { .. } | SourceSpec::Drum { .. }))
    }

    /// The sample a channel plays, for the audio editor (sampler, audio clip or drum).
    pub fn channel_sample(&self, channel: usize) -> Result<engine::sample::Sample, String> {
        let sr = self.view.sample_rate;
        let project_dir = self.path.as_ref().and_then(|p| p.parent());
        match self.song.channels.get(channel).map(|c| &c.source) {
            Some(SourceSpec::Sampler { path } | SourceSpec::AudioClip { path } | SourceSpec::Slicer { path, .. }) => {
                let file = engine::sample::resolve_path(path, project_dir).ok_or_else(|| format!("sample not found: {path}"))?;
                engine::sample::load_audio(&file, sr)
            }
            Some(SourceSpec::Drum { drum }) => Ok(drum.render(sr)),
            _ => Err("this channel doesn't play a sample".into()),
        }
    }

    /// Saves an edited sample as a new WAV (the original file is left alone) and makes
    /// `channel` play it. Returns the new file.
    pub fn replace_sample(&mut self, channel: usize, sample: engine::sample::Sample) -> Result<PathBuf, String> {
        let Some(spec) = self.song.channels.get(channel) else { return Err("no such channel".into()) };
        if sample.is_empty() {
            return Err("the edited sample is empty".into());
        }
        let audio_clip = matches!(spec.source, SourceSpec::AudioClip { .. });
        let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.join("KVNG Edits"))).unwrap_or_else(|| crate::data_dir().join("Edits"));
        let stem: String = spec.name.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
        let file = (1..).map(|i| dir.join(format!("{stem} edit {i}.wav"))).find(|p| !p.exists()).unwrap();
        engine::edit::write_wav(&sample, &file, self.view.sample_rate.round() as u32)?;

        let raw = file.to_string_lossy().into_owned();
        let sample = Arc::new(sample);
        let mut new_spec = spec.clone();
        new_spec.source = if audio_clip { SourceSpec::AudioClip { path: raw } } else { SourceSpec::Sampler { path: raw } };
        let source = if audio_clip {
            Source::AudioClip { sample, voices: Vec::with_capacity(32) }
        } else {
            Source::Sampler { sample, root_note: new_spec.root_note, voices: Vec::with_capacity(64) }
        };
        let temp = Channel::with_gain(new_spec.name.clone(), source, 1.0, 0.0, 0);
        let fresh = engine::load::channel_view(&temp, &new_spec, ChannelStatus::Ok);
        self.commands.send(Command::ReplaceSource { channel, source: Box::new(temp.source) })?;
        let v = &mut self.view.channels[channel];
        (v.kind, v.detail, v.waveform, v.status) = (fresh.kind, fresh.detail, fresh.waveform, ChannelStatus::Ok);
        self.song.channels[channel] = new_spec;
        self.modified = true;
        Ok(file)
    }

    /// Plays a sample from the browser (replacing any preview), or stops with `None`.
    pub fn preview_sample(&self, sample: Option<Arc<engine::sample::Sample>>) {
        let _ = self.commands.send(Command::PreviewSample(sample));
    }

    /// Adds a sampler channel for `path` and returns its index.
    pub fn add_sample_channel(&mut self, path: &std::path::Path, sample: Arc<engine::sample::Sample>, audio_clip: bool) -> Result<usize, String> {
        let index = self.view.channels.len();
        if index >= engine::MAX_CHANNELS {
            return Err(format!("a project can have at most {} channels", engine::MAX_CHANNELS));
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Sample".into());
        let name = unique_name(&stem, self.view.channels.iter().filter(|c| !c.deleted).map(|c| c.name.as_str()));
        let raw = path.to_string_lossy().into_owned();
        let (source, source_spec) = if audio_clip {
            (Source::AudioClip { sample, voices: Vec::with_capacity(32) }, SourceSpec::AudioClip { path: raw })
        } else {
            (Source::Sampler { sample, root_note: 60, voices: Vec::with_capacity(64) }, SourceSpec::Sampler { path: raw })
        };
        let channel = Channel::with_gain(name.clone(), source, 1.0, 0.0, 0);
        let channel_spec = ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(), name, source: source_spec, volume: 1.0, pan: 0.0, route: 0, muted: false, root_note: 60 };
        let view = engine::load::channel_view(&channel, &channel_spec, ChannelStatus::Ok);
        let strip = &self.controls.channels[index];
        strip.volume.store(1.0);
        strip.pan.store(0.0);
        strip.set_route(0);
        strip.mute.store(false, std::sync::atomic::Ordering::Relaxed);
        self.commands.send(Command::AddChannel(Box::new(channel)))?;
        self.view.channels.push(view);
        self.song.channels.push(channel_spec);
        self.is_audio_clip.push(audio_clip);
        self.channel_meters.push(MeterState::default());
        self.channel = index;
        self.modified = true;
        // FL gives each new channel its own mixer track.
        self.route_to_free_insert(index);
        Ok(index)
    }

    /// Places `path` on the playlist as an audio clip (a new audio-clip channel).
    pub fn add_audio_clip(&mut self, path: &std::path::Path, sample: Arc<engine::sample::Sample>, track: usize, start: u64) -> Result<(), String> {
        let length = ((sample.len() as f64 / self.view.samples_per_tick).ceil() as u64).max(1);
        let channel = self.add_sample_channel(path, sample, true)?;
        self.checkpoint();
        let label = self.view.channels[channel].name.clone();
        self.view.clips.push(engine::view::ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false,
            track,
            start,
            length,
            label,
            kind: engine::view::ClipKind::Audio,
            pattern: None,
            channel: Some(channel),
            source_offset: 0.0,
        });
        self.mark_dirty();
        Ok(())
    }

    /// The current position (0..=1) of an automatable control.
    pub fn control_value(&self, target: AutomationTarget) -> f32 {
        let c = &self.controls;
        match target {
            AutomationTarget::ChannelVolume { channel } => c.channels.get(channel).map_or(0.78, |s| 0.781_25 * s.volume.load().max(0.0).powf(1.0 / 2.877)),
            AutomationTarget::ChannelPan { channel } => c.channels.get(channel).map_or(0.5, |s| (s.pan.load() + 1.0) / 2.0),
            AutomationTarget::InsertVolume { insert } => c.inserts.get(insert).map_or(0.8, |s| engine::gain::fl_volume_position(s.volume.load())),
            AutomationTarget::InsertPan { insert } => c.inserts.get(insert).map_or(0.5, |s| (s.pan.load() + 1.0) / 2.0),
            AutomationTarget::SynthCutoff { channel } => self.synth_params(channel).map_or(0.5, |p| ((p.cutoff / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0)),
            AutomationTarget::SynthResonance { channel } => self.synth_params(channel).map_or(0.25, |p| p.resonance),
            AutomationTarget::EffectParam { insert, slot, param } => self.builtin_effect(insert, slot).map_or(0.5, |fx| fx.normalized(param as usize)),
            AutomationTarget::InsertStrip { insert, param } => self.strip(insert).map_or(0.5, |st| st.normalized(param as usize)),
            AutomationTarget::ChannelPluginParam { .. } => 0.5,
        }
    }

    /// FL's "Create automation clip": a flat curve at the control's current value,
    /// spanning the song, on the first free playlist track. Returns its index.
    pub fn create_automation(&mut self, target: AutomationTarget) -> usize {
        let value = self.control_value(target);
        let name = target.label(&self.view);
        self.create_automation_named(target, name, value)
    }

    /// Like [`Session::create_automation`] with an explicit name and starting value
    /// (used for plugin parameters, whose values live in the plugin).
    pub fn create_automation_named(&mut self, target: AutomationTarget, name: String, value: f32) -> usize {
        self.checkpoint();
        let bar = self.ticks_per_bar();
        let end = self.view.clips.iter().map(|c| c.start + c.length).max().unwrap_or(0);
        let length = end.max(bar * 8).div_ceil(bar) * bar;
        self.view.automations.push(AutomationView { name: name.clone(), target, points: vec![AutoPoint { tick: 0, value, tension: 0.0 }, AutoPoint { tick: length, value, tension: 0.0 }] });
        let index = self.view.automations.len() - 1;
        let track = self.view.clips.iter().map(|c| c.track + 1).max().unwrap_or(0);
        while self.view.tracks.len() <= track {
            let n = self.view.tracks.len() + 1;
            self.view.tracks.push(engine::view::TrackView { name: format!("Track {n}"), muted: false, color: None, folder: false, collapsed: false, grouped: false });
        }
        self.view.clips.push(ClipView {
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
            reverse: false,
            color: None,
            automation: Some(index),
            track,
            start: 0,
            length,
            label: name,
            kind: ClipKind::Automation,
            pattern: None,
            channel: None,
            source_offset: 0.0,
        });
        self.mark_dirty();
        index
    }

    /// Adds a tempo change at `tick` (or retunes the one already there).
    pub fn set_tempo_change(&mut self, tick: u64, bpm: f64) {
        let pos = self.position_ticks();
        let bpm = bpm.clamp(10.0, 522.0);
        match self.view.tempo_changes.iter_mut().find(|c| c.tick == tick) {
            Some(c) => c.bpm = bpm,
            None => {
                self.view.tempo_changes.push(engine::view::TempoChange { tick, bpm });
                self.view.tempo_changes.sort_by_key(|c| c.tick);
            }
        }
        self.after_tempo_edit(pos);
    }

    /// Sets the time signature (beats per bar, over 4) from `bar` (0-based) on.
    pub fn set_meter_change(&mut self, bar: u64, beats: u32) {
        self.checkpoint_gesture(0x3E7E + bar);
        let beats = beats.clamp(1, 16);
        if bar == 0 {
            self.set_beats_per_bar(beats);
        } else {
            match self.view.meter_changes.iter_mut().find(|m| m.bar == bar) {
                Some(m) => m.beats_per_bar = beats,
                None => {
                    self.view.meter_changes.push(engine::view::MeterChange { bar, beats_per_bar: beats });
                    self.view.meter_changes.sort_by_key(|m| m.bar);
                }
            }
        }
        // A change that repeats the meter before it does nothing.
        let mut prev = self.view.beats_per_bar;
        self.view.meter_changes.retain(|m| {
            let keep = m.beats_per_bar != prev;
            prev = m.beats_per_bar;
            keep
        });
        self.mark_dirty();
    }

    pub fn remove_meter_change(&mut self, bar: u64) {
        self.checkpoint();
        self.view.meter_changes.retain(|m| m.bar != bar);
        self.mark_dirty();
    }

    pub fn remove_tempo_change(&mut self, index: usize) {
        let pos = self.position_ticks();
        if index < self.view.tempo_changes.len() {
            self.view.tempo_changes.remove(index);
        }
        self.after_tempo_edit(pos);
    }

    /// Keeps the playhead on the same musical position after the tempo map changes.
    fn after_tempo_edit(&mut self, pos_ticks: f64) {
        self.modified = true;
        self.recompile();
        self.seek_ticks(pos_ticks);
    }

    pub fn set_beats_per_bar(&mut self, beats: u32) {
        self.view.beats_per_bar = beats.clamp(1, 16);
        self.modified = true;
        self.sync_metronome();
        self.mark_dirty();
    }

    pub fn set_swing(&mut self, swing: f32) {
        self.view.swing = swing.clamp(0.0, 1.0);
        self.modified = true;
        self.mark_dirty();
    }

    /// Sets the selected pattern's length in steps (sixteenths).
    pub fn set_pattern_steps(&mut self, steps: u64) {
        self.checkpoint();
        self.view.set_pattern_steps(self.pattern, steps);
        self.mark_dirty();
    }

    pub fn set_pattern_color(&mut self, color: Option<[u8; 3]>) {
        if let Some(p) = self.view.patterns.get_mut(self.pattern) {
            p.color = color;
            self.modified = true;
        }
    }

    pub fn new_pattern(&mut self) {
        self.checkpoint();
        let p = self.view.new_pattern();
        self.select_pattern(p);
        self.mark_dirty();
    }

    pub fn clone_pattern(&mut self) {
        self.checkpoint();
        if let Some(p) = self.view.clone_pattern(self.pattern) {
            self.select_pattern(p);
        }
        self.mark_dirty();
    }

    pub fn delete_pattern(&mut self) {
        self.checkpoint();
        self.view.delete_pattern(self.pattern);
        self.pattern = self.pattern.min(self.view.patterns.len() - 1);
        self.mark_dirty();
    }

    pub fn rename_pattern(&mut self, name: &str) {
        self.checkpoint();
        self.view.rename_pattern(self.pattern, name);
        self.mark_dirty();
    }

    fn sync_metronome(&self) {
        self.controls.samples_per_beat.store((self.view.samples_per_tick * self.view.ppq as f64) as f32);
        self.controls.beats_per_bar.store(self.view.beats_per_bar, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn toggle_metronome(&self) {
        let on = !self.controls.metronome.load(std::sync::atomic::Ordering::Relaxed);
        self.controls.metronome.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn metronome_on(&self) -> bool {
        self.controls.metronome.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Applies MIDI controller moves to linked controls, or links one while learning.
    /// Returns a message when a link is made.
    pub fn poll_controllers(&mut self) -> Option<String> {
        let mut message = None;
        for (midi_channel, cc, value) in self.live.drain_cc() {
            if let Some(target) = self.learning.take() {
                self.view.controller_links.retain(|l| l.target != target && !(l.cc == cc && l.midi_channel == midi_channel));
                self.view.controller_links.push(engine::view::ControllerLink { midi_channel, cc, target });
                self.modified = true;
                message = Some(format!("Linked CC {cc} to {}", target.label(&self.view)));
                continue;
            }
            let v = value as f32 / 127.0;
            let targets: Vec<AutomationTarget> = self.view.controller_links.iter().filter(|l| l.cc == cc && l.midi_channel == midi_channel).map(|l| l.target).collect();
            for target in targets {
                match target {
                    AutomationTarget::SynthCutoff { channel } => {
                        if let Some(mut p) = self.synth_params(channel) {
                            p.cutoff = 20.0 * 1000f32.powf(v);
                            self.set_synth_params(channel, p);
                        }
                    }
                    AutomationTarget::SynthResonance { channel } => {
                        if let Some(mut p) = self.synth_params(channel) {
                            p.resonance = v;
                            self.set_synth_params(channel, p);
                        }
                    }
                    AutomationTarget::InsertStrip { insert, param } => {
                        if let Some(mut st) = self.strip(insert) {
                            st.set_normalized(param as usize, v);
                            self.strip_gesture = Some((insert, std::time::Instant::now()));
                            self.set_strip(insert, st);
                        }
                    }
                    AutomationTarget::EffectParam { insert, slot, param } if self.builtin_effect(insert, slot).is_some() => {
                        if let Some(mut fx) = self.builtin_effect(insert, slot) {
                            fx.set_normalized(param as usize, v);
                            self.set_builtin_effect(insert, slot, fx);
                        }
                    }
                    other => {
                        let _ = self.commands.send(Command::SetControl { target: other, value: v });
                        self.modified = true;
                    }
                }
            }
        }
        message
    }

    /// Routes live input to the selected channel and records it when armed (call every frame).
    pub fn poll_live(&mut self) {
        self.live.set_target(self.channel);
        let events = self.live.drain();
        if !(self.recording && self.is_playing()) {
            self.held.clear();
            self.modified_by_recording = false;
            return;
        }
        // Record into the current pattern at the loop position (PAT) or song position (SONG).
        let len = self.view.patterns.get(self.pattern).map_or(1, |p| p.length.max(1));
        let now = self.position_ticks() as u64;
        let tick = if self.mode == Mode::Pattern { now % len } else { now };
        for (key, velocity) in events {
            if velocity > 0 {
                self.held.insert(key, (tick, velocity));
            } else if let Some((start, vel)) = self.held.remove(&key) {
                let end = if tick >= start { tick } else { tick + len };
                let note = engine::view::NoteView { start, length: (end - start).max(1), key, velocity: ((vel as u32 * 128) / 127) as u8, channel: self.channel, pan: 64, mod_x: 128, mod_y: 128, slide: false };
                if !self.modified_by_recording {
                    self.checkpoint();
                    self.modified_by_recording = true;
                }
                self.view.add_note(self.pattern, note);
                self.mark_dirty();
            }
        }
    }

    /// Starts and stops audio recording with the transport (call every frame).
    /// Returns a message when a take is finished or recording fails.
    pub fn poll_audio_recording(&mut self) -> Option<String> {
        // Click while recording (if chosen), restoring the metronome afterwards.
        let use_click = self.metronome_when_recording && (self.recording || self.audio_armed) && self.is_playing();
        let metronome = &self.controls.metronome;
        match (use_click, self.metronome_before_recording) {
            (true, None) => {
                self.metronome_before_recording = Some(metronome.load(std::sync::atomic::Ordering::Relaxed));
                metronome.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            (false, Some(before)) => {
                metronome.store(before, std::sync::atomic::Ordering::Relaxed);
                self.metronome_before_recording = None;
            }
            _ => {}
        }

        let should_record = self.audio_armed && self.is_playing() && !self.counting_in();
        match (should_record, self.recorder.is_some()) {
            (true, false) => {
                let monitor = self.monitor_input.then(|| Arc::new(engine::monitor::MonitorRing::default()));
                let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.join("Recordings"))).unwrap_or_else(|| crate::data_dir().join("Recordings"));
                match super::recorder::Recorder::start(self.position_ticks() as u64, self.input_device.as_deref(), monitor.clone(), self.view.sample_rate.round() as u32, &dir, &self.view.name) {
                    Ok(r) => {
                        let msg = format!("Recording from {}{}", r.device_name, if r.monitoring { " (monitoring)" } else if monitor.is_some() { " (monitoring off: input/output rates differ)" } else { "" });
                        if r.monitoring {
                            let _ = self.commands.send(Command::SetMonitor(monitor, self.record_insert.unwrap_or(0)));
                        }
                        self.recorder = Some(r);
                        Some(msg)
                    }
                    Err(e) => {
                        self.audio_armed = false;
                        Some(e)
                    }
                }
            }
            (false, true) => {
                let _ = self.commands.send(Command::SetMonitor(None, 0));
                let rec = self.recorder.take().unwrap();
                let start = rec.start_tick;
                let result = rec.finish().and_then(|(path, dropped)| {
                    let sample = engine::sample::load_audio(&path, self.view.sample_rate)?;
                    let track = self.record_track.filter(|&t| t < self.view.tracks.len() && !self.view.tracks[t].folder)
                        .unwrap_or_else(|| self.view.clips.iter().map(|c| c.track + 1).max().unwrap_or(0).min(self.view.tracks.len().saturating_sub(1)));
                    self.add_audio_clip(&path, Arc::new(sample), track, start)?;
                    // Latency compensation: what was played at `start` reached us this much later.
                    let secs = self.record_offset_ms.map_or_else(|| 2.0 * self.buffer_frames.unwrap_or(512) as f64 / self.view.sample_rate, |ms| ms as f64 / 1000.0);
                    // The take plays through the armed mixer track, if one is armed.
                    if let (Some(insert), Some(channel)) = (self.record_insert, self.view.clips.last().and_then(|c| c.channel)) {
                        self.route_channel(channel, insert);
                    }
                    if let Some(clip) = self.view.clips.last_mut() {
                        clip.source_offset = secs.max(0.0);
                        let trim = (secs * self.view.sample_rate / self.view.samples_per_tick) as u64;
                        clip.length = clip.length.saturating_sub(trim).max(1);
                    }
                    self.mark_dirty();
                    Ok((path, dropped))
                });
                Some(match result {
                    Ok((path, 0)) => format!("Recorded {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    Ok((path, dropped)) => format!("Recorded {} with {dropped} dropped frames (disk could not keep up)", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    Err(e) => format!("Recording not saved: {e}"),
                })
            }
            _ => None,
        }
    }

    /// Length and level of the take in progress.
    pub fn recording_status(&self) -> Option<(f32, f32)> {
        self.recorder.as_ref().map(|r| (r.seconds(), r.recent_peak()))
    }

    /// Frees objects the engine has finished with (e.g. removed plugins).
    pub fn collect_garbage(&self) {
        self.commands.collect_garbage();
    }

    /// Saves to `path` as a `.kvng` project. Returns warnings for anything not captured.
    pub fn save(&mut self, path: PathBuf) -> Result<Vec<String>, String> {
        let (song, problems) = self.to_song();
        song.write(&path)?;
        self.view.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.path = Some(path);
        self.modified = false;
        Ok(problems)
    }

    /// Call before changing patterns or clips so the edit can be undone.
    pub fn checkpoint(&mut self) {
        self.modified = true;
        self.undo.push(self.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            patterns: self.view.patterns.clone(),
            clips: self.view.clips.clone(),
            automations: self.view.automations.clone(),
            strips: self.song.inserts.iter().map(|i| i.strip).collect(),
            tracks: self.view.tracks.clone(),
            markers: self.view.markers.clone(),
            tempo_changes: self.view.tempo_changes.clone(),
            meter_changes: self.view.meter_changes.clone(),
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        let Snapshot { patterns, clips, automations, strips, tracks, markers, tempo_changes, meter_changes } = snap;
        self.view.meter_changes = meter_changes;
        let tempo_changed = self.view.tempo_changes != tempo_changes;
        self.view.tracks = tracks;
        self.view.markers = markers;
        self.view.tempo_changes = tempo_changes;
        self.sync_track_mutes();
        if tempo_changed {
            let pos = self.position_ticks();
            self.after_tempo_edit(pos);
        }
        for (insert, settings) in strips.into_iter().enumerate() {
            if self.song.inserts.get(insert).is_some_and(|i| i.strip != settings) {
                self.song.inserts[insert].strip = settings;
                let _ = self.commands.send(Command::SetStrip { insert, settings: Box::new(settings) });
            }
        }
        self.view.patterns = patterns;
        self.view.clips = clips;
        self.view.automations = automations;
        self.pattern = self.pattern.min(self.view.patterns.len().saturating_sub(1));
        self.mark_dirty();
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.modified = true;
            let now = self.snapshot();
            self.redo.push(now);
            self.restore(prev);
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.modified = true;
            let now = self.snapshot();
            self.undo.push(now);
            self.restore(next);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Sends pending edits to the engine (once per frame).
    pub fn flush(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.recompile();
        }
    }

    fn recompile(&mut self) {
        // Channels that will play but weren't loaded at startup get loaded now.
        let wanted: Vec<usize> = match self.mode {
            Mode::Pattern => self.view.patterns.get(self.pattern).map(|p| p.notes.iter().map(|n| n.channel).collect()).unwrap_or_default(),
            Mode::Song => self
                .view
                .clips
                .iter()
                .flat_map(|c| match (c.pattern, c.channel) {
                    (Some(p), _) => self.view.patterns.get(p).map(|p| p.notes.iter().map(|n| n.channel).collect::<Vec<_>>()).unwrap_or_default(),
                    (None, Some(ch)) => vec![ch],
                    _ => vec![],
                })
                .collect(),
        };
        for c in wanted {
            self.request_load(c);
        }
        match self.mode {
            Mode::Song => {
                self.timeline.send(timeline::compile(&self.view, &self.is_audio_clip));
                self.apply_loop(None);
            }
            Mode::Pattern => {
                let (t, loop_len) = timeline::compile_pattern(&self.view, self.pattern);
                self.timeline.send(t);
                self.apply_loop(Some(loop_len));
            }
        }
    }

    fn apply_loop(&self, pattern_loop: Option<u64>) {
        let range = match (self.mode, pattern_loop) {
            (Mode::Pattern, Some(len)) => Some((0, len.max(1))),
            (Mode::Pattern, None) => self.controls.loop_range(),
            (Mode::Song, _) => self.song_loop.map(|(a, b)| (self.view.tick_to_sample(a as f64), self.view.tick_to_sample(b as f64))),
        };
        self.controls.set_loop(range);
    }

    /// Collects meter peaks from the engine.
    pub fn update_meters(&mut self, now: f64, dt: f32) {
        for (m, s) in self.channel_meters.iter_mut().zip(&self.controls.channels) {
            m.update([s.peak_l.take(), s.peak_r.take()], now, dt);
        }
        for (m, s) in self.insert_meters.iter_mut().zip(&self.controls.inserts) {
            m.update([s.peak_l.take(), s.peak_r.take()], now, dt);
        }
    }

    /// Formats a tick position as FL's bar:step:tick.
    pub fn format_bst(&self, ticks: f64) -> String {
        let t = ticks.max(0.0) as u64;
        let step = self.view.step_ticks();
        let (bar, start, _) = self.view.bar_at(t);
        let into = t - start;
        format!("{}:{:02}:{:02}", bar + 1, into / step + 1, into % step)
    }

    pub fn format_time(&self, ticks: f64) -> String {
        let secs = self.view.tick_to_sample(ticks) as f64 / self.view.sample_rate;
        format!("{}:{:02}:{:02}", (secs / 60.0) as u64, (secs % 60.0) as u64, ((secs.fract()) * 100.0) as u64)
    }
}

/// `base`, or `base 2`, `base 3`… so channel names stay unique.
fn unique_name<'a>(base: &str, existing: impl Iterator<Item = &'a str> + Clone) -> String {
    if !existing.clone().any(|n| n == base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !existing.clone().any(|e| e == n)).unwrap()
}
