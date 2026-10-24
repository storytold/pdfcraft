//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use pdfcraft_engine::Edit;
use pdfcraft_platform::staging::{StagingName, staging_suffixes};

use crate::PdfCraftApp;

/// What the user was doing when we asked whether to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this document's tab. By id, not tab index: tabs can close and shift while a save
    /// panel is open.
    Tab(pdfcraft_engine::DocId),
    /// Quit the application once every dirty document is resolved.
    Quit,
    /// File ▸ Close all: like Quit, but the application stays open.
    All,
}

/// A save held back by [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged): the file
/// on disk is another version than the document came from (#2166).
#[cfg(not(target_arch = "wasm32"))]
pub struct PendingOverwrite {
    pub id: pdfcraft_engine::DocId,
    pub dest: String,
    target: SaveTarget,
    /// What was to run once the document is written (close its tab, quit).
    after: Box<dyn FnOnce(&mut PdfCraftApp) + Send>,
}

/// Where Save writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    /// The file's own path; asks for one if the document has none (e.g. opened from bytes).
    InPlace,
    /// Always ask for a new location.
    As,
}

impl PdfCraftApp {
    /// Apply an edit to the active document. Returns `true` on success; failures are shown.
    pub fn apply_edit(&mut self, edit: Edit) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        let label = edit.label();
        let (form_before, page_count_before, had_xfa) = if let Some(d) = self.session.get(id) {
            (Some(std::sync::Arc::clone(&d.form)), d.info.pages.len(), d.xfa.is_some())
        } else {
            (None, 0, false)
        };
        match self.session.apply(id, edit.clone()) {
            Ok(()) => {
                let Some(doc) = self.session.get(id) else { return true };
                let info = &doc.info;
                let view = &mut self.views[i];
                view.applied_editor = None;
                // A picture picked with the Select tool may be the one this edit removed or moved:
                // drop its highlight now, not on the next frame (which only comes with the next input).
                view.picked_image = None;
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint();
                }
                if matches!(edit, Edit::AddAnnotation(_)) {
                    // A released drawing's raster can come now: it will have the annotation.
                    view.comments.drawings_committed();
                } else {
                    // Only a new drawing waits on its raster; any other edit may remove or move it.
                    view.comments.settling.clear();
                }
                view.signature_drag.committed(&edit, doc.edit_generation());
                view.move_background.committed(&edit, doc.edit_generation());
                view.text_background.committed(&edit);
                if let Some(page) = comment_page(&edit).or_else(|| content_page(&edit)) {
                    // Comment and content edits change one page: keep every other raster.
                    view.page_changed(page);
                } else if let Some(form_before) = form_before
                    && doc.info.pages.len() == page_count_before
                    && doc.xfa.is_some() == had_xfa
                    && doc.xfa.is_none()
                    && is_form_edit(&edit)
                {
                    let mut changed_pages = std::collections::BTreeSet::new();
                    for f in doc.form.iter() {
                        match form_before.iter().find(|old| old.name == f.name) {
                            Some(old) => {
                                if old != f {
                                    for w in &f.widgets {
                                        if let Some(p) = w.page {
                                            changed_pages.insert(p);
                                        }
                                    }
                                    for w in &old.widgets {
                                        if let Some(p) = w.page {
                                            changed_pages.insert(p);
                                        }
                                    }
                                    if old.value != f.value {
                                        for w in &f.widgets {
                                            if let Some(p) = w.page {
                                                view.forms.unbaked.insert((f.name.clone(), p));
                                            }
                                        }
                                    }
                                }
                            }
                            None => {
                                for w in &f.widgets {
                                    if let Some(p) = w.page {
                                        changed_pages.insert(p);
                                        view.forms.unbaked.insert((f.name.clone(), p));
                                    }
                                }
                            }
                        }
                    }
                    for old in form_before.iter() {
                        if !doc.form.iter().any(|f| f.name == old.name) {
                            for w in &old.widgets {
                                if let Some(p) = w.page {
                                    changed_pages.insert(p);
                                }
                            }
                        }
                    }
                    for page in changed_pages {
                        view.page_changed(page);
                    }
                } else {
                    view.document_changed(info);
                }
                if matches!(edit, Edit::AddAnnotation(_)) {
                    view.comments.comment_added();
                }
                // Keep the pages the user acted on selected, where they now are.
                match edit {
                    Edit::MovePages { pages, to } => {
                        let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
                        let to = to.min(info.pages.len() - n);
                        view.select_pages(&(to..to + n).collect::<Vec<_>>());
                    }
                    Edit::DuplicatePages { pages, at: Some(at) } => {
                        let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
                        let at = at.min(info.pages.len().saturating_sub(n));
                        view.select_pages(&(at..at + n).collect::<Vec<_>>());
                    }
                    Edit::InsertBlankPage { at, .. } => view.select_pages(&[at.min(info.pages.len() - 1)]),
                    Edit::DeletePages { .. } => view.select_pages(&[]),
                    // A stroke drawn with the pen stays unselected, so the selection box and
                    // author popup don't sit over the next stroke (#429).
                    Edit::AddAnnotation(a)
                        if matches!(a.shape, pdfcraft_engine::Shape::Ink { .. })
                            && self.quick_tool == crate::QuickTool::Comment(crate::comments::CommentTool::Ink) =>
                    {
                        view.comments.selected = None;
                    }
                    Edit::AddAnnotation(a) => {
                        // Select the new comment (appended last among the page's comments).
                        let newest = info.annotations.iter().filter(|x| x.page == a.page && x.in_reply_to.is_none()).map(|x| x.index).max();
                        view.comments.selected = newest.map(|n| (a.page, n));
                        view.comments.reveal = true;
                        // A new highlight opens its note for typing straight away, as in Acrobat,
                        // unless a note typed into another card is still unsaved.
                        if let (pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, .. }, Some(n)) = (&a.shape, newest)
                            && a.contents.is_empty()
                            && view.comments.editing.is_none()
                        {
                            view.comments.open_editor(a.page, n, String::new());
                        }
                    }
                    Edit::DeleteAnnotation { .. } => view.comments.selected = None,
                    _ => {}
                }
                if std::mem::take(&mut view.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                // Text saved as typed but drawn with `?` (its font can't draw it): said, not silent.
                // Before the scripts' output, so an alert they raise still shows.
                if let Some(n) = self.session.get(id).and_then(|d| d.edit_notices.first()).cloned() {
                    self.notify(crate::content_ui::undrawn_message(&n));
                }
                let out = self.session.take_js_output(id);
                self.handle_js(id, out);
                true
            }
            Err(e) => {
                if let Some(view) = self.views.get_mut(i) {
                    // A drawing whose annotation wasn't added must not stay painted.
                    view.comments.drawings_refused();
                    // A paragraph that couldn't be applied opens again with its typing, instead of
                    // the box the click moved to.
                    if let Edit::EditTextBlock { page, block, .. } = &edit
                        && let Some(mut ed) = view.applied_editor.take().filter(|ed| (ed.page, ed.block) == (*page, *block))
                    {
                        ed.focus = true;
                        view.line_editor = Some(ed);
                        view.pending_text_open = None;
                    }
                }
                let message = if matches!(edit, Edit::MoveObjects { .. }) { crate::object_ui::move_error(&e) } else { e.to_string() };
                self.notify_fmt("{label} failed: {e}", &[("label", &crate::i18n::edit_label(&label)), ("e", &message)]);
                false
            }
        }
    }

    /// Undo the last change: to the Combine files list while its tab shows, else the document.
    pub fn undo(&mut self) {
        // While a polygon, cloud or run of connected lines is being drawn, Undo takes back its
        // last point instead of the last finished edit.
        if let Some((i, _)) = self.active_ids()
            && self.views[i].comments.undo_point()
        {
            return;
        }
        if self.combine_showing() {
            self.combine_history_step(true);
        } else {
            self.history_step(true);
        }
    }

    pub fn redo(&mut self) {
        if self.combine_showing() {
            self.combine_history_step(false);
        } else {
            self.history_step(false);
        }
    }

    fn history_step(&mut self, undo: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let result = if undo { self.session.undo(id) } else { self.session.redo(id) };
        match result {
            Ok(label) => {
                if let Some(doc) = self.session.get(id) {
                    self.views[i].document_changed(&doc.info);
                }
                self.views[i].comments.selected = None;
                let label = crate::i18n::edit_label(&label);
                if undo {
                    self.notify_fmt("Undid {label}", &[("label", &label)]);
                } else {
                    self.notify_fmt("Redid {label}", &[("label", &label)]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// Apply any edit a view queued this frame (organize toolbar, keys).
    /// Returns false when a field refused the typing queued for it (its editor is open again).
    pub(crate) fn process_pending_edits(&mut self) -> bool {
        let Some(i) = self.active else { return true };
        let applied = self.apply_queued_edit(i);
        if !applied {
            // Nor may a shortcut pressed with it (⌘S right after Enter) run behind the refusal.
            self.deferred_commands.clear();
        }
        match self.views.get_mut(i).and_then(|v| v.pending_action.take()) {
            Some(crate::canvas::ViewAction::InsertFromFile) => self.insert_from_file_dialog(),
            Some(crate::canvas::ViewAction::InsertFromFileAt(at)) => self.insert_from_file_at(Some(at)),
            Some(crate::canvas::ViewAction::ReplacePages) => self.replace_pages_dialog(),
            Some(crate::canvas::ViewAction::Save) => {
                self.save_active(SaveTarget::InPlace);
            }
            Some(crate::canvas::ViewAction::Extract) => self.open_extract_dialog(),
            Some(crate::canvas::ViewAction::Split) => self.dialog = Some(crate::Dialog::Split),
            Some(crate::canvas::ViewAction::CopyPages { cut }) => self.copy_pages(cut),
            Some(crate::canvas::ViewAction::PastePages) => self.paste_pages(),
            Some(crate::canvas::ViewAction::EnhanceCamera) => self.enhance_camera(i),
            Some(crate::canvas::ViewAction::CancelCamera) => self.leave_camera(i),
            Some(crate::canvas::ViewAction::CameraRotate { clockwise }) => {
                if let Some(done) = self.views.get(i).and_then(|v| v.camera_done) {
                    let degrees = if clockwise { 90 } else { -90 };
                    if self.apply_edit(Edit::RotatePages { pages: vec![done.page], degrees })
                        && let Some(d) = self.views.get_mut(i).and_then(|v| v.camera_done.as_mut())
                    {
                        d.turns += degrees;
                        d.steps += 1;
                    }
                }
            }
            Some(crate::canvas::ViewAction::CameraLevel) => self.camera_level(i),
            Some(crate::canvas::ViewAction::CameraNext(command)) => {
                self.leave_camera(i);
                self.execute(command);
            }
            None => {}
        }
        applied
    }

    /// Enhance ▸ Camera image ▸ clean-up slider: undo the enhancement (and the turns made since)
    /// and enhance again from the photo at the new level, turning it as before. Only when the
    /// top of the undo history is still those steps; otherwise the slider goes back.
    fn camera_level(&mut self, i: usize) {
        let Some(view) = self.views.get(i) else { return };
        let (id, Some(done)) = (view.id, view.camera_done) else { return };
        let ours = |s: &pdfcraft_engine::Session, k: usize| {
            let label = s.get(id).and_then(|d| d.can_undo().map(str::to_owned));
            let want = if k + 1 == done.steps { "Enhance camera image" } else { "Rotate page" };
            label.as_deref() == Some(want)
        };
        // Undo the run this tool applied, checking each step's label first. If the run was
        // broken (another edit came in between), redo what was undone and leave the page alone:
        // enhancing a page that is still straightened would warp it a second time.
        let restore = |app: &mut Self| {
            if let Some(d) = app.views.get_mut(i).and_then(|v| v.camera_done.as_mut()) {
                (d.look, d.level) = d.applied;
            }
        };
        let mut undone = 0;
        while undone < done.steps && ours(&self.session, undone) && self.session.undo(id).is_ok() {
            undone += 1;
        }
        if undone < done.steps {
            for _ in 0..undone {
                if self.session.redo(id).is_err() {
                    break;
                }
            }
            restore(self);
            return;
        }
        let result = self.session.enhance_camera_page_at(id, done.page, done.corners, done.look, done.level).and_then(|()| {
            let turn = done.turns.rem_euclid(360);
            if turn == 0 { Ok(1) } else { self.session.apply(id, Edit::RotatePages { pages: vec![done.page], degrees: turn }).map(|()| 2) }
        });
        if let (Some(doc), Some(view)) = (self.session.get(id), self.views.get_mut(i)) {
            view.document_changed(&doc.info);
            if let Some(d) = view.camera_done.as_mut() {
                match result {
                    Ok(steps) => {
                        d.applied = (d.look, d.level);
                        d.steps = steps;
                    }
                    Err(_) => (d.look, d.level) = d.applied,
                }
            }
        }
        if let Err(e) = result {
            let label = crate::i18n::action_label("Enhance camera image");
            self.notify_fmt("{label} failed: {e}", &[("label", &label), ("e", &e.to_string())]);
        }
    }

    /// Leave Enhance ▸ Camera image on tab `i`.
    fn leave_camera(&mut self, i: usize) {
        if let Some(view) = self.views.get_mut(i) {
            view.camera = None;
            view.camera_done = None;
        }
        if self.quick_tool == crate::QuickTool::EnhanceCamera {
            self.quick_tool = crate::QuickTool::Select;
        }
    }

    /// Enhance ▸ Camera image: straighten what the corners on tab `i` enclose. On success the
    /// bar offers rotate, crop and organize for the page; on failure the corners stay so they
    /// can be adjusted.
    fn enhance_camera(&mut self, i: usize) {
        let Some(view) = self.views.get(i) else { return };
        let (id, Some(q)) = (view.id, view.camera) else { return };
        let Some(page) = self.session.get(id).and_then(|d| d.info.pages.get(q.page).cloned()) else { return };
        let corners = q.corners.map(|[x, y]| {
            let [u, v] = page.view_to_user(x, y);
            [u as f64, v as f64]
        });
        let (look, level) = (crate::camera_ui::Look::Colour, crate::camera_ui::DEFAULT_LEVEL);
        match self.session.enhance_camera_page_at(id, q.page, corners, look, level) {
            Ok(()) => {
                // The tool stays open with the clean-up slider, rotate, crop and organize.
                if let (Some(doc), Some(view)) = (self.session.get(id), self.views.get_mut(i)) {
                    view.document_changed(&doc.info);
                    view.camera = None;
                    view.camera_done =
                        Some(crate::camera_ui::CameraDone { page: q.page, corners, look, level, applied: (look, level), turns: 0, steps: 1 });
                }
            }
            Err(e) => {
                let label = crate::i18n::action_label("Enhance camera image");
                self.notify_fmt("{label} failed: {e}", &[("label", &label), ("e", &e.to_string())]);
            }
        }
    }

    /// Organize ▸ Copy / Cut: remember the selected pages (the document as it is now); Cut also
    /// deletes them (one page always stays).
    pub fn copy_pages(&mut self, cut: bool) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let pages = self.views[i].target_pages();
        let n = doc.info.pages.len();
        if cut && pages.len() >= n {
            self.notify_tr("A document needs at least one page: copy instead");
            return;
        }
        self.page_clipboard = Some(crate::PageClip { from: id, name: doc.name.clone(), bytes: doc.bytes.clone(), pages: pages.clone() });
        if cut {
            self.apply_edit(Edit::DeletePages { pages: pages.clone() });
        }
        let what = if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) };
        if cut {
            self.notify_fmt("Cut {what}", &[("what", &what)]);
        } else {
            self.notify_fmt("Copied {what}", &[("what", &what)]);
        }
    }

    /// Organize ▸ Paste: insert the copied pages after the selection (or the current page).
    pub fn paste_pages(&mut self) {
        let Some(clip) = self.page_clipboard.clone() else {
            self.notify_tr("Copy or cut pages first");
            return;
        };
        let Some(i) = self.active else { return };
        let at = self.views[i].target_pages().into_iter().max().map_or(0, |p| p + 1);
        let count = clip.pages.len();
        // Pasting into the document the pages came from is a duplicate (or, after Cut, a move):
        // its fields keep their names. Pasting into another document renames colliding ones.
        let field_conflicts =
            if self.views[i].id == clip.from { pdfcraft_engine::FieldConflicts::Merge } else { pdfcraft_engine::FieldConflicts::Rename };
        if self.apply_edit(Edit::InsertPagesFrom {
            name: clip.name.clone(),
            bytes: clip.bytes.clone(),
            pages: Some(clip.pages.clone()),
            at,
            field_conflicts,
        }) {
            self.views[i].select_pages(&(at..at + count).collect::<Vec<_>>());
        }
    }

    /// Make the active document say what its form shows (#166): apply an edit still queued on
    /// its tab, then commit the text in a field's open editor, which otherwise commits only on
    /// Enter, Tab or clicking away (the editor closes, as clicking away would). Run before
    /// anything reads the document: Save, Print, Export, Optimize, Sign, Extract, Split, Copy
    /// pages and button scripts.
    ///
    /// Returns false when the field rejects the value (a validation script or format; the user
    /// is told why). The editor stays open with the text, and the caller must stop: nothing is
    /// saved, printed or exported without it.
    pub(crate) fn commit_form_typing(&mut self) -> bool {
        let Some((i, id)) = self.active_ids() else { return true };
        if !self.apply_queued_edit(i) {
            return false;
        }
        let Some(draft) = self.views.get(i).and_then(|v| v.forms.focus.clone()) else { return true };
        let Some(edit) = self.session.get(id).and_then(|d| crate::forms_ui::draft_edit(&draft, &d.form)) else {
            return true; // nothing typed that the document doesn't already have
        };
        if let Some(view) = self.views.get_mut(i) {
            view.forms.focus = None;
        }
        if self.apply_edit(edit) {
            return true;
        }
        if let Some(view) = self.views.get_mut(i) {
            let mut draft = draft;
            draft.request_focus = true;
            view.forms.focus = Some(draft);
        }
        false
    }

    /// Apply the edit queued on tab `i` (the active one). Returns false only when it was a
    /// field's typing and the field refused it (#166): the editor reopens with the text, and the
    /// user has been told why, so the typing isn't lost and the caller can stop.
    fn apply_queued_edit(&mut self, i: usize) -> bool {
        let Some(edit) = self.views.get_mut(i).and_then(|v| v.pending_edit.take()) else { return true };
        let signature_page = self.views.get_mut(i).and_then(|v| v.fill_signature_page.take());
        let committed = self.views.get_mut(i).and_then(|v| v.forms.committed.take());
        let typed = match (&edit, &committed) {
            (Edit::SetFieldValue { name, .. }, Some(draft)) => *name == draft.name,
            _ => false,
        };
        if self.apply_edit(edit) {
            // Tab can wrap to the same field (including another widget of that field).
            // Its new editor was opened before this edit ran, so it still holds the old
            // value. Refresh from the accepted form before Save can commit that stale draft.
            if typed
                && let Some(view) = self.views.get_mut(i)
                && let Some(focus) = view.forms.focus.as_mut()
                && focus.request_focus
                && committed.as_ref().is_some_and(|draft| draft.name == focus.name)
                && let Some(field) = self.session.get(view.id).and_then(|doc| doc.form.iter().find(|field| field.name == focus.name))
            {
                focus.text = field.value.first().cloned().unwrap_or_default();
                focus.picked = field.value.clone();
            }
            if let Some(page) = signature_page
                && let Some(view) = self.views.get_mut(i)
            {
                // Signature imports are a labeled batch; select their appended stamp just
                // as AddAnnotation selects a typed or drawn signature.
                let newest = self
                    .session
                    .get(view.id)
                    .and_then(|d| d.info.annotations.iter().filter(|a| a.page == page && a.in_reply_to.is_none()).map(|a| a.index).max());
                view.comments.selected = newest.map(|index| (page, index));
                view.comments.reveal = true;
                self.quick_tool = crate::QuickTool::Select;
            }
            return true;
        }
        if !typed {
            return true;
        }
        if let (Some(mut draft), Some(view)) = (committed, self.views.get_mut(i)) {
            draft.request_focus = true;
            view.forms.focus = Some(draft);
        }
        false
    }

    /// [`Self::commit_form_typing`] for document `id`, which may not be the active tab (a save
    /// panel answering later, Close All). If its tab has typing or a queued edit, the tab is
    /// brought forward first, so the commit and any field scripts act on that document.
    pub(crate) fn commit_typing_in(&mut self, id: pdfcraft_engine::DocId) -> bool {
        let Some(i) = self.views.iter().position(|v| v.id == id) else { return true };
        // Added text the standard fonts can't draw can't be saved: show it rather than drop it.
        // Its page is brought into view, so the text, the warning and Discard are on screen.
        let blocked = self.views.get_mut(i).and_then(|v| {
            let c = v.content.blocked()?;
            v.content.hold_blocked();
            if let Some(page) = v.content.draft.as_ref().map(|d| d.page) {
                v.go_to_page(page);
            }
            Some(c)
        });
        if let Some(c) = blocked {
            self.active = Some(i);
            self.notify(crate::content_ui::undrawable_message(c));
            return false;
        }
        let typing = self
            .views
            .get(i)
            .is_some_and(|v| v.pending_edit.is_some() || v.forms.focus.is_some() || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty()));
        if self.active != Some(i) {
            if !typing {
                return true;
            }
            self.active = Some(i);
        }
        self.commit_form_typing() && self.commit_open_fill_text(id)
    }

    /// The Fill & Sign type box commits on click-away. Save does that first, so the text is in
    /// the file (and can be flattened) instead of being left on the screen.
    fn commit_open_fill_text(&mut self, id: pdfcraft_engine::DocId) -> bool {
        let Some(i) = self.views.iter().position(|v| v.id == id) else { return true };
        let Some(tb) = self.views[i].fill_text.clone() else { return true };
        let text = tb.text.trim().to_string();
        if text.is_empty() {
            if let Some(view) = self.views.get_mut(i) {
                view.fill_text = None;
            }
            return true;
        }
        let author = self.comment_prefs.author.clone();
        let rotation = self.session.get(id).map_or(0, |doc| crate::comments::page_rotation(&doc.info, tb.page));
        let edit = crate::fill_sign::typed(tb.page, tb.at, &text, tb.size, rotation, &author);
        if self.apply_edit(edit) {
            if let Some(view) = self.views.get_mut(i) {
                view.fill_text = None;
            }
            true
        } else {
            false
        }
    }

    /// When the preference is on, bake Fill & Sign marks before the bytes are written.
    /// Returns false when the document is not allowed to change (nothing is written).
    fn flatten_fill_sign_for_save(&mut self, id: pdfcraft_engine::DocId) -> bool {
        if !self.flatten_fill_sign_on_save {
            return true;
        }
        let before = self.session.get(id).map(|d| d.edit_generation());
        if let Err(e) = self.session.apply(id, Edit::FlattenFillSign) {
            self.notify_error(e);
            return false;
        }
        let after = self.session.get(id).map(|d| d.edit_generation());
        if before != after
            && let Some(doc) = self.session.get(id)
            && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
        {
            view.document_changed(&doc.info);
        }
        true
    }

    /// Whether tab `index` has work that isn't saved: edits, or form typing not committed yet.
    pub(crate) fn has_unsaved_work(&self, index: usize) -> bool {
        let Some(v) = self.views.get(index) else { return false };
        let Some(doc) = self.session.get(v.id) else { return false };
        doc.dirty
            || v.pending_edit.is_some()
            || v.content.blocked().is_some()
            || v.forms.focus.as_ref().is_some_and(|f| crate::forms_ui::draft_edit(f, &doc.form).is_some())
            || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
            || v.line_editor.as_ref().map_or_else(|| false, |ed| ed.has_unsaved_text())
    }

    /// Save the active document. Returns `true` if it was written.
    pub fn save_active(&mut self, target: SaveTarget) -> bool {
        match self.active {
            Some(i) => self.save_view(i, target),
            None => false,
        }
    }

    /// Save the document shown in tab `index`. Returns `true` if it was written now. When it has
    /// to ask where (a new document, or Save As) it returns `false` and saves on a later frame,
    /// once the user has chosen.
    pub fn save_view(&mut self, index: usize, target: SaveTarget) -> bool {
        self.save_then(index, target, |_| {})
    }

    /// [`Self::save_view`], then `after` once the document is written: now, or on a later frame
    /// when the user had to choose where. `after` never runs when the save fails or is cancelled.
    pub(crate) fn save_then(&mut self, index: usize, target: SaveTarget, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        // What's typed in a field is part of what's saved (#166).
        if let Some(id) = self.views.get(index).map(|v| v.id)
            && !self.commit_typing_in(id)
        {
            return false;
        }
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let (name, path) = (doc.name.clone(), doc.path.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let doc_path = path.clone();
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(p)) => p.clone(),
                // An email attachment's temporary copy is no place to keep work: ask, as for a new
                // document (`is_mail_temp_copy`).
                (SaveTarget::InPlace, Some(p), _) if !is_mail_temp_copy(&p) => p,
                _ => {
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
                    let dialog = in_document_folder(dialog, doc_path.as_deref());
                    // The bytes are taken once the user has chosen, so edits made meanwhile are saved too.
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, dest| {
                        app.save_doc_to_then(id, &dest.to_string_lossy(), target, Box::new(after));
                    });
                    return false;
                }
            };
            self.save_doc_to_then(id, &destination, target, Box::new(after))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            if !self.flatten_fill_sign_for_save(id) {
                return false;
            }
            let bytes = match self.session.save_bytes(id) {
                Ok(b) => b,
                Err(e) => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return false;
                }
            };
            // An embedding page that asked for saves (`?host=parent`) gets the bytes; otherwise
            // the browser downloads them.
            let to_host = self.host_save.is_some();
            let delivered = match &self.host_save {
                Some(save) => save(&name, &bytes, matches!(target, SaveTarget::As)),
                None => download(&name, &bytes),
            };
            match delivered {
                Ok(()) => {
                    let _ = self.session.mark_saved(id, bytes, None);
                    if let Some(doc) = self.session.get(id)
                        && let Some(view) = self.views.get_mut(index)
                    {
                        view.document_changed(&doc.info);
                    }
                    // The host reports where the file went (and any failure) itself.
                    if !to_host {
                        self.notify_fmt("Downloaded {name}", &[("name", &name)]);
                    }
                    after(self);
                    true
                }
                Err(e) if to_host => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e)]);
                    false
                }
                Err(e) => {
                    self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    false
                }
            }
        }
    }

    /// [`Self::save_doc_to`], then `after`. When `dest` is the document's own file and it is
    /// another version on disk than the one the document came from (something else wrote it
    /// since the open or the last save, #2166), it asks first: the save waits in
    /// `pending_overwrite` behind [`Dialog::OverwriteChanged`](crate::Dialog::OverwriteChanged).
    /// Returns `true` if written now.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to_then(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget, after: Box<dyn FnOnce(&mut Self) + Send>) -> bool {
        let own = self.session.get(id).and_then(|d| d.path.as_deref()).is_some_and(|p| crate::same_file(p, dest));
        if own && self.watch.disk_differs(id, dest, self.session.get(id).map(|d| d.saved_bytes())) {
            self.pending_overwrite = Some(PendingOverwrite { id, dest: dest.to_string(), target, after });
            self.dialog = Some(crate::Dialog::OverwriteChanged);
            return false;
        }
        let saved = self.save_doc_to(id, dest, target);
        if saved {
            after(self);
        }
        saved
    }

    /// The dialog's Save anyway: write over the changed file.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn overwrite_changed_file(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if self.save_doc_to(p.id, &p.dest, p.target) {
            (p.after)(self);
        }
    }

    /// The dialog's Save as…: leave the file on disk as it is and ask where to put this version.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn save_changed_file_elsewhere(&mut self) {
        let Some(p) = self.pending_overwrite.take() else { return };
        if let Some(index) = self.views.iter().position(|v| v.id == p.id) {
            self.save_then(index, SaveTarget::As, p.after);
        }
    }

    /// Write document `id` to `dest` and make it the document's file. Returns `true` if written.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to(&mut self, id: pdfcraft_engine::DocId, dest: &str, target: SaveTarget) -> bool {
        // Save As answers on a later frame: commit what was typed meanwhile, too (#166), even
        // if another tab is active by then.
        if !self.commit_typing_in(id) || !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let Some((name, full)) = self.session.get(id).map(|d| {
            // Save As to another file is a clean copy, without the earlier revisions that still
            // hold deleted pages or replaced text (#2165). A signed document stays incremental so
            // its signatures keep covering the file.
            let full = target == SaveTarget::As
                && !d.path.as_deref().is_some_and(|p| crate::same_file(p, dest))
                && d.read_only_reason.is_none()
                && !d.is_signed();
            (d.name.clone(), full)
        }) else {
            self.notify_tr("The document was closed before it could be saved.");
            return false;
        };
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) };
        let bytes = match bytes.and_then(|b| self.session.check_save(id, &b).map(|()| b)) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return false;
            }
        };
        if let Err(e) = write_atomically(dest, &bytes) {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", dest), ("e", &e.to_string())]);
            return false;
        }
        // Our own write is not a change on disk to reload (#431).
        self.watch.known_now(id, dest);
        match self.session.mark_saved(id, bytes, Some(dest.to_string())) {
            Ok(()) => {
                self.forget_recovery(id);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
                {
                    view.document_changed(&doc.info);
                    // Saving over a version that changed on disk keeps this one: nothing to ask.
                    view.disk_changed = false;
                }
                self.notify_fmt("Saved {name}", &[("name", &short_name(dest))]);
                #[cfg(target_os = "macos")]
                crate::macos_association::repair(std::path::Path::new(dest));
                true
            }
            Err(e) => {
                self.notify_fmt("Saved, but reopening failed: {e}", &[("e", &e.to_string())]);
                false
            }
        }
    }

    /// Close a tab, asking first if it has unsaved changes.
    pub fn request_close_tab(&mut self, index: usize) {
        // Text typed into a field counts as an unsaved change.
        if self.has_unsaved_work(index)
            && let Some(id) = self.views.get(index).map(|v| v.id)
        {
            self.close_request = Some(CloseRequest::Tab(id));
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    pub fn close_all(&mut self) {
        for i in (0..self.views.len()).rev() {
            if !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::All);
        }
    }

    /// File ▸ Revert (after the confirmation).
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
                    view.forms.committed = None;
                    view.forms.unbaked.clear();
                    view.pending_edit = None;
                }
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.has_unsaved_work(i))
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(req) = self.close_request.take() else { return };
        // The next question (another dirty document) starts on Save again.
        self.save_prompt_button = Default::default();
        let index = match req {
            CloseRequest::Tab(id) => match self.views.iter().position(|v| v.id == id) {
                Some(i) => i,
                None => return, // already closed
            },
            CloseRequest::Quit | CloseRequest::All => match self.first_dirty() {
                Some(i) => i,
                None => {
                    if req == CloseRequest::Quit {
                        self.quit(ctx);
                    }
                    return;
                }
            },
        };
        // Quitting closes the unsaved tabs one by one (and brings each forward to save it):
        // remember what was open, and which tab was active, before the first one goes (#442).
        if req == CloseRequest::Quit {
            match choice {
                Some(_) => self.note_quit_session(),
                None => self.forget_quit_session(),
            }
        }
        match choice {
            None => {} // cancelled: nothing closes
            Some(false) => self.close_and_continue(ctx, index, req),
            Some(true) => {
                let Some(id) = self.views.get(index).map(|v| v.id) else { return };
                // Bring the document forward: what's typed in its form is committed and saved
                // with it, and a field that rejects the value is shown where the user can fix it.
                self.active = Some(index);
                // Close only once the save has actually been written, which may be on a later
                // frame when the user has to choose where. A failed or cancelled save keeps the
                // document open.
                let ctx = ctx.clone();
                self.save_then(index, SaveTarget::InPlace, move |app| {
                    if let Some(i) = app.views.iter().position(|v| v.id == id) {
                        app.close_and_continue(&ctx, i, req);
                    }
                });
            }
        }
    }

    /// Close tab `index` for a close request, then ask about the next dirty document, or quit.
    fn close_and_continue(&mut self, ctx: &egui::Context, index: usize, req: CloseRequest) {
        self.close_tab(index);
        // A newer prompt wins: the user may have started another close while a save was pending.
        if (req == CloseRequest::Quit || req == CloseRequest::All) && self.close_request.is_none() {
            match self.first_dirty() {
                Some(_) => self.close_request = Some(req),
                None if req == CloseRequest::Quit => self.quit(ctx),
                None => {}
            }
        }
    }

    /// Request a normal application exit, preserving every unsaved document until answered.
    /// Returns false while the existing save/discard/cancel prompt needs a decision.
    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) -> bool {
        if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::Quit);
            ctx.request_repaint();
            false
        } else {
            self.shutdown_recovery();
            self.quit(ctx);
            true
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Publish whether any document has unsaved work, for the browser's reload warning (#812).
    pub(crate) fn sync_unsaved_flag(&self) {
        self.unsaved_flag.store(self.first_dirty().is_some(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Intercept window close while documents have unsaved changes.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        // egui's default quit shortcut ignores extra Alt/Shift, including Ctrl+Alt+Q for '@'.
        ctx.options_mut(|o| o.quit_shortcuts.clear());
        let shortcut = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::Q, pressed: true, modifiers, .. }
                    if modifiers.matches_exact(egui::Modifiers::COMMAND))
            })
        });
        if shortcut {
            self.request_quit(ctx);
        }
        self.sync_unsaved_flag();
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // `first_dirty` counts text typed into a field as an unsaved change.
        if !self.allow_quit && self.first_dirty().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_request = Some(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }
}

