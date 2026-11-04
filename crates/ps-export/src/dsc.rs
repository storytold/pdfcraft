//! DSC (Document Structuring Conventions) comments.
//!
//! The emitter wraps its output in DSC structure so viewers and the Ghostscript oracle can
//! recognise pages and bounding boxes: a `%!PS-Adobe-3.0` header (EPS adds ` EPSF-3.0` and drops
//! `%%Pages:`), `%%Page`/`%%PageBoundingBox` per page, and a `%%Trailer`/`%%EOF` tail. Every
//! fidelity note becomes one `%%PdfCraftNote:` comment line of printable ASCII only.

use crate::PsFormat;

/// Append one DSC comment line (text plus `\n`) to `out`.
fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

/// Round a bounding-box coordinate onto the enclosing integer: DSC requires
/// `%%BoundingBox` to enclose the page's marks, so the lower-left corner floors and
/// the upper-right corner ceils — nearest-integer rounding would cut up to half a
/// point of artwork at an edge (an A4 page's 595.28 pt right edge becomes 595).
/// Non-finite input (which the emitter never passes, but PDF data is hostile) maps
/// to `0` rather than saturating to `i64::MAX`.
fn int(v: f64, up: bool) -> i64 {
    if !v.is_finite() {
        return 0;
    }
    (if up { v.ceil() } else { v.floor() }) as i64
}

/// Render a `%%…BoundingBox: llx lly urx ury` comment body for `key`.
fn bbox_line(key: &str, [llx, lly, urx, ury]: [f64; 4]) -> String {
    format!("{key}: {} {} {} {}", int(llx, false), int(lly, false), int(urx, true), int(ury, true))
}

/// The document preamble: `%!PS-Adobe-3.0` (EPS adds ` EPSF-3.0`), the integer-rounded
/// `%%BoundingBox:`, `%%Pages:` for whole-document PS only, then `%%Creator:` and
/// `%%EndComments`.
pub(crate) fn document_header(format: PsFormat, pages: usize, bbox: [f64; 4]) -> Vec<u8> {
    let mut s = String::new();
    match format {
        PsFormat::Ps => line(&mut s, "%!PS-Adobe-3.0"),
        PsFormat::Eps => line(&mut s, "%!PS-Adobe-3.0 EPSF-3.0"),
    }
    line(&mut s, &bbox_line("%%BoundingBox", bbox));
    if matches!(format, PsFormat::Ps) {
        line(&mut s, &format!("%%Pages: {pages}"));
    }
    line(&mut s, "%%Creator: PdfCraft");
    line(&mut s, "%%EndComments");
    s.into_bytes()
}

/// Open page `no` (1-based): `%%Page:` carries the label and position, then the page's own
/// integer-rounded `%%PageBoundingBox:`.
pub(crate) fn page_header(no: usize, bbox: [f64; 4]) -> Vec<u8> {
    let mut s = String::new();
    line(&mut s, &format!("%%Page: {no} {no}"));
    line(&mut s, &bbox_line("%%PageBoundingBox", bbox));
    s.into_bytes()
}

/// Close the document: `%%Trailer`, the final `%%Pages:` count, `%%EOF`.
pub(crate) fn trailer(pages: usize) -> Vec<u8> {
    let mut s = String::new();
    line(&mut s, "%%Trailer");
    line(&mut s, &format!("%%Pages: {pages}"));
    line(&mut s, "%%EOF");
    s.into_bytes()
}

/// One `%%PdfCraftNote:` comment line. DSC comments must stay a single printable-ASCII line,
/// so every non-ASCII or non-printable character becomes `?`.
pub(crate) fn note(text: &str) -> Vec<u8> {
    let mut s = String::from("%%PdfCraftNote: ");
    for c in text.chars() {
        s.push(if c.is_ascii_graphic() || c == ' ' { c } else { '?' });
    }
    s.push('\n');
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::{document_header, note, page_header, trailer};
    use crate::PsFormat;

    const BBOX: [f64; 4] = [0.0, 0.0, 200.0, 300.0];

    /// PS header: full DSC preamble including `%%Pages:`.
    #[test]
    fn ps_document_header_exact() {
        let bytes = document_header(PsFormat::Ps, 2, BBOX);
        assert_eq!(bytes.as_slice(), b"%!PS-Adobe-3.0\n%%BoundingBox: 0 0 200 300\n%%Pages: 2\n%%Creator: PdfCraft\n%%EndComments\n",);
    }

    /// EPS header: ` EPSF-3.0` flavour and no `%%Pages:` line (EPS is single-page).
    #[test]
    fn eps_document_header_exact() {
        let bytes = document_header(PsFormat::Eps, 1, BBOX);
        assert_eq!(bytes.as_slice(), b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 200 300\n%%Creator: PdfCraft\n%%EndComments\n",);
    }

    /// Bounding-box integers enclose the marks: the lower-left corner floors and the
    /// upper-right corner ceils (nearest-integer rounding would cut up to half a point
    /// of artwork at an edge — an A4 page's 595.28 pt right edge must not become 595);
    /// the `pages` argument is ignored by the EPS header.
    #[test]
    fn bbox_rounds_to_integers() {
        let bytes = document_header(PsFormat::Eps, 1, [-0.4, 9.6, 199.5, 300.4]);
        assert_eq!(bytes.as_slice(), b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: -1 9 200 301\n%%Creator: PdfCraft\n%%EndComments\n",);
        let bytes = page_header(7, [-0.5, 0.2, 99.5, 100.6]);
        assert_eq!(bytes.as_slice(), b"%%Page: 7 7\n%%PageBoundingBox: -1 0 100 101\n");
    }

    /// Each page opens with its ordinal twice (label, then position) plus its own bbox.
    #[test]
    fn page_header_exact() {
        let bytes = page_header(3, BBOX);
        assert_eq!(bytes.as_slice(), b"%%Page: 3 3\n%%PageBoundingBox: 0 0 200 300\n");
    }

    /// The trailer restates the page count and closes the document.
    #[test]
    fn trailer_exact() {
        let bytes = trailer(2);
        assert_eq!(bytes.as_slice(), b"%%Trailer\n%%Pages: 2\n%%EOF\n");
    }

    /// Notes are one printable-ASCII line: non-ASCII (`é`) and control characters become `?`.
    #[test]
    fn note_is_one_printable_line() {
        assert_eq!(note("transparency ignored (blend Multiply)").as_slice(), b"%%PdfCraftNote: transparency ignored (blend Multiply)\n",);
        assert_eq!(note("caf\u{e9}").as_slice(), b"%%PdfCraftNote: caf?\n");
        assert_eq!(note("tab\there").as_slice(), b"%%PdfCraftNote: tab?here\n");
    }
}
