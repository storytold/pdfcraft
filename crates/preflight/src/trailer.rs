//! PDF/A-2 6.1.3, the file trailer: it has an `/ID`, a file identifier as ISO 32000-1 14.4
//! defines it, an array of two byte strings.
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
