//! Offline render of one channel's notes through its restored plugin.

use flp::{Channel, Note, Pattern, Project};
use engine::PluginInstance;
use plughost::{Host, MidiEvent};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK: usize = engine::MAX_BLOCK;
/// Extra time rendered after the last note so releases and reverb tails ring out.
const TAIL_SECONDS: f64 = 2.0;

struct Options {
    project: String,
    channel: String,
    pattern: Option<String>,
    out: String,
    editor: bool,
    /// Skip restoring the saved plugin state (for comparison/debugging).
    no_state: bool,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut it = args.iter();
    let project = it.next().ok_or("render: missing project path")?.clone();
    let mut opts = Options { project, channel: String::new(), pattern: None, out: "render.wav".into(), editor: false, no_state: false };
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--channel" => opts.channel = value()?,
            "--pattern" => opts.pattern = Some(value()?),
            "--out" => opts.out = value()?,
            "--editor" => opts.editor = true,
            "--no-state" => opts.no_state = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if opts.channel.is_empty() {
        return Err("render: --channel is required".into());
    }
    Ok(opts)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut opts = parse_args(args)?;
    opts.out = crate::play::absolute(&opts.out)?;
    let project = flp::read(&opts.project).map_err(|e| format!("{}: {e}", opts.project))?;

    let channel = find_channel(&project, &opts.channel)?;
    let pattern = find_pattern(&project, channel, opts.pattern.as_deref())?;
    let notes: Vec<Note> = pattern.notes.iter().copied().filter(|n| n.channel == channel.iid).collect();
    println!("Channel {:?}, pattern {:?}: {} notes", channel.name, pattern.name, notes.len());

    let host = Host::init();
    let mut plugin = load_channel_plugin(&host, &project, channel, !opts.no_state)?;
    println!(
        "Loaded {:?} ({}): {} out, MIDI {}, latency {} samples",
        plugin.name(),
        plugin.location(),
        plugin.num_outputs(),
        plugin.accepts_midi(),
        plugin.latency_samples()
    );

    // Some plugins finish loading samples asynchronously after a state restore.
    host.run_messages(1500);

    let audio = render_notes(&mut plugin, &notes, &project);
    write_wav(&opts.out, &audio, SAMPLE_RATE as u32)?;
    report_levels(&audio, SAMPLE_RATE);
    println!("Wrote {}", opts.out);

    if opts.editor {
        if plugin.open_editor() {
            println!("Editor open; close this terminal process (Ctrl+C) when done.");
            loop {
                host.run_messages(100);
            }
        }
        println!("Plugin has no editor.");
    }
    Ok(())
}

fn find_channel<'a>(project: &'a Project, query: &str) -> Result<&'a Channel, String> {
    let q = query.to_lowercase();
    project
        .channels
        .iter()
        .find(|c| c.name.to_lowercase() == q)
        .or_else(|| project.channels.iter().find(|c| c.name.to_lowercase().contains(&q)))
        .ok_or_else(|| format!("no channel matching {query:?}"))
}

fn find_pattern<'a>(project: &'a Project, channel: &Channel, query: Option<&str>) -> Result<&'a Pattern, String> {
    let plays_channel = |p: &&Pattern| p.notes.iter().any(|n| n.channel == channel.iid);
    match query {
        Some(q) => project
            .patterns
            .iter()
            .filter(plays_channel)
            .find(|p| p.name.eq_ignore_ascii_case(q))
            .ok_or_else(|| format!("no pattern named {q:?} with notes for {:?}", channel.name)),
        None => project
            .patterns
            .iter()
            .filter(plays_channel)
            .max_by_key(|p| p.notes.iter().filter(|n| n.channel == channel.iid).count())
            .ok_or_else(|| format!("{:?} has no notes in any pattern", channel.name)),
    }
}

fn load_channel_plugin(host: &Host, project: &Project, channel: &Channel, restore_state: bool) -> Result<PluginInstance, String> {
    let index = project.channels.iter().position(|c| c.iid == channel.iid).ok_or("channel not found")?;
    let song = engine::song::from_flp(project, std::path::Path::new(""));
    let engine::song::SourceSpec::Plugin(mut spec) = song.channels[index].source.clone() else {
        return Err(format!("{:?} is not a third-party plugin channel", channel.name));
    };
    if !restore_state {
        spec.state = None;
    }
    engine::load::load_plugin(Some(host), &spec, SAMPLE_RATE)
}

