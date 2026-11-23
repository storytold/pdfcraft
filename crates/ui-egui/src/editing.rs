//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use craft::behavior::edit_history::Step;
use pdfcraft_engine::Edit;

use crate::PdfCraftApp;

mod close;
pub(crate) mod save;

pub use craft::dialogs::save::SaveTarget;
pub use save::PendingLock;
#[cfg(test)]
use save::unredacted_file_name;

/// Text typed into a new text box that isn't added to the page yet.
fn new_text_typed(v: &crate::DocView) -> bool {
    v.session.content.draft.as_ref().is_some_and(|d| d.index.is_none() && !d.text.trim().is_empty())
}

/// Clear an unfinished page-anchored drawing or composer when a page edit shifts or removes `page`.
fn clear_shifted_page_comment_state(view: &mut crate::DocView, shifted_from: usize) {
    // The shape being clicked out is on the tool's page.
    if !view.session.comments.tool.drawing().is_empty() && view.session.comments.tool.page >= shifted_from {
        view.session.comments.tool.cancel_drawing();
    }
    if view.session.comments.composer.as_ref().is_some_and(|c| !matches!(c.kind, crate::comments::ComposerKind::Edit(_)) && c.page >= shifted_from) {
        view.session.comments.composer = None;
    }
}

/// Keeps the pages (or comment) the user acted on selected, where they now are. `annots_before`
/// is the page's annotations before a delete or a paste of comments (empty for other edits);
/// `across_pages`, for markup on a text selection across pages, the page that selection ends on.
fn select_after_edit(
    view: &mut crate::DocView,
    edit: &Edit,
    info: &pdfcraft_render::DocInfo,
    annots_before: &[pdfcraft_render::Annotation],
    quick_tool: crate::QuickTool,
    across_pages: Option<usize>,
) {
    match edit {
        Edit::MovePages { pages, to } => {
            clear_shifted_page_comment_state(view, 0);
            let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
            let to = (*to).min(info.pages.len() - n);
            view.select_pages(&(to..to + n).collect::<Vec<_>>());
        }
        Edit::DuplicatePages { pages, at: Some(at) } => {
            clear_shifted_page_comment_state(view, *at);
            let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
            let at = (*at).min(info.pages.len().saturating_sub(n));
            view.select_pages(&(at..at + n).collect::<Vec<_>>());
        }
        Edit::InsertBlankPage { at, .. } => {
            clear_shifted_page_comment_state(view, *at);
            view.select_pages(&[(*at).min(info.pages.len() - 1)]);
        }
        Edit::DeletePages { pages } => {
            if let Some(&first) = pages.iter().min() {
                clear_shifted_page_comment_state(view, first);
            }
            view.select_pages(&[]);
        }
        // A stroke drawn with the pen stays unselected, so the selection box and
        // author popup don't sit over the next stroke (#429).
        Edit::AddAnnotation(a)
            if matches!(a.shape, pdfcraft_engine::Shape::Ink { .. } | pdfcraft_engine::Shape::FreehandHighlight { .. })
                && matches!(
                    quick_tool,
                    crate::QuickTool::Comment(crate::comments::CommentTool::Ink | crate::comments::CommentTool::FreehandHighlight)
                ) =>
        {
            view.session.comments.clear_selection();
        }
        Edit::AddAnnotation(a) => {
            // Select the new comment alone (appended last among the page's comments).
            let newest = info.annotations.iter().filter(|x| x.page == a.page && !x.is_reply()).map(|x| x.index).max();
            view.session.comments.clear_selection();
            if let Some(n) = newest {
                view.session.comments.select_keys(&[(a.page, n)]);
            }
            view.session.comments.reveal = true;
            // A new highlight opens its note for typing straight away, as in Acrobat,
            // unless a note typed into another card is still unsaved.
            if let (pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, .. }, Some(n)) = (&a.shape, newest)
                && a.contents.is_empty()
                && view.session.comments.panel.editing.is_none()
            {
                view.session.comments.open_editor(a.page, n, String::new());
            }
        }
        // Markup on a text selection across pages (marked so when it was made,
        // [`crate::DocView::selection_marks_edit`]; other batches of comments, such as Redact's
        // page range or search, are not) is followed up as one markup is: on the page the
        // selection ends, where the drag finished and is on screen, its comment selected and, for
        // a highlight, its note open. One level: the batch holds only `AddAnnotation`s.
        Edit::Batch { edits, .. } if across_pages.is_some() && edits.iter().all(|e| matches!(e, Edit::AddAnnotation(_))) => {
            // The selection's pages are in page order: dragged back up, it ends on the first.
            let on_head = edits.iter().find(|e| matches!(e, Edit::AddAnnotation(a) if Some(a.page) == across_pages));
            if let Some(head) = on_head.or(edits.last()) {
                select_after_edit(view, head, info, &[], quick_tool, None);
            }
        }
        // A group moved together stays selected (its positions don't change), so it
        // can be moved or copied again; anything else in a batch clears the selection.
        Edit::Batch { edits, .. } if edits.iter().all(|e| matches!(e, Edit::MoveAnnotation { .. })) => {}
<<<<<<< HEAD
        Edit::Batch { edits, .. } => {
            view.session.comments.clear_selection();
        }
=======
        // Set Page Boxes over pages with different /UserUnit is a batch too; like one SetPageBox it keeps the selection.
        Edit::Batch { edits, .. } if edits.iter().all(|e| matches!(e, Edit::SetPageBox { .. })) => {}
        Edit::Batch { .. } => view.comments.clear_selection(),
>>>>>>> upstream/main
        // The copies the paste added (and nothing that was there before): select them all.
        Edit::PasteComments { page, .. } => {
            let new: Vec<(usize, usize)> = info
                .annotations
                .iter()
                .filter(|x| x.page == *page && !x.is_reply() && !annots_before.iter().any(|b| b.name == x.name))
                .map(|x| (*page, x.index))
                .collect();
            view.session.comments.clear_selection();
            if !new.is_empty() {
                // The first copy leads, as it did when it was the primary.
                view.session.comments.select_keys(&new);
                view.session.comments.reveal = true;
            }
        }
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
    pub fn apply_edit(&mut self, ctx: &egui::Context, edit: Edit) -> bool {
        let Some((i, id)) = self.active_ids() else { return false };
        // Taken whatever happens to this edit, so it never carries over to another.
        let across_pages = std::mem::take(&mut self.views[i].session.marks_across_pages).filter(|_| annotated_pages(&edit).is_some());
        let label = edit.label();
        let target_arrange_image = match &edit {
            Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Arrange(_) } => {
                let doc = self.session.get(id);
                let target = doc.and_then(|d| d.page_images(*page).get(*index).cloned());
                let ambiguous = doc.is_some_and(|d| {
                    if let Some(t) = &target {
                        d.page_images(*page)
                            .iter()
                            .filter(|img| img.name == t.name && img.object == t.object && rect_matches(img.rect, t.rect))
                            .count()
                            > 1
                    } else {
                        false
                    }
                });
                target.map(|t| (t, ambiguous))
            }
            _ => None,
        };
        let (form_before, page_count_before, had_xfa, signatures_before) = if let Some(d) = self.session.get(id) {
            (Some(std::sync::Arc::clone(&d.form)), d.info.pages.len(), d.xfa.is_some(), Some(std::sync::Arc::clone(&d.signatures)))
        } else {
            (None, 0, false, None)
        };
        // A deletion moves the comments after it on the page: these say where the selected one,
        // and the one open in the panel's editor, went.
        let annots_before = match (&edit, self.session.get(id)) {
            // A paste selects what it adds: the page's comments before it are the ones it didn't.
            (Edit::DeleteAnnotation { page, .. } | Edit::PasteComments { page, .. }, Some(d)) => {
                d.info.annotations.iter().filter(|a| a.page == *page).cloned().collect()
            }
            _ => Vec::new(),
        };
        // The comments the pop-ups hold unsaved text or a drag for, to see afterwards whether
        // their places still name them (pages may move, come or go).
        let popups_before: Vec<pdfcraft_render::Annotation> = match (self.views.get(i), self.session.get(id)) {
            (Some(v), Some(d)) => {
                let keys = v.session.comments.popup_keys();
                d.info.annotations.iter().filter(|a| keys.contains(&(a.page, a.index))).cloned().collect()
            }
            _ => Vec::new(),
        };
        match self.session.apply(id, edit.clone()) {
            Ok(()) => {
                let Some(doc) = self.session.get(id) else { return true };
                let info = &doc.info;
                let view = &mut self.views[i];
                view.session.applied_editor = None;
                // A picture picked with the Select tool may be the one this edit removed or moved:
                // drop its highlight now, not on the next frame (which only comes with the next input).
                view.session.picked_image = None;
                self.wake.wake();
                if matches!(edit, Edit::AddAnnotation(_)) {
                    // A released drawing's raster can come now: it will have the annotation.
                    view.session.comments.drawings_committed();
                } else {
                    // Only a new drawing waits on its raster; any other edit may remove or move it.
                    view.session.comments.settling.clear();
                }
                view.session.signature_drag.committed(&edit, doc.edit_generation());
                view.session.move_background.committed(&edit, doc.edit_generation());
                view.session.text_background.committed(&edit);
                // A form edit that leaves the pages and XFA as they were: the pages its fields are
                // on, and the values that show over them until those pages' rasters have them.
                let form_edit = form_before
                    .filter(|_| doc.info.pages.len() == page_count_before && doc.xfa.is_some() == had_xfa && doc.xfa.is_none() && is_form_edit(&edit))
                    .map(|before| form_edit_pages(&before, &doc.form));
                let one_page = comment_page(&edit).or_else(|| content_page(&edit));
                if signatures_before.is_some_and(|before| crate::canvas::signature_states_changed(&before, &doc.signatures)) {
                    signatures_changed_after_edit(ctx, view, &edit, info, &annots_before, form_edit.map(|(_, u)| u), one_page);
                } else if let Some(page) = one_page {
                    // Comment and content edits change one page: keep every other raster.
                    one_page_changed(view, &edit, page, &annots_before, info);
                } else if let Some(pages) = annotated_pages(&edit) {
                    // Markup on a selection across pages adds one comment on each of them: those
                    // pages' rasters, then the comments once, however many pages.
                    pages.into_iter().for_each(|page| view.page_changed(page));
                    view.session.comments.document_changed(info);
                } else if let Some((changed_pages, unbaked)) = form_edit {
                    view.session.forms.unbaked.extend(unbaked);
                    for page in changed_pages {
                        view.page_changed(page);
                    }
                    view.session.comments.document_changed(info);
                } else {
                    view.document_changed(ctx, info);
                }
                if let Some(selection) = view.session.image_selection.as_mut()
                    && selection.settled.is_some()
                {
                    let (document, page) = view.revisions.of(selection.page);
                    selection.settled_revision = Some(craft::pages::ui::PageRevision { document, page });
                }
                if matches!(edit, Edit::AddAnnotation(_)) || across_pages.is_some() {
                    view.session.comments.comment_added();
                }
                // A deletion moves the pop-ups' state with its comments (`annotation_deleted`).
                if !matches!(edit, Edit::DeleteAnnotation { .. }) {
                    view.session.comments.popups_after_edit(&popups_before, &info.annotations);
                }
                // Keep the pages the user acted on selected, where they now are.
                select_after_edit(view, &edit, info, &annots_before, self.quick_tool, across_pages);
                if let (Some((target, ambiguous_before)), Edit::EditPageImage { page, .. }) = (target_arrange_image, &edit) {
                    let page_images = doc.page_images(*page);
                    re_resolve_image_selection(view, *page, &target, ambiguous_before, &page_images);
                }
                if std::mem::take(&mut view.session.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                // Text saved as typed but drawn with `?` (its font can't draw it), or pages whose
                // own backdrop stays off, or places applied redactions left holding the removed text
                // (the Apply redactions dialog then says it after its count): said, not silent.
                // Before the scripts' output, so an alert they raise still shows.
                if let Some(n) = self.session.get(id).and_then(|d| d.edit_notices.first()).cloned() {
                    self.notify(match &n {
                        pdfcraft_engine::EditNotice::Undrawn(t) => crate::content_ui::undrawn_message(t),
                        pdfcraft_engine::EditNotice::BackdropsLeftOff { pages } => crate::marks_ui::backdrops_left_off_message(pages),
                        pdfcraft_engine::EditNotice::RedactionElsewhere(e) => {
                            crate::redact_ui::still_holding_note(e).unwrap_or_else(|| n.to_string())
                        }
                        pdfcraft_engine::EditNotice::RedactionIdentification(id) => {
                            crate::redact_ui::identification_note(id).unwrap_or_else(|| n.to_string())
                        }
                    });
                }
                let out = self.session.take_js_output(id);
                self.handle_js(ctx, id, out);
                true
            }
            Err(e) => {
                if let Some(view) = self.views.get_mut(i) {
                    // A drawing whose annotation wasn't added must not stay painted.
                    view.session.comments.drawings_refused();
                    if matches!(&edit, Edit::EditPageImage { change, .. } if !matches!(change, pdfcraft_engine::ImageEdit::Arrange(_))) {
                        view.session.image_selection = None;
                    }
                    // A paragraph that couldn't be applied opens again with its typing, instead of
                    // the box the click moved to.
                    if let Edit::EditTextBlock { page, block, .. } = &edit
                        && let Some(mut ed) = view.session.applied_editor.take().filter(|ed| (ed.page, ed.block) == (*page, *block))
                    {
                        ed.focus = true;
                        view.session.line_editor = Some(ed);
                        view.session.pending_text_open = None;
                    }
                }
                let message = if matches!(edit, Edit::MoveObjects { .. }) { crate::object_ui::move_error(&e) } else { e.to_string() };
                let text = crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", &crate::i18n::edit_label(&label)), ("e", &message)]);
                self.last_edit_refusal = Some(text.clone());
                self.notify(text);
                false
            }
        }
    }

    /// Undo the last change: to the Combine files list while its tab shows, else the document.
    pub fn undo(&mut self, ctx: &egui::Context) {
        // While a polygon, cloud or run of connected lines is being drawn, Undo takes back its
        // last point instead of the last finished edit.
        if let Some((i, _)) = self.active_ids()
            && self.views[i].session.comments.tool.undo_point()
        {
            return;
        }
        if self.combine_showing() {
            self.combine_history_step(ctx, true);
        } else if !self.undo_in_page_box() {
            if let Some((i, _)) = self.active_ids()
                && self.views[i].session.comments.panel.editing.as_ref().is_some_and(|e| e.1 > 0)
            {
                self.views[i].session.comments.panel.editing = None;
            }
            self.history_step(ctx, true);
        }
    }

    /// Undo from the menu or toolbar while a box on the page holds typing the page doesn't have
    /// yet: the box takes the step, as it does for the key, one step of its typing at a time.
    /// Applying the typing first was a new edit, which dropped the document's Redo, and then the
    /// typing was taken back all at once. True when the box took the step.
    fn undo_in_page_box(&mut self) -> bool {
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return false };
        if let Some(ed) = view.session.line_editor.as_mut().filter(|ed| ed.has_unsaved_text()) {
            return step_typing(&mut ed.history, &mut ed.text, &mut ed.selection, &mut ed.look.aligns, Step::Undo);
        }
        if let Some(d) = view.session.content.draft.as_mut().filter(|d| d.has_unapplied_typing()) {
            return step_typing(&mut d.history, &mut d.text, &mut d.selection, &mut d.style.aligns, Step::Undo);
        }
        false
    }

    pub fn redo(&mut self, ctx: &egui::Context) {
        if self.combine_showing() {
            self.combine_history_step(ctx, false);
        } else if !self.redo_in_page_box() {
            if let Some((i, _)) = self.active_ids()
                && self.views[i].session.comments.panel.editing.as_ref().is_some_and(|e| e.1 > 0)
            {
                self.views[i].session.comments.panel.editing = None;
            }
            self.history_step(ctx, false);
        }
    }

    /// Redo from the menu or toolbar while a box on the page has typing of its own: the step is
    /// the box's, as the key's is. What Undo took back in the box comes back; typing the page
    /// doesn't have yet stays as it is, since applying it first is a new edit, which drops the
    /// document's Redo. True when the box took the step.
    fn redo_in_page_box(&mut self) -> bool {
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return false };
        if let Some(ed) = view.session.line_editor.as_mut() {
            return step_typing(&mut ed.history, &mut ed.text, &mut ed.selection, &mut ed.look.aligns, Step::Redo) || ed.has_unsaved_text();
        }
        if let Some(d) = view.session.content.draft.as_mut() {
            return step_typing(&mut d.history, &mut d.text, &mut d.selection, &mut d.style.aligns, Step::Redo) || d.has_unapplied_typing();
        }
        false
    }

    fn history_step(&mut self, ctx: &egui::Context, undo: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        if !self.close_text_editors(ctx, i, id) {
            return;
        }
        let target_image = self.views.get(i).and_then(|v| v.session.image_selection.as_ref()).and_then(|s| {
            let doc = self.session.get(id)?;
            let images = doc.page_images(s.page);
            let target = images.get(s.index).cloned()?;
            let ambiguous =
                images.iter().filter(|img| img.name == target.name && img.object == target.object && rect_matches(img.rect, target.rect)).count() > 1;
            Some((s.page, target, ambiguous))
        });
        let had_selection = self.views.get(i).and_then(|v| v.session.image_selection.as_ref()).is_some();
        let result = if undo { self.session.undo(id) } else { self.session.redo(id) };
        match result {
            Ok(label) => {
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.get_mut(i)
                {
                    view.document_changed(ctx, &doc.info);
                    view.session.picked_image = None;
                    if let Some((page, target, ambiguous_before)) = target_image {
                        let page_images = doc.page_images(page);
                        re_resolve_image_selection(view, page, &target, ambiguous_before, &page_images);
                    } else if had_selection {
                        view.session.image_selection = None;
                    }
                }
                if let Some(view) = self.views.get_mut(i) {
                    view.session.comments.history_stepped();
                }
                if label == (Edit::ApplyRedactions { pages: None }).label() {
                    let view = &mut self.views[i];
                    let (applied, undone) = (view.redactions_applied, view.redactions_undone_after_save);
                    // Undo past the last save counts as undone; Redo first takes those back.
                    match (undo, applied, undone) {
                        (true, 0, _) => view.redactions_undone_after_save = undone.saturating_add(1),
                        (true, _, _) => view.redactions_applied = applied.saturating_sub(1),
                        (false, _, 0) => view.redactions_applied = applied.saturating_add(1),
                        (false, _, _) => view.redactions_undone_after_save = undone.saturating_sub(1),
                    }
                }
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
    pub(crate) fn close_text_editors(&mut self, ctx: &egui::Context, i: usize, id: pdfcraft_engine::DocId) -> bool {
        let Some(view) = self.views.get_mut(i) else { return true };
        if let Some(c) = view.session.content.blocked() {
            view.session.content.hold_blocked();
            self.notify(crate::content_ui::undrawable_message(c));
            return false;
        }
        if let Some(ed) = view.session.line_editor.take()
            && let Some(edit) = crate::edit_text_ui::finished_edit(ed.clone())
            && !self.apply_edit(ctx, edit)
        {
            if let Some(view) = self.views.get_mut(i) {
                view.session.line_editor = Some(ed);
            }
            return false;
        }
        let added = self.session.get(id).map(|d| d.added().to_vec()).unwrap_or_default();
        let Some(view) = self.views.get_mut(i) else { return true };
        // Finishing new text also asks for it to be selected once it exists: put that back too.
        let (draft, select_added) = (view.session.content.draft.clone(), view.session.content.select_added);
        if let Some(edit) = view.session.content.finish_draft(&added)
            && !self.apply_edit(ctx, edit)
        {
            if let Some(view) = self.views.get_mut(i) {
                (view.session.content.draft, view.session.content.select_added) = (draft, select_added);
            }
            return false;
        }
        true
    }

    /// Whether the keyboard is in a box typed into on the page: a paragraph being edited, or added
    /// text.
    // PLAN: callers still pass a ctx (`commands.rs:880`, `:882`, owner pdf-hub-w3): drop the argument there
    pub(crate) fn page_box_focused(&self) -> bool {
        self.active
            .and_then(|i| self.views.get(i))
            .is_some_and(|v| v.session.line_editor.as_ref().is_some_and(|ed| ed.focus) || v.session.content.draft.as_ref().is_some_and(|d| d.focus))
    }

    /// Whether the focused box on the page has typing its own Redo would put back: after Undo in
    /// the box restores what the page has, Redo still belongs to the box, not the document.
    pub(crate) fn page_box_can_redo(&self) -> bool {
        let Some(view) = self.active.and_then(|i| self.views.get(i)) else { return false };
        if let Some(ed) = view.session.line_editor.as_ref().filter(|ed| ed.focus) {
            return ed.history.can_redo();
        }
        view.session.content.draft.as_ref().filter(|d| d.focus).is_some_and(|d| d.history.can_redo())
    }

    /// Whether the box open on the page has a Redo of its own, the step
    /// [`Self::redo_in_page_box`] takes for Redo from the menu or toolbar: focus has moved to the
    /// menu by then, so unlike [`Self::page_box_can_redo`] this doesn't ask for it.
    pub(crate) fn open_box_can_redo(&self) -> bool {
        let Some(view) = self.active.and_then(|i| self.views.get(i)) else { return false };
        if let Some(ed) = &view.session.line_editor {
            return ed.history.can_redo();
        }
        view.session.content.draft.as_ref().is_some_and(|d| d.history.can_redo())
    }

    /// Typing in a box on the page (a paragraph being edited, added text) that the page doesn't
    /// have yet: Undo and Redo keys take it back in the box first.
    pub(crate) fn page_typing_unapplied(&self) -> bool {
        self.active.and_then(|i| self.views.get(i)).is_some_and(|v| {
            v.session.line_editor.as_ref().is_some_and(|ed| ed.has_unsaved_text())
                || v.session.content.draft.as_ref().is_some_and(|d| d.has_unapplied_typing())
        })
    }

    /// Apply any edit a view queued this frame (organize toolbar, keys).
    /// Returns false when a field refused the typing queued for it (its editor is open again).
    pub(crate) fn process_pending_edits(&mut self, ctx: &egui::Context) -> bool {
        let Some(i) = self.active else { return true };
        // Split view: the other pane's tab can hold an edit too (its field committed when the
        // click that brought this pane forward took the focus). It goes in first, as it came first.
        let (other_applied, stay) = self.apply_other_pane_edit(ctx, i);
        let modal = self.modal_open();
        let generation = |app: &Self| app.views.get(i).and_then(|v| app.session.get(v.id)).map(|d| d.edit_generation());
        let before = generation(self);
        let queued = self.views.get(i).is_some_and(|v| v.session.pending_edit.is_some());
        let applied = self.apply_queued_edit(ctx, i) && other_applied;
        // A click's Mouse Up waits on the edit it queued (a check box's toggle, a list's pick):
        // when that was refused, the click did nothing and its Mouse Up doesn't run either.
        if queued
            && generation(self) == before
            && let Some(view) = self.views.get_mut(i)
        {
            view.session.forms.triggers.retain(|(_, t)| *t != pdfcraft_engine::FieldTrigger::MouseUp);
        }
        // After the queued edit: a value typed into the field the focus left has been taken (and
        // its change has run the field's exit) before the focus scripts run.
        self.run_focus_scripts(ctx, i);
        self.run_field_triggers(ctx, i);
        if let Some((at, files)) = self.views.get_mut(i).and_then(|v| v.dropped_files.take()) {
            self.insert_dropped(ctx, files, at);
        }
        match self.views.get_mut(i).and_then(|v| v.pending_action.take()) {
            Some(crate::canvas::ViewAction::InsertFromFile) => self.insert_from_file_dialog(),
            Some(crate::canvas::ViewAction::InsertFromFileAt(at)) => self.insert_from_file_at(Some(at)),
            Some(crate::canvas::ViewAction::ReplacePages) => self.replace_pages_dialog(),
            Some(crate::canvas::ViewAction::Save) => {
                self.save_active(ctx, SaveTarget::InPlace);
            }
            Some(crate::canvas::ViewAction::Extract) => self.open_extract(ctx),
            Some(crate::canvas::ViewAction::Split) => self.open_split(ctx, false),
            // Split before the selected pages: the dialog opens over the selection scope.
            Some(crate::canvas::ViewAction::SplitBeforeSelected) => self.open_split(ctx, true),
            Some(crate::canvas::ViewAction::CopyPages { cut }) => self.copy_pages(ctx, cut),
            Some(crate::canvas::ViewAction::PastePages) => self.paste_pages(ctx),
            Some(crate::canvas::ViewAction::EnhanceCamera) => self.enhance_camera(ctx, i),
            Some(crate::canvas::ViewAction::CancelCamera) => self.leave_camera(i),
            Some(crate::canvas::ViewAction::CameraRotate { clockwise }) => {
                if let Some(done) = self.views.get(i).and_then(|v| v.session.camera_done) {
                    let degrees = if clockwise { 90 } else { -90 };
                    if self.apply_edit(ctx, Edit::RotatePages { pages: vec![done.page], degrees })
                        && let Some(d) = self.views.get_mut(i).and_then(|v| v.session.camera_done.as_mut())
                    {
                        d.turns += degrees;
                        d.steps += 1;
                    }
                }
            }
            Some(crate::canvas::ViewAction::CameraLevel) => self.camera_level(ctx, i),
            Some(crate::canvas::ViewAction::LockSignatures) => {
                if let Some(id) = self.views.get(i).map(|v| v.id) {
                    self.lock_signatures_now(ctx, id);
                }
            }
            Some(crate::canvas::ViewAction::CameraNext(command)) => {
                self.leave_camera(i);
                self.execute(ctx, command);
            }
            Some(crate::canvas::ViewAction::Replace { all }) => self.replace_found(ctx, i, all),
            None => {}
        }
        // That tab comes forward when its value was refused (its editor is open again with the
        // text) or its edit opened a dialog for its document, unless this tab's action opened one.
        if let Some(key) = stay
            && self.active == Some(i)
            && (modal || !self.modal_open())
        {
            self.active = self.tab_with_key(key).or(self.active);
        }
        applied
    }

    /// Split view: apply the edit queued on the tab the other pane shows, with that tab active
    /// while it goes in (tab `i`, the active one, is active again afterwards). Left queued, it was
    /// lost when the next frame replaced that tab to follow a change made through tab `i`
    /// (`settle_panes`). Returns false when a field refused it, and that tab's view key when it
    /// should come forward: its value was refused, or its edit opened a dialog.
    fn apply_other_pane_edit(&mut self, ctx: &egui::Context, i: usize) -> (bool, Option<u64>) {
        use crate::panes::Place;
        let Some(place) = self.views.get(i).map(|v| v.place()) else { return (true, None) };
        let other = if place == Place::Main { Place::Second } else { Place::Main };
        let Some((k, key, id)) = self
            .panes
            .split
            .and_then(|_| self.pane_front(other))
            .filter(|&k| k != i)
            .and_then(|k| self.views.get(k).filter(|v| v.session.pending_edit.is_some()).map(|v| (k, v.view_key(), v.id)))
        else {
            return (true, None);
        };
        let generation = |app: &Self| app.session.get(id).map(|d| d.edit_generation());
        let (before, modal) = (generation(self), self.modal_open());
        self.active = Some(k);
        let applied = self.apply_queued_edit(ctx, k);
        let opened = !modal && self.modal_open();
        self.active = Some(i);
        // Tab `i` on the same document catches up now, keeping what it holds (this frame's click
        // may have queued an edit or opened a field): the next frame replaces only the tab that
        // isn't active, as if this change had come through the active one.
        if generation(self) != before
            && let Some(doc) = self.session.get(id)
        {
            for (j, v) in self.views.iter_mut().enumerate() {
                if j != k && v.id == id {
                    v.document_changed(ctx, &doc.info);
                }
            }
        }
        (applied, (!applied || opened).then_some(key))
    }

    /// Enhance ▸ Camera image ▸ clean-up slider: undo the enhancement (and the turns made since)
    /// and enhance again from the photo at the new level, turning it as before. Only when the
    /// top of the undo history is still those steps; otherwise the slider goes back.
    fn camera_level(&mut self, ctx: &egui::Context, i: usize) {
        let Some(view) = self.views.get(i) else { return };
        let (id, Some(done)) = (view.id, view.session.camera_done) else { return };
        let ours = |s: &pdfcraft_engine::Session, k: usize| {
            let label = s.get(id).and_then(|d| d.can_undo().map(str::to_owned));
            let want = if k + 1 == done.steps { "Enhance camera image" } else { "Rotate page" };
            label.as_deref() == Some(want)
        };
        // Undo the run this tool applied, checking each step's label first. If the run was
        // broken (another edit came in between), redo what was undone and leave the page alone:
        // enhancing a page that is still straightened would warp it a second time.
        let restore = |app: &mut Self| {
            if let Some(d) = app.views.get_mut(i).and_then(|v| v.session.camera_done.as_mut()) {
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
            view.document_changed(ctx, &doc.info);
            if let Some(d) = view.session.camera_done.as_mut() {
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
            view.session.camera = None;
            view.session.camera_done = None;
        }
        if self.quick_tool == crate::QuickTool::EnhanceCamera {
            self.quick_tool = crate::QuickTool::Select;
        }
    }

    /// Enhance ▸ Camera image: straighten what the corners on tab `i` enclose. On success the
    /// bar offers rotate, crop and organize for the page; on failure the corners stay so they
    /// can be adjusted.
    fn enhance_camera(&mut self, ctx: &egui::Context, i: usize) {
        let Some(view) = self.views.get(i) else { return };
        let (id, Some(q)) = (view.id, view.session.camera) else { return };
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
                    view.document_changed(ctx, &doc.info);
                    view.session.camera = None;
                    view.session.camera_done =
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
    pub fn copy_pages(&mut self, ctx: &egui::Context, cut: bool) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
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
            self.apply_edit(ctx, Edit::DeletePages { pages: pages.clone() });
        }
        let what = if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) };
        if cut {
            self.notify_fmt("Cut {what}", &[("what", &what)]);
        } else {
            self.notify_fmt("Copied {what}", &[("what", &what)]);
        }
    }

    /// Organize ▸ Paste: insert the copied pages after the selection (or the current page).
    pub fn paste_pages(&mut self, ctx: &egui::Context) {
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
        if self.apply_edit(
            ctx,
            Edit::InsertPagesFrom { name: clip.name.clone(), bytes: clip.bytes.clone(), pages: Some(clip.pages.clone()), at, field_conflicts },
        ) {
            self.views[i].select_pages(&(at..at + count).collect::<Vec<_>>());
        }
    }

    /// Make the active document say what its form shows (#166): apply an edit still queued on
    /// its tab, then commit the text in a field's open editor, which otherwise commits only on
    /// Enter, Tab or clicking away (the editor closes, as clicking away would), then run the
    /// XFA enter and exit scripts of a focus move whose scripts haven't run yet (closing the
    /// editor is one; so is a move made in the same frame), so the document has what they
    /// change too. Run before anything reads the document: Save, Print, Export, Optimize, Sign,
    /// Extract, Split, Copy pages and button scripts.
    ///
    /// Returns false when the field rejects the value (a validation script or format; the user
    /// is told why). The editor stays open with the text, and the caller must stop: nothing is
    /// saved, printed or exported without it.
    pub(crate) fn commit_form_typing(&mut self, ctx: &egui::Context) -> bool {
        let Some((i, id)) = self.active_ids() else { return true };
        if !self.apply_queued_edit(ctx, i) {
            return false;
        }
        let Some(draft) = self.views.get(i).and_then(|v| v.session.forms.focus.clone()) else {
            self.run_focus_scripts(ctx, i);
            return true;
        };
        let Some(edit) = self.session.get(id).and_then(|d| crate::forms_ui::draft_edit(&draft, &d.form)) else {
            // Nothing typed that the document doesn't already have.
            self.run_focus_scripts(ctx, i);
            return true;
        };
        if let Some(view) = self.views.get_mut(i) {
            view.session.forms.focus = None;
        }
        if self.apply_edit(ctx, edit) {
            // The editor closed: the field's exit runs now, unless its change just ran it.
            self.run_focus_scripts(ctx, i);
            return true;
        }
        // The refused draft goes back as it is: its editor shows again while `forms.focus` holds it.
        if let Some(view) = self.views.get_mut(i) {
            view.session.forms.focus = Some(draft);
        }
        false
    }

    /// XFA enter and exit scripts for where the focus went on tab `i` since it was last looked at,
    /// however it moved (a click, Tab or Shift+Tab, Escape, a click on the page): the
    /// field it left runs its exit, then the field it reached runs its enter. The engine skips an
    /// exit the field's committed change already ran. A tab that closes runs nothing: its
    /// document is going away, and a script changing it then would only ask to save it. A script
    /// that changes the field being edited (an enter clearing a placeholder) shows in its editor
    /// ([`crate::forms_ui::refresh_draft`]), so leaving the field doesn't write the old value back.
    fn run_focus_scripts(&mut self, ctx: &egui::Context, i: usize) {
        let Some(view) = self.views.get_mut(i) else { return };
        // The field's object too: moving between fields sharing a name is a move (#344 review).
        let now = view.session.forms.focus.as_ref().map(|f| (f.name.clone(), f.obj, f.widget));
        if view.session.forms.focused == now {
            return;
        }
        let left = std::mem::replace(&mut view.session.forms.focused, now.clone());
        let id = view.id;
        let edited = view.session.forms.focus.clone();
        use pdfcraft_engine::js::FieldFocus;
        for ((name, obj, widget), focus) in [left.map(|l| (l, FieldFocus::Exit)), now.map(|n| (n, FieldFocus::Enter))].into_iter().flatten() {
            // The edited field's value before this script, to tell its untouched draft from typing:
            // the exit of the field left may have changed it already, and the draft with it.
            let before: Option<(String, Vec<String>)> = edited.as_ref().and_then(|e| {
                crate::forms_ui::focused_field(e, &self.session.get(id)?.form).map(|f| (crate::forms_ui::draft_text(f), f.value.clone()))
            });
            match self.session.field_focus_target(id, &name, obj.map(pdfcraft_engine::js::field_object_id), widget, focus) {
                Ok(changed) => {
                    if changed
                        && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                        && let Some(view) = self.views.get_mut(i)
                    {
                        view.document_changed(ctx, &info);
                        if let Some(before) = &before
                            && let Some(draft) = view.session.forms.focus.as_mut()
                            && let Some(f) = self.session.get(id).and_then(|d| crate::forms_ui::focused_field(draft, &d.form))
                        {
                            crate::forms_ui::refresh_draft(draft, (&before.0, &before.1), (crate::forms_ui::draft_text(f), &f.value));
                        }
                    }
                    let out = self.session.take_js_output(id);
                    self.handle_js(ctx, id, out);
                }
                Err(e) => self.notify_error(e),
            }
        }
    }

    /// The pointer triggers tab `i` queued (Mouse Enter, Exit, Down, and a check box's Mouse Up
    /// after its toggle), each running its widget's JavaScript in the document's realm (#269).
    fn run_field_triggers(&mut self, ctx: &egui::Context, i: usize) {
        let Some(view) = self.views.get_mut(i) else { return };
        let triggers = std::mem::take(&mut view.session.forms.triggers);
        let id = view.id;
        for ((name, object, widget), trigger) in triggers {
            // The field being edited as it was, so a script changing it shows in its editor
            // while a draft the user already typed into is kept (as `run_focus_scripts` does).
            let before: Option<(String, Vec<String>)> = self.views.get(i).and_then(|v| v.session.forms.focus.as_ref()).and_then(|e| {
                crate::forms_ui::focused_field(e, &self.session.get(id)?.form).map(|f| (crate::forms_ui::draft_text(f), f.value.clone()))
            });
            match self.session.field_trigger_target(id, &name, Some(object), widget, trigger) {
                Ok(changed) => {
                    if changed
                        && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                        && let Some(view) = self.views.get_mut(i)
                    {
                        view.document_changed(ctx, &info);
                        if let Some(before) = &before
                            && let Some(draft) = view.session.forms.focus.as_mut()
                            && let Some(f) = self.session.get(id).and_then(|d| crate::forms_ui::focused_field(draft, &d.form))
                        {
                            crate::forms_ui::refresh_draft(draft, (&before.0, &before.1), (crate::forms_ui::draft_text(f), &f.value));
                        }
                    }
                    let out = self.session.take_js_output(id);
                    if !out.is_empty() {
                        self.handle_js(ctx, id, out);
                    }
                }
                Err(e) => self.notify_error(e),
            }
        }
    }

    /// Apply the edit queued on tab `i` (the active one). Returns false only when it was a
    /// field's typing and the field refused it (#166): the editor reopens with the text, and the
    /// user has been told why, so the typing isn't lost and the caller can stop.
    fn apply_queued_edit(&mut self, ctx: &egui::Context, i: usize) -> bool {
        let Some(edit) = self.views.get_mut(i).and_then(|v| v.session.pending_edit.take()) else { return true };
        let signature_page = self.views.get_mut(i).and_then(|v| v.session.fill_signature.take());
        let doc_id = self.views.get(i).map(|v| v.id);
        let committed = self.views.get_mut(i).and_then(|v| v.session.forms.committed.take());
        let typed = match (&edit, &committed) {
            (Edit::SetFieldValue { name, .. }, Some(draft)) => *name == draft.name,
            _ => false,
        };
        if self.apply_edit(ctx, edit) {
            if signature_page.is_some()
                && let Some(id) = doc_id
            {
                self.placed_signatures.insert(id);
                // A new signature shows the Lock now notice again, even if the last was dismissed.
                if let Some(view) = self.views.get_mut(i) {
                    view.lock_offer_dismissed = false;
                }
            }
            // Tab can wrap to the same field (including another widget of that field).
            // Its new editor was opened before this edit ran, so it still holds the old
            // value. Refresh from the accepted form before Save can commit that stale draft.
            if typed
                && let Some(view) = self.views.get_mut(i)
                && let Some(focus) = view.session.forms.focus.as_mut()
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
                view.session.comments.select(newest.map(|index| (page, index)));
                view.session.comments.reveal = true;
                self.quick_tool = crate::QuickTool::Select;
            }
            return true;
        }
        if !typed {
            return true;
        }
        if let (Some(draft), Some(view)) = (committed, self.views.get_mut(i)) {
            view.session.forms.focus = Some(draft);
        }
        false
    }

    /// [`Self::commit_form_typing`] for document `id`, which may not be the active tab (a save
    /// panel answering later, Close All). If its tab has typing or a queued edit, the tab is
    /// brought forward first, so the commit and any field scripts act on that document.
    ///
    /// Each tab of the document holds typing of its own (split view). The tab in use stays active
    /// when the others have been committed.
    pub(crate) fn commit_typing_in(&mut self, ctx: &egui::Context, id: pdfcraft_engine::DocId) -> bool {
        let keep = self.active_ids().filter(|(_, active)| *active == id).and_then(|(i, _)| self.views.get(i)).map(|v| v.pane.key);
        let tabs: Vec<u64> = self.views.iter().filter(|v| v.id == id).map(|v| v.pane.key).collect();
        for key in tabs {
            if let Some(i) = self.tab_with_key(key)
                && !self.commit_typing_of_tab(ctx, i, id)
            {
                return false;
            }
        }
        if let Some(i) = keep.and_then(|k| self.tab_with_key(k)) {
            self.active = Some(i);
        }
        true
    }

    /// [`Self::commit_typing_in`] for one tab of the document.
    fn commit_typing_of_tab(&mut self, ctx: &egui::Context, i: usize, id: pdfcraft_engine::DocId) -> bool {
        // Added text the standard fonts can't draw can't be saved: show it rather than drop it.
        // Its page is brought into view, so the text, the warning and Discard are on screen.
        let blocked = self.views.get_mut(i).and_then(|v| {
            let c = v.session.content.blocked()?;
            v.session.content.hold_blocked();
            if let Some(page) = v.session.content.draft.as_ref().map(|d| d.page) {
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
            v.session.pending_edit.is_some()
                || v.session.forms.focus.is_some()
                || v.session.content.draft.is_some()
                || v.session.line_editor.as_ref().is_some_and(|ed| ed.has_changes())
                || v.session.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
        });
        if self.active != Some(i) {
            if !typing {
                return true;
            }
            self.active = Some(i);
        }
        self.commit_form_typing(ctx) && self.commit_open_fill_text(ctx, i, id) && self.commit_open_text_edits(ctx, i, id)
    }

    /// The Edit a PDF ▸ Text boxes (new text, and a paragraph open for editing) commit on
    /// click-away. Save does that first, so the typed text is in the file instead of being left
    /// open on the screen (#3496).
    fn commit_open_text_edits(&mut self, ctx: &egui::Context, i: usize, id: pdfcraft_engine::DocId) -> bool {
        let Some(doc) = self.session.get(id) else { return true };
        let Some(view) = self.views.get_mut(i) else { return true };
        // Both are taken out of the view before either edit runs, so keep what is needed to put
        // back whichever one the document refuses: typed text must not be lost.
        let (draft, select_added, editor) = (view.session.content.draft.clone(), view.session.content.select_added, view.session.line_editor.clone());
        let added = view.session.content.finish_draft(doc.added());
        let paragraph = view.session.line_editor.take_if(|ed| ed.has_changes()).and_then(crate::edit_text_ui::finished_edit);
        let added_ok = added.is_none_or(|edit| self.apply_edit(ctx, edit));
        let paragraph_ok = paragraph.is_none_or(|edit| self.apply_edit(ctx, edit));
        if let Some(view) = self.views.get_mut(i) {
            if !added_ok {
                (view.session.content.draft, view.session.content.select_added) = (draft, select_added);
            }
            if !paragraph_ok {
                view.session.line_editor = editor;
            }
        }
        added_ok && paragraph_ok
    }

    /// The Fill & Sign type box commits on click-away. Save does that first, so the text is in
    /// the file (and can be flattened) instead of being left on the screen.
    fn commit_open_fill_text(&mut self, ctx: &egui::Context, i: usize, id: pdfcraft_engine::DocId) -> bool {
        let Some(tb) = self.views.get(i).and_then(|v| v.session.fill_text.clone()) else { return true };
        let text = tb.text.trim().to_string();
        if text.is_empty() {
            if let Some(view) = self.views.get_mut(i) {
                view.session.fill_text = None;
            }
            return true;
        }
        let author = self.comment_prefs.author.clone();
        let rotation = self.session.get(id).map_or(0, |doc| crate::comments::page_rotation(&doc.info, tb.page));
        let look = crate::fill_sign::TypedLook { color: tb.color, border: tb.border };
        let edit = crate::fill_sign::typed_with(tb.page, tb.at, &text, tb.size, rotation, &author, look);
        if self.apply_edit(ctx, edit) {
            if let Some(view) = self.views.get_mut(i) {
                view.session.fill_text = None;
            }
            true
        } else {
            false
        }
    }

    /// When the preference is on, bake Fill & Sign marks before the bytes are written.
    /// Returns false when the document is not allowed to change (nothing is written).
    fn flatten_fill_sign_for_save(&mut self, ctx: &egui::Context, id: pdfcraft_engine::DocId) -> bool {
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
            && let Some(view) = self.asking_tab(id).and_then(|i| self.views.get_mut(i))
        {
            // The tab in use: `sync_views` brings any other tab of the document up to date.
            view.document_changed(ctx, &doc.info);
        }
        true
    }

    /// Whether tab `index` has work that isn't saved: edits, or form typing not committed yet.
    pub(crate) fn has_unsaved_work(&self, index: usize) -> bool {
        self.views.get(index).and_then(|v| self.session.get(v.id)).is_some_and(|doc| doc.dirty) || self.has_unsaved_typing(index)
    }

    /// Whether tab `index` holds work of its own that isn't in its document yet: form typing not
    /// committed, an edit waiting, text being typed. (The document's own edits are shared with
    /// any other tab of it, in split view.)
    pub(crate) fn has_unsaved_typing(&self, index: usize) -> bool {
        let Some(v) = self.views.get(index) else { return false };
        let Some(doc) = self.session.get(v.id) else { return false };
        v.session.pending_edit.is_some()
            || v.session.content.blocked().is_some()
            || v.session.forms.focus.as_ref().is_some_and(|f| crate::forms_ui::draft_edit(f, &doc.form).is_some())
            || v.session.fill_text.as_ref().is_some_and(|t| !t.text.trim().is_empty())
            || new_text_typed(v)
            || v.session.line_editor.as_ref().map_or_else(|| false, |ed| ed.has_unsaved_text())
    }
}

fn rect_matches(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn re_resolve_image_selection(
    view: &mut crate::DocView,
    page: usize,
    target: &pdfcraft_engine::PageImage,
    ambiguous_before: bool,
    page_images: &[pdfcraft_engine::PageImage],
) {
    let exact_matches: Vec<usize> = page_images
        .iter()
        .enumerate()
        .filter(|(_, img)| img.name == target.name && img.object == target.object && rect_matches(img.rect, target.rect))
        .map(|(idx, _)| idx)
        .collect();
    let new_idx = if !ambiguous_before && exact_matches.len() == 1 {
        exact_matches.first().copied()
    } else if !ambiguous_before && exact_matches.is_empty() {
        let name_obj_matches: Vec<usize> =
            page_images.iter().enumerate().filter(|(_, img)| img.name == target.name && img.object == target.object).map(|(idx, _)| idx).collect();
        if name_obj_matches.len() == 1 {
            name_obj_matches.first().copied()
        } else if name_obj_matches.is_empty() {
            let name_matches: Vec<usize> = page_images.iter().enumerate().filter(|(_, img)| img.name == target.name).map(|(idx, _)| idx).collect();
            if name_matches.len() == 1 { name_matches.first().copied() } else { None }
        } else {
            None
        }
    } else {
        None
    };
    if let Some(s) = view.session.image_selection.as_mut().filter(|s| s.page == page) {
        if let Some(idx) = new_idx {
            s.index = idx;
        } else {
            view.session.image_selection = None;
        }
    }
}

/// The single page a content edit changes (`None` for other edits). Like comment edits,
/// these leave every other page's bytes, geometry and text alone, so their rasters stay valid.
fn content_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::MoveObjects { page, .. }
        | Edit::DeleteObjects { page, .. }
        | Edit::EditTextLine { page, .. }
        | Edit::EditTextBlock { page, .. }
        | Edit::EditPageImage { page, .. }
        | Edit::AddText { page, .. }
        | Edit::AddImage { page, .. }
        | Edit::UpdateContent { page, .. }
        | Edit::DeleteContent { page, .. }
        | Edit::ReplaceImage { page, .. }
        | Edit::PasteObjects { page, .. } => Some(*page),
        _ => None,
    }
}

