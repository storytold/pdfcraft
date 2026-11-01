use super::listing::sig_fields;
use super::types::{Modification, kind};
use super::{Arc, Dict, Document, HashMap, HashSet, ObjRef, Object};

/// What later revisions changed, classified as Acrobat reports it, under DocMDP `p` (or none:
/// an approval signature permits form fill, comments and further signatures).
pub(super) fn classify_changes(doc: &Document, old: Option<Document>, p: Option<u8>) -> Modification {
    let Some(old) = old else {
        return Modification::Disallowed(vec![kind::UNREADABLE]);
    };
    let old_page_list = pdfcraft_annot::page_refs(&old).unwrap_or_default();
    let old_content: HashSet<ObjRef> = old_page_list
        .iter()
        .copied()
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
    // The signed revision's AcroForm object (if indirect).
    let old_acroform = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"AcroForm").and_then(Object::as_ref)));
    // The signed revision's XFA data packets: filling the form rewrites them.
    let old_xfa_data = xfa_data_packets(&old);
    // What each object was in the signed revision: its pages, its page tree, its signature
    // fields and widgets, and its trailer /Info. A changed object is judged by the role it held
    // when signed, never by the labels or keys its new, attacker-written version carries
    // (relabelling a signed page /Type /Sig, or dropping its /Type and adding /ModDate, must not
    // reclassify it away from page content).
    let signed_pages: HashSet<ObjRef> = old_page_list.iter().copied().collect();
    let signed_page_tree = signed_page_tree(&old, &signed_pages);
    // The signed pages' /Annots arrays that are objects of their own. Adding an annotation updates
    // such an array in place (the page dictionary does not change), so it is judged like a
    // page's inline /Annots: by the annotations it gained.
    let signed_annots: HashSet<ObjRef> =
        signed_pages.iter().filter_map(|pg| old.get(*pg).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref))).collect();
    // Signature fields and widgets in the signed revision: those already signed must never be
    // altered later, whereas an unsigned signature field or widget in the signed revision may be
    // signed later (under P != 1).
    let (signed_sig_widgets, signed_sig_fields, unsigned_sig_objects) = signed_sig_roles(&old);
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
    if pdfcraft_annot::page_refs(doc).ok().as_ref() != Some(&old_page_list) && !disallowed.contains(&kind::PAGES) {
        disallowed.push(kind::PAGES);
    }
    let classifier = ChangedObjectClassifier {
        doc,
        old: &old,
        old_content: &old_content,
        old_metadata,
        old_acroform,
        old_xfa_data: &old_xfa_data,
        signed_pages: &signed_pages,
        signed_page_tree: &signed_page_tree,
        signed_annots: &signed_annots,
        signed_sig_widgets: &signed_sig_widgets,
        signed_sig_fields: &signed_sig_fields,
        unsigned_sig_objects: &unsigned_sig_objects,
        signed_resources: &signed_resources,
        old_info,
        dss: &dss,
    };
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
        let Some(change) = classifier.classify(r, &new, before.as_deref()) else { continue };
        let (what, ok) = change.verdict(p);
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
        if let Some(what) = classifier.classify_freed(num)
            && !disallowed.contains(&what)
        {
            disallowed.push(what);
        }
    }
    if disallowed.is_empty() { Modification::Allowed(allowed) } else { Modification::Disallowed(disallowed) }
}

