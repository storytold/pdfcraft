use craft::host::queue::Priority;
// PLAN: needs the `platform-clipboard` feature on the `craft` dependency in crates/ui-egui/Cargo.toml (owner: pdf-hub-w1); `craft::platform::clipboard` exists only with it
use craft::platform::clipboard::{ClipboardImage, Flavor, HTML, RGBA8, TEXT};

use super::{CommentClip, DocId, DocView, DocumentCopyJob, PdfCraftApp, QuickTool, comments};

/// The browser's clipboard: a page can't read it outside a paste event, so pastes arrive as events
/// ([`PdfCraftApp::offer_clip_content`]) and copies go out through egui's platform output, which
/// eframe's web runner hands to the page. Only text goes out; the browser takes no other flavour from
/// a canvas app.
#[cfg(target_arch = "wasm32")]
pub(super) fn web_clipboard(ctx: &egui::Context) -> craft::platform::clipboard::Clipboard {
    use craft::platform::clipboard::{Clipboard, ClipboardError, EventClipboard};
    let (pass, out, wake) = (ctx.clone(), ctx.clone(), ctx.clone());
    let events = EventClipboard::new(
        move || pass.cumulative_pass_nr(),
        move |flavors: &[Flavor]| {
            let (index, text) = flavors.iter().enumerate().find(|(_, f)| f.mime == TEXT).ok_or(ClipboardError::NoFormat)?;
            out.copy_text(String::from_utf8_lossy(&text.data).into_owned());
            Ok(index)
        },
    )
    .with_wake(move || wake.request_repaint());
    Clipboard::new(events)
}

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

pub(crate) fn print_refused() -> &'static str {
    tl!("The document's security settings don't allow printing")
}

/// What objects copied in Edit a PDF put on the system clipboard for other applications, so ⌘V
/// can tell whether it still holds that copy or something copied since: their text, or else the
/// first picture's pixels (a hash of them), or nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum ObjectClipMark {
    Text(String),
    Picture(u64),
    #[default]
    None,
}

fn picture_hash(width: usize, height: usize, rgba: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (width, height, rgba).hash(&mut h);
    h.finish()
}