/// The single page a content edit changes (`None` for other edits). Like comment edits,
/// these leave every other page's bytes, geometry and text alone, so their rasters stay valid.
fn content_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::MoveObjects { page, .. }
        | Edit::EditTextLine { page, .. }
        | Edit::EditTextBlock { page, .. }
        | Edit::EditPageImage { page, .. }
        | Edit::AddText { page, .. }
        | Edit::AddImage { page, .. }
        | Edit::UpdateContent { page, .. }
        | Edit::DeleteContent { page, .. }
        | Edit::ReplaceImage { page, .. } => Some(*page),
        _ => None,
    }
}

fn comment_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::AddAnnotation(a) => Some(a.page),
        Edit::DeleteAnnotation { page, .. }
        | Edit::SetAnnotationContents { page, .. }
        | Edit::ReplyToAnnotation { page, .. }
        | Edit::SetAnnotationStatus { page, .. }
        | Edit::MoveAnnotation { page, .. }
        | Edit::ReshapeAnnotation { page, .. }
        | Edit::ResizeAnnotation { page, .. }
        | Edit::StyleAnnotation { page, .. }
        | Edit::FillAnnotation { page, .. }
        | Edit::FontSizeAnnotation { page, .. }
        | Edit::SetAnnotationInfo { page, .. } => Some(*page),
        _ => None,
    }
}

