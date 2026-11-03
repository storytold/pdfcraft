//! Other files to combine with or copy pages from: opening them, and why one can't be used.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use pdfcraft_organize::order::{FileFacts, FileSort, file_order};

use crate::{EditError, SourceProblem};

/// A file to combine: its name (the bookmark title), bytes, and page range (`None`: all).
pub type CombineSource = (String, Arc<Vec<u8>>, Option<String>);

/// What a combine produced: the new PDF bytes, and the fields renamed to keep colliding names
/// separate (old fully-qualified name → new one), in order across the sources.
#[derive(Debug)]
pub struct Combined {
    pub bytes: Arc<Vec<u8>>,
    pub renamed_fields: Vec<(String, String)>,
}

/// Parse another PDF to copy pages from.
pub(crate) fn open_source(name: &str, bytes: &Arc<Vec<u8>>) -> Result<pdfcraft_cos::Document, EditError> {
    open_source_with(name, bytes, None)
}

/// [`open_source`], authenticating with `password` (user or owner) for an encrypted file.
pub(crate) fn open_source_with(name: &str, bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<pdfcraft_cos::Document, EditError> {
    source_document(bytes, password).map_err(|p| EditError::Source(format!("{name}: {p}")))
}

/// Import the comments of `src` onto the pages of `doc` with the same numbers.
pub(crate) fn import_pdf_comments(doc: &mut pdfcraft_cos::Document, src: &pdfcraft_cos::Document) -> Result<(), EditError> {
    if pdfcraft_organize::import_comments(doc, src)? == 0 {
        return Err(pdfcraft_xfdf::DataError::NothingImported.into());
    }
    Ok(())
}

/// Parse another PDF to import comments from, whatever its permissions allow.
pub(crate) fn comment_source(bytes: &Arc<Vec<u8>>) -> Result<pdfcraft_cos::Document, SourceProblem> {
    match std::panic::catch_unwind(|| pdfcraft_cos::Document::open(bytes.clone())) {
        Ok(Ok(d)) => Ok(d),
        Ok(Err(pdfcraft_cos::CosError::NeedsPassword)) => Err(SourceProblem::Password),
        Ok(Err(e)) => Err(SourceProblem::Unreadable(e.to_string())),
        Err(_) => Err(SourceProblem::Unreadable("the file could not be read".into())),
    }
}

fn source_document(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<pdfcraft_cos::Document, SourceProblem> {
    match std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)) {
        Ok(Ok(d)) if d.permissions().is_some_and(|p| !p.assemble()) => Err(SourceProblem::NotPermitted),
        Ok(Ok(d)) => Ok(d),
        Ok(Err(pdfcraft_cos::CosError::NeedsPassword)) => Err(SourceProblem::Password),
        Ok(Err(pdfcraft_cos::CosError::WrongPassword)) => Err(SourceProblem::WrongPassword),
        Ok(Err(e)) => Err(SourceProblem::Unreadable(e.to_string())),
        Err(_) => Err(SourceProblem::Unreadable("the file could not be read".into())),
    }
}

/// Whether `bytes` can be combined, opened with `password` (user or owner) if given, and if not
/// why: what Combine would refuse it for, so the Combine files list can say so before Combine
/// is pressed. The owner (permissions) password lifts the restriction on copying pages.
pub fn combine_source_check(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<(), SourceProblem> {
    source_document(bytes, password).map(|_| ())
}

/// A file about to be combined, as [`combine_order`] reads it.
#[derive(Clone, Copy, Debug)]
pub struct SortedSource<'a> {
    pub bytes: &'a Arc<Vec<u8>>,
    pub path: &'a Path,
    /// When the file was last modified, which the caller reads under its own rules for what it
    /// may touch.
    pub modified: Option<SystemTime>,
    pub password: Option<&'a str>,
}

/// The order `files` go in when sorted `by`, ascending or not, as positions in `files`. A file
/// whose pages can't be counted sorts as having none; combining it then says what is wrong with it.
pub fn combine_order(files: &[SortedSource<'_>], by: FileSort, ascending: bool) -> Vec<usize> {
    let names: Vec<String> = files.iter().map(|f| f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
    let facts: Vec<FileFacts> = files
        .iter()
        .zip(&names)
        .map(|(file, name)| FileFacts {
            name,
            // Counting opens the file, so only a sort by pages pays for it.
            pages: if by == FileSort::Pages { source_page_count(file.bytes, file.password).unwrap_or(0) } else { 0 },
            size: file.bytes.len(),
            modified: file.modified,
            folder: file.path.parent(),
            rank: 0,
        })
        .collect();
    file_order(&facts, by, ascending)
}

/// The number of pages of `bytes`, opened with `password` (user or owner) if given, whatever
/// its permissions allow; `None` when it can't be read.
pub fn source_page_count(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<usize> {
    let doc = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)).ok()?.ok()?;
    pdfcraft_organize::page_count(&doc).ok()
}
