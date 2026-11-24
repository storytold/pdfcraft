//! Legacy PowerPoint 97–2003 import (`.ppt`, `.pot`, `.pps`), written from Microsoft's public
//! Open Specifications: [MS-CFB] (the compound file container), [MS-PPT] (the presentation
//! records) and [MS-ODRAW] (the shapes and pictures).
//!
//! [`import`] reads a presentation leniently into the DeckCraft model: unknown records are
//! skipped, damaged ones are left out, and every size, count and depth is bounded. Saving goes
//! through the `.pptx` and native writers; there is no `.ppt` writer.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod blip;
mod cfb;
mod convert;
mod doc;
mod draw;
mod geom;
mod record;
mod text;

use deckcraft_model::Presentation;

/// Name of the stream holding the presentation.
const DOCUMENT_STREAM: &str = "PowerPoint Document";

#[derive(Debug, thiserror::Error)]
pub enum PptError {
    #[error("not a PowerPoint 97-2003 presentation")]
    NotPpt,
    #[error("the presentation is password-protected")]
    Encrypted,
    #[error("the presentation was made by PowerPoint 95 or earlier")]
    TooOld,
    #[error("the presentation is damaged: {0}")]
    Damaged(String),
}

/// Does this look like a PowerPoint 97–2003 file (a compound file with a `PowerPoint Document`
/// stream)?
pub fn sniff(bytes: &[u8]) -> bool {
    cfb::sniff(bytes) && cfb::CompoundFile::open(bytes).is_ok_and(|f| f.root_names().iter().any(|n| n.eq_ignore_ascii_case(DOCUMENT_STREAM)))
}

/// Read a `.ppt` / `.pot` / `.pps` file.
pub fn import(bytes: &[u8]) -> Result<Presentation, PptError> {
    import_checked(bytes).map(|(p, _)| p)
}

/// [`import`], plus the slides the file lists but whose objects couldn't be found, so they are not
/// in the presentation.
pub fn import_checked(bytes: &[u8]) -> Result<(Presentation, Vec<String>), PptError> {
    let file = cfb::CompoundFile::open(bytes).map_err(|e| match e {
        cfb::CfbError::NotCfb => PptError::NotPpt,
        other => PptError::Damaged(other.to_string()),
    })?;
    let stream = file.read_root_stream(DOCUMENT_STREAM).map_err(|e| match e {
        cfb::CfbError::NotFound(_) => PptError::NotPpt,
        other => PptError::Damaged(other.to_string()),
    })?;
    let current_user = file.read_root_stream("Current User").unwrap_or_default();
    // A damaged or missing picture stream costs the pictures, not the presentation.
    let pictures = file.read_root_stream("Pictures").unwrap_or_default();
    let d = doc::read(&current_user, &stream)?;
    Ok((convert::convert(&d, &pictures), d.lost.clone()))
}

#[cfg(test)]
mod tests;
