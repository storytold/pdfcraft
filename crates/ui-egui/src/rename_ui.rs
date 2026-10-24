//! Rename… on a document tab (#2102, #2867): rename the file on disk, in its own folder, without
//! closing it. The tab, the undo history and the view stay as they are; Recent files, Reopen Closed
//! Tab and the file watcher follow the new name. A document with unsaved changes is saved first
//! ("Save and Rename"), so the renamed file is always the version on screen (Acrobat on macOS asks
//! the same). The name's rules are the engine's ([`pdfcraft_engine::new_file_name`]).

use egui::{Align, Layout};
use pdfcraft_engine::DocId;

use crate::{Dialog, PdfCraftApp, theme, widgets};

/// The Rename dialog's state: the document and the name being typed.
pub(crate) struct RenameDraft {
    pub id: DocId,
    pub name: String,
    /// Select the name and focus the field on the dialog's first frame.
    pub fresh: bool,
}

impl PdfCraftApp {
    /// Rename… for document `id` (tab menu, double-click on the tab): the dialog opens with the
    /// name selected, without its extension. Nothing happens for a document that has no file.
    pub(crate) fn start_rename(&mut self, id: DocId) {
        let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) else { return };
        let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.rename_draft = Some(RenameDraft { id, name, fresh: true });
        self.dialog = Some(Dialog::RenameFile);
    }

    /// The dialog's Rename (or Save and Rename).
    pub(crate) fn rename_with_draft(&mut self) {
        let Some(draft) = self.rename_draft.take() else { return };
        let Some(index) = self.views.iter().position(|v| v.id == draft.id) else { return };
        if self.has_unsaved_work(index) {
            // Saved in place first; the rename follows only if the save went through.
            let (id, name) = (draft.id, draft.name);
            self.save_then(index, crate::editing::SaveTarget::InPlace, move |app| app.rename_now(id, &name));
        } else {
            self.rename_now(draft.id, &draft.name);
        }
    }

    fn rename_now(&mut self, id: DocId, typed: &str) {
        let Some(old) = self.session.get(id).and_then(|d| d.path.clone()) else { return };
        match self.session.rename_file(id, typed) {
            Ok(new) if new != old => {
                let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
                for recent in self.recent.iter_mut().filter(|r| r.path == old) {
                    recent.path = new.clone();
                    recent.name = name.clone();
                }
                for closed in self.closed_tabs.iter_mut().filter(|p| **p == old) {
                    *closed = new.clone();
                }
                // The renamed file is the version the document came from: not a change on disk.
                #[cfg(not(target_arch = "wasm32"))]
                self.watch.known_now(id, &new);
                self.notify_fmt("Renamed to {name}", &[("name", &name)]);
            }
            Ok(_) => {}
            Err(e) => {
                let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't rename {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }
}

/// The Rename dialog. Returns `(close, rename)`.
pub(crate) fn body(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> (bool, bool) {
    let (mut close, mut go) = (false, false);
    let Some(doc_id) = app.rename_draft.as_ref().map(|d| d.id) else { return (true, false) };
    let Some((path, current)) = app.session.get(doc_id).and_then(|d| d.path.clone().map(|p| (p, d.name.clone()))) else { return (true, false) };
    let unsaved = app.views.iter().position(|v| v.id == doc_id).is_some_and(|i| app.has_unsaved_work(i));
    let Some(draft) = app.rename_draft.as_mut() else { return (true, false) };
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Rename “{name}”"), &[("name", &current)])).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let id = ui.make_persistent_id("rename-file-name");
    if draft.fresh {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let end = egui::text::CCursor::new(draft.name.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
        state.store(ui.ctx(), id);
    }
    let field = ui.add(egui::TextEdit::singleline(&mut draft.name).id(id).desired_width(f32::INFINITY).hint_text(tl!("File name")));
    if std::mem::take(&mut draft.fresh) {
        field.request_focus();
    }
    // Checked as it is typed: the reason shows under the field and Rename waits for a usable name.
    let check = pdfcraft_engine::rename_target(std::path::Path::new(&path), &draft.name);
    let t = theme::Tokens::get(ui.ctx());
    match &check {
        Err(e) => {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(e.to_string()).color(ui.visuals().error_fg_color).font(theme::regular(12.0)));
        }
        Ok(Some(target)) => {
            ui.add_space(4.0);
            let new = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(egui::RichText::new(new).color(t.text_muted).font(theme::regular(12.0)));
        }
        Ok(None) => {}
    }
    if unsaved {
        ui.add_space(8.0);
        ui.label(tl!("This document has unsaved changes. They are saved first, then the file is renamed."));
    }
    let ready = matches!(check, Ok(Some(_)));
    if ready && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        go = true;
        close = true;
    }
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let label = if unsaved { tl!("Save and Rename") } else { tl!("Rename") };
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            go = true;
            close = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            close = true;
        }
    });
    (close, go)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::PdfCraftApp;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    fn harness(dir: &std::path::Path) -> Harness<'static, PdfCraftApp> {
        let file = dir.join("first.pdf");
        std::fs::write(&file, include_bytes!("../tests/data/form.pdf")).unwrap();
        let path = file.to_string_lossy().into_owned();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("first.pdf", Some(path), include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            app
        });
        h.run_steps(8);
        h
    }

    fn open_rename(h: &mut Harness<'static, PdfCraftApp>, tab: &str) {
        h.get_by_role_and_label(egui::accesskit::Role::Tab, tab).click_secondary();
        h.run_steps(2);
        h.get_by_label("Rename…").click();
        h.run_steps(6);
    }

    fn type_name(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
        h.state_mut().rename_draft.as_mut().unwrap().name = name.to_string();
        h.run_steps(3);
    }

    #[test]
    fn rename_from_the_tab_menu_moves_the_file_and_keeps_the_tab() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-rename-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("taken.pdf"), b"other").unwrap();
        let mut h = harness(&dir);
        let id = h.state().views[0].id;
        open_rename(&mut h, "first.pdf");
        assert_eq!(h.state().rename_draft.as_ref().unwrap().name, "first", "the name opens without its extension");
        // Refusals show under the field and keep Rename disabled.
        type_name(&mut h, "taken");
        h.get_by_label_contains("already exists");
        assert!(h.get_by_label("Rename").accesskit_node().is_disabled());
        type_name(&mut h, "CON");
        h.get_by_label_contains("reserves");
        type_name(&mut h, "Budget 2026");
        h.get_by_label("Budget 2026.pdf");
        h.get_by_label("Rename").click();
        h.run_steps(4);
        assert!(dir.join("Budget 2026.pdf").exists() && !dir.join("first.pdf").exists());
        assert_eq!(h.state().views.len(), 1, "the tab stays");
        assert_eq!(h.state().views[0].id, id);
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.name, "Budget 2026.pdf");
        assert!(h.state().recent.iter().any(|r| r.name == "Budget 2026.pdf" && r.path.ends_with("Budget 2026.pdf")));
        assert!(!h.state().recent.iter().any(|r| r.path.ends_with("first.pdf")), "Recent files follow the rename");
        h.get_by_role_and_label(egui::accesskit::Role::Tab, "Budget 2026.pdf");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_changes_are_saved_before_the_rename() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-rename-ui-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = harness(&dir);
        let id = h.state().views[0].id;
        h.state_mut().apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Saved first".into() });
        h.run_steps(3);
        assert!(h.state().session.get(id).unwrap().dirty);
        open_rename(&mut h, "first.pdf (edited)"); // an edited tab says so to assistive technology
        type_name(&mut h, "second");
        h.get_by_label("This document has unsaved changes. They are saved first, then the file is renamed.");
        h.get_by_label("Save and Rename").click();
        h.run_steps(6);
        let doc = h.state().session.get(id).unwrap();
        assert!(!doc.dirty, "saved");
        assert_eq!(doc.name, "second.pdf");
        let saved = std::fs::read(dir.join("second.pdf")).unwrap();
        assert!(String::from_utf8_lossy(&saved).contains("Saved first"), "the renamed file is the saved version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_document_without_a_file_offers_no_rename() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("unsaved.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            app
        });
        h.run_steps(8);
        h.get_by_role_and_label(egui::accesskit::Role::Tab, "unsaved.pdf").click_secondary();
        h.run_steps(2);
        assert!(h.get_by_label("Rename…").accesskit_node().is_disabled());
    }
}