/// Partition the signed revision's signature objects into signed widgets, signed field/value/policy
/// objects, and unsigned signature fields/widgets.
fn signed_sig_roles(old: &Document) -> (HashSet<ObjRef>, HashSet<ObjRef>, HashSet<ObjRef>) {
    let mut signed_sig_widgets: HashSet<ObjRef> = HashSet::new();
    let mut signed_sig_fields: HashSet<ObjRef> = HashSet::new();
    let mut unsigned_sig_objects: HashSet<ObjRef> = HashSet::new();
    for f in sig_fields(old) {
        if let Some(v) = f.signature(old) {
            signed_sig_fields.extend(f.r);
            if let Some(vr) = f.dict.get(b"V").and_then(Object::as_ref) {
                signed_sig_fields.insert(vr);
            }
            if let Some(refs) = v.get(b"Reference").map(|o| old.resolve(o)).and_then(|o| o.as_array().cloned()) {
                for entry in &refs {
                    if let Some(er) = entry.as_ref() {
                        signed_sig_fields.insert(er);
                    }
                    if let Some(tr) = old.resolve(entry).as_dict().and_then(|d| d.get(b"TransformParams").and_then(Object::as_ref)) {
                        signed_sig_fields.insert(tr);
                    }
                }
            }
            signed_sig_widgets.extend(f.widgets.iter().filter_map(|(r, _)| *r));
        } else {
            unsigned_sig_objects.extend(f.r);
            unsigned_sig_objects.extend(f.widgets.iter().filter_map(|(r, _)| *r));
        }
    }
    if let Some(perms_ref) = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"Perms").and_then(Object::as_ref))) {
        signed_sig_fields.insert(perms_ref);
    }
    (signed_sig_widgets, signed_sig_fields, unsigned_sig_objects)
}

/// A classified object change, distinguishing document timestamps (permitted even under DocMDP
/// `P=1`) from other signature and document modifications.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassifiedChange {
    /// An RFC 3161 document timestamp change, reported as [`kind::SIGNATURE`] and permitted under
    /// all DocMDP levels including `P=1` (ISO 32000-2 §12.8.2.2.2 / PAdES-LTA).
    DocTimestamp,
    /// Any other change kind.
    Kind(&'static str),
}

impl ClassifiedChange {
    /// The reported change kind and whether it is permitted under DocMDP level `p`.
    fn verdict(self, p: Option<u8>) -> (&'static str, bool) {
        match self {
            Self::DocTimestamp => (kind::SIGNATURE, true),
            Self::Kind(what) => {
                let ok = matches!(
                    (what, p),
                    (kind::SECURITY_STORE | kind::METADATA, _)
                        | (kind::SIGNATURE | kind::FORM_FILL, Some(2 | 3) | None)
                        | (kind::COMMENTS, Some(3) | None)
                );
                (what, ok)
            }
        }
    }
}

/// The page-tree nodes (`/Pages` dictionaries and indirect `/Kids` arrays) of the signed revision,
/// reached from the signed catalog's `/Pages` entry or from any signed page's `/Parent` chain.
fn signed_page_tree(old: &Document, pages: &HashSet<ObjRef>) -> HashSet<ObjRef> {
    const LIMIT: usize = 50_000;
    let mut tree = HashSet::new();
    let mut stack = Vec::new();
    if let Some(root_pages) = old.root().and_then(|r| old.get(r).as_dict().and_then(|c| c.get(b"Pages").and_then(Object::as_ref))) {
        stack.push(root_pages);
    }
    for pg in pages {
        if let Some(parent) = old.get(*pg).as_dict().and_then(|d| d.get(b"Parent").and_then(Object::as_ref)) {
            stack.push(parent);
        }
    }
    while let Some(r) = stack.pop() {
        if pages.contains(&r) || Some(r) == old.root() || tree.len() >= LIMIT || !tree.insert(r) {
            continue;
        }
        if let Some(d) = old.get(r).as_dict() {
            if let Some(parent) = d.get(b"Parent").and_then(Object::as_ref) {
                stack.push(parent);
            }
            if let Some(kids) = d.get(b"Kids") {
                if let Some(kr) = kids.as_ref() {
                    tree.insert(kr);
                }
                if let Some(arr) = old.resolve(kids).as_array() {
                    for kid in arr {
                        if let Some(kr) = kid.as_ref()
                            && !pages.contains(&kr)
                        {
                            stack.push(kr);
                        }
                    }
                }
            }
        }
    }
    tree
}

/// Context for classifying individual object changes between the signed revision `old` and `doc`.
struct ChangedObjectClassifier<'a> {
    doc: &'a Document,
    old: &'a Document,
    old_content: &'a HashSet<ObjRef>,
    old_metadata: Option<ObjRef>,
    old_acroform: Option<ObjRef>,
    old_xfa_data: &'a HashSet<ObjRef>,
    signed_pages: &'a HashSet<ObjRef>,
    signed_page_tree: &'a HashSet<ObjRef>,
    signed_annots: &'a HashSet<ObjRef>,
    signed_sig_widgets: &'a HashSet<ObjRef>,
    signed_sig_fields: &'a HashSet<ObjRef>,
    unsigned_sig_objects: &'a HashSet<ObjRef>,
    signed_resources: &'a HashMap<ObjRef, bool>,
    old_info: Option<ObjRef>,
    dss: &'a HashSet<u32>,
}

