//! Which tools All tools, the ⌘K palette and Home's recommended tools list (Preferences ▸ Tools
//! shown in All tools). Hiding only tidies those lists: the palette still finds every command by
//! name, and the menus, the mode tabs, keyboard shortcuts, the control channel (`ui.set tool`) and
//! automation keep every tool, so nothing a user or a script relies on stops working.

use pdfcraft_engine::catalog::{self, TOOL_GROUPS, TOOL_PRESETS, ToolPreset};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The tools the user hid, as catalogue ids in catalogue order, each once. The hidden ones are
/// kept rather than the shown ones, so a tool a later version adds shows up instead of being
/// missing without anyone having chosen that.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HiddenTools(Vec<&'static str>);

impl HiddenTools {
    /// The known tool ids among `ids`, once each and in catalogue order. Anything else is dropped,
    /// and nothing is allocated per input id (settings files are untrusted).
    pub fn from_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Self {
        let mut hide = [false; TOOL_GROUPS.len()];
        for id in ids {
            if let Some(slot) = TOOL_GROUPS.iter().position(|g| g.id == id).and_then(|i| hide.get_mut(i)) {
                *slot = true;
            }
        }
        Self(TOOL_GROUPS.iter().zip(hide).filter(|(_, hidden)| *hidden).map(|(g, _)| g.id).collect())
    }

    /// What a preset hides.
    pub fn of_preset(preset: &ToolPreset) -> Self {
        Self::from_ids(preset.hidden())
    }

    /// The `ui.set hidden-tools` value: comma-separated tool ids, or `none`. An unknown id is an
    /// error, so a typo doesn't quietly show a tool the caller meant to hide.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value == "none" {
            return Ok(Self::default());
        }
        if value.split(',').any(|id| id.trim().is_empty()) {
            return Err("hidden-tools takes none or a comma-separated list of tool ids; an empty entry is not allowed".to_string());
        }
        if let Some(unknown) = value.split(',').map(str::trim).find(|id| catalog::group(id).is_none()) {
            let known: Vec<&str> = TOOL_GROUPS.iter().map(|g| g.id).collect();
            return Err(format!("unknown tool {unknown:?}: hidden-tools takes none or a comma-separated list of {}", known.join(", ")));
        }
        Ok(Self::from_ids(value.split(',').map(str::trim)))
    }

    /// Reads the saved list. `None` when the value isn't a list (`restore` then hides nothing);
    /// entries that aren't known tool ids are dropped.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        value.as_array().map(|ids| Self::from_ids(ids.iter().filter_map(serde_json::Value::as_str)))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::from(self.0.clone())
    }

    pub fn ids(&self) -> &[&'static str] {
        &self.0
    }

    /// Whether the lists show the tool `id`.
    pub fn shows(&self, id: &str) -> bool {
        !self.0.contains(&id)
    }

    pub fn set_shown(&mut self, id: &str, shown: bool) {
        *self = Self::from_ids(self.0.iter().copied().filter(|hidden| *hidden != id).chain((!shown).then_some(id)));
    }

    /// The preset that hides exactly these tools, or `None` for a custom selection.
    pub fn preset(&self) -> Option<&'static ToolPreset> {
        catalog::preset_hiding(&self.0)
    }

    /// The preset's id, or `custom`, as `ui.state` reports it.
    pub fn preset_id(&self) -> &'static str {
        self.preset().map_or("custom", |p| p.id)
    }
}

/// A preset's name in the interface language.
fn preset_label(preset: &ToolPreset) -> &'static str {
    match preset.id {
        "all" => tl!("All tools"),
        "read-comment" => tl!("Read and comment"),
        "forms-sign" => tl!("Forms and signing"),
        other => other,
    }
}

