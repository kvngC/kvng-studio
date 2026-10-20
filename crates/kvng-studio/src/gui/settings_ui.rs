//! The Settings window (⌘,): audio, MIDI, plugins, recording, files and general options.

use eframe::egui::{self, Ui, vec2};

use super::picker::Picker;
use super::prefs::Prefs;
use super::session::Session;
use super::{audio, recovery, theme};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Audio,
    Midi,
    Plugins,
    Recording,
    Files,
    General,
}

/// What the window asks the app to do.
pub enum SettingsAction {
    /// Restart the audio engine with new device settings (reloads the project in place).
    RestartAudio,
    /// Rescan plugins (true = rescan everything, not just new/changed ones).
    Rescan(bool),
    Message(String),
}

/// Live audio facts shown on the Audio tab.
pub struct AudioInfo {
    pub device: String,
    pub sample_rate: f64,
    pub buffer: Option<u32>,
    /// Live engine figures.
    pub controls: Option<std::sync::Arc<engine::control::Controls>>,
}

#[derive(Default)]
pub struct SettingsWindow {
    pub open: bool,
    tab: Tab,
    /// Audio choices being edited (applied with the Apply button).
    draft: Option<Prefs>,
    outputs: Vec<String>,
    inputs: Vec<String>,
    /// Which device the capabilities were read for, its sample rates and buffer range.
    caps: Option<DeviceCaps>,
    midi_inputs: Vec<String>,
    midi_outputs: Vec<String>,
}

type DeviceCaps = (Option<String>, Vec<u32>, Option<(u32, u32)>);

const BUFFER_SIZES: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

impl SettingsWindow {
    pub fn show(&mut self, tab: Tab) {
        self.open = true;
        self.tab = tab;
        self.refresh_devices();
    }

