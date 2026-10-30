use std::cell::OnceCell;

use super::types::{Modification, kind};
use super::{Arc, Dict, Document, HashMap, HashSet, ObjRef, Object};

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
    let classifier = ChangedObjectClassifier {
        doc,
        old: &old,
        old_content: &old_content,
        old_metadata,
        old_xfa_data: &old_xfa_data,
        signed_pages: &signed_pages,
        signed_annots: &signed_annots,
        signed_resources: &signed_resources,
        old_info,
        dss: &dss,
        form_roles: OnceCell::new(),
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

struct ChangedObjectClassifier<'a> {
    doc: &'a Document,
    old: &'a Document,
    old_content: &'a HashSet<ObjRef>,
    old_metadata: Option<ObjRef>,
    old_xfa_data: &'a HashSet<ObjRef>,
    signed_pages: &'a HashSet<ObjRef>,
    signed_annots: &'a HashSet<ObjRef>,
    signed_resources: &'a HashMap<ObjRef, bool>,
    old_info: Option<ObjRef>,
    dss: &'a HashSet<u32>,
    /// [`form_fill_roles`] of the signed revision, worked out the first time a change would
    /// otherwise be refused.
    form_roles: OnceCell<HashMap<ObjRef, FormRole>>,
}

impl ChangedObjectClassifier<'_> {
    fn classify(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        if let Some(kind) = self.resource_kind(r, new, before) {
            return Some(kind);
        }
        if self.dss.contains(&r.num) {
            return Some(kind::SECURITY_STORE);
        }
        let what = match new {
            Object::Array(now) if self.is_signed_annotation_array(r, before) => Some(self.annotation_array_change(now, before)),
            Object::Dict(dict) => Some(self.dictionary_change(r, dict, before)),
            Object::Stream(stream) => self.stream_change(r, stream, before),
            _ if before.is_none() => None,
            _ => Some(kind::OTHER),
        };
        if what != Some(kind::OTHER) {
            return what;
        }
        // A field appearance or a form resource rewritten in place (#3442), judged by the role the
        // object held when signed and held nowhere else.
        match self.form_roles.get_or_init(|| form_fill_roles(self.old)).get(&r) {
            Some(FormRole::Appearance) => Some(kind::FORM_FILL),
            Some(FormRole::Resources) if before.is_some_and(|b| only_gained_entries(b, new, 0)) => Some(kind::FORM_FILL),
            _ => what,
        }
    }

    fn resource_kind(&self, r: ObjRef, new: &Object, before: Option<&Object>) -> Option<&'static str> {
        match (self.signed_resources.get(&r), new) {
            (Some(false), _) => Some(kind::PAGE_CONTENT),
            (Some(true), Object::Dict(dict)) if self.keeps_previous_entries(dict, before) => None,
            (Some(true), _) => Some(kind::PAGE_CONTENT),
            (None, _) => None,
        }
    }

    fn keeps_previous_entries(&self, dict: &Dict, before: Option<&Object>) -> bool {
        before.and_then(Object::as_dict).is_some_and(|previous| previous.iter().all(|(key, value)| dict.get(key) == Some(value)))
    }

    fn is_signed_annotation_array(&self, r: ObjRef, before: Option<&Object>) -> bool {
        self.signed_annots.contains(&r) && before.is_some_and(|object| object.as_array().is_some())
    }

    fn annotation_array_change(&self, now: &[Object], before: Option<&Object>) -> &'static str {
        let was: HashSet<ObjRef> =
            before.and_then(Object::as_array).map(|items| items.iter().filter_map(Object::as_ref).collect()).unwrap_or_default();
        let kept: HashSet<ObjRef> = now.iter().filter_map(Object::as_ref).collect();
        let removed: Vec<ObjRef> = was.iter().filter(|reference| !kept.contains(reference)).copied().collect();
        if removed.is_empty() {
            let added: Vec<ObjRef> = now.iter().filter_map(Object::as_ref).filter(|reference| !was.contains(reference)).collect();
            added_annots(self.doc, &added)
        } else if removed.iter().any(|reference| self.old.get(*reference).as_dict().is_some_and(|dict| dict.name(b"Subtype") == Some(b"Widget"))) {
            // A form field or signature widget taken off the page: no permission covers it.
            kind::OTHER
        } else {
            // Deleting comments is a comment change (permitted at P 3 only).
            kind::COMMENTS
        }
    }

    fn dictionary_change(&self, r: ObjRef, dict: &Dict, before: Option<&Object>) -> &'static str {
        let previous = before.and_then(Object::as_dict);
        if self.signed_pages.contains(&r) {
            // A signed page is judged by what the page shows, whatever its new labels say.
            page_change_kind(self.doc, self.old, dict, previous)
        } else if previous.is_some() && Some(r) == self.old_info {
            // Only the signed revision's own /Info dictionary is a metadata change.
            kind::METADATA
        } else {
            change_kind(self.doc, self.old, dict, previous)
        }
    }

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

