//! Custom-drawn controls: knobs, faders, meters, LEDs, step buttons.

use std::f32::consts::PI;

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::theme;

/// Rotary knob over `0.0..=1.0`. Drag up/down to change (Shift for fine),
/// double-click to reset. `bipolar` draws the arc from the centre (for pan).
pub fn knob(ui: &mut Ui, value: &mut f32, default: f32, diameter: f32, bipolar: bool, tooltip: impl FnOnce(f32) -> String) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click_and_drag());
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let delta = -response.drag_delta().y / if fine { 600.0 } else { 150.0 };
        *value = (*value + delta).clamp(0.0, 1.0);
        response.mark_changed();
    }
    if response.double_clicked() {
        *value = default;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let c = rect.center();
        let total_r = diameter * 0.5 - 0.5;
        let hot = response.hovered() || response.dragged();

        // 1. Recessed dark chassis socket
        painter.circle_filled(c, total_r, theme::METAL_SHADOW);
        painter.circle_stroke(c, total_r, Stroke::new(1.0, theme::LINE_SOFT));

        // 270° sweep, from 7:30 to 4:30.
        let (start, sweep) = (0.75 * PI, 1.5 * PI);
        let angle = |t: f32| start + sweep * t;

        // 2. Halo indicator track (outer ring)
        let track_r = total_r - 1.2;
        arc(painter, c, track_r, angle(0.0), angle(1.0), Stroke::new(2.0, Color32::from_rgb(20, 22, 28)));

        // Center detent notch at 12 o'clock for bipolar (e.g. pan) knobs.
        if bipolar {
            let detent_a = angle(0.5);
            let d_dir = vec2(detent_a.cos(), detent_a.sin());
            painter.line_segment([c + d_dir * (track_r - 2.5), c + d_dir * (track_r + 2.5)], Stroke::new(1.2, theme::LINE_STRONG));
        }

        // Active glowing LED arc
        let (from, to) = if bipolar { (0.5, *value) } else { (0.0, *value) };
        let (a0, a1) = if from <= to { (angle(from), angle(to)) } else { (angle(to), angle(from)) };
        let arc_color = if hot { theme::ACCENT_HOT } else { theme::ACCENT };
        arc(painter, c, track_r, a0, a1, Stroke::new(2.4, arc_color));

        // 3. Physical dial body (machined aluminum potentiometer cap)
        let cap_r = total_r - 3.8;
        if cap_r > 3.0 {
            // Drop shadow behind the raised metallic cap
            painter.circle_filled(c + vec2(0.0, 1.2), cap_r, theme::METAL_SHADOW);

            // Outer chamfered metallic collar
            painter.circle_filled(c, cap_r, theme::METAL_RIM_DARK);
            painter.circle_stroke(c, cap_r, Stroke::new(1.0, if hot { theme::METAL_SHINE } else { theme::METAL_RIM }));

            // Knurled edge notches for tactile grip (radial grooves around collar)
            if cap_r >= 10.0 {
                let notches = 16;
                for i in 0..notches {
                    let na = (i as f32 / notches as f32) * 2.0 * PI;
                    let ndir = vec2(na.cos(), na.sin());
                    let p1 = c + ndir * (cap_r - 1.6);
                    let p2 = c + ndir * cap_r;
                    painter.line_segment([p1, p2], Stroke::new(0.8, theme::alpha(theme::METAL_SHINE, 0.25)));
                }
            }

            // Inner brushed face
            let face_r = (cap_r - 1.8).max(2.0);
            painter.circle_filled(c, face_r, if hot { theme::METAL_FACE_HI } else { theme::METAL_FACE });

            // Brushed aluminum top specular crescent (light sheen hitting top bevel)
            painter.circle_stroke(c, face_r, Stroke::new(0.8, theme::alpha(Color32::WHITE, 0.15)));
            let highlight_c = c - vec2(0.0, face_r * 0.35);
            painter.circle_filled(highlight_c, face_r * 0.45, theme::alpha(Color32::WHITE, 0.08));

            // 4. Physical pointer needle / indicator line
            let a = angle(*value);
            let dir = vec2(a.cos(), a.sin());
            let line_start = c + dir * (face_r * 0.18);
            let line_end = c + dir * (face_r * 0.94);

            // Needle shadow
            painter.line_segment([line_start + vec2(0.5, 0.8), line_end + vec2(0.5, 0.8)], Stroke::new(2.0, theme::METAL_SHADOW));
            // Needle body
            painter.line_segment([line_start, line_end], Stroke::new(2.0, if hot { theme::ACCENT_HOT } else { Color32::WHITE }));

            // Center brass pin / screw axle
            painter.circle_filled(c, (face_r * 0.22).clamp(1.5, 3.2), theme::METAL_RIM_DARK);
            painter.circle_stroke(c, (face_r * 0.22).clamp(1.5, 3.2), Stroke::new(0.8, theme::METAL_SHINE));
        }
    }
    let text = tooltip(*value);
    response.on_hover_text(text)
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, a0: f32, a1: f32, stroke: Stroke) {
    if a1 - a0 < 0.01 {
        return;
    }
    let n = ((a1 - a0) / 0.12).ceil().max(2.0) as usize;
    let points: Vec<Pos2> = (0..=n).map(|i| {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        center + vec2(a.cos(), a.sin()) * radius
    }).collect();
    painter.add(Shape::line(points, stroke));
}

