//! App preferences (Settings window), saved as JSON in the data folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Output device by name (None = the system default).
    pub output_device: Option<String>,
    /// Input device for recording (None = the system default).
    pub input_device: Option<String>,
    /// None = the device's own rate.
    pub sample_rate: Option<u32>,
    /// Frames per buffer (None = the device's default).
    pub buffer_size: Option<u32>,
    /// MIDI inputs the user switched off, by port name.
    pub disabled_midi_inputs: Vec<String>,
    /// Extra VST3 folders to scan.
    pub plugin_folders: Vec<PathBuf>,
    /// Minutes between autosaves (0 = off).
    pub autosave_minutes: u32,
    /// Backups kept per project when saving over it.
    pub backups: u32,
    /// Interface zoom (1.0 = 100%).
    pub ui_scale: f32,
    /// Bars of metronome before recording starts (0 = none).
    pub count_in_bars: u32,
    /// Click while recording even if the metronome is off.
    pub metronome_when_recording: bool,
    /// Metronome level, 0..=2 (1 = default).
    pub metronome_volume: f32,
    /// Hear the input device through the outputs while recording audio.
    pub monitor_input: bool,
    /// Manual recording offset in ms (None = estimate from the buffer size).
    pub record_offset_ms: Option<f32>,
    /// Default export format.
    pub export_bits: u16,
    pub export_sample_rate: Option<u32>,
    /// Folder new projects and the Open dialog start in.
    pub project_folder: Option<PathBuf>,
    /// Reopen the last project on launch.
    pub reopen_last: bool,
    pub last_project: Option<PathBuf>,
    /// Show hints in tooltips.
    pub hints: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            output_device: None,
            input_device: None,
            sample_rate: None,
            buffer_size: None,
            disabled_midi_inputs: Vec::new(),
            plugin_folders: Vec::new(),
            autosave_minutes: 5,
            backups: 10,
            ui_scale: 1.0,
            count_in_bars: 1,
            metronome_when_recording: true,
            metronome_volume: 1.0,
            monitor_input: false,
            record_offset_ms: None,
            export_bits: 24,
            export_sample_rate: None,
            project_folder: None,
            reopen_last: false,
            last_project: None,
            hints: true,
        }
    }
}

fn path() -> PathBuf {
    crate::data_dir().join("settings.json")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    /// Whether changing from `old` to `self` needs the audio engine restarted.
    pub fn audio_changed(&self, old: &Prefs) -> bool {
        self.output_device != old.output_device || self.sample_rate != old.sample_rate || self.buffer_size != old.buffer_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_partial_files_get_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"backups": 3}"#).unwrap();
        assert_eq!(p.backups, 3);
        assert_eq!(p.autosave_minutes, 5);
        assert_eq!(p.ui_scale, 1.0);
        let round: Prefs = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(round, p);
    }
}
