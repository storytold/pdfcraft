use super::types::{Modification, Status, TrustStore};
use super::validate::{is_doc_timestamp, validate_doc_timestamp, validate_into};
use super::{Arc, Dict, DigestAlg, Document, HashMap, HashSet, ObjRef, Object, SignatureInfo};

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

pub(super) struct Field {
    pub(super) name: String,
    /// The field dictionary (and its widget(s)).
    pub(super) r: Option<ObjRef>,
    pub(super) dict: Dict,
    pub(super) widgets: Vec<(Option<ObjRef>, Dict)>,
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

/// Every signature field, depth first in `/Fields` order.
pub(super) fn sig_fields(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let Some(root) = doc.root() else { return out };
    let Some(af) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()) else {
        return out;
    };
    let Some(fields) = af.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()) else { return out };
    let mut seen = HashSet::new();
    let mut stack: Vec<(Object, String, Option<Vec<u8>>)> = fields.into_iter().rev().map(|f| (f, String::new(), None)).collect();
    while let Some((o, prefix, inherited_ft)) = stack.pop() {
        if let Some(r) = o.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { continue };
        let part = text(doc, &d, b"T");
        let name = match (&part, prefix.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{prefix}.{p}"),
            (None, _) => prefix.clone(),
        };
        let ft = d.name(b"FT").map(<[u8]>::to_vec).or(inherited_ft);
        let kids: Vec<Object> = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        // Kids with a /T are fields; others are this field's widgets.
        let (sub, widgets): (Vec<Object>, Vec<Object>) = kids.into_iter().partition(|k| doc.resolve(k).as_dict().is_some_and(|kd| kd.contains(b"T")));
        for k in sub.into_iter().rev() {
            stack.push((k, name.clone(), ft.clone()));
        }
        if ft.as_deref() == Some(b"Sig") && part.is_some() {
            let mut ws: Vec<(Option<ObjRef>, Dict)> =
                widgets.iter().filter_map(|w| doc.resolve(w).as_dict().cloned().map(|wd| (w.as_ref(), wd))).collect();
            if d.name(b"Subtype") == Some(b"Widget") {
                ws.insert(0, (o.as_ref(), d.clone()));
            }
            out.push(Field { name, r: o.as_ref(), dict: d, widgets: ws });
        }
    }
    out
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

pub(super) fn nums(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    doc.resolve(d.get(key)?).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())
}

/// List and validate every signature field. `bytes` is the file as stored (the ranges index it).
pub fn list(doc: &Document, bytes: &[u8], trust: &TrustStore) -> Vec<SignatureInfo> {
    list_cached(doc, bytes, trust, &DigestCache::default())
}

/// [`list`], reusing digests and standalone-timestamp discovery from `cache`. Field/widget
/// structure and validation against `bytes` and `trust` are read again on every call.
pub fn list_cached(doc: &Document, bytes: &[u8], trust: &TrustStore, cache: &DigestCache) -> Vec<SignatureInfo> {
    let pages = annot_pages(doc);
    let mut out = Vec::new();
    let mut field_values: HashSet<ObjRef> = HashSet::new();
    for f in sig_fields(doc) {
        if let Some(v_ref) = f.dict.get(b"V").and_then(Object::as_ref) {
            field_values.insert(v_ref);
        }
        let widget = f.widgets.first();
        let page = widget.and_then(|(r, w)| r.and_then(|r| pages.get(&r).copied()).or_else(|| w.reference(b"P").and_then(|p| page_index(doc, p))));
        let rect = widget
            .and_then(|(_, w)| nums(doc, w, b"Rect"))
            .filter(|r| r.len() == 4)
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
        // Shown when it has a box, and its widget isn't Hidden (2) or NoView (32), as the forms
        // crate reads a widget's flags.
        let flags = widget.and_then(|(_, w)| w.get(b"F")).and_then(|o| doc.resolve(o).as_int()).unwrap_or(0);
        let visible = rect.is_some_and(|r| r[2] - r[0] > 0.5 && r[3] - r[1] > 0.5) && flags & (2 | 32) == 0;
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
            details: Vec::new(),
        };
        validate_doc_timestamp(doc, bytes, d, &mut info, cache, trust);
        out.push(info);
    }
    out
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
