//! The annotations a new layout does not generate: comments, markups and the links a user added.
//! Laying a form out again replaces its pages (ISO 32000-2 §7.7.3.2), and with them each page's
//! `/Annots` (§12.5.1); a layout writes only the widgets and the links of its rich text, so the
//! rest is taken off the old pages first and put back on the new ones.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Most pages read for annotations to carry over.
const MAX_PAGES: usize = 10_000;
/// Most page-tree nodes visited while finding them (intermediate nodes included).
const MAX_NODES: usize = 20_000;
/// Most levels of the page tree followed.
const MAX_DEPTH: usize = 64;
/// Most annotations carried over in one layout.
const MAX_CARRIED: usize = 100_000;
/// Most `/Annots` entries read in one layout, over all pages (pages may share one long array).
const MAX_READ: usize = 1_000_000;

/// The annotations of the document's pages that a layout does not write, each with the index of
/// the page it is on.
pub(crate) struct Carried {
    annots: Vec<(usize, Object)>,
    /// Annotations past [`MAX_CARRIED`], not carried over (said in a warning).
    left: usize,
    /// `/Annots` entries past [`MAX_READ`] were not read (said in a warning).
    cut: bool,
}

/// Whether `d` is an annotation a layout writes: a widget (every field's widget is generated
/// again from the template), or a link over rich text as `Emitter::text_link` writes it. That one
/// has exactly these keys, an invisible `/Border` and a URI action; a link added by hand has a
/// border style, colour and highlight mode (`/BS`, `/C`, `/H`) instead, or keys of its own.
fn written_by_layout(doc: &Document, d: &Dict) -> bool {
    match d.name(b"Subtype") {
        Some(b"Widget") => true,
        Some(b"Link") => {
            const KEYS: [&[u8]; 7] = [b"Type", b"Subtype", b"Rect", b"P", b"F", b"Border", b"A"];
            let only_those = d.iter().all(|(k, _)| KEYS.contains(&k.as_slice()));
            let invisible = d
                .get(b"Border")
                .map(|b| doc.resolve(b))
                .and_then(|b| b.as_array().cloned())
                .is_some_and(|b| b.len() == 3 && b.iter().all(|v| v.as_f64() == Some(0.0)));
            let uri = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().map(|a| a.name(b"S") == Some(b"URI".as_slice()))).unwrap_or(false);
            only_those && invisible && uri && d.int(b"F") == Some(4)
        }
        _ => false,
    }
}

/// Take note of the annotations on `doc`'s pages that a layout would not write again, in page
/// order. The page tree is walked with a seen-set and depth, node and page limits.
pub(crate) fn take(doc: &Document) -> Carried {
    let mut out = Carried { annots: Vec::new(), left: 0, cut: false };
    let Some(top) = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages")).and_then(Object::as_ref)) else {
        return out;
    };
    let mut seen = HashSet::new();
    let mut stack = vec![(top, 0usize)];
    let mut page = 0usize;
    // An annotation listed twice (on two pages, or twice on one) is carried once, where first met.
    let mut carried = HashSet::new();
    let mut read = 0usize;
    while let Some((node, depth)) = stack.pop() {
        if seen.len() >= MAX_NODES || page >= MAX_PAGES || !seen.insert(node.num) {
            continue;
        }
        let object = doc.get(node);
        let Some(d) = object.as_dict() else { continue };
        let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned());
        if d.name(b"Type") == Some(b"Pages".as_slice()) || (kids.is_some() && d.name(b"Type") != Some(b"Page".as_slice())) {
            if depth < MAX_DEPTH {
                // Pushed in reverse, so the first kid is visited first.
                let kids = kids.unwrap_or_default();
                let room = MAX_NODES.saturating_sub(seen.len());
                stack.extend(kids.iter().take(room).rev().filter_map(Object::as_ref).map(|k| (k, depth + 1)));
            }
            continue;
        }
        let annots = d.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in annots {
            if read >= MAX_READ {
                out.cut = true;
                break;
            }
            read += 1;
            if let Object::Ref(r) = &a
                && !carried.insert(r.num)
            {
                continue;
            }
            let resolved = doc.resolve(&a);
            let Some(ad) = resolved.as_dict() else { continue };
            if written_by_layout(doc, ad) {
                continue;
            }
            if out.annots.len() >= MAX_CARRIED {
                out.left = out.left.saturating_add(1);
                continue;
            }
            out.annots.push((page, a));
        }
        page += 1;
    }
    out
}

