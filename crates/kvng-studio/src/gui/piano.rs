//! The piano roll: notes of one channel in the selected pattern, with ghost notes
//! from the other channels and a velocity lane.

use eframe::egui::{self, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::view::{ArpMode, NoteView, StrumOptions};

const ROOTS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

use super::session::{Mode, Session};
use super::{theme, transport, widgets};

const KEYS_W: f32 = 58.0;
const RULER_H: f32 = 22.0;
const VELOCITY_H: f32 = 90.0;
const RESIZE_GRIP: f32 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Draw,
    Paint,
    Select,
    Slice,
    Delete,
}

pub struct PianoState {
    pub tool: Tool,
    pub px_per_tick: f32,
    pub key_h: f32,
    pub scroll_x: f64,
    /// Pixels scrolled down from the top key (127).
    pub scroll_y: f32,
    /// (pattern, channel) last centred on, to re-centre when either changes.
    framed: Option<(usize, usize)>,
    /// Selected notes (indices into the pattern's notes).
    selected: Vec<usize>,
    clipboard: Vec<NoteView>,
    drag: Option<NoteDrag>,
    /// Corner where a selection box started.
    marquee: Option<egui::Pos2>,
    marquee_add: bool,
    /// The last painted grid cell and whether this stroke has an undo checkpoint.
    paint_last: Option<(u64, u8)>,
    paint_checkpointed: bool,
    /// Highlighted scale: root key class (0 = C) and scale.
    pub scale: Option<(u8, Scale)>,
    /// What the bottom lane edits.
    pub lane: Lane,
    /// Quantize strength (0..=1) and whether note ends are quantized too.
    quant_strength: f32,
    quant_ends: bool,
    /// Humanize ranges: timing (ticks) and velocity.
    human_time: u64,
    human_vel: u8,
    /// Randomize ranges and LFO tool settings (lane, period in beats, depth, centre, shape).
    rand_keys: u8,
    rand_vel: u8,
    rand_scale: bool,
    lfo: (u8, f32, f32, f32, u8),
    /// Gentle, deterministic velocity variation for newly drawn notes.
    natural_velocities: bool,
    feel_amount: f32,
    strum: StrumOptions,
    /// A strum or piano-feel menu is open: its edits are previewed live from these
    /// original notes, so moving a slider never builds on the previous result.
    preview: Option<ToolPreview>,
    /// Chord stamp: drawing a note adds this chord (FL's chord stamp).
    chord: Option<engine::view::Chord>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Velocity,
    Pan,
    ModX,
    ModY,
}

impl Lane {
    const ALL: [Lane; 4] = [Lane::Velocity, Lane::Pan, Lane::ModX, Lane::ModY];

