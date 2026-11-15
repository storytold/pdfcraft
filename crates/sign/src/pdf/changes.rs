use super::types::{Modification, kind};
use super::{Arc, Dict, Document, HashMap, HashSet, ObjRef, Object};
use std::cell::{Cell, OnceCell};

/// The most annotation-list entries one classification expands. Pages sharing one long /Annots
/// array, or many pages rewritten to list one, would otherwise cost pages × entries; past this
/// the lists are not compared, and the change is refused.
const MAX_ANNOT_ENTRIES: usize = 1 << 22;

/// What later revisions changed, classified as Acrobat reports it, under DocMDP `p` (or none:
/// an approval signature permits form fill, comments and further signatures).
pub(super) fn classify_changes(doc: &Document, old: Option<Document>, p: Option<u8>) -> Modification {
    let Some(old) = old else {
        return Modification::Disallowed(vec![kind::UNREADABLE]);
    };
    // Every role below is keyed by object number, not by the reference as written: a damaged
    // file's slot accepts two generations (#1143), and a page signed as `3 5 R` and edited as
    // `3 0 R` is still that page. The loop over changed objects refuses any slot whose accepted
    // generations changed, so a number names the same slot in both revisions.
    let old_content: HashSet<u32> = pdfcraft_annot::page_refs(&old)
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
    // The signed revision's document-level XMP stream.
    let old_metadata = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"Metadata").and_then(Object::as_ref))).map(|r| r.num);
    // The signed revision's XFA data packets: filling the form rewrites them.
    let old_xfa_data = xfa_data_packets(&old);
    // What each object was in the signed revision: its pages, and its trailer /Info. A changed
    // object is judged by the role it held when signed, never by the labels or keys its new,
    // attacker-written version carries (relabelling a signed page /Type /Sig, or dropping its
    // /Type and adding /ModDate, must not reclassify it away from page content).
    let signed_page_refs = pdfcraft_annot::page_refs(&old).unwrap_or_default();
    let signed_pages: HashSet<u32> = signed_page_refs.iter().map(|r| r.num).collect();
    // The signed pages' /Annots arrays that are objects of their own. Adding an annotation updates
    // such an array in place (the page dictionary does not change), so it is judged like a
    // page's inline /Annots: by the annotations it gained.
    let signed_annots: HashSet<u32> = signed_page_refs
        .iter()
        .filter_map(|pg| old.get(*pg).as_dict().and_then(|d| d.get(b"Annots").and_then(Object::as_ref)))
        .map(|r| r.num)
        .collect();
    // The annotations the signed pages listed. One of them listed again elsewhere was moved or
    // copied there, not added.
    let annot_budget = Cell::new(MAX_ANNOT_ENTRIES);
    let (mut placed_annots, mut expanded, mut annots_unbounded) = (HashSet::new(), HashSet::new(), false);
    for pg in &signed_page_refs {
        let page = old.get(*pg);
        let Some(list) = page.as_dict().and_then(|d| d.get(b"Annots")) else { continue };
        // Pages sharing one array object list the same annotations.
        if list.as_ref().is_some_and(|r| !expanded.insert(r.num)) {
            continue;
        }
        let Some(found) = annot_refs(&old, Some(list), &annot_budget) else {
            annots_unbounded = true;
            break;
        };
        placed_annots.extend(found.into_iter().map(|r| r.num));
    }
    // The signed page tree's nodes: a change to one changes the pages, whatever it is labelled
    // now. Every node, and every /Kids array object of its own, is read once; past the bound the
    // tree is not known in full, and no change is judged against it.
    let page_tree = page_tree_nodes(&old);
    let page_tree_unbounded = page_tree.is_none();
    let (page_nodes, kids_arrays) = page_tree.unwrap_or_default();
    // What each signed page draws from, by the role it held when signed (#1787).
    let signed_resources = signed_page_resources(&old, &signed_page_refs);
    let old_info = old.trailer().get(b"Info").and_then(Object::as_ref).map(|r| r.num);
    // The signed catalog's /AcroForm, when it is an object of its own.
    let old_form = old.root().and_then(|c| old.get(c).as_dict().and_then(|c| c.get(b"AcroForm").and_then(Object::as_ref))).map(|r| r.num);
    let signed_role = |num: u32| old_content.contains(&num) || signed_pages.contains(&num) || signed_resources.contains_key(&num);
    let (mut allowed, mut disallowed): (Vec<&'static str>, Vec<&'static str>) = (Vec::new(), Vec::new());
    if annots_unbounded {
        disallowed.push(kind::OTHER);
    }
    if page_tree_unbounded {
        // Past the bound the tree is not known in full: no change is judged against it.
        disallowed.push(kind::PAGES);
    }
    // Every reference the signed revision makes, read only when the generations a slot accepts
    // changed.
    let signed_references = OnceCell::new();
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
        let what = if old_content.contains(num) || signed_pages.contains(num) { kind::PAGE_CONTENT } else { kind::OTHER };
        if !disallowed.contains(&what) {
            disallowed.push(what);
        }
    }
    // The trailer chooses the catalog. A later revision that points it at another catalog (one
    // already in the signed file, so no object changes) shows another document.
    if doc.root() != old.root() {
        disallowed.push(kind::STRUCTURE);
    }
    let classifier = ChangedObjectClassifier {
        doc,
        old: &old,
        old_content: &old_content,
        old_metadata,
        old_xfa_data: &old_xfa_data,
        signed_pages: &signed_pages,
        signed_annots: &signed_annots,
        page_nodes: &page_nodes,
        kids_arrays: &kids_arrays,
        old_form,
        placed_annots: &placed_annots,
        annot_budget: &annot_budget,
        signed_resources: &signed_resources,
        old_info,
        dss: &dss,
    };
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
        // it gave up. A member of an object stream accepts every generation, and the same object
        // written on its own (a form fill) only its own, or the other way round: the same slot
        // when every signed reference to it resolves, or fails to, as it did.
        let signed_generations = old.accepted_generations(num);
        let generations = doc.accepted_generations(num);
        let (signed_any, any) = (old.accepts_any_generation(num), doc.accepts_any_generation(num));
        let same_for_signed_references = || {
            !generations.is_empty()
                && generations.iter().all(|g| signed_any || signed_generations.contains(g))
                && signed_references.get_or_init(|| references(&old)).as_ref().is_some_and(|found: &HashMap<u32, HashSet<u16>>| {
                    found
                        .get(&num)
                        .is_none_or(|used| used.iter().all(|g| (signed_any || signed_generations.contains(g)) == (any || generations.contains(g))))
                })
        };
        if !signed_generations.is_empty() && (signed_generations != generations || signed_any != any) && !same_for_signed_references() {
            let what = if signed_role(num) { kind::PAGE_CONTENT } else { kind::OTHER };
            if !disallowed.contains(&what) {
                disallowed.push(what);
            }
            continue;
        }
        let r = ObjRef { num, generation: doc.generation(num) };
        let new = doc.get(r);
        let before = old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null));
        if before.as_deref().is_some_and(|b| same_object(b, &new)) {
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
        let Some(what) = classifier.classify(r, &new, before.as_deref()) else { continue };
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
        let was = old.get(ObjRef { num, generation: old.generation(num) });
        let signed_role = signed_role(num);
        // A signed page's content or resource is judged by that role, whatever /Type its stream carried.
        if !signed_role && matches!(&*was, Object::Stream(s) if matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm"))) {
            continue;
        }
        let what = if signed_role { kind::PAGE_CONTENT } else { kind::OTHER };
        if !disallowed.contains(&what) {
            disallowed.push(what);
        }
    }
    // DocMDP P=1 ("no changes allowed", ISO 32000-2 §12.8.2.2) permits only validation data to
    // follow: the document security store, and document timestamps added for PAdES long-term
    // archiving (§12.8.5). Those timestamps draw nothing. Any other signature added since — an
    // approval signature, an unsigned signature widget, or a widget that paints over the signed
    // page — is a change the certification forbids, however it is labelled.
    if p == Some(1) {
        only_validation_data(&mut allowed, &mut disallowed, doc, &old);
    }
    if disallowed.is_empty() { Modification::Allowed(allowed) } else { Modification::Disallowed(disallowed) }
}