/// Round on/off light (channel and track mute).
pub fn led(ui: &mut Ui, on: bool, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    let r = size * 0.32;
    if on {
        painter.circle_filled(c, r + 2.5, theme::ACCENT_GLOW);
        painter.circle_filled(c, r, theme::ACCENT);
        painter.circle_filled(c - vec2(r * 0.3, r * 0.3), r * 0.35, theme::alpha(Color32::WHITE, 0.35));
    } else {
        painter.circle_filled(c, r, theme::RAISED);
        painter.circle_stroke(c, r, Stroke::new(1.0, theme::LINE_STRONG));
    }
    response
}

/// Physical tactile push button that depresses with active LED illumination when `active` (S/M, tool and mode buttons).
pub fn toggle(ui: &mut Ui, active: bool, label: &str, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let painter = ui.painter();

    // 1. Recessed socket shadow
    painter.rect_filled(rect.translate(vec2(0.0, 1.0)), theme::RADIUS_SM, theme::METAL_SHADOW);

    // 2. Button cap geometry (shifts down by 1px when physically pressed or active)
    let cap_rect = if active || pressed {
        rect.translate(vec2(0.0, 1.0))
    } else {
        rect
    };

    let (fill, stroke, text) = if active {
        (theme::ACCENT_DIM, theme::ACCENT, Color32::WHITE)
    } else if hovered {
        (theme::METAL_FACE_HI, theme::LINE_STRONG, theme::TEXT)
    } else {
        (theme::METAL_FACE, theme::LINE, theme::TEXT_DIM)
    };

    // 3. Machined button surface & perimeter
    painter.rect(cap_rect, theme::RADIUS_SM, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);

    // 4. Metallic top specular bevel (light reflection along top edge)
    if !active && !pressed {
        let top_highlight = Rect::from_min_max(
            pos2(cap_rect.min.x + 1.5, cap_rect.min.y + 0.6),
            pos2(cap_rect.max.x - 1.5, cap_rect.min.y + 1.8),
        );
        painter.rect_filled(top_highlight, 0.5, theme::alpha(Color32::WHITE, if hovered { 0.18 } else { 0.10 }));
    }

    // 5. Active indicator LED pip & text rendering
    if active && rect.width() >= 36.0 {
        let pip_x = cap_rect.min.x + 6.5;
        let pip_c = pos2(pip_x, cap_rect.center().y);
        painter.circle_filled(pip_c, 3.2, theme::ACCENT_GLOW);
        painter.circle_filled(pip_c, 1.8, theme::ACCENT_HOT);
        painter.text(pos2(cap_rect.center().x + 3.0, cap_rect.center().y), Align2::CENTER_CENTER, label, theme::ui_font(11.0), text);
    } else {
        painter.text(cap_rect.center(), Align2::CENTER_CENTER, label, theme::ui_font(11.0), if active { theme::ACCENT_HOT } else { text });
    }
    response
}