/// A signature shows its state on its own page, which an edit elsewhere can change (a
/// certification forbidding it, say): every page redraws. An edit that moves no page (a form edit,
/// or one to a page's comments or content) keeps the values typed, in it and before it, showing
/// until their pages redraw.
fn signatures_changed_after_edit(
    ctx: &egui::Context,
    view: &mut crate::DocView,
    edit: &Edit,
    info: &pdfcraft_render::DocInfo,
    annots_before: &[pdfcraft_render::Annotation],
    form_unbaked: Option<Vec<(String, usize)>>,
    one_page: Option<usize>,
) {
    let waiting = (form_unbaked.is_some() || one_page.is_some()).then(|| std::mem::take(&mut view.session.forms.unbaked));
    if let Edit::DeleteAnnotation { page, index } = edit {
        // Remap surviving comments before generic slot pruning loses their state.
        view.session.comments.annotation_deleted(annots_before, info, (*page, *index));
    }
    view.document_changed(ctx, info);
    if let Some(waiting) = waiting {
        view.session.forms.unbaked = waiting;
    }
    if let Some(unbaked) = form_unbaked {
        view.session.forms.unbaked.extend(unbaked);
    }
}

/// A comment or content edit changes one page, `page`: keep every other raster. Opening, closing
/// or moving a pop-up changes none: the window is drawn over the page, and a /Popup annotation
/// paints nothing in the raster (ISO 32000-2 §12.5.6.14).
fn one_page_changed(
    view: &mut crate::DocView,
    edit: &Edit,
    page: usize,
    annots_before: &[pdfcraft_render::Annotation],
    info: &pdfcraft_render::DocInfo,
) {
    if !matches!(edit, Edit::SetAnnotationPopup { .. }) {
        view.page_changed(page);
    }
    if let Edit::DeleteAnnotation { page, index } = edit {
        // Remap surviving comments before generic slot pruning loses their state.
        view.session.comments.annotation_deleted(annots_before, info, (*page, *index));
    } else {
        view.session.comments.document_changed(info);
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
        | Edit::SetAnnotationPopup { page, .. }
        | Edit::ResizeAnnotation { page, .. }
        | Edit::StyleAnnotation { page, .. }
        | Edit::FillAnnotation { page, .. }
        | Edit::PartOpacity { page, .. }
        | Edit::FontSizeAnnotation { page, .. }
        | Edit::BorderColorAnnotation { page, .. }
        | Edit::BorderShapeAnnotation { page, .. }
        | Edit::SetAnnotationInfo { page, .. } => Some(*page),
        _ => None,
    }
}

