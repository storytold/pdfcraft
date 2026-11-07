//! Reading the model a 3D annotation shows (ISO 32000-2 §13.6.3 and §13.6.4): its `/3DD` is a
//! 3D stream, or a 3D reference dictionary naming the stream another annotation shares.

use super::lookup::{annotation_flags, annots, page_ref, page_refs};
use super::threed::MAX_MODEL_BYTES;
use super::{AnnotError, Dict, Document, Object};

/// The format of a 3D stream, from its `/Subtype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model3dKind {
    /// Product Representation Compact (ISO 14739-1).
    Prc,
    /// Universal 3D (ECMA-363).
    U3d,
}

/// A 3D annotation's model: its format and the decoded stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Model3d {
    pub kind: Model3dKind,
    pub data: Vec<u8>,
}

/// The `/Annots` positions of the 3D annotations on 0-based `page`, in order.
///
/// # Errors
/// [`AnnotError::NoPageTree`] or [`AnnotError::NoSuchPage`].
pub fn model_3d_annotations(doc: &Document, page: usize) -> Result<Vec<usize>, AnnotError> {
    let list = annots(doc, page_ref(doc, page)?);
    Ok(list.iter().enumerate().filter(|(_, a)| doc.dict(a).is_some_and(|d| is_3d(doc, &d))).map(|(i, _)| i).collect())
}

/// Most 3D annotations [`shown_model_3d_positions`] lists, as `pdfcraft_render` lists at most.
const MAX_SHOWN_3D: usize = 1024;

/// The `(page, /Annots position)` of each 3D annotation the pages show, in the order and by the
/// rules of `pdfcraft_render`'s `DocInfo::annots_3d`: pages in order; an `/Annots` array object
/// several pages share, and an annotation listed more than once, counted where first listed;
/// those flagged Hidden or NoView (§12.5.3) left out; at most 1024.
///
/// # Errors
/// [`AnnotError::NoPageTree`].
pub fn shown_model_3d_positions(doc: &Document) -> Result<Vec<(usize, usize)>, AnnotError> {
    let mut arrays = std::collections::HashSet::new();
    let mut listed = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (page, page_ref) in page_refs(doc)?.into_iter().enumerate() {
        let shared = doc.get(page_ref).as_dict().and_then(|d| d.reference(b"Annots"));
        if shared.is_some_and(|r| !arrays.insert(r)) {
            continue;
        }
        for (index, a) in annots(doc, page_ref).iter().enumerate() {
            if a.as_ref().is_some_and(|r| !listed.insert(r)) {
                continue;
            }
            if doc.dict(a).is_some_and(|d| is_3d(doc, &d) && annotation_flags(doc, &d) & (2 | 32) == 0) {
                if out.len() == MAX_SHOWN_3D {
                    return Ok(out);
                }
                out.push((page, index));
            }
        }
    }
    Ok(out)
}

/// Whether `d` is a 3D annotation, its `/Subtype` read through a reference.
fn is_3d(doc: &Document, d: &Dict) -> bool {
    d.get(b"Subtype").is_some_and(|s| doc.resolve(s).as_name() == Some(b"3D"))
}

/// The model of the 3D annotation at `/Annots` position `index` on 0-based `page`, decoded.
///
/// # Errors
/// [`AnnotError::NoSuchAnnotation`] when there is no annotation there; [`AnnotError::Invalid`]
/// when it is not a 3D annotation, has no readable 3D stream, names a format other than PRC or
/// U3D, or decodes past [`MAX_MODEL_BYTES`].
pub fn model_3d(doc: &Document, page: usize, index: usize) -> Result<Model3d, AnnotError> {
    let list = annots(doc, page_ref(doc, page)?);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let annot = doc.dict(entry).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let at = || format!("annotation {} on page {}", index.saturating_add(1), page.saturating_add(1));
    if !is_3d(doc, &annot) {
        return Err(AnnotError::Invalid(format!("{} is not a 3D annotation", at())));
    }
    let stream = stream_of(doc, &annot).ok_or_else(|| AnnotError::Invalid(format!("{} has no 3D stream", at())))?;
    let Object::Stream(s) = stream.as_ref() else {
        return Err(AnnotError::Invalid(format!("{} has no 3D stream", at())));
    };
    let kind = match name_of(doc, &s.dict, b"Subtype").as_deref() {
        Some(b"PRC") => Model3dKind::Prc,
        Some(b"U3D") => Model3dKind::U3d,
        other => {
            let name = other.map_or_else(|| "none".to_owned(), |n| String::from_utf8_lossy(n).into_owned());
            return Err(AnnotError::Invalid(format!("{}'s 3D stream is of an unknown format ({name})", at())));
        }
    };
    let data = doc
        .with_direct_parms(s)
        .decoded_within(MAX_MODEL_BYTES)
        .map_err(|e| AnnotError::Invalid(format!("{}'s 3D stream could not be decoded: {e}", at())))?;
    Ok(Model3d { kind, data })
}

/// The name under `key`, read through a reference as the inspector reads it.
fn name_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    doc.resolve(d.get(key)?).as_name().map(<[u8]>::to_vec)
}

/// What an annotation's `/3DD` names: the 3D stream itself, or the `/3D` entry of a 3D reference
/// dictionary. A reference is followed once; the caller refuses anything that is not a stream.
fn stream_of(doc: &Document, annot: &Dict) -> Option<std::sync::Arc<Object>> {
    let obj = doc.resolve(annot.get(b"3DD")?);
    match obj.as_ref() {
        Object::Dict(d) if name_of(doc, d, b"Type").as_deref() == Some(b"3DRef") => Some(doc.resolve(d.get(b"3D")?)),
        _ => Some(obj),
    }
}

#[cfg(test)]
mod tests;
