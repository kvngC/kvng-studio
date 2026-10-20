//! Builds a running [`Engine`] (plus the UI's [`ProjectView`]) from a [`Song`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use bridge::{Helper, RemotePlugin};
use plughost::{Host, Plugin};

use crate::sample::{self, Sample};
use crate::song::{self, ChannelSpec, EffectSource, PluginFormatSpec, PluginSpec, Song, SourceSpec, StateSpec};
use crate::view::{self, ChannelStatus, ChannelView, EffectView, InsertView, ProjectView};
use crate::{Channel, Effect, Engine, Insert, MAX_BLOCK, PluginInstance, Processor, Source, native, timeline};

pub struct Loaded {
    pub engine: Engine,
    /// Things that won't sound like they did in FL Studio, for the user to see.
    pub warnings: Vec<String>,
    /// What the UI shows and edits.
    pub view: ProjectView,
    /// The document this was built from (sources, plugin identities, mixer layout).
    pub song: Song,
}

/// Imports an FL Studio project. See [`build`].
pub fn from_project(host: Option<&Host>, project: &flp::Project, project_path: &Path, sample_rate: f64, progress: &mut dyn FnMut(&str)) -> Loaded {
    let song = song::from_flp(project, project_path);
    let mut loaded = build(host, song, project_path, sample_rate, progress);
    if project.arrangements.is_empty() {
        loaded.warnings.push("project has no arrangement".into());
    }
    loaded
}

/// Builds an engine for `song`. `host` is only needed for in-process plugin
/// hosting (normally plugins run in helper processes and it can be `None`).
/// `song_path` locates relative sample paths. `progress` receives status messages.
pub fn build(host: Option<&Host>, mut song: Song, song_path: &Path, sample_rate: f64, progress: &mut dyn FnMut(&str)) -> Loaded {
    if song.inserts.is_empty() {
        song.inserts.push(crate::song::InsertSpec { name: "Master".into(), routes_to: vec![], ..crate::song::InsertSpec::blank(0) });
    }
    while song.inserts.len() <= crate::song::MAX_INSERTS {
        let i = song.inserts.len();
        song.inserts.push(crate::song::InsertSpec::blank(i));
    }
    let mut warnings = Vec::new();
    let project_dir = song_path.parent();
    let used = song.used_channels();
    let mut samples: HashMap<String, Arc<Sample>> = HashMap::new();

    let mut channels = Vec::with_capacity(song.channels.len());
    let mut statuses = Vec::with_capacity(song.channels.len());
    for (i, spec) in song.channels.iter().enumerate() {
        // Plugin channels nothing plays yet load later, on first use: faster, and a broken
        // plugin nobody plays can't crash us. Built-in sounds are cheap, so they always load.
        let is_used = used.contains(&i) || !is_lazy(&spec.source);
        let source = if is_used {
            progress(&format!("Loading {}", spec.name));
            channel_source(host, spec, project_dir, sample_rate, &mut samples, &mut warnings)
        } else {
            Source::Silent
        };
        statuses.push(if is_used { status_of(&source, &spec.source) } else { ChannelStatus::Unused });
        let mut channel = Channel::with_gain(spec.name.clone(), source, spec.volume, spec.pan, spec.route);
        channel.sampler = spec.sampler;
        channel.arp.settings = spec.arp;
        channels.push(channel);
    }

    let mut inserts: Vec<Insert> =
        song.inserts
            .iter()
            .map(|s| {
                let mut ins = Insert::with_gain(s.name.clone(), !s.muted, s.volume, s.pan, s.routes_to.clone());
                ins.strip = crate::channel_strip::ChannelStrip::new(s.strip, sample_rate);
                ins.route_levels = (0..s.routes_to.len()).map(|k| s.route_levels.get(k).copied().unwrap_or(1.0)).collect();
                ins.sidechain_to = s.sidechain_to.clone();
                ins
            })
            .collect();
    if inserts.is_empty() {
        inserts.push(Insert::with_gain("Master".into(), true, 1.0, 0.0, Vec::new()));
    }

    // Effects only on inserts something feeds, to keep load time down.
    let mut insert_used = vec![false; inserts.len()];
    insert_used[0] = true;
    for ch in &channels {
        if let Some(u) = insert_used.get_mut(ch.insert) {
            *u = true;
        }
    }
    propagate_use(&mut insert_used, &inserts);
    for (i, spec) in song.inserts.iter().enumerate() {
        if !insert_used.get(i).copied().unwrap_or(false) {
            continue;
        }
        for fx in spec.effects.iter().filter(|f| f.enabled) {
            progress(&format!("Loading {} on {}", fx.name, spec.name));
            match effect_processor(host, &fx.source, sample_rate) {
                Ok(processor) => inserts[i].effects.push(Effect { name: fx.name.clone(), processor, mix: fx.mix, bypass: fx.bypassed }),
                Err(e) => warnings.push(format!("{} on {}: {e}", fx.name, spec.name)),
            }
        }
    }

    progress("Building the arrangement");
    let view = build_view(&song, &channels, &statuses, &inserts, &insert_used, sample_rate, &warnings);
    let timeline = timeline::compile(&view, &audio_clip_channels(&channels));
    let engine = Engine::new(sample_rate, channels, inserts, timeline.events, timeline.length);
    let controls = engine.controls();
    for (i, t) in view.tracks.iter().enumerate() {
        controls.set_track_muted(i, t.muted);
    }
    for (strip, spec) in controls.channels.iter().zip(&song.channels) {
        strip.mute.store(spec.muted, std::sync::atomic::Ordering::Relaxed);
    }
    Loaded { engine, warnings, view, song }
}

