//! User presets: plugin states saved per plugin, effect chains, and favourite plugins.
//! Stored under `<data>/Presets/<plugin>/`, `<data>/Chains/` and `<data>/favorites.json`.

use std::path::PathBuf;

const EXT: &str = "kvngpreset";

fn safe(name: &str) -> String {
    name.chars().map(|c| if matches!(c, '/' | ':' | '\\' | '\0') { '_' } else { c }).collect::<String>().trim().to_string()
}

fn plugin_dir(plugin: &str) -> PathBuf {
    crate::data_dir().join("Presets").join(safe(plugin))
}

/// Saved presets for `plugin`, sorted by name.
pub fn list(plugin: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(plugin_dir(plugin))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == EXT)).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())).flatten()
        })
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

pub fn save(plugin: &str, name: &str, state: &[u8]) -> Result<PathBuf, String> {
    let name = safe(name);
    if name.is_empty() {
        return Err("give the preset a name".into());
    }
    let dir = plugin_dir(plugin);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{name}.{EXT}"));
    std::fs::write(&path, state).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

pub fn load(plugin: &str, name: &str) -> Result<Vec<u8>, String> {
    let path = plugin_dir(plugin).join(format!("{}.{EXT}", safe(name)));
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn delete(plugin: &str, name: &str) {
    let _ = std::fs::remove_file(plugin_dir(plugin).join(format!("{}.{EXT}", safe(name))));
}

fn chains_dir() -> PathBuf {
    crate::data_dir().join("Chains")
}

/// Saved effect chains, sorted by name.
pub fn chains() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(chains_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json")).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())).flatten()
        })
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

pub fn save_chain(name: &str, effects: &[engine::song::EffectSpec]) -> Result<(), String> {
    let name = safe(name);
    if name.is_empty() {
        return Err("give the chain a name".into());
    }
    std::fs::create_dir_all(chains_dir()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(effects).map_err(|e| e.to_string())?;
    std::fs::write(chains_dir().join(format!("{name}.json")), json).map_err(|e| e.to_string())
}

pub fn load_chain(name: &str) -> Result<Vec<engine::song::EffectSpec>, String> {
    let text = std::fs::read_to_string(chains_dir().join(format!("{}.json", safe(name)))).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn favorites_path() -> PathBuf {
    crate::data_dir().join("favorites.json")
}

/// Favourite plugins, by "name|vendor" key.
pub fn favorites() -> Vec<String> {
    std::fs::read_to_string(favorites_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn set_favorites(list: &[String]) {
    if let Ok(json) = serde_json::to_string_pretty(list) {
        let _ = std::fs::create_dir_all(crate::data_dir());
        let _ = std::fs::write(favorites_path(), json);
    }
}

/// The key a plugin is remembered by in favourites.
pub fn favorite_key(name: &str, vendor: &str) -> String {
    format!("{}|{}", name.to_lowercase(), vendor.to_lowercase())
}

/// A "Presets" submenu for a helper-hosted plugin: load, save as, delete.
/// Returns a status message after an action.
pub fn menu(ui: &mut eframe::egui::Ui, plugin: &str, handle: &bridge::EditorHandle) -> Option<String> {
    use eframe::egui;
    let mut message = None;
    ui.menu_button("Presets", |ui| {
        let saved = list(plugin);
        if saved.is_empty() {
            ui.label(egui::RichText::new(format!("No saved presets for {plugin}")).small().color(super::theme::TEXT_DIM));
        }
        for name in &saved {
            ui.horizontal(|ui| {
                if ui.button(name).clicked() {
                    message = Some(match load(plugin, name).and_then(|state| handle.load_state(&state)) {
                        Ok(()) => format!("Loaded preset “{name}”"),
                        Err(e) => format!("Couldn't load “{name}”: {e}"),
                    });
                    ui.close();
                }
                if ui.small_button("✕").on_hover_text("Delete this preset").clicked() {
                    delete(plugin, name);
                }
            });
        }
        ui.separator();
        let id = egui::Id::new(("preset-name", plugin));
        let mut name: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut name).hint_text("New preset name").desired_width(150.0));
            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Save")).clicked() {
                message = Some(match handle.save_state().and_then(|state| save(plugin, &name, &state)) {
                    Ok(_) => format!("Saved preset “{}”", name.trim()),
                    Err(e) => format!("Couldn't save the preset: {e}"),
                });
                name.clear();
                ui.close();
            }
        });
        ui.data_mut(|d| d.insert_temp(id, name));
    });
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_for_files() {
        assert_eq!(safe(" Lead/Bass: v2 "), "Lead_Bass_ v2");
        assert_eq!(favorite_key("Serum", "Xfer"), "serum|xfer");
    }
}
