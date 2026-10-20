//! The built-in EQ and compressor on every mixer strip, and their expanded editor.

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::channel_strip::{self, PARAMS, StripSettings};
use engine::view::AutomationTarget;

use super::automation::{self, ControlAction};
use super::session::Session;
use super::{theme, widgets};

/// What the compact section asked for.
pub enum StripAction {
    Expand,
    Control(ControlAction),
}

/// A small knob for strip control `param`, labelled underneath.
#[allow(clippy::too_many_arguments)]
fn knob(ui: &mut Ui, st: &mut StripSettings, insert: usize, param: usize, label: &str, size: f32, width: f32, dim: bool, action: &mut Option<ControlAction>) {
    let (_, spec) = PARAMS[param];
    let Some(value) = st.value_mut(param) else { return };
    let mut t = spec.to_normalized(*value);
    let bipolar = spec.lo < 0.0 && spec.hi > 0.0 && spec.default == 0.0;
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.horizontal(|ui| {
            ui.add_space(((width - size) / 2.0).max(0.0));
            ui.add_enabled_ui(true, |ui| {
                if dim { ui.multiply_opacity(0.68); }
                let r = widgets::knob(ui, &mut t, spec.to_normalized(spec.default), size, bipolar, |t| format!("{}: {}", spec.name, spec.format(spec.from_normalized(t))));
                if r.changed() {
                    *value = spec.from_normalized(t);
                }
                automation::menu(&r, AutomationTarget::InsertStrip { insert, param: param as u32 }, action);
            });
        });
        let color = if dim { theme::TEXT_DIM } else { theme::TEXT };
        let (label_rect, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());
        ui.painter().text(label_rect.center(), Align2::CENTER_CENTER, label, theme::ui_font(11.0), color);
        let readout = match param {
            channel_strip::LOW | channel_strip::MID | channel_strip::HIGH => format!("{:+.1}", *value),
            channel_strip::THRESHOLD => format!("{:.0} dB", *value),
            channel_strip::RATIO => format!("{:.1}:1", *value),
            _ => spec.format(*value),
        };
        let (value_rect, _) = ui.allocate_exact_size(vec2(width, 15.0), Sense::hover());
        ui.painter().text(value_rect.center(), Align2::CENTER_CENTER, readout, theme::mono(11.5), if dim { theme::TEXT_DIM } else { theme::ACCENT_HOT });
    });
}

/// A labelled power switch; lit when on.
fn power(ui: &mut Ui, on: bool, label: &str, width: f32, tooltip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 21.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();

    // 1. Recessed socket drop shadow
    p.rect_filled(r.translate(vec2(0.0, 1.0)), theme::RADIUS_SM, theme::METAL_SHADOW);

    // 2. Physical button face
    let cap_r = if on || pressed { r.translate(vec2(0.0, 1.0)) } else { r };
    let fill = if on { theme::ACCENT_DIM } else if hovered { theme::METAL_FACE_HI } else { theme::METAL_FACE };
    let stroke = if on { theme::ACCENT } else if hovered { theme::LINE_STRONG } else { theme::LINE };
    p.rect(cap_r, theme::RADIUS_SM, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);

    // 3. Metallic specular sheen
    if !on && !pressed {
        let top_highlight = Rect::from_min_max(
            pos2(cap_r.min.x + 1.5, cap_r.min.y + 0.6),
            pos2(cap_r.max.x - 1.5, cap_r.min.y + 1.8),
        );
        p.rect_filled(top_highlight, 0.5, theme::alpha(Color32::WHITE, if hovered { 0.18 } else { 0.10 }));
    }

    if on {
        p.circle_filled(pos2(cap_r.min.x + 7.0, cap_r.center().y), 3.2, theme::ACCENT_GLOW);
        p.circle_filled(pos2(cap_r.min.x + 7.0, cap_r.center().y), 1.8, theme::ACCENT_HOT);
    } else {
        p.circle_filled(pos2(cap_r.min.x + 7.0, cap_r.center().y), 1.8, theme::TEXT_FAINT);
    }
    p.text(pos2(cap_r.min.x + 15.0, cap_r.center().y), Align2::LEFT_CENTER, label, theme::ui_font(11.0), if on { Color32::WHITE } else { theme::TEXT_DIM });
    p.text(pos2(cap_r.max.x - 5.0, cap_r.center().y), Align2::RIGHT_CENTER, if on { "ON" } else { "OFF" }, theme::mono(10.0), if on { theme::ACCENT_HOT } else { theme::TEXT_DIM });
    resp.on_hover_text(tooltip)
}

