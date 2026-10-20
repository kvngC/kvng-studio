//! A read-only description of a loaded project for the UI.
//!
//! Positions are in ticks (see [`ProjectView::ppq`]); indices refer to the
//! engine's channel and insert order.

use std::sync::Arc;

use bridge::EditorHandle;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct ProjectView {
    pub name: String,
    pub tempo: f64,
    pub ppq: u32,
    pub beats_per_bar: u32,
    pub sample_rate: f64,
    /// Ticks to samples at the project tempo.
    pub samples_per_tick: f64,
    /// End of the last clip.
    pub length_ticks: u64,
    pub tracks: Vec<TrackView>,
    pub clips: Vec<ClipView>,
    pub patterns: Vec<PatternView>,
    pub automations: Vec<AutomationView>,
    /// Channel rack swing, 0.0..=1.0 (delays every second sixteenth note).
    pub swing: f32,
    pub markers: Vec<Marker>,
    /// Tempo changes after the start (sorted by tick); `tempo` applies before the first.
    pub tempo_changes: Vec<TempoChange>,
    /// Time-signature changes, sorted by bar (the first bars use `beats_per_bar`).
    pub meter_changes: Vec<MeterChange>,
    pub controller_links: Vec<ControllerLink>,
    pub channels: Vec<ChannelView>,
    pub inserts: Vec<InsertView>,
    pub warnings: Vec<String>,
}

impl ProjectView {
    /// Ticks in one bar of the project's starting meter.
    pub fn ticks_per_bar(&self) -> u64 {
        (self.ppq * self.beats_per_bar) as u64
    }

    /// Beats in bar `bar` (0-based), following time-signature changes.
    pub fn beats_in_bar(&self, bar: u64) -> u32 {
        self.meter_changes.iter().rev().find(|m| m.bar <= bar).map_or(self.beats_per_bar, |m| m.beats_per_bar).max(1)
    }

    /// Where bar `bar` (0-based) starts, in ticks.
    pub fn bar_start(&self, bar: u64) -> u64 {
        let ppq = self.ppq.max(1) as u64;
        let mut tick = 0;
        let mut b = 0;
        let mut beats = self.beats_per_bar.max(1) as u64;
        for m in self.meter_changes.iter().filter(|m| m.bar <= bar) {
            tick += (m.bar - b) * beats * ppq;
            b = m.bar;
            beats = m.beats_per_bar.max(1) as u64;
        }
        tick + (bar - b) * beats * ppq
    }

    /// The bar (0-based) containing `tick`, where it starts, and its length in ticks.
    pub fn bar_at(&self, tick: u64) -> (u64, u64, u64) {
        let ppq = self.ppq.max(1) as u64;
        let (mut b, mut start, mut beats) = (0u64, 0u64, self.beats_per_bar.max(1) as u64);
        for m in &self.meter_changes {
            let change_at = start + (m.bar.saturating_sub(b)) * beats * ppq;
            if change_at > tick {
                break;
            }
            start = change_at;
            b = m.bar;
            beats = m.beats_per_bar.max(1) as u64;
        }
        let len = beats * ppq;
        let n = (tick - start) / len;
        (b + n, start + n * len, len)
    }

    fn samples_per_tick_at(&self, bpm: f64) -> f64 {
        self.sample_rate * 60.0 / (bpm.max(1.0) * self.ppq.max(1) as f64)
    }

    /// Tempo segments as (start tick, samples per tick), following the tempo map.
    fn segments(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        std::iter::once((0.0, self.samples_per_tick)).chain(self.tempo_changes.iter().filter(|c| c.tick > 0).map(|c| (c.tick as f64, self.samples_per_tick_at(c.bpm))))
    }

    /// Exact sample position of `tick`, following tempo changes.
    pub fn tick_to_sample_f(&self, tick: f64) -> f64 {
        let tick = tick.max(0.0);
        let segs: Vec<(f64, f64)> = self.segments().collect();
        let mut samples = 0.0;
        for (i, &(start, spt)) in segs.iter().enumerate() {
            let end = segs.get(i + 1).map_or(f64::INFINITY, |s| s.0);
            if tick <= start {
                break;
            }
            samples += (tick.min(end) - start) * spt;
        }
        samples
    }

    pub fn tick_to_sample(&self, tick: f64) -> u64 {
        self.tick_to_sample_f(tick) as u64
    }

    pub fn sample_to_tick(&self, sample: u64) -> f64 {
        let target = sample as f64;
        let segs: Vec<(f64, f64)> = self.segments().collect();
        let mut samples = 0.0;
        for (i, &(start, spt)) in segs.iter().enumerate() {
            let end = segs.get(i + 1).map_or(f64::INFINITY, |s| s.0);
            let seg_samples = (end - start) * spt;
            if target < samples + seg_samples || end.is_infinite() {
                return start + (target - samples) / spt;
            }
            samples += seg_samples;
        }
        target / self.samples_per_tick
    }

    /// The tempo in effect at `tick`.
    pub fn tempo_at(&self, tick: u64) -> f64 {
        self.tempo_changes.iter().rev().find(|c| c.tick <= tick && c.tick > 0).map_or(self.tempo, |c| c.bpm)
    }

    /// One step-sequencer step: a sixteenth note.
    pub fn step_ticks(&self) -> u64 {
        (self.ppq as u64 / 4).max(1)
    }

    /// Where a note starting at `tick` actually plays once swing is applied: notes on
    /// every second sixteenth move later by up to two thirds of a step.
    pub fn swung(&self, tick: u64) -> f64 {
        let step = self.step_ticks();
        let on_offbeat = (tick / step) % 2 == 1 && tick % step <= step / 8;
        if self.swing > 0.0 && on_offbeat { tick as f64 + self.swing as f64 * step as f64 * (2.0 / 3.0) } else { tick as f64 }
    }

    /// Sets a pattern's length in steps (at least one).
    pub fn set_pattern_steps(&mut self, pattern: usize, steps: u64) {
        let len = steps.max(1) * self.step_ticks();
        if let Some(p) = self.patterns.get_mut(pattern) {
            p.length = len;
        }
    }

    /// Whether `channel` has a hit starting on `step` of `pattern`.
    pub fn step_on(&self, pattern: usize, channel: usize, step: u64) -> bool {
        let (lo, hi) = (step * self.step_ticks(), (step + 1) * self.step_ticks());
        self.patterns.get(pattern).is_some_and(|p| p.notes.iter().any(|n| n.channel == channel && n.start >= lo && n.start < hi))
    }