/// Renders stereo audio for `notes`, returning interleaved-by-channel buffers.
fn render_notes(plugin: &mut PluginInstance, notes: &[Note], project: &Project) -> [Vec<f32>; 2] {
    let samples_per_tick = SAMPLE_RATE * 60.0 / (project.tempo * project.ppq as f64);
    let to_samples = |ticks: u32| (ticks as f64 * samples_per_tick).round() as u64;

    let mut events: Vec<(u64, MidiEvent)> = Vec::with_capacity(notes.len() * 2);
    for n in notes {
        let key = n.key.min(127) as u8;
        // FL velocities run 0..=128 with 100 as default.
        let velocity = (n.velocity as u32 * 127 / 128).clamp(1, 127) as u8;
        let start = to_samples(n.position);
        let end = to_samples(n.position + n.length.max(1));
        events.push((start, MidiEvent::note_on(0, 0, key, velocity)));
        events.push((end, MidiEvent::note_off(0, 0, key)));
    }
    // Note-offs before note-ons at the same instant, so retriggered keys aren't cut.
    events.sort_by_key(|(t, e)| (*t, e.bytes[0] & 0xf0 == 0x90));

    let last = events.last().map_or(0, |(t, _)| *t);
    let total = last + (TAIL_SECONDS * SAMPLE_RATE) as u64;
    let channels = plugin.num_outputs().max(2);

    let mut out = [Vec::with_capacity(total as usize), Vec::with_capacity(total as usize)];
    let mut block: Vec<Vec<f32>> = vec![vec![0.0; BLOCK]; channels];
    let mut block_midi = Vec::new();
    let mut next_event = 0;
    let mut pos: u64 = 0;

    while pos < total {
        let len = BLOCK.min((total - pos) as usize);
        block_midi.clear();
        while next_event < events.len() && events[next_event].0 < pos + len as u64 {
            let (t, mut e) = events[next_event];
            e.sample_offset = (t - pos) as u32;
            block_midi.push(e);
            next_event += 1;
        }

        for ch in block.iter_mut() {
            ch[..len].fill(0.0);
        }
        let mut slices: Vec<&mut [f32]> = block.iter_mut().map(|c| &mut c[..len]).collect();
        plugin.process(&mut slices, &block_midi);

        out[0].extend_from_slice(&block[0][..len]);
        out[1].extend_from_slice(&block[1.min(channels - 1)][..len]);
        pos += len as u64;
    }
    out
}

pub fn write_wav(path: &str, audio: &[Vec<f32>; 2], sample_rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("{path}: {e}"))?;
    for (l, r) in audio[0].iter().zip(&audio[1]) {
        w.write_sample(*l).and_then(|_| w.write_sample(*r)).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

pub fn report_levels(audio: &[Vec<f32>; 2], sample_rate: f64) {
    let peak = audio.iter().flatten().fold(0.0f32, |m, s| m.max(s.abs()));
    let n = audio[0].len().max(1) as f64;
    let rms = (audio.iter().flatten().map(|s| (*s as f64).powi(2)).sum::<f64>() / (2.0 * n)).sqrt();
    let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { f64::NEG_INFINITY };
    println!("Length {:.1}s, peak {:.1} dBFS, RMS {:.1} dBFS", n / sample_rate, db(peak as f64), db(rms));
}

/// `probe <project> --channel <name>`: loads one channel's plugin, plays a test
/// note through it, and reports whether it survived.
pub fn probe(args: &[String]) -> Result<(), String> {
    let mut opts = parse_args(args)?;
    opts.out = String::new();
    let project = flp::read(&opts.project).map_err(|e| format!("{}: {e}", opts.project))?;
    let channel = find_channel(&project, &opts.channel)?;
    let host = Host::init();
    let mut plugin = load_channel_plugin(&host, &project, channel, true)?;
    println!("Loaded {:?} ({})", plugin.name(), plugin.location());
    if let PluginInstance::Remote(r) = &plugin {
        println!("Editor open test: {:?}", r.editor_handle().open());
        match r.editor_handle().parameters() {
            Ok(params) => {
                let shown: Vec<String> = params.iter().take(6).map(|(n, v)| format!("{n}={:.2}", v)).collect();
                println!("{} parameters: {}…", params.len(), shown.join(", "));
            }
            Err(e) => println!("parameters unavailable: {e}"),
        }
    }

    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];
    let blocks = (SAMPLE_RATE as usize * 2) / BLOCK;
    let mut peak = 0.0f32;
    for i in 0..blocks {
        let midi = match i {
            0 => vec![MidiEvent::note_on(0, 0, 60, 100)],
            40 => vec![MidiEvent::note_off(0, 0, 60)],
            _ => vec![],
        };
        l.fill(0.0);
        r.fill(0.0);
        plugin.process(&mut [&mut l[..], &mut r[..]], &midi);
        peak = l.iter().chain(&r).fold(peak, |m, s| m.max(s.abs()));
        if !plugin.is_alive() {
            println!("Plugin crashed after {i} blocks; KVNG Studio kept running.");
            return Ok(());
        }
    }
    println!("Plugin survived 2s of audio; peak {:.1} dBFS", 20.0 * peak.max(1e-9).log10());
    Ok(())
}