/// The Preferences section: a preset, one checkbox per tool, and what hiding does.
pub(crate) fn preferences_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Tools shown in All tools")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Preset"));
        let current = app.hidden_tools.preset();
        let selected = current.map_or(tl!("Custom selection"), preset_label);
        egui::ComboBox::from_id_salt("tool-preset").selected_text(selected).show_ui(ui, |ui| {
            for preset in TOOL_PRESETS {
                if ui.selectable_label(current.is_some_and(|c| c.id == preset.id), preset_label(preset)).clicked() {
                    app.hidden_tools = HiddenTools::of_preset(preset);
                    ui.close();
                }
            }
        });
    });
    egui::Grid::new("shown-tools").num_columns(2).spacing([24.0, 2.0]).show(ui, |ui| {
        for (i, g) in TOOL_GROUPS.iter().enumerate() {
            let mut shown = app.hidden_tools.shows(g.id);
            if ui.add(widgets::checkbox(&mut shown, tl!(g.label))).changed() {
                app.hidden_tools.set_shown(g.id, shown);
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.label(
        egui::RichText::new(tl!("Hidden tools stay available from the menus, the mode tabs and keyboard shortcuts.")).small().color(t.text_muted),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saved_lists_keep_known_tools_once_in_catalogue_order() {
        let hidden = HiddenTools::from_json(&json!(["ocr", "redact", 7, null, "compress", "redact", "", "../x", {"a": 1}])).unwrap();
        assert_eq!(hidden.ids(), ["redact", "compress"]);
        assert_eq!(HiddenTools::from_json(&json!("compress")), None);
        assert_eq!(HiddenTools::from_json(&json!(null)), None);
        assert_eq!(HiddenTools::from_json(&json!({"compress": true})), None);
        assert_eq!(HiddenTools::from_json(&json!([])), Some(HiddenTools::default()));
    }

    #[test]
    fn a_huge_saved_list_is_bounded_by_the_catalogue() {
        let mut ids = vec![json!("compress"); 100_000];
        ids.extend((0..100_000).map(|i| json!(format!("tool{i}"))));
        let hidden = HiddenTools::from_json(&serde_json::Value::Array(ids)).unwrap();
        assert_eq!(hidden.ids(), ["compress"]);
        let everything = HiddenTools::from_ids(TOOL_GROUPS.iter().chain(TOOL_GROUPS).map(|g| g.id));
        assert_eq!(everything.ids().len(), TOOL_GROUPS.len());
    }

    #[test]
    fn the_control_value_rejects_unknown_tools() {
        assert_eq!(HiddenTools::parse("none"), Ok(HiddenTools::default()));
        assert_eq!(HiddenTools::parse(" compress , redact ").unwrap().ids(), ["redact", "compress"]);
        assert!(HiddenTools::parse("compress,ocr").unwrap_err().contains("\"ocr\""));
        assert!(HiddenTools::parse("").unwrap_err().contains("empty entry"));
        assert!(HiddenTools::parse("compress,").unwrap_err().contains("empty entry"));
    }

    #[test]
    fn showing_and_hiding_one_tool_keeps_the_rest() {
        let mut hidden = HiddenTools::default();
        hidden.set_shown("compress", false);
        hidden.set_shown("export", false);
        hidden.set_shown("compress", false);
        assert_eq!(hidden.ids(), ["export", "compress"]);
        hidden.set_shown("export", true);
        assert_eq!(hidden.ids(), ["compress"]);
        hidden.set_shown("no-such-tool", false);
        assert_eq!(hidden.ids(), ["compress"]);
    }

    #[test]
    fn the_preset_follows_the_list() {
        assert_eq!(HiddenTools::default().preset_id(), "all");
        let mut hidden = HiddenTools::of_preset(catalog::preset("read-comment").unwrap());
        assert_eq!(hidden.preset_id(), "read-comment");
        assert!(hidden.shows("comment") && !hidden.shows("compress"));
        hidden.set_shown("redact", true);
        assert_eq!(hidden.preset_id(), "custom");
        hidden.set_shown("redact", false);
        assert_eq!(hidden.preset_id(), "read-comment");
    }

    #[test]
    fn hidden_tools_survive_a_restart() {
        let mut app = PdfCraftApp::new();
        app.set_option("hidden-tools", "compress,scan").unwrap();
        let mut restored = PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(restored.hidden_tools.ids(), ["scan", "compress"]);
        // A settings file from before this preference, or with a broken value, hides nothing.
        let mut old = PdfCraftApp::new();
        old.restore(r#"{"hidden_tools": "compress"}"#);
        assert_eq!(old.hidden_tools, HiddenTools::default());
        // Even in an app that already hides tools.
        restored.restore(r#"{"hidden_tools": "compress"}"#);
        assert_eq!(restored.hidden_tools, HiddenTools::default());
    }
}