impl ChangedObjectClassifier<'_> {
    /// Classify a modified or added object `r` by the role it held in the signed revision.
    fn classify(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<ClassifiedChange> {
        if let Some(kind) = self.resource_kind(r, new, before) {
            return Some(ClassifiedChange::Kind(kind));
        }
        if self.dss.contains(&r.num) {
            return Some(ClassifiedChange::Kind(kind::SECURITY_STORE));
        }
        if self.signed_page_tree.contains(&r) {
            return Some(ClassifiedChange::Kind(kind::PAGES));
        }
        if self.signed_sig_widgets.contains(&r) {
            return Some(ClassifiedChange::Kind(kind::PAGE_CONTENT));
        }
        if self.signed_sig_fields.contains(&r) {
            return Some(ClassifiedChange::Kind(kind::OTHER));
        }
        match new {
            Object::Dict(dict) if Some(r) == self.old.root() => Some(catalog_change_kind(self.doc, self.old, dict, before.and_then(Object::as_dict))),
            _ if Some(r) == self.old.root() => Some(ClassifiedChange::Kind(kind::STRUCTURE)),
            Object::Dict(dict) if self.signed_pages.contains(&r) => {
                Some(page_change_kind(self.doc, self.old, dict, before.and_then(Object::as_dict)))
            }
            _ if self.signed_pages.contains(&r) => Some(ClassifiedChange::Kind(kind::PAGE_CONTENT)),
            Object::Array(now) if self.is_signed_annotation_array(r, before) => Some(self.annotation_array_change(now, before)),
            Object::Dict(dict) => Some(self.dictionary_change(r, dict, before)),
            Object::Stream(stream) => self.stream_change(r, stream, before).map(ClassifiedChange::Kind),
            _ if before.is_none() => None,
            _ => Some(ClassifiedChange::Kind(kind::OTHER)),
        }
    }

    /// Classify changes to an object that was a page resource in the signed revision.
    fn resource_kind(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (self.signed_resources.get(&r), new) {
            (Some(false), _) => Some(kind::PAGE_CONTENT),
            (Some(true), Object::Dict(dict)) if self.keeps_previous_entries(dict, before) => None,
            (Some(true), _) => Some(kind::PAGE_CONTENT),
            (None, _) => None,
        }
    }

    /// Whether `dict` preserves every key-value entry that `before` had.
    fn keeps_previous_entries(&self, dict: &Dict, before: Option<&Object>) -> bool {
        before.and_then(Object::as_dict).is_some_and(|previous| previous.iter().all(|(key, value)| dict.get(key) == Some(value)))
    }

    /// Whether `r` was an indirect `/Annots` array of a signed page.
    fn is_signed_annotation_array(&self, r: ObjRef, before: Option<&Object>) -> bool {
        self.signed_annots.contains(&r) && before.is_some_and(|object| object.as_array().is_some())
    }

    /// Classify additions or removals on a signed page's indirect `/Annots` array.
    fn annotation_array_change(&self, now: &[Object], before: Option<&Object>) -> ClassifiedChange {
        let was: HashSet<ObjRef> =
            before.and_then(Object::as_array).map(|items| items.iter().filter_map(Object::as_ref).collect()).unwrap_or_default();
        let kept: HashSet<ObjRef> = now.iter().filter_map(Object::as_ref).collect();
        let removed: Vec<ObjRef> = was.iter().filter(|reference| !kept.contains(reference)).copied().collect();
        if removed.is_empty() {
            let added: Vec<ObjRef> = now.iter().filter_map(Object::as_ref).filter(|reference| !was.contains(reference)).collect();
            added_annots(self.doc, &added)
        } else if removed.iter().any(|reference| self.old.get(*reference).as_dict().is_some_and(|dict| dict.name(b"Subtype") == Some(b"Widget"))) {
            // A form field or signature widget taken off the page: no permission covers it.
            ClassifiedChange::Kind(kind::OTHER)
        } else {
            // Deleting comments is a comment change (permitted at P 3 only).
            ClassifiedChange::Kind(kind::COMMENTS)
        }
    }

    /// Classify a changed or newly added dictionary object.
    fn dictionary_change(&self, r: ObjRef, dict: &Dict, before: Option<&Object>) -> ClassifiedChange {
        let previous = before.and_then(Object::as_dict);
        if previous.is_some() && Some(r) == self.old_acroform {
            acroform_kind(self.doc, self.old, dict, previous)
        } else if previous.is_some() && Some(r) == self.old_info {
            // Only the signed revision's own /Info dictionary is a metadata change.
            ClassifiedChange::Kind(kind::METADATA)
        } else {
            change_kind(self.doc, self.old, dict, previous, self.unsigned_sig_objects.contains(&r))
        }
    }

    /// Classify a changed or newly added stream object.
    fn stream_change(&self, r: ObjRef, stream: &pdfcraft_cos::Stream, before: Option<&Object>) -> Option<&'static str> {
        // File plumbing is the stream's role when signed; a later /Type label cannot change it.
        let was_plumbing = matches!(before, Some(Object::Stream(previous)) if matches!(previous.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")));
        if self.old_content.contains(&r) {
            Some(kind::PAGE_CONTENT)
        } else if before.is_none() || was_plumbing {
            None
        } else if stream.dict.name(b"Type") == Some(b"Metadata") && Some(r) == self.old_metadata {
            // Only the signed catalog's XMP stream is metadata.
            Some(kind::METADATA)
        } else if self.old_xfa_data.contains(&r) {
            // Signed XFA data and state packets are changed by a form fill.
            Some(kind::FORM_FILL)
        } else {
            Some(kind::OTHER)
        }
    }

    /// Classify an object `num` that existed in the signed revision and was freed in a later one.
    fn classify_freed(&self, num: u32) -> Option<&'static str> {
        let r = ObjRef { num, generation: self.old.generation(num) };
        let was = self.old.get(r);
        let page_role = self.old_content.contains(&r)
            || self.signed_pages.contains(&r)
            || self.signed_resources.contains_key(&r)
            || self.signed_sig_widgets.contains(&r);
        let struct_role = Some(r) == self.old.root();
        let pages_role = self.signed_page_tree.contains(&r);
        let signed_role = page_role || struct_role || pages_role;
        // A signed page's content or resource is judged by that role, whatever /Type its stream carried.
        if !signed_role && matches!(&*was, Object::Stream(s) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm"))) {
            return None;
        }
        Some(if struct_role {
            kind::STRUCTURE
        } else if pages_role {
            kind::PAGES
        } else if page_role {
            kind::PAGE_CONTENT
        } else {
            kind::OTHER
        })
    }
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

/// Whether `d` is a document timestamp dictionary (`/SubFilter /ETSI.RFC3161` with `/ByteRange`
/// and `/Contents`).
fn is_doc_timestamp_dict(d: &Dict) -> bool {
    super::validate::is_doc_timestamp(d) && d.contains(b"ByteRange") && d.contains(b"Contents")
}

/// Whether a signature field or widget dictionary `d` holds a document timestamp in `/V`
/// (directly or on its `/Parent` field).
fn is_timestamp_field_or_widget(doc: &Document, d: &Dict) -> bool {
    let v = d.get(b"V").cloned().or_else(|| d.get(b"Parent").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().and_then(|pd| pd.get(b"V").cloned())));
    v.is_some_and(|v| doc.resolve(&v).as_dict().is_some_and(is_doc_timestamp_dict))
}

