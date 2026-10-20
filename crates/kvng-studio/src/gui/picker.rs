//! The plugin picker: a searchable grid of installed plugins with their pictures.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use bridge::scan::{PluginId, PluginInfo, Scanner};
use eframe::egui::{self, Align2, Color32, ColorImage, CornerRadius, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, pos2, vec2};
use engine::song::{PluginFormatSpec, PluginSpec};

use super::{theme, widgets};

const TILE_W: f32 = 188.0;
const IMAGE_H: f32 = 112.0;
const TILE_H: f32 = IMAGE_H + 46.0;
/// Pictures are decoded at twice the tile size for sharp display on Retina screens.
const THUMB_MAX: (u32, u32) = (TILE_W as u32 * 2, IMAGE_H as u32 * 2);

/// Where a chosen plugin goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Instruments become new channels; effects go on the selected insert.
    Browse,
    Instrument,
    Effect { insert: usize },
    /// Swap the sound of an existing channel (keeps its notes and mixer track).
    Replace { channel: usize },
}

/// Drag payloads: drop a picker tile on a channel rack row to replace that channel's sound.
pub struct DraggedPlugin(pub PluginInfo);
pub struct DraggedBuiltin(pub engine::song::SourceSpec);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Instruments,
    Effects,
    Favorites,
}

pub struct Choice {
    /// A scanned plugin, or…
    pub info: Option<PluginInfo>,
    /// …one of KVNG Studio's built-in instruments…
    pub builtin: Option<engine::song::SourceSpec>,
    /// …or one of its built-in effects.
    pub builtin_effect: Option<engine::effects::BuiltinEffect>,
    pub target: Target,
}

enum ScanMsg {
    Progress(usize, usize, String),
    Done(Result<(Vec<PluginInfo>, Vec<String>), String>),
}

pub struct Picker {
    pub open: bool,
    pub target: Target,
    search: String,
    filter: Filter,
    show_duplicates: bool,
    list_view: bool,
    plugins: Vec<PluginInfo>,
    loaded: bool,
    scan: Option<(Receiver<ScanMsg>, usize, usize, String)>,
    failures: Vec<String>,
    thumbs: Thumbnails,
    message: Option<String>,
    /// Put the cursor in the search box on the next frame (when the picker opens).
    focus_search: bool,
    /// Favourite plugins ("name|vendor" keys).
    favorites: Vec<String>,
}

impl Default for Picker {
    fn default() -> Self {
        Self {
            open: false,
            target: Target::Browse,
            search: String::new(),
            filter: Filter::All,
            show_duplicates: false,
            list_view: false,
            plugins: Vec::new(),
            loaded: false,
            scan: None,
            failures: Vec::new(),
            thumbs: Thumbnails::new(),
            message: None,
            focus_search: false,
            favorites: super::presets::favorites(),
        }
    }
}

fn scanner() -> Scanner {
    Scanner::new(crate::data_dir()).with_extra_roots(super::prefs::Prefs::load().plugin_folders)
}

impl Picker {
    /// Opens the picker for `target`.
    pub fn show_for(&mut self, target: Target) {
        self.target = target;
        self.filter = match target {
            Target::Instrument | Target::Replace { .. } => Filter::Instruments,
            Target::Effect { .. } => Filter::Effects,
            Target::Browse => self.filter,
        };
        self.open = true;
        self.focus_search = true;
    }

