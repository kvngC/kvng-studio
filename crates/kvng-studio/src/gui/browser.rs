//! The browser: sample folders on the left, like FL Studio's browser.
//! Click a sound to hear it, double-click to add it as a channel, or drag it to the
//! playlist (audio clip) or channel rack (sampler channel).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Align2, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use engine::sample::Sample;

use super::session::Session;
use super::theme;

/// What is dragged out of the browser.
#[derive(Clone)]
pub struct DraggedFile(pub PathBuf);

/// A finished background sample load and what to do with it.
pub enum Loaded {
    Preview(Arc<Sample>),
    AddChannel(PathBuf, Arc<Sample>),
    AddClip { path: PathBuf, sample: Arc<Sample>, track: usize, start: u64 },
    /// Replace an existing channel's sound with this sample.
    Replace { channel: usize, path: PathBuf, sample: Arc<Sample> },
}

pub struct Browser {
    roots: Vec<PathBuf>,
    expanded: HashSet<PathBuf>,
    listing: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    selected: Option<PathBuf>,
    filter: String,
    jobs: Vec<Receiver<Result<Loaded, String>>>,
    /// Favourite samples (shown in a "Favourites" folder at the top).
    favorites: Vec<PathBuf>,
    /// The last previewed sample: path, peak envelope, length in seconds.
    preview: Option<(PathBuf, Arc<Vec<f32>>, f64)>,
    /// File details shown on hover ("0:03.2 · 44.1 kHz · stereo · 24-bit").
    info: HashMap<PathBuf, String>,
    /// Every audio file under the roots, for searching (built on first search).
    index: Option<Vec<PathBuf>>,
}

impl Default for Browser {
    fn default() -> Self {
        let mut b = Self {
            roots: Vec::new(),
            expanded: HashSet::new(),
            listing: HashMap::new(),
            selected: None,
            filter: String::new(),
            jobs: Vec::new(),
            favorites: std::fs::read_to_string(favorites_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
            preview: None,
            info: HashMap::new(),
            index: None,
        };
        b.roots = b.saved_roots();
        if let Some(first) = b.roots.first() { b.expanded.insert(first.clone()); }
        b
    }
}

fn favorites_file() -> PathBuf {
    crate::data_dir().join("browser-favorites.json")
}

/// Format, length and channel details from the header (cheap: no samples are read).
fn describe(path: &Path) -> String {
    match engine::sample::audio_description(path) {
        Ok(info) => info,
        Err(e) => format!("can't read: {e}"),
    }
}

fn roots_file() -> PathBuf {
    crate::data_dir().join("browser-folders.json")
}

impl Browser {
    fn saved_roots(&self) -> Vec<PathBuf> {
        if let Some(saved) = std::fs::read_to_string(roots_file()).ok().and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok()) {
            return saved.into_iter().filter(|p| p.is_dir()).collect();
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        ["Documents/Image-Line/samples", "Music", "Downloads", "Desktop"].iter().map(|r| home.join(r)).filter(|p| p.is_dir()).collect()
    }

    fn save_roots(&self) {
        let _ = std::fs::create_dir_all(crate::data_dir());
        let _ = std::fs::write(roots_file(), serde_json::to_string(&self.roots).unwrap_or_default());
    }

