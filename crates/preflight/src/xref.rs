//! PDF/A-2 and -3 6.1.4 (PDF/A-4 6.1.4): the cross-reference table. The `xref` keyword and the
//! first subsection header after it are separated by a single end-of-line marker: CR, LF, or CR
//! followed by LF (ISO 32000-2 7.2.3, 7.5.4), with no white space before or after it.
//!
//! The tables read are those of the file's revisions, where the `startxref` chain places them. A
//! cross-reference stream has no `xref` keyword, and a section not found at its offset is left
//! out, since where it is written isn't known. A document whose next save rewrites it in full is
//! judged by the table that save writes, `xref` and one LF.

use pdfcraft_cos::Document;

/// Whether a cross-reference table of `doc`, as it will be saved, doesn't follow `xref` with a
/// single end-of-line marker and then its first subsection header.
pub(crate) fn check(doc: &Document) -> bool {
    if doc.full_save_required() || doc.encryption_changed() {
        return false;
    }
    let data = doc.bytes().as_slice();
    doc.revisions().iter().filter(|r| !r.is_stream).any(|r| {
        let shift = if r.header_relative { doc.header_offset() } else { 0 };
        let at = usize::try_from(r.xref_offset).ok().and_then(|o| o.checked_add(shift));
        let Some(rest) = at.and_then(|at| data.get(at..)).and_then(|rest| rest.strip_prefix(b"xref")) else { return false };
        let after = match rest {
            [b'\r', b'\n', after @ ..] | [b'\r' | b'\n', after @ ..] => after,
            _ => return true,
        };
        // The subsection header starts with the first object number.
        !after.first().is_some_and(u8::is_ascii_digit)
    })
}

/// Has the next save of `doc` write its cross-reference table afresh when [`check`] finds one
/// wrong; what it fixed, if anything.
pub(crate) fn rewrite(doc: &mut Document) -> Option<String> {
    if !check(doc) {
        return None;
    }
    doc.require_full_save();
    Some("Rewrote the cross-reference table".into())
}
