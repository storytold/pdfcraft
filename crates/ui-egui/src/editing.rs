//! Editing glue: apply engine edits from the UI, undo/redo, save/save-as, and the
//! "save changes?" prompt when closing a tab or quitting with unsaved edits.

use pdfcraft_engine::Edit;

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
        match self.session.apply(id, edit.clone()) {
            Ok(()) => {
                let Some(doc) = self.session.get(id) else { return true };
                let info = &doc.info;
                let view = &mut self.views[i];
                match comment_page(&edit) {
                    // Comment edits change one page: keep every other raster.
                    Some(page) => view.page_changed(page),
                    None => view.document_changed(info),
                }
                // Keep the pages the user acted on selected, where they now are.
                match edit {
                    Edit::MovePages { pages, to } => {
                        let n = pages.iter().collect::<std::collections::BTreeSet<_>>().len();
                        let to = to.min(info.pages.len() - n);
                        view.select_pages(&(to..to + n).collect::<Vec<_>>());
                    }
                    Edit::InsertBlankPage { at, .. } => view.select_pages(&[at.min(info.pages.len() - 1)]),
                    Edit::DeletePages { .. } => view.select_pages(&[]),
                    Edit::AddAnnotation(a) => {
                        // Select the new comment (appended last among the page's comments).
                        let newest = info.annotations.iter().filter(|x| x.page == a.page && x.in_reply_to.is_none()).map(|x| x.index).max();
                        view.comments.selected = newest.map(|n| (a.page, n));
                        view.comments.reveal = true;
                    }
                    Edit::DeleteAnnotation { .. } => view.comments.selected = None,
                    _ => {}
                }
                if std::mem::take(&mut view.comments.tool_done) && !self.comment_prefs.pinned {
                    self.quick_tool = crate::QuickTool::Select;
                }
                let out = self.session.take_js_output(id);
                self.handle_js(id, out);
                true
            }
            Err(e) => {
                self.notify_fmt("{label} failed: {e}", &[("label", &crate::i18n::action_label(&label)), ("e", &e.to_string())]);
                false
            }
        }
    }

    pub fn undo(&mut self) {
        self.history_step(true);
    }

    pub fn redo(&mut self) {
        self.history_step(false);
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
                let label = crate::i18n::action_label(&label);
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
    pub(crate) fn process_pending_edits(&mut self) {
        let Some(i) = self.active else { return };
        if let Some(edit) = self.views.get_mut(i).and_then(|v| v.pending_edit.take()) {
            self.apply_edit(edit);
        }
        match self.views.get_mut(i).and_then(|v| v.pending_action.take()) {
            Some(crate::canvas::ViewAction::InsertFromFile) => self.insert_from_file_dialog(),
            Some(crate::canvas::ViewAction::Extract) => self.dialog = Some(crate::Dialog::Extract),
            Some(crate::canvas::ViewAction::Split) => self.dialog = Some(crate::Dialog::Split),
            Some(crate::canvas::ViewAction::CopyPages { cut }) => self.copy_pages(cut),
            Some(crate::canvas::ViewAction::PastePages) => self.paste_pages(),
            None => {}
        }
    }

    /// Organize ▸ Copy / Cut: remember the selected pages (the document as it is now); Cut also
    /// deletes them (one page always stays).
    pub fn copy_pages(&mut self, cut: bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let pages = self.views[i].target_pages();
        let n = doc.info.pages.len();
        if cut && pages.len() >= n {
            self.notify_tr("A document needs at least one page: copy instead");
            return;
        }
        self.page_clipboard = Some(crate::PageClip { name: doc.name.clone(), bytes: doc.bytes.clone(), pages: pages.clone() });
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
        if self.apply_edit(Edit::InsertPagesFrom { name: clip.name.clone(), bytes: clip.bytes.clone(), pages: Some(clip.pages.clone()), at }) {
            self.views[i].select_pages(&(at..at + count).collect::<Vec<_>>());
        }
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
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let (name, path) = (doc.name.clone(), doc.path.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let destination = match (target, path, &self.save_override) {
                (_, _, Some(p)) => p.clone(),
                (SaveTarget::InPlace, Some(p), _) => p,
                _ => {
                    let name = if name.to_ascii_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
                    // The bytes are taken once the user has chosen, so edits made meanwhile are saved too.
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, dest| {
                        if app.save_doc_to(id, &dest.to_string_lossy()) {
                            after(app);
                        }
                    });
                    return false;
                }
            };
            let saved = self.save_doc_to(id, &destination);
            if saved {
                after(self);
            }
            saved
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (target, path);
            let bytes = match self.session.save_bytes(id) {
                Ok(b) => b,
                Err(e) => {
                    self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return false;
                }
            };
            match download(&name, &bytes) {
                Ok(()) => {
                    let _ = self.session.mark_saved(id, bytes, None);
                    if let Some(doc) = self.session.get(id) {
                        self.views[index].document_changed(&doc.info);
                    }
                    self.notify_fmt("Downloaded {name}", &[("name", &name)]);
                    after(self);
                    true
                }
                Err(e) => {
                    self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    false
                }
            }
        }
    }

    /// Write document `id` to `dest` and make it the document's file. Returns `true` if written.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_doc_to(&mut self, id: pdfcraft_engine::DocId, dest: &str) -> bool {
        let Some(name) = self.session.get(id).map(|d| d.name.clone()) else {
            self.notify_tr("The document was closed before it could be saved.");
            return false;
        };
        let bytes = match self.session.save_bytes(id) {
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
        match self.session.mark_saved(id, bytes, Some(dest.to_string())) {
            Ok(()) => {
                self.forget_recovery(id);
                if let Some(doc) = self.session.get(id)
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
                {
                    view.document_changed(&doc.info);
                }
                self.notify_fmt("Saved {name}", &[("name", &short_name(dest))]);
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
        let dirty = self.views.get(index).and_then(|v| self.session.get(v.id)).is_some_and(|d| d.dirty);
        if dirty && let Some(id) = self.views.get(index).map(|v| v.id) {
            self.close_request = Some(CloseRequest::Tab(id));
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    pub fn close_all(&mut self) {
        for i in (0..self.views.len()).rev() {
            if !self.session.get(self.views[i].id).is_some_and(|d| d.dirty) {
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
                    self.views[i].document_changed(&d.info);
                }
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        self.views.iter().position(|v| self.session.get(v.id).is_some_and(|d| d.dirty))
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(req) = self.close_request.take() else { return };
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
        match choice {
            None => {} // cancelled: nothing closes
            Some(false) => self.close_and_continue(ctx, index, req),
            Some(true) => {
                let Some(id) = self.views.get(index).map(|v| v.id) else { return };
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

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Intercept window close while documents have unsaved changes.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.allow_quit && self.first_dirty().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_request = Some(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }
}

/// The page a comment edit changes (`None` for other edits).
fn comment_page(edit: &Edit) -> Option<usize> {
    match edit {
        Edit::AddAnnotation(a) => Some(a.page),
        Edit::DeleteAnnotation { page, .. }
        | Edit::SetAnnotationContents { page, .. }
        | Edit::ReplyToAnnotation { page, .. }
        | Edit::SetAnnotationStatus { page, .. }
        | Edit::MoveAnnotation { page, .. }
        | Edit::ResizeAnnotation { page, .. }
        | Edit::StyleAnnotation { page, .. }
        | Edit::SetAnnotationInfo { page, .. } => Some(*page),
        _ => None,
    }
}

/// Write via a temporary file in the same directory and rename over the target, so a crash or
/// full disk never leaves a half-written PDF where the original was.
pub fn write_atomically(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::path::Path::new(path);
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let tmp = dir.join(format!(".{}.pdfcraft-{}.tmp", target.file_name().and_then(|n| n.to_str()).unwrap_or("save"), std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
                self.apply_edit(Edit::AddBookmark { parent: vec![], index: n, title: "Untitled".into(), page: current });
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
        };
        self.apply_edit(edit);
    }
}

fn bookmark_at<'a>(items: &'a [pdfcraft_render::OutlineItem], path: &[usize]) -> Option<&'a pdfcraft_render::OutlineItem> {
    let (first, rest) = path.split_first()?;
    let item = items.get(*first)?;
    if rest.is_empty() { Some(item) } else { bookmark_at(&item.children, rest) }
}