/// Mini real-time activity LED (for channel rack and mixer tracks).
#[allow(dead_code)]
pub fn activity_led(painter: &egui::Painter, center: Pos2, active: bool, peak_db: f32) {
    let r = 2.5;
    if active {
        let color = if peak_db > -2.0 {
            theme::ERROR
        } else if peak_db > -10.0 {
            theme::WARN
        } else {
            theme::STATUS_PLAY
        };
        painter.circle_filled(center, r + 2.0, theme::alpha(color, 0.4));
        painter.circle_filled(center, r, color);
    } else {
        painter.circle_filled(center, r, theme::RAISED);
        painter.circle_stroke(center, r, Stroke::new(0.8, theme::LINE));
    }
}

/// Draws a tactile step-sequencer pad. `group_alt` alternates the shade every beat;
/// `playing` marks the step under the playhead.
pub fn step_pad(painter: &egui::Painter, rect: Rect, on: bool, group_alt: bool, hovered: bool, playing: bool) {
    let r = theme::RADIUS_SM;
    // Outer recessed socket border
    let socket = rect.expand(1.0);
    painter.rect_filled(socket, r, theme::BG);
    painter.rect_stroke(socket, r, Stroke::new(1.0, theme::LINE_SOFT), StrokeKind::Inside);

    if on {
        painter.rect_filled(rect, r, if hovered { theme::ACCENT_DIM } else { theme::ACCENT_SUBTLE });
        painter.rect_stroke(rect, r, Stroke::new(1.0, if hovered { theme::ACCENT_HOT } else { theme::ACCENT }), StrokeKind::Inside);

        // Center glowing LED core
        let led_w = (rect.width() * 0.6).min(12.0);
        let led_h = (rect.height() * 0.35).min(7.0);
        let led_rect = Rect::from_center_size(rect.center(), vec2(led_w, led_h));
        painter.rect_filled(led_rect.expand(2.0), theme::RADIUS_SM, theme::ACCENT_GLOW);
        painter.rect_filled(led_rect, theme::RADIUS_SM, if hovered { theme::ACCENT_HOT } else { theme::ACCENT });

        // Glass top highlight
        let top = Rect::from_min_max(rect.min + vec2(1.5, 1.5), pos2(rect.max.x - 1.5, rect.min.y + 4.0));
        painter.rect_filled(top, r, theme::alpha(Color32::WHITE, 0.30));
    } else {
        // Distinct beat alternation: Beats 0/2 vs Beats 1/3
        let base = if group_alt { Color32::from_rgb(0x2A, 0x2A, 0x36) } else { Color32::from_rgb(0x1F, 0x1F, 0x27) };
        let fill = if hovered { theme::RAISED_HI } else { base };
        painter.rect_filled(rect, r, fill);
        painter.rect_stroke(rect, r, Stroke::new(1.0, if hovered { theme::LINE_STRONG } else { theme::LINE }), StrokeKind::Inside);

        // Inset center groove
        let slit_w = (rect.width() * 0.45).min(8.0);
        let slit_h = 2.5;
        let slit = Rect::from_center_size(rect.center(), vec2(slit_w, slit_h));
        painter.rect_filled(slit, 1.0, if hovered { theme::LINE_STRONG } else { theme::LINE_SOFT });

        // Subtle 3D top bevel
        let top = Rect::from_min_max(rect.min + vec2(1.5, 1.0), pos2(rect.max.x - 1.5, rect.min.y + 2.5));
        painter.rect_filled(top, r, theme::alpha(Color32::WHITE, 0.08));
    }

    if playing {
        // High-visibility playhead beam
        painter.rect_stroke(rect.expand(1.5), r, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    }
}

/// A small numeric box changed by dragging vertically or scrolling (mixer insert routing).
pub fn number_box(ui: &mut Ui, value: &mut usize, max: usize, size: Vec2, tooltip: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let id = response.id.with("acc");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, 0.0f32));
    }
    if response.dragged() {
        let acc = ui.data(|d| d.get_temp::<f32>(id).unwrap_or(0.0)) - response.drag_delta().y;
        let steps = (acc / 8.0).trunc();
        if steps != 0.0 {
            *value = (*value as i64 + steps as i64).clamp(0, max as i64) as usize;
            response.mark_changed();
        }
        ui.data_mut(|d| d.insert_temp(id, acc - steps * 8.0));
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            *value = (*value as i64 + scroll.signum() as i64).clamp(0, max as i64) as usize;
            response.mark_changed();
        }
    }
    let painter = ui.painter();
    painter.rect(rect, theme::RADIUS_SM, theme::BG, Stroke::new(1.0, if response.hovered() { theme::LINE_STRONG } else { theme::LINE }), StrokeKind::Inside);
    let label = if *value == 0 { "M".to_string() } else { value.to_string() };
    painter.text(rect.center(), Align2::CENTER_CENTER, label, theme::mono(11.0), theme::TEXT);
    response.on_hover_text(tooltip)
}

