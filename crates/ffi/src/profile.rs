//! Runtime profile and budget configuration for mobile devices.

/// Runtime configuration for threading, memory limits, and render budgets.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileProfile {
    /// Number of worker threads for background rendering (default: 2 on mobile).
    pub worker_threads: u32,
    /// Maximum raster pixels for a single rendered page or tile (default: 64,000,000).
    pub max_pixels: u32,
    /// Maximum single side of a raster in pixels (default: 8192).
    pub max_side: u32,
    /// Decoded image cache budget in bytes (default: 64 MiB).
    pub image_cache_bytes: u64,
    /// Per-stream decode limit in bytes (default: 256 MiB).
    pub max_decoded_stream: u64,
    /// Per-page decoded content budget in bytes (default: 512 MiB).
    pub max_page_content: u64,
    /// Preferred tile dimension in device pixels (default: 512).
    pub tile_size: u32,
}

impl Default for MobileProfile {
    fn default() -> Self {
        Self {
            worker_threads: 2,
            max_pixels: 64_000_000,
            max_side: 8192,
            image_cache_bytes: 64 * 1024 * 1024,
            max_decoded_stream: 256 * 1024 * 1024,
            max_page_content: 512 * 1024 * 1024,
            tile_size: 512,
        }
    }
}

impl MobileProfile {
    /// Creates a conservative profile tuned for low-memory devices (R2 class, ~4 GB RAM).
    pub fn low_memory() -> Self {
        Self {
            worker_threads: 1,
            max_pixels: 16_000_000,
            max_side: 4096,
            image_cache_bytes: 16 * 1024 * 1024,
            max_decoded_stream: 64 * 1024 * 1024,
            max_page_content: 128 * 1024 * 1024,
            tile_size: 512,
        }
    }
}

#[uniffi::export]
pub fn default_mobile_profile() -> MobileProfile {
    MobileProfile::default()
}

#[uniffi::export]
pub fn low_memory_mobile_profile() -> MobileProfile {
    MobileProfile::low_memory()
}
