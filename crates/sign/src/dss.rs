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
        pull(&doc, d, b"Certs", &mut certs)?;
        pull(&doc, d, b"OCSPs", &mut ocsps)?;
        pull(&doc, d, b"CRLs", &mut crls)?;
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
    Ok(pdfcraft_cos::write_incremental(&doc, &SaveOptions::default())?)
}

/// Store one DER blob as a `/Type /Embed` stream.
fn embed_stream(doc: &mut Document, blob: &[u8]) -> pdfcraft_cos::ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Embed"));
    doc.add(Object::Stream(Stream::from_raw(d, blob.to_vec())))
}

/// Pull the raw bytes of an existing DSS array into `into`.
fn pull(doc: &Document, dss: &Dict, key: &[u8], into: &mut Vec<Vec<u8>>) -> Result<(), SignError> {
    let Some(a) = dss.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()) else {
        return Ok(());
    };
    for item in a {
        let o = doc.resolve(&item);
        let Object::Stream(s) = &*o else { continue };
        if let Ok(bytes) = s.decoded()
            && !into.iter().any(|b| b == &bytes)
        {
            into.push(bytes);
        }
    }
    Ok(())
}
