//! pdfcraft-preview — first-page preview of PDF files in the Windows File Explorer preview pane.
//!
//! Rendering reuses [`pdfcraft_thumbnails::render_page_fit`]. On Windows the crate is also a
//! cdylib, `pdfcraft_preview.dll`: the `IPreviewHandler` registered for `PdfCraft.Document` files
//! ([`windows`] module, see the README for its registration).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub use pdfcraft_thumbnails::{MAX_PREVIEW_SIDE, Thumbnail, render_page_fit};

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod windows;

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(w: u32, h: u32, content: &str) -> Vec<u8> {
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

    #[test]
    fn the_first_page_is_fitted_into_the_pane() {
        let t = render_page_fit(pdf(200, 100, "1 0 0 rg 0 0 100 100 re f"), 320, 240).unwrap();
        assert_eq!((t.width, t.height), (320, 160));
    }
}
