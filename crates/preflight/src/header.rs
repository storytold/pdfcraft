//! PDF/A-2 6.1.2, the file header: the file starts at its first byte with `%PDF-1.n`, `n` from 0
//! to 7, and an end-of-line marker; the next line is a comment, `%` and at least four bytes each
//! above 127, which marks the file as binary (ISO 32000-1 7.5.2).
//!
//! A document whose next save rewrites it in full is judged by the header that save writes: the
//! version it keeps, followed by the writer's own binary comment.

use pdfcraft_cos::Document;

/// What the header of `doc`, as it will be saved, breaks of 6.1.2.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Header {
    /// The first line isn't `%PDF-1.0` to `%PDF-1.7` at the start of the file.
    pub(crate) version: bool,
    /// The second line isn't a comment of at least four bytes above 127.
    pub(crate) comment: bool,
}

/// What the header of `doc` breaks of 6.1.2: as stored, or, when the next save rewrites the
/// file, as that save will write it.
pub(crate) fn check(doc: &Document) -> Header {
    if doc.full_save_required() || doc.encryption_changed() {
        let written = doc.version().get(..3).unwrap_or_default();
        return Header { version: !allowed(written.as_bytes()), comment: false };
    }
    let data = doc.bytes();
    let Some(rest) = data.strip_prefix(b"%PDF-") else { return Header { version: true, comment: false } };
    let version = rest.get(..3).unwrap_or_default();
    let Some(rest) = rest.get(3..).and_then(eol) else {
        return Header { version: true, comment: !binary_comment(after_line(rest)) };
    };
    Header { version: !allowed(version), comment: !binary_comment(rest) }
}

/// Whether a version is `1.0` to `1.7`.
fn allowed(version: &[u8]) -> bool {
    matches!(version, [b'1', b'.', b'0'..=b'7'])
}

/// What follows an end-of-line marker (CR, LF or CR LF) at the start of `data`.
fn eol(data: &[u8]) -> Option<&[u8]> {
    match data {
        [b'\r', b'\n', rest @ ..] | [b'\r', rest @ ..] | [b'\n', rest @ ..] => Some(rest),
        _ => None,
    }
}

/// What follows the first end-of-line marker in `data`, or nothing.
fn after_line(data: &[u8]) -> &[u8] {
    let end = data.iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(data.len());
    data.get(end..).and_then(eol).unwrap_or_default()
}

/// Whether `data` starts with `%` and four bytes above 127.
fn binary_comment(data: &[u8]) -> bool {
    matches!(data, [b'%', a, b, c, d, ..] if [a, b, c, d].iter().all(|&&x| x > 127))
}

/// Makes the next save of `doc` write a header 6.1.2 allows, when the current one breaks it:
/// a full save, at version 1.7 when the version kept isn't 1.0 to 1.7. What it fixed, if anything.
pub(crate) fn rewrite(doc: &mut Document) -> Option<String> {
    let header = check(doc);
    if !header.version && !header.comment {
        return None;
    }
    if !allowed(doc.version().as_bytes().get(..3).unwrap_or_default()) {
        doc.set_version("1.7");
    }
    doc.require_full_save();
    Some("Rewrote the file header".into())
}