    fn refresh_devices(&mut self) {
        self.outputs = audio::device_names(false);
        self.inputs = audio::device_names(true);
        self.caps = None;
        self.midi_inputs = midir::MidiInput::new("KVNG Studio").map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect()).unwrap_or_default();
        self.midi_outputs = engine::midi_out::ports();
    }

    pub fn ui(&mut self, ctx: &egui::Context, prefs: &mut Prefs, picker: &mut Picker, mut session: Option<&mut Session>, info: Option<&AudioInfo>) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        if !self.open {
            return actions;
        }
        picker.poll_scan();
        let before = prefs.clone();
        let mut open = true;
        egui::Window::new("Settings").id(egui::Id::new("app-settings")).open(&mut open).default_width(620.0).default_height(460.0).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, label) in [(Tab::Audio, "Audio"), (Tab::Midi, "MIDI"), (Tab::Plugins, "Plugins"), (Tab::Recording, "Recording"), (Tab::Files, "Files & backup"), (Tab::General, "General")] {
                    if ui.selectable_label(self.tab == t, label).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.separator();
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(520.0).show(ui, |ui| match self.tab {
                Tab::Audio => self.audio_tab(ui, prefs, info, &mut actions),
                Tab::Midi => self.midi_tab(ui, prefs, session.as_deref_mut()),
                Tab::Plugins => plugins_tab(ui, prefs, picker, &mut actions),
                Tab::Recording => recording_tab(ui, prefs, &self.inputs),
                Tab::Files => files_tab(ui, prefs, session.as_deref()),
                Tab::General => general_tab(ui, prefs, ctx),
            });
        });
        self.open = open;
        if !open {
            self.draft = None;
        }
        if *prefs != before {
            if let Err(e) = prefs.save() {
                actions.push(SettingsAction::Message(format!("Couldn't save settings: {e}")));
            }
            if let Some(s) = session {
                s.apply_prefs(prefs);
            }
        }
        actions
    }

    fn audio_tab(&mut self, ui: &mut Ui, prefs: &mut Prefs, info: Option<&AudioInfo>, actions: &mut Vec<SettingsAction>) {
        let mut draft = self.draft.take().unwrap_or_else(|| prefs.clone());
        let draft = &mut draft;
        if self.caps.as_ref().is_none_or(|(name, _, _)| *name != draft.output_device) {
            let (rates, buffers) = audio::capabilities(draft.output_device.as_deref());
            self.caps = Some((draft.output_device.clone(), rates, buffers));
        }
        let (_, rates, buffers) = self.caps.clone().unwrap_or_default();

        section(ui, "Output");
        egui::Grid::new("audio-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label("Device");
            egui::ComboBox::from_id_salt("out-device").width(300.0).selected_text(draft.output_device.clone().unwrap_or_else(|| "System default".into())).show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.output_device, None, "System default");
                for d in &self.outputs {
                    ui.selectable_value(&mut draft.output_device, Some(d.clone()), d);
                }
            });
            ui.end_row();

            ui.label("Sample rate");
            let rate_label = |r: Option<u32>| r.map_or("Device default".to_string(), |r| format!("{:.1} kHz", r as f32 / 1000.0));
            egui::ComboBox::from_id_salt("sample-rate").width(160.0).selected_text(rate_label(draft.sample_rate)).show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.sample_rate, None, "Device default");
                for r in &rates {
                    ui.selectable_value(&mut draft.sample_rate, Some(*r), rate_label(Some(*r)));
                }
            });
            ui.end_row();

            ui.label("Buffer size");
            let sr = draft.sample_rate.map(f64::from).or(info.map(|i| i.sample_rate)).unwrap_or(48_000.0);
            let buf_label = |b: Option<u32>| b.map_or("Device default".to_string(), |b| format!("{b} samples ({:.1} ms)", b as f64 / sr * 1000.0));
            egui::ComboBox::from_id_salt("buffer").width(200.0).selected_text(buf_label(draft.buffer_size)).show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.buffer_size, None, "Device default");
                for b in BUFFER_SIZES.iter().filter(|b| buffers.is_none_or(|(lo, hi)| **b >= lo && **b <= hi)) {
                    ui.selectable_value(&mut draft.buffer_size, Some(*b), buf_label(Some(*b)));
                }
            });
            ui.end_row();
        });
        ui.label(egui::RichText::new("Smaller buffers lower latency (playing live, recording) but cost more CPU; raise it if you hear crackles.").small().color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        let changed = draft.audio_changed(prefs);
        let mut refresh = false;
        ui.horizontal(|ui| {
            if ui.add_enabled(changed, egui::Button::new("Apply")).on_hover_text("Restarts audio with these settings; your project stays open").clicked() {
                prefs.output_device = draft.output_device.clone();
                prefs.sample_rate = draft.sample_rate;
                prefs.buffer_size = draft.buffer_size;
                actions.push(SettingsAction::RestartAudio);
            }
            if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                *draft = prefs.clone();
            }
            if ui.button("Refresh devices").clicked() {
                refresh = true;
            }
        });
        if refresh {
            self.refresh_devices();
        }
        self.draft = Some(draft.clone());

        if let Some(i) = info {
            ui.add_space(10.0);
            section(ui, "Now running");
            egui::Grid::new("audio-now").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                ui.label("Device");
                ui.label(&i.device);
                ui.end_row();
                ui.label("Sample rate");
                ui.label(format!("{:.0} Hz", i.sample_rate));
                ui.end_row();
                ui.label("Buffer");
                ui.label(i.buffer.map_or("device default".to_string(), |b| format!("{b} samples · {:.1} ms output latency", b as f64 / i.sample_rate * 1000.0)));
                ui.end_row();
                if let Some(c) = &i.controls {
                    let over = c.overloads.load(std::sync::atomic::Ordering::Relaxed);
                    ui.label("Audio load");
                    ui.label(format!("{:.0}% of the time available per buffer", c.load.load() * 100.0));
                    ui.end_row();
                    ui.label("Overloads");
                    ui.horizontal(|ui| {
                        let text = egui::RichText::new(if over == 0 { "none".to_string() } else { format!("{over} (each may be a crackle; raise the buffer size)") });
                        ui.label(if over == 0 { text.color(theme::TEXT_DIM) } else { text.color(theme::WARN) });
                        if over > 0 && ui.small_button("Reset").clicked() {
                            c.overloads.store(0, std::sync::atomic::Ordering::Relaxed);
                        }
                    });
                    ui.end_row();
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                }
            });
        }
    }

    fn midi_tab(&mut self, ui: &mut Ui, prefs: &mut Prefs, session: Option<&mut Session>) {
        section(ui, "Inputs");
        ui.label(egui::RichText::new("Notes from enabled inputs play the selected channel; controllers can be linked to knobs (right-click a knob).").small().color(theme::TEXT_FAINT));
        let mut changed = false;
        if self.midi_inputs.is_empty() {
            ui.label(egui::RichText::new("No MIDI inputs found.").color(theme::TEXT_DIM));
        }
        for port in &self.midi_inputs {
            let mut on = !prefs.disabled_midi_inputs.contains(port);
            if ui.checkbox(&mut on, port).changed() {
                if on {
                    prefs.disabled_midi_inputs.retain(|p| p != port);
                } else {
                    prefs.disabled_midi_inputs.push(port.clone());
                }
                changed = true;
            }
        }
        ui.add_space(8.0);
        section(ui, "Outputs");
        ui.label(egui::RichText::new("Add a MIDI Out channel (plugin picker → Built-in) to play these devices.").small().color(theme::TEXT_FAINT));
        if self.midi_outputs.is_empty() {
            ui.label(egui::RichText::new("No MIDI outputs found (enable the IAC Driver in Audio MIDI Setup for a virtual one).").color(theme::TEXT_DIM));
        }
        for port in &self.midi_outputs {
            ui.label(format!("• {port}"));
        }
        ui.add_space(8.0);
        let refresh = ui.button("Refresh devices").on_hover_text("Look for MIDI devices plugged in since KVNG Studio started").clicked();
        if refresh {
            self.refresh_devices();
        }
        if (changed || refresh)
            && let Some(s) = session
        {
            s.reconnect_midi(&prefs.disabled_midi_inputs);
        }
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(egui::RichText::new(title.to_uppercase()).font(theme::ui_font(10.5)).color(theme::ACCENT));
    ui.add_space(2.0);
}

