//! The KVNG Synth's window: oscillators, filter, envelope.

use eframe::egui::{self, Ui, vec2};
use engine::instrument::{SynthParams, Wave};

use super::session::Session;
use super::{theme, widgets};

/// Maps 0..1 to `lo..hi` logarithmically (and back), for frequencies and times.
fn log_map(t: f32, lo: f32, hi: f32) -> f32 {
    lo * (hi / lo).powf(t.clamp(0.0, 1.0))
}

fn log_unmap(v: f32, lo: f32, hi: f32) -> f32 {
    ((v.max(lo) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
}

/// Draws the synth controls for `channel`. Returns false if it isn't a KVNG Synth.
pub fn show(ui: &mut Ui, s: &mut Session, channel: usize) -> bool {
    let Some(mut p) = s.synth_params(channel) else { return false };
    let before = p.clone();
    let mut automate = None;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Preset").small().color(theme::TEXT_DIM));
        let current = SynthParams::presets().into_iter().find(|(_, q)| *q == p).map_or("Custom", |(n, _)| n);
        egui::ComboBox::from_id_salt(("synth-preset", channel)).width(150.0).selected_text(current).show_ui(ui, |ui| {
            for (name, preset) in SynthParams::presets() {
                if ui.selectable_label(preset == p, name).clicked() {
                    p = preset;
                }
            }
        });
    });
    ui.add_space(4.0);

    ui.horizontal_top(|ui| {
        for (i, osc) in p.osc.iter_mut().enumerate() {
            section(ui, &format!("OSC {}", i + 1), |ui| {
                egui::ComboBox::from_id_salt(("synth-wave", channel, i)).width(84.0).selected_text(osc.wave.label()).show_ui(ui, |ui| {
                    for w in Wave::ALL {
                        ui.selectable_value(&mut osc.wave, w, w.label());
                    }
                });
                ui.horizontal(|ui| {
                    labeled_knob(ui, "Level", &mut osc.level, 0.8, |v| format!("{:.0}%", v * 100.0));
                    let mut fine = (osc.fine + 100.0) / 200.0;
                    if labeled_knob(ui, "Fine", &mut fine, 0.5, |v| format!("{:+.0} cents", v * 200.0 - 100.0)) {
                        osc.fine = fine * 200.0 - 100.0;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Coarse").small().color(theme::TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut osc.coarse).range(-24..=24).suffix(" st"));
                });
            });
        }
    });
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        section(ui, "FILTER", |ui| {
            ui.horizontal(|ui| {
                let mut cut = log_unmap(p.cutoff, 20.0, 20000.0);
                let (changed, r) = labeled_knob_resp(ui, "Cutoff", &mut cut, log_unmap(2400.0, 20.0, 20000.0), |v| format!("{:.0} Hz", log_map(v, 20.0, 20000.0)));
                if changed {
                    p.cutoff = log_map(cut, 20.0, 20000.0);
                }
                if let Some(r) = r {
                    super::automation::menu(&r, engine::view::AutomationTarget::SynthCutoff { channel }, &mut automate);
                }
                let (_, r) = labeled_knob_resp(ui, "Reso", &mut p.resonance, 0.25, |v| format!("{:.0}%", v * 100.0));
                if let Some(r) = r {
                    super::automation::menu(&r, engine::view::AutomationTarget::SynthResonance { channel }, &mut automate);
                }
                let mut env = p.filter_env / 6.0;
                if labeled_knob(ui, "Env", &mut env, 0.25, |v| format!("{:.1} oct", v * 6.0)) {
                    p.filter_env = env * 6.0;
                }
            });
        });
        section(ui, "ENVELOPE", |ui| {
            ui.horizontal(|ui| {
                for (label, value, lo, hi, default) in [
                    ("Attack", &mut p.attack, 0.001, 5.0, 0.005),
                    ("Decay", &mut p.decay, 0.01, 5.0, 0.3),
                ] {
                    let mut t = log_unmap(*value, lo, hi);
                    if labeled_knob(ui, label, &mut t, log_unmap(default, lo, hi), |v| format!("{:.0} ms", log_map(v, lo, hi) * 1000.0)) {
                        *value = log_map(t, lo, hi);
                    }
                }
                labeled_knob(ui, "Sustain", &mut p.sustain, 0.7, |v| format!("{:.0}%", v * 100.0));
                let mut t = log_unmap(p.release, 0.01, 8.0);
                if labeled_knob(ui, "Release", &mut t, log_unmap(0.25, 0.01, 8.0), |v| format!("{:.0} ms", log_map(v, 0.01, 8.0) * 1000.0)) {
                    p.release = log_map(t, 0.01, 8.0);
                }
            });
        });
        section(ui, "VOICE", |ui| {
            ui.horizontal(|ui| {
                let mut g = (p.glide / 1.0).sqrt();
                if labeled_knob(ui, "Glide", &mut g, 0.0, |v| if v <= 0.0 { "Off".into() } else { format!("{:.0} ms", v * v * 1000.0) }) {
                    p.glide = g * g;
                }
                labeled_knob(ui, "Volume", &mut p.volume, 0.7, |v| format!("{:.0}%", v * 100.0));
            });
        });
    });
    ui.add_space(4.0);
    if p != before {
        s.set_synth_params(channel, p);
    }
    if let Some(a) = automate {
        super::automation::apply(a, s);
    }
    true
}

