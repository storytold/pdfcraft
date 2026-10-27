use super::types::{Modification, Status, TrustStore};
use super::validate::{is_doc_timestamp, validate_doc_timestamp, validate_into};
use super::*;

/// Digests of signed byte ranges (and parsed signed revisions), kept across revalidations of
/// one document: incremental edits only append to the file, so the signed bytes and their
/// digest don't change, and rehashing a large file on every edit was the expensive part of
/// validation (120 MB: 84 ms per comment edit before, 6 ms after). Keyed by the ranges, the
/// algorithm and a sampled fingerprint of the covered bytes; use one cache per document, whose
/// bytes only ever grow.
#[derive(Debug, Default)]
pub struct DigestCache {
    map: std::sync::Mutex<HashMap<RangeKey, Vec<u8>>>,
    /// Signed revisions opened for change classification, by length and fingerprint.
    revisions: std::sync::Mutex<HashMap<(usize, [u8; 32]), Document>>,
    /// Candidate identities only: validation still uses current bytes and current trust.
    pub(crate) discovery: std::sync::Mutex<Option<TimestampCandidates>>,
    #[cfg(test)]
    pub(crate) discovery_scans: std::sync::atomic::AtomicUsize,
}

/// Keep only the most recently visited source/overlay. In particular, this must not retain
/// a Document, its bytes, or parsed signature dictionaries across edits and saves.
#[derive(Debug)]
pub(crate) struct TimestampCandidates {
    source: pdfcraft_cos::SourceIdentity,
    modified: Vec<u32>,
    pub(crate) candidates: Vec<ObjRef>,
}

pub(crate) const DISCOVERY_CACHE_LIMIT: usize = 4096;

pub(crate) fn is_document_timestamp(object: &Object) -> bool {
    object.as_dict().is_some_and(|d| is_doc_timestamp(d) && d.contains(b"ByteRange"))
}

/// Only dictionary/null edits have a local effect on candidate membership. Changes to an
/// object stream or a scalar used as an indirect /Length can affect unedited objects too.
/// Checking both the original and current objects also covers freeing those dependencies.
/// Unsupported or unreadable edits deliberately fall back to the complete bounded scan.
fn changed_timestamp_candidates(doc: &Document, numbers: &[u32]) -> Option<Vec<ObjRef>> {
    for (reference, object) in doc.scan_original_objects_subset(numbers.to_vec()) {
        match object.ok()?.as_ref() {
            Object::Dict(_) => {}
            Object::Null if matches!(doc.xref_entry(reference.num), None | Some(pdfcraft_cos::XrefEntry::Free { .. })) => {}
            _ => return None,
        }
    }
    let mut candidates = Vec::new();
    for (reference, object) in doc.scan_objects_subset(numbers.to_vec()) {
        let object = object.ok()?;
        if !matches!(object.as_ref(), Object::Dict(_) | Object::Null) {
            return None;
        }
        if is_document_timestamp(&object) {
            candidates.push(reference);
        }
    }
    Some(candidates)
}

/// (end of first range, start of second, end of second, algorithm, fingerprint).
type RangeKey = (usize, usize, usize, DigestAlg, [u8; 32]);

/// 64 KiB sampled from across `covered`, plus its length.
fn fingerprint(covered: &[u8]) -> [u8; 32] {
    let mut parts: Vec<&[u8]> = (0..64)
        .map(|i| {
            let at = covered.len().saturating_sub(1024) * i / 63;
            &covered[at..(at + 1024).min(covered.len())]
        })
        .collect();
    let len = (covered.len() as u64).to_be_bytes();
    parts.push(&len);
    DigestAlg::Sha256.digest(&parts).try_into().unwrap_or([0; 32])
}

