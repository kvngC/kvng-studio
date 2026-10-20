//! The top bar: song mode, transport, tempo, position, pattern and snap.

use eframe::egui::{self, Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::session::{Mode, Session, Snap};
use super::widgets::{self, Icon};
use super::{Windows, theme};

fn pill_box<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = egui::Frame::new()
        .fill(theme::PILL_BG)
        .stroke(Stroke::new(1.0, theme::PILL_LINE))
        .corner_radius(theme::RADIUS_MD)
        .inner_margin(egui::Margin::symmetric(6, 4));

    let res = frame.show(ui, add_contents);

    // Milled top specular bevel and bottom shadow line on the metallic container
    let rect = res.response.rect;
    let p = ui.painter();
    p.line_segment(
        [pos2(rect.min.x + 4.0, rect.min.y + 0.5), pos2(rect.max.x - 4.0, rect.min.y + 0.5)],
        Stroke::new(0.8, theme::alpha(Color32::WHITE, 0.08)),
    );
    p.line_segment(
        [pos2(rect.min.x + 4.0, rect.max.y - 0.5), pos2(rect.max.x - 4.0, rect.max.y - 0.5)],
        Stroke::new(0.8, theme::METAL_SHADOW),
    );

    res.inner
}

pub fn bar(ui: &mut Ui, s: &mut Session, windows: &mut Windows, typing_keyboard: &mut bool) {
    let bar_rect = ui.max_rect();
    let p = ui.painter();
    theme::draw_gunmetal_surface(p, bar_rect, theme::RADIUS_SM, theme::PANEL_2);
    theme::draw_screw(p, pos2(bar_rect.min.x + 6.0, bar_rect.center().y), 4.0, 0.7);
    theme::draw_screw(p, pos2(bar_rect.max.x - 6.0, bar_rect.center().y), 4.0, 1.8);

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        // Wordmark with 3D logo emblem.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let logo = super::splash::logo_texture(ui.ctx());
            ui.image((logo.id(), vec2(22.0, 22.0)));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(egui::RichText::new("KVNG").font(theme::ui_font(15.0)).strong().color(theme::ACCENT));
                ui.label(egui::RichText::new("STUDIO").font(theme::ui_font(10.0)).color(theme::TEXT_DIM));
            });
        });

        // 1. Playback / Transport Module
        pill_box(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            mode_switch(ui, s);
            if widgets::icon_button(ui, Icon::Play, s.is_playing(), 28.0, "Play / pause (Space)").clicked() {
                s.toggle_play();
            }
            if widgets::icon_button(ui, Icon::Stop, false, 28.0, "Stop and return to start").clicked() {
                s.stop();
            }
            let looping = s.mode == Mode::Song && s.song_loop.is_some();
            if widgets::icon_button(ui, Icon::Loop, looping, 28.0, "Clear song loop").clicked() {
                s.set_song_loop(None);
            }
            ui.label(egui::RichText::new("REC").font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
            if widgets::icon_button(ui, Icon::Record, s.recording, 24.0, "Record played notes into the selected pattern (R)").clicked() {
                s.recording = !s.recording;
            }
            ui.label(egui::RichText::new("Notes").font(theme::ui_font(11.0)).color(if s.recording { theme::STATUS_REC } else { theme::TEXT_DIM }));
            if widgets::toggle(ui, s.audio_armed, "Audio", vec2(52.0, 24.0)).on_hover_text("Arm audio recording; press Play to start capturing the input").clicked() {
                s.audio_armed = !s.audio_armed;
                if !s.audio_armed { s.record_track = None; }
            }
        });

        if s.counting_in() {
            let left = s.controls.count_in_left() as f32;
            let spb = s.controls.samples_per_beat.load().max(1.0);
            ui.label(egui::RichText::new(format!("COUNT-IN {}", (left / spb).ceil() as u32)).font(super::theme::mono(11.0)).color(super::theme::WARN));
        }

        // Calculate available space to ensure no overlap occurs between center modules and right pods
        const RIGHT_CLUSTER_WIDTH: f32 = 295.0; // Window toggles (~165px) + Perf meter (~116px) + Spacing (~14px)
        let center_avail_width = (ui.available_width() - RIGHT_CLUSTER_WIDTH).max(120.0);

        ui.allocate_ui_with_layout(
            vec2(center_avail_width, ui.available_height()),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;

                // 2. Tempo & Tools Module
                pill_box(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    tempo(ui, s);
                    if widgets::toggle(ui, s.metronome_on(), "Click", vec2(42.0, 24.0)).on_hover_text("Metronome").clicked() {
                        s.toggle_metronome();
                    }
                    if widgets::toggle(ui, *typing_keyboard, "Keys", vec2(42.0, 24.0))
                        .on_hover_text("Typing keyboard to piano")
                        .clicked()
                    {
                        *typing_keyboard = !*typing_keyboard;
                    }
                });

                // 3. LCD Time Display
                position(ui, s);

                // 4. Pattern & Snap Module
                pill_box(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    pattern_picker(ui, s, "transport");
                    snap_picker(ui, s, "transport-snap");
                });
            },
        );

        // 5. Workspace Window Toggles & CPU Badge (Right-aligned, strictly isolated)
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            pill_box(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (open, icon, label, key) in [
                    (&mut windows.mixer, widgets::WindowIcon::Mixer, "Mixer", "F9"),
                    (&mut windows.piano, widgets::WindowIcon::PianoRoll, "Piano roll", "F7"),
                    (&mut windows.rack, widgets::WindowIcon::ChannelRack, "Channel rack", "F6"),
                    (&mut windows.playlist, widgets::WindowIcon::Playlist, "Playlist", "F5"),
                    (&mut windows.browser, widgets::WindowIcon::Browser, "Browser", "⌥F8"),
                ] {
                    if widgets::window_toggle(ui, *open, icon, label, key, vec2(28.0, 26.0)).clicked() {
                        *open = !*open;
                    }
                }
            });
            ui.add_space(6.0);
            performance_meter(ui, s.controls.load.load());
        });
    });
}

