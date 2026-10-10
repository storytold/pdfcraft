//! Pending work and bounded completed results. Pixels already being rendered are outside
//! this storage budget; at most one oversized completed result is admitted by itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
#[cfg(any(test, not(target_arch = "wasm32")))]
use std::time::Duration;

use super::{RenderRequest, RenderedPage, RequestKind, Tile, lock};

const RESULT_BYTES: usize = 32 * 1024 * 1024;
const RESULT_COUNT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    page: usize,
    kind: RequestKind,
    tile: Option<Tile>,
    scale: u32,
    tag: u64,
}

impl From<RenderRequest> for Key {
    fn from(r: RenderRequest) -> Self {
        Self { page: r.page, kind: r.kind, tile: r.tile, scale: r.scale.to_bits(), tag: r.tag }
    }
}

struct Completed {
    page: RenderedPage,
    bytes: usize,
    valid: Option<Arc<AtomicBool>>,
}

fn valid(token: &Option<Arc<AtomicBool>>) -> bool {
    token.as_ref().is_none_or(|v| v.load(Ordering::Acquire))
}

#[cfg(any(test, not(target_arch = "wasm32")))]
fn result_bytes(page: &RenderedPage) -> usize {
    let mut bytes = std::mem::size_of::<RenderedPage>().saturating_add(page.rgba.byte_capacity());
    if let Some(error) = &page.error {
        bytes = bytes.saturating_add(error.capacity());
    }
    if let Some(text) = &page.text {
        bytes = bytes
            .saturating_add(std::mem::size_of_val(&**text))
            .saturating_add(text.glyphs.capacity().saturating_mul(std::mem::size_of::<crate::TextGlyph>()))
            .saturating_add(text.line_of.capacity().saturating_mul(std::mem::size_of::<u32>()))
            .saturating_add(text.space_before.capacity());
        for glyph in &text.glyphs {
            bytes = bytes.saturating_add(glyph.text.capacity());
        }
    }
    bytes
}

#[derive(Default)]
struct State {
    /// Most urgent last, so a worker can pop in constant time.
    pending: Vec<RenderRequest>,
    /// Current consumer priority: zero is the most urgent visible request.
    desired: HashMap<Key, usize>,
    active: HashSet<Key>,
    results: VecDeque<Completed>,
    bytes: usize,
    closed: bool,
}

impl State {
    fn retry(&mut self, req: RenderRequest) {
        let key = Key::from(req);
        self.active.remove(&key);
        if !self.closed
            && let Some(&priority) = self.desired.get(&key)
            && !self.pending.iter().any(|r| Key::from(*r) == key)
        {
            // Preserve reverse demand order: the pop end belongs to visible work, even
            // when an older background completion needs retrying after parser retirement.
            let index = self.pending.partition_point(|r| self.desired.get(&Key::from(*r)).copied().unwrap_or(usize::MAX) > priority);
            self.pending.insert(index, req);
        }
    }

    fn take(&mut self) -> Option<RenderedPage> {
        while !self.results.is_empty() {
            // Completion order can differ from demand order, especially after a page jump
            // while search results are buffered. Look up current priority at delivery so
            // arrivals after replace() also put visible pixels ahead of background work.
            let index = self
                .results
                .iter()
                .enumerate()
                .min_by_key(|(_, r)| self.desired.get(&Key::from(r.page.request)).copied().unwrap_or(usize::MAX))
                .map(|(index, _)| index)?;
            let done = self.results.remove(index)?;
            self.bytes = self.bytes.saturating_sub(done.bytes);
            let key = Key::from(done.page.request);
            if !self.desired.contains_key(&key) {
                continue;
            }
            if !valid(&done.valid) {
                self.retry(done.page.request);
                continue;
            }
            return Some(done.page);
        }
        None
    }
}

pub(super) struct WorkQueue {
    state: Mutex<State>,
    changed: Condvar,
    max_bytes: usize,
    max_count: usize,
}

impl Default for WorkQueue {
    fn default() -> Self {
        Self { state: Mutex::default(), changed: Condvar::new(), max_bytes: RESULT_BYTES, max_count: RESULT_COUNT }
    }
}