    fn label(self) -> &'static str {
        match self {
            Lane::Velocity => "Velocity",
            Lane::Pan => "Pan",
            Lane::ModX => "Mod X",
            Lane::ModY => "Mod Y",
        }
    }

    fn bipolar(self) -> bool {
        !matches!(self, Lane::Velocity)
    }

    /// The note's value on this lane, 0..=1.
    fn get(self, n: &NoteView) -> f32 {
        match self {
            Lane::Velocity => n.velocity as f32 / 128.0,
            Lane::Pan => n.pan as f32 / 128.0,
            Lane::ModX => n.mod_x as f32 / 255.0,
            Lane::ModY => n.mod_y as f32 / 255.0,
        }
    }

    fn set(self, n: &mut NoteView, v: f32) {
        match self {
            Lane::Velocity => n.velocity = (v * 128.0).round().clamp(1.0, 128.0) as u8,
            Lane::Pan => n.pan = (v * 128.0).round().clamp(0.0, 128.0) as u8,
            Lane::ModX => n.mod_x = (v * 255.0).round() as u8,
            Lane::ModY => n.mod_y = (v * 255.0).round() as u8,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Major,
    Minor,
    HarmonicMinor,
    Dorian,
    Phrygian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl Scale {
    const ALL: [Scale; 8] = [Scale::Major, Scale::Minor, Scale::HarmonicMinor, Scale::Dorian, Scale::Phrygian, Scale::MajorPentatonic, Scale::MinorPentatonic, Scale::Blues];

    fn label(self) -> &'static str {
        match self {
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::HarmonicMinor => "Harmonic minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::MajorPentatonic => "Major pentatonic",
            Scale::MinorPentatonic => "Minor pentatonic",
            Scale::Blues => "Blues",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }

    fn contains(self, root: u8, key: u8) -> bool {
        self.intervals().contains(&((key + 12 - root % 12) % 12))
    }
}

impl Default for PianoState {
    fn default() -> Self {
        Self {
            tool: Tool::Draw,
            px_per_tick: 1.0,
            key_h: 14.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            framed: None,
            selected: Vec::new(),
            clipboard: Vec::new(),
            drag: None,
            marquee: None,
            marquee_add: false,
            paint_last: None,
            paint_checkpointed: false,
            scale: None,
            lane: Lane::Velocity,
            quant_strength: 1.0,
            quant_ends: false,
            human_time: 4,
            human_vel: 12,
            rand_keys: 3,
            rand_vel: 16,
            rand_scale: true,
            lfo: (0, 1.0, 0.6, 0.6, 0),
            natural_velocities: true,
            feel_amount: 0.75,
            strum: StrumOptions::default(),
            preview: None,
            chord: None,
        }
    }
}

#[derive(Clone, Copy)]
enum DragKind {
    Move,
    Resize,
}

/// Moving or resizing the selected notes; `originals` are their values at the start.
struct NoteDrag {
    kind: DragKind,
    grab_tick: f64,
    grab_key: i32,
    originals: Vec<(usize, NoteView)>,
}

fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    // FL Studio names MIDI 60 "C5".
    format!("{}{}", NAMES[key as usize % 12], key / 12)
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

pub fn show(ui: &mut Ui, s: &mut Session, st: &mut PianoState) {
    toolbar(ui, s, st);
    ui.add_space(2.0);

    let rect = ui.available_rect_before_wrap();
    if rect.height() < 120.0 {
        return;
    }
    let grid = Rect::from_min_max(pos2(rect.min.x + KEYS_W, rect.min.y + RULER_H), pos2(rect.max.x, rect.max.y - VELOCITY_H - 4.0));
    let ruler = Rect::from_min_max(pos2(grid.min.x, rect.min.y), pos2(grid.max.x, grid.min.y));
    let keys = Rect::from_min_max(pos2(rect.min.x, grid.min.y), pos2(grid.min.x, grid.max.y));
    let vel = Rect::from_min_max(pos2(grid.min.x, grid.max.y + 4.0), pos2(rect.max.x, rect.max.y));
    let vel_label = Rect::from_min_max(pos2(rect.min.x, vel.min.y), pos2(grid.min.x, vel.max.y));

    let Some(pattern) = s.view.patterns.get(s.pattern) else { return };
    let pattern_len = pattern.length;
    let channel = s.channel;

    // Frame the channel's notes when switching pattern or channel.
    if st.framed != Some((s.pattern, channel)) {
        st.framed = Some((s.pattern, channel));
        st.selected.clear();
        st.drag = None;
        let keys_used: Vec<u8> = pattern.notes.iter().filter(|n| n.channel == channel).map(|n| n.key).collect();
        let centre = if keys_used.is_empty() { 60.0 } else { keys_used.iter().map(|k| *k as f32).sum::<f32>() / keys_used.len() as f32 };
        st.scroll_y = ((127.0 - centre) * st.key_h - grid.height() / 2.0).max(0.0);
        st.scroll_x = 0.0;
        st.px_per_tick = (grid.width() / pattern_len.max(1) as f32).clamp(0.05, 4.0);
    }

    let tick_to_x = |st: &PianoState, t: f64| grid.min.x + ((t - st.scroll_x) as f32) * st.px_per_tick;
    let x_to_tick = |st: &PianoState, x: f32| st.scroll_x + ((x - grid.min.x) / st.px_per_tick) as f64;
    let key_to_y = |st: &PianoState, k: i32| grid.min.y + (127 - k) as f32 * st.key_h - st.scroll_y;
    let y_to_key = |st: &PianoState, y: f32| 127 - ((y - grid.min.y + st.scroll_y) / st.key_h).floor() as i32;

    // Scroll / zoom.
    if ui.rect_contains_pointer(rect) {
        let (scroll, zoom, cmd, shift, alt, pointer) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command, i.modifiers.shift, i.modifiers.alt, i.pointer.hover_pos()));
        let anchor_x = pointer.map_or(grid.center().x, |p| p.x.max(grid.min.x));
        let zoom_x = |st: &mut PianoState, f: f32| {
            let anchor = x_to_tick(st, anchor_x);
            st.px_per_tick = (st.px_per_tick * f).clamp(0.05, 8.0);
            st.scroll_x = (anchor - ((anchor_x - grid.min.x) / st.px_per_tick) as f64).max(0.0);
        };
        if zoom != 1.0 {
            zoom_x(st, zoom);
        } else if cmd && scroll.y != 0.0 {
            zoom_x(st, (scroll.y * 0.01).exp());
        } else if alt && scroll.y != 0.0 {
            st.key_h = (st.key_h * (scroll.y * 0.01).exp()).clamp(6.0, 32.0);
        } else {
            let (dx, dy) = if shift { (scroll.y, scroll.x) } else { (scroll.x, scroll.y) };
            st.scroll_x = (st.scroll_x - (dx / st.px_per_tick) as f64).max(0.0);
            st.scroll_y = (st.scroll_y - dy).clamp(0.0, (128.0 * st.key_h - grid.height()).max(0.0));
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);

    // Key rows.
    let top_key = y_to_key(st, grid.min.y).min(127);
    let bottom_key = y_to_key(st, grid.max.y).max(0);
    let grid_painter = ui.painter_at(grid);
    for k in bottom_key..=top_key {
        let y = key_to_y(st, k);
        let row = Rect::from_min_max(pos2(grid.min.x, y), pos2(grid.max.x, y + st.key_h));
        let out_of_scale = st.scale.is_some_and(|(root, sc)| !sc.contains(root, k as u8));
        let fill = match (st.scale, out_of_scale) {
            (Some(_), true) => theme::mix(theme::PANEL, theme::BG, 0.75),
            (Some((root, _)), false) if (k as u8) % 12 == root % 12 => theme::mix(theme::PANEL, theme::ACCENT_DIM, 0.35),
            (Some(_), false) => theme::mix(theme::PANEL, theme::ACCENT_DIM, 0.12),
            (None, _) => if is_black(k as u8) { theme::mix(theme::PANEL, theme::BG, 0.55) } else { theme::PANEL },
        };
        grid_painter.rect_filled(row, 0.0, fill);
        let line = if k % 12 == 0 { theme::LINE } else { theme::LINE_SOFT };
        grid_painter.line_segment([pos2(grid.min.x, row.max.y), pos2(grid.max.x, row.max.y)], Stroke::new(1.0, line));
    }
    // Shade beyond the pattern end.
    let end_x = tick_to_x(st, pattern_len as f64);
    if end_x < grid.max.x {
        grid_painter.rect_filled(Rect::from_min_max(pos2(end_x.max(grid.min.x), grid.min.y), grid.max), 0.0, theme::alpha(theme::BG, 0.55));
    }

    // Vertical grid: bars, beats, steps.
    let step = s.view.step_ticks() as f64;
    let beat = s.ppq() as f64;
    let bar = s.ticks_per_bar() as f64;
    let show_steps = step as f32 * st.px_per_tick > 8.0;
    let first = (st.scroll_x / step).floor() as i64;
    let last = (x_to_tick(st, grid.max.x) / step).ceil() as i64;
    for i in first.max(0)..=last {
        let t = i as f64 * step;
        let is_bar = t % bar == 0.0;
        let is_beat = t % beat == 0.0;
        let (color, width, show) = if is_bar {
            (Color32::from_rgb(0x4C, 0x4C, 0x62), 1.2, true)
        } else if is_beat {
            (theme::LINE, 1.0, true)
        } else {
            (theme::LINE_SOFT, 1.0, show_steps)
        };
        if show {
            let x = tick_to_x(st, t);
            grid_painter.line_segment([pos2(x, grid.min.y), pos2(x, grid.max.y)], Stroke::new(width, color));
            painter.line_segment([pos2(x, vel.min.y), pos2(x, vel.max.y)], Stroke::new(1.0, if is_bar { theme::LINE } else { theme::LINE_SOFT }));
        }
    }

    // Notes: ghosts first, then the selected channel's notes.
    let color = s.channel_color(channel);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let note_count = pattern.notes.len();
    st.selected.retain(|&i| i < note_count);
    let mut hovered_note: Option<(usize, bool)> = None;
    for pass in [false, true] {
        for (ni, n) in pattern.notes.iter().enumerate() {
            if (n.channel == channel) != pass {
                continue;
            }
            let r = note_rect(n, |t| tick_to_x(st, t), |k| key_to_y(st, k), st.key_h);
            if !r.intersects(grid) {
                continue;
            }
            if pass {
                let selected = st.selected.contains(&ni);
                let vel_t = n.velocity as f32 / 128.0;
                let base_color = theme::mix(color, Color32::WHITE, 0.15 * vel_t);
                let fill = if selected { theme::ACCENT } else { base_color };
                let stroke = if selected { Stroke::new(1.5, theme::ACCENT_HOT) } else { Stroke::new(1.0, theme::mix(color, theme::BG, 0.4)) };
                grid_painter.rect(r, theme::RADIUS_SM, fill, stroke, StrokeKind::Inside);
                // Bevel top shine
                let top_shine = Rect::from_min_max(pos2(r.min.x + 1.0, r.min.y + 1.0), pos2(r.max.x - 1.0, r.min.y + 2.5));
                grid_painter.rect_filled(top_shine, 1.0, theme::alpha(Color32::WHITE, 0.25));
                if n.slide {
                    // Slide notes: a rising ramp across the note (FL draws them with a triangle).
                    let ramp = vec![pos2(r.min.x + 1.0, r.max.y - 1.0), pos2(r.max.x - 1.0, r.min.y + 1.0), pos2(r.max.x - 1.0, r.max.y - 1.0)];
                    grid_painter.add(egui::Shape::convex_polygon(ramp, theme::alpha(Color32::BLACK, 0.35), Stroke::NONE));
                }
                if r.width() > 26.0 && st.key_h >= 11.0 {
                    let tp = grid_painter.with_clip_rect(r.intersect(grid));
                    tp.text(pos2(r.min.x + 3.0, r.center().y), Align2::LEFT_CENTER, key_name(n.key), theme::ui_font(9.0), if selected { theme::BG } else { Color32::WHITE });
                }
                if let Some(p) = pointer
                    && r.contains(p)
                {
                    hovered_note = Some((ni, p.x > r.max.x - RESIZE_GRIP));
                }
            } else {
                grid_painter.rect(r, theme::RADIUS_SM, theme::alpha(theme::TEXT_DIM, 0.12), Stroke::new(1.0, theme::alpha(theme::TEXT_DIM, 0.25)), StrokeKind::Inside);
            }
        }
    }

    // Note editing.
    let grid_resp = ui.interact(grid, Id::new("piano-grid"), Sense::click_and_drag());
    if let Some((_, edge)) = hovered_note
        && matches!(st.tool, Tool::Draw | Tool::Select)
    {
        ui.ctx().set_cursor_icon(if edge { CursorIcon::ResizeHorizontal } else { CursorIcon::Grab });
    }
    let (primary_pressed, primary_down, secondary_down, released, modifiers) =
        ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.secondary_down(), i.pointer.any_released(), i.modifiers));
    let snap = s.snap_ticks().max(1) as f64;

    if grid_resp.hovered() && let Some(p) = pointer {
        // Right button deletes what it touches (FL behaviour), in any tool.
        let deleting = secondary_down || (st.tool == Tool::Delete && primary_down);
        if deleting {
            if let Some((ni, _)) = hovered_note {
                if !ui.data(|d| d.get_temp::<bool>(Id::new("piano-deleting")).unwrap_or(false)) {
                    s.checkpoint();
                    ui.data_mut(|d| d.insert_temp(Id::new("piano-deleting"), true));
                }
                s.view.remove_note(s.pattern, ni);
                st.selected.clear();
                s.mark_dirty();
            }
        } else if st.tool == Tool::Paint && primary_down {
            let start = ((x_to_tick(st, p.x) / snap).floor() * snap).max(0.0) as u64;
            let key = y_to_key(st, p.y).clamp(0, 127) as u8;
            if st.paint_last != Some((start, key)) {
                st.paint_last = Some((start, key));
                let occupied = s.view.patterns[s.pattern].notes.iter().any(|n| n.channel == channel && n.start == start && n.key == key);
                if !occupied {
                    if !st.paint_checkpointed {
                        s.checkpoint();
                        st.paint_checkpointed = true;
                    }
                    let length = s.last_note_length.min(snap as u64).max(1);
                    if let Some(i) = insert_note(s, st, start, key, channel, length) {
                        let velocity = s.view.patterns[s.pattern].notes[i].velocity;
                        s.preview_on(channel, key, velocity);
                    }
                }
            }
        } else if primary_pressed && matches!(st.tool, Tool::Draw | Tool::Select | Tool::Slice) {
            match hovered_note {
                Some((ni, _)) if st.tool == Tool::Slice => {
                    let n = s.view.patterns[s.pattern].notes[ni];
                    let cut = ((x_to_tick(st, p.x) / snap).round() * snap).max(0.0) as u64;
                    if cut > n.start && cut < n.start.saturating_add(n.length) {
                        s.checkpoint();
                        s.view.patterns[s.pattern].notes[ni].length = cut - n.start;
                        if let Some(right) = s.view.add_note(s.pattern, NoteView { start: cut, length: n.start + n.length - cut, ..n }) {
                            st.selected = vec![ni, right];
                        }
                        s.mark_dirty();
                    }
                }
                Some((ni, edge)) => {
                    if modifiers.shift {
                        // Shift-click toggles a note in the selection.
                        if let Some(pos) = st.selected.iter().position(|&i| i == ni) {
                            st.selected.remove(pos);
                        } else {
                            st.selected.push(ni);
                        }
                    } else {
                        if !st.selected.contains(&ni) {
                            st.selected = vec![ni];
                        }
                        s.checkpoint();
                        start_drag(st, s, if edge { DragKind::Resize } else { DragKind::Move }, x_to_tick(st, p.x), y_to_key(st, p.y), ni);
                    }
                    let n = s.view.patterns[s.pattern].notes[ni];
                    s.preview_on(channel, n.key, n.velocity);
                }
                // Shift-drag (or ⌘-drag, or the Select tool) draws a selection box; with
                // Shift held it adds to the current selection.
                None if st.tool == Tool::Select || modifiers.command || modifiers.shift => {
                    st.marquee = Some(p);
                    st.marquee_add = modifiers.shift;
                }
                None if st.tool == Tool::Draw => {
                    s.checkpoint();
                    let start = ((x_to_tick(st, p.x) / snap).floor() * snap).max(0.0) as u64;
                    let key = y_to_key(st, p.y).clamp(0, 127) as u8;
                    let length = s.last_note_length;
                    if let Some(index) = insert_note(s, st, start, key, channel, length) {
                        start_drag(st, s, DragKind::Move, x_to_tick(st, p.x), y_to_key(st, p.y), index);
                        let velocity = s.view.patterns[s.pattern].notes[index].velocity;
                        s.preview_on(channel, key, velocity);
                    }
                }
                None => {}
            }
        }
    }
    if !secondary_down && !primary_down {
        ui.data_mut(|d| d.remove::<bool>(Id::new("piano-deleting")));
        st.paint_last = None;
        st.paint_checkpointed = false;
    }

    // Dragging the selection.
    if let (Some(d), Some(p)) = (&st.drag, pointer) {
        let dt = ((x_to_tick(st, p.x) - d.grab_tick) / snap).round() * snap;
        let dk = y_to_key(st, p.y) - d.grab_key;
        let mut changed = false;
        let mut preview_key = None;
        for &(i, orig) in &d.originals {
            let (start, key, length) = match d.kind {
                DragKind::Move => (orig.start as i64 + dt as i64, orig.key as i32 + dk, orig.length as i64),
                DragKind::Resize => (orig.start as i64, orig.key as i32, orig.length as i64 + dt as i64),
            };
            let cur = s.view.patterns[s.pattern].notes.get(i).copied();
            if let Some(n) = cur
                && (n.start as i64 != start.max(0) || n.key as i32 != key.clamp(0, 127) || n.length as i64 != length)
            {
                if n.key as i32 != key.clamp(0, 127) {
                    preview_key = Some(key.clamp(0, 127) as u8);
                }
                s.view.set_note(s.pattern, i, start, key, length);
                changed = true;
            }
        }
        if changed {
            s.mark_dirty();
        }
        if let Some(k) = preview_key
            && d.originals.len() == 1
        {
            s.preview_on(channel, k, 100);
        }
    }
    if released && st.drag.is_some() {
        if let Some(d) = st.drag.take()
            && let Some(&(i, _)) = d.originals.first()
            && let Some(n) = s.view.patterns[s.pattern].notes.get(i)
        {
            s.last_note_length = n.length;
        }
        s.preview_off();
    } else if released {
        s.preview_off();
    }

    // Selection box (⌘-drag on empty space).
    if let Some(anchor) = st.marquee {
        if let Some(p) = pointer {
            let r = Rect::from_two_pos(anchor, p);
            grid_painter.rect(r, 0.0, theme::alpha(theme::ACCENT, 0.12), Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
            if released || !primary_down {
                let hits: Vec<usize> = s.view.patterns[s.pattern]
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.channel == channel && note_rect(n, |t| tick_to_x(st, t), |k| key_to_y(st, k), st.key_h).intersects(r))
                    .map(|(i, _)| i)
                    .collect();
                if !st.marquee_add {
                    st.selected.clear();
                }
                for i in hits {
                    if !st.selected.contains(&i) {
                        st.selected.push(i);
                    }
                }
                st.marquee = None;
            }
        } else if released {
            st.marquee = None;
        }
    }

    // Keyboard shortcuts while the pointer is over the piano roll.
    if ui.rect_contains_pointer(rect) && !ui.ctx().egui_wants_keyboard_input() {
        keyboard_shortcuts(ui, s, st, channel, snap as u64);
    }

    // Keyboard.
    let keys_resp = ui.interact(keys, Id::new("piano-keys"), Sense::click_and_drag());
    let pressed_key = if keys_resp.hovered() || keys_resp.dragged() {
        ui.input(|i| i.pointer.latest_pos()).map(|p| y_to_key(st, p.y).clamp(0, 127) as u8)
    } else {
        None
    };
    if keys_resp.is_pointer_button_down_on()
        && let Some(p) = keys_resp.interact_pointer_pos()
    {
        let k = y_to_key(st, p.y).clamp(0, 127) as u8;
        if keys_resp.drag_started() || keys_resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
            s.preview_on(channel, k, 100);
        }
    }
    let key_painter = ui.painter_at(keys);

    // Pass 1: White keys (full width)
    for k in bottom_key..=top_key {
        if is_black(k as u8) {
            continue;
        }
        let y = key_to_y(st, k);
        let r = Rect::from_min_max(pos2(keys.min.x, y), pos2(keys.max.x, y + st.key_h));
        let is_pressed = pressed_key == Some(k as u8) && ui.input(|i| i.pointer.primary_down());
        let fill = if is_pressed {
            theme::ACCENT_HOT
        } else {
            Color32::from_rgb(0xD8, 0xDA, 0xE4)
        };
        key_painter.rect_filled(r, 0.0, fill);
        key_painter.line_segment([pos2(keys.min.x, r.max.y), pos2(keys.max.x, r.max.y)], Stroke::new(1.0, Color32::from_rgb(0x94, 0x96, 0xA2)));
        if k % 12 == 0 && st.key_h >= 8.0 {
            key_painter.text(pos2(keys.max.x - 5.0, r.center().y), Align2::RIGHT_CENTER, key_name(k as u8), theme::ui_font(9.0), Color32::from_rgb(0x1A, 0x1A, 0x26));
        }
    }

    // Pass 2: Black keys (raised, with drop shadow and bevel)
    for k in bottom_key..=top_key {
        if !is_black(k as u8) {
            continue;
        }
        let y = key_to_y(st, k);
        let r = Rect::from_min_max(pos2(keys.min.x, y), pos2(keys.max.x, y + st.key_h));
        let width = keys.width() * 0.64;
        let bk = Rect::from_min_size(r.min, vec2(width, st.key_h));
        let is_pressed = pressed_key == Some(k as u8) && ui.input(|i| i.pointer.primary_down());

        // Drop shadow below black key
        key_painter.rect_filled(Rect::from_min_size(pos2(bk.min.x, bk.max.y), vec2(width, 2.0)), 0.0, theme::alpha(Color32::BLACK, 0.40));

        let fill = if is_pressed {
            theme::ACCENT
        } else {
            Color32::from_rgb(0x18, 0x18, 0x22)
        };
        key_painter.rect(bk, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, fill, Stroke::new(1.0, Color32::from_rgb(0x0C, 0x0C, 0x12)), StrokeKind::Inside);
        // Top edge specular highlight
        key_painter.line_segment([pos2(bk.min.x, bk.min.y + 1.0), pos2(bk.max.x - 1.0, bk.min.y + 1.0)], Stroke::new(1.0, Color32::from_rgb(0x38, 0x38, 0x48)));
    }
    painter.line_segment([pos2(keys.max.x, rect.min.y), pos2(keys.max.x, rect.max.y)], Stroke::new(1.2, theme::LINE_STRONG));

    // Ruler with bar numbers.
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.max.x, ruler.max.y)), 0.0, theme::PANEL_2);
    let bars_first = (st.scroll_x / bar).floor() as i64;
    let bars_last = (x_to_tick(st, grid.max.x) / bar).ceil() as i64;
    for b in bars_first.max(0)..=bars_last {
        let x = tick_to_x(st, b as f64 * bar);
        if x >= ruler.min.x {
            painter.text(pos2(x + 4.0, ruler.center().y), Align2::LEFT_CENTER, (b + 1).to_string(), theme::mono(10.0), theme::TEXT_DIM);
        }
    }

    // Playhead (pattern mode plays this pattern).
    if s.mode == Mode::Pattern && s.is_playing() {
        let t = s.position_ticks() % pattern_len.max(1) as f64;
        let x = tick_to_x(st, t);
        if x >= grid.min.x && x <= grid.max.x {
            painter.line_segment([pos2(x, ruler.min.y), pos2(x, grid.max.y)], Stroke::new(1.5, theme::ACCENT));
        }
    }

    // Control lane: velocity, pan or modulation per note.
    painter.rect_filled(vel, 0.0, theme::PANEL_2);
    painter.rect_filled(vel_label, 0.0, theme::PANEL_2);
    let mut lane_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(vel_label.min + vec2(4.0, 4.0), vec2(vel_label.width() - 6.0, 22.0))));
    egui::ComboBox::from_id_salt("piano-lane").width(vel_label.width() - 14.0).selected_text(st.lane.label()).show_ui(&mut lane_ui, |ui| {
        for lane in Lane::ALL {
            ui.selectable_value(&mut st.lane, lane, lane.label());
        }
    });
    let lane = st.lane;
    let vel_painter = ui.painter_at(vel);
    let (bottom, height) = (vel.max.y - 4.0, vel.height() - 8.0);
    let centre_y = bottom - height / 2.0;
    if lane.bipolar() {
        vel_painter.line_segment([pos2(vel.min.x, centre_y), pos2(vel.max.x, centre_y)], Stroke::new(1.0, theme::LINE));
    }
    let notes = s.view.patterns[s.pattern].notes.clone();
    for n in notes.iter().filter(|n| n.channel == channel) {
        let x = tick_to_x(st, n.start as f64);
        let top = bottom - height * lane.get(n);
        let base = if lane.bipolar() { centre_y } else { bottom };
        vel_painter.line_segment([pos2(x + 1.0, base), pos2(x + 1.0, top)], Stroke::new(2.0, color));
        vel_painter.circle_filled(pos2(x + 1.0, top), 3.0, color);
    }
    let vel_resp = ui.interact(vel, Id::new("piano-velocity"), Sense::click_and_drag());
    if let Some(p) = vel_resp.interact_pointer_pos()
        && (vel_resp.dragged() || vel_resp.clicked())
    {
        if vel_resp.drag_started() || vel_resp.clicked() {
            s.checkpoint();
        }
        let t = x_to_tick(st, p.x);
        let v = ((bottom - p.y) / height).clamp(0.0, 1.0);
        let tolerance = (6.0 / st.px_per_tick) as f64;
        let p_notes = &mut s.view.patterns[s.pattern].notes;
        let mut changed = false;
        for n in p_notes.iter_mut().filter(|n| n.channel == channel) {
            let near = (n.start as f64 - t).abs() <= tolerance.max(1.0);
            if near && (lane.get(n) - v).abs() > 1e-3 {
                lane.set(n, v);
                changed = true;
            }
        }
        if changed {
            s.mark_dirty();
        }
    }
    vel_resp.on_hover_text(format!("{} lane: click or drag to set", lane.label()));

    ui.allocate_rect(rect, Sense::hover());
}