impl DigestCache {
    pub(crate) fn timestamp_candidates(&self, doc: &Document) -> Vec<ObjRef> {
        let mut slot = self.discovery.lock().ok();
        let previous = slot.as_mut().and_then(|slot| slot.take());
        let source = doc.source_identity();
        let modified = doc.modified_objects();
        // Reconstructed/unknown input and output-security changes take the original path.
        let eligible = slot.is_some()
            && doc.repair_log().is_empty()
            && !doc.revisions().is_empty()
            && !doc.encryption_changed()
            && modified.len() <= DISCOVERY_CACHE_LIMIT;
        if eligible && let Some(previous) = previous.filter(|previous| previous.source == source) {
            // Undo/redo can remove overlays as well as add them. Revisit the union so an
            // original candidate replaced or freed in the last snapshot can reappear.
            let mut changed = previous.modified;
            changed.extend_from_slice(&modified);
            changed.sort_unstable();
            changed.dedup();
            if changed.len() <= DISCOVERY_CACHE_LIMIT
                && let Some(mut candidates) = changed_timestamp_candidates(doc, &changed)
            {
                candidates.extend(previous.candidates.into_iter().filter(|r| changed.binary_search(&r.num).is_err()));
                candidates.sort_unstable_by_key(|r| r.num);
                if candidates.len() <= DISCOVERY_CACHE_LIMIT
                    && let Some(slot) = slot.as_mut()
                {
                    **slot = Some(TimestampCandidates { source, modified, candidates: candidates.clone() });
                }
                return candidates;
            }
        }
        #[cfg(test)]
        self.discovery_scans.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut complete = true;
        let candidates: Vec<_> = doc
            .scan_objects_checked()
            .filter_map(|(reference, object)| match object {
                Ok(object) => is_document_timestamp(&object).then_some(reference),
                Err(_) => {
                    complete = false;
                    None
                }
            })
            .collect();
        if eligible
            && complete
            && candidates.len() <= DISCOVERY_CACHE_LIMIT
            && changed_timestamp_candidates(doc, &modified).is_some()
            && let Some(slot) = slot.as_mut()
        {
            **slot = Some(TimestampCandidates { source, modified, candidates: candidates.clone() });
        }
        candidates
    }

    /// The signed revision `bytes[..end]`, parsed (and kept). An encrypted one is opened with
    /// `doc`'s own key (the same `/Encrypt` and `/ID`), so a user password the document was
    /// opened with is neither needed again nor kept here (#1808).
    pub(super) fn revision(&self, doc: &Document, bytes: &[u8], end: usize) -> Option<Document> {
        let key = (end, fingerprint(&bytes[..end]));
        if let Some(d) = self.revisions.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Some(d);
        }
        let d = doc.open_with_same_credentials(Arc::new(bytes[..end].to_vec())).ok()?;
        if let Ok(mut m) = self.revisions.lock() {
            if m.len() > 8 {
                m.clear();
            }
            m.insert(key, d.clone());
        }
        Some(d)
    }

    pub(super) fn digest(&self, alg: DigestAlg, bytes: &[u8], l0: usize, o1: usize, end: usize) -> Vec<u8> {
        let key = (l0, o1, end, alg, fingerprint(&bytes[..end]));
        if let Some(d) = self.map.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return d;
        }
        let d = alg.digest(&[&bytes[..l0], &bytes[o1..end]]);
        if let Ok(mut m) = self.map.lock() {
            if m.len() > 64 {
                m.clear();
            }
            m.insert(key, d.clone());
        }
        d
    }
}