/// Whether `edit` is a form-related modification.
fn is_form_edit(edit: &Edit) -> bool {
    matches!(
        edit,
        Edit::SetFieldValue { .. }
            | Edit::ResetForm { .. }
            | Edit::AddField { .. }
            | Edit::DeleteField { .. }
            | Edit::DuplicateField { .. }
            | Edit::SetFieldProps { .. }
            | Edit::SetFieldImage { .. }
            | Edit::SetFieldScript { .. }
            | Edit::SetCalculationOrder { .. }
            | Edit::ApplyScriptChanges { .. }
    )
}

/// Whether `path` is the temporary copy an email client opened an attachment from, so that Save
/// asks where to keep the document instead of writing to it.
///
/// Outlook opens an attachment from its own cache: `INetCache\Content.Outlook\<random>\` for
/// classic Outlook (`Temporary Internet Files\Content.Outlook\` on older Windows), and the
/// `Packages\Microsoft.OutlookForWindows_<id>` package folder for the new Outlook. The patterns
/// follow those layouts, so an ordinary folder that merely shares a name doesn't match. Saving there either fails,
/// because Outlook can still hold the file, or appears to work while the edits stay out of the
/// email and are deleted with the cache. Apple Mail opens one from `Mail Downloads/<UUID>/` in
/// its sandbox container, where other apps can't write, so saving there fails (#2751). Matched on
/// either separator and in any case, so it can be tested on every platform.
pub(crate) fn is_mail_temp_copy(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    // Outlook's cache holds `<marker><random folder>/<file>`: exactly one folder, then the file.
    let in_outlook_cache = |marker: &str| {
        p.split_once(marker)
            .is_some_and(|(_, rest)| matches!(rest.split('/').collect::<Vec<_>>()[..], [dir, file] if !dir.is_empty() && !file.is_empty()))
    };
    in_outlook_cache("/inetcache/content.outlook/")
        || in_outlook_cache("/temporary internet files/content.outlook/")
        || p.contains("/packages/microsoft.outlookforwindows_")
        || p.contains("/library/containers/com.apple.mail/data/library/mail downloads/")
}

