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
        #[cfg(test)]
        if allocation_probe::refuse(width, height) {
            return None;
        }
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

/// Observe the real allocator boundary without allocating a large raster in tests.
#[cfg(test)]
pub(crate) mod allocation_probe {
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct Attempts {
        pub calls: u8,
        pub first: Option<(usize, usize)>,
    }

    thread_local! {
        static ACTIVE: Cell<Option<Attempts>> = const { Cell::new(None) };
    }

    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.set(None);
        }
    }

    pub(crate) fn observe<T>(f: impl FnOnce() -> T) -> (T, Attempts) {
        assert!(ACTIVE.get().is_none(), "nested raster allocation probe");
        let _reset = Reset;
        ACTIVE.set(Some(Attempts::default()));
        let result = f();
        let attempts = ACTIVE.get().expect("active raster allocation probe");
        (result, attempts)
    }

    pub(super) fn refuse(width: usize, height: usize) -> bool {
        let Some(mut attempts) = ACTIVE.get() else { return false };
        attempts.calls = attempts.calls.checked_add(1).expect("bounded allocation probe calls");
        attempts.first.get_or_insert((width, height));
        ACTIVE.set(Some(attempts));
        true
    }

    #[test]
    fn raster_allocation_probe_restores_after_unwind() {
        let panic = std::panic::catch_unwind(|| {
            observe(|| {
                assert!(super::Pixels::zeroed(2, 3).is_none());
                panic!("intentional test-harness unwind outside rendering");
            });
        });
        assert!(panic.is_err());
        assert!(ACTIVE.get().is_none());
        assert_eq!(super::Pixels::zeroed(2, 3).expect("probe restored").len(), 24);
        let (pixels, attempts) = observe(|| super::Pixels::zeroed(3, 2));
        assert!(pixels.is_none());
        assert_eq!(attempts, Attempts { calls: 1, first: Some((3, 2)) });
        assert!(ACTIVE.get().is_none());
    }
}
