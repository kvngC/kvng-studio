//! The playlist: arrangement of pattern and audio clips over playlist tracks.

use eframe::egui::{self, Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::view::{ClipKind, ClipView, WAVEFORM_STEP};

use super::session::{Mode, Session};
use super::{theme, widgets};

const HEADER_W: f32 = 210.0;
const RULER_H: f32 = 26.0;
const TRACK_H: f32 = 46.0;
const CLIP_TITLE_H: f32 = 14.0;
/// Pixels from a clip's right edge that grab it for resizing.
const RESIZE_GRIP: f32 = 7.0;

pub struct PlaylistState {
    pub px_per_tick: f32,
    /// Leftmost visible tick.
    pub scroll_x: f64,
    pub scroll_y: f32,
    pub follow: bool,
    /// Selected clips (indices into the view's clips).
    pub selected: Vec<usize>,
    clipboard: Vec<ClipView>,
    /// Slice tool: clicking a clip splits it.
    pub slice: bool,
    marquee: Option<egui::Pos2>,
    /// Track being renamed and the text typed so far.
    renaming: Option<(usize, String)>,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            px_per_tick: 0.22,
            scroll_x: 0.0,
            scroll_y: 0.0,
            follow: true,
            selected: Vec::new(),
            clipboard: Vec::new(),
            slice: false,
            marquee: None,
            renaming: None,
        }
    }
}

#[derive(Clone, Copy)]
enum DragKind {
    Move,
    /// Right edge.
    Resize,
    /// Left edge: trims the start (the clip's content stays where it is).
    ResizeLeft,
    /// Alt-drag inside an audio clip: moves the audio within the clip.
    Slip,
    FadeIn,
    FadeOut,
}

/// Grab distance around audio clip fade handles.
const FADE_GRIP: f32 = 6.0;

/// Where an audio clip's fade handles are (fade-in, fade-out), in screen space.
fn fade_handles(clip: &ClipView, r: Rect, px_per_tick: f32) -> (Pos2, Pos2) {
    let y = r.min.y + CLIP_TITLE_H + 3.0;
    let fi = (r.min.x + clip.fade_in as f32 * px_per_tick).clamp(r.min.x + 3.0, r.max.x - 3.0);
    let fo = (r.max.x - clip.fade_out as f32 * px_per_tick).clamp(r.min.x + 3.0, r.max.x - 3.0);
    (pos2(fi, y), pos2(fo, y))
}

/// Ticks → the clip's source units (seconds for audio, ticks for patterns/automation).
fn source_units(s: &Session, clip: &ClipView, ticks: f64) -> f64 {
    match clip.kind {
        ClipKind::Audio => ticks * s.view.samples_per_tick / s.view.sample_rate,
        _ => ticks,
    }
}

/// Moving or resizing clips; `originals` are the selected clips as they were.
#[derive(Clone)]
struct ClipDrag {
    kind: DragKind,
    /// Pointer tick and track row when the drag started.
    grab_tick: f64,
    grab_row: i64,
    originals: Vec<(usize, ClipView)>,
}

#[derive(Default)]
pub struct Outcome {
    /// A pattern clip was double-clicked: open it in the piano roll.
    pub open_pattern: Option<usize>,
    /// An automation clip was double-clicked: open its editor.
    pub open_automation: Option<usize>,
    /// A browser file was dropped: (file, track, start tick).
    pub dropped: Option<(std::path::PathBuf, usize, u64)>,
    /// A status message (e.g. after stretching a clip).
    pub message: Option<String>,
}