/// The folder a Save As style dialog should open in for a document stored at `path`: the
/// document's own folder, as Acrobat and Foxit do, instead of whichever folder the system picks.
/// `None` (keep the system's choice) for a new document, a path with no parent, or an email
/// attachment's temporary copy (`is_mail_temp_copy`), which is no place to keep work. Also right
/// for UNC paths on Windows (`\\server\share\dir\f.pdf` gives `\\server\share\dir`).
pub(crate) fn save_dialog_folder(path: Option<&str>) -> Option<std::path::PathBuf> {
    let path = path.filter(|p| !is_mail_temp_copy(p))?;
    let parent = std::path::Path::new(path).parent()?;
    (!parent.as_os_str().is_empty()).then(|| parent.to_path_buf())
}

/// `dialog` opened in the folder of the document stored at `path` ([`save_dialog_folder`]).
pub(crate) fn in_document_folder(dialog: rfd::AsyncFileDialog, path: Option<&str>) -> rfd::AsyncFileDialog {
    match save_dialog_folder(path) {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_atomically_with(path, bytes, staging_suffixes())
}

/// [`write_atomically`], trying the staging names that `suffixes` give.
fn write_atomically_with(path: &str, bytes: &[u8], suffixes: impl IntoIterator<Item = u64>) -> std::io::Result<()> {
    write_atomically_syncing(path, bytes, suffixes, std::fs::File::sync_all)
}

/// [`write_atomically_with`], flushing the staging file with `sync_all` (the standard library's
/// in the app, a failing one in tests). The writer itself is the platform crate's, shared with
/// the tools and the CLI (#2166); the desktop app keeps its staging-name form.
fn write_atomically_syncing(
    path: &str,
    bytes: &[u8],
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    pdfcraft_platform::staging::write_atomically_with(std::path::Path::new(path), bytes, StagingName::TagThenSuffix, suffixes, sync_all)
}

#[cfg(not(target_arch = "wasm32"))]
fn short_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Offer bytes as a browser download.
#[cfg(target_arch = "wasm32")]
pub(crate) fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "anchor")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