/// DocMDP P=1 ("no changes allowed") permits only validation data to follow the signed revision:
/// the security store, and document timestamps that draw nothing. A later approval signature, a
/// visible widget or a changed catalog `/Perms` breaks the certification.
fn only_validation_data(allowed: &mut Vec<&'static str>, disallowed: &mut Vec<&'static str>, doc: &Document, old: &Document) {
    if allowed.contains(&kind::SIGNATURE) && !only_invisible_timestamps_added(doc, old) {
        allowed.retain(|k| *k != kind::SIGNATURE);
        if !disallowed.contains(&kind::SIGNATURE) {
            disallowed.push(kind::SIGNATURE);
        }
    }
    // The catalog's /Perms names the certification signature itself. Elsewhere a change to it is
    // judged as signing; under P=1 dropping or redirecting it is a change like any other.
    // Compared by content: an indirect /Perms rewritten in place keeps its reference (and, shared
    // with the trailer's /Info, would otherwise pass as a metadata change).
    let perms = |d: &Document| d.root().and_then(|r| d.get(r).as_dict().and_then(|c| c.get(b"Perms").map(|p| (*d.resolve(p)).clone())));
    if !same_entry(perms(doc).as_ref(), perms(old).as_ref()) && !disallowed.contains(&kind::STRUCTURE) {
        disallowed.push(kind::STRUCTURE);
    }
}