pub(crate) fn text(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    let o = doc.resolve(d.get(key)?);
    let s = match &*o {
        Object::String(s) => s.to_text(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    let s = s.trim_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// A signature field [`walk_sig_fields`] found.
pub(crate) struct Field {
    /// Fully qualified: the `/T` parts down the tree joined with dots, at most
    /// [`MAX_FIELD_NAME_BYTES`].
    pub(crate) name: String,
    /// The field dictionary (and its widget(s)).
    pub(super) r: Option<ObjRef>,
    pub(super) dict: Dict,
    /// A widget several fields list is listed under each of them, as one shared copy.
    pub(crate) widgets: Vec<(Option<ObjRef>, Rc<Dict>)>,
}

impl Field {
    /// The signature dictionary in /V. A field without one is unsigned, whether /V is missing,
    /// null or not a dictionary.
    pub(super) fn signature(&self, doc: &Document) -> Option<Dict> {
        self.dict.get(b"V").map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned())
    }

    /// True when /V holds something that is neither null nor a signature dictionary: signing the
    /// field would overwrite a value we can't read as a signature.
    pub(super) fn has_foreign_value(&self, doc: &Document) -> bool {
        self.dict.get(b"V").map(|v| doc.resolve(v)).is_some_and(|v| !matches!(*v, Object::Null) && v.as_dict().is_none())
    }
}

/// Most fields looked at for signature fields; real forms have far fewer. Past it no more are
/// taken, and [`list_cached`] stands for the ones not read with one row ([`unread_fields`]).
pub(super) const MAX_FIELDS: usize = 20_000;
/// Most entries read from `/Fields` and all the `/Kids` arrays together: room for a few widgets
/// for each of [`MAX_FIELDS`]. A `/Kids` array several fields list is read under each of them, so
/// this bounds the walk where the field count can't. Past it no more entries are read: fields
/// already taken are still listed, without the widgets past it.
pub(super) const MAX_ENTRIES: usize = 100_000;
/// Longest fully qualified field name read, in bytes. Real names are far shorter; a field named
/// longer is passed over, and counts as unread as fields past [`MAX_FIELDS`] do unless it is a leaf
/// of another type.
pub(crate) const MAX_FIELD_NAME_BYTES: usize = 4096;
/// Most bytes of names the walk keeps, all the fields it lists together. A kid's name repeats its
/// parent's, so under the caps above one long name over many kids would still come to 80 MB.
pub(super) const MAX_NAME_BYTES: usize = 4 << 20;

/// Every signature field the walk reads ([`walk_sig_fields`] has its caps), depth first in
/// `/Fields` order.
pub(super) fn sig_fields(doc: &Document) -> Vec<Field> {
    walk_sig_fields(doc).0
}

/// [`sig_fields`], and whether the walk read the whole form. It did not when a cap stopped it or a
/// field that is or holds a signature field was passed over for its name; a field it did not reach
/// may then claim one of the widgets listed.
///
/// Kids share their parent's name instead of each copying it. A widget is listed under every field
/// that lists it, so two claims on one widget stay visible, and it is copied once. A field taken
/// by reference is taken where the walk first reaches it; one written in place in an indirect
/// array is queued once, however many fields list that array.
pub(crate) fn walk_sig_fields(doc: &Document) -> (Vec<Field>, bool) {
    let mut out = Vec::new();
    let Some(root) = doc.root() else { return (out, true) };
    let Some(af) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()) else {
        return (out, true);
    };
    let Some(top) = af.get(b"Fields") else { return (out, true) };
    let (mut fields_left, mut entries_left, mut names_left, mut whole) = (MAX_FIELDS, MAX_ENTRIES, MAX_NAME_BYTES, true);
    // Fields by reference, and by slot those written in place in an indirect array.
    let mut seen = HashSet::new();
    let mut copies: HashMap<(ObjRef, Option<usize>), Rc<Dict>> = HashMap::new();
    let unnamed: Rc<str> = Rc::from("");
    let mut stack: Vec<(Object, Rc<str>, bool)> = Vec::new();
    let fields = doc.resolve(top);
    for (i, f) in fields.as_array().into_iter().flatten().enumerate() {
        if !take(&mut entries_left) {
            whole = false;
            break;
        }
        // A field written in place here is marked, so one that also lists this array as its /Kids
        // does not walk it again.
        if let Some(s) = slot(f, top.as_ref(), i) {
            seen.insert(s);
        }
        if !take(&mut fields_left) {
            whole = false;
            break;
        }
        stack.push((f.clone(), unnamed.clone(), false));
    }
    stack.reverse();
    while let Some((o, prefix, inherited_sig)) = stack.pop() {
        if let Some(r) = o.as_ref()
            && !seen.insert((r, None))
        {
            continue;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { continue };
        let sig = d.name(b"FT").map_or(inherited_sig, |ft| ft == b"Sig");
        // Passed over for its name, a leaf of another type hides no signature: it is none, and
        // has no kids to hold one.
        let hides = sig || d.contains(b"Kids");
        // Even as UTF-16, a name within the cap takes at most twice its length plus a two-byte
        // mark. A /T stored in more is passed over undecoded, whatever it would trim to: a long
        // string named by many fields would otherwise be decoded once for each.
        let stored = d.get(b"T").map(|t| doc.resolve(t)).map_or(0, |t| match &*t {
            Object::String(s) => s.bytes.len(),
            Object::Name(n) => n.len(),
            _ => 0,
        });
        if stored > 2 * MAX_FIELD_NAME_BYTES + 2 {
            whole &= !hides;
            continue;
        }
        let part = text(doc, &d, b"T");
        let name: Rc<str> = match (&part, prefix.is_empty()) {
            (Some(p), true) => p.as_str().into(),
            (Some(p), false) => format!("{prefix}.{p}").into(),
            (None, _) => prefix,
        };
        if name.len() > MAX_FIELD_NAME_BYTES {
            whole &= !hides;
            continue;
        }
        let listed = sig && part.is_some();
        let mut ws: Vec<(Option<ObjRef>, Rc<Dict>)> = Vec::new();
        if listed && d.name(b"Subtype") == Some(b"Widget") {
            ws.push((o.as_ref(), Rc::new(d.clone())));
        }
        let array = d.get(b"Kids").and_then(Object::as_ref);
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        let mut sub = Vec::new();
        for (i, k) in kids.as_deref().and_then(Object::as_array).into_iter().flatten().enumerate() {
            if !take(&mut entries_left) {
                whole = false;
                break;
            }
            let resolved;
            let kd = match k {
                Object::Ref(r) => {
                    resolved = doc.get(*r);
                    resolved.as_dict()
                }
                direct => direct.as_dict(),
            };
            let Some(kd) = kd else { continue };
            let at = slot(k, array, i);
            // Kids with a /T are fields; others are this field's widgets.
            if kd.contains(b"T") {
                if at.is_some_and(|s| !seen.insert(s)) {
                    continue;
                }
                if !take(&mut fields_left) {
                    whole = false;
                    continue;
                }
                sub.push(k.clone());
            } else if listed {
                let w = match k.as_ref().map(|r| (r, None)).or(at) {
                    Some(key) => copies.entry(key).or_insert_with(|| Rc::new(kd.clone())).clone(),
                    None => Rc::new(kd.clone()),
                };
                ws.push((k.as_ref(), w));
            }
        }
        for k in sub.into_iter().rev() {
            stack.push((k, name.clone(), sig));
        }
        if listed {
            let Some(left) = names_left.checked_sub(name.len()) else {
                whole = false;
                break;
            };
            names_left = left;
            out.push(Field { name: name.to_string(), r: o.as_ref(), dict: d, widgets: ws });
        }
    }
    (out, whole)
}

/// The slot of an entry written in place in an indirect array, which tells it apart when several
/// fields list that array. An entry by reference, or in a direct array (one owner), has none.
fn slot(entry: &Object, array: Option<ObjRef>, i: usize) -> Option<(ObjRef, Option<usize>)> {
    array.filter(|_| entry.as_ref().is_none()).map(|a| (a, Some(i)))
}

/// Take one from a budget; false once it is spent.
fn take(left: &mut usize) -> bool {
    let some = *left > 0;
    *left = left.saturating_sub(1);
    some
}

/// Which page holds each annotation.
fn annot_pages(doc: &Document) -> HashMap<ObjRef, usize> {
    let mut map = HashMap::new();
    for (i, p) in pdfcraft_annot::page_refs(doc).unwrap_or_default().into_iter().enumerate() {
        if let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()) {
            for r in a.iter().filter_map(Object::as_ref) {
                map.insert(r, i);
            }
        }
    }
    map
}

/// Whether a signature widget is shown on its page: it isn't Hidden (`/F` bit 2) or NoView (bit 6),
/// as the forms crate reads a widget's flags, and its `/Rect` is four finite numbers whose box,
/// corners in either order, is wider and taller than half a point. The page badge
/// ([`SignatureInfo::visible`]) and the redrawn status layers ([`crate::show_status_layers`]) both
/// go by it, so they never disagree about a widget.
pub(crate) fn widget_shown(doc: &Document, widget: &Dict) -> bool {
    let flags = widget.get(b"F").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0);
    let spans = |a: f64, b: f64| (a - b).abs() > 0.5;
    flags & (2 | 32) == 0
        && nums(doc, widget, b"Rect").is_some_and(|r| r.len() == 4 && r.iter().all(|v| v.is_finite()) && spans(r[0], r[2]) && spans(r[1], r[3]))
}

