use super::info::text_value;
use super::types::{FLAG_HIDDEN, FLAG_NO_ROTATE, FLAG_NO_VIEW, Shape};
use super::{AnnotError, Dict, Document, ObjRef, Object, appearance};

// ── pages and /Annots ───────────────────────────────────────────────────────────────────────

/// The page object of each leaf page, in order: the shared page-tree walk
/// (`pdfcraft_model::PageLeaves`). `NoPageTree` when the catalog names no page tree, and
/// `PageTree` when the walk meets one of its limits before the end of the tree.
pub fn page_refs(doc: &Document) -> Result<Vec<ObjRef>, AnnotError> {
    let root = doc.root().ok_or(AnnotError::NoPageTree)?;
    let pages = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")).ok_or(AnnotError::NoPageTree)?;
    Ok(pdfcraft_model::PageLeaves::under(doc, pages).map(|leaf| leaf.map(|leaf| leaf.obj)).collect::<Result<_, _>>()?)
}

/// Each leaf page of the whole tree, in order, with its `/Annots` entries (references or inline
/// dictionaries); an `Err` where the walk exceeds one of its limits. `/Annots` isn't inherited, so
/// nothing is copied. A change to annotations across the document collects this before it changes
/// anything; a reader may stop at the first `Err`.
pub fn page_annots(doc: &Document) -> impl Iterator<Item = Result<(ObjRef, Vec<Object>), pdfcraft_model::PageTreeLimit>> + '_ {
    pdfcraft_model::PageLeaves::new(doc).map(|leaf| leaf.map(|leaf| (leaf.obj, annots(doc, leaf.obj))))
}

pub(crate) fn page_ref(doc: &Document, page: usize) -> Result<ObjRef, AnnotError> {
    page_refs(doc)?.get(page).copied().ok_or(AnnotError::NoSuchPage(page))
}

/// The entries of a page's `/Annots` (references or inline dictionaries).
pub(crate) fn annots(doc: &Document, page: ObjRef) -> Vec<Object> {
    let obj = doc.get(page);
    let Some(a) = obj.as_dict().and_then(|d| d.get(b"Annots")) else { return Vec::new() };
    let list = doc.resolve(a).as_array().cloned().unwrap_or_default();
    #[cfg(test)]
    ANNOTS_ENTRIES_COPIED.with(|n| n.set(n.get() + list.len()));
    list
}

#[cfg(test)]
thread_local! {
    /// Entries copied out of pages' `/Annots` arrays.
    pub(crate) static ANNOTS_ENTRIES_COPIED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Replace a page's `/Annots`, writing into the shared array object when it is indirect. An empty
/// list removes a direct `/Annots`.
pub fn set_annots(doc: &mut Document, page: ObjRef, list: Vec<Object>) -> Result<(), pdfcraft_cos::CosError> {
    let existing = doc.get(page).as_dict().and_then(|d| d.reference(b"Annots"));
    match existing {
        Some(r) if doc.get(r).as_array().is_some() => doc.set(r, Object::Array(list)),
        _ => doc.update_dict(page, |d| {
            if list.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(list));
            }
        })?,
    }
    Ok(())
}

/// The annotation at `(page, index)` as an indirect object (inline dictionaries are promoted).
pub(crate) fn annot_ref(doc: &mut Document, page: usize, index: usize) -> Result<(ObjRef, ObjRef), AnnotError> {
    let p = page_ref(doc, page)?;
    let mut list = annots(doc, p);
    let entry = list.get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let r = match entry {
        Object::Ref(r) if doc.get(r).as_dict().is_some() => r,
        Object::Dict(d) => {
            let r = doc.add(Object::Dict(d));
            list[index] = Object::Ref(r);
            set_annots(doc, p, list)?;
            r
        }
        _ => return Err(AnnotError::NoSuchAnnotation { page, index }),
    };
    Ok((p, r))
}

pub(crate) fn annot_dict(doc: &Document, r: ObjRef) -> Dict {
    doc.get(r).as_dict().cloned().unwrap_or_default()
}

/// A Fill & Sign item: typed text, a check/cross/dot/line, or a typed, drawn or image signature.
///
/// New items carry `/PCFillSign`. Older files are recognised from the dictionaries PdfCraft
/// already writes (`/IT /FreeTextTypeWriter`, stamp `/Name`, drawn ink whose subject is
/// "Signature").
pub fn is_fill_sign(doc: &Document, d: &Dict) -> bool {
    if matches!(d.get(b"PCFillSign").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true))) {
        return true;
    }
    let name_is = |key: &[u8], expected: &[u8]| d.get(key).is_some_and(|o| doc.resolve(o).as_name() == Some(expected));
    if name_is(b"Subtype", b"FreeText") && name_is(b"IT", b"FreeTextTypeWriter") {
        return true;
    }
    if name_is(b"Subtype", b"Stamp") {
        if matches!(d.name(b"Name"), Some(b"PCCheck" | b"PCCross" | b"PCDot" | b"PCLine" | b"PCTypedSignature")) {
            return true;
        }
        if matches!(d.name(b"Name"), Some(b"PCCustomSignature" | b"PCCustomInitials"))
            && matches!(d.get(b"PCPictureImage").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true)))
        {
            return true;
        }
    }
    name_is(b"Subtype", b"Ink") && text_value(doc, d, b"Subj").as_deref() == Some("Signature")
}