/// Whether every signature object `doc` gained or changed since the signed revision `old` is an
/// invisible document timestamp. A signature annotation (a widget, or a dictionary carrying a
/// signature value or role) that the signed revision did not hold byte for byte must be a
/// `/SubFilter /ETSI.RFC3161` value in a widget that shows nothing — no positive-area `/Rect`,
/// or the Hidden or NoView flag set. An approval signature, an empty signature widget, or a
/// visible one fails, whatever `/Type` or `/SubFilter` it claims. Judged on the objects
/// themselves, not on the `/AcroForm /Fields` listing, so a widget with no `/T` or one reached
/// only through a page's `/Annots` is covered too.
fn only_invisible_timestamps_added(doc: &Document, old: &Document) -> bool {
    /// A widget shows nothing: no `/Rect` enclosing a positive area, or Hidden (2) / NoView (32).
    fn invisible(doc: &Document, w: &Dict) -> bool {
        let flags = w.get(b"F").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0);
        let area =
            super::listing::nums(doc, w, b"Rect").filter(|r| r.len() == 4).is_some_and(|r| (r[0] - r[2]).abs() > 0.5 && (r[1] - r[3]).abs() > 0.5);
        !area || flags & (2 | 32) != 0
    }
    for num in doc.object_numbers() {
        let new = doc.get(ObjRef { num, generation: doc.generation(num) });
        let Some(d) = new.as_dict() else { continue };
        // A signature object: a signature value, or a field or widget that carries one.
        let is_value = matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp"));
        let is_field = d.name(b"FT") == Some(b"Sig") || d.name(b"Subtype") == Some(b"Widget");
        if !is_value && !is_field {
            continue;
        }
        // Unchanged since signing: the certification already covered it.
        if old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null)).is_some_and(|o| same_object(&o, &new)) {
            continue;
        }
        let value = if is_value { Some(d.clone()) } else { d.get(b"V").map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned()) };
        // A new or changed signature object passes only as an invisible document timestamp. A
        // widget with no value (an empty signature field placed on the page) never does.
        let timestamp = value.as_ref().is_some_and(super::validate::is_doc_timestamp);
        if !timestamp || (is_field && !invisible(doc, d)) {
            return false;
        }
    }
    true
}

