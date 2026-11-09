//! Clean Up: links and bookmarks whose destination doesn't exist, and named destinations nothing
//! refers to.

use std::collections::{HashSet, VecDeque};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

fn resolve(doc: &Document, obj: &Object) -> Object {
    let mut cur = obj.clone();
    for _ in 0..16 {
        match cur {
            Object::Ref(r) => cur = (*doc.get(r)).clone(),
            _ => return cur,
        }
    }
    cur
}

fn dict_name(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    d.get(key).and_then(|v| resolve(doc, v).as_name().map(<[u8]>::to_vec))
}

fn target_ref(doc: &Document, obj: &Object) -> Option<ObjRef> {
    let mut cur = obj.clone();
    let mut last = None;
    for _ in 0..16 {
        match cur {
            Object::Ref(r) => {
                last = Some(r);
                cur = (*doc.get(r)).clone();
            }
            _ => break,
        }
    }
    last
}

/// Named destinations whose target page exists: the `/Names /Dests` tree and the old `/Dests`
/// dictionary.
fn named_dests(doc: &Document, pages: &HashSet<ObjRef>, count: usize) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let Some(cat) = doc.root().and_then(|r| doc.get(r).as_dict().cloned()) else { return out };
    if let Some(d) = cat.get(b"Dests").and_then(|d| resolve(doc, d).as_dict().cloned()) {
        out.extend(d.iter().filter(|(_, v)| valid_explicit(doc, v, pages, count)).map(|(k, _)| k.clone()));
    }
    if let Some(tree) = names_tree(doc, &cat) {
        for (k, v) in tree_entries(doc, &tree) {
            if valid_explicit(doc, &v, pages, count) {
                out.insert(k);
            }
        }
    }
    out
}

fn names_tree(doc: &Document, cat: &Dict) -> Option<Object> {
    let names = cat.get(b"Names").and_then(|n| resolve(doc, n).as_dict().cloned())?;
    names.get(b"Dests").cloned()
}

