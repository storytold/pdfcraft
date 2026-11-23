//! Rename… on a document tab (#2102, #2867): rename the file on disk, in its own folder, without
//! closing it. The tab, the undo history and the view stay as they are; Recent files, Reopen Closed
//! Tab and the file watcher follow the new name. A document with unsaved changes is saved first
//! ("Save and Rename"), so the renamed file is always the version on screen (Acrobat on macOS asks
//! the same). The name's rules are the engine's ([`pdfcraft_engine::new_file_name`]).

use std::path::PathBuf;

use craft::theme::variant::Tone;
use craft::ui::contract::{Bind, Show as _};
use craft::ui::dialog::dialog_id;
use craft::ui::display::{Shade, Text};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use pdfcraft_engine::DocId;

use crate::PdfCraftApp;
use crate::editing::SaveTarget;

/// The dialog's key.
const KEY: &str = "file.rename";

/// The Rename dialog's document and the name it opens with; while the dialog is open, the
/// document it renames.
pub(crate) struct RenameDraft {
    pub id: DocId,
    pub name: String,
}

/// Where a "Save and Rename" hands the typed name back when its save does not go through.
pub(crate) type Returned = std::sync::Arc<std::sync::Mutex<Option<RenameDraft>>>;

/// Held by the step that follows the save. If that step is dropped without running (the save
/// failed, the Save As picker was cancelled, the overwrite question was declined), the name goes
/// back to the app so the dialog can open again with it.
struct Pending {
    draft: Option<RenameDraft>,
    back: Returned,
}

impl Pending {
    /// The save went through: the rename takes the name.
    fn take(&mut self) -> Option<RenameDraft> {
        self.draft.take()
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(draft) = self.draft.take() {
            *self.back.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(draft);
        }
    }
}

/// What the dialog shows about the document it renames, fixed when it opens.
struct RenameFacts {
    /// The file's path now.
    path: PathBuf,
    /// The document's name, for the title.
    current: String,
    /// Whether the document has unsaved work, so the rename saves first.
    unsaved: bool,
}

impl RenameFacts {
    /// The dialog over the values: the name row, and under it the new file name or why the name
    /// can't be used, checked as it is typed. Rename waits for a usable name; Enter is Rename.
    fn dialog<'b>(&self, fields: &'b mut DialogFields) -> ParamDialog<'b> {
        let typed = fields.text("name").unwrap_or_default().to_owned();
        let check = pdfcraft_engine::rename_target(&self.path, &typed);
        let ready = matches!(check, Ok(Some(_)));
        let (line, refused) = match &check {
            Err(e) => (Some(e.to_string()), true),
            Ok(Some(target)) => (target.file_name().map(|n| n.to_string_lossy().into_owned()), false),
            Ok(None) => (None, false),
        };
        let unsaved = self.unsaved;
        let title = crate::i18n::fmt(tl!("Rename “{name}”"), &[("name", &self.current)]);
        let ok = if unsaved { tl!("Save and Rename") } else { tl!("Rename") };
        let params = vec![Param::new("name", tl!("File name"), ParamKind::text("")).hint(tl!("File name")).full_width()];
        let reason = line.clone().filter(|_| refused);
        let mut dialog =
            ParamDialog::new(KEY, title, params).values(Bind::bound(fields)).ok_label(ok.into()).ok_enabled(ready).below(move |ui, _| {
                match &line {
                    Some(line) if refused => {
                        Text::new(line.clone()).tone(Tone::Bad).show(ui);
                    }
                    Some(line) => {
                        Text::new(line.clone()).shade(Shade::Muted).show(ui);
                    }
                    None => {}
                }
                if unsaved {
                    Text::new(tl!("This document has unsaved changes. They are saved first, then the file is renamed.")).show(ui);
                }
            });
        if let Some(reason) = reason {
            dialog = dialog.ok_reason(reason.into());
        }
        dialog
    }
}

impl PdfCraftApp {
    /// Opens Rename again with the name that was typed, once a save that was to precede the
    /// rename has come to nothing. Called every frame; waits while another dialog is open.
    pub(crate) fn reopen_returned_rename(&mut self, ctx: &egui::Context) {
        if self.modal_open() {
            return;
        }
        let Some(draft) = self.rename_returned.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { return };
        if self.views.iter().any(|v| v.id == draft.id) {
            self.open_rename(ctx, draft);
        }
    }