    /// Turns a step on (adding a one-step note at the channel's root key) or off
    /// (removing every note of that channel starting in the step). Returns the new state.
    pub fn toggle_step(&mut self, pattern: usize, channel: usize, step: u64) -> bool {
        let len = self.step_ticks();
        let (lo, hi) = (step * len, (step + 1) * len);
        let root = self.channels.get(channel).map_or(60, |c| c.root_note);
        let ticks_per_bar = self.ticks_per_bar();
        let Some(p) = self.patterns.get_mut(pattern) else { return false };
        let before = p.notes.len();
        p.notes.retain(|n| !(n.channel == channel && n.start >= lo && n.start < hi));
        let on = p.notes.len() == before;
        if on {
            p.notes.push(NoteView { start: lo, length: len, key: root, velocity: 100, channel, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        }
        p.fit_length(ticks_per_bar);
        on
    }

    /// Adds a note and returns its index in the pattern.
    pub fn add_note(&mut self, pattern: usize, note: NoteView) -> Option<usize> {
        let ticks_per_bar = self.ticks_per_bar();
        let p = self.patterns.get_mut(pattern)?;
        p.notes.push(note);
        p.fit_length(ticks_per_bar);
        Some(p.notes.len() - 1)
    }

    pub fn remove_note(&mut self, pattern: usize, index: usize) {
        if let Some(p) = self.patterns.get_mut(pattern)
            && index < p.notes.len()
        {
            p.notes.remove(index);
        }
    }

    /// Moves and/or resizes a note, keeping it inside the valid key and time range.
    pub fn set_note(&mut self, pattern: usize, index: usize, start: i64, key: i32, length: i64) {
        let ticks_per_bar = self.ticks_per_bar();
        let min_len = (self.step_ticks() / 4).max(1) as i64;
        if let Some(p) = self.patterns.get_mut(pattern)
            && let Some(n) = p.notes.get_mut(index)
        {
            n.start = start.max(0) as u64;
            n.key = key.clamp(0, 127) as u8;
            n.length = length.max(min_len) as u64;
            p.fit_length(ticks_per_bar);
        }
    }
}

impl ProjectView {
    /// Adds an empty one-bar pattern and returns its index.
    pub fn new_pattern(&mut self) -> usize {
        let name = (1..).map(|i| format!("Pattern {i}")).find(|n| !self.patterns.iter().any(|p| &p.name == n)).unwrap();
        self.patterns.push(PatternView { color: None, name, notes: vec![], length: self.ticks_per_bar() });
        self.patterns.len() - 1
    }

    /// Duplicates a pattern (named like FL: "name #2") and returns the copy's index.
    pub fn clone_pattern(&mut self, pattern: usize) -> Option<usize> {
        let mut copy = self.patterns.get(pattern)?.clone();
        let base = copy.name.split(" #").next().unwrap_or(&copy.name).to_string();
        copy.name = (2..).map(|i| format!("{base} #{i}")).find(|n| !self.patterns.iter().any(|p| &p.name == n)).unwrap();
        self.patterns.insert(pattern + 1, copy);
        // Clips pointing past the insertion point shift by one.
        for c in &mut self.clips {
            if let Some(p) = c.pattern.as_mut()
                && *p > pattern
            {
                *p += 1;
            }
        }
        Some(pattern + 1)
    }

    /// Removes a pattern and every playlist clip of it.
    pub fn delete_pattern(&mut self, pattern: usize) {
        if pattern >= self.patterns.len() {
            return;
        }
        self.patterns.remove(pattern);
        self.clips.retain(|c| c.pattern != Some(pattern));
        for c in &mut self.clips {
            if let Some(p) = c.pattern.as_mut()
                && *p > pattern
            {
                *p -= 1;
            }
        }
        if self.patterns.is_empty() {
            self.new_pattern();
        }
    }

    /// Renames a pattern and the labels of its clips.
    pub fn rename_pattern(&mut self, pattern: usize, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(p) = self.patterns.get_mut(pattern) {
            p.name = name.to_string();
        }
        for c in self.clips.iter_mut().filter(|c| c.pattern == Some(pattern)) {
            c.label = name.to_string();
        }
    }

    /// Inserts `ticks` of empty time at `at`: clips, markers and tempo changes from there on
    /// move later. A clip that spans `at` is split so its second half moves too.
    pub fn insert_time(&mut self, at: u64, ticks: u64) {
        if ticks == 0 {
            return;
        }
        // Whole bars inserted at a bar line push later time-signature changes back.
        let (bar, start, len) = self.bar_at(at);
        if start == at && ticks.is_multiple_of(len) {
            let n = ticks / len;
            for m in &mut self.meter_changes {
                if m.bar >= bar {
                    m.bar += n;
                }
            }
        }
        let spanning: Vec<usize> = (0..self.clips.len()).filter(|&i| self.clips[i].start < at && self.clips[i].start + self.clips[i].length > at).collect();
        for i in spanning {
            self.split_clip_at(i, at);
        }
        for c in &mut self.clips {
            if c.start >= at {
                c.start += ticks;
            }
        }
        for m in &mut self.markers {
            if m.tick >= at {
                m.tick += ticks;
            }
        }
        for t in &mut self.tempo_changes {
            if t.tick >= at {
                t.tick += ticks;
            }
        }
    }

    /// Removes the time `a..b`: what's inside goes, later material moves earlier.
    pub fn delete_time(&mut self, a: u64, b: u64) {
        if b <= a {
            return;
        }
        // Whole bars removed take their time-signature changes along; later ones move up.
        let (bar_a, start_a, _) = self.bar_at(a);
        let (bar_b, start_b, _) = self.bar_at(b);
        if start_a == a && start_b == b {
            let n = bar_b - bar_a;
            self.meter_changes.retain(|m| m.bar < bar_a || m.bar >= bar_b);
            for m in &mut self.meter_changes {
                if m.bar >= bar_b {
                    m.bar -= n;
                }
            }
        }
        let span = b - a;
        for edge in [a, b] {
            let spanning: Vec<usize> = (0..self.clips.len()).filter(|&i| self.clips[i].start < edge && self.clips[i].start + self.clips[i].length > edge).collect();
            for i in spanning {
                self.split_clip_at(i, edge);
            }
        }
        self.clips.retain(|c| !(c.start >= a && c.start < b));
        for c in &mut self.clips {
            if c.start >= b {
                c.start -= span;
            }
        }
        self.markers.retain(|m| !(m.tick >= a && m.tick < b));
        for m in &mut self.markers {
            if m.tick >= b {
                m.tick -= span;
            }
        }
        self.tempo_changes.retain(|t| !(t.tick >= a && t.tick < b));
        for t in &mut self.tempo_changes {
            if t.tick >= b {
                t.tick -= span;
            }
        }
    }

    /// Repeats the time `a..b` right after itself (FL's "duplicate selection").
    pub fn duplicate_time(&mut self, a: u64, b: u64) {
        if b <= a {
            return;
        }
        self.insert_time(b, b - a);
        for edge in [a] {
            let spanning: Vec<usize> = (0..self.clips.len()).filter(|&i| self.clips[i].start < edge && self.clips[i].start + self.clips[i].length > edge).collect();
            for i in spanning {
                self.split_clip_at(i, edge);
            }
        }
        let copies: Vec<ClipView> = self
            .clips
            .iter()
            .filter(|c| c.start >= a && c.start < b)
            .map(|c| {
                let mut c = c.clone();
                c.start += b - a;
                c.length = c.length.min(b - (c.start - (b - a)));
                c
            })
            .collect();
        self.clips.extend(copies);
    }

    /// Splits clip `index` at tick `at` (no-op unless `at` is strictly inside it).
    pub fn split_clip_at(&mut self, index: usize, at: u64) -> bool {
        let Some(clip) = self.clips.get(index).cloned() else { return false };
        if at <= clip.start || at >= clip.start + clip.length {
            return false;
        }
        let first_len = at - clip.start;
        let mut second = clip.clone();
        second.start = at;
        second.length = clip.length - first_len;
        let units = |ticks: u64| match clip.kind {
            ClipKind::Pattern | ClipKind::Automation => ticks as f64,
            ClipKind::Audio => ticks as f64 * self.samples_per_tick / self.sample_rate,
        };
        let first = &mut self.clips[index];
        first.length = first_len;
        if clip.reverse {
            first.source_offset = clip.source_offset + units(second.length);
        } else {
            second.source_offset += units(first_len);
        }
        first.fade_out = 0;
        first.fade_in = clip.fade_in.min(first_len);
        second.fade_in = 0;
        second.fade_out = clip.fade_out.min(second.length);
        self.clips.push(second);
        true
    }

    /// The folder track each track belongs to (None for top-level tracks and folders).
    pub fn track_folders(&self) -> Vec<Option<usize>> {
        let mut folder = None;
        self.tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if t.folder {
                    folder = Some(i);
                    None
                } else if t.grouped {
                    folder
                } else {
                    folder = None;
                    None
                }
            })
            .collect()
    }

