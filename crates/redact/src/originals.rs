//! The content streams a redacted page no longer draws (#7922).
//!
//! A page whose content changes gets new stream objects, and the originals stay: another page may
//! still draw them, in its `/Contents` or, for one that is a form XObject, with `Do`. Such an
//! original stays, with every reference to it. Anything else that names an original -- a private
//! catalog key, a structure element's `/Stm`, a resource name nothing draws, `/PieceInfo` -- would
//! have a full save write the unredacted bytes. Once the other steps have run, those references
//! are pointed at the redacted copy, and a second walk proves that no original is left for a full
//! save to write.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::RedactError;

/// Deeper than the parser reads objects written in place (128), so only an object built in
/// memory is refused.
const MAX_DIRECT_DEPTH: usize = 256;

/// `replaced` pairs each original content stream of a redacted page with its redacted copy.
/// An original some page still lists in `/Contents`, or paints as a form XObject (with `Do`, each
/// name looked up where the stream using it draws from, or as an annotation's appearance:
/// [`crate::interp::forms_pages_draw`]), stays: that page shows what it holds, and is left as
/// it was. Every other reference to an original, from any object a full save writes or from the
/// trailer, is pointed at its copy.
///
/// # Errors
///
/// [`RedactError::TooDeep`] for an object nested past [`MAX_DIRECT_DEPTH`], and
/// [`RedactError::Residue`] if an original is a form XObject and what the pages draw can't be
/// told, an original is reached only through a pattern, soft mask or Type 3 font some content
/// selects, or an original is still reachable afterwards.
pub(crate) fn release(doc: &mut Document, replaced: &[(ObjRef, ObjRef)]) -> Result<(), RedactError> {
    if replaced.is_empty() {
        return Ok(());
    }
    let drawn = page_contents(doc);
    // By number: a reference under another accepted generation names the same object.
    let mut copies: HashMap<u32, ObjRef> = HashMap::new();
    for &(original, copy) in replaced {
        if !drawn.contains(&original.num) {
            // The same original redacted on two pages: either copy holds no more than the page
            // that drew it shows.
            copies.entry(original.num).or_insert(copy);
        }
    }
    if copies.is_empty() {
        return Ok(());
    }
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    if !copies.keys().any(|num| reachable.contains(num)) {
        return Ok(());
    }
    // Only a form XObject can be drawn with `Do`, and few content streams are one. What the pages
    // draw is read only for those, and only now, as it reads every page.
    let is_form =
        |num: u32| matches!(&*doc.get(ObjRef::new(num, doc.generation(num))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form"));
    if copies.keys().any(|&num| is_form(num)) {
        let forms = crate::interp::forms_pages_draw(doc)?;
        let painted: HashSet<u32> = forms.painted.iter().map(|f| f.num).collect();
        // An original reached only through a pattern, soft mask or Type 3 font some page selects
        // may or may not be shown: keeping it could leave the marked text in the file, and
        // pointing it at the copy could change what that page shows. Refuse instead.
        if forms.selected.iter().any(|f| copies.contains_key(&f.num) && !painted.contains(&f.num)) {
            return Err(RedactError::Residue(1));
        }
        copies.retain(|num, _| !painted.contains(num));
        if copies.is_empty() {
            return Ok(());
        }
    }
    for num in reachable {
        if copies.contains_key(&num) {
            continue;
        }
        let r = ObjRef::new(num, doc.generation(num));
        let mut object = (*doc.get(r)).clone();
        if repoint(&mut object, &copies, 0)? {
            doc.set_existing(r, object)?;
        }
    }
    let mut trailer = doc.trailer().clone();
    if repoint_dict(&mut trailer, &copies, 0)? {
        *doc.trailer_mut() = trailer;
    }
    let reachable = pdfcraft_cos::reachable_objects(doc)?;
    let left = copies.keys().filter(|num| reachable.contains(num)).count();
    if left > 0 {
        return Err(RedactError::Residue(left));
    }
    Ok(())
}

/// The object numbers every page lists in `/Contents`, an indirect array's elements included.
fn page_contents(doc: &Document) -> HashSet<u32> {
    let mut drawn = HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        let Some(contents) = page.dict.get(b"Contents") else { continue };
        if let Some(r) = contents.as_ref() {
            drawn.insert(r.num);
        }
        if let Object::Array(list) = &*doc.resolve(contents) {
            drawn.extend(list.iter().filter_map(Object::as_ref).map(|r| r.num));
        }
    }
    drawn
}

/// Points every reference in `object` (and what it holds in place) that names a key of `copies`
/// at its value. Returns whether anything changed.
fn repoint(object: &mut Object, copies: &HashMap<u32, ObjRef>, depth: usize) -> Result<bool, RedactError> {
    if depth > MAX_DIRECT_DEPTH {
        return Err(RedactError::TooDeep);
    }
    let mut changed = false;
    match object {
        Object::Ref(r) => {
            if let Some(copy) = copies.get(&r.num) {
                *r = *copy;
                changed = true;
            }
        }
        Object::Array(items) => {
            for item in items {
                changed |= repoint(item, copies, depth + 1)?;
            }
        }
        Object::Dict(dict) | Object::Stream(Stream { dict, .. }) => changed = repoint_dict(dict, copies, depth + 1)?,
        _ => {}
    }
    Ok(changed)
}

/// [`repoint`] for each value of `dict`.
fn repoint_dict(dict: &mut Dict, copies: &HashMap<u32, ObjRef>, depth: usize) -> Result<bool, RedactError> {
    let keys: Vec<Vec<u8>> = dict.iter().map(|(key, _)| key.clone()).collect();
    let mut changed = false;
    for key in keys {
        if let Some(value) = dict.get_mut(&key) {
            changed |= repoint(value, copies, depth)?;
        }
    }
    Ok(changed)
}
