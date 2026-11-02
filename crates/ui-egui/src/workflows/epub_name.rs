//! The name an EPUB export is saved under, from the path the Save dialog returned.

use std::path::Path;

/// The file name the user chose with a trailing `.epub` (any case) taken off, since the export
/// adds it back. Only that extension goes: `report.v2` stays `report.v2`, so the book is
/// `report.v2.epub` and not `report.epub`. `fallback` is used when the path has no file name.
pub fn epub_stem(path: &Path, fallback: &str) -> String {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return fallback.to_string() };
    // `.epub` is ASCII, so the last five bytes are on a char boundary exactly when they match.
    let cut = name.len().checked_sub(5).filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".epub"));
    match cut {
        Some(at) if at > 0 => name[..at].to_string(),
        _ => name,
    }
}
