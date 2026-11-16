//! Where a save, a signature or a Document Properties change writes the document's own metadata.
//! Every one of them asks [`home`], which reads the document's PDF/A-4 identification
//! ([`pdfcraft_preflight::declares_pdfa4`]), and lets [`pdfcraft_organize::metadata`] write it: a
//! file that declares PDF/A-4 keeps its metadata in XMP and gets no `/Info` (ISO 19005-4 §6.1.3);
//! every other file is written exactly as before.

use std::borrow::Cow;

use pdfcraft_cos::Document;
use pdfcraft_organize::MetadataHome;

use crate::EditError;

/// Where `cos` keeps its document-level metadata.
pub(crate) fn home(cos: &Document) -> MetadataHome {
    MetadataHome::of(cos, pdfcraft_preflight::declares_pdfa4(cos))
}

/// `cos` ready for a save that would stamp `mod_date` (a PDF date) into `/Info /ModDate`, and the
/// `mod_date` to give the writer ([`pdfcraft_cos::SaveOptions::mod_date`]): see
/// [`pdfcraft_organize::metadata::record_save`]. A document that keeps its metadata in `/Info`
/// comes back as it went in, borrowed or not, with `mod_date` unchanged.
pub(crate) fn prepare_save(mut cos: Cow<'_, Document>, mod_date: Option<String>) -> Result<(Cow<'_, Document>, Option<String>), EditError> {
    let home = home(&cos);
    if home == MetadataHome::Info {
        return Ok((cos, mod_date));
    }
    let mod_date = pdfcraft_organize::metadata::record_save(cos.to_mut(), home, mod_date)?;
    Ok((cos, mod_date))
}
