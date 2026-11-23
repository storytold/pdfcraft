use super::{Color32, Edit, LineEditor, PageXform, Pos2, TextTarget};

/// A cached, read-only PDF raster. Paragraph drafts include their real PDF glyphs;
/// retyping an added item still uses a textless background.
#[derive(Default)]
pub(crate) struct TextBackground {
    key: Option<(u64, usize, TextTarget)>,
    pub(super) renderer: Option<pdfcraft_render::RenderPool>,
    pub(super) texture: Option<egui::TextureHandle>,
    pub(super) tag: Option<u64>,
    received: Option<u64>,
    // Wait for cancelled workers to exit before starting another draft. A slow image decoder
    // must not accumulate one render thread per keystroke.
    retiring: Vec<pdfcraft_render::Retiring>,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    render_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_layout: Option<pdfcraft_engine::TextEditLayout>,
    draft_error: Option<String>,
    /// This frame's place for the PDF draft under the open paragraph's editor. It stays
    /// empty, so the page shows its own text, until the paragraph is changed ([`Self::reveal`]).
    slot: Option<(egui::Painter, egui::layers::ShapeIdx, egui::Shape)>,
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
                && self.texture.is_some()
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

    fn reset(&mut self, key: Option<(u64, usize, TextTarget)>) {
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
        let key = editor
            .map(|ed| (doc.edit_generation(), ed.page, TextTarget::Block(ed.block)))
            .or_else(|| draft.and_then(|d| d.index.map(|index| (doc.edit_generation(), d.page, TextTarget::Added(index)))));
        self.slot = None;
        if key.is_none() && self.settling.is_some() {
            return;
        }
        self.retiring.retain(|pool| !pool.exited());
        if self.key != key {
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
            if let Some(error) = out.error {
                self.draft_error = Some(error);
                self.retire_renderer();
                self.received = Some(tag);
                ctx.request_repaint();
                return;
            }
            if out.request.tag == tag {
                self.texture = Some(ctx.load_texture(
                    "text-editor-background",
                    crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba),
                    egui::TextureOptions::LINEAR,
                ));
                self.received = Some(tag);
                // The draft may have changed while this render was running.
                ctx.request_repaint();
            }
        }
        if self.received != Some(tag) {
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
        // Paint the complete PDF draft so overhanging glyphs cannot leave stale source ink
        // outside the estimated paragraph rectangle.
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
        }
    }

    /// The open paragraph has changed: show its PDF draft under the caret this frame.
    pub fn reveal(&mut self) {
        if let Some((painter, idx, shape)) = self.slot.take() {
            painter.set(idx, shape);
        }
    }
}