struct ChangedObjectClassifier<'a> {
    doc: &'a Document,
    old: &'a Document,
    old_content: &'a HashSet<u32>,
    old_metadata: Option<u32>,
    old_xfa_data: &'a HashSet<u32>,
    signed_pages: &'a HashSet<u32>,
    signed_annots: &'a HashSet<u32>,
    page_nodes: &'a HashSet<u32>,
    kids_arrays: &'a HashSet<u32>,
    placed_annots: &'a HashSet<u32>,
    annot_budget: &'a Cell<usize>,
    signed_resources: &'a HashMap<u32, bool>,
    old_info: Option<u32>,
    old_form: Option<u32>,
    dss: &'a HashSet<u32>,
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
            Object::Array(_) if before.is_some() && self.kids_arrays.contains(&r.num) => Some(kind::PAGES),
            Object::Array(_) if self.is_signed_annotation_array(r, before) => self.annotation_array_change(new, before),
            Object::Dict(dict) => self.dictionary_change(r, dict, before),
            Object::Stream(stream) => self.stream_change(r, stream, before),
            _ if before.is_none() => None,
            _ => Some(kind::OTHER),
        }
    }

    fn resource_kind(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (self.signed_resources.get(&r.num), new) {
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
        self.signed_annots.contains(&r.num) && before.is_some_and(|object| object.as_array().is_some())
    }

    fn annotation_array_change(&self, now: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (annot_refs(self.old, before, self.annot_budget), annot_refs(self.doc, Some(now), self.annot_budget)) {
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
        let moved: Vec<u32> = added.iter().map(|r| r.num).filter(|num| self.placed_annots.contains(num)).collect();
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
            Some(added_annots(self.doc, &added))
        }
    }

    fn dictionary_change(&self, r: ObjRef, dict: &Dict, before: Option<&Object>) -> Option<&'static str> {
        let previous = before.and_then(Object::as_dict);
        if self.signed_pages.contains(&r.num) {
            // A signed page is judged by what the page shows, whatever its new labels say.
            self.page_change_kind(dict, previous)
        } else if previous.is_some() && self.page_nodes.contains(&r.num) {
            // A signed page-tree node is a page-tree change, whatever its new version is labelled.
            Some(kind::PAGES)
        } else if let Some(previous) = previous.filter(|_| self.old.root().is_some_and(|root| root.num == r.num)) {
            // The signed catalog, whatever /Type its new version carries.
            Some(catalog_kind(self.doc, self.old, dict, previous))
        } else if let Some(previous) = previous.filter(|_| self.old_form == Some(r.num)) {
            // The signed form dictionary, held in an object of its own, whatever keys it carries now.
            Some(acroform_kind(self.doc, self.old, dict, Some(previous)))
        } else if previous.is_some() && Some(r.num) == self.old_info {
            // Only the signed revision's own /Info dictionary is a metadata change.
            Some(kind::METADATA)
        } else {
            Some(change_kind(self.doc, self.old, dict, previous))
        }
    }

    /// A changed object that was a page in the signed revision, judged by what the page shows
    /// rather than by any `/Type` its new version carries. Only its annotations may change (see
    /// [`Self::annots_change`]); any other difference (content, resources, a relabelled or dropped
    /// `/Type`) is a page content change.
    fn page_change_kind(&self, d: &Dict, before: Option<&Dict>) -> Option<&'static str> {
        match before {
            Some(b) if !same_but_annots(d, b) => Some(kind::PAGE_CONTENT),
            Some(b) => match (annot_refs(self.old, b.get(b"Annots"), self.annot_budget), annot_refs(self.doc, d.get(b"Annots"), self.annot_budget)) {
                (Some(was), Some(now)) => self.annots_change(&was, &now),
                _ => Some(kind::OTHER),
            },
            None => Some(kind::PAGE_CONTENT),
        }
    }

    fn stream_change(&self, r: ObjRef, stream: &pdfcraft_cos::Stream, before: Option<&Object>) -> Option<&'static str> {
        // File plumbing is the stream's role when signed; a later /Type label cannot change it.
        let was_plumbing = matches!(before, Some(Object::Stream(previous)) if matches!(previous.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")));
        if self.old_content.contains(&r.num) {
            Some(kind::PAGE_CONTENT)
        } else if before.is_none() || was_plumbing {
            None
        } else if stream.dict.name(b"Type") == Some(b"Metadata") && Some(r.num) == self.old_metadata {
            // Only the signed catalog's XMP stream is metadata.
            Some(kind::METADATA)
        } else if self.old_xfa_data.contains(&r.num) {
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

/// The references in the annotation list `o` (inline or indirect), its length spent from
/// `budget`. `None` past the budget.
fn annot_refs(doc: &Document, o: Option<&Object>, budget: &Cell<usize>) -> Option<Vec<ObjRef>> {
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
fn references(doc: &Document) -> Option<HashMap<u32, HashSet<u16>>> {
    const MAX_REFERENCES: usize = 1 << 22;
    const MAX_DEPTH: u8 = 64;
    fn walk(o: &Object, depth: u8, found: &mut (HashMap<u32, HashSet<u16>>, usize)) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match o {
            Object::Ref(r) => {
                if found.0.entry(r.num).or_default().insert(r.generation) {
                    found.1 = found.1.saturating_add(1);
                }
                found.1 <= MAX_REFERENCES
            }
            Object::Dict(d) => d.iter().all(|(_, v)| walk(v, depth + 1, found)),
            Object::Stream(s) => s.dict.iter().all(|(_, v)| walk(v, depth + 1, found)),
            Object::Array(a) => a.iter().all(|v| walk(v, depth + 1, found)),
            _ => true,
        }
    }
    let mut found = (HashMap::new(), 0);
    for num in doc.object_numbers() {
        if !walk(&*doc.try_get(num).ok()?, 0, &mut found) {
            return None;
        }
    }
    doc.trailer().iter().all(|(_, v)| walk(v, 0, &mut found)).then_some(found.0)
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
    // A field taken out of the form: no DocMDP permission covers dropping a signed field.
    if lost_entries(doc, old, before.get(b"Fields"), now.get(b"Fields")) {
        return kind::OTHER;
    }
    if !same_entries(now, before, &[b"Fields", b"SigFlags"]) {
        return kind::FORM_FILL;
    }
    let old_fields = refs(old, before.get(b"Fields"));
    let added: Vec<ObjRef> = refs(doc, now.get(b"Fields")).into_iter().filter(|r| !old_fields.contains(r)).collect();
    if added.is_empty() || added_annots(doc, &added) == kind::SIGNATURE { kind::SIGNATURE } else { kind::FORM_FILL }
}

/// Whether two page dictionaries are the same apart from `/Annots`. Dictionaries are compared
/// without regard to key order at every depth: a writer may re-serialise a page's inline
/// resources in another order without changing what the page shows.
fn same_but_annots(d: &Dict, b: &Dict) -> bool {
    same_entries(d, b, &[b"Annots"])
}

/// Whether two dictionaries hold the same entries apart from `skip`: both sorted by key, then
/// compared pairwise, so neither side can hide a key the other lacks.
fn same_entries(a: &Dict, b: &Dict, skip: &[&[u8]]) -> bool {
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
fn same_object(a: &Object, b: &Object) -> bool {
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
fn same_entry(a: Option<&Object>, b: Option<&Object>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same_object(a, b),
        (None, None) => true,
        _ => false,
    }
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
fn catalog_kind(doc: &Document, old: &Document, d: &Dict, b: &Dict) -> &'static str {
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
        acroform_kind(doc, old, &now, None)
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
fn lost_entries(doc: &Document, old: &Document, was: Option<&Object>, now: Option<&Object>) -> bool {
    let live = |d: &Document, r: &ObjRef| !matches!(*d.get(*r), Object::Null);
    let now: HashSet<u32> = refs(doc, now).into_iter().filter(|r| live(doc, r) && live(old, r)).map(|r| r.num).collect();
    refs(old, was).iter().filter(|r| live(old, r)).any(|r| !now.contains(&r.num))
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
    }) && !before.is_some_and(|b| same_entries(d, b, &[]))
    {
        return kind::PAGE_CONTENT;
    }
    // A signed form field or widget keeps that role: its value, appearance or signature may
    // change, but turning it into a comment, another kind of annotation or another type of
    // field is no form fill.
    if before.is_some_and(|b| {
        (b.name(b"Subtype") == Some(b"Widget") || b.contains(b"FT")) && (d.name(b"Subtype") != b.name(b"Subtype") || d.name(b"FT") != b.name(b"FT"))
    }) {
        return kind::OTHER;
    }
    // Where a signed widget sits and whether it shows are what the page shows: filling the form
    // changes its value and appearance, never its rectangle or its flags.
    if before.is_some_and(|b| {
        b.name(b"Subtype") == Some(b"Widget") && (!same_entry(d.get(b"Rect"), b.get(b"Rect")) || !same_entry(d.get(b"F"), b.get(b"F")))
    }) {
        return kind::PAGE_CONTENT;
    }
    match ty {
        // Only a new signature value or policy: rewriting a signed one is not a signature (#1788).
        Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams") if before.is_none() => return kind::SIGNATURE,
        Some(b"Catalog") => return before.map_or(kind::FORM_FILL, |b| catalog_kind(doc, old, d, b)),
        Some(b"Page") => {
            return match before {
                Some(b) if !same_but_annots(d, b) => kind::PAGE_CONTENT,
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
    // The role an existing object plays (annotation, form field, the form) is the one it had when
    // signed: keys its new version carries are written by whoever changed it. An indirect
    // /OCProperties rewritten with an added /FT still decides which layers show (#1787 judges a
    // signed page's resources this way; this applies it to every other object).
    let role = before.unwrap_or(d);
    // Nor does an existing annotation or widget turned into another kind of annotation edit the
    // one it was: an empty signature widget rewritten as a /Square that covers signed text is no
    // signature change, and no comment either.
    if before.is_some_and(|b| b.name(b"Subtype") != d.name(b"Subtype")) {
        return kind::OTHER;
    }
    // The new version's /FT still decides whether this is a signature: a signed widget relabelled
    // as a text field is not a signature change, whatever it was when signed.
    match role.name(b"Subtype") {
        Some(b"Widget") => return if d.name(b"FT") == Some(b"Sig") && signature_ok { kind::SIGNATURE } else { kind::FORM_FILL },
        Some(b"Link") => return kind::LINKS,
        Some(_) if role.name(b"Type") == Some(b"Annot") || role.contains(b"Rect") => return kind::COMMENTS,
        _ => {}
    }
    if role.name(b"FT") == Some(b"Sig") && signature_ok {
        return kind::SIGNATURE;
    }
    if role.contains(b"Fields") {
        return acroform_kind(doc, old, d, before);
    }
    if role.contains(b"FT") || (role.contains(b"T") && role.contains(b"Kids")) || role.contains(b"DR") {
        return kind::FORM_FILL;
    }
    if before.is_none() {
        // New objects referenced by changed ones (fonts, appearance resources).
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
        let (none, budget, resources) = (HashSet::new(), Cell::new(MAX_ANNOT_ENTRIES), HashMap::new());
        let classifier = ChangedObjectClassifier {
            doc,
            old,
            old_content: &none,
            old_metadata: None,
            old_xfa_data: &none,
            signed_pages: &none,
            signed_annots: &none,
            page_nodes: &none,
            kids_arrays: &none,
            placed_annots: &none,
            annot_budget: &budget,
            signed_resources: &resources,
            old_info: None,
            old_form: None,
            dss: &none,
        };
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
        assert_eq!(change_kind(&doc, &doc, &reordered, Some(&before)), change_kind(&doc, &doc, &before, Some(&before)));

        // A changed value inside the nested dictionary still is a page content change.
        let changed = page(vec![("F2", font(&order, "Helvetica")), ("F1", font(&order, "Courier"))], vec![]);
        assert_eq!(page_change_kind(&doc, &doc, &changed, Some(&before)), Some(kind::PAGE_CONTENT));
        assert_eq!(change_kind(&doc, &doc, &changed, Some(&before)), kind::PAGE_CONTENT);
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
            assert_eq!(change_kind(&doc, &doc, now, Some(then)), kind::PAGE_CONTENT);
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
        match classify_changes(&doc, Some(old), Some(1)) {
            Modification::Disallowed(kinds) => assert!(kinds.contains(&kind::STRUCTURE), "{kinds:?}"),
            other => panic!("the certification's /Perms changed in place under P=1: {other:?}"),
        }
    }
}