fn expand_button(ui: &mut Ui) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(r, theme::RADIUS_SM, if resp.hovered() { theme::RAISED_HI } else { theme::RAISED });
    // Two diagonal arrows ("expand").
    let (c, k) = (r.center(), 3.5);
    let stroke = Stroke::new(1.2, if resp.hovered() { theme::TEXT } else { theme::TEXT_DIM });
    p.line_segment([c + vec2(-k, k), c + vec2(k, -k)], stroke);
    p.line_segment([c + vec2(k, -k), c + vec2(k - 2.5, -k)], stroke);
    p.line_segment([c + vec2(k, -k), c + vec2(k, -k + 2.5)], stroke);
    p.line_segment([c + vec2(-k, k), c + vec2(-k + 2.5, k)], stroke);
    p.line_segment([c + vec2(-k, k), c + vec2(-k, k - 2.5)], stroke);
    resp.on_hover_text("Open the full EQ / compressor editor")
}

/// Horizontal gain-reduction bar (0–24 dB, fills right to left like a console GR meter).
fn gr_meter(ui: &mut Ui, insert: usize, gr_db: f32, width: f32) {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 7.0), Sense::click());
    let max_id = egui::Id::new(("gr-meter-max", insert));
    let mut max_gr: f32 = ui.ctx().data(|d| d.get_temp(max_id)).unwrap_or_else(|| {
        ui.ctx().data(|d| d.get_temp(egui::Id::new("comp_graph_highest_gr"))).unwrap_or(0.0)
    });
    if gr_db > max_gr {
        max_gr = gr_db;
        ui.ctx().data_mut(|d| {
            d.insert_temp(max_id, max_gr);
            d.insert_temp(egui::Id::new("comp_graph_highest_gr"), max_gr);
        });
    }
    if resp.clicked() {
        max_gr = 0.0;
        ui.ctx().data_mut(|d| {
            d.insert_temp(max_id, 0.0f32);
            d.insert_temp(egui::Id::new("comp_graph_highest_gr"), 0.0f32);
        });
    }

    let p = ui.painter();
    p.rect_filled(r, 1.0, theme::BG);
    let t = (gr_db / 24.0).clamp(0.0, 1.0);
    if t > 0.0 {
        p.rect_filled(Rect::from_min_max(pos2(r.max.x - r.width() * t, r.min.y), r.max), 1.0, theme::gr_color(gr_db));
    }
    for db in [3.0, 6.0, 12.0] {
        let x = r.max.x - r.width() * db / 24.0;
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], (1.0, theme::LINE));
    }
    if max_gr > 0.1 {
        let max_x = r.max.x - r.width() * (max_gr / 24.0).clamp(0.0, 1.0);
        p.line_segment([pos2(max_x, r.min.y), pos2(max_x, r.max.y)], (1.5, theme::ACCENT_HOT));
    }
    let tip = if gr_db >= theme::GR_HEAVY_DB {
        format!("Over-compressed: {gr_db:.1} dB GR\nHighest GR: {max_gr:.1} dB\n(Click to reset)")
    } else {
        format!("Gain reduction {gr_db:.1} dB\nHighest GR: {max_gr:.1} dB\n(Click to reset)")
    };
    resp.on_hover_text(tip);
}

