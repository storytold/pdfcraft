//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use pdfcraft_engine::Edit;
use pdfcraft_platform::staging::{StagingName, staging_suffixes};

use crate::PdfCraftApp;

mod close;
mod save;

pub use close::CloseRequest;
#[cfg(not(target_arch = "wasm32"))]
pub use save::PendingOverwrite;
#[cfg(target_arch = "wasm32")]
pub(crate) use save::download;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use save::in_document_folder;
pub use save::{SaveTarget, write_atomically};
#[cfg(test)]
pub(crate) use save::{is_mail_temp_copy, save_dialog_folder};
#[cfg(test)]
use save::{write_atomically_syncing, write_atomically_with};

/// Text typed into a new text box that isn't added to the page yet.
fn new_text_typed(v: &crate::DocView) -> bool {
    v.content.draft.as_ref().is_some_and(|d| d.index.is_none() && !d.text.trim().is_empty())
}

/// Keeps the pages (or comment) the user acted on selected, where they now are. `annots_before`
/// is a deleted annotation's page's annotations before the delete (empty for other edits).
fn select_after_edit(
    view: &mut crate::DocView,
    edit: Edit,
    info: &pdfcraft_render::DocInfo,
    annots_before: &[pdfcraft_render::Annotation],
    quick_tool: crate::QuickTool,
) {
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
                && quick_tool == crate::QuickTool::Comment(crate::comments::CommentTool::Ink) =>
        {
            view.comments.selected = None;
        }
        Edit::AddAnnotation(a) => {
            // Select the new comment (appended last among the page's comments).
            let newest = info.annotations.iter().filter(|x| x.page == a.page && !x.is_reply()).map(|x| x.index).max();
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
        Edit::DeleteAnnotation { page, index } => view.comments.annotation_deleted(annots_before, &info.annotations, (page, index)),
        _ => {}
    }
}

