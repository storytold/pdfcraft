//! pdfcraft-thumbnails — first-page thumbnails of PDF files for the system file manager.
//!
//! [`render_thumbnail`] is the platform-independent part. On Windows the crate is also a
//! cdylib, `pdfcraft_thumbnails.dll`: the `IThumbnailProvider` that File Explorer loads for
//! `PdfCraft.Document` files ([`windows`] module, see the README for its registration).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod windows;

/// The largest square a thumbnail is fitted into. File Explorer asks for at most 2560 pixels
/// (its biggest thumbnail cache); larger requests are reduced to it.
pub const MAX_SIDE: u32 = 2560;

/// A rendered thumbnail: `width` × `height` pixels, top-down rows, 4 bytes per pixel in B, G, R, A
/// order (a Windows 32-bit DIB), opaque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Renders the first page of the PDF in `bytes`, fitted into a `side` × `side` square (at least
/// 1, at most [`MAX_SIDE`]), onto white paper. Never panics; an unreadable, password-protected or
/// empty document is an error.
pub fn render_thumbnail(bytes: Vec<u8>, side: u32) -> Result<Thumbnail, String> {
    let side = side.clamp(1, MAX_SIDE);
    catch_unwind(AssertUnwindSafe(|| render(bytes, side))).unwrap_or_else(|_| Err("the renderer crashed".into()))
}

fn render(bytes: Vec<u8>, side: u32) -> Result<Thumbnail, String> {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    if renderer.page_count() == 0 {
        return Err("the document could not be opened or has no pages".into());
    }
    let (w, h) = renderer.page_size(0).ok_or("the first page's size could not be read")?;
    let longest = w.max(h);
    if !longest.is_finite() || longest < 0.5 {
        return Err(format!("the first page has an invalid size ({w}×{h} pt)"));
    }
    let scale = side as f32 / longest;
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: true });
    if let Some(e) = page.error {
        return Err(e);
    }
    let pixels =
        usize::try_from(page.width).ok().and_then(|w| w.checked_mul(usize::try_from(page.height).ok()?)).ok_or("the thumbnail is too large")?;
    let rgba: &[u8] = &page.rgba;
    if pixels == 0 || rgba.len() != pixels.saturating_mul(4) {
        return Err("the renderer returned no pixels".into());
    }
    // Premultiplied RGBA over white: each channel gains the paper showing through, 255 - alpha.
    let mut bgra = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        let paper = 255 - p[3];
        bgra.extend_from_slice(&[p[2].saturating_add(paper), p[1].saturating_add(paper), p[0].saturating_add(paper), 255]);
    }
    Ok(Thumbnail { width: page.width, height: page.height, bgra })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A one-page PDF of `w` × `h` points whose content is `content`, with a correct xref table.
    pub(crate) fn pdf(w: u32, h: u32, content: &str) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len() + 1),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    fn pixel(t: &Thumbnail, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * t.width + x) * 4) as usize;
        [t.bgra[i], t.bgra[i + 1], t.bgra[i + 2], t.bgra[i + 3]]
    }

    #[test]
    fn the_first_page_is_fitted_into_the_square_on_white_paper() {
        // A landscape page, red in its left half and blank (paper) in its right half.
        let t = render_thumbnail(pdf(200, 100, "1 0 0 rg 0 0 100 100 re f"), 256).unwrap();
        assert_eq!((t.width, t.height), (256, 128));
        assert_eq!(t.bgra.len(), 256 * 128 * 4);
        assert_eq!(pixel(&t, 60, 64), [0, 0, 255, 255], "red, as B, G, R, A");
        assert_eq!(pixel(&t, 200, 64), [255, 255, 255, 255], "the blank half is white paper, not transparent");
        // Portrait pages are fitted by their height.
        let t = render_thumbnail(pdf(100, 200, ""), 96).unwrap();
        assert_eq!((t.width, t.height), (48, 96));
    }

    #[test]
    fn requested_sizes_are_kept_within_limits() {
        let t = render_thumbnail(pdf(100, 100, ""), 0).unwrap();
        assert_eq!((t.width, t.height), (1, 1));
        let t = render_thumbnail(pdf(10, 10, ""), u32::MAX).unwrap();
        assert_eq!((t.width, t.height), (MAX_SIDE, MAX_SIDE));
    }

    #[test]
    fn files_that_are_not_readable_pdfs_are_errors_not_crashes() {
        assert!(render_thumbnail(Vec::new(), 256).is_err());
        assert!(render_thumbnail(b"not a PDF at all".to_vec(), 256).is_err());
        let mut truncated = pdf(100, 100, "0 0 1 rg 0 0 50 50 re f");
        truncated.truncate(40);
        assert!(render_thumbnail(truncated, 256).is_err());
    }
}
