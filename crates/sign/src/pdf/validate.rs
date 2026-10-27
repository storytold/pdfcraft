use super::changes::classify_changes;
use super::listing::{DigestCache, list, nums, sig_fields, text};
use super::types::{Detail, Fill, Modification, Status, TrustStore};
use super::*;

/// Validate one signature dictionary.
pub fn validate(doc: &Document, bytes: &[u8], trust: &TrustStore, field: &str) -> Option<SignatureInfo> {
    list(doc, bytes, trust).into_iter().find(|s| s.field == field)
}

fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    let val = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    digits.chunks(2).map(|p| Some(val(p[0])? << 4 | p.get(1).map_or(Some(0), |c| val(*c))?)).collect()
}

/// A document timestamp dictionary: `/SubFilter /ETSI.RFC3161`, typed `/DocTimeStamp`
/// (ISO 32000-2 §12.8.5) or, as older writers have it, `/Sig`.
pub(super) fn is_doc_timestamp(d: &Dict) -> bool {
    d.name(b"SubFilter") == Some(b"ETSI.RFC3161") && matches!(d.name(b"Type"), Some(b"DocTimeStamp" | b"Sig"))
}

/// The DocMDP permission level (`/P`, 1–3) of signature value `v`, or `None` for an approval
/// signature.
fn docmdp(doc: &Document, v: &Dict) -> Option<u8> {
    v.get(b"Reference")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|r| doc.resolve(&r).as_dict().cloned())
        .find(|r| r.name(b"TransformMethod") == Some(b"DocMDP"))
        .map(|r| r.get(b"TransformParams").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().and_then(|p| p.int(b"P"))).unwrap_or(2).clamp(1, 3) as u8)
}

