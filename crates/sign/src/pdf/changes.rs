use super::types::kind;
use super::{Arc, Dict, Document, HashMap, HashSet, ObjRef, Object};
use std::cell::{Cell, OnceCell};

/// The most annotation-list entries one classification expands. Pages sharing one long /Annots
/// array, or many pages rewritten to list one, would otherwise cost pages × entries; past this
/// the lists are not compared, and the change is refused.
const MAX_ANNOT_ENTRIES: usize = 1 << 22;

/// Where each revision of the document ends ([`Document::revision_ends`]), found on first use and
/// then shared by one classification: finding them scans the file, and every widget that brings
/// a signature value is checked against them.
pub(super) type RevisionEnds = OnceCell<Vec<usize>>;

/// What the signed revision held, read once for one judgement: the roles its objects played
/// when signed (stage A of #7032). A changed object is judged by these, never by the labels its
/// new, attacker-written version carries.
pub(super) struct Snapshot {
    /// Every content stream (and `/Contents` array) a signed page draws.
    pub(super) old_content: HashSet<u32>,
    /// The signed catalog's document-level XMP stream.
    pub(super) old_metadata: Option<u32>,
    /// The signed revision's XFA data packets: filling the form rewrites them.
    pub(super) old_xfa_data: HashSet<u32>,
    pub(super) signed_pages: HashSet<u32>,
    /// The signed pages' `/Annots` arrays that are objects of their own.
    pub(super) signed_annots: HashSet<u32>,
    /// The annotations the signed pages listed.
    pub(super) placed_annots: HashSet<u32>,
    pub(super) annots_unbounded: bool,
    pub(super) page_nodes: HashSet<u32>,
    pub(super) kids_arrays: HashSet<u32>,
    pub(super) page_tree_unbounded: bool,
    /// What each signed page draws from (#1787).
    pub(super) signed_resources: HashMap<u32, bool>,
    pub(super) old_info: Option<u32>,
    /// The signed catalog's `/AcroForm`, when it is an object of its own.
    pub(super) old_form: Option<u32>,
    pub(super) annot_budget: Cell<usize>,
}

impl Snapshot {
    pub(super) fn of(old: &Document) -> Snapshot {
        // Every role below is keyed by object number, not by the reference as written: a damaged
        // file's slot accepts two generations (#1143), and a page signed as `3 5 R` and edited as
        // `3 0 R` is still that page. The loop over changed objects refuses any slot whose accepted
        // generations changed, so a number names the same slot in both revisions.
        let old_content: HashSet<u32> = pdfcraft_annot::page_refs(old)
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
            .map(|r| r.num)
            .collect();
        let old_metadata =
            old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"Metadata").and_then(Object::as_ref))).map(|r| r.num);
        let old_xfa_data = xfa_data_packets(old);
        let signed_page_refs = pdfcraft_annot::page_refs(old).unwrap_or_default();
        let signed_pages: HashSet<u32> = signed_page_refs.iter().map(|r| r.num).collect();
        // Adding an annotation updates such an array in place (the page dictionary does not
        // change), so it is judged like a page's inline /Annots: by the annotations it gained.
        let signed_annots: HashSet<u32> = signed_page_refs
            .iter()
            .filter_map(|pg| old.get(*pg).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref)))
            .map(|r| r.num)
            .collect();
        // One of the signed pages' annotations listed again elsewhere was moved or copied there,
        // not added.
        let annot_budget = Cell::new(MAX_ANNOT_ENTRIES);
        let (mut placed_annots, mut expanded, mut annots_unbounded) = (HashSet::new(), HashSet::new(), false);
        for pg in &signed_page_refs {
            let page = old.get(*pg);
            let Some(list) = page.as_dict().and_then(|d| d.get(b"Annots")) else { continue };
            // Pages sharing one array object list the same annotations.
            if list.as_ref().is_some_and(|r| !expanded.insert(r.num)) {
                continue;
            }
            let Some(found) = annot_refs(old, Some(list), &annot_budget) else {
                annots_unbounded = true;
                break;
            };
            placed_annots.extend(found.into_iter().map(|r| r.num));
        }
        // The signed page tree's nodes: a change to one changes the pages, whatever it is labelled
        // now. Every node, and every /Kids array object of its own, is read once; past the bound
        // the tree is not known in full, and no change is judged against it.
        let page_tree = page_tree_nodes(old);
        let page_tree_unbounded = page_tree.is_none();
        let (page_nodes, kids_arrays) = page_tree.unwrap_or_default();
        let signed_resources = signed_page_resources(old, &signed_page_refs);
        let old_info = old.trailer().get(b"Info").and_then(Object::as_ref).map(|r| r.num);
        let old_form = old.root().and_then(|c| old.get(c).as_dict().and_then(|c| c.get(b"AcroForm").and_then(Object::as_ref))).map(|r| r.num);
        Snapshot {
            old_content,
            old_metadata,
            old_xfa_data,
            signed_pages,
            signed_annots,
            placed_annots,
            annots_unbounded,
            page_nodes,
            kids_arrays,
            page_tree_unbounded,
            signed_resources,
            old_info,
            old_form,
            annot_budget,
        }
    }

    /// Whether object `num` was drawn by a signed page: its content, the page itself, or what it
    /// draws from.
    pub(super) fn signed_role(&self, num: u32) -> bool {
        self.old_content.contains(&num) || self.signed_pages.contains(&num) || self.signed_resources.contains_key(&num)
    }
}

/// One object a later revision changed or added, with what it was when signed (`None` for a new
/// one) and the word that says what changing it amounts to by the role it had then (`None` when
/// the change shows nothing different, as a resources dictionary that only gained names).
pub(super) struct Change {
    pub(super) num: u32,
    pub(super) before: Option<Arc<Object>>,
    pub(super) now: Arc<Object>,
    pub(super) label: Option<&'static str>,
}

/// What a later revision changed, as seen against the signed revision: the changes no operation
/// can explain (`hard`: plumbing a renderer may read differently, freed signed objects, a new
/// catalog), and every object it changed or added.
pub(super) struct Observed {
    pub(super) snapshot: Snapshot,
    pub(super) dss: HashSet<u32>,
    pub(super) ends: RevisionEnds,
    pub(super) hard: Vec<&'static str>,
    pub(super) changes: Vec<Change>,
    /// Objects the signed revision had that are gone now.
    pub(super) freed: Vec<&'static str>,
}

/// Push `what` once.
pub(super) fn note(list: &mut Vec<&'static str>, what: &'static str) {
    if !list.contains(&what) {
        list.push(what);
    }
}