/// Meter readout kept by the UI between frames (smoothed fall, peak hold, clip latch).
#[derive(Clone, Copy)]
pub struct MeterState {
    level: [f32; 2],
    hold: [f32; 2],
    hold_until: [f64; 2],
    pub clipped: bool,
}

impl Default for MeterState {
    fn default() -> Self {
        Self { level: [-90.0; 2], hold: [-90.0; 2], hold_until: [0.0; 2], clipped: false }
    }
}

pub const METER_MIN_DB: f32 = -60.0;
pub const METER_MAX_DB: f32 = 6.0;

pub fn to_db(gain: f32) -> f32 {
    if gain <= 1e-6 { -120.0 } else { 20.0 * gain.log10() }
}

impl MeterState {
    /// Feeds new peaks (linear) for this frame.
    pub fn update(&mut self, peaks: [f32; 2], now: f64, dt: f32) {
        const FALL_DB_PER_SEC: f32 = 24.0;
        const HOLD_SECS: f64 = 1.5;
        for (c, peak) in peaks.iter().enumerate() {
            let db = to_db(*peak);
            self.level[c] = db.max(self.level[c] - FALL_DB_PER_SEC * dt);
            if db >= self.hold[c] || now > self.hold_until[c] {
                self.hold[c] = db.max(self.level[c]);
                self.hold_until[c] = now + HOLD_SECS;
            }
            if *peak > 1.0 {
                self.clipped = true;
            }
        }
    }

    pub fn active(&self) -> bool {
        self.level.iter().any(|l| *l > METER_MIN_DB)
    }
}

/// Position (0 at the bottom, 1 at the top) of `db` on a meter or fader scale.
pub fn db_position(db: f32) -> f32 {
    ((db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0).powf(1.6)
}

/// Stereo peak meter with hold lines and a clip light on top (click it to reset).
pub fn meter(ui: &mut Ui, state: &mut MeterState, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        state.clipped = false;
    }
    let painter = ui.painter();
    let clip_h = 4.0;
    let clip = Rect::from_min_size(rect.min, vec2(rect.width(), clip_h));
    painter.rect_filled(clip, theme::RADIUS_SM, if state.clipped { theme::ERROR } else { theme::RAISED });
    let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + clip_h + 2.0), rect.max);
    painter.rect_filled(body, theme::RADIUS_SM, theme::BG);

    let lane_w = (body.width() - 1.0) / 2.0;
    for c in 0..2 {
        let x0 = body.min.x + c as f32 * (lane_w + 1.0);
        let lane = Rect::from_min_size(pos2(x0, body.min.y), vec2(lane_w, body.height()));
        let top = lane.max.y - lane.height() * db_position(state.level[c]);
        // Three zones: quiet, normal, hot.
        for (lo, hi, color) in [(-120.0, -18.0, theme::ACCENT_DIM), (-18.0, -6.0, theme::ACCENT), (-6.0, 120.0, theme::ACCENT_HOT)] {
            let zone_top = lane.max.y - lane.height() * db_position(hi);
            let zone_bot = lane.max.y - lane.height() * db_position(lo);
            let y0 = zone_top.max(top);
            if y0 < zone_bot {
                painter.rect_filled(Rect::from_min_max(pos2(lane.min.x, y0), pos2(lane.max.x, zone_bot)), 0.0, color);
            }
        }
        if state.hold[c] > METER_MIN_DB {
            let y = lane.max.y - lane.height() * db_position(state.hold[c]);
            painter.line_segment([pos2(lane.min.x, y), pos2(lane.max.x, y)], Stroke::new(1.0, theme::TEXT));
        }
    }
    response
}

