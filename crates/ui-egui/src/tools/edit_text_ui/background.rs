use super::*;

/// A cached, read-only page background. The source glyphs are removed before rasterization;
/// colour, pictures and unselected text beneath the editor remain visible.
#[derive(Default)]
pub(crate) struct TextBackground {
    key: Option<(u64, usize, TextTarget)>,
    pub(super) renderer: Option<pdfcraft_render::RenderPool>,
    pub(super) texture: Option<egui::TextureHandle>,
    pub(super) tag: Option<u64>,
    received: Option<u64>,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_rect: Option<[f64; 4]>,
    /// This frame's place for the textless page under the open paragraph's editor. It stays
    /// empty, so the page shows its own text, until the paragraph is changed ([`Self::reveal`]).
    slot: Option<(egui::Painter, egui::layers::ShapeIdx, egui::Shape)>,
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
                && self.texture.is_some()
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
        let key = editor
            .map(|ed| (doc.edit_generation(), ed.page, TextTarget::Block(ed.block)))
            .or_else(|| draft.and_then(|d| d.index.map(|index| (doc.edit_generation(), d.page, TextTarget::Added(index)))));
        self.slot = None;
        if key.is_none() && self.settling.is_some() {
            return;
        }
        if self.key != key {
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
        let (Some((_, page, _)), Some(renderer)) = (self.key, self.renderer.as_ref()) else { return };
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
                self.texture = None;
                return;
            }
            if out.request.tag == tag {
                self.texture = Some(ctx.load_texture(
                    "text-editor-background",
                    crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba),
                    egui::TextureOptions::LINEAR,
                ));
                self.received = Some(tag);
            }
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
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))) && self.texture.is_some()
    }

    pub fn ready_added(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Added(index))) && self.texture.is_some()
    }

    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        xf: &PageXform,
        page: usize,
        editor: Option<&LineEditor>,
        draft: Option<&crate::content_ui::TextDraft>,
    ) {
        let for_editor = editor.is_some_and(|ed| ed.page == page && self.ready(page, ed.block));
        // A zoom or a turn of the view since the paragraph was applied would misplace its rows:
        // the page raster (with the old text) shows until the new one arrives instead.
        let settled =
            self.settling.as_ref().filter(|look| look.page == page && look.rot == xf.rot && (look.page_width - xf.rect.width()).abs() < 0.5);
        let ready =
            for_editor || draft.is_some_and(|d| d.page == page && d.index.is_some_and(|index| self.ready_added(page, index))) || settled.is_some();
        if !ready {
            return;
        }
        let Some(tex) = &self.texture else { return };
        // Font metrics can underestimate the actual glyph ink. Paint the complete textless
        // page so overhanging glyphs cannot remain outside the estimated source rectangle.
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
        if for_editor {
            // Its place under everything drawn on the page later; the editor fills it once the
            // paragraph changes (an unchanged paragraph looks as the page draws it).
            let mut mesh = egui::Mesh::with_texture(tex.id());
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                mesh.vertices.push(egui::epaint::Vertex { pos: xf.norm_to_screen(u, v), uv: Pos2::new(u, v), color: Color32::WHITE });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            let idx = painter.add(egui::Shape::Noop);
            self.slot = Some((painter, idx, egui::Shape::mesh(mesh)));
        } else {
            xf.paint_image(&painter, tex.id(), 0.0, 0.0, 1.0, 1.0);
            if let Some(look) = settled {
                painter.galley(xf.norm_to_screen(look.at.0, look.at.1), Arc::clone(&look.galley), look.color);
            }
        }
    }

    /// The open paragraph has changed: show the textless page under its editor this frame.
    pub fn reveal(&mut self) {
        if let Some((painter, idx, shape)) = self.slot.take() {
            painter.set(idx, shape);
        }
    }
}
