use std::sync::Arc;

use craft::ui::primitives::textures::{CachedTexture, Revision, TextureState, TextureStore, TextureStoreOptions};
use craft::ui::primitives::{ContentPainterCx, Draw};

use super::layout::editor_fill;
use super::worker::{DraftWorker, Request};
use super::{Edit, LineEditor, PageXform, Pos2, Rect, TextTarget};

/// The store's salt: the one raster of the open paragraph's page lives under it.
pub(super) const BACKGROUND_SALT: &str = "pdf-text-editor-background";

/// What the store keys the raster by: the document generation, the page and the paragraph or added
/// text it was rendered for.
pub(super) type BackgroundKey = (u64, usize, TextTarget);

/// The store the raster lives in: the page-sized budget, since the raster stands in for the page.
pub(super) fn background_store(ctx: &egui::Context) -> TextureStore<BackgroundKey> {
    TextureStore::in_context(ctx, BACKGROUND_SALT, TextureStoreOptions::pages())
}

/// A cached, read-only PDF raster. Paragraph drafts include their real PDF glyphs;
/// retyping an added item still uses a textless background.
#[derive(Default)]
pub(crate) struct TextBackground {
    key: Option<BackgroundKey>,
    pub(super) renderer: Option<pdfcraft_render::RenderPool>,
    /// The scale (in thousandths) the raster in flight was asked for; the raster's revision.
    pub(super) tag: Option<u64>,
    received: Option<u64>,
    /// The raster as the store holds it, kept so `ready` and `paint` need no context. Its id stays
    /// valid while the key is in the store; every `prepare` marks the entry seen so it is not evicted.
    /// A new draft is put under the same key, so the last faithful raster stays up until it lands.
    pub(super) shown: Option<CachedTexture>,
    // Wait for cancelled workers to exit before starting another draft. A slow image decoder
    // must not accumulate one render thread per keystroke.
    retiring: Vec<pdfcraft_render::Retiring>,
    pub(super) worker: Option<DraftWorker>,
    source: Option<Arc<pdfcraft_engine::TextDraftSource>>,
    serial: Option<u64>,
    pub(super) submitted_at: Option<web_time::Instant>,
    scale: f32,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    render_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    /// The draft the texture shows: set only when one is uploaded, so a draft whose preview
    /// failed or timed out never passes for the one on screen.
    texture_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_layout: Option<pdfcraft_engine::TextEditLayout>,
    draft_error: Option<String>,
    /// How the open paragraph looked in its editor last frame.
    look: Option<EditorLook>,
    /// An applied paragraph's last look. The page raster still has the old text until the page
    /// is rendered again, so the draft stays up until then: its raster, or, when it has none, the
    /// editor's own drawing of it.
    settling: Option<EditorLook>,
}

/// The page placement of the last draft, retained until the applied page raster arrives.
struct EditorLook {
    page: usize,
    block: usize,
    page_width: f32,
    rot: u16,
    /// The editor's own drawing of the draft, for a draft without a raster.
    drawing: Option<DraftDrawing>,
}

/// The open paragraph's draft as its editor lays it out, to draw where no raster shows that draft.
pub(super) struct DraftDrawing {
    text: String,
    style: pdfcraft_engine::BlockStyle,
    galley: Arc<egui::Galley>,
    color: egui::Color32,
    /// The text's top left, and the corners of the box that hides the page's own text, on the page.
    at: (f32, f32),
    cover: [(f32, f32); 2],
}

impl DraftDrawing {
    /// `ed`'s draft drawn by `galley` at `origin`, over the box `cover` and what the text typed in this
    /// pass adds to it.
    pub(super) fn new(xf: &PageXform, ed: &LineEditor, galley: Arc<egui::Galley>, origin: Pos2, color: egui::Color32, cover: Rect) -> Self {
        let cover = cover.union(galley.rect.translate(origin.to_vec2()));
        Self {
            text: ed.text.clone(),
            style: ed.style(),
            galley,
            color,
            at: xf.screen_to_norm(origin),
            cover: [xf.screen_to_norm(cover.min), xf.screen_to_norm(cover.max)],
        }
    }
}

impl TextBackground {
    fn raster_pending(&self) -> bool {
        self.renderer.is_some() && self.received != self.tag
    }

