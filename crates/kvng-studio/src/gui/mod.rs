//! KVNG Studio's window: welcome, loading, and the main workspace.

mod audio;
mod audio_editor;
mod automation;
mod browser;
mod effect_ui;
mod midi;
mod params;
mod mixer;
mod help;
mod inspector;
mod piano;
mod picker;
mod prefs;
mod presets;
mod recovery;
mod settings_ui;
mod playlist;
mod rack;
mod recorder;
mod session;
mod strip_ui;
mod synth_ui;
mod theme;
mod splash;
mod transport;
mod widgets;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use eframe::egui::{self, Align2, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use session::{Mode, Session};

/// Which panels are open.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Windows {
    pub browser: bool,
    pub playlist: bool,
    pub rack: bool,
    pub piano: bool,
    pub mixer: bool,
    pub inspector: bool,
}

impl Default for Windows {
    fn default() -> Self {
        Self { browser: true, playlist: true, rack: true, piano: false, mixer: false, inspector: false }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
enum DockTab { #[default] Rack, Piano }

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct WorkspaceLayout {
    windows: Windows,
    dock_tab: DockTab,
    rack_floating: bool,
    piano_floating: bool,
    mixer_detached: bool,
    show_all_inserts: bool,
    wide_mixer_strips: bool,
}

impl WorkspaceLayout {
    fn path() -> PathBuf { crate::data_dir().join("workspace.json") }
    fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn save(self) {
        let path = Self::path();
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(json) = serde_json::to_string_pretty(&self) {
            let temp = path.with_extension("json.tmp");
            if std::fs::write(&temp, json).is_ok() { let _ = std::fs::rename(temp, path); }
        }
    }
}

enum LoadMsg {
    Progress(String),
    Done(Result<Box<engine::load::Loaded>, String>),
}

/// What to open: an FL Studio project, a KVNG project, or a blank one.
#[derive(Clone)]
enum Source {
    Flp(PathBuf),
    Kvng(PathBuf),
    New,
    /// An in-memory project: reopening after an audio device change, or a recovered autosave.
    Memory { song: Box<engine::song::Song>, path: Option<PathBuf>, modified: bool },
}

impl Source {
    fn from_path(path: PathBuf) -> Source {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(engine::song::EXTENSION)) { Source::Kvng(path) } else { Source::Flp(path) }
    }

    fn label(&self) -> String {
        match self {
            Source::Flp(p) | Source::Kvng(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Source::New => "a new project".into(),
            Source::Memory { song, .. } => song.name.clone(),
        }
    }
}

enum State {
    Welcome { error: Option<String> },
    Loading { source: Source, status: String, rx: Receiver<LoadMsg>, output: audio::Output, started: Instant },
    Ready(Box<Session>),
}

/// An action held back until the user decides about unsaved changes.
#[derive(Clone)]
enum Pending {
    Open(Source),
    Quit,
}

enum ExportMsg {
    Progress(String, f32),
    /// The written path and a short report (peak level, clipping).
    Done(Result<(String, ExportReport), String>),
}

/// What an export measured.
#[derive(Clone, Copy)]
struct ExportReport {
    /// Peak of the rendered mix before any normalizing, dBFS.
    peak_db: f32,
    /// Integer formats clip above 0 dBFS (when not normalized).
    clipped: bool,
    /// Gain applied by normalizing, dB.
    normalized_db: Option<f32>,
}

/// What part of the song to export.
#[derive(Clone, Copy, PartialEq)]
enum ExportRange {
    Song,
    /// The playlist's loop selection (ticks).
    Loop(u64, u64),
}

/// Render settings, like FL's export dialog.
#[derive(Clone, Copy, PartialEq)]
struct ExportOptions {
    /// One file per mixer insert (plus the master) instead of a single mixdown.
    stems: bool,
    /// 16 or 24 (integer) or 32 (float).
    bits: u16,
    sample_rate: u32,
    range: ExportRange,
    /// Seconds rendered after the end so reverb and delay tails ring out.
    tail: f32,
    /// Raise (or lower) the mix so its peak sits at −0.3 dBFS.
    normalize: bool,
    /// Add triangular dither when writing 16-bit.
    dither: bool,
    /// Write FLAC (lossless, smaller) instead of WAV (16 or 24 bit).
    flac: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { stems: false, bits: 24, sample_rate: 48_000, range: ExportRange::Song, tail: 2.0, normalize: false, dither: true, flac: false }
    }
}

struct Export {
    rx: Receiver<ExportMsg>,
    status: String,
    fraction: f32,
    result: Option<Result<(String, ExportReport), String>>,
}

struct App {
    state: State,
    windows: Windows,
    dock_tab: DockTab,
    rack_floating: bool,
    piano_floating: bool,
    saved_layout: WorkspaceLayout,
    playlist: playlist::PlaylistState,
    piano: piano::PianoState,
    mixer: mixer::MixerState,
    /// A message shown in the status bar for a few seconds.
    notice: Option<(String, Instant)>,
    show_warnings: bool,
    last_frame: Instant,
    started: Instant,
    /// Developer screenshot (`KVNG_SCREENSHOT=out.png`): frames rendered since loading, and whether requested.
    screenshot: Option<Screenshot>,
    /// Waiting on the "save changes?" prompt.
    confirm: Option<Pending>,
    /// The user chose to quit; let the next close request through.
    quitting: bool,
    export: Option<Export>,
    prefs: prefs::Prefs,
    settings: settings_ui::SettingsWindow,
    audio_info: Option<settings_ui::AudioInfo>,
    last_autosave: Instant,
    /// Periodic project snapshots, including plugin-state capture and disk writes,
    /// run away from the UI frame.
    autosave_job: Option<std::thread::JoinHandle<Result<(), String>>>,
    /// Autosaves left by a crash, offered on the welcome screen.
    recoveries: Vec<recovery::AutosaveInfo>,
    /// Channels loading in the background because something started playing them.
    channel_loads: Vec<Receiver<ChannelLoad>>,
    /// An effect chain loading in the background: (insert, name, result).
    chain_load: Option<(usize, String, Receiver<Vec<ChainItem>>)>,
    /// A channel being rendered to audio: (channel, output file, result).
    bounce: Option<Bounce>,
    help_window: Option<HelpWindow>,
    picker: picker::Picker,
    browser: browser::Browser,
    /// A plugin being loaded (in the background) for the picker's target.
    plugin_load: Option<PluginLoad>,
    /// Channel or insert being renamed, with the text typed so far.
    rename: Option<(RenameTarget, String)>,
    /// The computer keyboard plays notes (FL's typing keyboard).
    typing_keyboard: bool,
    /// Channel whose built-in instrument window is open.
    channel_window: Option<usize>,
    audio_editor: Option<audio_editor::AudioEditor>,
    /// Screenshot mode: open the audio editor on the first sample channel once loaded.
    pending_audio_editor: bool,
    /// Automation clip open in the editor.
    automation_window: Option<usize>,
    /// Built-in effect (insert, slot) whose settings are open.
    effect_window: Option<(usize, usize)>,
    strip_window: Option<usize>,
    /// The export dialog, while open.
    export_setup: Option<ExportOptions>,
    /// Last-used export settings.
    export_options: ExportOptions,
    settings_open: bool,
    /// Choosing a plugin parameter to automate.
    param_picker: Option<params::ParamPicker>,
    splash: splash::Splash,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HelpWindow {
    Shortcuts,
    About,
}

#[derive(Clone, Copy)]
enum RenameTarget {
    Channel(usize),
    Insert(usize),
}

struct PluginLoad {
    name: String,
    target: picker::Target,
    spec: engine::song::PluginSpec,
    rx: Receiver<Result<engine::PluginInstance, String>>,
}

struct Screenshot {
    path: PathBuf,
    frames: u32,
    requested: bool,
    play: bool,
    /// The project has been set up (played, test edits made) once it finished loading.
    prepared: bool,
}

pub fn run(project: Option<PathBuf>) -> Result<(), String> {
    // Developer screenshots skip the splash unless they ask for it.
    let splash_enabled = std::env::var_os("KVNG_SCREENSHOT").is_none() || std::env::var_os("KVNG_SCREENSHOT_SPLASH").is_some();
    let mut viewport = egui::ViewportBuilder::default().with_title("KVNG Studio").with_drag_and_drop(true);
    // Dock/window icon: the splash logo (the bundled app also has it as its .icns).
    if let Some(img) = splash::decode_logo_image() {
        let rgba = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        viewport = viewport.with_icon(std::sync::Arc::new(egui::IconData { rgba, width: img.size[0] as u32, height: img.size[1] as u32 }));
    }
    // Like FL Studio, the app first appears as just the logo floating on the desktop: a small
    // borderless, transparent window that becomes the main window when the splash ends.
    let viewport = if splash_enabled {
        viewport
            .with_inner_size([splash::SPLASH_WINDOW, splash::SPLASH_WINDOW])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
    } else {
        viewport.with_inner_size([1440.0, 900.0]).with_min_inner_size([960.0, 600.0])
    };
    // eframe restores the last saved window size/position over ours; `persist_window: false` stops
    // new saves, and the hook forces the small splash size over any size saved by older sessions.
    let window_builder: Option<eframe::WindowBuilderHook> =
        splash_enabled.then(|| Box::new(|b: egui::ViewportBuilder| b.with_inner_size([splash::SPLASH_WINDOW, splash::SPLASH_WINDOW])) as eframe::WindowBuilderHook);
    let options = eframe::NativeOptions { viewport, centered: true, persist_window: false, window_builder, ..Default::default() };
    eframe::run_native(
        "KVNG Studio",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let layout = WorkspaceLayout::load();
            let mut app = App {
                state: State::Welcome { error: None },
                prefs: prefs::Prefs::load(),
                settings: Default::default(),
                audio_info: None,
                last_autosave: Instant::now(),
                autosave_job: None,
                recoveries: recovery::leftovers(),
                channel_loads: Vec::new(),
                chain_load: None,
                bounce: None,
                help_window: None,
                windows: layout.windows,
                dock_tab: layout.dock_tab,
                rack_floating: layout.rack_floating,
                piano_floating: layout.piano_floating,
                saved_layout: layout,
                playlist: Default::default(),
                piano: Default::default(),
                mixer: mixer::MixerState { detached: layout.mixer_detached, show_all: layout.show_all_inserts, wide_strips: layout.wide_mixer_strips, ..Default::default() },
                notice: None,
                show_warnings: false,
                last_frame: Instant::now(),
                started: Instant::now(),
                screenshot: std::env::var_os("KVNG_SCREENSHOT").map(|p| Screenshot {
                    path: PathBuf::from(p),
                    frames: 0,
                    requested: false,
                    play: std::env::var_os("KVNG_SCREENSHOT_PLAY").is_some(),
                    prepared: false,
                }),
                confirm: None,
                quitting: false,
                export: None,
                picker: Default::default(),
                browser: Default::default(),
                plugin_load: None,
                rename: None,
                typing_keyboard: false,
                channel_window: None,
                audio_editor: None,
                pending_audio_editor: false,
                automation_window: None,
                effect_window: None,
                strip_window: None,
                export_setup: None,
                export_options: ExportOptions::default(),
                settings_open: false,
                param_picker: None,
                splash: splash::Splash::new(splash_enabled),
            };
            if let Ok(list) = std::env::var("KVNG_SCREENSHOT_WINDOWS") {
                app.rack_floating = list.contains("rack-float");
                app.piano_floating = list.contains("piano-float");
                app.mixer.detached = list.contains("mixer-detached");
                app.mixer.wide_strips = list.contains("mixer-wide");
                app.windows = Windows {
                    browser: list.contains("browser"),
                    playlist: list.contains("playlist"),
                    rack: list.contains("rack"),
                    piano: list.contains("piano"),
                    mixer: list.contains("mixer"),
                    inspector: list.contains("inspector"),
                };
                if list.contains("picker") {
                    app.picker.show_for(picker::Target::Browse);
                }
                if let Ok(tab) = std::env::var("KVNG_SCREENSHOT_SETTINGS") {
                    app.settings.show(match tab.as_str() {
                        "midi" => settings_ui::Tab::Midi,
                        "plugins" => settings_ui::Tab::Plugins,
                        "recording" => settings_ui::Tab::Recording,
                        "files" => settings_ui::Tab::Files,
                        "general" => settings_ui::Tab::General,
                        _ => settings_ui::Tab::Audio,
                    });
                }
                if list.contains("about") {
                    app.help_window = Some(HelpWindow::About);
                } else if list.contains("shortcuts") {
                    app.help_window = Some(HelpWindow::Shortcuts);
                }
                if list.contains("strip") {
                    app.strip_window = Some(1);
                }
                if list.contains("audio") {
                    app.pending_audio_editor = true;
                }
            }
            cc.egui_ctx.set_zoom_factor(app.prefs.ui_scale);
            app.export_options.bits = app.prefs.export_bits;
            let screenshot_mode = std::env::var_os("KVNG_SCREENSHOT").is_some();
            let project = project.or_else(|| {
                (app.prefs.reopen_last && app.recoveries.is_empty() && !screenshot_mode).then(|| app.prefs.last_project.clone()).flatten().filter(|p| p.is_file())
            });
            if let Some(path) = project {
                app.load(Source::from_path(path), &cc.egui_ctx);
            } else if std::env::var_os("KVNG_SCREENSHOT_NEW").is_some() {
                app.load(Source::New, &cc.egui_ctx);
                if std::env::var("KVNG_SCREENSHOT_WINDOWS").is_ok_and(|w| w.contains("synth")) {
                    app.channel_window = Some(4);
                }
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

impl App {
    /// Opens `source`, asking first if the current project has unsaved changes.
    fn request(&mut self, action: Pending, ctx: &egui::Context) {
        if let State::Ready(s) = &self.state
            && s.modified
        {
            self.confirm = Some(action);
            return;
        }
        self.perform(action, ctx);
    }

    fn perform(&mut self, action: Pending, ctx: &egui::Context) {
        self.finish_autosave();
        // Leaving the project on purpose: its crash-recovery copy isn't needed any more.
        if let State::Ready(s) = &self.state {
            recovery::discard(s.path.as_deref(), s.session_id);
        }
        match action {
            Pending::Open(source) => self.load(source, ctx),
            Pending::Quit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn load(&mut self, source: Source, ctx: &egui::Context) {
        self.finish_autosave();
        // Stop the current project's audio (and its plugin helpers) before loading the next one.
        self.state = State::Welcome { error: None };
        let output = match audio::Output::open(&self.prefs) {
            Ok((o, warning)) => {
                if let Some(w) = warning {
                    self.notify(w);
                }
                o
            }
            Err(e) => {
                self.state = State::Welcome { error: Some(e) };
                return;
            }
        };
        let sample_rate = output.sample_rate();
        let (tx, rx) = mpsc::channel();
        let thread_source = source.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let mut progress = move |msg: &str| {
                let _ = progress_tx.send(LoadMsg::Progress(msg.to_string()));
                progress_ctx.request_repaint();
            };
            let result = match &thread_source {
                Source::Flp(path) => flp::read(path)
                    .map_err(|e| format!("Couldn't read {}: {e}", path.display()))
                    .map(|project| Box::new(engine::load::from_project(None, &project, path, sample_rate, &mut progress))),
                Source::Kvng(path) => engine::song::Song::read(path).map(|song| Box::new(engine::load::build(None, song, path, sample_rate, &mut progress))),
                Source::New => {
                    let song = engine::song::Song::empty("Untitled");
                    Ok(Box::new(engine::load::build(None, song, Path::new("Untitled.kvng"), sample_rate, &mut progress)))
                }
                Source::Memory { song, path, .. } => {
                    let origin = path.clone().unwrap_or_else(|| PathBuf::from("Untitled.kvng"));
                    Ok(Box::new(engine::load::build(None, (**song).clone(), &origin, sample_rate, &mut progress)))
                }
            };
            let _ = tx.send(LoadMsg::Done(result));
            ctx.request_repaint();
        });
        self.state = State::Loading { source, status: "Reading project".into(), rx, output, started: Instant::now() };
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.notice = Some((msg.into(), Instant::now()));
    }

    fn poll_loading(&mut self) {
        let State::Loading { rx, status, output, source, .. } = &mut self.state else { return };
        let mut done = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                LoadMsg::Progress(p) => *status = p,
                LoadMsg::Done(r) => done = Some(r),
            }
        }
        let Some(result) = done else { return };
        let (path, modified) = match source {
            Source::Kvng(p) => (Some(p.clone()), false),
            Source::Memory { path, modified, .. } => (path.clone(), *modified),
            _ => (None, false),
        };
        self.audio_info = Some(settings_ui::AudioInfo { device: output.device_name.clone(), sample_rate: output.sample_rate(), buffer: output.buffer_frames(), controls: None });
        let next = match result.and_then(|loaded| Session::new(*loaded, output, path)) {
            Ok(mut session) => {
                session.modified |= modified;
                session.apply_prefs(&self.prefs);
                if let Some(info) = &mut self.audio_info {
                    info.controls = Some(session.controls.clone());
                    session.buffer_frames = info.buffer;
                }
                if let Some(p) = &session.path
                    && self.prefs.last_project.as_ref() != Some(p)
                {
                    self.prefs.last_project = Some(p.clone());
                    let _ = self.prefs.save();
                }
                self.last_autosave = Instant::now();
                self.playlist = Default::default();
                self.piano = Default::default();
                self.audio_editor = None;
                self.channel_window = None;
                self.show_warnings = !session.view.warnings.is_empty();
                State::Ready(Box::new(session))
            }
            Err(e) => State::Welcome { error: Some(e) },
        };
        self.state = next;
    }

    fn pick_file(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Projects", &[engine::song::EXTENSION, "flp"])
            .add_filter("KVNG Studio project", &[engine::song::EXTENSION])
            .add_filter("FL Studio project", &["flp"])
            .set_title("Open project")
            .pick_file()
        {
            self.request(Pending::Open(Source::from_path(path)), ctx);
        }
    }

    /// Saves to the current `.kvng` path, or asks for one. Returns false if cancelled or failed.
    fn save(&mut self, save_as: bool) -> bool {
        self.finish_autosave();
        let State::Ready(s) = &mut self.state else { return false };
        let path = match (&s.path, save_as) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(mut p) = rfd::FileDialog::new()
                    .add_filter("KVNG Studio project", &[engine::song::EXTENSION])
                    .set_file_name(format!("{}.{}", s.view.name, engine::song::EXTENSION))
                    .set_title("Save project")
                    .save_file()
                else {
                    return false;
                };
                if p.extension().is_none() {
                    p.set_extension(engine::song::EXTENSION);
                }
                p
            }
        };
        let backup_error = recovery::backup(&path, self.prefs.backups).err();
        let old_path = s.path.clone();
        let result = s.save(path.clone());
        if result.is_ok() {
            recovery::discard(old_path.as_deref(), s.session_id);
            recovery::discard(Some(&path), s.session_id);
            self.prefs.last_project = Some(path.clone());
            let _ = self.prefs.save();
        }
        if let Some(e) = backup_error {
            self.notify(format!("Couldn't back up the previous version: {e}"));
        }
        match result {
            Ok(problems) if problems.is_empty() => {
                self.notify(format!("Saved {}", path.display()));
                true
            }
            Ok(problems) => {
                self.notify(format!("Saved, but: {}", problems.join("; ")));
                true
            }
            Err(e) => {
                self.notify(format!("Couldn't save: {e}"));
                false
            }
        }
    }

    /// Renders the current song in the background with `opts` (asks where to save).
    fn start_export(&mut self, ctx: &egui::Context, opts: ExportOptions) {
        let State::Ready(s) = &self.state else { return };
        let out = if opts.stems {
            rfd::FileDialog::new().set_title("Choose a folder for the stems").pick_folder()
        } else {
            let (kind, ext) = if opts.flac { ("FLAC audio", "flac") } else { ("WAV audio", "wav") };
            rfd::FileDialog::new().add_filter(kind, &[ext]).set_file_name(format!("{}.{ext}", s.view.name)).set_title("Export").save_file().map(|mut p| {
                p.set_extension(ext);
                p
            })
        };
        let Some(out) = out else { return };
        let (song, _) = s.to_song();
        let origin = s.path.clone().unwrap_or_else(|| PathBuf::from(format!("{}.kvng", s.view.name)));
        let used: Vec<(usize, String)> = s.view.inserts.iter().filter(|i| i.used || i.index == 0).map(|i| (i.index, i.name.clone())).collect();
        let name = s.view.name.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send = |m: ExportMsg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let result = export_audio(song, &origin, &out, &name, &used, opts, &mut |status, fraction| send(ExportMsg::Progress(status.to_string(), fraction)));
            send(ExportMsg::Done(result.map(|report| (out.display().to_string(), report))));
        });
        self.export = Some(Export { rx, status: "Preparing".into(), fraction: 0.0, result: None });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (save, save_as, export, new) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                i.key_pressed(Key::S) && cmd && !i.modifiers.shift,
                i.key_pressed(Key::S) && cmd && i.modifiers.shift,
                i.key_pressed(Key::R) && cmd,
                i.key_pressed(Key::N) && cmd,
            )
        });
        if save || save_as {
            self.save(save_as);
            return;
        }
        if export {
            self.export_setup = Some(self.export_options);
            return;
        }
        if new {
            self.request(Pending::Open(Source::New), ctx);
            return;
        }
        let (space, f5, f6, f7, f9, l, z, shift_z, open, home) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::F5),
                i.key_pressed(Key::F6),
                i.key_pressed(Key::F7),
                i.key_pressed(Key::F9),
                i.key_pressed(Key::L) && !cmd,
                i.key_pressed(Key::Z) && cmd && !i.modifiers.shift,
                i.key_pressed(Key::Z) && cmd && i.modifiers.shift,
                i.key_pressed(Key::O) && cmd,
                i.key_pressed(Key::Home),
            )
        });
        if open {
            self.pick_file(ctx);
            return;
        }
        if f5 {
            self.windows.playlist = !self.windows.playlist;
        }
        if f6 {
            self.windows.rack = !self.windows.rack;
            if self.windows.rack { self.dock_tab = DockTab::Rack; }
        }
        if f7 {
            self.windows.piano = !self.windows.piano;
            if self.windows.piano { self.dock_tab = DockTab::Piano; }
        }
        if f9 {
            self.windows.mixer = !self.windows.mixer;
        }
        if ctx.input(|i| i.key_pressed(Key::F8) && i.modifiers.alt) {
            self.windows.browser = !self.windows.browser;
        } else if ctx.input(|i| i.key_pressed(Key::F8)) {
            if self.picker.open {
                self.picker.open = false;
            } else {
                self.picker.show_for(picker::Target::Browse);
            }
        }
        let State::Ready(s) = &mut self.state else { return };
        if typing {
            return;
        }
        if self.typing_keyboard {
            // Note keys play instead of acting as shortcuts.
            let events: Vec<(Key, bool)> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, pressed, repeat: false, modifiers, .. } if !modifiers.command => Some((*key, *pressed)),
                        _ => None,
                    })
                    .collect()
            });
            for (key, pressed) in events {
                if let Some(note) = midi::typing_key(key) {
                    s.live.note(note, if pressed { 100 } else { 0 });
                }
            }
            if space {
                s.toggle_play();
            }
            return;
        }
        if s.learning.is_some() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            s.learning = None;
        }
        if ctx.input(|i| i.key_pressed(Key::R) && !i.modifiers.command) {
            s.recording = !s.recording;
        }
        if space {
            s.toggle_play();
        }
        if home {
            s.seek_ticks(0.0);
        }
        if l {
            s.set_mode(if s.mode == Mode::Pattern { Mode::Song } else { Mode::Pattern });
        }
        if z {
            s.undo();
        }
        if shift_z {
            s.redo();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        self.poll_loading();
        self.poll_export();
        self.shortcuts(&ctx);

        // Closing the window with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting
            && matches!(&self.state, State::Ready(s) if !s.modified)
        {
            self.finish_autosave();
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting
            && let State::Ready(s) = &self.state
        {
            if s.modified {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm = Some(Pending::Quit);
            } else {
                recovery::discard(s.path.as_deref(), s.session_id);
            }
        }
        self.autosave_tick();
        self.settings_window(&ctx);
        self.poll_channel_loads(&ctx);

        // Dropping a project file on the window opens it.
        let dropped: Option<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| {
                p.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp") || e.eq_ignore_ascii_case(engine::song::EXTENSION))
            })
        });
        if let Some(path) = dropped {
            self.request(Pending::Open(Source::from_path(path)), &ctx);
        }
        // Dropping a MIDI file imports it into the open project.
        let dropped_midi: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))));
        if let (Some(path), State::Ready(s)) = (dropped_midi, &mut self.state) {
            let msg = s.import_midi(&path).unwrap_or_else(|e| format!("Couldn't import: {e}"));
            self.notify(msg);
        }

        // The startup splash: the window is borderless and transparent, and shows only the logo.
        if self.splash.is_window() {
            splash::show_splash(&ctx, &mut self.splash);
            self.developer_screenshot(&ctx);
            return;
        }

        match &mut self.state {
            State::Welcome { error } => {
                let error = error.clone();
                if std::env::var_os("KVNG_SCREENSHOT_WELCOME").is_some() {
                    self.developer_screenshot(&ctx);
                }
                match welcome(ui, error.as_deref(), &self.recoveries) {
                    Some(WelcomeAction::Open) => self.pick_file(&ctx),
                    Some(WelcomeAction::New) => self.load(Source::New, &ctx),
                    Some(WelcomeAction::Recover(i)) => {
                        let info = self.recoveries.remove(i);
                        match engine::song::Song::read(&info.file) {
                            Ok(song) => {
                                recovery::forget(&info);
                                self.load(Source::Memory { song: Box::new(song), path: info.original.clone(), modified: true }, &ctx);
                            }
                            Err(e) => self.state = State::Welcome { error: Some(format!("Couldn't recover {}: {e}", info.name)) },
                        }
                    }
                    Some(WelcomeAction::Forget(i)) => {
                        let info = self.recoveries.remove(i);
                        recovery::forget(&info);
                    }
                    None => {}
                }
            }
            State::Loading { source, status, started, .. } => {
                splash::draw_project_loading(ui, &source.label(), status, started.elapsed().as_secs_f32());
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            State::Ready(_) => {
                self.workspace(ui, dt);
                self.dialogs(&ctx);
                // Meters and the playhead need continuous redraws while audio runs.
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        self.developer_screenshot(&ctx);
        if self.screenshot.is_none() {
            let layout = WorkspaceLayout {
                windows: self.windows,
                dock_tab: self.dock_tab,
                rack_floating: self.rack_floating,
                piano_floating: self.piano_floating,
                mixer_detached: self.mixer.detached,
                show_all_inserts: self.mixer.show_all,
                wide_mixer_strips: self.mixer.wide_strips,
            };
            if layout != self.saved_layout {
                layout.save();
                self.saved_layout = layout;
            }
        }
    }

    /// Fully transparent while the splash plays (only the logo shows on the desktop), opaque after.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.splash.is_window() { [0.0; 4] } else { egui::Rgba::from(theme::BG).to_array() }
    }
}

