//! `play` (real-time through the speakers) and `bounce` (offline to WAV) for a whole song.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::Engine;
use plughost::Host;

struct Options {
    project: String,
    from_bar: f64,
    out: String,
    /// Stop live playback after this many seconds.
    seconds: Option<f64>,
    /// Bounce one file per used mixer insert into this folder.
    stems: Option<String>,
}

fn parse_args(args: &[String], default_out: &str) -> Result<Options, String> {
    let mut it = args.iter();
    let project = it.next().ok_or("missing project path")?.clone();
    let mut opts = Options { project, from_bar: 1.0, out: default_out.into(), seconds: None, stems: None };
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--from" => opts.from_bar = value()?.parse().map_err(|_| "--from expects a bar number")?,
            "--out" => opts.out = value()?,
            "--stems" => opts.stems = Some(value()?),
            "--for" => opts.seconds = Some(value()?.parse().map_err(|_| "--for expects seconds")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(opts)
}

/// Plugins may change the working directory while loading, so resolve paths first.
pub fn absolute(path: &str) -> Result<String, String> {
    std::path::absolute(path).map(|p| p.to_string_lossy().into_owned()).map_err(|e| format!("{path}: {e}"))
}

/// Loads an FL Studio (`.flp`) or KVNG (`.kvng`) project.
pub fn load_any(host: &Host, path: &str, sample_rate: f64) -> Result<engine::load::Loaded, String> {
    let p = Path::new(path);
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(engine::song::EXTENSION)) {
        let song = engine::song::Song::read(p)?;
        Ok(engine::load::build(Some(host), song, p, sample_rate, &mut |_| {}))
    } else {
        let project = flp::read(path).map_err(|e| format!("{path}: {e}"))?;
        Ok(engine::load::from_project(Some(host), &project, p, sample_rate, &mut |_| {}))
    }
}

/// `convert <project> --out file.kvng`: loads a project (starting its plugins) and saves it
/// as KVNG, capturing each plugin's current settings the same way the app's Save does.
pub fn convert(args: &[String]) -> Result<(), String> {
    let opts = parse_args(args, "")?;
    if opts.out.is_empty() {
        return Err("convert: --out <file.kvng> is required".into());
    }
    let out = absolute(&opts.out)?;
    let path = absolute(&opts.project)?;
    let host = Host::init();
    let loaded = load_any(&host, &path, 48_000.0)?;
    host.run_messages(1500);
    let (song, problems) = engine::song::capture(&loaded.song, &loaded.view, &loaded.engine.controls());
    song.write(Path::new(&out))?;
    for p in &problems {
        println!("  ! {p}");
    }
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("Wrote {out} ({:.1} MB)", bytes as f64 / 1e6);
    Ok(())
}

struct Song {
    engine: Engine,
    samples_per_bar: f64,
}

fn load_song(host: &Host, path: &str, sample_rate: f64) -> Result<Song, String> {
    let path = &absolute(path)?;
    let loaded = load_any(host, path, sample_rate)?;
    println!("Loaded {:?} ({} BPM)", Path::new(path).file_name().unwrap_or_default(), loaded.view.tempo);
    for w in &loaded.warnings {
        println!("  ! {w}");
    }
    let engine = loaded.engine;
    let beats_per_bar = loaded.view.beats_per_bar.max(1) as f64;
    let samples_per_bar = sample_rate * 60.0 / loaded.view.tempo.max(1.0) * beats_per_bar;

    let plugins = engine.channels.iter().filter(|c| matches!(c.source, engine::Source::Plugin { .. })).count();
    let effects: usize = engine.inserts.iter().map(|i| i.effects.len()).sum();
    println!(
        "Ready: {} channels ({} plugins), {} mixer effects, {:.0}s",
        engine.channels.len(),
        plugins,
        effects,
        engine.length as f64 / sample_rate
    );
    Ok(Song { engine, samples_per_bar })
}