fn mode_switch(ui: &mut Ui, s: &mut Session) {
    let (rect, response) = ui.allocate_exact_size(vec2(48.0, 28.0), Sense::click());
    let painter = ui.painter();
    let hovered = response.hovered();

    // 1. Recessed metallic socket
    painter.rect(
        rect,
        theme::RADIUS_SM,
        theme::METAL_SHADOW,
        Stroke::new(1.0, theme::LINE_SOFT),
        StrokeKind::Inside,
    );

    let half = (rect.height() - 2.0) / 2.0;
    for (i, (label, mode)) in [("PAT", Mode::Pattern), ("SONG", Mode::Song)].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x + 1.0, rect.min.y + 1.0 + i as f32 * half), vec2(rect.width() - 2.0, half)).shrink(1.0);
        let on = s.mode == mode;
        if on {
            // Tactile pressed / illuminated state
            painter.rect_filled(r, theme::RADIUS_SM, theme::ACCENT_DIM);
            painter.rect_stroke(r, theme::RADIUS_SM, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
            // Top highlight on the button
            painter.line_segment(
                [pos2(r.min.x + 2.0, r.min.y + 0.8), pos2(r.max.x - 2.0, r.min.y + 0.8)],
                Stroke::new(0.8, theme::alpha(Color32::WHITE, 0.25)),
            );
            // LED status pip
            painter.circle_filled(pos2(r.min.x + 6.0, r.center().y), 1.8, theme::ACCENT_HOT);
            painter.text(pos2(r.center().x + 2.0, r.center().y), Align2::CENTER_CENTER, label, theme::ui_font(9.0), Color32::WHITE);
        } else {
            // Unpressed brushed metal rocker half
            let bg = if hovered { theme::METAL_FACE_HI } else { theme::METAL_FACE };
            painter.rect_filled(r, theme::RADIUS_SM, bg);
            painter.rect_stroke(r, theme::RADIUS_SM, Stroke::new(0.8, theme::LINE), StrokeKind::Inside);
            // Metallic top bevel
            painter.line_segment(
                [pos2(r.min.x + 1.5, r.min.y + 0.6), pos2(r.max.x - 1.5, r.min.y + 0.6)],
                Stroke::new(0.6, theme::alpha(Color32::WHITE, 0.12)),
            );
            painter.text(r.center(), Align2::CENTER_CENTER, label, theme::ui_font(9.0), theme::TEXT_DIM);
        }
    }
    if response.clicked() {
        s.set_mode(if s.mode == Mode::Pattern { Mode::Song } else { Mode::Pattern });
    }
    response.on_hover_text("Pattern / song mode (L)");
}