pub(super) fn validate_into(doc: &Document, bytes: &[u8], trust: &TrustStore, v: &Dict, info: &mut SignatureInfo, cache: &DigestCache) {
    // A document timestamp can also be the value of a signature field (Documenso writes
    // `Timestamp_1` that way). Its `/Contents` is a bare RFC 3161 token, not a signature over the
    // document, so it must not be read as one.
    if is_doc_timestamp(v) {
        info.sub_filter = Some("ETSI.RFC3161".to_string());
        info.doc_timestamp = true;
        info.timestamp = true;
        validate_doc_timestamp(doc, bytes, v, info, cache, trust);
        return;
    }
    info.date = text(doc, v, b"M");
    info.reason = text(doc, v, b"Reason");
    info.location = text(doc, v, b"Location");
    info.contact = text(doc, v, b"ContactInfo");
    info.sub_filter = v.name(b"SubFilter").map(|s| String::from_utf8_lossy(s).into_owned());
    info.signer = text(doc, v, b"Name");
    info.certify = docmdp(doc, v);
    let invalid = |info: &mut SignatureInfo, why: Detail| {
        info.status = Status::Invalid;
        info.details.push(why);
    };
    // Byte ranges: [0 a b c] with the gap holding exactly the hex /Contents.
    let Some(br) = nums(doc, v, b"ByteRange").filter(|b| b.len() == 4 && b.iter().all(|x| *x >= 0.0)) else {
        return invalid(info, Detail::plain("The signature has no valid byte range."));
    };
    let [o0, l0, o1, l1] = [br[0] as usize, br[1] as usize, br[2] as usize, br[3] as usize];
    if o0 != 0 || o1 < l0 || o1.checked_add(l1).is_none_or(|end| end > bytes.len()) || o1 - l0 < 2 {
        return invalid(info, Detail::plain("The signature's byte range does not match the file."));
    }
    let gap = &bytes[l0..o1];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return invalid(info, Detail::plain("The signature's byte range does not exclude exactly its contents."));
    }
    let Some(contents) = unhex(&gap[1..gap.len() - 1]) else { return invalid(info, Detail::plain("The signature contents are not hexadecimal.")) };
    let covered = o1 + l1;
    info.signed_len = covered;
    // The signed revision's number: its cross-reference sections (1 for a reconstructed file).
    info.revision = cache.revision(doc, bytes, covered).map_or(1, |d| d.revisions().len().max(1));
    // What can't be checked is unknown, not invalid: only data that is wrong or fails its check
    // is "invalid".
    let unsupported = |info: &mut SignatureInfo, why: &str| {
        info.status = Status::Unknown;
        // What is not supported yet is named by the reader, in its own words.
        info.details.push(Detail::of("PdfCraft can't check this signature yet: {}.", Fill::Reason(why.to_owned())));
    };
    if info.sub_filter.as_deref() == Some("adbe.x509.rsa_sha1") {
        return validate_x509_rsa_sha1(doc, bytes, trust, v, info, cache, (l0, o1, covered), &contents);
    }
    let sd = match SignedData::parse(&contents) {
        Ok(sd) => sd,
        Err(SignError::Unsupported(e)) => return unsupported(info, &e),
        // What the reader made of it goes in as it stands: it is a developer's detail, not a sentence.
        Err(e) => return invalid(info, Detail::of("The signature could not be read ({}).", Fill::Reason(e.to_string()))),
    };
    let s = &sd.signer;
    // A quirk is a whole sentence the reader words for itself, so it goes on the list in `WORDINGS`.
    info.details.extend(sd.quirks.iter().copied().map(Detail::plain));
    info.digest = Some(s.digest);
    info.timestamp = s.timestamp;
    let mut unverified_time = None;
    if let Some(raw) = &s.timestamp_token {
        match crate::timestamp::parse_token(raw) {
            Ok(t) => {
                let imprint = t.digest.digest(&[&s.signature]);
                if imprint != t.imprint {
                    info.details.push(Detail::plain("The signature's timestamp token does not cover the signature value."));
                } else {
                    // Only a token from a trusted authority (valid when it stamped) is trusted
                    // time; otherwise its time is reported but validation uses the signer's
                    // own claimed time, as without a timestamp.
                    let mut pool = crate::timestamp::token_certs(raw);
                    pool.extend(trust.certs.iter().cloned());
                    let tsa = t.signer_certificate();
                    let trusted_tsa = tsa
                        .as_ref()
                        .is_some_and(|c| c.valid_at(t.gen_time) && build_chain(c, &pool, Some(t.gen_time)).iter().any(|x| trust.trusts(x)));
                    if !trusted_tsa {
                        unverified_time = Some(t.gen_time);
                    } else if tsa.as_ref().is_some_and(Certificate::may_timestamp) {
                        info.timestamp_time = Some(t.gen_time);
                    } else {
                        info.details.push(Detail::plain("The timestamp authority's certificate is not issued for time stamping."));
                    }
                }
            }
            Err(e) => info.details.push(Detail::of("The signature's timestamp token could not be verified ({}).", Fill::Reason(e.to_string()))),
        }
    }
    // The digest the signature commits to.
    let content_digest = match &sd.content {
        // adbe.pkcs7.sha1: the document's SHA-1 digest is the signed content.
        Some(c) => {
            if *c != cache.digest(DigestAlg::Sha1, bytes, l0, o1, covered) {
                return invalid(info, Detail::plain("The document has been altered or corrupted since the signature was applied."));
            }
            s.digest.digest(&[c])
        }
        None => cache.digest(s.digest, bytes, l0, o1, covered),
    };
    // With signed attributes the signature is made over them, not over the document: their
    // message digest is what ties it to the signed bytes, and their content type says what those
    // bytes are (data, for a document). RFC 5652 §5.3 requires both.
    if s.signed_attrs.is_some() {
        match s.content_type.as_deref() {
            Some(crate::cms::DATA) => {}
            Some(_) => {
                return invalid(
                    info,
                    Detail::plain("The signature's signed attributes name a content type other than data, so it is not a signature of the document."),
                );
            }
            None => {
                info.details.push(Detail::plain("The signature's signed attributes have no content type, which CMS requires; it was read anyway."))
            }
        }
        if s.message_digest.is_none() {
            return invalid(
                info,
                Detail::plain("The signature's signed attributes have no message digest, so the signature does not cover the document."),
            );
        }
    }
    // A PDF signature signs data: the document, or (adbe.pkcs7.sha1) its digest. A content-type
    // attribute was checked above, and `SignedData::parse` refuses one that differs from
    // `eContentType`; without that attribute, `eContentType` is the only place that says so.
    if sd.econtent_type != crate::cms::DATA {
        let why = SignError::Malformed("CMS: the encapsulated content is not data".into());
        return invalid(info, Detail::of("The signature could not be read ({}).", Fill::Reason(why.to_string())));
    }
    if let Some(md) = &s.message_digest
        && *md != content_digest
    {
        return invalid(info, Detail::plain("The document has been altered or corrupted since the signature was applied."));
    }
    let Some(cert) = sd.signer_certificate().cloned() else {
        return invalid(info, Detail::plain("The signer's certificate is not in the signature."));
    };
    info.algorithm = Some(format!("{} with {}", cert.public_key.describe(), s.scheme_digest.unwrap_or(s.digest).name()));
    info.signer = Some(cert.display_name());
    let mut notes = Vec::new();
    match sd.verify_signature_noting(&cert, &content_digest, &mut notes) {
        // A note is a whole sentence the reader words for itself, like a quirk.
        Ok(true) => info.details.extend(notes.into_iter().map(Detail::plain)),
        Ok(false) => {
            info.certificate = Some(cert);
            return invalid(info, Detail::plain("The signature value does not match the signer's certificate: the signature is corrupt."));
        }
        Err(SignError::Unsupported(e)) => {
            info.certificate = Some(cert);
            return unsupported(info, &e);
        }
        Err(e) => {
            info.certificate = Some(cert);
            return invalid(info, Detail::of("The signature could not be checked ({}).", Fill::Reason(e.to_string())));
        }
    }
    finish_validation(doc, bytes, trust, info, cache, covered, cert, &sd.certificates, s.signing_time, unverified_time);
}

