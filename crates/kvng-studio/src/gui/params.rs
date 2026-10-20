//! Choosing a plugin parameter to automate.

use eframe::egui::{self, Ui};
use engine::view::AutomationTarget;

use super::session::Session;
use super::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Channel(usize),
    Effect { insert: usize, slot: usize },
}

pub struct ParamPicker {
    pub owner: Owner,
    list: Option<Result<Vec<(String, f32)>, String>>,
    search: String,
}

impl ParamPicker {
    pub fn new(owner: Owner) -> Self {
        Self { owner, list: None, search: String::new() }
    }

    fn plugin_name(&self, s: &Session) -> String {
        match self.owner {
            Owner::Channel(c) => s.view.channels.get(c).map(|c| c.name.clone()).unwrap_or_default(),
            Owner::Effect { insert, slot } => s.view.inserts.get(insert).and_then(|i| i.effects.get(slot)).map(|e| e.name.clone()).unwrap_or_default(),
        }
    }

    /// Draws the list. Returns true when a parameter was chosen (the picker can close).
    pub fn ui(&mut self, ui: &mut Ui, s: &mut Session) -> bool {
        if self.list.is_none()
            && let Owner::Effect { insert, slot } = self.owner
            && let Some(fx) = s.builtin_effect(insert, slot)
        {
            self.list = Some(Ok(fx.specs().iter().enumerate().map(|(i, p)| (p.name.to_string(), fx.normalized(i))).collect()));
        }
        if self.list.is_none() {
            let handle = match self.owner {
                Owner::Channel(c) => s.view.channels.get(c).and_then(|c| c.editor.clone()),
                Owner::Effect { insert, slot } => s.view.inserts.get(insert).and_then(|i| i.effects.get(slot)).and_then(|e| e.editor.clone()),
            };
            self.list = Some(handle.ok_or_else(|| "This plugin doesn't expose its parameters.".to_string()).and_then(|h| h.parameters()));
        }
        let plugin = self.plugin_name(s);
        ui.label(egui::RichText::new(format!("Choose a parameter of {plugin} to automate")).color(theme::TEXT_DIM));
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search parameters").desired_width(f32::INFINITY));
        ui.add_space(4.0);
        let mut chosen = None;
        match self.list.as_ref().unwrap() {
            Err(e) => {
                ui.label(egui::RichText::new(e).color(theme::WARN));
            }
            Ok(list) if list.is_empty() => {
                ui.label(egui::RichText::new("This plugin has no automatable parameters.").color(theme::TEXT_DIM));
            }
            Ok(list) => {
                let q = self.search.to_lowercase();
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for (i, (name, value)) in list.iter().enumerate() {
                        if !q.is_empty() && !name.to_lowercase().contains(&q) {
                            continue;
                        }
                        let label = if name.is_empty() { format!("Parameter {}", i + 1) } else { name.clone() };
                        if ui.selectable_label(false, format!("{label}   ({:.0}%)", value * 100.0)).clicked() {
                            chosen = Some((i as u32, label, *value));
                        }
                    }
                });
            }
        }
        if let Some((param, label, value)) = chosen {
            let target = match self.owner {
                Owner::Channel(channel) => AutomationTarget::ChannelPluginParam { channel, param },
                Owner::Effect { insert, slot } => AutomationTarget::EffectParam { insert, slot, param },
            };
            s.create_automation_named(target, format!("{plugin}: {label}"), value);
            return true;
        }
        false
    }
}