fn note_rect(n: &NoteView, tick_to_x: impl Fn(f64) -> f32, key_to_y: impl Fn(i32) -> f32, key_h: f32) -> Rect {
    let x0 = tick_to_x(n.start as f64);
    let x1 = tick_to_x((n.start + n.length) as f64);
    let y = key_to_y(n.key as i32);
    Rect::from_min_max(pos2(x0, y + 1.0), pos2(x1.max(x0 + 3.0), y + key_h - 1.0))
}

fn target_notes(s: &Session, st: &PianoState) -> Vec<usize> {
    if !st.selected.is_empty() {
        return st.selected.clone();
    }
    s.view.patterns.get(s.pattern).map(|p| p.notes.iter().enumerate().filter(|(_, n)| n.channel == s.channel).map(|(i, _)| i).collect()).unwrap_or_default()
}

/// Which note tool is being previewed, and the settings it was last applied with.
#[derive(Clone, Copy, PartialEq)]
enum PreviewTool {
    Strum(StrumOptions),
    Feel(f32),
}

struct ToolPreview {
    tool: PreviewTool,
    pattern: usize,
    /// The target notes as they were when the menu opened.
    originals: Vec<(usize, NoteView)>,
    targets: Vec<usize>,
}

/// Starts or updates a live preview: restores the original notes and re-applies `tool`.
fn preview_tool(s: &mut Session, st: &mut PianoState, tool: PreviewTool) {
    let same_kind = |a: &PreviewTool| matches!((a, &tool), (PreviewTool::Strum(_), PreviewTool::Strum(_)) | (PreviewTool::Feel(_), PreviewTool::Feel(_)));
    if st.preview.as_ref().is_some_and(|p| !same_kind(&p.tool) || p.pattern != s.pattern) {
        st.preview = None;
    }
    if st.preview.is_none() {
        let targets = target_notes(s, st);
        let Some(p) = s.view.patterns.get(s.pattern) else { return };
        let originals = targets.iter().filter_map(|&i| p.notes.get(i).map(|n| (i, *n))).collect();
        s.checkpoint();
        st.preview = Some(ToolPreview { tool, pattern: s.pattern, originals, targets });
        apply_preview(s, st, tool);
        return;
    }
    if st.preview.as_ref().is_some_and(|p| p.tool != tool) {
        apply_preview(s, st, tool);
    }
}