pub(super) fn nums(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    doc.resolve(d.get(key)?).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())
}

/// List and validate every signature field. `bytes` is the file as stored (the ranges index it).
/// A form too large to read whole gets one more row, signed and of unknown validity, for the
/// fields not read: any of them may be signed, so the document still counts as signed.
pub fn list(doc: &Document, bytes: &[u8], trust: &TrustStore) -> Vec<SignatureInfo> {
    list_cached(doc, bytes, trust, &DigestCache::default())
}

/// [`list`], reusing digests and standalone-timestamp discovery from `cache`. Field/widget
/// structure and validation against `bytes` and `trust` are read again on every call.
pub fn list_cached(doc: &Document, bytes: &[u8], trust: &TrustStore, cache: &DigestCache) -> Vec<SignatureInfo> {
    let pages = annot_pages(doc);
    let mut out = Vec::new();
    let mut field_values: HashSet<ObjRef> = HashSet::new();
    let (fields, whole) = walk_sig_fields(doc);
    for f in fields {
        if let Some(v_ref) = f.dict.get(b"V").and_then(Object::as_ref) {
            field_values.insert(v_ref);
        }
        let widget = f.widgets.first();
        let page = widget.and_then(|(r, w)| r.and_then(|r| pages.get(&r).copied()).or_else(|| w.reference(b"P").and_then(|p| page_index(doc, p))));
        let rect = widget
            .and_then(|(_, w)| nums(doc, w, b"Rect"))
            .filter(|r| r.len() == 4)
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
        let visible = widget.is_some_and(|(_, w)| widget_shown(doc, w));
        let v = f.signature(doc);
        let mut info = SignatureInfo {
            field: f.name.clone(),
            signed: v.is_some(),
            page,
            rect,
            visible,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify: None,
            revision: 0,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status: Status::Unknown,
            modification: Modification::None,
            verdict_shown: false,
            details: Vec::new(),
        };
        if let Some(v) = v {
            validate_into(doc, bytes, trust, &v, &mut info, cache);
        }
        out.push(info);
    }
    // Standalone document timestamps (ISO 32000-2 §12.8.2.2) live outside AcroForm fields.
    let candidates = cache.timestamp_candidates(doc).into_iter().map(|r| r.num).collect();
    for (r, o) in doc.scan_objects_subset(candidates) {
        if field_values.contains(&r) {
            continue;
        }
        let Ok(o) = o else { continue };
        let Some(d) = o.as_dict() else { continue };
        if !is_document_timestamp(&o) {
            continue;
        }
        let mut info = SignatureInfo {
            field: "DocumentTimestamp".to_string(),
            signed: true,
            page: None,
            rect: None,
            visible: false,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: Some("ETSI.RFC3161".to_string()),
            doc_timestamp: true,
            certify: None,
            revision: 0,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: true,
            timestamp_time: None,
            status: Status::Unknown,
            modification: Modification::None,
            verdict_shown: false,
            details: Vec::new(),
        };
        validate_doc_timestamp(doc, bytes, d, &mut info, cache, trust);
        out.push(info);
    }
    if !whole {
        out.push(unread_fields());
    }
    out
}