    pub fn pending(&self) -> bool {
        self.raster_pending() || self.serial.is_some() || self.draft_key != self.render_key || self.retiring.iter().any(|pool| !pool.exited())
    }

    /// The open paragraph's editor was drawn at this page placement this frame.
    pub(super) fn remember(&mut self, xf: &PageXform, page: usize, block: usize, drawing: Option<DraftDrawing>) {
        self.look = Some(EditorLook { page, block, page_width: xf.rect.width(), rot: xf.rot, drawing });
    }

    /// An edit was applied. When it is the open paragraph's, its last look stays up until the page
    /// is rendered again; any other edit ends it.
    pub fn committed(&mut self, edit: &Edit) {
        let look = self.look.take().and_then(|mut look| {
            let Edit::EditTextBlock { page, block, text, style } = edit else { return None };
            if (look.page, look.block) != (*page, *block)
                || !self.key.is_some_and(|(_, page, target)| (page, target) == (look.page, TextTarget::Block(look.block)))
            {
                return None;
            }
            // Only a raster of the applied text and style stands in for it. Applied before its preview
            // came, or after it failed, the draft has none: the editor's own drawing of it does.
            if self.shown.is_some() && self.texture_key.as_ref().is_some_and(|(shown, shown_style)| shown == text && shown_style == style) {
                look.drawing = None;
                return Some(look);
            }
            look.drawing.as_ref().is_some_and(|drawn| drawn.text == *text && drawn.style == *style).then_some(look)
        });
        match look {
            Some(look) => {
                self.retire_renderer();
                if let Some(worker) = &mut self.worker {
                    worker.release();
                }
                // The released worker publishes nothing more: no draft is in flight or asked for.
                self.serial = None;
                self.submitted_at = None;
                self.draft_key = self.render_key.clone();
                self.settling = Some(look);
            }
            None => self.reset(None),
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.settling.as_ref().is_some_and(|look| look.page == page) {
            self.reset(None);
        }
    }

    fn retire_renderer(&mut self) {
        if let Some(pool) = self.renderer.take() {
            self.retiring.push(pool.retire());
        }
    }

    fn reset(&mut self, key: Option<BackgroundKey>) {
        self.retire_renderer();
        let retiring = std::mem::take(&mut self.retiring);
        let mut worker = self.worker.take();
        if let Some(worker) = &mut worker {
            worker.release();
        }
        *self = Self { key, retiring, worker, ..Default::default() };
    }

    pub fn prepare(
        &mut self,
        ctx: &egui::Context,
        doc: &pdfcraft_engine::Document,
        editor: Option<&LineEditor>,
        draft: Option<&crate::content_ui::TextDraft>,
        scale: f32,
    ) {
        let store = background_store(ctx);
        let key = editor
            .map(|ed| (doc.edit_generation(), ed.page, TextTarget::Block(ed.block)))
            .or_else(|| draft.and_then(|d| d.index.map(|index| (doc.edit_generation(), d.page, TextTarget::Added(index)))));
        if key.is_none() && self.settling.is_some() {
            return;
        }
        self.retiring.retain(|pool| !pool.exited());
        if self.key != key {
            if let Some(old) = &self.key {
                store.forget(ctx, old);
            }
            self.reset(key);
        }
        if !self.retiring.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if let Some(ed) = editor {
            let Some(key) = self.key else { return };
            let Some(page) = doc.info.pages.get(ed.page) else { return };
            // One raster stands in for the page's tiles while a paragraph is open, so everything
            // around the editor is drawn from it: at the tiling threshold it stays sharp on a Retina
            // display up to about 2.5x on a Letter or A4 page, where the backdrop side (2048) went
            // soft past 1.2x.
            let max_side = ctx.input(|i| i.max_texture_side) as f32;
            let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / page.width.max(page.height).max(1.0));
            let tag = (scale * 1000.0) as u64;
            if self.tag != Some(tag) {
                self.draft_key = None;
            }
            // Marks the entry seen this frame, so the store keeps it; a raster the store dropped is
            // prepared again.
            if let Some(shown_tag) = self.received.filter(|_| self.shown.is_some())
                && matches!(store.state(ctx, &key, Revision::new(shown_tag)), TextureState::Missing)
            {
                self.shown = None;
                self.received = None;
                self.texture_key = None;
                self.draft_key = None;
            }
            self.scale = scale;
            self.tag = Some(tag);
            self.layout_for(ctx, doc, ed, &ed.text);
            if let Some(reply) = self.worker.as_mut().and_then(DraftWorker::poll)
                && self.serial == Some(reply.serial)
            {
                self.serial = None;
                self.submitted_at = None;
                self.render_key = self.draft_key.clone();
                match reply.result {
                    Ok(out) => {
                        self.draft_layout = Some(out.layout);
                        let image = crate::canvas::texture_image([out.page.width as usize, out.page.height as usize], out.page.rgba);
                        // In place under the key: the editor's last draft raster is replaced, not stacked.
                        self.shown = Some(store.put(ctx, key, Revision::new(tag), image));
                        self.texture_key = self.render_key.clone();
                        self.draft_error = None;
                    }
                    Err(error) => {
                        self.draft_layout = None;
                        self.draft_error = Some(error);
                    }
                }
                self.received = self.tag;
                ctx.request_repaint();
            }
            // Keep the former render watchdog's bounded wait without spawning replacement
            // workers. A slow decoder may finish later; its cancelled result is never shown.
            if self.serial.is_some() && self.submitted_at.is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(20)) {
                if let Some(worker) = &mut self.worker {
                    worker.cancel();
                }
                self.serial = None;
                self.submitted_at = None;
                self.render_key = self.draft_key.clone();
                self.draft_layout = None;
                self.draft_error = Some("The text preview took too long. Your typing is preserved; try a shorter draft or apply it.".into());
            }
            return;
        } else if let Some((_, page, TextTarget::Added(index))) = key
            && self.renderer.is_none()
            && self.tag.is_none()
            && self.retiring.is_empty()
        {
            self.renderer = doc.added_text_preview(page, index).ok().map(|p| p.renderer().without_replacements());
        }
        let (Some(key), Some(renderer)) = (self.key, self.renderer.as_ref()) else { return };
        let page = key.1;
        let Some(p) = doc.info.pages.get(page) else { return };
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            renderer.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            self.tag = Some(tag);
        }
        if let Some(out) = renderer.try_recv() {
            if let Some(error) = out.error {
                self.draft_error = Some(error);
                self.retire_renderer();
                self.received = Some(tag);
                ctx.request_repaint();
                return;
            }
            if out.request.tag == tag {
                let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                self.shown = Some(store.put(ctx, key, Revision::new(tag), image));
                self.received = Some(tag);
            }
        }
        // Marks the entry seen this frame, so the store keeps it; one the store dropped is asked for again.
        if let Some(shown_tag) = self.received.filter(|_| self.shown.is_some())
            && matches!(store.state(ctx, &key, Revision::new(shown_tag)), TextureState::Missing)
        {
            self.shown = None;
            self.received = None;
            self.tag = None;
        }
        if self.received != Some(tag) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Submit without parsing, rewriting or rendering on the frame thread. Exact geometry
    /// is available only with the raster for the same text and style.
    pub fn layout_for(
        &mut self,
        ctx: &egui::Context,
        doc: &pdfcraft_engine::Document,
        ed: &LineEditor,
        text: &str,
    ) -> Option<&pdfcraft_engine::TextEditLayout> {
        let key = Some((doc.edit_generation(), ed.page, TextTarget::Block(ed.block)));
        if self.key != key {
            self.reset(key);
        }
        let draft_key = (text.to_owned(), ed.style());
        if self.draft_key.as_ref() != Some(&draft_key) {
            self.draft_key = Some(draft_key.clone());
            let result = (|| {
                if self.source.is_none() {
                    self.source = Some(Arc::new(doc.text_draft_source(ed.page, ed.block)?));
                }
                if self.worker.is_none() {
                    self.worker = Some(DraftWorker::new(ctx)?);
                }
                let source = self.source.as_ref().ok_or("the draft source is unavailable")?.clone();
                let worker = self.worker.as_mut().ok_or("the draft worker is unavailable")?;
                self.submitted_at = Some(web_time::Instant::now());
                ctx.request_repaint_after(std::time::Duration::from_secs(20));
                self.serial = Some(worker.submit(Request { source, text: text.to_owned(), style: draft_key.1.clone(), scale: self.scale.max(0.01) }));
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                self.draft_error = Some(error);
                self.render_key = Some(draft_key);
                self.serial = None;
            }
        }
        if self.serial.is_none() && self.draft_key == self.render_key { self.draft_layout.as_ref() } else { None }
    }

    #[cfg(test)]
    pub(super) fn settling(&self) -> bool {
        self.settling.is_some()
    }

    /// The draft whose editor drawing stands in for the applied paragraph.
    #[cfg(test)]
    pub(super) fn settling_drawing(&self) -> Option<&str> {
        self.settling.as_ref().and_then(|look| look.drawing.as_ref()).map(|drawn| drawn.text.as_str())
    }

    #[cfg(test)]
    pub(super) fn source(&self) -> Option<&Arc<pdfcraft_engine::TextDraftSource>> {
        self.source.as_ref()
    }

    pub fn current(&self, ed: &LineEditor) -> bool {
        self.serial.is_none()
            && self.draft_error.is_none()
            && self.render_key.as_ref().is_some_and(|(text, style)| text == &ed.text && style == &ed.style())
    }

    pub fn error_for(&self, page: usize, block: usize) -> Option<&str> {
        self.draft_error.as_deref().filter(|_| self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))))
    }

    /// The draft's measured extent, when it was measured for paragraph `block` on `page`.
    pub fn draft_rect_for(&self, page: usize, block: usize) -> Option<[f64; 4]> {
        self.draft_layout
            .as_ref()
            .and_then(|layout| layout.draft_rect)
            .filter(|_| self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))))
    }

    pub fn ready(&self, page: usize, block: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))) && self.shown.is_some()
    }

    /// The draft raster goes under the open paragraph's editor: the paragraph has changed and a raster
    /// is ready. Not once the draft's preview failed: the raster is of an earlier draft, whose rows can
    /// reach past the box the editor draws the draft on, and the page shows its own text instead.
    pub(super) fn shows_draft(&self, ed: &LineEditor) -> bool {
        ed.has_changes() && self.draft_error.is_none() && self.ready(ed.page, ed.block)
    }

    pub fn ready_added(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Added(index))) && self.shown.is_some()
    }

    /// Paints the PDF draft over `page` where a paragraph is being changed (the textless page for
    /// added text), and an applied paragraph's draft, its raster or the editor's own drawing of it, until
    /// the page raster catches up. An open
    /// paragraph that has not changed paints nothing: the page draws its own text.
    pub fn paint(
        &self,
        draw: &Draw<'_>,
        content: &ContentPainterCx<'_>,
        xf: &PageXform,
        page: usize,
        editor: Option<&LineEditor>,
        draft: Option<&crate::content_ui::TextDraft>,
    ) {
        let for_editor = editor.is_some_and(|ed| ed.page == page && self.shows_draft(ed));
        // A zoom or a turn of the view since the paragraph was applied would misplace its rows:
        // the page raster (with the old text) shows until the new one arrives instead.
        let settled =
            self.settling.as_ref().filter(|look| look.page == page && look.rot == xf.rot && (look.page_width - xf.rect.width()).abs() < 0.5);
        if let Some(drawn) = settled.and_then(|look| look.drawing.as_ref()) {
            // As the editor drew it: its box over the page's own text, then the text, on its page only.
            let [a, b] = drawn.cover.map(|(u, v)| xf.norm_to_screen(u, v));
            let painter = content.painter.with_clip_rect(content.painter.clip_rect().intersect(xf.rect));
            painter.rect_filled(Rect::from_two_pos(a, b), egui::CornerRadius::ZERO, editor_fill(drawn.color));
            painter.galley(xf.norm_to_screen(drawn.at.0, drawn.at.1), Arc::clone(&drawn.galley), drawn.color);
            return;
        }
        let ready =
            for_editor || draft.is_some_and(|d| d.page == page && d.index.is_some_and(|index| self.ready_added(page, index))) || settled.is_some();
        if !ready {
            return;
        }
        let Some(tex) = self.shown else { return };
        // Paint the complete PDF draft so overhanging glyphs cannot leave stale source ink
        // outside the estimated paragraph rectangle.
        if xf.rot == 0 {
            draw.texture(xf.rect, tex.id, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        } else {
            // `Draw::texture` paints an upright rect; a turned page maps the raster's corners instead.
            xf.paint_image(&content.painter, tex.id, 0.0, 0.0, 1.0, 1.0);
        }
    }
}