/// The verdict once the signature value has been checked: chain and trust, the signing time
/// against the certificate's validity, and what later revisions changed.
#[allow(clippy::too_many_arguments)]
fn finish_validation(
    doc: &Document,
    bytes: &[u8],
    trust: &TrustStore,
    info: &mut SignatureInfo,
    cache: &DigestCache,
    covered: usize,
    cert: Certificate,
    embedded: &[Certificate],
    cms_time: Option<Time>,
    unverified_time: Option<Time>,
) {
    info.signing_time = cms_time.or_else(|| info.date.as_deref().and_then(Time::from_pdf));
    let mut pool = embedded.to_vec();
    pool.extend(trust.certs.iter().cloned());
    // The time the chain is judged at: trusted timestamp, else the signer's claimed time.
    let at = info.timestamp_time.or(info.signing_time);
    let (chain, refused) = build_chain_noted(&cert, &pool, at);
    let chain: Vec<Certificate> = chain.into_iter().cloned().collect();
    let trusted = chain.iter().any(|c| trust.trusts(c));
    info.chain = chain;
    info.certificate = Some(cert.clone());
    if let Some(why) = refused
        && !trusted
    {
        info.details.push(why);
    }
    // Revocation evidence embedded in the document security store, if any. A verified
    // revocation is final: later blocks must not soften the verdict.
    let signer = info.chain.first().cloned();
    let revocation = check_dss_revocation(doc, signer.as_ref(), &at, info);
    let revoked = revocation == DssRevocation::Revoked;
    // Changes after signing.
    info.modification = if covered == bytes.len() || bytes[covered..].iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
        Modification::None
    } else {
        // The permissions are the signed revision's: a later one can rewrite `/P` (#1788). This
        // signature is the one whose byte range ends that revision.
        let old = cache.revision(doc, bytes, covered);
        if let Some(old) = &old
            && let Some(v) = sig_fields(old)
                .iter()
                .filter_map(|f| f.signature(old))
                .find(|v| nums(old, v, b"ByteRange").is_some_and(|b| b.len() == 4 && b[2] + b[3] == covered as f64))
        {
            info.certify = docmdp(old, &v);
        }
        classify_changes(doc, old, info.certify)
    };
    let mut problems = false;
    // The changes are handed over as the words they are, not stitched into the sentence, so an
    // interface can say both in its own language.
    match &info.modification {
        Modification::None => info.details.push(Detail::plain("This document has not been modified since this signature was applied.")),
        Modification::Allowed(kinds) => info.details.push(Detail::of(
            "The document has been modified since this signature was applied, but the changes are permitted ({}).",
            Fill::Changes(kinds.clone()),
        )),
        Modification::Disallowed(kinds) => {
            let said = Detail::of(
                "The document has been altered since this signature was applied in ways it does not permit ({}).",
                Fill::Changes(kinds.clone()),
            );
            info.status = Status::Invalid;
            info.details.push(said);
            problems = true;
        }
    }
    if revocation == DssRevocation::Incomplete {
        if !problems {
            info.status = Status::Unknown;
        }
        problems = true;
    }
    if let Some(t) = info.timestamp_time.or(info.signing_time)
        && !cert.valid_at(t)
    {
        info.details.push(Detail::plain("The signer's certificate was not valid at the time of signing."));
        // A verified revocation already made the signature invalid; keep it that way.
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
        problems = true;
    }
    if let Some(why) = cert.signing_problem() {
        info.details.push(why);
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
        problems = true;
    }
    if !trusted {
        info.details.push(Detail::plain(
            "The signer's identity is unknown because it has not been included in your list of trusted certificates and none of its parent certificates are trusted certificates.",
        ));
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
    } else {
        info.details.push(Detail::plain("The signer's identity is valid."));
        if !problems && !revoked {
            info.status = Status::Valid;
        }
    }
    if info.timestamp {
        info.details.push(Detail::plain("The signature includes an embedded timestamp."));
    } else {
        info.details.push(Detail::plain("Signing time is from the clock on the signer's computer."));
    }
    if let Some(t) = info.timestamp_time {
        info.details
            .push(Detail::of("The embedded timestamp token is valid and its authority is trusted; trusted time is {}.", Fill::Value(t.to_string())));
    } else if let Some(t) = unverified_time {
        info.details.push(Detail::of(
            "The signature carries an unverified timestamp ({}): the timestamp authority is not in your list of trusted certificates, so the signing time claimed by the signer is used instead.",
            Fill::Value(t.to_string()),
        ));
    }
}

