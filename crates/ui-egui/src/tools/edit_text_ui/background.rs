use craft::ui::primitives::textures::{CachedTexture, Revision, TextureState, TextureStore, TextureStoreOptions};
use craft::ui::primitives::{ContentPainterCx, Draw};
use egui::Color32;

use super::{Arc, Edit, LineEditor, PageXform, Pos2, TextTarget};

/// The store's salt: the one raster of the open paragraph's page lives under it.
pub(super) const BACKGROUND_SALT: &str = "pdf-text-editor-background";

/// What the store keys the raster by: the document generation, the page and the paragraph or added
/// text it was rendered without.
pub(super) type BackgroundKey = (u64, usize, TextTarget);

/// The store the raster lives in: the page-sized budget, since the raster stands in for the page.
pub(super) fn background_store(ctx: &egui::Context) -> TextureStore<BackgroundKey> {
    TextureStore::in_context(ctx, BACKGROUND_SALT, TextureStoreOptions::pages())
}

/// A cached, read-only page background. The source glyphs are removed before rasterization;
/// colour, pictures and unselected text beneath the editor remain visible.
#[derive(Default)]
pub(crate) struct TextBackground {
    key: Option<BackgroundKey>,
    renderer: Option<pdfcraft_render::RenderPool>,
    /// The scale (in thousandths) the raster in flight was asked for; the raster's revision.
    pub(super) tag: Option<u64>,
    received: Option<u64>,
    /// The raster as the store holds it, kept so `ready` and `paint` need no context. Its id stays
    /// valid while the key is in the store; every `prepare` marks the entry seen so it is not evicted.
    shown: Option<CachedTexture>,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_rect: Option<[f64; 4]>,
    /// How the open paragraph looked in its editor last frame.
    look: Option<EditorLook>,
    /// An applied paragraph's last look. The page raster still has the old text until the page
    /// is rendered again, so the textless page and this look stay up until then.
    settling: Option<EditorLook>,
}

/// A paragraph editor's text as last drawn: its laid-out rows, where they were on the page (in
/// the page's 0..1 coordinates), and the page's width and turn on screen then.
struct EditorLook {
    page: usize,
    block: usize,
    galley: Arc<egui::Galley>,
    at: (f32, f32),
    color: Color32,
    page_width: f32,
    rot: u16,
}

impl TextBackground {
    pub fn pending(&self) -> bool {
        self.renderer.is_some() && self.received != self.tag
    }

    /// The open paragraph's editor drew `galley` at `pos` this frame.
    pub fn remember(&mut self, xf: &PageXform, page: usize, block: usize, galley: Arc<egui::Galley>, pos: Pos2, color: Color32) {
        self.look = Some(EditorLook { page, block, galley, at: xf.screen_to_norm(pos), color, page_width: xf.rect.width(), rot: xf.rot });
    }

    /// An edit was applied. When it is the open paragraph's, its last look stays up over the
    /// textless page until the page is rendered again; any other edit ends both.
    pub fn committed(&mut self, edit: &Edit) {
        let look = self.look.take().filter(|look| {
            matches!(edit, Edit::EditTextBlock { page, block, .. } if (look.page, look.block) == (*page, *block))
                && self.key.is_some_and(|(_, page, target)| (page, target) == (look.page, TextTarget::Block(look.block)))
                && self.shown.is_some()
        });
        match look {
            Some(look) => {
                self.renderer = None;
                self.settling = Some(look);
            }
            None => *self = Self::default(),
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.settling.as_ref().is_some_and(|look| look.page == page) {
            *self = Self::default();
        }
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
        if self.key != key {
            if let Some(old) = &self.key {
                store.forget(ctx, old);
            }
            *self = Self { key, ..Default::default() };
            if let Some((_, page, target)) = key {
                self.renderer = match target {
                    TextTarget::Block(block) => doc.text_edit_preview(page, block),
                    TextTarget::Added(index) => doc.added_text_preview(page, index),
                }
                .ok()
                .map(|p| p.renderer());
            }
        }
        if let Some(ed) = editor {
            let draft_key = (ed.text.clone(), ed.style());
            if self.draft_key.as_ref() != Some(&draft_key) {
                self.draft_rect = doc.text_edit_layout(ed.page, ed.block, &ed.text, &draft_key.1).ok().and_then(|layout| layout.draft_rect);
                self.draft_key = Some(draft_key);
            }
        }
        let (Some(key), Some(renderer)) = (self.key, self.renderer.as_ref()) else { return };
        let page = key.1;
        let Some(p) = doc.info.pages.get(page) else { return };
        // One raster stands in for the page's tiles while a paragraph is open, so everything
        // around the editor is drawn from it: at the tiling threshold it stays sharp on a Retina
        // display up to about 2.5x on a Letter or A4 page, where the backdrop side (2048) went
        // soft past 1.2x.
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            renderer.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            self.tag = Some(tag);
        }
        if let Some(out) = renderer.try_recv() {
            if out.error.is_some() {
                self.renderer = None;
                self.shown = None;
                store.forget(ctx, &key);
                return;
            }
            if out.request.tag == tag {
                let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                self.shown = Some(store.put(ctx, key, Revision::new(tag), image));
                self.received = Some(tag);
            }
        }
        // Marks the entry seen this frame, so the store keeps it; one the store dropped is asked for again.
        if self.shown.is_some() && matches!(store.state(ctx, &key, Revision::new(tag)), TextureState::Missing) {
            self.shown = None;
            self.received = None;
        }
        if self.received != Some(tag) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// The draft's measured extent, when it was measured for paragraph `block` on `page`.
    pub fn draft_rect_for(&self, page: usize, block: usize) -> Option<[f64; 4]> {
        self.draft_rect.filter(|_| self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))))
    }

    pub fn ready(&self, page: usize, block: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))) && self.shown.is_some()
    }

    pub fn ready_added(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Added(index))) && self.shown.is_some()
    }

    /// Paints the textless page over `page` where a paragraph or added text is being changed, and an
    /// applied paragraph's last look until the page raster catches up. An open paragraph that has not
    /// changed paints nothing: the page draws its own text.
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
        // Font metrics can underestimate the actual glyph ink. Paint the complete textless
        // page so overhanging glyphs cannot remain outside the estimated source rectangle.
        if xf.rot == 0 {
            draw.texture(xf.rect, tex.id, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        } else {
            // `Draw::texture` paints an upright rect; a turned page maps the raster's corners instead.
            xf.paint_image(&content.painter, tex.id, 0.0, 0.0, 1.0, 1.0);
        }
        if let Some(look) = settled {
            content.painter.galley(xf.norm_to_screen(look.at.0, look.at.1), Arc::clone(&look.galley), look.color);
        }
    }
}
