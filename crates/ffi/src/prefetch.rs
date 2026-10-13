//! Direction-aware prefetch planner and request coalescing.

use crate::cache::TileCacheKey;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Direction of user scroll motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiScrollDirection {
    Idle,
    Forward,
    Backward,
}

/// Priority tier for render and prefetch scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, uniffi::Enum)]
pub enum FfiPrefetchPriority {
    Visible,
    Adjacent,
    Thumbnail,
}

/// A scheduled prefetch target page and rendering tier.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiPrefetchPlan {
    /// 0-based page index to render or prefetch.
    pub page: u32,
    /// Scheduling priority tier (Visible > Adjacent > Thumbnail).
    pub priority: FfiPrefetchPriority,
    /// Whether to generate a low-detail preview (for fast scroll/thumbnails).
    pub preview: bool,
}

/// Computes a prioritized list of pages to prefetch based on current position and scroll direction.
pub fn plan_prefetch_pages(current_page: u32, total_pages: u32, direction: FfiScrollDirection, lookahead: u32) -> Vec<FfiPrefetchPlan> {
    if total_pages == 0 {
        return Vec::new();
    }
    let cur = current_page.min(total_pages.saturating_sub(1));
    let mut plans = Vec::new();

    // 1. Current visible page has top priority
    plans.push(FfiPrefetchPlan { page: cur, priority: FfiPrefetchPriority::Visible, preview: false });

    let lookahead = lookahead.max(1);

    match direction {
        FfiScrollDirection::Forward => {
            // Forward pages: Adjacent priority (full quality)
            let max_ahead = (cur + lookahead).min(total_pages - 1);
            for p in (cur + 1)..=max_ahead {
                plans.push(FfiPrefetchPlan { page: p, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
            // 1 page behind in case user reverses slightly
            if cur > 0 {
                plans.push(FfiPrefetchPlan { page: cur - 1, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
            // Further ahead pages: Thumbnail priority (low-detail preview)
            let max_thumb = (max_ahead + 4).min(total_pages - 1);
            for p in (max_ahead + 1)..=max_thumb {
                plans.push(FfiPrefetchPlan { page: p, priority: FfiPrefetchPriority::Thumbnail, preview: true });
            }
        }
        FfiScrollDirection::Backward => {
            // Backward pages: Adjacent priority (full quality)
            let min_behind = cur.saturating_sub(lookahead);
            for p in (min_behind..cur).rev() {
                plans.push(FfiPrefetchPlan { page: p, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
            // 1 page ahead in case user bounces
            if cur + 1 < total_pages {
                plans.push(FfiPrefetchPlan { page: cur + 1, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
            // Further behind pages: Thumbnail priority (low-detail preview)
            let min_thumb = min_behind.saturating_sub(4);
            for p in (min_thumb..min_behind).rev() {
                plans.push(FfiPrefetchPlan { page: p, priority: FfiPrefetchPriority::Thumbnail, preview: true });
            }
        }
        FfiScrollDirection::Idle => {
            // Stationary: 1 page ahead, 1 page behind
            if cur + 1 < total_pages {
                plans.push(FfiPrefetchPlan { page: cur + 1, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
            if cur > 0 {
                plans.push(FfiPrefetchPlan { page: cur - 1, priority: FfiPrefetchPriority::Adjacent, preview: false });
            }
        }
    }

    plans
}

/// Tracks in-flight render requests to coalesce redundant duplicate requests
/// and manage generation tag counter for scroll/zoom invalidation.
#[derive(Debug, Default)]
pub struct RequestCoalescer {
    current_generation: AtomicU64,
    in_flight: Mutex<HashSet<TileCacheKey>>,
}

impl RequestCoalescer {
    pub fn new() -> Self {
        Self { current_generation: AtomicU64::new(1), in_flight: Mutex::new(HashSet::new()) }
    }

    /// Advances the generation tag (e.g. on fast scroll or zoom) and returns the new tag.
    pub fn next_generation(&self) -> u64 {
        self.current_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Returns the current active generation tag.
    pub fn current_generation(&self) -> u64 {
        self.current_generation.load(Ordering::SeqCst)
    }

    /// Attempts to register an in-flight render request.
    /// Returns `true` if this caller is the first to request this key and should render it,
    /// or `false` if an identical render is already in flight (coalesced).
    pub fn try_acquire(&self, key: &TileCacheKey) -> bool {
        if let Ok(mut set) = self.in_flight.lock() { set.insert(*key) } else { true }
    }

    /// Releases an in-flight key once rendering or cancellation completes.
    pub fn release(&self, key: &TileCacheKey) {
        if let Ok(mut set) = self.in_flight.lock() {
            set.remove(key);
        }
    }

    /// Returns the number of currently in-flight requests.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.lock().map(|s| s.len()).unwrap_or(0)
    }

    /// Clears all in-flight trackers (e.g. on document close or memory trim).
    pub fn clear(&self) {
        if let Ok(mut set) = self.in_flight.lock() {
            set.clear();
        }
    }
}