/// What an object of the signed revision was to its interactive form, when that was all it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FormRole {
    /// An appearance of an ordinary field's widget: the widget's `/AP` dictionary, its `/N`, `/R`
    /// or `/D` dictionary of states, or an appearance stream. Filling the field redraws it, so any
    /// rewrite is a form fill.
    Appearance,
    /// The form's default resources (`/AcroForm /DR`) or one of its category dictionaries.
    /// Filling a field may add the font it is drawn with, so a rewrite that only adds entries is a
    /// form fill; one that changes or drops an entry is not.
    Resources,
}

/// How the walk in [`form_fill_roles`] treats an object it comes to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    /// Marked reached, and everything it refers to walked.
    Plain,
    /// The AcroForm, written inline in the catalog.
    InlineForm,
    /// `/DR`: not marked reached, its categories walked as [`Visit::Category`], anything else in it
    /// as [`Visit::Plain`].
    Resources,
    /// A category dictionary of `/DR`: not marked reached, what it names walked.
    Category,
}

/// The objects of the signed revision that a form fill may rewrite in place (#3442), with their
/// [`FormRole`]. A writer may redraw a filled field in its existing appearance stream, and add the
/// font it drew with to an existing indirect `/DR`, rather than write new objects.
///
/// The policy:
/// - An object qualifies only through the role it held when signed, never through what a later
///   revision says it is.
/// - It qualifies only when that role is all it held: nothing in the signed file refers to it,
///   directly or through other objects, except ordinary widgets' appearances and the form's
///   default resources. The file is walked from the trailer, everything except the `/AP` of
///   ordinary widgets, and `/DR` and its category dictionaries themselves (what they name is
///   walked); whatever that walk reaches (a page's content or resources, another annotation's or
///   a signature field's appearance, an action, the catalog's own entries, a font or XObject
///   `/DR` names) is left to the stricter judgement it had.
/// - Ordinary widgets may share an appearance: radio buttons often share their off appearance.
///   Rewriting it changes only how those fields look, as filling them may.
/// - An ordinary widget is one of a text, button or choice field (`/FT`, its own or inherited).
///   Signature fields, comments and widgets without a field type are not.
/// - Each object must have the shape of its role: an appearance dictionary has only `/N`, `/R`
///   and `/D`; a dictionary of states holds only references to streams; an appearance is a
///   stream that is untyped or `/Type /XObject`; `/DR` has only resource categories.
///
/// Rewriting an ordinary field's appearance in place grants nothing a form fill can't already do:
/// filling may give the widget a new appearance stream drawing anything. Past `MAX_VISITED`
/// objects the walk gives up and nothing qualifies, so the changes are judged as before.
fn form_fill_roles(old: &Document) -> HashMap<ObjRef, FormRole> {
    const MAX_VISITED: usize = 500_000;
    const STATES: [&[u8]; 3] = [b"N", b"R", b"D"];
    const RESOURCE_CATEGORIES: [&[u8]; 8] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties", b"Encoding"];

    let acroform_ref = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"AcroForm").and_then(Object::as_ref)));
    let mut seen: HashSet<ObjRef> = HashSet::new();
    // `/DR` and its category dictionaries: walked through without being marked reached.
    let mut containers: HashSet<ObjRef> = HashSet::new();
    let (mut appearances, mut resources) = (Vec::new(), Vec::new());
    let mut stack: Vec<(Object, Visit)> = old.trailer().iter().map(|(_, v)| (v.clone(), Visit::Plain)).collect();
    while let Some((o, visit)) = stack.pop() {
        let held;
        let (here, o) = match o {
            Object::Ref(r) => {
                let first = if matches!(visit, Visit::Resources | Visit::Category) { containers.insert(r) } else { seen.insert(r) };
                if !first {
                    continue;
                }
                if seen.len().saturating_add(containers.len()) > MAX_VISITED {
                    return HashMap::new();
                }
                held = old.get(r);
                (Some(r), &*held)
            }
            ref other => (None, other),
        };
        match (o, visit) {
            // What the form's default resources name is reached like anything else: an appearance
            // that `/DR` also names is not only an appearance.
            (Object::Dict(d), Visit::Resources) => stack
                .extend(d.iter().map(|(k, v)| (v.clone(), if RESOURCE_CATEGORIES.contains(&k.as_slice()) { Visit::Category } else { Visit::Plain }))),
            (Object::Dict(d), Visit::Category) => stack.extend(d.iter().map(|(_, v)| (v.clone(), Visit::Plain))),
            // A `/DR` or category that is no dictionary qualifies for nothing: walked as anything.
            (_, Visit::Resources | Visit::Category) => {
                if let Some(r) = here {
                    containers.remove(&r);
                    stack.push((Object::Ref(r), Visit::Plain));
                } else {
                    stack.push((o.clone(), Visit::Plain));
                }
            }
            (Object::Dict(d), _) => {
                let widget = d.name(b"Subtype") == Some(b"Widget") && is_ordinary_field(old, d);
                let form = visit == Visit::InlineForm || (here.is_some() && here == acroform_ref);
                let catalog = here.is_some() && here == old.root();
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"AP" if widget => appearances.push(v.clone()),
                        b"DR" if form => {
                            resources.push(v.clone());
                            stack.push((v.clone(), Visit::Resources));
                        }
                        b"AcroForm" if catalog => stack.push((v.clone(), Visit::InlineForm)),
                        _ => stack.push((v.clone(), Visit::Plain)),
                    }
                }
            }
            (Object::Stream(s), _) => stack.extend(s.dict.iter().map(|(_, v)| (v.clone(), Visit::Plain))),
            (Object::Array(a), _) => stack.extend(a.iter().map(|v| (v.clone(), Visit::Plain))),
            _ => {}
        }
    }

    let mut roles = HashMap::new();
    // Anything the walk reached has another role too; a conflict takes the stricter role.
    let mut claim = |r: ObjRef, role: FormRole| {
        if !seen.contains(&r) {
            roles
                .entry(r)
                .and_modify(|held: &mut FormRole| {
                    if *held != role {
                        *held = FormRole::Resources;
                    }
                })
                .or_insert(role);
        }
    };
    let is_appearance = |o: &Object| matches!(&*old.resolve(o), Object::Stream(s) if matches!(s.dict.name(b"Type"), None | Some(b"XObject")));
    for ap in appearances {
        let ap = match ap {
            Object::Ref(r) => match &*old.get(r) {
                Object::Dict(d) if d.iter().all(|(k, _)| STATES.contains(&k.as_slice())) => {
                    claim(r, FormRole::Appearance);
                    d.clone()
                }
                _ => continue,
            },
            Object::Dict(d) => d,
            _ => continue,
        };
        for v in STATES.iter().filter_map(|key| ap.get(key)) {
            let (states, indirect) = match v {
                Object::Ref(r) if is_appearance(v) => {
                    claim(*r, FormRole::Appearance);
                    continue;
                }
                Object::Ref(r) => (old.get(*r).as_dict().cloned(), Some(*r)),
                Object::Dict(d) => (Some(d.clone()), None),
                _ => continue,
            };
            let Some(states) = states.filter(|d| d.iter().all(|(_, s)| s.as_ref().is_some() && is_appearance(s))) else { continue };
            if let Some(r) = indirect {
                claim(r, FormRole::Appearance);
            }
            states.iter().filter_map(|(_, s)| s.as_ref()).for_each(|s| claim(s, FormRole::Appearance));
        }
    }
    let is_resources = |d: &Dict| d.iter().all(|(k, _)| k.as_slice() == b"ProcSet" || RESOURCE_CATEGORIES.contains(&k.as_slice()));
    for dr in resources {
        let d = match dr {
            Object::Ref(r) => match &*old.get(r) {
                Object::Dict(d) if is_resources(d) => {
                    claim(r, FormRole::Resources);
                    d.clone()
                }
                _ => continue,
            },
            Object::Dict(d) if is_resources(&d) => d,
            _ => continue,
        };
        for category in RESOURCE_CATEGORIES.iter().filter_map(|key| d.get(key)).filter_map(Object::as_ref) {
            if old.get(category).as_dict().is_some() {
                claim(category, FormRole::Resources);
            }
        }
    }
    roles
}