/// Put `carried` back on the pages `doc` now has: each on the page with its old index, or, when
/// the new layout has fewer pages, on its last page (with a warning, as the rest of what was on
/// those pages is gone). Each annotation's `/P` names its new page (§12.5.2, Table 166). Returns
/// the warnings.
pub(crate) fn put_back(doc: &mut Document, carried: Carried) -> Result<Vec<String>, crate::XfaError> {
    let mut warnings = Vec::new();
    if carried.cut {
        warnings.push(format!(
            "The pages list more than {MAX_READ} annotations; those past them were not read, and not carried over when the form was laid out again"
        ));
    }
    if carried.left > 0 {
        warnings.push(format!("{} annotation(s) past the first {MAX_CARRIED} were not carried over when the form was laid out again", carried.left));
    }
    if carried.annots.is_empty() {
        return Ok(warnings);
    }
    let pages: Vec<ObjRef> = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages")).map(|p| doc.resolve(p)))
        .and_then(|p| p.as_dict().and_then(|d| d.get(b"Kids")).map(|k| doc.resolve(k)))
        .and_then(|k| k.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Object::as_ref)
        .collect();
    let Some(&last) = pages.last() else {
        warnings.push(format!("{} annotation(s) were not carried over: the form was laid out to no pages", carried.annots.len()));
        return Ok(warnings);
    };
    let mut moved = 0usize;
    let mut by_page: Vec<Vec<Object>> = vec![Vec::new(); pages.len()];
    for (index, mut a) in carried.annots {
        let (slot, target) = match pages.get(index) {
            Some(&p) => (index, p),
            None => {
                moved = moved.saturating_add(1);
                (pages.len().saturating_sub(1), last)
            }
        };
        match &mut a {
            Object::Ref(r) => {
                let r = *r;
                if doc.get(r).as_dict().is_some() {
                    doc.update_dict(r, |d| d.set(b"P".to_vec(), Object::Ref(target)))?;
                }
            }
            Object::Dict(d) => d.set(b"P".to_vec(), Object::Ref(target)),
            _ => {}
        }
        if let Some(list) = by_page.get_mut(slot) {
            list.push(a);
        }
    }
    for (page, extra) in pages.iter().zip(by_page) {
        if extra.is_empty() {
            continue;
        }
        let at = doc.get(*page).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref));
        let mut list =
            doc.get(*page).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        // After the page's widgets and links: drawn over them, as they were (§12.5.1).
        list.extend(extra);
        match at {
            Some(a) => doc.set(a, Object::Array(list)),
            None => doc.update_dict(*page, |d| d.set(b"Annots".to_vec(), Object::Array(list)))?,
        }
    }
    if moved > 0 {
        warnings.push(format!("{moved} annotation(s) were on pages the form no longer lays out, and were moved to its last page"));
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: &[(&[u8], Object)]) -> Object {
        let mut d = Dict::new();
        for (k, v) in entries {
            d.set(k.to_vec(), v.clone());
        }
        Object::Dict(d)
    }

    /// A page tree whose node lists itself and its parent, and three pages sharing one `/Annots`
    /// array of 1.2 million entries naming one comment: the walk ends, reads at most
    /// [`MAX_READ`] entries, carries the comment once, and says it stopped reading.
    #[test]
    fn hostile_page_trees_and_shared_annotation_lists_are_read_within_the_limits() {
        let mut doc = Document::new_empty();
        let note = doc.add(dict(&[(b"Type", Object::name("Annot")), (b"Subtype", Object::name("Text"))]));
        let list = doc.add(Object::Array(vec![Object::Ref(note); 1_200_000]));
        let pages = doc.add(Object::Null);
        let node = doc.add(Object::Null);
        let page = |doc: &mut Document| doc.add(dict(&[(b"Type", Object::name("Page")), (b"Annots", Object::Ref(list))]));
        let kids: Vec<Object> = (0..3).map(|_| Object::Ref(page(&mut doc))).collect();
        doc.set(
            node,
            dict(&[(b"Type", Object::name("Pages")), (b"Kids", Object::Array([kids, vec![Object::Ref(node), Object::Ref(pages)]].concat()))]),
        );
        doc.set(pages, dict(&[(b"Type", Object::name("Pages")), (b"Kids", Object::Array(vec![Object::Ref(node)]))]));
        let catalog = doc.add(dict(&[(b"Type", Object::name("Catalog")), (b"Pages", Object::Ref(pages))]));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(catalog));
        let carried = take(&doc);
        assert_eq!(carried.annots.len(), 1);
        assert!(carried.cut);
        assert_eq!(carried.left, 0);
    }
}
