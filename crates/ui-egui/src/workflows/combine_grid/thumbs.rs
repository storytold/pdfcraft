use super::{Arc, HashMap, RenderConfig, RenderPool, RenderRequest, Retiring, TextureHandle};

/// Render threads for thumbnails at once, counting those of retired pools until they exit.
pub(super) const MAX_THREADS: usize = 3;

/// Results taken per frame, and the bytes they may upload.
const UPLOADS_PER_FRAME: usize = 4;

const UPLOAD_BYTES_PER_FRAME: usize = 8 << 20;

/// Thumbnail textures kept, in bytes. Those out of view go first, longest unseen first; those in
/// view are kept even over budget (at most a screenful of thumbnails of at most [`MAX_SIDE`]).
const TEXTURE_BUDGET: usize = 32 << 20;

/// The longest side of a thumbnail, in pixels.
/// (Room for a Letter page at 500% on a screen of 1.5 pixels per point; sharper screens upscale a
/// little at the largest sizes.) This only binds near the top of the zoom range: a card below it
/// asks for less than this and costs no more than it did. Near the top one thumbnail is a few
/// megabytes, so [`TEXTURE_BUDGET`] and [`UPLOAD_BYTES_PER_FRAME`] keep the cache at a screenful
/// plus one frame's uploads — and at the top of the range a screenful is a card or two, because a
/// card is wider than most windows.
pub(super) const MAX_SIDE: f32 = 1536.0;

/// Thumbnail sizes round up to this many pixels, so a slightly different layout reuses them.
pub(super) const SIZE_STEP: u32 = 16;

/// A thumbnail's place: the file (by id) and the page (0-based, in the file). A file's card and
/// the card of its first page when expanded show the same thumbnail.
pub(super) type Slot = (u64, usize);

/// The page shown large (a card's magnifier) is kept beside the thumbnails under this slot, so it
/// shares their threads, their limits and their checks.
pub(super) const PREVIEW: Slot = (u64::MAX, 0);

/// The longest side of the page shown large, in pixels.
pub(super) const PREVIEW_SIDE: f32 = 2048.0;

/// A file whose render is done keeps its document open this long (seconds), so the next page of
/// the same file renders without reading the whole file again.
const KEEP_OPEN: f64 = 2.0;

/// What the grid has for a file (tests and the control channel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbState {
    /// Nothing yet: not in view, or nothing to draw (locked, unreadable, a bad page range, a page
    /// whose size is unknown).
    None,
    /// Being rendered.
    Pending,
    /// A thumbnail of page `page` (0-based), `width` × `height` pixels.
    Ready { page: usize, width: u32, height: u32 },
    /// The page couldn't be drawn.
    Failed(String),
}

/// Everything a thumbnail depends on: the file's bytes, how it was read (its password), the page
/// and the size. A cached thumbnail is used only while all of them still match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Key {
    /// The address of the file's shared bytes: the same file, even after undo brings it back.
    pub(super) source: usize,
    pub(super) auth: u64,
    pub(super) page: usize,
    pub(super) px: [u32; 2],
}

impl Key {
    fn same_page(&self, other: &Key) -> bool {
        (self.source, self.auth, self.page) == (other.source, other.auth, other.page)
    }
}

pub(super) struct Entry {
    pub(super) key: Key,
    pub(super) tex: Option<TextureHandle>,
    pub(super) error: Option<String>,
    /// The frame the card last showed it.
    pub(super) seen: u64,
    pub(super) bytes: usize,
}

pub(super) struct Job {
    pub(super) slot: Slot,
    pub(super) key: Key,
    pub(super) pool: RenderPool,
    pub(super) tag: u64,
}

/// A pool whose render is done, with its document still open: kept a moment for the next page of
/// the same file, read the same way.
pub(super) struct Open {
    source: usize,
    auth: u64,
    pool: RenderPool,
    /// When its render finished (seconds).
    since: f64,
}

/// A thumbnail a card needs this frame.
pub(super) struct Want {
    pub(super) slot: Slot,
    pub(super) key: Key,
    pub(super) bytes: Arc<Vec<u8>>,
    pub(super) password: Option<Arc<str>>,
    pub(super) scale: f32,
    pub(super) visible: bool,
}

