//! The one way to the operating system's clipboard.
//!
//! The system clipboard is a single process-wide resource: on Windows, two threads that
//! open it at once can corrupt the heap. Every access goes through [`with`], which holds a
//! lock across the whole open, use and close, so parallel callers (test threads, workers)
//! take turns instead of racing.

use std::sync::{Mutex, PoisonError};

static LOCK: Mutex<()> = Mutex::new(());

/// Opens the system clipboard, runs `f` on it and closes it, with no other thread inside.
pub(crate) fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T, arboard::Error> {
    let _turn = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Declared after the guard, so it's closed before the lock is released.
    let mut clipboard = arboard::Clipboard::new()?;
    f(&mut clipboard)
}

/// Nonempty text supersedes the app's cached image, even if another clipboard format also exists.
pub(crate) fn has_text() -> bool {
    with(|cb| cb.get_text()).is_ok_and(|text| !text.is_empty())
}

/// The system clipboard as image paste sees it; text takes precedence over a cached image.
pub(crate) fn for_image_paste() -> Option<crate::create_ui::Clip> {
    if has_text() { Some(crate::create_ui::Clip::Text(String::new())) } else { crate::create_ui::read_clipboard() }
}

/// The `image/svg+xml` clipboard payload: the SVG's UTF-8 bytes and one NUL, as browsers place
/// it (readers that take the format's size as the text's length then still find its end).
#[cfg(any(windows, test))]
pub(crate) fn svg_clip_bytes(svg: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(svg.len().saturating_add(1));
    v.extend_from_slice(svg.as_bytes());
    v.push(0);
    v
}

/// The size of a `BITMAPV5HEADER`.
#[cfg(any(windows, test))]
const DIB_V5_HEADER: u32 = 124;

