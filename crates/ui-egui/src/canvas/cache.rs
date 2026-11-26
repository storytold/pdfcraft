// PLAN: `craft::text::find::FindMatches` needs the facade's `text` feature and craft-text's `find` feature in the app's Cargo.toml (pdf-hub-w1 owns the manifest); every other error here is a `DocView` field (`view`, `placed`, `revisions`, `reports`, `text_queue`, `text`) that pdf-canvas-w8's `DocView` hunk adds
use craft::host::queue::{Priority, QueueCounts};
use craft::text::find::FindMatches;
use craft::ui::primitives::textures::TextureState;

use super::{Arc, Color32, DocView, HashSet, PT, PageText, RenderRequest, RequestKind, TextLink};

/// At most this many text layers are cached beyond visible, selected and matched pages.
/// Text layers are glyph structs, far smaller than rasters.
pub(super) const MAX_CACHED_TEXT_PAGES: usize = 24;

/// Pages whose raster would exceed this many device pixels on a side are drawn in tiles.
pub(crate) const TILE_THRESHOLD: f32 = 4096.0;

/// Longest side of the backdrop drawn under tiles, and of a live page preview (signature drag).
pub(crate) const BASE_SIDE: f32 = 2048.0;

/// What a text-layer job found: the layer, or why there is none.
#[derive(Default)]
pub(crate) struct TextOutcome {
    text: Option<Arc<PageText>>,
    error: Option<String>,
    cut: bool,
}

impl DocView {
    /// Pages whose content was cut at the renderer's limits, so part of each is not drawn and
    /// its text is incomplete (#623; for automation; 0-based, in order).
    pub fn cut_pages(&self) -> Vec<usize> {
        self.reports.cut_pages()
    }

    /// Pages that could not be rendered, with the reason (for automation; 0-based pages, in order).
    pub fn page_errors(&self) -> Vec<(usize, String)> {
        self.reports.errors()
    }

    /// Whether a text layer, a background or a page picture is still on its way.
    pub fn render_pending(&self) -> bool {
        self.session.text_queue.as_ref().is_some_and(|queue| queue.counts() != QueueCounts::default())
            || self.session.text_background.pending()
            || self.session.move_background.pending()
            || self.session.placed.iter().any(|p| matches!(p.picture, TextureState::Loading { .. } | TextureState::Missing))
    }

    /// Re-render every page and text layer (the document's appearance changed, e.g. a layer
    /// was toggled). Out-of-date pictures stay on screen until their replacements arrive, so the
    /// view never flashes blank.
    pub fn invalidate_content(&mut self) {
        // Undo, redo and other document-wide changes: a released drawing's preview would be a ghost.
        self.session.comments.settling.clear();
        self.revisions.bump_all();
        self.session.forms.unbaked.clear();
        self.session.texts.clear();
        self.session.text_failed.clear();
        if let Some(queue) = &self.session.text_queue {
            let _dropped = queue.reprioritize(|_, _| None);
        }
        self.reports.clear();
        // Marks a tool waits to make go with the selection: its glyphs may name other text now.
        self.session.text.selection = None;
        self.session.marks_waiting = None;
        self.session.all_text = false;
        // A content change is not reader navigation: keep the automatic anchor so the
        // re-extracted layers still follow it, and keep `follow_ended` so a reader who already
        // took over stays in control. `document_changed` clamps `current`; if the view no longer
        // sits where the automatic choice left it, `follow_page` ends it.
        let current = self.current();
        if let Some(f) = self.find.as_mut() {
            f.matches = FindMatches::new(current);
        }
    }

    /// Only `page` changed (a comment was added, edited or removed): re-render that page and
    /// re-read its text, keep everything else.
    pub fn page_changed(&mut self, page: usize) {
        self.revisions.bump(page);
        self.session.texts.remove(&page);
        self.session.text_failed.remove(&page);
        if let Some(queue) = &self.session.text_queue {
            let _dropped = queue.reprioritize(|key, priority| (*key != page).then_some(priority));
        }
        self.reports.forget(page);
        if self.session.text.selection.as_ref().is_some_and(|s| s.pages().contains(&page)) {
            // With the marks a tool waits to make on it (#5017 review).
            self.session.text.selection = None;
            self.session.marks_waiting = None;
        }
    }

    /// Finds the addresses written as plain text on `page` (#4207): once per text layer, and only
    /// once the page's text is known. The addresses of text layers that were dropped or replaced
    /// are forgotten.
    pub(crate) fn refresh_text_links(&mut self, page: usize) {
        let texts = &self.session.texts;
        self.session.text_links.retain(|p, (t, _)| texts.get(p).is_some_and(|current| Arc::ptr_eq(current, t)));
        if self.session.text_links.contains_key(&page) {
            return;
        }
        if let Some(text) = self.session.texts.get(&page).cloned() {
            let found: Arc<[TextLink]> = pdfcraft_engine::text_links::find_text_links(&text).into();
            self.session.text_links.insert(page, (text, found));
        }
    }

    /// Whether `page`'s text layer has arrived from the renderer. Selecting text, the I-beam and
    /// find matches on a page all wait for it (tests and automation).
    pub fn has_page_text(&self, page: usize) -> bool {
        self.session.texts.contains_key(&page)
    }