impl PdfCraftApp {
    /// Carry out a Bookmarks-panel action as an undoable edit.
    pub fn bookmark_action(&mut self, action: crate::panels::BmAction) {
        use crate::panels::BmAction as A;
        let Some((i, id)) = self.active_ids() else { return };
        let current = self.views[i].current;
        let parent_of = |p: &[usize]| p[..p.len() - 1].to_vec();
        let edit = match action {
            A::New => {
                let n = self.session.get(id).map_or(0, |d| d.info.outline.len());
                // Where the reader is: the selection's corner, else the top of the window.
                let (page, [left, top]) =
                    self.session.get(id).and_then(|d| self.views.get(i)?.bookmark_position(&d.info)).unwrap_or((current, [None, None]));
                let (left, top) = (left.map(f64::from), top.map(f64::from));
                self.apply_edit(Edit::AddBookmark { parent: vec![], index: n, title: "Untitled".into(), page, left, top });
                // Like Acrobat: the new bookmark starts in rename mode.
                self.bookmark_rename = Some((vec![n], "Untitled".into()));
                self.right = Some(crate::RightPanel::Bookmarks);
                return;
            }
            A::StartRename(path) => {
                let title = self.session.get(id).and_then(|d| bookmark_at(&d.info.outline, &path)).map(|b| b.title.clone()).unwrap_or_default();
                self.bookmark_rename = Some((path, title));
                return;
            }
            A::Rename(path, title) => Edit::RenameBookmark { path, title },
            A::SetToCurrentPage(path) => Edit::SetBookmarkPage { path, page: current },
            A::SetStyle(path, bold, italic) => Edit::SetBookmarkStyle { path, bold, italic },
            A::SetColor(path, color) => Edit::SetBookmarkColor { path, color: Some(color) },
            A::Delete(path) => Edit::DeleteBookmark { path },
            A::MoveUp(path) => {
                let at = path[path.len() - 1].saturating_sub(1);
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::MoveDown(path) => {
                let at = path[path.len() - 1] + 1;
                Edit::MoveBookmark { to_parent: parent_of(&path), from: path, index: at }
            }
            A::Indent(path) => {
                let mut to_parent = parent_of(&path);
                to_parent.push(path[path.len() - 1].saturating_sub(1));
                Edit::MoveBookmark { from: path, to_parent, index: usize::MAX }
            }
            A::Outdent(path) => {
                let parent = parent_of(&path);
                let grand = parent_of(&parent);
                let at = parent[parent.len() - 1] + 1;
                Edit::MoveBookmark { from: path, to_parent: grand, index: at }
            }
            A::FromStructure => {
                self.right = Some(crate::RightPanel::Bookmarks);
                Edit::BookmarksFromStructure
            }
        };
        self.apply_edit(edit);
    }
}

fn bookmark_at<'a>(items: &'a [pdfcraft_render::OutlineItem], path: &[usize]) -> Option<&'a pdfcraft_render::OutlineItem> {
    let (first, rest) = path.split_first()?;
    let item = items.get(*first)?;
    if rest.is_empty() { Some(item) } else { bookmark_at(&item.children, rest) }
}

