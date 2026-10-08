//! Clean Up: links and bookmarks whose destination doesn't exist, and named destinations nothing
//! refers to.

use std::collections::{HashSet, VecDeque};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

/// Named destinations: the `/Names /Dests` tree (entries) and the old `/Dests` dictionary.
fn named_dests(doc: &Document) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let Some(cat) = doc.root().and_then(|r| doc.get(r).as_dict().cloned()) else { return out };
    if let Some(d) = cat.get(b"Dests").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()) {
        out.extend(d.iter().map(|(k, _)| k.clone()));
    }
    if let Some(tree) = names_tree(doc, &cat) {
        for (k, _) in tree_entries(doc, &tree) {
            out.insert(k);
        }
    }
    out
}

fn names_tree(doc: &Document, cat: &Dict) -> Option<Object> {
    let names = cat.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_dict().cloned())?;
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
        if let Object::Ref(r) = n
            && !seen.insert(r)
        {
            continue;
        }
        let Some(d) = doc.resolve(&n).as_dict().cloned() else { continue };
        if let Some(a) = d.get(b"Names").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned()) {
            for pair in a.chunks(2) {
                if let [k, v] = pair
                    && let Some(s) = doc.resolve(k).as_string()
                {
                    out.push((s.bytes.clone(), v.clone()));
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|x| doc.resolve(x)).and_then(|x| x.as_array().cloned()) {
            queue.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
    }
    out
}

/// The destination of a link, bookmark or action dictionary: `/Dest`, or a GoTo's `/D`.
fn destination(doc: &Document, d: &Dict) -> Option<Object> {
    if let Some(dest) = d.get(b"Dest") {
        return Some(doc.resolve(dest).as_ref().clone());
    }
    let a = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned())?;
    (a.name(b"S") == Some(b"GoTo")).then(|| a.get(b"D").map(|x| doc.resolve(x).as_ref().clone())).flatten()
}

/// The name a destination refers to, if it is a named one.
fn dest_name(o: &Object) -> Option<Vec<u8>> {
    match o {
        Object::String(s) => Some(s.bytes.clone()),
        Object::Name(n) => Some(n.to_vec()),
        _ => None,
    }
}

fn valid(o: &Object, pages: &HashSet<ObjRef>, count: usize, names: &HashSet<Vec<u8>>) -> bool {
    match o {
        Object::Array(a) => match a.first() {
            Some(Object::Ref(r)) => pages.contains(r),
            Some(Object::Int(i)) => *i >= 0 && (*i as usize) < count,
            _ => false,
        },
        other => dest_name(other).is_some_and(|n| names.contains(&n)),
    }
}

/// Remove links whose destination is missing, and clear the destination of such bookmarks
/// (they stay, so their children do too). Returns (links, bookmarks).
pub(crate) fn remove_invalid(doc: &mut Document, pages: &[ObjRef]) -> Result<(usize, usize), pdfcraft_cos::CosError> {
    let set: HashSet<ObjRef> = pages.iter().copied().collect();
    let names = named_dests(doc);
    let mut links = 0;
    for p in pages {
        let Some(pd) = doc.get(*p).as_dict().cloned() else { continue };
        let Some(annots) = pd.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()) else { continue };
        let keep: Vec<Object> = annots
            .iter()
            .filter(|x| {
                let d = doc.resolve(x).as_dict().cloned().unwrap_or_default();
                if d.name(b"Subtype") != Some(b"Link") {
                    return true;
                }
                destination(doc, &d).is_none_or(|dest| valid(&dest, &set, pages.len(), &names))
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
        .and_then(|c| c.get(b"Outlines").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().and_then(|d| d.reference(b"First"))));
    let mut stack: Vec<ObjRef> = first.into_iter().collect();
    let mut seen = HashSet::new();
    while let Some(r) = stack.pop() {
        if !seen.insert(r) || seen.len() > 100_000 {
            continue;
        }
        let Some(d) = doc.get(r).as_dict().cloned() else { continue };
        if let Some(n) = d.reference(b"Next") {
            stack.push(n);
        }
        if let Some(f) = d.reference(b"First") {
            stack.push(f);
        }
        if let Some(dest) = destination(doc, &d)
            && !valid(&dest, &set, pages.len(), &names)
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

/// Every named destination some object refers to (links, bookmarks, actions, open action).
fn referenced_names(doc: &Document) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let mut visit = |o: &Object| {
        let mut stack = vec![o.clone()];
        while let Some(o) = stack.pop() {
            match o {
                Object::Dict(d) => {
                    if let Some(n) = d.get(b"Dest").and_then(dest_name) {
                        out.insert(n);
                    }
                    if matches!(d.name(b"S"), Some(b"GoTo" | b"GoToR" | b"GoToE"))
                        && let Some(n) = d.get(b"D").and_then(dest_name)
                    {
                        out.insert(n);
                    }
                    stack.extend(d.iter().map(|(_, v)| v.clone()));
                }
                Object::Array(a) => stack.extend(a),
                Object::Stream(s) => stack.push(Object::Dict(s.dict.clone())),
                _ => {}
            }
        }
    };
    for num in doc.object_numbers() {
        visit(&doc.get(ObjRef::new(num, doc.generation(num))));
    }
    if let Some(r) = doc.root() {
        visit(&doc.get(r));
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
                Some(Object::Ref(r)) => doc.update_dict(r, |n| n.set(b"Dests".to_vec(), Object::Ref(node)))?,
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
