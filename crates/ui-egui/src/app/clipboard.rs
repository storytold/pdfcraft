use super::*;

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

pub(crate) fn print_refused() -> &'static str {
    tl!("The document's security settings don't allow printing")
}

impl PdfCraftApp {
    /// Whether text and images may be copied out of document `id`; says why not when they may
    /// not. Reading aloud and accessibility tools don't ask (assistive technology may read
    /// anything, §7.6.4.2).
    pub(crate) fn may_copy(&mut self, id: DocId) -> bool {
        if self.session.get(id).is_none_or(pdfcraft_engine::Document::allows_copying) {
            return true;
        }
        self.notify(copy_refused().to_string());
        false
    }

    /// ⌘C copies the selected text, or the selected comments when no text is selected; ⌘V
    /// pastes copied comments onto the current page. Not while a text field has the keyboard
    /// (it copies and pastes its own text).
    pub(super) fn copy_key(&mut self, i: usize, ctx: &egui::Context) {
        // ⌘C and ⌘V arrive as Copy and Paste events on most platforms.
        let (copy, pasted) = ctx.input(|input| {
            let copy = input.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let pasted = input.events.iter().find_map(|e| if let egui::Event::Paste(text) = e { Some(text.clone()) } else { None });
            (copy, pasted)
        });
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if copy {
            let text_selected = self.views.get(i).and_then(|v| v.selected_text()).is_some_and(|t| !t.is_empty());
            if text_selected || !self.copy_selected_comments(i) {
                self.copy_selected_text(i, ctx);
            }
        }
        if let Some(text) = pasted {
            // The organize grid takes ⌘V for pages; a text that is no XFDF with comments is not
            // ours either: copied comments come from the system clipboard when it holds them,
            // from the app's own only when it holds nothing legible (a word copied in another
            // app since is that word, not the comments copied before it).
            if self.views.get(i).is_some_and(|v| !v.organize) {
                if pdfcraft_engine::is_xfdf_comments(&text) {
                    self.paste_comments(i, Some(text), None);
                } else if text.trim().is_empty() && self.comment_clipboard.is_some() {
                    self.paste_comments(i, None, None);
                }
            }
        }
    }

    /// Copy the selected comments of tab `i` as XFDF, each with its replies: into the app's
    /// clipboard (for ⌘V in any tab or window) and onto the system clipboard as text. False
    /// when nothing is selected.
    pub fn copy_selected_comments(&mut self, i: usize) -> bool {
        let Some(view) = self.views.get(i) else { return false };
        let keys = view.comments.selection();
        if keys.is_empty() {
            return false;
        }
        let id = view.id;
        match self.session.export_comments(id, &keys) {
            Ok(xfdf) => {
                if let Some(ctx) = &self.ctx {
                    ctx.copy_text(xfdf.clone());
                }
                self.comment_clipboard = Some(CommentClip { xfdf, count: keys.len() });
                true
            }
            Err(e) => {
                self.notify_error(e);
                false
            }
        }
    }

    /// Paste copied comments (`text`, XFDF from the system clipboard, or else the app's
    /// clipboard) into tab `i`. With `at` (a page and a user-space point, from the page's menu)
    /// the bundle is placed at once, centred there; otherwise it hangs on the pointer as one
    /// bundle until a click places it (`Gesture::Place`), so a copy never lands on its original
    /// and a bundle from a large page goes where it fits on a small one. The copies are selected
    /// once placed (see `apply_edit`).
    pub fn paste_comments(&mut self, i: usize, text: Option<String>, at: Option<(usize, [f64; 2])>) -> bool {
        let Some(xfdf) = text.or_else(|| self.comment_clipboard.as_ref().map(|c| c.xfdf.clone())) else {
            self.notify_tr("Copy comments first");
            return false;
        };
        let Some(view) = self.views.get(i) else { return false };
        let id = view.id;
        let Some(doc) = self.session.get(id) else { return false };
        if !doc.allows_annotation() {
            self.notify_tr("The document's security settings don't allow this change");
            return false;
        }
        let count = pdfcraft_engine::comment_count(&xfdf);
        let Some(bounds) = pdfcraft_engine::comments_bounds(&xfdf).filter(|_| count > 0) else {
            self.notify_tr("The clipboard holds no comments");
            return false;
        };
        match at {
            Some((page, [x, y])) => {
                let offset = [x - (bounds[0] + bounds[2]) / 2.0, y - (bounds[1] + bounds[3]) / 2.0];
                self.apply_edit(pdfcraft_engine::Edit::PasteComments { xfdf, page, offset: Some(offset) })
            }
            None => {
                let Some(view) = self.views.get_mut(i) else { return false };
                view.comments.clear_selection();
                let outlines = pdfcraft_engine::comment_outlines(&xfdf);
                view.comments.gesture = Some(comments::Gesture::Place { xfdf, bounds, count, outlines });
                self.notify_tr("Click where the copied comments go; Escape lets them go");
                true
            }
        }
    }

    /// Shared permission gate for copying selected page text from menus and keyboard events.
    /// After Select All of the document (#2639), pages whose text wasn't read for display are read
    /// off the UI thread ([`DocumentCopy`]), and the clipboard gets the text when they're done.
    pub(crate) fn copy_selected_text(&mut self, i: usize, ctx: &egui::Context) -> bool {
        // A newer copy wins: a whole-document copy still reading must not replace it later.
        self.document_copy = None;
        let Some(v) = self.views.get(i) else { return false };
        let id = v.id;
        if v.all_text && !v.pages_without_text().is_empty() {
            // Checked before any page is read: a document that refuses copying reads nothing.
            if !self.may_copy(id) {
                return false;
            }
            let Some(job) = self.views.get(i).and_then(|v| Some(DocumentCopyJob::of(v, self.session.get(id)?))) else { return false };
            self.document_copy = Some(DocumentCopy::start(id, job, ctx));
            return true;
        }
        let Some(text) = v.selected_text().filter(|text| !text.is_empty()) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        ctx.copy_text(text);
        true
    }

    /// The clipboard text of a whole-document copy, once its pages are read; dropped with the
    /// document it was copied from, and by any later copy (a text field's, a comment's), which
    /// it must not replace. Run at the end of the frame, so it sees this frame's copies.
    pub(crate) fn finish_document_copy(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.document_copy else { return };
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        if copied || !self.views.iter().any(|v| v.id == c.doc) {
            self.document_copy = None;
            return;
        }
        match c.rx.try_recv() {
            Ok(text) => {
                self.document_copy = None;
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.document_copy = None,
        }
    }
}