    pub fn start_scan(&mut self, full: bool, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress_ctx = ctx.clone();
            let ptx = tx.clone();
            let result = scanner().scan(full, &mut |done, total, name| {
                let _ = ptx.send(ScanMsg::Progress(done, total, name.to_string()));
                progress_ctx.request_repaint();
            });
            let _ = tx.send(ScanMsg::Done(result));
            ctx.request_repaint();
        });
        self.scan = Some((rx, 0, 0, "Looking for plugins".into()));
    }

    /// Scan progress (done, total, current plugin) while a scan runs.
    pub fn scan_progress(&self) -> Option<(usize, usize, String)> {
        self.scan.as_ref().map(|(_, d, t, c)| (*d, *t, c.clone()))
    }

    pub fn plugin_count(&self) -> usize {
        if self.loaded { self.plugins.len() } else { scanner().cached().len() }
    }

    pub fn failures(&self) -> Vec<String> {
        self.failures.clone()
    }

    pub fn poll_scan(&mut self) {
        let Some((rx, done, total, current)) = &mut self.scan else { return };
        let mut finished = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ScanMsg::Progress(d, t, name) => {
                    *done = d;
                    *total = t;
                    *current = name;
                }
                ScanMsg::Done(r) => finished = Some(r),
            }
        }
        if let Some(result) = finished {
            match result {
                Ok((plugins, failures)) => {
                    self.plugins = plugins;
                    self.failures = failures;
                    self.thumbs.forget_all();
                }
                Err(e) => self.message = Some(e),
            }
            self.scan = None;
        }
    }

    /// Draws the picker window. Returns a plugin when one is chosen.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Choice> {
        if !self.open {
            return None;
        }
        if !self.loaded {
            self.loaded = true;
            self.plugins = scanner().cached();
            if self.plugins.is_empty() {
                self.start_scan(false, ctx);
            }
        }
        self.poll_scan();
        self.thumbs.poll(ctx);

        let title = match self.target {
            Target::Browse => "Plugin picker".to_string(),
            Target::Instrument => "Add instrument".to_string(),
            Target::Effect { insert } => format!("Add effect to insert {insert}"),
            Target::Replace { channel } => format!("Replace the sound of channel {}", channel + 1),
        };
        let mut open = self.open;
        let mut choice = None;
        egui::Window::new(title)
            .id(egui::Id::new("plugin-picker"))
            .open(&mut open)
            .default_size(vec2(860.0, 600.0))
            .min_size(vec2(520.0, 360.0))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| choice = self.contents(ui, ctx));
        self.open = open && choice.is_none();
        choice
    }

    fn contents(&mut self, ui: &mut Ui, ctx: &egui::Context) -> Option<Choice> {
        let mut choice = None;
        ui.horizontal(|ui| {
            let search = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search plugins or vendors").desired_width(240.0));
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            ui.add_space(8.0);
            for (f, label) in [(Filter::All, "All"), (Filter::Instruments, "Instruments"), (Filter::Effects, "Effects"), (Filter::Favorites, "★ Favourites")] {
                if widgets::toggle(ui, self.filter == f, label, vec2(86.0, 22.0)).clicked() {
                    self.filter = f;
                }
            }
            ui.add_space(8.0);
            ui.checkbox(&mut self.show_duplicates, "Show AU duplicates").on_hover_text("Plugins installed as both VST3 and AU are shown once (VST3) unless this is on");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let scanning = self.scan.is_some();
                if ui.add_enabled(!scanning, egui::Button::new("Rescan")).on_hover_text("Look for new or updated plugins").clicked() {
                    self.start_scan(false, ctx);
                }
                if widgets::toggle(ui, self.list_view, "List", vec2(44.0, 22.0)).on_hover_text("Show installed plugins as compact rows").clicked() {
                    self.list_view = !self.list_view;
                }
            });
        });

        if let Some((_, done, total, current)) = &self.scan {
            ui.add_space(4.0);
            let fraction = if *total == 0 { 0.0 } else { *done as f32 / *total as f32 };
            ui.add(egui::ProgressBar::new(fraction).fill(theme::ACCENT).text(format!("Scanning {done}/{total}: {current}")));
            ui.label(egui::RichText::new("Each plugin is loaded in its own helper, so a misbehaving plugin can't affect KVNG Studio.").small().color(theme::TEXT_FAINT));
        }
        if let Some(m) = &self.message {
            ui.label(egui::RichText::new(m).color(theme::WARN).small());
        }
        ui.add_space(6.0);

        // Cloned so tiles can update the picker (pictures, rescans) while drawing.
        let list: Vec<PluginInfo> = self.visible().into_iter().cloned().collect();
        let columns = ((ui.available_width() + 10.0) / (TILE_W + 10.0)).floor().max(1.0) as usize;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if !matches!(self.target, Target::Effect { .. }) && self.filter != Filter::Effects {
                ui.label(egui::RichText::new("BUILT-IN").font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    use engine::instrument::{Drum, SynthParams};
                    let mut items = vec![("KVNG Synth", "3 oscillators · filter · envelope", engine::song::SourceSpec::Synth { params: SynthParams::default() })];
                    for (name, params) in SynthParams::presets().into_iter().skip(1).take(4) {
                        items.push((name, "KVNG Synth preset", engine::song::SourceSpec::Synth { params }));
                    }
                    for d in Drum::ALL {
                        items.push((d.label(), "Synthesized drum", engine::song::SourceSpec::Drum { drum: d }));
                    }
                    items.push(("MIDI Out", "Play an external synth or device", engine::song::SourceSpec::MidiOut { port: String::new(), midi_channel: 0 }));
                    let q = self.search.to_lowercase();
                    for (name, detail, spec) in items {
                        if !q.is_empty() && !name.to_lowercase().contains(&q) {
                            continue;
                        }
                        let (tint, tag) = match &spec {
                            engine::song::SourceSpec::Synth { .. } => {
                                let tag = if name.contains("808") {
                                    "808"
                                } else if name.contains("Sub") {
                                    "SUB"
                                } else if name.contains("Lead") {
                                    "LEAD"
                                } else if name.contains("Pluck") {
                                    "PLK"
                                } else {
                                    "SYN"
                                };
                                (Color32::from_rgb(175, 120, 245), tag)
                            }
                            engine::song::SourceSpec::Drum { drum } => {
                                let tag = match drum {
                                    engine::instrument::Drum::Kick => "KIK",
                                    engine::instrument::Drum::Snare => "SNR",
                                    engine::instrument::Drum::Clap => "CLP",
                                    engine::instrument::Drum::Hat => "HAT",
                                };
                                (Color32::from_rgb(245, 170, 75), tag)
                            }
                            engine::song::SourceSpec::MidiOut { .. } => (Color32::from_rgb(90, 180, 250), "MIDI"),
                            _ => (theme::ACCENT, "KVNG"),
                        };
                        let resp = builtin_tile(ui, name, detail, tint, tag);
                        resp.dnd_set_drag_payload(DraggedBuiltin(spec.clone()));
                        if resp.clicked() {
                            let target = if matches!(self.target, Target::Replace { .. }) { self.target } else { Target::Instrument };
                            choice = Some(Choice { info: None, builtin: Some(spec), builtin_effect: None, target });
                        }
                    }
                });
                ui.add_space(10.0);
            }
            if self.filter != Filter::Instruments && !matches!(self.target, Target::Instrument | Target::Replace { .. }) {
                ui.label(egui::RichText::new("BUILT-IN EFFECTS").font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let q = self.search.to_lowercase();
                    for fx in engine::effects::BuiltinEffect::all() {
                        if !q.is_empty() && !fx.name().to_lowercase().contains(&q) {
                            continue;
                        }
                        let (tint, tag) = match &fx {
                            engine::effects::BuiltinEffect::Eq(_) => (Color32::from_rgb(60, 210, 160), "EQ"),
                            engine::effects::BuiltinEffect::Filter(_) => (Color32::from_rgb(50, 200, 190), "FLT"),
                            engine::effects::BuiltinEffect::Compressor(_) => (Color32::from_rgb(245, 110, 120), "CMP"),
                            engine::effects::BuiltinEffect::Gate(_) => (Color32::from_rgb(245, 110, 120), "GATE"),
                            engine::effects::BuiltinEffect::Reverb(_) => (Color32::from_rgb(70, 200, 240), "RVB"),
                            engine::effects::BuiltinEffect::Delay(_) => (Color32::from_rgb(70, 200, 240), "DLY"),
                            engine::effects::BuiltinEffect::Distortion(_) => (Color32::from_rgb(245, 80, 60), "DIST"),
                            engine::effects::BuiltinEffect::Chorus(_) => (Color32::from_rgb(225, 100, 210), "CHO"),
                            engine::effects::BuiltinEffect::Flanger(_) => (Color32::from_rgb(225, 100, 210), "FLG"),
                            engine::effects::BuiltinEffect::Phaser(_) => (Color32::from_rgb(225, 100, 210), "PHS"),
                        };
                        if builtin_tile(ui, fx.name(), fx.description(), tint, tag).clicked() {
                            let target = match self.target {
                                Target::Effect { insert } => Target::Effect { insert },
                                _ => Target::Browse,
                            };
                            choice = Some(Choice { info: None, builtin: None, builtin_effect: Some(fx), target });
                        }
                    }
                });
                ui.add_space(10.0);
            }
            for section in [true, false] {
                let items: Vec<&PluginInfo> = list.iter().filter(|p| p.is_instrument == section).collect();
                if items.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(egui::RichText::new(if section { "INSTRUMENTS" } else { "EFFECTS" }).font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
                ui.add_space(2.0);
                let row_size = if self.list_view { 1 } else { columns };
                for row in items.chunks(row_size) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        for info in row {
                            if let Some(action) = if self.list_view { self.list_row(ui, info) } else { self.tile(ui, info) } {
                                match action {
                                    TileAction::Choose => choice = Some(Choice { info: Some((*info).clone()), builtin: None, builtin_effect: None, target: self.target }),
                                    TileAction::SetPicture => self.set_picture(&info.id),
                                    TileAction::Retake => self.retake(&info.id, ctx),
                                    TileAction::ToggleFavorite => {
                                        let key = super::presets::favorite_key(&info.name, &info.vendor);
                                        if let Some(i) = self.favorites.iter().position(|k| *k == key) {
                                            self.favorites.remove(i);
                                        } else {
                                            self.favorites.push(key);
                                        }
                                        super::presets::set_favorites(&self.favorites);
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(if self.list_view { 2.0 } else { 10.0 });
                }
            }
            if list.is_empty() && self.scan.is_none() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("No plugins match.").color(theme::TEXT_FAINT)));
            }
            if !self.failures.is_empty() {
                ui.add_space(8.0);
                egui::CollapsingHeader::new(egui::RichText::new(format!("{} plugins couldn't be loaded", self.failures.len())).color(theme::WARN).small()).show(ui, |ui| {
                    for f in &self.failures {
                        ui.label(egui::RichText::new(f).small().color(theme::TEXT_DIM));
                    }
                });
            }
        });
        choice
    }

    fn visible(&self) -> Vec<&PluginInfo> {
        let q = self.search.to_lowercase();
        let mut seen = HashSet::new();
        let mut out: Vec<&PluginInfo> = self
            .plugins
            .iter()
            .filter(|p| match self.filter {
                Filter::All => true,
                Filter::Instruments => p.is_instrument,
                Filter::Effects => !p.is_instrument,
                Filter::Favorites => self.favorites.contains(&super::presets::favorite_key(&p.name, &p.vendor)),
            })
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.vendor.to_lowercase().contains(&q) || p.category.to_lowercase().contains(&q))
            .collect();
        // VST3 first, so it wins when both formats are installed.
        out.sort_by_key(|p| (p.name.to_lowercase(), matches!(p.id, PluginId::AudioUnit { .. })));
        if !self.show_duplicates {
            out.retain(|p| seen.insert((p.name.to_lowercase(), p.vendor.to_lowercase())));
        }
        out
    }

    fn tile(&mut self, ui: &mut Ui, info: &PluginInfo) -> Option<TileAction> {
        let (rect, resp) = ui.allocate_exact_size(vec2(TILE_W, TILE_H), Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            return None;
        }
        if info.is_instrument {
            resp.dnd_set_drag_payload(DraggedPlugin(info.clone()));
        }
        let hovered = resp.hovered();
        let painter = ui.painter();
        painter.rect(rect, theme::RADIUS, if hovered { theme::RAISED_HI } else { theme::RAISED }, Stroke::new(1.0, if hovered { theme::ACCENT } else { theme::LINE }), StrokeKind::Inside);

        let image_rect = Rect::from_min_size(rect.min + vec2(5.0, 5.0), vec2(TILE_W - 10.0, IMAGE_H - 5.0));
        let texture = info.snapshot.as_ref().and_then(|p| self.thumbs.get(p));
        match texture {
            Some(tex) => {
                // Cover the image area, cropping the excess (centred).
                let size = tex.size_vec2();
                let scale = (image_rect.width() / size.x).max(image_rect.height() / size.y);
                let shown = vec2(image_rect.width() / (size.x * scale), image_rect.height() / (size.y * scale));
                let uv = Rect::from_center_size(pos2(0.5, 0.5), shown);
                painter.rect_filled(image_rect, theme::RADIUS_SM, theme::BG);
                painter.image(tex.id(), image_rect, uv, Color32::WHITE);
            }
            None => placeholder(painter, image_rect, info),
        }

        let text_x = rect.min.x + 8.0;
        let name = theme::elide(painter, &info.name, theme::ui_font(12.0), TILE_W - 16.0);
        painter.text(pos2(text_x, rect.min.y + IMAGE_H + 12.0), Align2::LEFT_CENTER, name, theme::ui_font(12.0), theme::TEXT);
        let vendor = theme::elide(painter, &info.vendor, theme::ui_font(10.0), TILE_W - 80.0);
        painter.text(pos2(text_x, rect.min.y + IMAGE_H + 30.0), Align2::LEFT_CENTER, vendor, theme::ui_font(10.0), theme::TEXT_DIM);
        let mut bx = rect.max.x - 8.0;
        for (label, color) in [(Some(info.format_name()), theme::TEXT_FAINT), ((info.arch == "x86_64").then_some("Intel"), theme::WARN)] {
            let Some(label) = label else { continue };
            let galley = painter.layout_no_wrap(label.to_string(), theme::ui_font(9.0), color);
            let badge = Rect::from_min_max(pos2(bx - galley.size().x - 8.0, rect.min.y + IMAGE_H + 22.0), pos2(bx, rect.min.y + IMAGE_H + 38.0));
            painter.rect_stroke(badge, theme::RADIUS_SM, Stroke::new(1.0, color), StrokeKind::Inside);
            painter.galley(pos2(badge.min.x + 4.0, badge.center().y - galley.size().y / 2.0), galley, color);
            bx = badge.min.x - 4.0;
        }

        let mut action = None;
        // Favourite star (top-right corner).
        let key = super::presets::favorite_key(&info.name, &info.vendor);
        let fav = self.favorites.contains(&key);
        let star_rect = Rect::from_min_size(pos2(rect.max.x - 24.0, rect.min.y + 4.0), vec2(20.0, 20.0));
        let star_resp = ui.interact(star_rect, ui.id().with(("fav", &key)), Sense::click());
        if fav || hovered || star_resp.hovered() {
            let c = star_rect.center();
            let pts: Vec<egui::Pos2> = (0..10)
                .map(|k| {
                    let a = std::f32::consts::PI * (k as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
                    let r = if k % 2 == 0 { 7.5 } else { 3.2 };
                    pos2(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            let fill = if fav { theme::ACCENT } else { theme::alpha(theme::BG, 0.6) };
            // A star isn't convex: draw it as triangles from the centre.
            for k in 0..10 {
                ui.painter().add(egui::Shape::convex_polygon(vec![c, pts[k], pts[(k + 1) % 10]], fill, Stroke::NONE));
            }
            ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.0, if star_resp.hovered() { theme::ACCENT_HOT } else { theme::TEXT_DIM })));
        }
        if star_resp.on_hover_text(if fav { "Remove from favourites" } else { "Add to favourites" }).clicked() {
            action = Some(TileAction::ToggleFavorite);
        } else if resp.clicked() {
            action = Some(TileAction::Choose);
        }
        let intel = if info.arch == "x86_64" { "\nRuns in the Intel helper (Rosetta)" } else { "" };
        let resp = resp.on_hover_text(format!("{}\n{} · {} {}{intel}\nClick to add", info.name, info.vendor, info.format_name(), info.category));
        resp.context_menu(|ui| {
            if ui.button("Add").clicked() {
                action = Some(TileAction::Choose);
                ui.close();
            }
            if ui.button("Set picture…").clicked() {
                action = Some(TileAction::SetPicture);
                ui.close();
            }
            if ui.button("Retake picture").on_hover_text("Scan this plugin again, e.g. after dismissing a first-run popup").clicked() {
                action = Some(TileAction::Retake);
                ui.close();
            }
        });
        action
    }

    fn list_row(&mut self, ui: &mut Ui, info: &PluginInfo) -> Option<TileAction> {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 38.0), Sense::click_and_drag());
        if !ui.is_rect_visible(rect) { return None; }
        if info.is_instrument { resp.dnd_set_drag_payload(DraggedPlugin(info.clone())); }
        let p = ui.painter();
        p.rect(rect, theme::RADIUS_SM, if resp.hovered() { theme::RAISED_HI } else { theme::RAISED }, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
        let name = theme::elide(p, &info.name, theme::ui_font(12.5), rect.width() * 0.46);
        p.text(pos2(rect.min.x + 10.0, rect.center().y), Align2::LEFT_CENTER, name, theme::ui_font(12.5), theme::TEXT);
        let vendor = theme::elide(p, &info.vendor, theme::ui_font(11.0), rect.width() * 0.25);
        p.text(pos2(rect.min.x + rect.width() * 0.5, rect.center().y), Align2::LEFT_CENTER, vendor, theme::ui_font(11.0), theme::TEXT_DIM);
        p.text(pos2(rect.max.x - 38.0, rect.center().y), Align2::RIGHT_CENTER, info.format_name(), theme::mono(10.5), theme::TEXT_DIM);
        let key = super::presets::favorite_key(&info.name, &info.vendor);
        let favorite = self.favorites.contains(&key);
        let star = Rect::from_center_size(pos2(rect.max.x - 17.0, rect.center().y), vec2(26.0, 26.0));
        let star_resp = ui.interact(star, ui.id().with(("list-fav", &key)), Sense::click());
        p.text(star.center(), Align2::CENTER_CENTER, "★", theme::ui_font(16.0), if favorite { theme::ACCENT } else { theme::TEXT_DIM });
        let mut action = if star_resp.clicked() { Some(TileAction::ToggleFavorite) } else if resp.clicked() { Some(TileAction::Choose) } else { None };
        resp.on_hover_text(format!("{} · {} · {}", info.vendor, info.category, info.format_name())).context_menu(|ui| {
            if ui.button("Add").clicked() { action = Some(TileAction::Choose); ui.close(); }
            if ui.button("Set picture…").clicked() { action = Some(TileAction::SetPicture); ui.close(); }
            if ui.button("Retake picture").clicked() { action = Some(TileAction::Retake); ui.close(); }
        });
        action
    }

    fn set_picture(&mut self, id: &PluginId) {
        let Some(path) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_title("Choose a picture").pick_file() else { return };
        match scanner().set_picture(id, &path) {
            Ok(_) => {
                self.plugins = scanner().cached();
                self.thumbs.forget_all();
            }
            Err(e) => self.message = Some(e),
        }
    }

    fn retake(&mut self, id: &PluginId, ctx: &egui::Context) {
        let id = id.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = scanner().rescan(&id).map(|_| (scanner().cached(), Vec::new()));
            let _ = tx.send(ScanMsg::Done(r));
            ctx.request_repaint();
        });
        self.scan = Some((rx, 0, 1, "Retaking picture".into()));
    }
}

