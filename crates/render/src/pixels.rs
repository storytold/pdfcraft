//! Raster pixels in a buffer a GUI can take over as texture data.

use std::ops::Deref;

/// A raster's premultiplied RGBA8 bytes, row-major, kept in 4-byte words: one word per pixel,
/// with the bytes in R, G, B, A order in memory. The words make the buffer aligned to whole
/// pixels, so it can become a GUI's pixel array (egui's `Color32`) without copying. Read it as
/// bytes through `Deref`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Pixels(Vec<u32>);

impl Pixels {
    /// Zeroed pixels for a `width` × `height` raster; `None` when the size overflows.
    pub(crate) fn zeroed(width: usize, height: usize) -> Option<Self> {
        Some(Self(vec![0; width.checked_mul(height)?]))
    }

    /// The bytes, for the rasterizer to write into.
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        bytemuck::cast_slice_mut(&mut self.0)
    }

    /// The buffer itself: one word per pixel, its bytes in R, G, B, A memory order.
    pub fn into_words(self) -> Vec<u32> {
        self.0
    }

    /// Bytes the buffer holds allocated (its capacity), for memory budgets.
    pub(crate) fn byte_capacity(&self) -> usize {
        self.0.capacity().saturating_mul(4)
    }
}

/// Pixels from words, one per pixel, its bytes in R, G, B, A memory order.
impl From<Vec<u32>> for Pixels {
    fn from(words: Vec<u32>) -> Self {
        Self(words)
    }
}

impl Deref for Pixels {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        bytemuck::cast_slice(&self.0)
    }
}

impl AsRef<[u8]> for Pixels {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({} bytes)", self.len())
    }
}
