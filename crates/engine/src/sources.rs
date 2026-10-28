//! Other files to combine with or copy pages from: opening them, and why one can't be used.

use std::sync::Arc;

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

/// [`open_source`] for pages inserted into `dest`. A source that withholds page extraction but
/// allows assembly is accepted when `dest` will be saved under the very same protection (file
/// key and `/Encrypt` values), as when pages cut or copied from a document are pasted back into
/// it: they never leave its protection (#5081).
pub(crate) fn open_source_into(name: &str, bytes: &Arc<Vec<u8>>, dest: &pdfcraft_cos::Document) -> Result<pdfcraft_cos::Document, EditError> {
    let same_protection = |src: &pdfcraft_cos::Document| match (src.security(), dest.output_handler()) {
        (Some(s), Some(d)) => s.file_key() == d.file_key() && s.dict() == d.dict(),
        _ => false,
    };
    let permitted = |src: &pdfcraft_cos::Document| src.permissions().is_none_or(|p| p.extract_pages() || (p.assemble() && same_protection(src)));
    source_document_if(bytes, None, permitted).map_err(|p| EditError::Source(format!("{name}: {p}")))
}

fn source_document(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<pdfcraft_cos::Document, SourceProblem> {
    source_document_if(bytes, password, |d| d.permissions().is_none_or(|p| p.extract_pages()))
}

/// Open `bytes` as a page source, refused as [`SourceProblem::NotPermitted`] unless `permitted`.
fn source_document_if(
    bytes: &Arc<Vec<u8>>,
    password: Option<&str>,
    permitted: impl Fn(&pdfcraft_cos::Document) -> bool,
) -> Result<pdfcraft_cos::Document, SourceProblem> {
    match std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)) {
        Ok(Ok(d)) if !permitted(&d) => Err(SourceProblem::NotPermitted),
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

/// The number of pages of `bytes`, opened with `password` (user or owner) if given, whatever
/// its permissions allow; `None` when it can't be read.
pub fn source_page_count(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<usize> {
    let doc = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)).ok()?.ok()?;
    pdfcraft_organize::page_count(&doc).ok()
}