/// Every object a later revision of `doc` changed or added since the signed revision `old`, each
/// labelled by the role it had when signed, and the changes nothing can permit.
pub(super) fn observe(doc: &Document, old: &Document) -> Observed {
    let snapshot = Snapshot::of(old);
    let mut hard = Vec::new();
    let mut changes = Vec::new();
    let s = &snapshot;
    let signed_role = |num: u32| s.signed_role(num);
    if s.annots_unbounded {
        hard.push(kind::OTHER);
    }
    if s.page_tree_unbounded {
        // Past the bound the tree is not known in full: no change is judged against it.
        hard.push(kind::PAGES);
    }
    // Every reference the signed revision makes, read only when a slot's generations narrowed.
    let signed_references = OnceCell::new();
    let dss = dss_parts(doc, old);
    let ends = RevisionEnds::new();
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
        let what = if s.old_content.contains(num) || s.signed_pages.contains(num) { kind::PAGE_CONTENT } else { kind::OTHER };
        note(&mut hard, what);
    }
    refuse_unlisted_bodies(doc, old, &signed_role, &mut hard);
    // The trailer chooses the catalog. A later revision that points it at another catalog (one
    // already in the signed file, so no object changes) shows another document.
    if doc.root() != old.root() {
        hard.push(kind::STRUCTURE);
    }
    let classifier = ChangedObjectClassifier { doc, old, snapshot: s, dss: &dss, ends: &ends };
    for num in doc.object_numbers() {
        if same_place(num) {
            continue;
        }
        // The signed revision's references reached this slot by the generations it accepted. If
        // it accepts others now, those references may reach nothing here while another reader
        // follows them back through /Prev to an intermediate revision, or to another alias, and
        // shows what that holds: the object compared below is not provably what is displayed.
        // No writer changes a signed object's generation. A damaged slot rewritten cleanly
        // accepts fewer: that is the same slot when nothing signed refers to it by a generation
        // it gave up.
        let signed_generations = old.accepted_generations(num);
        let generations = doc.accepted_generations(num);
        let narrowed_unreferenced = || {
            !generations.is_empty()
                && generations.iter().all(|g| signed_generations.contains(g))
                && signed_references.get_or_init(|| references(old)).as_ref().is_some_and(|found: &HashSet<ObjRef>| {
                    signed_generations.iter().filter(|g| !generations.contains(g)).all(|g| !found.contains(&ObjRef { num, generation: *g }))
                })
        };
        if !signed_generations.is_empty() && signed_generations != generations && !narrowed_unreferenced() {
            note(&mut hard, if signed_role(num) { kind::PAGE_CONTENT } else { kind::OTHER });
            continue;
        }
        // A new place for the object. What is read there is what the renderer reads only when
        // the entry locates the object itself: where this reader had to search for it, another
        // reader may arrive at a different body, and an entry that leads to no object at all
        // (which this reader reads as null, and so as nothing) makes the renderer rebuild its
        // object index from the file's headers (#6754). Whatever the object reads as here, no
        // writer has a reason to write such an entry after signing.
        if !doc.is_edited(num) && !doc.xref_entry_is_exact(num) {
            note(&mut hard, kind::OTHER);
            continue;
        }
        let r = ObjRef { num, generation: doc.generation(num) };
        let new = doc.get(r);
        let before = old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null));
        if before.as_deref().is_some_and(|b| same_object(b, &new)) {
            // The same content at a new place.
            continue;
        }
        // A new object that reads as nothing (a null) is no object at all.
        if before.is_none() && matches!(*new, Object::Null) {
            continue;
        }
        // A resource of a signed page is page content whatever keys its new version carries
        // (#1787): a /FT or /Subtype /Widget written by whoever rewrote a font, an XObject or a
        // graphics state does not make that a form fill. A container (the resources dictionary,
        // a category dictionary) that kept every entry it had and only gained names changes
        // nothing the signed content draws, and is judged as before.
        let label = classifier.classify(r, &new, before.as_deref());
        changes.push(Change { num, before, now: new, label });
    }
    // An object the signed revision had and this one frees is a change too, and the loop above
    // only sees objects that still exist. File plumbing (cross-reference and object streams) is
    // rewritten by every save and is not a change.
    let current: HashSet<u32> = doc.object_numbers().into_iter().collect();
    let mut freed = Vec::new();
    for num in old.object_numbers().into_iter().filter(|n| !current.contains(n)) {
        let was = old.get(ObjRef { num, generation: old.generation(num) });
        let signed_role = signed_role(num);
        // A signed page's content or resource is judged by that role, whatever /Type its stream carried.
        if !signed_role && matches!(&*was, Object::Stream(s) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm"))) {
            continue;
        }
        note(&mut freed, if signed_role { kind::PAGE_CONTENT } else { kind::OTHER });
    }
    Observed { snapshot, dss, ends, hard, changes, freed }
}

/// Whether the catalog's `/Perms` differs from the signed revision's. It names the certification
/// signature itself. Elsewhere a change to it is judged as signing; under P=1 dropping or
/// redirecting it is a change like any other. Compared by content: an indirect /Perms rewritten
/// in place keeps its reference (and, shared with the trailer's /Info, would otherwise pass as a
/// metadata change).
pub(super) fn perms_changed(doc: &Document, old: &Document) -> bool {
    let perms = |d: &Document| d.root().and_then(|r| d.get(r).as_dict().and_then(|c| c.get(b"Perms").map(|p| (*d.resolve(p)).clone())));
    !same_entry(perms(doc).as_ref(), perms(old).as_ref())
}

struct ChangedObjectClassifier<'a> {
    doc: &'a Document,
    old: &'a Document,
    snapshot: &'a Snapshot,
    dss: &'a HashSet<u32>,
    ends: &'a RevisionEnds,
}

impl ChangedObjectClassifier<'_> {
    fn classify(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        if let Some(kind) = self.resource_kind(r, new, before) {
            return Some(kind);
        }
        if self.dss.contains(&r.num) {
            return Some(kind::SECURITY_STORE);
        }
        match new {
            // A signed page-tree node's /Kids array of its own lists the pages.
            Object::Array(_) if before.is_some() && self.snapshot.kids_arrays.contains(&r.num) => Some(kind::PAGES),
            Object::Array(_) if self.is_signed_annotation_array(r, before) => self.annotation_array_change(new, before),
            Object::Dict(dict) => self.dictionary_change(r, dict, before),
            Object::Stream(stream) => self.stream_change(r, stream, before),
            _ if before.is_none() => None,
            _ => Some(kind::OTHER),
        }
    }

    fn resource_kind(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (self.snapshot.signed_resources.get(&r.num), new) {
            (Some(false), _) => Some(kind::PAGE_CONTENT),
            (Some(true), Object::Dict(dict)) if self.keeps_previous_entries(dict, before) => None,
            (Some(true), _) => Some(kind::PAGE_CONTENT),
            (None, _) => None,
        }
    }

    fn keeps_previous_entries(&self, dict: &Dict, before: Option<&Object>) -> bool {
        before.and_then(Object::as_dict).is_some_and(|previous| previous.iter().all(|(key, value)| same_entry(dict.get(key), Some(value))))
    }

    fn is_signed_annotation_array(&self, r: ObjRef, before: Option<&Object>) -> bool {
        self.snapshot.signed_annots.contains(&r.num) && before.is_some_and(|object| object.as_array().is_some())
    }

    fn annotation_array_change(&self, now: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (annot_refs(self.old, before, &self.snapshot.annot_budget), annot_refs(self.doc, Some(now), &self.snapshot.annot_budget)) {
            (Some(was), Some(now)) => self.annots_change(&was, &now),
            _ => Some(kind::OTHER),
        }
    }

    /// A signed page's annotations going from `was` to `now`, compared by object number. Only an
    /// entry that resolves is on the page: one rewritten to a generation its slot does not accept
    /// draws nothing, whatever its number. `None` when the page shows the same annotations in
    /// the same order.
    fn annots_change(&self, was: &[ObjRef], now: &[ObjRef]) -> Option<&'static str> {
        let live =
            |doc: &Document, list: &[ObjRef]| -> Vec<ObjRef> { list.iter().filter(|r| !matches!(*doc.get(**r), Object::Null)).copied().collect() };
        let (was, now) = (live(self.old, was), live(self.doc, now));
        let had: HashSet<u32> = was.iter().map(|r| r.num).collect();
        let kept: HashSet<u32> = now.iter().map(|r| r.num).collect();
        let removed: Vec<u32> = had.iter().filter(|num| !kept.contains(num)).copied().collect();
        let added: Vec<ObjRef> = now.iter().filter(|r| !had.contains(&r.num)).copied().collect();
        let was_widget = |num: u32| self.old.try_get(num).ok().is_some_and(|o| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")));
        // A signed annotation listed on another page was moved or copied there.
        let moved: Vec<u32> = added.iter().map(|r| r.num).filter(|num| self.snapshot.placed_annots.contains(num)).collect();
        // The annotations kept, in their new order: another order draws another one on top.
        let reordered = !now.iter().map(|r| r.num).filter(|num| had.contains(num)).eq(was.iter().map(|r| r.num));
        if removed.iter().chain(&moved).any(|num| was_widget(*num)) {
            // A form field or signature widget taken off its page, or onto another: no permission covers it.
            Some(kind::OTHER)
        } else if !removed.is_empty() || !moved.is_empty() || reordered {
            // Deleting, moving or restacking comments is a comment change (permitted at P 3 only).
            Some(kind::COMMENTS)
        } else if added.is_empty() {
            None
        } else {
            Some(added_annots(self.doc, self.old, self.ends, &added))
        }
    }

    fn dictionary_change(&self, r: ObjRef, dict: &Dict, before: Option<&Object>) -> Option<&'static str> {
        let previous = before.and_then(Object::as_dict);
        if self.snapshot.signed_pages.contains(&r.num) {
            // A signed page is judged by what the page shows, whatever its new labels say.
            self.page_change_kind(dict, previous)
        } else if previous.is_some() && self.snapshot.page_nodes.contains(&r.num) {
            // A signed page-tree node is a page-tree change, whatever its new version is labelled.
            Some(kind::PAGES)
        } else if let Some(previous) = previous.filter(|_| self.old.root().is_some_and(|root| root.num == r.num)) {
            // The signed catalog, whatever /Type its new version carries.
            Some(catalog_kind(self.doc, self.old, self.ends, dict, previous))
        } else if let Some(previous) = previous.filter(|_| self.snapshot.old_form == Some(r.num)) {
            // The signed form dictionary, held in an object of its own, whatever keys it carries now.
            Some(acroform_kind(self.doc, self.old, self.ends, dict, Some(previous)))
        } else if previous.is_some() && Some(r.num) == self.snapshot.old_info {
            // Only the signed revision's own /Info dictionary is a metadata change.
            Some(kind::METADATA)
        } else {
            // A new dictionary is not labelled by its shape: an operation claims it or it is refused.
            previous.map(|previous| change_kind(self.doc, self.old, self.ends, r.num, dict, previous))
        }
    }

    /// A changed object that was a page in the signed revision, judged by what the page shows
    /// rather than by any `/Type` its new version carries. Only its annotations may change (see
    /// [`Self::annots_change`]); any other difference (content, resources, a relabelled or dropped
    /// `/Type`) is a page content change.
    fn page_change_kind(&self, d: &Dict, before: Option<&Dict>) -> Option<&'static str> {
        match before {
            Some(b) if !same_but_annots(d, b) => Some(kind::PAGE_CONTENT),
            Some(b) => match (
                annot_refs(self.old, b.get(b"Annots"), &self.snapshot.annot_budget),
                annot_refs(self.doc, d.get(b"Annots"), &self.snapshot.annot_budget),
            ) {
                (Some(was), Some(now)) => self.annots_change(&was, &now),
                _ => Some(kind::OTHER),
            },
            None => Some(kind::PAGE_CONTENT),
        }
    }

    fn stream_change(&self, r: ObjRef, stream: &pdfcraft_cos::Stream, before: Option<&Object>) -> Option<&'static str> {
        // File plumbing is the stream's role when signed; a later /Type label cannot change it.
        let was_plumbing = matches!(before, Some(Object::Stream(previous)) if matches!(previous.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")));
        if self.snapshot.old_content.contains(&r.num) {
            Some(kind::PAGE_CONTENT)
        } else if before.is_none() || was_plumbing {
            None
        } else if stream.dict.name(b"Type") == Some(b"Metadata") && Some(r.num) == self.snapshot.old_metadata {
            // Only the signed catalog's XMP stream is metadata.
            Some(kind::METADATA)
        } else if self.snapshot.old_xfa_data.contains(&r.num) {
            // Signed XFA data and state packets are changed by a form fill.
            Some(kind::FORM_FILL)
        } else {
            Some(kind::OTHER)
        }
    }
}

