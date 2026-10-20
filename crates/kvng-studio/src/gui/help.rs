//! Help menu windows: keyboard shortcuts and About (with third-party credits).

use eframe::egui::{self, Ui};

use super::theme;

const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Transport & project",
        &[
            ("Space", "Play / stop"),
            ("Home", "Back to the start"),
            ("L", "Switch pattern / song mode"),
            ("R", "Record (arm note recording)"),
            ("⌘N / ⌘O", "New / open project"),
            ("⌘S / ⇧⌘S", "Save / save as"),
            ("⌘R", "Export audio"),
            ("⌘Z / ⇧⌘Z", "Undo / redo"),
            ("⌘,", "Settings"),
        ],
    ),
    (
        "Windows",
        &[
            ("F5", "Playlist"),
            ("F6", "Channel rack"),
            ("F7", "Piano roll"),
            ("F8", "Plugin picker"),
            ("⌥F8", "Browser"),
            ("F9", "Mixer"),
            ("⌘I", "Inspector"),
        ],
    ),
    (
        "Piano roll",
        &[
            ("Click / drag", "Add a note / move or resize"),
            ("P / B / E (S) / C / D", "Draw / Paint / Select / Slice / Delete tools"),
            ("Drag in Select", "Box-select notes (Shift adds to selection)"),
            ("⌥S", "Apply the current strum settings"),
            ("Piano feel", "Vary new note velocities or apply to existing notes"),
            ("Right-click", "Delete notes"),
            ("Shift-drag / ⌘-drag", "Select with a box (Shift adds to the selection)"),
            ("Shift-click", "Add / remove a note from the selection"),
            ("⌘A", "Select all"),
            ("⌘C / ⌘V", "Copy / paste"),
            ("⌘B", "Duplicate selection"),
            ("Q", "Quantize (strength in Tools → Quantize)"),
            ("↑ / ↓", "Transpose a semitone (⇧ = octave)"),
            ("← / →", "Nudge by the snap"),
            ("⌫", "Delete selection"),
        ],
    ),
    (
        "Playlist",
        &[
            ("Click", "Place the current pattern"),
            ("C / P", "Slice tool / draw tool"),
            ("Shift-drag", "Select clips with a box"),
            ("Shift-click", "Add / remove a clip from the selection"),
            ("⌘A", "Select all clips"),
            ("⌫", "Delete selection"),
            ("Drag clip edge", "Resize (left edge trims audio clips)"),
            ("⌥-drag audio", "Slip the audio inside the clip"),
            ("Corner handles", "Audio clip fades"),
            ("Drag on the ruler", "Set the song loop"),
        ],
    ),
    (
        "Audio editor",
        &[("Drag", "Select"), ("Double-click / ⌘A", "Select all"), ("⌘X / ⌘C / ⌘V", "Cut / copy / paste"), ("⌫", "Delete selection"), ("⌘-scroll", "Zoom")],
    ),
    (
        "Typing keyboard (Keys)",
        &[("Z S X D C V G B H N J M", "C5 – B5"), ("Q 2 W 3 E R 5 T 6 Y 7 U", "C6 – B6")],
    ),
];

pub fn shortcuts(ui: &mut Ui) {
    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
        for (section, keys) in SHORTCUTS {
            ui.label(egui::RichText::new(section.to_uppercase()).font(theme::ui_font(10.5)).color(theme::ACCENT));
            egui::Grid::new(("shortcuts", *section)).num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
                for (key, what) in *keys {
                    ui.label(egui::RichText::new(*key).font(theme::mono(11.5)));
                    ui.label(*what);
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
        }
    });
}

pub fn about(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        let logo = super::splash::logo_texture(ui.ctx());
        ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(140.0, 140.0)));
        ui.label(egui::RichText::new(format!("KVNG Studio {}", env!("CARGO_PKG_VERSION"))).font(theme::ui_font(16.0)));
        ui.label(egui::RichText::new("A music workstation for macOS").color(theme::TEXT_DIM));
    });
    ui.add_space(10.0);
    ui.label(egui::RichText::new("BUILT WITH").font(theme::ui_font(10.5)).color(theme::ACCENT));
    for (name, what) in [
        ("JUCE", "VST3 / Audio Unit plugin hosting (juce.com)"),
        ("egui / eframe", "user interface"),
        ("cpal", "audio input and output"),
        ("midir, midly", "MIDI devices and MIDI files"),
        ("Symphonia", "audio sample decoding"),
        ("hound, flacenc", "WAV and FLAC files"),
        ("serde", "project files"),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(name).strong());
            ui.label(egui::RichText::new(what).color(theme::TEXT_DIM));
        });
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Opens FL Studio projects for compatibility. KVNG Studio is not affiliated with Image-Line. VST is a trademark of Steinberg Media Technologies GmbH; Audio Units is a trademark of Apple Inc.").small().color(theme::TEXT_FAINT));
}