/// Vertical fader over `0.0..=1.0` where 0.8 is unity (FL's convention). Drag the
/// cap, Shift for fine control, double-click for unity.
pub fn fader(ui: &mut Ui, pos: &mut f32, size: Vec2) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let travel = rect.height() - 20.0;
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        *pos = (*pos - response.drag_delta().y / travel * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
        response.mark_changed();
    }
    if response.double_clicked() {
        *pos = 0.8;
        response.mark_changed();
    }
    let painter = ui.painter();
    let slot = Rect::from_center_size(pos2(rect.center().x, rect.center().y), vec2(5.0, travel));
    painter.rect_filled(slot, 2.0, Color32::from_rgb(0x08, 0x08, 0x0D));
    painter.rect_stroke(slot, 2.0, Stroke::new(1.0, theme::LINE_SOFT), StrokeKind::Outside);
    // Center recess trace
    painter.line_segment([pos2(rect.center().x, slot.min.y + 1.0), pos2(rect.center().x, slot.max.y - 1.0)], Stroke::new(1.0, Color32::from_rgb(0x04, 0x04, 0x07)));

    // Unity mark.
    let unity_y = slot.max.y - travel * 0.8;
    painter.line_segment([pos2(rect.min.x + 2.0, unity_y), pos2(rect.max.x - 2.0, unity_y)], Stroke::new(1.2, theme::LINE_STRONG));

    let cap_y = slot.max.y - travel * *pos;
    let cap_w = (rect.width() - 2.0).clamp(18.0, 26.0);
    let cap = Rect::from_center_size(pos2(rect.center().x, cap_y), vec2(cap_w, 18.0));
    let hot = response.hovered() || response.dragged();

    // Drop shadow
    painter.rect_filled(cap.translate(vec2(0.0, 1.5)), theme::RADIUS_SM, theme::alpha(Color32::BLACK, 0.45));
    // Cap body
    let cap_fill = if hot { Color32::from_rgb(0x35, 0x35, 0x44) } else { Color32::from_rgb(0x28, 0x28, 0x33) };
    painter.rect(cap, theme::RADIUS_SM, cap_fill, Stroke::new(1.0, if hot { theme::LINE_STRONG } else { theme::LINE }), StrokeKind::Inside);

    // Bevel edges
    let top_edge = Rect::from_min_max(pos2(cap.min.x + 1.0, cap.min.y + 1.0), pos2(cap.max.x - 1.0, cap.min.y + 2.5));
    painter.rect_filled(top_edge, 1.0, theme::alpha(Color32::WHITE, 0.20));
    let bot_edge = Rect::from_min_max(pos2(cap.min.x + 1.0, cap.max.y - 2.5), pos2(cap.max.x - 1.0, cap.max.y - 1.0));
    painter.rect_filled(bot_edge, 1.0, theme::alpha(Color32::BLACK, 0.40));

    // Grip ridges
    for dy in [-4.0, 4.0] {
        painter.line_segment([pos2(cap.center().x - 6.0, cap_y + dy), pos2(cap.center().x + 6.0, cap_y + dy)], Stroke::new(1.0, theme::LINE_SOFT));
    }
    // Center illuminated indicator line
    painter.line_segment([pos2(cap.min.x + 2.0, cap_y), pos2(cap.max.x - 2.0, cap_y)], Stroke::new(2.0, theme::ACCENT));
    response
}