/// The streams of `doc`'s XFA form that hold its data and state: the `datasets` and `form`
/// packets of the AcroForm's `/XFA` array (name, stream, name, stream, …). A form given as one
/// XFA stream holds its template too, and is not listed.
fn xfa_data_packets(doc: &Document) -> HashSet<u32> {
    let Some(form) = acroform(doc) else { return HashSet::new() };
    let Some(xfa) = form.get(b"XFA").map(|x| doc.resolve(x)) else { return HashSet::new() };
    let Object::Array(xfa) = &*xfa else { return HashSet::new() };
    xfa.as_chunks::<2>()
        .0
        .iter()
        .filter(|[name, _]| doc.resolve(name).as_string().is_some_and(|s| matches!(s.to_text().as_str(), "datasets" | "form")))
        .filter_map(|[_, packet]| packet.as_ref())
        .map(|r| r.num)
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

/// The objects that hold the store's structure in `doc`: its dictionary, arrays, `/VRI` and its
/// entries, but not the data they list. One of them is validation data only as [`dss_parts`]
/// recognises it, never because the store reaches it.
pub(super) fn dss_structure(doc: &Document) -> HashSet<u32> {
    dss_walk(doc).into_iter().filter(|(_, role)| *role != DssRole::Data).map(|(r, _)| r.num).collect()
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
                _ => b.len() <= MAX_COMPARED && n.len() <= MAX_COMPARED && b.iter().all(|x| n.iter().any(|y| same_object(x, y))),
            }
        }
        (Object::Dict(b), Object::Dict(n)) => b.iter().all(|(k, v)| match n.get(k) {
            Some(nv) => same_object(nv, v) || (depth < 4 && only_grew(doc, old, &old.resolve(v), &doc.resolve(nv), depth + 1)),
            None => false,
        }),
        _ => same_object(before, now),
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
            // `/Filter`, a name, is what PdfCraft's own store carries beside the standard keys;
            // it names the handler and reaches no object.
            (DssRole::Dss, Object::Dict(d)) => d.iter().all(|(k, v)| match k.as_slice() {
                b"Type" | b"Certs" | b"CRLs" | b"OCSPs" | b"VRI" => true,
                b"Filter" => matches!(v, Object::Name(_)),
                _ => false,
            }),
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

/// What adding `added` annotations amounts to: signing when each is a signature widget that
/// brings a new signature with it, else comments. Filling a form changes the values of the
/// fields it has; it adds no widget (ISO 32000-2 Table 257 permits creating annotations only at
/// P=3). An empty signature widget, or a form widget, is a new annotation that may draw anything
/// over the signed page (#6755, #6756).
fn added_annots(doc: &Document, old: &Document, ends: &RevisionEnds, added: &[ObjRef]) -> &'static str {
    let signs =
        |r: &ObjRef| doc.get(*r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget") && gains_signature(doc, old, ends, r.num, d, None));
    if !added.is_empty() && added.iter().all(signs) { kind::SIGNATURE } else { kind::COMMENTS }
}

/// The most `/Parent` links followed to find what a field inherits.
const MAX_FIELD_DEPTH: usize = 64;

/// The entry `key` of the form field or widget `d`, or else of its nearest ancestor field that
/// has it (ISO 32000-2 §12.7.4.1: `/FT` and `/V` are inheritable), with the number of the object
/// that holds it (`None` for `d` itself). The `/Parent` chain is followed at most
/// [`MAX_FIELD_DEPTH`] links, and not round a cycle.
pub(super) fn inherited(doc: &Document, d: &Dict, key: &[u8]) -> Option<(Option<u32>, Object)> {
    if let Some(v) = d.get(key) {
        return Some((None, v.clone()));
    }
    let mut seen = HashSet::new();
    let mut parent = d.get(b"Parent").and_then(Object::as_ref);
    while let Some(r) = parent {
        if !seen.insert(r.num) || seen.len() > MAX_FIELD_DEPTH {
            return None;
        }
        let field = doc.get(r);
        let field = field.as_dict()?;
        if let Some(v) = field.get(key) {
            return Some((Some(r.num), v.clone()));
        }
        parent = field.get(b"Parent").and_then(Object::as_ref);
    }
    None
}

/// Whether `d`, a form field or widget, is (or belongs to) a signature field.
pub(super) fn is_signature_field(doc: &Document, d: &Dict) -> bool {
    inherited(doc, d, b"FT").is_some_and(|(_, ft)| doc.resolve(&ft).as_name() == Some(b"Sig".as_slice()))
}

