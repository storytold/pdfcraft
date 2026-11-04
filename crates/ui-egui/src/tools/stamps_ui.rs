//! Custom stamps (Add a stamp ▸ Custom stamps ▸ Create): a PDF page or an image saved in the
//! user's stamp library under a category and a name, then placed like any stamp.

use std::sync::Arc;

use egui::{Align, Layout};
use serde::{Deserialize, Serialize};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, QuickTool, widgets};

/// Stamp files larger than this aren't kept in the library (it lives in the app's settings).
pub const MAX_STAMP_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomStamp {
    /// Stable library identity; unlike a vector position it survives sorting and edits.
    #[serde(default)]
    pub id: u64,
    pub category: String,
    pub name: String,
    /// The source file's name (its extension tells images from PDFs).
    pub file: String,
    /// The page of a PDF (0-based).
    #[serde(default)]
    pub page: usize,
    #[serde(with = "b64")]
    pub data: Arc<Vec<u8>>,
}

mod b64 {
    use std::sync::Arc;

    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v.as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map(Arc::new).map_err(serde::de::Error::custom)
    }
}

/// The Create Custom Stamp dialog: the chosen file, its category and name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StampDraft {
    pub file: String,
    pub data: Arc<Vec<u8>>,
    pub category: String,
    pub name: String,
}

pub(crate) fn create_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let categories: Vec<String> = {
        let mut c: Vec<String> = app.custom_stamps.iter().map(|s| s.category.clone()).collect();
        c.sort();
        c.dedup();
        c
    };
    let d = &mut app.stamp_draft;
    ui.label(egui::RichText::new(tl!("Create Custom Stamp")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("From {file}"), &[("file", &d.file)])).color(t.text_muted));
    ui.add_space(8.0);
    egui::Grid::new("stamp-create").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        let l = ui.label(tl!("Category:"));
        ui.horizontal(|ui| {
            ui.add(crate::widgets::line(&mut d.category).desired_width(200.0).hint_text(tl!("e.g. My stamps"))).labelled_by(l.id);
            if !categories.is_empty() {
                craft_ui::ComboBox::from_id_salt("stamp-categories").selected_text("").width(24.0).show_ui(ui, |ui| {
                    for c in &categories {
                        if ui.selectable_label(d.category == *c, c).clicked() {
                            d.category = c.clone();
                        }
                    }
                });
            }
        });
        ui.end_row();
        let l = ui.label(tl!("Name:"));
        ui.add(crate::widgets::line(&mut d.name).desired_width(200.0)).labelled_by(l.id);
        ui.end_row();
    });
    ui.add_space(12.0);
    let ok = !d.category.trim().is_empty() && !d.name.trim().is_empty();
    let (mut save, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
            save = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (save, cancel)
}