enum TileAction {
    Choose,
    SetPicture,
    Retake,
    ToggleFavorite,
}

fn builtin_tile(ui: &mut Ui, name: &str, detail: &str, tint: Color32, icon: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(TILE_W, 64.0), Sense::click_and_drag());
    let painter = ui.painter();
    let hovered = resp.hovered();
    let bg = if hovered { theme::RAISED_HI } else { theme::RAISED };
    let border = if hovered { tint } else { theme::LINE };
    painter.rect(rect, theme::RADIUS, bg, Stroke::new(1.0, border), StrokeKind::Inside);

    // Subtle category indicator tab on left edge
    let left_tab = Rect::from_min_size(rect.min, vec2(3.0, rect.height()));
    painter.rect_filled(left_tab, CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, tint);

    let badge = Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(48.0, 48.0));
    painter.rect_filled(badge, theme::RADIUS_SM, theme::mix(tint, theme::BG, 0.78));
    painter.rect_stroke(badge, theme::RADIUS_SM, Stroke::new(1.0, theme::mix(tint, theme::LINE, 0.45)), StrokeKind::Inside);
    painter.text(badge.center(), Align2::CENTER_CENTER, icon, theme::mono(12.5), tint);

    painter.text(pos2(badge.max.x + 10.0, rect.min.y + 24.0), Align2::LEFT_CENTER, name, theme::ui_font(12.5), theme::TEXT);
    painter.text(pos2(badge.max.x + 10.0, rect.min.y + 42.0), Align2::LEFT_CENTER, detail, theme::ui_font(9.5), theme::TEXT_DIM);
    resp.on_hover_text("Click to add · drag onto a channel to replace its sound")
}

