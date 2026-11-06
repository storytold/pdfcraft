//! The Document Security Store (ISO 32000-2 §12.8.2.3, formerly A.6.5): a `/DSS` dictionary in
//! the catalog holds certificates and revocation evidence (OCSP responses, CRLs) so signatures
//! stay verifiable offline (PAdES B-LT), and `/VRI` indexes the evidence per signature.
//!
//! Evidence is caller-supplied DER for now: validating OCSP responses and CRLs themselves is
//! the remaining piece of the LTV plan. Embedding never rewrites signed bytes — it appends an
//! incremental update, which the change classifier treats as a permitted "document security
//! store" change.

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream};

use crate::SignError;

/// Revocation evidence to embed, as DER blobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    pub certs: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
}

/// Uppercase-hex SHA-1 of a signature's `/Contents` — the `/VRI` key convention.
fn vri_key(contents: &[u8]) -> String {
    crate::keys::DigestAlg::Sha1.digest(&[contents]).iter().map(|b| format!("{b:02X}")).collect()
}

/// Merge `evidence` into the catalog's `/DSS` (deduplicating byte-identical blobs against any
/// existing store), add `/VRI` entries for every signature in the file, and return the file
/// written incrementally.
pub fn embed(doc: &Document, evidence: &Evidence) -> Result<Vec<u8>, SignError> {
    let mut doc = doc.clone();
    let root = doc.root().ok_or_else(|| SignError::Pdf("the document has no catalog".into()))?;
    let existing = doc.get(root).as_dict().and_then(|c| c.get(b"DSS").cloned()).map(|d| doc.resolve(&d)).and_then(|d| d.as_dict().cloned());

    // What is already in the store, so the update only appends new bytes.
    let mut certs: Vec<Vec<u8>> = Vec::new();
    let mut ocsps: Vec<Vec<u8>> = Vec::new();
    let mut crls: Vec<Vec<u8>> = Vec::new();
    if let Some(d) = &existing {
        let mut budget = MAX_EXISTING_EVIDENCE;
        pull(&doc, d, b"Certs", &mut certs, &mut budget)?;
        pull(&doc, d, b"OCSPs", &mut ocsps, &mut budget)?;
        pull(&doc, d, b"CRLs", &mut crls, &mut budget)?;
    }
    let add = |list: &mut Vec<Vec<u8>>, blob: &[u8]| {
        if !list.iter().any(|b| b.as_slice() == blob) {
            list.push(blob.to_vec());
        }
    };
    for c in &evidence.certs {
        add(&mut certs, c);
    }
    for o in &evidence.ocsps {
        add(&mut ocsps, o);
    }
    for c in &evidence.crls {
        add(&mut crls, c);
    }
    if certs.is_empty() && ocsps.is_empty() && crls.is_empty() {
        return Err(SignError::Pdf("no evidence to embed".into()));
    }
    // New objects for everything we are adding; VRI points at the union.
    let cert_refs: Vec<Object> = certs.iter().map(|c| Object::Ref(embed_stream(&mut doc, c))).collect();
    let ocsp_refs: Vec<Object> = ocsps.iter().map(|o| Object::Ref(embed_stream(&mut doc, o))).collect();
    let crl_refs: Vec<Object> = crls.iter().map(|c| Object::Ref(embed_stream(&mut doc, c))).collect();

    let mut dss = Dict::new();
    dss.set(b"Type".to_vec(), Object::name("DSS"));
    dss.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    dss.set(b"Certs".to_vec(), Object::Ref(doc.add(Object::Array(cert_refs.clone()))));
    if !ocsp_refs.is_empty() {
        dss.set(b"OCSPs".to_vec(), Object::Ref(doc.add(Object::Array(ocsp_refs.clone()))));
    }
    if !crl_refs.is_empty() {
        dss.set(b"CRLs".to_vec(), Object::Ref(doc.add(Object::Array(crl_refs.clone()))));
    }
    // Keep any existing /VRI entries, then cover every signature in the file.
    let mut vri =
        existing.as_ref().and_then(|d| d.get(b"VRI").cloned()).map(|v| doc.resolve(&v)).and_then(|v| v.as_dict().cloned()).unwrap_or_default();
    for contents in crate::pdf::signature_contents(&doc) {
        let mut e = Dict::new();
        e.set(b"Type".to_vec(), Object::name("VRI"));
        e.set(b"Cert".to_vec(), Object::Array(cert_refs.clone()));
        if !ocsp_refs.is_empty() {
            e.set(b"OCSP".to_vec(), Object::Array(ocsp_refs.clone()));
        }
        if !crl_refs.is_empty() {
            e.set(b"CRL".to_vec(), Object::Array(crl_refs.clone()));
        }
        vri.set(vri_key(&contents).into_bytes(), Object::Dict(e));
    }
    if !vri.is_empty() {
        dss.set(b"VRI".to_vec(), Object::Dict(vri));
    }
    let dss_ref = doc.add(Object::Dict(dss));
    doc.update_dict(root, |c| c.set(b"DSS".to_vec(), Object::Ref(dss_ref)))?;
    // What the user removed again before this (a cleared redaction mark) stays out of the file,
    // as in a save; the store and everything in it are reachable from the catalog.
    Ok(pdfcraft_cos::write_incremental(&doc, &SaveOptions { drop_unlinked: true, ..SaveOptions::default() })?)
}

