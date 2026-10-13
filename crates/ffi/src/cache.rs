//! Byte-budgeted memory LRU tile cache and thumbnail disk cache.

use crate::render::{FfiRenderRequest, FfiRenderResult};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, create_dir_all};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Cache key uniquely identifying a rendered page tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCacheKey {
    pub doc_id: u64,
    pub doc_version: u64,
    pub page: u32,
    pub tile_x: u32,
    pub tile_y: u32,
    pub tile_w: u32,
    pub tile_h: u32,
    pub scale_bits: u32,
    pub reading_colors_mode: u8,
    pub reading_colors_text: u32,
    pub reading_colors_bg: u32,
    pub hide_comments: bool,
    pub preview: bool,
}

impl TileCacheKey {
    pub fn from_request(doc_id: u64, doc_version: u64, req: &FfiRenderRequest) -> Self {
        let (tx, ty, tw, th) = match req.tile {
            Some(t) => (t.x, t.y, t.width, t.height),
            None => (0, 0, 0, 0),
        };
        let (rc_mode, rc_text, rc_bg) = match req.reading_colors {
            Some(rc) => (rc.mode as u8 + 1, rc.custom_text_rgb.unwrap_or(0), rc.custom_background_rgb.unwrap_or(0)),
            None => (0, 0, 0),
        };

        Self {
            doc_id,
            doc_version,
            page: req.page,
            tile_x: tx,
            tile_y: ty,
            tile_w: tw,
            tile_h: th,
            scale_bits: req.scale.to_bits(),
            reading_colors_mode: rc_mode,
            reading_colors_text: rc_text,
            reading_colors_bg: rc_bg,
            hide_comments: req.hide_comments,
            preview: req.preview,
        }
    }
}

/// Thread-safe byte-budgeted LRU cache for rendered raster tiles.
pub struct ByteBudgetedTileCache {
    max_bytes: usize,
    current_bytes: usize,
    entries: HashMap<TileCacheKey, FfiRenderResult>,
    order: VecDeque<TileCacheKey>,
}

impl ByteBudgetedTileCache {
    /// Creates a new tile cache with the given maximum byte budget (default: 64 MiB).
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes, current_bytes: 0, entries: HashMap::new(), order: VecDeque::new() }
    }

    /// Looks up a cached tile and marks it as most recently used.
    pub fn get(&mut self, key: &TileCacheKey) -> Option<FfiRenderResult> {
        if let Some(res) = self.entries.get(key) {
            let res = res.clone();
            // Move key to front of LRU order
            if let Some(idx) = self.order.iter().position(|k| k == key) {
                let k = self.order.remove(idx).unwrap_or(*key);
                self.order.push_front(k);
            }
            Some(res)
        } else {
            None
        }
    }

    /// Inserts a newly rendered tile, evicting oldest entries if the byte budget is exceeded.
    pub fn insert(&mut self, key: TileCacheKey, result: FfiRenderResult) {
        let entry_bytes = result.pixels.len();
        if entry_bytes > self.max_bytes {
            // A single tile larger than total cache budget is returned but not cached
            return;
        }

        // If replacing an existing key, subtract its byte size first
        if let Some(old) = self.entries.remove(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.pixels.len());
            if let Some(idx) = self.order.iter().position(|k| k == &key) {
                self.order.remove(idx);
            }
        }

        // Evict LRU entries until we fit within budget
        while self.current_bytes + entry_bytes > self.max_bytes && !self.order.is_empty() {
            if let Some(lru_key) = self.order.pop_back()
                && let Some(evicted) = self.entries.remove(&lru_key)
            {
                self.current_bytes = self.current_bytes.saturating_sub(evicted.pixels.len());
            }
        }

        self.current_bytes += entry_bytes;
        self.entries.insert(key, result);
        self.order.push_front(key);
    }

    /// Evicts all cached tiles for a specific closed document.
    pub fn evict_for_doc(&mut self, doc_id: u64) {
        self.order.retain(|k| {
            if k.doc_id == doc_id {
                if let Some(evicted) = self.entries.remove(k) {
                    self.current_bytes = self.current_bytes.saturating_sub(evicted.pixels.len());
                }
                false
            } else {
                true
            }
        });
    }

    /// Evicts all cached tiles for a specific page of a document.
    pub fn evict_page(&mut self, doc_id: u64, page: u32) {
        self.order.retain(|k| {
            if k.doc_id == doc_id && k.page == page {
                if let Some(evicted) = self.entries.remove(k) {
                    self.current_bytes = self.current_bytes.saturating_sub(evicted.pixels.len());
                }
                false
            } else {
                true
            }
        });
    }

    /// Evicts all cached tiles (used on OS memory warnings).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.current_bytes = 0;
    }

    /// Maximum configured byte budget.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Trims cache entries until total memory is under `target_bytes`.
    pub fn trim_to_bytes(&mut self, target_bytes: usize) {
        while self.current_bytes > target_bytes && !self.order.is_empty() {
            if let Some(lru_key) = self.order.pop_back()
                && let Some(evicted) = self.entries.remove(&lru_key)
            {
                self.current_bytes = self.current_bytes.saturating_sub(evicted.pixels.len());
            }
        }
    }

    /// Current memory usage in bytes.
    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Total number of cached tiles.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// On-disk cache for low-detail page thumbnails.