/// The grid's thumbnails and the renders under way.
#[derive(Default)]
pub(crate) struct Thumbs {
    pub(super) entries: HashMap<Slot, Entry>,
    pub(super) jobs: Vec<Job>,
    /// Pools done rendering, their documents still open (their threads count).
    pub(super) open: Vec<Open>,
    /// Threads of pools let go of, until they exit.
    pub(super) retiring: Vec<Retiring>,
    pub(super) frame: u64,
    pub(super) next_tag: u64,
    pub(super) wants: Vec<Want>,
    /// Render on the UI thread, as the web build does (tests).
    #[cfg(test)]
    pub(super) inline: bool,
}

/// What a card can draw now.
pub(super) enum Look {
    Ready(egui::TextureId),
    Failed(String),
    /// Not ready; an older thumbnail of the same page may stand in.
    Pending(Option<egui::TextureId>),
}

impl Thumbs {
    /// Forget every thumbnail (the tab closed). Renders under way are let go of: told to stop at
    /// their next drawing operator, and counted until their threads exit.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.wants.clear();
        for job in self.jobs.drain(..) {
            self.retiring.push(job.pool.retire());
        }
        for open in self.open.drain(..) {
            self.retiring.push(open.pool.retire());
        }
    }

    /// Render threads that exist now (real threads, not pools): those of pools at work, of pools
    /// keeping a document open, and of pools let go of.
    pub(super) fn threads(&mut self) -> usize {
        self.retiring.retain_mut(|r| r.running() > 0);
        let retiring: usize = self.retiring.iter_mut().map(Retiring::running).sum();
        let working: usize = self.jobs.iter().map(|j| j.pool.threads()).sum();
        let open: usize = self.open.iter().map(|o| o.pool.threads()).sum();
        working.saturating_add(open).saturating_add(retiring)
    }

    pub(super) fn look(&mut self, slot: Slot, key: Key) -> Look {
        let frame = self.frame;
        match self.entries.get_mut(&slot) {
            Some(e) if e.key == key => {
                e.seen = frame;
                match (&e.tex, &e.error) {
                    (Some(tex), _) => Look::Ready(tex.id()),
                    (None, Some(err)) => Look::Failed(err.clone()),
                    (None, None) => Look::Pending(None),
                }
            }
            // Another size of the same page stands in until this one arrives. A page that failed
            // isn't tried again at another size: it would fail (or hang) the same way, each zoom.
            Some(e) if e.key.same_page(&key) => {
                e.seen = frame;
                match &e.error {
                    Some(err) if e.tex.is_none() => Look::Failed(err.clone()),
                    _ => Look::Pending(e.tex.as_ref().map(TextureHandle::id)),
                }
            }
            _ => Look::Pending(None),
        }
    }

    pub(super) fn state(&self, slot: Slot) -> ThumbState {
        if let Some(e) = self.entries.get(&slot) {
            if let Some(tex) = &e.tex {
                let [w, h] = tex.size();
                return ThumbState::Ready {
                    page: e.key.page,
                    width: u32::try_from(w).unwrap_or(u32::MAX),
                    height: u32::try_from(h).unwrap_or(u32::MAX),
                };
            }
            if let Some(err) = &e.error {
                return ThumbState::Failed(err.clone());
            }
        }
        if self.jobs.iter().any(|j| j.slot == slot) { ThumbState::Pending } else { ThumbState::None }
    }

    /// Once per frame, whatever shows: drop what no longer matches the list, take finished
    /// renders, start those the grid asked for and keep the textures within budget. `current`
    /// says what a thumbnail must match now (looked up, so the cost follows the files and the
    /// thumbnails kept, not the pages of an expanded file).
    pub(super) fn finish(&mut self, ctx: &egui::Context, current: &dyn Fn(Slot) -> Option<Key>) {
        let now = ctx.input(|i| i.time);
        // Reconcile with the list as it is now (undo, redo, unlock, a new range, removal): a
        // thumbnail of another page, password or file is never shown again.
        self.entries.retain(|slot, e| current(*slot).is_some_and(|k| k.same_page(&e.key)));
        // Renders nobody wants any more are let go of now (on the web, before they would render).
        let (keep, gone): (Vec<Job>, Vec<Job>) = self.jobs.drain(..).partition(|job| current(job.slot).is_some_and(|k| k.same_page(&job.key)));
        self.jobs = keep;
        self.retiring.extend(gone.into_iter().map(|job| job.pool.retire()));
        // Documents kept open past their time are closed.
        let (keep, gone): (Vec<Open>, Vec<Open>) = self.open.drain(..).partition(|o| now - o.since < KEEP_OPEN);
        self.open = keep;
        self.retiring.extend(gone.into_iter().map(|o| o.pool.retire()));

        // Finished renders, a few per frame. A pool without threads (the web) renders inside
        // `try_recv`: at most one of those per frame.
        let gpu_side = ctx.input(|i| i.max_texture_side);
        let max_side = gpu_side.min(2 * MAX_SIDE as usize);
        let (mut taken, mut uploaded, mut inline_used) = (0usize, 0usize, false);
        let mut i = 0;
        while i < self.jobs.len() {
            if taken >= UPLOADS_PER_FRAME || uploaded >= UPLOAD_BYTES_PER_FRAME {
                break;
            }
            let Some(job) = self.jobs.get(i) else { break };
            if job.pool.is_inline() {
                if inline_used {
                    i += 1;
                    continue;
                }
                inline_used = true;
            }
            // Not done yet, or not this job's answer (cannot happen with one request per pool):
            // keep waiting. A pool without threads answers on the call that renders, so there no
            // answer is one that never comes: the thumbnail fails rather than hold up the rest.
            let answer = job.pool.try_recv().filter(|out| out.request.tag == job.tag);
            if answer.is_none() && !job.pool.is_inline() {
                i += 1;
                continue;
            }
            let job = self.jobs.swap_remove(i);
            taken += 1;
            // A pool that rendered (rather than failed or hung) can keep its document open.
            let healthy = answer.as_ref().is_some_and(|out| out.error.is_none());
            let failed = |e: String| Entry { key: job.key, tex: None, error: Some(e), seen: self.frame, bytes: 0 };
            let entry = match answer {
                None => failed("the page wasn't drawn".to_owned()),
                Some(out) => match out.error {
                    Some(e) => failed(e),
                    None => match texture(
                        ctx,
                        job.slot,
                        [out.width, out.height],
                        out.rgba,
                        if job.slot == PREVIEW { gpu_side.min(PREVIEW_SIDE as usize) } else { max_side },
                    ) {
                        Ok((tex, bytes)) => {
                            uploaded = uploaded.saturating_add(bytes);
                            Entry { key: job.key, tex: Some(tex), error: None, seen: self.frame, bytes }
                        }
                        Err(e) => failed(e),
                    },
                },
            };
            self.entries.insert(job.slot, entry);
            // Its document stays open a moment for the file's next page (its thread, idle, still
            // counts); otherwise its thread (idle, or finishing a render the watchdog gave up on)
            // counts until it exits.
            if healthy && job.pool.threads() <= 1 && self.open.len() < MAX_THREADS {
                self.open.push(Open { source: job.key.source, auth: job.key.auth, pool: job.pool, since: now });
            } else {
                self.retiring.push(job.pool.retire());
            }
        }

        // New renders: the cards in view first, in grid order; one per thumbnail at a time.
        let mut wants = std::mem::take(&mut self.wants);
        wants.sort_by_key(|w| (w.slot != PREVIEW, !w.visible));
        // A pool without threads (the web) parses its document when it is made and keeps it until
        // it renders, one per frame: while one waits, nothing else starts.
        // (Nor in the frame that rendered one: one heavy step per frame on the UI thread.)
        let mut threads = if inline_used || self.jobs.iter().any(|j| j.pool.is_inline()) { MAX_THREADS } else { self.threads() };
        let mut waiting = false;
        for w in wants {
            // Wanted for the list as it is now (not changed by this frame's action); one render
            // per thumbnail at a time; and a want made earlier this frame may have just been
            // answered.
            if current(w.slot).is_none_or(|k| !k.same_page(&w.key))
                || self.jobs.iter().any(|j| j.slot == w.slot)
                || self.entries.get(&w.slot).is_some_and(|e| e.key == w.key)
            {
                continue;
            }
            // The same file, read the same way, still open from a render just done: no new read of
            // the file (and its thread is already counted). On the web, not while another waits.
            let inline_waiting = self.jobs.iter().any(|j| j.pool.is_inline());
            if let Some(k) =
                self.open.iter().position(|o| o.source == w.key.source && o.auth == w.key.auth && !(o.pool.is_inline() && inline_waiting))
            {
                let pool = self.open.swap_remove(k).pool;
                self.next_tag = self.next_tag.wrapping_add(1);
                let tag = self.next_tag;
                pool.set_queue(vec![RenderRequest { page: w.key.page, scale: w.scale, tag, ..Default::default() }]);
                if pool.is_inline() {
                    threads = MAX_THREADS;
                }
                self.jobs.push(Job { slot: w.slot, key: w.key, pool, tag });
                continue;
            }
            if threads >= MAX_THREADS {
                // A document kept open for another file gives its thread up: it exits, and this
                // starts then.
                if let Some(k) = self.open.iter().enumerate().min_by(|(_, a), (_, b)| a.since.total_cmp(&b.since)).map(|(k, _)| k) {
                    let open = self.open.swap_remove(k);
                    self.retiring.push(open.pool.retire());
                }
                waiting = true;
                break;
            }
            let config = RenderConfig { password: w.password, ..Default::default() };
            #[cfg(test)]
            let pool = if self.inline { RenderPool::new_inline(w.bytes, config) } else { RenderPool::new(w.bytes, 1, config).without_replacements() };
            #[cfg(not(test))]
            let pool = RenderPool::new(w.bytes, 1, config).without_replacements();
            self.next_tag = self.next_tag.wrapping_add(1);
            let tag = self.next_tag;
            // The request stays in the pool's queue (it is never replaced) until its answer.
            pool.set_queue(vec![RenderRequest { page: w.key.page, scale: w.scale, tag, ..Default::default() }]);
            threads = if pool.is_inline() { MAX_THREADS } else { threads.saturating_add(1) };
            self.jobs.push(Job { slot: w.slot, key: w.key, pool, tag });
        }

        // Within budget: drop the thumbnails out of view longest unseen first.
        // (The page shown large has its own: one texture, gone when it closes.)
        let mut total: usize = self.entries.iter().filter(|(f, _)| **f != PREVIEW).map(|(_, e)| e.bytes).sum();
        if total > TEXTURE_BUDGET {
            let mut old: Vec<(u64, Slot, usize)> = self
                .entries
                .iter()
                .filter(|(f, e)| **f != PREVIEW && e.seen != self.frame && e.bytes > 0)
                .map(|(f, e)| (e.seen, *f, e.bytes))
                .collect();
            old.sort_unstable();
            for (_, slot, bytes) in old {
                if total <= TEXTURE_BUDGET {
                    break;
                }
                self.entries.remove(&slot);
                total = total.saturating_sub(bytes);
            }
        }

        // The cards were drawn before these results arrived: draw them again. Keep polling while
        // renders are under way, and while a thumbnail waits for a thread to exit (not when
        // nothing waits: a thread exiting changes nothing on screen).
        if taken > 0 || self.jobs.iter().any(|j| j.pool.is_inline()) {
            ctx.request_repaint();
        } else if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if !self.open.is_empty() {
            // To close the documents kept open when their time is up.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(KEEP_OPEN));
        }
        self.frame = self.frame.wrapping_add(1);
    }
}

/// A rendered thumbnail as a texture, and its size in bytes. A raster that doesn't add up, or is
/// larger than a thumbnail can be (or than the GPU accepts), is refused, never uploaded.
fn texture(
    ctx: &egui::Context,
    slot: Slot,
    size: [u32; 2],
    pixels: pdfcraft_render::Pixels,
    max_side: usize,
) -> Result<(TextureHandle, usize), String> {
    let [w, h] = size.map(|v| v as usize);
    if w == 0 || h == 0 || w > max_side || h > max_side {
        return Err(format!("the thumbnail came back {w}×{h} pixels"));
    }
    let bytes = w.checked_mul(h).and_then(|p| p.checked_mul(4)).ok_or("the thumbnail is too large")?;
    if pixels.len() != bytes {
        return Err("the thumbnail came back incomplete".to_owned());
    }
    let image = crate::canvas::texture_image([w, h], pixels);
    Ok((ctx.load_texture(format!("combine-thumb-{}-{}", slot.0, slot.1), image, egui::TextureOptions::LINEAR), bytes))
}
