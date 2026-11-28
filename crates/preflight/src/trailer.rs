//! PDF/A-2 6.1.3, the file trailer: it has an `/ID`, a file identifier as ISO 32000-1 14.4
//! defines it, an array of two byte strings; and nothing follows the last end-of-file marker but
//! a single optional end-of-line marker (ISO 32000-1 7.5.5).
//!
//! The trailer read is the one the file's last `startxref` points to: the latest update's, or a
//! linearized file's first-page trailer (ISO 32000-1 F.3.4). Trailers earlier in the chain don't
//! stand in for it.

use pdfcraft_cos::{Document, Object};

/// The longest string a conforming file may hold (ISO 32000-1 Annex C, Table C.1).
const MAX_STRING: usize = 32767;

/// What is wrong with the file identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdProblem {
    /// The trailer has no `/ID`, or a null one.
    Missing,
    /// The `/ID` isn't an array of two non-empty strings.
    Malformed,
}

/// What 6.1.3 finds wrong with the file identifier of `doc`, if anything.
pub(crate) fn check(doc: &Document) -> Option<IdProblem> {
    let id = doc.trailer().get(b"ID").map(|id| doc.resolve(id)).filter(|id| !matches!(**id, Object::Null));
    let Some(id) = id else { return Some(IdProblem::Missing) };
    let strings = |parts: &[Object]| parts.iter().all(|p| doc.resolve(p).as_string().is_some_and(|s| !s.bytes.is_empty()));
    match id.as_array() {
        Some(parts) if parts.len() == 2 && strings(parts) => None,
        _ => Some(IdProblem::Malformed),
    }
}

/// Gives `doc` a file identifier when it lacks a good one; what it fixed, if anything. A first
/// string a conforming file can keep (non-empty, within [`MAX_STRING`]) is the permanent
/// identifier (ISO 32000-1 14.4): it is kept, and only the second is made afresh. Otherwise both
/// are new, and the report says the identifier was added rather than completed.
pub(crate) fn fix(doc: &mut Document) -> Option<String> {
    check(doc)?;
    let Object::Array(mut id) = pdfcraft_cos::new_file_id(doc.bytes(), 0) else { return None };
    let permanent = doc
        .trailer()
        .get(b"ID")
        .map(|old| doc.resolve(old))
        .and_then(|old| old.as_array().and_then(|parts| parts.first().map(|p| doc.resolve(p))))
        .filter(|first| first.as_string().is_some_and(|s| (1..=MAX_STRING).contains(&s.bytes.len())));
    let kept = match (permanent, id.first_mut()) {
        (Some(permanent), Some(slot)) => {
            *slot = (*permanent).clone();
            true
        }
        _ => false,
    };
    doc.trailer_mut().set(b"ID".to_vec(), Object::Array(id));
    Some(if kept { "Completed the file identifier (ID)" } else { "Added a file identifier (ID)" }.into())
}

/// Whether data follows the last end-of-file marker of `doc`, as it will be saved, other than a
/// single end-of-line marker (CR, LF or CR LF, ISO 32000-1 7.2.2 and 7.5.5). A document its next
/// save rewrites is judged by that save, which ends the file itself. A file without `%%EOF` is
/// left to the reader that opened it.
pub(crate) fn data_after_end(doc: &Document) -> bool {
    if doc.full_save_required() || doc.encryption_changed() {
        return false;
    }
    let data = doc.bytes();
    // The marker is a line of its own (ISO 32000-1 7.5.5): `%%EOF` written inside other data after
    // it is that data, not the marker.
    let at_line_start = |p: usize| p == 0 || matches!(data.get(p.saturating_sub(1)), Some(b'\r' | b'\n'));
    let Some(end) = data.windows(5).enumerate().rev().find(|&(p, w)| w == b"%%EOF" && at_line_start(p)).map(|(p, _)| p) else { return false };
    let after = data.get(end.saturating_add(5)..).unwrap_or_default();
    let rest = match after {
        [b'\r', b'\n', rest @ ..] | [b'\r' | b'\n', rest @ ..] => rest,
        rest => rest,
    };
    !rest.is_empty()
}

/// Has the next save of `doc` write the file afresh, ending at its end-of-file marker, when data
/// follows the last one; what it fixed, if anything.
pub(crate) fn rewrite_end(doc: &mut Document) -> Option<String> {
    if !data_after_end(doc) {
        return None;
    }
    doc.require_full_save();
    Some("Removed the data after the end-of-file marker".into())
}
