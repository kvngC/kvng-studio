//! Standard MIDI files: importing their notes into patterns, and exporting patterns.

use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::view::NoteView;

/// One MIDI track's notes, in project ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    pub name: String,
    /// (start, length, key, velocity 0..=128)
    pub notes: Vec<(u64, u64, u8, u8)>,
}

/// What a MIDI file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub tracks: Vec<ImportedTrack>,
    /// The first tempo in the file, if any.
    pub tempo: Option<f64>,
}

/// Reads a MIDI file, converting its timing to `ppq` ticks per quarter note. Tracks without
/// notes are dropped; a type-0 file's 16 channels become separate tracks.
pub fn import(bytes: &[u8], ppq: u32) -> Result<Imported, String> {
    let smf = Smf::parse(bytes).map_err(|e| format!("not a MIDI file ({e})"))?;
    let file_ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as f64,
        Timing::Timecode(..) => return Err("SMPTE-timed MIDI files aren't supported".into()),
    };
    let scale = ppq as f64 / file_ppq.max(1.0);
    let mut tempo = None;
    let mut tracks = Vec::new();
    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut name = String::new();
        // Notes held: (channel, key) -> (start, velocity). Kept per channel so type-0 files split.
        let mut held: std::collections::HashMap<(u8, u8), (u64, u8)> = std::collections::HashMap::new();
        let mut by_channel: std::collections::BTreeMap<u8, Vec<(u64, u64, u8, u8)>> = Default::default();
        let mut t: u64 = 0;
        for ev in track {
            t += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) => name = String::from_utf8_lossy(n).trim().to_string(),
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if tempo.is_none() => tempo = Some(60_000_000.0 / us.as_int().max(1) as f64),
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    let (key, on_vel) = match message {
                        MidiMessage::NoteOn { key, vel } => (key.as_int(), vel.as_int()),
                        MidiMessage::NoteOff { key, .. } => (key.as_int(), 0),
                        _ => continue,
                    };
                    if let Some((start, vel)) = held.remove(&(ch, key)) {
                        let (s, e) = ((start as f64 * scale).round() as u64, (t as f64 * scale).round() as u64);
                        by_channel.entry(ch).or_default().push((s, (e - s).max(1), key, ((vel as u32 * 128) / 127) as u8));
                    }
                    if on_vel > 0 {
                        held.insert((ch, key), (t, on_vel));
                    }
                }
                _ => {}
            }
        }
        // Notes never released end where the track ends.
        for ((ch, key), (start, vel)) in held {
            let (s, e) = ((start as f64 * scale).round() as u64, (t as f64 * scale).round() as u64);
            by_channel.entry(ch).or_default().push((s, (e.max(s + 1)) - s, key, ((vel as u32 * 128) / 127) as u8));
        }
        let split = by_channel.len() > 1;
        for (ch, mut notes) in by_channel {
            notes.sort();
            let base = if name.is_empty() { format!("Track {}", ti + 1) } else { name.clone() };
            let name = if split { format!("{base} ch{}", ch + 1) } else { base };
            tracks.push(ImportedTrack { name, notes });
        }
    }
    if tracks.is_empty() {
        return Err("the MIDI file has no notes".into());
    }
    Ok(Imported { tracks, tempo })
}

/// Writes a type-1 MIDI file: a tempo track, then one track per (name, notes) group.
pub fn export(groups: &[(String, Vec<NoteView>)], ppq: u32, tempo: f64) -> Vec<u8> {
    let ppq = ppq.clamp(1, 32_767);
    let mut tracks: Vec<Vec<TrackEvent>> = Vec::new();
    let us = (60_000_000.0 / tempo.max(1.0)).round() as u32;
    tracks.push(vec![
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us.min(0xFF_FFFF)))) },
        TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) },
    ]);
    let names: Vec<Vec<u8>> = groups.iter().map(|(n, _)| n.as_bytes().to_vec()).collect();
    for (gi, (_, notes)) in groups.iter().enumerate() {
        let ch = u4::new((gi % 16) as u8);
        // (time, is_on, key, velocity) — offs before ons at the same time.
        let mut evs: Vec<(u64, bool, u8, u8)> = Vec::new();
        for n in notes {
            let vel = ((n.velocity.min(128) as u32 * 127) / 128).clamp(1, 127) as u8;
            evs.push((n.start, true, n.key.min(127), vel));
            evs.push((n.start + n.length.max(1), false, n.key.min(127), 0));
        }
        evs.sort_by_key(|e| (e.0, e.1));
        let mut track = vec![TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::TrackName(&names[gi])) }];
        let mut last = 0u64;
        for (t, on, key, vel) in evs {
            let delta = u28::new((t - last).min(0x0FFF_FFFF) as u32);
            last = t;
            let message = if on { MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(vel) } } else { MidiMessage::NoteOff { key: u7::new(key), vel: u7::new(0) } };
            track.push(TrackEvent { delta, kind: TrackEventKind::Midi { channel: ch, message } });
        }
        track.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        tracks.push(track);
    }
    let smf = Smf { header: Header::new(Format::Parallel, Timing::Metrical(u15::new(ppq as u16))), tracks };
    let mut out = Vec::new();
    let _ = smf.write_std(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u64, length: u64, key: u8, velocity: u8) -> NoteView {
        NoteView { start, length, key, velocity, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false }
    }

    #[test]
    fn export_then_import_round_trips_notes_and_tempo() {
        let groups = vec![("Lead".to_string(), vec![note(0, 96, 60, 100), note(96, 48, 64, 128), note(96, 48, 67, 64)]), ("Bass".to_string(), vec![note(0, 384, 36, 110)])];
        let bytes = export(&groups, 96, 140.0);
        let back = import(&bytes, 96).unwrap();
        assert!((back.tempo.unwrap() - 140.0).abs() < 0.01);
        assert_eq!(back.tracks.len(), 2);
        assert_eq!(back.tracks[0].name, "Lead");
        assert_eq!(back.tracks[0].notes.iter().map(|n| (n.0, n.1, n.2)).collect::<Vec<_>>(), vec![(0, 96, 60), (96, 48, 64), (96, 48, 67)]);
        assert_eq!(back.tracks[1].notes[0].1, 384);
        // Velocity survives the 0..128 ↔ 0..127 conversion within one step.
        assert!((back.tracks[0].notes[1].3 as i32 - 128).abs() <= 1);
    }

    #[test]
    fn rescales_timing_and_rejects_garbage() {
        let bytes = export(&[("x".to_string(), vec![note(480, 240, 60, 100)])], 960, 120.0);
        let back = import(&bytes, 96).unwrap();
        assert_eq!((back.tracks[0].notes[0].0, back.tracks[0].notes[0].1), (48, 24));
        assert!(import(b"not midi", 96).is_err());
    }
}