/// Whether the signature field or widget `d`, object `num` (as it was when signed: `before`, or
/// nothing for a new one), now carries a signature it did not carry then: its value, its own or
/// inherited, is a signature dictionary that is not one the signed revision held, written in the
/// same revision as `d` as it is now, and the field held no signature when signed. Rewriting an
/// empty signature field's appearance, pointing a new widget at a signature already in the file,
/// or adding a widget after the signature whose value it takes, signs nothing (#6755).
pub(super) fn gains_signature(doc: &Document, old: &Document, ends: &RevisionEnds, num: u32, d: &Dict, before: Option<&Dict>) -> bool {
    if !is_signature_field(doc, d) {
        return false;
    }
    let Some((holder, value)) = inherited(doc, d, b"V") else { return false };
    let Some(signed) = doc.resolve(&value).as_dict().and_then(|v| signed_revision(doc, old, ends, v)) else { return false };
    // Signing writes the widget that shows a signature in the revision that signature signs. One
    // added or changed in a later revision only points at an earlier signature.
    if !stored_at(doc, num).iter().any(|at| signed.contains(at)) {
        return false;
    }
    let existed = |num: u32| old.try_get(num).ok().is_some_and(|o| !matches!(*o, Object::Null));
    if value.as_ref().is_some_and(|r| existed(r.num)) {
        return false;
    }
    // The value the holder of this one had when signed: the widget itself, or the ancestor field.
    let signed_value = match holder {
        None => before.and_then(|b| inherited(old, b, b"V")).map(|(_, v)| v),
        Some(num) => old.try_get(num).ok().and_then(|o| o.as_dict().and_then(|f| f.get(b"V").cloned())),
    };
    !signed_value.is_some_and(|v| old.resolve(&v).as_dict().is_some())
}

/// The bytes of the revision of `doc` that the signature dictionary `v` signs, when that is a
/// revision added after the signed revision `old`: `v` has a `/Contents` string, and a
/// `/ByteRange` `[0 a b c]` (`a` ≤ `b`) that ends where that revision ends. That is what signing (a
/// document timestamp included) writes, with or without `/Type`, and what the signature list then
/// checks, so a forged one is shown as invalid there. Any other dictionary in `/V` signs nothing:
/// `<< /Foo /Bar >>` under an opaque appearance is a new annotation, not a signature.
pub(super) fn signed_revision(doc: &Document, old: &Document, ends: &RevisionEnds, v: &Dict) -> Option<std::ops::Range<usize>> {
    let whole = |x: &f64| x.fract() == 0.0 && (0.0..9.0e15).contains(x);
    let range = super::listing::nums(doc, v, b"ByteRange").and_then(|r| <[f64; 4]>::try_from(r).ok());
    let [start, first_end, second_start, second_len] = range.filter(|r| r.iter().all(whole))?;
    let contents = v.get(b"Contents").is_some_and(|c| doc.resolve(c).as_string().is_some());
    // Whole and below 9e15, so the sum is exact and fits. A revision's end counts up to two
    // end-of-line bytes after its `%%EOF`, which a signature's range may leave out.
    let end = (second_start + second_len) as usize;
    if !contents || start != 0.0 || first_end > second_start || end <= old.bytes().len() {
        return None;
    }
    let ends = ends.get_or_init(|| doc.revision_ends());
    let at = ends.iter().position(|e| (end..=end.saturating_add(2)).contains(e))?;
    let from = at.checked_sub(1).and_then(|i| ends.get(i)).copied().unwrap_or(0);
    Some(from..ends.get(at).copied()?)
}

/// Where the stored body of object `num` is in `doc`'s file: at its own offset, or at its object
/// stream's. An offset may count from a `%PDF-` header that does not start the file, so both
/// readings are given.
pub(super) fn stored_at(doc: &Document, num: u32) -> Vec<usize> {
    use pdfcraft_cos::XrefEntry;
    let offset = match doc.xref_entry(num) {
        Some(XrefEntry::InStream { stream, .. }) => match doc.xref_entry(stream) {
            Some(XrefEntry::InFile { offset, .. }) => offset,
            _ => return Vec::new(),
        },
        Some(XrefEntry::InFile { offset, .. }) => offset,
        _ => return Vec::new(),
    };
    let Ok(offset) = usize::try_from(offset) else { return Vec::new() };
    [Some(offset), offset.checked_add(doc.header_offset())].into_iter().flatten().collect()
}

/// A change to an existing form widget that is not a signature's, `before` as signed and `d`
/// now. Filling the form changes the field's value (the widget's own `/V`, or the one it inherits
/// from its field, as a radio button or a field with several widgets does), a list box's selected
/// indices with it (`/I`, as a field without widgets of its own may change them too), and the
/// appearance that shows it (`/AS`, `/AP`): that is form fill. Any other change, or a new
/// appearance without a new value, is a change to the annotation (#6756).
fn widget_change(doc: &Document, old: &Document, d: &Dict, before: &Dict) -> &'static str {
    let value = |doc: &Document, d: &Dict| inherited(doc, d, b"V").map(|(_, v)| (*doc.resolve(&v)).clone());
    let filled = !same_entry(value(doc, d).as_ref(), value(old, before).as_ref());
    if filled && same_entries(d, before, &[b"V", b"I", b"AS", b"AP"]) { kind::FORM_FILL } else { kind::COMMENTS }
}

/// The references in the annotation list `o` (inline or indirect), its length spent from
/// `budget`. `None` past the budget.
pub(super) fn annot_refs(doc: &Document, o: Option<&Object>, budget: &Cell<usize>) -> Option<Vec<ObjRef>> {
    let held;
    let items = match o {
        Some(Object::Ref(_)) => {
            held = o.map(|o| doc.resolve(o));
            held.as_deref().and_then(Object::as_array)
        }
        other => other.and_then(Object::as_array),
    };
    let Some(items) = items else { return Some(Vec::new()) };
    budget.set(budget.get().checked_sub(items.len())?);
    Some(items.iter().filter_map(Object::as_ref).collect())
}

/// Every indirect reference in `doc`'s objects and trailer. `None` (nothing can be shown
/// unreferenced) when an object can't be read, or past a bound on references or nesting.
pub(super) fn references(doc: &Document) -> Option<HashSet<ObjRef>> {
    const MAX_REFERENCES: usize = 1 << 22;
    const MAX_DEPTH: u8 = 64;
    fn walk(o: &Object, depth: u8, found: &mut HashSet<ObjRef>) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match o {
            Object::Ref(r) => {
                found.insert(*r);
                found.len() <= MAX_REFERENCES
            }
            Object::Dict(d) => d.iter().all(|(_, v)| walk(v, depth + 1, found)),
            Object::Stream(s) => s.dict.iter().all(|(_, v)| walk(v, depth + 1, found)),
            Object::Array(a) => a.iter().all(|v| walk(v, depth + 1, found)),
            _ => true,
        }
    }
    let mut found = HashSet::new();
    for num in doc.object_numbers() {
        if !walk(&*doc.try_get(num).ok()?, 0, &mut found) {
            return None;
        }
    }
    doc.trailer().iter().all(|(_, v)| walk(v, 0, &mut found)).then_some(found)
}

/// When an entry it follows leads to no object, the renderer rebuilds its object index from the
/// `N G obj` headers in the file, and there the last header for a number wins. A body appended
/// after signing that is not where this reader reads its number can then be drawn in place of
/// the object compared: a new body for a signed page's content that no section lists, or a
/// revision written back to an earlier body (#6754). No writer leaves one, so each is refused,
/// as page content when the number had a signed role (`signed_role`).
fn refuse_unlisted_bodies(doc: &Document, old: &Document, signed_role: &dyn Fn(u32) -> bool, disallowed: &mut Vec<&'static str>) {
    let refused: Vec<&'static str> = match unlisted_bodies(doc, old) {
        Some(nums) => nums.into_iter().map(|num| if signed_role(num) { kind::PAGE_CONTENT } else { kind::OTHER }).collect(),
        None => vec![kind::OTHER],
    };
    for what in refused {
        if !disallowed.contains(&what) {
            disallowed.push(what);
        }
    }
}

