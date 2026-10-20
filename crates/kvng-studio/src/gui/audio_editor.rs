//! The audio editor: a waveform view of a channel's sample with selection, clipboard,
//! and destructive edits (crop, fades, normalize, ...). Edits stay in the editor until
//! applied; applying writes a new WAV next to the project and points the channel at it.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, pos2, vec2};
use engine::edit;
use engine::sample::Sample;

use super::session::Session;
use super::theme;

const WAVE_H: f32 = 220.0;
const RULER_H: f32 = 18.0;
const MAX_UNDO: usize = 30;

/// What the cached slice points were computed for: (version, equal, sensitivity, count).
type SliceKey = (u64, bool, u32, usize);

pub struct AudioEditor {
    pub channel: usize,
    sample: Sample,
    sample_rate: f64,
    undo: Vec<Sample>,
    redo: Vec<Sample>,
    /// Selected frames (anchor, current); ordered with [`AudioEditor::selection`].
    sel: Option<(usize, usize)>,
    cursor: usize,
    /// First visible frame and frames per pixel (0 = fit on next draw).
    view_start: f64,
    frames_per_px: f64,
    clipboard: Option<Sample>,
    dragging: bool,
    gain_db: f32,
    normalize_db: f32,
    /// Time-stretch (percent of the current length), pitch shift (semitones), fit-to-bars.
    stretch_pct: f64,
    pitch_st: f64,
    fit_bars: u32,
    /// Slicing: by transients (sensitivity) or into equal parts; cut points cached per version.
    slice_equal: bool,
    slice_sensitivity: f32,
    slice_count: usize,
    slice_cache: Option<(SliceKey, Vec<usize>)>,
    pub dirty: bool,
    pub confirm_close: bool,
    /// Frame where the current preview started (for the playhead).
    playing_from: Option<usize>,
    play_started: std::time::Instant,
    /// Playhead position this frame, if playing.
    playhead: Option<usize>,
    /// Min/max per pixel column for both channels, and what they were computed for.
    cache: Vec<[(f32, f32); 2]>,
    cache_key: (u64, i64, u64, usize),
    version: u64,
}

impl AudioEditor {
    pub fn open(s: &Session, channel: usize) -> Result<AudioEditor, String> {
        let sample = s.channel_sample(channel)?;
        Ok(AudioEditor {
            channel,
            sample,
            sample_rate: s.view.sample_rate,
            undo: Vec::new(),
            redo: Vec::new(),
            sel: None,
            cursor: 0,
            view_start: 0.0,
            frames_per_px: 0.0,
            clipboard: None,
            dragging: false,
            gain_db: 3.0,
            normalize_db: -0.3,
            stretch_pct: 100.0,
            pitch_st: 0.0,
            fit_bars: 1,
            slice_equal: false,
            slice_sensitivity: 0.5,
            slice_count: 8,
            slice_cache: None,
            dirty: false,
            confirm_close: false,
            playing_from: None,
            play_started: std::time::Instant::now(),
            playhead: None,
            cache: Vec::new(),
            cache_key: (u64::MAX, 0, 0, 0),
            version: 0,
        })
    }

    fn len(&self) -> usize {
        self.sample.len()
    }

    /// The selected frame range, if any (never empty).
    fn selection(&self) -> Option<std::ops::Range<usize>> {
        let (a, b) = self.sel?;
        let r = a.min(b)..a.max(b).min(self.len());
        (!r.is_empty()).then_some(r)
    }

    /// The range an edit applies to: the selection, or the whole sample.
    fn target(&self) -> std::ops::Range<usize> {
        self.selection().unwrap_or(0..self.len())
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.sample.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self) {
        self.version += 1;
        self.dirty = true;
        self.cursor = self.cursor.min(self.len());
        if let Some((a, b)) = self.sel {
            self.sel = Some((a.min(self.len()), b.min(self.len())));
        }
    }

    /// Applies `f` to the sample as one undoable step.
    fn apply(&mut self, f: impl FnOnce(&mut Sample, std::ops::Range<usize>)) {
        if self.len() == 0 {
            return;
        }
        self.checkpoint();
        let r = self.target();
        f(&mut self.sample, r);
        self.changed();
    }