/// The pixels of `image` for egui; `None` when its buffer isn't `width × height × 4` bytes.
fn color_image(image: &ClipboardImage) -> Option<egui::ColorImage> {
    let size = [usize::try_from(image.width).ok()?, usize::try_from(image.height).ok()?];
    let bytes = size[0].checked_mul(size[1])?.checked_mul(4)?;
    (bytes == image.rgba.len()).then(|| egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba))
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

    /// Offer the content delivered with the fired paste in the frame that runs it.
    pub(crate) fn offer_clip_content(&mut self, content: &craft::ui::runtime::commands::ClipContent) {
        let mut flavors = Vec::new();
        if let Some(text) = &content.text {
            flavors.push(Flavor::text(text.as_ref()));
        }
        if let Some(image) = &content.image
            && let (Ok(width), Ok(height)) = (u32::try_from(image.size[0]), u32::try_from(image.size[1]))
        {
            let mut data = Vec::new();
            data.extend_from_slice(&width.to_le_bytes());
            data.extend_from_slice(&height.to_le_bytes());
            data.extend(image.pixels.iter().flat_map(egui::Color32::to_srgba_unmultiplied));
            flavors.push(Flavor::new(RGBA8, data));
        }
        if !flavors.is_empty() {
            self.clipboard.offer_paste(flavors);
        }
    }

    /// Writes `text` to the clipboard; says why when it can't. Silent when automation's input is
    /// fenced off from the user's clipboard.
    pub(crate) fn put_text(&mut self, text: &str) -> bool {
        match self.clipboard.write_text(text) {
            Ok(_) => true,
            Err(e) => {
                if let Some(t) = e.text() {
                    self.notify(crate::i18n::platform_text(t));
                }
                false
            }
        }
    }

    /// A newer copy wins over a whole-document copy still reading its pages: drops the waiting
    /// job and cancels the running one.
    pub(crate) fn cancel_text_copies(&self) {
        self.text_copies.reprioritize(|_, _| None);
    }

    /// Takes the whole-document copies whose pages are read and puts their text on the clipboard.
    /// A text field's or an image's copy this frame, and the document closing, drop it instead
    /// (called at the end of the frame, so it sees this frame's copies).
    pub(crate) fn drain_text_copies(&mut self, ctx: &egui::Context) {
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        let open = |views: &[crate::DocView], doc: &DocId| views.iter().any(|v| v.id == *doc);
        while let Some(done) = self.text_copies.try_recv() {
            if !copied && open(&self.views, &done.key) && !done.result.is_empty() {
                self.put_text(&done.result);
            }
        }
        let views = &self.views;
        self.text_copies.reprioritize(|doc, priority| (!copied && open(views, doc)).then_some(priority));
    }

    /// Copy on the canvas: with objects selected in Edit a PDF, those; with no text selected, the
    /// selected comments with their replies; else the text; else the selected image.
    pub(crate) fn copy_from_canvas(&mut self, ctx: &egui::Context, i: usize) -> bool {
        if self.views.get(i).is_some_and(|v| v.session.forms.focus.is_some() || v.organize) {
            return false;
        }
        // Objects selected together in Edit a PDF take the copy.
        if self.quick_tool == QuickTool::EditText && self.views.get(i).is_some_and(|v| v.session.objects.count() > 0) {
            return self.copy_objects(ctx, i, false);
        }
        // A selection across pages still being read has no text yet, but is selected all the
        // same: its copy starts rather than copying the selected comments.
        let text_selected =
            self.views.get(i).is_some_and(|v| v.selection_waits_for_text().is_some() || v.selected_text().is_some_and(|t| !t.is_empty()));
        if !text_selected && self.copy_selected_comments(i) {
            return true;
        }
        if self.copy_selected_text(i) || self.copy_picked_image(i) {
            return true;
        }
        let Some(sel) = self.views.get(i).and_then(|v| v.session.image_selection.clone()) else { return false };
        let Some(id) = self.views.get(i).map(|v| v.id) else { return false };
        self.may_copy(id) && self.copy_page_image(sel.page, sel.index)
    }

    /// Cut on the canvas: objects selected in Edit a PDF, else the selected image.
    pub(crate) fn cut_from_canvas(&mut self, ctx: &egui::Context, i: usize) -> bool {
        if self.views.get(i).is_some_and(|v| v.session.forms.focus.is_some() || v.organize) {
            return false;
        }
        if self.quick_tool == QuickTool::EditText && self.views.get(i).is_some_and(|v| v.session.objects.count() > 0) {
            return self.copy_objects(ctx, i, true);
        }
        if let Some(sel) = self.views.get(i).and_then(|v| v.session.image_selection.clone())
            && let Some(id) = self.views.get(i).map(|v| v.id)
            && self.may_copy(id)
        {
            return self.cut_page_image(ctx, sel.page, sel.index);
        }
        false
    }

    /// Paste on the canvas, in this order: the objects copied in Edit a PDF while the clipboard
    /// still holds their copy; a picture the clipboard holds; its text (XFDF is pasted as
    /// comments, and text beats an image the app only remembers); else the remembered image.
    pub(crate) fn paste_from_clipboard(&mut self, ctx: &egui::Context, i: usize) {
        self.paste_from_clipboard_with(ctx, i, None);
    }

    /// [`Self::paste_from_clipboard`], preferring content delivered with the paste event (`pasted`, F-89)
    /// over a fresh system-clipboard read so desktop `SystemClipboard` hosts still receive event payloads.
    pub(crate) fn paste_from_clipboard_with(&mut self, ctx: &egui::Context, i: usize, pasted: Option<&craft::ui::runtime::commands::ClipContent>) {
        self.paste_from_clipboard_at(ctx, i, pasted, None);
    }

    pub(crate) fn paste_from_clipboard_at(
        &mut self,
        ctx: &egui::Context,
        i: usize,
        pasted: Option<&craft::ui::runtime::commands::ClipContent>,
        at: Option<egui::Pos2>,
    ) {
        if self.views.get(i).is_some_and(|v| v.session.forms.focus.is_some() || v.organize) {
            return;
        }
        let target = at.and_then(|at| {
            let view = self.views.get(i)?;
            let doc = self.session.get(view.id)?;
            view.session.user_point_at(view.geometry(), &doc.info, at)
        });
        let page = target.map_or_else(|| self.views.get(i).map_or(0, |v| v.current()), |(page, _)| page);
        let delivered_text = pasted.and_then(|c| c.text.as_deref()).filter(|t| !t.is_empty()).map(str::to_owned);
        let delivered_image = pasted.and_then(|c| c.image.as_ref()).and_then(|img| {
            let (width, height) = (u32::try_from(img.size[0]).ok()?, u32::try_from(img.size[1]).ok()?);
            let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
            rgba.extend(img.pixels.iter().flat_map(egui::Color32::to_srgba_unmultiplied));
            Some(ClipboardImage { width, height, rgba })
        });
        let text = delivered_text
            .or_else(|| if self.clipboard.has(&[TEXT]) { self.clipboard.read_text().ok().flatten().filter(|t| !t.is_empty()) } else { None });
        let image = delivered_image.or_else(|| self.clipboard.read_image().ok().flatten());
        if self.quick_tool == QuickTool::EditText && self.object_clipboard.is_some() && self.pastes_objects(text.as_deref(), image.as_ref()) {
            self.paste_objects(ctx, i);
            return;
        }
        if let Some(image) = image {
            match color_image(&image) {
                // PLAN: assumed `paste_received_image(page, &egui::ColorImage) -> bool` (pdf-edit-w7)
                Some(pixels) => {
                    self.paste_received_image_at(ctx, page, &pixels, target.map(|(_, at)| at));
                }
                None => self.notify_error("the clipboard image is empty, malformed or exceeds size limits"),
            }
            return;
        }
        // An explicit empty paste can fall back to the app's copied comments, after object and
        // image precedence has been resolved using the actual clipboard contents.
        let text = pasted.and_then(|c| c.text.as_deref()).filter(|t| t.is_empty()).map(str::to_owned).or(text);
        if let Some(text) = text {
            if !text.is_empty() {
                self.last_copied_image = None;
                self.last_copied_file = None;
            }
            if self.paste_comment_text(ctx, i, text) {
                return;
            }
        }
        self.paste_image(ctx, page, None);
    }

    /// Copy in Edit a PDF with objects selected together: copy them for Paste in any open
    /// document, and put their text (or else their first picture) on the system clipboard for
    /// other applications. A cut then deletes them, as one undo step.
    pub(crate) fn copy_objects(&mut self, ctx: &egui::Context, i: usize, cut: bool) -> bool {
        let Some((id, Some((page, targets)))) = self.views.get(i).map(|v| (v.id, v.session.objects.selection())) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let Some(doc) = self.session.get(id) else { return false };
        if cut && !doc.allows_modification() {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        let clip = match doc.copy_objects(page, &targets) {
            Ok(clip) => clip,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        self.cancel_text_copies();
        let text = clip.text();
        let picture =
            clip.objects().iter().find(|o| o.target.kind == pdfcraft_engine::ObjectKind::Image && o.content_type == "image").map(|o| o.target.index);
        self.object_clip_mark = if !text.is_empty() {
            self.put_text(&text);
            ObjectClipMark::Text(text)
        } else if let Some(index) = picture
            && self.copy_page_image(page, index)
            && let Some((w, h, rgba)) = &self.last_copied_image
        {
            ObjectClipMark::Picture(picture_hash(*w as usize, *h as usize, rgba))
        } else {
            ObjectClipMark::None
        };
        self.object_clipboard = Some(pdfcraft_engine::ObjectClipboard::new(clip));
        if cut && let Some(delete) = pdfcraft_engine::Edit::delete_objects(page, targets) {
            self.apply_edit(ctx, delete);
        }
        true
    }

    /// Whether Paste's `text` and `image` (what the clipboard holds, if anything) are the objects
    /// copied in Edit a PDF, rather than something copied since, here or in another application.
    fn pastes_objects(&self, text: Option<&str>, image: Option<&ClipboardImage>) -> bool {
        let text_is_ours = text.is_none_or(|text| match &self.object_clip_mark {
            ObjectClipMark::Text(copied) => text.replace("\r\n", "\n").trim_end() == copied.trim_end(),
            ObjectClipMark::None => !pdfcraft_engine::is_xfdf_comments(text),
            ObjectClipMark::Picture(_) => false,
        });
        let image_is_ours = image.is_none_or(|image| match &self.object_clip_mark {
            ObjectClipMark::Picture(hash) => picture_hash(image.width as usize, image.height as usize, &image.rgba) == *hash,
            ObjectClipMark::None => true,
            ObjectClipMark::Text(_) => false,
        });
        text_is_ours && image_is_ours
    }

    /// Paste in Edit a PDF: the copied objects onto the current page of tab `i`, as page content
    /// that draws the same (one undo step), and selected.
    pub(crate) fn paste_objects(&mut self, ctx: &egui::Context, i: usize) -> bool {
        let Some((id, page)) = self.views.get(i).map(|v| (v.id, v.current())) else { return false };
        let (Some(doc), Some(clipboard)) = (self.session.get(id), self.object_clipboard.as_mut()) else { return false };
        if !doc.allows_modification() {
            self.notify_error(tl!("The document's security settings don't allow this change"));
            return false;
        }
        let before = doc.editable_objects(page).unwrap_or_default();
        let edit = match clipboard.paste(doc, page) {
            Ok(edit) => edit,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        if !self.apply_edit(ctx, edit) {
            return false;
        }
        if let (Some(Ok(after)), Some(view)) = (self.session.get(id).map(|d| d.editable_objects(page)), self.views.get_mut(i)) {
            view.session.objects.select_pasted(page, &before, after);
        }
        true
    }

    /// A text pasted onto the canvas: XFDF with comments is pasted as comments. Copied comments
    /// come from the system clipboard when it holds them, from the app's own only when it holds
    /// nothing legible (a word copied in another app since is that word, not the comments copied
    /// before it).
    fn paste_comment_text(&mut self, ctx: &egui::Context, i: usize, text: String) -> bool {
        if pdfcraft_engine::is_xfdf_comments(&text) {
            self.paste_comments(ctx, i, Some(text), None);
            true
        } else if text.trim().is_empty() {
            if self.comment_clipboard.is_some() {
                self.paste_comments(ctx, i, None, None);
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Copy the selected comments of tab `i` as XFDF, each with its replies: into the app's
    /// clipboard (for Paste in any tab or window) and onto the system clipboard as text. False
    /// when nothing is selected.
    pub fn copy_selected_comments(&mut self, i: usize) -> bool {
        let Some(view) = self.views.get(i) else { return false };
        let keys = view.session.comments.selection();
        if keys.is_empty() {
            return false;
        }
        let id = view.id;
        match self.session.export_comments(id, &keys) {
            Ok(xfdf) => {
                self.cancel_text_copies();
                self.put_text(&xfdf);
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
    /// bundle until a click places it (`view.comments.paste`), so a copy never lands on its
    /// original and a bundle from a large page goes where it fits on a small one. The copies are
    /// selected once placed (see `apply_edit`).
    pub fn paste_comments(&mut self, ctx: &egui::Context, i: usize, text: Option<String>, at: Option<(usize, [f64; 2])>) -> bool {
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
                self.apply_edit(ctx, pdfcraft_engine::Edit::PasteComments { xfdf, page, offset: Some(offset) })
            }
            None => {
                let Some(view) = self.views.get_mut(i) else { return false };
                view.session.comments.clear_selection();
                let outlines = pdfcraft_engine::comment_outlines(&xfdf);
                // PLAN: assumed `comments::PastePlacement { xfdf, bounds, count, outlines }` (pdf-comments-w8)
                view.session.comments.paste = Some(comments::PastePlacement { xfdf, bounds, count, outlines });
                self.notify_tr("Click where the copied comments go; Escape lets them go");
                true
            }
        }
    }

    /// Shared permission gate for copying selected page text from menus and the Copy command.
    /// After Select All of the document (#2639), pages whose text wasn't read for display are read
    /// off the UI thread (a `text_copies` job), and the clipboard gets the text when they're done.
    pub(crate) fn copy_selected_text(&mut self, i: usize) -> bool {
        self.cancel_text_copies();
        let Some(v) = self.views.get(i) else { return false };
        let id = v.id;
        // So are a selection's pages not read yet (a selection across pages copied before every
        // page it covers has its text), rather than copying nothing.
        let waiting = v.selection_waits_for_text();
        if (v.session.all_text && !v.pages_without_text().is_empty()) || waiting.is_some() {
            // Checked before any page is read: a document that refuses copying reads nothing.
            if !self.may_copy(id) {
                return false;
            }
            let job = |v: &crate::DocView, doc: &pdfcraft_engine::Document| match waiting {
                Some(span) if !v.session.all_text => DocumentCopyJob::of_selection(v, doc, span),
                _ => DocumentCopyJob::of(v, doc),
            };
            let Some(job) = self.views.get(i).and_then(|v| Some(job(v, self.session.get(id)?))) else { return false };
            self.text_copies.submit(id, Priority::BACKGROUND, move |cancel| job.text(cancel).unwrap_or_default());
            return true;
        }
        let Some(text) = v.selected_text().filter(|text| !text.is_empty()) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        let html = self
            .views
            .get(i)
            .and_then(DocView::selected_glyphs)
            .and_then(|(page, layer, glyphs)| self.session.get(id)?.rich_copy(page, &layer, &glyphs).ok())
            // A capped copy would paste less than the plain text holds; paste that instead.
            .filter(|copy| !copy.truncated)
            .map(|copy| copy.html)
            .filter(|html| !html.is_empty());
        // The HTML and its plain alternative go out as one set; where that fails the plain text
        // is written alone.
        let rich = html.is_some_and(|html| self.clipboard.write(&[Flavor::new(HTML, html.into_bytes()), Flavor::text(&text)]).is_ok());
        if !rich {
            self.put_text(&text);
        }
        true
    }
}

#[cfg(test)]
mod review_tests {
    use std::sync::{Arc, Mutex};

    use craft::host::queue::{JobQueue, QueueOptions};
    use craft::host::{ManualJobs, Wake};
    use craft::platform::clipboard::{Clipboard, EventClipboard};

    use super::*;

    type Writes = Arc<Mutex<Vec<Vec<Flavor>>>>;

    /// An app on a blank document whose clipboard is fed by paste events and records its writes.
    fn app_with_clipboard() -> (PdfCraftApp, Writes) {
        let writes: Writes = Arc::default();
        let record = writes.clone();
        let mut app = PdfCraftApp::for_unit_tests();
        app.clipboard = Clipboard::new(EventClipboard::new(
            || 0,
            move |flavors| {
                record.lock().unwrap().push(flavors.to_vec());
                Ok(0)
            },
        ));
        app.create_blank();
        (app, writes)
    }

    fn green() -> egui::Event {
        egui::Event::PasteImage(Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GREEN)))
    }

    /// One frame's paste events offered, then Paste run as its command; the images on page 1.
    fn paste(app: &mut PdfCraftApp, events: Vec<egui::Event>) -> usize {
        let mut content = craft::ui::runtime::commands::ClipContent::default();
        for event in events {
            match event {
                egui::Event::Paste(text) => content.text = Some(text.into()),
                egui::Event::PasteImage(image) => content.image = Some(image),
                _ => {}
            }
        }
        app.offer_clip_content(&content);
        assert!(app.execute(&egui::Context::default(), "edit.paste"));
        app.session.get(app.views[0].id).unwrap().page_images(0).len()
    }

    #[test]
    fn delivered_image_is_authoritative_and_not_duplicated_by_keys() {
        let (mut app, _) = app_with_clipboard();
        assert_eq!(paste(&mut app, vec![egui::Event::Paste("newer OS contents".into()), green()]), 1);
        assert_eq!(app.last_copied_image.as_ref().unwrap().2, vec![0, 255, 0, 255]);
    }

    /// The clipboard holds one paste at a time: an image offered with text pastes, with the
    /// pasted pixels kept as the app's cache, whichever event came first.
    #[test]
    fn image_then_text_batch_pastes_image_and_leaves_text_authoritative() {
        let (mut app, _) = app_with_clipboard();
        assert_eq!(paste(&mut app, vec![green(), egui::Event::Paste("text".into())]), 1);
        assert!(app.last_copied_file.is_none());
    }

    #[test]
    fn text_then_image_batch_pastes_image_and_keeps_its_cache() {
        let (mut app, _) = app_with_clipboard();
        assert_eq!(paste(&mut app, vec![egui::Event::Paste("text".into()), green()]), 1);
        assert_eq!(app.last_copied_image.unwrap().2, vec![0, 255, 0, 255]);
    }

    #[test]
    fn separate_delivered_images_in_one_batch_each_paste_once() {
        let (mut app, _) = app_with_clipboard();
        assert_eq!(paste(&mut app, vec![green()]), 1);
        assert_eq!(paste(&mut app, vec![green()]), 2);
    }

    #[test]
    fn review_failed_keyboard_cut_keeps_the_image_selection() {
        let (mut app, _) = app_with_clipboard();
        app.views[0].session.image_selection = Some(crate::edit_text_ui::ImageSelection::at(0, 0));
        app.execute(&egui::Context::default(), "edit.cut");
        assert!(app.views[0].session.image_selection.is_some());
    }

    /// Review of #5017: Copy on a text selection still waiting for its pages' text starts its
    /// copy, even with a comment selected too, rather than copying the comment.
    #[test]
    fn a_selection_waiting_for_its_text_takes_copy_over_a_selected_comment() {
        let ctx = &egui::Context::default();
        let (mut app, writes) = app_with_clipboard();
        let jobs = Arc::new(ManualJobs::new());
        app.text_copies = JobQueue::new(jobs.clone(), Wake::none(), QueueOptions::default());
        let shape = pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] };
        let style = pdfcraft_engine::Style::default_for(&shape);
        let note = pdfcraft_engine::NewAnnotation { page: 0, shape, style, contents: String::new(), author: String::new() };
        assert!(app.apply_edit(ctx, pdfcraft_engine::Edit::AddAnnotation(note)));
        app.views[0].session.comments.select_only((0, 0));
        // The page's text is not read yet: the selection waits for it.
        app.views[0].session.texts.clear();
        app.views[0].select_text(0, 0, 0);
        assert!(app.views[0].selection_waits_for_text().is_some());
        assert!(app.execute(&egui::Context::default(), "edit.copy"));
        assert_eq!(jobs.waiting(), 1, "the selection's copy started");
        assert!(app.comment_clipboard.is_none(), "the comment was not copied");
        assert!(writes.lock().unwrap().is_empty(), "no comment payload reached the clipboard");
    }
}