impl App {
    fn workspace(&mut self, ui: &mut Ui, dt: f32) {
        let now_secs = self.started.elapsed().as_secs_f64();
        let ctx = ui.ctx().clone();
        let mut menu_action = None;
        {
            let State::Ready(s) = &mut self.state else { return };
            s.update_meters(now_secs, dt);
            let title = format!("{}{} — KVNG Studio", s.view.name, if s.modified { " •" } else { "" });
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

            egui::Panel::top("menu")
                .exact_size(24.0)
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(8, 2)))
                .show(ui, |ui| {
                    egui::MenuBar::new().ui(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            for (label, key, action) in [
                                ("New", "⌘N", MenuAction::New),
                                ("Open…", "⌘O", MenuAction::Open),
                                ("Save", "⌘S", MenuAction::Save),
                                ("Save As…", "⇧⌘S", MenuAction::SaveAs),
                                ("Export WAV…", "⌘R", MenuAction::Export),
                                ("Project settings…", "", MenuAction::Settings),
                                ("Settings…", "⌘,", MenuAction::Preferences),
                                ("Import MIDI file…", "", MenuAction::ImportMidi),
                                ("Export pattern as MIDI…", "", MenuAction::ExportMidi),
                                ("Collect samples into project folder", "", MenuAction::CollectSamples),
                                ("Find missing samples…", "", MenuAction::FindMissing),
                            ] {
                                if ui.add(egui::Button::new(label).shortcut_text(key)).clicked() {
                                    menu_action = Some(action);
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Edit", |ui| {
                            if ui.add_enabled(s.can_undo(), egui::Button::new("Undo").shortcut_text("⌘Z")).clicked() {
                                s.undo();
                                ui.close();
                            }
                            if ui.add(egui::Button::new("Redo").shortcut_text("⇧⌘Z")).clicked() {
                                s.redo();
                                ui.close();
                            }
                        });
                        ui.menu_button("Plugins", |ui| {
                            if ui.add(egui::Button::new("Plugin picker…").shortcut_text("F8")).clicked() {
                                menu_action = Some(MenuAction::Picker);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Find new plugins").on_hover_text("Scan for plugins installed or updated since the last scan").clicked() {
                                menu_action = Some(MenuAction::Rescan(false));
                                ui.close();
                            }
                            if ui.button("Rescan all plugins").clicked() {
                                menu_action = Some(MenuAction::Rescan(true));
                                ui.close();
                            }
                            if ui.button("Plugin folders…").clicked() {
                                menu_action = Some(MenuAction::PluginSettings);
                                ui.close();
                            }
                        });
                        ui.menu_button("View", |ui| {
                            for (open, label, key) in [
                                (&mut self.windows.browser, "Browser", "⌥F8"),
                                (&mut self.windows.playlist, "Playlist", "F5"),
                                (&mut self.windows.rack, "Channel rack", "F6"),
                                (&mut self.windows.piano, "Piano roll", "F7"),
                                (&mut self.windows.mixer, "Mixer", "F9"),
                                (&mut self.windows.inspector, "Inspector", "⌘I"),
                            ] {
                                if ui.add(egui::Button::selectable(*open, label).shortcut_text(key)).clicked() {
                                    *open = !*open;
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Help", |ui| {
                            if ui.button("Keyboard shortcuts").clicked() {
                                menu_action = Some(MenuAction::Shortcuts);
                                ui.close();
                            }
                            if ui.button("About KVNG Studio").clicked() {
                                menu_action = Some(MenuAction::About);
                                ui.close();
                            }
                        });
                    });
                });
        }
        match menu_action {
            Some(MenuAction::New) => self.request(Pending::Open(Source::New), &ctx),
            Some(MenuAction::Open) => self.pick_file(&ctx),
            Some(MenuAction::Save) => {
                self.save(false);
            }
            Some(MenuAction::SaveAs) => {
                self.save(true);
            }
            Some(MenuAction::Export) => self.export_setup = Some(self.export_options),
            Some(MenuAction::Picker) => self.picker.show_for(picker::Target::Browse),
            Some(MenuAction::Settings) => self.settings_open = true,
            Some(MenuAction::Preferences) => self.settings.show(settings_ui::Tab::Audio),
            Some(MenuAction::Shortcuts) => self.help_window = Some(HelpWindow::Shortcuts),
            Some(MenuAction::About) => self.help_window = Some(HelpWindow::About),
            Some(MenuAction::ImportMidi) => {
                if let State::Ready(s) = &mut self.state
                    && let Some(path) = rfd::FileDialog::new().add_filter("MIDI file", &["mid", "midi"]).set_title("Import MIDI file").pick_file()
                {
                    let msg = s.import_midi(&path).unwrap_or_else(|e| format!("Couldn't import: {e}"));
                    self.notify(msg);
                }
            }
            Some(MenuAction::ExportMidi) => {
                if let State::Ready(s) = &mut self.state {
                    let name = s.view.patterns.get(s.pattern).map_or("Pattern".to_string(), |p| p.name.clone());
                    if let Some(mut path) = rfd::FileDialog::new().add_filter("MIDI file", &["mid"]).set_file_name(format!("{name}.mid")).set_title("Export pattern as MIDI").save_file() {
                        if path.extension().is_none() {
                            path.set_extension("mid");
                        }
                        let bytes = s.export_pattern_midi();
                        let msg = match std::fs::write(&path, bytes) {
                            Ok(()) => format!("Exported {}", path.display()),
                            Err(e) => format!("Couldn't export: {e}"),
                        };
                        self.notify(msg);
                    }
                }
            }
            Some(MenuAction::CollectSamples) => {
                if let State::Ready(s) = &mut self.state {
                    let msg = match s.collect_samples() {
                        Ok((n, problems)) if problems.is_empty() => format!("Collected samples: {n} copied into the project's Samples folder. Save to keep the new paths."),
                        Ok((n, problems)) => format!("{n} copied; {}", problems.join("; ")),
                        Err(e) => e,
                    };
                    self.notify(msg);
                }
            }
            Some(MenuAction::FindMissing) => {
                if let State::Ready(s) = &mut self.state {
                    let missing = s.missing_samples();
                    if missing.is_empty() {
                        self.notify("No samples are missing");
                    } else if let Some(folder) = rfd::FileDialog::new().set_title(format!("Find {} missing sample(s) in…", missing.len())).pick_folder() {
                        let (n, still) = s.relink_samples(&folder);
                        let msg = if still.is_empty() { format!("Relinked {n} sample(s)") } else { format!("Relinked {n}; still missing: {}", still.join(", ")) };
                        self.notify(msg);
                    }
                }
            }
            Some(MenuAction::PluginSettings) => self.settings.show(settings_ui::Tab::Plugins),
            Some(MenuAction::Rescan(full)) => {
                self.picker.start_scan(full, &ctx);
                self.settings.show(settings_ui::Tab::Plugins);
            }
            None => {}
        }
        let State::Ready(s) = &mut self.state else { return };

