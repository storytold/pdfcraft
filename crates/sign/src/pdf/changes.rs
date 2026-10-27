use super::types::{Modification, kind};
use super::*;

/// What later revisions changed, classified as Acrobat reports it, under DocMDP `p` (or none:
/// an approval signature permits form fill, comments and further signatures).
pub(super) fn classify_changes(doc: &Document, old: Option<Document>, p: Option<u8>) -> Modification {
    let Some(old) = old else {
        return Modification::Disallowed(vec![kind::UNREADABLE]);
    };
    let old_content: HashSet<ObjRef> = pdfcraft_annot::page_refs(&old)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|pg| {
            let c = old.get(pg).as_dict().and_then(|d| d.get(b"Contents").cloned());
            match c {
                Some(Object::Ref(r)) => match &*old.get(r) {
                    Object::Array(a) => a.iter().filter_map(Object::as_ref).chain([r]).collect::<Vec<_>>(),
                    _ => vec![r],
                },
                Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
                _ => Vec::new(),
            }
        })
        .collect();
    // The signed revision's document-level XMP stream.
    let old_metadata = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"Metadata").and_then(Object::as_ref)));
    // The signed revision's XFA data packets: filling the form rewrites them.
    let old_xfa_data = xfa_data_packets(&old);
    // What each object was in the signed revision: its pages, and its trailer /Info. A changed
    // object is judged by the role it held when signed, never by the labels or keys its new,
    // attacker-written version carries (relabelling a signed page /Type /Sig, or dropping its
    // /Type and adding /ModDate, must not reclassify it away from page content).
    let signed_pages: HashSet<ObjRef> = pdfcraft_annot::page_refs(&old).unwrap_or_default().into_iter().collect();
    // The signed pages' /Annots arrays that are objects of their own. Adding an annotation updates
    // such an array in place (the page dictionary does not change), so it is judged like a
    // page's inline /Annots: by the annotations it gained.
    let signed_annots: HashSet<ObjRef> =
        signed_pages.iter().filter_map(|pg| old.get(*pg).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref))).collect();
    // What each signed page draws from, by the role it held when signed (#1787).
    let signed_resources = signed_page_resources(&old, &signed_pages);
    let old_info = old.trailer().get(b"Info").and_then(Object::as_ref);
    let (mut allowed, mut disallowed): (Vec<&'static str>, Vec<&'static str>) = (Vec::new(), Vec::new());
    let dss = dss_parts(doc, &old);
    // Stored at the same place in both (and not edited since): the same bytes, unchanged.
    let same_place = |num: u32| -> bool {
        use pdfcraft_cos::XrefEntry;
        if doc.is_edited(num) {
            return false;
        }
        match (doc.xref_entry(num), old.xref_entry(num)) {
            (Some(a @ XrefEntry::InFile { .. }), Some(b)) => a == b,
            (Some(a @ XrefEntry::InStream { stream, .. }), Some(b)) => {
                a == b && !doc.is_edited(stream) && doc.xref_entry(stream) == old.xref_entry(stream)
            }
            _ => false,
        }
    };
    // A later cross-reference section may carry an entry this reader sets aside (an in-use entry
    // at offset 0, a second entry for one object, a reserved entry type). The renderer may follow
    // it and draw another object than the one compared below, so the page could show new content
    // while every comparison here finds the signed object untouched. No writer has a reason to
    // add such an entry after signing: it is never a permitted change.
    for (_, num) in doc.xref_oddities().iter().filter(|o| !old.xref_oddities().contains(o)) {
        let r = ObjRef { num: *num, generation: doc.generation(*num) };
        let what = if old_content.contains(&r) || signed_pages.contains(&r) { kind::PAGE_CONTENT } else { kind::OTHER };
        if !disallowed.contains(&what) {
            disallowed.push(what);
        }
    }
    // The trailer chooses the catalog. A later revision that points it at another catalog (one
    // already in the signed file, so no object changes) shows another document.
    if doc.root() != old.root() {
        disallowed.push(kind::STRUCTURE);
    }
    for num in doc.object_numbers() {
        if same_place(num) {
            continue;
        }
        let r = ObjRef { num, generation: doc.generation(num) };
        let new = doc.get(r);
        let before = old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null));
        if before.as_deref() == Some(&*new) {
            // The same content at a new place. That is true for the renderer as well only when
            // the entry locates the object itself: where this reader had to search for the
            // object, another reader may arrive at a different one.
            if !doc.is_edited(num) && !doc.xref_entry_is_exact(num) && !disallowed.contains(&kind::OTHER) {
                disallowed.push(kind::OTHER);
            }
            continue;
        }
        // A resource of a signed page is page content whatever keys its new version carries
        // (#1787): a /FT or /Subtype /Widget written by whoever rewrote a font, an XObject or a
        // graphics state does not make that a form fill. A container (the resources dictionary,
        // a category dictionary) that kept every entry it had and only gained names changes
        // nothing the signed content draws, and is judged as before.
        let kept_entries = |d: &Dict| before.as_deref().and_then(Object::as_dict).is_some_and(|b| b.iter().all(|(k, v)| d.get(k) == Some(v)));
        let resource_kind = match (signed_resources.get(&r), &*new) {
            (Some(false), _) => Some(kind::PAGE_CONTENT),
            (Some(true), Object::Dict(d)) if kept_entries(d) => None,
            (Some(true), _) => Some(kind::PAGE_CONTENT),
            (None, _) => None,
        };
        let what = match (resource_kind, &*new) {
            (Some(k), _) => k,
            // The Document Security Store's own arrays and dictionaries, when they only grew.
            (_, _) if dss.contains(&num) => kind::SECURITY_STORE,
            // A signed page's indirect /Annots array, still an array: what it lost, then what it
            // gained. Dropping a signed annotation is never hidden behind an added signature.
            (_, Object::Array(now)) if signed_annots.contains(&r) && before.as_deref().is_some_and(|b| b.as_array().is_some()) => {
                let was: HashSet<ObjRef> =
                    before.as_deref().and_then(Object::as_array).map(|a| a.iter().filter_map(Object::as_ref).collect()).unwrap_or_default();
                let kept: HashSet<ObjRef> = now.iter().filter_map(Object::as_ref).collect();
                let removed: Vec<ObjRef> = was.iter().filter(|r| !kept.contains(r)).copied().collect();
                if removed.is_empty() {
                    let added: Vec<ObjRef> = now.iter().filter_map(Object::as_ref).filter(|r| !was.contains(r)).collect();
                    added_annots(doc, &added)
                } else if removed.iter().any(|r| old.get(*r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget"))) {
                    // A form field or signature widget taken off the page: no permission covers it.
                    kind::OTHER
                } else {
                    // Deleting comments is a comment change (permitted at P 3 only).
                    kind::COMMENTS
                }
            }
            (_, Object::Dict(d)) => {
                let was = before.as_deref().and_then(Object::as_dict);
                if signed_pages.contains(&r) {
                    // A signed page is always judged by what the page shows, whatever it is now
                    // relabelled: a rewritten /Contents or /Resources is a page content change.
                    page_change_kind(doc, &old, d, was)
                } else if was.is_some() && Some(r) == old_info {
                    // Only the signed revision's own /Info dictionary is a metadata change.
                    kind::METADATA
                } else {
                    change_kind(doc, &old, d, was)
                }
            }
            (_, Object::Stream(s)) => {
                // File plumbing is what the stream was when signed: the /Type it has now is
                // written by whoever changed it, and the renderer ignores /Type (a page's Form
                // XObject relabelled /ObjStm still draws its new content).
                let was_plumbing = matches!(before.as_deref(), Some(Object::Stream(b)) if matches!(b.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")));
                if old_content.contains(&r) {
                    kind::PAGE_CONTENT
                } else if before.is_none() || was_plumbing {
                    continue;
                } else if s.dict.name(b"Type") == Some(b"Metadata") && Some(r) == old_metadata {
                    // Only the signed catalog's XMP stream. A /Type /Metadata label on anything
                    // else, even one it already had when signed (a page's Form XObject: the
                    // renderer ignores /Type), does not make rewriting it a metadata change.
                    kind::METADATA
                } else if old_xfa_data.contains(&r) {
                    // The form's XFA data (`datasets`) or state (`form`) as the signed revision
                    // named it: filling an XFA form rewrites these, as a Reader Extensions (UR3)
                    // save of a certified form does (#2759). The role comes from the signed
                    // revision, so a later one can't name page content as XFA data.
                    kind::FORM_FILL
                } else {
                    kind::OTHER
                }
            }
            _ if before.is_none() => continue,
            _ => kind::OTHER,
        };
        let ok = match (what, p) {
            (kind::SIGNATURE | kind::SECURITY_STORE, _) => true,
            (kind::FORM_FILL, Some(2 | 3) | None) => true,
            (kind::COMMENTS, Some(3) | None) => true,
            // Every save updates /Info (ModDate), signing included.
            (kind::METADATA, _) => true,
            _ => false,
        };
        let list = if ok { &mut allowed } else { &mut disallowed };
        if !list.contains(&what) {
            list.push(what);
        }
    }
    // An object the signed revision had and this one frees is a change too, and the loop above
    // only sees objects that still exist. File plumbing (cross-reference and object streams) is
    // rewritten by every save and is not a change.
    let current: HashSet<u32> = doc.object_numbers().into_iter().collect();
    for num in old.object_numbers().into_iter().filter(|n| !current.contains(n)) {
        let r = ObjRef { num, generation: old.generation(num) };
        let was = old.get(r);
        let signed_role = old_content.contains(&r) || signed_pages.contains(&r) || signed_resources.contains_key(&r);
        // A signed page's content or resource is judged by that role, whatever /Type its stream carried.
        if !signed_role && matches!(&*was, Object::Stream(s) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm"))) {
            continue;
        }
        let what = if signed_role { kind::PAGE_CONTENT } else { kind::OTHER };
        if !disallowed.contains(&what) {
            disallowed.push(what);
        }
    }
    if disallowed.is_empty() { Modification::Allowed(allowed) } else { Modification::Disallowed(disallowed) }
}

/// The streams of `doc`'s XFA form that hold its data and state: the `datasets` and `form`
/// packets of the AcroForm's `/XFA` array (name, stream, name, stream, …). A form given as one
/// XFA stream holds its template too, and is not listed.
fn xfa_data_packets(doc: &Document) -> HashSet<ObjRef> {
    let Some(form) = acroform(doc) else { return HashSet::new() };
    let Some(xfa) = form.get(b"XFA").map(|x| doc.resolve(x)) else { return HashSet::new() };
    let Object::Array(xfa) = &*xfa else { return HashSet::new() };
    xfa.as_chunks::<2>()
        .0
        .iter()
        .filter(|[name, _]| doc.resolve(name).as_string().is_some_and(|s| matches!(s.to_text().as_str(), "datasets" | "form")))
        .filter_map(|[_, packet]| packet.as_ref())
        .collect()
}

/// Where an object sits in the Document Security Store (ISO 32000-2 §12.8.4.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DssRole {
    /// The `/DSS` dictionary.
    Dss,
    /// A `/Certs`, `/CRLs`, `/OCSPs` (or a VRI entry's `/Cert`, `/CRL`, `/OCSP`) array.
    DataArray,
    /// A certificate, CRL, OCSP response or timestamp token stream.
    Data,
    /// The `/VRI` dictionary.
    VriMap,
    /// One entry of `/VRI`.
    VriEntry,
}

