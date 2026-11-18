//! Where the Combine grid's pictures come from: the first page each file contributes (and every page
//! of an expanded file), rendered off the UI thread through [`RenderPool`], which keeps the renderer's
//! panic guard, size caps and watchdog. The thumbnail store, its budgets and its lanes are the
//! framework's ([`Thumbnails`]); this module only says what a key's picture is and how it is made.
//!
//! A file's render keeps its parsed document open for [`OpenDocuments::KEEP_OPEN`], so the next page
//! of the same file does not read the whole file again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use craft::host::queue::Cancel;
use craft::ui::primitives::textures::{
    LoadFailure, LoadedPicture, Revision, TextureState, ThumbJob, ThumbRender, ThumbRequest, ThumbSource, ThumbnailOptions, Thumbnails,
};
use pdfcraft_render::{RenderPool, RenderRequest};
use web_time::Instant;

use super::{PageList, thumbnail_config};
use crate::combine_ui::{CombineFile, CombineKey};

/// The longest side of a thumbnail, in pixels. A card at the top of the zoom ladder on a dense screen
/// asks for more than this and is drawn a little upscaled.
pub(super) const MAX_SIDE: u32 = 1536;

/// The store's salt: the grid's thumbnails and nothing else live under it.
const SALT: &str = "pdfcraft-combine-thumbnails";

/// The thumbnails of the Combine grid in `ctx`.
pub(super) fn thumbnails(ctx: &egui::Context) -> Thumbnails<CombineKey> {
    Thumbnails::<CombineKey>::in_context(ctx, SALT, ThumbnailOptions::new().max_side(MAX_SIDE))
}

/// Page `page`'s size in points when it is one a picture can be drawn at.
pub(super) fn page_size(sizes: &[(f32, f32)], page: usize) -> Option<(f32, f32)> {
    sizes.get(page).copied().filter(|(w, h)| w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0)
}

/// The open documents, whether or not a render that panicked held the lock.
pub(super) fn lock(documents: &Mutex<OpenDocuments>) -> MutexGuard<'_, OpenDocuments> {
    documents.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A pool whose render is done, with its document still open for the file's next page.
struct Open {
    /// The address of the file's shared bytes: the same file, even after undo brings it back.
    source: usize,
    /// How the file was read (with which password).
    auth: u64,
    pool: RenderPool,
    since: Instant,
}

impl std::fmt::Debug for Open {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Open").field("source", &self.source).field("auth", &self.auth).finish_non_exhaustive()
    }
}

/// The documents kept open between one page's render and the next, shared by the render jobs of one
/// Combine tab.
#[derive(Debug, Default)]
pub(crate) struct OpenDocuments {
    open: Vec<Open>,
    /// How many documents were opened (parsed) since the tab began.
    opened: usize,
}

impl OpenDocuments {
    /// A file whose render is done keeps its document open this long, so the next page of the same
    /// file renders without reading the whole file again.
    pub(super) const KEEP_OPEN: Duration = Duration::from_secs(2);
    /// Documents kept open at once (each holds a parsed file and an idle thread).
    const MAX_OPEN: usize = 3;

    /// Closes the documents kept open past their time, as of `now`.
    pub(super) fn sweep(&mut self, now: Instant) {
        self.open.retain(|o| now.saturating_duration_since(o.since) < Self::KEEP_OPEN);
    }

    /// Closes every document (the tab closed).
    pub(crate) fn clear(&mut self) {
        self.open.clear();
    }

    /// How many documents were opened (parsed) since the tab began.
    #[cfg(test)]
    pub(super) fn opened(&self) -> usize {
        self.opened
    }

    /// How many documents are kept open now.
    #[cfg(test)]
    pub(super) fn kept(&self) -> usize {
        self.open.len()
    }

    /// The pool that has `source` open as `auth` read it, or one from `make`; counts the ones made.
    fn take_or_open(&mut self, source: usize, auth: u64, now: Instant, make: impl FnOnce() -> RenderPool) -> RenderPool {
        self.sweep(now);
        match self.open.iter().position(|o| o.source == source && o.auth == auth) {
            Some(at) => self.open.swap_remove(at).pool,
            None => {
                self.opened = self.opened.saturating_add(1);
                make()
            }
        }
    }

    /// Keeps `pool` open for `source`'s next page; the longest-open document makes room.
    fn keep(&mut self, source: usize, auth: u64, pool: RenderPool, now: Instant) {
        self.sweep(now);
        if self.open.len() >= Self::MAX_OPEN
            && let Some(oldest) = self.open.iter().enumerate().min_by_key(|(_, o)| o.since).map(|(at, _)| at)
        {
            self.open.swap_remove(oldest);
        }
        self.open.push(Open { source, auth, pool, since: now });
    }
}

/// The job that renders `page` of `bytes` at `scale` pixels per point.
pub(super) fn page_job(
    documents: Arc<Mutex<OpenDocuments>>,
    bytes: Arc<Vec<u8>>,
    auth: u64,
    password: Option<Arc<str>>,
    page: usize,
    scale: f32,
) -> ThumbJob {
    Box::new(move |cancel| render_page(&documents, &bytes, auth, password, page, scale, cancel))
}

