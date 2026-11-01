use super::find::Follow;
use super::{
    Arc, Color32, DocInfo, DocView, HashSet, PRINT_TAG, PT, PageText, Range, RenderPool, RenderRequest, RequestKind, TextureHandle, TextureOptions,
    device_pixels,
};

pub(super) const THUMB_TAG: u64 = 1 << 63;

pub(super) const TEXT_TAG: u64 = 1 << 62;

/// A page rendered for the organize grid at the size it is drawn there.
pub(super) const GRID_TAG: u64 = 1 << 61;

/// Preview rasters tagged with this bit are converted to luminance on receipt, matching the
/// monochrome print requested by Print ▸ Print in grayscale. It is a bit of its own: the
/// renderer's print bit ([`PRINT_TAG`]) is carried by every print preview, so sharing it would
/// turn colour previews into monochrome ones too.
pub(super) const GRAYSCALE_TAG: u64 = 1 << 59;

/// The tag of a raster that is out of date (shown until its replacement arrives).
pub(super) const STALE_TAG: u64 = u64::MAX;

/// Pages whose raster would exceed this many device pixels on a side are drawn in tiles.
pub(crate) const TILE_THRESHOLD: f32 = 4096.0;

/// Base device-pixel side of a page tile. The actual side grows with the zoom
/// ([`tile_side`]), so a zoomed viewport is covered by a few tiles instead of many.
pub(super) const TILE: u32 = 1024;

/// Longest side of the backdrop drawn under tiles, and of a live page preview (signature drag).
pub(crate) const BASE_SIDE: f32 = 2048.0;

/// Thumbnail slot before density scaling. The pages panel draws at most 150 logical points and
/// the organize grid about 146; when the byte budget allows, density reaches the screen so those
/// images are sampled down instead of stretched.
pub(super) const THUMB_W: f32 = 240.0;

pub(super) const THUMB_H: f32 = 320.0;

pub(super) const THUMB_BYTES: usize = 24 * 1024 * 1024;

pub(super) const PAGE_BYTES: usize = 128 * 1024 * 1024;

pub(super) const TILE_BYTES: usize = 128 * 1024 * 1024;

const UPLOAD_BYTES_PER_FRAME: usize = 16 * 1024 * 1024;

const RESULTS_PER_FRAME: usize = 8;

/// A defensive ceiling for unusually large viewports / n-up previews.
pub(super) const MAX_THUMB_DEMAND: usize = 256;

/// At most this many whole-page rasters are cached beyond the ones on screen.
const MAX_CACHED_PAGES: usize = 24;

/// At most this many text layers are cached beyond visible, selected and matched pages.
/// Text layers are glyph structs, far smaller than rasters; deliberately independent of
/// `MAX_CACHED_PAGES` so raster tuning never silently retunes text memory.
pub(super) const MAX_CACHED_TEXT_PAGES: usize = 24;

pub(super) fn rgba_bytes(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(4)
}

pub(super) fn texture_bytes(tex: &TextureHandle) -> usize {
    let [w, h] = tex.size();
    rgba_bytes(w, h)
}

/// Visible grid rows, plus one adjacent row in either direction. Work is independent of
/// document length, including after a large scroll jump.
pub(crate) fn thumbnail_rows(top: f32, bottom: f32, row_height: f32, rows: usize) -> Range<usize> {
    let first = (top.max(0.0) / row_height).floor() as usize;
    let end = (bottom.max(0.0) / row_height).ceil() as usize;
    first.saturating_sub(1).min(rows)..end.saturating_add(1).min(rows)
}

pub(super) struct PageTex {
    pub(super) tag: u64,
    pub(super) tex: TextureHandle,
}

/// Retained RGBA texture accounting for automation and performance regressions. These
/// surface bytes exclude driver overhead and temporary upload buffers; they are not RSS.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct RasterMemory {
    pub thumbnail_count: usize,
    pub thumbnail_bytes: usize,
    pub page_count: usize,
    pub page_bytes: usize,
    pub tile_count: usize,
    pub tile_bytes: usize,
    pub thumbnail_demand: usize,
    /// Last submitted missing results; workers may have completed some since submission.
    pub pending_requests: usize,
}

