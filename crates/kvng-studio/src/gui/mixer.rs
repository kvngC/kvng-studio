//! The mixer: console-style channel strips for each insert, master pinned right.

use std::sync::atomic::Ordering;

use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::session::Session;
use super::{rack, theme, widgets};

const MIN_STRIP_W: f32 = 132.0;
const COMPACT_STRIP_W: f32 = 164.0;
const WIDE_STRIP_W: f32 = 224.0;
/// Maximum effect slots per insert. Empty slots stay collapsed to make room for meters.
const SLOTS: usize = 10;
const SLOT_H: f32 = 20.0;

#[derive(Default)]
pub struct MixerState {
    pub show_all: bool,
    pub wide_strips: bool,
    pub fullscreen: bool,
    pub detached: bool,
}


pub enum Action {
    Close,
    ToggleFullscreen,
    ToggleDetached,
    /// Open the plugin picker to add an effect to this insert.
    AddEffect(usize),
    RemoveEffect(usize, usize),
    RenameInsert(usize),
    /// Open a built-in effect's settings.
    OpenEffect(usize, usize),
    /// Choose a parameter of a plugin effect to automate.
    AutomateEffect(usize, usize),
    /// Open the insert's EQ/compressor editor.
    OpenStrip(usize),
    Message(String),
    /// Show the next free mixer track.
    AddTrack,
    /// Append a saved effect chain to an insert.
    LoadChain(usize, String),
}

pub fn show(ui: &mut Ui, s: &mut Session, st: &mut MixerState) -> Option<Action> {
    let mut message = None;

    let bar_rect = ui.allocate_space(vec2(ui.available_width(), 28.0)).1;
    let p = ui.painter();
    theme::draw_gunmetal_surface(&p, bar_rect, theme::RADIUS_SM, theme::PANEL_2);
    theme::draw_screw(&p, pos2(bar_rect.min.x + 8.0, bar_rect.center().y), 4.0, 0.8);
    theme::draw_screw(&p, pos2(bar_rect.max.x - 8.0, bar_rect.center().y), 4.0, 2.3);

    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar_rect.shrink2(vec2(16.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let b_ui = &mut bar_ui;

    b_ui.horizontal(|ui| {
        ui.label(egui::RichText::new("MIXER").font(theme::ui_font(14.0)).strong().color(theme::TEXT));
        ui.add_space(8.0);
        if widgets::toggle(ui, st.show_all, "Show all inserts", vec2(118.0, 20.0)).clicked() {
            st.show_all = !st.show_all;
        }
        if widgets::toggle(ui, st.wide_strips, "Wide strips", vec2(90.0, 20.0))
            .on_hover_text("Use roomier mixer channels; compact strips are the default")
            .clicked()
        {
            st.wide_strips = !st.wide_strips;
        }
        if s.controls.any_insert_soloed() && ui.small_button("Clear solos").clicked() {
            for strip in &s.controls.inserts {
                strip.solo.store(false, Ordering::Relaxed);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("✕  Close mixer").on_hover_text("Return to the playlist (F9)").clicked() {
                message = Some(Action::Close);
            }
            if ui.button(if st.fullscreen { "Exit full screen" } else { "⛶  Full screen" }).on_hover_text("Use the whole display for mixing").clicked() {
                message = Some(Action::ToggleFullscreen);
            }
            if ui.button(if st.detached { "Attach mixer" } else { "Detach mixer" }).on_hover_text("Move the mixer to its own window, or attach it to the workspace").clicked() {
                message = Some(Action::ToggleDetached);
            }
        });
    });
    ui.add_space(4.0);

    let height = ui.available_height();
    let visible: Vec<usize> = (1..s.view.inserts.len()).filter(|&i| st.show_all || s.view.inserts[i].used).collect();
    // Give a small session's controls more room without changing the horizontal
    // scrolling behaviour of sessions with many inserts.
    let max_strip_w = if st.wide_strips { WIDE_STRIP_W } else { COMPACT_STRIP_W };
    let strip_w = ((ui.available_width() - 52.0) / (visible.len() + 1) as f32).clamp(MIN_STRIP_W, max_strip_w);

    ui.horizontal_top(|ui| {
        let master_w = strip_w + 16.0;
        let scroll_w = (ui.available_width() - master_w).max(strip_w);
        ui.allocate_ui(vec2(scroll_w, height), |ui| {
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for &i in &visible {
                        if let Some(m) = strip(ui, s, i, height, strip_w) {
                            message = Some(m);
                        }
                    }
                    if !st.show_all {
                        let (r, resp) = ui.allocate_exact_size(vec2(strip_w, height), Sense::click());
                        let p = ui.painter();
                        p.rect(r, theme::RADIUS, if resp.hovered() { theme::RAISED } else { theme::mix(theme::PANEL_2, theme::BG, 0.5) }, Stroke::new(1.0, if resp.hovered() { theme::ACCENT_BORDER } else { theme::LINE }), StrokeKind::Inside);
                        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 3.0)), theme::RADIUS_SM, if resp.hovered() { theme::ACCENT } else { theme::LINE_STRONG });
                        p.circle(r.center(), 21.0, theme::RAISED, Stroke::new(1.0, if resp.hovered() { theme::ACCENT_BORDER } else { theme::LINE_STRONG }));
                        p.text(r.center(), Align2::CENTER_CENTER, "+", theme::ui_font(27.0), if resp.hovered() { theme::ACCENT_HOT } else { theme::TEXT_DIM });
                        p.text(r.center() + vec2(0.0, 37.0), Align2::CENTER_CENTER, "ADD MIXER TRACK", theme::ui_font(10.5), theme::TEXT_DIM);
                        if resp.on_hover_text("Add a mixer track").clicked() {
                            message = Some(Action::AddTrack);
                        }
                    }
                });
            });
        });
        ui.add_space(4.0);
        let (sep, _) = ui.allocate_exact_size(vec2(2.0, height), Sense::hover());
        ui.painter().rect_filled(sep, 0.0, theme::LINE);
        ui.add_space(4.0);
        if let Some(m) = strip(ui, s, 0, height, strip_w) {
            message = Some(m);
        }
    });
    message
}