/// Marks inserts fed (directly or through routing) by a used insert as used.
fn propagate_use(used: &mut [bool], inserts: &[Insert]) {
    let mut changed = true;
    while changed {
        changed = false;
        for (i, ins) in inserts.iter().enumerate() {
            if used[i] {
                for &d in &ins.routes_to {
                    if d < used.len() && !used[d] {
                        used[d] = true;
                        changed = true;
                    }
                }
            }
        }
    }
}

/// Builds the processor for one mixer effect.
pub fn effect_processor(host: Option<&Host>, source: &EffectSource, sample_rate: f64) -> Result<Processor, String> {
    match source {
        EffectSource::Plugin(spec) => load_plugin(host, spec, sample_rate).map(|p| Processor::Plugin(Box::new(p))),
        EffectSource::Builtin { effect } => Ok(Processor::Builtin(Box::new(crate::effects::Builtin::new(effect.clone(), sample_rate)))),
        EffectSource::FlNative { internal_name, data } => match native::effect(internal_name, data, sample_rate) {
            Some(Ok(fx)) => Ok(Processor::Native(fx)),
            Some(Err(e)) => Err(format!("couldn't read its settings ({e})")),
            None => Err(format!("FL Studio's {internal_name:?} isn't available in KVNG Studio yet (skipped)")),
        },
    }
}

fn build_view(
    song: &Song,
    channels: &[Channel],
    statuses: &[ChannelStatus],
    inserts: &[Insert],
    insert_used: &[bool],
    sample_rate: f64,
    warnings: &[String],
) -> ProjectView {
    let ppq = song.ppq.max(1);
    let ticks_per_bar = (ppq * song.beats_per_bar.max(1)) as u64;
    let length_ticks = song.clips.iter().map(|c| c.start + c.length).max().unwrap_or(ticks_per_bar * 8);

    let channel_views = channels.iter().zip(&song.channels).zip(statuses).map(|((ch, spec), status)| channel_view(ch, spec, *status)).collect();

    let insert_views = inserts
        .iter()
        .enumerate()
        .map(|(i, ins)| InsertView {
            index: i,
            name: ins.name.clone(),
            effects: ins.effects.iter().map(effect_view).collect(),
            routes_to: ins.routes_to.clone(),
            route_levels: (0..ins.routes_to.len()).map(|k| ins.route_levels.get(k).copied().unwrap_or(1.0)).collect(),
            sidechain_to: ins.sidechain_to.clone(),
            color: song.inserts.get(i).and_then(|s| s.color),
            used: insert_used.get(i).copied().unwrap_or(false),
        })
        .collect();

    ProjectView {
        name: song.name.clone(),
        tempo: song.tempo,
        ppq,
        beats_per_bar: song.beats_per_bar.max(1),
        sample_rate,
        samples_per_tick: sample_rate * 60.0 / (song.tempo.max(1.0) * ppq as f64),
        length_ticks,
        tracks: song.tracks.clone(),
        clips: song.clips.clone(),
        patterns: song.patterns.clone(),
        automations: song.automations.clone(),
        swing: song.swing,
        markers: song.markers.clone(),
        tempo_changes: song.tempo_changes.clone(),
        meter_changes: song.meter_changes.clone(),
        controller_links: song.controller_links.clone(),
        channels: channel_views,
        inserts: insert_views,
        warnings: warnings.to_vec(),
    }
}