/// The compact EQ + COMP section drawn inside a mixer strip.
pub fn compact(ui: &mut Ui, s: &mut Session, insert: usize, width: f32) -> Option<StripAction> {
    let mut st = s.strip(insert)?;
    let before = st;
    let mut out = None;
    let mut control = None;
    let gr = s.controls.inserts.get(insert).map_or(0.0, |c| c.gain_reduction.load());
    ui.spacing_mut().item_spacing = vec2(3.0, 2.0);

    // EQ
    ui.horizontal(|ui| {
        let r = power(ui, st.eq.on, "EQ", width - 18.0, "Channel EQ on/off (settings are kept when off)");
        if r.clicked() {
            st.eq.on = !st.eq.on;
        }
        if expand_button(ui).clicked() {
            out = Some(StripAction::Expand);
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let kw = width / 3.0;
        let size = (kw * 0.52).clamp(28.0, 38.0);
        let dim = !st.eq.on;
        knob(ui, &mut st, insert, channel_strip::LOW, "LOW", size, kw, dim, &mut control);
        knob(ui, &mut st, insert, channel_strip::MID, "MID", size, kw, dim, &mut control);
        knob(ui, &mut st, insert, channel_strip::HIGH, "HIGH", size, kw, dim, &mut control);
    });
    if st.eq.on && st.eq.hidden_processing() {
        ui.label(egui::RichText::new("+ more EQ").font(theme::ui_font(8.0)).color(theme::ACCENT_HOT)).on_hover_text("Low-mid/high-mid bands or HP/LP filters are active — open the editor to see them");
    }
    ui.add_space(2.0);

    // Compressor
    ui.horizontal(|ui| {
        let over = st.comp.on && gr >= theme::GR_HEAVY_DB;
        let r = power(ui, st.comp.on, if over { "COMP  OVER" } else { "COMP" }, width - 18.0, "Channel compressor on/off (settings are kept when off)");
        if over {
            ui.painter().rect_stroke(r.rect, theme::RADIUS_SM, (1.5, theme::ERROR), StrokeKind::Inside);
        }
        if r.clicked() {
            st.comp.on = !st.comp.on;
        }
        if expand_button(ui).clicked() {
            out = Some(StripAction::Expand);
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let kw = width / 2.0;
        let size = (kw * 0.42).clamp(28.0, 42.0);
        let dim = !st.comp.on;
        knob(ui, &mut st, insert, channel_strip::THRESHOLD, "THRESH", size, kw, dim, &mut control);
        knob(ui, &mut st, insert, channel_strip::RATIO, "RATIO", size, kw, dim, &mut control);
    });
    gr_meter(ui, insert, if st.comp.on { gr } else { 0.0 }, width);

    if st != before {
        s.set_strip(insert, st);
    }
    out.or(control.map(StripAction::Control))
}

/// Frequency axis helpers for the EQ graph (20 Hz – 20 kHz, logarithmic).
fn freq_to_x(f: f32, r: Rect) -> f32 {
    r.min.x + r.width() * ((f / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0)
}

fn x_to_freq(x: f32, r: Rect) -> f32 {
    20.0 * 1000f32.powf(((x - r.min.x) / r.width()).clamp(0.0, 1.0))
}

const GRAPH_DB: f32 = 15.0;

fn db_to_y(db: f32, r: Rect) -> f32 {
    r.center().y - db / GRAPH_DB * r.height() / 2.0
}

/// EQ response graph with draggable band handles. Returns true when settings changed.
fn eq_graph(ui: &mut Ui, st: &mut StripSettings, sample_rate: f32, height: f32) -> bool {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, theme::RADIUS, theme::BG);
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = freq_to_x(f, rect);
        p.line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], (1.0, theme::LINE_SOFT));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        p.text(pos2(x + 2.0, rect.max.y - 2.0), Align2::LEFT_BOTTOM, label, theme::mono(8.5), theme::TEXT_FAINT);
    }
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = db_to_y(db, rect);
        p.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], (1.0, if db == 0.0 { theme::LINE_STRONG } else { theme::LINE_SOFT }));
        p.text(pos2(rect.min.x + 2.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), theme::mono(8.5), theme::TEXT_FAINT);
    }

    let color = if st.eq.on { theme::ACCENT } else { theme::TEXT_FAINT };
    let n = (width as usize).clamp(64, 400);
    let curve: Vec<Pos2> = (0..n)
        .map(|i| {
            let x = rect.min.x + rect.width() * i as f32 / (n - 1) as f32;
            let db = st.eq_response_db(x_to_freq(x, rect), sample_rate).clamp(-GRAPH_DB * 1.5, GRAPH_DB * 1.5);
            pos2(x, db_to_y(db, rect))
        })
        .collect();
    for w in curve.windows(2) {
        let mid = rect.center().y;
        let poly = vec![w[0], w[1], pos2(w[1].x, mid), pos2(w[0].x, mid)];
        p.add(egui::Shape::convex_polygon(poly, theme::alpha(color, 0.12), Stroke::NONE));
    }
    p.add(egui::Shape::line(curve, Stroke::new(2.0, color)));

    // Handles: 5 bands (drag = freq + gain, scroll = Q) and the HP/LP filters (drag = freq).
    let mut changed = false;
    for (b, short) in ["L", "LM", "M", "HM", "H"].into_iter().enumerate() {
        let band = st.eq.bands[b];
        let c = pos2(freq_to_x(band.freq, rect), db_to_y(band.gain, rect));
        let hit = Rect::from_center_size(c, vec2(18.0, 18.0));
        let resp = ui.interact(hit, ui.id().with(("eq-band", b)), Sense::click_and_drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
            let (lo, hi) = match b {
                0 => (20.0, 1000.0),
                1 => (20.0, 5000.0),
                2 => (20.0, 20000.0),
                3 => (200.0, 20000.0),
                _ => (1000.0, 20000.0),
            };
            let f = x_to_freq(freq_to_x(band.freq, rect) + d.x * fine, rect).clamp(lo, hi);
            let g = (band.gain - d.y * fine * 2.0 * GRAPH_DB / rect.height()).clamp(-12.0, 12.0);
            st.eq.bands[b].freq = f;
            st.eq.bands[b].gain = g;
            changed = true;
        }
        if resp.hovered() && (1..=3).contains(&b) {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                st.eq.bands[b].q = (band.q * (scroll * 0.01).exp()).clamp(0.3, 15.0);
                changed = true;
            }
        }
        if resp.double_clicked() {
            st.eq.bands[b].gain = 0.0;
            changed = true;
        }
        let active = band.on && st.eq.on;
        let fill = if resp.hovered() || resp.dragged() { theme::ACCENT_HOT } else if active { theme::ACCENT } else { theme::RAISED_HI };
        p.circle(c, 7.0, fill, Stroke::new(1.0, theme::BG));
        p.text(c, Align2::CENTER_CENTER, short, theme::ui_font(7.5), theme::BG);
        let tip = format!("{} · {:.0} Hz · {:+.1} dB{}", band_name(b), band.freq, band.gain, if (1..=3).contains(&b) { format!(" · Q {:.2}", band.q) } else { String::new() });
        resp.on_hover_text(format!("{tip}\nDrag: frequency/gain · scroll: Q · double-click: reset gain"));
    }
    for (is_hp, on, freq) in [(true, st.eq.hp_on, st.eq.hp_freq), (false, st.eq.lp_on, st.eq.lp_freq)] {
        if !on {
            continue;
        }
        let c = pos2(freq_to_x(freq, rect), db_to_y(-3.0, rect));
        let resp = ui.interact(Rect::from_center_size(c, vec2(16.0, 16.0)), ui.id().with(("eq-pass", is_hp)), Sense::drag());
        if resp.dragged() {
            let f = x_to_freq(freq_to_x(freq, rect) + resp.drag_delta().x, rect);
            if is_hp {
                st.eq.hp_freq = f.clamp(20.0, 2000.0);
            } else {
                st.eq.lp_freq = f.clamp(1000.0, 20000.0);
            }
            changed = true;
        }
        p.rect(Rect::from_center_size(c, vec2(11.0, 11.0)), 2.0, theme::ACCENT_DIM, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
        resp.on_hover_text(format!("{} {:.0} Hz (12 dB/oct) · drag to move", if is_hp { "High-pass" } else { "Low-pass" }, freq));
    }
    p.rect_stroke(rect, theme::RADIUS, (1.0, theme::LINE), StrokeKind::Inside);
    changed
}