    /// Whether each track is hidden inside a collapsed folder.
    pub fn hidden_tracks(&self) -> Vec<bool> {
        self.track_folders().iter().map(|f| f.is_some_and(|f| self.tracks[f].collapsed)).collect()
    }

    /// Whether each track is silent: muted itself, or inside a muted folder.
    pub fn effective_mutes(&self) -> Vec<bool> {
        self.track_folders().iter().enumerate().map(|(i, f)| self.tracks[i].muted || f.is_some_and(|f| self.tracks[f].muted)).collect()
    }

    /// Inserts an empty track at `index` (clips below move down).
    pub fn insert_track(&mut self, index: usize) {
        let index = index.min(self.tracks.len());
        for c in &mut self.clips {
            if c.track >= index {
                c.track += 1;
            }
        }
        self.tracks.insert(index, TrackView { name: format!("Track {}", self.tracks.len() + 1), muted: false, color: None, folder: false, collapsed: false, grouped: false });
    }

    /// Deletes track `index` and its clips (tracks below move up).
    pub fn delete_track(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }
        self.clips.retain(|c| c.track != index);
        for c in &mut self.clips {
            if c.track > index {
                c.track -= 1;
            }
        }
        self.tracks.remove(index);
    }

    /// Moves track `from` to position `to`, taking its clips along.
    pub fn move_track(&mut self, from: usize, to: usize) {
        let n = self.tracks.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let t = self.tracks.remove(from);
        self.tracks.insert(to, t);
        for c in &mut self.clips {
            c.track = if c.track == from {
                to
            } else if from < to && c.track > from && c.track <= to {
                c.track - 1
            } else if to < from && c.track >= to && c.track < from {
                c.track + 1
            } else {
                c.track
            };
        }
    }

    /// Gives a pattern clip its own copy of the pattern (FL's "make unique").
    pub fn make_clip_unique(&mut self, clip: usize) -> Option<usize> {
        let p = self.clips.get(clip)?.pattern?;
        let mut copy = self.patterns.get(p)?.clone();
        let base = copy.name.split(" #").next().unwrap_or(&copy.name).to_string();
        copy.name = (2..).map(|i| format!("{base} #{i}")).find(|n| !self.patterns.iter().any(|q| &q.name == n)).unwrap();
        self.patterns.push(copy);
        let index = self.patterns.len() - 1;
        self.clips[clip].pattern = Some(index);
        self.clips[clip].label = self.patterns[index].name.clone();
        Some(index)
    }

    /// Places a clip of `pattern` on the playlist.
    pub fn add_pattern_clip(&mut self, pattern: usize, track: usize, start: u64) {
        let Some(p) = self.patterns.get(pattern) else { return };
        self.clips.push(ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false,
            track,
            start,
            length: p.length,
            label: p.name.clone(),
            kind: ClipKind::Pattern,
            pattern: Some(pattern),
            channel: None,
            source_offset: 0.0,
        });
    }
}

/// Chord shapes for the piano roll's chord stamp (semitones above the root).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Major7,
    Minor7,
    Dominant7,
    Major9,
    Minor9,
    Power,
}

impl Chord {
    pub const ALL: [Chord; 12] = [Chord::Major, Chord::Minor, Chord::Diminished, Chord::Augmented, Chord::Sus2, Chord::Sus4, Chord::Major7, Chord::Minor7, Chord::Dominant7, Chord::Major9, Chord::Minor9, Chord::Power];

    pub fn label(self) -> &'static str {
        match self {
            Chord::Major => "Major",
            Chord::Minor => "Minor",
            Chord::Diminished => "Diminished",
            Chord::Augmented => "Augmented",
            Chord::Sus2 => "Sus2",
            Chord::Sus4 => "Sus4",
            Chord::Major7 => "Major 7",
            Chord::Minor7 => "Minor 7",
            Chord::Dominant7 => "Dominant 7",
            Chord::Major9 => "Major 9",
            Chord::Minor9 => "Minor 9",
            Chord::Power => "Power (5th)",
        }
    }

    pub fn intervals(self) -> &'static [u8] {
        match self {
            Chord::Major => &[0, 4, 7],
            Chord::Minor => &[0, 3, 7],
            Chord::Diminished => &[0, 3, 6],
            Chord::Augmented => &[0, 4, 8],
            Chord::Sus2 => &[0, 2, 7],
            Chord::Sus4 => &[0, 5, 7],
            Chord::Major7 => &[0, 4, 7, 11],
            Chord::Minor7 => &[0, 3, 7, 10],
            Chord::Dominant7 => &[0, 4, 7, 10],
            Chord::Major9 => &[0, 4, 7, 11, 14],
            Chord::Minor9 => &[0, 3, 7, 10, 14],
            Chord::Power => &[0, 7, 12],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
}

/// Piano-roll strum settings; edits note data before it reaches any instrument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrumOptions {
    pub spread: u64,
    /// -1..=1; positive values accelerate the spacing across a chord.
    pub time_tension: f32,
    pub reverse: bool,
    pub alternate: bool,
    /// Positive values soften successive notes; negative values accent them.
    pub velocity_falloff: i16,
    pub velocity_tension: f32,
    pub preserve_end: bool,
    /// Straddle the chord's original start instead of beginning there.
    pub trigger_ahead: bool,
    /// Notes on one channel starting within this many ticks of each other count as one
    /// chord (live-played chords are never perfectly together).
    pub chord_window: u64,
}

impl Default for StrumOptions {
    fn default() -> Self {
        Self { spread: 6, time_tension: 0.0, reverse: false, alternate: false, velocity_falloff: 0, velocity_tension: 0.0, preserve_end: true, trigger_ahead: false, chord_window: 6 }
    }
}

impl PatternView {
    /// Chords: groups of selected notes on one channel that start together.
    /// Chords allowing a little timing slop: notes on one channel whose starts lie within
    /// `window` ticks of the chord's first note. Each chord's notes are sorted low to high;
    /// the start returned is the earliest.
    fn chords_within(&self, selection: &[usize], window: u64) -> Vec<(u64, Vec<usize>)> {
        let mut sorted: Vec<usize> = selection.iter().copied().filter(|&i| i < self.notes.len()).collect();
        sorted.sort_by_key(|&i| (self.notes[i].channel, self.notes[i].start, self.notes[i].key));
        sorted.dedup();
        let mut out: Vec<(u64, Vec<usize>)> = Vec::new();
        let mut current: Option<(usize, u64, Vec<usize>)> = None;
        for i in sorted {
            let n = &self.notes[i];
            match &mut current {
                Some((ch, first, idx)) if *ch == n.channel && n.start - *first <= window => idx.push(i),
                _ => {
                    if let Some((_, first, idx)) = current.take() {
                        out.push((first, idx));
                    }
                    current = Some((n.channel, n.start, vec![i]));
                }
            }
        }
        if let Some((_, first, idx)) = current {
            out.push((first, idx));
        }
        for (_, idx) in &mut out {
            idx.sort_by_key(|&i| self.notes[i].key);
        }
        out
    }

    fn chords(&self, selection: &[usize]) -> Vec<(u64, Vec<usize>)> {
        let mut groups: std::collections::BTreeMap<(usize, u64), Vec<usize>> = std::collections::BTreeMap::new();
        for &i in selection {
            if let Some(n) = self.notes.get(i) {
                groups.entry((n.channel, n.start)).or_default().push(i);
            }
        }
        groups
            .into_iter()
            .map(|((_, start), mut idx)| {
                idx.sort_by_key(|&i| self.notes[i].key);
                (start, idx)
            })
            .collect()
    }