/// dB scale labels beside a fader of the given travel.
pub fn fader_scale(painter: &egui::Painter, rect: Rect) {
    let travel = rect.height() - 20.0;
    let bottom = rect.center().y + travel / 2.0;
    for db in [6.0f32, 0.0, -6.0, -12.0, -24.0, -48.0] {
        let gain = 10f32.powf(db / 20.0);
        let pos = engine::gain::fl_volume_position(gain).min(1.0);
        let y = bottom - travel * pos;
        let is_unity = db == 0.0;
        painter.text(pos2(rect.max.x, y), Align2::RIGHT_CENTER, format!("{}", db as i32), theme::mono(8.5), if is_unity { theme::TEXT } else { theme::TEXT_FAINT });
    }
}

/// Play triangle / stop square / record dot icons for transport buttons.
pub fn transport_icon(painter: &egui::Painter, rect: Rect, kind: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.height() * 0.28;
    match kind {
        Icon::Play => {
            let pts = vec![c + vec2(-s * 0.75, -s), c + vec2(s * 1.05, 0.0), c + vec2(-s * 0.75, s)];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 1.6)), 1.5, color);
        }
        Icon::Record => {
            painter.circle_filled(c, s * 0.95, color);
        }
        Icon::Loop => {
            arc(painter, c, s, 0.3, 2.0 * PI - 0.3, Stroke::new(1.6, color));
            let tip = c + vec2((0.3f32).cos(), (0.3f32).sin()) * s;
            painter.add(Shape::convex_polygon(vec![tip + vec2(-3.0, -1.0), tip + vec2(3.0, -1.0), tip + vec2(0.0, 3.5)], color, Stroke::NONE));
        }
    }
}

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Stop,
    Record,
    Loop,
}

/// A square transport/tool button with an icon and semantic status coloring.
pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, size: f32, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();

    // 1. Recessed chassis socket shadow
    painter.rect_filled(rect.translate(vec2(0.0, 1.0)), theme::RADIUS, theme::METAL_SHADOW);

    // 2. Physical button cap geometry
    let cap_rect = if active || pressed { rect.translate(vec2(0.0, 1.0)) } else { rect };

    let (fill, fg, stroke) = match (icon, active) {
        (Icon::Play, true) => (theme::STATUS_PLAY_GLOW, theme::STATUS_PLAY, theme::STATUS_PLAY),
        (Icon::Record, true) => (theme::STATUS_REC_GLOW, theme::STATUS_REC, theme::STATUS_REC),
        (_, true) => (theme::ACCENT_DIM, theme::ACCENT_HOT, theme::ACCENT),
        (_, false) if hovered => (theme::METAL_FACE_HI, theme::TEXT, theme::LINE_STRONG),
        (_, false) => (theme::METAL_FACE, theme::TEXT_DIM, theme::LINE),
    };

    // 3. Machined button surface & perimeter
    painter.rect(cap_rect, theme::RADIUS, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);

    // 4. Metallic top specular reflection
    if !active && !pressed {
        let top_highlight = Rect::from_min_max(
            pos2(cap_rect.min.x + 2.0, cap_rect.min.y + 0.6),
            pos2(cap_rect.max.x - 2.0, cap_rect.min.y + 2.0),
        );
        painter.rect_filled(top_highlight, 0.5, theme::alpha(Color32::WHITE, if hovered { 0.22 } else { 0.12 }));
    }

    transport_icon(painter, cap_rect, icon, fg);
    response.on_hover_text(tooltip)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowIcon {
    Browser,
    Playlist,
    ChannelRack,
    PianoRoll,
    Mixer,
}