fn tempo(ui: &mut Ui, s: &mut Session) {
    let mut bpm = s.view.tempo;
    let r = ui.add(
        egui::DragValue::new(&mut bpm)
            .range(10.0..=522.0)
            .speed(0.1)
            .fixed_decimals(3)
            .custom_formatter(|v, _| format!("{v:.3}")),
    );
    if r.changed() {
        s.set_tempo(bpm);
    }
    r.on_hover_text("Tempo (BPM): drag, or double-click to type");
    // Tap tempo: the average of the last few taps (a pause of 2 s starts over).
    let tap = ui.small_button("Tap").on_hover_text("Tap in time to set the tempo");
    if tap.clicked() {
        let now = ui.input(|i| i.time);
        let id = egui::Id::new("tap-tempo");
        let mut taps: Vec<f64> = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
        if taps.last().is_some_and(|t| now - t > 2.0) {
            taps.clear();
        }
        taps.push(now);
        if taps.len() > 6 {
            taps.remove(0);
        }
        if taps.len() >= 2 {
            let avg = (taps[taps.len() - 1] - taps[0]) / (taps.len() - 1) as f64;
            if avg > 0.0 {
                s.set_tempo((60.0 / avg).clamp(10.0, 522.0).round());
            }
        }
        ui.data_mut(|d| d.insert_temp(id, taps));
    }
}