    /// Replaces the target range with `f` applied to it (length may change), as one undo step.
    fn process_range(&mut self, f: impl FnOnce(&Sample) -> Sample) {
        if self.len() == 0 {
            return;
        }
        self.checkpoint();
        let r = self.target();
        let processed = f(&edit::copy(&self.sample, r.clone()));
        let new_len = processed.len();
        edit::delete(&mut self.sample, r.clone());
        edit::insert(&mut self.sample, r.start, &processed);
        if self.sel.is_some() {
            self.sel = Some((r.start, r.start + new_len));
        }
        self.frames_per_px = 0.0;
        self.changed();
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.sample, prev));
            self.changed();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.sample, next));
            self.changed();
        }
    }

    fn copy(&mut self) {
        if let Some(r) = self.selection() {
            self.clipboard = Some(edit::copy(&self.sample, r));
        }
    }

    fn cut(&mut self) {
        if let Some(r) = self.selection() {
            self.clipboard = Some(edit::copy(&self.sample, r.clone()));
            self.checkpoint();
            edit::delete(&mut self.sample, r.clone());
            self.sel = None;
            self.cursor = r.start;
            self.changed();
        }
    }

    fn delete(&mut self) {
        if let Some(r) = self.selection() {
            self.checkpoint();
            edit::delete(&mut self.sample, r.clone());
            self.sel = None;
            self.cursor = r.start;
            self.changed();
        }
    }

    /// Pastes over the selection, or at the cursor.
    fn paste(&mut self) {
        let Some(clip) = self.clipboard.clone() else { return };
        self.checkpoint();
        let at = match self.selection() {
            Some(r) => {
                edit::delete(&mut self.sample, r.clone());
                r.start
            }
            None => self.cursor,
        };
        edit::insert(&mut self.sample, at, &clip);
        self.sel = Some((at, at + clip.len()));
        self.changed();
    }

    fn seconds(&self, frames: usize) -> String {
        let t = frames as f64 / self.sample_rate;
        if t >= 60.0 { format!("{}:{:06.3}", (t / 60.0) as u64, t % 60.0) } else { format!("{t:.3} s") }
    }

    fn fit(&mut self, width: f32) {
        self.view_start = 0.0;
        self.frames_per_px = (self.len().max(1) as f64 / width.max(1.0) as f64).max(1.0 / 64.0);
    }

    fn zoom(&mut self, factor: f64, around_px: f32, width: f32) {
        let anchor = self.view_start + around_px as f64 * self.frames_per_px;
        let max = (self.len().max(1) as f64 / width.max(1.0) as f64).max(1.0 / 64.0);
        self.frames_per_px = (self.frames_per_px * factor).clamp(1.0 / 64.0, max);
        self.view_start = anchor - around_px as f64 * self.frames_per_px;
        self.clamp_view(width);
    }

    fn clamp_view(&mut self, width: f32) {
        let max_start = (self.len() as f64 - width as f64 * self.frames_per_px).max(0.0);
        self.view_start = self.view_start.clamp(0.0, max_start);
    }

    /// Draws the editor. Returns a message for the status bar, if any.
    pub fn show(&mut self, ui: &mut Ui, s: &mut Session) -> Option<String> {
        let mut message = None;
        let has_sel = self.selection().is_some();

        // Unsaved-edits prompt when the window was closed.
        if self.confirm_close {
            egui::Frame::new().fill(theme::RAISED).corner_radius(theme::RADIUS).inner_margin(8.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Apply your edits to the channel?");
                    if ui.button("Apply").clicked() {
                        message = Some(self.save(s));
                    }
                    if ui.button("Discard").clicked() {
                        self.dirty = false;
                    }
                    if ui.button("Keep editing").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
            ui.add_space(6.0);
        }

        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.button("▶ Play").on_hover_text("Play the selection (or from the cursor)").clicked() {
                let r = self.selection().unwrap_or(self.cursor.min(self.len())..self.len());
                self.playing_from = Some(r.start);
                self.play_started = std::time::Instant::now();
                s.preview_sample(Some(Arc::new(edit::copy(&self.sample, r))));
            }
            if ui.button("■").on_hover_text("Stop").clicked() {
                self.playing_from = None;
                s.preview_sample(None);
            }
            ui.separator();
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                self.undo();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).clicked() {
                self.redo();
            }
            ui.separator();
            if ui.add_enabled(has_sel, egui::Button::new("Cut")).on_hover_text("⌘X").clicked() {
                self.cut();
            }
            if ui.add_enabled(has_sel, egui::Button::new("Copy")).on_hover_text("⌘C").clicked() {
                self.copy();
            }
            if ui.add_enabled(self.clipboard.is_some(), egui::Button::new("Paste")).on_hover_text("⌘V · replaces the selection, or inserts at the cursor").clicked() {
                self.paste();
            }
            if ui.add_enabled(has_sel, egui::Button::new("Delete")).on_hover_text("⌫").clicked() {
                self.delete();
            }
            if ui.add_enabled(has_sel, egui::Button::new("Crop")).on_hover_text("Keep only the selection").clicked() {
                let r = self.target();
                self.apply(|smp, _| edit::crop(smp, r));
                self.sel = None;
                self.cursor = 0;
                self.view_start = 0.0;
                self.frames_per_px = 0.0;
            }
            if ui.button("Trim silence").on_hover_text("Remove silence (below −60 dB) at the start and end").clicked() {
                let r = edit::trim_silence_range(&self.sample, -60.0);
                if r.len() < self.len() && !r.is_empty() {
                    self.checkpoint();
                    edit::crop(&mut self.sample, r);
                    self.sel = None;
                    self.frames_per_px = 0.0;
                    self.changed();
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let scope = if has_sel { "selection" } else { "whole sample" };
            if ui.button("Fade in").on_hover_text(format!("Fade in across the {scope}")).clicked() {
                self.apply(edit::fade_in);
            }
            if ui.button("Fade out").on_hover_text(format!("Fade out across the {scope}")).clicked() {
                self.apply(edit::fade_out);
            }
            if ui.button("Reverse").on_hover_text(format!("Reverse the {scope}")).clicked() {
                self.apply(edit::reverse);
            }
            if ui.button("Silence").on_hover_text(format!("Silence the {scope}")).clicked() {
                self.apply(edit::silence);
            }
            if ui.button("Remove DC").on_hover_text(format!("Remove DC offset from the {scope}")).clicked() {
                self.apply(edit::remove_dc);
            }
            ui.separator();
            let db = self.normalize_db;
            if ui.button("Normalize").on_hover_text(format!("Raise the {scope}'s peak to {db:.1} dBFS")).clicked() {
                self.apply(|smp, r| edit::normalize(smp, r, db));
            }
            ui.add(egui::DragValue::new(&mut self.normalize_db).range(-24.0..=0.0).speed(0.1).suffix(" dB"));
            ui.separator();
            let g = self.gain_db;
            if ui.button("Gain").on_hover_text(format!("Change the {scope}'s level by {g:+.1} dB")).clicked() {
                self.apply(|smp, r| edit::gain(smp, r, 10f32.powf(g / 20.0)));
            }
            ui.add(egui::DragValue::new(&mut self.gain_db).range(-48.0..=24.0).speed(0.1).suffix(" dB"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let scope = if self.selection().is_some() { "selection" } else { "whole sample" };
            let sr = self.sample_rate;
            if ui.button("Stretch").on_hover_text(format!("Change the {scope}'s length without changing its pitch")).clicked() {
                let ratio = self.stretch_pct / 100.0;
                self.process_range(|smp| edit::time_stretch(smp, ratio, sr));
            }
            ui.add(egui::DragValue::new(&mut self.stretch_pct).range(25.0..=400.0).speed(0.5).suffix(" %"));
            ui.separator();
            if ui.button("Pitch").on_hover_text(format!("Shift the {scope}'s pitch without changing its length")).clicked() {
                let st = self.pitch_st;
                self.process_range(|smp| edit::pitch_shift(smp, st, sr));
            }
            ui.add(egui::DragValue::new(&mut self.pitch_st).range(-24.0..=24.0).speed(0.05).fixed_decimals(2).suffix(" st"));
            ui.separator();
            let bar_secs = 60.0 / s.view.tempo.max(1.0) * s.view.beats_per_bar.max(1) as f64;
            if ui.button("Fit to").on_hover_text(format!("Stretch the {scope} to exactly this many bars at {:.1} BPM (keeps pitch)", s.view.tempo)).clicked() {
                let len = self.target().len().max(1) as f64;
                let ratio = self.fit_bars.max(1) as f64 * bar_secs * sr / len;
                self.process_range(|smp| edit::time_stretch(smp, ratio, sr));
            }
            ui.add(egui::DragValue::new(&mut self.fit_bars).range(1..=64).suffix(" bar(s)"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new("Slice").small().color(theme::TEXT_DIM));
            ui.selectable_value(&mut self.slice_equal, false, "At hits");
            ui.selectable_value(&mut self.slice_equal, true, "Equal");
            if self.slice_equal {
                ui.add(egui::DragValue::new(&mut self.slice_count).range(2..=64).suffix(" slices"));
            } else {
                ui.add(egui::Slider::new(&mut self.slice_sensitivity, 0.0..=1.0).text("sensitivity").show_value(false));
            }
            let cuts = self.slice_points();
            ui.label(egui::RichText::new(format!("{} slices", cuts.len())).small().color(theme::TEXT_DIM));
            if ui.button("Slice to new channel").on_hover_text("Make a slicer channel (slice 1 on C5, slice 2 on C#5, …) and a pattern that plays the slices in order").clicked() {
                message = Some(self.slice_to_channel(s, &cuts));
            }
        });
        ui.add_space(4.0);

        // Playhead: the engine reports how far the preview has played.
        let played = s.controls.preview_position.load(std::sync::atomic::Ordering::Relaxed);
        self.playhead = match self.playing_from {
            Some(_) if played == engine::control::NO_PREVIEW => {
                // Not started yet (the command is still queued) or finished.
                if self.playhead.is_some() || self.play_started.elapsed().as_secs_f32() > 1.0 {
                    self.playing_from = None;
                }
                None
            }
            Some(from) => Some(from + played as usize),
            None => None,
        };
        if self.playing_from.is_some() {
            ui.ctx().request_repaint();
        }

        // Waveform.
        let width = ui.available_width().max(200.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, RULER_H + WAVE_H), Sense::click_and_drag());
        if self.frames_per_px <= 0.0 {
            self.fit(width);
        }
        self.clamp_view(width);
        self.handle_input(ui, &resp, rect);
        self.draw(ui, rect);

        // Info line and zoom.
        ui.horizontal(|ui| {
            let info = match self.selection() {
                Some(r) => format!(
                    "Selection {} – {} ({})   ·   peak {}",
                    self.seconds(r.start),
                    self.seconds(r.end),
                    self.seconds(r.len()),
                    db_label(edit::peak(&self.sample, r))
                ),
                None => format!("Length {}   ·   cursor {}   ·   peak {}", self.seconds(self.len()), self.seconds(self.cursor), db_label(edit::peak(&self.sample, 0..self.len()))),
            };
            ui.label(egui::RichText::new(info).font(theme::mono(11.0)).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.fit(width);
                }
                if ui.add_enabled(has_sel, egui::Button::new("Zoom to selection")).clicked()
                    && let Some(r) = self.selection()
                {
                    self.frames_per_px = (r.len() as f64 / width as f64).max(1.0 / 64.0);
                    self.view_start = r.start as f64;
                    self.clamp_view(width);
                }
                if ui.button("+").clicked() {
                    self.zoom(0.5, width / 2.0, width);
                }
                if ui.button("−").clicked() {
                    self.zoom(2.0, width / 2.0, width);
                }
            });
        });

        ui.separator();
        ui.horizontal(|ui| {
            let apply = egui::Button::new(egui::RichText::new("Apply to channel").color(if self.dirty { theme::BG } else { theme::TEXT_DIM }))
                .fill(if self.dirty { theme::ACCENT } else { theme::RAISED });
            if ui.add_enabled(self.dirty, apply).on_hover_text("Save the edit as a new WAV and play it on this channel (the original file is kept)").clicked() {
                message = Some(self.save(s));
            }
            if ui.add_enabled(self.dirty, egui::Button::new("Revert")).on_hover_text("Reload the channel's sample").clicked() {
                match s.channel_sample(self.channel) {
                    Ok(smp) => {
                        self.checkpoint();
                        self.sample = smp;
                        self.changed();
                        self.dirty = false;
                    }
                    Err(e) => message = Some(e),
                }
            }
            ui.label(egui::RichText::new("Drag to select · double-click selects all · scroll to move · ⌘/pinch-scroll to zoom").small().color(theme::TEXT_FAINT));
        });
        message
    }

    /// Where the current slicing settings cut the sample (cached until something changes).
    fn slice_points(&mut self) -> Vec<usize> {
        let key = (self.version, self.slice_equal, (self.slice_sensitivity * 1000.0) as u32, self.slice_count);
        if let Some((k, cuts)) = &self.slice_cache
            && *k == key
        {
            return cuts.clone();
        }
        let cuts = if self.slice_equal { edit::equal_slices(&self.sample, self.slice_count) } else { edit::detect_transients(&self.sample, self.slice_sensitivity, (self.sample_rate * 0.05) as usize) };
        self.slice_cache = Some((key, cuts.clone()));
        cuts
    }

    /// Writes the audio as it is now and builds a slicer channel from it.
    fn slice_to_channel(&mut self, s: &mut Session, cuts: &[usize]) -> String {
        let name = s.view.channels.get(self.channel).map(|c| c.name.clone()).unwrap_or_else(|| "Loop".into());
        let dir = s.path.as_ref().and_then(|p| p.parent().map(|d| d.join("KVNG Edits"))).unwrap_or_else(|| crate::data_dir().join("Edits"));
        let stem: String = name.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
        let file = (1..).map(|i| dir.join(format!("{stem} slices {i}.wav"))).find(|p| !p.exists()).unwrap();
        if let Err(e) = edit::write_wav(&self.sample, &file, self.sample_rate.round() as u32) {
            return e;
        }
        match s.create_slicer(&file, Arc::new(self.sample.clone()), cuts, &name) {
            Ok(c) => format!("Made {} with {} slices, and a pattern that plays them", s.view.channels[c].name, cuts.len()),
            Err(e) => e,
        }
    }

    fn save(&mut self, s: &mut Session) -> String {
        match s.replace_sample(self.channel, self.sample.clone()) {
            Ok(path) => {
                self.dirty = false;
                format!("Saved {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            }
            Err(e) => e,
        }
    }

    fn frame_at(&self, x: f32, rect: Rect) -> usize {
        ((self.view_start + (x - rect.min.x) as f64 * self.frames_per_px).max(0.0) as usize).min(self.len())
    }

    fn x_of(&self, frame: usize, rect: Rect) -> f32 {
        rect.min.x + ((frame as f64 - self.view_start) / self.frames_per_px) as f32
    }

    fn handle_input(&mut self, ui: &Ui, resp: &egui::Response, rect: Rect) {
        let width = rect.width();
        if resp.double_clicked() {
            self.sel = Some((0, self.len()));
        } else if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let f = self.frame_at(p.x, rect);
                let extend = ui.input(|i| i.modifiers.shift);
                self.sel = Some(match (extend, self.sel) {
                    (true, Some((a, _))) => (a, f),
                    _ => (f, f),
                });
                self.dragging = true;
            }
        } else if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let f = self.frame_at(p.x, rect);
            if ui.input(|i| i.modifiers.shift) {
                let a = self.sel.map_or(self.cursor, |(a, _)| a);
                self.sel = Some((a, f));
            } else {
                self.sel = None;
                self.cursor = f;
            }
        }
        if self.dragging {
            if let Some(p) = ui.ctx().pointer_latest_pos() {
                let f = self.frame_at(p.x, rect);
                if let Some((a, _)) = self.sel {
                    self.sel = Some((a, f));
                }
                // Scroll while dragging past the edges.
                if p.x > rect.max.x {
                    self.view_start += (p.x - rect.max.x) as f64 * self.frames_per_px * 0.5;
                } else if p.x < rect.min.x {
                    self.view_start -= (rect.min.x - p.x) as f64 * self.frames_per_px * 0.5;
                }
                self.clamp_view(width);
            }
            if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                self.dragging = false;
                if let Some((a, b)) = self.sel
                    && a == b
                {
                    self.sel = None;
                    self.cursor = a;
                }
            }
        }

        if resp.hovered() {
            let (scroll, zoom, cmd) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command));
            let px = ui.ctx().pointer_latest_pos().map_or(width / 2.0, |p| p.x - rect.min.x);
            if zoom != 1.0 {
                self.zoom(1.0 / zoom as f64, px, width);
            } else if cmd && scroll.y != 0.0 {
                self.zoom((-scroll.y as f64 * 0.01).exp(), px, width);
            } else if scroll != egui::Vec2::ZERO {
                let delta = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                self.view_start -= delta as f64 * self.frames_per_px;
                self.clamp_view(width);
            }

            if !ui.ctx().egui_wants_keyboard_input() {
                let consume = |m: egui::Modifiers, k: egui::Key| ui.ctx().input_mut(|i| i.consume_key(m, k));
                if consume(egui::Modifiers::COMMAND, egui::Key::A) {
                    self.sel = Some((0, self.len()));
                }
                if consume(egui::Modifiers::COMMAND, egui::Key::C) {
                    self.copy();
                }
                if consume(egui::Modifiers::COMMAND, egui::Key::X) {
                    self.cut();
                }
                if consume(egui::Modifiers::COMMAND, egui::Key::V) {
                    self.paste();
                }
                if consume(egui::Modifiers::NONE, egui::Key::Delete) || consume(egui::Modifiers::NONE, egui::Key::Backspace) {
                    self.delete();
                }
                if consume(egui::Modifiers::NONE, egui::Key::Escape) {
                    self.sel = None;
                }
            }
        }
    }

    /// Min/max per pixel column, recomputed only when the sample or view changes.
    fn columns(&mut self, width: usize) -> &[[(f32, f32); 2]] {
        let key = (self.version, self.view_start as i64, self.frames_per_px.to_bits(), width);
        if key != self.cache_key {
            self.cache_key = key;
            self.cache.clear();
            let len = self.len();
            for x in 0..width {
                let a = (self.view_start + x as f64 * self.frames_per_px) as usize;
                let b = ((self.view_start + (x + 1) as f64 * self.frames_per_px).ceil() as usize).max(a + 1);
                let mut col = [(0.0f32, 0.0f32); 2];
                if a < len {
                    for (c, data) in [&self.sample.left, &self.sample.right].into_iter().enumerate() {
                        let (lo, hi) = data[a..b.min(len)].iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
                        col[c] = (lo, hi);
                    }
                }
                self.cache.push(col);
            }
        }
        &self.cache
    }

    fn draw(&mut self, ui: &Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme::RADIUS, theme::PANEL);
        let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_H));
        let wave = Rect::from_min_max(pos2(rect.min.x, ruler.max.y), rect.max);
        painter.rect_filled(ruler, 0.0, theme::PANEL_2);

        // Time ruler: pick a tick spacing that leaves ~80 px between labels.
        let secs_per_px = self.frames_per_px / self.sample_rate;
        let step = [0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| s / secs_per_px >= 80.0).unwrap_or(120.0);
        let first = (self.view_start / self.sample_rate / step).floor() as i64;
        let last = ((self.view_start + rect.width() as f64 * self.frames_per_px) / self.sample_rate / step).ceil() as i64;
        for i in first..=last {
            let t = i as f64 * step;
            let x = self.x_of((t * self.sample_rate) as usize, rect);
            painter.line_segment([pos2(x, ruler.max.y - 5.0), pos2(x, ruler.max.y)], (1.0, theme::LINE_STRONG));
            painter.line_segment([pos2(x, wave.min.y), pos2(x, wave.max.y)], (1.0, theme::LINE_SOFT));
            let label = if step < 1.0 { format!("{t:.3}") } else { format!("{t:.0}s") };
            painter.text(pos2(x + 3.0, ruler.min.y + 2.0), egui::Align2::LEFT_TOP, label, theme::mono(9.5), theme::TEXT_FAINT);
        }

        // Selection.
        if let Some(r) = self.selection() {
            let (x0, x1) = (self.x_of(r.start, rect).max(rect.min.x), self.x_of(r.end, rect).min(rect.max.x));
            if x1 > x0 {
                painter.rect_filled(Rect::from_min_max(pos2(x0, rect.min.y), pos2(x1.max(x0 + 1.0), rect.max.y)), 0.0, theme::alpha(theme::ACCENT, 0.18));
            }
        }

        // Two lanes: left on top, right below.
        let lane_h = wave.height() / 2.0;
        let cols: Vec<[(f32, f32); 2]> = self.columns(rect.width().ceil() as usize).to_vec();
        let sel = self.selection();
        for c in 0..2 {
            let mid = wave.min.y + lane_h * (c as f32 + 0.5);
            painter.line_segment([pos2(wave.min.x, mid), pos2(wave.max.x, mid)], (1.0, theme::LINE));
            if c == 1 {
                painter.line_segment([pos2(wave.min.x, wave.min.y + lane_h), pos2(wave.max.x, wave.min.y + lane_h)], (1.0, theme::LINE_STRONG));
            }
            let amp = lane_h * 0.46;
            if self.frames_per_px < 1.0 {
                // Zoomed in past one frame per pixel: draw the samples as a line.
                let data = if c == 0 { &self.sample.left } else { &self.sample.right };
                let a = self.view_start as usize;
                let b = ((self.view_start + rect.width() as f64 * self.frames_per_px).ceil() as usize + 1).min(data.len());
                let points: Vec<Pos2> = (a..b).map(|f| pos2(self.x_of(f, rect), mid - data[f].clamp(-1.0, 1.0) * amp)).collect();
                painter.add(egui::Shape::line(points, (1.5, theme::ACCENT_HOT)));
            } else {
                for (x, col) in cols.iter().enumerate() {
                    let (lo, hi) = col[c];
                    if lo > hi {
                        continue;
                    }
                    let px = rect.min.x + x as f32 + 0.5;
                    let frame = self.frame_at(px, rect);
                    let color: Color32 = if sel.as_ref().is_some_and(|r| r.contains(&frame)) { theme::ACCENT_HOT } else { theme::ACCENT };
                    let (y0, y1) = (mid - hi.clamp(-1.0, 1.0) * amp, mid - lo.clamp(-1.0, 1.0) * amp);
                    painter.line_segment([pos2(px, y0), pos2(px, y1.max(y0 + 1.0))], (1.0, color));
                }
            }
            painter.text(pos2(wave.min.x + 4.0, wave.min.y + lane_h * c as f32 + 3.0), egui::Align2::LEFT_TOP, if c == 0 { "L" } else { "R" }, theme::mono(9.5), theme::TEXT_FAINT);
        }

        if let Some(f) = self.playhead {
            let x = self.x_of(f, rect);
            if (rect.min.x..=rect.max.x).contains(&x) {
                painter.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], (1.5, theme::WARN));
            }
        }

        // Slice markers.
        if let Some((_, cuts)) = &self.slice_cache {
            for (k, &c) in cuts.iter().enumerate() {
                let x = self.x_of(c, rect);
                if (rect.min.x..=rect.max.x).contains(&x) {
                    painter.line_segment([pos2(x, rect.min.y + RULER_H), pos2(x, rect.max.y)], (1.0, theme::alpha(theme::TEXT, 0.45)));
                    painter.text(pos2(x + 2.0, rect.min.y + RULER_H + 2.0), egui::Align2::LEFT_TOP, (k + 1).to_string(), theme::mono(8.5), theme::TEXT_DIM);
                }
            }
        }

        // Cursor.
        if self.selection().is_none() {
            let x = self.x_of(self.cursor, rect);
            if (rect.min.x..=rect.max.x).contains(&x) {
                painter.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], (1.5, theme::TEXT));
            }
        }
        painter.rect_stroke(rect, theme::RADIUS, (1.0, theme::LINE), egui::StrokeKind::Inside);
    }
}

fn db_label(peak: f32) -> String {
    if peak <= 1e-9 { "−∞ dB".into() } else { format!("{:.1} dB", 20.0 * peak.log10()) }
}
