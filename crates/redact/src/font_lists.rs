//! A simple font's lists of the characters it shows, after Apply Redactions. The `/ToUnicode`
//! map and the `/Widths` array list the characters a font's text uses, so after that text is gone
//! they would still say which characters it had. What the marked pages show before the apply is
//! read first ([`crate::type3::shown_codes`]); [`prune`] reads what the whole document shows after
//! it, and a code shown before and nowhere after leaves its font's map and gets a width of 0.
//!
//! Left as they were: form field fonts (`/AcroForm /DR`), and all fonts when the document has hidden
//! page templates or anything can't be read.
//! Composite fonts, a subset's `/CharSet` and the glyph outlines in the font program stay too.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object, Stream};

/// After an apply: take the codes `before` lists and the document shows nowhere now out of their
/// simple fonts' lists.
pub(crate) fn prune(doc: &mut Document, before: Option<HashMap<ObjRef, HashSet<u32>>>) -> Result<(), CosError> {
    let Some(before) = before.filter(|b| !b.is_empty()) else { return Ok(()) };
    let Some(after) = crate::type3::shown_codes(doc, None) else { return Ok(()) };
    let Some(fields) = form_fonts(doc) else { return Ok(()) };
    for (font, codes) in before {
        let gone: HashSet<u32> = codes.into_iter().filter(|c| !after.get(&font).is_some_and(|a| a.contains(c))).collect();
        if !gone.is_empty() && !fields.contains(&font) {
            simple_font(doc, font, &gone)?;
        }
    }
    Ok(())
}

/// The fonts in `/AcroForm /DR`: form fields write new text with them. `None` when the document
/// has hidden page templates (`/Names /Templates`): a form can show their pages later, and they
/// are out of the page tree the scan reads. Also `None` when the catalog, `/Names`, `/AcroForm`,
/// `/DR` or its `/Font` is there but can't be read as a dictionary.
fn form_fonts(doc: &Document) -> Option<HashSet<ObjRef>> {
    let mut dict = doc.get(doc.root()?).as_dict()?.clone();
    if dict.get(b"Names").is_some_and(|n| doc.resolve(n).as_dict().is_none_or(|n| n.contains(b"Templates"))) {
        return None;
    }
    for key in [&b"AcroForm"[..], b"DR", b"Font"] {
        let Some(entry) = dict.get(key) else { return Some(HashSet::new()) };
        dict = doc.resolve(entry).as_dict()?.clone();
    }
    Some(dict.iter().filter_map(|(_, v)| v.as_ref()).collect())
}

/// A simple font loses `gone` from its `/ToUnicode` map (written as a new object, since another
/// font may share it) and from its `/Widths` (set to 0).
fn simple_font(doc: &mut Document, font: ObjRef, gone: &HashSet<u32>) -> Result<(), CosError> {
    let Some(d) = doc.get(font).as_dict().cloned() else { return Ok(()) };
    let subtype = d.get(b"Subtype").map(|s| doc.resolve(s));
    if !matches!(subtype.as_deref().and_then(Object::as_name), Some(b"Type1" | b"TrueType" | b"MMType1")) {
        return Ok(());
    }
    let map = d.get(b"ToUnicode").map(|m| doc.resolve(m)).and_then(|m| match &*m {
        // A map built on another (`/UseCMap`) can't be written again without its base: it stays.
        Object::Stream(s) if !s.dict.contains(b"UseCMap") => {
            doc.with_direct_parms(s).decoded().ok().and_then(|data| pdfcraft_fonts::pdf::to_unicode_without(&data, gone))
        }
        _ => None,
    });
    if let Some(map) = map {
        let map = doc.add(Object::Stream(Stream::flate(Dict::new(), &map)));
        doc.update_dict(font, |f| f.set(b"ToUnicode".to_vec(), Object::Ref(map)))?;
    }
    let first = d.get(b"FirstChar").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    if let Some(mut widths) = d.get(b"Widths").and_then(|w| doc.resolve(w).as_array().cloned()) {
        for code in gone {
            let at = i64::from(*code).checked_sub(first).and_then(|i| usize::try_from(i).ok());
            if let Some(slot) = at.and_then(|i| widths.get_mut(i)) {
                *slot = Object::Int(0);
            }
        }
        doc.update_dict(font, |f| f.set(b"Widths".to_vec(), Object::Array(widths)))?;
    }
    Ok(())
}