    /// Splits each selected note into pieces of `piece` ticks. Returns the new notes' indices.
    pub fn chop(&mut self, selection: &[usize], piece: u64) -> Vec<usize> {
        let piece = piece.max(1);
        let mut keep: Vec<NoteView> = Vec::new();
        let mut out: Vec<NoteView> = Vec::new();
        for (i, n) in self.notes.iter().enumerate() {
            if !selection.contains(&i) || n.length <= piece {
                keep.push(*n);
                continue;
            }
            let mut t = 0;
            while t < n.length {
                out.push(NoteView { start: n.start + t, length: piece.min(n.length - t), ..*n });
                t += piece;
            }
        }
        let first = keep.len();
        keep.extend(out);
        self.notes = keep;
        (first..self.notes.len()).collect()
    }

    /// Joins selected notes of the same key and channel that touch end-to-start.
    pub fn glue(&mut self, selection: &[usize]) -> Vec<usize> {
        let mut chosen: Vec<NoteView> = selection.iter().filter_map(|&i| self.notes.get(i).copied()).collect();
        chosen.sort_by_key(|n| (n.channel, n.key, n.start));
        let mut merged: Vec<NoteView> = Vec::new();
        for n in chosen {
            match merged.last_mut() {
                Some(m) if m.channel == n.channel && m.key == n.key && m.start + m.length >= n.start => {
                    m.length = (n.start + n.length).max(m.start + m.length) - m.start;
                }
                _ => merged.push(n),
            }
        }
        let mut rest: Vec<NoteView> = self.notes.iter().enumerate().filter(|(i, _)| !selection.contains(i)).map(|(_, n)| *n).collect();
        let first = rest.len();
        rest.extend(merged);
        self.notes = rest;
        (first..self.notes.len()).collect()
    }

    /// Moves note starts (and, with `ends`, note ends) toward the `grid` by `strength`
    /// (1.0 = fully onto the grid, 0.5 = halfway).
    pub fn quantize(&mut self, selection: &[usize], grid: u64, strength: f32, ends: bool) {
        let grid = grid.max(1) as f64;
        let k = strength.clamp(0.0, 1.0) as f64;
        for &i in selection {
            let Some(n) = self.notes.get_mut(i) else { continue };
            let start = n.start as f64;
            let end = start + n.length as f64;
            let qs = start + ((start / grid).round() * grid - start) * k;
            let qe = if ends { end + ((end / grid).round() * grid - end) * k } else { qs + n.length as f64 };
            n.start = qs.round().max(0.0) as u64;
            n.length = (qe - qs).round().max(1.0) as u64;
        }
    }

    /// Nudges timing by up to ±`timing` ticks and velocity by up to ±`velocity` (0..=128
    /// scale), pseudo-randomly but repeatably for a given `seed`.
    pub fn humanize(&mut self, selection: &[usize], timing: u64, velocity: u8, seed: u64) {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        for &i in selection {
            let Some(n) = self.notes.get_mut(i) else { continue };
            let dt = (next() * timing as f64).round() as i64;
            let dv = (next() * velocity as f64).round() as i32;
            n.start = (n.start as i64 + dt).max(0) as u64;
            n.velocity = (n.velocity as i32 + dv).clamp(1, 128) as u8;
        }
    }

    /// Moves notes outside the scale (root key class + intervals) to the nearest scale note
    /// (ties go down).
    pub fn snap_to_scale(&mut self, selection: &[usize], root: u8, intervals: &[u8]) {
        let in_scale = |k: i32| intervals.contains(&(((k - root as i32).rem_euclid(12)) as u8));
        for &i in selection {
            let Some(n) = self.notes.get_mut(i) else { continue };
            let k = n.key as i32;
            if in_scale(k) {
                continue;
            }
            for d in 1..12 {
                if in_scale(k - d) && k - d >= 0 {
                    n.key = (k - d) as u8;
                    break;
                }
                if in_scale(k + d) && k + d <= 127 {
                    n.key = (k + d) as u8;
                    break;
                }
            }
        }
    }