fn position(ui: &mut Ui, s: &Session) {
    let ticks = s.position_ticks();
    let (rect, _) = ui.allocate_exact_size(vec2(158.0, 32.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, theme::RADIUS_SM, Color32::from_rgb(0x06, 0x06, 0x0A), Stroke::new(1.0, theme::PILL_LINE), StrokeKind::Inside);
    let inner = rect.shrink(1.0);
    painter.rect_filled(inner, theme::RADIUS_SM, Color32::from_rgb(0x0B, 0x0D, 0x12));
    painter.text(pos2(rect.min.x + 8.0, rect.center().y), Align2::LEFT_CENTER, s.format_time(ticks), theme::mono(18.0), Color32::from_rgb(0xE8, 0xEE, 0xF5));
    painter.text(pos2(rect.max.x - 8.0, rect.min.y + 8.0), Align2::RIGHT_CENTER, "M:S:CS", theme::ui_font(8.0), theme::TEXT_FAINT);
    painter.text(pos2(rect.max.x - 8.0, rect.max.y - 9.0), Align2::RIGHT_CENTER, s.format_bst(ticks), theme::mono(10.0), theme::ACCENT_HOT);
}

/// Pattern selector with previous/next arrows (shared by the top bar and the rack).
pub fn pattern_picker(ui: &mut Ui, s: &mut Session, id: &str) {
    if ui.small_button("◀").on_hover_text("Previous pattern").clicked() && s.pattern > 0 {
        s.select_pattern(s.pattern - 1);
    }
    let current = s.view.patterns.get(s.pattern).map(|p| p.name.clone()).unwrap_or_default();
    let mut chosen = None;
    egui::ComboBox::from_id_salt(id).width(112.0).selected_text(current).show_ui(ui, |ui| {
        for (i, p) in s.view.patterns.iter().enumerate() {
            let label = if p.notes.is_empty() { format!("{} (empty)", p.name) } else { p.name.clone() };
            if ui.selectable_label(i == s.pattern, label).clicked() {
                chosen = Some(i);
            }
        }
    });
    if let Some(i) = chosen {
        s.select_pattern(i);
    }
    if ui.small_button("▶").on_hover_text("Next pattern").clicked() {
        s.select_pattern(s.pattern + 1);
    }
    let rename_id = egui::Id::new((id, "rename"));
    ui.menu_button("…", |ui| {
        if ui.button("New pattern").clicked() {
            s.new_pattern();
            ui.close();
        }
        if ui.button("Clone pattern").clicked() {
            s.clone_pattern();
            ui.close();
        }
        ui.separator();
        let mut name: String = ui.data(|d| d.get_temp(rename_id)).unwrap_or_else(|| s.view.patterns.get(s.pattern).map(|p| p.name.clone()).unwrap_or_default());
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
            if (ui.button("Rename").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && !name.trim().is_empty() {
                s.rename_pattern(&name);
                ui.data_mut(|d| d.remove::<String>(rename_id));
                ui.close();
            } else {
                ui.data_mut(|d| d.insert_temp(rename_id, name.clone()));
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("Colour").small().color(theme::TEXT_DIM));
        ui.horizontal(|ui| {
            for c in theme::CLIP_PALETTE {
                let (sw, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                ui.painter().rect_filled(sw, 3.0, c);
                if r.clicked() {
                    s.set_pattern_color(Some([c.r(), c.g(), c.b()]));
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button(egui::RichText::new("Delete pattern").color(theme::ERROR)).on_hover_text("Also removes its clips from the playlist").clicked() {
            s.delete_pattern();
            ui.close();
        }
    })
    .response
    .on_hover_text("Pattern options");
}

pub fn snap_picker(ui: &mut Ui, s: &mut Session, id: &str) {
    egui::ComboBox::from_id_salt(id).width(72.0).selected_text(s.snap.label()).show_ui(ui, |ui| {
        for snap in Snap::ALL {
            ui.selectable_value(&mut s.snap, snap, snap.label());
        }
    });
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn memory_usage() -> (u64, u64) {
    use std::mem::MaybeUninit;
    unsafe {
        let mut info = MaybeUninit::<libc::mach_task_basic_info>::uninit();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>() / std::mem::size_of::<libc::natural_t>()) as libc::mach_msg_type_number_t;
        let kerr = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr() as *mut libc::integer_t,
            &mut count,
        );
        if kerr == libc::KERN_SUCCESS {
            let info = info.assume_init();
            (info.resident_size, info.virtual_size)
        } else {
            (0, 0)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn memory_usage() -> (u64, u64) {
    (0, 0)
}

fn format_ram(bytes: u64) -> String {
    if bytes == 0 {
        return "0M".into();
    }
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb < 1000.0 {
        format!("{:.0}M", mb)
    } else {
        format!("{:.1}G", mb / 1024.0)
    }
}

fn performance_meter(ui: &mut Ui, load: f32) {
    let (ram_resident, _) = memory_usage();
    let (rect, resp) = ui.allocate_exact_size(vec2(128.0, 26.0), Sense::hover());
    let painter = ui.painter();
    let hovered = resp.hovered();

    // Container background & subtle border
    painter.rect(
        rect,
        theme::RADIUS_MD,
        if hovered { theme::RAISED_HI } else { theme::PILL_BG },
        Stroke::new(1.0, if hovered { theme::ACCENT_SUBTLE } else { theme::PILL_LINE }),
        StrokeKind::Inside,
    );

    // Left half: CPU (width: 63.0)
    let cpu_rect = Rect::from_min_max(rect.min, pos2(rect.min.x + 63.0, rect.max.y));
    let cpu_pct = (load * 100.0).clamp(0.0, 100.0);
    let cpu_color = if load > 0.85 {
        theme::ERROR
    } else if load > 0.60 {
        theme::WARN
    } else {
        theme::STATUS_PLAY
    };

    // "CPU" label + value
    painter.text(pos2(cpu_rect.min.x + 6.0, cpu_rect.min.y + 4.0), Align2::LEFT_TOP, "CPU", theme::mono(8.0), theme::TEXT_DIM);
    painter.text(pos2(cpu_rect.max.x - 5.0, cpu_rect.min.y + 3.0), Align2::RIGHT_TOP, format!("{cpu_pct:.0}%"), theme::mono(9.5), theme::TEXT);

    // CPU mini meter bar
    let bar_cpu = Rect::from_min_max(pos2(cpu_rect.min.x + 6.0, cpu_rect.max.y - 6.0), pos2(cpu_rect.max.x - 5.0, cpu_rect.max.y - 3.0));
    painter.rect_filled(bar_cpu, theme::RADIUS_SM, Color32::from_rgb(14, 14, 20));
    let fill_cpu_w = (bar_cpu.width() * (cpu_pct / 100.0)).clamp(0.0, bar_cpu.width());
    if fill_cpu_w > 0.0 {
        painter.rect_filled(Rect::from_min_size(bar_cpu.min, vec2(fill_cpu_w.max(2.0), bar_cpu.height())), theme::RADIUS_SM, cpu_color);
    }

    // Divider line between CPU and RAM
    painter.line_segment(
        [pos2(rect.min.x + 64.0, rect.min.y + 4.0), pos2(rect.min.x + 64.0, rect.max.y - 4.0)],
        Stroke::new(1.0, theme::LINE_SOFT),
    );

    // Right half: RAM (width: 63.0)
    let ram_rect = Rect::from_min_max(pos2(rect.min.x + 65.0, rect.min.y), rect.max);
    let ram_str = format_ram(ram_resident);
    let ram_color = Color32::from_rgb(70, 190, 245); // Tech Cyan

    // "RAM" label + value
    painter.text(pos2(ram_rect.min.x + 6.0, ram_rect.min.y + 4.0), Align2::LEFT_TOP, "RAM", theme::mono(8.0), theme::TEXT_DIM);
    painter.text(pos2(ram_rect.max.x - 6.0, ram_rect.min.y + 3.0), Align2::RIGHT_TOP, &ram_str, theme::mono(9.5), theme::TEXT);

    // RAM mini meter bar
    let bar_ram = Rect::from_min_max(pos2(ram_rect.min.x + 6.0, ram_rect.max.y - 6.0), pos2(ram_rect.max.x - 6.0, ram_rect.max.y - 3.0));
    painter.rect_filled(bar_ram, theme::RADIUS_SM, Color32::from_rgb(14, 14, 20));
    // Relative gauge: assume up to 4GB scale for DAW baseline indicator
    let ram_fraction = (ram_resident as f32 / (4.0 * 1024.0 * 1024.0 * 1024.0)).clamp(0.02, 1.0);
    let fill_ram_w = (bar_ram.width() * ram_fraction).clamp(0.0, bar_ram.width());
    if fill_ram_w > 0.0 {
        painter.rect_filled(Rect::from_min_size(bar_ram.min, vec2(fill_ram_w.max(2.0), bar_ram.height())), theme::RADIUS_SM, ram_color);
    }

    resp.on_hover_ui(|ui| {
        ui.heading(egui::RichText::new("System Performance").font(theme::ui_font(12.0)).color(theme::TEXT));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Audio Engine Budget:").font(theme::ui_font(11.0)).color(theme::TEXT_DIM));
            ui.label(egui::RichText::new(format!("{cpu_pct:.1}%")).font(theme::mono(11.0)).color(cpu_color));
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Memory Footprint (RSS):").font(theme::ui_font(11.0)).color(theme::TEXT_DIM));
            ui.label(egui::RichText::new(&ram_str).font(theme::mono(11.0)).color(ram_color));
        });
        ui.add_space(2.0);
        ui.label(egui::RichText::new("CPU reflects buffer time used. RAM shows resident memory allocated.").font(theme::ui_font(9.5)).color(theme::TEXT_FAINT));
    });
}
