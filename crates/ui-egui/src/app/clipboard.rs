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

    /// ⌘C copies the selected text, unless a text field has the keyboard (it copies its own).
    pub(super) fn copy_key(&mut self, i: usize, ctx: &egui::Context) {
        // ⌘C arrives as a Copy event on most platforms.
        let copy = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if !copy || ctx.egui_wants_keyboard_input() {
            return;
        }
        self.copy_selected_text(i, ctx);
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