    /// Gives selected notes a repeatable performance contour without discarding their
    /// existing velocity differences. Bass and melody voices lead a chord, while
    /// small note-specific variation prevents every key from sounding identical.
    pub fn piano_feel(&mut self, selection: &[usize], beat_ticks: u64, bar_ticks: u64, amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        let beat = beat_ticks.max(1);
        let bar = bar_ticks.max(beat);
        // Chords within a 64th note of each other share voicing (bass, inner, melody).
        for (_, indices) in self.chords_within(selection, (beat / 16).max(1)) {
            let count = indices.len();
            for (rank, i) in indices.into_iter().enumerate() {
                let n = &mut self.notes[i];
                let mut hash = n.start ^ ((n.key as u64) << 32) ^ (n.channel as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                hash ^= hash >> 33;
                hash = hash.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
                hash ^= hash >> 33;
                hash = hash.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
                hash ^= hash >> 33;
                let variation = (hash % 2001) as f32 / 1000.0 - 1.0;
                let pulse = if n.start % bar == 0 { 0.045 } else if n.start % beat == 0 { 0.015 } else { -0.015 };
                let voice = if count == 1 { 0.0 } else if rank == count - 1 { 0.06 } else if rank == 0 { 0.035 } else { -0.055 };
                let factor = 1.0 + amount * (pulse + voice + variation * 0.055);
                n.velocity = (n.velocity as f32 * factor).round().clamp(1.0, 128.0) as u8;
            }
        }
    }

    /// Mirrors the selection in time (last note first) within its own span.
    pub fn flip_time(&mut self, selection: &[usize]) {
        let sel: Vec<usize> = selection.iter().copied().filter(|&i| i < self.notes.len()).collect();
        let (Some(lo), Some(hi)) = (sel.iter().map(|&i| self.notes[i].start).min(), sel.iter().map(|&i| self.notes[i].start + self.notes[i].length).max()) else { return };
        for &i in &sel {
            let n = &mut self.notes[i];
            n.start = lo + (hi - (n.start + n.length));
        }
    }

    /// Mirrors the selection in pitch around the middle of its range (melodies turn upside down).
    pub fn flip_pitch(&mut self, selection: &[usize]) {
        let sel: Vec<usize> = selection.iter().copied().filter(|&i| i < self.notes.len()).collect();
        let (Some(lo), Some(hi)) = (sel.iter().map(|&i| self.notes[i].key).min(), sel.iter().map(|&i| self.notes[i].key).max()) else { return };
        for &i in &sel {
            let n = &mut self.notes[i];
            n.key = hi - (n.key - lo);
        }
    }

    /// Randomizes pitch within ±`keys` (snapped to `scale` intervals from `root` if given) and
    /// velocity within ±`velocity`, repeatably for `seed`.
    pub fn randomize(&mut self, selection: &[usize], keys: u8, velocity: u8, scale: Option<(u8, &[u8])>, seed: u64) {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        for &i in selection {
            let Some(n) = self.notes.get_mut(i) else { continue };
            let dk = (next() * keys as f64).round() as i32;
            let dv = (next() * velocity as f64).round() as i32;
            let mut key = (n.key as i32 + dk).clamp(0, 127);
            if let Some((root, intervals)) = scale {
                let in_scale = |k: i32| intervals.contains(&(((k - root as i32).rem_euclid(12)) as u8));
                if let Some(k) = (0..12).flat_map(|d| [key - d, key + d]).find(|&k| (0..=127).contains(&k) && in_scale(k)) {
                    key = k;
                }
            }
            n.key = key as u8;
            n.velocity = (n.velocity as i32 + dv).clamp(1, 128) as u8;
        }
    }

    /// Shapes a note property with an LFO over time (FL's piano-roll LFO tool): `lane` 0 =
    /// velocity, 1 = pan, 2 = Mod X, 3 = Mod Y; `period` in ticks; `depth` 0..1 around `centre`
    /// (0..1); `shape` 0 = sine, 1 = triangle, 2 = saw, 3 = square.
    #[allow(clippy::too_many_arguments)]
    pub fn lfo(&mut self, selection: &[usize], lane: u8, period: u64, depth: f32, centre: f32, shape: u8, phase: f32) {
        let period = period.max(1) as f64;
        for &i in selection {
            let Some(n) = self.notes.get_mut(i) else { continue };
            let t = (n.start as f64 / period + phase as f64).fract() as f32;
            let w = match shape {
                1 => 1.0 - 4.0 * (t - 0.5).abs(),
                2 => 2.0 * t - 1.0,
                3 => if t < 0.5 { 1.0 } else { -1.0 },
                _ => (t * std::f32::consts::TAU).sin(),
            };
            let v = (centre + w * depth * 0.5).clamp(0.0, 1.0);
            match lane {
                1 => n.pan = (v * 128.0).round() as u8,
                2 => n.mod_x = (v * 255.0).round() as u8,
                3 => n.mod_y = (v * 255.0).round() as u8,
                _ => n.velocity = (v * 128.0).round().max(1.0) as u8,
            }
        }
    }

    /// Staggers the notes of each selected chord by `spread` ticks, lowest first.
    pub fn strum(&mut self, selection: &[usize], spread: u64) {
        self.strum_with(selection, StrumOptions { spread, ..Default::default() });
    }

    /// Strums each chord with curved timing, optional trigger-ahead, alternate
    /// strokes, velocity shaping and optional fixed note ends. Single notes stay untouched.
    pub fn strum_with(&mut self, selection: &[usize], options: StrumOptions) {
        let curve = |fraction: f64, tension: f32| fraction.powf(2.0_f64.powf(tension.clamp(-1.0, 1.0) as f64 * 2.0));
        for (chord_index, (chord_start, mut idx)) in self.chords_within(selection, options.chord_window).into_iter().enumerate() {
            if idx.len() < 2 {
                continue;
            }
            if options.reverse ^ (options.alternate && chord_index % 2 == 1) {
                idx.reverse();
            }
            let total = options.spread.saturating_mul((idx.len() - 1) as u64);
            // Strummed from the chord's start (a slightly ragged chord is evened out first).
            let mut base = chord_start as i128 - if options.trigger_ahead { (total / 2) as i128 } else { 0 };
            // Straddling at the very start can't go before tick 0: shift, don't squash.
            base = base.max(0);
            for (k, &i) in idx.iter().enumerate() {
                let n = &mut self.notes[i];
                let fraction = k as f64 / (idx.len() - 1) as f64;
                let offset = (total as f64 * curve(fraction, options.time_tension)).round() as i128;
                let old_end = n.start.saturating_add(n.length);
                n.start = (base + offset).clamp(0, u64::MAX as i128) as u64;
                if options.preserve_end {
                    n.length = old_end.saturating_sub(n.start).max(1);
                }
                let velocity_offset = (options.velocity_falloff as f64 * (idx.len() - 1) as f64 * curve(fraction, options.velocity_tension)).round() as i32;
                n.velocity = (n.velocity as i32 - velocity_offset).clamp(1, 128) as u8;
            }
        }
    }

    /// Replaces each selected chord with an arpeggio of `step`-long notes over its length.
    pub fn arpeggiate(&mut self, selection: &[usize], step: u64, mode: ArpMode) -> Vec<usize> {
        let step = step.max(1);
        let chords = self.chords(selection);
        let mut out = Vec::new();
        for (start, idx) in &chords {
            let mut keys: Vec<NoteView> = idx.iter().map(|&i| self.notes[i]).collect();
            if keys.len() < 2 {
                out.extend(keys);
                continue;
            }
            let span = keys.iter().map(|n| n.length).max().unwrap_or(step);
            let order: Vec<usize> = match mode {
                ArpMode::Up => (0..keys.len()).collect(),
                ArpMode::Down => (0..keys.len()).rev().collect(),
                ArpMode::UpDown => (0..keys.len()).chain((1..keys.len() - 1).rev()).collect(),
            };
            keys.sort_by_key(|n| n.key);
            let mut t = 0;
            let mut k = 0;
            while t < span {
                let base = keys[order[k % order.len()]];
                out.push(NoteView { start: start + t, length: step.min(span - t), ..base });
                t += step;
                k += 1;
            }
        }
        let removed: std::collections::HashSet<usize> = chords.iter().flat_map(|(_, i)| i.iter().copied()).collect();
        let mut rest: Vec<NoteView> = self.notes.iter().enumerate().filter(|(i, _)| !removed.contains(i)).map(|(_, n)| *n).collect();
        let first = rest.len();
        rest.extend(out);
        self.notes = rest;
        (first..self.notes.len()).collect()
    }

    /// Grows the pattern to whole bars covering its notes (never below one bar).
    pub fn fit_length(&mut self, ticks_per_bar: u64) {
        let end = self.notes.iter().map(|n| n.start + n.length).max().unwrap_or(0);
        self.length = self.length.max(end.div_ceil(ticks_per_bar).max(1) * ticks_per_bar);
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TrackView {
    pub name: String,
    pub muted: bool,
    /// Colour chosen by the user (kept when renamed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// A folder track: groups the `grouped` tracks below it (it can hold clips too).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folder: bool,
    /// A folder whose tracks are hidden.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Belongs to the nearest folder above it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grouped: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Pattern,
    Audio,
    Automation,
}

/// What an automation clip controls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTarget {
    ChannelVolume { channel: usize },
    ChannelPan { channel: usize },
    InsertVolume { insert: usize },
    InsertPan { insert: usize },
    SynthCutoff { channel: usize },
    SynthResonance { channel: usize },
    /// A parameter of the plugin playing on a channel.
    ChannelPluginParam { channel: usize, param: u32 },
    /// A parameter of a plugin effect in a mixer slot.
    EffectParam { insert: usize, slot: usize, param: u32 },
    /// A control of an insert's built-in EQ/compressor (see [`crate::channel_strip::StripSettings::spec`]).
    InsertStrip { insert: usize, param: u32 },
}

impl AutomationTarget {
    pub fn label(&self, view: &ProjectView) -> String {
        let ch = |c: usize| view.channels.get(c).map(|c| c.name.clone()).unwrap_or_default();
        let ins = |i: usize| view.inserts.get(i).map(|i| i.name.clone()).unwrap_or_default();
        match *self {
            AutomationTarget::ChannelVolume { channel } => format!("{} volume", ch(channel)),
            AutomationTarget::ChannelPan { channel } => format!("{} pan", ch(channel)),
            AutomationTarget::InsertVolume { insert } => format!("{} fader", ins(insert)),
            AutomationTarget::InsertPan { insert } => format!("{} mixer pan", ins(insert)),
            AutomationTarget::SynthCutoff { channel } => format!("{} cutoff", ch(channel)),
            AutomationTarget::SynthResonance { channel } => format!("{} resonance", ch(channel)),
            AutomationTarget::ChannelPluginParam { channel, param } => format!("{} parameter {}", ch(channel), param + 1),
            AutomationTarget::EffectParam { insert, slot, param } => {
                let fx = view.inserts.get(insert).and_then(|i| i.effects.get(slot)).map(|e| e.name.clone()).unwrap_or_default();
                format!("{fx} parameter {} ({})", param + 1, ins(insert))
            }
            AutomationTarget::InsertStrip { insert, param } => match crate::channel_strip::PARAMS.get(param as usize) {
                Some((section, spec)) => format!("{} {section} {}", ins(insert), spec.name.to_lowercase()),
                None => format!("{} strip parameter {}", ins(insert), param + 1),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoPoint {
    pub tick: u64,
    /// 0.0..=1.0 on the target's control (knob/fader position).
    pub value: f32,
    /// Bend of the segment that ends at this point: 0 = straight, toward ±1 = a curve that
    /// changes late (positive) or early (negative), like FL's curve tension.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub tension: f32,
}

fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

/// Shapes 0..=1 progress through a segment with `tension` (-1..=1; 0 = linear).
pub fn curve_shape(t: f32, tension: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let k = tension.clamp(-1.0, 1.0);
    if k.abs() < 1e-4 {
        t
    } else if k > 0.0 {
        t.powf(1.0 + k * 4.0)
    } else {
        1.0 - (1.0 - t).powf(1.0 - k * 4.0)
    }
}

/// An automation clip's curve.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutomationView {
    pub name: String,
    pub target: AutomationTarget,
    /// Sorted by tick.
    pub points: Vec<AutoPoint>,
}

impl AutomationView {
    /// The value at `tick` (linear between points, held outside them).
    pub fn value_at(&self, tick: f64) -> f32 {
        let Some(first) = self.points.first() else { return 0.5 };
        if tick <= first.tick as f64 {
            return first.value;
        }
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if tick <= b.tick as f64 {
                let span = (b.tick - a.tick).max(1) as f64;
                let t = curve_shape(((tick - a.tick as f64) / span) as f32, b.tension);
                return a.value + (b.value - a.value) * t;
            }
        }
        self.points.last().unwrap().value
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ClipView {
    pub track: usize,
    pub start: u64,
    pub length: u64,
    pub label: String,
    pub kind: ClipKind,
    /// Index into [`ProjectView::patterns`] for pattern clips.
    pub pattern: Option<usize>,
    /// Engine channel index for audio clips.
    pub channel: Option<usize>,
    /// Where the clip starts inside its source: ticks for patterns, seconds for audio.
    pub source_offset: f64,
    /// Index into [`ProjectView::automations`] for automation clips.
    #[serde(default)]
    pub automation: Option<usize>,
    /// Colour chosen by the user (otherwise derived from the name).
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Audio clips: level (linear), fade-in/out lengths in ticks, and reversed playback.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub gain: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_in: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_out: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reverse: bool,
}

fn unity() -> f32 {
    1.0
}

fn is_unity(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// The tempo from `tick` onwards (until the next change).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TempoChange {
    pub tick: u64,
    pub bpm: f64,
}

/// A MIDI controller (CC) driving a control.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ControllerLink {
    /// MIDI channel 0..=15 and controller number 0..=127.
    pub midi_channel: u8,
    pub cc: u8,
    pub target: AutomationTarget,
}

/// A time-signature change at the start of bar `bar` (0-based): `beats_per_bar` / 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterChange {
    pub bar: u64,
    pub beats_per_bar: u32,
}

/// A named position on the playlist ruler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub tick: u64,
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PatternView {
    pub name: String,
    /// Colour chosen by the user (clips of this pattern use it).
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    pub notes: Vec<NoteView>,
    /// Length in ticks (end of the last note, rounded up to a bar).
    pub length: u64,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct NoteView {
    pub start: u64,
    pub length: u64,
    pub key: u8,
    pub velocity: u8,
    /// Engine channel index.
    pub channel: usize,
    /// Per-note pan, 0..=128 (64 = centre), as in FL Studio.
    #[serde(default = "centre_pan")]
    pub pan: u8,
    /// Per-note modulation X/Y, 0..=255 (128 = neutral). On KVNG Synth X moves the
    /// filter cutoff and Y the resonance.
    #[serde(default = "centre_mod")]
    pub mod_x: u8,
    #[serde(default = "centre_mod")]
    pub mod_y: u8,
    /// A slide note: instead of starting a new note it glides the channel's playing notes
    /// to its pitch over its length (FL's slide notes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slide: bool,
}

fn centre_pan() -> u8 {
    64
}

fn centre_mod() -> u8 {
    128
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Ok,
    /// Loaded, but something differs from FL (e.g. a replacement sample).
    Substituted,
    /// Can't be played.
    Missing,
    /// Not used by the arrangement, so not loaded.
    Unused,
}

#[derive(Clone)]
pub struct ChannelView {
    pub name: String,
    /// "Sampler", "Audio clip", "Plugin", "FL plugin", ...
    pub kind: String,
    /// Plugin name and where it runs, or the sample's file name.
    pub detail: String,
    pub status: ChannelStatus,
    pub insert: usize,
    /// Key used for new step-sequencer hits (FL default: C5 = 60).
    pub root_note: u8,
    pub editor: Option<EditorHandle>,
    /// Peak envelope of the channel's sample (one value per [`WAVEFORM_STEP`] samples).
    pub waveform: Option<Arc<Vec<f32>>>,
    /// Removed by the user; hidden, and dropped when the project is saved.
    pub deleted: bool,
    /// User-chosen colour.
    pub color: Option<[u8; 3]>,
}

/// Samples per waveform-overview point.
pub const WAVEFORM_STEP: usize = 256;

#[derive(Clone)]
pub struct InsertView {
    pub index: usize,
    pub name: String,
    pub effects: Vec<EffectView>,
    pub routes_to: Vec<usize>,
    /// Send level (linear) per entry of `routes_to`.
    pub route_levels: Vec<f32>,
    /// Inserts whose compressor this insert keys.
    pub sidechain_to: Vec<usize>,
    /// User-chosen colour.
    pub color: Option<[u8; 3]>,
    /// Something in the project feeds this insert.
    pub used: bool,
}

#[derive(Clone)]
pub struct EffectView {
    pub name: String,
    pub bypass: bool,
    /// 0.0 = dry, 1.0 = fully wet.
    pub mix: f32,
    /// "Plugin (helper arm64)", "KVNG adapter", ...
    pub detail: String,
    pub editor: Option<EditorHandle>,
}

/// Peak envelope of a stereo sample for drawing.
pub fn waveform(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.chunks(WAVEFORM_STEP)
        .zip(right.chunks(WAVEFORM_STEP))
        .map(|(l, r)| l.iter().chain(r).fold(0.0f32, |m, s| m.max(s.abs())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u64, length: u64, key: u8) -> NoteView {
        NoteView { start, length, key, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false }
    }

    #[test]
    fn quantize_strength_humanize_and_scale_snap() {
        let mut p = PatternView { color: None, name: String::new(), notes: vec![note(10, 30, 61), note(50, 20, 64)], length: 384 };
        p.quantize(&[0, 1], 24, 0.5, false);
        assert_eq!((p.notes[0].start, p.notes[0].length), (5, 30), "halfway to 0");
        assert_eq!(p.notes[1].start, 49, "halfway to 48");
        p.quantize(&[0, 1], 24, 1.0, true);
        assert_eq!((p.notes[0].start, p.notes[0].start + p.notes[0].length), (0, 24));

        let before: Vec<_> = p.notes.clone();
        p.humanize(&[0, 1], 6, 10, 42);
        for (a, b) in p.notes.iter().zip(&before) {
            assert!((a.start as i64 - b.start as i64).abs() <= 6 && (a.velocity as i32 - b.velocity as i32).abs() <= 10);
        }
        let mut again = PatternView { notes: before, ..p.clone() };
        again.humanize(&[0, 1], 6, 10, 42);
        assert_eq!(again.notes.iter().map(|n| (n.start, n.velocity)).collect::<Vec<_>>(), p.notes.iter().map(|n| (n.start, n.velocity)).collect::<Vec<_>>(), "repeatable");

        // C major: C# (61) snaps down to C (60); E (64) stays.
        p.snap_to_scale(&[0, 1], 0, &[0, 2, 4, 5, 7, 9, 11]);
        assert_eq!((p.notes[0].key, p.notes[1].key), (60, 64));
        assert_eq!(Chord::Minor7.intervals(), &[0, 3, 7, 10]);
    }

    fn arrangement() -> ProjectView {
        let clip = |track: usize, start: u64, length: u64| ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track, start, length, label: String::new(), kind: ClipKind::Pattern, pattern: Some(0), channel: None, source_offset: 0.0 };
        ProjectView {
            automations: vec![],
            swing: 0.0,
            markers: vec![Marker { tick: 400, name: "B".into() }],
            tempo_changes: vec![],
            meter_changes: vec![],
            controller_links: vec![],
            name: String::new(),
            tempo: 120.0,
            ppq: 96,
            beats_per_bar: 4,
            sample_rate: 48_000.0,
            samples_per_tick: 250.0,
            length_ticks: 0,
            tracks: (0..3).map(|i| TrackView { name: format!("T{i}"), muted: false, color: None, folder: false, collapsed: false, grouped: false }).collect(),
            clips: vec![clip(0, 0, 384), clip(1, 384, 384), clip(2, 200, 400)],
            patterns: vec![],
            channels: vec![],
            inserts: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn inserting_and_deleting_bars_moves_meter_changes() {
        let mut v = arrangement();
        v.meter_changes = vec![MeterChange { bar: 2, beats_per_bar: 3 }];
        v.insert_time(v.bar_start(1), v.bar_at(v.bar_start(1)).2);
        assert_eq!(v.meter_changes, vec![MeterChange { bar: 3, beats_per_bar: 3 }]);
        v.delete_time(v.bar_start(0), v.bar_start(2));
        assert_eq!(v.meter_changes, vec![MeterChange { bar: 1, beats_per_bar: 3 }]);
    }

    #[test]
    fn bars_follow_time_signature_changes() {
        let mut v = arrangement();
        // 4/4 for bars 1-2, 3/4 from bar 3, 7/4 from bar 5 (0-based bars 2 and 4).
        v.meter_changes = vec![MeterChange { bar: 2, beats_per_bar: 3 }, MeterChange { bar: 4, beats_per_bar: 7 }];
        let q = 96;
        assert_eq!(v.bar_start(0), 0);
        assert_eq!(v.bar_start(2), 8 * q);
        assert_eq!(v.bar_start(3), 11 * q);
        assert_eq!(v.bar_start(4), 14 * q);
        assert_eq!(v.bar_start(5), 21 * q);
        assert_eq!(v.bar_at(0), (0, 0, 4 * q));
        assert_eq!(v.bar_at(9 * q), (2, 8 * q, 3 * q));
        assert_eq!(v.bar_at(15 * q), (4, 14 * q, 7 * q));
        assert_eq!(v.bar_at(22 * q), (5, 21 * q, 7 * q));
        assert_eq!(v.beats_in_bar(3), 3);
        for bar in 0..8 {
            assert_eq!(v.bar_at(v.bar_start(bar)).0, bar, "round trip bar {bar}");
        }
    }

    #[test]
    fn folders_hide_and_mute_their_tracks() {
        let mut v = arrangement();
        v.tracks[0].folder = true;
        v.tracks[1].grouped = true;
        v.tracks[2].grouped = false;
        assert_eq!(v.track_folders(), vec![None, Some(0), None]);
        v.tracks[0].collapsed = true;
        assert_eq!(v.hidden_tracks(), vec![false, true, false]);
        v.tracks[0].muted = true;
        assert_eq!(v.effective_mutes(), vec![true, true, false]);
    }

    #[test]
    fn inserting_and_deleting_time_moves_later_material() {
        let mut v = arrangement();
        v.insert_time(384, 96);
        assert_eq!(v.clips[1].start, 480, "clip after the insert point moves");
        assert_eq!(v.markers[0].tick, 496);
        // The clip spanning 384 was split; its tail moved.
        let tail = v.clips.iter().find(|c| c.track == 2 && c.start > 200).unwrap();
        assert_eq!((tail.start, tail.length, tail.source_offset), (480, 216, 184.0));
        let mut v = arrangement();
        v.delete_time(384, 768);
        assert!(v.clips.iter().all(|c| c.start < 384), "everything in the range is gone");
        assert!(v.clips.iter().all(|c| c.start + c.length <= 384));
        assert!(v.markers.is_empty());
    }

    #[test]
    fn duplicating_time_repeats_it_and_track_ops_keep_clips_attached() {
        let mut v = arrangement();
        v.duplicate_time(0, 384);
        assert_eq!(v.clips.iter().filter(|c| c.track == 0).map(|c| c.start).collect::<Vec<_>>(), vec![0, 384]);
        assert_eq!(v.clips.iter().find(|c| c.track == 1).unwrap().start, 768, "later clip moved after the copy");

        let mut v = arrangement();
        v.move_track(0, 2);
        assert_eq!(v.tracks[2].name, "T0");
        assert_eq!(v.clips[0].track, 2);
        assert_eq!(v.clips[1].track, 0);
        v.insert_track(0);
        assert_eq!(v.clips[1].track, 1);
        v.delete_track(1);
        assert!(v.clips.iter().all(|c| c.track != 1 || c.start == 200));
        assert_eq!(v.tracks.len(), 3);
    }

    #[test]
    fn curve_tension_bends_but_keeps_the_ends() {
        for k in [-1.0, -0.4, 0.0, 0.5, 1.0] {
            assert_eq!(curve_shape(0.0, k), 0.0);
            assert!((curve_shape(1.0, k) - 1.0).abs() < 1e-6);
        }
        assert_eq!(curve_shape(0.5, 0.0), 0.5);
        assert!(curve_shape(0.5, 0.8) < 0.2, "late curve");
        assert!(curve_shape(0.5, -0.8) > 0.8, "early curve");
        let auto = AutomationView { name: String::new(), target: AutomationTarget::ChannelPan { channel: 0 }, points: vec![AutoPoint { tick: 0, value: 0.0, tension: 0.0 }, AutoPoint { tick: 100, value: 1.0, tension: 0.6 }] };
        assert!(auto.value_at(50.0) < 0.5);
    }

    fn view() -> ProjectView {
        ProjectView {
            automations: vec![],
            swing: 0.0,
            markers: vec![],
            tempo_changes: vec![],
            meter_changes: vec![],
            controller_links: vec![],
            name: "t".into(),
            tempo: 120.0,
            ppq: 96,
            beats_per_bar: 4,
            sample_rate: 48_000.0,
            samples_per_tick: 250.0,
            length_ticks: 384,
            tracks: vec![],
            clips: vec![],
            patterns: vec![PatternView { color: None, name: "p".into(), notes: vec![], length: 384 }],
            channels: vec![ChannelView {
                name: "kick".into(),
                kind: "Sampler".into(),
                detail: String::new(),
                status: ChannelStatus::Ok,
                insert: 1,
                root_note: 60,
                editor: None,
                waveform: None,
                deleted: false,
                color: None,
            }],
            inserts: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn steps_toggle_on_and_off() {
        let mut v = view();
        assert!(v.toggle_step(0, 0, 4));
        assert!(v.step_on(0, 0, 4));
        let n = v.patterns[0].notes[0];
        assert_eq!((n.start, n.length, n.key), (96, 24, 60));
        assert!(!v.toggle_step(0, 0, 4));
        assert!(!v.step_on(0, 0, 4));
    }

    #[test]
    fn pattern_grows_to_fit_notes() {
        let mut v = view();
        v.toggle_step(0, 0, 20); // step 20 is in bar 2
        assert_eq!(v.patterns[0].length, 768);
    }

    #[test]
    fn pattern_clone_and_delete_keep_clips_consistent() {
        let mut v = view();
        v.add_pattern_clip(0, 0, 0);
        let second = v.new_pattern();
        v.add_pattern_clip(second, 1, 384);
        let copy = v.clone_pattern(0).unwrap();
        assert_eq!(copy, 1);
        assert_eq!(v.patterns[1].name, "p #2");
        assert_eq!(v.clips[1].pattern, Some(2), "clip of the later pattern shifted");
        v.delete_pattern(0);
        assert_eq!(v.clips.len(), 1);
        assert_eq!(v.clips[0].pattern, Some(1));
        v.rename_pattern(1, "Verse");
        assert_eq!(v.clips[0].label, "Verse");
    }

    #[test]
    fn swing_moves_only_offbeat_sixteenths() {
        let mut v = view();
        v.swing = 1.0;
        assert_eq!(v.swung(0), 0.0);
        assert_eq!(v.swung(24), 24.0 + 16.0, "second sixteenth moves by 2/3 of a step");
        assert_eq!(v.swung(48), 48.0);
        v.swing = 0.0;
        assert_eq!(v.swung(24), 24.0);
    }

    fn chord() -> PatternView {
        let n = |key| NoteView { start: 0, length: 96, key, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false };
        PatternView { color: None, name: "c".into(), notes: vec![n(60), n(64), n(67)], length: 384 }
    }

    #[test]
    fn chop_glue_strum_arpeggiate() {
        let mut p = chord();
        let sel = p.chop(&[0], 24);
        assert_eq!(sel.len(), 4);
        assert!(p.notes.iter().filter(|n| n.key == 60).all(|n| n.length == 24));
        let glued = p.glue(&sel);
        assert_eq!(glued.len(), 1);
        assert_eq!(p.notes[glued[0]].length, 96);

        let mut p = chord();
        p.strum(&[0, 1, 2], 10);
        let mut starts: Vec<(u8, u64)> = p.notes.iter().map(|n| (n.key, n.start)).collect();
        starts.sort();
        assert_eq!(starts, vec![(60, 0), (64, 10), (67, 20)]);

        let mut p = chord();
        let arp = p.arpeggiate(&[0, 1, 2], 24, ArpMode::Up);
        let keys: Vec<u8> = arp.iter().map(|&i| p.notes[i].key).collect();
        assert_eq!(keys, vec![60, 64, 67, 60]);
    }

    #[test]
    fn piano_feel_varies_keys_but_preserves_authored_dynamics() {
        let mut p = chord();
        p.notes.push(NoteView { start: 96, key: 60, velocity: 70, ..p.notes[0] });
        let original = p.notes.clone();
        p.piano_feel(&[0, 1, 2, 3], 96, 384, 0.0);
        assert_eq!(p.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), original.iter().map(|n| n.velocity).collect::<Vec<_>>());
        p.piano_feel(&[0, 1, 2, 3], 96, 384, 1.0);
        assert_ne!(p.notes[0].velocity, p.notes[1].velocity);
        assert_ne!(p.notes[1].velocity, p.notes[2].velocity);
        assert!(p.notes[3].velocity < p.notes[0].velocity);
        let again = {
            let mut copy = PatternView { notes: original, ..chord() };
            copy.piano_feel(&[0, 1, 2, 3], 96, 384, 1.0);
            copy
        };
        assert_eq!(p.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), again.notes.iter().map(|n| n.velocity).collect::<Vec<_>>());
    }

    #[test]
    fn configurable_strum_alternates_direction_and_tapers_velocity() {
        let mut p = chord();
        let second: Vec<NoteView> = p.notes.iter().map(|n| NoteView { start: 96, ..*n }).collect();
        p.notes.extend(second);
        p.strum_with(&[0, 1, 2, 3, 4, 5], StrumOptions {
            spread: 5, reverse: false, alternate: true, velocity_falloff: 10, preserve_end: false, ..Default::default()
        });
        assert_eq!(p.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 5, 10, 106, 101, 96]);
        assert_eq!(p.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![100, 90, 80, 80, 90, 100]);
        assert!(p.notes.iter().all(|n| n.length == 96));
    }

    #[test]
    fn flip_randomize_and_lfo_tools() {
        let mut p = PatternView { color: None, name: String::new(), notes: vec![note(0, 24, 60), note(48, 48, 64), note(96, 24, 67)], length: 384 };
        p.flip_time(&[0, 1, 2]);
        assert_eq!(p.notes.iter().map(|n| (n.start, n.key)).collect::<Vec<_>>(), vec![(96, 60), (24, 64), (0, 67)]);
        p.flip_pitch(&[0, 1, 2]);
        assert_eq!(p.notes.iter().map(|n| n.key).collect::<Vec<_>>(), vec![67, 63, 60]);
        // Randomize within a C major scale lands on scale notes only.
        let major: &[u8] = &[0, 2, 4, 5, 7, 9, 11];
        p.randomize(&[0, 1, 2], 5, 10, Some((0, major)), 7);
        assert!(p.notes.iter().all(|n| major.contains(&(n.key % 12))));
        // Square LFO over a 96-tick period alternates velocity high/low.
        let mut q = PatternView { color: None, name: String::new(), notes: (0..4).map(|k| note(k * 48, 24, 60)).collect(), length: 384 };
        q.lfo(&[0, 1, 2, 3], 0, 96, 1.0, 0.5, 3, 0.0);
        assert_eq!(q.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![128, 1, 128, 1]);
    }

    #[test]
    fn strum_handles_ragged_chords_and_the_pattern_start() {
        // A live-played chord: starts 0, 3, 5.
        let mut p = chord();
        p.notes[1].start = 3;
        p.notes[2].start = 5;
        p.strum_with(&[0, 1, 2], StrumOptions { spread: 10, preserve_end: false, ..Default::default() });
        assert_eq!(p.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 10, 20]);
        // Straddling a chord at tick 0 keeps its spacing instead of piling notes up at 0.
        let mut p = chord();
        p.strum_with(&[0, 1, 2], StrumOptions { spread: 10, trigger_ahead: true, ..Default::default() });
        assert_eq!(p.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 10, 20]);
        // Notes farther apart than the window are separate (single notes stay put).
        let mut p = chord();
        p.notes[2].start = 48;
        p.strum_with(&[0, 1, 2], StrumOptions { spread: 10, ..Default::default() });
        assert_eq!(p.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 10, 48]);
    }

    #[test]
    fn strum_curves_and_trigger_ahead_preserve_original_ends() {
        let mut p = chord();
        for n in &mut p.notes {
            n.start = 48;
        }
        p.strum_with(&[0, 1, 2], StrumOptions {
            spread: 12, time_tension: 1.0, velocity_falloff: -10,
            velocity_tension: 1.0, trigger_ahead: true, ..Default::default()
        });
        // Positive tension accelerates the stroke: the second gap is larger.
        assert_eq!(p.notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![36, 38, 60]);
        assert_eq!(p.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![100, 101, 120]);
        assert!(p.notes.iter().all(|n| n.start + n.length == 144));
    }

    #[test]
    fn tempo_map_converts_both_ways() {
        let mut v = view();
        // 120 BPM, 96 PPQ, 48 kHz → 250 samples per tick; double tempo from bar 2.
        v.tempo_changes.push(TempoChange { tick: 384, bpm: 240.0 });
        assert_eq!(v.tick_to_sample(384.0), 96_000);
        assert_eq!(v.tick_to_sample(768.0), 96_000 + 48_000);
        assert!((v.sample_to_tick(120_000) - 576.0).abs() < 1e-6);
        assert_eq!(v.tempo_at(500), 240.0);
        assert_eq!(v.tempo_at(100), 120.0);
    }

    #[test]
    fn note_edits_are_clamped() {
        let mut v = view();
        let i = v.add_note(0, NoteView { start: 0, length: 96, key: 60, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false }).unwrap();
        v.set_note(0, i, -50, 200, 1);
        let n = v.patterns[0].notes[i];
        assert_eq!((n.start, n.key, n.length), (0, 127, 6));
    }
}
