//! The channel rack: one row per channel with mute, volume, pan, mixer routing,
//! name, and the step sequencer for the selected pattern.

use eframe::egui::{self, Align2, Color32, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::view::{AutomationTarget, ChannelStatus};

use super::session::{Mode, Session};
use super::{theme, transport, widgets};

const ROW_H: f32 = 32.0;
const PAD_W: f32 = 20.0;
const PAD_H: f32 = 24.0;
const PAD_GAP: f32 = 3.0;
const BEAT_GAP: f32 = 5.0;
const NAME_W: f32 = 150.0;

pub enum Action {
    OpenPiano(usize),
    OpenEditor(usize),
    AddInstrument,
    /// Open a built-in instrument's window.
    ChannelSettings(usize),
    /// Open the channel's sample in the audio editor.
    EditAudio(usize),
    Rename(usize),
    Delete(usize),
    /// A control menu action happened (automation clip or MIDI link), with a message.
    Automated(String),
    /// Choose one of the channel plugin's parameters to automate.
    AutomateParam(usize),
    /// Open the plugin picker to swap this channel's sound.
    ReplaceInstrument(usize),
    /// Give the channel its own free mixer track.
    RouteToFreeInsert(usize),
    /// Render the channel to an audio clip on the playlist (bounce in place).
    Bounce(usize),
    /// A browser sample, picker plugin or built-in was dropped on a channel.
    ReplaceSample(usize, std::path::PathBuf),
    ReplacePlugin(usize, bridge::scan::PluginInfo),
    ReplaceBuiltin(usize, engine::song::SourceSpec),
}

/// Step painting: the value being painted along a row while the mouse is held.
#[derive(Clone, Copy)]
struct Paint {
    channel: usize,
    on: bool,
}

pub fn show(ui: &mut Ui, s: &mut Session) -> Option<Action> {
    let mut action = None;
    let mut automate = None;

    let bar_rect = ui.allocate_space(vec2(ui.available_width(), 28.0)).1;
    let p = ui.painter();
    theme::draw_gunmetal_surface(&p, bar_rect, theme::RADIUS_SM, theme::PANEL_2);
    theme::draw_screw(&p, pos2(bar_rect.min.x + 8.0, bar_rect.center().y), 4.0, 1.2);
    theme::draw_screw(&p, pos2(bar_rect.max.x - 8.0, bar_rect.center().y), 4.0, 2.7);

    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar_rect.shrink2(vec2(16.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let b_ui = &mut bar_ui;

    b_ui.horizontal(|ui| {
        transport::pattern_picker(ui, s, "rack-pattern");
        ui.add_space(8.0);
        let mut steps = pattern_steps(s);
        let r = ui.add(egui::DragValue::new(&mut steps).range(1..=256).suffix(" steps")).on_hover_text("Pattern length in steps (sixteenths)");
        if r.changed() {
            s.set_pattern_steps(steps);
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Swing").small().color(theme::TEXT_DIM));
        let mut swing = s.view.swing;
        if widgets::knob(ui, &mut swing, 0.0, 22.0, false, |v| format!("Swing {:.0}%", v * 100.0)).changed() {
            s.set_swing(swing);
        }
        ui.add_space(8.0);
        // Channel filter groups.
        let groups = s.channel_groups();
        let filter_id = Id::new("rack-group-filter");
        let mut filter: Option<String> = ui.data(|d| d.get_temp::<Option<String>>(filter_id)).flatten().filter(|g| groups.contains(g));
        let shown = filter.clone().unwrap_or_else(|| "All".into());
        egui::ComboBox::from_id_salt("rack-groups").width(100.0).selected_text(shown).show_ui(ui, |ui| {
            if ui.selectable_label(filter.is_none(), "All").clicked() {
                filter = None;
            }
            for g in &groups {
                if ui.selectable_label(filter.as_ref() == Some(g), g).clicked() {
                    filter = Some(g.clone());
                }
            }
            if groups.is_empty() {
                ui.label(egui::RichText::new("Right-click a channel → Group to make groups").small().color(theme::TEXT_DIM));
            }
        })
        .response
        .on_hover_text("Show one group of channels");
        ui.data_mut(|d| d.insert_temp(filter_id, filter));
        let graph_open = ui.data(|d| d.get_temp::<GraphLane>(Id::new("rack-graph"))).is_some();
        if widgets::toggle(ui, graph_open, "Graph", vec2(54.0, 22.0)).on_hover_text("Graph editor: draw velocity, pan, pitch and mod per step for the selected channel").clicked() {
            ui.data_mut(|d| {
                if graph_open {
                    d.remove::<GraphLane>(Id::new("rack-graph"));
                } else {
                    d.insert_temp(Id::new("rack-graph"), GraphLane::Velocity);
                }
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if s.can_undo() && ui.small_button("Undo").on_hover_text("Undo (⌘Z)").clicked() {
                s.undo();
            }
        });
    });
    ui.add_space(4.0);
    let graph: Option<GraphLane> = ui.data(|d| d.get_temp(Id::new("rack-graph")));
    let group_filter: Option<String> = ui.data(|d| d.get_temp::<Option<String>>(Id::new("rack-group-filter"))).flatten();
    let mut steps_x = 0.0f32;

    let steps = pattern_steps(s);
    let playing_step = (s.is_playing() && s.mode == Mode::Pattern).then(|| {
        let len = s.view.patterns.get(s.pattern).map_or(1, |p| p.length.max(1)) as f64;
        (s.position_ticks() % len) as u64 / s.view.step_ticks()
    });
    let paint_id = Id::new("rack-paint");
    let (pointer, primary_pressed, down, secondary_down) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.secondary_down(),
        )
    });
    let mut paint: Option<Paint> = if down { ui.data(|d| d.get_temp(paint_id)) } else { None };

    egui::ScrollArea::both().auto_shrink([false, true]).show(ui, |ui| {
        for ci in 0..s.view.channels.len() {
            if s.view.channels[ci].deleted {
                continue;
            }
            if let Some(g) = &group_filter
                && s.channel_group(ci).as_ref() != Some(g)
            {
                continue;
            }
            let selected = ci == s.channel;
            ui.horizontal(|ui| {
                ui.set_height(ROW_H);
                ui.spacing_mut().item_spacing.x = 5.0;
                let controls = s.controls.clone();
                let strip = &controls.channels[ci];

                // Mute light.
                if widgets::led(ui, !strip.is_muted(), 18.0).on_hover_text("Mute / unmute").clicked() {
                    strip.mute.store(!strip.is_muted(), std::sync::atomic::Ordering::Relaxed);
                    s.touch();
                }

                // Volume (unity at FL's default 78%) and pan.
                let gain = strip.volume.load();
                let mut vol = 0.781_25 * gain.max(0.0).powf(1.0 / 2.877);
                let vol_resp = widgets::knob(ui, &mut vol, 0.781_25, 22.0, false, |v| {
                    let g = engine::gain::channel_volume(v);
                    format!("Volume {:.0}% ({:+.1} dB)\nRight-click to automate", v * 100.0, widgets::to_db(g))
                });
                if vol_resp.changed() {
                    strip.volume.store(engine::gain::channel_volume(vol));
                    s.touch();
                }
                super::automation::menu(&vol_resp, AutomationTarget::ChannelVolume { channel: ci }, &mut automate);
                let mut pan = (strip.pan.load() + 1.0) / 2.0;
                let pan_resp = widgets::knob(ui, &mut pan, 0.5, 22.0, true, |v| pan_label(v * 2.0 - 1.0));
                super::automation::menu(&pan_resp, AutomationTarget::ChannelPan { channel: ci }, &mut automate);
                if pan_resp.changed() {
                    strip.pan.store(pan * 2.0 - 1.0);
                    s.touch();
                }

                // Mixer insert.
                let mut route = strip.route();
                let max = s.view.inserts.len().saturating_sub(1);
                if widgets::number_box(ui, &mut route, max, vec2(30.0, 22.0), "Mixer insert (drag or scroll)").changed() {
                    s.route_channel(ci, route);
                    s.touch();
                }

                // Name button with color strip and type icon.
                let ch = &s.view.channels[ci];
                let (rect, resp) = ui.allocate_exact_size(vec2(NAME_W, 24.0), Sense::click());
                let painter = ui.painter();
                let hovered = resp.hovered();
                let btn_pressed = resp.is_pointer_button_down_on();

                // 1. Recessed socket shadow
                painter.rect_filled(rect.translate(vec2(0.0, 1.0)), theme::RADIUS_SM, theme::METAL_SHADOW);

                // 2. Physical button face
                let cap_rect = if selected || btn_pressed { rect.translate(vec2(0.0, 1.0)) } else { rect };
                let bg_color = if selected {
                    theme::ACCENT_DIM
                } else if hovered {
                    theme::METAL_FACE_HI
                } else {
                    theme::METAL_FACE
                };
                painter.rect(
                    cap_rect,
                    theme::RADIUS_SM,
                    bg_color,
                    Stroke::new(1.0, if selected { theme::ACCENT } else { theme::LINE }),
                    StrokeKind::Inside,
                );

                // 3. Metallic specular sheen
                if !selected && !btn_pressed {
                    let top_highlight = Rect::from_min_max(
                        pos2(cap_rect.min.x + 4.0, cap_rect.min.y + 0.6),
                        pos2(cap_rect.max.x - 2.0, cap_rect.min.y + 1.8),
                    );
                    painter.rect_filled(top_highlight, 0.5, theme::alpha(Color32::WHITE, if hovered { 0.18 } else { 0.10 }));
                }

                // Channel color pip on the left
                let ch_color = s.channel_color(ci);
                let color_pip = Rect::from_min_max(cap_rect.min, pos2(cap_rect.min.x + 3.0, cap_rect.max.y));
                painter.rect_filled(color_pip, theme::RADIUS_SM, ch_color);

                // Channel type badge
                let tag = match ch.kind.as_str() {
                    "Sampler" | "Audio clip" => "SMP",
                    "KVNG Synth" => "SYN",
                    "KVNG Drum" => "DRM",
                    "MIDI Out" => "MID",
                    _ if ch.editor.is_some() => "PLG",
                    _ => "GEN",
                };
                painter.text(
                    pos2(rect.min.x + 13.0, rect.center().y),
                    Align2::CENTER_CENTER,
                    tag,
                    theme::mono(7.5),
                    if selected { theme::ACCENT_HOT } else { theme::TEXT_DIM },
                );

                let status_color = match ch.status {
                    ChannelStatus::Ok => None,
                    ChannelStatus::Substituted => Some(theme::WARN),
                    ChannelStatus::Missing => Some(theme::ERROR),
                    ChannelStatus::Unused => Some(theme::TEXT_FAINT),
                };
                let name_x = rect.min.x + 26.0;
                let name = theme::elide(painter, &ch.name, theme::ui_font(11.5), rect.width() - 36.0);
                painter.text(
                    pos2(name_x, rect.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    theme::ui_font(11.5),
                    if ch.status == ChannelStatus::Unused { theme::TEXT_DIM } else { theme::TEXT },
                );
                if let Some(c) = status_color {
                    painter.circle_filled(pos2(rect.max.x - 7.0, rect.center().y), 3.0, c);
                }
                let tip = format!("{} · {}\nClick: select · Double-click: open {}", ch.kind, ch.detail, if ch.editor.is_some() { "plugin" } else { "piano roll" });
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    s.channel = ci;
                }
                if resp.double_clicked() {
                    action = Some(if s.view.channels[ci].editor.is_some() {
                        Action::OpenEditor(ci)
                    } else if s.view.channels[ci].kind == "Audio clip" {
                        Action::EditAudio(ci)
                    } else if matches!(s.view.channels[ci].kind.as_str(), "KVNG Synth" | "Sampler" | "KVNG Drum" | "Replacement" | "MIDI Out" | "Slicer") {
                        Action::ChannelSettings(ci)
                    } else {
                        Action::OpenPiano(ci)
                    });
                }
                // Drops: a sample, plugin or built-in replaces this channel's sound.
                let dropping = resp.dnd_hover_payload::<super::browser::DraggedFile>().is_some()
                    || resp.dnd_hover_payload::<super::picker::DraggedPlugin>().is_some()
                    || resp.dnd_hover_payload::<super::picker::DraggedBuiltin>().is_some();
                if dropping {
                    ui.painter().rect_stroke(rect.expand(1.0), theme::RADIUS, (2.0, theme::ACCENT_HOT), StrokeKind::Outside);
                    ui.painter().rect_filled(rect, theme::RADIUS, theme::ACCENT_DIM);
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Replace", theme::ui_font(11.0), theme::TEXT);
                }
                if let Some(f) = resp.dnd_release_payload::<super::browser::DraggedFile>() {
                    action = Some(Action::ReplaceSample(ci, f.0.clone()));
                }
                if let Some(p) = resp.dnd_release_payload::<super::picker::DraggedPlugin>() {
                    action = Some(Action::ReplacePlugin(ci, p.0.clone()));
                }
                if let Some(b) = resp.dnd_release_payload::<super::picker::DraggedBuiltin>() {
                    action = Some(Action::ReplaceBuiltin(ci, b.0.clone()));
                }
                let has_editor = s.view.channels[ci].editor.is_some();
                let has_sample = s.has_sample(ci);
                resp.context_menu(|ui| {
                    if has_editor {
                        if ui.button("Open plugin window").clicked() {
                            action = Some(Action::OpenEditor(ci));
                            ui.close();
                        }
                        if ui.button("Automate parameter…").clicked() {
                            action = Some(Action::AutomateParam(ci));
                            ui.close();
                        }
                        if let (Some(plugin), Some(handle)) = (s.channel_plugin_name(ci), s.view.channels[ci].editor.clone())
                            && let Some(m) = super::presets::menu(ui, &plugin, &handle)
                        {
                            action = Some(Action::Automated(m));
                        }
                    } else if matches!(s.view.channels[ci].kind.as_str(), "KVNG Synth" | "Sampler" | "KVNG Drum" | "Replacement" | "MIDI Out" | "Slicer")
                        && ui.button("Open instrument window").clicked() {
                            action = Some(Action::ChannelSettings(ci));
                            ui.close();
                    }
                    if ui.button("Channel settings").clicked() {
                        action = Some(Action::ChannelSettings(ci));
                        ui.close();
                    }
                    if has_sample && ui.button("Edit audio…").clicked() {
                        action = Some(Action::EditAudio(ci));
                        ui.close();
                    }
                    if ui.button("Piano roll").clicked() {
                        action = Some(Action::OpenPiano(ci));
                        ui.close();
                    }
                    if ui.button("Rename…").clicked() {
                        action = Some(Action::Rename(ci));
                        ui.close();
                    }
                    ui.label(egui::RichText::new("Colour").small().color(theme::TEXT_DIM));
                    ui.horizontal(|ui| {
                        for c in theme::CLIP_PALETTE {
                            let (sw, swr) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                            ui.painter().rect_filled(sw, 3.0, c);
                            if swr.clicked() {
                                s.set_channel_color(ci, Some([c.r(), c.g(), c.b()]));
                                ui.close();
                            }
                        }
                    });
                    if ui.button("Replace instrument…").on_hover_text("Choose a new sound for this channel (or drag a plugin or sample onto it)").clicked() {
                        action = Some(Action::ReplaceInstrument(ci));
                        ui.close();
                    }
                    if ui.button("Render to audio clip").on_hover_text("Bounce this channel (through its mixer track) to audio on the playlist, then mute the channel").clicked() {
                        action = Some(Action::Bounce(ci));
                        ui.close();
                    }
                    ui.menu_button("Group", |ui| {
                        let current = s.channel_group(ci);
                        for g in s.channel_groups() {
                            if ui.selectable_label(current.as_ref() == Some(&g), &g).clicked() {
                                s.set_channel_group(ci, Some(g));
                                ui.close();
                            }
                        }
                        if current.is_some() && ui.button("Unsorted (no group)").clicked() {
                            s.set_channel_group(ci, None);
                            ui.close();
                        }
                        ui.separator();
                        let id = Id::new(("new-group", ci));
                        let mut name: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut name).hint_text("New group").desired_width(110.0));
                            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Add")).clicked() {
                                s.set_channel_group(ci, Some(name.clone()));
                                name.clear();
                                ui.close();
                            }
                        });
                        ui.data_mut(|d| d.insert_temp(id, name));
                    });
                    if ui.button("Route to free mixer track").clicked() {
                        action = Some(Action::RouteToFreeInsert(ci));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(egui::RichText::new("Delete channel").color(theme::ERROR)).clicked() {
                        action = Some(Action::Delete(ci));
                        ui.close();
                    }
                });

                // Activity / selection meter pip.
                let (bar, _) = ui.allocate_exact_size(vec2(8.0, 24.0), Sense::hover());
                let active = s.channel_meters[ci].active();
                let bp = ui.painter();
                bp.rect_filled(bar, theme::RADIUS_SM, theme::BG);
                bp.rect_stroke(bar, theme::RADIUS_SM, Stroke::new(1.0, theme::LINE_SOFT), StrokeKind::Inside);
                if selected {
                    bp.rect_filled(bar.shrink(1.0), theme::RADIUS_SM, theme::ACCENT);
                } else if active {
                    let fill_h = bar.height() * 0.75;
                    let active_r = Rect::from_min_max(pos2(bar.min.x + 1.0, bar.max.y - fill_h), pos2(bar.max.x - 1.0, bar.max.y - 1.0));
                    bp.rect_filled(active_r, 1.0, theme::STATUS_PLAY);
                }
                ui.add_space(4.0);

                // Steps.
                let (row, _) = ui.allocate_exact_size(vec2(steps_width(steps), PAD_H), Sense::hover());
                steps_x = row.min.x;
                for step in 0..steps {
                    let beat = step / 4;
                    let x = row.min.x + step as f32 * (PAD_W + PAD_GAP) + beat as f32 * (BEAT_GAP - PAD_GAP).max(0.0);
                    let pad = Rect::from_min_size(pos2(x, row.min.y), vec2(PAD_W, PAD_H));
                    let over = pointer.is_some_and(|p| pad.contains(p));
                    let on = s.view.step_on(s.pattern, ci, step);
                    if over && secondary_down && on {
                        s.checkpoint();
                        s.view.toggle_step(s.pattern, ci, step);
                        s.mark_dirty();
                    } else if over && primary_pressed {
                        s.checkpoint();
                        let now_on = s.view.toggle_step(s.pattern, ci, step);
                        paint = Some(Paint { channel: ci, on: now_on });
                        s.channel = ci;
                        s.mark_dirty();
                        if now_on && !s.is_playing() {
                            let root = s.view.channels[ci].root_note;
                            s.preview_blip(ci, root);
                        }
                    } else if over && down && let Some(p) = paint.filter(|p| p.channel == ci && p.on != on) {
                        let _ = p;
                        s.view.toggle_step(s.pattern, ci, step);
                        s.mark_dirty();
                    }
                    let on = s.view.step_on(s.pattern, ci, step);
                    widgets::step_pad(ui.painter(), pad, on, beat % 2 == 1, over, playing_step == Some(step));
                }
            });
            if let Some(lane) = graph.filter(|_| selected) {
                graph_editor(ui, s, ci, steps, steps_x, lane);
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(28.0);
            if widgets::toggle(ui, false, "+  Add instrument", vec2(150.0, 24.0)).on_hover_text("Choose a plugin from the picker (F8)").clicked() {
                action = Some(Action::AddInstrument);
            }
        });
    });

    if let Some(a) = automate {
        action = Some(Action::Automated(super::automation::apply(a, s)));
    }
    ui.data_mut(|d| match paint {
        Some(p) => {
            d.insert_temp(paint_id, p);
        }
        None => d.remove::<Paint>(paint_id),
    });
    action
}

