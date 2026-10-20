//! The inspector: exact values for whatever is selected (playlist clip, channel, pattern),
//! editable in one place.

use eframe::egui::{self, Ui};
use engine::view::ClipKind;

use super::session::Session;
use super::theme;

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title.to_uppercase()).font(theme::ui_font(10.5)).color(theme::ACCENT));
    ui.add_space(2.0);
}

/// Ticks shown as bars (1-based) with a fractional part, e.g. 3.25 = bar 3, second beat.
fn bars(s: &Session, ticks: u64) -> f64 {
    ticks as f64 / s.ticks_per_bar().max(1) as f64
}

/// Draws the inspector. `selected` are the playlist's selected clips.
pub fn show(ui: &mut Ui, s: &mut Session, selected: &[usize]) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(egui::RichText::new("INSPECTOR").font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
        clip(ui, s, selected);
        channel(ui, s);
        pattern(ui, s);
    });
}

fn clip(ui: &mut Ui, s: &mut Session, selected: &[usize]) {
    let [ci] = selected else {
        if selected.len() > 1 {
            section(ui, "Clips");
            ui.label(format!("{} clips selected", selected.len()));
        }
        return;
    };
    let Some(mut c) = s.view.clips.get(*ci).cloned() else { return };
    let before = c.clone();
    section(ui, match c.kind {
        ClipKind::Audio => "Audio clip",
        ClipKind::Pattern => "Pattern clip",
        ClipKind::Automation => "Automation clip",
    });
    ui.label(egui::RichText::new(&c.label).strong());
    let per_bar = s.ticks_per_bar().max(1) as f64;
    egui::Grid::new("insp-clip").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label("Start (bar)");
        let mut start = bars(s, c.start) + 1.0;
        if ui.add(egui::DragValue::new(&mut start).range(1.0..=10_000.0).speed(0.01).fixed_decimals(3)).changed() {
            c.start = ((start - 1.0).max(0.0) * per_bar).round() as u64;
        }
        ui.end_row();
        ui.label("Length (bars)");
        let mut len = bars(s, c.length);
        if ui.add(egui::DragValue::new(&mut len).range(0.01..=10_000.0).speed(0.01).fixed_decimals(3)).changed() {
            c.length = (len * per_bar).round().max(1.0) as u64;
        }
        ui.end_row();
        ui.label("Track");
        let mut track = c.track + 1;
        if ui.add(egui::DragValue::new(&mut track).range(1..=s.view.tracks.len().max(1))).changed() {
            c.track = track - 1;
        }
        ui.end_row();
        if c.kind == ClipKind::Audio {
            ui.label("Gain");
            let mut db = if c.gain <= 1e-5 { -60.0 } else { 20.0 * c.gain.log10() };
            if ui.add(egui::DragValue::new(&mut db).range(-60.0..=12.0).speed(0.1).fixed_decimals(1).suffix(" dB")).changed() {
                c.gain = 10f32.powf(db / 20.0);
            }
            ui.end_row();
            let beat = (per_bar / s.view.beats_per_bar.max(1) as f64).max(1.0);
            ui.label("Fade in (beats)");
            let mut fi = c.fade_in as f64 / beat;
            if ui.add(egui::DragValue::new(&mut fi).range(0.0..=256.0).speed(0.02).fixed_decimals(2)).changed() {
                c.fade_in = ((fi * beat).round() as u64).min(c.length.saturating_sub(c.fade_out));
            }
            ui.end_row();
            ui.label("Fade out (beats)");
            let mut fo = c.fade_out as f64 / beat;
            if ui.add(egui::DragValue::new(&mut fo).range(0.0..=256.0).speed(0.02).fixed_decimals(2)).changed() {
                c.fade_out = ((fo * beat).round() as u64).min(c.length.saturating_sub(c.fade_in));
            }
            ui.end_row();
            ui.label("Starts at");
            let mut off = c.source_offset;
            if ui.add(egui::DragValue::new(&mut off).range(0.0..=36_000.0).speed(0.005).fixed_decimals(3).suffix(" s into the file")).changed() {
                c.source_offset = off.max(0.0);
            }
            ui.end_row();
            ui.label("Reverse");
            ui.checkbox(&mut c.reverse, "");
            ui.end_row();
        }
    });
    let changed = (c.start, c.length, c.track, c.fade_in, c.fade_out, c.reverse) != (before.start, before.length, before.track, before.fade_in, before.fade_out, before.reverse)
        || c.gain != before.gain
        || c.source_offset != before.source_offset;
    if changed {
        s.checkpoint_gesture(*ci as u64);
        s.view.clips[*ci] = c;
        s.mark_dirty();
    }
}

fn channel(ui: &mut Ui, s: &mut Session) {
    let ci = s.channel;
    let Some(ch) = s.view.channels.get(ci).cloned() else { return };
    if ch.deleted {
        return;
    }
    section(ui, "Channel");
    ui.label(egui::RichText::new(&ch.name).strong());
    ui.label(egui::RichText::new(format!("{} · {}", ch.kind, ch.detail)).small().color(theme::TEXT_DIM));
    let controls = s.controls.clone();
    let Some(strip) = controls.channels.get(ci) else { return };
    egui::Grid::new("insp-channel").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label("Volume");
        let mut pos = 0.781_25 * strip.volume.load().max(0.0).powf(1.0 / 2.877);
        let db = super::widgets::to_db(strip.volume.load());
        if ui.add(egui::Slider::new(&mut pos, 0.0..=1.0).show_value(false)).changed() {
            strip.volume.store(engine::gain::channel_volume(pos));
            s.touch();
        }
        ui.end_row();
        ui.label("");
        ui.label(egui::RichText::new(if db < -100.0 { "-inf dB".to_string() } else { format!("{db:+.1} dB") }).small().color(theme::TEXT_DIM));
        ui.end_row();
        ui.label("Pan");
        let mut pan = strip.pan.load();
        if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).show_value(false)).changed() {
            strip.pan.store(pan);
            s.touch();
        }
        ui.end_row();
        ui.label("Mixer track");
        let mut route = strip.route();
        if ui.add(egui::DragValue::new(&mut route).range(0..=s.view.inserts.len().saturating_sub(1))).changed() {
            s.route_channel(ci, route);
        }
        ui.end_row();
        ui.label("Root note");
        let mut root = s.view.channels[ci].root_note;
        let name = |k: u8| format!("{}{}", ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][(k % 12) as usize], k as i32 / 12);
        if ui.add(egui::DragValue::new(&mut root).range(0..=127).custom_formatter(move |v, _| name(v as u8))).changed() {
            s.view.channels[ci].root_note = root;
            if let Some(spec) = s.song_channel_mut(ci) {
                spec.root_note = root;
            }
            s.touch();
        }
        ui.end_row();
        ui.label("Colour");
        ui.horizontal(|ui| {
            for c in theme::CLIP_PALETTE {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                ui.painter().rect_filled(r, 3.0, c);
                if resp.clicked() {
                    s.set_channel_color(ci, Some([c.r(), c.g(), c.b()]));
                }
            }
        });
        ui.end_row();
    });
}

fn pattern(ui: &mut Ui, s: &mut Session) {
    let pi = s.pattern;
    let Some(p) = s.view.patterns.get(pi).cloned() else { return };
    section(ui, "Pattern");
    ui.label(egui::RichText::new(&p.name).strong());
    let notes = p.notes.len();
    let per_bar = s.ticks_per_bar().max(1);
    ui.label(egui::RichText::new(format!("{notes} notes · {:.2} bars", p.length as f64 / per_bar as f64)).small().color(theme::TEXT_DIM));
}