/// A stand-in picture: the vendor's initials on a colour derived from its name.
fn placeholder(painter: &egui::Painter, rect: Rect, info: &PluginInfo) {
    let base = theme::clip_color(&info.vendor);
    painter.rect_filled(rect, theme::RADIUS_SM, theme::mix(base, theme::BG, 0.65));
    let initials: String = info.name.split_whitespace().take(2).filter_map(|w| w.chars().next()).collect::<String>().to_uppercase();
    painter.text(rect.center(), Align2::CENTER_CENTER, initials, theme::ui_font(30.0), theme::mix(base, Color32::WHITE, 0.35));
}

/// Plugin pictures, decoded and downscaled on a worker thread.
struct Thumbnails {
    textures: HashMap<PathBuf, TextureHandle>,
    pending: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    tx: Sender<PathBuf>,
    rx: Receiver<(PathBuf, Option<ColorImage>)>,
}

impl Thumbnails {
    fn new() -> Self {
        let (tx, worker_rx) = mpsc::channel::<PathBuf>();
        let (worker_tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(path) = worker_rx.recv() {
                let image = decode_thumbnail(&path);
                if worker_tx.send((path, image)).is_err() {
                    break;
                }
            }
        });
        Self { textures: HashMap::new(), pending: HashSet::new(), failed: HashSet::new(), tx, rx }
    }

    fn get(&mut self, path: &Path) -> Option<&TextureHandle> {
        if !self.textures.contains_key(path) && !self.pending.contains(path) && !self.failed.contains(path) {
            self.pending.insert(path.to_path_buf());
            let _ = self.tx.send(path.to_path_buf());
        }
        self.textures.get(path)
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut any = false;
        while let Ok((path, image)) = self.rx.try_recv() {
            self.pending.remove(&path);
            match image {
                Some(img) => {
                    let tex = ctx.load_texture(path.to_string_lossy(), img, TextureOptions::LINEAR);
                    self.textures.insert(path, tex);
                }
                None => {
                    self.failed.insert(path);
                }
            }
            any = true;
        }
        if any || !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    fn forget_all(&mut self) {
        self.textures.clear();
        self.failed.clear();
    }
}