/// Distinct evidence streams read from one `/DSS` array.
pub(crate) const MAX_DSS_ENTRIES: usize = 1024;

/// Decoded bytes read from one `/DSS` array; real CRLs and OCSP responses are kilobytes each.
pub(crate) const MAX_DSS_BYTES: usize = 64 << 20;

/// The decoded streams of the `/DSS` evidence array `key` (`/CRLs`, `/OCSPs`), one at a time so
/// each is checked before the next decodes: each stream once and within the evidence size cap,
/// at most [`MAX_DSS_ENTRIES`] distinct ones and [`MAX_DSS_BYTES`] together. A stream past a cap
/// sets [`DssEvidence::cut`] and ends the iteration.
pub(crate) struct DssEvidence<'a> {
    doc: &'a Document,
    items: std::vec::IntoIter<Object>,
    seen: HashSet<ObjRef>,
    taken: usize,
    left: usize,
    /// A cap stopped the array being read in full.
    pub(crate) cut: bool,
}

impl<'a> DssEvidence<'a> {
    pub(crate) fn new(doc: &'a Document, dss: &Dict, key: &[u8]) -> Self {
        let items = dss.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        DssEvidence { doc, items: items.into_iter(), seen: HashSet::new(), taken: 0, left: MAX_DSS_BYTES, cut: false }
    }
}

impl Iterator for DssEvidence<'_> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Vec<u8>> {
        if self.cut {
            return None;
        }
        // Repeats are skipped before the caps count anything, so repeating one stream cannot use
        // them up; the scan of the array itself is bounded by the parser's cap on array length.
        for item in self.items.by_ref() {
            if item.as_ref().is_some_and(|r| !self.seen.insert(r)) {
                continue;
            }
            let o = self.doc.resolve(&item);
            let Object::Stream(s) = o.as_ref() else { continue };
            if self.taken >= MAX_DSS_ENTRIES {
                self.cut = true;
                return None;
            }
            self.taken += 1;
            // A stream past the evidence cap would not parse as evidence either.
            let Ok(bytes) = s.decoded_within(crate::revocation::MAX_EVIDENCE) else { continue };
            let Some(left) = self.left.checked_sub(bytes.len()) else {
                self.cut = true;
                return None;
            };
            self.left = left;
            return Some(bytes);
        }
        None
    }
}

/// What the catalog's `/DSS` says about the signer's certificate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DssRevocation {
    /// Verified evidence revokes it.
    Revoked,
    /// A cap stopped the evidence being read in full, and what was read does not revoke it.
    Incomplete,
    /// All of the evidence was read, and none of it revokes it.
    NotRevoked,
}