/// A name tree's entries (key bytes, value), flattened.
fn tree_entries(doc: &Document, node: &Object) -> Vec<(Vec<u8>, Object)> {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([(node.clone(), 0usize)]);
    let mut seen = HashSet::new();
    while let Some((n, depth)) = queue.pop_front() {
        if depth > 64 {
            continue;
        }
        if let Some(r) = target_ref(doc, &n)
            && !seen.insert(r)
        {
            continue;
        }
        let Some(d) = resolve(doc, &n).as_dict().cloned() else { continue };
        if let Some(a) = d.get(b"Names").and_then(|x| resolve(doc, x).as_array().cloned()) {
            for pair in a.chunks(2) {
                if let [k, v] = pair
                    && let Some(s) = resolve(doc, k).as_string()
                {
                    out.push((s.bytes.clone(), v.clone()));
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").and_then(|x| resolve(doc, x).as_array().cloned()) {
            queue.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
    }
    out
}

/// The destination of a link, bookmark or action dictionary: `/Dest`, or a GoTo's `/D`.
fn destination(doc: &Document, d: &Dict) -> Option<Object> {
    if let Some(dest) = d.get(b"Dest") {
        return Some(resolve(doc, dest));
    }
    let a = d.get(b"A").and_then(|a| resolve(doc, a).as_dict().cloned())?;
    (dict_name(doc, &a, b"S").as_deref() == Some(b"GoTo")).then(|| a.get(b"D").map_or(Object::Null, |x| resolve(doc, x)))
}

/// The name a destination refers to, if it is a named one.
fn dest_name(o: &Object) -> Option<Vec<u8>> {
    match o {
        Object::String(s) => Some(s.bytes.clone()),
        Object::Name(n) => Some(n.to_vec()),
        _ => None,
    }
}

/// The `[page /Fit …]` array of an explicit destination (an array or a `<< /D […] >>` dictionary).
fn explicit_array(doc: &Document, o: &Object) -> Option<Vec<Object>> {
    match resolve(doc, o) {
        Object::Array(a) => Some(a),
        Object::Dict(d) => d.get(b"D").and_then(|x| resolve(doc, x).as_array().cloned()),
        _ => None,
    }
}

/// Whether an explicit destination targets one of the document's pages.
fn valid_explicit(doc: &Document, o: &Object, pages: &HashSet<ObjRef>, count: usize) -> bool {
    explicit_array(doc, o).is_some_and(|a| match a.first() {
        Some(first) => {
            if let Some(i) = resolve(doc, first).as_int() {
                usize::try_from(i).is_ok_and(|i| i < count)
            } else {
                target_ref(doc, first).is_some_and(|r| pages.contains(&r))
            }
        }
        None => false,
    })
}

fn valid(doc: &Document, o: &Object, pages: &HashSet<ObjRef>, count: usize, names: &HashSet<Vec<u8>>) -> bool {
    valid_explicit(doc, o, pages, count) || dest_name(o).is_some_and(|n| names.contains(&n))
}

/// Remove links whose destination is missing, and clear the destination of such bookmarks
/// (they stay, so their children do too). Returns (links, bookmarks).
pub(crate) fn remove_invalid(doc: &mut Document, pages: &[ObjRef]) -> Result<(usize, usize), pdfcraft_cos::CosError> {
    let set: HashSet<ObjRef> = pages.iter().copied().collect();
    let names = named_dests(doc, &set, pages.len());
    let mut links = 0;
    for p in pages {
        let Some(pd) = doc.get(*p).as_dict().cloned() else { continue };
        let Some(annots) = pd.get(b"Annots").and_then(|a| resolve(doc, a).as_array().cloned()) else { continue };
        let keep: Vec<Object> = annots
            .iter()
            .filter(|x| {
                let d = resolve(doc, x).as_dict().cloned().unwrap_or_default();
                if dict_name(doc, &d, b"Subtype").as_deref() != Some(b"Link") {
                    return true;
                }
                destination(doc, &d).is_none_or(|dest| valid(doc, &dest, &set, pages.len(), &names))
            })
            .cloned()
            .collect();
        if keep.len() != annots.len() {
            links += annots.len() - keep.len();
            doc.update_dict(*p, |d| d.set(b"Annots".to_vec(), Object::Array(keep)))?;
        }
    }
    // Bookmarks, depth-first.
    let mut bookmarks = 0;
    let first = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"Outlines").and_then(|o| resolve(doc, o).as_dict().and_then(|d| d.get(b"First").and_then(|f| target_ref(doc, f)))));
    let mut stack: Vec<ObjRef> = first.into_iter().collect();
    let mut seen = HashSet::new();
    while let Some(r) = stack.pop() {
        if seen.len() >= 100_000 || !seen.insert(r) {
            continue;
        }
        let Some(d) = doc.get(r).as_dict().cloned() else { continue };
        if let Some(n) = d.get(b"Next").and_then(|n| target_ref(doc, n)) {
            stack.push(n);
        }
        if let Some(f) = d.get(b"First").and_then(|f| target_ref(doc, f)) {
            stack.push(f);
        }
        if let Some(dest) = destination(doc, &d)
            && !valid(doc, &dest, &set, pages.len(), &names)
        {
            doc.update_dict(r, |d| {
                d.remove(b"Dest");
                d.remove(b"A");
            })?;
            bookmarks += 1;
        }
    }
    Ok((links, bookmarks))
}

/// Every named destination some reachable object refers to (links, bookmarks, actions, open action).
fn referenced_names(doc: &Document) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    let mut stack = vec![Object::Dict(doc.trailer().clone())];
    if let Some(r) = doc.root() {
        stack.push(Object::Ref(r));
    }
    while let Some(o) = stack.pop() {
        match o {
            Object::Ref(r) => {
                if seen.insert(r) {
                    stack.push(doc.get(r).as_ref().clone());
                }
            }
            Object::Dict(d) => {
                // Resolve first: /Dest and a GoTo's /D may be an indirect reference to the name
                // string, and an unresolved ref has no name, so the destination it uses would
                // look unreferenced and be removed. A GoToR or GoToE /D names a destination in
                // another document, so it does not count as a use of this one's.
                if let Some(n) = d.get(b"Dest").and_then(|x| dest_name(&resolve(doc, x))) {
                    out.insert(n);
                }
                if dict_name(doc, &d, b"S").as_deref() == Some(b"GoTo")
                    && let Some(n) = d.get(b"D").and_then(|x| dest_name(&resolve(doc, x)))
                {
                    out.insert(n);
                }
                stack.extend(d.iter().map(|(_, v)| v.clone()));
            }
            Object::Array(a) => stack.extend(a),
            Object::Stream(s) => stack.push(Object::Dict(s.dict)),
            _ => {}
        }
    }
    out
}