/// Opens the graph editor (velocity lane).
pub fn open_graph(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(Id::new("rack-graph"), GraphLane::Velocity));
}

/// What the step graph editor edits (FL's graph editor).
#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphLane {
    Velocity,
    Pan,
    Pitch,
    ModX,
    ModY,
}

impl GraphLane {
    const ALL: [GraphLane; 5] = [GraphLane::Velocity, GraphLane::Pan, GraphLane::Pitch, GraphLane::ModX, GraphLane::ModY];

    fn label(self) -> &'static str {
        match self {
            GraphLane::Velocity => "Vel",
            GraphLane::Pan => "Pan",
            GraphLane::Pitch => "Pitch",
            GraphLane::ModX => "Mod X",
            GraphLane::ModY => "Mod Y",
        }
    }

    /// The note's value as 0..1, and whether the lane is centred (pan, pitch, mods).
    fn get(self, n: &engine::view::NoteView, root: u8) -> f32 {
        match self {
            GraphLane::Velocity => n.velocity as f32 / 128.0,
            GraphLane::Pan => n.pan as f32 / 128.0,
            GraphLane::Pitch => ((n.key as f32 - root as f32) / 24.0 + 0.5).clamp(0.0, 1.0),
            GraphLane::ModX => n.mod_x as f32 / 255.0,
            GraphLane::ModY => n.mod_y as f32 / 255.0,
        }
    }

    fn set(self, n: &mut engine::view::NoteView, root: u8, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match self {
            GraphLane::Velocity => n.velocity = (v * 128.0).round().max(1.0) as u8,
            GraphLane::Pan => n.pan = (v * 128.0).round() as u8,
            GraphLane::Pitch => n.key = (root as f32 + ((v - 0.5) * 24.0).round()).clamp(0.0, 127.0) as u8,
            GraphLane::ModX => n.mod_x = (v * 255.0).round() as u8,
            GraphLane::ModY => n.mod_y = (v * 255.0).round() as u8,
        }
    }

    fn centred(self) -> bool {
        !matches!(self, GraphLane::Velocity)
    }

    fn describe(self, n: &engine::view::NoteView, root: u8) -> String {
        match self {
            GraphLane::Velocity => format!("Velocity {:.0}%", n.velocity as f32 / 1.28),
            GraphLane::Pan => pan_label((n.pan as f32 - 64.0) / 64.0),
            GraphLane::Pitch => format!("Pitch {:+} st", n.key as i32 - root as i32),
            GraphLane::ModX => format!("Mod X {:+.0}%", (n.mod_x as f32 - 128.0) / 1.28),
            GraphLane::ModY => format!("Mod Y {:+.0}%", (n.mod_y as f32 - 128.0) / 1.28),
        }
    }
}