/// The row [`list_cached`] adds for the fields a walk cut short did not read ([`walk_sig_fields`]).
/// Any of them may be signed, so the row is signed, of unknown validity: the document still counts
/// as signed, and nothing rewrites it in full.
fn unread_fields() -> SignatureInfo {
    SignatureInfo {
        field: "UnreadFields".to_string(),
        signed: true,
        page: None,
        rect: None,
        visible: false,
        signer: None,
        certificate: None,
        chain: Vec::new(),
        date: None,
        signing_time: None,
        reason: None,
        location: None,
        contact: None,
        sub_filter: None,
        doc_timestamp: false,
        certify: None,
        revision: 0,
        signed_len: 0,
        digest: None,
        algorithm: None,
        timestamp: false,
        timestamp_time: None,
        status: Status::Unknown,
        modification: Modification::None,
        verdict_shown: false,
        details: vec![Detail::plain(
            "This form has too many fields, or a field name too long, for PdfCraft to read them all. Any signature among the rest is not listed, so the document is treated as signed.",
        )],
    }
}

fn page_index(doc: &Document, p: ObjRef) -> Option<usize> {
    pdfcraft_annot::page_refs(doc).ok()?.iter().position(|r| *r == p)
}

/// A widget's `/MK /R` as a quarter turn (0, 90, 180 or 270); missing, malformed or not a
/// multiple of 90 counts as 0, as `pdfcraft-forms` reads it for the fields it draws.
pub(super) fn mk_rotation(doc: &Document, widget: &Dict) -> i64 {
    let r = widget
        .get(b"MK")
        .map(|mk| doc.resolve(mk))
        .and_then(|mk| mk.as_dict().and_then(|mk| mk.get(b"R")).map(|r| doc.resolve(r)))
        .and_then(|r| r.as_int().or_else(|| r.as_f64().filter(|v| v.is_finite() && v.abs() <= 1.0e6).map(|v| v.round() as i64)));
    r.map(|r| r.rem_euclid(360)).filter(|q| matches!(q, 90 | 180 | 270)).unwrap_or(0)
}
