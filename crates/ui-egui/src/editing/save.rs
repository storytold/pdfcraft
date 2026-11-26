use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use craft::dialogs::save::{SaveDocument, SaveStep};
use craft::host::{FileFilter, WriteIntent};
#[cfg(target_arch = "wasm32")]
use craft::host::{PickId, PickRequest, Picked};
#[cfg(not(target_arch = "wasm32"))]
use craft::ui::dialog::{DialogId, PromptChoice, StandardPrompt};
use pdfcraft_engine::DocId;

use super::PdfCraftApp;
#[cfg(not(target_arch = "wasm32"))]
use super::SaveTarget;

/// Where a save writes, on the web. The desktop's is the framework save flow's
/// (`craft::dialogs::save::SaveTarget`); craft-dialogs builds only natively (rfd's blocking panels,
/// threads), so the web keeps the same three answers here.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SaveTarget {
    /// File › Save: the document's own file; asks for one when it has none.
    InPlace,
    /// File › Save As: always asks; the document takes the new file.
    As,
    /// File › Save a Copy: always asks; the document keeps its own file.
    Copy,
}

/// A web save the browser's save picker holds ([`craft::host::Files::save`]): the bytes went with the
/// request, so the answer only says where they ended up.
#[cfg(target_arch = "wasm32")]
struct BrowserSave {
    ticket: PickId,
    bytes: Arc<Vec<u8>>,
}

/// What runs once a save has ended one way or the other, given the context of the pass it ends on.
type SaveThen = Box<dyn FnOnce(&mut PdfCraftApp, &egui::Context)>;

/// A save held back by [`Dialog::LockSignatures`](crate::Dialog::LockSignatures) (#2149).
pub struct PendingLock {
    pub id: DocId,
    target: SaveTarget,
    /// What was to run once the document is written (close its tab, quit).
    after: SaveThen,
    /// What runs when the save does not happen after all.
    // PLAN: a cancelled Lock signatures dialog drops this without running it (`dialogs.rs`, pdf-hub-w2): run it there
    failed: SaveThen,
}

/// The save under way. The framework's save flow runs one at a time, so one slot holds what the
/// request needs across the passes its picker or changed-on-disk question takes.
pub struct PendingSave {
    key: DocId,
    /// What runs once the document is written (close its tab, quit); never when the save fails
    /// or is cancelled.
    after: Option<SaveThen>,
    /// What runs when the save is cancelled, does not start or fails (a close that waited on it
    /// stops); never when it succeeds.
    failed: Option<SaveThen>,
    /// Whether the content held applied redactions, and whether it restored saved ones, when the
    /// save proposed its name: a change while the picker was open makes that name wrong.
    // PLAN: the decision names `(usize, usize)` counts; the rule compares these two facts, as the old continuation did (`save.rs:112-113`), so a further Apply alone does not ask again
    // The web asks nothing between the name and the write: its bytes go with the picker.
    #[cfg(not(target_arch = "wasm32"))]
    redactions: (bool, bool),
    /// The changed-on-disk question on screen, answered in [`PdfCraftApp::save_flow_pass`].
    #[cfg(not(target_arch = "wasm32"))]
    prompt: Option<DialogId>,
    /// The browser's save picker, on the web.
    #[cfg(target_arch = "wasm32")]
    browser: Option<BrowserSave>,
}

impl PendingSave {
    /// The changed-on-disk question is on screen or answered and not yet taken: the document must
    /// stay the version the user was asked about until the save takes the answer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn asking(&self) -> bool {
        self.prompt.is_some()
    }
}