/// The object numbers with a body in the bytes appended after the signed revision `old` that
/// `doc` does not read for them: for each number and generation, the last `N G obj` header there
/// must be where `doc`'s cross-reference entry for that number points, with that generation. A
/// renderer that rebuilds its object index from the file's headers takes the last one, so any
/// other body is one it may draw while this reader compares another. `None` past a bound on the
/// headers found, or on the work spent finding them, when the appended bytes cannot be judged.
///
/// The scan finds more headers than a renderer's would (it does not skip dictionaries or check
/// that an object follows), so it errs towards refusing.
fn unlisted_bodies(doc: &Document, old: &Document) -> Option<Vec<u32>> {
    const MAX_HEADERS: usize = 1 << 20;
    let data = doc.bytes();
    let start = old.bytes().len();
    let Some(appended) = data.get(start..) else { return Some(Vec::new()) };
    // Each candidate header may read a comment, a generation token or white space to its end, and
    // later ones can start inside or before it (`%1%1%1…`, `1 +1+1+1…`, `1+1+1…` before long
    // white space): without a bound, that is quadratic in the appended bytes. Ordinary bytes,
    // binary stream data included, spend a small fraction of it.
    let mut work = appended.len().saturating_mul(16).saturating_add(1 << 20);
    let mut last: HashMap<(u32, u16), usize> = HashMap::new();
    let mut i = 0;
    while let Some(b) = appended.get(i) {
        if work == 0 {
            return None;
        }
        let starts_number = b.is_ascii_digit() && i.checked_sub(1).and_then(|p| appended.get(p)).is_none_or(|p| !p.is_ascii_digit());
        if let Some((num, generation, end)) = starts_number.then(|| object_header(appended, i, &mut work)).flatten() {
            last.insert((num, generation), start.saturating_add(i));
            if last.len() > MAX_HEADERS {
                return None;
            }
            i = end;
        } else {
            i += 1;
        }
    }
    // Where an entry's offset leads, past any white space before the header, that white space
    // spent from `work`: many entries can point before one long run of it. Offsets may count from
    // a `%PDF-` header that does not start the file.
    let header_at = |offset: u64, work: &mut usize| -> Vec<usize> {
        let Ok(offset) = usize::try_from(offset) else { return Vec::new() };
        let shifted = (doc.header_offset() != 0).then(|| offset.checked_add(doc.header_offset())).flatten();
        let mut found = Vec::new();
        for at in [Some(offset), shifted].into_iter().flatten() {
            let Some(rest) = data.get(at..).filter(|_| *work > 0) else { continue };
            let blank = rest.iter().take_while(|b| is_white_space(**b)).count();
            *work = work.saturating_sub(blank);
            found.push(at.saturating_add(blank));
        }
        found
    };
    // A revision's own cross-reference stream with no row for itself, as many writers leave it
    // (ISO 32000-2 §7.5.8.3 asks for one; readers do without): the section this reader followed
    // from `startxref` or `/Prev`, so the object there is that stream. Exempt only at exactly
    // where such a section starts, and only under a number that names nothing else: the signed
    // revision has no entry for it, this one lists none, and nothing refers to it. A stream
    // numbered as a signed object, or one a page lists as an annotation, is judged as any body.
    let sections = xref_stream_sections(doc);
    let referenced = OnceCell::new();
    let own_section = |num: u32, at: usize| {
        sections.contains(&at)
            && old.xref_entry(num).is_none()
            && referenced.get_or_init(|| references(doc)).as_ref().is_some_and(|found: &HashSet<ObjRef>| !found.iter().any(|r| r.num == num))
    };
    let mut unlisted: Vec<u32> = last
        .into_iter()
        .filter(|((num, generation), at)| match doc.xref_entry(*num) {
            Some(pdfcraft_cos::XrefEntry::InFile { offset, generation: g }) => g != *generation || !header_at(offset, &mut work).contains(at),
            None => !own_section(*num, *at),
            _ => true,
        })
        .map(|((num, _), _)| num)
        .collect();
    if work == 0 {
        return None;
    }
    unlisted.sort_unstable();
    unlisted.dedup();
    Some(unlisted)
}

/// Where each of `doc`'s cross-reference stream sections starts in the file: the header of the
/// stream object the reader followed from `startxref` or a `/Prev`, past any white space before it.
fn xref_stream_sections(doc: &Document) -> Vec<usize> {
    let data = doc.bytes();
    doc.revisions()
        .iter()
        .filter(|r| r.is_stream)
        .filter_map(|r| {
            let offset = usize::try_from(r.xref_offset).ok()?;
            // An offset may count from a `%PDF-` header that does not start the file.
            let offset = if r.header_relative { offset.checked_add(doc.header_offset())? } else { offset };
            data.get(offset..).map(|rest| offset.saturating_add(rest.iter().take_while(|b| is_white_space(**b)).count()))
        })
        .collect()
}

/// PDF white space (ISO 32000-2 §7.2.3, Table 1).
fn is_white_space(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

/// An `N G obj` header starting at `at` in `bytes`: its number, its generation (0 when it is no
/// generation number, as a renderer rebuilding its index reads it) and where it ends. White space
/// and comments may separate the parts, and the generation may be any number. The white space,
/// comment and generation bytes read are spent from `work`.
fn object_header(bytes: &[u8], at: usize, work: &mut usize) -> Option<(u32, u16, usize)> {
    let number_end = at.checked_add(count_while(bytes, at, |c| c.is_ascii_digit()))?;
    let num = std::str::from_utf8(bytes.get(at..number_end)?).ok()?.parse::<u32>().ok()?;
    let generation_at = skip_space_and_comments(bytes, number_end, work);
    let generation_len = count_while(bytes, generation_at, |c| c.is_ascii_digit() || matches!(c, b'.' | b'+' | b'-'));
    *work = work.saturating_sub(generation_len);
    if generation_len == 0 {
        return None;
    }
    let generation_end = generation_at.checked_add(generation_len)?;
    let generation = std::str::from_utf8(bytes.get(generation_at..generation_end)?).ok().and_then(|g| g.parse::<u16>().ok()).unwrap_or(0);
    let keyword = skip_space_and_comments(bytes, generation_end, work);
    (bytes.get(keyword..keyword.checked_add(3)?)? == b"obj").then_some((num, generation, keyword + 3))
}

/// How many bytes from `from` on `accept` takes.
fn count_while(bytes: &[u8], from: usize, accept: fn(u8) -> bool) -> usize {
    bytes.get(from..).map_or(0, |rest| rest.iter().take_while(|b| accept(**b)).count())
}

/// Where the white space and comments from `from` end, every byte passed spent from `work`: many
/// candidate headers can end in the same long run of white space, as in `1+1+1…` before it.
fn skip_space_and_comments(bytes: &[u8], mut from: usize, work: &mut usize) -> usize {
    while let Some(b) = bytes.get(from) {
        if is_white_space(*b) {
            *work = work.saturating_sub(1);
            from = from.saturating_add(1);
        } else if *b == b'%' {
            let comment = count_while(bytes, from.saturating_add(1), |c| c != b'\n' && c != b'\r').saturating_add(1);
            *work = work.saturating_sub(comment);
            from = from.saturating_add(comment);
        } else {
            break;
        }
    }
    from
}

pub(super) fn refs(doc: &Document, o: Option<&Object>) -> Vec<ObjRef> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
}

/// The interactive form dictionary of a document, resolved.
pub(super) fn acroform(doc: &Document) -> Option<Dict> {
    let c = doc.get(doc.root()?).as_dict()?.get(b"AcroForm").cloned()?;
    doc.resolve(&c).as_dict().cloned()
}

/// An AcroForm change that only adds signature fields that bring new signatures (and sets
/// `/SigFlags`) is signing. Any other field added is a new annotation, not a form fill (#6756).
/// Without `before`, the signed revision's AcroForm (wherever it was) is the reference.
fn acroform_kind(doc: &Document, old: &Document, ends: &RevisionEnds, now: &Dict, before: Option<&Dict>) -> &'static str {
    let fallback = acroform(old).unwrap_or_default();
    let before = before.unwrap_or(&fallback);
    // A field taken out of the form: no DocMDP permission covers dropping a signed field.
    if lost_entries(doc, old, before.get(b"Fields"), now.get(b"Fields")) {
        return kind::OTHER;
    }
    let old_fields = refs(old, before.get(b"Fields"));
    let signs = |r: &ObjRef| doc.get(*r).as_dict().is_some_and(|d| gains_signature(doc, old, ends, r.num, d, None));
    if !refs(doc, now.get(b"Fields")).iter().filter(|r| !old_fields.contains(r)).all(signs) {
        return kind::COMMENTS;
    }
    if !same_entries(now, before, &[b"Fields", b"SigFlags"]) {
        return kind::FORM_FILL;
    }
    kind::SIGNATURE
}

/// Whether two page dictionaries are the same apart from `/Annots`. Dictionaries are compared
/// without regard to key order at every depth: a writer may re-serialise a page's inline
/// resources in another order without changing what the page shows.
fn same_but_annots(d: &Dict, b: &Dict) -> bool {
    same_entries(d, b, &[b"Annots"])
}