fn band_name(b: usize) -> &'static str {
    ["Low shelf", "Low-mid bell", "Mid bell", "High-mid bell", "High shelf"][b]
}

/// Compressor transfer curve with a point for the loudest signal sample in the last block.
fn comp_graph(ui: &mut Ui, st: &StripSettings, insert: usize, gr: f32, input_db: f32, output_db: f32, detector_db: f32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, theme::RADIUS, theme::BG);
    let to = |db: f32| (db + 60.0) / 60.0;
    for db in [-48.0, -36.0, -24.0, -12.0] {
        let t = to(db);
        p.line_segment([pos2(rect.min.x + rect.width() * t, rect.min.y), pos2(rect.min.x + rect.width() * t, rect.max.y)], (1.0, theme::LINE_SOFT));
        p.line_segment([pos2(rect.min.x, rect.max.y - rect.height() * t), pos2(rect.max.x, rect.max.y - rect.height() * t)], (1.0, theme::LINE_SOFT));
    }
    p.line_segment([rect.left_bottom(), rect.right_top()], (1.0, theme::LINE_STRONG));
    let pts: Vec<Pos2> = (0..=60).map(|i| {
        let input = -60.0 + i as f32;
        let out = st.comp_curve_db(input).clamp(-60.0, 0.0);
        pos2(rect.min.x + rect.width() * to(input), rect.max.y - rect.height() * to(out))
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, if st.comp.on { theme::ACCENT } else { theme::TEXT_FAINT })));
    let tx = rect.min.x + rect.width() * to(st.comp.threshold);
    p.line_segment([pos2(tx, rect.min.y), pos2(tx, rect.max.y)], (1.0, theme::alpha(theme::WARN, 0.5)));
    let ty = rect.max.y - rect.height() * to(st.comp.threshold);
    p.line_segment([pos2(rect.min.x, ty), pos2(rect.max.x, ty)], (1.0, theme::alpha(theme::WARN, 0.3)));

    // 1. Ratio readout in top-left corner
    let ratio_str = format!("{:.1}:1", st.comp.ratio);
    let ratio_rect = Rect::from_min_max(pos2(rect.min.x + 4.0, rect.min.y + 4.0), pos2(rect.min.x + 46.0, rect.min.y + 19.0));
    p.rect_filled(ratio_rect, 3.0, theme::alpha(theme::PANEL, 0.92));
    p.rect_stroke(ratio_rect, 3.0, (1.0, theme::alpha(theme::ACCENT_HOT, 0.5)), StrokeKind::Inside);
    p.text(ratio_rect.center(), Align2::CENTER_CENTER, ratio_str, theme::mono(10.0), theme::ACCENT_HOT);

    // 2. Threshold indicator
    let thr_str = format!("THR {:.0}", st.comp.threshold);
    let thr_near_right = tx > rect.max.x - 56.0;
    let thr_x = if thr_near_right { tx - 4.0 } else { tx + 4.0 };
    let thr_align = if thr_near_right { Align2::RIGHT_TOP } else { Align2::LEFT_TOP };
    let thr_bg = if thr_near_right {
        Rect::from_min_max(pos2(tx - 52.0, rect.min.y + 4.0), pos2(tx - 1.0, rect.min.y + 19.0))
    } else {
        Rect::from_min_max(pos2(tx + 1.0, rect.min.y + 4.0), pos2(tx + 52.0, rect.min.y + 19.0))
    };
    p.rect_filled(thr_bg, 3.0, theme::alpha(theme::PANEL, 0.92));
    p.text(pos2(thr_x, rect.min.y + 5.0), thr_align, thr_str, theme::mono(9.5), theme::WARN);

    p.text(rect.left_bottom() + vec2(5.0, -5.0), Align2::LEFT_BOTTOM, "OUTPUT dB", theme::mono(10.0), theme::TEXT_DIM);
    p.text(rect.right_bottom() - vec2(5.0, 5.0), Align2::RIGHT_BOTTOM, "INPUT dB", theme::mono(10.0), theme::TEXT_DIM);

    // 3. Live sample level indicators
    let has_signal = input_db > -59.5;
    let in_clamped = input_db.clamp(-60.0, 0.0);
    let out_clamped = output_db.clamp(-60.0, 0.0);

    // Subtle edge meter bars along INPUT (bottom) and OUTPUT (left) axes
    if has_signal {
        let in_w = rect.width() * to(in_clamped);
        p.rect_filled(Rect::from_min_max(pos2(rect.min.x, rect.max.y - 3.0), pos2(rect.min.x + in_w, rect.max.y)), 1.0, theme::alpha(theme::ACCENT, 0.65));
        let out_h = rect.height() * to(out_clamped);
        p.rect_filled(Rect::from_min_max(pos2(rect.min.x, rect.max.y - out_h), pos2(rect.min.x + 3.0, rect.max.y)), 1.0, theme::alpha(theme::ACCENT_HOT, 0.65));
    }

    // Dynamic transfer curve sample level point
    if has_signal {
        let dot = pos2(
            (rect.min.x + rect.width() * to(in_clamped)).clamp(rect.min.x + 8.0, rect.max.x - 8.0),
            (rect.max.y - rect.height() * to(out_clamped)).clamp(rect.min.y + 8.0, rect.max.y - 8.0),
        );
        p.circle_filled(dot, 8.0, theme::alpha(theme::ACCENT_HOT, 0.35));
        p.circle_filled(dot, 5.0, theme::TEXT);
        p.circle_stroke(dot, 7.5, Stroke::new(1.5, theme::ACCENT_HOT));

        // A sidechain or detector HPF can hear a different level from the audio itself.
        if detector_db > -59.5 {
            let key_x = (rect.min.x + rect.width() * to(detector_db.clamp(-60.0, 0.0))).clamp(rect.min.x + 4.0, rect.max.x - 4.0);
            p.circle_filled(pos2(key_x, rect.max.y - 8.0), 3.5, theme::WARN);
            p.circle_stroke(pos2(key_x, rect.max.y - 8.0), 5.0, Stroke::new(1.0, theme::alpha(theme::WARN, 0.5)));
        }
    } else {
        // Clear resting / parked indicator dot at baseline (-60 dB) so user always sees the sample level indicator
        let rest_dot = pos2(rect.min.x + 8.0, rect.max.y - 8.0);
        p.circle_filled(rest_dot, 3.5, theme::alpha(theme::TEXT_FAINT, 0.45));
        p.circle_stroke(rest_dot, 5.5, Stroke::new(1.0, theme::alpha(theme::LINE_STRONG, 0.7)));
        p.text(rest_dot + vec2(10.0, -1.0), Align2::LEFT_CENTER, "idle", theme::mono(8.5), theme::alpha(theme::TEXT_FAINT, 0.5));
    }
    p.rect_stroke(rect, theme::RADIUS, (1.0, theme::LINE), StrokeKind::Inside);

    // Vertical GR meter beside it with persistent highest GR marker.
    let (m, resp) = ui.allocate_exact_size(vec2(16.0, size), Sense::click());
    let p = ui.painter();
    p.rect_filled(m, 2.0, theme::BG);

    // Track highest GR (synchronized across channel and mixer)
    let peak_gr_id = egui::Id::new(("gr-meter-max", insert));
    let mut peak_gr: f32 = ui.ctx().data(|d| d.get_temp(peak_gr_id)).unwrap_or_else(|| {
        ui.ctx().data(|d| d.get_temp(egui::Id::new("comp_graph_highest_gr"))).unwrap_or(0.0)
    });
    if gr > peak_gr {
        peak_gr = gr;
        ui.ctx().data_mut(|d| {
            d.insert_temp(peak_gr_id, peak_gr);
            d.insert_temp(egui::Id::new("comp_graph_highest_gr"), peak_gr);
        });
    }
    if resp.clicked() {
        peak_gr = 0.0;
        ui.ctx().data_mut(|d| {
            d.insert_temp(peak_gr_id, 0.0f32);
            d.insert_temp(egui::Id::new("comp_graph_highest_gr"), 0.0f32);
        });
    }

    let t = (gr / 24.0).clamp(0.0, 1.0);
    p.rect_filled(Rect::from_min_max(m.min, pos2(m.max.x, m.min.y + m.height() * t)), 2.0, theme::gr_color(gr));

    // Draw Highest GR peak hold indicator line
    if peak_gr > 0.05 {
        let peak_y = m.min.y + m.height() * (peak_gr / 24.0).clamp(0.0, 1.0);
        p.line_segment([pos2(m.min.x, peak_y), pos2(m.max.x, peak_y)], (1.5, theme::ACCENT_HOT));
        // Numeric label at bottom of GR meter
        p.text(pos2(m.center().x, m.max.y - 3.0), Align2::CENTER_BOTTOM, format!("{:.0}", peak_gr), theme::mono(8.0), theme::ACCENT_HOT);
    } else {
        p.text(pos2(m.center().x, m.max.y - 3.0), Align2::CENTER_BOTTOM, "GR", theme::mono(7.5), theme::TEXT_FAINT);
    }
    p.rect_stroke(m, 2.0, (1.0, theme::LINE_SOFT), StrokeKind::Inside);
    resp.on_hover_text(format!("Live GR: {gr:.1} dB\nHighest GR on track: {peak_gr:.1} dB\n(Click to reset)"));
}