    /// The text-layer step of a pass: takes the layers the jobs have finished (every result is for the current
    /// content: a change drops the jobs it outdates, so their results are never delivered), then asks for the ones the
    /// pass needs: the `visible` pages first, then `rest` in the order given (the selection's pages, and while a search
    /// runs every page from the current one on). Pages whose layer is held, failed or already on its way are not asked
    /// again, so calling it every pass submits nothing new. The queue's own wake asks for the pass a result needs.
    pub(crate) fn want_text(&mut self, session: &pdfcraft_engine::Session, visible: &[usize], rest: &[usize]) {
        while let Some(done) = self.session.text_queue.as_ref().and_then(|queue| queue.try_recv()) {
            let page = done.key;
            let outcome = done.result;
            // Whether drawn or read for its text, a page cut at the limits is named (#623).
            if outcome.cut {
                self.reports.cut(page);
            }
            match outcome.text {
                Some(t) => {
                    self.session.texts.insert(page, t);
                    self.refresh_find_page(page);
                }
                None => {
                    log::warn!("page {} text: {}", page + 1, outcome.error.unwrap_or_default());
                    self.session.text_failed.insert(page);
                    if let Some(f) = self.find.as_mut() {
                        f.matches.set_page(page, Vec::new());
                    }
                }
            }
        }
        self.retain_text_layers(visible);
        let Some(doc) = session.get(self.id) else { return };
        let Some(queue) = self.session.text_queue.as_ref() else { return };
        let config = session.render_config(self.id).unwrap_or_default();
        let ranked = visible.iter().map(|p| (*p, Priority::VISIBLE)).chain(rest.iter().map(|p| (*p, Priority::BACKGROUND)));
        for (page, priority) in ranked {
            if page >= self.page_count || self.session.texts.contains_key(&page) || self.session.text_failed.contains(&page) || queue.contains(&page)
            {
                continue;
            }
            let (bytes, config) = (doc.display.clone(), config.clone());
            queue.submit(page, priority, move |cancel| {
                if cancel.is_cancelled() {
                    return TextOutcome::default();
                }
                let mut renderer = pdfcraft_render::PageRenderer::new(bytes, config);
                let out = renderer.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
                TextOutcome { cut: out.warnings.contains(&pdfcraft_render::RenderWarning::ContentTruncated), text: out.text, error: out.error }
            });
        }
    }

    /// Drop text layers nothing can still need. Keep visible, selected and matched pages plus
    /// the nearest prefetch window; evicted pages re-extract on revisit. (Count window only: unlike
    /// pictures there is no byte budget here.) While a search is active keep everything:
    /// demand still wants every page, so evicting would re-request evicted pages each
    /// pass and churn the workers. Failures are never evicted either: they cost nothing
    /// to remember, and evicting one would re-request and re-fail it every pass.
    fn retain_text_layers(&mut self, visible: &[usize]) {
        if self.session.texts.len() <= MAX_CACHED_TEXT_PAGES {
            return;
        }
        if self.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
            return;
        }
        let mut keep: HashSet<usize> = visible.iter().copied().collect();
        if let Some(s) = self.session.text.selection.as_ref() {
            keep.extend(s.pages());
        }
        if let Some(f) = self.find.as_ref() {
            keep.extend(f.matches.matches().iter().map(|m| m.page));
        }
        let mut rest: Vec<usize> = self.session.texts.keys().filter(|p| !keep.contains(*p)).copied().collect();
        let current = self.current();
        rest.sort_by_key(|p| p.abs_diff(current));
        keep.extend(rest.into_iter().take(MAX_CACHED_TEXT_PAGES));
        self.session.texts.retain(|p, _| keep.contains(p));
    }

    /// Device scale of a page raster: exactly the device scale, as a raster at any other scale is
    /// resampled on screen, which blurs every line and glyph (#260).
    pub(super) fn render_scale(&self, ppp: f32) -> f32 {
        self.view.zoom as f32 * PT * ppp
    }
}

/// A rendered raster as texture data. Its words are premultiplied RGBA bytes, exactly
/// `Color32`s, so the renderer's buffer becomes the image without a copy on the UI thread.
pub(crate) fn texture_image(size: [usize; 2], pixels: pdfcraft_render::Pixels) -> egui::ColorImage {
    match bytemuck::allocation::try_cast_vec::<u32, Color32>(pixels.into_words()) {
        Ok(px) if px.len() == size[0].saturating_mul(size[1]) => egui::ColorImage::new(size, px),
        Ok(px) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&px)),
        Err((_, words)) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&words)),
    }
}

/// A luminance preview for a monochrome print. `Color32` stores premultiplied pixels, so a
/// non-opaque colour's alpha is restored after multiplying the unmultiplied RGB by its
/// luminance. This uses the display gamma-space weights users expect in editors/print dialogs,
/// not the linear-light coefficients of a colour-managed render.
pub(super) fn grayscale(mut img: egui::ColorImage) -> egui::ColorImage {
    for c in img.pixels.iter_mut() {
        let a = c.a();
        let luminance = f32::from(c.r()) * 0.299_f32 + f32::from(c.g()) * 0.587_f32 + f32::from(c.b()) * 0.114_f32;
        let gray = if a == 0 || a == 255 {
            luminance.round() as u8
        } else {
            // Unmultiply first, so translucent content keeps its paper-relative luminance.
            let f = f32::from(a) / 255.0;
            let un = [f32::from(c.r()) / f, f32::from(c.g()) / f, f32::from(c.b()) / f];
            let l = un[0] * 0.299_f32 + un[1] * 0.587_f32 + un[2] * 0.114_f32;
            (f * l).round() as u8
        };
        *c = Color32::from_rgba_premultiplied(gray, gray, gray, a);
    }
    img
}