/// Whether two dictionaries hold the same entries apart from `skip`: both sorted by key, then
/// compared pairwise, so neither side can hide a key the other lacks.
pub(super) fn same_entries(a: &Dict, b: &Dict, skip: &[&[u8]]) -> bool {
    fn sorted<'a>(x: &'a Dict, skip: &[&[u8]]) -> Vec<(&'a [u8], &'a Object)> {
        let mut v: Vec<(&[u8], &Object)> = x.iter().map(|(k, o)| (k.as_slice(), o)).filter(|(k, _)| !skip.contains(k)).collect();
        v.sort_by(|p, q| p.0.cmp(q.0));
        v
    }
    let (x, y) = (sorted(a, skip), sorted(b, skip));
    x.len() == y.len() && x.iter().zip(&y).all(|((k, v), (l, w))| k == l && same_object(v, w))
}

/// Object equality where dictionaries (and stream dictionaries) ignore key order; arrays keep it.
/// A string is its bytes (ISO 32000-2 §7.3.4.1): written as a literal or as hexadecimal, it is the
/// same string, so a later save that respells one changes nothing it says. Likewise a number is
/// its value (§7.3.3: "wherever a real number is expected, an integer may be used instead"), so a
/// signed `/Rect [20.0 20.0 120.0 40.0]` saved again as `[20 20 120 40]` is the same rectangle.
/// References compare by object and generation, and stream data byte for byte as stored. The
/// recursion follows the parsed nesting, which the parser bounds.
pub(super) fn same_object(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::String(a), Object::String(b)) => a.bytes == b.bytes,
        // Compared in i128, where every i64 and every whole real near its range is exact (the cast
        // saturates far beyond it): no fractional real, NaN or infinity (whose fract is NaN)
        // equals an integer, nor does the real 2^63 pass for i64::MAX.
        (Object::Int(i), Object::Real(r)) | (Object::Real(r), Object::Int(i)) => r.fract() == 0.0 && *r as i128 == i128::from(*i),
        (Object::Dict(a), Object::Dict(b)) => same_entries(a, b, &[]),
        (Object::Stream(a), Object::Stream(b)) => a.raw == b.raw && same_entries(&a.dict, &b.dict, &[]),
        (Object::Array(a), Object::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(v, w)| same_object(v, w)),
        _ => a == b,
    }
}