/// Draw a vector icon representing a workspace window.
pub fn draw_window_icon(painter: &egui::Painter, rect: Rect, icon: WindowIcon, active: bool, color: Color32) {
    let c = pos2(rect.center().x, rect.center().y - 0.5);
    match icon {
        WindowIcon::Browser => {
            // Folder icon with tab and inner file lines
            let min_x = c.x - 7.0;
            let max_x = c.x + 7.0;
            let min_y = c.y - 5.5;
            let max_y = c.y + 5.5;

            // Tab at top-left
            let tab = Rect::from_min_max(pos2(min_x, min_y), pos2(min_x + 5.5, min_y + 2.5));
            painter.rect_filled(tab, 1.0, if active { theme::ACCENT_DIM } else { theme::RAISED_HI });
            painter.rect_stroke(tab, 1.0, Stroke::new(1.0, color), StrokeKind::Inside);

            // Folder body
            let body = Rect::from_min_max(pos2(min_x, min_y + 2.0), pos2(max_x, max_y));
            painter.rect(
                body,
                1.5,
                if active { theme::ACCENT_SUBTLE } else { theme::PANEL_2 },
                Stroke::new(1.2, color),
                StrokeKind::Inside,
            );

            // Inner content lines
            let line_color = if active { theme::ACCENT_HOT } else { theme::TEXT_FAINT };
            painter.line_segment([pos2(min_x + 3.0, c.y + 0.5), pos2(max_x - 3.0, c.y + 0.5)], Stroke::new(1.0, line_color));
            painter.line_segment([pos2(min_x + 3.0, c.y + 3.0), pos2(max_x - 5.0, c.y + 3.0)], Stroke::new(1.0, line_color));
        }
        WindowIcon::Playlist => {
            // Multi-track arranger timeline with staggered clip blocks
            let min_x = c.x - 7.0;
            let max_x = c.x + 7.0;

            // Track lanes (subtle guide lines)
            let lane_stroke = Stroke::new(0.8, if active { theme::ACCENT_DIM } else { theme::LINE_SOFT });
            for dy in [-4.4, 0.0, 4.4] {
                painter.line_segment([pos2(min_x, c.y + dy), pos2(max_x, c.y + dy)], lane_stroke);
            }

            // Clips
            let clips = [
                // Track 0
                Rect::from_min_size(pos2(min_x, c.y - 5.6), vec2(5.5, 2.4)),
                Rect::from_min_size(pos2(min_x + 7.0, c.y - 5.6), vec2(7.0, 2.4)),
                // Track 1
                Rect::from_min_size(pos2(min_x + 2.5, c.y - 1.2), vec2(9.0, 2.4)),
                // Track 2
                Rect::from_min_size(pos2(min_x, c.y + 3.2), vec2(7.0, 2.4)),
                Rect::from_min_size(pos2(min_x + 8.5, c.y + 3.2), vec2(5.5, 2.4)),
            ];
            for clip in clips {
                painter.rect_filled(clip, 0.8, color);
            }
        }
        WindowIcon::ChannelRack => {
            // 2 rows of 3 sequencer step buttons
            let pad_w = 3.6;
            let pad_h = 4.2;
            let col_step = 5.2;
            let row_step = 6.2;
            let x0 = c.x - col_step;
            let y0 = c.y - row_step * 0.5;

            for row in 0..2 {
                for col in 0..3 {
                    let pad_rect = Rect::from_center_size(
                        pos2(x0 + col as f32 * col_step, y0 + row as f32 * row_step),
                        vec2(pad_w, pad_h),
                    );
                    let is_active_step = matches!((row, col), (0, 0) | (0, 2) | (1, 1) | (1, 2));
                    if is_active_step {
                        painter.rect_filled(pad_rect, 0.8, color);
                    } else {
                        painter.rect(
                            pad_rect,
                            0.8,
                            if active { theme::ACCENT_SUBTLE } else { theme::PANEL },
                            Stroke::new(1.0, if active { theme::ACCENT_DIM } else { theme::LINE_STRONG }),
                            StrokeKind::Inside,
                        );
                    }
                }
            }
        }
        WindowIcon::PianoRoll => {
            // Mini piano keyboard: white keys bed with black keys
            let min_x = c.x - 7.0;
            let max_x = c.x + 7.0;
            let min_y = c.y - 5.5;
            let max_y = c.y + 5.5;
            let frame = Rect::from_min_max(pos2(min_x, min_y), pos2(max_x, max_y));

            // White keys bed
            painter.rect(
                frame,
                1.0,
                if active { theme::ACCENT_SUBTLE } else { Color32::from_rgb(0x1E, 0x1E, 0x26) },
                Stroke::new(1.0, color),
                StrokeKind::Inside,
            );

            // White key dividers
            for dx in [3.5, 7.0, 10.5] {
                painter.line_segment(
                    [pos2(min_x + dx, min_y), pos2(min_x + dx, max_y)],
                    Stroke::new(0.8, if active { theme::ACCENT_DIM } else { theme::LINE_STRONG }),
                );
            }

            // Black keys hanging from top
            let bk_h = 6.0;
            let bk_w = 2.0;
            let bk_color = color;
            painter.rect_filled(Rect::from_center_size(pos2(min_x + 3.5, min_y + bk_h * 0.5), vec2(bk_w, bk_h)), 0.6, bk_color);
            painter.rect_filled(Rect::from_center_size(pos2(min_x + 8.8, min_y + bk_h * 0.5), vec2(bk_w, bk_h)), 0.6, bk_color);
        }
        WindowIcon::Mixer => {
            // 3 vertical fader channel sliders with handle caps
            let xs = [c.x - 5.0, c.x, c.x + 5.0];
            let stem_stroke = Stroke::new(1.2, if active { theme::ACCENT_DIM } else { theme::LINE_STRONG });
            for &x in &xs {
                painter.line_segment([pos2(x, c.y - 6.0), pos2(x, c.y + 6.0)], stem_stroke);
            }

            // Slider thumb caps at staggered levels
            let caps = [(xs[0], c.y - 2.5), (xs[1], c.y + 2.5), (xs[2], c.y - 0.5)];
            for (cap_x, cap_y) in caps {
                let cap_rect = Rect::from_center_size(pos2(cap_x, cap_y), vec2(4.4, 2.6));
                painter.rect_filled(cap_rect, 0.8, color);
                painter.line_segment(
                    [pos2(cap_x - 1.5, cap_y), pos2(cap_x + 1.5, cap_y)],
                    Stroke::new(1.0, if active { theme::TEXT } else { theme::BG }),
                );
            }
        }
    }
}