/// The indirect objects the catalog's `/DSS` reaches by the keys the standard defines, and the
/// role each plays there. Any other key is not followed: `/DSS << /X 5 0 R >>` does not make
/// object 5 part of the store.
fn dss_walk(doc: &Document) -> Vec<(ObjRef, DssRole)> {
    const LIMIT: usize = 100_000;
    let mut out = Vec::new();
    let Some(dss) = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"DSS").cloned())) else { return out };
    // The roles nest at most four deep (DSS, VRI, entry, array, data), so no seen-set is needed.
    let mut stack = vec![(dss, DssRole::Dss)];
    let mut steps = 0usize;
    while let Some((o, role)) = stack.pop() {
        steps += 1;
        if steps > LIMIT {
            break;
        }
        if let Object::Ref(r) = &o {
            out.push((*r, role));
        }
        match (role, &*doc.resolve(&o)) {
            (DssRole::Dss, Object::Dict(d)) => {
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"Certs" | b"CRLs" | b"OCSPs" => stack.push((v.clone(), DssRole::DataArray)),
                        b"VRI" => stack.push((v.clone(), DssRole::VriMap)),
                        _ => {}
                    }
                }
            }
            (DssRole::DataArray, Object::Array(a)) => stack.extend(a.iter().map(|v| (v.clone(), DssRole::Data))),
            (DssRole::VriMap, Object::Dict(d)) => stack.extend(d.iter().map(|(_, v)| (v.clone(), DssRole::VriEntry))),
            (DssRole::VriEntry, Object::Dict(d)) => {
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"Cert" | b"CRL" | b"OCSP" => stack.push((v.clone(), DssRole::DataArray)),
                        b"TS" => stack.push((v.clone(), DssRole::Data)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `now` only adds to `before`: an array keeps every element, a dictionary keeps every
/// entry (an entry that is an array or dictionary may itself have grown).
pub(crate) fn only_grew(doc: &Document, old: &Document, before: &Object, now: &Object, depth: u8) -> bool {
    /// Arrays of anything but references are compared element by element (quadratic), so only
    /// up to this size; a store's `/Certs`/`/CRLs`/`/OCSPs` are references, compared as sets.
    const MAX_COMPARED: usize = 10_000;
    match (before, now) {
        (Object::Array(b), Object::Array(n)) => {
            if b.len() > n.len() {
                return false;
            }
            let refs = |a: &[Object]| a.iter().map(Object::as_ref).collect::<Option<HashSet<ObjRef>>>();
            match (refs(b), refs(n)) {
                (Some(b), Some(n)) => b.is_subset(&n),
                _ => b.len() <= MAX_COMPARED && n.len() <= MAX_COMPARED && b.iter().all(|x| n.contains(x)),
            }
        }
        (Object::Dict(b), Object::Dict(n)) => b.iter().all(|(k, v)| match n.get(k) {
            Some(nv) => nv == v || (depth < 4 && only_grew(doc, old, &old.resolve(v), &doc.resolve(nv), depth + 1)),
            None => false,
        }),
        _ => before == now,
    }
}

/// The objects of the Document Security Store that count as validation data added after
/// signing: its dictionary, `/Certs`/`/CRLs`/`/OCSPs` arrays, `/VRI` and its entries, when
/// - they are reached by the standard's keys and have the shape of their role, and
/// - they are new, or were already part of the signed store and have only grown.
///
/// An existing object is never made "document security store" by pointing the store at it: the
/// page's `/Contents` array, or a dictionary labelled `/Type /DSS`, keep being judged as what they
/// are. The data streams need no entry here: new objects are not changes, and rewriting an
/// existing one is judged on its own.
fn dss_parts(doc: &Document, old: &Document) -> HashSet<u32> {
    let signed: HashSet<u32> = dss_walk(old).into_iter().map(|(r, _)| r.num).collect();
    let is_stream = |o: &Object| matches!(&*doc.resolve(o), Object::Stream(_));
    let mut parts = HashSet::new();
    for (r, role) in dss_walk(doc) {
        let now = doc.get(r);
        let shaped = match (role, &*now) {
            (DssRole::Dss, Object::Dict(d)) => d.iter().all(|(k, _)| matches!(k.as_slice(), b"Type" | b"Certs" | b"CRLs" | b"OCSPs" | b"VRI")),
            (DssRole::DataArray, Object::Array(a)) => a.iter().all(|e| matches!(e, Object::Ref(_)) && is_stream(e)),
            (DssRole::VriMap, Object::Dict(d)) => d.iter().all(|(_, v)| matches!(&*doc.resolve(v), Object::Dict(_))),
            (DssRole::VriEntry, Object::Dict(d)) => {
                d.iter().all(|(k, _)| matches!(k.as_slice(), b"Type" | b"Cert" | b"CRL" | b"OCSP" | b"TU" | b"TS"))
            }
            _ => false,
        };
        let before = old.try_get(r.num).ok().filter(|o| !matches!(**o, Object::Null));
        let history = match before {
            None => true,
            Some(b) => signed.contains(&r.num) && only_grew(doc, old, &b, &now, 0),
        };
        if shaped && history {
            parts.insert(r.num);
        }
    }
    parts
}

/// What adding `added` annotations amounts to: signing (signature widgets only), form fill
/// (other widgets) or comments.
fn added_annots(doc: &Document, added: &[ObjRef]) -> &'static str {
    let kinds: Vec<(bool, bool)> = added
        .iter()
        .map(|r| {
            let o = doc.get(*r);
            let d = o.as_dict();
            (d.is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")), d.is_some_and(|d| d.name(b"FT") == Some(b"Sig")))
        })
        .collect();
    if !kinds.is_empty() && kinds.iter().all(|(w, s)| *w && *s) {
        kind::SIGNATURE
    } else if !kinds.is_empty() && kinds.iter().all(|(w, _)| *w) {
        kind::FORM_FILL
    } else {
        kind::COMMENTS
    }
}

fn refs(doc: &Document, o: Option<&Object>) -> Vec<ObjRef> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
}

/// The interactive form dictionary of a document, resolved.
fn acroform(doc: &Document) -> Option<Dict> {
    let c = doc.get(doc.root()?).as_dict()?.get(b"AcroForm").cloned()?;
    doc.resolve(&c).as_dict().cloned()
}

/// An AcroForm change that only adds signature fields (and sets `/SigFlags`) is signing.
/// Without `before`, the signed revision's AcroForm (wherever it was) is the reference.
fn acroform_kind(doc: &Document, old: &Document, now: &Dict, before: Option<&Dict>) -> &'static str {
    let fallback = acroform(old).unwrap_or_default();
    let before = before.unwrap_or(&fallback);
    let strip = |d: &Dict| -> Vec<(Vec<u8>, Object)> {
        let mut v: Vec<(Vec<u8>, Object)> =
            d.iter().filter(|(k, _)| !matches!(k.as_slice(), b"Fields" | b"SigFlags")).map(|(k, o)| (k.to_vec(), o.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    if strip(now) != strip(before) {
        return kind::FORM_FILL;
    }
    let old_fields = refs(old, before.get(b"Fields"));
    let added: Vec<ObjRef> = refs(doc, now.get(b"Fields")).into_iter().filter(|r| !old_fields.contains(r)).collect();
    if added.is_empty() || added_annots(doc, &added) == kind::SIGNATURE { kind::SIGNATURE } else { kind::FORM_FILL }
}

/// Dictionary entries in a stable order, without `keys`.
fn without(x: &Dict, keys: &[&[u8]]) -> Vec<(Vec<u8>, Object)> {
    let mut v: Vec<(Vec<u8>, Object)> = x.iter().filter(|(k, _)| !keys.contains(&k.as_slice())).map(|(k, o)| (k.to_vec(), o.clone())).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Whether a dictionary was a signature object: a signature or timestamp value, a signature
/// reference or its transform parameters, or a signature form field. Used to decide from the
/// signed revision, so a later relabel of some other object cannot pose as a signature change.
fn is_signature_dict(d: &Dict) -> bool {
    matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams")) || d.name(b"FT") == Some(b"Sig")
}

/// The objects the signed pages draw from, by reference, with the role each held when signed
/// (#1787): `true` for a container (a page's resources dictionary, or one of its category
/// dictionaries), where a name added later can't be reached by unchanged content; `false` for
/// everything reached from a container's entries (fonts and their files, XObjects and their own
/// resources, graphics states and soft masks, colour spaces, patterns, shadings, optional-content
/// properties), where any change changes what the page shows. Resources are the page's own or
/// inherited. Bounded by depth and by the number of objects visited; past the bound the rest is
/// judged as before.
fn signed_page_resources(old: &Document, pages: &HashSet<ObjRef>) -> HashMap<ObjRef, bool> {
    const CATEGORIES: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];
    const MAX_VISITED: usize = 50_000;
    const MAX_DEPTH: u8 = 32;
    const MAX_INHERITANCE: usize = 64;

    /// Everything reachable from `o` is content the page draws (a leaf role, overriding a
    /// container one reached by another path). A signed page or the catalog reached this way
    /// (a `/P` or `/Parent` back-reference) keeps the role of its own: the walk stops there.
    fn reach(old: &Document, o: &Object, depth: u8, stop: &HashSet<ObjRef>, roles: &mut HashMap<ObjRef, bool>) {
        if roles.len() >= MAX_VISITED || depth > MAX_DEPTH {
            return;
        }
        let resolved;
        let o = match o {
            Object::Ref(r) => {
                if stop.contains(r) || roles.insert(*r, false).is_some() {
                    return;
                }
                resolved = old.get(*r);
                &*resolved
            }
            other => other,
        };
        match o {
            Object::Dict(d) => d.iter().for_each(|(_, v)| reach(old, v, depth + 1, stop, roles)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, v)| reach(old, v, depth + 1, stop, roles)),
            Object::Array(a) => a.iter().for_each(|v| reach(old, v, depth + 1, stop, roles)),
            _ => {}
        }
    }

    /// A page's `/Resources`, its own or the nearest ancestor's.
    fn resources(old: &Document, page: ObjRef) -> Option<Object> {
        let mut seen = HashSet::new();
        let mut cur = page;
        while seen.insert(cur) && seen.len() <= MAX_INHERITANCE {
            let d = old.get(cur);
            let d = d.as_dict()?;
            if let Some(r) = d.get(b"Resources") {
                return Some(r.clone());
            }
            cur = d.get(b"Parent").and_then(Object::as_ref)?;
        }
        None
    }

    let mut roles = HashMap::new();
    let mut stop = pages.clone();
    stop.extend(old.root());
    for page in pages {
        let Some(res) = resources(old, *page) else { continue };
        let container = |o: &Object, roles: &mut HashMap<ObjRef, bool>| -> Arc<Object> {
            match o {
                Object::Ref(r) => {
                    roles.entry(*r).or_insert(true);
                    old.get(*r)
                }
                other => Arc::new(other.clone()),
            }
        };
        let res = container(&res, &mut roles);
        let Some(d) = res.as_dict() else { continue };
        for category in CATEGORIES {
            let Some(v) = d.get(category) else { continue };
            let category = container(v, &mut roles);
            if let Some(entries) = category.as_dict() {
                entries.iter().for_each(|(_, entry)| reach(old, entry, 0, &stop, &mut roles));
            }
        }
    }
    roles
}

/// A changed object that was a page in the signed revision, judged by what the page shows rather
/// than by any `/Type` its new version carries. Only added annotations are permitted; any other
/// difference (content, resources, a relabelled or dropped `/Type`) is a page content change.
fn page_change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> &'static str {
    match before {
        Some(b) if without(d, &[b"Annots"]) != without(b, &[b"Annots"]) => kind::PAGE_CONTENT,
        Some(b) => {
            let was = refs(old, b.get(b"Annots"));
            let added: Vec<ObjRef> = refs(doc, d.get(b"Annots")).into_iter().filter(|r| !was.contains(r)).collect();
            added_annots(doc, &added)
        }
        None => kind::PAGE_CONTENT,
    }
}

/// Whether the catalog `d` keeps every developer extension the signed catalog `b` declared, each
/// with the same value: it may only declare more.
fn extensions_kept(doc: &Document, old: &Document, d: &Dict, b: &Dict) -> bool {
    let now = d.get(b"Extensions").map(|e| doc.resolve(e)).and_then(|e| e.as_dict().cloned());
    let was = b.get(b"Extensions").map(|e| old.resolve(e)).and_then(|e| e.as_dict().cloned());
    match (was, now) {
        (None, _) => true,
        (Some(was), Some(now)) => was.iter().all(|(k, v)| now.get(k) == Some(v)),
        (Some(_), None) => false,
    }
}

fn change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> &'static str {
    let ty = d.name(b"Type");
    // A later revision cannot make an existing object a signature by relabelling it: a signature
    // change is only a brand-new object, or one that was already a signature object when signed.
    let signature_ok = before.is_none_or(is_signature_dict);
    // A signature widget that already held a signature is what the signature shows: moving it or
    // swapping its appearance can hide the signed page under it. Judged by the signed revision,
    // so relabelling the widget now does not change that.
    if before.is_some_and(|b| {
        b.name(b"Subtype") == Some(b"Widget") && b.name(b"FT") == Some(b"Sig") && b.get(b"V").is_some_and(|v| old.resolve(v).as_dict().is_some())
    }) && before != Some(d)
    {
        return kind::PAGE_CONTENT;
    }
    match ty {
        // Only a new signature value or policy: rewriting a signed one is not a signature (#1788).
        Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams") if before.is_none() => return kind::SIGNATURE,
        Some(b"Catalog") => {
            let keys: &[&[u8]] = &[b"AcroForm", b"DSS", b"Perms", b"Metadata", b"NeedsRendering", b"Extensions"];
            return match before {
                Some(b) if without(d, keys) != without(b, keys) => kind::STRUCTURE,
                // Developer extensions (ISO 32000-2 §7.12) only declare what the file uses. A
                // writer adding a /DSS also declares the ESIC extension it belongs to, so a
                // declaration may be added, but none that was signed may change or go.
                Some(b) if !extensions_kept(doc, old, d, b) => kind::STRUCTURE,
                Some(b) => {
                    if d.get(b"AcroForm") != b.get(b"AcroForm") {
                        // Compare the forms themselves, inline or not.
                        let now = acroform(doc).unwrap_or_default();
                        acroform_kind(doc, old, &now, None)
                    } else if b.get(b"Perms") != d.get(b"Perms") {
                        kind::SIGNATURE
                    } else {
                        kind::METADATA
                    }
                }
                None => kind::FORM_FILL,
            };
        }
        Some(b"Page") => {
            return match before {
                Some(b) if without(d, &[b"Annots"]) != without(b, &[b"Annots"]) => kind::PAGE_CONTENT,
                Some(b) => {
                    let was = refs(old, b.get(b"Annots"));
                    let added: Vec<ObjRef> = refs(doc, d.get(b"Annots")).into_iter().filter(|r| !was.contains(r)).collect();
                    added_annots(doc, &added)
                }
                None => kind::COMMENTS,
            };
        }
        Some(b"Pages") => return if before.is_some() { kind::PAGES } else { kind::STRUCTURE },
        _ => {}
    }
    match d.name(b"Subtype") {
        Some(b"Widget") => return if d.name(b"FT") == Some(b"Sig") && signature_ok { kind::SIGNATURE } else { kind::FORM_FILL },
        Some(b"Link") => return kind::LINKS,
        Some(_) if ty == Some(b"Annot") || d.contains(b"Rect") => return kind::COMMENTS,
        _ => {}
    }
    if d.name(b"FT") == Some(b"Sig") && signature_ok {
        return kind::SIGNATURE;
    }
    if d.contains(b"Fields") {
        return acroform_kind(doc, old, d, before);
    }
    if d.contains(b"FT") || (d.contains(b"T") && d.contains(b"Kids")) || d.contains(b"DR") {
        return kind::FORM_FILL;
    }
    if before.is_none() {
        // New objects referenced by changed ones (fonts, appearance resources).
        return kind::FORM_FILL;
    }
    kind::OTHER
}