fn plugins_tab(ui: &mut Ui, prefs: &mut Prefs, picker: &mut Picker, actions: &mut Vec<SettingsAction>) {
    section(ui, "Plugin folders");
    let scanner = bridge::scan::Scanner::new(crate::data_dir()).with_extra_roots(prefs.plugin_folders.clone());
    for root in scanner.vst3_roots() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(root.display().to_string()).font(theme::mono(11.0)));
            if prefs.plugin_folders.contains(&root) && ui.small_button("Remove").clicked() {
                prefs.plugin_folders.retain(|p| *p != root);
            }
        });
    }
    ui.label(egui::RichText::new("Audio Units are found through macOS automatically.").small().color(theme::TEXT_FAINT));
    if ui.button("Add VST3 folder…").clicked()
        && let Some(dir) = rfd::FileDialog::new().set_title("Choose a folder with VST3 plugins").pick_folder()
        && !prefs.plugin_folders.contains(&dir)
    {
        prefs.plugin_folders.push(dir);
    }
    ui.add_space(10.0);
    section(ui, "Scan");
    match picker.scan_progress() {
        Some((done, total, current)) => {
            ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).text(format!("{done}/{total} · {current}")).desired_width(420.0));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        None => {
            ui.label(format!("{} plugins known", picker.plugin_count()));
            ui.horizontal(|ui| {
                if ui.button("Find new plugins").on_hover_text("Scans only plugins that are new or changed since the last scan").clicked() {
                    actions.push(SettingsAction::Rescan(false));
                }
                if ui.button("Rescan all").on_hover_text("Scans every plugin again (slower)").clicked() {
                    actions.push(SettingsAction::Rescan(true));
                }
            });
        }
    }
    let failures = picker.failures();
    if !failures.is_empty() {
        ui.add_space(6.0);
        ui.collapsing(format!("{} plugins couldn't be scanned", failures.len()), |ui| {
            for f in failures {
                ui.label(egui::RichText::new(f).small().color(theme::WARN));
            }
        });
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Each plugin is scanned in its own helper process, so a crashing plugin can't take KVNG Studio down.").small().color(theme::TEXT_FAINT));
}