/// Whether the dictionary `now` keeps every entry of `before`: the same value, or an inline
/// dictionary that itself only gained entries. A reference stays the same reference, so an entry
/// pointed at another object, however alike, is a change.
fn only_gained_entries(before: &Object, now: &Object, depth: u8) -> bool {
    match (before, now) {
        (Object::Dict(b), Object::Dict(n)) => {
            b.iter().all(|(k, v)| n.get(k).is_some_and(|nv| nv == v || (depth < 4 && only_gained_entries(v, nv, depth + 1))))
        }
        _ => false,
    }
}

/// Whether `widget`'s field (the widget itself, or the nearest ancestor with a `/FT`) is a text,
/// button or choice field: one whose appearance a form fill redraws.
fn is_ordinary_field(old: &Document, widget: &Dict) -> bool {
    const MAX_ANCESTORS: usize = 64;
    let mut ft = widget.name(b"FT").map(<[u8]>::to_vec);
    let mut parent = widget.get(b"Parent").and_then(Object::as_ref);
    let mut seen = HashSet::new();
    while ft.is_none()
        && let Some(p) = parent
        && seen.insert(p)
        && seen.len() <= MAX_ANCESTORS
    {
        let held = old.get(p);
        let Some(d) = held.as_dict() else { break };
        ft = d.name(b"FT").map(<[u8]>::to_vec);
        parent = d.get(b"Parent").and_then(Object::as_ref);
    }
    matches!(ft.as_deref(), Some(b"Tx" | b"Btn" | b"Ch"))
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