/// Thumbnail device-pixel density when `demand` pages share the thumbnail allowance.
pub(super) fn thumbnail_density(demand: usize, ppp: f32) -> f32 {
    let per_thumb = THUMB_BYTES / demand.max(1).next_power_of_two();
    ppp.clamp(0.25, 4.0).min((per_thumb as f32 / (4.0 * THUMB_W * THUMB_H)).sqrt() * 0.98)
}

pub(super) fn thumbnail_scale(width: f32, height: f32, density: f32) -> f32 {
    (THUMB_W / width.max(1.0)).min(THUMB_H / height.max(1.0)) * density
}

impl DocView {
    pub fn raster_memory(&self) -> RasterMemory {
        RasterMemory {
            thumbnail_count: self.thumbs.len(),
            thumbnail_bytes: self.thumbs.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            page_count: self.pages.len(),
            page_bytes: self.pages.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            tile_count: self.tiles.len(),
            tile_bytes: self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex))),
            thumbnail_demand: self.thumb_demand.len(),
            pending_requests: self.last_queue.len(),
        }
    }

    /// Pages that could not be rendered, with the reason (for automation; 0-based pages).
    pub fn page_errors(&self) -> Vec<(usize, &str)> {
        let mut v: Vec<(usize, &str)> = self.errors.iter().map(|(p, e)| (*p, e.as_str())).collect();
        v.sort_unstable_by_key(|(p, _)| *p);
        v
    }

    pub fn render_pending(&self) -> bool {
        !self.last_queue.is_empty() || self.text_background.pending() || self.move_background.pending()
    }

    /// The cached tile keys of a page, as (scale tag, tile x, tile y) (diagnostics and tests).
    pub fn tiles_debug(&self, page: usize) -> Vec<(u64, u32, u32)> {
        let mut k: Vec<(u64, u32, u32)> = self.tiles.keys().filter(|key| key.0 == page).map(|key| (key.1, key.2, key.3)).collect();
        k.sort_unstable();
        k
    }

    /// Re-render every page and text layer (the document's appearance changed, e.g. a layer
    /// was toggled). Out-of-date rasters stay on screen until their replacements arrive, so the
    /// view never flashes blank.
    pub fn invalidate_content(&mut self) {
        // Undo, redo and other document-wide changes: a released drawing's preview would be a ghost.
        self.comments.settling.clear();
        for p in self.pages.values_mut() {
            p.tag = STALE_TAG;
        }
        self.stale_thumbs.extend(self.thumbs.keys().copied());
        for slot in self.print_pages.values_mut() {
            slot.0 = STALE_TAG;
        }
        // Pages may have moved: a sharp render of another page would be worse than a soft one.
        self.grid_pages.clear();
        self.tiles.clear();
        self.stale_tiles.clear();
        self.forms.unbaked.clear();
        self.texts.clear();
        self.text_failed.clear();
        self.errors.clear();
        self.slow_pages.clear();
        self.waiting_since.clear();
        self.last_queue.clear();
        self.selection = None;
        self.all_text = false;
        // A content change is not reader navigation: keep the automatic anchor so the
        // re-extracted layers still follow it, and keep `follow_ended` so a reader who already
        // took over stays in control. `document_changed` clamps `current`; if the view no longer
        // sits where the automatic choice left it, `follow_page` ends it.
        if let Some(f) = self.find.as_mut() {
            f.matches.clear();
            f.current = None;
        }
    }

    /// Only `page` changed (a comment was added, edited or removed): re-render that page and
    /// re-read its text, keep everything else. Out-of-date tiles stay on screen until replacements arrive.
    pub fn page_changed(&mut self, page: usize) {
        if let Some(p) = self.pages.get_mut(&page) {
            p.tag = STALE_TAG;
        }
        if self.thumbs.contains_key(&page) {
            self.stale_thumbs.insert(page);
        }
        if let Some(slot) = self.print_pages.get_mut(&page) {
            slot.0 = STALE_TAG;
        }
        let current_tag = if self.current_scale_tag != 0 {
            self.current_scale_tag
        } else {
            self.tiles.keys().filter(|k| k.0 == page).map(|k| k.1).max().unwrap_or(0)
        };
        self.tiles.retain(|key, _| key.0 != page || key.1 == current_tag);
        self.stale_tiles.retain(|key| key.0 != page || key.1 == current_tag);
        for &key in self.tiles.keys() {
            if key.0 == page && key.1 == current_tag {
                self.stale_tiles.insert(key);
            }
        }
        self.slow_pages.remove(&page);
        self.texts.remove(&page);
        self.text_failed.remove(&page);
        self.errors.remove(&page);
        self.last_queue.clear();
        if self.selection.is_some_and(|s| s.page == page) {
            self.selection = None;
        }
    }

    /// A page's thumbnail texture, when rendered.
    pub(crate) fn thumb_id(&self, page: usize) -> Option<egui::TextureId> {
        self.thumbs.get(&page).map(|t| t.tex.id())
    }

    /// The whole-page raster on screen for `page` (the low-res backdrop of a tiled page), if any.
    pub(crate) fn page_raster(&self, page: usize) -> Option<egui::TextureId> {
        self.pages.get(&page).map(|p| p.tex.id())
    }

    /// Whether `page`'s whole-page raster (a tiled page's backdrop too) is still the one from
    /// before its last change, whatever its tiles are.
    pub(crate) fn page_raster_out_of_date(&self, page: usize) -> bool {
        self.pages.get(&page).is_some_and(|p| p.tag == STALE_TAG)
    }

    /// Whether `page` is still drawn, somewhere, as it was before its last change: its whole-page
    /// raster (a tiled page's backdrop too) or any of its tiles waits for a replacement.
    pub(crate) fn page_out_of_date(&self, page: usize) -> bool {
        self.pages.get(&page).is_some_and(|p| p.tag == STALE_TAG) || self.stale_tiles.iter().any(|k| k.0 == page)
    }

    /// The print preview's picture of `page`: the sheet raster when it has arrived, otherwise the
    /// thumbnail.
    pub(crate) fn page_preview(&self, page: usize) -> Option<egui::TextureId> {
        self.print_pages.get(&page).map(|(_, tex)| tex.id()).or_else(|| self.thumb_id(page))
    }

    pub(crate) fn page_text(&self, page: usize) -> Option<Arc<PageText>> {
        self.texts.get(&page).cloned()
    }

    pub fn thumb(&self, page: usize) -> Option<&TextureHandle> {
        self.thumbs.get(&page).map(|t| &t.tex)
    }

    /// How many device pixels wide the grid's sharp render of `page` is, if it has one.
    pub fn grid_page_pixels(&self, page: usize) -> Option<u32> {
        self.grid_pages.get(&page).map(|(w, _)| *w)
    }

    /// Start collecting demand from every consumer before scheduling or uploading results.
    pub(crate) fn begin_render_frame(&mut self) {
        self.frame_geometry_changed = false;
        self.pages_panel = None;
        self.panel_gap = None;
        self.thumb_demand.clear();
        self.frame_queue.clear();
        self.frame_visible.clear();
    }

    pub(crate) fn need_thumbnail(&mut self, page: usize, visible: bool) {
        if page >= self.page_count {
            return;
        }
        if let Some(priority) = self.thumb_demand.get_mut(&page) {
            *priority |= visible;
        } else if self.thumb_demand.len() < MAX_THUMB_DEMAND {
            self.thumb_demand.insert(page, visible);
        } else if visible && let Some(prefetch) = self.thumb_demand.iter().find_map(|(&p, &v)| (!v).then_some(p)) {
            self.thumb_demand.remove(&prefetch);
            self.thumb_demand.insert(page, true);
        }
    }

    /// Only the displayed document owns raster textures. Tabs keep their document and view
    /// state, but do not each retain a separate full application texture allowance.
    pub(crate) fn suspend_rendering(&mut self, pool: &RenderPool) {
        self.begin_render_frame();
        self.pages.clear();
        self.tiles.clear();
        self.thumbs.clear();
        self.grid_pages.clear();
        self.print_pages.clear();
        self.stale_thumbs.clear();
        self.stale_tiles.clear();
        self.waiting_since.clear();
        // Signature previews own a separate renderer and image/background textures.
        self.signature_drag = Default::default();
        self.text_background = Default::default();
        self.move_background = Default::default();
        if !self.last_queue.is_empty() {
            pool.set_queue(Vec::new());
            self.last_queue.clear();
        }
        // Results already finished for this tab would otherwise sit in its pool, rasters and
        // all, until the tab is shown again. (An inline pool renders inside try_recv: skip it.)
        if !pool.is_inline() {
            while pool.try_recv().is_some() {}
        }
    }

    pub(super) fn thumbnail_requests(&self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let mut pages: Vec<_> = self.thumb_demand.iter().map(|(&p, &visible)| (p, visible)).collect();
        pages.sort_unstable_by_key(|&(p, visible)| (!visible, p));
        // At high DPI or in a very large viewport every demanded cell still fits the same
        // byte allowance. Both dimensions are capped, including tall/wide page boxes.
        // The count is rounded up to a power of two: the scale is part of each request's tag,
        // so a density that followed every row scrolling in or out would re-render them all.
        let density = thumbnail_density(pages.len(), ppp);
        pages
            .into_iter()
            .filter_map(|(page, _)| {
                let p = info.pages.get(page)?;
                let scale = thumbnail_scale(p.width, p.height, density);
                Some(RenderRequest {
                    page,
                    kind: RequestKind::Pixels,
                    tile: None,
                    scale,
                    tag: THUMB_TAG | u64::from(scale.to_bits()),
                    // Thumbnails are previews: tiny images are skipped for them.
                    preview: true,
                })
            })
            .collect()
    }

    /// Admit current demand within explicit byte limits. Cached and pending surfaces share
    /// the allowance, so an evicted page can be requested again without a permanent done set.
    pub(super) fn prepare_render_queue(&mut self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let requests = std::mem::take(&mut self.frame_queue);
        let mut queue = Vec::new();
        let mut pages = HashSet::new();
        let mut tiles = HashSet::new();
        let mut tile_tags = HashSet::new();
        let mut grid = Vec::new();
        let (mut page_bytes, mut tile_bytes) = (0usize, 0usize);
        for req in requests.iter().filter(|r| r.kind == RequestKind::Pixels) {
            let Some(p) = info.pages.get(req.page) else { continue };
            // Organize-grid renders: only the cells in view that need one (organize_grid).
            if req.tag & GRID_TAG != 0 {
                grid.push(*req);
                continue;
            }
            // Print-preview rasters: a few sheets at the pane's size (the dialog bounds them).
            if req.tag & PRINT_TAG != 0 {
                if !self.errors.contains_key(&req.page) {
                    queue.push(*req);
                }
                continue;
            }
            // On-screen work is always admitted (the viewport bounds it); the limits only
            // decide how much prefetch fits beside it.
            let visible = self.frame_visible.contains(&req.page);
            if let Some(tile) = req.tile {
                // Keyed by exact device-pixel origin: the tile side varies with the zoom
                // (`tile_side`), so grid indices would need the scale to decode.
                let key = (req.page, req.tag, tile.x, tile.y);
                let bytes = rgba_bytes(tile.w as usize, tile.h as usize);
                if (!visible && tile_bytes.saturating_add(bytes) > TILE_BYTES) || !tiles.insert(key) {
                    continue;
                }
                tile_bytes = tile_bytes.saturating_add(bytes);
                tile_tags.insert(req.tag);
                if !self.tiles.contains_key(&key) || self.stale_tiles.contains(&key) {
                    queue.push(*req);
                }
            } else {
                let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
                if (!visible && (page_bytes.saturating_add(bytes) > PAGE_BYTES || pages.len() >= MAX_CACHED_PAGES)) || !pages.insert(req.page) {
                    continue;
                }
                page_bytes = page_bytes.saturating_add(bytes);
                if self.pages.get(&req.page).is_none_or(|p| p.tag != req.tag) {
                    queue.push(*req);
                }
            }
        }
        // Pages no longer demanded stay cached while the allowance has room, nearest to the
        // current page first, so turning back a page shows it at once instead of rendering.
        let mut undemanded: Vec<usize> = self.pages.keys().copied().filter(|p| !pages.contains(p)).collect();
        undemanded.sort_by_key(|p| p.abs_diff(self.current));
        let mut kept = pages.len();
        for page in undemanded {
            let bytes = self.pages.get(&page).map_or(0, |p| texture_bytes(&p.tex));
            if kept < MAX_CACHED_PAGES && page_bytes.saturating_add(bytes) <= PAGE_BYTES {
                page_bytes = page_bytes.saturating_add(bytes);
                kept += 1;
                pages.insert(page);
            }
        }
        self.pages.retain(|p, _| pages.contains(p));
        // Tiles of an earlier zoom (not a demanded scale) stay while the canvas keeps them, a
        // bounded few under a page whose current tiles are still missing (OLD_TILES). Tiles of a
        // page on screen stay too even when this frame does not demand them (#2196): the viewport
        // moves while zooming, so a tile that was sharp a moment ago falls outside the demanded
        // rectangle, and dropping it made every pan or zoom back a cold miss that re-blurred the
        // page and re-interpreted it once per tile. The per-frame cap below (OLD_TILES,
        // nearest scale first) is what bounds how much of that stays.
        self.tiles.retain(|key, _| tiles.contains(key) || !tile_tags.contains(&key.1) || self.frame_visible.contains(&key.0));
        self.stale_tiles.retain(|key| self.tiles.contains_key(key));
        queue.extend(grid);
        // Opening (or turning to) a page queues its pixels alongside thumbnails and
        // text extraction. The workers would otherwise spend the first seconds on
        // thumbnails and search text while the visible page stays blank: hold those
        // until no visible main-page raster is still missing. Errored pages never
        // block: they are never re-requested.
        let visible_mains_pending = queue.iter().any(|r| {
            r.kind == RequestKind::Pixels
                && r.tag & (GRID_TAG | PRINT_TAG | THUMB_TAG | TEXT_TAG) == 0
                && self.frame_visible.contains(&r.page)
                && !self.errors.contains_key(&r.page)
        });
        if visible_mains_pending {
            // Caches are kept; demand is re-evaluated next frame once pixels arrive.
            return queue;
        }
        let thumbs = self.thumbnail_requests(info, ppp);
        let mut thumb_bytes = 0usize;
        let mut admitted = HashSet::new();
        for req in thumbs {
            let Some(p) = info.pages.get(req.page) else { continue };
            let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
            if thumb_bytes.saturating_add(bytes) > THUMB_BYTES || self.errors.contains_key(&req.page) {
                continue;
            }
            thumb_bytes += bytes;
            admitted.insert(req.page);
            if self.thumbs.get(&req.page).is_none_or(|t| t.tag != req.tag) || self.stale_thumbs.contains(&req.page) {
                queue.push(req);
            }
        }
        self.thumbs.retain(|p, _| admitted.contains(p));
        self.stale_thumbs.retain(|p| admitted.contains(p));
        self.retain_text_layers();
        // Visible main pages and thumbnails take precedence over background search work.
        queue.extend(requests.into_iter().filter(|r| r.kind == RequestKind::Text));
        queue
    }

    /// Drop text layers nothing can still need. Every visited page's text used to stay
    /// cached for the session. Keep visible, selected and matched pages plus the nearest
    /// prefetch window; evicted pages re-extract on revisit. (Count window only: unlike
    /// rasters there is no byte budget here.) While a search is active keep everything:
    /// demand still wants every page, so evicting would re-request evicted pages each
    /// frame and churn the workers. Failures are never evicted either: they cost nothing
    /// to remember, and evicting one would re-request and re-fail it every frame.
    fn retain_text_layers(&mut self) {
        if self.texts.len() <= MAX_CACHED_TEXT_PAGES {
            return;
        }
        if self.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
            return;
        }
        let mut keep: HashSet<usize> = self.frame_visible.clone();
        if let Some(s) = self.selection {
            keep.insert(s.page);
        }
        if let Some(f) = self.find.as_ref() {
            keep.extend(f.matches.iter().map(|(p, _)| *p));
        }
        let mut rest: Vec<usize> = self.texts.keys().filter(|p| !keep.contains(*p)).copied().collect();
        rest.sort_by_key(|p| p.abs_diff(self.current));
        keep.extend(rest.into_iter().take(MAX_CACHED_TEXT_PAGES));
        self.texts.retain(|p, _| keep.contains(p));
    }

    pub(crate) fn finish_render_frame(&mut self, ctx: &egui::Context, info: &DocInfo, pool: &RenderPool) {
        if self.frame_geometry_changed {
            // Dialogs and edits can replace pages after the canvas collected demand. Its
            // scales and indexes belong to the old pages, not to the new renderer (#3149).
            self.begin_render_frame();
            self.last_queue.clear();
            pool.set_queue(Vec::new());
            ctx.request_repaint();
            return;
        }
        let queue = self.prepare_render_queue(info, ctx.pixels_per_point());
        if queue != self.last_queue {
            pool.set_queue(queue.clone());
            self.last_queue = queue;
        }
        if !self.hold_rasters {
            self.receive(ctx, pool);
        }
        if !self.last_queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// Pull a bounded batch of current results into textures. Demand is committed first, so
    /// offscreen results and obsolete scale/configuration tags never allocate a ColorImage.
    pub fn receive(&mut self, ctx: &egui::Context, pool: &RenderPool) {
        let budget = if pool.is_inline() { 1 } else { RESULTS_PER_FRAME };
        let mut uploaded = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        for _ in 0..budget {
            let Some(r) = pool.try_recv() else { break };
            uploaded = uploaded.saturating_add(self.receive_result(ctx, r));
            // One whole-page upload may exceed the frame allowance; it must still progress.
            if uploaded >= UPLOAD_BYTES_PER_FRAME {
                break;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if started.elapsed() >= std::time::Duration::from_millis(4) {
                break;
            }
        }
    }

    pub(super) fn receive_result(&mut self, ctx: &egui::Context, r: pdfcraft_render::RenderedPage) -> usize {
        if !self.last_queue.contains(&r.request) {
            return 0;
        }
        ctx.request_repaint();
        if r.request.kind == RequestKind::Text {
            match r.text {
                Some(t) => {
                    self.texts.insert(r.request.page, t);
                    self.refresh_find_page(r.request.page);
                }
                None => {
                    log::warn!("page {} text: {}", r.request.page + 1, r.error.unwrap_or_default());
                    self.text_failed.insert(r.request.page);
                    if let Some(f) = self.find.as_mut() {
                        let previous = f.current.and_then(|c| f.matches.get(c).cloned());
                        let follow = f.follow_page(self.current);
                        f.matches.retain(|(p, _)| *p != r.request.page);
                        match follow {
                            // An automatic choice keeps following its anchor; a failure never
                            // flashes, so the view stays exactly where it was.
                            Follow::Active(from) => f.current = f.first_at_or_after(from),
                            Follow::Ended | Follow::Never => {
                                let preserved = previous.as_ref().and_then(|k| f.matches.iter().position(|m| m == k));
                                f.current = preserved.or_else(|| f.first_at_or_after(self.current));
                            }
                        }
                    }
                }
            }
            return 0;
        }
        if let Some(e) = r.error {
            log::warn!("page {}: {e}", r.request.page + 1);
            self.errors.insert(r.request.page, e);
            return 0;
        }
        // Even current demand may have been sized with obsolete geometry or a different
        // device limit. Check before cache eviction, conversion and every upload path.
        let max_side = ctx.input(|i| i.max_texture_side);
        if r.width == 0 || r.height == 0 || r.width as usize > max_side || r.height as usize > max_side {
            return 0;
        }
        // Remember heavy pages: only these get the zoom/pan settle holds, so fast pages
        // never pay added latency. Stale entries die with the tiles on edits.
        if r.millis >= SLOW_MILLIS {
            self.slow_pages.insert(r.request.page);
        }
        let page = r.request.page;
        let bytes = rgba_bytes(r.width as usize, r.height as usize);
        if bytes != r.rgba.len() || !self.admit_texture(r.request, bytes) {
            return 0;
        }
        // The length was checked above, so the renderer's buffer becomes the texture data as is.
        let img = texture_image([r.width as usize, r.height as usize], r.rgba);
        if let Some(t) = r.request.tile {
            let key = (page, r.request.tag, t.x, t.y);
            let tex = ctx.load_texture(format!("tile-{:?}-{page}-{}-{}", self.id, t.x, t.y), img, TextureOptions::LINEAR);
            self.tiles.insert(key, tex);
            self.stale_tiles.remove(&key);
            if !self.stale_tiles.iter().any(|k| k.0 == page) {
                self.forms.unbaked.retain(|(_, p)| *p != page);
                self.comments.page_rendered(page);
            }
            if !self.page_out_of_date(page) {
                self.text_background.page_received(page);
            }
        } else if r.request.tag & PRINT_TAG != 0 {
            let tex = ctx.load_texture(
                format!("print-{:?}-{page}", self.id),
                if r.request.tag & GRAYSCALE_TAG != 0 { grayscale(img) } else { img },
                TextureOptions::LINEAR,
            );
            self.print_pages.insert(page, (r.request.tag, tex));
        } else if r.request.tag & GRID_TAG != 0 {
            let tex = ctx.load_texture(format!("grid-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.grid_pages.insert(page, (r.width, tex));
        } else if r.request.tag & THUMB_TAG != 0 {
            let tex = ctx.load_texture(format!("thumb-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.thumbs.insert(page, PageTex { tag: r.request.tag, tex });
            self.stale_thumbs.remove(&page);
        } else {
            let tex = ctx.load_texture(format!("page-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.pages.insert(page, PageTex { tag: r.request.tag, tex });
            self.signature_drag.page_received(page);
            self.move_background.page_received(page);
            if !self.page_out_of_date(page) {
                self.text_background.page_received(page);
            }
            self.comments.page_rendered(page);
            self.waiting_since.remove(&page);
            if !self.stale_tiles.iter().any(|k| k.0 == page) {
                self.forms.unbaked.retain(|(_, p)| *p != page);
            }
        }
        bytes
    }

    /// Check real dimensions before conversion, including stale textures kept during edits.
    /// Dropping an old handle releases its backend allocation after the current frame.
    pub(super) fn admit_texture(&mut self, req: RenderRequest, bytes: usize) -> bool {
        // An on-screen page or tile is always taken (the viewport bounds it): it may evict
        // off-screen rasters but never another on-screen one, and is never refused.
        let visible = req.tag & THUMB_TAG == 0 && self.frame_visible.contains(&req.page);
        // Organize-grid renders are bounded by the cells in view (organize_grid). Print
        // previews are the sheets around the one on screen.
        if req.tag & (GRID_TAG | PRINT_TAG) != 0 {
            return true;
        }
        if let Some(t) = req.tile {
            let key = (req.page, req.tag, t.x, t.y);
            self.tiles.remove(&key);
            let retained = self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex)));
            visible || bytes <= TILE_BYTES.saturating_sub(retained)
        } else {
            let thumb = req.tag & THUMB_TAG != 0;
            let (cache, limit) = if thumb { (&mut self.thumbs, THUMB_BYTES) } else { (&mut self.pages, PAGE_BYTES) };
            cache.remove(&req.page);
            let mut retained = cache.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex)));
            // A zoom/DPI change can temporarily retain larger stale images than the new
            // demand estimates. Evict those first, then let the current demand refill them.
            let on_screen = |p: &usize| !thumb && self.frame_visible.contains(p);
            while retained.saturating_add(bytes) > limit {
                let Some(page) = cache.keys().copied().filter(|p| !on_screen(p)).max_by_key(|p| p.abs_diff(self.current)) else { break };
                if let Some(old) = cache.remove(&page) {
                    retained = retained.saturating_sub(texture_bytes(&old.tex));
                }
            }
            visible || bytes <= limit.saturating_sub(retained)
        }
    }

    pub(super) fn render_scale(&self, ppp: f32) -> f32 {
        // Exactly the device scale: a raster at any other scale is resampled on screen, which
        // blurs every line and glyph (#260).
        self.zoom * PT * ppp
    }

    /// Set whether the print preview rasters become luminance before upload. Toggling it drops
    /// the colour previews: new requests carry the grayscale tag, so stale ones are replaced.
    pub(crate) fn set_print_grayscale(&mut self, grayscale: bool) {
        if self.grayscale != grayscale {
            self.grayscale = grayscale;
            self.print_pages.clear();
        }
    }

    /// Queue sharp rasters for the print preview. Pages that leave the nearby sheets are dropped.
    /// Previews render with reading colours off (the request's tag tells the renderer): they must
    /// match the printout, which is imposed from the uncoloured document.
    pub(crate) fn queue_print_previews(&mut self, pages: &[(usize, f32)]) {
        self.print_pages.retain(|page, _| pages.iter().any(|(p, _)| p == page));
        for &(page, scale) in pages {
            if self.errors.contains_key(&page) || !scale.is_finite() || scale <= 0.0 {
                continue;
            }
            let tag = scale_tag(scale) | PRINT_TAG | if self.grayscale { GRAYSCALE_TAG } else { 0 };
            let fresh = self.print_pages.get(&page).is_some_and(|(have, _)| *have == tag);
            if !fresh {
                self.frame_queue.push(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag, preview: false });
            }
        }
    }
}