/// Decodes a PNG and box-filters it down to at most [`THUMB_MAX`].
fn decode_thumbnail(path: &Path) -> Option<ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.color_type.samples();
    let rgba = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * channels;
        match channels {
            4 => [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]],
            3 => [buf[i], buf[i + 1], buf[i + 2], 255],
            2 => [buf[i], buf[i], buf[i], buf[i + 1]],
            _ => [buf[i], buf[i], buf[i], 255],
        }
    };
    let scale = (THUMB_MAX.0 as f32 / w as f32).min(THUMB_MAX.1 as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale) as usize).max(1), ((h as f32 * scale) as usize).max(1));
    let mut pixels = Vec::with_capacity(tw * th);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = rgba(x, y);
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            pixels.push(Color32::from_rgba_unmultiplied((acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, (acc[3] / n) as u8));
        }
    }
    Some(ColorImage::new([tw, th], pixels))
}

/// The engine's description of a scanned plugin.
pub fn plugin_spec(info: &PluginInfo) -> PluginSpec {
    let format = match &info.id {
        PluginId::Vst3 { path, name } => PluginFormatSpec::Vst3 { path: path.clone(), name: Some(name.clone()) },
        PluginId::AudioUnit { kind, subtype, manufacturer } => {
            PluginFormatSpec::AudioUnit { kind: kind.clone(), subtype: subtype.clone(), manufacturer: manufacturer.clone() }
        }
    };
    PluginSpec { name: info.name.clone(), vendor: info.vendor.clone(), format, state: None }
}