/// What the UI shows for a channel.
pub fn channel_view(ch: &Channel, spec: &ChannelSpec, status: ChannelStatus) -> ChannelView {
    let (kind, detail, editor, waveform) = match &ch.source {
        Source::Sampler { sample, .. } => (
            match (&spec.source, status) {
                (_, ChannelStatus::Substituted) => "Replacement",
                (SourceSpec::Drum { .. }, _) => "KVNG Drum",
                _ => "Sampler",
            },
            match &spec.source {
                SourceSpec::Sampler { path } => file_name(path).to_string(),
                SourceSpec::Drum { drum } => format!("built-in {}", drum.label().to_lowercase()),
                _ => "replacement sample".to_string(),
            },
            None,
            Some(Arc::new(view::waveform(&sample.left, &sample.right))),
        ),
        Source::Slicer { sample, slices, .. } => (
            "Slicer",
            match &spec.source {
                SourceSpec::Slicer { path, .. } => format!("{} slices · {}", slices.len(), file_name(path)),
                _ => format!("{} slices", slices.len()),
            },
            None,
            Some(Arc::new(view::waveform(&sample.left, &sample.right))),
        ),
        Source::AudioClip { sample, .. } => (
            "Audio clip",
            match &spec.source {
                SourceSpec::AudioClip { path } => file_name(path).to_string(),
                _ => String::new(),
            },
            None,
            Some(Arc::new(view::waveform(&sample.left, &sample.right))),
        ),
        Source::Plugin { plugin, .. } => (
            "Plugin",
            format!("{} · {}", plugin.name(), plugin.location()),
            match plugin.as_ref() {
                PluginInstance::Remote(p) => Some(p.editor_handle()),
                PluginInstance::Local(_) => None,
            },
            None,
        ),
        Source::Synth(_) => ("KVNG Synth", "built-in synth".to_string(), None, None),
        Source::MidiOut { out, midi_channel } => ("MIDI Out", format!("{} · channel {}", out.port, midi_channel + 1), None, None),
        Source::Silent => {
            let detail = match (&spec.source, status) {
                (_, ChannelStatus::Unused) => "not used yet".to_string(),
                (SourceSpec::FlNative { internal_name }, _) => format!("FL Studio {internal_name} (not available)"),
                (SourceSpec::Plugin(p), _) => format!("{} (didn't load)", p.name),
                (SourceSpec::Sampler { path } | SourceSpec::AudioClip { path } | SourceSpec::Slicer { path, .. }, _) => format!("{} (not found)", file_name(path)),
                (SourceSpec::Empty, _) => "empty channel".to_string(),
                (SourceSpec::Synth { .. } | SourceSpec::Drum { .. }, _) => "built-in".to_string(),
                (SourceSpec::MidiOut { port, .. }, _) => format!("MIDI output {port:?} not available"),
            };
            (if matches!(spec.source, SourceSpec::Plugin(_)) { "Plugin" } else { "Channel" }, detail, None, None)
        }
    };
    ChannelView {
        name: ch.name.clone(),
        kind: kind.into(),
        detail,
        status,
        insert: ch.insert,
        root_note: spec.root_note,
        editor,
        waveform,
        deleted: false,
        color: spec.color,
    }
}