/// Renders one page on the pool of its file, taking the document kept open if there is one.
fn render_page(
    documents: &Mutex<OpenDocuments>,
    bytes: &Arc<Vec<u8>>,
    auth: u64,
    password: Option<Arc<str>>,
    page: usize,
    scale: f32,
    cancel: &Cancel,
) -> Result<LoadedPicture, LoadFailure> {
    let source = Arc::as_ptr(bytes) as usize;
    let pool = lock(documents)
        .take_or_open(source, auth, Instant::now(), || RenderPool::new(bytes.clone(), 1, thumbnail_config(password)).without_replacements());
    pool.set_queue(vec![RenderRequest { page, scale, tag: 1, ..Default::default() }]);
    // Poll the pool: its worker is the one that can be given up on (the watchdog answers an error),
    // so a page that never finishes holds no queue lane for good. A pool without threads renders
    // inside `try_recv`, so there no answer is one that never comes.
    let answer = loop {
        if cancel.is_cancelled() {
            return Err(LoadFailure::Missing);
        }
        match pool.try_recv() {
            Some(out) if out.request.page == page => break Some(out),
            Some(_) => {}
            None if pool.is_inline() => break None,
            None => std::thread::sleep(Duration::from_millis(4)),
        }
    };
    let out = answer.ok_or_else(|| LoadFailure::Message("the page wasn't drawn".to_owned()))?;
    if let Some(error) = out.error {
        return Err(LoadFailure::Message(error));
    }
    let (w, h) = (out.width as usize, out.height as usize);
    let limit = 2 * MAX_SIDE as usize;
    if w == 0 || h == 0 || w > limit || h > limit {
        return Err(LoadFailure::Message(format!("the thumbnail came back {w}×{h} pixels")));
    }
    if w.checked_mul(h).and_then(|p| p.checked_mul(4)) != Some(out.rgba.len()) {
        return Err(LoadFailure::Message("the thumbnail came back incomplete".to_owned()));
    }
    // The renderer's words are premultiplied RGBA, as egui's colours are.
    let image = egui::ColorImage::from_rgba_premultiplied([w, h], &out.rgba);
    if pool.threads() <= 1 {
        lock(documents).keep(source, auth, pool, Instant::now());
    }
    Ok(LoadedPicture::Image(image))
}

/// What the grid needs of one file to say its pictures.
struct Entry {
    bytes: Arc<Vec<u8>>,
    auth: u64,
    password: Option<Arc<str>>,
    sizes: Arc<[(f32, f32)]>,
    /// The pages the file shows (0-based, in the file): its first taken page, or all of them when it is
    /// expanded; none for a file that can't show a page (locked, unreadable, a bad range).
    pages: Arc<[usize]>,
}

/// The thumbnail source of the Combine grid, built each pass from the list: `File(id)` is the first page
/// the entry takes, `Page(id, nth)` the `nth` of the pages it adds.
pub(super) struct CombineThumbs {
    files: HashMap<u64, Entry>,
    documents: Arc<Mutex<OpenDocuments>>,
}

impl CombineThumbs {
    /// The source for `draft`; `expanded` holds the pages of each file shown as its pages. Documents kept
    /// open past their time are closed.
    pub(super) fn new(draft: &[CombineFile], expanded: &HashMap<u64, PageList>, documents: Arc<Mutex<OpenDocuments>>) -> Self {
        lock(&documents).sweep(Instant::now());
        let files = draft
            .iter()
            .map(|f| {
                let pages: Arc<[usize]> = if f.lock.is_some() || f.problem.is_some() {
                    Arc::from([])
                } else if let Some(list) = expanded.get(&f.id) {
                    list.order.clone()
                } else {
                    f.first_taken().into_iter().collect()
                };
                let entry = Entry { bytes: f.bytes.clone(), auth: f.auth, password: f.password().map(Arc::from), sizes: f.sizes.clone(), pages };
                (f.id, entry)
            })
            .collect();
        CombineThumbs { files, documents }
    }

    /// The entry of `key` and the page (0-based, in the file) it shows.
    fn page_of(&self, key: &CombineKey) -> Option<(&Entry, usize)> {
        let (id, nth) = match key {
            CombineKey::File(id) => (id, 0),
            CombineKey::Page(id, nth) => (id, *nth),
        };
        let entry = self.files.get(id)?;
        entry.pages.get(nth).map(|page| (entry, *page))
    }

    /// The version of `key`'s picture: how the file was read and the page it shows. The requested side is
    /// the store's to add.
    pub(super) fn revision_of(&self, key: &CombineKey) -> Revision {
        Revision::of(&self.page_of(key).map(|(entry, page)| (entry.auth, page)))
    }

    /// What `key` shows now, read without starting a render, at the side `request` asks.
    pub(super) fn state_of(&self, ctx: &egui::Context, key: &CombineKey, request: &ThumbRequest) -> TextureState {
        // A key with no page (a locked file, a range that takes none) shows nothing, whatever the store
        // still holds of an earlier read.
        if self.page_of(key).is_none() {
            return TextureState::Missing;
        }
        let side = request.side.min(MAX_SIDE);
        // The store keys a picture by the source's revision, the side and the region (none: the whole page).
        // PLAN: `Thumbnails` keeps that combination private; it needs a read that starts no work, so this
        // does not repeat its hash (owner: the framework's textures).
        let revision = Revision::of(&(self.revision_of(key), side, None::<[u32; 4]>));
        thumbnails(ctx).cache().state(ctx, key, revision)
    }
}

impl ThumbSource<CombineKey> for CombineThumbs {
    fn revision(&self, key: &CombineKey, _: &ThumbRequest) -> Revision {
        self.revision_of(key)
    }

    fn render(&self, key: &CombineKey, request: &ThumbRequest) -> ThumbRender {
        let Some((entry, page)) = self.page_of(key) else { return ThumbRender::Blank };
        let Some((w, h)) = page_size(&entry.sizes, page) else { return ThumbRender::Blank };
        // Pixels per point: the longest side of the page at `side` pixels.
        let scale = request.side as f32 / w.max(h);
        if !(scale.is_finite() && scale > 0.0) {
            return ThumbRender::Blank;
        }
        ThumbRender::Later(page_job(self.documents.clone(), entry.bytes.clone(), entry.auth, entry.password.clone(), page, scale))
    }
}