impl PdfCraftApp {
    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, ctx: &egui::Context, target: SaveTarget) -> bool {
        match self.active {
            Some(i) => self.save_view(ctx, i, target),
            None => false,
        }
    }

    /// Save the document shown in tab `index`. Returns `true` if it was written now. When it has
    /// to ask (where to save, or whether to replace a file changed on disk) it returns `false`
    /// and saves on a later pass, once the user has answered.
    pub fn save_view(&mut self, ctx: &egui::Context, index: usize, target: SaveTarget) -> bool {
        self.save_then(ctx, index, target, |_, _| {}, |_, _| {})
    }

    /// [`Self::save_view`], then `after` once the document is written: now, or on a later pass
    /// when the user had to answer first. `after` never runs when the save fails or is cancelled;
    /// `failed` runs then, and when the save cannot start, and never otherwise.
    // PLAN: `editing/close.rs:160` (pdf-shell-w15) passes the new `failed` argument
    pub(crate) fn save_then(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        target: SaveTarget,
        after: impl FnOnce(&mut Self, &egui::Context) + 'static,
        failed: impl FnOnce(&mut Self, &egui::Context) + 'static,
    ) -> bool {
        // What's typed in a field is part of what's saved (#166).
        if let Some(id) = self.views.get(index).map(|v| v.id)
            && !self.commit_typing_in(ctx, id)
        {
            failed(self, ctx);
            return false;
        }
        let Some(id) = self.views.get(index).map(|v| v.id) else {
            failed(self, ctx);
            return false;
        };
        // A signature just placed with Fill & Sign is a removable comment unless it is flattened:
        // ask once whether to lock it into the page (#2149). Only while a Fill & Sign mark is still
        // there (undo may have taken it away), and never on a digitally signed document: locking
        // changes page content in an update, which validators report as a change after signing.
        let still_marked = self.session.get(id).is_some_and(|d| d.info.annotations.iter().any(|a| a.fill_sign));
        let signed = self.session.get(id).is_some_and(|d| d.is_signed());
        let answered = self.lock_answered.take() == Some(id);
        if self.ask_to_lock_signatures
            && !self.flatten_fill_sign_on_save
            && self.placed_signatures.contains(&id)
            && still_marked
            && !signed
            && !answered
        {
            crate::dialogs::signature::ask_lock(self, PendingLock { id, target, after: Box::new(after), failed: Box::new(failed) });
            return false;
        }
        let Some(doc) = self.session.get(id) else {
            failed(self, ctx);
            return false;
        };
        let (name, path, saved) = (doc.name.clone(), doc.path.clone(), doc.saved_bytes());
        // After Apply redactions, Save asks where and proposes the name with `_Redacted`.
        let redacted = self.views.get(index).is_some_and(|v| v.redactions_applied > 0);
        let not_delivered = self.views.get(index).is_some_and(|v| v.redactions_applied > v.redactions_downloaded);
        let name = if redacted { pdfcraft_engine::redacted_file_name(&name) } else { name };
        // Undo past a saved Apply redactions brings back content that the file on disk has
        // redacted: ask where instead of writing it over that file.
        let unredacted = self.views.get(index).is_some_and(restores_saved_redactions);
        // The restored content is not a redacted copy: propose the name without `_Redacted`, so
        // the dialog's default is not the redacted file.
        let name = if unredacted && !redacted { unredacted_file_name(&name) } else { name };
        // A path read back from a recovery sidecar is not written until a save confirms it:
        // anyone who can write the recovery folder can name any file there (#3923). An email
        // attachment's temporary copy asks too: that is the flow's own rule.
        let target = if not_delivered || unredacted || self.recovered_paths.contains(&id) { SaveTarget::As } else { target };
        let pending = PendingSave {
            key: id,
            after: Some(Box::new(after)),
            failed: Some(Box::new(failed)),
            #[cfg(not(target_arch = "wasm32"))]
            redactions: (redacted, unredacted),
            #[cfg(not(target_arch = "wasm32"))]
            prompt: None,
            #[cfg(target_arch = "wasm32")]
            browser: None,
        };
        self.start_save(ctx, pending, name, path, saved, target)
    }

    /// Hands the save to the framework's save flow: the picker, the changed-on-disk question, then the write.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_save(
        &mut self,
        ctx: &egui::Context,
        mut pending: PendingSave,
        name: String,
        path: Option<String>,
        saved: Arc<Vec<u8>>,
        target: SaveTarget,
    ) -> bool {
        let document = SaveDocument {
            key: pending.key,
            name,
            path: path.map(PathBuf::from),
            extension: "pdf".into(),
            filters: vec![FileFilter::new("PDF", &["pdf"])],
            purpose: "document".into(),
            saved: Some(saved),
        };
        let step = self.save_flow.start(self.host.files(), &self.watch, document, target);
        if step == SaveStep::Busy {
            self.notify_tr("Finish the save in progress before starting another.");
            if let Some(failed) = pending.failed.take() {
                failed(self, ctx);
            }
            return false;
        }
        self.pending_save = Some(pending);
        self.run_save_step(ctx, step)
    }

    /// The web's save: the document's own file goes to the page or the downloads now; anything else
    /// is written through the browser's save picker ([`craft::host::Files::save`], the platform's
    /// BrowserFiles), which falls back to a download, and answers on a later pass
    /// ([`Self::save_flow_pass`]). Nothing on the page can change behind the app's back, so there is
    /// no changed-on-disk question.
    #[cfg(target_arch = "wasm32")]
    fn start_save(
        &mut self,
        ctx: &egui::Context,
        mut pending: PendingSave,
        name: String,
        path: Option<String>,
        _saved: Arc<Vec<u8>>,
        target: SaveTarget,
    ) -> bool {
        let id = pending.key;
        if self.pending_save.is_some() {
            self.notify_tr("Finish the save in progress before starting another.");
            if let Some(failed) = pending.failed.take() {
                failed(self, ctx);
            }
            return false;
        }
        if target == SaveTarget::InPlace
            && let Some(path) = path
        {
            let saved = self.save_doc_to(ctx, id, Path::new(&path), target);
            let then = if saved { pending.after.take() } else { pending.failed.take() };
            if let Some(then) = then {
                then(self, ctx);
            }
            return saved;
        }
        let Some((_, bytes)) = self.document_bytes(ctx, id, target, None) else {
            if let Some(failed) = pending.failed.take() {
                failed(self, ctx);
            }
            return false;
        };
        let file_name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
        let mut request = PickRequest::save("document").file_name(file_name).suggested_extension("pdf");
        request.filters = vec![FileFilter::new("PDF", &["pdf"])];
        let ticket = self.host.files().save(request, bytes.to_vec());
        pending.browser = Some(BrowserSave { ticket, bytes });
        self.pending_save = Some(pending);
        false
    }

    /// The browser's save picker answered: where the bytes went, a failure or a cancel.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn save_flow_pass(&mut self, ctx: &egui::Context) {
        let Some(ticket) = self.pending_save.as_ref().and_then(|p| p.browser.as_ref()).map(|b| b.ticket) else { return };
        let Some(picked) = self.host.files().take_answer(ticket) else { return };
        let Some(mut pending) = self.pending_save.take() else { return };
        let Some(browser) = pending.browser.take() else { return };
        let id = pending.key;
        let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
        let saved = match picked {
            // The picker wrote the file the user chose; the document takes its name.
            Picked::Paths(paths) => match paths.first() {
                Some(path) => {
                    let saved = self.finish_save(ctx, id, browser.bytes, Some(path.to_string_lossy().into_owned()));
                    if saved {
                        self.notify_fmt("Saved {name}", &[("name", &short_name(path))]);
                    }
                    saved
                }
                None => false,
            },
            // A download names no file the app could write again: the document keeps its own.
            Picked::Downloaded(file) => {
                let saved = self.finish_save(ctx, id, browser.bytes, None);
                if saved {
                    self.notify_fmt("Downloaded {name}", &[("name", &file)]);
                }
                saved
            }
            Picked::Failed(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e)]);
                false
            }
            Picked::Cancelled | Picked::Unavailable(_) | Picked::Contents(_) => false,
        };
        let then = if saved { pending.after.take() } else { pending.failed.take() };
        if let Some(then) = then {
            then(self, ctx);
        }
    }

    /// Each pass: the save picker's answer, and the changed-on-disk question's. Neither is the
    /// frame's input: both are taken once from where they wait, so a second pass in a frame finds
    /// nothing the first took, and no answer is applied twice.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn save_flow_pass(&mut self, ctx: &egui::Context) {
        if let Some(step) = self.save_flow.take_pick(self.host.files(), &self.watch) {
            self.run_save_step(ctx, step);
        }
        let Some(prompt) = self.pending_save.as_ref().and_then(|p| p.prompt) else { return };
        let Some(result) = self.dialogs.take_by_id::<StandardPrompt>(prompt) else { return };
        if let Some(pending) = self.pending_save.as_mut() {
            pending.prompt = None;
        }
        let step = match result.ok().map(|answer| answer.choice) {
            // Replace: write over the version on disk.
            Some(PromptChoice::Proceed) => self.save_flow.replace(),
            // Save As…: leave that version alone and ask where to put this one.
            Some(PromptChoice::Alternate) => self.save_flow.save_elsewhere(self.host.files()),
            _ => match self.save_flow.cancel(self.host.files()) {
                Some(step) => step,
                None => return,
            },
        };
        self.run_save_step(ctx, step);
    }

    /// Carry out what the save flow asked for next. Returns `true` if the document was written.
    #[cfg(not(target_arch = "wasm32"))]
    fn run_save_step(&mut self, ctx: &egui::Context, step: SaveStep<DocId>) -> bool {
        match step {
            SaveStep::Write { key, path, target } => self.write_step(ctx, key, &path, target),
            SaveStep::AskChangedOnDisk { key, .. } => {
                // The file on disk is another version than the document came from (#2166).
                let name = self.session.get(key).map(|d| d.name.clone()).unwrap_or_default();
                // Production's wording and buttons: Save anyway is an ordinary answer, not a red Replace.
                let message = crate::i18n::fmt(
                    tl!("“{name}” changed on disk after it was opened or last saved. Saving now replaces that newer version with this one."),
                    &[("name", &name)],
                );
                let prompt = StandardPrompt::custom(message)
                    .title(tl!("Save over the changed file?").to_string())
                    .button(PromptChoice::Proceed, tl!("Save anyway").to_string())
                    .button(PromptChoice::Alternate, tl!("Save as…").to_string())
                    .button(PromptChoice::Cancel, tl!("Cancel").to_string())
                    // Production's order: Save as… between Cancel and Save anyway.
                    .alternate_role(craft::ui::sections::DialogRole::Alternative);
                let prompt = crate::dialogs::host::open(self, prompt);
                if let Some(pending) = self.pending_save.as_mut().filter(|p| p.key == key) {
                    pending.prompt = Some(prompt);
                }
                false
            }
            SaveStep::Cancelled { key } => {
                if let Some(failed) = self.pending_save.take_if(|p| p.key == key).and_then(|p| p.failed) {
                    failed(self, ctx);
                }
                false
            }
            SaveStep::Picking | SaveStep::Busy => false,
        }
    }

    /// The flow's Write step: write document `id` to `path`, then run what waited on the save.
    #[cfg(not(target_arch = "wasm32"))]
    fn write_step(&mut self, ctx: &egui::Context, id: DocId, path: &Path, target: SaveTarget) -> bool {
        // An Apply redactions or an Undo of one while the picker was open makes the name it
        // proposed wrong: ask again with the name for the current content.
        let asked = self.pending_save.as_ref().filter(|p| p.key == id).map(|p| p.redactions);
        if let Some(asked) = asked
            && let Some(index) = self.views.iter().position(|v| v.id == id && (v.redactions_applied > 0, restores_saved_redactions(v)) != asked)
        {
            let (after, failed) = self.pending_save.take().map(|p| (p.after, p.failed)).unwrap_or_default();
            let _ = self.save_flow.cancel(self.host.files());
            return self.save_then(
                ctx,
                index,
                SaveTarget::As,
                move |app, ctx| {
                    if let Some(after) = after {
                        after(app, ctx);
                    }
                },
                move |app, ctx| {
                    if let Some(failed) = failed {
                        failed(app, ctx);
                    }
                },
            );
        }
        let saved = self.save_doc_to(ctx, id, path, target);
        if !saved {
            self.save_flow.failed(&id);
        }
        let pending = self.pending_save.take_if(|p| p.key == id);
        if saved {
            if let Some(after) = pending.and_then(|p| p.after) {
                after(self, ctx);
            }
        } else if let Some(failed) = pending.and_then(|p| p.failed) {
            failed(self, ctx);
        }
        saved
    }

    /// The Lock signatures dialog answered (#2149): lock the document's Fill & Sign marks into
    /// its pages first when `lock`, then save as was asked. With `remember`, the answer becomes
    /// the preference (flatten on every save, or never ask again). Nothing changes when locking
    /// fails: the document still counts as holding a placed signature, so the next save asks
    /// again, and the answer isn't remembered. The placed-signature mark stays until the save
    /// succeeds, so a save that fails or is cancelled asks again too.
    pub(crate) fn answer_lock_signatures(&mut self, ctx: &egui::Context, lock: bool, remember: bool) {
        let Some(p) = self.pending_lock.take() else { return };
        if lock && !self.flatten_fill_sign_on_save && !self.flatten_fill_sign_in(ctx, p.id) {
            (p.failed)(self, ctx);
            return;
        }
        self.lock_answered = Some(p.id);
        if remember {
            if lock {
                self.flatten_fill_sign_on_save = true;
            } else {
                self.ask_to_lock_signatures = false;
            }
        }
        if let Some(index) = self.views.iter().position(|v| v.id == p.id) {
            self.save_then(ctx, index, p.target, p.after, p.failed);
        } else {
            (p.failed)(self, ctx);
        }
    }

    /// Lock now, on the notice shown after a signature is placed (#2149): lock document `id`'s
    /// Fill & Sign marks into its pages, as the save-time question's Lock does but without
    /// saving. Undoable. The next save doesn't ask, as no Fill & Sign mark is left to lock; the
    /// placed-signature mark itself stays, so undoing the lock brings back both the notice and the
    /// save-time question. Never on a digitally signed document.
    pub(crate) fn lock_signatures_now(&mut self, ctx: &egui::Context, id: DocId) {
        if self.session.get(id).is_none_or(|d| d.is_signed()) {
            return;
        }
        if self.flatten_fill_sign_in(ctx, id) {
            self.notify_tr("Signature locked into the page.");
        }
    }

    /// Lock document `id`'s Fill & Sign marks into its pages (an undoable edit). Returns false,
    /// having told the user why, when that fails.
    fn flatten_fill_sign_in(&mut self, ctx: &egui::Context, id: DocId) -> bool {
        let before = self.session.get(id).map(|d| d.edit_generation());
        if let Err(e) = self.session.apply(id, pdfcraft_engine::Edit::FlattenFillSign) {
            self.notify_error(e);
            return false;
        }
        if before != self.session.get(id).map(|d| d.edit_generation())
            && let Some(doc) = self.session.get(id)
            && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
        {
            view.document_changed(ctx, &doc.info);
            // The marks left the comments: a selected one is gone, and its (page, index) could
            // now name another comment.
            view.session.comments.clear_selection();
        }
        true
    }

    /// Write document `id` to `dest` through the host's file service and, unless the target is
    /// the document's own file kept as it is, make `dest` the document's file. Returns `true` if
    /// written. The service writes atomically on the desktop and hands the bytes to the page or
    /// the browser's downloads on the web.
    fn save_doc_to(&mut self, ctx: &egui::Context, id: DocId, dest: &Path, target: SaveTarget) -> bool {
        let dest_text = dest.to_string_lossy().into_owned();
        let Some((_, bytes)) = self.document_bytes(ctx, id, target, Some(&dest_text)) else { return false };
        let intent = if target == SaveTarget::InPlace { WriteIntent::Save } else { WriteIntent::SaveAs };
        let to_page = match self.host.files().write(dest, &bytes, intent) {
            Ok(to_page) => to_page,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &dest_text), ("e", &e.to_string())]);
                return false;
            }
        };
        // Our own write is not a change on disk to reload (#431), and the flow is free again.
        #[cfg(not(target_arch = "wasm32"))]
        self.save_flow.written(&mut self.watch, id, dest);
        if !self.finish_save(ctx, id, bytes, Some(dest_text)) {
            return false;
        }
        // A page that took the bytes reports where they went itself; the browser's downloads say
        // nothing, so the notice does.
        if !to_page {
            let short = short_name(dest);
            if cfg!(target_arch = "wasm32") {
                self.notify_fmt("Downloaded {name}", &[("name", &short)]);
            } else {
                self.notify_fmt("Saved {name}", &[("name", &short)]);
            }
        }
        #[cfg(target_os = "macos")]
        crate::macos_association::repair(dest);
        true
    }

    /// The bytes a save of document `id` writes, with the document's name; `None` (with a notice
    /// when something failed) when nothing should be written. Commits what is typed and runs the
    /// document's Will Save actions first. `dest` is the file being written, when known.
    fn document_bytes(&mut self, ctx: &egui::Context, id: DocId, target: SaveTarget, dest: Option<&str>) -> Option<(String, Arc<Vec<u8>>)> {
        // Save As answers on a later pass: commit what was typed meanwhile, too (#166), even if
        // another tab is active by then.
        if !self.commit_typing_in(ctx, id) || !self.flatten_fill_sign_for_save(ctx, id) {
            return None;
        }
        let recovered = self.recovered_paths.contains(&id);
        let Some((name, full)) = self.session.get(id).map(|d| {
            // Save As to another file and a recovered document are clean copies, without earlier
            // revisions that may retain deleted pages or replaced text (#2165, #3923). A signed
            // document stays incremental so its signatures keep covering the file.
            let full = (target != SaveTarget::InPlace || recovered)
                && !d.path.as_deref().is_some_and(|p| dest.is_some_and(|dest| crate::same_file(p, dest)))
                && d.read_only_reason.is_none()
                && !d.is_signed();
            (d.name.clone(), full)
        }) else {
            self.notify_tr("The document was closed before it could be saved.");
            return None;
        };
        self.run_document_actions(ctx, id, pdfcraft_engine::js::DocumentScriptEvent::WillSave);
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) };
        match bytes.and_then(|b| self.session.check_save(id, &b).map(|()| b)) {
            Ok(b) => Some((name, b)),
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                None
            }
        }
    }

    /// The document `id` was written as `bytes`: it is saved now, under `path` when it takes a new
    /// file. Returns `true` unless reopening the written bytes failed.
    fn finish_save(&mut self, ctx: &egui::Context, id: DocId, bytes: Arc<Vec<u8>>, path: Option<String>) -> bool {
        match self.session.mark_saved(id, bytes, path) {
            Ok(()) => {
                self.forget_recovery(id);
                self.placed_signatures.remove(&id);
                // The tab in use: `sync_views` brings any other tab of the document up to date.
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.asking_tab(id).and_then(|i| self.views.get_mut(i))
                {
                    view.document_changed(ctx, &doc.info);
                }
                // Saving over a version that changed on disk keeps this one: nothing to ask.
                for view in self.views.iter_mut().filter(|v| v.id == id) {
                    view.disk_changed = false;
                    view.redactions_applied = 0;
                    view.redactions_undone_after_save = 0;
                    view.saved_redactions_cut_off = false;
                }
                true
            }
            Err(e) => {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                false
            }
        }
    }
}

/// Whether `view` holds content that a saved or delivered Apply redactions removed: a saved step
/// is undone, cannot be redone, or fewer steps are in effect than were delivered.
fn restores_saved_redactions(view: &crate::DocView) -> bool {
    view.redactions_undone_after_save > 0 || view.saved_redactions_cut_off || view.redactions_applied < view.redactions_downloaded
}

/// `name` without the `_Redacted` that [`pdfcraft_engine::redacted_file_name`] adds before the
/// extension ("contract_Redacted.pdf" gives "contract.pdf"). Any other name is returned as it is.
pub(crate) fn unredacted_file_name(name: &str) -> String {
    let cut = name.len().saturating_sub(4);
    let (stem, ext) = match (name.get(..cut), name.get(cut..)) {
        (Some(stem), Some(ext)) if ext.eq_ignore_ascii_case(".pdf") => (stem, ext),
        _ => (name, ""),
    };
    match stem.strip_suffix("_Redacted") {
        Some(base) if !base.is_empty() => format!("{base}{ext}"),
        _ => name.to_string(),
    }
}

/// The file name of `path`, for the notice after a save.
fn short_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}