fn apply_preview(s: &mut Session, st: &mut PianoState, tool: PreviewTool) {
    let Some(pv) = st.preview.as_mut() else { return };
    pv.tool = tool;
    let (beat, bar) = (s.ppq(), s.ticks_per_bar());
    let Some(p) = s.view.patterns.get_mut(pv.pattern) else { return };
    for (i, n) in &pv.originals {
        if let Some(slot) = p.notes.get_mut(*i) {
            *slot = *n;
        }
    }
    match tool {
        PreviewTool::Strum(o) => p.strum_with(&pv.targets, o),
        PreviewTool::Feel(amount) if amount > 0.0 => p.piano_feel(&pv.targets, beat, bar, amount),
        PreviewTool::Feel(_) => {}
    }
    p.fit_length(bar);
    s.mark_dirty();
}

/// Puts the notes back as they were before the menu opened.
fn cancel_preview(s: &mut Session, st: &mut PianoState) {
    if let Some(pv) = st.preview.take()
        && let Some(p) = s.view.patterns.get_mut(pv.pattern)
    {
        for (i, n) in pv.originals {
            if let Some(slot) = p.notes.get_mut(i) {
                *slot = n;
            }
        }
        s.mark_dirty();
    }
}

fn apply_strum(s: &mut Session, st: &PianoState) {
    let targets = target_notes(s, st);
    if targets.is_empty() {
        return;
    }
    s.checkpoint();
    let bar = s.ticks_per_bar();
    if let Some(p) = s.view.patterns.get_mut(s.pattern) {
        p.strum_with(&targets, st.strum);
        p.fit_length(bar);
    }
    s.mark_dirty();
}