/// The pages of a batch that only adds annotations (markup on a text selection across pages),
/// each of which changes only its own page; `None` for anything else.
fn annotated_pages(edit: &Edit) -> Option<Vec<usize>> {
    let Edit::Batch { edits, .. } = edit else { return None };
    let pages: Vec<usize> = edits
        .iter()
        .map(|e| match e {
            Edit::AddAnnotation(a) => Some(a.page),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!pages.is_empty()).then_some(pages)
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
            | Edit::SetFieldImageTarget { .. }
            | Edit::SetFieldScript { .. }
            | Edit::SetCalculationOrder { .. }
            | Edit::ApplyScriptChanges { .. }
    )
}

impl PdfCraftApp {
    /// Carry out a Bookmarks-panel action as an undoable edit.
    pub fn bookmark_action(&mut self, ctx: &egui::Context, action: crate::panels::BmAction) {
        use crate::panels::BmAction as A;
        let Some((i, id)) = self.active_ids() else { return };
        let current = self.views[i].current();
        let parent_of = |p: &[usize]| p[..p.len() - 1].to_vec();
        let refresh_open = matches!(action, A::FromStructure | A::Indent(_));
        let indent_parent = match &action {
            A::Indent(path) if !path.is_empty() => {
                let mut to_parent = parent_of(path);
                to_parent.push(path[path.len() - 1].saturating_sub(1));
                Some(to_parent)
            }
            _ => None,
        };
        let edit = match action {
            A::New => {
                let n = self.session.get(id).map_or(0, |d| d.info.outline.len());
                // Where the reader is: the selection's corner, else the top of the window.
                let (page, [left, top]) =
                    self.session.get(id).and_then(|d| self.views.get(i)?.bookmark_position(&d.info)).unwrap_or((current, [None, None]));
                let (left, top) = (left.map(f64::from), top.map(f64::from));
                self.apply_edit(ctx, Edit::AddBookmark { parent: vec![], index: n, title: "Untitled".into(), page, left, top });
                // Like Acrobat: the new bookmark starts in rename mode.
                crate::panels::bookmarks::start_rename(ctx, id, &[n]);
                self.set_right(Some(crate::RightPanel::Bookmarks));
                return;
            }
            A::StartRename(path) => {
                crate::panels::bookmarks::start_rename(ctx, id, &path);
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
            A::Copy(path) => {
                self.copy_bookmark(id, &path);
                return;
            }
            A::Cut(path) => match self.copy_bookmark(id, &path) {
                Some(false) => Edit::DeleteBookmark { path },
                // Paste could not bring back what the copy left out, so the bookmarks stay.
                Some(true) => {
                    self.notify_tr("Bookmarks nested too deep to list could not be copied, so nothing was cut.");
                    return;
                }
                None => return,
            },
            A::Paste { parent, index } => {
                self.paste_bookmarks(ctx, id, parent, index);
                return;
            }
            A::FromStructure => {
                self.set_right(Some(crate::RightPanel::Bookmarks));
                Edit::BookmarksFromStructure
            }
            // A dialog chooses the styles; it makes the edit.
            A::FromTextStyles => {
                self.open_text_style_bookmarks(ctx, id);
                return;
            }
        };
        if self.apply_edit(ctx, edit) && refresh_open {
            if let Some(parent) = indent_parent {
                if let Some(view) = self.views.get_mut(i) {
                    view.bookmarks_open.set(&parent, true);
                }
            } else if let (Some(doc), Some(view)) = (self.session.get(id), self.views.get_mut(i)) {
                view.bookmarks_open = crate::panels::bookmarks::opened_as_filed(&doc.info.outline);
            }
        }
    }
}

impl PdfCraftApp {
    /// Put the bookmark at `path` and its subtree on the bookmark clipboard. `Some(true)` when
    /// bookmarks nested too deep to list were left out; `None` if it could not be read (the reason
    /// is shown).
    fn copy_bookmark(&mut self, id: pdfcraft_engine::DocId, path: &[usize]) -> Option<bool> {
        match self.session.outline_tree(id, path) {
            Some(Ok(tree)) => {
                if tree.truncated {
                    self.notify_tr("Bookmarks nested too deep to list were left out of the copy.");
                }
                self.bookmark_clipboard = Some(tree.nodes);
                Some(tree.truncated)
            }
            Some(Err(e)) => {
                self.notify_error(e);
                None
            }
            None => None,
        }
    }