impl WorkQueue {
    pub(super) fn replace(&self, requests: Vec<RenderRequest>) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        let mut desired = HashSet::with_capacity(requests.len());
        let requests: Vec<_> = requests.into_iter().filter(|r| desired.insert(Key::from(*r))).collect();
        state.results.retain(|r| desired.contains(&Key::from(r.page.request)) && valid(&r.valid));
        state.bytes = state.results.iter().fold(0usize, |n, r| n.saturating_add(r.bytes));
        let completed: HashSet<_> = state.results.iter().map(|r| Key::from(r.page.request)).collect();
        state.desired = requests.iter().enumerate().map(|(priority, req)| (Key::from(*req), priority)).collect();
        state.pending =
            requests.into_iter().rev().filter(|r| !state.active.contains(&Key::from(*r)) && !completed.contains(&Key::from(*r))).collect();
        drop(state);
        self.changed.notify_all();
    }

    pub(super) fn pop(&self) -> Option<RenderRequest> {
        let mut state = lock(&self.state);
        if state.closed || state.results.len() >= self.max_count || state.bytes >= self.max_bytes {
            return None;
        }
        let req = state.pending.pop()?;
        state.active.insert(Key::from(req));
        Some(req)
    }

    /// Skip an obsolete request before entering the next expensive stage. Work already
    /// executing cannot be interrupted, but cancelled parsing need not start a raster.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn cancel_obsolete(&self, req: RenderRequest) -> bool {
        let mut state = lock(&self.state);
        let key = Key::from(req);
        if state.closed || !state.desired.contains_key(&key) {
            state.active.remove(&key);
            true
        } else {
            false
        }
    }

    /// Inline rendering has no completed buffer. Remove its in-flight identity on return.
    pub(super) fn finish_inline(&self, req: RenderRequest) {
        lock(&self.state).active.remove(&Key::from(req));
    }

    #[cfg(test)]
    pub(super) fn send(&self, page: RenderedPage) -> Result<(), ()> {
        self.send_valid(page, None)
    }

    /// A failed shared parser invalidates its token. Such results are retried by a worker
    /// using the pool's fallback parser, including when failure occurs after enqueueing.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn send_valid(&self, page: RenderedPage, token: Option<Arc<AtomicBool>>) -> Result<(), ()> {
        let key = Key::from(page.request);
        let bytes = result_bytes(&page);
        let mut state = lock(&self.state);
        loop {
            if state.closed {
                state.active.remove(&key);
                return Err(());
            }
            if !state.desired.contains_key(&key) {
                state.active.remove(&key);
                return Ok(());
            }
            if !valid(&token) {
                state.retry(page.request);
                self.changed.notify_all();
                return Ok(());
            }
            if state.results.len() < self.max_count && (state.results.is_empty() || state.bytes.saturating_add(bytes) <= self.max_bytes) {
                state.active.remove(&key);
                state.bytes = state.bytes.saturating_add(bytes);
                state.results.push_back(Completed { page, bytes, valid: token });
                drop(state);
                self.changed.notify_all();
                return Ok(());
            }
            // The parser validity token can change independently of this queue. Bound the
            // wait so failed generations are released even while the consumer is paused.
            state = self.changed.wait_timeout(state, Duration::from_millis(25)).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// A shared-parser timeout may belong to another page holding a cold-load gate.
    /// Retry still-current demand after the pool switches permanently to private parsers.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn retry(&self, request: RenderRequest) {
        lock(&self.state).retry(request);
        self.changed.notify_all();
    }

    /// Watchdog errors contain no page payload. They must never block the UI thread behind
    /// a full result buffer; evict/retry one completed request if necessary.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn abandon(&self, page: RenderedPage) {
        let mut state = lock(&self.state);
        let req = page.request;
        state.active.remove(&Key::from(req));
        if state.closed || !state.desired.contains_key(&Key::from(req)) {
            return;
        }
        let bytes = result_bytes(&page);
        while !state.results.is_empty() && (state.results.len() >= self.max_count || state.bytes.saturating_add(bytes) > self.max_bytes) {
            if let Some(old) = state.results.pop_back() {
                state.bytes = state.bytes.saturating_sub(old.bytes);
                state.retry(old.page.request);
            }
        }
        state.bytes = state.bytes.saturating_add(bytes);
        state.results.push_front(Completed { page, bytes, valid: None });
        drop(state);
        self.changed.notify_all();
    }

    fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        state.pending.clear();
        state.desired.clear();
        state.results.clear();
        state.bytes = 0;
        drop(state);
        self.changed.notify_all();
    }
}

pub(super) struct ResultReceiver(pub(super) Arc<WorkQueue>);

impl ResultReceiver {
    pub(super) fn try_recv(&self) -> Result<RenderedPage, std::sync::mpsc::TryRecvError> {
        let mut state = lock(&self.0.state);
        let result = state.take();
        let closed = state.closed;
        drop(state);
        self.0.changed.notify_all();
        result.ok_or(if closed { std::sync::mpsc::TryRecvError::Disconnected } else { std::sync::mpsc::TryRecvError::Empty })
    }