pub fn show(ui: &mut Ui, s: &mut Session, st: &mut PlaylistState) -> Outcome {
    let rect = ui.available_rect_before_wrap();
    let body = Rect::from_min_max(pos2(rect.min.x + HEADER_W, rect.min.y + RULER_H), rect.max);
    let ruler = Rect::from_min_max(pos2(body.min.x, rect.min.y), pos2(rect.max.x, rect.min.y + RULER_H));
    let headers = Rect::from_min_max(pos2(rect.min.x, body.min.y), pos2(body.min.x, rect.max.y));
    let mut open_pattern = None;
    let mut open_automation = None;
    let mut stretch_message: Option<String> = None;

    let tick_to_x = |st: &PlaylistState, t: f64| body.min.x + ((t - st.scroll_x) as f32) * st.px_per_tick;
    let x_to_tick = |st: &PlaylistState, x: f32| st.scroll_x + ((x - body.min.x) / st.px_per_tick) as f64;
    // Tracks inside collapsed folders are hidden: map tracks to on-screen rows and back.
    let hidden = s.view.hidden_tracks();
    let folders = s.view.track_folders();
    let mut vis_row: Vec<Option<usize>> = Vec::with_capacity(hidden.len());
    let mut row_track: Vec<usize> = Vec::new();
    for (i, h) in hidden.iter().enumerate() {
        if *h {
            vis_row.push(None);
        } else {
            vis_row.push(Some(row_track.len()));
            row_track.push(i);
        }
    }
    let visible_rows = row_track.len().max(1);
    // Hidden tracks are placed far below the view so everything skips them.
    let track_y = |st: &PlaylistState, i: usize| body.min.y + vis_row.get(i).copied().flatten().map_or(1.0e7, |r| r as f32 * TRACK_H) - st.scroll_y;
    let row_to_track = |row: i64| -> usize { row_track.get(row.clamp(0, row_track.len() as i64 - 1).max(0) as usize).copied().unwrap_or(0) };

    // Scrolling and zooming.
    let hover_body = ui.rect_contains_pointer(Rect::from_min_max(pos2(rect.min.x, body.min.y), rect.max));
    if hover_body || ui.rect_contains_pointer(ruler) {
        let (scroll, zoom, cmd, shift, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command, i.modifiers.shift, i.pointer.hover_pos()));
        let anchor_x = pointer.map_or(body.center().x, |p| p.x.max(body.min.x));
        let zoom_by = |st: &mut PlaylistState, factor: f32| {
            let anchor = x_to_tick(st, anchor_x);
            st.px_per_tick = (st.px_per_tick * factor).clamp(0.01, 4.0);
            st.scroll_x = (anchor - ((anchor_x - body.min.x) / st.px_per_tick) as f64).max(0.0);
        };
        if zoom != 1.0 {
            zoom_by(st, zoom);
        } else if cmd && scroll.y != 0.0 {
            zoom_by(st, (scroll.y * 0.01).exp());
        } else {
            let (dx, dy) = if shift { (scroll.y, scroll.x) } else { (scroll.x, scroll.y) };
            if dx != 0.0 {
                st.scroll_x = (st.scroll_x - (dx / st.px_per_tick) as f64).max(0.0);
                st.follow = false;
            }
            let max_y = (visible_rows as f32 * TRACK_H - body.height() + TRACK_H).max(0.0);
            st.scroll_y = (st.scroll_y - dy).clamp(0.0, max_y);
        }
    }

    // Keep the playhead in view while playing.
    let playhead = s.position_ticks();
    if s.is_playing() && s.mode == Mode::Song && st.follow {
        let x = tick_to_x(st, playhead);
        if x > body.max.x - body.width() * 0.1 || x < body.min.x {
            st.scroll_x = (playhead - (body.width() * 0.1 / st.px_per_tick) as f64).max(0.0);
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);

    // Track lanes.
    let lane_mutes = s.view.effective_mutes();
    for (i, track) in s.view.tracks.iter().enumerate() {
        let y = track_y(st, i);
        if y + TRACK_H < body.min.y || y > body.max.y {
            continue;
        }
        let lane = Rect::from_min_max(pos2(body.min.x, y), pos2(body.max.x, y + TRACK_H));
        painter.rect_filled(lane, 0.0, if i % 2 == 0 { theme::PANEL } else { theme::mix(theme::PANEL, theme::BG, 0.4) });
        if lane_mutes.get(i).copied().unwrap_or(track.muted) {
            painter.rect_filled(lane, 0.0, theme::alpha(theme::BG, 0.45));
        }
        painter.line_segment([pos2(body.min.x, y + TRACK_H), pos2(body.max.x, y + TRACK_H)], Stroke::new(1.0, theme::LINE_SOFT));
    }

    // Grid.
    let bar = s.ticks_per_bar() as f64;
    let beat = s.ppq() as f64;
    // Bars follow time-signature changes, so they're laid out with the view's bar map.
    let first = s.view.bar_at(st.scroll_x.max(0.0) as u64).0 as i64;
    let last = s.view.bar_at(x_to_tick(st, body.max.x).max(0.0) as u64).0 as i64 + 1;
    let show_beats = beat as f32 * st.px_per_tick > 10.0;
    for b in first.max(0)..=last {
        let start = s.view.bar_start(b as u64) as f64;
        let x = tick_to_x(st, start);
        painter.line_segment([pos2(x, body.min.y), pos2(x, body.max.y)], Stroke::new(1.2, Color32::from_rgb(0x4C, 0x4C, 0x62)));
        if show_beats {
            for k in 1..s.view.beats_in_bar(b as u64) {
                let bx = tick_to_x(st, start + k as f64 * beat);
                painter.line_segment([pos2(bx, body.min.y), pos2(bx, body.max.y)], Stroke::new(1.0, theme::LINE));
            }
        }
    }

    // Clicking an empty spot places the selected pattern there (FL's draw tool).
    let body_resp = ui.interact(body, Id::new("playlist-body"), Sense::click_and_drag());
    let clip_count = s.view.clips.len();
    st.selected.retain(|&i| i < clip_count);
    let snap = s.snap_ticks().max(1) as f64;
    let row_at = |st: &PlaylistState, y: f32| ((y - body.min.y + st.scroll_y) / TRACK_H).floor() as i64;

    // Clips.
    let drag_id = Id::new("playlist-clip-drag");
    let mut drag: Option<ClipDrag> = ui.data(|d| d.get_temp(drag_id));
    let (pointer, shift, cmd) = ui.input(|i| (i.pointer.hover_pos(), i.modifiers.shift, i.modifiers.command));
    let mut delete = None;
    let mut clip_menu: Option<(usize, ClipMenu)> = None;
    let mut select = None;
    let mut slice_at: Option<(usize, u64)> = None;
    let mut start_drag: Option<(usize, DragKind)> = None;
    let clip_painter = ui.painter_at(body);
    for (ci, clip) in s.view.clips.iter().enumerate() {
        let x0 = tick_to_x(st, clip.start as f64);
        let x1 = tick_to_x(st, (clip.start + clip.length) as f64);
        let y = track_y(st, clip.track);
        if x1 < body.min.x || x0 > body.max.x || y + TRACK_H < body.min.y || y > body.max.y {
            continue;
        }
        let r = Rect::from_min_max(pos2(x0, y + 1.0), pos2(x1.max(x0 + 3.0), y + TRACK_H - 1.0));
        let muted = s.view.tracks.get(clip.track).is_some_and(|t| t.muted);
        draw_clip(&clip_painter, s, clip, r, muted, st.selected.contains(&ci));

        let resp = ui.interact(r.intersect(body), Id::new(("clip", ci)), Sense::click_and_drag());
        let near_edge = pointer.is_some_and(|p| p.x > r.max.x - RESIZE_GRIP);
        let near_left = pointer.is_some_and(|p| p.x < r.min.x + RESIZE_GRIP && p.y > r.min.y + CLIP_TITLE_H);
        let is_audio = clip.kind == ClipKind::Audio;
        let (fi, fo) = fade_handles(clip, r, st.px_per_tick);
        let on_fade = |h: Pos2| is_audio && r.width() > 24.0 && pointer.is_some_and(|p| p.distance(h) <= FADE_GRIP);
        let (near_fade_in, near_fade_out) = (on_fade(fi), on_fade(fo));
        let alt = ui.input(|i| i.modifiers.alt);
        if resp.hovered() {
            let icon = if st.slice {
                CursorIcon::VerticalText
            } else if near_fade_in || near_fade_out {
                CursorIcon::PointingHand
            } else if near_edge || near_left {
                CursorIcon::ResizeHorizontal
            } else if alt && is_audio {
                CursorIcon::AllScroll
            } else {
                CursorIcon::Grab
            };
            ui.ctx().set_cursor_icon(icon);
            if st.slice && let Some(p) = pointer {
                clip_painter.line_segment([pos2(p.x, r.min.y), pos2(p.x, r.max.y)], Stroke::new(1.5, theme::ACCENT_HOT));
            }
        }
        if resp.clicked() {
            if st.slice {
                if let Some(p) = pointer {
                    let t = ((x_to_tick(st, p.x) / snap).round() * snap) as u64;
                    slice_at = Some((ci, t));
                }
            } else if shift {
                if let Some(pos) = st.selected.iter().position(|&i| i == ci) {
                    st.selected.remove(pos);
                } else {
                    st.selected.push(ci);
                }
            } else {
                st.selected = vec![ci];
                select = clip.pattern;
            }
        }
        if resp.double_clicked() && !st.slice {
            open_pattern = clip.pattern;
            if clip.automation.is_some() {
                open_automation = clip.automation;
            }
        }
        if resp.secondary_clicked() {
            delete = Some(ci);
        }
        // Clip menu (▾ at the start of the title strip).
        let menu_rect = Rect::from_min_size(r.min, vec2(14.0, CLIP_TITLE_H)).intersect(body);
        if r.width() > 30.0 && menu_rect.width() > 0.0 {
            let menu_resp = ui.interact(menu_rect, Id::new(("clip-menu", ci)), Sense::click());
            clip_painter.text(menu_rect.center(), Align2::CENTER_CENTER, "▾", theme::ui_font(10.0), theme::BG);
            egui::Popup::menu(&menu_resp).show(|ui| {
                ui.label(egui::RichText::new("Colour").small().color(theme::TEXT_DIM));
                ui.horizontal(|ui| {
                    for c in theme::CLIP_PALETTE {
                        let (sw, swr) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().rect_filled(sw, 3.0, c);
                        if swr.clicked() {
                            clip_menu = Some((ci, ClipMenu::Color(Some([c.r(), c.g(), c.b()]))));
                            ui.close();
                        }
                    }
                });
                if ui.button("Default colour").clicked() {
                    clip_menu = Some((ci, ClipMenu::Color(None)));
                    ui.close();
                }
                if clip.kind == ClipKind::Audio {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Gain");
                        let mut db = if clip.gain <= 1e-5 { -60.0 } else { 20.0 * clip.gain.log10() };
                        if ui.add(egui::DragValue::new(&mut db).range(-60.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB")).changed() {
                            clip_menu = Some((ci, ClipMenu::Gain(10f32.powf(db / 20.0))));
                        }
                    });
                    let mut rev = clip.reverse;
                    if ui.checkbox(&mut rev, "Reverse").clicked() {
                        clip_menu = Some((ci, ClipMenu::Reverse(rev)));
                    }
                    ui.menu_button("Stretch", |ui| {
                        let id = Id::new(("clip-orig-bpm", ci));
                        let mut bpm: f64 = ui.data_mut(|d| d.get_temp(id)).unwrap_or(s.view.tempo);
                        ui.horizontal(|ui| {
                            ui.label("Original tempo");
                            ui.add(egui::DragValue::new(&mut bpm).range(20.0..=400.0).speed(0.1).fixed_decimals(2).suffix(" BPM"));
                        });
                        ui.data_mut(|d| d.insert_temp(id, bpm));
                        if ui.button(format!("Stretch to project tempo ({:.2} BPM)", s.view.tempo)).on_hover_text("Changes the length to match the project's tempo; the pitch stays the same").clicked() {
                            clip_menu = Some((ci, ClipMenu::Stretch(bpm / s.view.tempo.max(1.0))));
                            ui.close();
                        }
                        ui.separator();
                        let bars_id = Id::new(("clip-fit-bars", ci));
                        let mut bars: u32 = ui.data_mut(|d| d.get_temp(bars_id)).unwrap_or(4);
                        ui.horizontal(|ui| {
                            ui.label("Fit to");
                            ui.add(egui::DragValue::new(&mut bars).range(1..=256).suffix(" bar(s)"));
                        });
                        ui.data_mut(|d| d.insert_temp(bars_id, bars));
                        if ui.button("Fit clip").on_hover_text("Stretch the whole clip to exactly this many bars").clicked() {
                            let target = bars as f64 * s.ticks_per_bar() as f64;
                            clip_menu = Some((ci, ClipMenu::Stretch(target / clip.length.max(1) as f64)));
                            ui.close();
                        }
                    });
                    if (clip.fade_in > 0 || clip.fade_out > 0) && ui.button("Remove fades").clicked() {
                        clip_menu = Some((ci, ClipMenu::ResetFades));
                        ui.close();
                    }
                    ui.label(egui::RichText::new("Drag the corner handles for fades · Alt-drag to slip the audio · drag either edge to trim").small().color(theme::TEXT_FAINT));
                    ui.separator();
                }
                if clip.pattern.is_some() && ui.button("Make unique").on_hover_text("Give this clip its own copy of the pattern").clicked() {
                    clip_menu = Some((ci, ClipMenu::MakeUnique));
                    ui.close();
                }
                if ui.button(egui::RichText::new("Delete").color(theme::ERROR)).clicked() {
                    clip_menu = Some((ci, ClipMenu::Delete));
                    ui.close();
                }
            });
        }
        if resp.drag_started() && !st.slice {
            let kind = if near_fade_in {
                DragKind::FadeIn
            } else if near_fade_out {
                DragKind::FadeOut
            } else if near_edge {
                DragKind::Resize
            } else if near_left {
                DragKind::ResizeLeft
            } else if alt && is_audio {
                DragKind::Slip
            } else {
                DragKind::Move
            };
            start_drag = Some((ci, kind));
        }
    }
    if let Some(p) = select {
        s.select_pattern(p);
    }
    if let (Some((ci, kind)), Some(p)) = (start_drag, pointer) {
        if !st.selected.contains(&ci) {
            st.selected = vec![ci];
        }
        s.checkpoint();
        drag = Some(ClipDrag {
            kind,
            grab_tick: x_to_tick(st, p.x),
            grab_row: row_at(st, p.y),
            originals: st.selected.iter().filter_map(|&i| s.view.clips.get(i).map(|c| (i, c.clone()))).collect(),
        });
    }
    if let Some((ci, t)) = slice_at {
        s.checkpoint();
        if split_clip(&mut s.view, ci, t) {
            st.selected.clear();
            s.mark_dirty();
        }
    }

    let mut dropped = None;
    if let Some(file) = body_resp.dnd_release_payload::<super::browser::DraggedFile>()
        && let Some(p) = pointer
    {
        let start = ((x_to_tick(st, p.x) / snap).floor() * snap).max(0.0) as u64;
        let track = row_to_track(row_at(st, p.y));
        dropped = Some((file.0.clone(), track, start));
    } else if body_resp.drag_started() && (cmd || shift) {
        // Shift-drag (or ⌘-drag) on empty space draws a selection box.
        st.marquee = pointer;
    } else if body_resp.clicked()
        && let Some(p) = body_resp.interact_pointer_pos()
    {
        st.selected.clear();
        let start = ((x_to_tick(st, p.x) / snap).floor() * snap).max(0.0) as u64;
        let row = row_at(st, p.y);
        let track = row_to_track(row);
        if row >= 0 && (row as usize) < row_track.len() && !st.slice {
            s.checkpoint();
            s.view.add_pattern_clip(s.pattern, track, start);
            st.selected = vec![s.view.clips.len() - 1];
            s.mark_dirty();
        }
    }

    // Selection box (Shift- or ⌘-drag on empty space; Shift adds to the selection).
    if let Some(anchor) = st.marquee {
        let released = ui.input(|i| i.pointer.any_released());
        if let Some(p) = pointer {
            let r = Rect::from_two_pos(anchor, p);
            clip_painter.rect(r, 0.0, theme::alpha(theme::ACCENT, 0.12), Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
            if released {
                let keep: Vec<usize> = if shift { st.selected.clone() } else { Vec::new() };
                let boxed: Vec<usize> = s
                    .view
                    .clips
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        let y = track_y(st, c.track);
                        Rect::from_min_max(pos2(tick_to_x(st, c.start as f64), y), pos2(tick_to_x(st, (c.start + c.length) as f64), y + TRACK_H)).intersects(r)
                    })
                    .map(|(i, _)| i)
                    .collect();
                st.selected = keep;
                for i in boxed {
                    if !st.selected.contains(&i) {
                        st.selected.push(i);
                    }
                }
            }
        }
        if released {
            st.marquee = None;
        }
    }

    // Apply an active clip drag (all selected clips move together).
    if let Some(d) = &drag {
        let released = ui.input(|i| i.pointer.any_released());
        if let Some(p) = pointer {
            let raw = x_to_tick(st, p.x) - d.grab_tick;
            let dt = (raw / snap).round() * snap;
            let dr = row_at(st, p.y) - d.grab_row;
            let mut changed = false;
            for (i, orig) in &d.originals {
                let Some(mut next) = s.view.clips.get(*i).cloned() else { continue };
                match d.kind {
                    DragKind::Move => {
                        next.start = (orig.start as f64 + dt).max(0.0) as u64;
                        let row = vis_row.get(orig.track).copied().flatten().unwrap_or(0) as i64 + dr;
                        next.track = row_to_track(row);
                    }
                    DragKind::Resize => next.length = (orig.length as f64 + dt).max(snap) as u64,
                    DragKind::ResizeLeft => {
                        // Can't trim past the right edge, before tick 0, or before the source starts.
                        let back = orig.source_offset / source_units(s, orig, 1.0).max(1e-12);
                        let delta = dt.max(-(orig.start as f64)).max(-back).min(orig.length as f64 - snap);
                        next.start = (orig.start as f64 + delta).round() as u64;
                        next.length = (orig.length as f64 - delta).round().max(1.0) as u64;
                        next.source_offset = (orig.source_offset + source_units(s, orig, delta)).max(0.0);
                        next.fade_in = orig.fade_in.min(next.length);
                        next.fade_out = orig.fade_out.min(next.length);
                    }
                    DragKind::Slip => next.source_offset = (orig.source_offset - source_units(s, orig, raw)).max(0.0),
                    DragKind::FadeIn => next.fade_in = (orig.fade_in as f64 + raw).clamp(0.0, orig.length.saturating_sub(orig.fade_out) as f64) as u64,
                    DragKind::FadeOut => next.fade_out = (orig.fade_out as f64 - raw).clamp(0.0, orig.length.saturating_sub(orig.fade_in) as f64) as u64,
                }
                let clip = &mut s.view.clips[*i];
                let differs = (clip.start, clip.track, clip.length, clip.fade_in, clip.fade_out) != (next.start, next.track, next.length, next.fade_in, next.fade_out) || clip.source_offset != next.source_offset;
                if differs {
                    *clip = next;
                    changed = true;
                }
            }
            if changed {
                s.mark_dirty();
            }
        }
        if released {
            drag = None;
        }
    }
    ui.data_mut(|data| match drag {
        Some(d) => {
            data.insert_temp(drag_id, d);
        }
        None => data.remove::<ClipDrag>(drag_id),
    });
    match clip_menu {
        Some((ci, ClipMenu::Color(c))) => {
            s.checkpoint();
            for i in if st.selected.contains(&ci) { st.selected.clone() } else { vec![ci] } {
                if let Some(clip) = s.view.clips.get_mut(i) {
                    clip.color = c;
                }
            }
        }
        Some((ci, ClipMenu::MakeUnique)) => {
            s.checkpoint();
            if let Some(p) = s.view.make_clip_unique(ci) {
                s.select_pattern(p);
            }
            s.mark_dirty();
        }
        Some((ci, ClipMenu::Delete)) => delete = Some(ci),
        Some((ci, ClipMenu::Stretch(ratio))) => match s.stretch_audio_clip(ci, ratio) {
            Ok(m) | Err(m) => stretch_message = Some(m),
        },
        Some((ci, edit @ (ClipMenu::Gain(_) | ClipMenu::Reverse(_) | ClipMenu::ResetFades))) => {
            if !matches!(edit, ClipMenu::Gain(_)) || s.view.clips.get(ci).is_some_and(|c| (c.gain - 1.0).abs() < 1e-6) {
                s.checkpoint();
            }
            for i in if st.selected.contains(&ci) { st.selected.clone() } else { vec![ci] } {
                if let Some(clip) = s.view.clips.get_mut(i).filter(|c| c.kind == ClipKind::Audio) {
                    match edit {
                        ClipMenu::Gain(g) => clip.gain = g,
                        ClipMenu::Reverse(r) => clip.reverse = r,
                        _ => (clip.fade_in, clip.fade_out) = (0, 0),
                    }
                }
            }
            s.mark_dirty();
        }
        None => {}
    }
    if let Some(ci) = delete {
        s.checkpoint();
        s.view.clips.remove(ci);
        st.selected.clear();
        s.mark_dirty();
    }
    if ui.rect_contains_pointer(rect) && !ui.ctx().egui_wants_keyboard_input() {
        clip_shortcuts(ui, s, st);
    }

    // Ruler: click to jump, drag to set a loop.
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.max.x, ruler.max.y)), 0.0, theme::PANEL_2);
    let bar_label_every = ((40.0 / (bar as f32 * st.px_per_tick)).ceil() as i64).max(1);
    for b in first.max(0)..=last {
        if b % bar_label_every != 0 {
            continue;
        }
        let x = tick_to_x(st, s.view.bar_start(b as u64) as f64);
        if x >= ruler.min.x {
            painter.text(pos2(x + 4.0, ruler.center().y), Align2::LEFT_CENTER, (b + 1).to_string(), theme::mono(10.5), theme::TEXT_DIM);
            painter.line_segment([pos2(x, ruler.max.y - 6.0), pos2(x, ruler.max.y)], Stroke::new(1.0, theme::LINE_STRONG));
        }
    }
    if let Some((a, b)) = s.song_loop.filter(|_| s.mode == Mode::Song) {
        let lr = Rect::from_min_max(pos2(tick_to_x(st, a as f64).max(ruler.min.x), ruler.min.y + 3.0), pos2(tick_to_x(st, b as f64), ruler.max.y - 3.0));
        if lr.width() > 0.0 {
            painter.rect_filled(lr, theme::RADIUS_SM, theme::alpha(theme::ACCENT, 0.25));
            painter.rect_stroke(lr, theme::RADIUS_SM, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
        }
    }
    // Time-signature changes (top right of their bar's number).
    for m in &s.view.meter_changes {
        let x = tick_to_x(st, s.view.bar_start(m.bar) as f64);
        if x < ruler.min.x || x > ruler.max.x {
            continue;
        }
        let label = format!("{}/4", m.beats_per_bar);
        let w = painter.layout_no_wrap(label.clone(), theme::ui_font(9.0), theme::BG).size().x + 6.0;
        let flag = Rect::from_min_size(pos2(x + 22.0, ruler.min.y + 2.0), vec2(w, 10.0));
        painter.rect_filled(flag, 2.0, theme::TEXT_DIM);
        painter.text(pos2(flag.min.x + 3.0, flag.center().y), Align2::LEFT_CENTER, label, theme::ui_font(9.0), theme::BG);
    }
    // Tempo changes (bottom of the ruler).
    for c in &s.view.tempo_changes {
        let x = tick_to_x(st, c.tick as f64);
        if x < ruler.min.x || x > ruler.max.x {
            continue;
        }
        let label = format!("♩{:.0}", c.bpm);
        let w = painter.layout_no_wrap(label.clone(), theme::ui_font(9.0), theme::BG).size().x + 6.0;
        let flag = Rect::from_min_size(pos2(x, ruler.max.y - 11.0), vec2(w, 10.0));
        painter.rect_filled(flag, 2.0, theme::ACCENT_HOT);
        painter.text(pos2(flag.min.x + 3.0, flag.center().y), Align2::LEFT_CENTER, label, theme::ui_font(9.0), theme::BG);
    }

    // Markers.
    for m in &s.view.markers {
        let x = tick_to_x(st, m.tick as f64);
        if x < ruler.min.x || x > ruler.max.x {
            continue;
        }
        painter.line_segment([pos2(x, ruler.min.y + 2.0), pos2(x, body.max.y)], Stroke::new(1.0, theme::alpha(theme::WARN, 0.5)));
        let flag = Rect::from_min_size(pos2(x, ruler.min.y + 2.0), vec2(painter.layout_no_wrap(m.name.clone(), theme::ui_font(9.5), theme::BG).size().x + 8.0, 12.0));
        painter.rect_filled(flag, 2.0, theme::WARN);
        painter.text(pos2(flag.min.x + 4.0, flag.center().y), Align2::LEFT_CENTER, &m.name, theme::ui_font(9.5), theme::BG);
    }
    let ruler_resp = ui.interact(ruler, Id::new("playlist-ruler"), Sense::click_and_drag());
    let marker_at = |st: &PlaylistState, x: f32| s.view.markers.iter().position(|m| (tick_to_x(st, m.tick as f64) - x).abs() < 6.0 || (x >= tick_to_x(st, m.tick as f64) && x <= tick_to_x(st, m.tick as f64) + 60.0));
    let hovered_marker = ruler_resp.hover_pos().filter(|p| p.y < ruler.max.y - 12.0).and_then(|p| marker_at(st, p.x));
    let hovered_tempo = ruler_resp
        .hover_pos()
        .filter(|p| p.y >= ruler.max.y - 12.0)
        .and_then(|p| s.view.tempo_changes.iter().position(|c| p.x >= tick_to_x(st, c.tick as f64) - 2.0 && p.x <= tick_to_x(st, c.tick as f64) + 40.0));
    let pointer_tick = ruler_resp.hover_pos().map(|p| x_to_tick(st, p.x).max(0.0));
    let mut marker_action = None;
    ruler_resp.context_menu(|ui| {
        if let Some(i) = hovered_tempo {
            let mut bpm = s.view.tempo_changes[i].bpm;
            ui.horizontal(|ui| {
                ui.label("Tempo");
                if ui.add(egui::DragValue::new(&mut bpm).range(10.0..=522.0).speed(0.1).fixed_decimals(2)).changed() {
                    marker_action = Some(MarkerAction::Tempo(s.view.tempo_changes[i].tick, bpm));
                }
            });
            if ui.button(egui::RichText::new("Remove tempo change").color(theme::ERROR)).clicked() {
                marker_action = Some(MarkerAction::RemoveTempo(i));
                ui.close();
            }
            return;
        }
        if hovered_marker.is_none()
            && let Some(t) = pointer_tick
            && ui.button("Tempo change here").clicked()
        {
            let bar = s.ticks_per_bar() as f64;
            let tick = ((t / bar).round() * bar).max(bar) as u64;
            marker_action = Some(MarkerAction::Tempo(tick, s.view.tempo_at(tick)));
            ui.close();
        }
        if s.mode == Mode::Song
            && let Some(t) = pointer_tick
        {
            let (bar_index, at, _) = s.view.bar_at(t as u64);
            ui.separator();
            // Time signature from this bar on.
            let mut beats = s.view.beats_in_bar(bar_index);
            ui.horizontal(|ui| {
                ui.label(format!("Time signature from bar {}", bar_index + 1));
                if ui.add(egui::DragValue::new(&mut beats).range(1..=16).suffix("/4")).changed() {
                    marker_action = Some(MarkerAction::Meter(bar_index, beats));
                }
            });
            if s.view.meter_changes.iter().any(|m| m.bar == bar_index) && ui.button("Remove time signature change").clicked() {
                marker_action = Some(MarkerAction::RemoveMeter(bar_index));
                ui.close();
            }
            if ui.button(format!("Insert bar before bar {}", bar_index + 1)).clicked() {
                marker_action = Some(MarkerAction::InsertBar(at));
                ui.close();
            }
            if ui.button(format!("Delete bar {}", bar_index + 1)).clicked() {
                marker_action = Some(MarkerAction::DeleteBar(at));
                ui.close();
            }
            if let Some((a, b)) = s.song_loop {
                if ui.button("Duplicate loop selection").on_hover_text("Repeat the looped bars right after themselves").clicked() {
                    marker_action = Some(MarkerAction::DuplicateRange(a, b));
                    ui.close();
                }
                if ui.button("Delete loop selection").on_hover_text("Remove the looped bars and close the gap").clicked() {
                    marker_action = Some(MarkerAction::DeleteRange(a, b));
                    ui.close();
                }
            }
            ui.separator();
        }
        match hovered_marker {
            Some(i) => {
                let mut name = s.view.markers[i].name.clone();
                if ui.text_edit_singleline(&mut name).changed() {
                    marker_action = Some(MarkerAction::Rename(i, name));
                }
                if ui.button(egui::RichText::new("Delete marker").color(theme::ERROR)).clicked() {
                    marker_action = Some(MarkerAction::Delete(i));
                    ui.close();
                }
            }
            None => {
                if ui.button("Add marker here").clicked() {
                    marker_action = Some(MarkerAction::Add);
                    ui.close();
                }
            }
        }
    });
    let loop_id = Id::new("playlist-loop-anchor");
    if let Some(p) = ruler_resp.interact_pointer_pos() {
        let beat_snap = beat;
        let t = ((x_to_tick(st, p.x) / beat_snap).round() * beat_snap).max(0.0);
        if ruler_resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(loop_id, t));
        }
        if ruler_resp.dragged() && let Some(anchor) = ui.data(|d| d.get_temp::<f64>(loop_id)) {
            let (a, b) = if t < anchor { (t, anchor) } else { (anchor, t) };
            if s.mode == Mode::Song {
                s.set_song_loop(Some((a as u64, b as u64)));
            }
        }
        if ruler_resp.clicked() {
            // Clicking a marker jumps to it exactly.
            let t = hovered_marker.map_or(x_to_tick(st, p.x).max(0.0), |i| s.view.markers[i].tick as f64);
            s.seek_ticks(t);
            st.follow = true;
        }
    }

    match marker_action {
        Some(MarkerAction::Add) => {
            if let Some(p) = ruler_resp.interact_pointer_pos().or(ui.input(|i| i.pointer.latest_pos())) {
                let bar = s.ticks_per_bar() as f64;
                let tick = ((x_to_tick(st, p.x) / bar).round() * bar).max(0.0) as u64;
                let n = s.view.markers.len() + 1;
                s.view.markers.push(engine::view::Marker { tick, name: format!("Marker {n}") });
                s.view.markers.sort_by_key(|m| m.tick);
                s.touch();
            }
        }
        Some(MarkerAction::Rename(i, name)) => {
            s.view.markers[i].name = name;
            s.touch();
        }
        Some(MarkerAction::Delete(i)) => {
            s.view.markers.remove(i);
            s.touch();
        }
        Some(MarkerAction::Tempo(tick, bpm)) => s.set_tempo_change(tick, bpm),
        Some(MarkerAction::Meter(bar, beats)) => s.set_meter_change(bar, beats),
        Some(MarkerAction::RemoveMeter(bar)) => s.remove_meter_change(bar),
        Some(MarkerAction::InsertBar(at)) => {
            s.checkpoint();
            let bar = s.view.bar_at(at).2;
            s.view.insert_time(at, bar);
            st.selected.clear();
            s.mark_dirty();
        }
        Some(MarkerAction::DeleteBar(at)) => {
            s.checkpoint();
            let bar = s.view.bar_at(at).2;
            s.view.delete_time(at, at + bar);
            st.selected.clear();
            s.mark_dirty();
        }
        Some(MarkerAction::DuplicateRange(a, b)) => {
            s.checkpoint();
            s.view.duplicate_time(a, b);
            s.set_song_loop(Some((b, b + (b - a))));
            st.selected.clear();
            s.mark_dirty();
        }
        Some(MarkerAction::DeleteRange(a, b)) => {
            s.checkpoint();
            s.view.delete_time(a, b);
            s.set_song_loop(None);
            st.selected.clear();
            s.mark_dirty();
        }
        Some(MarkerAction::RemoveTempo(i)) => s.remove_tempo_change(i),
        None => {}
    }

    // Playhead.
    if s.mode == Mode::Song {
        let x = tick_to_x(st, playhead);
        if x >= body.min.x && x <= body.max.x {
            painter.line_segment([pos2(x, ruler.min.y), pos2(x, body.max.y)], Stroke::new(1.5, theme::ACCENT));
            let tri = vec![pos2(x - 6.0, ruler.min.y), pos2(x + 6.0, ruler.min.y), pos2(x, ruler.min.y + 8.0)];
            painter.add(egui::Shape::convex_polygon(tri, theme::ACCENT, Stroke::NONE));
        }
    }

    // Track headers drawn last so clips never cover them.
    painter.rect_filled(headers, 0.0, theme::PANEL_2);
    painter.rect_filled(Rect::from_min_max(rect.min, pos2(headers.max.x, ruler.max.y)), 0.0, theme::PANEL_2);
    // Tools in the corner above the track names.
    let corner = Rect::from_min_max(rect.min, pos2(headers.max.x, ruler.max.y));
    let mut tools = ui.new_child(egui::UiBuilder::new().max_rect(corner.shrink2(vec2(4.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    tools.spacing_mut().item_spacing.x = 3.0;
    if widgets::toggle(&mut tools, !st.slice, "Draw", vec2(40.0, 20.0)).on_hover_text("Draw tool: click to place the selected pattern, drag clips to move (P)").clicked() {
        st.slice = false;
    }
    if widgets::toggle(&mut tools, st.slice, "Slice", vec2(40.0, 20.0)).on_hover_text("Slice tool: click a clip to split it (C)").clicked() {
        st.slice = true;
    }
    for (label, tip, factor) in [("−", "Zoom out", 0.7f32), ("+", "Zoom in", 1.0 / 0.7)] {
        if widgets::toggle(&mut tools, false, label, vec2(18.0, 20.0)).on_hover_text(tip).clicked() {
            st.px_per_tick = (st.px_per_tick * factor).clamp(0.01, 4.0);
        }
    }
    if widgets::toggle(&mut tools, false, "Fit", vec2(26.0, 20.0)).on_hover_text("Fit the whole song in view").clicked() {
        let end = s.view.clips.iter().map(|c| c.start + c.length).max().unwrap_or(s.ticks_per_bar() * 8).max(1);
        st.px_per_tick = ((body.width() - 20.0) / end as f32).clamp(0.01, 4.0);
        st.scroll_x = 0.0;
    }
    let mut track_action: Option<TrackAction> = None;
    let header_painter = ui.painter_at(headers);
    for i in 0..s.view.tracks.len() {
        let y = track_y(st, i);
        if y + TRACK_H < headers.min.y || y > headers.max.y {
            continue;
        }
        let row = Rect::from_min_max(pos2(headers.min.x, y), pos2(headers.max.x, y + TRACK_H));
        let track = s.view.tracks[i].clone();
        let track_color = track.color.map_or_else(|| theme::clip_color(&track.name), |[r, g, b]| Color32::from_rgb(r, g, b));
        header_painter.rect_filled(Rect::from_min_max(row.min, pos2(row.min.x + 3.0, row.max.y)), 0.0, track_color);
        header_painter.line_segment([pos2(row.min.x, row.max.y), pos2(row.max.x, row.max.y)], Stroke::new(1.0, theme::LINE_SOFT));
        // Folder headers get a collapse arrow; grouped tracks are indented under them.
        let indent = if folders[i].is_some() { 14.0 } else { 0.0 };
        if track.folder {
            header_painter.rect_filled(Rect::from_min_max(pos2(row.min.x + 3.0, row.min.y), row.max), 0.0, theme::alpha(track_color, 0.10));
            let arrow = Rect::from_center_size(pos2(row.min.x + 14.0, row.center().y), vec2(16.0, 16.0));
            let ar = ui.interact(arrow, Id::new(("folder-arrow", i)), Sense::click());
            header_painter.text(arrow.center(), Align2::CENTER_CENTER, if track.collapsed { "▸" } else { "▾" }, theme::ui_font(12.0), if ar.hovered() { theme::ACCENT_HOT } else { theme::TEXT_DIM });
            let n = folders.iter().filter(|f| **f == Some(i)).count();
            if ar.on_hover_text(format!("{} folder ({n} track{})", if track.collapsed { "Expand" } else { "Collapse" }, if n == 1 { "" } else { "s" })).clicked() {
                track_action = Some(TrackAction::ToggleCollapse(i));
            }
        }
        let name_rect = Rect::from_min_max(pos2(row.min.x + 10.0 + indent + if track.folder { 14.0 } else { 0.0 }, row.min.y + 8.0), pos2(row.max.x - 89.0, row.max.y - 8.0));
        if st.renaming.as_ref().is_some_and(|(t, _)| *t == i) {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect));
            let (_, text) = st.renaming.as_mut().unwrap();
            let r = child.add(egui::TextEdit::singleline(text).desired_width(name_rect.width()));
            r.request_focus();
            if r.lost_focus() {
                let name = text.trim().to_string();
                if !name.is_empty() && ui.input(|i| !i.key_pressed(egui::Key::Escape)) {
                    s.view.tracks[i].name = name;
                    s.touch();
                }
                st.renaming = None;
            }
        } else {
            let label_x = name_rect.min.x + 6.0;
            let muted = s.view.effective_mutes().get(i).copied().unwrap_or(track.muted);
            let font = if track.folder { theme::ui_font(12.5) } else { theme::ui_font(12.0) };
            header_painter.text(pos2(label_x, row.center().y), Align2::LEFT_CENTER, &track.name, font, if muted { theme::TEXT_FAINT } else { theme::TEXT });
            let name_resp = ui.interact(name_rect.intersect(headers), Id::new(("track-name", i)), Sense::click());
            let name_resp = name_resp.on_hover_text("Double-click to rename · right-click for track options");
            if name_resp.double_clicked() {
                st.renaming = Some((i, s.view.tracks[i].name.clone()));
            }
            let count = s.view.tracks.len();
            name_resp.context_menu(|ui| {
                if ui.button("Rename…").clicked() {
                    st.renaming = Some((i, s.view.tracks[i].name.clone()));
                    ui.close();
                }
                ui.label(egui::RichText::new("Colour").small().color(theme::TEXT_DIM));
                ui.horizontal(|ui| {
                    for c in theme::CLIP_PALETTE {
                        let (sw, swr) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().rect_filled(sw, 3.0, c);
                        if swr.clicked() {
                            track_action = Some(TrackAction::Color(i, Some([c.r(), c.g(), c.b()])));
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button("Insert track above").clicked() {
                    track_action = Some(TrackAction::Insert(i));
                    ui.close();
                }
                ui.separator();
                if ui.button(if s.view.tracks[i].folder { "Turn folder back into a track" } else { "Make this a folder" }).on_hover_text("A folder groups the tracks below it that are set to \"In folder above\"").clicked() {
                    track_action = Some(TrackAction::ToggleFolder(i));
                    ui.close();
                }
                if !s.view.tracks[i].folder {
                    let mut grouped = s.view.tracks[i].grouped;
                    if ui.checkbox(&mut grouped, "In folder above").clicked() {
                        track_action = Some(TrackAction::ToggleGrouped(i));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.add_enabled(i > 0, egui::Button::new("Move up")).clicked() {
                    track_action = Some(TrackAction::Move(i, i - 1));
                    ui.close();
                }
                if ui.add_enabled(i + 1 < count, egui::Button::new("Move down")).clicked() {
                    track_action = Some(TrackAction::Move(i, i + 1));
                    ui.close();
                }
                if ui.add_enabled(count > 1, egui::Button::new(egui::RichText::new("Delete track and its clips").color(theme::ERROR))).clicked() {
                    track_action = Some(TrackAction::Delete(i));
                    ui.close();
                }
            });
        }
        for (x, label) in [(row.max.x - 76.0, "S"), (row.max.x - 49.0, "M"), (row.max.x - 22.0, "A")] {
            let button = Rect::from_center_size(pos2(x, row.center().y), vec2(25.0, 22.0));
            if !headers.contains_rect(button) || track.folder && label != "M" { continue; }
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(button));
            let active = match label { "S" => s.solo_track == Some(i), "M" => track.muted, _ => s.audio_armed && s.record_track == Some(i) };
            let tip = match label { "S" => "Solo this playlist track", "M" => "Mute this playlist track", _ => "Arm this playlist track for audio recording" };
            if widgets::toggle(&mut child, active, label, vec2(25.0, 22.0)).on_hover_text(tip).clicked() {
                match label {
                    "S" => { s.solo_track = if active { None } else { Some(i) }; s.sync_track_mutes(); }
                    "M" => { s.view.tracks[i].muted = !track.muted; s.sync_track_mutes(); s.touch(); }
                    _ => { s.record_track = if active { None } else { Some(i) }; s.audio_armed = !active; }
                }
            }
        }
    }
    painter.line_segment([pos2(headers.max.x, rect.min.y), pos2(headers.max.x, rect.max.y)], Stroke::new(1.0, theme::LINE));
    if let Some(a) = track_action {
        let structure_changed = matches!(a, TrackAction::Insert(_) | TrackAction::Delete(_) | TrackAction::Move(_, _));
        s.checkpoint();
        match a {
            TrackAction::Insert(i) => s.view.insert_track(i),
            TrackAction::Delete(i) => s.view.delete_track(i),
            TrackAction::Move(from, to) => s.view.move_track(from, to),
            TrackAction::Color(i, c) => s.view.tracks[i].color = c,
            TrackAction::ToggleFolder(i) => {
                let t = &mut s.view.tracks[i];
                t.folder = !t.folder;
                t.collapsed = false;
                if t.folder {
                    t.grouped = false;
                    // Group the tracks right below that have clips, up to the next folder.
                    let mut k = i + 1;
                    while k < s.view.tracks.len() && !s.view.tracks[k].folder && s.view.clips.iter().any(|c| c.track == k) {
                        s.view.tracks[k].grouped = true;
                        k += 1;
                    }
                }
            }
            TrackAction::ToggleGrouped(i) => s.view.tracks[i].grouped = !s.view.tracks[i].grouped,
            TrackAction::ToggleCollapse(i) => s.view.tracks[i].collapsed = !s.view.tracks[i].collapsed,
        }
        if structure_changed {
            s.solo_track = None;
            if s.record_track.take().is_some() { s.audio_armed = false; }
        }
        st.selected.clear();
        s.sync_track_mutes();
        s.mark_dirty();
    }

    ui.allocate_rect(rect, Sense::hover());
    Outcome { open_pattern, open_automation, dropped, message: stretch_message }
}

fn draw_clip(painter: &egui::Painter, s: &Session, clip: &ClipView, r: Rect, muted: bool, selected: bool) {
    let chosen = clip.color.or_else(|| clip.pattern.and_then(|p| s.view.patterns.get(p)).and_then(|p| p.color));
    let base = chosen.map_or_else(|| theme::clip_color(&clip.label), |[r, g, b]| Color32::from_rgb(r, g, b));
    let base = if muted { theme::mix(base, theme::PANEL, 0.7) } else { base };
    let body_fill = theme::mix(base, theme::BG, 0.72);
    painter.rect(r, theme::RADIUS_SM, body_fill, Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { theme::ACCENT_HOT } else { theme::mix(base, theme::BG, 0.3) }), StrokeKind::Inside);
    let title = Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + CLIP_TITLE_H));
    painter.rect_filled(title, theme::RADIUS_SM, theme::mix(base, theme::BG, 0.25));
    // Subtle top line highlight
    painter.line_segment([pos2(title.min.x + 1.0, title.min.y + 1.0), pos2(title.max.x - 1.0, title.min.y + 1.0)], Stroke::new(1.0, theme::alpha(Color32::WHITE, 0.22)));
    if r.width() > 24.0 {
        let text_painter = painter.with_clip_rect(title.intersect(painter.clip_rect()));
        let label = theme::elide(&text_painter, &clip.label, theme::ui_font(10.0), title.width() - 8.0);
        text_painter.text(pos2(title.min.x + 5.0, title.center().y), Align2::LEFT_CENTER, label, theme::ui_font(10.0), Color32::WHITE);
    }
    let content = Rect::from_min_max(pos2(r.min.x + 2.0, title.max.y + 2.0), pos2(r.max.x - 2.0, r.max.y - 2.0));
    if content.width() < 4.0 || content.height() < 4.0 {
        return;
    }
    let ink = if muted { theme::TEXT_FAINT } else { theme::mix(base, Color32::WHITE, 0.35) };
    let painter = painter.with_clip_rect(content.intersect(painter.clip_rect()));
    match clip.kind {
        ClipKind::Automation => {
            let Some(auto) = clip.automation.and_then(|a| s.view.automations.get(a)) else { return };
            let px_per_tick = r.width() / clip.length.max(1) as f32;
            let mut pts = Vec::new();
            let mut x = content.min.x.max(painter.clip_rect().min.x);
            while x <= content.max.x.min(painter.clip_rect().max.x) {
                let tick = clip.source_offset + ((x - r.min.x) / px_per_tick) as f64;
                let v = auto.value_at(tick);
                pts.push(Pos2::new(x, content.max.y - v * content.height()));
                x += 2.0;
            }
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, ink)));
            }
        }
        ClipKind::Pattern => {
            let Some(p) = clip.pattern.and_then(|i| s.view.patterns.get(i)) else { return };
            if p.notes.is_empty() {
                return;
            }
            let (lo, hi) = p.notes.iter().fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
            let span = (hi - lo) as f32 + 1.0;
            let px_per_tick = r.width() / clip.length.max(1) as f32;
            let row_h = (content.height() / span).clamp(1.5, 6.0);
            for n in &p.notes {
                let t = n.start as f64 - clip.source_offset;
                if t < 0.0 || t >= clip.length as f64 {
                    continue;
                }
                let x = content.min.x + t as f32 * px_per_tick;
                let w = (n.length as f32 * px_per_tick).max(1.5);
                let y = content.max.y - ((n.key - lo) as f32 + 1.0) / span * content.height();
                painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, row_h)), 0.0, ink);
            }
        }
        ClipKind::Audio => {
            let Some(wave) = clip.channel.and_then(|c| s.view.channels.get(c)).and_then(|c| c.waveform.clone()) else { return };
            let samples_per_px = (clip.length as f64 * s.view.samples_per_tick) / r.width() as f64;
            let offset_samples = clip.source_offset * s.view.sample_rate;
            let clip_samples = clip.length as f64 * s.view.samples_per_tick;
            let mid = content.center().y;
            let half = content.height() / 2.0;
            let px_per_tick = r.width() / clip.length.max(1) as f32;
            let fade_in_px = clip.fade_in as f32 * px_per_tick;
            let fade_out_px = clip.fade_out as f32 * px_per_tick;
            let mut x = content.min.x.max(painter.clip_rect().min.x);
            while x < content.max.x.min(painter.clip_rect().max.x) {
                let into = (x - r.min.x) as f64 * samples_per_px;
                let into = if clip.reverse { (clip_samples - into - samples_per_px).max(0.0) } else { into };
                let s0 = offset_samples + into;
                let s1 = s0 + samples_per_px.max(1.0);
                let (i0, i1) = ((s0 / WAVEFORM_STEP as f64) as usize, ((s1 / WAVEFORM_STEP as f64) as usize).max((s0 / WAVEFORM_STEP as f64) as usize + 1));
                let peak = wave.get(i0..i1.min(wave.len())).map_or(0.0, |w| w.iter().fold(0.0f32, |m, v| m.max(*v)));
                let dx = x - r.min.x;
                let fade = if fade_in_px > 0.0 && dx < fade_in_px { engine::fade_curve(dx / fade_in_px) } else { 1.0 }
                    * if fade_out_px > 0.0 && r.max.x - x < fade_out_px { engine::fade_curve((r.max.x - x) / fade_out_px) } else { 1.0 };
                let h = (peak * clip.gain * fade).min(1.0) * half;
                painter.line_segment([Pos2::new(x, mid - h.max(0.5)), Pos2::new(x, mid + h.max(0.5))], Stroke::new(1.0, ink));
                x += 1.0;
            }
            // Fade curves and their handles.
            // Smooth equal-power curves (the same shape the engine plays), shading what's faded.
            let (top, bottom) = (content.min.y, content.max.y);
            let line = Stroke::new(1.5, theme::alpha(Color32::WHITE, 0.8));
            let curve = |x0: f32, w: f32, rising: bool| -> Vec<Pos2> {
                let n = (w / 2.0).clamp(8.0, 96.0) as usize;
                (0..=n)
                    .map(|k| {
                        let t = k as f32 / n as f32;
                        let g = engine::fade_curve(if rising { t } else { 1.0 - t });
                        pos2(x0 + w * t, bottom - g * (bottom - top))
                    })
                    .collect()
            };
            for (x0, w, rising) in [(r.min.x, fade_in_px, true), (r.max.x - fade_out_px, fade_out_px, false)] {
                if w <= 0.0 {
                    continue;
                }
                let pts = curve(x0, w, rising);
                // Shade above the curve in thin vertical slices (the area isn't convex).
                for pair in pts.windows(2) {
                    let poly = vec![pos2(pair[0].x, top), pos2(pair[1].x, top), pair[1], pair[0]];
                    painter.add(egui::Shape::convex_polygon(poly, theme::alpha(theme::BG, 0.45), Stroke::NONE));
                }
                painter.add(egui::Shape::line(pts, line));
            }
            if r.width() > 24.0 {
                let (fi, fo) = fade_handles(clip, r, px_per_tick);
                for h in [fi, fo] {
                    painter.rect(Rect::from_center_size(h, vec2(6.0, 6.0)), 1.0, theme::alpha(Color32::WHITE, 0.85), Stroke::new(1.0, theme::BG), StrokeKind::Inside);
                }
            }
            if clip.reverse {
                painter.text(pos2(content.max.x - 3.0, content.max.y - 2.0), Align2::RIGHT_BOTTOM, "REV", theme::ui_font(8.0), theme::alpha(Color32::WHITE, 0.8));
            }
            if (clip.gain - 1.0).abs() > 1e-3 {
                let db = if clip.gain <= 1e-5 { "-inf".to_string() } else { format!("{:+.1} dB", 20.0 * clip.gain.log10()) };
                painter.text(pos2(content.min.x + 3.0, content.max.y - 2.0), Align2::LEFT_BOTTOM, db, theme::ui_font(8.0), theme::alpha(Color32::WHITE, 0.8));
            }
        }
    }
}

