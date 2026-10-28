//! Performance Budget Watcher (<50ms Page Rasterization, <150MB Heap Budget).

#[derive(Debug, Clone)]
pub struct PlatformBudgetLimits {
    pub max_page_render_time_ms: u64,
    pub max_heap_memory_mb: usize,
}

impl Default for PlatformBudgetLimits {
    fn default() -> Self {
        Self {
            max_page_render_time_ms: 50, // 50ms budget for interactive 60fps response
            max_heap_memory_mb: 150,     // 150MB baseline budget
        }
    }
}

pub struct PerformanceBudgetWatcher {
    pub limits: PlatformBudgetLimits,
}

impl Default for PerformanceBudgetWatcher {
    fn default() -> Self {
        Self { limits: PlatformBudgetLimits::default() }
    }
}

impl PerformanceBudgetWatcher {
    pub fn new(limits: PlatformBudgetLimits) -> Self {
        Self { limits }
    }

    /// Verifies if a render duration meets the performance budget.
    pub fn check_render_duration(&self, duration_ms: u64) -> bool {
        duration_ms <= self.limits.max_page_render_time_ms
    }

    /// Verifies if heap usage is within memory budget.
    pub fn check_heap_usage(&self, current_heap_mb: usize) -> bool {
        current_heap_mb <= self.limits.max_heap_memory_mb
    }
}