/// Toggle button with a vector icon for workspace windows, showing active LED bar and tooltip on hover.
pub fn window_toggle(ui: &mut Ui, active: bool, icon: WindowIcon, label: &str, shortcut: &str, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let painter = ui.painter();

    // 1. Recessed chassis socket shadow
    painter.rect_filled(rect.translate(vec2(0.0, 1.0)), theme::RADIUS_SM, theme::METAL_SHADOW);

    // 2. Button cap geometry
    let cap_rect = if active || pressed { rect.translate(vec2(0.0, 1.0)) } else { rect };

    let (fill, stroke, icon_color) = if active {
        (theme::ACCENT_DIM, theme::ACCENT, theme::ACCENT_HOT)
    } else if hovered {
        (theme::METAL_FACE_HI, theme::LINE_STRONG, theme::TEXT)
    } else {
        (theme::METAL_FACE, theme::LINE, theme::TEXT_DIM)
    };

    // 3. Machined button surface & perimeter
    painter.rect(cap_rect, theme::RADIUS_SM, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);

    // 4. Metallic top specular reflection
    if !active && !pressed {
        let top_highlight = Rect::from_min_max(
            pos2(cap_rect.min.x + 1.5, cap_rect.min.y + 0.6),
            pos2(cap_rect.max.x - 1.5, cap_rect.min.y + 1.8),
        );
        painter.rect_filled(top_highlight, 0.5, theme::alpha(Color32::WHITE, if hovered { 0.20 } else { 0.10 }));
    }

    draw_window_icon(painter, cap_rect, icon, active, icon_color);

    if active {
        let led_y = cap_rect.max.y - 2.0;
        let led_bar = Rect::from_min_max(pos2(cap_rect.min.x + 4.0, led_y - 1.2), pos2(cap_rect.max.x - 4.0, led_y + 0.3));
        painter.rect_filled(led_bar, 0.8, theme::ACCENT_HOT);
    }

    response.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(label).color(theme::TEXT).strong());
            ui.label(egui::RichText::new(shortcut).color(theme::ACCENT_HOT).font(theme::mono(10.5)));
        });
    })
}
