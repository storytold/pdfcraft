//! Redaction and the structure tree: tags must not keep what was removed from the page.
//!
//! Marked content (MCIDs) that applying redactions changed is found by comparing each MCID's
//! operators before and after. Elements owning such content lose their alternate text, actual
//! text, expansion and title (which may spell out the removed words), and so does every element
//! above them, since those describe the same subtree; references to marked content that is now
//! empty are dropped from the element.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::interp::REPLACEMENT_KEYS;

/// Each MCID's operators (serialized), and whether anything is still drawn inside it.
fn by_mcid(doc: &Document, resources: &Dict, data: &[u8]) -> HashMap<i64, (Vec<u8>, bool)> {
    let mut out: HashMap<i64, (Vec<u8>, bool)> = HashMap::new();
    let mut stack: Vec<Option<i64>> = Vec::new();
    for op in pdfcraft_content::parse(data).ops {
        match op.op.as_slice() {
            b"BDC" => {
                let id = match op.operands.get(1) {
                    Some(Object::Name(n)) => resources.get(b"Properties").and_then(|p| {
                        let props = doc.resolve(p);
                        let prop = doc.resolve(props.as_dict()?.get(n)?);
                        prop.as_dict()?.int(b"MCID")
                    }),
                    Some(p) => doc.resolve(p).as_dict().and_then(|d| d.int(b"MCID")),
                    _ => None,
                };
                if let Some(id) = id {
                    out.entry(id).or_default();
                }
                stack.push(id);
                continue;
            }
            b"BMC" => {
                stack.push(None);
                continue;
            }
            b"EMC" => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let draws = match op.op.as_slice() {
            b"Tj" | b"'" | b"\"" => op.operands.last().and_then(Object::as_string).is_some_and(|s| !s.bytes.is_empty()),
            b"TJ" => {
                op.operands.first().and_then(Object::as_array).is_some_and(|a| a.iter().any(|x| x.as_string().is_some_and(|s| !s.bytes.is_empty())))
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" | b"Do" | b"BI" => true,
            _ => false,
        };
        let mut bytes = Vec::new();
        pdfcraft_content::write_op(&op, &mut bytes);
        for id in stack.iter().flatten() {
            let e = out.entry(*id).or_default();
            e.0.extend_from_slice(&bytes);
            e.1 |= draws;
        }
    }
    out
}

/// The MCIDs whose content changed between `before` and `after`, and those left empty.
pub(crate) fn touched(doc: &Document, resources: &Dict, before: &[u8], after: &[u8]) -> (HashSet<i64>, HashSet<i64>) {
    let (b, a) = (by_mcid(doc, resources, before), by_mcid(doc, resources, after));
    let mut changed = HashSet::new();
    let mut empty = HashSet::new();
    for (id, (ops, _)) in &b {
        match a.get(id) {
            Some((new, draws)) => {
                if new != ops {
                    changed.insert(*id);
                }
                if !draws {
                    empty.insert(*id);
                }
            }
            None => {
                changed.insert(*id);
                empty.insert(*id);
            }
        }
    }
    (changed, empty)
}

fn mcid_of(doc: &Document, o: &Object, page: Option<ObjRef>, target: ObjRef, stream: Option<(ObjRef, ObjRef)>) -> Option<i64> {
    match &*doc.resolve(o) {
        Object::Int(n) if page == Some(target) && stream.is_none() => Some(*n),
        Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
            let pg = m.get(b"Pg").and_then(Object::as_ref).or(page);
            let stm = m.get(b"Stm").and_then(Object::as_ref);
            (pg == Some(target) && stm == stream.map(|s| s.0)).then(|| m.int(b"MCID")).flatten()
        }
        _ => None,
    }
}

/// A kid of the structure tree waiting to be visited, with what it gets from above.
struct Pending {
    kid: Object,
    /// The page it inherits (the nearest `/Pg` above it).
    page: Option<ObjRef>,
    /// The element above it.
    parent: Option<ObjRef>,
}

/// A structure element with marked content of the cleaned page or form among its kids.
struct Hit {
    element: ObjRef,
    /// The element as the walk found it.
    dict: Dict,
    /// The page its kids inherit (its own `/Pg`, or the nearest above it).
    page: Option<ObjRef>,
    /// It owns marked content that changed, so its descriptive text goes.
    sensitive: bool,
}

/// Clean the elements owning `changed` marked content on `page`; returns how many changed.
pub(crate) fn clean(
    doc: &mut Document,
    page: ObjRef,
    stream: Option<(ObjRef, ObjRef)>,
    changed: &HashSet<i64>,
    empty: &HashSet<i64>,
) -> Result<usize, pdfcraft_cos::CosError> {
    if changed.is_empty() && stream.is_none() {
        return Ok(0);
    }
    let Some(root) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"StructTreeRoot").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()))
    else {
        return Ok(0);
    };
    let mut todo: Vec<Pending> = root.get(b"K").map(|k| vec![Pending { kid: k.clone(), page: None, parent: None }]).unwrap_or_default();
    let mut seen = HashSet::new();
    // Each element's parent as the walk found it (not `/P`, which a malformed tree may point
    // anywhere): an element is reached once, after its parent, so following this is acyclic.
    let mut parents: HashMap<ObjRef, ObjRef> = HashMap::new();
    let mut hits: Vec<Hit> = Vec::new();
    while let Some(Pending { kid, page: pg, parent }) = todo.pop() {
        match kid {
            Object::Array(a) => todo.extend(a.into_iter().map(|kid| Pending { kid, page: pg, parent })),
            Object::Ref(r) => {
                if !seen.insert(r) || seen.len() > 2_000_000 {
                    continue;
                }
                let object = doc.get(r);
                if let Object::Array(a) = &*object {
                    todo.extend(a.iter().cloned().map(|kid| Pending { kid, page: pg, parent }));
                    continue;
                }
                let Some(d) = object.as_dict().cloned() else { continue };
                if !d.contains(b"S") {
                    continue;
                }
                if let Some(p) = parent {
                    parents.insert(r, p);
                }
                let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                let kids: Vec<Object> = match d.get(b"K").map(|k| doc.resolve(k)) {
                    Some(o) => match &*o {
                        Object::Array(a) => a.clone(),
                        _ => d.get(b"K").cloned().into_iter().collect(),
                    },
                    None => Vec::new(),
                };
                let sensitive = kids.iter().any(|x| mcid_of(doc, x, pg, page, stream).is_some_and(|m| changed.contains(&m)));
                let remap = stream.is_some() && kids.iter().any(|x| mcid_of(doc, x, pg, page, stream).is_some());
                if sensitive || remap {
                    hits.push(Hit { element: r, dict: d.clone(), page: pg, sensitive });
                }
                todo.extend(kids.into_iter().map(|kid| Pending { kid, page: pg, parent: Some(r) }));
            }
            _ => {}
        }
    }
    let mut n = 0;
    let owners: Vec<ObjRef> = hits.iter().filter(|hit| hit.sensitive).map(|hit| hit.element).collect();
    for Hit { element: r, dict: d, page: pg, sensitive } in hits {
        let kids = d.get(b"K").map(|k| doc.resolve(k));
        let array = kids.as_ref().is_some_and(|k| matches!(&**k, Object::Array(_)));
        let list = match kids.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => d.get(b"K").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let mut keep = Vec::new();
        for mut kid in list {
            let mcid = mcid_of(doc, &kid, pg, page, stream);
            if mcid.is_some_and(|m| empty.contains(&m)) {
                continue;
            }
            if let Some((original, replacement)) = stream.filter(|_| mcid.is_some()) {
                if let Some(reference) = kid.as_ref() {
                    doc.update_dict(reference, |m| {
                        m.set(b"Stm".to_vec(), Object::Ref(replacement));
                        if m.get(b"StmOwn").and_then(Object::as_ref) == Some(original) {
                            m.set(b"StmOwn".to_vec(), Object::Ref(replacement));
                        }
                    })?;
                } else if let Object::Dict(m) = &mut kid {
                    m.set(b"Stm".to_vec(), Object::Ref(replacement));
                    if m.get(b"StmOwn").and_then(Object::as_ref) == Some(original) {
                        m.set(b"StmOwn".to_vec(), Object::Ref(replacement));
                    }
                }
            }
            keep.push(kid);
        }
        doc.update_dict(r, |e| {
            if sensitive {
                strip_descriptive(e);
            }
            if array {
                e.set(b"K".to_vec(), Object::Array(keep));
            } else if let Some(k) = keep.into_iter().next() {
                e.set(b"K".to_vec(), k);
            } else {
                e.remove(b"K");
            }
        })?;
        n += usize::from(sensitive && doc.get(r).as_dict() != Some(&d));
    }
    // The elements above an owner describe the same subtree (their actual text replaces it, their
    // alternate text and title describe it): they lose that text too, up to the tree root. One
    // cleaned already was climbed from before, and so were its ancestors.
    let mut cleaned: HashSet<ObjRef> = HashSet::new();
    for owner in owners {
        let mut cur = parents.get(&owner).copied();
        let mut steps = 0usize;
        while let Some(p) = cur {
            steps = steps.saturating_add(1);
            if steps > 100_000 || !cleaned.insert(p) {
                break;
            }
            if doc.get(p).as_dict().is_some_and(has_descriptive) {
                doc.update_dict(p, strip_descriptive)?;
                n += 1;
            }
            cur = parents.get(&p).copied();
        }
    }
    Ok(n)
}

/// The keys under which a structure element says something about its content: the replacement
/// text a reader may show in its place (every one of `REPLACEMENT_KEYS`) and the title that names
/// it (`/T`). A title names a section rather than spelling its content, but nothing says it never
/// repeats the removed words, so it goes too (fail closed).
fn descriptive_keys() -> impl Iterator<Item = &'static [u8]> {
    REPLACEMENT_KEYS.into_iter().chain(std::iter::once(b"T".as_slice()))
}

/// Does this structure element describe its content (hold any of the `descriptive_keys`)?
fn has_descriptive(d: &Dict) -> bool {
    descriptive_keys().any(|key| d.contains(key))
}

/// Remove what this structure element says about its content (every one of the
/// `descriptive_keys`).
fn strip_descriptive(d: &mut Dict) {
    for key in descriptive_keys() {
        d.remove(key);
    }
}