pub fn bounce(args: &[String]) -> Result<(), String> {
    const SAMPLE_RATE: f64 = 48_000.0;
    let opts = parse_args(args, "bounce.wav")?;
    let out = absolute(&opts.out)?;
    let host = Host::init();
    let mut song = load_song(&host, &opts.project, SAMPLE_RATE)?;
    // Let plugins finish any asynchronous loading triggered by their restored state.
    host.run_messages(1500);

    song.engine.seek(((opts.from_bar - 1.0).max(0.0) * song.samples_per_bar) as u64);
    let stem_inserts: Vec<usize> = (0..song.engine.inserts.len()).filter(|&i| i == 0 || song.engine.channels.iter().any(|c| c.insert == i)).collect();
    if opts.stems.is_some() {
        song.engine.capture_stems(&stem_inserts);
    }
    let audio = song.engine.render();
    if let Some(dir) = &opts.stems {
        let dir = absolute(dir)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let latency = song.engine.latency;
        for (i, [l, r]) in song.engine.take_stems() {
            let name = format!("{dir}/{i:02} {}.wav", song.engine.inserts[i].name.replace('/', "_"));
            let skip = latency.min(l.len());
            crate::render::write_wav(&name, &[l[skip..].to_vec(), r[skip..].to_vec()], SAMPLE_RATE as u32)?;
        }
        println!("Wrote {} stems to {dir}", stem_inserts.len());
    }
    crate::render::write_wav(&out, &audio, SAMPLE_RATE as u32)?;
    crate::render::report_levels(&audio, SAMPLE_RATE);
    println!("Wrote {out}");
    Ok(())
}

pub fn play(args: &[String]) -> Result<(), String> {
    let opts = parse_args(args, "")?;
    let host = Host::init();

    let device = cpal::default_host().default_output_device().ok_or("no audio output device")?;
    let supported = device.default_output_config().map_err(|e| format!("audio device: {e}"))?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("unsupported output format {:?}", supported.sample_format()));
    }
    let config: cpal::StreamConfig = supported.into();
    let sample_rate = config.sample_rate as f64;
    let out_channels = config.channels.max(1) as usize;

    let mut song = load_song(&host, &opts.project, sample_rate)?;
    host.run_messages(1500);
    song.engine.seek(((opts.from_bar - 1.0).max(0.0) * song.samples_per_bar) as u64);

    let position = Arc::new(AtomicU64::new(song.engine.position()));
    let finished = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst)).map_err(|e| e.to_string())?;
    }

    let mut engine = song.engine;
    let controls = engine.controls();
    controls.play();
    let (pos_out, fin_out, ctl) = (position.clone(), finished.clone(), controls.clone());
    // Worst callback time as a share of the time the callback's audio lasts (per mille).
    let max_load = Arc::new(AtomicU64::new(0));
    let load_out = max_load.clone();
    let total_load = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
    let total_out = total_load.clone();
    // Scratch buffers sized generously so the callback never allocates.
    let mut left = vec![0.0f32; 16384];
    let mut right = vec![0.0f32; 16384];
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let started = std::time::Instant::now();
                for chunk in data.chunks_mut(left.len() * out_channels) {
                    let frames = chunk.len() / out_channels;
                    engine.process(&mut left[..frames], &mut right[..frames]);
                    for (i, frame) in chunk.chunks_exact_mut(out_channels).enumerate() {
                        frame[0] = left[i];
                        if out_channels > 1 {
                            frame[1] = right[i];
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0.0;
                        }
                    }
                }
                let budget = (data.len() / out_channels) as f64 / sample_rate;
                let load = (started.elapsed().as_secs_f64() / budget * 1000.0) as u64;
                load_out.fetch_max(load, Ordering::Relaxed);
                total_out.0.fetch_add(load, Ordering::Relaxed);
                total_out.1.fetch_add(1, Ordering::Relaxed);
                pos_out.store(engine.position(), Ordering::Relaxed);
                if !ctl.is_playing() {
                    fin_out.store(true, Ordering::Relaxed);
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("couldn't open audio output: {e}"))?;
    stream.play().map_err(|e| format!("couldn't start audio: {e}"))?;

    println!("Playing at {} Hz. Ctrl+C to stop.", sample_rate);
    let started = std::time::Instant::now();
    let deadline = opts.seconds.map(std::time::Duration::from_secs_f64);
    while !stop.load(Ordering::SeqCst)
        && !finished.load(Ordering::Relaxed)
        && deadline.is_none_or(|d| started.elapsed() < d)
    {
        // Keeps plugin windows and JUCE's housekeeping alive while audio runs.
        host.run_messages(100);
        let bar = position.load(Ordering::Relaxed) as f64 / song.samples_per_bar + 1.0;
        print!("\r  bar {:6.2}", bar);
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    drop(stream);
    let load = max_load.load(Ordering::Relaxed) as f64 / 10.0;
    let avg = total_load.0.load(Ordering::Relaxed) as f64 / total_load.1.load(Ordering::Relaxed).max(1) as f64 / 10.0;
    println!(
        "\nStopped. Audio load: average {avg:.0}%, peak {load:.0}%{}",
        if load >= 100.0 { " (dropouts likely)" } else { "" }
    );
    Ok(())
}
