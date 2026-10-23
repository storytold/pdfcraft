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

    /// Invert all color channels in place.
    ///
    /// In premultiplied RGBA8, each color channel is bounded by alpha `a` (`0 <= c <= a`).
    /// Inverting the color while preserving alpha produces `a - c`. For opaque pixels
    /// (`a == 255`), this simplifies to `255 - c`. Transparent pixels (`a == 0`) remain
    /// transparent. Inverting twice is an exact identity.
    pub fn invert(&mut self) {
        for chunk in self.bytes_mut().as_chunks_mut::<4>().0 {
            let a = chunk[3];
            chunk[0] = a.saturating_sub(chunk[0]);
            chunk[1] = a.saturating_sub(chunk[1]);
            chunk[2] = a.saturating_sub(chunk[2]);
        }
    }

    /// Invert with a chosen palette: a white page pixel becomes `palette.paper` and a black one
    /// `palette.ink`, with every other value in between per channel (so colour images keep their
    /// hues, inverted). The default palette (black paper, white ink) is exactly [`Self::invert`].
    pub fn recolor(&mut self, palette: InvertPalette) {
        if palette == InvertPalette::default() {
            return self.invert();
        }
        for chunk in self.bytes_mut().as_chunks_mut::<4>().0 {
            let a = i32::from(chunk[3]);
            for (c, (&paper, &ink)) in chunk[..3].iter_mut().zip(palette.paper.iter().zip(&palette.ink)) {
                let (paper, ink) = (i32::from(paper), i32::from(ink));
                // Premultiplied: the paper share of the pixel plus the ink share, `a - c` of 255.
                let v = (paper * a + (ink - paper) * (a - i32::from(*c).min(a))) / 255;
                *c = v.clamp(0, a) as u8;
            }
        }
    }
}

/// The colours a page takes in inverted (dark) reading mode. The default, black paper and white
/// ink, is a plain colour inversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InvertPalette {
    /// What the page background (white in the document) becomes.
    pub paper: [u8; 3],
    /// What black text and line art become.
    pub ink: [u8; 3],
}

impl InvertPalette {
    /// Pure black paper and pure white ink (high contrast).
    pub const PURE: Self = Self { paper: [0, 0, 0], ink: [255, 255, 255] };
    /// Soft dark gray paper (#222226) and off-white ink (#dcdcdc), a restful dark theme.
    pub const SOFT_DARK: Self = Self { paper: [0x22, 0x22, 0x26], ink: [0xdc, 0xdc, 0xdc] };
    /// Warm sepia night reading.
    pub const SEPIA: Self = Self { paper: [0x2a, 0x24, 0x20], ink: [0xea, 0xd8, 0xc4] };
}

impl Default for InvertPalette {
    fn default() -> Self {
        Self::PURE
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recolor_maps_white_to_paper_and_black_to_ink() {
        let palette = InvertPalette { paper: [20, 30, 40], ink: [200, 210, 220] };
        let mut pixels = Pixels::from(vec![
            u32::from_ne_bytes([255, 255, 255, 255]),
            u32::from_ne_bytes([0, 0, 0, 255]),
            u32::from_ne_bytes([0, 0, 0, 0]),
            // Malformed premultiplied pixel (c > a) must not panic or wrap.
            u32::from_ne_bytes([255, 0, 9, 10]),
        ]);
        pixels.recolor(palette);
        assert_eq!(&pixels[0..4], &[20, 30, 40, 255]);
        assert_eq!(&pixels[4..8], &[200, 210, 220, 255]);
        assert_eq!(&pixels[8..12], &[0, 0, 0, 0]);
        assert!(pixels[12..15].iter().all(|&c| c <= 10));
        // The default palette is exactly the plain inversion.
        let (mut a, mut b) =
            (Pixels::from(vec![u32::from_ne_bytes([200, 100, 50, 255])]), Pixels::from(vec![u32::from_ne_bytes([200, 100, 50, 255])]));
        a.recolor(InvertPalette::default());
        b.invert();
        assert_eq!(a, b);
    }

    #[test]
    fn invert_pixels_preserves_alpha_and_round_trips() {
        let mut pixels = Pixels::from(vec![
            // Opaque white -> black
            u32::from_ne_bytes([255, 255, 255, 255]),
            // Opaque black -> white
            u32::from_ne_bytes([0, 0, 0, 255]),
            // Opaque color [200, 100, 50, 255] -> [55, 155, 205, 255]
            u32::from_ne_bytes([200, 100, 50, 255]),
            // Fully transparent [0, 0, 0, 0] -> [0, 0, 0, 0]
            u32::from_ne_bytes([0, 0, 0, 0]),
            // Semi-transparent [50, 25, 10, 100] -> [50, 75, 90, 100]
            u32::from_ne_bytes([50, 25, 10, 100]),
        ]);

        pixels.invert();
        assert_eq!(&pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&pixels[4..8], &[255, 255, 255, 255]);
        assert_eq!(&pixels[8..12], &[55, 155, 205, 255]);
        assert_eq!(&pixels[12..16], &[0, 0, 0, 0]);
        assert_eq!(&pixels[16..20], &[50, 75, 90, 100]);

        // Double inversion returns exactly to original
        pixels.invert();
        assert_eq!(&pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&pixels[8..12], &[200, 100, 50, 255]);
        assert_eq!(&pixels[12..16], &[0, 0, 0, 0]);
        assert_eq!(&pixels[16..20], &[50, 25, 10, 100]);
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