/// Check the revocation evidence in the catalog's `/DSS` against the signer's chain at `at`.
/// Only evidence whose signature verifies against the issuer counts; returns whether the
/// certificate is revoked (which invalidates the signature), or whether a cap kept the check from
/// reading all of the evidence.
fn check_dss_revocation(doc: &Document, signer: Option<&Certificate>, at: &Option<Time>, info: &mut SignatureInfo) -> DssRevocation {
    use crate::revocation::{CertificateList, OcspResponse, RevocationStatus};
    let (Some(signer), Some(at)) = (signer, *at) else { return DssRevocation::NotRevoked };
    let Some(root) = doc.root() else { return DssRevocation::NotRevoked };
    let Some(dss) = doc.get(root).as_dict().and_then(|c| c.get(b"DSS").cloned()).map(|d| doc.resolve(&d)).and_then(|d| d.as_dict().cloned()) else {
        return DssRevocation::NotRevoked;
    };
    let issuer = info.chain.get(1).unwrap_or(signer);
    // A verified revocation wins; "good" from any covering evidence is reported, but never
    // contradicts a revocation.
    let mut revoked = None;
    let mut covered = false;
    let mut sources = Vec::new();
    let mut crls = DssEvidence::new(doc, &dss, b"CRLs");
    for r in crls.by_ref() {
        let Ok(list) = CertificateList::parse(&r) else { continue };
        sources.push("CRL");
        match list.check(signer, issuer, at) {
            RevocationStatus::Revoked { at } => {
                revoked = Some(at);
                break;
            }
            RevocationStatus::Good => covered = true,
            RevocationStatus::Unknown => {}
        }
    }
    let mut cut = crls.cut;
    if revoked.is_none() {
        let mut ocsps = DssEvidence::new(doc, &dss, b"OCSPs");
        for r in ocsps.by_ref() {
            let Ok(resp) = OcspResponse::parse(&r) else { continue };
            sources.push("OCSP");
            match resp.check(signer, issuer, at) {
                RevocationStatus::Revoked { at } => {
                    revoked = Some(at);
                    break;
                }
                RevocationStatus::Good => covered = true,
                RevocationStatus::Unknown => {}
            }
        }
        cut |= ocsps.cut;
    }
    match revoked {
        Some(at) => {
            info.status = Status::Invalid;
            // The evidence and the date go in together: one hole, so a reader can put the
            // sentence's parts in whatever order its language wants.
            info.details.push(Detail::of("The signer's certificate has been revoked ({}).", Fill::Value(format!("{}, {at}", sources.join(", ")))));
            DssRevocation::Revoked
        }
        // Evidence past a cap may revoke the certificate, so a partial check proves nothing.
        None if cut => {
            info.details.push(Detail::of(
                "The signature could not be checked ({}).",
                Fill::Reason("the /DSS holds more revocation evidence than PdfCraft reads".into()),
            ));
            DssRevocation::Incomplete
        }
        None if covered => {
            info.details
                .push(Detail::plain("The document's embedded revocation information shows that the signer's certificate has not been revoked."));
            DssRevocation::NotRevoked
        }
        None => DssRevocation::NotRevoked,
    }
}

