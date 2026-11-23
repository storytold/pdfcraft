use craft::ui::primitives::textures::{CachedTexture, Revision, TextureState, TextureStore, TextureStoreOptions};
use craft::ui::primitives::{ContentPainterCx, Draw};

use super::{Edit, LineEditor, PageXform, Pos2, TextTarget};

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
    shown: Option<CachedTexture>,
    // Wait for cancelled workers to exit before starting another draft. A slow image decoder
    // must not accumulate one render thread per keystroke.
    retiring: Vec<pdfcraft_render::Retiring>,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    render_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_layout: Option<pdfcraft_engine::TextEditLayout>,
    draft_error: Option<String>,
    /// How the open paragraph looked in its editor last frame.
    look: Option<EditorLook>,
    /// An applied paragraph's last look. The page raster still has the old text until the page
    /// is rendered again, so the draft raster stays up until then.
    settling: Option<EditorLook>,
}

/// The page placement of the last draft, retained until the applied page raster arrives.
struct EditorLook {
    page: usize,
    block: usize,
    page_width: f32,
    rot: u16,
}

impl TextBackground {
    fn raster_pending(&self) -> bool {
        self.renderer.is_some() && self.received != self.tag
    }

    pub fn pending(&self) -> bool {
        self.raster_pending() || self.draft_key != self.render_key || self.retiring.iter().any(|pool| !pool.exited())
    }

    /// The open paragraph's editor was drawn at this page placement this frame.
    pub fn remember(&mut self, xf: &PageXform, page: usize, block: usize) {
        self.look = Some(EditorLook { page, block, page_width: xf.rect.width(), rot: xf.rot });
    }

    /// An edit was applied. When it is the open paragraph's, its last look stays up over the
    /// draft raster until the page is rendered again; any other edit ends both.
    pub fn committed(&mut self, edit: &Edit) {
        let look = self.look.take().filter(|look| {
            matches!(edit, Edit::EditTextBlock { page, block, .. } if (look.page, look.block) == (*page, *block))
                && self.key.is_some_and(|(_, page, target)| (page, target) == (look.page, TextTarget::Block(look.block)))
                && self.shown.is_some()
        });
        match look {
            Some(look) => {
                self.retire_renderer();
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
        *self = Self { key, retiring, ..Default::default() };
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
            let draft_key = (ed.text.clone(), ed.style());
            self.layout_for(doc, ed, &ed.text);
            // Coalesce typing while one raster is in flight; the next render uses the latest
            // draft. Keep the last faithful raster visible instead of exposing UI fallback ink.
            if self.render_key.as_ref() != Some(&draft_key) && !self.raster_pending() && self.retiring.is_empty() {
                if self.renderer.is_some() {
                    self.retire_renderer();
                    ctx.request_repaint();
                    return;
                }
                match doc.text_edit_draft_preview(ed.page, ed.block, &ed.text, &draft_key.1) {
                    Ok(preview) => {
                        self.renderer = Some(preview.renderer().without_replacements());
                        self.draft_error = None;
                    }
                    Err(error) => self.draft_error = Some(error),
                }
                self.tag = None;
                self.received = None;
                self.render_key = Some(draft_key);
            }
        } else if let Some((_, page, TextTarget::Added(index))) = key
            && self.renderer.is_none()
            && self.tag.is_none()
            && self.retiring.is_empty()
        {
            self.renderer = doc.added_text_preview(page, index).ok().map(|p| p.renderer().without_replacements());
        }
        let Some(key) = self.key else { return };
        let page = key.1;
        let Some(p) = doc.info.pages.get(page) else { return };
        // One raster stands in for the page's tiles while a paragraph is open, so everything
        // around the editor is drawn from it: at the tiling threshold it stays sharp on a Retina
        // display up to about 2.5x on a Letter or A4 page, where the backdrop side (2048) went
        // soft past 1.2x.
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if let Some(renderer) = self.renderer.as_ref() {
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
                    // In place under the key: the editor's last draft raster is replaced, not stacked.
                    self.shown = Some(store.put(ctx, key, Revision::new(tag), image));
                    self.received = Some(tag);
                    // The draft may have changed while this render was running.
                    ctx.request_repaint();
                }
            }
        }
        // Marks the entry seen this frame, so the store keeps it; one the store dropped is asked for again.
        if let Some(shown_tag) = self.received.filter(|_| self.shown.is_some())
            && matches!(store.state(ctx, &key, Revision::new(shown_tag)), TextureState::Missing)
        {
            self.shown = None;
            self.received = None;
            // The raster is asked for again: a paragraph's draft from a new preview, added text's
            // textless page from its renderer.
            self.tag = None;
            self.render_key = None;
        }
        if self.renderer.is_some() && self.received != Some(tag) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Measure the current keystroke as well as the draft prepared before UI input.
    pub fn layout_for(&mut self, doc: &pdfcraft_engine::Document, ed: &LineEditor, text: &str) -> Option<&pdfcraft_engine::TextEditLayout> {
        let key = Some((doc.edit_generation(), ed.page, TextTarget::Block(ed.block)));
        if self.key != key {
            self.reset(key);
        }
        let draft_key = (text.to_owned(), ed.style());
        if self.draft_key.as_ref() != Some(&draft_key) {
            self.draft_layout = doc.text_edit_layout(ed.page, ed.block, text, &draft_key.1).ok();
            self.draft_key = Some(draft_key);
        }
        self.draft_layout.as_ref()
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

    pub fn ready_added(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Added(index))) && self.shown.is_some()
    }

    /// Paints the PDF draft over `page` where a paragraph is being changed (the textless page for
    /// added text), and an applied paragraph's draft until the page raster catches up. An open
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
        let for_editor = editor.is_some_and(|ed| ed.page == page && ed.has_changes() && self.ready(page, ed.block));
        // A zoom or a turn of the view since the paragraph was applied would misplace its rows:
        // the page raster (with the old text) shows until the new one arrives instead.
        let settled =
            self.settling.as_ref().filter(|look| look.page == page && look.rot == xf.rot && (look.page_width - xf.rect.width()).abs() < 0.5);
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
