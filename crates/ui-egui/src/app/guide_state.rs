//! The app's side of guides, the grid and guide sets: the rulers' menu choices, the guide-set
//! folder, the Save guides as a set window, and what the control channel reports.

use crate::guides::RulerMenuAction;
use crate::{PdfCraftApp, guide_sets, guides};

impl PdfCraftApp {
    /// Carry out a choice from the rulers' menu (or a click on their corner) on the tab at `index`.
    pub(crate) fn ruler_menu_action(&mut self, index: usize, action: RulerMenuAction) {
        match action {
            RulerMenuAction::Units(u) => self.units = u,
            RulerMenuAction::ToggleGuides => self.show_guides = !self.show_guides,
            RulerMenuAction::ToggleGrid => self.show_grid = !self.show_grid,
            RulerMenuAction::ToggleSnap => self.snap_to_grid = !self.snap_to_grid,
            RulerMenuAction::ClearGuides => {
                if let Some(v) = self.views.get_mut(index) {
                    v.guides.clear();
                }
            }
            RulerMenuAction::SaveSet => self.guide_set_draft = Some((String::new(), None, self.show_grid)),
            RulerMenuAction::ToggleSet(name) => self.toggle_guide_set(&name),
            RulerMenuAction::ApplySet(name) => {
                self.apply_guide_set(&name);
            }
        }
    }

    /// Each page's size in points (width, height) as displayed before View ▸ Rotate, for the tab at `index`.
    pub(crate) fn page_sizes(&self, index: usize) -> Vec<[f32; 2]> {
        self.views
            .get(index)
            .and_then(|v| self.session.get(v.id))
            .map(|d| d.info.pages.iter().map(|p| [p.width, p.height]).collect())
            .unwrap_or_default()
    }

    /// Read the guide-set folder again (after a hand edit, or a file another user sent).
    pub(crate) fn reload_guide_sets(&mut self) {
        self.guide_sets_loaded = true;
        self.guide_sets = self.guide_sets_dir.as_deref().map(guide_sets::load_dir).unwrap_or_default();
        for broken in self.guide_sets.iter().filter_map(|l| l.set.as_ref().err()) {
            log::warn!("guide set: {broken}");
        }
    }

    /// Read the folder once, the first time sets are needed.
    pub(crate) fn ensure_guide_sets(&mut self) {
        if !self.guide_sets_loaded {
            self.reload_guide_sets();
        }
    }

    /// Show the set named `name` if it is hidden, hide it if it is shown.
    pub(crate) fn toggle_guide_set(&mut self, name: &str) {
        if !self.shown_guide_sets.remove(name) {
            self.shown_guide_sets.insert(name.to_string());
        }
    }

    /// Ctrl+Shift+`n`: show or hide the set with that shortcut; say so when no set has it.
    pub(crate) fn toggle_guide_set_shortcut(&mut self, n: u8) {
        self.ensure_guide_sets();
        let name = self.guide_sets.iter().find_map(|l| l.set.as_ref().ok().filter(|s| s.shortcut == Some(n)).map(|s| s.name.clone()));
        match name {
            Some(name) => {
                self.toggle_guide_set(&name);
                let shown = self.shown_guide_sets.contains(&name);
                self.notify(crate::i18n::fmt(
                    if shown { tl!("Guide set “{name}” shown") } else { tl!("Guide set “{name}” hidden") },
                    &[("name", &name)],
                ));
            }
            None => self.notify(crate::i18n::fmt(tl!("No guide set has the shortcut {n}"), &[("n", &n.to_string())])),
        }
    }

    /// Save the active document's guides as a set named `name` in the folder: the guides, the
    /// unit, and the grid when `with_grid`.
    pub(crate) fn save_guide_set(&mut self, name: &str, shortcut: Option<u8>, with_grid: bool) -> Result<std::path::PathBuf, String> {
        let view = self.active.and_then(|i| self.views.get(i)).ok_or_else(|| tl!("Open a document first").to_string())?;
        if view.guides.guides.is_empty() && !with_grid {
            return Err(tl!("This document has no guides to save").to_string());
        }
        let dir = self.guide_sets_dir.clone().ok_or_else(|| tl!("Guide sets can't be saved in the web version").to_string())?;
        let set = guide_sets::from_document(name, &view.guides.guides, self.units, shortcut, with_grid.then_some(self.grid));
        let path = guide_sets::save(&dir, &set)?;
        // Another set keeps its shortcut only if it has a different one.
        self.reload_guide_sets();
        Ok(path)
    }