/// [`same_object`] for an entry that may be absent: both absent, or both present and the same.
pub(super) fn same_entry(a: Option<&Object>, b: Option<&Object>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same_object(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The objects the signed pages draw from, by reference, with the role each held when signed
/// (#1787): `true` for a container (a page's resources dictionary, or one of its category
/// dictionaries), where a name added later can't be reached by unchanged content; `false` for
/// everything reached from a container's entries (fonts and their files, XObjects and their own
/// resources, graphics states and soft masks, colour spaces, patterns, shadings, optional-content
/// properties), where any change changes what the page shows. Resources are the page's own or
/// inherited. Bounded by depth and by the number of objects visited; past the bound the rest is
/// judged as before.
fn signed_page_resources(old: &Document, pages: &[ObjRef]) -> HashMap<u32, bool> {
    const CATEGORIES: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];
    const MAX_VISITED: usize = 50_000;
    const MAX_DEPTH: u8 = 32;
    const MAX_INHERITANCE: usize = 64;

    /// Everything reachable from `o` is content the page draws (a leaf role, overriding a
    /// container one reached by another path, under any generation alias). A signed page or the
    /// catalog reached this way (a `/P` or `/Parent` back-reference) keeps the role of its own:
    /// the walk stops there.
    fn reach(old: &Document, o: &Object, depth: u8, stop: &HashSet<u32>, roles: &mut HashMap<u32, bool>) {
        if roles.len() >= MAX_VISITED || depth > MAX_DEPTH {
            return;
        }
        let resolved;
        let o = match o {
            Object::Ref(r) => {
                if stop.contains(&r.num) || roles.insert(r.num, false).is_some() {
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
    let mut stop: HashSet<u32> = pages.iter().map(|r| r.num).collect();
    stop.extend(old.root().map(|r| r.num));
    for page in pages {
        let Some(res) = resources(old, *page) else { continue };
        let container = |o: &Object, roles: &mut HashMap<u32, bool>| -> Arc<Object> {
            match o {
                Object::Ref(r) => {
                    roles.entry(r.num).or_insert(true);
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

/// Whether the catalog `d` keeps every developer extension the signed catalog `b` declared, each
/// with the same value: it may only declare more.
fn extensions_kept(doc: &Document, old: &Document, d: &Dict, b: &Dict) -> bool {
    let now = d.get(b"Extensions").map(|e| doc.resolve(e)).and_then(|e| e.as_dict().cloned());
    let was = b.get(b"Extensions").map(|e| old.resolve(e)).and_then(|e| e.as_dict().cloned());
    match (was, now) {
        (None, _) => true,
        (Some(was), Some(now)) => was.iter().all(|(k, v)| same_entry(now.get(k), Some(v))),
        (Some(_), None) => false,
    }
}

/// A change to the signed catalog `b`, now `d`.
fn catalog_kind(doc: &Document, old: &Document, ends: &RevisionEnds, d: &Dict, b: &Dict) -> &'static str {
    let keys: &[&[u8]] = &[b"AcroForm", b"DSS", b"Perms", b"Metadata", b"NeedsRendering", b"Extensions"];
    if !same_entries(d, b, keys) {
        kind::STRUCTURE
    } else if !extensions_kept(doc, old, d, b) {
        // Developer extensions (ISO 32000-2 §7.12) only declare what the file uses. A writer
        // adding a /DSS also declares the ESIC extension it belongs to, so a declaration may be
        // added, but none that was signed may change or go.
        kind::STRUCTURE
    } else if !same_entry(d.get(b"AcroForm"), b.get(b"AcroForm")) {
        // Compare the forms themselves, inline or not.
        let now = acroform(doc).unwrap_or_default();
        acroform_kind(doc, old, ends, &now, None)
    } else if !same_entry(b.get(b"Perms"), d.get(b"Perms")) {
        kind::SIGNATURE
    } else {
        kind::METADATA
    }
}

/// The nodes of `doc`'s page tree and the /Kids arrays that are objects of their own, by number:
/// every dictionary with /Kids from the catalog's /Pages down. Each node, and each /Kids array
/// object, is read once. `None` past a million nodes.
fn page_tree_nodes(doc: &Document) -> Option<(HashSet<u32>, HashSet<u32>)> {
    const MAX_NODES: usize = 1_000_000;
    let (mut nodes, mut arrays) = (HashSet::new(), HashSet::new());
    let top = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages").and_then(Object::as_ref)));
    let mut stack: Vec<ObjRef> = top.into_iter().collect();
    while let Some(r) = stack.pop() {
        if nodes.len() >= MAX_NODES {
            return None;
        }
        let node = doc.get(r);
        // A leaf (a page) has no /Kids.
        let Some(kids) = node.as_dict().and_then(|d| d.get(b"Kids")) else { continue };
        if !nodes.insert(r.num) || kids.as_ref().is_some_and(|a| !arrays.insert(a.num)) {
            continue;
        }
        if let Some(kids) = doc.resolve(kids).as_array() {
            stack.extend(kids.iter().filter_map(Object::as_ref));
        }
    }
    Some((nodes, arrays))
}

/// Whether a list of references (`/Fields`) no longer holds an entry it held when signed. An
/// entry is still held only by a reference that resolves now and that the signed revision
/// resolved too, so to the same slot (a repaired alias counts): one rewritten to a generation
/// the slot does not accept names nothing, and one to a newer generation names another object.
pub(super) fn lost_entries(doc: &Document, old: &Document, was: Option<&Object>, now: Option<&Object>) -> bool {
    let live = |d: &Document, r: &ObjRef| !matches!(*d.get(*r), Object::Null);
    let now: HashSet<u32> = refs(doc, now).into_iter().filter(|r| live(doc, r) && live(old, r)).map(|r| r.num).collect();
    refs(old, was).iter().filter(|r| live(old, r)).any(|r| !now.contains(&r.num))
}

/// What changing the dictionary `b` (as signed) into `d` amounts to, judged by the role the
/// object had when signed: signing, form fill, comments, page content or another change. A new
/// object has no role yet, and is not labelled: an operation claims it or it is refused
/// ([`super::operations`]).
fn change_kind(doc: &Document, old: &Document, ends: &RevisionEnds, num: u32, d: &Dict, b: &Dict) -> &'static str {
    // A signature widget that already held a signature is what the signature shows: moving it or
    // swapping its appearance can hide the signed page under it. Judged by the signed revision,
    // so relabelling the widget now does not change that.
    if b.name(b"Subtype") == Some(b"Widget")
        && b.name(b"FT") == Some(b"Sig")
        && b.get(b"V").is_some_and(|v| old.resolve(v).as_dict().is_some())
        && !same_entries(d, b, &[])
    {
        return kind::PAGE_CONTENT;
    }
    // A signed form field or widget keeps that role: its value, appearance or signature may
    // change, but turning it into a comment, another kind of annotation or another type of
    // field is no form fill.
    if (b.name(b"Subtype") == Some(b"Widget") || b.contains(b"FT")) && (d.name(b"Subtype") != b.name(b"Subtype") || d.name(b"FT") != b.name(b"FT")) {
        return kind::OTHER;
    }
    // Where a signed widget sits and whether it shows are what the page shows: filling the form
    // changes its value and appearance, never its rectangle or its flags.
    if b.name(b"Subtype") == Some(b"Widget") && (!same_entry(d.get(b"Rect"), b.get(b"Rect")) || !same_entry(d.get(b"F"), b.get(b"F"))) {
        return kind::PAGE_CONTENT;
    }
    match d.name(b"Type") {
        Some(b"Catalog") => return catalog_kind(doc, old, ends, d, b),
        Some(b"Page") if !same_but_annots(d, b) => return kind::PAGE_CONTENT,
        Some(b"Page") => {
            let was = refs(old, b.get(b"Annots"));
            let added: Vec<ObjRef> = refs(doc, d.get(b"Annots")).into_iter().filter(|r| !was.contains(r)).collect();
            return added_annots(doc, old, ends, &added);
        }
        Some(b"Pages") => return kind::PAGES,
        _ => {}
    }
    // The role an existing object plays (annotation, form field, the form) is the one it had when
    // signed: keys its new version carries are written by whoever changed it. An indirect
    // /OCProperties rewritten with an added /FT still decides which layers show (#1787 judges a
    // signed page's resources this way; this applies it to every other object).
    // Nor does an existing annotation or widget turned into another kind of annotation edit the
    // one it was: an empty signature widget rewritten as a /Square that covers signed text is no
    // signature change, and no comment either.
    if b.name(b"Subtype") != d.name(b"Subtype") {
        return kind::OTHER;
    }
    match b.name(b"Subtype") {
        // A signature field's widget, when signed and still: signing only when its field gains a
        // signature. A restyled empty one signs nothing (#6755).
        Some(b"Widget") if is_signature_field(old, b) => {
            return if is_signature_field(doc, d) && gains_signature(doc, old, ends, num, d, Some(b)) { kind::SIGNATURE } else { kind::COMMENTS };
        }
        Some(b"Widget") => return widget_change(doc, old, d, b),
        Some(b"Link") => return kind::LINKS,
        Some(_) if b.name(b"Type") == Some(b"Annot") || b.contains(b"Rect") => return kind::COMMENTS,
        _ => {}
    }
    // A later revision cannot make an existing object a signature by relabelling it: only one
    // that was a signature field when signed is judged as one.
    if b.name(b"FT") == Some(b"Sig") {
        return kind::SIGNATURE;
    }
    if b.contains(b"Fields") {
        return acroform_kind(doc, old, ends, d, Some(b));
    }
    if b.contains(b"FT") || (b.contains(b"T") && b.contains(b"Kids")) || b.contains(b"DR") {
        return kind::FORM_FILL;
    }
    kind::OTHER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A signed page's change, judged by [`ChangedObjectClassifier::page_change_kind`] with no
    /// other signed roles in play; `None` when nothing the page shows changed.
    fn page_change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> Option<&'static str> {
        let snapshot = Snapshot {
            old_content: HashSet::new(),
            old_metadata: None,
            old_xfa_data: HashSet::new(),
            signed_pages: HashSet::new(),
            signed_annots: HashSet::new(),
            placed_annots: HashSet::new(),
            annots_unbounded: false,
            page_nodes: HashSet::new(),
            kids_arrays: HashSet::new(),
            page_tree_unbounded: false,
            signed_resources: HashMap::new(),
            old_info: None,
            old_form: None,
            annot_budget: Cell::new(MAX_ANNOT_ENTRIES),
        };
        let (none, ends) = (HashSet::new(), RevisionEnds::new());
        let classifier = ChangedObjectClassifier { doc, old, snapshot: &snapshot, dss: &none, ends: &ends };
        classifier.page_change_kind(d, before)
    }

    fn font(order: &[&str], base: &str) -> Object {
        let mut d = Dict::new();
        for k in order {
            match *k {
                "Type" => d.set(b"Type".to_vec(), Object::name("Font")),
                "Subtype" => d.set(b"Subtype".to_vec(), Object::name("Type1")),
                "BaseFont" => d.set(b"BaseFont".to_vec(), Object::name(base)),
                _ => {
                    let mut w = Dict::new();
                    w.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(32), Object::name("space")]));
                    w.set(b"Type".to_vec(), Object::name("Encoding"));
                    d.set(b"Encoding".to_vec(), Object::Dict(w));
                }
            }
        }
        Object::Dict(d)
    }

    fn page(fonts: Vec<(&str, Object)>, annots: Vec<Object>) -> Dict {
        let mut f = Dict::new();
        fonts.into_iter().for_each(|(k, v)| f.set(k.as_bytes().to_vec(), v));
        let mut r = Dict::new();
        r.set(b"Font".to_vec(), Object::Dict(f));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Resources".to_vec(), Object::Dict(r));
        p.set(b"Annots".to_vec(), Object::Array(annots));
        p
    }

    #[test]
    fn reordered_inline_resources_are_not_page_content() {
        let doc = Document::new_empty();
        let order = ["Type", "Subtype", "BaseFont", "Encoding"];
        let before = page(vec![("F1", font(&order, "Times-Roman")), ("F2", font(&order, "Helvetica"))], vec![]);
        let unchanged = page_change_kind(&doc, &doc, &before, Some(&before));
        let reordered = page(
            vec![
                ("F2", font(&["Encoding", "BaseFont", "Type", "Subtype"], "Helvetica")),
                ("F1", font(&["BaseFont", "Subtype", "Encoding", "Type"], "Times-Roman")),
            ],
            vec![],
        );
        assert_eq!(page_change_kind(&doc, &doc, &reordered, Some(&before)), unchanged);
        assert_eq!(change_kind(&doc, &doc, &OnceCell::new(), 0, &reordered, &before), change_kind(&doc, &doc, &OnceCell::new(), 0, &before, &before));

        // A changed value inside the nested dictionary still is a page content change.
        let changed = page(vec![("F2", font(&order, "Helvetica")), ("F1", font(&order, "Courier"))], vec![]);
        assert_eq!(page_change_kind(&doc, &doc, &changed, Some(&before)), Some(kind::PAGE_CONTENT));
        assert_eq!(change_kind(&doc, &doc, &OnceCell::new(), 0, &changed, &before), kind::PAGE_CONTENT);
    }

    /// The page as signed and the same page with its inline resources reordered are each no
    /// page change at all, stated as `None` rather than only equal to each other, and so is a page
    /// whose numbers and strings are respelled as well (§7.3.3, §7.3.4.1).
    #[test]
    fn reordered_and_respelled_pages_are_no_page_change() {
        let doc = Document::new_empty();
        let order = ["Type", "Subtype", "BaseFont", "Encoding"];
        let mut before = page(vec![("F1", font(&order, "Times-Roman")), ("F2", font(&order, "Helvetica"))], vec![]);
        before.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(612.0), Object::Real(792.0)]));
        before.set(
            b"PieceInfo".to_vec(),
            Object::Dict(Dict::from_iter([(b"Note".to_vec(), Object::String(pdfcraft_cos::PdfString::literal("signed")))])),
        );
        let mut reordered = page(
            vec![
                ("F2", font(&["Encoding", "BaseFont", "Type", "Subtype"], "Helvetica")),
                ("F1", font(&["BaseFont", "Subtype", "Encoding", "Type"], "Times-Roman")),
            ],
            vec![],
        );
        reordered.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Real(0.0), Object::Int(0), Object::Int(612), Object::Int(792)]));
        reordered.set(
            b"PieceInfo".to_vec(),
            Object::Dict(Dict::from_iter([(b"Note".to_vec(), Object::String(pdfcraft_cos::PdfString { bytes: b"signed".to_vec(), hex: true }))])),
        );
        assert_eq!(page_change_kind(&doc, &doc, &before, Some(&before)), None);
        assert_eq!(page_change_kind(&doc, &doc, &reordered, Some(&before)), None);
        assert_eq!(page_change_kind(&doc, &doc, &before, Some(&reordered)), None);

        // A real that is not the integer it replaced is a moved box.
        let mut moved = reordered.clone();
        moved.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(612.5), Object::Int(792)]));
        assert_eq!(page_change_kind(&doc, &doc, &moved, Some(&before)), Some(kind::PAGE_CONTENT));
    }

    /// Numbers compare by value across integer and real, and only when the values are equal.
    #[test]
    fn integers_and_reals_compare_by_value() {
        let same = |a: Object, b: Object| same_object(&a, &b) && same_object(&b, &a);
        assert!(same(Object::Int(20), Object::Real(20.0)));
        assert!(same(Object::Int(0), Object::Real(-0.0)));
        assert!(same(Object::Int(-7), Object::Real(-7.0)));
        assert!(!same(Object::Int(20), Object::Real(20.5)));
        assert!(!same(Object::Int(20), Object::Real(f64::NAN)));
        assert!(!same(Object::Int(i64::MAX), Object::Real(f64::INFINITY)));
        assert!(!same(Object::Int(i64::MAX), Object::Real(9_223_372_036_854_775_808.0)));
        assert!(!same(Object::Int(1), Object::Bool(true)));
    }

    #[test]
    fn a_swapped_nested_key_is_page_content_both_ways() {
        let doc = Document::new_empty();
        let order = ["Type", "Subtype", "BaseFont"];
        let signed = page(vec![("F1", font(&order, "Times-Roman"))], vec![]);
        // Same entry count, but /Subtype was dropped and /Encoding added in its place.
        let swapped = page(vec![("F1", font(&["Type", "Encoding", "BaseFont"], "Times-Roman"))], vec![]);
        for (now, then) in [(&swapped, &signed), (&signed, &swapped)] {
            assert_eq!(page_change_kind(&doc, &doc, now, Some(then)), Some(kind::PAGE_CONTENT));
            assert_eq!(change_kind(&doc, &doc, &OnceCell::new(), 0, now, then), kind::PAGE_CONTENT);
        }
    }

    #[test]
    fn reordered_arrays_are_still_page_content() {
        let doc = Document::new_empty();
        let mut before = page(vec![], vec![]);
        before.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(612), Object::Int(792)]));
        let mut after = before.clone();
        after.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(792), Object::Int(612)]));
        assert_eq!(page_change_kind(&doc, &doc, &after, Some(&before)), Some(kind::PAGE_CONTENT));
    }

    /// A PDF of `objs` (numbered from 1) whose trailer also holds `trailer_extra`.
    fn pdf(objs: &[&str], trailer_extra: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A P=1 signed revision whose catalog `/Perms` and trailer `/Info` are one indirect
    /// dictionary (object 3). A later revision points its `/DocMDP` at another signature
    /// dictionary already in the file, in place: the reference stays, no object is added, and
    /// as the signed `/Info` the object reads as metadata, but the certification's own record
    /// has changed.
    #[test]
    fn a_perms_dictionary_shared_with_info_rewritten_in_place_is_a_structure_change() {
        let signed = pdf(
            &[
                "<< /Type /Catalog /Pages 2 0 R /Perms 3 0 R >>",
                "<< /Type /Pages /Kids [] /Count 0 >>",
                "<< /DocMDP 4 0 R >>",
                "<< /Type /Sig >>",
                "<< /Type /Sig >>",
            ],
            "/Info 3 0 R",
        );
        let old = Document::open(Arc::new(signed.clone())).unwrap();
        let mut doc = Document::open(Arc::new(signed)).unwrap();
        let other = ObjRef { num: 5, generation: 0 };
        doc.update_dict(ObjRef { num: 3, generation: 0 }, |p| p.set(b"DocMDP".to_vec(), Object::Ref(other))).unwrap();
        let edited = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions { object_streams: false, ..Default::default() }).unwrap();
        let doc = Document::open(Arc::new(edited)).unwrap();
        match super::super::operations::judge_changes(&doc, Some(old), Some(1), &super::super::locks::LockScope::none()) {
            super::super::Modification::Disallowed(kinds) => assert!(kinds.contains(&kind::STRUCTURE), "{kinds:?}"),
            other => panic!("the certification's /Perms changed in place under P=1: {other:?}"),
        }
    }

    /// `signed` with a revision appended that writes `before` and then object 3, listed where its
    /// header is.
    fn append_object_3(signed: &[u8], before: &[u8]) -> Vec<u8> {
        let at = signed.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
        let prev: String =
            signed.get(at..).unwrap().iter().skip_while(|b| b.is_ascii_whitespace()).take_while(|b| b.is_ascii_digit()).map(|b| *b as char).collect();
        let mut bytes = signed.to_vec();
        bytes.extend_from_slice(before);
        let object = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n(x)\nendobj\n");
        let section = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n3 1\n{object:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
        );
        bytes
    }

    /// A comment of digits (`%1%1%1…`), a run of signed numbers (`1 +1+1+1…`), or a short one
    /// before long white space, appended after signing, would make the header scan start at each
    /// digit and read to the end of the run every time, quadratic in the appended bytes. Past a
    /// bound on that work the appended bytes are not judged; an ordinary revision is.
    #[test]
    fn the_header_scan_is_bounded_on_runs_that_restart_it() {
        let signed = pdf(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "");
        let old = Document::open(Arc::new(signed.clone())).unwrap();
        let comment = [b"%".to_vec(), b"1%".repeat(50_000), b"\n".to_vec()].concat();
        let numbers = [b"1 ".to_vec(), b"+1".repeat(50_000), b"\n".to_vec()].concat();
        // Each start reads the short run of numbers, then the same long white space after it.
        let spaced = [b"1".to_vec(), b"+1".repeat(2_000), b" ".repeat(1 << 20), b"\n".to_vec()].concat();
        for (what, run) in [("a comment of digits", comment), ("a run of signed numbers", numbers), ("signed numbers before white space", spaced)] {
            let doc = Document::open(Arc::new(append_object_3(&signed, &run))).unwrap();
            assert_eq!(unlisted_bodies(&doc, &old), None, "{what}");
        }
        let doc = Document::open(Arc::new(append_object_3(&signed, b"% an ordinary comment 1 2 3\n"))).unwrap();
        assert_eq!(unlisted_bodies(&doc, &old), Some(Vec::new()));
    }

    /// Many entries of an appended section that point at one place before a long run of white
    /// space would make matching each header to its entry read that run again every time. Past
    /// the same bound on work, the appended bytes are not judged.
    #[test]
    fn matching_headers_to_their_entries_is_bounded_too() {
        let signed = pdf(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "");
        let old = Document::open(Arc::new(signed.clone())).unwrap();
        let at = signed.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
        let prev: String =
            signed.get(at..).unwrap().iter().skip_while(|b| b.is_ascii_whitespace()).take_while(|b| b.is_ascii_digit()).map(|b| *b as char).collect();
        let mut bytes = signed.clone();
        let blank = bytes.len();
        bytes.extend_from_slice(&b" ".repeat(1 << 20));
        bytes.push(b'\n');
        let count = 2_000u32;
        for num in 3..3 + count {
            bytes.extend_from_slice(format!("{num} 0 obj\n(x)\nendobj\n").as_bytes());
        }
        let section = bytes.len();
        bytes.extend_from_slice(format!("xref\n3 {count}\n").as_bytes());
        for _ in 0..count {
            bytes.extend_from_slice(format!("{blank:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n", 3 + count).as_bytes());
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(unlisted_bodies(&doc, &old), None);
    }
}