#[cfg(test)]
mod tests {
    use super::{SaveTarget, is_mail_temp_copy, save_dialog_folder, write_atomically, write_atomically_syncing, write_atomically_with};

    #[test]
    fn save_dialogs_open_in_the_documents_folder() {
        use std::path::PathBuf;
        assert_eq!(save_dialog_folder(Some("/home/a/plans/f.pdf")), Some(PathBuf::from("/home/a/plans")));
        // A new document has no folder yet; a bare name has an empty parent; the root has none.
        assert_eq!(save_dialog_folder(None), None);
        assert_eq!(save_dialog_folder(Some("f.pdf")), None);
        assert_eq!(save_dialog_folder(Some("/")), None);
        // An email attachment's temporary copy is not offered.
        assert_eq!(save_dialog_folder(Some("C:\\Users\\a\\AppData\\Local\\Microsoft\\Windows\\INetCache\\Content.Outlook\\AB12\\f.pdf")), None);
        assert_eq!(save_dialog_folder(Some("/Users/a/Library/Containers/com.apple.mail/Data/Library/Mail Downloads/UUID/f.pdf")), None);
    }

    /// The Windows rules for `Path::parent` (UNC share roots included) only apply on Windows.
    #[cfg(windows)]
    #[test]
    fn save_dialog_folder_handles_unc_paths() {
        use std::path::PathBuf;
        assert_eq!(save_dialog_folder(Some(r"\\nas\projets\2026\f.pdf")), Some(PathBuf::from(r"\\nas\projets\2026")));
        assert_eq!(save_dialog_folder(Some(r"\\nas\projets\f.pdf")), Some(PathBuf::from(r"\\nas\projets\")));
        assert_eq!(save_dialog_folder(Some(r"\\nas\projets")), None);
        assert_eq!(save_dialog_folder(Some(r"C:\Users\a\f.pdf")), Some(PathBuf::from(r"C:\Users\a")));
    }
    use pdfcraft_platform::staging::{STAGING_ATTEMPTS, staging_suffixes};
    use std::path::{Path, PathBuf};

    /// A fresh, empty folder for one staging test.
    fn staging_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-staging-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn staged(dir: &Path, suffix: u64) -> PathBuf {
        dir.join(format!(".out.pdf.pdfcraft-{suffix:016x}.tmp"))
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// A symbolic link to a file, where the system allows one (Windows needs Developer Mode or an
    /// administrator for it).
    fn file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(std::io::Error::other(format!("no symbolic links here: {} {}", target.display(), link.display())))
        }
    }

    #[test]
    fn saving_never_writes_through_a_file_planted_at_the_staging_name() {
        let dir = staging_dir("planted");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        let path = target.to_string_lossy().into_owned();
        // A hard link needs no privileges on any system, and writing to it writes to `outside`.
        std::fs::hard_link(&outside, staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 2), "PLANTED").unwrap();
        write_atomically_with(&path, b"NEW", [1, 2, 3]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 1)), "PRECIOUS");
        assert_eq!(read(&staged(&dir, 2)), "PLANTED");
        assert!(!staged(&dir, 3).exists(), "the staging file was renamed into place");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_never_writes_through_a_symbolic_link_at_the_staging_name() {
        let dir = staging_dir("symlink");
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, "PRECIOUS").unwrap();
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&outside, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&outside), "PRECIOUS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_never_creates_a_file_through_a_dangling_link_at_the_staging_name() {
        let dir = staging_dir("dangling");
        let unborn = dir.join("created-through-a-link.txt");
        let target = dir.join("out.pdf");
        if let Err(e) = file_symlink(&unborn, &staged(&dir, 1)) {
            eprintln!("symbolic links not checked: {e}");
            return;
        }
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert!(!unborn.exists(), "nothing was created through the dangling link");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_at_the_staging_name_is_left_alone() {
        let dir = staging_dir("folder");
        std::fs::create_dir(staged(&dir, 1)).unwrap();
        std::fs::write(staged(&dir, 1).join("inside.txt"), "PLANTED").unwrap();
        let target = dir.join("out.pdf");
        write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(read(&staged(&dir, 1).join("inside.txt")), "PLANTED");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_gives_up_rather_than_reuse_a_taken_name() {
        let dir = staging_dir("taken");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        for s in 1..=STAGING_ATTEMPTS as u64 {
            std::fs::write(staged(&dir, s), "PLANTED").unwrap();
        }
        assert!(write_atomically_with(&target.to_string_lossy(), b"NEW", 1..).is_err());
        assert_eq!(read(&target), "OLD");
        for s in 1..=STAGING_ATTEMPTS as u64 {
            assert_eq!(read(&staged(&dir, s)), "PLANTED");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The names in a folder: what a test can see was left behind.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn saving_a_long_name_leaves_only_the_saved_file() {
        assert_ne!(staging_suffixes().next(), staging_suffixes().next(), "each save draws new names");
        // 60 four-byte characters: a 244-byte name, within every system's limit. Its staging name
        // must be too (on Linux the whole name in it would be 275 bytes).
        let dir = staging_dir("clean");
        let name = format!("{}.pdf", "\u{1F600}".repeat(60));
        let path = dir.join(&name).to_string_lossy().into_owned();
        write_atomically(&path, b"NEW").unwrap();
        write_atomically(&path, b"NEWER").unwrap();
        assert_eq!(read(&dir.join(&name)), "NEWER");
        assert_eq!(listing(&dir), [name], "no staging file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A read-only target is refused before anything is staged, so no staging file is left
    /// behind.
    #[cfg(windows)]
    #[test]
    fn a_failed_save_removes_the_staging_file() {
        let dir = staging_dir("readonly");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms.clone()).unwrap();
        let e = write_atomically_with(&target.to_string_lossy(), b"NEW", [7]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "a read-only target is refused: {e}");
        assert_eq!(read(&target), "OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On macOS `sync_all` asks for a full flush, which SMB and FUSE volumes (a Cryptomator vault,
    /// #981) refuse as unsupported: ENOTSUP (45) or ENOTTY (25). The save still goes through.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_volume_without_full_flush_still_saves() {
        for errno in [45, 25] {
            let dir = staging_dir(&format!("nofullsync-{errno}"));
            let target = dir.join("out.pdf");
            std::fs::write(&target, "OLD").unwrap();
            let asked = std::cell::Cell::new(false);
            let refuse = |_: &std::fs::File| {
                asked.set(true);
                Err(std::io::Error::from_raw_os_error(errno))
            };
            write_atomically_syncing(&target.to_string_lossy(), b"NEW", [7], refuse).unwrap();
            assert!(asked.get(), "the full flush was tried");
            assert_eq!(read(&target), "NEW");
            assert_eq!(listing(&dir), ["out.pdf"], "no staging file is left behind");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Only "not supported" is let through: any other flush failure fails the save and keeps the
    /// original.
    #[test]
    fn a_failed_flush_keeps_the_original() {
        let dir = staging_dir("syncfails");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let e = write_atomically_syncing(&target.to_string_lossy(), b"NEW", [7], |_| Err(std::io::Error::other("disk gone"))).unwrap_err();
        assert_eq!(e.to_string(), "disk gone");
        assert_eq!(read(&target), "OLD");
        assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_save_preserves_opening_settings_changed_while_staging() {
        let dir = staging_dir("opening-metadata-changed");
        let target = dir.join("out.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        xattr::set(&target, "com.apple.LaunchServices.OpenWith", b"old handler").unwrap();
        xattr::set(&target, "com.apple.quarantine", b"0082;1234;Preview;").unwrap();
        let suffixes = std::iter::once_with(|| {
            xattr::set(&target, "com.apple.LaunchServices.OpenWith", b"new handler").unwrap();
            xattr::set(&target, "com.apple.quarantine", b"0082;5678;another app;").unwrap();
            7
        });
        write_atomically_with(&target.to_string_lossy(), b"NEW", suffixes).unwrap();
        assert_eq!(read(&target), "NEW");
        assert_eq!(xattr::get(&target, "com.apple.LaunchServices.OpenWith").unwrap(), Some(b"new handler".to_vec()));
        assert_eq!(xattr::get(&target, "com.apple.quarantine").unwrap(), Some(b"0082;5678;another app;".to_vec()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn outlook_attachment_copies_are_recognised_on_either_separator_and_in_any_case() {
        for p in [
            r"C:\Users\me\AppData\Local\Microsoft\Windows\INetCache\Content.Outlook\AB12CD34\Quote.pdf",
            r"c:\users\me\appdata\local\microsoft\windows\inetcache\content.outlook\ab12cd34\quote (2).pdf",
            r"C:\Users\me\AppData\Local\Microsoft\Windows\Temporary Internet Files\Content.Outlook\AB12CD34\Quote.pdf",
            r"C:\Users\me\AppData\Local\Packages\Microsoft.OutlookForWindows_8wekyb3d8bbwe\LocalCache\Quote.pdf",
            "C:/Users/me/AppData/Local/Microsoft/Windows/INetCache/Content.Outlook/AB12CD34/Quote.pdf",
        ] {
            assert!(is_mail_temp_copy(p), "{p}");
        }
    }

    #[test]
    fn apple_mail_attachment_copies_are_recognised() {
        for p in [
            "/Users/me/Library/Containers/com.apple.mail/Data/Library/Mail Downloads/6F1C2A9E-3B4D-4E5F-8A7B-9C0D1E2F3A4B/booking.pdf",
            "/Users/me/library/containers/COM.APPLE.MAIL/data/library/mail downloads/6F1C2A9E/Booking (2).pdf",
        ] {
            assert!(is_mail_temp_copy(p), "{p}");
        }
    }

    #[test]
    fn ordinary_paths_are_not_mail_copies() {
        for p in [
            r"C:\Users\me\Documents\Quote.pdf",
            r"C:\Users\me\OneDrive - Contoso\Content.Outlook notes\Quote.pdf",
            r"C:\Users\me\AppData\Local\Microsoft\Windows\INetCache\IE\Quote.pdf",
            "/home/microsoft.outlookforwindows/report.pdf",
            "/x/inetcache/content.outlook/y.pdf",
            "/x/temporary internet files/content.outlook/y.pdf",
            "/home/me/Documents/Quote.pdf",
            "/Users/me/Downloads/Mail Downloads/booking.pdf",
            "/Users/me/Library/Containers/com.apple.Notes/Data/Library/Mail Downloads/booking.pdf",
            "Quote.pdf",
        ] {
            assert!(!is_mail_temp_copy(p), "{p}");
        }
    }

    /// A one-page document with a proper xref table.
    fn one_page_pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    #[test]
    fn save_asks_where_to_keep_an_email_attachment_and_leaves_the_temporary_copy_alone() {
        let dir = staging_dir("mail-temp");
        let cache = dir.join("INetCache").join("Content.Outlook").join("AB12CD34");
        std::fs::create_dir_all(&cache).unwrap();
        let attachment = cache.join("Quote.pdf");
        let original = one_page_pdf();
        std::fs::write(&attachment, &original).unwrap();
        let kept = dir.join("Quote.pdf");

        let mut app = crate::PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("Quote.pdf", Some(attachment.to_string_lossy().into_owned()), original.clone()).unwrap();
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.pick_override = Some(vec![kept.to_string_lossy().into_owned()]);

        assert!(!app.save_active(SaveTarget::InPlace), "Save asks instead of writing the temporary copy");
        app.process_picked();
        assert_eq!(std::fs::read(&attachment).unwrap(), original, "the attachment's copy is untouched");
        assert!(std::fs::read(&kept).unwrap().starts_with(b"%PDF"), "the document went where the user chose");

        // The chosen file is the document's file now: the next Save writes it without asking.
        app.pick_override = None;
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        assert!(app.save_active(SaveTarget::InPlace));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod revert_draft_tests {
    use super::*;
    use crate::forms_ui::Focus;
    use pdfcraft_engine::FieldValue;

    fn focus(text: &str) -> Focus {
        Focus {
            name: "name".into(),
            widget: 0,
            text: text.into(),
            picked: Vec::new(),
            request_focus: false,
            select_all: false,
            press: None,
            calendar: None,
            calendar_rect: None,
            calendar_text: None,
        }
    }

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        app
    }

    fn queue_draft(app: &mut PdfCraftApp, index: usize, text: &str) {
        app.views[index].forms.focus = Some(focus(text));
        let form = app.session.get(app.views[index].id).unwrap().form.clone();
        crate::forms_ui::commit(&mut app.views[index], &form);
        assert!(app.views[index].forms.committed.is_some() && app.views[index].pending_edit.is_some());
    }

    #[test]
    fn control_quit_prompts_for_uncommitted_typing_and_cancel_retains_it() {
        for queued in [false, true] {
            let mut app = app();
            let id = app.views[0].id;
            app.open_bytes("other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            if queued {
                queue_draft(&mut app, 0, "Unsaved typing");
            } else {
                app.views[0].forms.focus = Some(focus("Unsaved typing"));
            }
            assert!(!app.session.get(id).unwrap().dirty);
            assert_eq!(app.active, Some(1));
            let ctx = egui::Context::default();
            assert!(!app.request_quit(&ctx));
            assert_eq!(app.close_request, Some(CloseRequest::Quit));
            assert!(!app.allow_quit);
            app.resolve_close(&ctx, None);
            assert!(app.close_request.is_none());
            assert_eq!(app.views.len(), 2);
            assert!(app.has_unsaved_work(0));
            assert_eq!(app.views[0].id, id);
            assert!(!app.session.get(id).unwrap().dirty);
            if queued {
                assert!(
                    matches!(&app.views[0].pending_edit, Some(Edit::SetFieldValue { value: FieldValue::Text(text), .. }) if text == "Unsaved typing")
                );
            } else {
                assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Unsaved typing");
            }
        }
    }

    /// A value the field's font can't draw is kept, and the user is told why it shows `?`, in the
    /// interface language; a drawable value says nothing.
    #[test]
    fn a_value_its_font_cannot_draw_is_kept_and_explained() {
        let mut app = app();
        let fill =
            |app: &mut PdfCraftApp, text: &str| app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text(text.into()) });
        assert!(fill(&mut app, "张三"));
        let toast = app.toast.as_ref().map(|t| t.0.clone()).expect("a notice");
        assert!(toast.contains("“张” (U+5F20)") && toast.contains("other PDF viewers redraw the field"), "{toast}");
        assert_eq!(app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value, ["张三"]);
        // The shell sets the drawing language each frame; here, directly (per thread).
        let previous = crate::i18n::current();
        crate::i18n::set_current(crate::i18n::Lang::from_code("zh-hans").expect("registered"));
        assert!(fill(&mut app, "李四"));
        crate::i18n::set_current(previous);
        let toast = app.toast.as_ref().map(|t| t.0.clone()).expect("a notice");
        assert!(toast.contains("此字段的字体无法显示“李”（U+674E）"), "{toast}");
        app.toast = None;
        assert!(fill(&mut app, "Ada"));
        assert!(app.toast.is_none(), "{:?}", app.toast);
    }

    #[test]
    fn unsaved_flag_follows_dirty_state() {
        use std::sync::atomic::Ordering;
        // `guard_quit` runs every frame, also on the web, where no close request ever arrives.
        let ctx = egui::Context::default();
        let mut app = app();
        app.guard_quit(&ctx);
        assert!(!app.unsaved_flag.load(Ordering::Relaxed));
        assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Changed".into()) }));
        app.guard_quit(&ctx);
        assert!(app.unsaved_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn successful_revert_clears_focused_or_queued_drafts_only_on_its_document() {
        for queued in [false, true] {
            let mut app = app();
            let source = app.views[0].id;
            assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Committed change".into()) }));
            if queued {
                queue_draft(&mut app, 0, "Queued source draft");
            } else {
                app.views[0].forms.focus = Some(focus("Focused source draft"));
            }
            app.open_bytes("other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let other = app.views[1].id;
            assert!(app.apply_edit(Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Other committed value".into()) }));
            queue_draft(&mut app, 1, "Other queued draft");
            app.views[1].forms.focus = Some(focus("Other focused draft"));
            let other_bytes = app.session.get(other).unwrap().bytes.clone();
            let other_generation = app.session.get(other).unwrap().edit_generation();
            let source_generation = app.session.get(source).unwrap().edit_generation();
            app.active = Some(0);
            app.revert_active();
            let reverted = app.session.get(source).unwrap();
            assert!(!reverted.dirty);
            assert_eq!(reverted.edit_generation(), source_generation + 1);
            assert!(reverted.form.iter().find(|field| field.name == "name").unwrap().value.is_empty());
            assert!(app.views[0].forms.focus.is_none() && app.views[0].forms.committed.is_none() && app.views[0].pending_edit.is_none());
            assert!(!app.has_unsaved_work(0));
            assert_eq!(app.views[1].forms.focus.as_ref().unwrap().text, "Other focused draft");
            assert_eq!(app.views[1].forms.committed.as_ref().unwrap().text, "Other queued draft");
            assert!(
                matches!(&app.views[1].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Other queued draft")
            );
            let untouched = app.session.get(other).unwrap();
            assert_eq!(untouched.bytes, other_bytes);
            assert_eq!(untouched.edit_generation(), other_generation);
            assert!(untouched.dirty && app.has_unsaved_work(1));
            assert_eq!(app.active_ids(), Some((0, source)));
        }
    }

    #[test]
    fn failed_revert_preserves_focused_and_queued_drafts() {
        let mut app = app();
        let source = app.views[0].id;
        queue_draft(&mut app, 0, "Queued draft");
        app.views[0].forms.focus = Some(focus("Focused draft"));
        // Exercise the ordinary NoDocument error boundary without a corrupt fixture.
        // This verifies UI failure preservation, not other engine refresh failures.
        app.session.close(source);
        app.revert_active();
        assert_eq!(app.views[0].forms.focus.as_ref().unwrap().text, "Focused draft");
        assert_eq!(app.views[0].forms.committed.as_ref().unwrap().text, "Queued draft");
        assert!(
            matches!(&app.views[0].pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Queued draft")
        );
        assert_eq!(app.active_ids(), Some((0, source)));
        assert_eq!(app.toast.as_ref().unwrap().0, pdfcraft_engine::EditError::NoDocument.to_string());
    }
}

#[cfg(test)]
mod content_page_tests {
    use super::content_page;
    use pdfcraft_engine::{Edit, ObjectKind, ObjectTarget};

    /// Single-page content edits keep every other page's rasters; anything else still
    /// invalidates the whole document.
    #[test]
    fn single_page_content_edits_map_to_their_page() {
        let moved = Edit::MoveObjects { page: 3, objects: vec![ObjectTarget { kind: ObjectKind::Text, index: 0 }], offset: [1.0, 0.0] };
        assert_eq!(content_page(&moved), Some(3));
        assert_eq!(content_page(&Edit::RotatePages { pages: vec![0], degrees: 90 }), None);
        assert_eq!(content_page(&Edit::SetInfo { key: "Title".into(), value: "x".into() }), None);
    }
}