    /// Paste the bookmark clipboard as one undoable edit. Bookmarks whose page this document lacks
    /// arrive without a destination, and the toast says how many.
    fn paste_bookmarks(&mut self, ctx: &egui::Context, id: pdfcraft_engine::DocId, parent: Vec<usize>, index: usize) {
        let Some(bookmarks) = self.bookmark_clipboard.clone() else { return };
        let pages = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let unmapped = pdfcraft_engine::outline_unmapped(&bookmarks, pages).len();
        if self.apply_edit(ctx, Edit::ImportBookmarks { parent, index, bookmarks, replace: false }) && unmapped > 0 {
            let n = u64::try_from(unmapped).unwrap_or(u64::MAX);
            self.notify(crate::i18n::trn(
                crate::i18n::current(),
                n,
                "{n} pasted bookmark points past the last page of this document and has no destination.",
                "{n} pasted bookmarks point past the last page of this document and have no destination.",
            ));
        }
    }
}

/// The open box's own Undo or Redo, as its keys do it: the box's text, selection and line
/// alignments one step back (or forward) in its typing history. False when the history has no
/// such step, and then nothing changes.
fn step_typing(
    history: &mut crate::edit_text_ui::TypingHistory,
    text: &mut String,
    selection: &mut Option<craft::text::ui::editor::TextSelection>,
    aligns: &mut Vec<Option<pdfcraft_engine::TextAlign>>,
    step: Step,
) -> bool {
    let mut now = crate::edit_text_ui::Typed::of(std::mem::take(text), selection.take(), std::mem::take(aligns));
    let stepped = history.step(&mut now, step);
    *text = now.text;
    // A restored byte range belonged to the old text; the editor will place a fresh caret.
    *selection = if stepped { None } else { now.selection };
    *aligns = now.aligns;
    stepped
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod revert_draft_tests {
    use super::*;
    use crate::forms_ui::Focus;
    use pdfcraft_engine::FieldValue;

    /// An open editor on the first widget of the field "name", holding `text` as typed.
    fn focus(text: &str) -> Focus {
        Focus {
            name: "name".into(),
            obj: None,
            widget: 0,
            text: text.into(),
            picked: Vec::new(),
            // PLAN: assumed forms-w7's new fields `selection: TextSelection` (with `Default`), `list_open`, `calendar_open` (pdf-forms-w7 sheet :14)
            selection: Default::default(),
            list_open: false,
            calendar_open: false,
        }
    }

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "source.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
        app
    }

    /// `app` in a window, so its dialogs are drawn and answered by their buttons.
    fn shown(app: PdfCraftApp) -> egui_kittest::Harness<'static, PdfCraftApp> {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            app
        });
        h.run_steps(2);
        h
    }

    /// Presses the button called `label` of the dialog on top (the last such node drawn: the
    /// toolbar may have one of the same name), then lets the app take the answer.
    fn press(h: &mut egui_kittest::Harness<'static, PdfCraftApp>, label: &str) {
        use egui_kittest::kittest::Queryable;
        h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label:?} button on screen")).click();
        h.run_steps(3);
    }

    /// The text of the notices posted since the last call.
    fn posted(app: &mut PdfCraftApp) -> String {
        crate::notices::texts(app, &egui::Context::default()).join("; ")
    }

    fn queue_draft(app: &mut PdfCraftApp, index: usize, text: &str) {
        app.views[index].session.forms.focus = Some(focus(text));
        let form = app.session.get(app.views[index].id).unwrap().form.clone();
        crate::forms_ui::commit(&mut app.views[index].session, &form);
        assert!(app.views[index].session.forms.committed.is_some() && app.views[index].session.pending_edit.is_some());
    }

    #[test]
    fn control_quit_prompts_for_uncommitted_typing_and_cancel_retains_it() {
        for queued in [false, true] {
            let mut app = app();
            let id = app.views[0].id;
            app.open_bytes(&egui::Context::default(), "other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            if queued {
                queue_draft(&mut app, 0, "Unsaved typing");
            } else {
                app.views[0].session.forms.focus = Some(focus("Unsaved typing"));
            }
            assert!(!app.session.get(id).unwrap().dirty);
            assert_eq!(app.active, Some(1));
            let mut h = shown(app);
            let ctx = h.ctx.clone();
            assert!(!h.state_mut().request_quit(&ctx), "the typing holds the quit");
            assert_eq!(h.state().close_guard.as_ref().and_then(|g| g.current()), Some(&id), "the question is about the typed document");
            h.run_steps(2);
            press(&mut h, "Cancel");
            let app = h.state();
            assert!(app.close_guard.is_none(), "Cancel ends the quit");
            assert_eq!(app.views.len(), 2);
            assert!(app.has_unsaved_work(0));
            assert_eq!(app.views[0].id, id);
            assert!(!app.session.get(id).unwrap().dirty);
            if queued {
                assert!(
                    matches!(&app.views[0].session.pending_edit, Some(Edit::SetFieldValue { value: FieldValue::Text(text), .. }) if text == "Unsaved typing")
                );
            } else {
                assert_eq!(app.views[0].session.forms.focus.as_ref().unwrap().text, "Unsaved typing");
            }
        }
    }

    /// A value the field's font can't draw is kept, and the user is told why it shows `?`, in the
    /// interface language; a drawable value says nothing.
    #[test]
    fn a_value_its_font_cannot_draw_is_kept_and_explained() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        let mut app = app();
        let fill = |app: &mut PdfCraftApp, text: &str| {
            app.apply_edit(&egui::Context::default(), Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text(text.into()) })
        };
        assert!(fill(&mut app, "张三"));
        let toast = posted(&mut app);
        assert!(toast.contains("“张” (U+5F20)") && toast.contains("other PDF viewers redraw the field"), "{toast}");
        assert_eq!(app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value, ["张三"]);
        // The shell sets the drawing language each frame; here, directly (per thread).
        let previous = crate::i18n::current();
        crate::i18n::set_current(crate::i18n::Lang::from_code("zh-hans").expect("registered"));
        assert!(fill(&mut app, "李四"));
        crate::i18n::set_current(previous);
        let toast = posted(&mut app);
        assert!(toast.contains("此字段的字体无法显示“李”（U+674E）"), "{toast}");
        assert!(fill(&mut app, "Ada"));
        let silent = posted(&mut app);
        assert!(silent.is_empty(), "{silent:?}");
    }

    /// Text typed into a new text box is lost on close unless the save prompt appears; an empty
    /// box has nothing to lose.
    #[test]
    fn text_typed_into_a_new_text_box_counts_as_unsaved_work() {
        use crate::content_ui::{TextDraft, default_style};
        let mut app = app();
        let draft = |text: &str| {
            let mut draft = TextDraft::new(0, None, [0.0; 4], text.into(), default_style());
            draft.focus = true;
            draft
        };
        assert!(!app.has_unsaved_work(0));
        app.views[0].session.content.draft = Some(draft("  \n"));
        assert!(!app.has_unsaved_work(0));
        app.views[0].session.content.draft = Some(draft("Typed"));
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
            let mut app = PdfCraftApp::for_unit_tests();
            app.open_bytes(&egui::Context::default(), "source.pdf", Some(path.clone()), bytes).unwrap();
            let rect = [72.0, 72.0, 272.0, 92.0];
            let mut draft = TextDraft::new(0, None, rect, "Typed".into(), default_style());
            draft.focus = true;
            app.views[0].session.content.draft = Some(draft);
            let mut h = shown(app);
            let ctx = h.ctx.clone();
            if quit {
                assert!(!h.state_mut().request_quit(&ctx));
            } else {
                let ctx_h = h.ctx.clone();
                h.state_mut().request_close_tab(&ctx_h, 0);
            }
            assert!(h.state().close_guard.is_some(), "the typed box holds the close");
            h.run_steps(2);
            // The question's "save it" answer: the app words it as a document app does on every platform.
            let save = {
                use craft::i18n::WordSet as _;
                let words = craft::ui::dialog::DialogWords::english();
                words.unsaved_save.as_str().to_string()
            };
            press(&mut h, &save);
            assert!(h.state().views.is_empty(), "the tab closes once saved");

            let mut reopened = PdfCraftApp::for_unit_tests();
            reopened.open_bytes(&egui::Context::default(), "source.pdf", Some(path.clone()), std::fs::read(&path).unwrap()).unwrap();
            let doc = reopened.session.get(reopened.views[0].id).unwrap();
            assert!(doc.added().iter().any(|a| matches!(&a.content, AddedContent::Text(t) if t.text == "Typed")), "quit: {quit}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Revert drops the focus without running the exit of the field that had it: the document
    /// is the saved one again, and nothing changes it after.
    #[test]
    fn revert_runs_no_exit_for_the_field_that_had_the_focus() {
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "focus.pdf", None, pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::focus_template()))
            .unwrap();
        let id = app.views[0].id;
        let count = |app: &PdfCraftApp, name: &str| {
            app.session.get(id).unwrap().form.iter().find(|f| f.name == name).and_then(|f| f.value.first().cloned()).unwrap_or_default()
        };
        app.views[0].session.forms.focus = Some(Focus { name: "probe".into(), ..focus("") });
        let ctx = egui::Context::default();
        assert!(app.process_pending_edits(&ctx));
        assert_eq!(count(&app, "enters"), "1", "reaching probe ran its enter");
        app.revert_active(&egui::Context::default());
        assert_eq!(count(&app, "enters"), "0");
        assert!(app.process_pending_edits(&ctx));
        assert_eq!(count(&app, "exits"), "0", "no exit ran on the reverted document");
        assert!(!app.session.get(id).unwrap().dirty);
    }

    /// A focus move whose scripts the frame hasn't run yet (Save pressed as the focus reaches a
    /// field) runs them before anything reads the document: `hint`'s enter clears its "Type here".
    #[test]
    fn committing_the_form_runs_the_focus_scripts_of_a_move_not_yet_run() {
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "focus.pdf", None, pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::focus_template()))
            .unwrap();
        let hint = |app: &PdfCraftApp| {
            app.session
                .get(app.views[0].id)
                .unwrap()
                .form
                .iter()
                .find(|f| f.name == "hint")
                .and_then(|f| f.value.first().cloned())
                .unwrap_or_default()
        };
        assert_eq!(hint(&app), "Type here");
        // As the editor opens on it: its text and value.
        app.views[0].session.forms.focus =
            Some(Focus { name: "hint".into(), text: "Type here".into(), picked: vec!["Type here".into()], ..focus("") });
        assert!(app.commit_form_typing(&egui::Context::default()));
        assert_eq!(hint(&app), "", "hint's enter ran before the document is read");
        assert_eq!(app.views[0].session.forms.focus.as_ref().map(|f| f.text.as_str()), Some(""), "and its editor shows it");
    }
    /// A pointer trigger whose script changes the field being edited shows the change in its
    /// editor, unless the user already typed there: then the typed draft is kept.
    #[test]
    fn a_pointer_trigger_changing_the_edited_field_refreshes_an_untouched_draft() {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (old) /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hot) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 /AA << /E << /S /JavaScript /JS (getField\\('name'\\).value = 'new';) >> >> >>".into(),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        for (typed, shown) in [(false, "new"), (true, "typed")] {
            let mut app = PdfCraftApp::for_unit_tests();
            app.open_bytes(&egui::Context::default(), "hot.pdf", None, pdf.clone()).unwrap();
            let hot = app.session.get(app.views[0].id).unwrap().form.iter().find(|f| f.name == "hot").unwrap().obj;
            let text = if typed { "typed" } else { "old" };
            app.views[0].session.forms.focus = Some(Focus { text: text.into(), picked: vec!["old".into()], ..focus("") });
            let hot = ("hot".to_string(), pdfcraft_engine::js::field_object_id(hot), 0);
            app.views[0].session.forms.triggers.push((hot, pdfcraft_engine::FieldTrigger::MouseEnter));
            app.run_field_triggers(&egui::Context::default(), 0);
            assert_eq!(app.views[0].session.forms.focus.as_ref().map(|f| f.text.as_str()), Some(shown), "typed: {typed}");
        }
    }

    #[test]
    fn successful_revert_clears_focused_or_queued_drafts_only_on_its_document() {
        for queued in [false, true] {
            let mut app = app();
            let source = app.views[0].id;
            assert!(app.apply_edit(
                &egui::Context::default(),
                Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Committed change".into()) }
            ));
            if queued {
                queue_draft(&mut app, 0, "Queued source draft");
            } else {
                app.views[0].session.forms.focus = Some(focus("Focused source draft"));
            }
            app.open_bytes(&egui::Context::default(), "other.pdf", None, include_bytes!("../tests/data/form.pdf").to_vec()).unwrap();
            let other = app.views[1].id;
            assert!(app.apply_edit(
                &egui::Context::default(),
                Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Other committed value".into()) }
            ));
            queue_draft(&mut app, 1, "Other queued draft");
            app.views[1].session.forms.focus = Some(focus("Other focused draft"));
            let other_bytes = app.session.get(other).unwrap().bytes.clone();
            let other_generation = app.session.get(other).unwrap().edit_generation();
            let source_generation = app.session.get(source).unwrap().edit_generation();
            app.active = Some(0);
            app.revert_active(&egui::Context::default());
            let reverted = app.session.get(source).unwrap();
            assert!(!reverted.dirty);
            assert_eq!(reverted.edit_generation(), source_generation + 1);
            assert!(reverted.form.iter().find(|field| field.name == "name").unwrap().value.is_empty());
            assert!(
                app.views[0].session.forms.focus.is_none()
                    && app.views[0].session.forms.committed.is_none()
                    && app.views[0].session.pending_edit.is_none()
            );
            assert!(!app.has_unsaved_work(0));
            assert_eq!(app.views[1].session.forms.focus.as_ref().unwrap().text, "Other focused draft");
            assert_eq!(app.views[1].session.forms.committed.as_ref().unwrap().text, "Other queued draft");
            assert!(
                matches!(&app.views[1].session.pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Other queued draft")
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
        app.views[0].session.forms.focus = Some(focus("Focused draft"));
        // Exercise the ordinary NoDocument error boundary without a corrupt fixture.
        // This verifies UI failure preservation, not other engine refresh failures.
        app.session.close(source);
        app.revert_active(&egui::Context::default());
        assert_eq!(app.views[0].session.forms.focus.as_ref().unwrap().text, "Focused draft");
        assert_eq!(app.views[0].session.forms.committed.as_ref().unwrap().text, "Queued draft");
        assert!(
            matches!(&app.views[0].session.pending_edit, Some(Edit::SetFieldValue { name, value: FieldValue::Text(text) }) if name == "name" && text == "Queued draft")
        );
        assert_eq!(app.active_ids(), Some((0, source)));
        assert_eq!(posted(&mut app), pdfcraft_engine::EditError::NoDocument.to_string());
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
