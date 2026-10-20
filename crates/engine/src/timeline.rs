//! Compiles the editable project (a [`ProjectView`]) into the engine's event list,
//! and carries new timelines to the audio thread while it plays.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};

use crate::view::{AutomationTarget, ClipKind, ProjectView};
use crate::{Event, EventKind, NoteExpr};

/// Time rendered after the last event so releases and effect tails ring out.
pub const TAIL_SECONDS: f64 = 3.0;

/// No playlist track (events that can't be muted per track).
pub const NO_TRACK: u16 = u16::MAX;

pub struct Timeline {
    /// Sorted by time, note-offs before note-ons at equal times.
    pub events: Vec<Event>,
    /// Song length in samples, including the tail.
    pub length: u64,
    /// Automation, sorted by start.
    pub automation: Vec<AutoSegment>,
    /// Beat positions for the metronome (sample, first beat of a bar), following the
    /// tempo map. Empty means "use the constant beat length".
    pub beats: Vec<(u64, bool)>,
}

/// A straight line of automation between two samples.
#[derive(Clone, Copy, Debug)]
pub struct AutoSegment {
    pub target: AutomationTarget,
    pub start: u64,
    pub end: u64,
    pub from: f32,
    pub to: f32,
}

impl AutoSegment {
    pub fn value_at(&self, sample: u64) -> f32 {
        if self.end <= self.start {
            return self.to;
        }
        let t = (sample.saturating_sub(self.start) as f64 / (self.end - self.start) as f64).clamp(0.0, 1.0) as f32;
        self.from + (self.to - self.from) * t
    }
}

/// Flattens the playlist into timed events. `is_audio_clip[c]` says whether engine
/// channel `c` plays audio clips (only those respond to audio clip items).
pub fn compile(view: &ProjectView, is_audio_clip: &[bool]) -> Timeline {
    let mutes = view.effective_mutes();
    let at = |ticks: f64| view.tick_to_sample_f(ticks).round() as u64;
    let mut events = Vec::new();
    let mut automation = Vec::new();
    let mut last = 0u64;

    for clip in &view.clips {
        let track = clip.track.min(NO_TRACK as usize - 1) as u16;
        let clip_start = clip.start as f64;
        let clip_end = clip_start + clip.length as f64;
        last = last.max(at(clip_end));

        match clip.kind {
            ClipKind::Pattern => {
                let Some(pattern) = clip.pattern.and_then(|p| view.patterns.get(p)) else { continue };
                for note in &pattern.notes {
                    let start = clip_start + view.swung(note.start) - clip.source_offset;
                    if start < clip_start || start >= clip_end {
                        continue;
                    }
                    let end = (start + note.length.max(1) as f64).min(clip_end);
                    let channel = note.channel;
                    if note.slide {
                        let duration = at(end).saturating_sub(at(start)) as u32;
                        events.push(Event { time: at(start), channel, track, kind: EventKind::Slide { key: note.key, duration } });
                        continue;
                    }
                    events.push(Event { time: at(start), channel, track, kind: EventKind::NoteOn { key: note.key, velocity: note.velocity, expr: NoteExpr::from_note(note.pan, note.mod_x, note.mod_y) } });
                    events.push(Event { time: at(end), channel, track, kind: EventKind::NoteOff { key: note.key } });
                }
            }
            ClipKind::Automation => {
                let Some(auto) = clip.automation.and_then(|a| view.automations.get(a)) else { continue };
                if mutes.get(clip.track).copied().unwrap_or(false) {
                    continue;
                }
                // Sample the curve at its points inside the clip, plus the clip's edges.
                let mut ticks: Vec<f64> = vec![clip.source_offset, clip.source_offset + clip.length as f64];
                ticks.extend(auto.points.iter().map(|p| p.tick as f64).filter(|t| *t > clip.source_offset && *t < clip.source_offset + clip.length as f64));
                ticks.sort_by(f64::total_cmp);
                // Curved segments are played as many short straight pieces.
                if auto.points.iter().any(|p| p.tension != 0.0) {
                    let coarse = std::mem::take(&mut ticks);
                    for w in coarse.windows(2) {
                        for k in 0..32 {
                            ticks.push(w[0] + (w[1] - w[0]) * k as f64 / 32.0);
                        }
                    }
                    if let Some(&last) = coarse.last() {
                        ticks.push(last);
                    }
                }
                for w in ticks.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    automation.push(AutoSegment {
                        target: auto.target,
                        start: at(clip_start + a - clip.source_offset),
                        end: at(clip_start + b - clip.source_offset),
                        from: auto.value_at(a),
                        to: auto.value_at(b),
                    });
                }
            }
            ClipKind::Audio => {
                let Some(channel) = clip.channel.filter(|&c| is_audio_clip.get(c).copied().unwrap_or(false)) else { continue };
                let offset = (clip.source_offset * view.sample_rate) as usize;
                let length = (at(clip_end) - at(clip_start)) as usize;
                let fade = |ticks: u64| ((ticks.min(clip.length) as f64 * view.samples_per_tick) as u32).min(length as u32);
                let kind = EventKind::Clip { offset, length, gain: clip.gain.max(0.0), fade_in: fade(clip.fade_in), fade_out: fade(clip.fade_out), reverse: clip.reverse };
                events.push(Event { time: at(clip_start), channel, track, kind });
            }
        }
    }

    sort(&mut events);
    automation.sort_by_key(|s| s.start);
    let length = last + (TAIL_SECONDS * view.sample_rate) as u64;
    // Metronome clicks follow the tempo map and time-signature changes (accent = bar start).
    let beats = if view.tempo_changes.is_empty() && view.meter_changes.is_empty() {
        Vec::new()
    } else {
        let ppq = view.ppq.max(1) as u64;
        (0..)
            .map(|b: u64| {
                let tick = b * ppq;
                (at(tick as f64), view.bar_at(tick).1 == tick)
            })
            .take_while(|(t, _)| *t < length)
            .collect()
    };
    Timeline { events, length, automation, beats }
}