        egui::Panel::top("transport")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(theme::PANEL_2).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, theme::LINE)))
            .show(ui, |ui| transport::bar(ui, s, &mut self.windows, &mut self.typing_keyboard));

        let mut notice = None;
        let mut edit_audio: Option<usize> = None;
        let mut pending_plugin: Option<(bridge::scan::PluginInfo, picker::Target)> = None;
        let mut toggle_warnings = false;
        egui::Panel::bottom("status")
            .exact_size(24.0)
            .frame(egui::Frame::new().fill(theme::PANEL_2).inner_margin(egui::Margin::symmetric(12, 3)).stroke(Stroke::new(1.0, theme::LINE)))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                theme::draw_gunmetal_surface(ui.painter(), rect, theme::RADIUS_ZERO, theme::PANEL_2);
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new(&s.view.name).color(theme::TEXT).small());
                    ui.label(egui::RichText::new(format!("{:.0} Hz · {} · {} BPM", s.view.sample_rate, s.device_name, s.view.tempo)).color(theme::TEXT_FAINT).small());
                    let midi = if s.live.port_names.is_empty() { "No MIDI input".to_string() } else { format!("MIDI: {}", s.live.port_names.join(", ")) };
                    ui.label(egui::RichText::new(midi).color(theme::TEXT_FAINT).small());
                    if s.recording {
                        ui.label(egui::RichText::new("● REC").color(theme::ERROR).small());
                    }
                    if let Some(t) = s.learning {
                        ui.label(egui::RichText::new(format!("Move a MIDI control to link it to {} (Esc cancels)", t.label(&s.view))).color(theme::ACCENT_HOT).small());
                    }
                    if let Some((secs, peak)) = s.recording_status() {
                        ui.label(egui::RichText::new(format!("● Recording audio {:.1}s  {:.0} dB", secs, widgets::to_db(peak))).color(theme::ERROR).small());
                    } else if s.audio_armed {
                        let target = s.record_track.and_then(|i| s.view.tracks.get(i)).map_or("new track", |t| t.name.as_str());
                        ui.label(egui::RichText::new(format!("Audio armed → {target} · press Play to record")).color(theme::WARN).small());
                    }
                    let warnings = s.view.warnings.len();
                    if warnings > 0 {
                        let text = egui::RichText::new(format!("⚠ {warnings} item{} need attention", if warnings == 1 { "" } else { "s" })).color(theme::WARN).small();
                        if ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Show details").clicked() {
                            toggle_warnings = true;
                        }
                    }
                    if let Some((msg, at)) = &self.notice
                        && at.elapsed().as_secs() < 6
                    {
                        ui.label(egui::RichText::new(msg).color(theme::ACCENT_HOT).small());
                    }
                });
            });
        if toggle_warnings {
            self.show_warnings = !self.show_warnings;
        }

        if !self.windows.mixer && self.mixer.fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.mixer.fullscreen = false;
        }
        let showing_mixer = self.windows.mixer && !self.mixer.detached;
        if !showing_mixer && self.windows.browser {
            egui::Panel::left("browser")
                .resizable(true)
                .default_size(250.0)
                .min_size(180.0)
                .max_size(480.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(8)).stroke(Stroke::new(1.0, theme::LINE)))
                .show(ui, |ui| self.browser.ui(ui, s));
        }
        if !showing_mixer && self.windows.inspector {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(260.0)
                .min_size(220.0)
                .max_size(420.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(8)).stroke(Stroke::new(1.0, theme::LINE)))
                .show(ui, |ui| inspector::show(ui, s, &self.playlist.selected));
        }
        for loaded in self.browser.poll() {
            let result = match loaded {
                Ok(browser::Loaded::Preview(sample)) => {
                    s.preview_sample(Some(sample));
                    Ok(())
                }
                Ok(browser::Loaded::AddChannel(path, sample)) => s.add_sample_channel(&path, sample, false).map(|_| ()),
                Ok(browser::Loaded::AddClip { path, sample, track, start }) => s.add_audio_clip(&path, sample, track, start),
                Ok(browser::Loaded::Replace { channel, path, sample }) => s.replace_with_sample(channel, &path, sample),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                notice = Some(e);
            }
        }

        let mixer_action = if showing_mixer {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(8)).stroke(Stroke::new(1.0, theme::LINE)))
                .show(ui, |ui| mixer::show(ui, s, &mut self.mixer)).inner
        } else if self.windows.mixer && self.mixer.detached {
            let viewport_id = egui::ViewportId::from_hash_of("kvng-studio-mixer");
            ctx.show_viewport_immediate(
                viewport_id,
                egui::ViewportBuilder::default()
                    .with_title("KVNG Studio — Mixer")
                    .with_inner_size([1400.0, 850.0])
                    .with_min_inner_size([700.0, 500.0]),
                |ui, _class| {
                    if ui.input(|i| i.viewport().close_requested() || i.key_pressed(Key::F9)) {
                        return Some(mixer::Action::Close);
                    }
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(8)).stroke(Stroke::new(1.0, theme::LINE)))
                        .show(ui, |ui| mixer::show(ui, s, &mut self.mixer)).inner
                },
            )
        } else {
            None
        };
        match mixer_action {
            Some(mixer::Action::Close) => self.windows.mixer = false,
            Some(mixer::Action::ToggleFullscreen) => {
                self.mixer.fullscreen = !self.mixer.fullscreen;
                let viewport_id = if self.mixer.detached { egui::ViewportId::from_hash_of("kvng-studio-mixer") } else { egui::ViewportId::ROOT };
                ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Fullscreen(self.mixer.fullscreen));
            }
            Some(mixer::Action::ToggleDetached) => {
                if self.mixer.fullscreen {
                    let viewport_id = if self.mixer.detached { egui::ViewportId::from_hash_of("kvng-studio-mixer") } else { egui::ViewportId::ROOT };
                    ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Fullscreen(false));
                    self.mixer.fullscreen = false;
                }
                self.mixer.detached = !self.mixer.detached;
            }
            Some(mixer::Action::Message(m)) => notice = Some(m),
            Some(mixer::Action::AddEffect(insert)) => self.picker.show_for(picker::Target::Effect { insert }),
            Some(mixer::Action::LoadChain(insert, name)) => match presets::load_chain(&name) {
                Ok(chain) => {
                    let sr = s.view.sample_rate;
                    let (tx, rx) = mpsc::channel();
                    let repaint = ctx.clone();
                    std::thread::spawn(move || {
                        let items: Vec<ChainItem> = chain
                            .into_iter()
                            .map(|spec| {
                                let plugin = match &spec.source {
                                    engine::song::EffectSource::Plugin(p) => Some(engine::load::load_plugin(None, p, sr)),
                                    _ => None,
                                };
                                (spec, plugin)
                            })
                            .collect();
                        let _ = tx.send(items);
                        repaint.request_repaint();
                    });
                    notice = Some(format!("Loading effect chain “{name}”…"));
                    self.chain_load = Some((insert, name, rx));
                }
                Err(e) => notice = Some(format!("Couldn't read the chain: {e}")),
            },
            Some(mixer::Action::AddTrack) => {
                if s.add_mixer_track().is_none() {
                    notice = Some("All 125 mixer tracks are in use".into());
                }
            }
            Some(mixer::Action::OpenEffect(i, slot)) => self.effect_window = Some((i, slot)),
            Some(mixer::Action::OpenStrip(i)) => self.strip_window = Some(i),
            Some(mixer::Action::AutomateEffect(insert, slot)) => self.param_picker = Some(params::ParamPicker::new(params::Owner::Effect { insert, slot })),
            Some(mixer::Action::RenameInsert(i)) => self.rename = Some((RenameTarget::Insert(i), s.view.inserts[i].name.clone())),
            Some(mixer::Action::RemoveEffect(insert, index)) => {
                if let Err(e) = s.remove_effect(insert, index) {
                    notice = Some(e);
                }
            }
            None => {}
        }

        let mut rack_action = None;
        if !showing_mixer && (self.windows.rack && !self.rack_floating || self.windows.piano && !self.piano_floating) {
            let rack_docked = self.windows.rack && !self.rack_floating;
            let piano_docked = self.windows.piano && !self.piano_floating;
            if !rack_docked { self.dock_tab = DockTab::Piano; }
            if !piano_docked { self.dock_tab = DockTab::Rack; }
            let dock = egui::Panel::bottom(if self.dock_tab == DockTab::Piano { "piano-dock" } else { "rack-dock" })
                .resizable(true)
                .default_size(if self.dock_tab == DockTab::Piano { 430.0 } else { 280.0 })
                .min_size(190.0)
                .max_size(650.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(8)).stroke(Stroke::new(1.0, theme::LINE)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("EDITORS").font(theme::ui_font(11.0)).color(theme::TEXT_DIM));
                        if rack_docked && widgets::toggle(ui, self.dock_tab == DockTab::Rack, "Channel rack", vec2(112.0, 23.0)).clicked() { self.dock_tab = DockTab::Rack; }
                        if piano_docked && widgets::toggle(ui, self.dock_tab == DockTab::Piano, "Piano roll", vec2(100.0, 23.0)).clicked() { self.dock_tab = DockTab::Piano; }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Float ↗").on_hover_text("Move this editor to its own window").clicked() {
                                match self.dock_tab { DockTab::Rack => self.rack_floating = true, DockTab::Piano => self.piano_floating = true }
                            }
                            if ui.small_button("✕").on_hover_text("Close this editor").clicked() {
                                match self.dock_tab { DockTab::Rack => self.windows.rack = false, DockTab::Piano => self.windows.piano = false }
                            }
                        });
                    });
                    ui.separator();
                    match self.dock_tab {
                        DockTab::Rack => rack_action = rack::show(ui, s),
                        DockTab::Piano => piano::show(ui, s, &mut self.piano),
                    }
                });
            if self.dock_tab == DockTab::Rack
                && let Some(file) = dock.response.dnd_release_payload::<browser::DraggedFile>() {
                    self.browser.load(file.0.clone(), s.view.sample_rate, browser::Loaded::AddChannel);
                }
        }

        if !showing_mixer { egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ui, |ui| {
            if self.windows.playlist {
                let outcome = playlist::show(ui, s, &mut self.playlist);
                if let Some(a) = outcome.open_automation {
                    self.automation_window = Some(a);
                } else if let Some(p) = outcome.open_pattern {
                    s.select_pattern(p);
                    self.windows.piano = true;
                    self.dock_tab = DockTab::Piano;
                }
                if let Some((path, track, start)) = outcome.dropped {
                    self.browser.load(path, s.view.sample_rate, move |path, sample| browser::Loaded::AddClip { path, sample, track, start });
                }
                if let Some(m) = outcome.message {
                    self.notice = Some((m, Instant::now()));
                }
            } else {
                ui.centered_and_justified(|ui| ui.label(egui::RichText::new("Playlist hidden (F5)").color(theme::TEXT_FAINT)));
            }
        }); }

        let mut rack_open = self.windows.rack;
        let rack_window = if !showing_mixer && self.rack_floating && self.windows.rack { egui::Window::new("Channel rack")
            .open(&mut rack_open)
            .default_pos(pos2(260.0, 150.0))
            .default_size(vec2(780.0, 320.0))
            .resizable(true)
            .collapsible(true)
            .show(&ctx, |ui| {
                if ui.small_button("Dock ↓").clicked() { self.rack_floating = false; }
                rack_action = rack::show(ui, s);
            }) } else { None };
        self.windows.rack = rack_open;
        if let Some(r) = &rack_window
            && let Some(file) = r.response.dnd_release_payload::<browser::DraggedFile>()
        {
            self.browser.load(file.0.clone(), s.view.sample_rate, browser::Loaded::AddChannel);
        }
        match rack_action {
            Some(rack::Action::OpenPiano(c)) => {
                s.channel = c;
                self.windows.piano = true;
                self.dock_tab = DockTab::Piano;
            }
            Some(rack::Action::OpenEditor(c)) => {
                s.channel = c;
                if let Some(h) = &s.view.channels[c].editor {
                    if let Err(e) = h.open() {
                        notice = Some(format!("{}: {e}", s.view.channels[c].name));
                    }
                } else {
                    self.channel_window = Some(c);
                }
            }
            Some(rack::Action::AddInstrument) => self.picker.show_for(picker::Target::Instrument),
            Some(rack::Action::ReplaceInstrument(c)) => self.picker.show_for(picker::Target::Replace { channel: c }),
            Some(rack::Action::Bounce(c)) => {
                if self.bounce.is_some() {
                    notice = Some("A render is already running".into());
                } else {
                    let (mut song, _) = s.to_song();
                    for (i, ch) in song.channels.iter_mut().enumerate() {
                        ch.muted = i != c;
                    }
                    // Not on the playlist yet (pattern mode): render the current pattern instead.
                    let on_playlist = s.view.clips.iter().any(|clip| clip.pattern.and_then(|p| s.view.patterns.get(p)).is_some_and(|p| p.notes.iter().any(|n| n.channel == c)));
                    if !on_playlist && let Some(p) = s.view.patterns.get(s.pattern) {
                        song.clips = vec![engine::view::ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track: 0, start: 0, length: p.length.max(1), label: p.name.clone(), kind: engine::view::ClipKind::Pattern, pattern: Some(s.pattern), channel: None, source_offset: 0.0 }];
                        for t in &mut song.tracks {
                            t.muted = false;
                        }
                    }
                    let name = s.view.channels[c].name.clone();
                    let dir = s.path.as_ref().and_then(|p| p.parent().map(|d| d.join("Rendered"))).unwrap_or_else(|| crate::data_dir().join("Rendered"));
                    let file_name: String = name.chars().map(|ch| if matches!(ch, '/' | ':' | '\\') { '_' } else { ch }).collect();
                    let out = (1..).map(|i| dir.join(format!("{file_name} render {i}.wav"))).find(|p| !p.exists()).unwrap();
                    let origin = s.path.clone().unwrap_or_else(|| PathBuf::from("Untitled.kvng"));
                    let opts = ExportOptions { bits: 32, sample_rate: s.view.sample_rate.round() as u32, tail: 2.0, normalize: false, stems: false, ..Default::default() };
                    let (tx, rx) = mpsc::channel();
                    let (thread_out, repaint) = (out.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let result = std::fs::create_dir_all(thread_out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string()).and_then(|_| export_audio(song, &origin, &thread_out, &name, &[], opts, &mut |_, _| {}));
                        let _ = tx.send(result.map(|_| ()));
                        repaint.request_repaint();
                    });
                    notice = Some(format!("Rendering {}…", s.view.channels[c].name));
                    self.bounce = Some((c, out, rx));
                }
            }
            Some(rack::Action::RouteToFreeInsert(c)) => {
                let before = s.controls.channels.get(c).map_or(0, |x| x.route());
                let i = s.route_to_free_insert(c);
                notice = Some(if i != before { format!("{} → mixer track {i}", s.view.channels[c].name) } else { "No free mixer track".into() });
            }
            Some(rack::Action::ReplaceSample(c, path)) => {
                self.browser.load(path, s.view.sample_rate, move |path, sample| browser::Loaded::Replace { channel: c, path, sample });
            }
            Some(rack::Action::ReplaceBuiltin(c, spec)) => match s.replace_with_builtin(c, spec) {
                Ok(()) => notice = Some(format!("Replaced with {}", s.view.channels[c].name)),
                Err(e) => notice = Some(e),
            },
            Some(rack::Action::ReplacePlugin(c, info)) => pending_plugin = Some((info, picker::Target::Replace { channel: c })),
            Some(rack::Action::Automated(m)) => notice = Some(m),
            Some(rack::Action::AutomateParam(c)) => self.param_picker = Some(params::ParamPicker::new(params::Owner::Channel(c))),
            Some(rack::Action::ChannelSettings(c)) => {
                s.channel = c;
                self.channel_window = Some(c);
            }
            Some(rack::Action::EditAudio(c)) => edit_audio = Some(c),
            Some(rack::Action::Rename(c)) => self.rename = Some((RenameTarget::Channel(c), s.view.channels[c].name.clone())),
            Some(rack::Action::Delete(c)) => {
                let name = s.view.channels[c].name.clone();
                match s.delete_channel(c) {
                    Ok(()) => notice = Some(format!("Deleted {name}")),
                    Err(e) => notice = Some(e),
                }
            }
            None => {}
        }

        // Renaming a channel.
        let mut finish_rename = None;
        if let Some((c, text)) = &mut self.rename {
            let mut open = true;
            let title = if matches!(c, RenameTarget::Insert(_)) { "Rename insert" } else { "Rename channel" };
            egui::Window::new(title).id(egui::Id::new("rename-channel")).collapsible(false).resizable(false).open(&mut open).show(&ctx, |ui| {
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(260.0));
                r.request_focus();
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                        finish_rename = Some(Some((*c, text.clone())));
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        finish_rename = Some(None);
                    }
                });
            });
            if !open {
                finish_rename = Some(None);
            }
        }
        if let Some(result) = finish_rename {
            match result {
                Some((RenameTarget::Channel(c), name)) => s.rename_channel(c, name),
                Some((RenameTarget::Insert(i), name)) => s.rename_insert(i, &name),
                None => {}
            }
            self.rename = None;
        }

        // Plugin parameter picker.
        if let Some(picker) = &mut self.param_picker {
            let mut open = true;
            let mut done = false;
            egui::Window::new("Automate parameter").id(egui::Id::new("param-picker")).open(&mut open).default_width(380.0).collapsible(false).show(&ctx, |ui| done = picker.ui(ui, s));
            if done {
                notice = Some("Created an automation clip on the playlist".into());
            }
            if done || !open {
                self.param_picker = None;
            }
        }

        // Project settings.
        if self.settings_open {
            let mut open = true;
            egui::Window::new("Project settings").id(egui::Id::new("project-settings")).open(&mut open).resizable(false).collapsible(false).show(&ctx, |ui| {
                egui::Grid::new("settings-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                    ui.label("Name");
                    let mut name = s.view.name.clone();
                    if ui.text_edit_singleline(&mut name).changed() {
                        s.view.name = name;
                        s.touch();
                    }
                    ui.end_row();
                    ui.label("Tempo");
                    let mut bpm = s.view.tempo;
                    if ui.add(egui::DragValue::new(&mut bpm).range(10.0..=522.0).speed(0.1).fixed_decimals(3).suffix(" BPM")).changed() {
                        s.set_tempo(bpm);
                    }
                    ui.end_row();
                    ui.label("Time signature");
                    ui.horizontal(|ui| {
                        let mut beats = s.view.beats_per_bar;
                        if ui.add(egui::DragValue::new(&mut beats).range(1..=16)).changed() {
                            s.set_beats_per_bar(beats);
                        }
                        ui.label("/ 4");
                    });
                    ui.end_row();
                    ui.label("Swing");
                    let mut swing = s.view.swing * 100.0;
                    if ui.add(egui::Slider::new(&mut swing, 0.0..=100.0).suffix("%")).changed() {
                        s.set_swing(swing / 100.0);
                    }
                    ui.end_row();
                    ui.label("Tempo changes");
                    ui.label(egui::RichText::new(format!("{} (right-click the playlist ruler to add)", s.view.tempo_changes.len())).small().color(theme::TEXT_DIM));
                    ui.end_row();
                });
            });
            self.settings_open = open;
        }

        // Built-in effect settings.
        if let Some((i, slot)) = self.effect_window {
            let mut open = true;
            let title = format!(
                "{} — {}",
                s.view.inserts.get(i).and_then(|x| x.effects.get(slot)).map(|e| e.name.clone()).unwrap_or_default(),
                s.view.inserts.get(i).map(|x| x.name.clone()).unwrap_or_default()
            );
            egui::Window::new(title)
                .id(egui::Id::new("effect-window"))
                .open(&mut open)
                .resizable(false)
                .collapsible(true)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10)).stroke(Stroke::new(1.0, theme::LINE_STRONG)))
                .show(&ctx, |ui| {
                    let rect = ui.max_rect();
                    theme::draw_hardware_faceplate(ui.painter(), rect, theme::RADIUS_SM, theme::PANEL, true);
                    if !effect_ui::show(ui, s, i, slot) {
                        ui.label("This effect has no built-in settings.");
                    }
                });
            if !open {
                self.effect_window = None;
            }
        }

        // Automation clip editor.
        if let Some(a) = self.automation_window {
            let mut open = true;
            let title = format!("Automation — {}", s.view.automations.get(a).map(|x| x.name.as_str()).unwrap_or(""));
            egui::Window::new(title).id(egui::Id::new("automation-window")).open(&mut open).default_width(720.0).collapsible(true).show(&ctx, |ui| automation::show(ui, s, a));
            if !open {
                self.automation_window = None;
            }
        }

        // Built-in instrument window.
        if let Some(c) = self.channel_window {
            let mut open = true;
            let name = s.view.channels.get(c).map(|ch| ch.name.clone()).unwrap_or_default();
            let title = format!("Channel settings — {name}");
            egui::Window::new(title)
                .id(egui::Id::new("synth-window"))
                .open(&mut open)
                .resizable(false)
                .collapsible(true)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10)).stroke(Stroke::new(1.0, theme::LINE_STRONG)))
                .show(&ctx, |ui| {
                    let rect = ui.max_rect();
                    theme::draw_hardware_faceplate(ui.painter(), rect, theme::RADIUS_SM, theme::PANEL, true);
                    if !synth_ui::show(ui, s, c) && !synth_ui::sampler(ui, s, c) && !synth_ui::midi_out(ui, s, c, &mut notice) && s.view.channels.get(c).is_some_and(|ch| ch.editor.is_none()) {
                        ui.label("This channel has no built-in controls.");
                    }
                    synth_ui::plugin_settings(ui, s, c);
                synth_ui::arpeggiator(ui, s, c);
                    if s.has_sample(c) {
                        ui.add_space(6.0);
                        if ui.button("Edit audio…").on_hover_text("Open the sample in the audio editor").clicked() {
                            edit_audio = Some(c);
                        }
                    }
                });
            if !open {
                self.channel_window = None;
            }
        }

        // Channel strip (EQ/compressor) editor.
        if let Some(i) = self.strip_window {
            let mut open = true;
            let name = s.view.inserts.get(i).map(|x| x.name.clone()).unwrap_or_default();
            egui::Window::new(format!("Channel strip — {name}"))
                .id(egui::Id::new("strip-window"))
                .open(&mut open)
                .default_size(vec2(920.0, 700.0))
                .min_size(vec2(700.0, 480.0))
                .max_size(vec2(1150.0, 790.0))
                .collapsible(true)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(10)).stroke(Stroke::new(1.0, theme::LINE_STRONG)))
                .show(&ctx, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        if let Some(m) = strip_ui::editor(ui, s, i) { notice = Some(m); }
                    });
                });
            if !open {
                self.strip_window = None;
            }
        }

        // Audio editor.
        if let Ok(name) = std::env::var("KVNG_SCREENSHOT_FX")
            && self.effect_window.is_none()
            && let Some(fx) = engine::effects::BuiltinEffect::all().into_iter().find(|f| f.name().to_lowercase().contains(&name.to_lowercase()))
            && let Ok(slot) = s.add_builtin_effect(1, fx)
        {
            self.effect_window = Some((1, slot));
        }
        if self.pending_audio_editor {
            self.pending_audio_editor = false;
            edit_audio = (0..s.view.channels.len()).find(|&c| s.has_sample(c));
        }
        if let Some(c) = edit_audio {
            if self.audio_editor.as_ref().is_some_and(|e| e.dirty && e.channel != c) {
                notice = Some("Apply or discard the edits in the audio editor first".into());
            } else {
                match audio_editor::AudioEditor::open(s, c) {
                    Ok(e) => {
                        s.channel = c;
                        self.audio_editor = Some(e);
                    }
                    Err(e) => notice = Some(e),
                }
            }
        }
        if let Some(editor) = self.audio_editor.as_mut() {
            let mut open = true;
            let name = s.view.channels.get(editor.channel).map(|ch| ch.name.clone()).unwrap_or_default();
            let title = format!("Audio editor — {name}{}", if editor.dirty { " •" } else { "" });
            egui::Window::new(title).id(egui::Id::new("audio-editor")).open(&mut open).default_width(820.0).min_width(480.0).collapsible(true).show(&ctx, |ui| {
                if let Some(m) = editor.show(ui, s) {
                    notice = Some(m);
                }
            });
            if !open && editor.dirty {
                editor.confirm_close = true;
            }
            if (!open || editor.confirm_close) && !editor.dirty {
                s.preview_sample(None);
                self.audio_editor = None;
            }
        }

        // Plugin picker: choosing a plugin loads it in the background (built-ins are instant).
        let mut choice = self.picker.ui(&ctx);
        if let Some((fx, target)) = choice.as_mut().and_then(|c| c.builtin_effect.take().map(|fx| (fx, c.target))) {
            let insert = match target {
                picker::Target::Effect { insert } => insert,
                _ => s.controls.channels.get(s.channel).map_or(0, |c| c.route()),
            };
            match s.add_builtin_effect(insert, fx) {
                Ok(slot) => {
                    notice = Some(format!("Added {} to {}", s.view.inserts[insert].effects[slot].name, s.view.inserts[insert].name));
                    self.effect_window = Some((insert, slot));
                }
                Err(e) => notice = Some(e),
            }
            choice = None;
        }
        if let Some((builtin, picker::Target::Replace { channel })) = choice.as_mut().and_then(|c| c.builtin.clone().map(|b| (b, c.target))) {
            match s.replace_with_builtin(channel, builtin) {
                Ok(()) => notice = Some(format!("Replaced with {}", s.view.channels[channel].name)),
                Err(e) => notice = Some(e),
            }
            self.picker.open = false;
            choice = None;
        }
        if let Some(builtin) = choice.as_mut().and_then(|c| c.builtin.take()) {
            match s.add_builtin(builtin) {
                Ok(c) => {
                    notice = Some(format!("Added {}", s.view.channels[c].name));
                    if s.view.channels[c].kind == "KVNG Synth" {
                        self.channel_window = Some(c);
                    }
                }
                Err(e) => notice = Some(e),
            }
            choice = None;
        }
        let chosen = choice.and_then(|c| c.info.map(|i| (i, c.target))).or(pending_plugin.take());
        if let Some(choice) = chosen {
            let (info, chosen_target) = choice;
            if matches!(chosen_target, picker::Target::Replace { .. }) {
                self.picker.open = false;
            }
            let choice = picker::Choice { info: Some(info.clone()), builtin: None, builtin_effect: None, target: chosen_target };
            let info = choice.info.clone().unwrap();
            let spec = picker::plugin_spec(&info);
            let target = match choice.target {
                picker::Target::Browse if info.is_instrument => picker::Target::Instrument,
                picker::Target::Browse => picker::Target::Effect { insert: s.controls.channels.get(s.channel).map_or(0, |c| c.route()) },
                t => t,
            };
            let (tx, rx) = mpsc::channel();
            let (thread_spec, sample_rate, repaint) = (spec.clone(), s.view.sample_rate, ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(engine::load::load_plugin(None, &thread_spec, sample_rate));
                repaint.request_repaint();
            });
            notice = Some(format!("Loading {}…", info.name));
            self.plugin_load = Some(PluginLoad { name: info.name.clone(), target, spec, rx });
        }
        if let Some(load) = &self.plugin_load
            && let Ok(result) = load.rx.try_recv()
        {
            let load = self.plugin_load.take().unwrap();
            let outcome = result.and_then(|plugin| match load.target {
                picker::Target::Effect { insert } => s.add_effect(insert, load.spec, plugin).map(|_| None),
                picker::Target::Replace { channel } => s.replace_with_plugin(channel, load.spec, plugin).map(|_| Some(channel)),
                _ => s.add_instrument(load.spec, plugin).map(Some),
            });
            match outcome {
                Ok(Some(channel)) => {
                    // FL opens a new instrument's window straight away.
                    if let Some(h) = &s.view.channels[channel].editor {
                        let _ = h.open();
                    }
                    notice = Some(format!("Added {}", load.name));
                }
                Ok(None) => notice = Some(format!("Added {}", load.name)),
                Err(e) => notice = Some(format!("{}: {e}", load.name)),
            }
        }
        s.collect_garbage();
        if let Some(m) = s.record_automation(automation::take_touched(&ctx)) {
            notice = Some(m);
        }
        s.tick_preview();
        s.poll_live();
        if let Some(m) = s.poll_audio_recording() {
            notice = Some(m);
        }
        if let Some(m) = s.poll_controllers() {
            notice = Some(m);
        }

        let mut piano_open = self.windows.piano;
        let title = format!("Piano roll — {}", s.view.channels.get(s.channel).map(|c| c.name.as_str()).unwrap_or(""));
        if !showing_mixer && self.piano_floating && self.windows.piano { egui::Window::new(title)
            .id(egui::Id::new("piano-window"))
            .open(&mut piano_open)
            .default_pos(pos2(120.0, 110.0))
            .default_size(vec2(1080.0, 560.0))
            .resizable(true)
            .collapsible(true)
            .show(&ctx, |ui| {
                if ui.small_button("Dock ↓").clicked() { self.piano_floating = false; }
                ui.set_min_size(vec2(600.0, 320.0));
                piano::show(ui, s, &mut self.piano);
            }); }
        self.windows.piano = piano_open;

        if self.show_warnings {
            let mut open = true;
            let screen = ctx.content_rect();
            egui::Window::new("Project notes")
                .id(egui::Id::new("project-notes-window"))
                .open(&mut open)
                .default_width(520.0)
                .pivot(Align2::RIGHT_TOP)
                .default_pos(pos2(screen.max.x - 16.0, 70.0))
                .show(&ctx, |ui| {
                ui.label(egui::RichText::new("These parts of the project don't sound exactly as they did in FL Studio:").color(theme::TEXT_DIM));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for w in &s.view.warnings {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("•").color(theme::WARN));
                            ui.label(w);
                        });
                    }
                });
            });
            self.show_warnings = open;
        }

        s.flush();
        if let Some(n) = notice {
            self.notify(n);
        }
    }
}

