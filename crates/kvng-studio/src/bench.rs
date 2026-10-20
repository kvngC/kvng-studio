//! `kvng-studio bench`: measures the engine's mixing and channel-strip cost across insert
//! counts, sample rates and buffer sizes (PROFESSIONAL_ROADMAP.md, verification item 6).
//!
//! Each insert gets one channel playing a sustained noise sample, so the figures show
//! mixer + EQ/compressor cost rather than instrument cost. CPU% is processing time divided
//! by the audio's duration on one core; under 100% is real time, lower is more headroom.

use std::sync::Arc;
use std::time::Instant;

use engine::channel_strip::{CompSettings, StripSettings};
use engine::sample::Sample;
use engine::{Channel, Engine, Event, EventKind, Insert, NoteExpr, Source};

fn engine_for(inserts: usize, sample_rate: f64, strip_on: bool) -> Engine {
    let seconds = 3.0;
    let n = (sample_rate * seconds) as usize;
    let mut seed = 0x1234_5678u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32 - 0.5) * 0.2
    };
    let sample = Arc::new(Sample { left: (0..n).map(|_| noise()).collect(), right: (0..n).map(|_| noise()).collect() });
    let mut strip = StripSettings::default();
    if strip_on {
        strip.eq.on = true;
        strip.eq.bands[0].gain = 3.0;
        strip.eq.bands[2].gain = -2.0;
        strip.eq.bands[4].gain = 2.0;
        strip.eq.hp_on = true;
        strip.comp = CompSettings { on: true, threshold: -24.0, ..Default::default() };
    }
    let mut ins = vec![Insert::with_gain("Master".into(), true, 1.0, 0.0, vec![])];
    let mut channels = Vec::new();
    let mut events = Vec::new();
    for i in 1..=inserts {
        let mut insert = Insert::with_gain(format!("{i}"), true, 0.5, 0.0, vec![0]);
        insert.strip.settings = strip;
        ins.push(insert);
        channels.push(Channel::new(format!("ch{i}"), Source::Sampler { sample: sample.clone(), root_note: 60, voices: Vec::new() }, 0.5, 0.0, i));
        events.push(Event { time: 0, channel: i - 1, track: engine::timeline::NO_TRACK, kind: EventKind::NoteOn { key: 60, velocity: 100, expr: NoteExpr::default() } });
    }
    Engine::new(sample_rate, channels, ins, events, n as u64)
}

/// CPU% for rendering 2 s at `block` frames per call.
fn measure(inserts: usize, sample_rate: f64, block: usize, strip_on: bool) -> f64 {
    let mut engine = engine_for(inserts, sample_rate, strip_on);
    engine.controls().play();
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    // Warm up (first notes, caches), then time.
    for _ in 0..(sample_rate as usize / 10 / block).max(1) {
        engine.process(&mut l, &mut r);
    }
    let frames = (sample_rate * 2.0) as usize;
    let started = Instant::now();
    let mut done = 0;
    while done < frames {
        engine.process(&mut l, &mut r);
        done += block;
    }
    started.elapsed().as_secs_f64() / (done as f64 / sample_rate) * 100.0
}

pub fn run(args: &[String]) -> Result<(), String> {
    let quick = args.iter().any(|a| a == "--quick");
    let inserts: &[usize] = if quick { &[16, 128] } else { &[1, 16, 64, 128] };
    let rates: &[f64] = if quick { &[48_000.0] } else { &[44_100.0, 48_000.0, 96_000.0] };
    let blocks: &[usize] = if quick { &[128] } else { &[64, 128, 512] };
    println!("KVNG Studio engine benchmark ({} build)", if cfg!(debug_assertions) { "debug — use --release for real figures" } else { "release" });
    println!("CPU% of one core per second of audio (strips off → EQ + compressor on every insert)\n");
    println!("{:>8} {:>9} {:>7} {:>11} {:>11} {:>10}", "inserts", "rate", "block", "strips off", "strips on", "strip cost");
    for &n in inserts {
        for &sr in rates {
            for &block in blocks {
                let off = measure(n, sr, block, false);
                let on = measure(n, sr, block, true);
                println!("{n:>8} {:>8.1}k {block:>7} {off:>10.2}% {on:>10.2}% {:>9.3}%/insert", sr / 1000.0, (on - off).max(0.0) / n as f64);
            }
        }
    }
    Ok(())
}
