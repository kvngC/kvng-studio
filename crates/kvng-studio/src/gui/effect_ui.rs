//! Settings windows for KVNG Studio's built-in effects.
//!
//! Knobs come from each effect's [`ParamSpec`] table; right-click a knob to automate it or
//! link it to a MIDI controller.

use eframe::egui::{self, Ui, vec2};
use engine::effects::{BuiltinEffect, DistortionMode, FilterMode, ParamSpec};
use engine::view::AutomationTarget;

use super::automation::{self, ControlAction};
use super::session::Session;
use super::{theme, widgets};

/// A knob for `spec`, labelled underneath.
fn knob(ui: &mut Ui, spec: &ParamSpec, value: &mut f32, target: AutomationTarget, action: &mut Option<ControlAction>) {
    let mut t = spec.to_normalized(*value);
    ui.vertical(|ui| {
        ui.set_width(58.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let r = widgets::knob(ui, &mut t, spec.to_normalized(spec.default), 32.0, false, |t| spec.format(spec.from_normalized(t)));
            if r.changed() {
                *value = spec.from_normalized(t);
            }
            automation::menu(&r, target, action);
        });
        ui.add_sized(vec2(58.0, 12.0), egui::Label::new(egui::RichText::new(spec.name).font(theme::ui_font(9.0)).color(theme::TEXT_DIM)));
    });
}

/// Draws the settings for the built-in effect in `insert`'s slot `index`.
pub fn show(ui: &mut Ui, s: &mut Session, insert: usize, index: usize) -> bool {
    let Some(mut fx) = s.builtin_effect(insert, index) else { return false };
    let before = fx.clone();
    let mut action = None;
    ui.label(egui::RichText::new(fx.description()).small().color(theme::TEXT_DIM));
    ui.add_space(6.0);

    // Switches and choices that aren't knobs.
    match &mut fx {
        BuiltinEffect::Delay(p) => {
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.sync, "Sync to tempo");
                ui.checkbox(&mut p.ping_pong, "Ping-pong");
                if p.sync {
                    egui::ComboBox::from_id_salt(("delay-beats", insert, index)).width(90.0).selected_text(beats_label(p.beats)).show_ui(ui, |ui| {
                        for b in [0.25, 0.5, 0.75, 1.0, 1.5, 2.0] {
                            ui.selectable_value(&mut p.beats, b, beats_label(b));
                        }
                    });
                }
            });
        }
        BuiltinEffect::Distortion(p) => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Mode").small().color(theme::TEXT_DIM));
                egui::ComboBox::from_id_salt(("dist-mode", insert, index)).width(110.0).selected_text(p.mode.label()).show_ui(ui, |ui| {
                    for m in DistortionMode::ALL {
                        ui.selectable_value(&mut p.mode, m, m.label());
                    }
                });
            });
        }
        BuiltinEffect::Filter(p) => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Type").small().color(theme::TEXT_DIM));
                egui::ComboBox::from_id_salt(("filter-mode", insert, index)).width(110.0).selected_text(p.mode.label()).show_ui(ui, |ui| {
                    for m in FilterMode::ALL {
                        ui.selectable_value(&mut p.mode, m, m.label());
                    }
                });
            });
        }
        _ => {}
    }
    ui.add_space(4.0);

    let synced_delay = matches!(&fx, BuiltinEffect::Delay(p) if p.sync);
    let specs = fx.specs();
    ui.horizontal_wrapped(|ui| {
        for (i, (spec, value)) in specs.iter().zip(fx.values_mut()).enumerate() {
            if synced_delay && i == 0 {
                continue; // "Time" in ms is replaced by the beat menu
            }
            knob(ui, spec, value, AutomationTarget::EffectParam { insert, slot: index, param: i as u32 }, &mut action);
        }
    });
    if fx != before {
        s.set_builtin_effect(insert, index, fx);
    }
    if let Some(a) = action {
        match a {
            ControlAction::Automate(target @ AutomationTarget::EffectParam { param, .. }) => {
                let name = s.view.inserts.get(insert).and_then(|ins| ins.effects.get(index)).map(|e| e.name.clone()).unwrap_or_default();
                let spec = specs.get(param as usize).map_or("Parameter", |p| p.name);
                let value = s.control_value(target);
                s.create_automation_named(target, format!("{name}: {spec}"), value);
            }
            other => {
                automation::apply(other, s);
            }
        }
    }
    true
}

fn beats_label(beats: f32) -> &'static str {
    match beats {
        b if (b - 0.25).abs() < 1e-3 => "1/16",
        b if (b - 0.5).abs() < 1e-3 => "1/8",
        b if (b - 0.75).abs() < 1e-3 => "Dotted 1/8",
        b if (b - 1.0).abs() < 1e-3 => "1/4",
        b if (b - 1.5).abs() < 1e-3 => "Dotted 1/4",
        _ => "1/2",
    }
}