#[derive(Clone, Copy)]
enum MenuAction {
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Picker,
    Settings,
    Preferences,
    PluginSettings,
    CollectSamples,
    FindMissing,
    ImportMidi,
    ExportMidi,
    Shortcuts,
    About,
    Rescan(bool),
}

/// One effect of a chain being loaded: its spec, and the loaded plugin (for plugin effects).
type ChainItem = (engine::song::EffectSpec, Option<Result<engine::PluginInstance, String>>);

/// A channel render in progress: (channel, output file, result).
type Bounce = (usize, PathBuf, Receiver<Result<(), String>>);

type ChannelLoad = (usize, String, engine::Source, engine::view::ChannelStatus, Vec<String>);

impl App {
    /// Starts loading channels the session asked for, and installs finished ones.
    fn poll_channel_loads(&mut self, ctx: &egui::Context) {
        let State::Ready(s) = &mut self.state else {
            self.channel_loads.clear();
            return;
        };
        for (channel, spec, dir, sr) in s.take_load_requests() {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            self.notice = Some((format!("Loading {}…", spec.name), Instant::now()));
            std::thread::spawn(move || {
                let (source, status, warnings) = engine::load::load_channel(&spec, dir.as_deref(), sr);
                let _ = tx.send((channel, spec.name.clone(), source, status, warnings));
                ctx.request_repaint();
            });
            self.channel_loads.push(rx);
        }
        let mut finished = Vec::new();
        self.channel_loads.retain(|rx| match rx.try_recv() {
            Ok(done) => {
                finished.push(done);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        if let Some((channel, out, rx)) = &self.bounce
            && let Ok(result) = rx.try_recv()
        {
            let (channel, out) = (*channel, out.clone());
            self.bounce = None;
            let msg = match result.and_then(|_| engine::sample::load_audio(&out, s.view.sample_rate)) {
                Ok(sample) => {
                    // A new track below everything else, from the song's start.
                    let track = s.view.clips.iter().map(|c| c.track + 1).max().unwrap_or(0);
                    while s.view.tracks.len() <= track {
                        s.view.insert_track(s.view.tracks.len());
                    }
                    match s.add_audio_clip(&out, std::sync::Arc::new(sample), track, 0) {
                        Ok(()) => {
                            if let Some(strip) = s.controls.channels.get(channel) {
                                strip.mute.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            format!("Rendered {} to audio (the original channel is muted)", s.view.channels[channel].name)
                        }
                        Err(e) => e,
                    }
                }
                Err(e) => format!("Render failed: {e}"),
            };
            self.notice = Some((msg, Instant::now()));
        }
        if let Some((insert, name, rx)) = &self.chain_load
            && let Ok(items) = rx.try_recv()
        {
            let (insert, name) = (*insert, name.clone());
            self.chain_load = None;
            let mut problems = Vec::new();
            for (spec, plugin) in items {
                let result = match (spec.source.clone(), plugin) {
                    (engine::song::EffectSource::Builtin { effect }, _) => s.add_builtin_effect(insert, effect),
                    (engine::song::EffectSource::Plugin(p), Some(Ok(instance))) => s.add_effect(insert, p, instance).map(|_| s.view.inserts[insert].effects.len() - 1),
                    (_, Some(Err(e))) => Err(e),
                    _ => Err("FL Studio effects can't be added from a chain".into()),
                };
                match result {
                    Ok(slot) => {
                        s.set_effect_mix(insert, slot, spec.mix);
                        if spec.bypassed {
                            s.set_effect_bypass(insert, slot, true);
                        }
                    }
                    Err(e) => problems.push(format!("{}: {e}", spec.name)),
                }
            }
            self.notice = Some((if problems.is_empty() { format!("Added effect chain “{name}”") } else { format!("Chain “{name}”: {}", problems.join("; ")) }, Instant::now()));
        }
        for (channel, name, source, status, warnings) in finished {
            let msg = match s.finish_load(channel, source, status) {
                Ok(()) if warnings.is_empty() => format!("Loaded {name}"),
                Ok(()) => warnings.join("; "),
                Err(e) => format!("{name}: {e}"),
            };
            self.notice = Some((msg, Instant::now()));
        }
    }

    /// Writes a crash-recovery copy of unsaved work every few minutes.
    fn autosave_tick(&mut self) {
        if self.autosave_job.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            let result = self.autosave_job.take().unwrap().join();
            match result {
                Ok(Err(e)) => self.notify(format!("Autosave failed: {e}")),
                Err(_) => self.notify("Autosave worker stopped unexpectedly"),
                Ok(Ok(())) => {}
            }
        }
        if self.autosave_job.is_some() {
            return;
        }
        let minutes = self.prefs.autosave_minutes;
        if minutes == 0 || self.last_autosave.elapsed().as_secs() < u64::from(minutes) * 60 {
            return;
        }
        self.last_autosave = Instant::now();
        let State::Ready(s) = &self.state else { return };
        if !s.modified {
            return;
        }
        let (base, view, controls) = s.autosave_snapshot();
        let original = s.path.clone();
        let name = s.view.name.clone();
        let session_id = s.session_id;
        self.autosave_job = Some(std::thread::spawn(move || {
            let (song, _) = engine::song::capture(&base, &view, &controls);
            recovery::autosave(&song, original.as_deref(), &name, session_id)
        }));
    }

    /// Wait only when saving or leaving, so an older worker cannot recreate a
    /// recovery file after the current session deliberately discarded it.
    fn finish_autosave(&mut self) {
        if let Some(job) = self.autosave_job.take() {
            match job.join() {
                Ok(Err(e)) => self.notify(format!("Autosave failed: {e}")),
                Err(_) => self.notify("Autosave worker stopped unexpectedly"),
                Ok(Ok(())) => {}
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if let Some(w) = self.help_window {
            let mut open = true;
            let title = if w == HelpWindow::Shortcuts { "Keyboard shortcuts" } else { "About KVNG Studio" };
            egui::Window::new(title).id(egui::Id::new("help-window")).open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| match w {
                HelpWindow::Shortcuts => help::shortcuts(ui),
                HelpWindow::About => help::about(ui),
            });
            if !open {
                self.help_window = None;
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::I)) && !ctx.egui_wants_keyboard_input() {
            self.windows.inspector = !self.windows.inspector;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Comma)) {
            self.settings.show(settings_ui::Tab::Audio);
        }
        let session = match &mut self.state {
            State::Ready(s) => Some(&mut **s),
            _ => None,
        };
        let actions = self.settings.ui(ctx, &mut self.prefs, &mut self.picker, session, self.audio_info.as_ref());
        for a in actions {
            match a {
                settings_ui::SettingsAction::RestartAudio => self.restart_audio(ctx),
                settings_ui::SettingsAction::Rescan(full) => self.picker.start_scan(full, ctx),
                settings_ui::SettingsAction::Message(m) => self.notify(m),
            }
        }
    }

    /// Reopens the current project on the audio device the settings now choose, keeping
    /// unsaved changes (plugin states are captured from their helpers first).
    fn restart_audio(&mut self, ctx: &egui::Context) {
        match &self.state {
            State::Ready(s) => {
                let (song, problems) = s.to_song();
                let source = Source::Memory { song: Box::new(song), path: s.path.clone(), modified: s.modified };
                self.load(source, ctx);
                if !problems.is_empty() {
                    self.notify(format!("Note: {}", problems.join("; ")));
                }
            }
            _ => self.notify("Audio settings saved; they apply when a project opens"),
        }
    }
}

impl App {
    fn poll_export(&mut self) {
        let Some(export) = &mut self.export else { return };
        while let Ok(msg) = export.rx.try_recv() {
            match msg {
                ExportMsg::Progress(status, fraction) => {
                    export.status = status;
                    export.fraction = fraction;
                }
                ExportMsg::Done(result) => export.result = Some(result),
            }
        }
    }

    /// The save-changes prompt and the export progress window.
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(pending) = self.confirm.clone() {
            let name = match &self.state {
                State::Ready(s) => s.view.name.clone(),
                _ => String::new(),
            };
            let mut choice = None;
            egui::Modal::new(egui::Id::new("confirm-save")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading(format!("Save changes to “{name}”?"));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Your changes will be lost if you don't save them.").color(theme::TEXT_DIM));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Don't save").clicked() {
                        choice = Some(false);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Save").color(theme::BG)).fill(theme::ACCENT)).clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm = None;
                        }
                    });
                });
            });
            match choice {
                Some(true) => {
                    self.confirm = None;
                    if self.save(false) {
                        self.perform(pending, ctx);
                    }
                }
                Some(false) => {
                    self.confirm = None;
                    self.perform(pending, ctx);
                }
                None => {}
            }
        }

        if let Some(mut opts) = self.export_setup {
            let mut go = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("export-setup")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Export");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.radio_value(&mut opts.stems, false, "Song mixdown");
                    ui.radio_value(&mut opts.stems, true, "Split mixer tracks (stems)");
                });
                ui.add_space(4.0);
                let song_loop = match &self.state {
                    State::Ready(s) => s.song_loop,
                    _ => None,
                };
                if let (ExportRange::Loop(..), None) = (opts.range, song_loop) {
                    opts.range = ExportRange::Song;
                }
                egui::Grid::new("export-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Range");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut opts.range, ExportRange::Song, "Whole song");
                        match song_loop {
                            Some((a, b)) => {
                                ui.radio_value(&mut opts.range, ExportRange::Loop(a, b), "Loop selection");
                            }
                            None => {
                                ui.add_enabled(false, egui::RadioButton::new(false, "Loop selection")).on_disabled_hover_text("Drag on the playlist ruler to set a loop first");
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Tail");
                    ui.add(egui::Slider::new(&mut opts.tail, 0.0..=10.0).suffix(" s").step_by(0.5)).on_hover_text("Extra time after the end so reverb and delay can ring out");
                    ui.end_row();
                    ui.label("Format");
                    let label = match (opts.flac, opts.bits) {
                        (true, 16) => "FLAC 16-bit",
                        (true, _) => "FLAC 24-bit",
                        (false, 16) => "WAV 16-bit",
                        (false, 32) => "WAV 32-bit float",
                        _ => "WAV 24-bit",
                    };
                    egui::ComboBox::from_id_salt("export-bits").selected_text(label).show_ui(ui, |ui| {
                        for (flac, bits, name) in [(false, 16, "WAV 16-bit"), (false, 24, "WAV 24-bit"), (false, 32, "WAV 32-bit float"), (true, 16, "FLAC 16-bit"), (true, 24, "FLAC 24-bit")] {
                            if ui.selectable_label(opts.flac == flac && opts.bits == bits, name).clicked() {
                                (opts.flac, opts.bits) = (flac, bits);
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("export-rate").selected_text(format!("{:.1} kHz", opts.sample_rate as f32 / 1000.0)).show_ui(ui, |ui| {
                        for r in [44_100, 48_000, 88_200, 96_000] {
                            ui.selectable_value(&mut opts.sample_rate, r, format!("{:.1} kHz", r as f32 / 1000.0));
                        }
                    });
                    ui.end_row();
                });
                ui.checkbox(&mut opts.normalize, "Normalize to −0.3 dBFS");
                if opts.bits == 16 {
                    ui.checkbox(&mut opts.dither, "Dither (recommended for 16-bit)");
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("Export…").color(theme::BG)).fill(theme::ACCENT)).clicked() {
                            go = true;
                        }
                    });
                });
            });
            self.export_setup = if go || cancel { None } else { Some(opts) };
            if go {
                self.export_options = opts;
                self.start_export(ctx, opts);
            }
        }

        let mut close = false;
        if let Some(export) = &self.export {
            egui::Modal::new(egui::Id::new("export")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Export WAV");
                ui.add_space(8.0);
                match &export.result {
                    None => {
                        ui.label(egui::RichText::new(&export.status).color(theme::TEXT_DIM));
                        ui.add_space(6.0);
                        ui.add(egui::ProgressBar::new(export.fraction).fill(theme::ACCENT).show_percentage());
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    Some(Ok((path, report))) => {
                        ui.label("Done. Saved to:");
                        ui.label(egui::RichText::new(path).color(theme::TEXT_DIM).small());
                        ui.add_space(4.0);
                        let peak = if report.peak_db < -120.0 { "silent".to_string() } else { format!("{:+.1} dBFS", report.peak_db) };
                        match report.normalized_db {
                            Some(g) => ui.label(egui::RichText::new(format!("Peak {peak} · normalized by {g:+.1} dB")).color(theme::TEXT_DIM)),
                            None if report.clipped => ui.label(
                                egui::RichText::new(format!("Peak {peak} — the export clipped. Turn the master down, add a limiter, choose 32-bit float, or tick Normalize."))
                                    .color(theme::ERROR),
                            ),
                            None => ui.label(egui::RichText::new(format!("Peak {peak}")).color(theme::TEXT_DIM)),
                        };
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Show in Finder").clicked() {
                                let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        });
                    }
                    Some(Err(e)) => {
                        ui.label(egui::RichText::new(format!("Export failed: {e}")).color(theme::ERROR));
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    }
                }
            });
        }
        if close {
            self.export = None;
        }
    }

    /// Captures the window to a PNG and quits, for checking the UI without screen-recording access.
    fn developer_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else { return };
        shot.frames += 1;
        if !shot.prepared
            && shot.play
            && let State::Ready(s) = &mut self.state
        {
            shot.prepared = true;
            if std::env::var_os("KVNG_SCREENSHOT_METER").is_some() {
                s.set_meter_change(2, 3);
                s.set_meter_change(5, 7);
            }
            if std::env::var_os("KVNG_SCREENSHOT_GRAPH").is_some() {
                for (k, step) in [0u64, 2, 4, 6, 8, 10, 12, 14].into_iter().enumerate() {
                    if !s.view.step_on(s.pattern, 0, step) {
                        s.view.toggle_step(s.pattern, 0, step);
                    }
                    let at = step * s.view.step_ticks();
                    let pattern = s.pattern;
                    if let Some(n) = s.view.patterns[pattern].notes.iter_mut().find(|n| n.channel == 0 && n.start == at) {
                        n.velocity = [128, 40, 90, 60, 128, 30, 100, 70][k];
                    }
                }
                s.channel = 0;
                s.mark_dirty();
                rack::open_graph(ctx);
            }
            if std::env::var_os("KVNG_SCREENSHOT_FOLDER").is_some() && s.view.tracks.len() > 6 {
                s.view.tracks[3].folder = true;
                s.view.tracks[3].name = "Drums".into();
                s.view.tracks[4].grouped = true;
                s.view.tracks[5].grouped = true;
                s.view.tracks[3].collapsed = std::env::var("KVNG_SCREENSHOT_FOLDER").is_ok_and(|v| v == "collapsed");
            }
            if let Some(f) = std::env::var_os("KVNG_SCREENSHOT_BROWSE") {
                self.browser.preview_file(PathBuf::from(f), s.view.sample_rate);
            }
            if std::env::var_os("KVNG_SCREENSHOT_AUTOMATION").is_some() {
                let a = s.create_automation(engine::view::AutomationTarget::InsertVolume { insert: 1 });
                let pts = &mut s.view.automations[a].points;
                let end = pts.last().map_or(1536, |p| p.tick);
                *pts = vec![
                    engine::view::AutoPoint { tick: 0, value: 0.1, tension: 0.0 },
                    engine::view::AutoPoint { tick: end / 3, value: 0.9, tension: -0.7 },
                    engine::view::AutoPoint { tick: end * 2 / 3, value: 0.3, tension: 0.6 },
                    engine::view::AutoPoint { tick: end, value: 0.8, tension: 0.0 },
                ];
                self.automation_window = Some(a);
            }
            if let Some(wav) = std::env::var_os("KVNG_SCREENSHOT_AUDIO")
                && let Ok(sample) = engine::sample::load_audio(Path::new(&wav), s.view.sample_rate)
                && s.add_audio_clip(Path::new(&wav), std::sync::Arc::new(sample), 0, 0).is_ok()
                && let Some(clip) = s.view.clips.last_mut()
            {
                clip.length = clip.length.min(96 * 16);
                (clip.fade_in, clip.fade_out, clip.gain) = (96 * 2, 96 * 4, 0.7);
                let mut second = clip.clone();
                (second.start, second.track, second.reverse, second.gain, second.fade_in, second.fade_out) = (96 * 4, 1, true, 1.0, 0, 96);
                s.view.clips.push(second);
                self.playlist.selected = vec![s.view.clips.len().saturating_sub(2)];
                if std::env::var_os("KVNG_SCREENSHOT_STRETCH").is_some() {
                    let ci = s.view.clips.len() - 2;
                    let before = s.view.clips[ci].length;
                    let msg = s.stretch_audio_clip(ci, 1.5).unwrap_or_else(|e| e);
                    eprintln!("stretch: {msg}; clip length {before} -> {}", s.view.clips[ci].length);
                }
                s.mark_dirty();
            }
            if std::env::var_os("KVNG_SCREENSHOT_COMP").is_some()
                && let Some(mut st) = s.strip(1)
            {
                st.comp.on = true;
                st.comp.threshold = -40.0;
                st.comp.ratio = 8.0;
                st.eq.on = true;
                st.eq.bands[0].gain = 6.0;
                s.set_strip(1, st);
                for step in [0, 4, 8, 12] {
                    if !s.view.step_on(s.pattern, 0, step) {
                        s.view.toggle_step(s.pattern, 0, step);
                    }
                }
                s.mark_dirty();
            }
            s.controls.play();
        }
        if std::env::var_os("KVNG_SCREENSHOT_AUTOREC").is_some()
            && shot.prepared
            && let State::Ready(s) = &mut self.state
        {
            s.recording = true;
            if s.mode != Mode::Song {
                s.set_mode(Mode::Song);
                s.controls.play();
            }
            let t = s.position_ticks() / s.ticks_per_bar() as f64;
            let v = 0.55 + 0.35 * (t * std::f64::consts::TAU / 2.0).sin() as f32;
            s.controls.channels[0].volume.store(engine::gain::channel_volume(v));
            s.record_automation(vec![engine::view::AutomationTarget::ChannelVolume { channel: 0 }]);
        }
        let wait: u32 = std::env::var("KVNG_SCREENSHOT_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
        if shot.frames == wait && !shot.requested {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            if let Err(e) = write_png(&shot.path, &image) {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn write_png(path: &Path, image: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.size[0] as u32, image.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}

enum WelcomeAction {
    New,
    Open,
    Recover(usize),
    Forget(usize),
}

/// The empty state: open a project or start a new one.
fn welcome(ui: &mut Ui, error: Option<&str>, recoveries: &[recovery::AutosaveInfo]) -> Option<WelcomeAction> {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, theme::BG);
    let extra = if recoveries.is_empty() { 0.0 } else { 46.0 + 30.0 * recoveries.len().min(4) as f32 };
    let card = Rect::from_center_size(rect.center(), vec2(480.0, if error.is_some() { 410.0 } else { 360.0 } + extra));
    let painter = ui.painter();
    painter.rect(card, egui::CornerRadius::same(12), theme::PANEL_2, Stroke::new(1.0, theme::LINE_STRONG), StrokeKind::Inside);

    // 3D Logo Emblem
    let logo = splash::logo_texture(ui.ctx());
    let logo_center = pos2(card.center().x, card.min.y + 75.0);
    painter.circle_filled(logo_center, 65.0, theme::ACCENT_GLOW);
    painter.image(logo.id(), Rect::from_center_size(logo_center, vec2(120.0, 120.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);

    painter.text(pos2(card.center().x, card.min.y + 152.0), Align2::CENTER_CENTER, "KVNG STUDIO", theme::ui_font(20.0), theme::TEXT);
    painter.text(pos2(card.center().x, card.min.y + 174.0), Align2::CENTER_CENTER, "PRODUCER EDITION • 64-BIT HYBRID ENGINE", theme::mono(10.0), theme::ACCENT_HOT);
    painter.text(
        pos2(card.center().x, card.min.y + 204.0),
        Align2::CENTER_CENTER,
        "Open a KVNG or FL Studio project, or drop one here.",
        theme::ui_font(12.5),
        theme::TEXT_DIM,
    );
    let mut action = None;
    let open = Rect::from_center_size(pos2(card.center().x + 96.0, card.min.y + 252.0), vec2(176.0, 38.0));
    let new = Rect::from_center_size(pos2(card.center().x - 96.0, card.min.y + 252.0), vec2(176.0, 38.0));
    for (r, label, primary, key, a) in [(open, "Open project…", true, "⌘O", WelcomeAction::Open), (new, "New project", false, "⌘N", WelcomeAction::New)] {
        let resp = ui.interact(r, egui::Id::new(("welcome", label)), Sense::click());
        let painter = ui.painter();
        let (fill, text) = if primary {
            (if resp.hovered() { theme::ACCENT_HOT } else { theme::ACCENT }, theme::BG)
        } else {
            (if resp.hovered() { theme::RAISED_HI } else { theme::RAISED }, theme::TEXT)
        };
        painter.rect(r, theme::RADIUS, fill, Stroke::new(1.0, if primary { theme::ACCENT } else { theme::LINE_STRONG }), StrokeKind::Inside);
        painter.text(r.center(), Align2::CENTER_CENTER, label, theme::ui_font(14.0), text);
        painter.text(pos2(r.center().x, r.max.y + 14.0), Align2::CENTER_CENTER, key, theme::ui_font(10.0), theme::TEXT_FAINT);
        if resp.clicked() {
            action = Some(a);
        }
    }
    let mut y = open.max.y + 34.0;
    if !recoveries.is_empty() {
        let r = Rect::from_min_max(pos2(card.min.x + 20.0, y), pos2(card.max.x - 20.0, y + extra - 6.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            ui.label(egui::RichText::new("RECOVER UNSAVED WORK").font(theme::ui_font(10.5)).color(theme::WARN));
            ui.label(egui::RichText::new("KVNG Studio didn't close normally. These autosaves have changes that weren't saved:").small().color(theme::TEXT_DIM));
            for (i, info) in recoveries.iter().enumerate().take(4) {
                ui.horizontal(|ui| {
                    ui.label(format!("{} · {}", info.name, recovery::age(info.saved_at)));
                    if ui.small_button("Recover").clicked() {
                        action = Some(WelcomeAction::Recover(i));
                    }
                    if ui.small_button("Discard").clicked() {
                        action = Some(WelcomeAction::Forget(i));
                    }
                });
            }
        });
        y += extra;
    }
    if let Some(e) = error {
        let r = Rect::from_min_max(pos2(card.min.x + 20.0, y), pos2(card.max.x - 20.0, card.max.y - 14.0));
        ui.put(r, egui::Label::new(egui::RichText::new(e).color(theme::ERROR).small()).wrap());
    }
    action
}

/// Builds a fresh engine for `song` (with its own plugin helpers) and renders it: a
/// single mixdown to `out`, or (stems) one file per used insert into the folder `out`.
#[allow(clippy::too_many_arguments)]
fn export_audio(
    song: engine::song::Song,
    origin: &Path,
    out: &Path,
    name: &str,
    inserts: &[(usize, String)],
    opts: ExportOptions,
    progress: &mut dyn FnMut(&str, f32),
) -> Result<ExportReport, String> {
    let sample_rate = opts.sample_rate as f64;
    let loaded = engine::load::build(None, song, origin, sample_rate, &mut |msg| progress(msg, 0.0));
    let view = loaded.view;
    let mut engine = loaded.engine;
    let controls = engine.controls();
    // Plugins may finish loading samples asynchronously after their state is restored.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    if opts.stems {
        engine.capture_stems(&inserts.iter().map(|(i, _)| *i).collect::<Vec<_>>());
    }
    // The engine's song length includes its own release tail; swap that for the chosen one.
    let tail = (opts.tail.max(0.0) as f64 * sample_rate) as usize;
    let (start, body) = match opts.range {
        ExportRange::Song => (0, (engine.length as usize).saturating_sub((engine::timeline::TAIL_SECONDS * sample_rate) as usize)),
        ExportRange::Loop(a, b) => {
            let (sa, sb) = (view.tick_to_sample_f(a as f64).round() as u64, view.tick_to_sample_f(b as f64).round() as u64);
            (sa, sb.saturating_sub(sa) as usize)
        }
    };
    controls.set_loop(None);
    controls.seek(start);
    controls.play();

    let latency = engine.latency;
    let total = body + tail + latency;
    let chunk = opts.sample_rate as usize;
    let (mut l, mut r) = (vec![0.0f32; chunk], vec![0.0f32; chunk]);
    let mut mix = [Vec::with_capacity(total), Vec::with_capacity(total)];
    let mut done = 0;
    while done < total {
        let n = chunk.min(total - done);
        engine.process(&mut l[..n], &mut r[..n]);
        mix[0].extend_from_slice(&l[..n]);
        mix[1].extend_from_slice(&r[..n]);
        done += n;
        progress("Rendering", done as f32 / total as f32);
    }

    let skip = latency.min(mix[0].len());
    let peak = mix[0][skip..].iter().chain(&mix[1][skip..]).fold(0.0f32, |m, v| m.max(v.abs()));
    let peak_db = if peak > 0.0 { 20.0 * peak.log10() } else { -150.0 };
    let gain = if opts.normalize && peak > 1e-6 { 10f32.powf(-0.3 / 20.0) / peak } else { 1.0 };
    let report = ExportReport { peak_db, clipped: !opts.normalize && opts.bits != 32 && peak > 1.0, normalized_db: (opts.normalize && peak > 1e-6).then(|| 20.0 * gain.log10()) };
    if !opts.stems {
        write_wav(out, &mix[0][skip..], &mix[1][skip..], opts, gain)?;
        return Ok(report);
    }
    progress("Writing stems", 1.0);
    // Inserts reach the master `latency − master effects' latency` samples late.
    let stem_delay = latency.saturating_sub(engine.inserts.first().map_or(0, |m| m.latency()));
    for (index, [sl, sr]) in engine.take_stems() {
        let label = inserts.iter().find(|(i, _)| *i == index).map(|(_, n)| n.clone()).unwrap_or_default();
        let file_label = if index == 0 { "Master".to_string() } else { format!("{index:02} {label}") };
        let safe: String = format!("{name} - {file_label}").chars().map(|c| if matches!(c, '/' | ':' | '\\') { '_' } else { c }).collect();
        let skip = if index == 0 { latency } else { stem_delay }.min(sl.len());
        // Stems share the mixdown's normalizing gain so they still sum to the mix.
        write_wav(&out.join(format!("{safe}.wav")), &sl[skip..], &sr[skip..], opts, gain)?;
    }
    Ok(report)
}

/// Writes 16- or 24-bit FLAC (lossless).
fn write_flac(path: &Path, l: &[f32], r: &[f32], opts: ExportOptions, gain: f32) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let bits = if opts.bits == 16 { 16 } else { 24 };
    let scale = ((1u32 << (bits - 1)) - 1) as f32;
    let dither = opts.dither && bits == 16;
    let mut seed: u32 = 0x9E37_79B9;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let mut interleaved = Vec::with_capacity(l.len() * 2);
    for (a, b) in l.iter().zip(r) {
        for s in [a, b] {
            let noise = if dither { rand() - rand() } else { 0.0 };
            interleaved.push(((s * gain).clamp(-1.0, 1.0) * scale + noise).round().clamp(-scale - 1.0, scale) as i32);
        }
    }
    let config = flacenc::config::Encoder::default().into_verified().map_err(|e| format!("FLAC settings: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(&interleaved, 2, bits, opts.sample_rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).map_err(|e| format!("FLAC encoding failed: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| format!("FLAC encoding failed: {e:?}"))?;
    std::fs::write(path, sink.as_slice()).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_wav(path: &Path, l: &[f32], r: &[f32], opts: ExportOptions, gain: f32) -> Result<(), String> {
    if opts.flac {
        return write_flac(&path.with_extension("flac"), l, r, opts, gain);
    }
    let float = opts.bits == 32;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: opts.sample_rate,
        bits_per_sample: opts.bits,
        sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    let scale = ((1u32 << (opts.bits.min(24) - 1)) - 1) as f32;
    // TPDF dither (two uniform randoms, ±1 LSB) for 16-bit, from a small xorshift generator.
    let dither = opts.dither && opts.bits == 16;
    let mut seed: u32 = 0x9E37_79B9;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for (a, b) in l.iter().zip(r) {
        if float {
            w.write_sample(a * gain).and_then(|_| w.write_sample(b * gain)).map_err(|e| e.to_string())?;
        } else {
            for s in [a, b] {
                let noise = if dither { rand() - rand() } else { 0.0 };
                w.write_sample(((s * gain).clamp(-1.0, 1.0) * scale + noise).round().clamp(-scale - 1.0, scale) as i32).map_err(|e| e.to_string())?;
            }
        }
    }
    w.finalize().map_err(|e| e.to_string())
}

#[cfg(test)]
mod export_tests {
    use super::*;

    #[test]
    fn exports_a_loop_range_with_tail_and_normalizes() {
        let mut song = engine::song::Song::empty("t");
        for beat in 0..8u64 {
            song.patterns[0].notes.push(engine::view::NoteView { start: beat * 96, length: 24, key: 60, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        }
        song.patterns[0].length = 768;
        song.clips.push(engine::view::ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track: 0, start: 0, length: 768, label: "P".into(), kind: engine::view::ClipKind::Pattern, pattern: Some(0), channel: None, source_offset: 0.0 });
        let dir = std::env::temp_dir().join(format!("kvng-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("loop.wav");
        // One bar (384 ticks at 130 BPM ≈ 1.846 s) plus a 1 s tail.
        let opts = ExportOptions { range: ExportRange::Loop(384, 768), tail: 1.0, normalize: true, bits: 24, sample_rate: 48_000, ..Default::default() };
        let report = export_audio(song, &dir.join("t.kvng"), &out, "t", &[], opts, &mut |_, _| {}).unwrap();
        assert!(report.normalized_db.is_some() && !report.clipped);
        let reader = hound::WavReader::open(&out).unwrap();
        let secs = reader.duration() as f64 / 48_000.0;
        assert!((secs - (60.0 / 130.0 * 4.0 + 1.0)).abs() < 0.01, "length {secs}");
        let peak = reader.into_samples::<i32>().map(|s| s.unwrap().unsigned_abs()).max().unwrap() as f64 / ((1 << 23) - 1) as f64;
        assert!((20.0 * peak.log10() + 0.3).abs() < 0.05, "normalized peak {peak}");
        // The same render as FLAC decodes back to the same length.
        let flac = dir.join("loop.flac");
        let mut song2 = engine::song::Song::empty("t");
        song2.patterns[0].notes.push(engine::view::NoteView { start: 0, length: 24, key: 60, velocity: 100, channel: 0, pan: 64, mod_x: 128, mod_y: 128, slide: false });
        song2.clips.push(engine::view::ClipView { automation: None, color: None, gain: 1.0, fade_in: 0, fade_out: 0, reverse: false, track: 0, start: 0, length: 384, label: "P".into(), kind: engine::view::ClipKind::Pattern, pattern: Some(0), channel: None, source_offset: 0.0 });
        let fopts = ExportOptions { flac: true, bits: 16, sample_rate: 44_100, tail: 0.5, ..Default::default() };
        export_audio(song2, &dir.join("t.kvng"), &flac, "t", &[], fopts, &mut |_, _| {}).unwrap();
        let bytes = std::fs::read(&flac).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        if let Ok(keep) = std::env::var("KVNG_KEEP_FLAC") {
            let _ = std::fs::copy(&flac, keep);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