fn section(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(theme::PANEL_2).stroke(egui::Stroke::new(1.0, theme::LINE)).corner_radius(theme::RADIUS).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.label(egui::RichText::new(title).font(theme::ui_font(9.5)).color(theme::ACCENT));
        ui.add_space(2.0);
        contents(ui);
    });
}

fn labeled_knob_resp(ui: &mut Ui, label: &str, value: &mut f32, default: f32, tip: impl FnOnce(f32) -> String) -> (bool, Option<egui::Response>) {
    let mut out = (false, None);
    ui.vertical(|ui| {
        ui.set_width(46.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let r = widgets::knob(ui, value, default, 32.0, false, tip);
            out = (r.changed(), Some(r));
        });
        ui.add_sized(vec2(46.0, 12.0), egui::Label::new(egui::RichText::new(label).font(theme::ui_font(9.0)).color(theme::TEXT_DIM)));
    });
    out
}

fn labeled_knob(ui: &mut Ui, label: &str, value: &mut f32, default: f32, tip: impl FnOnce(f32) -> String) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.set_width(46.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            changed = widgets::knob(ui, value, default, 32.0, false, tip).changed();
        });
        ui.add_sized(vec2(46.0, 12.0), egui::Label::new(egui::RichText::new(label).font(theme::ui_font(9.0)).color(theme::TEXT_DIM)));
    });
    changed
}

/// Sampler channel settings: pitch, envelope, cut itself, reverse.
pub fn sampler(ui: &mut Ui, s: &mut Session, channel: usize) -> bool {
    let Some(mut st) = s.sampler_settings(channel) else { return false };
    let before = st;
    ui.horizontal_top(|ui| {
        section(ui, "PLAYBACK", |ui| {
            ui.horizontal(|ui| {
                let mut pitch = (st.pitch + 24.0) / 48.0;
                if labeled_knob(ui, "Pitch", &mut pitch, 0.5, |v| format!("{:+.1} st", v * 48.0 - 24.0)) {
                    st.pitch = (pitch * 48.0 - 24.0).round();
                }
                ui.vertical(|ui| {
                    ui.checkbox(&mut st.cut_itself, "Cut itself").on_hover_text("A new note stops the previous one (808s, hats)");
                    ui.checkbox(&mut st.follow_note, "Follow note length").on_hover_text("Stop when the note ends instead of playing the whole sample");
                    ui.checkbox(&mut st.reverse, "Reverse");
                });
            });
        });
        section(ui, "ENVELOPE", |ui| {
            ui.checkbox(&mut st.envelope, "Use envelope");
            ui.add_enabled_ui(st.envelope, |ui| {
                ui.horizontal(|ui| {
                    for (label, value, lo, hi, default) in [("Attack", &mut st.attack, 0.0005, 5.0, 0.002), ("Decay", &mut st.decay, 0.01, 5.0, 0.3)] {
                        let mut t = log_unmap(*value, lo, hi);
                        if labeled_knob(ui, label, &mut t, log_unmap(default, lo, hi), |v| format!("{:.0} ms", log_map(v, lo, hi) * 1000.0)) {
                            *value = log_map(t, lo, hi);
                        }
                    }
                    labeled_knob(ui, "Sustain", &mut st.sustain, 1.0, |v| format!("{:.0}%", v * 100.0));
                    let mut t = log_unmap(st.release, 0.005, 8.0);
                    if labeled_knob(ui, "Release", &mut t, log_unmap(0.08, 0.005, 8.0), |v| format!("{:.0} ms", log_map(v, 0.005, 8.0) * 1000.0)) {
                        st.release = log_map(t, 0.005, 8.0);
                    }
                });
            });
        });
        section(ui, "FILTER", |ui| {
            egui::ComboBox::from_id_salt(("sampler-filter", channel)).width(96.0).selected_text(["Off", "Low-pass", "High-pass", "Band-pass"][st.filter.min(3) as usize]).show_ui(ui, |ui| {
                for (i, name) in ["Off", "Low-pass", "High-pass", "Band-pass"].iter().enumerate() {
                    ui.selectable_value(&mut st.filter, i as u8, *name);
                }
            });
            ui.add_enabled_ui(st.filter != 0, |ui| {
                ui.horizontal(|ui| {
                    let mut t = log_unmap(st.cutoff, 20.0, 20_000.0);
                    if labeled_knob(ui, "Cutoff", &mut t, log_unmap(2000.0, 20.0, 20_000.0), |v| {
                        let hz = log_map(v, 20.0, 20_000.0);
                        if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") }
                    }) {
                        st.cutoff = log_map(t, 20.0, 20_000.0);
                    }
                    labeled_knob(ui, "Res", &mut st.resonance, 0.2, |v| format!("{:.0}%", v * 100.0));
                });
            });
        });
    });
    if st != before {
        s.set_sampler_settings(channel, st);
    }
    true
}