fn recording_tab(ui: &mut Ui, prefs: &mut Prefs, inputs: &[String]) {
    section(ui, "Audio input");
    egui::Grid::new("rec-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        ui.label("Input device");
        egui::ComboBox::from_id_salt("in-device").width(300.0).selected_text(prefs.input_device.clone().unwrap_or_else(|| "System default".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut prefs.input_device, None, "System default");
            for d in inputs {
                ui.selectable_value(&mut prefs.input_device, Some(d.clone()), d);
            }
        });
        ui.end_row();
        ui.label("Count-in");
        egui::ComboBox::from_id_salt("count-in").width(120.0).selected_text(if prefs.count_in_bars == 0 { "Off".to_string() } else { format!("{} bar{}", prefs.count_in_bars, if prefs.count_in_bars > 1 { "s" } else { "" }) }).show_ui(ui, |ui| {
            for n in 0..=4 {
                ui.selectable_value(&mut prefs.count_in_bars, n, if n == 0 { "Off".to_string() } else { format!("{n} bar{}", if n > 1 { "s" } else { "" }) });
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut prefs.metronome_when_recording, "Metronome while recording");
    ui.horizontal(|ui| {
        ui.label("Metronome volume");
        ui.add(egui::Slider::new(&mut prefs.metronome_volume, 0.0..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
    });
    ui.checkbox(&mut prefs.monitor_input, "Hear the input while recording audio (use headphones)");
    ui.horizontal(|ui| {
        let mut manual = prefs.record_offset_ms.is_some();
        if ui.checkbox(&mut manual, "Manual latency compensation").on_hover_text("Recorded audio arrives late by the interface's input + output latency; KVNG shifts takes earlier by this much. Automatic uses the buffer size.").changed() {
            prefs.record_offset_ms = manual.then_some(10.0);
        }
        if let Some(ms) = &mut prefs.record_offset_ms {
            ui.add(egui::DragValue::new(ms).range(0.0..=500.0).speed(0.1).suffix(" ms"));
        } else {
            ui.label(egui::RichText::new("automatic").small().color(theme::TEXT_DIM));
        }
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new("macOS asks for microphone access the first time you record.").small().color(theme::TEXT_FAINT));
}

fn files_tab(ui: &mut Ui, prefs: &mut Prefs, session: Option<&Session>) {
    section(ui, "Autosave");
    egui::Grid::new("files-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        ui.label("Autosave every");
        egui::ComboBox::from_id_salt("autosave").width(120.0).selected_text(if prefs.autosave_minutes == 0 { "Off".to_string() } else { format!("{} min", prefs.autosave_minutes) }).show_ui(ui, |ui| {
            for m in [0, 1, 2, 5, 10, 15, 30] {
                ui.selectable_value(&mut prefs.autosave_minutes, m, if m == 0 { "Off".to_string() } else { format!("{m} min") });
            }
        });
        ui.end_row();
        ui.label("Backups per project");
        ui.add(egui::DragValue::new(&mut prefs.backups).range(0..=100));
        ui.end_row();
        ui.label("Project folder");
        ui.horizontal(|ui| {
            ui.label(prefs.project_folder.as_ref().map_or("(last used)".to_string(), |p| p.display().to_string()));
            if ui.small_button("Choose…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                prefs.project_folder = Some(dir);
            }
            if prefs.project_folder.is_some() && ui.small_button("Clear").clicked() {
                prefs.project_folder = None;
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut prefs.reopen_last, "Reopen the last project when KVNG Studio starts");
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Unsaved work is autosaved separately and offered for recovery after a crash. Saving over a project keeps the previous version as a backup.").small().color(theme::TEXT_FAINT));
    ui.horizontal(|ui| {
        if ui.button("Show autosaves").clicked() {
            reveal(&crate::data_dir().join("Autosave"));
        }
        let backups = session.and_then(|s| s.path.as_ref()).map(|p| recovery::backups_dir(p)).unwrap_or_else(|| crate::data_dir().join("Backups"));
        if ui.button("Show backups").clicked() {
            reveal(&backups);
        }
    });
}

fn reveal(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("open").arg(dir).spawn();
}

fn general_tab(ui: &mut Ui, prefs: &mut Prefs, ctx: &egui::Context) {
    section(ui, "Interface");
    egui::Grid::new("general-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
        ui.label("Interface size");
        ui.horizontal(|ui| {
            for (label, z) in [("Compact", 0.9), ("Normal", 1.0), ("Large", 1.15), ("Larger", 1.3)] {
                if ui.selectable_label((prefs.ui_scale - z).abs() < 0.01, label).clicked() {
                    prefs.ui_scale = z;
                    ctx.set_zoom_factor(z);
                }
            }
        });
        ui.end_row();
        ui.label("Export default");
        ui.horizontal(|ui| {
            for b in [16u16, 24, 32] {
                ui.selectable_value(&mut prefs.export_bits, b, format!("{b}-bit"));
            }
        });
        ui.end_row();
    });
    ui.checkbox(&mut prefs.hints, "Show hints in tooltips");
    ui.add_space(10.0);
    section(ui, "Data");
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::data_dir().display().to_string()).font(theme::mono(11.0)));
        if ui.small_button("Show").clicked() {
            reveal(&crate::data_dir());
        }
    });
    ui.add_space(vec2(0.0, 4.0).y);
}