/// The pages whose widgets differ between two form states, and the (field, page) pairs whose
/// value changed and so must not be drawn from the baked page.
fn form_edit_pages(
    before: &[pdfcraft_engine::FormField],
    after: &[pdfcraft_engine::FormField],
) -> (std::collections::BTreeSet<usize>, Vec<(String, usize)>) {
    let mut changed_pages = std::collections::BTreeSet::new();
    let mut unbaked = Vec::new();
    for f in after {
        let pages = |field: &pdfcraft_engine::FormField| field.widgets.iter().filter_map(|w| w.page).collect::<Vec<_>>();
        match before.iter().find(|old| old.name == f.name) {
            Some(old) if old == f => {}
            Some(old) => {
                changed_pages.extend(pages(f));
                changed_pages.extend(pages(old));
                if old.value != f.value {
                    unbaked.extend(pages(f).into_iter().map(|p| (f.name.clone(), p)));
                }
            }
            None => {
                changed_pages.extend(pages(f));
                unbaked.extend(pages(f).into_iter().map(|p| (f.name.clone(), p)));
            }
        }
    }
    for old in before.iter().filter(|old| !after.iter().any(|f| f.name == old.name)) {
        changed_pages.extend(old.widgets.iter().filter_map(|w| w.page));
    }
    (changed_pages, unbaked)
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
        // A deletion moves the comments after it on the page: these say where the selected one,
        // and the one open in the panel's editor, went.
        let annots_before = match (&edit, self.session.get(id)) {
            (Edit::DeleteAnnotation { page, .. }, Some(d)) => d.info.annotations.iter().filter(|a| a.page == *page).cloned().collect(),
            _ => Vec::new(),
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
                    let (changed_pages, unbaked) = form_edit_pages(&form_before, &doc.form);
                    view.forms.unbaked.extend(unbaked);
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
                select_after_edit(view, edit, info, &annots_before, self.quick_tool);
                if std::mem::take(&mut view.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                // Text saved as typed but drawn with `?` (its font can't draw it), or pages whose
                // own backdrop stays off: said, not silent. Before the scripts' output, so an alert
                // they raise still shows.
                if let Some(n) = self.session.get(id).and_then(|d| d.edit_notices.first()).cloned() {
                    self.notify(match &n {
                        pdfcraft_engine::EditNotice::Undrawn(t) => crate::content_ui::undrawn_message(t),
                        pdfcraft_engine::EditNotice::BackdropsLeftOff { pages } => crate::marks_ui::backdrops_left_off_message(pages),
                    });
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
        } else if !self.undo_in_page_box() {
            self.history_step(true);
        }
    }

    /// Undo from the menu or toolbar while a box on the page holds typing the page doesn't have
    /// yet: the box takes the step, as it does for the key, one step of its typing at a time.
    /// Applying the typing first was a new edit, which dropped the document's Redo, and then the
    /// typing was taken back all at once. True when the box took the step.
    fn undo_in_page_box(&mut self) -> bool {
        let Some(ctx) = self.ctx.clone() else { return false };
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return false };
        let doc = view.id;
        if let Some(ed) = view.line_editor.as_mut().filter(|ed| ed.has_unsaved_text()) {
            ed.claim_typing(&ctx, doc);
            let Some(text) = typing_step(&ctx, crate::edit_text_ui::input_id(), &ed.text, true) else { return false };
            let before = std::mem::replace(&mut ed.text, text);
            ed.selection = None;
            ed.follow_typed_rows(&ctx, &before);
            return true;
        }
        if let Some(d) = view.content.draft.as_mut().filter(|d| d.has_unapplied_typing()) {
            d.claim_typing(&ctx, doc);
            let Some(text) = typing_step(&ctx, crate::content_ui::input_id(), &d.text, true) else { return false };
            let before = std::mem::replace(&mut d.text, text);
            d.selection = None;
            crate::edit_text_ui::follow_typing(&ctx, crate::content_ui::input_id(), &before, &d.text, &mut [&mut d.style.aligns]);
            return true;
        }
        false
    }

    pub fn redo(&mut self) {
        if self.combine_showing() {
            self.combine_history_step(false);
        } else if !self.redo_in_page_box() {
            self.history_step(false);
        }
    }

    /// Redo from the menu or toolbar while a box on the page has typing of its own: the step is
    /// the box's, as the key's is. What Undo took back in the box comes back; typing the page
    /// doesn't have yet stays as it is, since applying it first is a new edit, which drops the
    /// document's Redo. True when the box took the step.
    fn redo_in_page_box(&mut self) -> bool {
        let Some(ctx) = self.ctx.clone() else { return false };
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return false };
        let doc = view.id;
        if let Some(ed) = view.line_editor.as_mut() {
            ed.claim_typing(&ctx, doc);
            let Some(text) = typing_step(&ctx, crate::edit_text_ui::input_id(), &ed.text, false) else { return ed.has_unsaved_text() };
            let before = std::mem::replace(&mut ed.text, text);
            ed.selection = None;
            ed.follow_typed_rows(&ctx, &before);
            return true;
        }
        if let Some(d) = view.content.draft.as_mut() {
            d.claim_typing(&ctx, doc);
            let Some(text) = typing_step(&ctx, crate::content_ui::input_id(), &d.text, false) else { return d.has_unapplied_typing() };
            let before = std::mem::replace(&mut d.text, text);
            d.selection = None;
            crate::edit_text_ui::follow_typing(&ctx, crate::content_ui::input_id(), &before, &d.text, &mut [&mut d.style.aligns]);
            return true;
        }
        false
    }

    fn history_step(&mut self, undo: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        if !self.close_text_editors(i, id) {
            return;
        }
        let result = if undo { self.session.undo(id) } else { self.session.redo(id) };
        match result {
            Ok(label) => {
                if let Some(doc) = self.session.get(id) {
                    self.views[i].document_changed(&doc.info);
                }
                self.views[i].comments.history_stepped();
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

    /// Before Undo or Redo: a paragraph or added text box open on the page shows the document as
    /// it is before the step, so it is applied as clicking away applies it, then closed. Left
    /// open, it kept showing what the step took away, and wrote from it when it was next applied.
    /// What was typed is then the step Undo takes back. False (nothing is undone, and the box
    /// stays open) while it holds text the standard fonts can't draw, or text that couldn't be
    /// applied: it is shown, never lost.
    pub(crate) fn close_text_editors(&mut self, i: usize, id: pdfcraft_engine::DocId) -> bool {
        let Some(view) = self.views.get_mut(i) else { return true };
        if let Some(c) = view.content.blocked() {
            view.content.hold_blocked();
            self.notify(crate::content_ui::undrawable_message(c));
            return false;
        }
        if let Some(ed) = view.line_editor.take()
            && let Some(edit) = crate::edit_text_ui::finished_edit(ed.clone())
            && !self.apply_edit(edit)
        {
            if let Some(view) = self.views.get_mut(i) {
                view.line_editor = Some(ed);
            }
            return false;
        }
        let added = self.session.get(id).map(|d| d.added().to_vec()).unwrap_or_default();
        let Some(view) = self.views.get_mut(i) else { return true };
        // Finishing new text also asks for it to be selected once it exists: put that back too.
        let (draft, select_added) = (view.content.draft.clone(), view.content.select_added);
        if let Some(edit) = view.content.finish_draft(&added)
            && !self.apply_edit(edit)
        {
            if let Some(view) = self.views.get_mut(i) {
                (view.content.draft, view.content.select_added) = (draft, select_added);
            }
            return false;
        }
        true
    }

    /// Whether the keyboard is in a box typed into on the page: a paragraph being edited, or added
    /// text.
    pub(crate) fn page_box_focused(&self, ctx: &egui::Context) -> bool {
        let focused = ctx.memory(|m| m.focused());
        focused == Some(crate::edit_text_ui::input_id()) || focused == Some(crate::content_ui::input_id())
    }

    /// Whether the focused box on the page has typing its own Redo would put back: after Undo in
    /// the box restores what the page has, Redo still belongs to the box, not the document. The
    /// same test egui makes when it handles the key.
    pub(crate) fn page_box_can_redo(&self, ctx: &egui::Context) -> bool {
        let Some(view) = self.active.and_then(|i| self.views.get(i)) else { return false };
        let focused = ctx.memory(|m| m.focused());
        let (id, text) = if focused == Some(crate::edit_text_ui::input_id())
            && let Some(ed) = &view.line_editor
        {
            (crate::edit_text_ui::input_id(), &ed.text)
        } else if focused == Some(crate::content_ui::input_id())
            && let Some(d) = &view.content.draft
        {
            (crate::content_ui::input_id(), &d.text)
        } else {
            return false;
        };
        has_typing_to_redo(ctx, id, text)
    }

    /// Whether the box open on the page has a Redo of its own, the step
    /// [`Self::redo_in_page_box`] takes for Redo from the menu or toolbar: focus has moved to the
    /// menu by then, so unlike [`Self::page_box_can_redo`] this doesn't ask for it.
    pub(crate) fn open_box_can_redo(&self) -> bool {
        let Some(ctx) = &self.ctx else { return false };
        let Some(view) = self.active.and_then(|i| self.views.get(i)) else { return false };
        if let Some(ed) = &view.line_editor {
            return has_typing_to_redo(ctx, crate::edit_text_ui::input_id(), &ed.text);
        }
        view.content.draft.as_ref().is_some_and(|d| has_typing_to_redo(ctx, crate::content_ui::input_id(), &d.text))
    }

    /// Typing in a box on the page (a paragraph being edited, added text) that the page doesn't
    /// have yet: Undo and Redo keys take it back in the box first.
    pub(crate) fn page_typing_unapplied(&self) -> bool {
        self.active.and_then(|i| self.views.get(i)).is_some_and(|v| {
            v.line_editor.as_ref().is_some_and(|ed| ed.has_unsaved_text()) || v.content.draft.as_ref().is_some_and(|d| d.has_unapplied_typing())
        })
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
            Some(crate::canvas::ViewAction::SplitBeforeSelected) => {
                self.split_draft.mode = crate::SplitMode::Selection;
                self.dialog = Some(crate::Dialog::Split);
            }
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
                focus.text = crate::forms_ui::draft_text(field);
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
                    .and_then(|d| d.info.annotations.iter().filter(|a| a.page == page && !a.is_reply()).map(|a| a.index).max());
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
        let typing = self.views.get(i).is_some_and(|v| {
            v.pending_edit.is_some()
                || v.forms.focus.is_some()
                || v.content.draft.is_some()
                || v.line_editor.as_ref().is_some_and(|ed| ed.has_changes())
                || v.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
        });
        if self.active != Some(i) {
            if !typing {
                return true;
            }
            self.active = Some(i);
        }
        self.commit_form_typing() && self.commit_open_fill_text(id) && self.commit_open_text_edits(id)
    }

    /// The Edit a PDF ▸ Text boxes (new text, and a paragraph open for editing) commit on
    /// click-away. Save does that first, so the typed text is in the file instead of being left
    /// open on the screen (#3496).
    fn commit_open_text_edits(&mut self, id: pdfcraft_engine::DocId) -> bool {
        let Some(doc) = self.session.get(id) else { return true };
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else { return true };
        // Both are taken out of the view before either edit runs, so keep what is needed to put
        // back whichever one the document refuses: typed text must not be lost.
        let (draft, select_added, editor) = (view.content.draft.clone(), view.content.select_added, view.line_editor.clone());
        let added = view.content.finish_draft(doc.added());
        let paragraph = view.line_editor.take_if(|ed| ed.has_changes()).and_then(crate::edit_text_ui::finished_edit);
        let added_ok = added.is_none_or(|edit| self.apply_edit(edit));
        let paragraph_ok = paragraph.is_none_or(|edit| self.apply_edit(edit));
        if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
            if !added_ok {
                (view.content.draft, view.content.select_added) = (draft, select_added);
            }
            if !paragraph_ok {
                view.line_editor = editor;
            }
        }
        added_ok && paragraph_ok
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
        let edit = crate::fill_sign::typed_with_color(tb.page, tb.at, &text, tb.size, rotation, &author, tb.color);
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
            || new_text_typed(v)
            || v.line_editor.as_ref().map_or_else(|| false, |ed| ed.has_unsaved_text())
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
            | Edit::DeleteWidget { .. }
            | Edit::DuplicateField { .. }
            | Edit::SetFieldProps { .. }
            | Edit::SetFieldImage { .. }
            | Edit::SetFieldScript { .. }
            | Edit::SetCalculationOrder { .. }
            | Edit::ApplyScriptChanges { .. }
    )
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
            // A dialog chooses the styles; it makes the edit.
            A::FromTextStyles => {
                self.open_text_style_bookmarks(id);
                return;
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

/// Whether the page text box `id`, showing `text`, has typing its own Redo would put back: the
/// test egui makes when it handles the key.
fn has_typing_to_redo(ctx: &egui::Context, id: egui::Id, text: &str) -> bool {
    egui::TextEdit::load_state(ctx, id)
        .is_some_and(|state| state.cursor.char_range().is_some_and(|range| state.undoer().has_redo(&(range, text.to_owned()))))
}

/// The box's own Undo (or Redo), as the key does it: the box's text one step back (or forward) in
/// its typing, with the box's history and cursor moved along. None when there is no such step.
fn typing_step(ctx: &egui::Context, id: egui::Id, text: &str, undo: bool) -> Option<String> {
    let mut state = egui::TextEdit::load_state(ctx, id)?;
    let mut undoer = state.undoer();
    let current = (state.cursor.char_range()?, text.to_owned());
    let (range, stepped) = if undo { undoer.undo(&current) } else { undoer.redo(&current) }?.clone();
    state.set_undoer(undoer);
    state.cursor.set_char_range(Some(range));
    egui::TextEdit::store_state(ctx, id, state);
    Some(stepped)
}

#[cfg(test)]
mod tests;

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
            had_focus: false,
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

    /// Text typed into a new text box is lost on close unless the save prompt appears; an empty
    /// box has nothing to lose.
    #[test]
    fn text_typed_into_a_new_text_box_counts_as_unsaved_work() {
        use crate::content_ui::{TextDraft, default_style};
        let mut app = app();
        let draft = |text: &str| TextDraft {
            page: 0,
            index: None,
            rect: [0.0; 4],
            text: text.into(),
            style: default_style(),
            focus: true,
            selection: None,
            typing_from: None,
        };
        assert!(!app.has_unsaved_work(0));
        app.views[0].content.draft = Some(draft("  \n"));
        assert!(!app.has_unsaved_work(0));
        app.views[0].content.draft = Some(draft("Typed"));
        assert!(!app.session.get(app.views[0].id).unwrap().dirty);
        assert!(app.has_unsaved_work(0));
    }

    /// Close ▸ Save, and Quit ▸ Save, write the text typed into a new text box before the tab
    /// closes.
    #[test]
    fn saving_on_close_keeps_text_typed_into_a_new_text_box() {
        use crate::content_ui::{TextDraft, default_style};
        use pdfcraft_engine::AddedContent;
        for quit in [false, true] {
            let dir = std::env::temp_dir().join(format!("pdfcraft-ui-new-text-{}-{quit}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("source.pdf").to_string_lossy().into_owned();
            let bytes = include_bytes!("../tests/data/form.pdf").to_vec();
            std::fs::write(&path, &bytes).unwrap();
            let mut app = PdfCraftApp::new();
            app.open_bytes("source.pdf", Some(path.clone()), bytes).unwrap();
            let rect = [72.0, 72.0, 272.0, 92.0];
            app.views[0].content.draft = Some(TextDraft {
                page: 0,
                index: None,
                rect,
                text: "Typed".into(),
                style: default_style(),
                focus: true,
                selection: None,
                typing_from: None,
            });
            let ctx = egui::Context::default();
            if quit {
                assert!(!app.request_quit(&ctx));
            } else {
                app.request_close_tab(0);
            }
            assert!(app.close_request.is_some());
            app.resolve_close(&ctx, Some(true));
            assert!(app.views.is_empty(), "the tab closes once saved");

            let mut reopened = PdfCraftApp::new();
            reopened.open_bytes("source.pdf", Some(path.clone()), std::fs::read(&path).unwrap()).unwrap();
            let doc = reopened.session.get(reopened.views[0].id).unwrap();
            assert!(doc.added().iter().any(|a| matches!(&a.content, AddedContent::Text(t) if t.text == "Typed")), "quit: {quit}");
            let _ = std::fs::remove_dir_all(&dir);
        }
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

#[cfg(test)]
mod attention_tests {
    use super::CloseRequest;
    use crate::PdfCraftApp;

    /// The attention requests one frame of `attend_save_prompt` sends, with the window `focused`
    /// or not.
    fn attend(app: &mut PdfCraftApp, ctx: &egui::Context, focused: bool) -> Vec<egui::UserAttentionType> {
        let mut out = ctx.run_ui(egui::RawInput { focused, ..Default::default() }, |ui| app.attend_save_prompt(ui.ctx()));
        // The frame's font texture isn't uploaded anywhere in a unit test.
        out.textures_delta.clear();
        out.viewport_output
            .values()
            .flat_map(|v| v.commands.iter())
            .filter_map(|c| if let egui::ViewportCommand::RequestUserAttention(a) = c { Some(*a) } else { None })
            .collect()
    }

    /// A save that finishes after a newer "save changes?" question opened in front, and after the
    /// user went to another app, asks for no attention: the user saw that question.
    #[test]
    fn a_save_finishing_behind_a_newer_question_the_user_saw_asks_for_nothing() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes(name, None, bytes.as_ref().clone()).unwrap();
            app.active = Some(app.views.len() - 1);
            assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        }
        // a.pdf was answered with Save while quitting. While that save is pending, the user closes
        // c.pdf's tab, and its question opens in front.
        let c = app.views[2].id;
        app.close_request = Some(CloseRequest::Tab(c));
        assert!(attend(&mut app, &ctx, true).is_empty());
        // a.pdf's save finishes once the user has gone to another app.
        app.close_and_continue(&ctx, 0, CloseRequest::Quit);
        assert_eq!(app.close_request, Some(CloseRequest::Tab(c)), "the newer question stands");
        assert!(attend(&mut app, &ctx, false).is_empty(), "the user has seen it");
    }
}