/// Whether `doc` gained a new document timestamp dictionary compared with `old`.
fn has_added_doc_timestamp(doc: &Document, old: &Document) -> bool {
    doc.object_numbers().into_iter().any(|num| {
        old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null)).is_none()
            && doc.get(ObjRef { num, generation: doc.generation(num) }).as_dict().is_some_and(is_doc_timestamp_dict)
    })
}

/// What adding `added` annotations amounts to: signing (signature widgets only), form fill
/// (other widgets) or comments.
fn added_annots(doc: &Document, added: &[ObjRef]) -> ClassifiedChange {
    let kinds: Vec<(bool, bool, bool)> = added
        .iter()
        .map(|r| {
            let o = doc.get(*r);
            let d = o.as_dict();
            (
                d.is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")),
                d.is_some_and(|d| d.name(b"FT") == Some(b"Sig")),
                d.is_some_and(|d| is_timestamp_field_or_widget(doc, d)),
            )
        })
        .collect();
    if !kinds.is_empty() && kinds.iter().all(|(w, s, _)| *w && *s) {
        if kinds.iter().all(|(_, _, ts)| *ts) { ClassifiedChange::DocTimestamp } else { ClassifiedChange::Kind(kind::SIGNATURE) }
    } else if !kinds.is_empty() && kinds.iter().all(|(w, _, _)| *w) {
        ClassifiedChange::Kind(kind::FORM_FILL)
    } else {
        ClassifiedChange::Kind(kind::COMMENTS)
    }
}