pub struct ThumbnailDiskCache {
    cache_dir: PathBuf,
}

impl ThumbnailDiskCache {
    pub fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self { cache_dir: dir.as_ref().to_path_buf() }
    }

    fn file_path(&self, doc_fingerprint: &str, page: u32, max_dim: u32) -> PathBuf {
        let filename = format!("{doc_fingerprint}_p{page}_{max_dim}px.thumb");
        self.cache_dir.join(filename)
    }

    /// Reads a cached thumbnail from disk if available.
    pub fn get(&self, doc_fingerprint: &str, page: u32, max_dim: u32) -> Option<FfiRenderResult> {
        let path = self.file_path(doc_fingerprint, page, max_dim);
        if !path.exists() {
            return None;
        }

        let mut file = File::open(&path).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let width = u32::from_le_bytes(header[0..4].try_into().ok()?);
        let height = u32::from_le_bytes(header[4..8].try_into().ok()?);

        let mut pixels = Vec::new();
        file.read_to_end(&mut pixels).ok()?;
        if pixels.len() != (width as usize * height as usize * 4) {
            return None;
        }

        Some(FfiRenderResult { page, width, height, tag: 0, pixels, error: None, render_time_ms: 0 })
    }

    /// Persists a thumbnail to disk atomically via a temporary file.
    pub fn put(&self, doc_fingerprint: &str, page: u32, max_dim: u32, result: &FfiRenderResult) -> std::io::Result<()> {
        if result.pixels.is_empty() || result.width == 0 || result.height == 0 {
            return Ok(());
        }
        create_dir_all(&self.cache_dir)?;
        let path = self.file_path(doc_fingerprint, page, max_dim);
        let tmp_path = path.with_extension("tmp");

        let mut file = File::create(&tmp_path)?;
        file.write_all(&result.width.to_le_bytes())?;
        file.write_all(&result.height.to_le_bytes())?;
        file.write_all(&result.pixels)?;
        file.sync_all()?;

        std::fs::rename(tmp_path, path)?;
        Ok(())
    }

    /// Removes all cached thumbnails from disk.
    pub fn clear(&self) -> std::io::Result<()> {
        if self.cache_dir.exists() {
            std::fs::remove_dir_all(&self.cache_dir)?;
            create_dir_all(&self.cache_dir)?;
        }
        Ok(())
    }
}

/// Global or engine-scoped cache state wrapped in Mutex.
pub struct EngineCacheState {
    pub tile_cache: Mutex<ByteBudgetedTileCache>,
    pub thumb_cache: Option<ThumbnailDiskCache>,
}

impl EngineCacheState {
    pub fn new(max_tile_bytes: usize, thumb_dir: Option<PathBuf>) -> Self {
        Self { tile_cache: Mutex::new(ByteBudgetedTileCache::new(max_tile_bytes)), thumb_cache: thumb_dir.map(ThumbnailDiskCache::new) }
    }
}