fn insert_note(s: &mut Session, st: &mut PianoState, start: u64, key: u8, channel: usize, length: u64) -> Option<usize> {
    let note = NoteView { start, length: length.max(1), key, velocity: 100, channel, pan: 64, mod_x: 128, mod_y: 128, slide: false };
    let index = s.view.add_note(s.pattern, note)?;
    st.selected = vec![index];
    if let Some(chord) = st.chord {
        for &interval in &chord.intervals()[1..] {
            if let Some(k) = key.checked_add(interval).filter(|k| *k <= 127)
                && let Some(i) = s.view.add_note(s.pattern, NoteView { key: k, ..note })
            {
                st.selected.push(i);
            }
        }
    }
    if st.natural_velocities {
        let (beat, bar) = (s.ppq(), s.ticks_per_bar());
        s.view.patterns[s.pattern].piano_feel(&st.selected, beat, bar, st.feel_amount);
    }
    s.mark_dirty();
    Some(index)
}

fn toolbar(ui: &mut Ui, s: &mut Session, st: &mut PianoState) {
    ui.horizontal_wrapped(|ui| {
        for (tool, label, width, tip) in [
            (Tool::Draw, "Draw", 54.0, "Draw (P): add, move or resize notes · Shift-drag to select"),
            (Tool::Paint, "Paint", 54.0, "Paint (B): drag across the grid to add notes"),
            (Tool::Select, "Select", 58.0, "Select (E or S): drag a box; Shift adds to the selection"),
            (Tool::Slice, "Slice", 54.0, "Slice (C): split a note at the snap grid"),
            (Tool::Delete, "Delete", 58.0, "Delete (D): erase notes"),
        ] {
            if widgets::toggle(ui, st.tool == tool, label, vec2(width, 22.0)).on_hover_text(tip).clicked() {
                st.tool = tool;
            }
        }
        ui.add_space(8.0);
        // Strum and Piano feel preview live while their menus are open: every change is
        // applied fresh to the notes as they were when the menu opened. Accept keeps it,
        // Cancel puts the notes back (closing the menu keeps what you hear).
        let strum_menu = ui.menu_button("Strum", |ui| {
            ui.label("Stagger chord notes (live preview)");
            ui.add(egui::Slider::new(&mut st.strum.spread, 0..=48).text("Ticks per note"));
            ui.add(egui::Slider::new(&mut st.strum.time_tension, -1.0..=1.0).text("Timing curve"));
            ui.add(egui::Slider::new(&mut st.strum.velocity_falloff, -32..=32).text("Velocity change"));
            ui.add(egui::Slider::new(&mut st.strum.velocity_tension, -1.0..=1.0).text("Velocity curve"));
            ui.add(egui::Slider::new(&mut st.strum.chord_window, 0..=24).text("Chord window (ticks)")).on_hover_text("Notes starting this close together count as one chord");
            ui.checkbox(&mut st.strum.reverse, "High notes first");
            ui.checkbox(&mut st.strum.alternate, "Alternate direction");
            ui.checkbox(&mut st.strum.trigger_ahead, "Straddle chord start");
            ui.checkbox(&mut st.strum.preserve_end, "Preserve note ends");
            preview_tool(s, st, PreviewTool::Strum(st.strum));
            ui.horizontal(|ui| {
                if ui.button("Accept").clicked() {
                    st.preview = None;
                    ui.close();
                }
                if ui.button("Cancel").clicked() {
                    cancel_preview(s, st);
                    ui.close();
                }
            });
        });
        strum_menu.response.on_hover_text("Strum selected chords, or all notes in this channel (⌥S applies the current settings)");
        let feel_menu = ui.menu_button("Piano feel", |ui| {
            ui.checkbox(&mut st.natural_velocities, "Vary new note velocities");
            ui.add(egui::Slider::new(&mut st.feel_amount, 0.0..=1.0).text("Amount").custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            preview_tool(s, st, PreviewTool::Feel(st.feel_amount));
            ui.horizontal(|ui| {
                if ui.button("Accept").clicked() {
                    st.preview = None;
                    ui.close();
                }
                if ui.button("Cancel").clicked() {
                    cancel_preview(s, st);
                    ui.close();
                }
            });
            ui.label("Adds subtle key, beat and chord-voice variation to the selected notes (or the channel's notes).");
        });
        feel_menu.response.on_hover_text("Make a piano performance less uniform");
        // Menu closed without a button: keep the previewed result.
        if strum_menu.inner.is_none() && feel_menu.inner.is_none() {
            st.preview = None;
        }
        ui.menu_button("Tools", |ui| {
            let snap = s.snap_ticks().max(1);
            let channel = s.channel;
            let targets: Vec<usize> = if st.selected.is_empty() {
                s.view.patterns.get(s.pattern).map(|p| p.notes.iter().enumerate().filter(|(_, n)| n.channel == channel).map(|(i, _)| i).collect()).unwrap_or_default()
            } else {
                st.selected.clone()
            };
            type PatternEdit = Box<dyn FnOnce(&mut engine::view::PatternView) -> Vec<usize>>;
            let mut op: Option<PatternEdit> = None;
            if ui.button("Toggle slide").on_hover_text("Slide notes glide the channel's playing notes to their pitch instead of starting new notes (KVNG Synth and sampler channels)").clicked() {
                let t = targets.clone();
                op = Some(Box::new(move |p| {
                    let make = t.iter().any(|&i| p.notes.get(i).is_some_and(|n| !n.slide));
                    for &i in &t {
                        if let Some(n) = p.notes.get_mut(i) {
                            n.slide = make;
                        }
                    }
                    t
                }));
            }
            if ui.button("Flip in time").on_hover_text("Play the selection backwards").clicked() {
                let t = targets.clone();
                op = Some(Box::new(move |p| {
                    p.flip_time(&t);
                    t
                }));
            }
            if ui.button("Flip pitch").on_hover_text("Turn the melody upside down").clicked() {
                let t = targets.clone();
                op = Some(Box::new(move |p| {
                    p.flip_pitch(&t);
                    t
                }));
            }
            ui.menu_button("Randomize", |ui| {
                ui.add(egui::Slider::new(&mut st.rand_keys, 0..=12).text("Pitch ± semitones"));
                ui.add(egui::Slider::new(&mut st.rand_vel, 0..=64).text("Velocity ±"));
                ui.add_enabled(st.scale.is_some(), egui::Checkbox::new(&mut st.rand_scale, "Keep notes in the scale"));
                if ui.button("Randomize").clicked() {
                    let (t, k, v) = (targets.clone(), st.rand_keys, st.rand_vel);
                    let scale = st.scale.filter(|_| st.rand_scale).map(|(r, sc)| (r, sc.intervals()));
                    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
                    op = Some(Box::new(move |p| {
                        p.randomize(&t, k, v, scale, seed);
                        t
                    }));
                    ui.close();
                }
            });
            ui.menu_button("LFO", |ui| {
                let (lane, period, depth, centre, shape) = &mut st.lfo;
                egui::ComboBox::from_id_salt("lfo-lane").selected_text(["Velocity", "Pan", "Mod X", "Mod Y"][*lane as usize % 4]).show_ui(ui, |ui| {
                    for (i, l) in ["Velocity", "Pan", "Mod X", "Mod Y"].iter().enumerate() {
                        ui.selectable_value(lane, i as u8, *l);
                    }
                });
                egui::ComboBox::from_id_salt("lfo-shape").selected_text(["Sine", "Triangle", "Saw", "Square"][*shape as usize % 4]).show_ui(ui, |ui| {
                    for (i, l) in ["Sine", "Triangle", "Saw", "Square"].iter().enumerate() {
                        ui.selectable_value(shape, i as u8, *l);
                    }
                });
                ui.add(egui::Slider::new(period, 0.25..=16.0).logarithmic(true).text("Period (beats)"));
                ui.add(egui::Slider::new(depth, 0.0..=1.0).text("Depth"));
                ui.add(egui::Slider::new(centre, 0.0..=1.0).text("Centre"));
                if ui.button("Apply LFO").clicked() {
                    let (t, l, per, d, c, sh) = (targets.clone(), *lane, (*period as f64 * s.ppq() as f64) as u64, *depth, *centre, *shape);
                    op = Some(Box::new(move |p| {
                        p.lfo(&t, l, per, d, c, sh, 0.0);
                        t
                    }));
                    ui.close();
                }
            });
            ui.separator();
            ui.menu_button("Quantize", |ui| {
                ui.add(egui::Slider::new(&mut st.quant_strength, 0.0..=1.0).text("Strength").custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                ui.checkbox(&mut st.quant_ends, "Quantize note ends too");
                if ui.button("Quantize to snap (Q)").clicked() {
                    let (t, k, ends) = (targets.clone(), st.quant_strength, st.quant_ends);
                    op = Some(Box::new(move |p| {
                        p.quantize(&t, snap, k, ends);
                        t
                    }));
                    ui.close();
                }
            });
            ui.menu_button("Humanize", |ui| {
                ui.add(egui::Slider::new(&mut st.human_time, 0..=48).text("Timing ± ticks"));
                ui.add(egui::Slider::new(&mut st.human_vel, 0..=64).text("Velocity ±"));
                if ui.button("Humanize").clicked() {
                    let (t, ht, hv) = (targets.clone(), st.human_time, st.human_vel);
                    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
                    op = Some(Box::new(move |p| {
                        p.humanize(&t, ht, hv, seed);
                        t
                    }));
                    ui.close();
                }
            });
            if let Some((root, scale)) = st.scale {
                if ui.button(format!("Snap to {} {}", ROOTS[root as usize], scale.label())).on_hover_text("Move notes outside the highlighted scale to the nearest scale note").clicked() {
                    let t = targets.clone();
                    op = Some(Box::new(move |p| {
                        p.snap_to_scale(&t, root, scale.intervals());
                        t
                    }));
                }
            } else {
                ui.add_enabled(false, egui::Button::new("Snap to scale")).on_disabled_hover_text("Choose a scale (Scale menu) first");
            }
            ui.separator();
            if op.is_some() {
            } else if ui.button("Chop (to snap)").clicked() {
                op = Some(Box::new(move |p| p.chop(&targets, snap)));
            } else if ui.button("Glue").clicked() {
                op = Some(Box::new(move |p| p.glue(&targets)));
            } else {
                for (label, mode) in [("Arpeggiate up", ArpMode::Up), ("Arpeggiate down", ArpMode::Down), ("Arpeggiate up-down", ArpMode::UpDown)] {
                    if ui.button(label).clicked() {
                        let t = targets.clone();
                        op = Some(Box::new(move |p| p.arpeggiate(&t, snap, mode)));
                    }
                }
            }
            if let Some(f) = op {
                s.checkpoint();
                let ticks_per_bar = s.ticks_per_bar();
                if let Some(p) = s.view.patterns.get_mut(s.pattern) {
                    st.selected = f(p);
                    p.fit_length(ticks_per_bar);
                }
                s.mark_dirty();
                ui.close();
            }
        })
        .response
        .on_hover_text("Applies to the selected notes, or all of this channel's notes");
        let chord_text = st.chord.map_or("Chord: off".to_string(), |c| format!("Chord: {}", c.label()));
        egui::ComboBox::from_id_salt("piano-chord").width(130.0).selected_text(chord_text).show_ui(ui, |ui| {
            if ui.selectable_label(st.chord.is_none(), "Off (single notes)").clicked() {
                st.chord = None;
            }
            for c in engine::view::Chord::ALL {
                if ui.selectable_label(st.chord == Some(c), c.label()).clicked() {
                    st.chord = Some(c);
                }
            }
        })
        .response
        .on_hover_text("Chord stamp: each note you draw becomes this chord");
        let scale_text = st.scale.map_or("Scale: off".to_string(), |(r, sc)| format!("{} {}", ROOTS[r as usize], sc.label()));
        egui::ComboBox::from_id_salt("piano-scale").width(130.0).selected_text(scale_text).show_ui(ui, |ui| {
            if ui.selectable_label(st.scale.is_none(), "Off").clicked() {
                st.scale = None;
            }
            for (ri, root) in ROOTS.iter().enumerate() {
                ui.menu_button(*root, |ui| {
                    for sc in Scale::ALL {
                        if ui.selectable_label(st.scale == Some((ri as u8, sc)), sc.label()).clicked() {
                            st.scale = Some((ri as u8, sc));
                            ui.close();
                        }
                    }
                });
            }
        });
        ui.add_space(8.0);
        transport::snap_picker(ui, s, "piano-snap");
        ui.add_space(8.0);
        let current = s.view.channels.get(s.channel).map(|c| c.name.clone()).unwrap_or_default();
        egui::ComboBox::from_id_salt("piano-channel").width(160.0).selected_text(format!("Channel: {current}")).show_ui(ui, |ui| {
            for (i, c) in s.view.channels.iter().enumerate() {
                if ui.selectable_label(i == s.channel, &c.name).clicked() {
                    s.channel = i;
                }
            }
        });
        ui.add_space(8.0);
        transport::pattern_picker(ui, s, "piano-pattern");
    });
}

/// Begins moving/resizing the selection, grabbed at note `grabbed`.
fn start_drag(st: &mut PianoState, s: &Session, kind: DragKind, tick: f64, key: i32, grabbed: usize) {
    let notes = &s.view.patterns[s.pattern].notes;
    let Some(g) = notes.get(grabbed) else { return };
    let _ = g;
    let (grab_tick, grab_key) = (tick, key);
    let originals = st.selected.iter().filter_map(|&i| notes.get(i).map(|n| (i, *n))).collect();
    st.drag = Some(NoteDrag { kind, grab_tick, grab_key, originals });
}

fn keyboard_shortcuts(ui: &Ui, s: &mut Session, st: &mut PianoState, channel: usize, snap: u64) {
    let (cmd, shift, alt) = ui.input(|i| (i.modifiers.command, i.modifiers.shift, i.modifiers.alt));
    let pressed = |k: egui::Key| ui.input(|i| i.key_pressed(k));
    let pattern = s.pattern;

    if !cmd && !alt {
        for (key, tool) in [
            (egui::Key::P, Tool::Draw),
            (egui::Key::B, Tool::Paint),
            (egui::Key::E, Tool::Select),
            (egui::Key::S, Tool::Select),
            (egui::Key::C, Tool::Slice),
            (egui::Key::D, Tool::Delete),
        ] {
            if pressed(key) {
                st.tool = tool;
            }
        }
    }
    if alt && !cmd && pressed(egui::Key::S) {
        apply_strum(s, st);
    }
    if cmd && pressed(egui::Key::A) {
        st.selected = s.view.patterns[pattern].notes.iter().enumerate().filter(|(_, n)| n.channel == channel).map(|(i, _)| i).collect();
    }
    if pressed(egui::Key::Escape) {
        st.selected.clear();
    }
    if (pressed(egui::Key::Delete) || pressed(egui::Key::Backspace)) && !st.selected.is_empty() {
        s.checkpoint();
        let mut sel = std::mem::take(&mut st.selected);
        sel.sort_unstable();
        for i in sel.into_iter().rev() {
            s.view.remove_note(pattern, i);
        }
        s.mark_dirty();
    }
    if cmd && pressed(egui::Key::C) && !st.selected.is_empty() {
        let notes = &s.view.patterns[pattern].notes;
        st.clipboard = st.selected.iter().filter_map(|&i| notes.get(i).copied()).collect();
    }
    let duplicate = cmd && pressed(egui::Key::B) && !st.selected.is_empty();
    if duplicate {
        let notes = &s.view.patterns[pattern].notes;
        st.clipboard = st.selected.iter().filter_map(|&i| notes.get(i).copied()).collect();
    }
    if (cmd && pressed(egui::Key::V) || duplicate) && !st.clipboard.is_empty() {
        // Paste right after the copied block, snapped to the bar grid FL uses for duplicates.
        let first = st.clipboard.iter().map(|n| n.start).min().unwrap_or(0);
        let last = st.clipboard.iter().map(|n| n.start + n.length).max().unwrap_or(0);
        let bar = s.ticks_per_bar();
        let span = (last - first).div_ceil(snap.max(1)) * snap.max(1);
        let span = if duplicate || span.is_multiple_of(bar) { span.max(snap) } else { span.div_ceil(bar) * bar };
        s.checkpoint();
        let mut new_sel = Vec::new();
        for n in st.clipboard.clone() {
            let note = NoteView { start: n.start + span, channel, ..n };
            if let Some(i) = s.view.add_note(pattern, note) {
                new_sel.push(i);
            }
        }
        st.clipboard = new_sel.iter().filter_map(|&i| s.view.patterns[pattern].notes.get(i).copied()).collect();
        st.selected = new_sel;
        s.mark_dirty();
    }
    // Quantize starts to the snap grid (selection, or every note of the channel).
    if pressed(egui::Key::Q) && !cmd {
        s.checkpoint();
        let targets: Vec<usize> = if st.selected.is_empty() {
            s.view.patterns[pattern].notes.iter().enumerate().filter(|(_, n)| n.channel == channel).map(|(i, _)| i).collect()
        } else {
            st.selected.clone()
        };
        let (k, ends) = (st.quant_strength, st.quant_ends);
        s.view.patterns[pattern].quantize(&targets, snap.max(1), k, ends);
        s.mark_dirty();
    }
    // Transpose and nudge the selection.
    let (dk, dt) = (
        if pressed(egui::Key::ArrowUp) { if shift { 12 } else { 1 } } else if pressed(egui::Key::ArrowDown) { if shift { -12 } else { -1 } } else { 0 },
        if pressed(egui::Key::ArrowRight) { snap as i64 } else if pressed(egui::Key::ArrowLeft) { -(snap as i64) } else { 0 },
    );
    if (dk != 0 || dt != 0) && !st.selected.is_empty() {
        s.checkpoint();
        for &i in &st.selected {
            if let Some(n) = s.view.patterns[pattern].notes.get(i).copied() {
                s.view.set_note(pattern, i, n.start as i64 + dt, n.key as i32 + dk, n.length as i64);
            }
        }
        s.mark_dirty();
    }
}
