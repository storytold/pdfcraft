//! Thread-safe Engine facade, document handles, and sandbox confinement.

use pdfcraft_engine::{DocId, Session};
use pdfcraft_render::{PageRenderer, RenderConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::cache::{ByteBudgetedTileCache, TileCacheKey};
use crate::document::{
    FfiDocInfo, FfiFindMatch, FfiLink, FfiOutlineItem, FfiPageGeometry, FfiRecoverySnapshot, FfiRect, find_in_page_text, map_doc_info, map_outline,
};
use crate::edit::{FfiAnnotationSummary, FfiCommentItem, FfiInkStroke, FfiPoint, FfiReviewState, FfiShapeKind, FfiTextMarkupKind, rgb_from_u32};
use crate::error::{FfiError, catch_panic};
use crate::forms::{FfiChoiceOption, FfiFormField};
use crate::prefetch::{FfiPrefetchPlan, FfiScrollDirection, RequestCoalescer, plan_prefetch_pages};
use crate::render::{CancellationTracker, FfiRenderRequest, FfiRenderResult, render_tile_sync};

fn view_rect_to_user_box(page_info: &pdfcraft_render::PageInfo, r: &FfiRect) -> [f64; 4] {
    let [ux0, uy0] = page_info.view_to_user(r.x, r.y);
    let [ux1, uy1] = page_info.view_to_user(r.x + r.width, r.y + r.height);
    let min_x = f64::from(ux0.min(ux1));
    let max_x = f64::from(ux0.max(ux1));
    let min_y = f64::from(uy0.min(uy1));
    let max_y = f64::from(uy0.max(uy1));
    [min_x, min_y, max_x, max_y]
}

fn user_box_to_view_rect(page_info: &pdfcraft_render::PageInfo, u: [f32; 4]) -> FfiRect {
    let [vx0, vy0] = page_info.user_to_view(u[0], u[1]);
    let [vx1, vy1] = page_info.user_to_view(u[2], u[3]);
    let min_x = vx0.min(vx1);
    let max_x = vx0.max(vx1);
    let min_y = vy0.min(vy1);
    let max_y = vy0.max(vy1);
    FfiRect { x: min_x, y: min_y, width: (max_x - min_x).max(0.0), height: (max_y - min_y).max(0.0) }
}

struct EngineInner {
    session: Session,
    root: Option<PathBuf>,
    doc_bytes: HashMap<u64, Arc<Vec<u8>>>,
    doc_passwords: HashMap<u64, Option<Arc<str>>>,
    doc_versions: HashMap<u64, u64>,
    cancellations: CancellationTracker,
    tile_cache: ByteBudgetedTileCache,
    coalescer: RequestCoalescer,
}

impl EngineInner {
    fn extract_page_text(&self, doc_id: u64, page: u32) -> Result<Arc<pdfcraft_render::text::PageText>, FfiError> {
        let doc = self.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
        if (page as usize) >= doc.info.pages.len() {
            return Err(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 });
        }
        let bytes = self.doc_bytes.get(&doc_id).cloned().ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
        let password = self.doc_passwords.get(&doc_id).cloned().flatten();
        let mut renderer = PageRenderer::new(bytes, RenderConfig { password, ..Default::default() });
        let out =
            renderer.render(pdfcraft_render::RenderRequest { page: page as usize, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
        Ok(out.text.unwrap_or_default())
    }

    fn apply_edit(&mut self, doc_id: u64, edit: pdfcraft_engine::Edit) -> Result<(), FfiError> {
        let doc_key = DocId(doc_id);
        self.session.apply(doc_key, edit).map_err(FfiError::from)?;
        if let Some(doc) = self.session.get(doc_key) {
            self.doc_bytes.insert(doc_id, doc.bytes.clone());
            let ver = self.doc_versions.entry(doc_id).or_insert(1);
            *ver += 1;
            self.tile_cache.evict_for_doc(doc_id);
        }
        Ok(())
    }
}

/// Stepwise memory pressure level signaled by the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiMemoryPressureLevel {
    /// Moderate memory pressure: trims tile cache to 50% budget and cancels speculative prefetch.
    Moderate,
    /// Critical memory pressure: drops all in-memory tile caches and cancels all render requests.
    Critical,
}

/// The core engine instance, safe to call concurrently from any thread.
///
/// Threading model:
/// A Mutex wraps `Session` to guarantee atomic state transitions, fast queries,
/// and sequential undo/redo histories without inter-thread channel hops.
/// Rendering takes an `Arc<Vec<u8>>` reference and runs concurrently without
/// holding the lock.
#[derive(uniffi::Object)]
pub struct Engine {
    inner: Mutex<EngineInner>,
}

#[uniffi::export]
impl Engine {
    /// Creates a new Engine instance with the default 64 MiB tile cache.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::with_tile_cache_bytes(64 * 1024 * 1024)
    }

    /// Creates an Engine instance with a custom tile cache memory budget in bytes.
    #[uniffi::constructor]
    pub fn with_tile_cache_bytes(tile_cache_bytes: u64) -> Arc<Self> {
        crate::threading::init_worker_threading_hook();
        Arc::new(Self {
            inner: Mutex::new(EngineInner {
                session: Session::new(),
                root: None,
                doc_bytes: HashMap::new(),
                doc_passwords: HashMap::new(),
                doc_versions: HashMap::new(),
                cancellations: CancellationTracker::new(),
                tile_cache: ByteBudgetedTileCache::new(tile_cache_bytes as usize),
                coalescer: RequestCoalescer::new(),
            }),
        })
    }

    /// Confines all file access to the specified sandbox root directory.
    pub fn set_sandbox_root(&self, root_path: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let root = PathBuf::from(root_path).canonicalize().map_err(|e| FfiError::Io { reason: format!("invalid root: {e}") })?;
            if !root.is_dir() {
                return Err(FfiError::Io { reason: "sandbox root must be a directory".into() });
            }
            inner.root = Some(root);
            Ok(())
        })
    }

    /// Opens a document from raw memory bytes.
    pub fn open_bytes(&self, name: String, bytes: Vec<u8>, password: Option<String>) -> Result<u64, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let arc_bytes = Arc::new(bytes);
            let doc_id = inner.session.open(&name, None, arc_bytes.clone(), password.as_deref()).map_err(FfiError::from)?;

            inner.doc_bytes.insert(doc_id.0, arc_bytes);
            inner.doc_passwords.insert(doc_id.0, password.map(Arc::from));
            inner.doc_versions.insert(doc_id.0, 1);
            Ok(doc_id.0)
        })
    }

    /// Opens a document from the local filesystem, confined to sandbox root if set.
    pub fn open_file(&self, path_str: String, password: Option<String>) -> Result<u64, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;

            let path = Path::new(&path_str);
            let canonical_path = if let Some(ref root) = inner.root {
                let joined = root.join(path);
                let real = joined.canonicalize().map_err(|e| FfiError::Io { reason: format!("failed to resolve {path_str}: {e}") })?;
                if !real.starts_with(root) {
                    return Err(FfiError::Io { reason: format!("{path_str} is outside sandbox root {}", root.display()) });
                }
                real
            } else {
                path.canonicalize().map_err(|e| FfiError::Io { reason: format!("cannot open {path_str}: {e}") })?
            };

            let name = canonical_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());

            let bytes = std::fs::read(&canonical_path).map_err(|e| FfiError::Io { reason: format!("cannot read {path_str}: {e}") })?;

            let arc_bytes = Arc::new(bytes);
            let doc_id = inner
                .session
                .open(&name, Some(canonical_path.to_string_lossy().into_owned()), arc_bytes.clone(), password.as_deref())
                .map_err(FfiError::from)?;

            inner.doc_bytes.insert(doc_id.0, arc_bytes);
            inner.doc_passwords.insert(doc_id.0, password.map(Arc::from));
            inner.doc_versions.insert(doc_id.0, 1);
            Ok(doc_id.0)
        })
    }

    /// Creates a new PDF document from a collection of raw image bytes (PNG, JPEG, etc.),
    /// opens it in the session, and returns its new document ID.
    pub fn create_from_images(&self, name: String, image_bytes: Vec<Vec<u8>>) -> Result<u64, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            if image_bytes.is_empty() {
                return Err(FfiError::InvalidArgs { reason: "at least one image is required to create a PDF".into() });
            }

            let images: Vec<(String, Vec<u8>)> =
                image_bytes.into_iter().enumerate().map(|(idx, bytes)| (format!("image_{}.jpg", idx + 1), bytes)).collect();

            let pdf_bytes = inner.session.create_from_images(&images).map_err(FfiError::from)?;
            let doc_id = inner.session.open(&name, None, pdf_bytes.clone(), None).map_err(FfiError::from)?;

            inner.doc_bytes.insert(doc_id.0, pdf_bytes);
            inner.doc_passwords.insert(doc_id.0, None);
            inner.doc_versions.insert(doc_id.0, 1);
            Ok(doc_id.0)
        })
    }

    /// Closes an open document and releases its associated buffers.
    pub fn close(&self, doc_id: u64) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            if !inner.doc_bytes.contains_key(&doc_id) {
                return Err(FfiError::NotFound { id: doc_id.to_string() });
            }
            inner.session.close(DocId(doc_id));
            inner.doc_bytes.remove(&doc_id);
            inner.doc_passwords.remove(&doc_id);
            inner.doc_versions.remove(&doc_id);
            inner.tile_cache.evict_for_doc(doc_id);
            Ok(())
        })
    }

    /// Closes all open documents and releases all buffers.
    pub fn close_all(&self) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.session = Session::new();
            inner.doc_bytes.clear();
            inner.doc_passwords.clear();
            inner.doc_versions.clear();
            inner.tile_cache.clear();
            let next_gen = inner.coalescer.next_generation();
            inner.cancellations.retire_older_than(next_gen);
            inner.coalescer.clear();
            Ok(())
        })
    }

    /// Handles stepwise memory pressure degradation (e.g. from iOS memory warnings or Android onTrimMemory).
    pub fn handle_memory_pressure(&self, level: FfiMemoryPressureLevel) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            match level {
                FfiMemoryPressureLevel::Moderate => {
                    let half_budget = inner.tile_cache.max_bytes() / 2;
                    inner.tile_cache.trim_to_bytes(half_budget);
                    inner.cancellations.retire_older_than(inner.coalescer.current_generation());
                }
                FfiMemoryPressureLevel::Critical => {
                    let next_gen = inner.coalescer.next_generation();
                    inner.cancellations.retire_older_than(next_gen);
                    inner.tile_cache.clear();
                    inner.coalescer.clear();
                }
            }
            Ok(())
        })
    }

    /// Trims internal caches and unneeded memory in response to OS memory warnings.
    pub fn trim_memory(&self) -> Result<(), FfiError> {
        self.handle_memory_pressure(FfiMemoryPressureLevel::Critical)
    }

    /// Returns the number of cached raster tiles in memory.
    pub fn cached_tile_count(&self) -> u32 {
        self.inner.lock().map(|inner| inner.tile_cache.len() as u32).unwrap_or(0)
    }

    /// Returns the current memory usage of cached tiles in bytes.
    pub fn cached_tile_bytes(&self) -> u64 {
        self.inner.lock().map(|inner| inner.tile_cache.current_bytes() as u64).unwrap_or(0)
    }

    /// Returns the number of currently open documents.
    pub fn document_count(&self) -> u32 {
        self.inner.lock().map(|inner| inner.session.docs().len() as u32).unwrap_or(0)
    }

    /// Returns summary metadata for an open document.
    pub fn doc_info(&self, doc_id: u64) -> Result<FfiDocInfo, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(map_doc_info(&doc.info))
        })
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self, doc_id: u64) -> Result<u32, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(doc.info.pages.len() as u32)
        })
    }

    /// Returns geometric dimensions and rotation for a single page.
    pub fn page_geometry(&self, doc_id: u64, page: u32) -> Result<FfiPageGeometry, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
            Ok(FfiPageGeometry {
                width: page_info.width,
                height: page_info.height,
                rotation: page_info.rotation as u32,
                label: page_info.label.clone(),
            })
        })
    }

    /// Extracts plain text from a page.
    pub fn page_text(&self, doc_id: u64, page: u32) -> Result<String, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            Ok(text.plain_text())
        })
    }

    /// Searches for query text on a page.
    pub fn find_text(&self, doc_id: u64, page: u32, query: String, case_sensitive: bool) -> Result<Vec<FfiFindMatch>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            Ok(find_in_page_text(page, &text, &query, case_sensitive))
        })
    }

    /// Returns the outline/bookmarks tree of the document.
    pub fn outline(&self, doc_id: u64) -> Result<Vec<FfiOutlineItem>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(map_outline(&doc.info.outline))
        })
    }

    /// Returns the interactive hyperlinks on a page.
    pub fn page_links(&self, doc_id: u64, page: u32) -> Result<Vec<FfiLink>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_idx = page as usize;
            let links = doc
                .info
                .links
                .iter()
                .filter(|l| l.page == page_idx)
                .map(|l| {
                    let (target_page, target_uri) = match &l.target {
                        pdfcraft_render::LinkTarget::Page(p, _) => (Some(*p as u32), None),
                        pdfcraft_render::LinkTarget::Uri(u) => (None, Some(u.clone())),
                        _ => (None, None),
                    };
                    FfiLink {
                        page,
                        rect: FfiRect { x: l.rect[0], y: l.rect[1], width: (l.rect[2] - l.rect[0]).abs(), height: (l.rect[3] - l.rect[1]).abs() },
                        target_page,
                        target_uri,
                    }
                })
                .collect();
            Ok(links)
        })
    }

    /// Advances the generation tag counter (e.g. on user scroll or pinch zoom) and returns the new tag.
    pub fn next_render_tag(&self) -> u64 {
        self.inner.lock().map(|i| i.coalescer.next_generation()).unwrap_or(1)
    }

    /// Returns the current active render generation tag.
    pub fn current_render_tag(&self) -> u64 {
        self.inner.lock().map(|i| i.coalescer.current_generation()).unwrap_or(1)
    }

    /// Computes a prioritized prefetch schedule for pages around the current viewport.
    pub fn plan_prefetch(&self, current_page: u32, total_pages: u32, direction: FfiScrollDirection, lookahead: u32) -> Vec<FfiPrefetchPlan> {
        plan_prefetch_pages(current_page, total_pages, direction, lookahead)
    }

    /// Synchronously renders a tile or full page (served from tile cache if available).
    pub fn render_tile(&self, doc_id: u64, request: FfiRenderRequest) -> Result<FfiRenderResult, FfiError> {
        catch_panic(|| {
            let (bytes, password, is_cancelled, cached_result, cache_key, is_first_requester) = {
                let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
                let bytes = inner.doc_bytes.get(&doc_id).cloned().ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let password = inner.doc_passwords.get(&doc_id).cloned().flatten();
                let cancelled = inner.cancellations.is_cancelled(request.tag);
                let version = inner.doc_versions.get(&doc_id).copied().unwrap_or(1);
                let key = TileCacheKey::from_request(doc_id, version, &request);
                let cached = if !cancelled { inner.tile_cache.get(&key) } else { None };
                let is_first = if cached.is_none() && !cancelled { inner.coalescer.try_acquire(&key) } else { false };
                (bytes, password, cancelled, cached, key, is_first)
            };

            if is_cancelled {
                return Ok(FfiRenderResult {
                    page: request.page,
                    width: 0,
                    height: 0,
                    tag: request.tag,
                    pixels: Vec::new(),
                    error: Some("request cancelled".into()),
                    render_time_ms: 0,
                });
            }

            if let Some(cached) = cached_result {
                return Ok(cached);
            }

            if !is_first_requester {
                // If another thread is actively rendering this key, check if completed
                if let Ok(mut inner) = self.inner.lock()
                    && let Some(cached) = inner.tile_cache.get(&cache_key)
                {
                    return Ok(cached);
                }
            }

            let result = render_tile_sync(bytes, password, &request);
            if let Ok(mut inner) = self.inner.lock() {
                inner.coalescer.release(&cache_key);
                if result.error.is_none() {
                    inner.tile_cache.insert(cache_key, result.clone());
                }
            }

            Ok(result)
        })
    }

    /// Cancels in-flight render requests tagged with values strictly older than `older_than_tag`.
    pub fn cancel_renders(&self, older_than_tag: u64) {
        if let Ok(inner) = self.inner.lock() {
            inner.cancellations.retire_older_than(older_than_tag);
        }
    }

    /// Extracts view-space selection rectangles for a glyph range on a page.
    pub fn text_selection_rects(&self, doc_id: u64, page: u32, start_glyph: u32, end_glyph: u32) -> Result<Vec<FfiRect>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            let start = (start_glyph.min(end_glyph)) as usize;
            let end = (start_glyph.max(end_glyph) as usize).saturating_add(1);
            let rects = text.line_rects(start..end);
            Ok(rects.into_iter().map(|r| FfiRect { x: r[0], y: r[1], width: (r[2] - r[0]).max(0.0), height: (r[3] - r[1]).max(0.0) }).collect())
        })
    }

    /// Returns plain text for a glyph range on a page.
    pub fn text_selection_string(&self, doc_id: u64, page: u32, start_glyph: u32, end_glyph: u32) -> Result<String, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            let start = (start_glyph.min(end_glyph)) as usize;
            let end = (start_glyph.max(end_glyph) as usize).saturating_add(1);
            Ok(text.text_of(start..end))
        })
    }

    /// Finds the glyph nearest to a point in view coordinates.
    pub fn nearest_glyph_at(&self, doc_id: u64, page: u32, x: f32, y: f32) -> Result<Option<u32>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            Ok(text.nearest(x, y).map(|i| i as u32))
        })
    }

    /// Adds a text markup annotation (highlight, underline, strikeout, squiggly) covering the specified glyph selection range.
    #[allow(clippy::too_many_arguments)]
    pub fn add_text_markup_selection(
        &self,
        doc_id: u64,
        page: u32,
        start_glyph: u32,
        end_glyph: u32,
        kind: FfiTextMarkupKind,
        color_rgb: Option<u32>,
        author: Option<String>,
        contents: Option<String>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let text = inner.extract_page_text(doc_id, page)?;
            let start = (start_glyph.min(end_glyph)) as usize;
            let end = (start_glyph.max(end_glyph) as usize).saturating_add(1);
            let rects = text.line_rects(start..end);
            if rects.is_empty() {
                return Ok(());
            }

            let quads: Vec<[f64; 8]> = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                rects.into_iter().map(|r| page_info.view_rect_to_quad(r)).collect()
            };

            let shape = pdfcraft_engine::Shape::TextMarkup { kind: kind.into(), quads };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = color_rgb {
                style.color = crate::edit::rgb_from_u32(rgb);
            }

            let new_annotation = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: contents.unwrap_or_default(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };

            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annotation))
        })
    }

    /// Adds a text markup annotation (highlight, underline, strikeout, squiggly) covering the given view-space rectangles.
    #[allow(clippy::too_many_arguments)]
    pub fn add_text_markup_rects(
        &self,
        doc_id: u64,
        page: u32,
        rects: Vec<FfiRect>,
        kind: FfiTextMarkupKind,
        color_rgb: Option<u32>,
        author: Option<String>,
        contents: Option<String>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            if rects.is_empty() {
                return Ok(());
            }
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let quads: Vec<[f64; 8]> = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                rects.into_iter().map(|r| page_info.view_rect_to_quad([r.x, r.y, r.x + r.width, r.y + r.height])).collect()
            };

            let shape = pdfcraft_engine::Shape::TextMarkup { kind: kind.into(), quads };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = color_rgb {
                style.color = crate::edit::rgb_from_u32(rgb);
            }

            let new_annotation = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: contents.unwrap_or_default(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };

            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annotation))
        })
    }

    /// Hit-tests for an interactive annotation at point (x, y) in view coordinates on a page.
    pub fn hit_test_annotation(&self, doc_id: u64, page: u32, x: f32, y: f32) -> Result<Option<FfiAnnotationSummary>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let hit = doc.info.annotations.iter().filter(|a| a.page == (page as usize) && a.in_reply_to.is_none()).rev().find(|a| {
                if !a.quads.is_empty() {
                    for q in &a.quads {
                        let min_ux = q[0].min(q[2]).min(q[4]).min(q[6]);
                        let max_ux = q[0].max(q[2]).max(q[4]).max(q[6]);
                        let min_uy = q[1].min(q[3]).min(q[5]).min(q[7]);
                        let max_uy = q[1].max(q[3]).max(q[5]).max(q[7]);
                        let vr = user_box_to_view_rect(page_info, [min_ux, min_uy, max_ux, max_uy]);
                        if x >= vr.x - 4.0 && x <= vr.x + vr.width + 4.0 && y >= vr.y - 4.0 && y <= vr.y + vr.height + 4.0 {
                            return true;
                        }
                    }
                }
                let vr = user_box_to_view_rect(page_info, a.rect);
                x >= vr.x - 4.0 && x <= vr.x + vr.width + 4.0 && y >= vr.y - 4.0 && y <= vr.y + vr.height + 4.0
            });

            Ok(hit.map(|a| {
                let vr = user_box_to_view_rect(page_info, a.rect);
                let color_rgb = a.color.map(|c| {
                    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
                    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
                    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
                    (r << 16) | (g << 8) | b
                });
                FfiAnnotationSummary {
                    page,
                    index: a.index as u32,
                    subtype: a.subtype.clone(),
                    author: a.author.clone(),
                    contents: a.contents.clone(),
                    rect: vr,
                    color_rgb,
                    locked: a.locked,
                }
            }))
        })
    }

    /// Moves an annotation on a page by delta view points.
    pub fn move_annotation(&self, doc_id: u64, page: u32, index: u32, dx_view: f32, dy_view: f32) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let (dx_user, dy_user) = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                let [u0, v0] = page_info.view_to_user(0.0, 0.0);
                let [u1, v1] = page_info.view_to_user(dx_view, dy_view);
                (f64::from(u1 - u0), f64::from(v1 - v0))
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::MoveAnnotation { page: page as usize, index: index as usize, dx: dx_user, dy: dy_user })
        })
    }

    /// Resizes a rectangle, oval or text box annotation to a new bounding box in view coordinates.
    pub fn resize_annotation(&self, doc_id: u64, page: u32, index: u32, new_rect_view: FfiRect) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let new_user_rect = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                view_rect_to_user_box(page_info, &new_rect_view)
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::ResizeAnnotation { page: page as usize, index: index as usize, rect: new_user_rect })
        })
    }

    /// Changes an annotation's style (stroke color, opacity, stroke width, and fill color).
    #[allow(clippy::too_many_arguments)]
    pub fn style_annotation(
        &self,
        doc_id: u64,
        page: u32,
        index: u32,
        stroke_color_rgb: Option<u32>,
        opacity: Option<f32>,
        stroke_width: Option<f32>,
        fill_color_rgb: Option<u32>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let color = stroke_color_rgb.map(rgb_from_u32);
            let opacity = opacity.map(|o| o as f64);
            let width = stroke_width.map(|w| w as f64);
            inner.apply_edit(
                doc_id,
                pdfcraft_engine::Edit::StyleAnnotation { page: page as usize, index: index as usize, color, opacity, width, endings: None },
            )?;
            if fill_color_rgb.is_some() {
                let fill = fill_color_rgb.map(rgb_from_u32);
                inner.apply_edit(doc_id, pdfcraft_engine::Edit::FillAnnotation { page: page as usize, index: index as usize, fill })?;
            }
            Ok(())
        })
    }

    /// Deletes an annotation from a page by its index.
    pub fn delete_annotation(&self, doc_id: u64, page: u32, index: u32) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::DeleteAnnotation { page: page as usize, index: index as usize })
        })
    }

    /// Adds a sticky note annotation at a point in view coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn add_note_annotation(
        &self,
        doc_id: u64,
        page: u32,
        at: FfiPoint,
        contents: String,
        author: Option<String>,
        icon: Option<String>,
        color_rgb: Option<u32>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let (ux, uy) = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                let [ux, uy] = page_info.view_to_user(at.x, at.y);
                (f64::from(ux), f64::from(uy))
            };
            let note_icon = icon.as_deref().and_then(pdfcraft_engine::NoteIcon::from_name).unwrap_or_default();
            let shape = pdfcraft_engine::Shape::Note { at: [ux, uy], icon: note_icon };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = color_rgb {
                style.color = rgb_from_u32(rgb);
            }
            let new_annot =
                pdfcraft_engine::NewAnnotation { page: page as usize, shape, style, contents, author: author.unwrap_or_else(|| "PdfCraft".into()) };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a free text / text box annotation in view coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn add_text_box_annotation(
        &self,
        doc_id: u64,
        page: u32,
        rect: FfiRect,
        contents: String,
        font_size: f32,
        author: Option<String>,
        color_rgb: Option<u32>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let user_rect = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                view_rect_to_user_box(page_info, &rect)
            };
            let shape = pdfcraft_engine::Shape::TextBox { rect: user_rect, font_size: f64::from(font_size) };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = color_rgb {
                style.color = rgb_from_u32(rgb);
            }
            let new_annot =
                pdfcraft_engine::NewAnnotation { page: page as usize, shape, style, contents, author: author.unwrap_or_else(|| "PdfCraft".into()) };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a vector shape annotation (rectangle, oval, line, arrow) in view coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn add_shape_annotation(
        &self,
        doc_id: u64,
        page: u32,
        kind: FfiShapeKind,
        rect: FfiRect,
        stroke_color: Option<u32>,
        fill_color: Option<u32>,
        stroke_width: Option<f32>,
        author: Option<String>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let user_rect = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                view_rect_to_user_box(page_info, &rect)
            };
            let shape = match kind {
                FfiShapeKind::Rectangle => pdfcraft_engine::Shape::Rectangle { rect: user_rect },
                FfiShapeKind::Oval => pdfcraft_engine::Shape::Oval { rect: user_rect },
                FfiShapeKind::Line => pdfcraft_engine::Shape::Line {
                    from: [user_rect[0], user_rect[1]],
                    to: [user_rect[2], user_rect[3]],
                    start: pdfcraft_engine::LineEnding::None,
                    end: pdfcraft_engine::LineEnding::None,
                },
                FfiShapeKind::Arrow => pdfcraft_engine::Shape::Line {
                    from: [user_rect[0], user_rect[1]],
                    to: [user_rect[2], user_rect[3]],
                    start: pdfcraft_engine::LineEnding::None,
                    end: pdfcraft_engine::LineEnding::OpenArrow,
                },
            };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = stroke_color {
                style.color = rgb_from_u32(rgb);
            }
            if let Some(w) = stroke_width {
                style.width = f64::from(w);
            }
            if let Some(rgb) = fill_color {
                style.fill = Some(rgb_from_u32(rgb));
            }
            let new_annot = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: String::new(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a rubber stamp annotation in view coordinates.
    pub fn add_stamp_annotation(&self, doc_id: u64, page: u32, rect: FfiRect, stamp_name: String, author: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let user_rect = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                view_rect_to_user_box(page_info, &rect)
            };
            let stamp = pdfcraft_engine::StampKind::ALL
                .into_iter()
                .find(|k| k.name().eq_ignore_ascii_case(&stamp_name) || k.label().eq_ignore_ascii_case(&stamp_name))
                .unwrap_or(pdfcraft_engine::StampKind::Approved);
            let shape = pdfcraft_engine::Shape::Stamp { rect: user_rect, stamp, by: author.clone() };
            let style = pdfcraft_engine::Style::default_for(&shape);
            let new_annot = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: stamp.label().to_string(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a multi-stroke ink drawing annotation in view coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn add_ink_annotation(
        &self,
        doc_id: u64,
        page: u32,
        strokes: Vec<Vec<FfiPoint>>,
        color_rgb: Option<u32>,
        stroke_width: Option<f32>,
        opacity: Option<f32>,
        author: Option<String>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            if strokes.is_empty() {
                return Ok(());
            }
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let user_strokes: Vec<Vec<[f64; 2]>> = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                strokes
                    .into_iter()
                    .filter(|s| s.len() >= 2)
                    .map(|s| {
                        s.into_iter()
                            .map(|pt| {
                                let [ux, uy] = page_info.view_to_user(pt.x, pt.y);
                                [f64::from(ux), f64::from(uy)]
                            })
                            .collect()
                    })
                    .collect()
            };

            if user_strokes.is_empty() {
                return Ok(());
            }

            let shape = pdfcraft_engine::Shape::Ink { strokes: user_strokes };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            if let Some(rgb) = color_rgb {
                style.color = rgb_from_u32(rgb);
            }
            if let Some(w) = stroke_width {
                style.width = f64::from(w);
            }
            if let Some(o) = opacity {
                style.opacity = f64::from(o);
            }

            let new_annot = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: String::new(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };

            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a single pen-up ink stroke as an annotation in view coordinates.
    pub fn add_ink_stroke(&self, doc_id: u64, stroke: FfiInkStroke) -> Result<(), FfiError> {
        self.add_ink_annotation(doc_id, stroke.page, vec![stroke.points], Some(stroke.color_rgb), Some(stroke.width), None, None)
    }

    /// Erases ink strokes intersecting a trajectory path on a page. Returns true if ink was erased.
    pub fn erase_ink_at(&self, doc_id: u64, page: u32, path: Vec<FfiPoint>, radius_view: f32) -> Result<bool, FfiError> {
        catch_panic(|| {
            if path.is_empty() || radius_view <= 0.0 {
                return Ok(false);
            }
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let (user_path, user_radius, mut hits) = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;
                let user_path: Vec<[f64; 2]> = path
                    .into_iter()
                    .map(|pt| {
                        let [ux, uy] = page_info.view_to_user(pt.x, pt.y);
                        [f64::from(ux), f64::from(uy)]
                    })
                    .collect();
                let [u0, v0] = page_info.view_to_user(0.0, 0.0);
                let [u1, v1] = page_info.view_to_user(radius_view, 0.0);
                let user_radius = f64::from((u1 - u0).hypot(v1 - v0));

                let mut min_x = f64::INFINITY;
                let mut min_y = f64::INFINITY;
                let mut max_x = f64::NEG_INFINITY;
                let mut max_y = f64::NEG_INFINITY;
                for p in &user_path {
                    min_x = min_x.min(p[0]);
                    min_y = min_y.min(p[1]);
                    max_x = max_x.max(p[0]);
                    max_y = max_y.max(p[1]);
                }

                let hits: Vec<usize> = doc
                    .info
                    .annotations
                    .iter()
                    .filter(|a| a.page == (page as usize) && a.subtype == "Ink" && !a.locked)
                    .filter(|a| {
                        let r = a.rect;
                        f64::from(r[0]) <= max_x + user_radius
                            && f64::from(r[2]) >= min_x - user_radius
                            && f64::from(r[1]) <= max_y + user_radius
                            && f64::from(r[3]) >= min_y - user_radius
                    })
                    .map(|a| a.index)
                    .collect();

                (user_path, user_radius, hits)
            };

            if hits.is_empty() {
                return Ok(false);
            }

            // Descending order so deletion index shifts don't disrupt earlier ones
            hits.sort_unstable_by(|a, b| b.cmp(a));
            for index in hits {
                inner.apply_edit(
                    doc_id,
                    pdfcraft_engine::Edit::EraseInk { page: page as usize, index, path: user_path.clone(), radius: user_radius },
                )?;
            }

            Ok(true)
        })
    }

    /// Returns all interactive comments on a page, including replies and status items.
    pub fn page_comments(&self, doc_id: u64, page: u32) -> Result<Vec<FfiCommentItem>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let comments = doc
                .info
                .annotations
                .iter()
                .filter(|a| a.page == (page as usize))
                .map(|a| {
                    let rect = user_box_to_view_rect(page_info, a.rect);
                    let color_rgb = a.color.map(|c| {
                        let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
                        let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
                        let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
                        (r << 16) | (g << 8) | b
                    });
                    let review_state = a.state.as_deref().and_then(|s| match s {
                        "Accepted" => Some(FfiReviewState::Accepted),
                        "Rejected" => Some(FfiReviewState::Rejected),
                        "Cancelled" => Some(FfiReviewState::Cancelled),
                        "Completed" => Some(FfiReviewState::Completed),
                        "None" => Some(FfiReviewState::None),
                        _ => None,
                    });
                    let is_marked = matches!(a.state.as_deref(), Some("Marked"));
                    FfiCommentItem {
                        page,
                        index: a.index as u32,
                        subtype: a.subtype.clone(),
                        author: a.author.clone(),
                        contents: a.contents.clone(),
                        modified: a.modified.clone(),
                        name: a.name.clone(),
                        in_reply_to: a.in_reply_to.clone(),
                        rect,
                        color_rgb,
                        review_state,
                        is_marked,
                        locked: a.locked,
                    }
                })
                .collect();

            Ok(comments)
        })
    }

    /// Returns all interactive comments across all pages in the document.
    pub fn all_comments(&self, doc_id: u64) -> Result<Vec<FfiCommentItem>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;

            let comments = doc
                .info
                .annotations
                .iter()
                .map(|a| {
                    let rect = if let Some(page_info) = doc.info.pages.get(a.page) {
                        user_box_to_view_rect(page_info, a.rect)
                    } else {
                        FfiRect { x: a.rect[0], y: a.rect[1], width: (a.rect[2] - a.rect[0]).abs(), height: (a.rect[3] - a.rect[1]).abs() }
                    };
                    let color_rgb = a.color.map(|c| {
                        let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
                        let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
                        let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
                        (r << 16) | (g << 8) | b
                    });
                    let review_state = a.state.as_deref().and_then(|s| match s {
                        "Accepted" => Some(FfiReviewState::Accepted),
                        "Rejected" => Some(FfiReviewState::Rejected),
                        "Cancelled" => Some(FfiReviewState::Cancelled),
                        "Completed" => Some(FfiReviewState::Completed),
                        "None" => Some(FfiReviewState::None),
                        _ => None,
                    });
                    let is_marked = matches!(a.state.as_deref(), Some("Marked"));
                    FfiCommentItem {
                        page: a.page as u32,
                        index: a.index as u32,
                        subtype: a.subtype.clone(),
                        author: a.author.clone(),
                        contents: a.contents.clone(),
                        modified: a.modified.clone(),
                        name: a.name.clone(),
                        in_reply_to: a.in_reply_to.clone(),
                        rect,
                        color_rgb,
                        review_state,
                        is_marked,
                        locked: a.locked,
                    }
                })
                .collect();

            Ok(comments)
        })
    }

    /// Adds a threaded reply to a comment annotation.
    pub fn reply_to_comment(&self, doc_id: u64, page: u32, index: u32, text: String, author: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::ReplyToAnnotation { page: page as usize, index: index as usize, text, author })
        })
    }

    /// Sets the review state (e.g. Accepted, Rejected, Completed) on an annotation thread.
    pub fn set_comment_review_state(&self, doc_id: u64, page: u32, index: u32, state: FfiReviewState, author: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(
                doc_id,
                pdfcraft_engine::Edit::SetAnnotationStatus { page: page as usize, index: index as usize, state: state.into(), author },
            )
        })
    }

    /// Updates the text contents of a comment annotation.
    pub fn set_comment_contents(&self, doc_id: u64, page: u32, index: u32, text: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetAnnotationContents { page: page as usize, index: index as usize, text })
        })
    }

    /// Toggles or sets Acrobat's "Mark with checkmark" review flag on an annotation.
    pub fn set_comment_marked(&self, doc_id: u64, page: u32, index: u32, marked: bool, author: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::MarkAnnotation { page: page as usize, index: index as usize, marked, author })
        })
    }

    /// Returns the list of interactive AcroForm fields in the document, optionally filtered to a specific page.
    pub fn form_fields(&self, doc_id: u64, page: Option<u32>) -> Result<Vec<FfiFormField>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let mut result = Vec::new();

            for f in doc.form.iter() {
                let required = f.has(pdfcraft_engine::field_flags::REQUIRED);
                let multiline = f.has(pdfcraft_engine::field_flags::MULTILINE);
                let password = f.has(pdfcraft_engine::field_flags::PASSWORD);
                let options: Vec<FfiChoiceOption> =
                    f.options.iter().map(|(exp, disp)| FfiChoiceOption { export_value: exp.clone(), display_text: disp.clone() }).collect();

                for (wi, w) in f.widgets.iter().enumerate() {
                    if w.hidden {
                        continue;
                    }
                    if page.is_some_and(|p| w.page != Some(p as usize)) {
                        continue;
                    }

                    let (rect, widget_page) = match w.page {
                        Some(p) => {
                            let rect = if let Some(page_info) = doc.info.pages.get(p) {
                                user_box_to_view_rect(page_info, [w.rect[0] as f32, w.rect[1] as f32, w.rect[2] as f32, w.rect[3] as f32])
                            } else {
                                FfiRect {
                                    x: w.rect[0] as f32,
                                    y: w.rect[1] as f32,
                                    width: (w.rect[2] - w.rect[0]).abs() as f32,
                                    height: (w.rect[3] - w.rect[1]).abs() as f32,
                                }
                            };
                            (rect, Some(p as u32))
                        }
                        None => (
                            FfiRect {
                                x: w.rect[0] as f32,
                                y: w.rect[1] as f32,
                                width: (w.rect[2] - w.rect[0]).abs() as f32,
                                height: (w.rect[3] - w.rect[1]).abs() as f32,
                            },
                            None,
                        ),
                    };

                    result.push(FfiFormField {
                        name: f.name.clone(),
                        page: widget_page,
                        widget_index: wi as u32,
                        kind: f.kind.into(),
                        value: f.value.clone(),
                        default_value: f.default.clone(),
                        options: options.clone(),
                        rect,
                        read_only: f.read_only() || w.locked,
                        required,
                        multiline,
                        password,
                        max_len: f.max_len.map(|m| m as u32),
                        tooltip: f.tooltip.clone(),
                        on_state: w.on_state.clone(),
                    });
                }
            }

            Ok(result)
        })
    }

    /// Sets the text value of a text form field.
    pub fn set_form_field_text(&self, doc_id: u64, name: String, text: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Text(text) })
        })
    }

    /// Sets the checked state of a check box form field.
    pub fn set_form_field_check(&self, doc_id: u64, name: String, checked: bool) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Check(checked) })
        })
    }

    /// Toggles a check box form field, returning the new checked state.
    pub fn toggle_form_field_check(&self, doc_id: u64, name: String) -> Result<bool, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let f = doc.form.iter().find(|f| f.name == name).ok_or_else(|| FfiError::NotFound { id: name.clone() })?;
            let new_checked = f.value.is_empty();
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Check(new_checked) })?;
            Ok(new_checked)
        })
    }

    /// Selects an on-state for a radio button group (or None to deselect).
    pub fn set_form_field_radio(&self, doc_id: u64, name: String, selected_option: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Radio(selected_option) })
        })
    }

    /// Sets the selected export value(s) of a combo box or list box form field.
    pub fn set_form_field_choice(&self, doc_id: u64, name: String, picked: Vec<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Choice(picked) })
        })
    }

    /// General setter for form fields by string representation according to field type.
    pub fn set_form_field_value(&self, doc_id: u64, name: String, value: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let field_val = {
                let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let f = doc.form.iter().find(|f| f.name == name).ok_or_else(|| FfiError::NotFound { id: name.clone() })?;
                match f.kind {
                    pdfcraft_engine::FormFieldKind::Text => pdfcraft_engine::FieldValue::Text(value),
                    pdfcraft_engine::FormFieldKind::CheckBox => {
                        let checked = matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on");
                        pdfcraft_engine::FieldValue::Check(checked)
                    }
                    pdfcraft_engine::FormFieldKind::Radio => {
                        let opt = if value.is_empty() { None } else { Some(value) };
                        pdfcraft_engine::FieldValue::Radio(opt)
                    }
                    pdfcraft_engine::FormFieldKind::Combo | pdfcraft_engine::FormFieldKind::List => pdfcraft_engine::FieldValue::Choice(vec![value]),
                    pdfcraft_engine::FormFieldKind::PushButton | pdfcraft_engine::FormFieldKind::Signature => {
                        return Err(FfiError::EditFailed { reason: "cannot set text value on button or signature field".into() });
                    }
                }
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::SetFieldValue { name, value: field_val })
        })
    }

    /// Resets named form fields back to their default values (or all fields if names is None).
    pub fn reset_form(&self, doc_id: u64, names: Option<Vec<String>>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::ResetForm { names })
        })
    }

    /// Adds a freehand drawn visual signature annotation on a page.
    pub fn add_drawn_signature(&self, doc_id: u64, page: u32, strokes: Vec<Vec<FfiPoint>>, author: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let user_strokes: Vec<Vec<[f64; 2]>> = strokes
                .into_iter()
                .map(|stroke| {
                    stroke
                        .into_iter()
                        .map(|pt| {
                            let [ux, uy] = page_info.view_to_user(pt.x, pt.y);
                            [f64::from(ux), f64::from(uy)]
                        })
                        .collect()
                })
                .collect();

            let shape = pdfcraft_engine::Shape::Signature { strokes: user_strokes };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            style.color = [0.0, 0.0, 0.0];
            let new_annot = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: "Signature".into(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Adds a typed visual script signature annotation in the given bounding box on a page.
    pub fn add_typed_signature(&self, doc_id: u64, page: u32, text: String, rect: FfiRect, author: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            if text.trim().is_empty() {
                return Err(FfiError::EditFailed { reason: "signature text cannot be empty".into() });
            }
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page as usize).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let user_rect = view_rect_to_user_box(page_info, &rect);
            let at = [user_rect[0], (user_rect[1] + user_rect[3]) / 2.0];
            let height = (user_rect[3] - user_rect[1]).abs();
            let rotation = page_info.rotation as i64;
            let shape = pdfcraft_engine::typed_signature_shape(at, &text, height, rotation)
                .unwrap_or_else(|| pdfcraft_engine::Shape::Typewriter { rect: user_rect, font_size: (height * 0.7).max(6.0) });

            let mut style = pdfcraft_engine::Style::default_for(&shape);
            style.color = [0.0, 0.0, 0.0];
            let new_annot = pdfcraft_engine::NewAnnotation {
                page: page as usize,
                shape,
                style,
                contents: text,
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Signs a signature form field using freehand drawn normalized ink strokes (0.0..=1.0).
    pub fn sign_form_field_drawn(
        &self,
        doc_id: u64,
        field_name: String,
        normalized_strokes: Vec<Vec<FfiPoint>>,
        author: Option<String>,
    ) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let f = doc.form.iter().find(|f| f.name == field_name).ok_or_else(|| FfiError::NotFound { id: field_name.clone() })?;
            let w = f.widgets.iter().find(|w| w.page.is_some() && !w.hidden).ok_or_else(|| FfiError::NotFound { id: field_name.clone() })?;
            let page = w.page.unwrap_or(0);

            let min_x = w.rect[0].min(w.rect[2]);
            let max_x = w.rect[0].max(w.rect[2]);
            let min_y = w.rect[1].min(w.rect[3]);
            let max_y = w.rect[1].max(w.rect[3]);
            let span_x = (max_x - min_x).max(1.0);
            let span_y = (max_y - min_y).max(1.0);
            let pad_x = span_x * 0.05;
            let pad_y = span_y * 0.05;
            let draw_min_x = min_x + pad_x;
            let draw_max_x = max_x - pad_x;
            let draw_min_y = min_y + pad_y;
            let draw_max_y = max_y - pad_y;

            let user_strokes: Vec<Vec<[f64; 2]>> = normalized_strokes
                .into_iter()
                .map(|stroke| {
                    stroke
                        .into_iter()
                        .map(|pt| {
                            let nx = f64::from(pt.x).clamp(0.0, 1.0);
                            let ny = f64::from(pt.y).clamp(0.0, 1.0);
                            let ux = draw_min_x + nx * (draw_max_x - draw_min_x);
                            let uy = draw_max_y - ny * (draw_max_y - draw_min_y);
                            [ux, uy]
                        })
                        .collect()
                })
                .collect();

            let shape = pdfcraft_engine::Shape::Signature { strokes: user_strokes };
            let mut style = pdfcraft_engine::Style::default_for(&shape);
            style.color = [0.0, 0.0, 0.0];
            let new_annot = pdfcraft_engine::NewAnnotation {
                page,
                shape,
                style,
                contents: "Signature".into(),
                author: author.unwrap_or_else(|| "PdfCraft".into()),
            };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Signs a signature form field by rendering a typed name in the script typeface.
    pub fn sign_form_field_typed(&self, doc_id: u64, field_name: String, text: String, author: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            if text.trim().is_empty() {
                return Err(FfiError::EditFailed { reason: "signature text cannot be empty".into() });
            }
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let f = doc.form.iter().find(|f| f.name == field_name).ok_or_else(|| FfiError::NotFound { id: field_name.clone() })?;
            let w = f.widgets.iter().find(|w| w.page.is_some() && !w.hidden).ok_or_else(|| FfiError::NotFound { id: field_name.clone() })?;
            let page = w.page.unwrap_or(0);
            let page_info = doc.info.pages.get(page).ok_or(FfiError::PageOutOfBounds { page: page as u32, total: doc.info.pages.len() as u32 })?;

            let min_x = w.rect[0].min(w.rect[2]);
            let max_x = w.rect[0].max(w.rect[2]);
            let min_y = w.rect[1].min(w.rect[3]);
            let max_y = w.rect[1].max(w.rect[3]);
            let height = (max_y - min_y).abs() * 0.75;
            let at = [min_x + (max_x - min_x) * 0.05, (min_y + max_y) / 2.0];
            let rotation = page_info.rotation as i64;

            let shape = pdfcraft_engine::typed_signature_shape(at, &text, height, rotation)
                .unwrap_or_else(|| pdfcraft_engine::Shape::Typewriter { rect: [min_x, min_y, max_x, max_y], font_size: (height * 0.7).max(6.0) });

            let mut style = pdfcraft_engine::Style::default_for(&shape);
            style.color = [0.0, 0.0, 0.0];
            let new_annot =
                pdfcraft_engine::NewAnnotation { page, shape, style, contents: text, author: author.unwrap_or_else(|| "PdfCraft".into()) };
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::AddAnnotation(new_annot))
        })
    }

    /// Returns the descriptive label of the next edit that can be undone, or None if undo stack is empty.
    pub fn can_undo(&self, doc_id: u64) -> Result<Option<String>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(doc.can_undo().map(|s| s.to_string()))
        })
    }

    /// Returns the descriptive label of the next edit that can be redone, or None if redo stack is empty.
    pub fn can_redo(&self, doc_id: u64) -> Result<Option<String>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(doc.can_redo().map(|s| s.to_string()))
        })
    }

    /// Undoes the last edit operation on the document, returning the label of the undone action.
    pub fn undo(&self, doc_id: u64) -> Result<String, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let label = inner.session.undo(DocId(doc_id)).map_err(FfiError::from)?;
            let doc_bytes = inner.session.get(DocId(doc_id)).map(|d| d.bytes.clone());
            if let Some(bytes) = doc_bytes {
                inner.doc_bytes.insert(doc_id, bytes);
                let ver = inner.doc_versions.entry(doc_id).or_insert(1);
                *ver += 1;
                inner.tile_cache.evict_for_doc(doc_id);
            }
            Ok(label.to_string())
        })
    }

    /// Redoes the last undone edit operation on the document, returning the label of the redone action.
    pub fn redo(&self, doc_id: u64) -> Result<String, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let label = inner.session.redo(DocId(doc_id)).map_err(FfiError::from)?;
            let doc_bytes = inner.session.get(DocId(doc_id)).map(|d| d.bytes.clone());
            if let Some(bytes) = doc_bytes {
                inner.doc_bytes.insert(doc_id, bytes);
                let ver = inner.doc_versions.entry(doc_id).or_insert(1);
                *ver += 1;
                inner.tile_cache.evict_for_doc(doc_id);
            }
            Ok(label.to_string())
        })
    }

    /// Returns whether the document has modifications that have not yet been saved to disk.
    pub fn is_dirty(&self, doc_id: u64) -> Result<bool, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            Ok(doc.dirty)
        })
    }

    /// Returns the full updated PDF bytes (incremental update with /ModDate stamped) suitable for saving.
    pub fn save_bytes(&self, doc_id: u64) -> Result<Vec<u8>, FfiError> {
        catch_panic(|| {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let bytes = inner.session.save_bytes(DocId(doc_id)).map_err(FfiError::from)?;
            Ok((*bytes).clone())
        })
    }

    /// Marks the document as cleanly saved after writing its bytes to disk.
    pub fn mark_saved(&self, doc_id: u64, path: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let bytes = inner.session.save_bytes(DocId(doc_id)).map_err(FfiError::from)?;
            inner.session.mark_saved(DocId(doc_id), bytes.clone(), path).map_err(FfiError::from)?;
            inner.doc_bytes.insert(doc_id, bytes);
            Ok(())
        })
    }

    /// Atomically writes the incremental PDF update to target_path and marks the document saved.
    ///
    /// Writes to a temporary sibling file and renames it to ensure the original file is never corrupted
    /// if the write process is interrupted or fails.
    pub fn save_to_path(&self, doc_id: u64, target_path: String) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let path = Path::new(&target_path);

            // Enforce sandbox confinement if root is configured
            if let Some(ref root) = inner.root {
                let parent = path.parent().unwrap_or(path);
                let real_parent = if parent.exists() {
                    parent.canonicalize().map_err(|e| FfiError::Io { reason: format!("invalid target path: {e}") })?
                } else {
                    root.clone()
                };
                if !real_parent.starts_with(root) {
                    return Err(FfiError::Io { reason: format!("{target_path} is outside sandbox root {}", root.display()) });
                }
            }

            let bytes = inner.session.save_bytes(DocId(doc_id)).map_err(FfiError::from)?;

            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("document.pdf");
            let temp_name = format!(
                ".{file_name}.pdfcraft_tmp.{}",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
            );
            let temp_path = parent.join(temp_name);

            let write_res = (|| -> Result<(), std::io::Error> {
                use std::io::Write;
                let mut file = std::fs::File::create(&temp_path)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                std::fs::rename(&temp_path, path)?;
                Ok(())
            })();

            if let Err(e) = write_res {
                let _ = std::fs::remove_file(&temp_path);
                return Err(FfiError::Io { reason: format!("failed to atomically save to {target_path}: {e}") });
            }

            inner.session.mark_saved(DocId(doc_id), bytes.clone(), Some(target_path)).map_err(FfiError::from)?;
            inner.doc_bytes.insert(doc_id, bytes);
            Ok(())
        })
    }

    /// Captures crash-recovery / autosave snapshots for any open documents with uncommitted modifications.
    pub fn autosave_snapshots(&self) -> Result<Vec<FfiRecoverySnapshot>, FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let snapshots = inner.session.autosave_snapshots();
            let result = snapshots
                .into_iter()
                .map(|s| FfiRecoverySnapshot { doc_id: s.doc.0, name: s.name, path: s.path, bytes: (*s.bytes).clone(), encrypted: s.encrypted })
                .collect();
            Ok(result)
        })
    }

    /// Marks a document recovered from an autosave file so Save knows where it belongs.
    pub fn mark_recovered(&self, doc_id: u64, path: Option<String>) -> Result<(), FfiError> {
        catch_panic(|| {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.session.mark_recovered(DocId(doc_id), path);
            Ok(())
        })
    }

    /// Invokes an automation tool within this engine instance, updating open documents and tile caches.
    pub fn invoke_tool(&self, name: String, args_json: String) -> Result<crate::tools::FfiToolResult, FfiError> {
        catch_panic(|| {
            let (classification, reason) = crate::tools::classify_tool(&name);
            if classification == crate::tools::FfiToolClassification::NotOnMobile
                && let Some(r) = reason
                && r.contains("feature not compiled into pdfcraft-ffi")
            {
                return Err(FfiError::InvalidArgs { reason: format!("Cannot invoke '{name}': {r}") });
            }

            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let args: serde_json::Value =
                serde_json::from_str(&args_json).map_err(|e| FfiError::InvalidArgs { reason: format!("invalid JSON args: {e}") })?;

            let session = std::mem::take(&mut inner.session);
            let mut auto = pdfcraft_automation::Automation::from_session(session, inner.root.clone());
            let result = auto.call(&name, &args);
            inner.session = auto.into_session();

            // Synchronize doc_bytes, doc_versions, and tile caches for all docs
            let docs_info: Vec<(u64, std::sync::Arc<Vec<u8>>)> = inner.session.docs().iter().map(|d| (d.id.0, d.bytes.clone())).collect();
            let active_doc_ids: std::collections::HashSet<u64> = docs_info.iter().map(|(id, _)| *id).collect();
            inner.doc_bytes.retain(|id, _| active_doc_ids.contains(id));
            inner.doc_versions.retain(|id, _| active_doc_ids.contains(id));

            for (id, bytes) in docs_info {
                let changed = match inner.doc_bytes.get(&id) {
                    Some(cached) => !std::sync::Arc::ptr_eq(cached, &bytes) && **cached != *bytes,
                    None => true,
                };
                if changed {
                    inner.doc_bytes.insert(id, bytes);
                    let ver = inner.doc_versions.entry(id).or_insert(1);
                    *ver += 1;
                    inner.tile_cache.evict_for_doc(id);
                }
            }

            match result {
                Ok(contents) => {
                    let mut out_values = Vec::new();
                    let mut output_files = Vec::new();
                    for (idx, c) in contents.into_iter().enumerate() {
                        match c {
                            pdfcraft_automation::Content::Json(val) => out_values.push(val),
                            pdfcraft_automation::Content::Png { width, height, data } => {
                                let file_name = format!("output_{}.png", idx + 1);
                                output_files.push(crate::tools::FfiToolOutputFile {
                                    name: file_name.clone(),
                                    mime_type: "image/png".into(),
                                    bytes: data,
                                });
                                out_values.push(serde_json::json!({
                                    "type": "png",
                                    "name": file_name,
                                    "width": width,
                                    "height": height,
                                }));
                            }
                        }
                    }
                    let output_json = serde_json::to_string(&out_values).unwrap_or_else(|_| "[]".into());
                    Ok(crate::tools::FfiToolResult { success: true, output_json, output_files, error: None })
                }
                Err(e) => Ok(crate::tools::FfiToolResult {
                    success: false,
                    output_json: "null".into(),
                    output_files: Vec::new(),
                    error: Some(e.to_string()),
                }),
            }
        })
    }

    // MARK: - Redaction & Security Audit (MOB-6.2)

    /// Marks a rectangle on a page for permanent redaction.
    /// Does not remove content until `apply_redactions` is called.
    pub fn mark_redaction_rect(&self, doc_id: u64, page: u32, rect: FfiRect, overlay_text: Option<String>) -> Result<(), FfiError> {
        let mut args = serde_json::json!({
            "doc": doc_id,
            "page": page + 1,
            "rect": [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height],
        });
        if let Some(text) = overlay_text {
            args["overlay"] = serde_json::Value::String(text);
        }
        let res = self.invoke_tool("redact_mark".into(), args.to_string())?;
        if !res.success {
            return Err(FfiError::InvalidArgs { reason: res.error.unwrap_or_else(|| "redact_mark failed".into()) });
        }
        Ok(())
    }

    /// Marks all occurrences of a search text string for redaction.
    pub fn mark_redaction_text(&self, doc_id: u64, text: String, overlay_text: Option<String>) -> Result<u32, FfiError> {
        let mut args = serde_json::json!({
            "doc": doc_id,
            "find": text,
        });
        if let Some(overlay) = overlay_text {
            args["overlay"] = serde_json::Value::String(overlay);
        }
        let res = self.invoke_tool("redact_mark".into(), args.to_string())?;
        if !res.success {
            return Err(FfiError::InvalidArgs { reason: res.error.unwrap_or_else(|| "redact_mark failed".into()) });
        }
        let parsed: serde_json::Value = serde_json::from_str(&res.output_json).unwrap_or_default();
        let marked = parsed.get(0).and_then(|v| v.get("marked")).and_then(|v| v.as_u64()).unwrap_or(0);
        Ok(marked as u32)
    }

    /// Clears all pending redaction marks without applying them.
    pub fn clear_redactions(&self, doc_id: u64) -> Result<(), FfiError> {
        let args = serde_json::json!({ "doc": doc_id }).to_string();
        let res = self.invoke_tool("redact_clear".into(), args)?;
        if !res.success {
            return Err(FfiError::InvalidArgs { reason: res.error.unwrap_or_else(|| "redact_clear failed".into()) });
        }
        Ok(())
    }

    /// Runs a security audit for faux redactions (covered text) or unapplied redaction marks.
    pub fn audit_redactions(&self, doc_id: u64) -> Result<Vec<crate::edit::FfiRedactionFinding>, FfiError> {
        let args = serde_json::json!({ "doc": doc_id }).to_string();
        let res = self.invoke_tool("doc_audit_redactions".into(), args)?;
        if !res.success {
            return Err(FfiError::InvalidArgs { reason: res.error.unwrap_or_else(|| "audit failed".into()) });
        }
        let parsed: serde_json::Value = serde_json::from_str(&res.output_json).unwrap_or_default();
        let mut findings = Vec::new();
        if let Some(list) = parsed.as_array().and_then(|a| a.first()).and_then(|f| f.get("findings")).and_then(|v| v.as_array()) {
            for item in list {
                let page = item.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let covered_text = item.get("covered_text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let rect = if let Some(r) = item.get("rect").and_then(|v| v.as_array()) {
                    if r.len() == 4 {
                        let x0 = r[0].as_f64().unwrap_or(0.0) as f32;
                        let y0 = r[1].as_f64().unwrap_or(0.0) as f32;
                        let x1 = r[2].as_f64().unwrap_or(0.0) as f32;
                        let y1 = r[3].as_f64().unwrap_or(0.0) as f32;
                        FfiRect { x: x0, y: y0, width: (x1 - x0).abs(), height: (y1 - y0).abs() }
                    } else {
                        FfiRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
                    }
                } else {
                    FfiRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
                };
                findings.push(crate::edit::FfiRedactionFinding { page, kind, rect, covered_text });
            }
        }
        Ok(findings)
    }

    /// Permanently applies all pending redaction marks, removes underlying content,
    /// and verifies removal with the audit crate.
    pub fn apply_redactions(&self, doc_id: u64) -> Result<crate::edit::FfiRedactionApplyResult, FfiError> {
        let args = serde_json::json!({ "doc": doc_id }).to_string();
        let res = self.invoke_tool("redact_apply".into(), args)?;
        if !res.success {
            return Err(FfiError::InvalidArgs { reason: res.error.unwrap_or_else(|| "redact_apply failed".into()) });
        }
        let parsed: serde_json::Value = serde_json::from_str(&res.output_json).unwrap_or_default();
        let applied_count = parsed.get(0).and_then(|v| v.get("applied")).and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        // Run post-redaction audit to verify that no covered text remains
        let audit_findings = self.audit_redactions(doc_id)?;
        let audit_passed = audit_findings.is_empty();

        Ok(crate::edit::FfiRedactionApplyResult { applied_count, audit_passed, audit_findings })
    }

    /// Inspects a PKCS#12 (.p12 / .pfx) digital identity bundle and returns its certificate details.
    pub fn inspect_digital_id(&self, p12_bytes: Vec<u8>, password: Option<String>) -> Result<crate::signing::FfiDigitalIdInfo, FfiError> {
        #[cfg(not(feature = "sign"))]
        {
            let _ = (p12_bytes, password);
            Err(FfiError::Unsupported { reason: "Digital signatures feature not enabled".into() })
        }
        #[cfg(feature = "sign")]
        {
            let id = pdfcraft_engine::sign::pkcs12::open(&p12_bytes, password.as_deref().unwrap_or("")).map_err(|e| match e {
                pdfcraft_engine::sign::SignError::WrongPassword => FfiError::InvalidArgs { reason: "the digital ID password is incorrect".into() },
                e => FfiError::EditFailed { reason: e.to_string() },
            })?;
            Ok(crate::signing::parse_digital_id_info(&id))
        }
    }

    /// Generates a self-signed digital ID certificate and private key, packaging them into PKCS#12 bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn create_self_signed_digital_id(
        &self,
        name: String,
        password: String,
        key_type: String,
        organization: Option<String>,
        unit: Option<String>,
        email: Option<String>,
        country: Option<String>,
        validity_years: u32,
    ) -> Result<Vec<u8>, FfiError> {
        #[cfg(not(feature = "sign"))]
        {
            let _ = (name, password, key_type, organization, unit, email, country, validity_years);
            Err(FfiError::Unsupported { reason: "Digital signatures feature not enabled".into() })
        }
        #[cfg(feature = "sign")]
        {
            let name_trimmed = name.trim();
            if name_trimmed.is_empty() {
                return Err(FfiError::InvalidArgs { reason: "name must not be empty".into() });
            }
            if password.chars().count() < 6 {
                return Err(FfiError::InvalidArgs { reason: "password must have at least 6 characters".into() });
            }
            let key = match key_type.to_lowercase().as_str() {
                "rsa2048" | "rsa_2048" | "rsa" => pdfcraft_engine::sign::PrivateKey::generate_rsa(2048),
                "rsa3072" | "rsa_3072" => pdfcraft_engine::sign::PrivateKey::generate_rsa(3072),
                "rsa4096" | "rsa_4096" => pdfcraft_engine::sign::PrivateKey::generate_rsa(4096),
                "p256" | "ecdsa_p256" | "ecdsa" => pdfcraft_engine::sign::PrivateKey::generate_p256(),
                other => {
                    return Err(FfiError::InvalidArgs {
                        reason: format!("unsupported key type: {other} (supported: rsa2048, rsa3072, rsa4096, p256)"),
                    });
                }
            }
            .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

            let dn = pdfcraft_engine::sign::Name::build(
                name_trimmed,
                unit.as_deref().unwrap_or(""),
                organization.as_deref().unwrap_or(""),
                email.as_deref().unwrap_or(""),
                country.as_deref().unwrap_or(""),
            );
            let years = validity_years.clamp(1, 50);
            let now = {
                let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
                inner.session.now_secs()
            };
            let serial = pdfcraft_engine::sign::DigestAlg::Sha256.digest(&[name_trimmed.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
            let cert = pdfcraft_engine::sign::Certificate::self_signed(&dn, &key, pdfcraft_engine::sign::der::Time::from_unix(now), years, &serial)
                .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

            let id = pdfcraft_engine::sign::DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some(name_trimmed.to_string()) };
            pdfcraft_engine::sign::pkcs12::write(&id, &password).map_err(|e| FfiError::EditFailed { reason: e.to_string() })
        }
    }

    /// Lists digital signatures in the document along with their validation verdict.
    pub fn list_digital_signatures(&self, doc_id: u64) -> Result<Vec<crate::signing::FfiDigitalSignature>, FfiError> {
        #[cfg(not(feature = "sign"))]
        {
            let _ = doc_id;
            Err(FfiError::Unsupported { reason: "Digital signatures feature not enabled".into() })
        }
        #[cfg(feature = "sign")]
        {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let pages = &doc.info.pages;
            let mut result = Vec::new();
            for sig in doc.signatures.iter() {
                result.push(crate::signing::signature_info_to_ffi(sig, pages));
            }
            Ok(result)
        }
    }

    /// Cryptographically signs a document using a PKCS#12 digital identity (PAdES B-B).
    pub fn sign_document_p12(
        &self,
        doc_id: u64,
        p12_bytes: Vec<u8>,
        password: Option<String>,
        options: crate::signing::FfiSignOptions,
    ) -> Result<crate::signing::FfiSignResult, FfiError> {
        #[cfg(not(feature = "sign"))]
        {
            let _ = (doc_id, p12_bytes, password, options);
            Err(FfiError::Unsupported { reason: "digital signing is not supported in this build".into() })
        }
        #[cfg(feature = "sign")]
        {
            let id = pdfcraft_engine::sign::pkcs12::open(&p12_bytes, password.as_deref().unwrap_or("")).map_err(|e| match e {
                pdfcraft_engine::sign::SignError::WrongPassword => FfiError::InvalidArgs { reason: "the digital ID password is incorrect".into() },
                e => FfiError::EditFailed { reason: e.to_string() },
            })?;

            let graphic = if let Some(txt) = &options.graphic_text {
                if let Some(c) = pdfcraft_engine::script_missing(txt) {
                    return Err(FfiError::InvalidArgs {
                        reason: format!("signature script font cannot render '{txt}' (missing U+{:04X})", c as u32),
                    });
                }
                Some(
                    pdfcraft_engine::typed_signature_graphic(txt)
                        .ok_or_else(|| FfiError::InvalidArgs { reason: "cannot generate script graphic".into() })?,
                )
            } else if let Some(strokes) = &options.graphic_strokes {
                let mut stroke_vecs: Vec<Vec<[f64; 2]>> = Vec::new();
                for stroke in strokes {
                    let pts: Vec<[f64; 2]> = stroke.iter().map(|p| [p.x as f64, p.y as f64]).collect();
                    if pts.len() >= 2 {
                        stroke_vecs.push(pts);
                    }
                }
                if !stroke_vecs.is_empty() { Some(pdfcraft_engine::sign::Graphic::Strokes(stroke_vecs)) } else { None }
            } else {
                None
            };

            let (signed_bytes, saved_path, signer_name, field_name, revision) = {
                let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
                let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let (field, page_idx, user_rect) = if let Some(field_name) = &options.field_name {
                    (Some(field_name.clone()), 0, None)
                } else {
                    let p_idx = options.page.unwrap_or(0) as usize;
                    let page = doc.info.pages.get(p_idx).ok_or(FfiError::InvalidArgs { reason: format!("page {p_idx} does not exist") })?;
                    let u_rect = if let Some(vr) = options.rect {
                        let [u0_x, u0_y] = page.view_to_user(vr.x, vr.y);
                        let [u1_x, u1_y] = page.view_to_user(vr.x + vr.width, vr.y + vr.height);
                        let u = [u0_x.min(u1_x) as f64, u0_y.min(u1_y) as f64, u0_x.max(u1_x) as f64, u0_y.max(u1_y) as f64];
                        if u[2] - u[0] < 4.0 || u[3] - u[1] < 4.0 {
                            return Err(FfiError::InvalidArgs { reason: "signature rectangle is too small (must be at least 4x4 points)".into() });
                        }
                        Some(u)
                    } else {
                        None
                    };
                    (None, p_idx, u_rect)
                };

                let sign_opts = pdfcraft_engine::SignOptions {
                    field,
                    page: page_idx,
                    rect: user_rect,
                    reason: options.reason,
                    location: options.location,
                    contact: options.contact,
                    certify: options.certify_level,
                    graphic,
                    ..Default::default()
                };

                let signed_bytes =
                    inner.session.sign(pdfcraft_engine::DocId(doc_id), &id, sign_opts).map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

                let saved_path = if let Some(out_p) = &options.out_path {
                    let p = std::path::Path::new(out_p);
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| FfiError::Io { reason: e.to_string() })?;
                    }
                    std::fs::write(p, signed_bytes.as_ref()).map_err(|e| FfiError::Io { reason: e.to_string() })?;
                    Some(out_p.clone())
                } else {
                    None
                };

                inner
                    .session
                    .mark_signed(pdfcraft_engine::DocId(doc_id), signed_bytes.clone(), saved_path.clone())
                    .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;
                inner.doc_bytes.insert(doc_id, signed_bytes.clone());

                let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
                let newest = doc.signatures.iter().filter(|s| s.signed).max_by_key(|s| s.revision);
                let (s_name, f_name, rev) = if let Some(s) = newest {
                    (s.signer.clone().unwrap_or_else(|| id.certificate.display_name()), s.field.clone(), s.revision as u32)
                } else {
                    (id.certificate.display_name(), "Signature".into(), 1)
                };

                (signed_bytes, saved_path, s_name, f_name, rev)
            };

            Ok(crate::signing::FfiSignResult { success: true, bytes: (*signed_bytes).clone(), saved_path, signer_name, field_name, revision })
        }
    }

    // MARK: - Scan & OCR

    /// Overrides or sets the directory containing the text-detection.rten and text-recognition.rten models.
    pub fn set_ocr_models_dir(&self, dir_path: String) -> Result<(), FfiError> {
        #[cfg(not(feature = "ocr"))]
        {
            let _ = dir_path;
            Err(FfiError::Unsupported { reason: "OCR is not supported in this build".into() })
        }
        #[cfg(feature = "ocr")]
        {
            let p = std::path::Path::new(&dir_path);
            if !p.is_dir() {
                return Err(FfiError::InvalidArgs { reason: format!("OCR models directory does not exist: {dir_path}") });
            }
            pdfcraft_engine::ocr::set_models_dir(p);
            Ok(())
        }
    }

    /// Checks if text recognition models are installed and accessible.
    pub fn is_ocr_available(&self) -> bool {
        pdfcraft_engine::ocr::available()
    }

    /// Queries OCR model status, search directories, and supported language codes.
    pub fn ocr_status(&self) -> Result<crate::ocr::FfiOcrStatus, FfiError> {
        #[cfg(not(feature = "ocr"))]
        {
            Err(FfiError::Unsupported { reason: "OCR is not supported in this build".into() })
        }
        #[cfg(feature = "ocr")]
        {
            let dirs: Vec<String> = pdfcraft_engine::ocr::Models::search_dirs().iter().map(|d| d.to_string_lossy().into_owned()).collect();
            let langs: Vec<String> = pdfcraft_engine::ocr::LANGUAGES.iter().map(|(c, n)| format!("{c}: {n}")).collect();
            Ok(crate::ocr::FfiOcrStatus { available: pdfcraft_engine::ocr::available(), search_dirs: dirs, languages: langs })
        }
    }

    /// Runs text recognition on a document page and writes the recognized words into an invisible searchable text layer.
    pub fn recognize_page_text(
        &self,
        doc_id: u64,
        page: u32,
        dpi: Option<f32>,
        skip_text_pages: bool,
    ) -> Result<crate::ocr::FfiOcrPageResult, FfiError> {
        #[cfg(not(feature = "ocr"))]
        {
            let _ = (doc_id, page, dpi, skip_text_pages);
            Err(FfiError::Unsupported { reason: "OCR is not supported in this build".into() })
        }
        #[cfg(feature = "ocr")]
        {
            if !pdfcraft_engine::ocr::available() {
                return Err(FfiError::Unsupported { reason: "OCR models are not installed (set PDFCRAFT_MODELS or call set_ocr_models_dir)".into() });
            }

            let settings = pdfcraft_engine::ocr::OcrSettings { dpi: dpi.unwrap_or(300.0), language: "en".into(), skip_text_pages, native_text_only: false };

            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let page_idx = page as usize;
            let found = inner
                .session
                .recognize_text(pdfcraft_engine::DocId(doc_id), &[page_idx], settings)
                .map_err(|e| FfiError::EditFailed { reason: e })?;

            if let Some(bytes) = inner.session.get(pdfcraft_engine::DocId(doc_id)).map(|d| d.bytes.clone()) {
                inner.doc_bytes.insert(doc_id, bytes);
                let ver = inner.doc_versions.entry(doc_id).or_insert(1);
                *ver += 1;
                inner.tile_cache.evict_page(doc_id, page);
            }

            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page_idx).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let first = found.into_iter().next().unwrap_or(pdfcraft_engine::ocr::OcrPage {
                page: page_idx,
                words: Vec::new(),
                already_on_page: 0,
                skipped: None,
            });

            let ffi_words: Vec<crate::ocr::FfiOcrWord> = first.words.iter().map(|w| crate::ocr::placed_word_to_ffi(w, page_info)).collect();
            let text = first.text();
            let word_count = ffi_words.len() as u32;

            Ok(crate::ocr::FfiOcrPageResult {
                page,
                words: ffi_words,
                text,
                word_count,
                already_on_page: first.already_on_page as u32,
                skipped: first.skipped,
            })
        }
    }

    /// Injects an invisible searchable text layer into a page using externally recognized words (e.g. from platform OCR like Apple Vision or Android ML Kit).
    pub fn apply_ocr_text_layer(&self, doc_id: u64, page: u32, words: Vec<crate::ocr::FfiOcrWord>) -> Result<u32, FfiError> {
        #[cfg(not(feature = "ocr"))]
        {
            let _ = (doc_id, page, words);
            Err(FfiError::Unsupported { reason: "OCR is not supported in this build".into() })
        }
        #[cfg(feature = "ocr")]
        {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let page_idx = page as usize;
            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let page_info = doc.info.pages.get(page_idx).ok_or(FfiError::PageOutOfBounds { page, total: doc.info.pages.len() as u32 })?;

            let mut placed = Vec::new();
            for w in &words {
                let [u_bl_x, u_bl_y] = page_info.view_to_user(w.rect.x, w.rect.y + w.rect.height);
                let [u_br_x, u_br_y] = page_info.view_to_user(w.rect.x + w.rect.width, w.rect.y + w.rect.height);
                let [u_tl_x, u_tl_y] = page_info.view_to_user(w.rect.x, w.rect.y);
                let origin = [u_bl_x as f64, u_bl_y as f64];
                let across = [(u_br_x - u_bl_x) as f64, (u_br_y - u_bl_y) as f64];
                let up = [(u_tl_x - u_bl_x) as f64, (u_tl_y - u_bl_y) as f64];
                placed.push(pdfcraft_engine::ocr::PlacedWord { text: w.text.clone(), origin, across, up });
            }

            let count = placed.len() as u32;
            let ocr_page = pdfcraft_engine::ocr::OcrPage { page: page_idx, words: placed, already_on_page: 0, skipped: None };

            inner.session.apply_ocr(pdfcraft_engine::DocId(doc_id), &[ocr_page]).map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

            if let Some(bytes) = inner.session.get(pdfcraft_engine::DocId(doc_id)).map(|d| d.bytes.clone()) {
                inner.doc_bytes.insert(doc_id, bytes);
                let ver = inner.doc_versions.entry(doc_id).or_insert(1);
                *ver += 1;
                inner.tile_cache.evict_page(doc_id, page);
            }

            Ok(count)
        }
    }

    // MARK: - Form JavaScript (MOB-6.5)

    /// Sets whether Acrobat Form JavaScript execution is enabled.
    pub fn set_form_javascript_enabled(&self, enabled: bool) -> Result<(), FfiError> {
        #[cfg(not(feature = "js"))]
        {
            let _ = enabled;
            Err(FfiError::Unsupported { reason: "Form JavaScript feature not enabled".into() })
        }
        #[cfg(feature = "js")]
        {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.session.set_javascript(enabled);
            Ok(())
        }
    }

    /// Checks if Acrobat Form JavaScript execution is enabled.
    pub fn is_form_javascript_enabled(&self) -> Result<bool, FfiError> {
        #[cfg(not(feature = "js"))]
        {
            Err(FfiError::Unsupported { reason: "Form JavaScript feature not enabled".into() })
        }
        #[cfg(feature = "js")]
        {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            Ok(inner.session.javascript())
        }
    }

    /// Runs a JavaScript snippet in the context of the document.
    pub fn run_form_javascript(&self, doc_id: u64, script: String, target: Option<String>) -> Result<crate::advanced::FfiJsResult, FfiError> {
        #[cfg(not(feature = "js"))]
        {
            let _ = (doc_id, script, target);
            Err(FfiError::Unsupported { reason: "Form JavaScript feature not enabled".into() })
        }
        #[cfg(feature = "js")]
        {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let outcome = inner.session.run_javascript(pdfcraft_engine::DocId(doc_id), &script, target.as_deref()).map_err(FfiError::from)?;

            if let Some(bytes) = inner.session.get(pdfcraft_engine::DocId(doc_id)).map(|d| d.bytes.clone()) {
                inner.doc_bytes.insert(doc_id, bytes);
                let ver = inner.doc_versions.entry(doc_id).or_insert(1);
                *ver += 1;
                inner.tile_cache.evict_for_doc(doc_id);
            }

            let output = outcome.console.join("\n");
            Ok(crate::advanced::FfiJsResult { success: outcome.rc, output, error: outcome.error })
        }
    }

    /// Retrieves and clears any pending JavaScript output (alerts, console messages, errors) for the document.
    pub fn take_form_javascript_output(&self, doc_id: u64) -> Result<crate::advanced::FfiJsOutput, FfiError> {
        #[cfg(not(feature = "js"))]
        {
            let _ = doc_id;
            Err(FfiError::Unsupported { reason: "Form JavaScript feature not enabled".into() })
        }
        #[cfg(feature = "js")]
        {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let out = inner.session.take_js_output(pdfcraft_engine::DocId(doc_id));
            Ok(crate::advanced::FfiJsOutput { alerts: out.alerts.into_iter().map(|m| m.text).collect(), console: out.console, errors: out.errors })
        }
    }

    // MARK: - XFA Forms (MOB-6.5)

    /// Inspects XFA form status and layout information.
    pub fn get_xfa_status(&self, doc_id: u64) -> Result<crate::advanced::FfiXfaStatus, FfiError> {
        #[cfg(not(feature = "xfa"))]
        {
            let _ = doc_id;
            Err(FfiError::Unsupported { reason: "XFA forms feature not enabled".into() })
        }
        #[cfg(feature = "xfa")]
        {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let kind = match doc.info.xfa {
                Some(pdfcraft_render::Xfa::Dynamic) => "dynamic",
                Some(pdfcraft_render::Xfa::Static) => "static",
                None => "none",
            }
            .to_string();
            let (layout_pages, layout_fields) = doc.xfa.as_ref().map(|x| (x.pages as u32, x.fields as u32)).unwrap_or((0, 0));
            let warnings = doc.xfa_warnings.clone();
            Ok(crate::advanced::FfiXfaStatus { kind, layout_pages, layout_fields, warnings })
        }
    }

    /// Exports the XFA form datasets XML.
    pub fn export_xfa_datasets(&self, doc_id: u64) -> Result<String, FfiError> {
        #[cfg(not(feature = "xfa"))]
        {
            let _ = doc_id;
            Err(FfiError::Unsupported { reason: "XFA forms feature not enabled".into() })
        }
        #[cfg(feature = "xfa")]
        {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let xml_bytes =
                inner.session.export_data(pdfcraft_engine::DocId(doc_id), pdfcraft_engine::DataFormat::Xml, false, true).map_err(FfiError::from)?;
            String::from_utf8(xml_bytes).map_err(|e| FfiError::Internal { reason: format!("invalid UTF-8 in XFA datasets: {e}") })
        }
    }

    /// Imports XFA datasets XML, updating form data and recalculating dependent fields.
    pub fn import_xfa_datasets(&self, doc_id: u64, datasets_xml: String) -> Result<(), FfiError> {
        #[cfg(not(feature = "xfa"))]
        {
            let _ = (doc_id, datasets_xml);
            Err(FfiError::Unsupported { reason: "XFA forms feature not enabled".into() })
        }
        #[cfg(feature = "xfa")]
        {
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(
                doc_id,
                pdfcraft_engine::Edit::ImportData { name: "datasets.xml".into(), bytes: std::sync::Arc::new(datasets_xml.into_bytes()) },
            )
        }
    }

    // MARK: - PDF/A Preflight & Conversion (MOB-6.5)

    /// Verifies compliance against a target PDF/A specification level ("pdfa2b", "pdfa3b").
    pub fn verify_pdfa(&self, doc_id: u64, level: String) -> Result<Vec<crate::advanced::FfiPreflightIssue>, FfiError> {
        #[cfg(not(feature = "preflight"))]
        {
            let _ = (doc_id, level);
            Err(FfiError::Unsupported { reason: "PDF/A preflight feature not enabled".into() })
        }
        #[cfg(feature = "preflight")]
        {
            let lvl = pdfcraft_engine::pdfa::Level::from_id(&level)
                .ok_or_else(|| FfiError::InvalidArgs { reason: format!("unsupported PDF/A level: '{level}' (supported: 'pdfa2b', 'pdfa3b')") })?;
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let issues = doc.pdfa_verify(lvl);
            Ok(issues
                .into_iter()
                .map(|i| crate::advanced::FfiPreflightIssue {
                    clause: i.clause.to_string(),
                    message: i.message,
                    page: i.page.map(|p| p as u32),
                    fixable: i.fixable,
                })
                .collect())
        }
    }

    /// Returns standards declared in document XMP metadata and output intents.
    pub fn declared_standards(&self, doc_id: u64) -> Result<crate::advanced::FfiPreflightDeclared, FfiError> {
        #[cfg(not(feature = "preflight"))]
        {
            let _ = doc_id;
            Err(FfiError::Unsupported { reason: "PDF/A preflight feature not enabled".into() })
        }
        #[cfg(feature = "preflight")]
        {
            let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let d = doc.standards();
            Ok(crate::advanced::FfiPreflightDeclared {
                pdfa_part: d.pdfa.as_ref().map(|(p, _)| *p as u32),
                pdfa_conformance: d.pdfa.as_ref().map(|(_, c)| c.clone()),
                pdfua_part: d.pdfua.map(|p| p as u32),
                output_intents: d.output_intents,
            })
        }
    }

    /// Converts document to the requested PDF/A standard level, returning the number of remaining unfixable issues.
    pub fn convert_to_pdfa(&self, doc_id: u64, level: String) -> Result<u32, FfiError> {
        #[cfg(not(feature = "preflight"))]
        {
            let _ = (doc_id, level);
            Err(FfiError::Unsupported { reason: "PDF/A preflight feature not enabled".into() })
        }
        #[cfg(feature = "preflight")]
        {
            let lvl = pdfcraft_engine::pdfa::Level::from_id(&level)
                .ok_or_else(|| FfiError::InvalidArgs { reason: format!("unsupported PDF/A level: '{level}' (supported: 'pdfa2b', 'pdfa3b')") })?;
            let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
            inner.apply_edit(doc_id, pdfcraft_engine::Edit::ConvertPdfA { level: lvl })?;

            let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
            let remaining = doc.pdfa_verify(lvl);
            Ok(remaining.len() as u32)
        }
    }

    // MARK: - Document Compare (MOB-6.5)

    /// Compares two documents and returns detailed text differences.
    pub fn compare_documents(&self, old_doc_id: u64, new_doc_id: u64) -> Result<Vec<crate::advanced::FfiDocumentChange>, FfiError> {
        let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
        let comp = inner
            .session
            .compare(pdfcraft_engine::DocId(old_doc_id), pdfcraft_engine::DocId(new_doc_id))
            .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

        let old_doc = inner.session.get(pdfcraft_engine::DocId(old_doc_id)).ok_or_else(|| FfiError::NotFound { id: old_doc_id.to_string() })?;
        let new_doc = inner.session.get(pdfcraft_engine::DocId(new_doc_id)).ok_or_else(|| FfiError::NotFound { id: new_doc_id.to_string() })?;

        let to_ffi_side = |s: &pdfcraft_engine::compare::Side, doc: &pdfcraft_engine::Document| {
            let page_info = doc.info.pages.get(s.page);
            let rects: Vec<FfiRect> = match page_info {
                Some(pi) => s.rects.iter().map(|&r| user_box_to_view_rect(pi, r.map(|v| v as f32))).collect(),
                None => Vec::new(),
            };
            crate::advanced::FfiChangeSide { text: s.text.clone(), page: s.page as u32, rects }
        };

        let changes = comp
            .changes
            .into_iter()
            .map(|ch| {
                let kind = match ch.kind {
                    pdfcraft_engine::compare::Kind::Inserted => crate::advanced::FfiChangeKind::Inserted,
                    pdfcraft_engine::compare::Kind::Deleted => crate::advanced::FfiChangeKind::Deleted,
                    pdfcraft_engine::compare::Kind::Replaced => crate::advanced::FfiChangeKind::Replaced,
                };
                crate::advanced::FfiDocumentChange { kind, old_side: to_ffi_side(&ch.old, old_doc), new_side: to_ffi_side(&ch.new, new_doc) }
            })
            .collect();

        Ok(changes)
    }

    /// Compares rendered pages of two documents and returns visual difference bounding boxes.
    pub fn compare_documents_visual(
        &self,
        old_doc_id: u64,
        new_doc_id: u64,
        dpi: Option<f32>,
    ) -> Result<Vec<crate::advanced::FfiVisualDiff>, FfiError> {
        let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
        let diffs = inner
            .session
            .compare_visual(pdfcraft_engine::DocId(old_doc_id), pdfcraft_engine::DocId(new_doc_id), dpi.unwrap_or(72.0))
            .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

        let new_doc = inner.session.get(pdfcraft_engine::DocId(new_doc_id)).ok_or_else(|| FfiError::NotFound { id: new_doc_id.to_string() })?;

        let mut result = Vec::new();
        for (page, user_box) in diffs {
            let rect = if let Some(pi) = new_doc.info.pages.get(page) {
                user_box_to_view_rect(pi, user_box.map(|v| v as f32))
            } else {
                FfiRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
            };
            result.push(crate::advanced::FfiVisualDiff { page: page as u32, rect });
        }
        Ok(result)
    }

    /// Marks differences between two documents as annotations in the newer document.
    pub fn mark_document_differences(&self, old_doc_id: u64, new_doc_id: u64) -> Result<u32, FfiError> {
        let mut inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
        let count = inner
            .session
            .mark_differences(pdfcraft_engine::DocId(old_doc_id), pdfcraft_engine::DocId(new_doc_id))
            .map_err(|e| FfiError::EditFailed { reason: e.to_string() })?;

        if let Some(bytes) = inner.session.get(pdfcraft_engine::DocId(new_doc_id)).map(|d| d.bytes.clone()) {
            inner.doc_bytes.insert(new_doc_id, bytes);
            let ver = inner.doc_versions.entry(new_doc_id).or_insert(1);
            *ver += 1;
            inner.tile_cache.evict_for_doc(new_doc_id);
        }

        Ok(count as u32)
    }

    // MARK: - Accessibility Checker (MOB-6.5)

    /// Evaluates the document against all 32 Acrobat accessibility rules (PDF/UA & WCAG).
    pub fn check_accessibility(&self, doc_id: u64) -> Result<crate::advanced::FfiA11yReport, FfiError> {
        let inner = self.inner.lock().map_err(|e| FfiError::Internal { reason: format!("lock poisoned: {e}") })?;
        let doc = inner.session.get(pdfcraft_engine::DocId(doc_id)).ok_or_else(|| FfiError::NotFound { id: doc_id.to_string() })?;
        let report = doc
            .accessibility_check(&pdfcraft_engine::a11y::Options::default())
            .ok_or_else(|| FfiError::EditFailed { reason: "document editor not available for accessibility check".into() })?;

        let mut passed_count = 0u32;
        let mut failed_count = 0u32;
        let mut manual_check_count = 0u32;
        let mut skipped_count = 0u32;
        let mut results = Vec::new();

        for r in &report.results {
            let status = match r.status {
                pdfcraft_engine::a11y::Status::Passed => {
                    passed_count += 1;
                    crate::advanced::FfiA11yStatus::Passed
                }
                pdfcraft_engine::a11y::Status::Failed => {
                    failed_count += 1;
                    crate::advanced::FfiA11yStatus::Failed
                }
                pdfcraft_engine::a11y::Status::Manual => {
                    manual_check_count += 1;
                    crate::advanced::FfiA11yStatus::NeedsManualCheck
                }
                pdfcraft_engine::a11y::Status::Skipped => {
                    skipped_count += 1;
                    crate::advanced::FfiA11yStatus::Skipped
                }
            };

            let findings = r.findings.iter().map(|f| f.message.clone()).collect();
            results.push(crate::advanced::FfiA11yRuleResult {
                rule_id: r.rule.id().to_string(),
                rule_name: r.rule.name().to_string(),
                category: r.rule.category().label().to_string(),
                status,
                description: r.rule.description().to_string(),
                explanation: r.rule.explanation().to_string(),
                findings,
            });
        }

        Ok(crate::advanced::FfiA11yReport { passed_count, failed_count, manual_check_count, skipped_count, results })
    }

    // MARK: - CJK Fallback Fonts (MOB-6.6 / O6)

    /// Returns the availability status of CJK fallback fonts in this engine build.
    pub fn get_cjk_font_status(&self) -> crate::advanced::FfiCjkFontStatus {
        crate::advanced::FfiCjkFontStatus {
            has_japanese_font: pdfcraft_fonts::document_japanese_font().is_some(),
            has_chinese_font: !pdfcraft_fonts::ui_chinese_fonts().is_empty(),
            craft_fonts_count: pdfcraft_fonts::CRAFT_FONTS.len() as u32,
        }
    }
}