pub fn effect_view(fx: &Effect) -> EffectView {
    match &fx.processor {
        Processor::Plugin(p) => EffectView {
            name: fx.name.clone(),
            bypass: fx.bypass,
            mix: fx.mix,
            detail: format!("Plugin · {}", p.location()),
            editor: match p.as_ref() {
                PluginInstance::Remote(r) => Some(r.editor_handle()),
                PluginInstance::Local(_) => None,
            },
        },
        Processor::Native(_) => EffectView { name: fx.name.clone(), bypass: fx.bypass, mix: fx.mix, detail: "KVNG adapter".into(), editor: None },
        Processor::Builtin(_) => EffectView { name: fx.name.clone(), bypass: fx.bypass, mix: fx.mix, detail: "Built-in · double-click for settings".into(), editor: None },
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Loads (or reuses) a sample, as a sampler or audio-clip source.
pub fn sample_source(raw: &str, audio_clip: bool, root_note: u8, project_dir: Option<&Path>, sample_rate: f64, cache: &mut HashMap<String, Arc<Sample>>) -> Result<Source, String> {
    let sample = match cache.get(raw) {
        Some(s) => s.clone(),
        None => {
            let path = sample::resolve_path(raw, project_dir).ok_or_else(|| format!("sample not found: {raw}"))?;
            let s = sample::load_audio(&path, sample_rate).map_err(|e| format!("couldn't read sample ({e})"))?;
            if s.is_empty() {
                return Err("sample is empty".into());
            }
            let s = Arc::new(s);
            cache.insert(raw.to_string(), s.clone());
            s
        }
    };
    Ok(if audio_clip {
        Source::AudioClip { sample, voices: Vec::with_capacity(32) }
    } else {
        Source::Sampler { sample, root_note, voices: Vec::with_capacity(64) }
    })
}

/// A slicer playing `sample`, cut at `starts` (fractions of its length).
pub fn slicer_source(sample: Arc<Sample>, starts: &[f64]) -> Source {
    let len = sample.len();
    let mut cuts: Vec<usize> = starts.iter().map(|f| (f.clamp(0.0, 1.0) * len as f64).round() as usize).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let slices = cuts.iter().enumerate().map(|(i, &a)| (a, cuts.get(i + 1).copied().unwrap_or(len))).filter(|(a, b)| b > a).collect();
    Source::Slicer { sample, slices, voices: Vec::with_capacity(64) }
}

/// Channels loaded only once something plays them (third-party and FL plugins).
pub fn is_lazy(source: &SourceSpec) -> bool {
    matches!(source, SourceSpec::Plugin(_) | SourceSpec::FlNative { .. })
}

fn status_of(source: &Source, spec: &SourceSpec) -> ChannelStatus {
    match (source, spec) {
        (Source::Silent, SourceSpec::Empty) => ChannelStatus::Ok,
        (Source::Silent, _) => ChannelStatus::Missing,
        (Source::Sampler { .. }, SourceSpec::Plugin(_) | SourceSpec::FlNative { .. }) => ChannelStatus::Substituted,
        _ => ChannelStatus::Ok,
    }
}

/// Loads a channel that was skipped at project load (see [`is_lazy`]), e.g. when the user
/// first gives it notes. Runs off the audio thread; may take a while for big plugins.
/// Returns the source, its status, and any warnings.
pub fn load_channel(spec: &ChannelSpec, project_dir: Option<&Path>, sample_rate: f64) -> (Source, ChannelStatus, Vec<String>) {
    let mut warnings = Vec::new();
    let source = channel_source(None, spec, project_dir, sample_rate, &mut HashMap::new(), &mut warnings);
    let status = status_of(&source, &spec.source);
    (source, status, warnings)
}

fn channel_source(
    host: Option<&Host>,
    spec: &ChannelSpec,
    project_dir: Option<&Path>,
    sample_rate: f64,
    cache: &mut HashMap<String, Arc<Sample>>,
    warnings: &mut Vec<String>,
) -> Source {
    let name = &spec.name;
    let problem = match &spec.source {
        SourceSpec::Empty => return Source::Silent,
        SourceSpec::Slicer { path, slices } => match sample_source(path, false, spec.root_note, project_dir, sample_rate, cache) {
            Ok(Source::Sampler { sample, .. }) => return slicer_source(sample, slices),
            Ok(_) => return Source::Silent,
            Err(e) => {
                warnings.push(format!("{name}: {e}"));
                return Source::Silent;
            }
        },
        SourceSpec::Sampler { path } | SourceSpec::AudioClip { path } => {
            let clip = matches!(spec.source, SourceSpec::AudioClip { .. });
            match sample_source(path, clip, spec.root_note, project_dir, sample_rate, cache) {
                Ok(s) => return s,
                Err(e) => {
                    warnings.push(format!("{name}: {e}"));
                    return Source::Silent;
                }
            }
        }
        SourceSpec::Plugin(p) => match load_plugin(host, p, sample_rate) {
            Ok(plugin) => return Source::Plugin { plugin: Box::new(plugin), midi: Vec::with_capacity(256) },
            Err(e) => format!("plugin didn't load ({e})"),
        },
        SourceSpec::FlNative { internal_name } => format!("FL Studio's {internal_name:?} isn't available in KVNG Studio"),
        SourceSpec::Synth { params } => return Source::Synth(Box::new(crate::instrument::Synth::new(params.clone(), sample_rate))),
        SourceSpec::Drum { drum } => {
            return Source::Sampler { sample: Arc::new(drum.render(sample_rate)), root_note: spec.root_note, voices: Vec::with_capacity(64) };
        }
        SourceSpec::MidiOut { port, midi_channel } => match crate::midi_out::MidiOut::open(port) {
            Ok(out) => return Source::MidiOut { out: Box::new(out), midi_channel: *midi_channel },
            Err(e) => format!("MIDI output unavailable ({e})"),
        },
    };
    if let Some(source) = replacement(spec, project_dir, sample_rate, warnings) {
        warnings.push(format!("{name}: {problem}; playing its replacement sample instead"));
        return source;
    }
    warnings.push(format!(
        "{name}: {problem} (channel is silent; add an audio file named \"{}\" in \"{REPLACEMENTS_DIR}\" next to the project, or replace the instrument in the rack)",
        file_safe(name)
    ));
    Source::Silent
}

/// Folder next to a project holding stand-in samples for channels we can't play.
pub const REPLACEMENTS_DIR: &str = "KVNG Replacements";

/// A replacement sample (`KVNG Replacements/<channel name>.<audio format>`) played as a sampler.
fn replacement(spec: &ChannelSpec, project_dir: Option<&Path>, sample_rate: f64, warnings: &mut Vec<String>) -> Option<Source> {
    let dir = project_dir?.join(REPLACEMENTS_DIR);
    let stem = file_safe(&spec.name);
    let mut paths: Vec<_> = std::fs::read_dir(&dir).ok()?.flatten().map(|entry| entry.path())
        .filter(|path| path.is_file() && path.file_stem().is_some_and(|s| s == std::ffi::OsStr::new(&stem)) && sample::is_audio_file(path))
        .collect();
    paths.sort_by_key(|path| path.extension().and_then(|ext| sample::AUDIO_EXTENSIONS.iter().position(|candidate| ext.eq_ignore_ascii_case(candidate))).unwrap_or(usize::MAX));
    for path in paths {
        match sample::load_audio(&path, sample_rate) {
            Ok(s) if !s.is_empty() => return Some(Source::Sampler { sample: Arc::new(s), root_note: spec.root_note, voices: Vec::with_capacity(64) }),
            Ok(_) => warnings.push(format!("{}: replacement is empty ({})", spec.name, path.display())),
            Err(e) => warnings.push(format!("{}: replacement unreadable ({e})", spec.name)),
        }
    }
    None
}

fn file_safe(name: &str) -> String {
    name.chars().map(|c| if matches!(c, '/' | ':' | '\\') { '_' } else { c }).collect()
}

/// Loads a third-party plugin and restores its saved state.
///
/// Plugins run in a helper process unless `KVNG_IN_PROCESS_PLUGINS` is set or no
/// helper is installed. On Apple Silicon, plugins without native code go to an
/// Intel helper (Rosetta) automatically.
pub fn load_plugin(host: Option<&Host>, spec: &PluginSpec, sample_rate: f64) -> Result<PluginInstance, String> {
    let native = Helper::native();
    if std::env::var_os("KVNG_IN_PROCESS_PLUGINS").is_some() || native.is_none() {
        let host = host.ok_or("the plugin helper (kvng-plugin-host) isn't installed next to KVNG Studio")?;
        return load_local(host, spec, sample_rate).map(PluginInstance::Local);
    }
    let native = native.unwrap();
    let intel = if cfg!(target_arch = "aarch64") { Helper::intel() } else { None };

    let needs_intel = matches!(&spec.format, PluginFormatSpec::Vst3 { path, .. } if !bridge::bundle_supports_this_arch(Path::new(path)));
    let first = if needs_intel { intel.clone().ok_or("it's Intel-only and the Intel plugin helper isn't installed")? } else { native };

    let mut plugin = match load_remote(&first, spec, sample_rate) {
        Ok(p) => p,
        // Audio Units don't expose their bundle path, so fall back to trying Intel.
        Err(e) if first.arch != "x86_64" && intel.is_some() => {
            load_remote(intel.as_ref().unwrap(), spec, sample_rate).map_err(|e2| format!("{e}; Intel helper: {e2}"))?
        }
        Err(e) => return Err(e),
    };
    let restored = match &spec.state {
        Some(StateSpec::Vst3 { component, controller }) => plugin.restore_vst3(component, controller.as_deref()),
        Some(StateSpec::AudioUnit { class_info }) => plugin.restore_au(class_info),
        Some(StateSpec::Host { data }) => plugin.load_state(data),
        None => Ok(()),
    };
    restored.map_err(|e| format!("loaded, but restoring its settings failed: {e}"))?;
    Ok(PluginInstance::Remote(plugin))
}

fn load_remote(helper: &Helper, spec: &PluginSpec, sample_rate: f64) -> Result<RemotePlugin, String> {
    match &spec.format {
        PluginFormatSpec::Vst3 { path, name } => RemotePlugin::load_vst3(helper, path, name.as_deref(), sample_rate, MAX_BLOCK),
        PluginFormatSpec::AudioUnit { kind, subtype, manufacturer } => RemotePlugin::load_au(helper, kind, subtype, manufacturer, sample_rate, MAX_BLOCK),
    }
}

/// Loads a plugin into this process (no crash isolation).
fn load_local(host: &Host, spec: &PluginSpec, sample_rate: f64) -> Result<Plugin, String> {
    let mut plugin = match &spec.format {
        PluginFormatSpec::Vst3 { path, name } => host.load_vst3(path, name.as_deref(), sample_rate, MAX_BLOCK).map_err(|e| {
            if !bridge::bundle_supports_this_arch(Path::new(path)) {
                "it's Intel-only; install the Intel plugin helper or run the Intel build".to_string()
            } else {
                e
            }
        })?,
        PluginFormatSpec::AudioUnit { kind, subtype, manufacturer } => host.load_au(kind, subtype, manufacturer, sample_rate, MAX_BLOCK)?,
    };
    match &spec.state {
        Some(StateSpec::Vst3 { component, controller }) => plugin.restore_vst3(component, controller.as_deref()),
        Some(StateSpec::AudioUnit { class_info }) => plugin.restore_au(class_info),
        Some(StateSpec::Host { data }) => plugin.load_state(data),
        None => {}
    }
    Ok(plugin)
}

/// Which engine channels play audio clips (needed to compile a timeline).
pub fn audio_clip_channels(channels: &[Channel]) -> Vec<bool> {
    channels.iter().map(|c| matches!(c.source, Source::AudioClip { .. })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_in_clicks_before_the_song_moves() {
        let mut loaded = build(None, Song::empty("t"), Path::new("t.kvng"), 48_000.0, &mut |_| {});
        let controls = loaded.engine.controls();
        controls.start_count_in(4096);
        controls.play();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        loaded.engine.process(&mut l, &mut r);
        assert_eq!(controls.position(), 0, "song waits during the count-in");
        assert!(l.iter().any(|v| v.abs() > 0.05), "count-in click is heard");
        for _ in 0..8 {
            loaded.engine.process(&mut l, &mut r);
        }
        assert_eq!(controls.count_in_left(), 0);
        assert!(controls.position() > 0, "song starts after the count-in");
    }

    /// The new-project template plays, and an insert's built-in compressor reports gain
    /// reduction through `Controls` while it does.
    #[test]
    fn template_plays_through_channel_strip() {
        let mut song = Song::empty("t");
        for beat in 0..4u64 {
            song.patterns[0].notes.push(crate::view::NoteView { start: beat * 96, length: 24, key: 60, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        }
        song.clips.push(crate::view::ClipView { track: 0, start: 0, length: 384, label: "P".into(), kind: crate::view::ClipKind::Pattern, pattern: Some(0), channel: None, source_offset: 0.0, automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false });
        song.inserts[1].strip.comp = crate::channel_strip::CompSettings { on: true, threshold: -40.0, ratio: 8.0, ..Default::default() };
        let mut loaded = build(None, song, Path::new("t.kvng"), 48_000.0, &mut |_| {});
        let controls = loaded.engine.controls();
        controls.play();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let (mut peak, mut gr) = (0.0f32, 0.0f32);
        for _ in 0..(48_000 / 512) {
            loaded.engine.process(&mut l, &mut r);
            peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
            gr = gr.max(controls.inserts[1].gain_reduction.load());
        }
        assert!(peak > 0.01, "template is silent (peak {peak})");
        assert!(gr > 3.0, "no gain reduction reported ({gr} dB)");
    }

    #[test]
    fn replacement_sample_is_found_by_channel_name() {
        let dir = std::env::temp_dir().join(format!("kvng-repl-test-{}", std::process::id()));
        let repl = dir.join(REPLACEMENTS_DIR);
        std::fs::create_dir_all(&repl).unwrap();
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(repl.join("808 Bounced.wav"), spec).unwrap();
        for i in 0..480 {
            w.write_sample(((i as f32 * 0.1).sin() * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();

        let ch = ChannelSpec { group: None, arp: Default::default(), color: None, sampler: Default::default(),
            name: "808 Bounced".into(),
            source: SourceSpec::FlNative { internal_name: "FLEX".into() },
            volume: 1.0,
            pan: 0.0,
            route: 15,
            muted: false,
            root_note: 60,
        };
        let mut warnings = Vec::new();
        let source = replacement(&ch, Some(&dir), 48_000.0, &mut warnings);
        assert!(matches!(source, Some(Source::Sampler { ref sample, .. }) if sample.len() == 480), "{warnings:?}");

        std::fs::remove_file(repl.join("808 Bounced.wav")).unwrap();
        let flac = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
        std::fs::copy(flac, repl.join("808 Bounced.FLAC")).unwrap();
        let source = replacement(&ch, Some(&dir), 48_000.0, &mut warnings);
        assert!(matches!(source, Some(Source::Sampler { ref sample, .. }) if sample.len() > 20_000), "{warnings:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_project_template_makes_sound() {
        use crate::view::{ClipKind, ClipView, NoteView};
        let mut song = Song::empty("t");
        // Kick (channel 0) on beat 1 and a synth chord note (channel 4) on beat 2.
        song.patterns[0].notes.push(NoteView { start: 0, length: 24, key: 60, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        song.patterns[0].notes.push(NoteView { start: 96, length: 96, key: 64, velocity: 100, channel: 4, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        song.clips.push(ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track: 0, start: 0, length: 384, label: "Pattern 1".into(), kind: ClipKind::Pattern, pattern: Some(0), channel: None, source_offset: 0.0 });
        let mut loaded = build(None, song, Path::new("/tmp/t.kvng"), 48_000.0, &mut |_| {});
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.view.channels[4].kind, "KVNG Synth");
        let [l, _] = loaded.engine.render();
        let samples_per_beat = 48_000.0 * 60.0 / 130.0;
        let rms = |a: usize, b: usize| (l[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32).sqrt();
        assert!(rms(0, 4000) > 0.05, "kick audible");
        let beat2 = samples_per_beat as usize;
        assert!(rms(beat2 + 2000, beat2 + 8000) > 0.01, "synth audible");
    }

    #[test]
    fn empty_song_builds_without_plugins() {
        let song = Song::empty("blank");
        let loaded = build(None, song, Path::new("/tmp/blank.kvng"), 48_000.0, &mut |_| {});
        assert_eq!(loaded.view.inserts.len(), crate::song::MAX_INSERTS + 1);
        assert_eq!(loaded.view.patterns.len(), 1);
        assert!(loaded.warnings.is_empty());
    }
}