/// Recent block peaks make the threshold crossing and resulting output visible over time.
fn comp_history(ui: &mut Ui, insert: usize, threshold: f32, input_db: f32, output_db: f32, running: bool) {
    let id = egui::Id::new(("comp-history", insert));
    let mut history = ui.ctx().data_mut(|d| d.get_temp::<Vec<(f32, f32)>>(id)).unwrap_or_default();
    if running {
        history.push((input_db, output_db));
        if history.len() > 96 { history.remove(0); }
    } else {
        history.clear();
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, history.clone()));
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 85.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, theme::RADIUS, theme::BG);
    let y = |db: f32| rect.max.y - rect.height() * ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let ty = y(threshold);
    p.line_segment([pos2(rect.min.x, ty), pos2(rect.max.x, ty)], (1.0, theme::WARN));
    let text_y = (ty - 2.0).clamp(rect.min.y + 12.0, rect.max.y - 2.0);
    p.text(pos2(rect.min.x + 5.0, text_y), Align2::LEFT_BOTTOM, format!("Threshold {:.0} dB", threshold), theme::mono(10.0), theme::WARN);
    for (which, color) in [(0, theme::TEXT), (1, theme::ACCENT)] {
        let points: Vec<Pos2> = history.iter().enumerate().map(|(i, (input, output))| {
            let x = rect.max.x - (history.len() - 1 - i) as f32 * rect.width() / 95.0;
            pos2(x, y(if which == 0 { *input } else { *output }))
        }).collect();
        if points.len() > 1 { p.add(egui::Shape::line(points, Stroke::new(1.8, color))); }
    }
    if running && input_db < -90.0 {
        p.text(rect.center(), Align2::CENTER_CENTER, "No signal at this insert", theme::ui_font(11.0), theme::TEXT_DIM);
    } else if !running {
        p.text(rect.center(), Align2::CENTER_CENTER, "Play audio to see compression", theme::ui_font(11.0), theme::TEXT_DIM);
    }
    p.rect_stroke(rect, theme::RADIUS, (1.0, theme::LINE), StrokeKind::Inside);
}