fn strip(ui: &mut Ui, s: &mut Session, index: usize, height: f32, strip_w: f32) -> Option<Action> {
    let mut message = None;
    let mut automate = None;
    let insert = s.view.inserts[index].clone();
    let is_master = index == 0;
    let color = if is_master { theme::ACCENT } else { s.insert_color(index) };
    let (outer, _) = ui.allocate_exact_size(vec2(strip_w, height), Sense::hover());
    let painter = ui.painter_at(outer);
    theme::draw_gunmetal_surface(&painter, outer, theme::RADIUS, if is_master { theme::mix(theme::PANEL_2, theme::ACCENT_DIM, 0.14) } else { theme::PANEL_2 });

    // Hardware chassis screw at the base of each channel strip
    theme::draw_screw(&painter, pos2(outer.center().x, outer.max.y - 6.0), 3.0, (index as f32) * 0.9);

    // Color accent tab along the top of each strip
    let top_tab = Rect::from_min_size(outer.min, vec2(outer.width(), 3.0));
    painter.rect_filled(top_tab, theme::RADIUS_SM, color);

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(outer.shrink2(vec2(5.0, 6.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
    let w = ui.available_width();

    // Built-in EQ and compressor (they process before the effect slots).
    match super::strip_ui::compact(ui, s, index, w) {
        Some(super::strip_ui::StripAction::Expand) => message = Some(Action::OpenStrip(index)),
        Some(super::strip_ui::StripAction::Control(a)) => automate = Some(a),
        None => {}
    }
    ui.add_space(2.0);

    // Effect slots.
    let inserts_label = ui.add(egui::Label::new(egui::RichText::new(format!("FX  {}/{}", insert.effects.len(), SLOTS)).font(theme::ui_font(11.0)).color(theme::TEXT_DIM)).sense(Sense::click()));
    inserts_label.on_hover_text("Right-click to save or load an effect chain").context_menu(|ui| {
        let id = egui::Id::new(("chain-name", index));
        let mut name: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut name).hint_text("Chain name").desired_width(140.0));
            if ui.add_enabled(!name.trim().is_empty() && !insert.effects.is_empty(), egui::Button::new("Save chain")).clicked() {
                let chain = s.effect_chain(index);
                message = Some(Action::Message(match super::presets::save_chain(&name, &chain) {
                    Ok(()) => format!("Saved effect chain “{}” ({} effects)", name.trim(), chain.len()),
                    Err(e) => format!("Couldn't save the chain: {e}"),
                }));
                name.clear();
                ui.close();
            }
        });
        ui.data_mut(|d| d.insert_temp(id, name));
        let chains = super::presets::chains();
        if !chains.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Add a saved chain").small().color(theme::TEXT_DIM));
            for c in chains {
                if ui.button(&c).clicked() {
                    message = Some(Action::LoadChain(index, c));
                    ui.close();
                }
            }
        }
    });
    egui::ScrollArea::vertical().id_salt(("mixer-fx", index)).max_height(5.0 * (SLOT_H + 3.0) - 3.0).auto_shrink([false, true]).show(ui, |ui| {
    let gap = 3.0;
    let shown = (insert.effects.len() + usize::from(insert.effects.len() < SLOTS)).min(SLOTS);
    let (grid, _) = ui.allocate_exact_size(vec2(w, shown as f32 * (SLOT_H + gap) - gap), Sense::hover());
    for slot in 0..shown {
        let fx = insert.effects.get(slot);
        let r = Rect::from_min_size(pos2(grid.min.x, grid.min.y + slot as f32 * (SLOT_H + gap)), vec2(w, SLOT_H));
        let resp = ui.interact(r, egui::Id::new(("fx-slot", index, slot)), Sense::click());
        let p = ui.painter();
        let filled = fx.is_some();
        let is_next_add = slot == insert.effects.len();
        
        let slot_fill = if resp.hovered() && filled {
            theme::RAISED_HI
        } else if filled {
            theme::RAISED
        } else if resp.hovered() && is_next_add {
            theme::PILL_BG
        } else {
            theme::alpha(theme::BG, 0.4)
        };
        let slot_stroke = if filled {
            Stroke::new(1.0, theme::LINE_SOFT)
        } else if resp.hovered() && is_next_add {
            Stroke::new(1.0, theme::ACCENT_BORDER)
        } else {
            Stroke::new(1.0, theme::alpha(theme::LINE_SOFT, 0.5))
        };
        p.rect(r, theme::RADIUS_SM, slot_fill, slot_stroke, StrokeKind::Inside);

        if let Some(fx) = fx {
            let name = theme::elide(p, &fx.name.replace("KVNG ", ""), theme::ui_font(9.5), r.width() - 12.0);
            p.text(pos2(r.min.x + 4.0, r.center().y), Align2::LEFT_CENTER, name, theme::ui_font(9.5), if fx.bypass { theme::TEXT_FAINT } else { theme::TEXT });
            // Bypass light on the right edge.
            p.circle_filled(pos2(r.max.x - 5.0, r.center().y), 2.0, if fx.bypass { theme::RAISED_HI } else { theme::ACCENT });
            let resp = resp.on_hover_text(format!("{}\n{}\nRight-click for options", fx.name, if fx.editor.is_some() { "Click to open its window" } else { &fx.detail }));
            if resp.clicked() {
                match &fx.editor {
                    Some(h) => {
                        if let Err(e) = h.open() {
                            message = Some(Action::Message(format!("{}: {e}", fx.name)));
                        }
                    }
                    None if fx.detail.starts_with("Built-in") => message = Some(Action::OpenEffect(index, slot)),
                    None => {}
                }
            }
            let (fx_bypass, fx_mix) = (fx.bypass, fx.mix);
            resp.context_menu(|ui| {
                let mut bypass = fx_bypass;
                if ui.checkbox(&mut bypass, "Bypass").changed() {
                    s.set_effect_bypass(index, slot, bypass);
                }
                let mut mix = fx_mix * 100.0;
                if ui.add(egui::Slider::new(&mut mix, 0.0..=100.0).text("Mix").suffix("%")).changed() {
                    s.set_effect_mix(index, slot, mix / 100.0);
                }
                ui.separator();
                if let (Some(plugin), Some(handle)) = (s.effect_plugin_name(index, slot), fx.editor.clone())
                    && let Some(m) = super::presets::menu(ui, &plugin, &handle)
                {
                    message = Some(Action::Message(m));
                }
                if (fx.editor.is_some() || fx.detail.starts_with("Built-in")) && ui.button("Automate parameter…").clicked() {
                    message = Some(Action::AutomateEffect(index, slot));
                    ui.close();
                }
                if let Some(h) = &fx.editor
                    && ui.button("Open window").clicked()
                {
                    if let Err(e) = h.open() {
                        message = Some(Action::Message(format!("{}: {e}", fx.name)));
                    }
                    ui.close();
                }
                if ui.button(egui::RichText::new("Remove").color(theme::ERROR)).clicked() {
                    message = Some(Action::RemoveEffect(index, slot));
                    ui.close();
                }
            });
        } else if is_next_add {
            p.text(r.center(), Align2::CENTER_CENTER, if resp.hovered() { "+ Add" } else { "+ Add FX" }, theme::ui_font(9.5), if resp.hovered() { theme::ACCENT_HOT } else { theme::TEXT_DIM });
            if resp.on_hover_text("Add an effect").clicked() {
                message = Some(Action::AddEffect(index));
            }
        }
    }
    });
    ui.add_space(2.0);

    // Output routing.
    let routes = if is_master {
        "Out: Main".to_string()
    } else {
        let names: Vec<String> = insert
            .routes_to
            .iter()
            .enumerate()
            .map(|(k, &d)| {
                let name = if d == 0 { "Master".to_string() } else { format!("{d}") };
                match insert.route_levels.get(k).copied().unwrap_or(1.0) {
                    l if (l - 1.0).abs() < 1e-3 => name,
                    l if l <= 1e-5 => format!("{name} (off)"),
                    l => format!("{name} ({:+.0})", 20.0 * l.log10()),
                }
            })
            .chain(insert.sidechain_to.iter().map(|d| format!("SC {d}")))
            .collect();
        format!("→ {}", names.join(", "))
    };
    let (r, route_resp) = ui.allocate_exact_size(vec2(w, 16.0), Sense::click());
    ui.painter().rect_filled(r, theme::RADIUS_SM, if route_resp.hovered() && !is_master { theme::RAISED } else { theme::BG });
    let routes = theme::elide(ui.painter(), &routes, theme::ui_font(9.0), r.width() - 4.0);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, routes, theme::ui_font(10.0), theme::TEXT_DIM);
    if !is_master {
        let all: Vec<(usize, String)> = s.view.inserts.iter().map(|i| (i.index, i.name.clone())).collect();
        let route_resp = route_resp.on_hover_text("Click to choose where this insert sends its output");
        egui::Popup::menu(&route_resp).show(|ui| route_menu(ui, s, index, &all));
    }

    let controls = s.controls.clone();
    let strip = &controls.inserts[index];
    let comp_on = s.strip(index).is_some_and(|st| st.comp.on);

    // Pan.
    ui.horizontal(|ui| {
        ui.add_space((w - 28.0) / 2.0);
        let mut pan = (strip.pan.load() + 1.0) / 2.0;
        let pan_resp = widgets::knob(ui, &mut pan, 0.5, 28.0, true, |v| rack::pan_label(v * 2.0 - 1.0));
        if pan_resp.changed() {
            strip.pan.store(pan * 2.0 - 1.0);
            s.touch();
        }
        super::automation::menu(&pan_resp, engine::view::AutomationTarget::InsertPan { insert: index }, &mut automate);
    });
    let (pan_label, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
    ui.painter().text(pan_label.center(), Align2::CENTER_CENTER, "PAN", theme::ui_font(10.0), theme::TEXT_DIM);

    // Solo / mute.
    ui.horizontal(|ui| {
        let bw = (w - 4.0) / 2.0;
        if !is_master {
            if widgets::toggle(ui, strip.is_soloed(), "S", vec2(bw, 20.0)).on_hover_text("Solo").clicked() {
                strip.solo.store(!strip.is_soloed(), Ordering::Relaxed);
            }
        } else {
            ui.add_space(bw + 4.0);
        }
        if widgets::toggle(ui, strip.is_muted(), "M", vec2(bw, 20.0)).on_hover_text("Mute").clicked() {
            strip.mute.store(!strip.is_muted(), Ordering::Relaxed);
            s.touch();
        }
    });

    // Fader, scale and meter.
    let name_h = 42.0;
    let fader_h = (ui.available_height() - name_h - 24.0).max(80.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let meter_w = (w * 0.16).clamp(16.0, 26.0);
        let group_w = 24.0 + 34.0 + meter_w + 16.0 + 12.0;
        ui.add_space(((w - group_w) / 2.0).max(0.0));
        let (scale_rect, _) = ui.allocate_exact_size(vec2(24.0, fader_h), Sense::hover());
        widgets::fader_scale(ui.painter(), scale_rect);
        let mut pos = engine::gain::fl_volume_position(strip.volume.load());
        let fader_resp = widgets::fader(ui, &mut pos, vec2(34.0, fader_h)).on_hover_text("Volume: drag (Shift for fine), double-click for 0 dB · right-click to automate");
        if fader_resp.changed() {
            strip.volume.store(engine::gain::fl_volume(pos));
            s.touch();
        }
        super::automation::menu(&fader_resp, engine::view::AutomationTarget::InsertVolume { insert: index }, &mut automate);
        widgets::meter(ui, &mut s.insert_meters[index], vec2(meter_w, fader_h)).on_hover_text("Click to reset the clip light");
        gr_meter(ui, index, strip.gain_reduction.load(), comp_on, vec2(16.0, fader_h));
    });

    // dB readout.
    let db = widgets::to_db(strip.volume.load());
    let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    ui.painter().rect(r, theme::RADIUS_SM, theme::BG, Stroke::new(1.0, theme::LINE_SOFT), StrokeKind::Inside);
    let db_text = if db < -100.0 { "-inf".to_string() } else { format!("{db:+.1}") };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, db_text, theme::mono(10.0), theme::TEXT);

    // Name and number plate.
    let (r, name_resp) = ui.allocate_exact_size(vec2(w, name_h - 6.0), Sense::click());
    let name_resp = name_resp.on_hover_text("Double-click to rename · right-click for colour");
    if name_resp.double_clicked() {
        message = Some(Action::RenameInsert(index));
    }
    if !is_master {
        name_resp.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                message = Some(Action::RenameInsert(index));
                ui.close();
            }
            ui.label(egui::RichText::new("Colour").small().color(theme::TEXT_DIM));
            ui.horizontal(|ui| {
                for c in theme::CLIP_PALETTE {
                    let (sw, swr) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                    ui.painter().rect_filled(sw, 3.0, c);
                    if swr.clicked() {
                        s.set_insert_color(index, Some([c.r(), c.g(), c.b()]));
                        ui.close();
                    }
                }
            });
            if ui.button("Default colour").clicked() {
                s.set_insert_color(index, None);
                ui.close();
            }
            ui.separator();
            let armed = s.record_insert == Some(index);
            if ui.button(if armed { "Disarm recording" } else { "Arm for audio recording" }).on_hover_text("Recorded audio (Mic on the transport) goes to this track, and monitoring plays through its effects").clicked() {
                s.record_insert = if armed { None } else { Some(index) };
                ui.close();
            }
        });
    }
    let p = ui.painter();
    p.rect(r, theme::RADIUS_SM, Color32::from_rgb(0x10, 0x10, 0x15), Stroke::new(1.0, theme::LINE_SOFT), StrokeKind::Inside);
    let tab = Rect::from_min_size(r.min, vec2(r.width(), 3.0));
    p.rect_filled(tab, 1.0, color);
    let label = ui.painter().with_clip_rect(r);
    let name = theme::elide(&label, &insert.name, theme::ui_font(10.5), r.width() - 4.0);
    label.text(pos2(r.center().x, r.min.y + 13.0), Align2::CENTER_CENTER, name, theme::ui_font(11.5), theme::TEXT);
    label.text(pos2(r.center().x, r.max.y - 6.0), Align2::CENTER_CENTER, if is_master { "M".to_string() } else { index.to_string() }, theme::mono(9.0), if is_master { theme::ACCENT_HOT } else { theme::TEXT_FAINT });
    if s.record_insert == Some(index) {
        // Armed for audio recording.
        label.circle_filled(pos2(r.max.x - 8.0, r.max.y - 6.0), 3.5, theme::STATUS_REC);
    }
    if let Some(a) = automate {
        message = Some(Action::Message(super::automation::apply(a, s)));
    }
    message
}

