//! Other files to combine with or copy pages from: opening them, and why one can't be used.

use std::sync::Arc;

use crate::{EditError, OfficeReport, SourceKind, SourceProblem};

/// A file to combine: its name (the bookmark title), bytes, and page range (`None`: all).
pub type CombineSource = (String, Arc<Vec<u8>>, Option<String>);

/// What a combine produced: the new PDF bytes, and the fields renamed to keep colliding names
/// separate (old fully-qualified name → new one), in order across the sources.
#[derive(Debug)]
pub struct Combined {
    pub bytes: Arc<Vec<u8>>,
    pub renamed_fields: Vec<(String, String)>,
}

/// A file made into PDF bytes by [`crate::Session::convert_to_pdf_reported`]: what it was, the
/// PDF, and the report of a native Word, PowerPoint or Excel conversion (`None` for every other
/// file, and for an Office document LibreOffice converted).
pub type Converted = (SourceKind, Arc<Vec<u8>>, Option<OfficeReport>);

/// Convert the Office document `bytes` named `name` (Word, Excel, PowerPoint, OpenDocument,
/// RTF) to PDF with an installed LibreOffice, and check the result opens. It runs a separate
/// process for seconds, so callers keep it off the frame thread. Desktop only: on the web it
/// returns an error.
pub fn office_to_pdf(name: &str, bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, EditError> {
    let pdf = Arc::new(pdfcraft_create::office_to_pdf(name, bytes)?);
    open_source(name, &pdf)?;
    Ok(pdf)
}

/// How an Office document may be converted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OfficeRoute {
    /// Through LibreOffice when it is installed, which takes seconds (off the frame thread), and
    /// otherwise natively for Office Open XML.
    Any,
    /// Natively only, on the frame thread: other Office formats are refused, pointing to
    /// File ▸ Open.
    Native,
}

/// Whether LibreOffice is installed (never on the web).
fn libreoffice_installed() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return pdfcraft_create::libreoffice().is_some();
    #[cfg(target_arch = "wasm32")]
    return false;
}

/// Convert the Office document `bytes` named `name` to PDF bytes along `route`, with the report
/// of a native conversion. LibreOffice reads every Office format with the documents' own layout;
/// the native converter ([`pdfcraft_create::from_office`]) reads Word, PowerPoint and Excel
/// packages where LibreOffice isn't installed, or fails, or would block the frame thread.
pub(crate) fn office_pdf(
    now: Option<i64>,
    name: &str,
    bytes: &Arc<Vec<u8>>,
    route: OfficeRoute,
    no_libreoffice: bool,
) -> Result<(Arc<Vec<u8>>, Option<OfficeReport>), EditError> {
    match no_libreoffice {
        // Never run it: a Word, PowerPoint or Excel file converts natively, any other is refused.
        true => office_pdf_with(false, now, name, bytes, OfficeRoute::Native),
        false => office_pdf_with(libreoffice_installed(), now, name, bytes, route),
    }
}

/// [`office_pdf`], given whether LibreOffice is installed.
pub(crate) fn office_pdf_with(
    libreoffice: bool,
    now: Option<i64>,
    name: &str,
    bytes: &Arc<Vec<u8>>,
    route: OfficeRoute,
) -> Result<(Arc<Vec<u8>>, Option<OfficeReport>), EditError> {
    let native = pdfcraft_create::is_ooxml(name, bytes);
    match route {
        OfficeRoute::Any if libreoffice || !native => match office_to_pdf(name, bytes) {
            Ok(pdf) => Ok((pdf, None)),
            // A Word, PowerPoint or Excel file LibreOffice couldn't convert still gets the native
            // conversion; LibreOffice's reason is kept if that fails too.
            Err(e) if native => ooxml_pdf(now, name, bytes).map(|(pdf, report)| (pdf, Some(report))).map_err(|_| e),
            Err(e) => Err(e),
        },
        _ if native => ooxml_pdf(now, name, bytes).map(|(pdf, report)| (pdf, Some(report))),
        _ => Err(EditError::Source(format!(
            "{name}: only Word, PowerPoint and Excel files (.docx, .pptx, .xlsx) can be added here; open this one with File ▸ Open, which converts it with LibreOffice"
        ))),
    }
}

/// A Word, PowerPoint or Excel file converted natively to new PDF bytes, and what could not be
/// carried over.
fn ooxml_pdf(now: Option<i64>, name: &str, bytes: &[u8]) -> Result<(Arc<Vec<u8>>, OfficeReport), EditError> {
    // The importer reads untrusted packages, and the document it builds is written, reopened and
    // rendered here: a panic anywhere in that must not take the app down.
    crate::guard(|| ooxml_pdf_unguarded(now, name, bytes)).map_err(|_| EditError::Source(format!("{name}: the file could not be read")))?
}

fn ooxml_pdf_unguarded(now: Option<i64>, name: &str, bytes: &[u8]) -> Result<(Arc<Vec<u8>>, OfficeReport), EditError> {
    let (doc, report) = pdfcraft_create::from_office(name, bytes)?;
    let opts = pdfcraft_cos::SaveOptions { mod_date: now.map(pdfcraft_cos::pdf_date), ..pdfcraft_cos::SaveOptions::default() };
    let pdf = pdfcraft_cos::write_full(&doc, &opts).map(Arc::new).map_err(|e| EditError::Write(e.to_string()))?;
    // Checked like every new file (`Session::write_new`) before anyone writes or opens it: it
    // reopens, and shows the pages it was built from.
    let expected = pdfcraft_organize::page_count(&doc)?;
    pdfcraft_cos::Document::open(pdf.clone()).map_err(|e| EditError::Reopen(e.to_string()))?;
    pdfcraft_render::check_pages(pdf.clone(), None, expected).map_err(|e| EditError::Reopen(e.to_string()))?;
    Ok((pdf, report))
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