    #[cfg(test)]
    pub(super) fn recv_timeout(&self, timeout: Duration) -> Result<RenderedPage, std::sync::mpsc::RecvTimeoutError> {
        let start = std::time::Instant::now();
        loop {
            if let Ok(page) = self.try_recv() {
                return Ok(page);
            }
            if start.elapsed() >= timeout {
                return Err(std::sync::mpsc::RecvTimeoutError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for ResultReceiver {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pixels;

    fn req(page: usize) -> RenderRequest {
        RenderRequest { page, scale: 1.0, ..Default::default() }
    }

    fn page(req: RenderRequest, bytes: usize) -> RenderedPage {
        RenderedPage {
            request: req,
            width: 1,
            height: 1,
            rgba: Pixels::from(vec![0u32; bytes / 4]),
            error: None,
            text: None,
            warnings: Vec::new(),
            millis: 0,
        }
    }

    #[test]
    fn replacing_demands_deduplicates_active_and_completed_then_allows_eviction_retry() {
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        q.replace(vec![req(0), req(0), req(1)]);
        assert_eq!(q.pop(), Some(req(0)));
        q.replace(vec![req(0), req(1)]);
        assert_eq!(q.pop(), Some(req(1)));
        q.send(page(req(0), 4)).unwrap();
        q.replace(vec![req(0), req(1)]);
        assert!(q.pop().is_none());
        assert_eq!(recv.try_recv().unwrap().request, req(0));
        q.replace(vec![req(1)]);
        q.send(page(req(1), 4)).unwrap();
        q.replace(vec![req(0)]);
        assert!(recv.try_recv().is_err(), "obsolete completion released before delivery");
        assert_eq!(q.pop(), Some(req(0)), "evicted UI textures can request the same key again");
    }

    #[test]
    fn request_identity_preserves_distinct_work_and_deduplicates_nan_scales() {
        // Keep #400's bitwise scale identity: NaN must match itself while rendering and
        // awaiting delivery, without conflating different pages, kinds, tiles or tags.
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        let base = RenderRequest { scale: f32::from_bits(0x7fc0_0001), ..req(0) };
        let requests = [
            base,
            RenderRequest { page: 1, ..base },
            RenderRequest { kind: RequestKind::Text, ..base },
            RenderRequest { tile: Some(Tile { x: 0, y: 0, w: 1, h: 1 }), ..base },
            RenderRequest { scale: f32::from_bits(0x7fc0_0002), ..base },
            RenderRequest { tag: 1, ..base },
        ];
        let repeated = || requests.into_iter().flat_map(|r| [r, r]).collect();
        q.replace(repeated());
        for request in requests {
            assert_eq!(q.pop().map(Key::from), Some(Key::from(request)));
        }
        q.replace(repeated());
        assert!(q.pop().is_none(), "active requests are not rendered twice");
        for request in requests {
            q.send(page(request, 4)).unwrap();
        }
        q.replace(repeated());
        assert!(q.pop().is_none(), "completed requests remain deduplicated until delivery");
        for request in requests {
            assert_eq!(Key::from(recv.try_recv().unwrap().request), Key::from(request));
        }
        assert!(recv.try_recv().is_err());
        q.replace(vec![base]);
        assert_eq!(q.pop().map(Key::from), Some(Key::from(base)), "delivery releases even a NaN request for a later retry");
    }

    #[test]
    fn returning_to_active_work_reuses_it_but_cancellation_allows_a_fresh_request() {
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        q.replace(vec![req(0)]);
        assert_eq!(q.pop(), Some(req(0)));
        q.replace(Vec::new());
        q.replace(vec![req(0)]);
        assert!(q.pop().is_none(), "returning to a page reuses its in-flight render");
        q.send(page(req(0), 4)).unwrap();
        assert_eq!(recv.try_recv().unwrap().request, req(0));
        assert!(recv.try_recv().is_err());

        q.replace(vec![req(0)]);
        assert_eq!(q.pop(), Some(req(0)));
        q.replace(Vec::new());
        assert!(q.cancel_obsolete(req(0)), "a worker can cancel before rasterization");
        q.replace(vec![req(0)]);
        assert_eq!(q.pop(), Some(req(0)), "a cancelled render must not suppress later demand");
    }

    #[test]
    fn delivery_uses_current_priority_for_buffered_and_later_completions() {
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        let text = RenderRequest { kind: RequestKind::Text, ..req(0) };
        let thumbnail = RenderRequest { tag: 1 << 63, ..req(1) };
        let visible = req(2);
        q.replace(vec![text, thumbnail, visible]);
        for _ in 0..3 {
            assert!(q.pop().is_some());
        }
        q.send(page(text, 4)).unwrap();
        q.send(page(thumbnail, 4)).unwrap();
        // A newly visible main page takes priority even though the other work is still
        // desired, already completed, and was requested first in an earlier frame.
        q.replace(vec![visible, thumbnail, text]);
        q.send(page(visible, 4)).unwrap();
        assert_eq!(recv.try_recv().unwrap().request, visible);
        assert_eq!(recv.try_recv().unwrap().request, thumbnail);
        assert_eq!(recv.try_recv().unwrap().request, text);
        assert!(recv.try_recv().is_err());
        assert_eq!(lock(&q.state).bytes, 0);
        assert!(q.pop().is_none(), "priority changes must not duplicate completed work");
    }

    #[test]
    fn invalid_background_completion_retries_after_pending_visible_work() {
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        let token = Arc::new(AtomicBool::new(true));
        let text = RenderRequest { kind: RequestKind::Text, ..req(0) };
        let visible = req(1);
        q.replace(vec![text]);
        assert_eq!(q.pop(), Some(text));
        q.send_valid(page(text, 4), Some(token.clone())).unwrap();
        q.replace(vec![visible, text]);
        token.store(false, Ordering::Release);
        assert!(recv.try_recv().is_err());
        assert_eq!(q.pop(), Some(visible));
        assert_eq!(q.pop(), Some(text));
        assert!(q.pop().is_none());
    }

    #[test]
    fn watchdog_eviction_retries_background_work_after_visible_work() {
        let q = Arc::new(WorkQueue { max_count: 1, ..Default::default() });
        let recv = ResultReceiver(q.clone());
        let text = RenderRequest { kind: RequestKind::Text, ..req(0) };
        let stuck = req(1);
        let visible = req(2);
        q.replace(vec![text, stuck]);
        assert_eq!(q.pop(), Some(text));
        assert_eq!(q.pop(), Some(stuck));
        q.send(page(text, 4)).unwrap();
        q.replace(vec![visible, text, stuck]);
        let mut error = page(stuck, 0);
        error.error = Some("watchdog".into());
        q.abandon(error);
        assert_eq!(recv.try_recv().unwrap().request, stuck);
        assert_eq!(q.pop(), Some(visible));
        assert_eq!(q.pop(), Some(text));
        assert!(q.pop().is_none());
    }

    #[test]
    fn stalled_consumer_bounds_storage_and_drop_unblocks_publishers() {
        let q = Arc::new(WorkQueue { max_bytes: 1024, max_count: 2, ..Default::default() });
        let recv = ResultReceiver(q.clone());
        q.replace(vec![req(0), req(1), req(2)]);
        assert!(q.pop().is_some());
        assert!(q.pop().is_some());
        q.send(page(req(0), 700)).unwrap();
        let blocked = q.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(blocked.send(page(req(1), 700))).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        assert!(lock(&q.state).bytes <= 1024);
        drop(recv);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        worker.join().unwrap();
    }

    #[test]
    fn oversized_completion_is_admitted_alone_and_cancellation_releases_waiters() {
        let q = Arc::new(WorkQueue { max_bytes: 512, max_count: 2, ..Default::default() });
        let recv = ResultReceiver(q.clone());
        q.replace(vec![req(0), req(1)]);
        q.pop();
        q.pop();
        q.send(page(req(0), 1024)).unwrap();
        let producer = q.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(producer.send(page(req(1), 1024))).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(lock(&q.state).results.len(), 1);
        q.replace(Vec::new());
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
        worker.join().unwrap();
        assert!(recv.try_recv().is_err());
        assert_eq!(lock(&q.state).bytes, 0);
    }

    #[test]
    fn invalid_parser_generations_are_retried_before_or_after_publication() {
        let q = Arc::new(WorkQueue::default());
        let recv = ResultReceiver(q.clone());
        let token = Arc::new(AtomicBool::new(true));
        q.replace(vec![req(0)]);
        q.pop();
        q.send_valid(page(req(0), 4), Some(token.clone())).unwrap();
        token.store(false, Ordering::Release);
        assert!(recv.try_recv().is_err());
        assert_eq!(q.pop(), Some(req(0)));
        q.send_valid(page(req(0), 4), Some(token)).unwrap();
        assert_eq!(q.pop(), Some(req(0)), "invalid pre-publication result retries too");
        q.send(page(req(0), 4)).unwrap();
        assert!(recv.try_recv().unwrap().error.is_none());
    }
}