/// Object references in an indirect or direct array `o`.
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
fn acroform_kind(doc: &Document, old: &Document, now: &Dict, before: Option<&Dict>) -> ClassifiedChange {
    let fallback = acroform(old).unwrap_or_default();
    let before = before.unwrap_or(&fallback);
    let strip = |d: &Dict| -> Vec<(Vec<u8>, Object)> {
        let mut v: Vec<(Vec<u8>, Object)> =
            d.iter().filter(|(k, _)| !matches!(k.as_slice(), b"Fields" | b"SigFlags")).map(|(k, o)| (k.to_vec(), o.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    if strip(now) != strip(before) {
        return ClassifiedChange::Kind(kind::FORM_FILL);
    }
    let old_fields = refs(old, before.get(b"Fields"));
    let now_fields = refs(doc, now.get(b"Fields"));
    if old_fields.iter().any(|r| !now_fields.contains(r)) {
        return ClassifiedChange::Kind(kind::OTHER);
    }
    let added: Vec<ObjRef> = now_fields.into_iter().filter(|r| !old_fields.contains(r)).collect();
    if added.is_empty() {
        if has_added_doc_timestamp(doc, old) { ClassifiedChange::DocTimestamp } else { ClassifiedChange::Kind(kind::SIGNATURE) }
    } else {
        match added_annots(doc, &added) {
            ClassifiedChange::DocTimestamp => ClassifiedChange::DocTimestamp,
            ClassifiedChange::Kind(kind::SIGNATURE) => ClassifiedChange::Kind(kind::SIGNATURE),
            _ => ClassifiedChange::Kind(kind::FORM_FILL),
        }
    }
}

/// Dictionary entries in a stable order, without `keys`.
fn without(x: &Dict, keys: &[&[u8]]) -> Vec<(Vec<u8>, Object)> {
    let mut v: Vec<(Vec<u8>, Object)> = x.iter().filter(|(k, _)| !keys.contains(&k.as_slice())).map(|(k, o)| (k.to_vec(), o.clone())).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Whether `b` in the signed revision was a signature dictionary, a timestamp, a signature
/// reference, or transform parameters — an immutable signature object once signed.
fn is_signed_sig_value_or_ref(b: &Dict) -> bool {
    matches!(b.name(b"Type"), Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams"))
        || b.contains(b"ByteRange")
        || b.contains(b"TransformMethod")
}

/// Whether `b` in the signed revision was an unsigned signature form field (`/FT /Sig` without a
/// signature dictionary in `/V`).
fn is_unsigned_sig_dict(old: &Document, b: &Dict) -> bool {
    b.name(b"FT") == Some(b"Sig") && !is_signed_sig_value_or_ref(b) && !b.get(b"V").is_some_and(|v| old.resolve(v).as_dict().is_some())
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

/// A changed object that was the catalog in the signed revision, judged by its role as the
/// catalog whatever `/Type` its new version carries.
fn catalog_change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> ClassifiedChange {
    // /NeedsRendering selects dynamic XFA rendering. A change of it is structure, not metadata.
    let keys: &[&[u8]] = &[b"AcroForm", b"DSS", b"Perms", b"Metadata", b"Extensions"];
    match before {
        Some(b) if without(d, keys) != without(b, keys) => ClassifiedChange::Kind(kind::STRUCTURE),
        // Developer extensions (ISO 32000-2 §7.12) only declare what the file uses. A
        // writer adding a /DSS also declares the ESIC extension it belongs to, so a
        // declaration may be added, but none that was signed may change or go.
        Some(b) if !extensions_kept(doc, old, d, b) => ClassifiedChange::Kind(kind::STRUCTURE),
        // Removing or replacing a `/Perms` dictionary that was already signed is never permitted.
        Some(b) if b.contains(b"Perms") && b.get(b"Perms") != d.get(b"Perms") => ClassifiedChange::Kind(kind::STRUCTURE),
        Some(b) => {
            if d.get(b"AcroForm") != b.get(b"AcroForm") {
                // Compare the forms themselves, inline or not.
                let now = acroform(doc).unwrap_or_default();
                acroform_kind(doc, old, &now, None)
            } else if b.get(b"Perms") != d.get(b"Perms") {
                ClassifiedChange::Kind(kind::SIGNATURE)
            } else {
                ClassifiedChange::Kind(kind::METADATA)
            }
        }
        None => ClassifiedChange::Kind(kind::STRUCTURE),
    }
}

/// A changed object that was a page in the signed revision, judged by what the page shows rather
/// than by any `/Type` its new version carries. Only added annotations are permitted; any other
/// difference (content, resources, a relabelled or dropped `/Type`) is a page content change.
fn page_change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> ClassifiedChange {
    match before {
        Some(b) if without(d, &[b"Annots"]) != without(b, &[b"Annots"]) => ClassifiedChange::Kind(kind::PAGE_CONTENT),
        Some(b) => {
            let was = refs(old, b.get(b"Annots"));
            let now = refs(doc, d.get(b"Annots"));
            let removed: Vec<ObjRef> = was.iter().filter(|r| !now.contains(r)).copied().collect();
            if removed.is_empty() {
                let added: Vec<ObjRef> = now.into_iter().filter(|r| !was.contains(r)).collect();
                added_annots(doc, &added)
            } else if removed.iter().any(|r| old.get(*r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget"))) {
                ClassifiedChange::Kind(kind::OTHER)
            } else {
                ClassifiedChange::Kind(kind::COMMENTS)
            }
        }
        None => ClassifiedChange::Kind(kind::PAGE_CONTENT),
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

/// Classify a signature field or widget dictionary `d` as either a document timestamp or a
/// regular signature change.
fn sig_field_change(doc: &Document, d: &Dict, before: Option<&Dict>) -> ClassifiedChange {
    let only_filled = before.is_none_or(|b| without(d, &[b"V", b"AP", b"Ff"]) == without(b, &[b"V", b"AP", b"Ff"]));
    if only_filled && is_timestamp_field_or_widget(doc, d) { ClassifiedChange::DocTimestamp } else { ClassifiedChange::Kind(kind::SIGNATURE) }
}

/// Classify a modified or newly added dictionary `d` against its signed version `before`.
fn change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>, was_unsigned_sig: bool) -> ClassifiedChange {
    let ty = d.name(b"Type");
    // A signature widget or field that already held a signature when signed, or a signed
    // signature/timestamp/SigRef/TransformParams dictionary, must never be rewritten later.
    if let Some(b) = before {
        if b.name(b"Subtype") == Some(b"Widget")
            && b.name(b"FT") == Some(b"Sig")
            && b.get(b"V").is_some_and(|v| old.resolve(v).as_dict().is_some())
            && before != Some(d)
        {
            return ClassifiedChange::Kind(kind::PAGE_CONTENT);
        }
        if is_signed_sig_value_or_ref(b) || (b.name(b"FT") == Some(b"Sig") && b.get(b"V").is_some_and(|v| old.resolve(v).as_dict().is_some())) {
            return ClassifiedChange::Kind(kind::OTHER);
        }
        if b.name(b"Type") == Some(b"Pages") {
            return ClassifiedChange::Kind(kind::PAGES);
        }
        if b.name(b"Type") == Some(b"Catalog") {
            return catalog_change_kind(doc, old, d, Some(b));
        }
        if b.name(b"Type") == Some(b"Page") {
            return page_change_kind(doc, old, d, Some(b));
        }
    }
    // A later revision cannot make an existing object a signature by relabelling it: a signature
    // change is only a brand-new object, or one that was an unsigned signature field/widget when signed.
    let signature_ok = before.is_none_or(|b| was_unsigned_sig || is_unsigned_sig_dict(old, b));
    match ty {
        // Only a new signature value or policy: rewriting a signed one is not a signature (#1788).
        Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams") if before.is_none() => {
            return if is_doc_timestamp_dict(d) { ClassifiedChange::DocTimestamp } else { ClassifiedChange::Kind(kind::SIGNATURE) };
        }
        Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams") => return ClassifiedChange::Kind(kind::OTHER),
        Some(b"Catalog") => return if before.is_some() { catalog_change_kind(doc, old, d, before) } else { ClassifiedChange::Kind(kind::STRUCTURE) },
        Some(b"Page") => return if before.is_some() { page_change_kind(doc, old, d, before) } else { ClassifiedChange::Kind(kind::COMMENTS) },
        Some(b"Pages") => return ClassifiedChange::Kind(if before.is_some() { kind::PAGES } else { kind::STRUCTURE }),
        _ => {}
    }
    if let Some(b) = before {
        // An existing dictionary is judged by the role it already held in the signed revision:
        // dropping `/Type` or adding `/FT`, `/Subtype`, `/Fields`, or `/DR` later cannot turn
        // an arbitrary signed dictionary into a form fill or comment annotation.
        if b.name(b"Type").is_some() && d.name(b"Type") != b.name(b"Type") {
            return ClassifiedChange::Kind(kind::OTHER);
        }
        match b.name(b"Subtype") {
            Some(b"Widget") if d.name(b"Subtype") == Some(b"Widget") => {
                return if d.name(b"FT") == Some(b"Sig") && signature_ok {
                    sig_field_change(doc, d, before)
                } else {
                    ClassifiedChange::Kind(kind::FORM_FILL)
                };
            }
            Some(b"Link") if d.name(b"Subtype") == Some(b"Link") => return ClassifiedChange::Kind(kind::LINKS),
            Some(st) if d.name(b"Subtype") == Some(st) && (b.name(b"Type") == Some(b"Annot") || b.contains(b"Rect")) => {
                return ClassifiedChange::Kind(kind::COMMENTS);
            }
            _ => {}
        }
        if d.name(b"FT") == Some(b"Sig") && signature_ok {
            return sig_field_change(doc, d, before);
        }
        if b.contains(b"Fields") && d.contains(b"Fields") {
            return acroform_kind(doc, old, d, Some(b));
        }
        if (b.contains(b"FT") && d.name(b"FT") == b.name(b"FT"))
            || (b.contains(b"T") && b.contains(b"Kids") && d.contains(b"T") && d.contains(b"Kids"))
            || (b.contains(b"DR") && d.contains(b"DR"))
        {
            return ClassifiedChange::Kind(kind::FORM_FILL);
        }
        return ClassifiedChange::Kind(kind::OTHER);
    }
    match d.name(b"Subtype") {
        Some(b"Widget") => {
            return if d.name(b"FT") == Some(b"Sig") && signature_ok {
                sig_field_change(doc, d, None)
            } else {
                ClassifiedChange::Kind(kind::FORM_FILL)
            };
        }
        Some(b"Link") => return ClassifiedChange::Kind(kind::LINKS),
        Some(_) if ty == Some(b"Annot") || d.contains(b"Rect") => return ClassifiedChange::Kind(kind::COMMENTS),
        _ => {}
    }
    if d.name(b"FT") == Some(b"Sig") && signature_ok {
        return sig_field_change(doc, d, None);
    }
    if d.contains(b"Fields") {
        return acroform_kind(doc, old, d, before);
    }
    if d.contains(b"FT") || (d.contains(b"T") && d.contains(b"Kids")) || d.contains(b"DR") {
        return ClassifiedChange::Kind(kind::FORM_FILL);
    }
    ClassifiedChange::Kind(kind::FORM_FILL)
}
