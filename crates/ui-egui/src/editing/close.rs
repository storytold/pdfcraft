use craft::quit::{CloseAnswer, CloseGuard, CloseScope, GuardStep};
use craft::ui::dialog::{DialogResult, UnsavedChangesDialog};
use craft::ui::runtime::windows::{WindowAction, WindowHost};
use craft::ui::shell::embed::CloseDecision;
use pdfcraft_engine::DocId;

use super::PdfCraftApp;
use super::SaveTarget;

/// Closes the root window once the app allows it; the close reaches [`PdfCraftApp::close_requested`].
fn close_root_window(ctx: &egui::Context) {
    let _ = WindowHost::of(ctx).act(egui::ViewportId::ROOT, WindowAction::Close);
    ctx.request_repaint();
}

impl PdfCraftApp {
    /// Close a tab, asking first if it has unsaved changes. A document under OCR review stays
    /// open until the review is accepted or rejected.
    pub fn request_close_tab(&mut self, index: usize) {
        if self.hold_close_for_ocr_review(index) {
            return;
        }
        // Text typed into a field counts as an unsaved change. Another tab of the document (split
        // view) keeps the document's changes open, so closing this one asks only about its own.
        if self.asks_before_closing(index)
            && let Some(id) = self.views.get(index).map(|v| v.id)
        {
            if self.has_twin(index) {
                // The question is about this tab of the document (`asking_tab`).
                self.active = Some(index);
            }
            self.begin_close(CloseScope::Close, vec![id]);
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    /// While a document is under OCR review, nothing closes: the review comes forward first.
    pub fn close_all(&mut self) {
        if self.hold_for_any_ocr_review() {
            return;
        }
        for i in (0..self.views.len()).rev() {
            if !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        let dirty = self.dirty_documents();
        self.begin_close(CloseScope::Close, dirty);
    }

    /// File ▸ Revert on the active document: asks, and reverts it unless the user cancels.
    // PLAN: the `file.revert` command (commands.rs, owner pdf-hub) calls this in place of the `Dialog::Revert` confirmation.
    pub fn request_revert(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        self.begin_close(CloseScope::Revert, vec![id]);
    }

    /// A document whose OCR review is still open (a run, or a reading not yet accepted or
    /// rejected) does not close under it: closing would leave the review with nothing to apply
    /// to (#6485). The document comes to the front with its review, and a notice says what to
    /// do. Whether the close was held.
    pub(crate) fn hold_close_for_ocr_review(&mut self, index: usize) -> bool {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        if !self.ocr_verify.holds_reading_of(id) {
            return false;
        }
        self.active = Some(index);
        self.ocr_verify.open = true;
        self.notify_tr("Accept or reject the OCR review before closing this document");
        true
    }

    /// Close all and Quit close every document, so a document under OCR review holds them too
    /// ([`Self::hold_close_for_ocr_review`]), even when it has no unsaved changes. Whether they
    /// were held.
    fn hold_for_any_ocr_review(&mut self) -> bool {
        self.ocr_review_index().is_some_and(|i| self.hold_close_for_ocr_review(i))
    }

    /// The tab of the document whose OCR review is still open, if any.
    fn ocr_review_index(&self) -> Option<usize> {
        self.views.iter().position(|v| self.ocr_verify.holds_reading_of(v.id))
    }

    /// The reversion itself, once the user confirmed it through the close guard's Revert question.
    pub fn revert_active(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.revert(id) {
            Ok(()) => {
                if let Some(i) = self.active
                    && let Some(d) = self.session.get(id)
                {
                    let view = &mut self.views[i];
                    view.document_changed(&d.info);
                    // Revert discards this source's uncommitted typing as well as model edits.
                    // Otherwise a later Save could write a rejected draft back into the file.
                    view.forms.focus = None;
                    // The focus goes with them, running no exit: a script changing the document
                    // just reverted would only make it unsaved again.
                    view.forms.focused = None;
                    view.forms.committed = None;
                    view.forms.unbaked.clear();
                    view.pending_edit = None;
                    // The redactions applied since the last save are discarded too. Those in the
                    // last web download stay, so later downloads keep the `_Redacted` name.
                    view.redactions_applied = view.redactions_downloaded;
                    view.redactions_undone_after_save = 0;
                    view.saved_redactions_cut_off = false;
                }
                // A signature placed since the last save is gone with the rest: the next save
                // doesn't ask to lock it (#2149).
                self.placed_signatures.remove(&id);
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.has_unsaved_work(i))
    }

    /// The documents with unsaved changes, in tab order, each once (split view shows one twice).
    fn dirty_documents(&self) -> Vec<DocId> {
        let mut out = Vec::new();
        for (i, view) in self.views.iter().enumerate() {
            if self.has_unsaved_work(i) && !out.contains(&view.id) {
                out.push(view.id);
            }
        }
        out
    }

    /// The tab a close question about document `id` is about: the active one when it shows `id`
    /// (in split view two tabs can), else the document's first.
    pub(crate) fn asking_tab(&self, id: DocId) -> Option<usize> {
        self.active.filter(|&a| self.views.get(a).is_some_and(|v| v.id == id)).or_else(|| self.views.iter().position(|v| v.id == id))
    }

    /// Starts the unsaved-changes question for `docs` under `scope`. A newer operation replaces
    /// one still asking; a quit it replaces is called off.
    fn begin_close(&mut self, scope: CloseScope, docs: Vec<DocId>) {
        let Some(first) = docs.first().copied() else { return };
        if self.close_guard.as_ref().is_some_and(|g| g.scope() == CloseScope::Quit) {
            self.session_keeper.quit_cancelled();
        }
        self.close_guard = Some(CloseGuard::new(scope, docs));
        self.run_guard_step(GuardStep::Ask(first));
    }

    /// Request a normal application exit, preserving every unsaved document until answered.
    /// Returns false while the save/discard/cancel questions need a decision, or while an OCR
    /// review is still to be accepted or rejected.
    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) -> bool {
        if self.hold_for_any_ocr_review() {
            ctx.request_repaint();
            return false;
        }
        let dirty = self.dirty_documents();
        if dirty.is_empty() {
            close_root_window(ctx);
            return true;
        }
        self.begin_close(CloseScope::Quit, dirty);
        ctx.request_repaint();
        false
    }

    /// The window's close, the one exit decision: held while an OCR review is open or a document
    /// has unsaved changes (text typed into a field counts), with the question asked; otherwise
    /// the window closes and nothing is left to recover.
    pub(crate) fn close_requested(&mut self) -> CloseDecision {
        if self.hold_for_any_ocr_review() {
            if self.close_guard.as_ref().is_some_and(|g| g.scope() == CloseScope::Quit) {
                self.close_guard = None;
                self.session_keeper.quit_cancelled();
            }
            return CloseDecision::Hold;
        }
        let dirty = self.dirty_documents();
        if !dirty.is_empty() {
            // A second close of the window while the quit is asking leaves the question as it is.
            if !self.close_guard.as_ref().is_some_and(|g| g.scope() == CloseScope::Quit) {
                self.begin_close(CloseScope::Quit, dirty);
            }
            return CloseDecision::Hold;
        }
        self.shutdown_recovery();
        CloseDecision::Close
    }

    /// Takes the answer of the unsaved-changes dialog and runs what follows. Called once per
    /// frame with the dialogs' other results.
    pub(crate) fn close_guard_pass(&mut self, ctx: &egui::Context) {
        let Some(result) = self.dialogs.take::<UnsavedChangesDialog>() else { return };
        let (answer, all) = match result {
            DialogResult::Ok(a) => (a.answer, a.apply_to_all),
            DialogResult::Cancel => (CloseAnswer::Cancel, false),
        };
        self.answer_close(answer, all);
        ctx.request_repaint();
    }

    /// Gives the guard the user's answer for the document it asks about.
    fn answer_close(&mut self, answer: CloseAnswer, all: bool) {
        let Some(scope) = self.close_guard.as_ref().map(|g| g.scope()) else { return };
        // Quitting closes the unsaved tabs one by one (and brings each forward to save it):
        // remember what was open, and which tab was active, before the first one goes (#442).
        if scope == CloseScope::Quit && answer != CloseAnswer::Cancel {
            let open = self.open_session();
            self.session_keeper.quit_started(|| open);
        }
        let Some(guard) = self.close_guard.as_mut() else { return };
        let step = if all { guard.answer_all(answer) } else { guard.answer(answer) };
        self.run_guard_step(step);
    }

    /// Runs the guard's steps until one waits for the user or a save.
    fn run_guard_step(&mut self, first: GuardStep<DocId>) {
        let mut step = first;
        loop {
            match step {
                GuardStep::Ask(doc) => {
                    self.ask_close(doc);
                    return;
                }
                GuardStep::Save(doc) => {
                    self.save_for_guard(doc);
                    return;
                }
                GuardStep::Discard(doc) => {
                    if self.discard_for_guard(doc) {
                        self.end_close_guard(false);
                        return;
                    }
                    let Some(guard) = self.close_guard.as_mut() else { return };
                    step = guard.discarded();
                }
                GuardStep::Cancelled => {
                    self.end_close_guard(false);
                    return;
                }
                GuardStep::Finished => {
                    self.end_close_guard(true);
                    return;
                }
            }
        }
    }

    /// Opens the question about `doc`. A document that has no tab left, or was saved by other
    /// means meanwhile, is answered as discarded: there is nothing to ask.
    fn ask_close(&mut self, doc: DocId) {
        let Some(scope) = self.close_guard.as_ref().map(|g| g.scope()) else { return };
        let asks = self.asking_tab(doc).is_some_and(|i| scope == CloseScope::Revert || self.has_unsaved_work(i));
        if !asks {
            self.answer_close(CloseAnswer::DontSave, false);
            return;
        }
        let name = self.session.get(doc).map(|d| d.name.clone()).unwrap_or_default();
        let remaining = self.close_guard.as_ref().map_or(1, |g| g.remaining());
        let _ = self.dialogs.open(UnsavedChangesDialog::new(scope, name).remaining(remaining));
    }

    /// Saves `doc`, then lets the guard go on. The save may finish on a later frame (the user
    /// choosing where); a failed or cancelled one ends the whole operation.
    fn save_for_guard(&mut self, doc: DocId) {
        let Some(index) = self.asking_tab(doc) else {
            if let Some(step) = self.close_guard.as_mut().map(|g| g.saved()) {
                self.run_guard_step(step);
            }
            return;
        };
        // Bring the document forward: what's typed in its form is committed and saved with it,
        // and a field that rejects the value is shown where the user can fix it.
        self.active = Some(index);
        // `save_then` returns false both for a save that waits for the user and for one that did
        // not start, so the failure end is its `failed` continuation, never the return value.
        // PLAN: assumed `save_then(index, target, after, failed)` with `failed` run when the save fails or is cancelled (pdf-shell-w14)
        self.save_then(index, SaveTarget::InPlace, move |app| app.guard_saved(doc), move |app| app.guard_save_failed(doc));
    }

    /// The save `doc`'s question waited for failed or was cancelled: the guard ends the whole
    /// operation and the document stays open.
    fn guard_save_failed(&mut self, doc: DocId) {
        if self.close_guard.as_ref().and_then(|g| g.current()) != Some(&doc) {
            return;
        }
        let Some(step) = self.close_guard.as_mut().map(|g| g.save_failed()) else { return };
        self.run_guard_step(step);
    }

    /// Whether a quit is waiting on the user's answers.
    pub(crate) fn quitting(&self) -> bool {
        self.close_guard.as_ref().is_some_and(|g| g.scope() == CloseScope::Quit)
    }

    /// The save `doc`'s question waited for was written: its tab closes and the guard goes on.
    /// An OCR review started meanwhile holds the close as it holds a new one.
    fn guard_saved(&mut self, doc: DocId) {
        if self.close_guard.as_ref().and_then(|g| g.current()) != Some(&doc) {
            return;
        }
        if let Some(i) = self.asking_tab(doc) {
            if self.hold_close_for_ocr_review(i) {
                self.end_close_guard(false);
                return;
            }
            self.close_tab(i);
        }
        let Some(step) = self.close_guard.as_mut().map(|g| g.saved()) else { return };
        self.run_guard_step(step);
    }

    /// Closes `doc`'s tab without saving (a revert keeps the tab and goes back to the saved file).
    /// Whether an OCR review held it.
    fn discard_for_guard(&mut self, doc: DocId) -> bool {
        let Some(index) = self.asking_tab(doc) else { return false };
        if self.close_guard.as_ref().is_some_and(|g| g.scope() == CloseScope::Revert) {
            self.active = Some(index);
            self.revert_active();
            return false;
        }
        if self.hold_close_for_ocr_review(index) {
            return true;
        }
        self.close_tab(index);
        false
    }

    /// The guard is done. A finished quit closes the window, which asks `close_requested` again
    /// and finds nothing left at risk; a quit that ended cancelled or held is called off.
    fn end_close_guard(&mut self, finished: bool) {
        let Some(guard) = self.close_guard.take() else { return };
        if guard.scope() != CloseScope::Quit {
            return;
        }
        if finished {
            if let Some(ctx) = self.ctx.clone() {
                close_root_window(&ctx);
            }
        } else {
            self.session_keeper.quit_cancelled();
        }
    }

    /// Whether leaving would lose work: a document with unsaved changes, or an OCR review not yet
    /// accepted or rejected. The browser's reload warning and the embedding page (`host_dirty`)
    /// both hear this.
    pub(crate) fn work_at_risk(&self) -> bool {
        self.first_dirty().is_some() || self.ocr_review_index().is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use craft::host::{AppInfo, Host, ManualJobs, ScriptedFiles};
    use craft::ui::dialog::UnsavedChangesDialog;
    use craft::ui::shell::embed::CloseDecision;

    use crate::PdfCraftApp;

    fn app() -> PdfCraftApp {
        let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
        let host = Host::builder(info).jobs(Arc::new(ManualJobs::new())).files(Arc::new(ScriptedFiles::new())).build();
        let wake = craft::host::Wake::none();
        let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::SystemClipboard::new());
        let (intake, _) = craft::platform::open::FileIntake::new(wake.clone());
        let watch = craft::platform::watch::FileWatch::new(wake.clone());
        PdfCraftApp::with_host(Arc::new(host), crate::AppServices::new(wake, clipboard, None, None, intake, watch))
    }

    /// The window's close is held, and the quit question asked, while a document has unsaved
    /// changes; a clean one closes.
    #[test]
    fn the_window_close_holds_while_work_is_at_risk_and_closes_when_clean() {
        let mut app = app();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("a.pdf", None, bytes.to_vec()).unwrap();
        assert_eq!(app.close_requested(), CloseDecision::Close, "nothing to lose");
        assert!(!app.dialogs.is_open::<UnsavedChangesDialog>());
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        assert_eq!(app.close_requested(), CloseDecision::Hold, "unsaved changes hold the close");
        assert!(app.dialogs.is_open::<UnsavedChangesDialog>(), "and the question is asked");
        assert_eq!(app.views.len(), 1);
    }
}