/// The expanded editor window's contents. Returns a status message, if any.
pub fn editor(ui: &mut Ui, s: &mut Session, insert: usize) -> Option<String> {
    let mut st = s.strip(insert)?;
    let before = st;
    let mut control = None;
    let running = s.is_playing();
    let gr = s.controls.inserts.get(insert).map_or(0.0, |c| c.gain_reduction.load());
    let (input_db, output_db, detector_db) = s.controls.inserts.get(insert)
        .map_or((-120.0, -120.0, -120.0), |c| (c.comp_input_db.load(), c.comp_output_db.load(), c.comp_detector_db.load()));
    let sr = s.view.sample_rate as f32;

    // Full Machined Gunmetal Hardware Faceplate
    let faceplate_rect = ui.max_rect();
    theme::draw_hardware_faceplate(ui.painter(), faceplate_rect, theme::RADIUS_SM, theme::PANEL, true);

    // Machined Gunmetal Hardware Rack Unit Header
    let header_rect = ui.allocate_space(vec2(ui.available_width(), 26.0)).1;
    let p = ui.painter();
    theme::draw_gunmetal_surface(&p, header_rect, theme::RADIUS_SM, theme::PANEL_2);
    theme::draw_screw(&p, pos2(header_rect.min.x + 8.0, header_rect.center().y), 4.5, 0.4);
    theme::draw_screw(&p, pos2(header_rect.max.x - 8.0, header_rect.center().y), 4.5, 2.1);
    p.text(
        header_rect.center(),
        Align2::CENTER_CENTER,
        "ANALOG CHANNEL STRIP  ·  PRECISION CONSOLE HARDWARE",
        theme::mono(10.5),
        theme::TEXT_DIM,
    );
    ui.add_space(4.0);

    ui.label(egui::RichText::new("Signal order: input → EQ → compressor → effect slots → fader").small().color(theme::TEXT_FAINT));
    ui.add_space(4.0);

    // EQ
    ui.horizontal(|ui| {
        if power(ui, st.eq.on, "EQ", 60.0, "Channel EQ on/off").clicked() {
            st.eq.on = !st.eq.on;
        }
        ui.label(egui::RichText::new("5-band EQ  ·  drag points to shape the response").color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.checkbox(&mut st.eq.hp_on, "High-pass");
        ui.checkbox(&mut st.eq.lp_on, "Low-pass");
        });
    });
    eq_graph(ui, &mut st, sr, 190.0);
    ui.add_space(4.0);
    ui.columns(5, |cols| {
        for (b, params) in [[0, 1, 24], [2, 3, 4], [5, 6, 7], [8, 9, 10], [11, 12, 24]].into_iter().enumerate() {
            let ui = &mut cols[b];
            egui::Frame::group(ui.style()).fill(theme::alpha(theme::BG, 0.50)).stroke(Stroke::new(1.0, theme::LINE_SOFT)).corner_radius(theme::RADIUS_SM).show(ui, |ui| {
                ui.checkbox(&mut st.eq.bands[b].on, band_name(b));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let dim = !st.eq.on || !st.eq.bands[b].on;
                    for (n, param) in params.into_iter().filter(|p| *p != 24).enumerate() {
                        knob(ui, &mut st, insert, param, if n == 0 { "GAIN" } else if n == 1 { "FREQ" } else { "Q" }, 30.0, 54.0, dim, &mut control);
                    }
                });
            });
        }
    });
    egui::Frame::group(ui.style()).fill(theme::alpha(theme::BG, 0.40)).stroke(Stroke::new(1.0, theme::LINE_SOFT)).corner_radius(theme::RADIUS_SM).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("FILTERS").font(theme::mono(10.5)).color(theme::TEXT_DIM));
            let hp_dim = !st.eq.on || !st.eq.hp_on;
            let lp_dim = !st.eq.on || !st.eq.lp_on;
            knob(ui, &mut st, insert, 13, "HP FREQ", 30.0, 70.0, hp_dim, &mut control);
            knob(ui, &mut st, insert, 14, "LP FREQ", 30.0, 70.0, lp_dim, &mut control);
        });
    });
    ui.add_space(8.0);
    ui.separator();

    // Compressor
    ui.horizontal(|ui| {
        if power(ui, st.comp.on, "COMP", 70.0, "Channel compressor on/off").clicked() {
            st.comp.on = !st.comp.on;
        }
        ui.add_space(8.0);
        ui.checkbox(&mut st.comp.hpf_on, "Detector high-pass").on_hover_text("Filter lows out of what the compressor listens to, so bass doesn't make it pump");
        ui.checkbox(&mut st.comp.sidechain, "Sidechain").on_hover_text("Listen to other tracks' sidechain sends instead of this track (e.g. duck the bass under the kick). Set sources in a track's routing menu → Sidechain to.");
        let peak_gr_id = egui::Id::new(("gr-meter-max", insert));
        let mut peak_gr: f32 = ui.ctx().data(|d| d.get_temp(peak_gr_id)).unwrap_or_else(|| {
            ui.ctx().data(|d| d.get_temp(egui::Id::new("comp_graph_highest_gr"))).unwrap_or(0.0)
        });
        if gr > peak_gr {
            peak_gr = gr;
            ui.ctx().data_mut(|d| {
                d.insert_temp(peak_gr_id, peak_gr);
                d.insert_temp(egui::Id::new("comp_graph_highest_gr"), peak_gr);
            });
        }
        let gr_color = if gr > 0.1 { theme::gr_color(gr) } else { theme::TEXT_FAINT };
        ui.label(egui::RichText::new(format!("GR {gr:.1} dB")).font(theme::mono(11.0)).color(gr_color));

        // MAX GR clickable pill button: clicking it resets the highest GR reading!
        let has_peak = peak_gr > 0.05;
        let max_label = if has_peak { format!("MAX {peak_gr:.1} dB") } else { "MAX 0.0 dB".to_string() };
        let max_btn = ui.add(
            egui::Button::new(
                egui::RichText::new(max_label)
                    .font(theme::mono(11.0))
                    .color(if has_peak { theme::ACCENT_HOT } else { theme::TEXT_FAINT })
            )
            .fill(if has_peak { theme::alpha(theme::ACCENT_HOT, 0.16) } else { theme::PANEL_2 })
            .stroke(egui::Stroke::new(1.0, if has_peak { theme::alpha(theme::ACCENT_HOT, 0.6) } else { theme::LINE }))
            .corner_radius(3)
        );
        if max_btn.clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(peak_gr_id, 0.0f32);
                d.insert_temp(egui::Id::new("comp_graph_highest_gr"), 0.0f32);
            });
        }
        max_btn.on_hover_text("Highest gain reduction reached on this channel compressor. Click to reset.");

        if st.comp.on && gr >= theme::GR_HEAVY_DB {
            ui.label(egui::RichText::new("Over-compressed — raise the threshold or lower the ratio").small().color(theme::ERROR));
        }
    });
    if st.comp.sidechain {
        let sources = s.sidechain_sources(insert);
        if sources.is_empty() {
            ui.label(egui::RichText::new("Sidechain is on but nothing sends to it — the compressor won't react. Open another track's routing menu → Sidechain to.").small().color(theme::WARN));
        } else {
            let names: Vec<String> = sources.iter().map(|&i| s.view.inserts.get(i).map_or(format!("{i}"), |x| format!("{i} {}", x.name))).collect();
            ui.label(egui::RichText::new(format!("Keyed by: {}", names.join(", "))).small().color(theme::TEXT_DIM));
        }
    }
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        comp_graph(ui, &st, insert, if st.comp.on { gr } else { 0.0 }, input_db, output_db, detector_db, 188.0);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            egui::Frame::group(ui.style()).fill(theme::alpha(theme::BG, 0.40)).stroke(Stroke::new(1.0, theme::LINE_SOFT)).corner_radius(theme::RADIUS_SM).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let dim = !st.comp.on;
                    for (param, label) in [(15, "Threshold"), (16, "Ratio"), (17, "Attack"), (18, "Release"), (19, "Knee"), (20, "Makeup"), (21, "Mix")] {
                        knob(ui, &mut st, insert, param, label, 34.0, 70.0, dim, &mut control);
                    }
                    if st.comp.hpf_on {
                        knob(ui, &mut st, insert, 22, "Det. HPF", 34.0, 70.0, dim, &mut control);
                    }
                });
            });
        });
    });
    let peak_gr: f32 = ui.ctx().data(|d| d.get_temp(egui::Id::new(("gr-meter-max", insert)))).unwrap_or_else(|| {
        ui.ctx().data(|d| d.get_temp(egui::Id::new("comp_graph_highest_gr"))).unwrap_or(0.0)
    });
    let in_str = if input_db > -90.0 { format!("{input_db:.0} dB") } else { "-inf dB".to_string() };
    let out_str = if output_db > -90.0 { format!("{output_db:.0} dB") } else { "-inf dB".to_string() };
    let det_str = if detector_db > -90.0 { format!("{detector_db:.0} dB") } else { "-inf dB".to_string() };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("LIVE SIGNAL   In {in_str}   →   Out {out_str}   ·   Detector {det_str}   ·   GR {gr:.1} dB")).font(theme::mono(11.5)).color(theme::TEXT_DIM));
        ui.add_space(6.0);
        let has_peak = peak_gr > 0.05;
        let peak_label = if has_peak { format!("Highest GR {peak_gr:.1} dB") } else { "Highest GR 0.0 dB".to_string() };
        let reset_btn = ui.add(
            egui::Button::new(
                egui::RichText::new(peak_label)
                    .font(theme::mono(11.0))
                    .color(if has_peak { theme::ACCENT_HOT } else { theme::TEXT_FAINT })
            )
            .fill(if has_peak { theme::alpha(theme::ACCENT_HOT, 0.14) } else { theme::PANEL_2 })
            .stroke(egui::Stroke::new(1.0, if has_peak { theme::alpha(theme::ACCENT_HOT, 0.45) } else { theme::LINE }))
            .corner_radius(3)
        );
        if reset_btn.clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new(("gr-meter-max", insert)), 0.0f32);
                d.insert_temp(egui::Id::new("comp_graph_highest_gr"), 0.0f32);
            });
        }
        reset_btn.on_hover_text("Highest gain reduction reached on this channel compressor. Click to reset.");
    });
    comp_history(ui, insert, st.comp.threshold, input_db, output_db, (st.comp.on && running) || (st.comp.on && input_db > -59.5));
    ui.label(egui::RichText::new("White = input  ·  orange = output  ·  amber line = threshold. Attack, release, sidechain and mix can move the live point off the static ratio curve.").font(theme::ui_font(11.0)).color(theme::TEXT_DIM));
    ui.add_space(6.0);
    ui.separator();
    egui::Frame::group(ui.style()).fill(theme::alpha(theme::BG, 0.35)).stroke(Stroke::new(1.0, theme::LINE_SOFT)).corner_radius(theme::RADIUS_SM).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("STEREO").font(theme::mono(10.5)).color(theme::ACCENT));
            knob(ui, &mut st, insert, 23, "Width", 30.0, 56.0, false, &mut control);
            ui.vertical(|ui| {
                ui.checkbox(&mut st.stereo.swap, "Swap left / right");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut st.stereo.invert_left, "Invert L");
                    ui.checkbox(&mut st.stereo.invert_right, "Invert R");
                });
            });
            if !st.stereo.is_neutral() && ui.small_button("Reset").clicked() {
                st.stereo = Default::default();
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Reset EQ").clicked() {
            st.eq = engine::channel_strip::EqSettings { on: st.eq.on, ..Default::default() };
        }
        if ui.button("Reset compressor").clicked() {
            st.comp = engine::channel_strip::CompSettings { on: st.comp.on, ..Default::default() };
        }
        ui.label(egui::RichText::new("Right-click a knob to automate it or link a MIDI controller · Shift-drag for fine · double-click resets").small().color(theme::TEXT_FAINT));
    });
    if st.comp.on || gr > 0.01 || input_db > -59.5 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
    }

    if st != before {
        s.set_strip(insert, st);
    }
    control.map(|a| automation::apply(a, s))
}