/// The graph editor under a channel's steps: one bar per step that has a note; drag to draw.
fn graph_editor(ui: &mut Ui, s: &mut Session, ci: usize, steps: u64, steps_x: f32, lane: GraphLane) {
    const H: f32 = 70.0;
    ui.horizontal(|ui| {
        // Lane buttons in the space left of the steps.
        let left = ui.cursor().min.x;
        ui.add_space(((steps_x - left) - 70.0).max(0.0));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for l in GraphLane::ALL {
                if widgets::toggle(ui, l == lane, l.label(), vec2(60.0, 13.0)).clicked() {
                    ui.data_mut(|d| d.insert_temp(Id::new("rack-graph"), l));
                }
            }
        });
        let x0 = ui.cursor().min.x.max(steps_x);
        let (area, resp) = ui.allocate_exact_size(vec2(steps_width(steps) + (steps_x - x0).max(0.0), H), Sense::click_and_drag());
        let area = Rect::from_min_max(pos2(steps_x, area.min.y), pos2(steps_x + steps_width(steps), area.max.y));
        let p = ui.painter();
        p.rect_filled(area.expand(2.0), theme::RADIUS_SM, theme::BG);
        let root = s.view.channels[ci].root_note;
        if lane.centred() {
            p.line_segment([pos2(area.min.x, area.center().y), pos2(area.max.x, area.center().y)], (1.0, theme::LINE));
        }
        let step_ticks = s.view.step_ticks();
        let color = s.channel_color(ci);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut edit: Option<(u64, f32)> = None;
        if (resp.dragged() || resp.clicked() || resp.drag_started())
            && let Some(pt) = pointer
            && area.expand(2.0).contains(pt)
        {
            let rel = pt.x - area.min.x;
            let mut step = 0;
            for k in 0..steps {
                let beat = k / 4;
                let x = k as f32 * (PAD_W + PAD_GAP) + beat as f32 * (BEAT_GAP - PAD_GAP).max(0.0);
                if rel >= x - PAD_GAP {
                    step = k;
                }
            }
            edit = Some((step, ((area.max.y - pt.y) / area.height()).clamp(0.0, 1.0)));
        }
        if resp.drag_started() || resp.clicked() {
            s.checkpoint_gesture(0xD1A6 + ci as u64);
        }
        let mut hover_tip = None;
        for step in 0..steps {
            let beat = step / 4;
            let x = area.min.x + step as f32 * (PAD_W + PAD_GAP) + beat as f32 * (BEAT_GAP - PAD_GAP).max(0.0);
            let col = Rect::from_min_size(pos2(x, area.min.y), vec2(PAD_W, H));
            let (lo, hi) = (step * step_ticks, (step + 1) * step_ticks);
            let notes: Vec<usize> = s.view.patterns.get(s.pattern).map(|p| p.notes.iter().enumerate().filter(|(_, n)| n.channel == ci && n.start >= lo && n.start < hi).map(|(i, _)| i).collect()).unwrap_or_default();
            if let Some((es, v)) = edit
                && es == step
            {
                for &i in &notes {
                    let n = &mut s.view.patterns[s.pattern].notes[i];
                    lane.set(n, root, v);
                }
                if !notes.is_empty() {
                    s.mark_dirty();
                }
            }
            let Some(&first) = notes.first() else {
                p.rect_filled(Rect::from_min_size(pos2(col.min.x, col.max.y - 2.0), vec2(PAD_W, 2.0)), 0.0, theme::LINE_SOFT);
                continue;
            };
            let n = s.view.patterns[s.pattern].notes[first];
            let v = lane.get(&n, root);
            let y = col.max.y - v * H;
            let bar = if lane.centred() {
                let mid = col.center().y;
                Rect::from_min_max(pos2(col.min.x + 2.0, y.min(mid)), pos2(col.max.x - 2.0, y.max(mid).max(y.min(mid) + 1.5)))
            } else {
                Rect::from_min_max(pos2(col.min.x + 2.0, y), pos2(col.max.x - 2.0, col.max.y))
            };
            p.rect_filled(bar, 1.0, color);
            if pointer.is_some_and(|pt| col.contains(pt)) {
                hover_tip = Some(lane.describe(&n, root));
            }
        }
        if let Some(t) = hover_tip {
            resp.on_hover_text(t);
        }
    });
}

fn pattern_steps(s: &Session) -> u64 {
    let step = s.view.step_ticks();
    s.view.patterns.get(s.pattern).map_or(16, |p| (p.length / step).max(1))
}

fn steps_width(steps: u64) -> f32 {
    let beats = steps.div_ceil(4) as f32;
    steps as f32 * (PAD_W + PAD_GAP) + beats * (BEAT_GAP - PAD_GAP)
}

pub fn pan_label(pan: f32) -> String {
    let pct = (pan * 100.0).round() as i32;
    match pct {
        0 => "Pan: centre".into(),
        p if p < 0 => format!("Pan: {}% left", -p),
        p => format!("Pan: {p}% right"),
    }
}