/// Remove named destinations nothing refers to. Returns how many went.
pub(crate) fn remove_unreferenced_dests(doc: &mut Document) -> Result<usize, pdfcraft_cos::CosError> {
    let Some(root) = doc.root() else { return Ok(0) };
    let cat = doc.get(root).as_dict().cloned().unwrap_or_default();
    let used = referenced_names(doc);
    let mut removed = 0;
    if let Some(tree) = names_tree(doc, &cat) {
        let entries = tree_entries(doc, &tree);
        let keep: Vec<(Vec<u8>, Object)> = entries.iter().filter(|(k, _)| used.contains(k)).cloned().collect();
        if keep.len() != entries.len() {
            removed += entries.len() - keep.len();
            let mut keep = keep;
            keep.sort_by(|a, b| a.0.cmp(&b.0));
            let mut arr = Vec::with_capacity(keep.len() * 2);
            for (k, v) in keep {
                arr.push(Object::String(PdfString::literal(k)));
                arr.push(v);
            }
            let mut node = Dict::new();
            node.set(b"Names".to_vec(), Object::Array(arr));
            let node = doc.add(Object::Dict(node));
            let names_obj = cat.get(b"Names").cloned();
            match names_obj {
                Some(obj @ Object::Ref(_)) => {
                    // Read and write the same terminal dictionary; keep the reference chain intact.
                    if let Some(r) = target_ref(doc, &obj).filter(|r| doc.get(*r).as_dict().is_some()) {
                        doc.update_dict(r, |n| n.set(b"Dests".to_vec(), Object::Ref(node)))?;
                    }
                }
                _ => doc.update_dict(root, |c| {
                    if let Some(Object::Dict(n)) = c.get_mut(b"Names") {
                        n.set(b"Dests".to_vec(), Object::Ref(node));
                    }
                })?,
            }
        }
    }
    if let Some(old) = cat.get(b"Dests") {
        let d = doc.resolve(old).as_dict().cloned().unwrap_or_default();
        let mut keep = Dict::new();
        for (k, v) in d.iter() {
            if used.contains(k) {
                keep.set(k.clone(), v.clone());
            } else {
                removed += 1;
            }
        }
        if keep.len() != d.len() {
            match old {
                Object::Ref(r) => doc.set(*r, Object::Dict(keep)),
                _ => doc.update_dict(root, |c| c.set(b"Dests".to_vec(), Object::Dict(keep)))?,
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::PdfString;

    /// Indirect integer destinations receive the same range check as direct indices.
    #[test]
    fn link_cleanup_preserves_indirect_integer_destinations() {
        for index in [-1, 0, 1] {
            for hops in 0..=2 {
                let mut doc = Document::new_empty();
                let root = doc.root().unwrap();
                let pages = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
                let mut first = Object::Int(index);
                for _ in 0..hops {
                    first = Object::Ref(doc.add(first));
                }
                let mut link = Dict::new();
                link.set(b"Subtype".to_vec(), Object::name("Link"));
                link.set(b"Dest".to_vec(), Object::Array(vec![first, Object::name("Fit")]));
                let link = doc.add(link);
                let mut page = Dict::new();
                page.set(b"Type".to_vec(), Object::name("Page"));
                page.set(b"Parent".to_vec(), Object::Ref(pages));
                page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()]));
                page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(link)]));
                let page = doc.add(page);
                doc.update_dict(pages, |d| {
                    d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
                    d.set(b"Count".to_vec(), Object::Int(1));
                })
                .unwrap();
                assert_eq!(remove_invalid(&mut doc, &[page]).unwrap().0, usize::from(index != 0), "index={index}, hops={hops}");
                let reopened =
                    Document::open(std::sync::Arc::new(pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap())).unwrap();
                let pages = reopened.get(reopened.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
                let page = reopened.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0].as_ref().unwrap();
                assert_eq!(reopened.get(page).as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap().len(), usize::from(index == 0));
            }
        }
    }

    #[test]
    fn invalid_names_chains_are_left_unchanged() {
        let mut doc = Document::new_empty();
        let root = doc.root().unwrap();
        let cycle = doc.add(Object::Null);
        doc.set(cycle, Object::Ref(cycle));
        let scalar = doc.add(Object::Int(7));
        let missing = ObjRef::new(999, 3);
        for value in [Object::Ref(cycle), Object::Ref(scalar), Object::Ref(missing), Object::Null] {
            doc.update_dict(root, |c| c.set(b"Names".to_vec(), value.clone())).unwrap();
            assert_eq!(remove_unreferenced_dests(&mut doc).unwrap(), 0);
            assert_eq!(doc.get(root).as_dict().unwrap().get(b"Names"), Some(&value));
        }
    }

    /// Cleanup mutates the terminal Names dictionary without replacing reference hops or siblings.
    #[test]
    fn chained_names_cleanup_preserves_siblings_and_roundtrips() {
        for hops in [0, 1, 2, 15] {
            let mut doc = Document::new_empty();
            let root = doc.root().unwrap();
            let mut tree = Dict::new();
            tree.set(
                b"Names".to_vec(),
                Object::Array(vec![Object::String(PdfString::text("kept")), Object::Null, Object::String(PdfString::text("unused")), Object::Null]),
            );
            let mut names = Dict::new();
            names.set(b"Dests".to_vec(), Object::Dict(tree));
            names.set(b"OtherTree".to_vec(), Object::Int(42));
            let mut value = Object::Dict(names);
            for _ in 0..hops {
                value = Object::Ref(doc.add(value));
            }
            doc.update_dict(root, |c| {
                c.set(b"Names".to_vec(), value.clone());
                let mut action = Dict::new();
                action.set(b"S".to_vec(), Object::name("GoTo"));
                action.set(b"D".to_vec(), Object::String(PdfString::text("kept")));
                c.set(b"OpenAction".to_vec(), Object::Dict(action));
            })
            .unwrap();
            let report = crate::optimize(&mut doc, &crate::Settings::default()).unwrap();
            assert_eq!(report.unreferenced_dests, 1, "hops={hops}");
            if hops > 0 {
                assert_eq!(doc.get(root).as_dict().unwrap().get(b"Names"), Some(&value));
            }
            let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
            let reopened = Document::open(std::sync::Arc::new(bytes)).unwrap();
            for doc in [&doc, &reopened] {
                let cat = doc.get(doc.root().unwrap()).as_dict().unwrap().clone();
                let names = resolve(doc, cat.get(b"Names").unwrap());
                assert_eq!(names.as_dict().unwrap().int(b"OtherTree"), Some(42));
                let entries = tree_entries(doc, &names_tree(doc, &cat).unwrap());
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, b"kept");
            }
        }
    }

    #[test]
    fn remove_invalid_and_unreferenced_handle_indirect_subtypes_broken_goto_and_orphans() {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        let page = doc.add(Object::Dict(p));
        doc.update_dict(pages, |d| {
            d.set(b"Count".to_vec(), Object::Int(1));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();

        let s = |t: &str| Object::String(PdfString::literal(t.as_bytes().to_vec()));
        let link_sub = doc.add(Object::name("Link"));
        let goto_name = doc.add(Object::name("GoTo"));

        // 1. Valid GoTo link with indirect /Subtype /Link, indirect /S /GoTo, and 2-hop /D -> "kept".
        let kept_str = doc.add(s("kept"));
        let kept_chain = doc.add(Object::Ref(kept_str));
        let mut valid_action = Dict::new();
        valid_action.set(b"S".to_vec(), Object::Ref(goto_name));
        valid_action.set(b"D".to_vec(), Object::Ref(kept_chain));
        let mut valid_link = Dict::new();
        valid_link.set(b"Subtype".to_vec(), Object::Ref(link_sub));
        valid_link.set(b"A".to_vec(), Object::Dict(valid_action));
        let valid_link = doc.add(Object::Dict(valid_link));

        // 2. Broken GoTo link with indirect /Subtype /Link and missing /D.
        let mut empty_goto = Dict::new();
        empty_goto.set(b"S".to_vec(), Object::Ref(goto_name));
        let mut broken_link = Dict::new();
        broken_link.set(b"Subtype".to_vec(), Object::Ref(link_sub));
        broken_link.set(b"A".to_vec(), Object::Dict(empty_goto));
        let broken_link = doc.add(Object::Dict(broken_link));

        // 3. Orphaned link object (not in /Annots) pointing to "orphan_only".
        let mut orphan = Dict::new();
        orphan.set(b"Subtype".to_vec(), Object::name("Link"));
        orphan.set(b"Dest".to_vec(), s("orphan_only"));
        let _orphan = doc.add(Object::Dict(orphan));

        doc.update_dict(page, |d| {
            d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(valid_link), Object::Ref(broken_link)]));
        })
        .unwrap();

        let fit = Object::Array(vec![Object::Ref(page), Object::name("Fit")]);
        let mut tree = Dict::new();
        tree.set(b"Names".to_vec(), Object::Array(vec![s("kept"), fit.clone(), s("orphan_only"), fit]));
        let tree = doc.add(Object::Dict(tree));
        let mut names = Dict::new();
        names.set(b"Dests".to_vec(), Object::Ref(tree));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names))).unwrap();

        let (invalid_links, invalid_bookmarks) = remove_invalid(&mut doc, &[page]).unwrap();
        assert_eq!((invalid_links, invalid_bookmarks), (1, 0));

        let unref = remove_unreferenced_dests(&mut doc).unwrap();
        assert_eq!(unref, 1, "orphan_only is removed while kept (via 2-hop /D and indirect /S) stays");
    }

    #[test]
    fn links_cleanup_terminates_on_cyclic_refs_and_clears_broken_bookmark_goto() {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        let page = doc.add(Object::Dict(p));
        doc.update_dict(pages, |d| {
            d.set(b"Count".to_vec(), Object::Int(1));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();

        let cyc = doc.add(Object::Null);
        doc.set(cyc, Object::Ref(cyc));

        // Bookmark with a GoTo action pointing to a cyclic /D reference and Next pointing to itself.
        let mut action = Dict::new();
        action.set(b"S".to_vec(), Object::name("GoTo"));
        action.set(b"D".to_vec(), Object::Ref(cyc));
        let action_ref = doc.add(Object::Dict(action));

        let item = doc.add(Object::Null);
        let mut item_d = Dict::new();
        item_d.set(b"A".to_vec(), Object::Ref(action_ref));
        item_d.set(b"Next".to_vec(), Object::Ref(item));
        doc.set(item, Object::Dict(item_d));

        let mut outlines = Dict::new();
        let item_chain = doc.add(Object::Ref(item));
        outlines.set(b"First".to_vec(), Object::Ref(item_chain));
        let outlines_ref = doc.add(Object::Dict(outlines));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Outlines".to_vec(), Object::Ref(outlines_ref))).unwrap();

        let (links, bookmarks) = remove_invalid(&mut doc, &[page]).unwrap();
        assert_eq!((links, bookmarks), (0, 1));
        assert!(!doc.get(item).as_dict().unwrap().contains(b"A"));
    }
}