/// Validate a standalone document timestamp dictionary (`/ETSI.RFC3161`): the token must be
/// cryptographically valid and its message imprint must cover the file's signed bytes.
pub(super) fn validate_doc_timestamp(doc: &Document, bytes: &[u8], v: &Dict, info: &mut SignatureInfo, cache: &DigestCache, trust: &TrustStore) {
    info.date = text(doc, v, b"M");
    let invalid = |info: &mut SignatureInfo, why: Detail| {
        info.status = Status::Invalid;
        info.details.push(why);
    };
    let Some(br) = nums(doc, v, b"ByteRange").filter(|b| b.len() == 4 && b.iter().all(|x| *x >= 0.0)) else {
        return invalid(info, Detail::plain("The timestamp has no valid byte range."));
    };
    let [o0, l0, o1, l1] = [br[0] as usize, br[1] as usize, br[2] as usize, br[3] as usize];
    if o0 != 0 || o1 < l0 || o1.checked_add(l1).is_none_or(|end| end > bytes.len()) || o1 - l0 < 2 {
        return invalid(info, Detail::plain("The timestamp's byte range does not match the file."));
    }
    let gap = &bytes[l0..o1];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return invalid(info, Detail::plain("The timestamp's byte range does not exclude exactly its contents."));
    }
    let Some(contents) = unhex(&gap[1..gap.len() - 1]) else {
        return invalid(info, Detail::plain("The timestamp contents are not hexadecimal."));
    };
    let covered = o1 + l1;
    info.signed_len = covered;
    info.revision = cache.revision(doc, bytes, covered).map_or(1, |d| d.revisions().len().max(1));
    let token = match crate::timestamp::parse_token(&contents) {
        Ok(t) => t,
        Err(e) => return invalid(info, Detail::of("The timestamp token could not be read ({}).", Fill::Reason(e.to_string()))),
    };
    info.digest = Some(token.digest);
    info.timestamp_time = Some(token.gen_time);
    info.algorithm = Some(format!("RFC 3161 timestamp ({})", token.digest.name()));
    let doc_digest = token.digest.digest(&[&bytes[..l0], &bytes[o1..covered]]);
    if doc_digest != token.imprint {
        return invalid(info, Detail::plain("The document has been altered or corrupted since the timestamp was applied."));
    }
    if let Some(cert) = token.signer_certificate() {
        info.signer = Some(cert.display_name());
        info.certificate = Some(cert.clone());
        if !cert.valid_at(token.gen_time) {
            info.status = Status::Unknown;
            info.details.push(Detail::plain("The timestamp authority's certificate was not valid at the time of timestamping."));
        }
        if !cert.may_timestamp() {
            info.status = Status::Unknown;
            info.details.push(Detail::plain("The timestamp authority's certificate is not issued for time stamping."));
        }
    }
    info.modification = if covered == bytes.len() || bytes[covered..].iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
        Modification::None
    } else {
        classify_changes(doc, cache.revision(doc, bytes, covered), None)
    };
    match &info.modification {
        Modification::None => info.details.push(Detail::plain("This document has not been modified since this timestamp was applied.")),
        Modification::Allowed(kinds) => info.details.push(Detail::of(
            "The document has been modified since this timestamp was applied, but the changes are permitted ({}).",
            Fill::Changes(kinds.clone()),
        )),
        Modification::Disallowed(kinds) => {
            info.status = Status::Invalid;
            info.details.push(Detail::of(
                "The document has been altered since this timestamp was applied in ways it does not permit ({}).",
                Fill::Changes(kinds.clone()),
            ));
        }
    }
    if info.status != Status::Invalid {
        // An untrusted TSA is not proof of anything: the verdict stays Unknown, like an
        // ordinary signature from an unknown signer.
        let mut pool = crate::timestamp::token_certs(&contents);
        pool.extend(trust.certs.iter().cloned());
        // A trusted authority whose certificate was not valid at the time it stamped proves
        // nothing either (its detail is already recorded above): never upgrade that to Valid.
        // Nor does one whose certificate is not issued for time stamping.
        let may_stamp = token.signer_certificate().is_some_and(|c| c.valid_at(token.gen_time) && c.may_timestamp());
        let trusted_tsa = token.signer_certificate().is_some_and(|c| build_chain(&c, &pool, Some(token.gen_time)).iter().any(|x| trust.trusts(x)));
        if trusted_tsa && may_stamp {
            info.status = Status::Valid;
            info.details.push(Detail::plain("The timestamp token is valid and its authority is trusted."));
        } else if trusted_tsa {
            info.status = Status::Unknown;
        } else {
            info.status = Status::Unknown;
            info.details
                .push(Detail::plain("The timestamp token is valid, but the timestamp authority is not in your list of trusted certificates."));
        }
    }
}