    fn children(&mut self, dir: &Path) -> Vec<(PathBuf, bool)> {
        if let Some(c) = self.listing.get(dir) {
            return c.clone();
        }
        let mut items: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let name = p.file_name()?.to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let is_dir = p.is_dir();
                let audio = engine::sample::is_audio_file(&p);
                (is_dir || audio).then_some((p, is_dir))
            })
            .collect();
        items.sort_by_key(|(p, d)| (!*d, p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
        self.listing.insert(dir.to_path_buf(), items.clone());
        items
    }

    /// Selects and previews a file (as a click would).
    pub fn preview_file(&mut self, path: PathBuf, sample_rate: f64) {
        self.selected = Some(path.clone());
        self.load(path, sample_rate, |_, sample| Loaded::Preview(sample));
    }

    /// Loads `path` at the session's rate on a worker thread.
    pub fn load(&mut self, path: PathBuf, sample_rate: f64, then: impl FnOnce(PathBuf, Arc<Sample>) -> Loaded + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = engine::sample::load_audio(&path, sample_rate).map(|s| then(path, Arc::new(s)));
            let _ = tx.send(r);
        });
        self.jobs.push(rx);
    }

    /// Finished loads, to apply to the session.
    pub fn poll(&mut self) -> Vec<Result<Loaded, String>> {
        let mut done = Vec::new();
        self.jobs.retain(|rx| match rx.try_recv() {
            Ok(r) => {
                done.push(r);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        // Keep a waveform of what was just previewed for the preview strip.
        for r in &done {
            if let (Ok(Loaded::Preview(sample)), Some(path)) = (r, &self.selected) {
                self.preview = Some((path.clone(), Arc::new(engine::view::waveform(&sample.left, &sample.right)), sample.len() as f64));
            }
        }
        done
    }

    pub fn ui(&mut self, ui: &mut Ui, s: &Session) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("BROWSER").font(theme::ui_font(10.5)).color(theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+ Folder").on_hover_text("Add a sample folder").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_title("Add a sample folder").pick_folder()
                    && !self.roots.contains(&dir)
                {
                    self.roots.push(dir);
                    self.save_roots();
                    self.index = None;
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search samples").desired_width(f32::INFINITY));
        ui.add_space(4.0);

        let preview_h = if self.preview.is_some() { 70.0 } else { 0.0 };
        let list_h = (ui.available_height() - preview_h).max(80.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
            // Searching (2+ letters) lists matches from every folder, flat.
            let q = self.filter.trim().to_lowercase();
            if q.chars().count() >= 2 {
                let roots = self.roots.clone();
                let index = self.index.get_or_insert_with(|| build_index(&roots)).clone();
                let hits: Vec<PathBuf> = index.into_iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q))).take(500).collect();
                if hits.is_empty() {
                    ui.label(egui::RichText::new("No samples match").color(theme::TEXT_DIM));
                }
                for p in hits {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.file_row(ui, &p, &name, 0, s);
                }
                return;
            }
            if !self.favorites.is_empty() {
                let fav_open = self.expanded.contains(Path::new("★"));
                if row(ui, 0, if fav_open { "▾" } else { "▸" }, "★ Favourites", false, true).clicked() {
                    if fav_open {
                        self.expanded.remove(Path::new("★"));
                    } else {
                        self.expanded.insert(PathBuf::from("★"));
                    }
                }
                if fav_open {
                    for p in self.favorites.clone() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.file_row(ui, &p, &name, 1, s);
                    }
                }
            }
            let roots = self.roots.clone();
            if roots.is_empty() {
                ui.add_space(14.0);
                ui.label(egui::RichText::new("No sample folders yet.").color(theme::TEXT));
                ui.label(egui::RichText::new("Choose + Folder to browse and preview your sounds.").color(theme::TEXT_DIM));
            }
            let mut remove = None;
            for root in &roots {
                let label = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
                let r = self.folder_row(ui, root, &label, 0, s);
                r.context_menu(|ui| {
                    if ui.button("Remove from browser").clicked() {
                        remove = Some(root.clone());
                        ui.close();
                    }
                    if ui.button("Refresh").clicked() {
                        self.listing.clear();
                        self.index = None;
                        ui.close();
                    }
                });
            }
            if let Some(r) = remove {
                self.roots.retain(|x| x != &r);
                self.save_roots();
                self.index = None;
            }
        });
        self.preview_strip(ui);
    }

    /// The previewed sample's waveform, name and length.
    fn preview_strip(&mut self, ui: &mut Ui) {
        let Some((path, wave, _frames)) = self.preview.clone() else { return };
        ui.add_space(4.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::click());
        let p = ui.painter();
        p.rect(rect, theme::RADIUS, theme::BG, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
        let wave_rect = rect.shrink2(vec2(6.0, 14.0)).translate(vec2(0.0, 6.0));
        let mid = wave_rect.center().y;
        if !wave.is_empty() {
            // Scaled to its own peak so quiet samples are still readable.
            let top = wave.iter().fold(0.0f32, |m, v| m.max(*v)).max(1e-4);
            let n = wave_rect.width().max(1.0) as usize;
            for x in 0..n {
                let a = x * wave.len() / n;
                let b = ((x + 1) * wave.len() / n).max(a + 1).min(wave.len());
                let peak = (wave[a..b].iter().fold(0.0f32, |m, v| m.max(*v)) / top).min(1.0);
                let h = (peak * wave_rect.height() / 2.0).max(0.5);
                let px = wave_rect.min.x + x as f32 + 0.5;
                p.line_segment([pos2(px, mid - h), pos2(px, mid + h)], Stroke::new(1.0, theme::ACCENT));
            }
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let label = theme::elide(p, &name, theme::ui_font(10.0), rect.width() - 70.0);
        p.text(pos2(rect.min.x + 6.0, rect.min.y + 4.0), Align2::LEFT_TOP, label, theme::ui_font(10.0), theme::TEXT);
        let info = self.info.entry(path.clone()).or_insert_with(|| describe(&path)).clone();
        let short = info.split(" · ").next().unwrap_or("").to_string();
        p.text(pos2(rect.max.x - 6.0, rect.min.y + 4.0), Align2::RIGHT_TOP, short, theme::mono(9.5), theme::TEXT_DIM);
        resp.on_hover_text(format!("{info}\nClick a sample to preview it here"));
    }

    fn toggle_favorite(&mut self, path: &Path) {
        if let Some(i) = self.favorites.iter().position(|p| p == path) {
            self.favorites.remove(i);
        } else {
            self.favorites.push(path.to_path_buf());
        }
        let _ = std::fs::create_dir_all(crate::data_dir());
        let _ = std::fs::write(favorites_file(), serde_json::to_string_pretty(&self.favorites).unwrap_or_default());
    }

    fn folder_row(&mut self, ui: &mut Ui, dir: &Path, label: &str, depth: usize, s: &Session) -> egui::Response {
        let open = self.expanded.contains(dir);
        let resp = row(ui, depth, if open { "▾" } else { "▸" }, label, false, true);
        if resp.clicked() {
            if open {
                self.expanded.remove(dir);
            } else {
                self.expanded.insert(dir.to_path_buf());
            }
        }
        if open {
            let q = self.filter.to_lowercase();
            for (path, is_dir) in self.children(dir) {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if is_dir {
                    self.folder_row(ui, &path, &name, depth + 1, s);
                } else if q.is_empty() || name.to_lowercase().contains(&q) {
                    self.file_row(ui, &path, &name, depth + 1, s);
                }
            }
        }
        resp
    }

    fn file_row(&mut self, ui: &mut Ui, path: &Path, name: &str, depth: usize, s: &Session) {
        let selected = self.selected.as_deref() == Some(path);
        let id = egui::Id::new(("browser-file", path));
        let fav = self.favorites.iter().any(|p| p == path);
        let resp = ui.dnd_drag_source(id, DraggedFile(path.to_path_buf()), |ui| row(ui, depth, if fav { "★" } else { "♪" }, name, selected, false)).response;
        let sr = s.view.sample_rate;
        if resp.clicked() {
            self.selected = Some(path.to_path_buf());
            self.load(path.to_path_buf(), sr, |_, sample| Loaded::Preview(sample));
        }
        if resp.double_clicked() {
            self.load(path.to_path_buf(), sr, Loaded::AddChannel);
        }
        if resp.hovered() && !self.info.contains_key(path) {
            self.info.insert(path.to_path_buf(), describe(path));
        }
        let info = self.info.get(path).cloned().unwrap_or_default();
        let resp = resp.on_hover_text(format!("{info}\nClick to preview · Double-click to add as a channel · Drag to the playlist or channel rack · Right-click for favourites"));
        resp.context_menu(|ui| {
            if ui.button(if fav { "Remove from favourites" } else { "Add to favourites" }).clicked() {
                self.toggle_favorite(path);
                ui.close();
            }
            if ui.button("Show in Finder").clicked() {
                let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
                ui.close();
            }
        });
    }
}

fn row(ui: &mut Ui, depth: usize, icon: &str, label: &str, selected: bool, folder: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect(rect, theme::RADIUS_SM, theme::ACCENT_DIM, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    } else if resp.hovered() {
        painter.rect_filled(rect, theme::RADIUS_SM, theme::RAISED);
    }
    let x = rect.min.x + 6.0 + depth as f32 * 12.0;
    painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, icon, theme::ui_font(10.0), if folder { theme::TEXT_DIM } else { theme::ACCENT });
    let text = theme::elide(painter, label, theme::ui_font(11.5), rect.max.x - x - 20.0);
    painter.text(pos2(x + 14.0, rect.center().y), Align2::LEFT_CENTER, text, theme::ui_font(11.5), if folder { theme::TEXT } else { theme::TEXT_DIM });
    resp
}

/// Every audio file under `roots` (bounded, so huge drives don't stall the UI for long).
fn build_index(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    let mut dirs = 0;
    while let Some(d) = stack.pop() {
        dirs += 1;
        if dirs > 5_000 || out.len() > 50_000 {
            break;
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if engine::sample::is_audio_file(&p) {
                out.push(p);
            }
        }
    }
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out
}