enum TrackAction {
    ToggleFolder(usize),
    ToggleGrouped(usize),
    ToggleCollapse(usize),
    Insert(usize),
    Delete(usize),
    Move(usize, usize),
    Color(usize, Option<[u8; 3]>),
}

enum MarkerAction {
    Add,
    /// Arrangement edits from the ruler menu.
    InsertBar(u64),
    Meter(u64, u32),
    RemoveMeter(u64),
    DeleteBar(u64),
    DuplicateRange(u64, u64),
    DeleteRange(u64, u64),
    Rename(usize, String),
    Delete(usize),
    Tempo(u64, f64),
    RemoveTempo(usize),
}

enum ClipMenu {
    Color(Option<[u8; 3]>),
    MakeUnique,
    Delete,
    /// Audio clips.
    Gain(f32),
    Reverse(bool),
    ResetFades,
    /// Time-stretch by this ratio (keeps pitch).
    Stretch(f64),
}

/// Splits clip `index` at tick `at` into two clips. Returns false if `at` isn't inside it.
fn split_clip(view: &mut engine::view::ProjectView, index: usize, at: u64) -> bool {
    view.split_clip_at(index, at)
}

fn clip_shortcuts(ui: &Ui, s: &mut Session, st: &mut PlaylistState) {
    let cmd = ui.input(|i| i.modifiers.command);
    let pressed = |k: egui::Key| ui.input(|i| i.key_pressed(k));
    if pressed(egui::Key::Escape) {
        st.selected.clear();
    }
    if !cmd && pressed(egui::Key::C) {
        st.slice = true;
    }
    if !cmd && pressed(egui::Key::P) {
        st.slice = false;
    }
    if cmd && pressed(egui::Key::A) {
        st.selected = (0..s.view.clips.len()).collect();
    }
    if (pressed(egui::Key::Delete) || pressed(egui::Key::Backspace)) && !st.selected.is_empty() {
        s.checkpoint();
        let mut sel = std::mem::take(&mut st.selected);
        sel.sort_unstable();
        for i in sel.into_iter().rev() {
            if i < s.view.clips.len() {
                s.view.clips.remove(i);
            }
        }
        s.mark_dirty();
    }
    if cmd && pressed(egui::Key::C) && !st.selected.is_empty() {
        st.clipboard = st.selected.iter().filter_map(|&i| s.view.clips.get(i).cloned()).collect();
    }
    let duplicate = cmd && pressed(egui::Key::B) && !st.selected.is_empty();
    let paste = cmd && pressed(egui::Key::V) && !st.clipboard.is_empty();
    if duplicate || paste {
        let source: Vec<ClipView> = if duplicate { st.selected.iter().filter_map(|&i| s.view.clips.get(i).cloned()).collect() } else { st.clipboard.clone() };
        let first = source.iter().map(|c| c.start).min().unwrap_or(0);
        let last = source.iter().map(|c| c.start + c.length).max().unwrap_or(0);
        // Duplicate right after the selection; paste at the playhead (snapped to the bar).
        let offset = if duplicate {
            last - first
        } else {
            let bar = s.ticks_per_bar().max(1);
            let at = (s.position_ticks() as u64 / bar) * bar;
            at.saturating_sub(first)
        } as i64;
        s.checkpoint();
        let mut new_sel = Vec::new();
        for c in source {
            let mut copy = c.clone();
            copy.start = (copy.start as i64 + if duplicate || paste { offset } else { 0 }).max(0) as u64;
            s.view.clips.push(copy);
            new_sel.push(s.view.clips.len() - 1);
        }
        st.selected = new_sel;
        s.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::view::{ClipKind, ClipView, ProjectView};

    #[test]
    fn splitting_an_audio_clip_offsets_the_second_half() {
        let mut v = ProjectView {
            automations: vec![],
            swing: 0.0,
            markers: vec![],
            tempo_changes: vec![],
            meter_changes: vec![],
            controller_links: vec![],
            name: String::new(),
            tempo: 120.0,
            ppq: 96,
            beats_per_bar: 4,
            sample_rate: 48_000.0,
            samples_per_tick: 250.0,
            length_ticks: 384,
            tracks: vec![],
            clips: vec![ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track: 0, start: 96, length: 384, label: "a".into(), kind: ClipKind::Audio, pattern: None, channel: Some(0), source_offset: 1.0 }],
            patterns: vec![],
            channels: vec![],
            inserts: vec![],
            warnings: vec![],
        };
        assert!(split_clip(&mut v, 0, 192));
        assert_eq!((v.clips[0].start, v.clips[0].length), (96, 96));
        assert_eq!((v.clips[1].start, v.clips[1].length), (192, 288));
        // 96 ticks × 250 samples/tick ÷ 48 kHz = 0.5 s further into the file.
        assert!((v.clips[1].source_offset - 1.5).abs() < 1e-9);
        assert!(!split_clip(&mut v, 0, 96), "can't split at the edge");
    }

    #[test]
    fn splitting_keeps_fades_at_the_outer_ends_and_handles_reverse() {
        let mut v = ProjectView {
            automations: vec![],
            swing: 0.0,
            markers: vec![],
            tempo_changes: vec![],
            meter_changes: vec![],
            controller_links: vec![],
            name: String::new(),
            tempo: 120.0,
            ppq: 96,
            beats_per_bar: 4,
            sample_rate: 48_000.0,
            samples_per_tick: 480.0,
            length_ticks: 400,
            tracks: vec![],
            clips: vec![ClipView { automation: None, color: None, gain: 1.0, fade_in: 50, fade_out: 60, reverse: true, track: 0, start: 0, length: 400, label: "a".into(), kind: ClipKind::Audio, pattern: None, channel: Some(0), source_offset: 0.0 }],
            patterns: vec![],
            channels: vec![],
            inserts: vec![],
            warnings: vec![],
        };
        assert!(split_clip(&mut v, 0, 100));
        let (a, b) = (&v.clips[0], &v.clips[1]);
        assert_eq!((a.fade_in, a.fade_out, b.fade_in, b.fade_out), (50, 0, 0, 60));
        // 480 samples per tick at 48 kHz = 10 ms per tick: the first (reversed) part is the
        // source's last 100 ticks, starting 3 s in; the second part starts at 0.
        assert!((a.source_offset - 3.0).abs() < 1e-9);
        assert_eq!(b.source_offset, 0.0);
    }
}