/// Device and MIDI channel for a MIDI Out channel. Returns false if it isn't one.
pub fn midi_out(ui: &mut Ui, s: &mut Session, channel: usize, notice: &mut Option<String>) -> bool {
    let Some((port, midi_channel)) = s.midi_out(channel) else { return false };
    let (mut new_port, mut new_channel) = (port.clone(), midi_channel);
    ui.label(egui::RichText::new("Notes on this channel are sent to an external MIDI device.").small().color(theme::TEXT_DIM));
    ui.add_space(4.0);
    egui::Grid::new(("midi-out", channel)).num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
        ui.label("Device");
        let ports = engine::midi_out::ports();
        let shown = if port.is_empty() { "(none)" } else { port.as_str() };
        egui::ComboBox::from_id_salt(("midi-out-port", channel)).width(220.0).selected_text(shown).show_ui(ui, |ui| {
            if ports.is_empty() {
                ui.label("No MIDI output devices found");
            }
            for p in ports {
                let selected = p == port;
                if ui.selectable_label(selected, &p).clicked() {
                    new_port = p;
                }
            }
        });
        ui.end_row();
        ui.label("MIDI channel");
        egui::ComboBox::from_id_salt(("midi-out-channel", channel)).width(60.0).selected_text((midi_channel + 1).to_string()).show_ui(ui, |ui| {
            for c in 0..16u8 {
                ui.selectable_value(&mut new_channel, c, (c + 1).to_string());
            }
        });
        ui.end_row();
    });
    if (new_port != port || new_channel != midi_channel)
        && let Err(e) = s.set_midi_out(channel, new_port, new_channel)
    {
        *notice = Some(e);
    }
    true
}

/// Plugin channels: pitch-bend range (slide notes bend the plugin by this much per semitone).
pub fn plugin_settings(ui: &mut Ui, s: &mut Session, channel: usize) {
    let Some(mut range) = s.bend_range(channel) else { return };
    ui.horizontal(|ui| {
        ui.label("Pitch bend range");
        if ui.add(egui::DragValue::new(&mut range).range(1.0..=48.0).speed(0.1).fixed_decimals(0).suffix(" st")).on_hover_text("Set this to the plugin's own pitch-bend range so slide notes land on the right pitch").changed() {
            s.set_bend_range(channel, range);
        }
    });
}

/// The channel's arpeggiator (FL's channel ARP). Returns false for audio clip channels.
pub fn arpeggiator(ui: &mut Ui, s: &mut Session, channel: usize) -> bool {
    use engine::arp::{ArpMode, RATES};
    if s.view.channels.get(channel).is_none_or(|c| c.kind == "Audio clip") {
        return false;
    }
    let Some(mut a) = s.arp(channel) else { return false };
    let before = a;
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("ARPEGGIATOR").font(theme::ui_font(10.5)).color(theme::ACCENT));
        ui.checkbox(&mut a.on, "On").on_hover_text("Play held notes one at a time, in time with the song");
    });
    ui.add_enabled_ui(a.on, |ui| {
        egui::Grid::new(("arp", channel)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label("Direction");
            egui::ComboBox::from_id_salt(("arp-mode", channel)).width(120.0).selected_text(a.mode.label()).show_ui(ui, |ui| {
                for m in ArpMode::ALL {
                    ui.selectable_value(&mut a.mode, m, m.label());
                }
            });
            ui.end_row();
            ui.label("Rate");
            let rate_label = RATES.iter().find(|(_, b)| (b - a.rate).abs() < 1e-4).map_or("custom", |(l, _)| l);
            egui::ComboBox::from_id_salt(("arp-rate", channel)).width(120.0).selected_text(rate_label).show_ui(ui, |ui| {
                for (label, beats) in RATES {
                    ui.selectable_value(&mut a.rate, beats, label);
                }
            });
            ui.end_row();
            ui.label("Gate");
            ui.add(egui::Slider::new(&mut a.gate, 0.05..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            ui.end_row();
            ui.label("Range");
            ui.add(egui::Slider::new(&mut a.octaves, 1..=4).suffix(" oct"));
            ui.end_row();
        });
    });
    if a != before {
        s.set_arp(channel, a);
    }
    true
}