pub(crate) fn annotation_flags(doc: &Document, d: &Dict) -> i64 {
    d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0)
}

/// Whether the renderer keeps the annotation upright on a turned page: the NoRotate flag, with
/// `/F` read as the renderer reads it (a real truncated, a value that isn't a `u32` no flags).
pub(crate) fn drawn_upright(doc: &Document, d: &Dict) -> bool {
    let f = match d.get(b"F").map(|f| doc.resolve(f)).as_deref() {
        Some(Object::Int(i)) => *i,
        Some(Object::Real(r)) => *r as i64,
        _ => 0,
    };
    u32::try_from(f).is_ok_and(|f| i64::from(f) & FLAG_NO_ROTATE != 0)
}

/// Whether any page has a Fill & Sign annotation that flatten would bake in (not hidden).
pub fn has_visible_fill_sign(doc: &Document) -> bool {
    let Ok(pages) = page_refs(doc) else { return false };
    for page in pages {
        for entry in annots(doc, page) {
            let obj = doc.resolve(&entry);
            let Some(d) = obj.as_dict() else { continue };
            let flags = annotation_flags(doc, d);
            if flags & (FLAG_HIDDEN | FLAG_NO_VIEW) != 0 {
                continue;
            }
            if is_fill_sign(doc, d) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn fill_sign_shape(shape: &Shape) -> bool {
    match shape {
        Shape::Typewriter { .. } | Shape::Mark { .. } | Shape::Signature { .. } | Shape::TypedSignature { .. } => true,
        Shape::CustomStamp { name, image: true, .. } => name == "Signature" || name == "Initials",
        _ => false,
    }
}

/// The embedded image of a Fill & Sign image signature or initials (0-based target).
/// Other stamps have appearances that can't be represented by this image alone.
pub fn signature_image(doc: &Document, page: usize, index: usize) -> Result<Option<ObjRef>, AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let obj = doc.resolve(entry);
    let Some(d) = obj.as_dict() else { return Ok(None) };
    Ok((d.name(b"Subtype") == Some(b"Stamp")
        && matches!(d.name(b"Name"), Some(b"PCCustomSignature" | b"PCCustomInitials"))
        && matches!(d.get(b"PCPictureImage"), Some(Object::Bool(true))))
    .then(|| d.reference(b"PCPicture"))
    .flatten())
}

/// The page `/Rotate` the picture of the image signature at `(page, index)` is drawn turned back
/// by, so it reads upright on a page shown that way: 0 for one added to an unturned page, or by
/// another app. The picture appears turned by the page's current rotation less this.
pub fn picture_rotation(doc: &Document, page: usize, index: usize) -> Result<i64, AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    Ok(doc.resolve(entry).as_dict().map_or(0, appearance::picture_turn))
}

/// Leave page `page` of (a copy of) a document showing only its annotation at `index`, without
/// the page's content or other annotations, so a render draws that annotation alone: to lay it
/// over the page while it is dragged. `false`, with `doc` unchanged, when the annotation has no
/// normal appearance (`/AP /N`) for that render to draw.
pub fn isolate_annotation(doc: &mut Document, page: usize, index: usize) -> Result<bool, AnnotError> {
    isolate_annotations(doc, page, &[index])
}

/// [`isolate_annotation`] for several annotations of one page: those of them with a normal
/// appearance stay, in their order on the page. `false`, with `doc` unchanged, when none has one.
/// An index that isn't one of the page's annotations is an error even when the others are, and
/// leaves `doc` unchanged too.
pub fn isolate_annotations(doc: &mut Document, page: usize, indices: &[usize]) -> Result<bool, AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let mut kept = Vec::new();
    for (index, entry) in list.iter().enumerate() {
        if !indices.contains(&index) {
            continue;
        }
        let obj = doc.resolve(entry);
        if obj.as_dict().and_then(|d| d.get(b"AP")).is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N"))) {
            kept.push(entry.clone());
        }
    }
    if let Some(&index) = indices.iter().find(|&&i| i >= list.len()) {
        return Err(AnnotError::NoSuchAnnotation { page, index });
    }
    if kept.is_empty() {
        return Ok(false);
    }
    set_annots(doc, p, kept)?;
    doc.update_dict(p, |d| {
        d.remove(b"Contents");
    })?;
    Ok(true)
}