pub(crate) fn edit_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) -> (bool, bool) {
    let Some(edit) = app.stamp_edit.as_mut() else { return (false, true) };
    ui.label(egui::RichText::new(tl!("Edit Custom Stamp")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("stamp-edit").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Category:"));
        ui.add(crate::widgets::line(&mut edit.category).desired_width(240.0));
        ui.end_row();
        ui.label(tl!("Name:"));
        ui.add(crate::widgets::line(&mut edit.name).desired_width(240.0));
        ui.end_row();
    });
    ui.add_space(12.0);
    let valid = !edit.name.trim().is_empty() && !edit.category.trim().is_empty();
    let (mut save, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(valid, |ui| widgets::pill_button(ui, tl!("Save"), true)).inner.clicked() {
            save = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    if cancel {
        app.stamp_edit = None;
    }
    (save, cancel)
}

/// The Custom stamps section of the stamps palette.
pub(crate) fn palette_section(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    widgets::section_title(ui, tl!("Custom"));
    let mut remove = None;
    let mut categories: Vec<String> = app.custom_stamps.iter().map(|s| s.category.clone()).collect();
    categories.sort();
    categories.dedup();
    for c in categories {
        ui.label(egui::RichText::new(&c).small().color(t.text_muted));
        for s in app.custom_stamps.iter().filter(|s| s.category == c) {
            let active = app.quick_tool == QuickTool::CustomStamp(s.id);
            let resp = ui.add(egui::Button::selectable(active, s.name.as_str()).min_size(egui::vec2(ui.available_width(), 28.0)));
            if resp.clicked() {
                app.quick_tool = QuickTool::CustomStamp(s.id);
            }
            resp.context_menu(|ui| {
                if ui.button(tl!("Rename / change category…")).clicked() {
                    app.stamp_edit = Some(StampEdit { id: s.id, name: s.name.clone(), category: s.category.clone() });
                    app.dialog = Some(Dialog::EditStamp);
                    ui.close();
                }
                if ui.button(tl!("Delete stamp")).clicked() {
                    remove = Some(s.id);
                    ui.close();
                }
            });
        }
    }
    if let Some(id) = remove {
        app.custom_stamps.retain(|s| s.id != id);
        if app.quick_tool == QuickTool::CustomStamp(id) {
            app.quick_tool = QuickTool::Select;
        }
    }
    ui.add_space(4.0);
    if widgets::pill_button(ui, tl!("Create custom stamp…"), false).on_hover_text(tl!("From a PDF page or an image")).clicked() {
        app.pick_stamp_file();
    }
}

impl PdfCraftApp {
    /// Create ▸ choose a PDF or an image for a new custom stamp.
    pub(crate) fn pick_stamp_file(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let read = |app: &mut Self, path: std::path::PathBuf| match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    app.start_custom_stamp(name, bytes);
                }
                Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) if [".png", ".jpg", ".pdf"].iter().any(|e| p.ends_with(e)) => read(self, p.into()),
                Some(_) => {}
                None => {
                    let dialog = rfd::AsyncFileDialog::new()
                        .add_filter(tl!("PDF or image"), &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
                        .set_title(tl!("Select a file for the stamp"));
                    self.ask_one(crate::pickers::Ask::File(dialog), None, read);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Custom stamps arrive on the web with file pickers for images");
    }

    /// Open the Create Custom Stamp dialog for a file.
    pub fn start_custom_stamp(&mut self, file: String, bytes: Vec<u8>) {
        if bytes.len() > MAX_STAMP_BYTES {
            self.notify_fmt("{file} is too large for a stamp (at most {n} MB)", &[("file", &file), ("n", &(MAX_STAMP_BYTES >> 20).to_string())]);
            return;
        }
        let name = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s).to_string();
        let category = self.custom_stamps.last().map(|s| s.category.clone()).unwrap_or_else(|| "My stamps".into());
        self.stamp_draft = crate::stamps_ui::StampDraft { file, data: Arc::new(bytes), category, name };
        self.dialog = Some(Dialog::CreateStamp);
    }

    /// Save the dialog's stamp in the library and choose it.
    pub(crate) fn save_custom_stamp(&mut self) {
        let id = match new_stamp_id(&mut self.next_custom_stamp_id, &self.custom_stamps) {
            Ok(id) => id,
            Err(error) => {
                self.notify(error);
                return;
            }
        };
        let d = std::mem::take(&mut self.stamp_draft);
        self.custom_stamps.push(CustomStamp {
            id,
            category: d.category.trim().into(),
            name: d.name.trim().into(),
            file: d.file,
            page: 0,
            data: d.data,
        });
        self.quick_tool = QuickTool::CustomStamp(id);
    }

    /// Rename a saved stamp or move it to another category without changing its source bytes.
    pub fn edit_custom_stamp(&mut self, id: u64, name: &str, category: &str) -> Result<(), String> {
        let name = name.trim();
        let category = category.trim();
        if name.is_empty() || category.is_empty() {
            return Err("stamp name and category must not be empty".into());
        }
        let stamp = self.custom_stamps.iter_mut().find(|stamp| stamp.id == id).ok_or_else(|| "custom stamp no longer exists".to_string())?;
        stamp.name = name.into();
        stamp.category = category.into();
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StampEdit {
    pub id: u64,
    pub name: String,
    pub category: String,
}

fn new_stamp_id(next_id: &mut u64, stamps: &[CustomStamp]) -> Result<u64, String> {
    if *next_id == 0 {
        return Err("custom stamp library has no available identity".into());
    }
    let floor = match stamps.iter().map(|stamp| stamp.id).max() {
        Some(max) => match max.checked_add(1) {
            Some(next) => next,
            None => {
                *next_id = 0;
                return Err("custom stamp library has no available identity".into());
            }
        },
        None => 1,
    };
    let mut candidate = (*next_id).max(floor);
    while stamps.iter().any(|stamp| stamp.id == candidate) {
        candidate = candidate.checked_add(1).ok_or_else(|| "custom stamp library has no available identity".to_string())?;
    }
    *next_id = candidate.checked_add(1).unwrap_or_default();
    Ok(candidate)
}

/// The custom stamps for `persist` (kept small: base64 of each file).
pub(crate) fn encode(stamps: &[CustomStamp]) -> serde_json::Value {
    serde_json::to_value(stamps).unwrap_or_default()
}

/// Read stamps and the identity counter back; malformed entries are dropped.
pub(crate) fn decode(v: &serde_json::Value, saved_next_id: Option<u64>) -> (Vec<CustomStamp>, u64) {
    let stamps: Vec<CustomStamp> = v
        .as_array()
        .map(|a| a.iter().filter_map(|x| serde_json::from_value::<CustomStamp>(x.clone()).ok()).filter(|s| s.data.len() <= MAX_STAMP_BYTES).collect())
        .unwrap_or_default();
    let reserved: std::collections::HashSet<u64> = stamps.iter().map(|stamp| stamp.id).filter(|id| *id != 0).collect();
    let mut ids = std::collections::HashSet::new();
    let mut fallback = 1u128;
    let mut repaired = Vec::with_capacity(stamps.len());
    for mut stamp in stamps {
        if stamp.id == 0 || ids.contains(&stamp.id) {
            let replacement = loop {
                let Ok(candidate) = u64::try_from(fallback) else { break None };
                fallback = fallback.saturating_add(1);
                if !reserved.contains(&candidate) && !ids.contains(&candidate) {
                    break Some(candidate);
                }
            };
            let Some(id) = replacement else { continue };
            stamp.id = id;
        }
        ids.insert(stamp.id);
        repaired.push(stamp);
    }
    let minimum_next_id = repaired.iter().map(|stamp| stamp.id).max().map(|id| id.checked_add(1).unwrap_or_default()).unwrap_or(1);
    let next_id = match saved_next_id {
        Some(0) => 0,
        Some(saved) if minimum_next_id != 0 => saved.max(minimum_next_id),
        Some(_) => 0,
        None => minimum_next_id,
    };
    (repaired, next_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_a_saved_stamp_keeps_its_selected_identity_and_source_bytes() {
        let mut app = PdfCraftApp::new();
        let bytes = Arc::new(vec![1, 2, 3]);
        app.custom_stamps = vec![
            CustomStamp { id: 101, category: "Old".into(), name: "Duplicate".into(), file: "one.png".into(), page: 0, data: bytes.clone() },
            CustomStamp { id: 202, category: "Other".into(), name: "Duplicate".into(), file: "two.png".into(), page: 0, data: Arc::new(vec![4]) },
        ];
        app.quick_tool = QuickTool::CustomStamp(101);
        app.edit_custom_stamp(101, "Renamed", "Moved").expect("valid edit");
        assert_eq!(app.custom_stamps[0].name, "Renamed");
        assert_eq!(app.custom_stamps[0].category, "Moved");
        assert!(Arc::ptr_eq(&app.custom_stamps[0].data, &bytes));
        app.custom_stamps.sort_by(|a, b| a.name.cmp(&b.name));
        assert!(app.custom_stamps.iter().any(|s| s.id
            == match app.quick_tool {
                QuickTool::CustomStamp(id) => id,
                _ => 0,
            }));
        app.custom_stamps.retain(|s| s.id != 202);
        assert!(app.custom_stamps.iter().any(|s| s.id == 101));
    }

    #[test]
    fn edits_refuse_blank_fields_and_missing_entries_without_mutating_the_library() {
        let mut app = PdfCraftApp::new();
        app.custom_stamps.push(CustomStamp {
            id: 77,
            category: "Tools".into(),
            name: "Mark".into(),
            file: "mark.png".into(),
            page: 0,
            data: Arc::new(vec![9]),
        });
        assert!(app.edit_custom_stamp(77, "  ", "New").is_err());
        assert!(app.edit_custom_stamp(77, "New", " ").is_err());
        assert!(app.edit_custom_stamp(88, "New", "New").is_err());
        assert_eq!(app.custom_stamps[0].name, "Mark");
        assert_eq!(app.custom_stamps[0].category, "Tools");
    }

    #[test]
    fn stamp_identity_survives_settings_round_trip_and_old_settings_get_ids() {
        let saved = [CustomStamp {
            id: u64::MAX - 1,
            category: "Tools".into(),
            name: "Mark".into(),
            file: "mark.png".into(),
            page: 0,
            data: Arc::new(vec![9]),
        }];
        let mut app = PdfCraftApp::new();
        app.custom_stamps = saved.to_vec();
        app.next_custom_stamp_id = u64::MAX;
        let settings = app.persist();
        let settings_value: serde_json::Value = serde_json::from_str(&settings).expect("settings are JSON");
        assert!(settings_value["custom_stamps"].is_array(), "older builds read custom_stamps as an array");
        assert_eq!(settings_value["custom_stamp_next_id"], u64::MAX);
        let mut restored_app = PdfCraftApp::new();
        restored_app.restore(&settings);
        assert_eq!(restored_app.custom_stamps[0].id, u64::MAX - 1);
        assert_eq!(restored_app.custom_stamps[0].data.as_slice(), &[9]);
        assert_eq!(restored_app.next_custom_stamp_id, u64::MAX);
        let old = serde_json::json!([{"category":"Tools","name":"Old","file":"old.png","page":0,"data":"CQ=="}]);
        let (restored, next_id) = decode(&old, None);
        assert_eq!(restored[0].id, 1);
        assert_eq!(next_id, 2);
    }

    #[test]
    fn saving_without_an_available_identity_keeps_the_stamp_draft() {
        let mut app = PdfCraftApp::new();
        app.next_custom_stamp_id = 0;
        app.stamp_draft = StampDraft { file: "mark.png".into(), data: Arc::new(vec![1, 2]), category: "Tools".into(), name: "Mark".into() };

        app.save_custom_stamp();

        assert_eq!(app.stamp_draft.file, "mark.png");
        assert_eq!(app.stamp_draft.category, "Tools");
        assert_eq!(app.stamp_draft.name, "Mark");
        assert_eq!(app.stamp_draft.data.as_slice(), &[1, 2]);
        assert!(app.custom_stamps.is_empty());
    }

    #[test]
    fn new_stamp_ids_remain_monotonic_after_deletion_and_settings_restore() {
        let make = |id| CustomStamp { id, category: String::new(), name: String::new(), file: String::new(), page: 0, data: Arc::new(Vec::new()) };
        let mut next_id = 1;
        assert_eq!(new_stamp_id(&mut next_id, &[]), Ok(1));
        assert_eq!(new_stamp_id(&mut next_id, &[make(1)]), Ok(2));
        let remaining = [make(2)];
        let (restored, mut next_id) = decode(&encode(&remaining), Some(next_id));
        assert_eq!(new_stamp_id(&mut next_id, &restored), Ok(3));
        let mut exhausted = 0;
        assert!(new_stamp_id(&mut exhausted, &[make(u64::MAX)]).is_err());
    }
}