    /// Copy a saved set into the active document's guides, replacing them, so they can be moved and edited.
    pub(crate) fn apply_guide_set(&mut self, name: &str) -> bool {
        let Some(i) = self.active else { return false };
        let sizes = self.page_sizes(i);
        let Some(set) = self.guide_sets.iter().filter_map(|l| l.set.as_ref().ok()).find(|s| s.name == name).cloned() else { return false };
        let Some(view) = self.views.get_mut(i) else { return false };
        let mut copied = guides::Guides::default();
        guide_sets::apply_to_document(&set, &mut copied, &sizes);
        view.guides.replace(copied);
        self.shown_guide_sets.remove(name);
        // A complete setup: its grid and unit become the current ones.
        self.units = set.unit;
        if let Some(grid) = set.grid {
            self.grid = grid.sanitized();
            self.show_grid = true;
        }
        true
    }

    /// Guides, grid and guide sets for the control channel's `ui.state`: what an agent needs to
    /// check guides (where each lies on screen) and to change them (the guide-set schema, sent back
    /// with `ui.set document-guides`).
    pub(crate) fn guides_state(&self) -> serde_json::Value {
        let view = self.active.and_then(|i| self.views.get(i));
        serde_json::json!({
            "show_rulers": self.show_rulers,
            "show_guides": self.show_guides,
            "show_grid": self.show_grid,
            "snap_to_grid": self.snap_to_grid,
            "units": self.units.label(),
            "grid": self.grid,
            "document": view.map(|v| guides::state_json(v, self.units)),
            "folder": self.guide_sets_dir.as_ref().map(|d| d.display().to_string()),
            "sets": self.guide_sets.iter().map(|l| serde_json::json!({
                "name": l.name(),
                "file": l.path.display().to_string(),
                "shown": self.shown_guide_sets.contains(&l.name()),
                "shortcut": l.set.as_ref().ok().and_then(|s| s.shortcut),
                "guides": l.set.as_ref().map(|s| s.guides.len()).ok(),
                "error": l.set.as_ref().err(),
            })).collect::<Vec<_>>(),
        })
    }

    /// View ▸ Save guides as a set: a small window asking for the set's name and shortcut.
    pub(crate) fn guide_set_window(&mut self, ctx: &egui::Context) {
        let Some((mut name, mut shortcut, mut with_grid)) = self.guide_set_draft.take() else { return };
        let mut open = true;
        let (mut save, mut cancel) = (false, false);
        let taken: Vec<(u8, String)> =
            self.guide_sets.iter().filter_map(|l| l.set.as_ref().ok()).filter_map(|s| s.shortcut.map(|n| (n, s.name.clone()))).collect();
        egui::Window::new(tl!("Save guides as a set")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(tl!("Name"));
            let field = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = true;
            }
            ui.label(tl!("Shortcut"));
            craft_ui::ComboBox::from_id_salt("guide-set-shortcut")
                .selected_text(shortcut.map_or_else(|| tl!("None").to_string(), |n| guides::set_shortcut_label(n, ctx)))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut shortcut, None, tl!("None"));
                    for n in 1..=9u8 {
                        let used = taken.iter().find(|(k, other)| *k == n && *other != name.trim()).map(|(_, o)| format!(" — {o}")).unwrap_or_default();
                        ui.selectable_value(&mut shortcut, Some(n), format!("{}{used}", guides::set_shortcut_label(n, ctx)));
                    }
                });
            ui.add(crate::widgets::checkbox(&mut with_grid, tl!("Include the grid settings")));
            ui.label(
                egui::RichText::new(tl!(
                    "Saves the guides, the unit and, if ticked, the grid as one set: a text file in the guide-set folder that can be edited, copied and shared."
                ))
                .small(),
            );
            ui.horizontal(|ui| {
                save |= ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tl!("Save"))).clicked();
                cancel = ui.button(tl!("Cancel")).clicked();
            });
        });
        if save && !name.trim().is_empty() {
            match self.save_guide_set(&name, shortcut, with_grid) {
                Ok(path) => self.notify(crate::i18n::fmt(tl!("Saved guide set to {path}"), &[("path", &path.display().to_string())])),
                Err(e) => self.notify(e),
            }
        } else if open && !cancel {
            self.guide_set_draft = Some((name, shortcut, with_grid));
        }
    }
}