/// Vertical gain-reduction meter: fills downward from the top (0 dB) as the channel's
/// compressor works, with a slowly falling peak marker and the current amount in dB.
fn gr_meter(ui: &mut Ui, insert: usize, gr: f32, active: bool, size: egui::Vec2) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let now = ui.input(|i| i.time);
    let id = egui::Id::new(("gr-meter", insert));
    let max_id = egui::Id::new(("gr-meter-max", insert));
    // Fast attack, ~20 dB/s fall for the bar; 1.5 s hold for the rolling peak marker.
    let (shown, peak, peak_at, last) = ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, (0.0f32, 0.0f32, 0.0f64, now)));
    let dt = (now - last).max(0.0) as f32;
    let gr = if active { gr.max(0.0) } else { 0.0 };
    let shown = gr.max(shown - 20.0 * dt);
    let (peak, peak_at) = if gr >= peak || now - peak_at > 1.5 { (gr, now) } else { (peak, peak_at) };
    ui.ctx().data_mut(|d| d.insert_temp(id, (shown, peak, peak_at, now)));

    // Track highest GR on track (persistent across the session run)
    let mut max_gr: f32 = ui.ctx().data(|d| d.get_temp(max_id)).unwrap_or_else(|| {
        ui.ctx().data(|d| d.get_temp(egui::Id::new("comp_graph_highest_gr"))).unwrap_or(0.0)
    });
    if active && gr > max_gr {
        max_gr = gr;
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

    if shown > 0.01 || peak > 0.01 {
        ui.ctx().request_repaint();
    }

    let p = ui.painter();
    p.rect_filled(rect, 2.0, theme::BG);
    const RANGE: f32 = 24.0;
    let y = |db: f32| rect.min.y + rect.height() * (db / RANGE).clamp(0.0, 1.0);
    for db in [3.0, 6.0, 12.0, 18.0] {
        p.line_segment([pos2(rect.min.x, y(db)), pos2(rect.min.x + 3.0, y(db))], (1.0, theme::LINE_STRONG));
    }
    // Zoned fill: amber to 6 dB, orange to 12 dB, red beyond (over-compressed).
    for (lo, hi) in [(0.0, 6.0), (6.0, theme::GR_HEAVY_DB), (theme::GR_HEAVY_DB, RANGE)] {
        if shown > lo {
            let top = y(lo);
            let bottom = y(shown.min(hi));
            p.rect_filled(Rect::from_min_max(pos2(rect.min.x + 1.0, top), pos2(rect.max.x - 1.0, bottom)), 1.0, theme::gr_color(lo + 0.01));
        }
    }
    let over = active && (peak >= theme::GR_HEAVY_DB || max_gr >= theme::GR_HEAVY_DB);
    if over {
        p.rect_stroke(rect.expand(1.0), 2.0, (1.5, theme::ERROR), StrokeKind::Outside);
    }
    // Highest GR indicator line
    if max_gr > 0.05 {
        let y_max = y(max_gr);
        p.line_segment([pos2(rect.min.x, y_max), pos2(rect.max.x, y_max)], (1.8, theme::ACCENT_HOT));
    }
    // Rolling transient peak indicator line
    if peak > 0.05 && (peak - max_gr).abs() > 0.5 {
        p.line_segment([pos2(rect.min.x, y(peak)), pos2(rect.max.x, y(peak))], (1.0, theme::WARN));
    }
    if active && max_gr > 0.5 {
        // Show highest GR reached on track at the bottom
        p.text(pos2(rect.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, format!("{:.0}", max_gr.round()), theme::mono(8.5), if max_gr >= RANGE - 2.0 { theme::ERROR } else { theme::ACCENT_HOT });
    } else if active && shown > 0.5 {
        p.text(pos2(rect.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, format!("{:.0}", shown.round()), theme::mono(8.5), theme::gr_color(shown));
    } else {
        p.text(pos2(rect.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, "GR", theme::ui_font(7.5), if active { theme::TEXT_DIM } else { theme::TEXT_FAINT });
    }
    p.rect_stroke(rect, 2.0, (1.0, theme::LINE_SOFT), StrokeKind::Inside);
    let tip = if over {
        format!("Over-compressed: {gr:.1} dB GR\nHighest GR on track: {max_gr:.1} dB\n(Click to reset peak)")
    } else if active {
        format!("Live GR: {gr:.1} dB\nHighest GR on track: {max_gr:.1} dB\n(Click to reset peak)")
    } else {
        "Compression meter — turn on COMP to compress this channel".to_string()
    };
    resp.on_hover_text(tip);
}

/// Checkboxes for each destination of insert `index` (FL's routing buttons).
fn route_menu(ui: &mut Ui, s: &mut Session, index: usize, all: &[(usize, String)]) {
    // Only the master and tracks in use (plus anything already connected) are listed.
    let listed: Vec<(usize, String)> = all
        .iter()
        .filter(|(i, _)| *i != index && (*i == 0 || s.view.inserts[*i].used || s.view.inserts[index].routes_to.contains(i) || s.view.inserts[index].sidechain_to.contains(i)))
        .cloned()
        .collect();
    let label = |i: usize, name: &str| if i == 0 { "Master".to_string() } else { format!("{i}  {name}") };

    ui.label(egui::RichText::new("SEND OUTPUT TO").font(theme::ui_font(9.5)).color(theme::TEXT_DIM));
    egui::ScrollArea::vertical().id_salt("route-sends").max_height(260.0).show(ui, |ui| {
        egui::Grid::new(("route-grid", index)).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
            for (i, name) in &listed {
                let ins = &s.view.inserts[index];
                let k = ins.routes_to.iter().position(|d| d == i);
                let mut on = k.is_some();
                let loops = !on && s.reaches(*i, index);
                let resp = ui.add_enabled(!loops, egui::Checkbox::new(&mut on, label(*i, name)));
                let resp = if loops { resp.on_disabled_hover_text("That track already feeds this one — routing here would make a loop") } else { resp };
                if resp.changed() {
                    let mut routes = s.view.inserts[index].routes_to.clone();
                    if on {
                        routes.push(*i);
                    } else {
                        routes.retain(|r| r != i);
                    }
                    routes.sort_unstable();
                    s.set_routes(index, routes);
                }
                match k {
                    Some(k) => {
                        let lin = s.view.inserts[index].route_levels.get(k).copied().unwrap_or(1.0);
                        let mut db = if lin <= 1e-5 { -100.0 } else { 20.0 * lin.log10() };
                        let r = ui.add(egui::DragValue::new(&mut db).range(-100.0..=6.0).speed(0.2).fixed_decimals(1).suffix(" dB").custom_formatter(|v, _| if v <= -99.9 { "-inf".into() } else { format!("{v:+.1} dB") }));
                        if r.changed() {
                            s.set_send_level(index, *i, if db <= -99.9 { 0.0 } else { 10f32.powf(db / 20.0) });
                        }
                        r.on_hover_text("Send level (drag; double-click to type)");
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
    });
    ui.separator();
    ui.label(egui::RichText::new("SIDECHAIN TO").font(theme::ui_font(9.5)).color(theme::TEXT_DIM));
    ui.label(egui::RichText::new("Sends this track to another track's compressor input (turn on Sidechain in its COMP).").small().color(theme::TEXT_FAINT));
    egui::ScrollArea::vertical().id_salt("route-sc").max_height(160.0).show(ui, |ui| {
        for (i, name) in listed.iter().filter(|(i, _)| *i != 0) {
            let mut on = s.view.inserts[index].sidechain_to.contains(i);
            let loops = !on && s.reaches(*i, index);
            if ui.add_enabled(!loops, egui::Checkbox::new(&mut on, label(*i, name))).changed() {
                let mut targets = s.view.inserts[index].sidechain_to.clone();
                if on {
                    targets.push(*i);
                    // The target compressor listens to its sidechain from now on.
                    if let Some(mut st) = s.strip(*i) {
                        st.comp.sidechain = true;
                        st.comp.on = true;
                        s.set_strip(*i, st);
                    }
                } else {
                    targets.retain(|t| t != i);
                }
                s.set_sidechains(index, targets);
            }
        }
    });
}