/// `CF_DIBV5` bytes for `width` × `height` premultiplied RGBA pixels: a `BITMAPV5HEADER`, then
/// premultiplied BGRA rows top-down (a negative height), with explicit channel masks and the
/// sRGB colour space, as browsers write it. `None` when the pixels are fewer than the size says,
/// either side is 0, or the bitmap is too large for the header's fields.
#[cfg(any(windows, test))]
pub(crate) fn dib_v5(width: u32, height: u32, premultiplied: &[u8], pixels_per_metre: u32) -> Option<Vec<u8>> {
    let w = i32::try_from(width).ok().filter(|w| *w > 0)?;
    let top_down = i32::try_from(height).ok().filter(|h| *h > 0)?.checked_neg()?;
    let len = usize::try_from(width).ok()?.checked_mul(usize::try_from(height).ok()?)?.checked_mul(4)?;
    let size_image = u32::try_from(len).ok()?;
    // The resolution fields are signed.
    i32::try_from(pixels_per_metre).ok()?;
    let pixels = premultiplied.get(..len)?;
    let mut out = Vec::with_capacity(len.checked_add(DIB_V5_HEADER as usize)?);
    for field in [DIB_V5_HEADER.to_le_bytes(), w.to_le_bytes(), top_down.to_le_bytes()] {
        out.extend_from_slice(&field);
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    // BI_BITFIELDS, the image size, the resolution, no palette, then the R, G, B and A masks,
    // the sRGB colour space ('sRGB'), no end points, no gamma, LCS_GM_IMAGES and no profile.
    for field in [3, size_image, pixels_per_metre, pixels_per_metre, 0, 0, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000, 0x7352_4742] {
        out.extend_from_slice(&u32::to_le_bytes(field));
    }
    out.extend_from_slice(&[0; 36 + 12]);
    for field in [4u32, 0, 0, 0] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    for &[r, g, b, a] in pixels.as_chunks::<4>().0 {
        out.extend_from_slice(&[b, g, r, a]);
    }
    Some(out)
}

/// Replaces the clipboard's contents with `svg` (as `image/svg+xml`), `png` (as `PNG`) and `dib`
/// (as `CF_DIBV5`), richest first, so each application pastes the best format it reads. All or
/// nothing: a failure part-way empties the clipboard again.
#[cfg(windows)]
pub(crate) fn put_vector(svg: &str, png: &[u8], dib: &[u8]) -> Result<(), String> {
    let _turn = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    // Declared after the guard, so it's closed before the lock is released.
    let _open = open_retrying()?;
    clipboard_win::raw::empty().map_err(|e| format!("couldn't clear the clipboard: {e}"))?;
    let placed = place_vector(svg, png, dib);
    if placed.is_err() {
        let _ = clipboard_win::raw::empty();
    }
    placed
}

/// Opens the clipboard, retrying briefly while another application holds it.
#[cfg(windows)]
fn open_retrying() -> Result<clipboard_win::Clipboard, String> {
    let mut last = String::new();
    for _ in 0..5 {
        match clipboard_win::Clipboard::new() {
            Ok(open) => return Ok(open),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err(format!("another application is holding the clipboard ({last})"))
}

#[cfg(windows)]
fn place_vector(svg: &str, png: &[u8], dib: &[u8]) -> Result<(), String> {
    use clipboard_win::raw;
    let named = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let id = raw::register_format(name).ok_or_else(|| format!("couldn't register the clipboard format {name}"))?;
        raw::set_without_clear(id.get(), bytes).map_err(|e| format!("couldn't place {name} on the clipboard: {e}"))
    };
    named("image/svg+xml", &svg_clip_bytes(svg))?;
    named("PNG", png)?;
    raw::set_without_clear(clipboard_win::formats::CF_DIBV5, dib).map_err(|e| format!("couldn't place a bitmap on the clipboard: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{DIB_V5_HEADER, dib_v5, svg_clip_bytes};

    #[test]
    fn svg_is_placed_with_one_trailing_nul() {
        assert_eq!(svg_clip_bytes("<svg/>"), b"<svg/>\0");
        assert_eq!(svg_clip_bytes(""), b"\0");
    }

    #[test]
    fn a_dib_v5_is_a_top_down_bgra_bitmap_in_srgb() {
        // Two pixels: red, and half-transparent premultiplied blue.
        let dib = dib_v5(2, 1, &[255, 0, 0, 255, 0, 0, 128, 128, 9, 9], 5906).expect("a bitmap");
        let header = DIB_V5_HEADER as usize;
        assert_eq!(dib.len(), header + 8);
        let u32_at = |at: usize| u32::from_le_bytes(dib[at..at + 4].try_into().unwrap());
        let i32_at = |at: usize| i32::from_le_bytes(dib[at..at + 4].try_into().unwrap());
        assert_eq!((u32_at(0), i32_at(4), i32_at(8)), (124, 2, -1), "size, width, a negative height for top-down");
        assert_eq!((u16::from_le_bytes([dib[12], dib[13]]), u16::from_le_bytes([dib[14], dib[15]])), (1, 32));
        assert_eq!((u32_at(16), u32_at(20), i32_at(24), i32_at(28)), (3, 8, 5906, 5906), "bit fields, the image size, the resolution");
        assert_eq!([u32_at(40), u32_at(44), u32_at(48), u32_at(52)], [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]);
        assert_eq!(u32_at(56), 0x7352_4742, "sRGB");
        assert_eq!(u32_at(108), 4, "LCS_GM_IMAGES");
        assert_eq!(&dib[header..], &[0, 0, 255, 255, 128, 0, 0, 128], "BGRA, still premultiplied");
    }

    #[test]
    fn a_dib_v5_of_a_bad_size_is_refused() {
        assert!(dib_v5(0, 1, &[], 0).is_none());
        assert!(dib_v5(1, 0, &[], 0).is_none());
        assert!(dib_v5(2, 2, &[0; 15], 0).is_none(), "fewer pixels than the size says");
        assert!(dib_v5(u32::MAX, 1, &[], 0).is_none());
        assert!(dib_v5(1 << 16, 1 << 15, &[], 0).is_none(), "an image size past u32");
        assert!(dib_v5(1, 1, &[0; 4], u32::MAX).is_none());
    }
}