/// The DER `/Contents` of every signature dictionary in the file (field signatures and
/// document timestamps), for `/VRI` keys and evidence selection.
pub(crate) fn signature_contents(doc: &Document) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut add = |d: &Dict, r: Option<ObjRef>| {
        if let Some(r) = r
            && !seen.insert(r)
        {
            return;
        }
        let contents = d.get(b"Contents").map(|c| doc.resolve(c));
        let Some(s) = contents.as_deref().and_then(Object::as_string) else { return };
        // Hex strings decode to the DER when parsed; a literal string carries hex text.
        let der = if s.hex { Some(s.bytes.clone()) } else { unhex(&s.bytes) };
        if let Some(der) = der {
            out.push(der);
        }
    };
    for f in sig_fields(doc) {
        if let Some(v) = f.dict.get(b"V") {
            let d = doc.resolve(v);
            if let Some(dd) = d.as_dict() {
                add(dd, v.as_ref());
            }
        }
    }
    for (r, o) in doc.scan_objects() {
        let Some(d) = o.as_dict() else { continue };
        if is_doc_timestamp(d) {
            add(d, Some(r));
        }
    }
    out
}

/// `adbe.x509.rsa_sha1` (ISO 32000-2 §12.8.3.2, deprecated): `/Contents` is the PKCS #1
/// signature as a DER OCTET STRING over the SHA-1 digest of the byte ranges, and `/Cert` holds the
/// signer's certificate (then any others) as DER byte strings.
#[allow(clippy::too_many_arguments)]
fn validate_x509_rsa_sha1(
    doc: &Document,
    bytes: &[u8],
    trust: &TrustStore,
    v: &Dict,
    info: &mut SignatureInfo,
    cache: &DigestCache,
    (l0, o1, covered): (usize, usize, usize),
    contents: &[u8],
) {
    let fail = |info: &mut SignatureInfo, why: Detail| {
        info.status = Status::Invalid;
        info.details.push(why);
    };
    let certs: Vec<Certificate> = match v.get(b"Cert").map(|c| doc.resolve(c)).as_deref() {
        Some(Object::Array(a)) => a.iter().filter_map(|c| doc.resolve(c).as_string().map(|s| s.bytes.clone())).collect::<Vec<_>>(),
        Some(Object::String(s)) => vec![s.bytes.clone()],
        _ => Vec::new(),
    }
    .iter()
    .filter_map(|raw| Certificate::parse(raw).ok())
    .collect();
    let Some(cert) = certs.first().cloned() else { return fail(info, Detail::plain("The signer's certificate is not in the signature.")) };
    let signature = match crate::der::Tlv::parse_ber(contents).and_then(|(t, _)| t.octets("signature").map(|o| o.into_owned())) {
        Ok(sig) => sig,
        // What the reader made of it goes in as it stands: it is a developer's detail, not a sentence.
        Err(e) => return fail(info, Detail::of("The signature could not be read ({}).", Fill::Reason(e.to_string()))),
    };
    info.digest = Some(DigestAlg::Sha1);
    info.algorithm = Some(format!("{} with SHA-1", cert.public_key.describe()));
    info.signer = Some(cert.display_name());
    let digest = cache.digest(DigestAlg::Sha1, bytes, l0, o1, covered);
    let mut notes = Vec::new();
    match cert.public_key.verify_noting(crate::keys::Scheme::RsaPkcs1, DigestAlg::Sha1, &digest, &signature, &mut notes) {
        // A note is a whole sentence the reader words for itself, so it goes on the list in `WORDINGS`.
        Ok(true) => info.details.extend(notes.into_iter().map(Detail::plain)),
        Ok(false) => {
            info.certificate = Some(cert);
            return fail(info, Detail::plain("The document has been altered or corrupted since the signature was applied."));
        }
        // Only what PdfCraft doesn't support yet is "can't check"; a key or value that is
        // malformed is a signature that doesn't verify (as on the CMS path).
        Err(SignError::Unsupported(e)) => {
            info.certificate = Some(cert);
            info.status = Status::Unknown;
            // What is not supported yet is named by the reader, in its own words.
            info.details.push(Detail::of("PdfCraft can't check this signature yet: {}.", Fill::Reason(e.to_string())));
            return;
        }
        Err(e) => {
            info.certificate = Some(cert);
            return fail(info, Detail::of("The signature could not be verified ({}).", Fill::Reason(e.to_string())));
        }
    }
    info.details.push(Detail::plain("The signature uses the legacy adbe.x509.rsa_sha1 format."));
    info.details.extend(DigestAlg::Sha1.weakness().map(Detail::plain));
    finish_validation(doc, bytes, trust, info, cache, covered, cert, &certs, None, None);
}