/// The request tag for a raster at `scale`: equal tags mean the same scale (to 1/65536).
pub(super) fn scale_tag(scale: f32) -> u64 {
    (f64::from(scale) * 65536.0).round() as u64
}

/// The scale of a raster tagged by [`scale_tag`].
pub(super) fn tag_scale(tag: u64) -> f32 {
    (tag as f64 / 65536.0) as f32
}

/// Device-pixel side of a page tile at `scale` for a page `page_max_px` device pixels on
/// its long side. Each tile costs a full page interpret, so tiles grow with the zoom and
/// with the page: a zoomed viewport, or a giant sheet shown whole, is covered by a few
/// large tiles instead of dozens of small ones. Bounded by the render backend (tile sides
/// above 4096 are clamped there) and kept granular at ordinary zooms.
pub(super) fn tile_side(scale: f32, page_max_px: u32) -> u32 {
    if !scale.is_finite() || scale <= 0.0 {
        return TILE;
    }
    let by_zoom = (TILE as f32 * (scale / 6.0)).clamp(TILE as f32, 4096.0) as u32;
    // At most four tiles along the long side, so a giant sheet needs a handful of
    // interprets instead of dozens.
    by_zoom.max(page_max_px.div_ceil(4).min(4096))
}

/// Tiles of earlier zooms kept for a page while its current tiles are rendered.
pub(super) const OLD_TILES: usize = 64;

/// A page whose raster took at least this long is heavy: zoom/pan holds apply to it,
/// so gestures stay on stretched surfaces instead of queueing full-page interprets.
const SLOW_MILLIS: u32 = 500;

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
