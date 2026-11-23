use super::worker::{DraftWorker, Request};
use super::{Color32, Edit, LineEditor, PageXform, Pos2, TextTarget};
use std::sync::Arc;

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
        self.raster_pending() || self.serial.is_some() || self.draft_key != self.render_key || self.retiring.iter().any(|pool| !pool.exited())
    }

    /// The open paragraph's editor was drawn at this page placement this frame.
    pub fn remember(&mut self, xf: &PageXform, page: usize, block: usize) {
        self.look = Some(EditorLook { page, block, page_width: xf.rect.width(), rot: xf.rot });
    }

    /// An edit was applied. When it is the open paragraph's, its last look stays up over the
    /// draft raster until the page is rendered again; any other edit ends both.
    pub fn committed(&mut self, edit: &Edit) {
        let look = self.look.take().filter(|look| {
            // Only a raster of the applied text and style stands in for it.
            matches!(edit, Edit::EditTextBlock { page, block, text, style }
                if (look.page, look.block) == (*page, *block)
                    && self.texture_key.as_ref().is_some_and(|(shown, shown_style)| shown == text && shown_style == style))
                && self.key.is_some_and(|(_, page, target)| (page, target) == (look.page, TextTarget::Block(look.block)))
                && self.texture.is_some()
        });
        match look {
            Some(look) => {
                self.retire_renderer();
                if let Some(worker) = &mut self.worker {
                    worker.release();
                }
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
            let Some(page) = doc.info.pages.get(ed.page) else { return };
            let max_side = ctx.input(|i| i.max_texture_side) as f32;
            let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / page.width.max(page.height).max(1.0));
            let tag = (scale * 1000.0) as u64;
            if self.tag != Some(tag) {
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
                        if let Some(texture) = &mut self.texture {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            self.texture = Some(ctx.load_texture("text-editor-background", image, egui::TextureOptions::LINEAR));
                        }
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