/// Store one DER blob as a `/Type /Embed` stream.
fn embed_stream(doc: &mut Document, blob: &[u8]) -> pdfcraft_cos::ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Embed"));
    doc.add(Object::Stream(Stream::from_raw(d, blob.to_vec())))
}

/// Most decoded bytes of the store's existing evidence one [`embed`] reads. The store is rewritten
/// as a union, so what is there has to be read; a file whose evidence is larger than this is
/// refused rather than extended, never silently trimmed.
const MAX_EXISTING_EVIDENCE: usize = 256 << 20;

/// Pull the raw bytes of an existing DSS array into `into`, each stream once (however often the
/// array lists it) and each distinct content once, spending the decoded size from `budget`.
fn pull(doc: &Document, dss: &Dict, key: &[u8], into: &mut Vec<Vec<u8>>, budget: &mut usize) -> Result<(), SignError> {
    let Some(a) = dss.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()) else {
        return Ok(());
    };
    let mut seen_refs = std::collections::HashSet::new();
    let mut seen_content: std::collections::HashSet<Vec<u8>> = into.iter().map(|b| crate::keys::DigestAlg::Sha256.digest(&[b.as_slice()])).collect();
    for item in a {
        if item.as_ref().is_some_and(|r| !seen_refs.insert(r)) {
            continue;
        }
        let o = doc.resolve(&item);
        let Object::Stream(s) = &*o else { continue };
        let too_big = || SignError::Pdf("the document's existing validation data is too large to extend".into());
        match s.decoded_within(*budget) {
            Ok(bytes) => {
                *budget = budget.checked_sub(bytes.len()).ok_or_else(too_big)?;
                if seen_content.insert(crate::keys::DigestAlg::Sha256.digest(&[bytes.as_slice()])) {
                    into.push(bytes);
                }
            }
            // The allowance is spent: refuse, rather than drop evidence the store holds.
            Err(e) if e.to_string().contains("exceeds the limit") => return Err(too_big()),
            // Not decodable at all: skipped, as before.
            Err(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(blobs: &[&[u8]], listed: &[usize]) -> (Document, Dict) {
        let mut doc = Document::new_empty();
        let refs: Vec<_> = blobs.iter().map(|b| doc.add(Object::Stream(Stream::from_raw(Dict::new(), b.to_vec())))).collect();
        let mut dss = Dict::new();
        dss.set(b"Certs".to_vec(), Object::Array(listed.iter().map(|i| Object::Ref(refs[*i])).collect()));
        (doc, dss)
    }

    #[test]
    fn a_stream_listed_many_times_is_read_once_and_the_decoded_size_is_spent() {
        let (doc, dss) = store(&[&[1; 100], &[1; 100], &[2; 50]], &[0, 0, 0, 1, 2, 2]);
        let mut out = Vec::new();
        let mut budget = 1000;
        pull(&doc, &dss, b"Certs", &mut out, &mut budget).unwrap();
        // Two distinct streams are listed with the same bytes: one copy; the repeats were not read.
        assert_eq!(out, [vec![1; 100], vec![2; 50]]);
        assert_eq!(budget, 1000 - 100 - 100 - 50);
        // Past the allowance the store is refused, not trimmed.
        let mut budget = 120;
        assert!(pull(&doc, &dss, b"Certs", &mut Vec::new(), &mut budget).is_err());
    }
}