/// PAT mode: just `pattern`, starting at zero, for looping over its length.
/// Returns the timeline and the loop length in samples.
pub fn compile_pattern(view: &ProjectView, pattern: usize) -> (Timeline, u64) {
    let at = |ticks: u64| (ticks as f64 * view.samples_per_tick).round() as u64;
    let mut events = Vec::new();
    let Some(p) = view.patterns.get(pattern) else {
        return (Timeline { events, length: 0, automation: vec![], beats: vec![] }, 0);
    };
    let at_f = |ticks: f64| (ticks * view.samples_per_tick).round() as u64;
    for note in &p.notes {
        if note.start >= p.length {
            continue;
        }
        let end = (note.start + note.length.max(1)).min(p.length);
        if note.slide {
            let duration = at(end).saturating_sub(at_f(view.swung(note.start))) as u32;
            events.push(Event { time: at_f(view.swung(note.start)), channel: note.channel, track: NO_TRACK, kind: EventKind::Slide { key: note.key, duration } });
            continue;
        }
        events.push(Event { time: at_f(view.swung(note.start)), channel: note.channel, track: NO_TRACK, kind: EventKind::NoteOn { key: note.key, velocity: note.velocity, expr: NoteExpr::from_note(note.pan, note.mod_x, note.mod_y) } });
        events.push(Event { time: at(end), channel: note.channel, track: NO_TRACK, kind: EventKind::NoteOff { key: note.key } });
    }
    sort(&mut events);
    let loop_len = at(p.length);
    (Timeline { events, length: loop_len + (TAIL_SECONDS * view.sample_rate) as u64, automation: vec![], beats: vec![] }, loop_len)
}

/// At equal times, note-offs go first so retriggered notes aren't cut off.
pub fn sort(events: &mut [Event]) {
    events.sort_by_key(|e| (e.time, !matches!(e.kind, EventKind::NoteOff { .. })));
}

/// The UI's end of the link to the audio thread.
pub struct TimelineSender {
    tx: SyncSender<Timeline>,
    /// Replaced timelines come back here so they're freed off the audio thread.
    garbage: Receiver<Timeline>,
}

impl TimelineSender {
    /// Queues a new timeline; the engine switches to it at its next block.
    pub fn send(&self, timeline: Timeline) {
        while self.garbage.try_recv().is_ok() {}
        match self.tx.try_send(timeline) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            // The engine hasn't picked up the previous one yet (e.g. audio stopped);
            // the newest edit wins, so retry once after making room.
            Err(TrySendError::Full(t)) => {
                std::thread::yield_now();
                let _ = self.tx.try_send(t);
            }
        }
    }
}

pub(crate) struct TimelineReceiver {
    pub rx: Receiver<Timeline>,
    pub garbage: Sender<Timeline>,
}

pub(crate) fn link() -> (TimelineSender, TimelineReceiver) {
    let (tx, rx) = mpsc::sync_channel(4);
    let (gtx, grx) = mpsc::channel();
    (TimelineSender { tx, garbage: grx }, TimelineReceiver { rx, garbage: gtx })
}