    /// Rename… for document `id` (tab menu, double-click on the tab): the dialog opens with the
    /// name, without its extension. Nothing happens for a document that has no file.
    pub(crate) fn start_rename(&mut self, ctx: &egui::Context, id: DocId) {
        let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) else { return };
        let name = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.open_rename(ctx, RenameDraft { id, name });
    }

    /// Opens the Rename dialog for `draft`'s document with its name in the field.
    fn open_rename(&mut self, ctx: &egui::Context, draft: RenameDraft) {
        let Some(doc) = self.session.get(draft.id) else { return };
        let Some(path) = doc.path.clone() else { return };
        let current = doc.name.clone();
        let unsaved = self.views.iter().position(|v| v.id == draft.id).is_some_and(|i| self.has_unsaved_work(i));
        let facts = RenameFacts { path: PathBuf::from(path), current, unsaved };
        let mut fields = DialogFields::new();
        fields.set("name", draft.name.as_str());
        self.rename_draft = Some(draft);
        let dialog = HostedParamDialog::new(ctx, KEY, fields, move |fields| facts.dialog(fields));
        crate::dialogs::host::open(self, dialog);
    }

    /// Takes the Rename dialog's answer: Rename (or Save and Rename) renames with the typed name
    /// once the dialog has closed, so a save it starts may ask about a file changed on disk.
    pub(crate) fn rename_pass(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(KEY)) else { return };
        match answer.ok() {
            Some(result) => {
                if let Some(draft) = self.rename_draft.as_mut() {
                    draft.name = result.fields.text("name").unwrap_or_default().to_owned();
                }
                self.rename_with_draft(ctx);
            }
            None => self.rename_draft = None,
        }
    }

    /// The dialog's Rename (or Save and Rename).
    pub(crate) fn rename_with_draft(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.rename_draft.take() else { return };
        let Some(index) = self.views.iter().position(|v| v.id == draft.id) else { return };
        if self.has_unsaved_work(index) {
            // Saved in place first; the rename follows only if the save went through. Every way the
            // save comes to nothing drops `after`, and with it `pending`, which hands the name back.
            let mut pending = Pending { draft: Some(draft), back: self.rename_returned.clone() };
            self.save_then(
                ctx,
                index,
                SaveTarget::InPlace,
                move |app, _| {
                    if let Some(draft) = pending.take() {
                        app.rename_now(draft.id, &draft.name);
                    }
                },
                |_, _| {},
            );
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
                self.watch.known_now(id, std::path::Path::new(&new));
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

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::KEY;
    use crate::PdfCraftApp;
    use craft::ui::dialog::dialog_id;
    use craft::ui::patterns::form_dialogs::HostedParamDialog;
    use egui::accesskit::Role;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    const PDF: &[u8] = include_bytes!("../../tests/data/form.pdf");

    /// An English app with one document named `name`, opened from `file` on disk (with no file when `None`).
    fn harness(file: Option<&std::path::Path>, name: &'static str) -> Harness<'static, PdfCraftApp> {
        let path = file.map(|f| {
            std::fs::write(f, PDF).unwrap();
            f.to_string_lossy().into_owned()
        });
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            PdfCraftApp::install_runtime(&cc.egui_ctx);
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app.open_bytes(&cc.egui_ctx, name, path.clone(), PDF.to_vec()).unwrap();
            app
        });
        h.run_steps(8);
        h
    }

    /// Rename… from the menu of the tab whose name starts with `tab` (an edited tab says so after its name).
    fn open_rename(h: &mut Harness<'static, PdfCraftApp>, tab: &str) {
        h.get_by(|n| n.role() == Role::Tab && n.label().is_some_and(|l| l.starts_with(tab))).click_secondary();
        h.run_steps(2);
        h.get_by_label("Rename…").click();
        h.run_steps(6);
    }

    /// Types `name` into the dialog's field, as automation sets it.
    fn type_name(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
        let ctx = h.ctx.clone();
        h.state_mut().dialogs.set_json_field(&ctx, dialog_id(KEY), "name", serde_json::json!(name)).unwrap();
        h.run_steps(3);
    }

    /// The name in the open Rename dialog's field; `None` while it is closed.
    fn shown_name(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
        h.state().dialogs.get_by_id::<HostedParamDialog>(dialog_id(KEY)).map(|d| d.fields().text("name").unwrap_or_default().to_owned())
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-rename-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rename_from_the_tab_menu_moves_the_file_and_keeps_the_tab() {
        let dir = scratch("menu");
        std::fs::write(dir.join("taken.pdf"), b"other").unwrap();
        let mut h = harness(Some(&dir.join("first.pdf")), "first.pdf");
        let id = h.state().views[0].id;
        open_rename(&mut h, "first.pdf");
        assert_eq!(shown_name(&h).as_deref(), Some("first"), "the name opens without its extension");
        // Refusals show under the field and keep Rename disabled.
        type_name(&mut h, "taken");
        h.get_by_label_contains("already exists");
        assert!(h.get_by_role_and_label(Role::Button, "Rename").accesskit_node().is_disabled());
        type_name(&mut h, "CON");
        h.get_by_label_contains("reserves");
        type_name(&mut h, "Budget 2026");
        h.get_by_label("Budget 2026.pdf");
        h.get_by_role_and_label(Role::Button, "Rename").click();
        h.run_steps(4);
        assert!(dir.join("Budget 2026.pdf").exists() && !dir.join("first.pdf").exists());
        assert_eq!(h.state().views.len(), 1, "the tab stays");
        assert_eq!(h.state().views[0].id, id);
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.name, "Budget 2026.pdf");
        assert!(h.state().recent.iter().any(|r| r.name == "Budget 2026.pdf" && r.path.ends_with("Budget 2026.pdf")));
        assert!(!h.state().recent.iter().any(|r| r.path.ends_with("first.pdf")), "Recent files follow the rename");
        h.get_by_role_and_label(Role::Tab, "Budget 2026.pdf");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_changes_are_saved_before_the_rename() {
        let dir = scratch("save");
        let mut h = harness(Some(&dir.join("first.pdf")), "first.pdf");
        let id = h.state().views[0].id;
        let ctx = h.ctx.clone();
        h.state_mut().apply_edit(&ctx, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Saved first".into() });
        h.run_steps(3);
        assert!(h.state().session.get(id).unwrap().dirty);
        open_rename(&mut h, "first.pdf");
        type_name(&mut h, "second");
        h.get_by_label("This document has unsaved changes. They are saved first, then the file is renamed.");
        h.get_by_role_and_label(Role::Button, "Save and Rename").click();
        h.run_steps(6);
        assert!(h.state().rename_draft.is_none() && shown_name(&h).is_none());
        let doc = h.state().session.get(id).unwrap();
        assert!(!doc.dirty, "saved");
        assert_eq!(doc.name, "second.pdf");
        let saved = std::fs::read(dir.join("second.pdf")).unwrap();
        assert!(String::from_utf8_lossy(&saved).contains("Saved first"), "the renamed file is the saved version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_that_fails_keeps_the_typed_name() {
        let dir = scratch("fail");
        let mut h = harness(Some(&dir.join("first.pdf")), "first.pdf");
        let id = h.state().views[0].id;
        let ctx = h.ctx.clone();
        h.state_mut().apply_edit(&ctx, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved".into() });
        h.run_steps(3);
        open_rename(&mut h, "first.pdf");
        type_name(&mut h, "second");
        // The save cannot be written: the file's place is now a folder.
        std::fs::remove_file(dir.join("first.pdf")).unwrap();
        std::fs::create_dir(dir.join("first.pdf")).unwrap();
        h.get_by_role_and_label(Role::Button, "Save and Rename").click();
        h.run_steps(6);
        assert_eq!(shown_name(&h).as_deref(), Some("second"), "the dialog is back with the name that was typed");
        assert!(h.state().session.get(id).unwrap().dirty, "nothing was saved");
        assert!(dir.join("first.pdf").is_dir() && !dir.join("second.pdf").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_document_without_a_file_offers_no_rename() {
        let mut h = harness(None, "unsaved.pdf");
        h.get_by_role_and_label(Role::Tab, "unsaved.pdf").click_secondary();
        h.run_steps(2);
        assert!(h.get_by_label("Rename…").accesskit_node().is_disabled());
    }
}
