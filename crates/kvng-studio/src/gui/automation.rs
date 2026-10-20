//! The automation clip editor: a curve of points over time.

use eframe::egui::{self, Align2, Id, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::view::AutoPoint;

use super::session::Session;
use super::theme;

const POINT_R: f32 = 5.0;

/// Draws the editor for automation `index`. Click to add a point, drag to move,
/// right-click to delete.
pub fn show(ui: &mut Ui, s: &mut Session, index: usize) {
    let Some(auto) = s.view.automations.get(index).cloned() else {
        ui.label("This automation clip no longer exists.");
        return;
    };
    ui.label(egui::RichText::new(format!("Controls: {}", auto.target.label(&s.view))).color(theme::TEXT_DIM));
    ui.label(egui::RichText::new("Click to add a point · drag to move · right-click to delete · drag a segment's middle handle up/down to curve it (double-click: straight)").small().color(theme::TEXT_FAINT));
    ui.add_space(4.0);

    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(500.0), 220.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect(rect, theme::RADIUS, theme::BG, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);

    let bar = s.ticks_per_bar().max(1);
    let span = auto.points.iter().map(|p| p.tick).max().unwrap_or(bar).max(bar);
    let inner = rect.shrink(8.0);
    let to_pos = |p: &AutoPoint| pos2(inner.min.x + p.tick as f32 / span as f32 * inner.width(), inner.max.y - p.value * inner.height());
    let snap = s.snap_ticks().max(1) as f64;
    let from_pos = |x: f32, y: f32| -> (u64, f32) {
        let tick = ((x - inner.min.x) / inner.width()).clamp(0.0, 1.0) as f64 * span as f64;
        (((tick / snap).round() * snap) as u64, ((inner.max.y - y) / inner.height()).clamp(0.0, 1.0))
    };

    // Bar lines.
    let bars = span / bar;
    for b in 0..=bars {
        let x = inner.min.x + (b * bar) as f32 / span as f32 * inner.width();
        painter.line_segment([pos2(x, inner.min.y), pos2(x, inner.max.y)], Stroke::new(1.0, theme::LINE_SOFT));
        if bars < 64 || b % 4 == 0 {
            painter.text(pos2(x + 3.0, inner.min.y + 2.0), Align2::LEFT_TOP, (b + 1).to_string(), theme::mono(9.0), theme::TEXT_FAINT);
        }
    }
    painter.line_segment([pos2(inner.min.x, inner.center().y), pos2(inner.max.x, inner.center().y)], Stroke::new(1.0, theme::LINE_SOFT));

    // Curve (sampled finely so bends look smooth) and points.
    let pts: Vec<_> = auto.points.iter().map(to_pos).collect();
    let mut line = Vec::with_capacity(inner.width() as usize + 2);
    let mut x = inner.min.x;
    while x <= inner.max.x {
        let tick = ((x - inner.min.x) / inner.width()) as f64 * span as f64;
        line.push(pos2(x, inner.max.y - auto.value_at(tick) * inner.height()));
        x += 1.0;
    }
    for w in line.windows(2) {
        painter.add(egui::Shape::convex_polygon(vec![w[0], w[1], pos2(w[1].x, inner.max.y), pos2(w[0].x, inner.max.y)], theme::alpha(theme::ACCENT, 0.08), Stroke::NONE));
    }
    painter.add(egui::Shape::line(line, Stroke::new(2.0, theme::ACCENT)));

    // Tension handles at the middle of each segment.
    let mid_of = |k: usize| -> egui::Pos2 {
        let (a, b) = (auto.points[k - 1], auto.points[k]);
        let tick = (a.tick + b.tick) as f64 / 2.0;
        pos2(inner.min.x + tick as f32 / span as f32 * inner.width(), inner.max.y - auto.value_at(tick) * inner.height())
    };
    let tension_id = Id::new(("auto-tension", index));
    let mut tension_drag: Option<usize> = ui.data(|d| d.get_temp(tension_id));
    let mut tension_change: Option<(usize, f32)> = None;
    for k in 1..auto.points.len() {
        if auto.points[k].tick == auto.points[k - 1].tick || (auto.points[k].value - auto.points[k - 1].value).abs() < 1e-4 {
            continue;
        }
        let c = mid_of(k);
        let h = ui.interact(egui::Rect::from_center_size(c, vec2(12.0, 12.0)), Id::new(("tension-handle", index, k)), Sense::click_and_drag());
        let hot = h.hovered() || tension_drag == Some(k);
        painter.circle(c, if hot { 4.5 } else { 3.5 }, theme::BG, Stroke::new(1.5, if hot { theme::ACCENT_HOT } else { theme::ACCENT }));
        if h.drag_started() {
            s.checkpoint();
            tension_drag = Some(k);
        }
        if h.dragged() && tension_drag == Some(k) {
            // Up bends the curve toward the higher end; the direction follows the segment.
            let rising = auto.points[k].value > auto.points[k - 1].value;
            let dy = h.drag_delta().y / 80.0;
            let t = auto.points[k].tension + if rising { dy } else { -dy };
            tension_change = Some((k, t.clamp(-1.0, 1.0)));
        }
        if h.double_clicked() {
            s.checkpoint();
            tension_change = Some((k, 0.0));
        }
        if h.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
    }
    if ui.input(|i| i.pointer.any_released()) {
        tension_drag = None;
    }
    ui.data_mut(|d| match tension_drag {
        Some(k) => {
            d.insert_temp(tension_id, k);
        }
        None => d.remove::<usize>(tension_id),
    });
    if let Some((k, t)) = tension_change {
        s.view.automations[index].points[k].tension = t;
        s.mark_dirty();
        return;
    }

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let hovered = pointer.and_then(|p| pts.iter().position(|q| q.distance(p) <= POINT_R + 3.0));
    for (i, p) in pts.iter().enumerate() {
        let hot = Some(i) == hovered;
        painter.circle_filled(*p, if hot { POINT_R + 1.5 } else { POINT_R }, if hot { theme::ACCENT_HOT } else { theme::ACCENT });
        painter.circle_stroke(*p, POINT_R, Stroke::new(1.0, theme::BG));
    }

    let drag_id = Id::new(("auto-drag", index));
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let mut points = auto.points.clone();
    let mut changed = false;

    if resp.secondary_clicked()
        && let Some(i) = hovered
        && points.len() > 1
    {
        s.checkpoint();
        points.remove(i);
        changed = true;
    } else if resp.drag_started() || resp.clicked() {
        if let Some(i) = hovered {
            if resp.drag_started() {
                s.checkpoint();
                dragging = Some(i);
            }
        } else if let Some(p) = pointer {
            s.checkpoint();
            let (tick, value) = from_pos(p.x, p.y);
            points.push(AutoPoint { tick, value, tension: 0.0 });
            points.sort_by_key(|p| p.tick);
            dragging = points.iter().position(|q| q.tick == tick && q.value == value);
            changed = true;
        }
    }
    if let (Some(i), Some(p)) = (dragging, pointer)
        && resp.dragged()
        && i < points.len()
    {
        let (tick, value) = from_pos(p.x, p.y);
        // Keep points in order: a point can't pass its neighbours.
        let lo = if i > 0 { points[i - 1].tick } else { 0 };
        let hi = points.get(i + 1).map_or(u64::MAX, |q| q.tick);
        points[i] = AutoPoint { tick: tick.clamp(lo, hi), value, tension: points[i].tension };
        changed = true;
    }
    if ui.input(|i| i.pointer.any_released()) {
        dragging = None;
    }
    ui.data_mut(|d| match dragging {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => d.remove::<usize>(drag_id),
    });

    if let Some(i) = hovered {
        let p = auto.points[i];
        let at = s.format_bst(p.tick as f64);
        resp.on_hover_text(format!("{:.0}% at {at}", p.value * 100.0));
    }
    if changed {
        s.view.automations[index].points = points;
        s.mark_dirty();
    }
}

/// What was picked from a control's right-click menu.
#[derive(Clone, Copy)]
pub enum ControlAction {
    Automate(engine::view::AutomationTarget),
    Link(engine::view::AutomationTarget),
}

/// Adds "Create automation clip" and "Link to MIDI controller" to a control's right-click menu.
pub fn menu(resp: &egui::Response, target: engine::view::AutomationTarget, out: &mut Option<ControlAction>) {
    // Remember which controls the user is moving, for automation recording.
    if resp.dragged() || resp.changed() {
        resp.ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<engine::view::AutomationTarget>>(touched_id()).push(target));
    }
    resp.context_menu(|ui| {
        if ui.button("Create automation clip").clicked() {
            *out = Some(ControlAction::Automate(target));
            ui.close();
        }
        if ui.button("Link to MIDI controller…").on_hover_text("Then move a knob or fader on your MIDI device").clicked() {
            *out = Some(ControlAction::Link(target));
            ui.close();
        }
    });
}

fn touched_id() -> egui::Id {
    egui::Id::new("automation-touched")
}

/// Controls the user moved since the last call (each once).
pub fn take_touched(ctx: &egui::Context) -> Vec<engine::view::AutomationTarget> {
    let mut list: Vec<engine::view::AutomationTarget> = ctx.data_mut(|d| d.remove_temp(touched_id())).unwrap_or_default();
    let mut seen = Vec::new();
    list.retain(|t| {
        if seen.contains(t) {
            false
        } else {
            seen.push(*t);
            true
        }
    });
    list
}

/// Carries out a control menu choice.
pub fn apply(action: ControlAction, s: &mut Session) -> String {
    match action {
        ControlAction::Automate(t) => {
            s.create_automation(t);
            "Created an automation clip on the playlist".into()
        }
        ControlAction::Link(t) => {
            s.learning = Some(t);
            format!("Move a control on your MIDI device to link it to {}", t.label(&s.view))
        }
    }
}
