//! Chain building: which certificates may vouch for which (RFC 5280 §6.1, the parts that matter
//! for trust). Certificates are issued in-process with chosen extensions, so every refusal is
//! tested on its own.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::der::{self, Time, tag};
use pdfcraft_sign::pkcs12::DigitalId;
use pdfcraft_sign::x509::{Certificate, Name, build_chain, build_chain_noted};
use pdfcraft_sign::{PrivateKey, SignOptions, Status, TrustStore, signatures};

fn time(year: u32) -> Time {
    Time { year, month: 6, day: 1, hour: 12, minute: 0, second: 0 }
}

fn ext(oid: &str, value: &[u8]) -> Vec<u8> {
    der::seq(&[&der::oid(oid), &der::boolean(true), &der::octets(value)])
}

/// basicConstraints: `cA` (omitted when false, as DER does) and an optional `pathLenConstraint`.
fn basic_constraints(ca: bool, path_len: Option<u64>) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    if ca {
        parts.push(der::boolean(true));
    }
    if let Some(n) = path_len {
        parts.push(der::int(n));
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    ext("2.5.29.19", &der::seq(&refs))
}

const DIGITAL_SIGNATURE: u8 = 0x80;
const KEY_CERT_SIGN: u8 = 0x04;

fn key_usage(bits: u8) -> Vec<u8> {
    ext("2.5.29.15", &der::tlv(tag::BIT_STRING, &[1, bits]))
}

struct Party {
    name: Name,
    key: PrivateKey,
    cert: Certificate,
}

fn name(cn: &str) -> Name {
    Name::build(cn, "", "Chain Tests", "", "US")
}

/// A certificate for a fresh key, signed by `issuer` (or by itself).
fn issue(cn: &str, issuer: Option<&Party>, valid: (u32, u32), extensions: &[Vec<u8>]) -> Party {
    let key = PrivateKey::generate_p256().unwrap();
    let name = name(cn);
    let signer = issuer.map_or(&key, |i| &i.key);
    let issuer_name = issuer.map_or(&name, |i| &i.name);
    let cert = certify(&name, &key, (signer, issuer_name), 1 + cn.len() as u8, valid, extensions);
    Party { name, key, cert }
}

/// A certificate naming `subject` for `key`'s public key, signed by `signer` as `issuer_name`.
fn certify(
    subject: &Name,
    key: &PrivateKey,
    (signer, issuer_name): (&PrivateKey, &Name),
    serial: u8,
    valid: (u32, u32),
    extensions: &[Vec<u8>],
) -> Certificate {
    let alg = signer.preferred_digest();
    let sig_alg = signer.signature_algorithm(alg);
    let exts: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let mut fields: Vec<Vec<u8>> = Vec::new();
    // v3 when there are extensions; with none, a v1 certificate (no version field), as old roots.
    if !exts.is_empty() {
        fields.push(der::explicit(0, &der::int(2)));
    }
    fields.extend([
        der::uint(&[serial]),
        sig_alg.clone(),
        issuer_name.raw.clone(),
        der::seq(&[&time(valid.0).encode(), &time(valid.1).encode()]),
        subject.raw.clone(),
        key.public_key().spki(),
    ]);
    if !exts.is_empty() {
        fields.push(der::explicit(3, &der::seq(&exts)));
    }
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    let tbs = der::seq(&refs);
    let signature = signer.sign(alg, &tbs).unwrap();
    Certificate::parse(&der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)])).unwrap()
}

const FOREVER: (u32, u32) = (2020, 2040);

fn root() -> Party {
    issue("Test Root", None, FOREVER, &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN)])
}

fn ca(cn: &str, under: &Party, path_len: Option<u64>) -> Party {
    issue(cn, Some(under), FOREVER, &[basic_constraints(true, path_len), key_usage(KEY_CERT_SIGN)])
}

fn end_entity(cn: &str, under: &Party) -> Party {
    issue(cn, Some(under), FOREVER, &[basic_constraints(false, None), key_usage(DIGITAL_SIGNATURE)])
}

fn names(chain: &[&Certificate]) -> Vec<String> {
    chain.iter().map(|c| c.subject.common_name().unwrap_or_default().to_string()).collect()
}

#[test]
fn a_proper_chain_is_built() {
    let r = root();
    let i = ca("Intermediate", &r, None);
    let l = end_entity("Leaf", &i);
    let pool = [i.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Intermediate", "Test Root"]);
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2026)))), ["Leaf", "Intermediate", "Test Root"]);
}

#[test]
fn an_end_entity_certificate_cannot_issue() {
    let r = root();
    let e = end_entity("Ordinary Subscriber", &r);
    // A certificate its holder made with the end-entity key.
    let forged = end_entity("Bank AG", &e);
    let pool = [e.cert.clone(), r.cert.clone()];
    let (chain, why) = build_chain_noted(&forged.cert, &pool, None);
    assert_eq!(names(&chain), ["Bank AG"], "the subscriber certificate does not vouch for what it signed");
    assert!(why.is_some_and(|w| w.message.contains("not a CA")), "the reason is reported");
    // Neither does one without basicConstraints (not a v1 root: it has an issuer).
    let plain = issue("No Constraints", Some(&r), FOREVER, &[]);
    let under_plain = end_entity("Under Plain", &plain);
    let pool = [plain.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&under_plain.cert, &pool, None)), ["Under Plain"]);
    // CA:TRUE does not help when keyUsage leaves out keyCertSign…
    let signing_only = issue("Signing Only CA", Some(&r), FOREVER, &[basic_constraints(true, None), key_usage(DIGITAL_SIGNATURE)]);
    let below = end_entity("Below Signing Only", &signing_only);
    let pool = [signing_only.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&below.cert, &pool, None)), ["Below Signing Only"]);
    // …and a CA with no keyUsage at all is fine.
    let any_usage = issue("Any Usage CA", Some(&r), FOREVER, &[basic_constraints(true, None)]);
    let below = end_entity("Below Any Usage", &any_usage);
    let pool = [any_usage.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&below.cert, &pool, None)), ["Below Any Usage", "Any Usage CA", "Test Root"]);
}

#[test]
fn a_path_length_constraint_limits_the_cas_below() {
    let r = root();
    let i1 = ca("Constrained", &r, Some(0));
    let i2 = ca("Sub CA", &i1, None);
    let l = end_entity("Leaf", &i2);
    let pool = [i1.cert.clone(), i2.cert.clone(), r.cert.clone()];
    // pathLen 0: no CA may follow it, so Sub CA's issuer is refused and the chain stops there.
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Sub CA"]);
    // pathLen 0 directly above the leaf is fine, and pathLen 1 allows one CA below.
    let direct = end_entity("Direct Leaf", &i1);
    assert_eq!(names(&build_chain(&direct.cert, &pool, None)), ["Direct Leaf", "Constrained", "Test Root"]);
    let i1 = ca("Constrained 1", &r, Some(1));
    let i2 = ca("Sub CA 1", &i1, None);
    let l = end_entity("Leaf 1", &i2);
    let pool = [i1.cert.clone(), i2.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf 1", "Sub CA 1", "Constrained 1", "Test Root"]);
}

#[test]
fn issuers_must_have_been_valid_when_it_matters() {
    let r = root();
    let old = issue("Short Lived CA", Some(&r), (2020, 2022), &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN)]);
    let l = end_entity("Leaf", &old);
    let pool = [old.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2021)))), ["Leaf", "Short Lived CA", "Test Root"]);
    let (chain, why) = build_chain_noted(&l.cert, &pool, Some(time(2023)));
    assert_eq!(names(&chain), ["Leaf"], "expired by then");
    assert!(why.is_some_and(|w| w.message.contains("not valid")));
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2019)))), ["Leaf"], "not yet valid");
    // With no time to judge by, nothing is refused on validity.
    assert_eq!(names(&build_chain(&l.cert, &pool, None)).len(), 3);
}

#[test]
fn an_old_v1_self_signed_root_without_constraints_still_anchors() {
    // No extensions at all: basicConstraints absent, as in v1 roots.
    let r = issue("Old V1 Root", None, FOREVER, &[]);
    assert!(r.cert.is_self_signed() && !r.cert.has_basic_constraints && r.cert.may_issue());
    let i = ca("Under V1", &r, None);
    let l = end_entity("Leaf", &i);
    let pool = [i.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Under V1", "Old V1 Root"]);
    // A self-signed certificate that says CA:FALSE is not a root that can issue.
    let not_a_ca = issue("Self Signed Leaf", None, FOREVER, &[basic_constraints(false, None)]);
    assert!(!not_a_ca.cert.may_issue());
}

fn fixture() -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/openssl-signed.pdf", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Sign a small PDF with `leaf`, embedding `embedded`, and validate it trusting only `anchor`.
fn verdict(leaf: Party, embedded: Vec<Certificate>, anchor: &Certificate) -> (Status, Vec<String>) {
    let id = DigitalId { key: leaf.key, certificate: leaf.cert, chain: embedded, friendly_name: None };
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let opts = SignOptions { page: 0, rect: None, date: "D:20260601120000Z".into(), ..SignOptions::default() };
    let signed = pdfcraft_sign::sign(&doc, &id, &opts).unwrap();
    let trust = TrustStore { certs: vec![anchor.clone()] };
    let s = signatures(&Document::open(Arc::new(signed.clone())).unwrap(), &signed, &trust).into_iter().rfind(|s| s.signed).unwrap();
    // The details as they read in English; what an interface says in another language is checked in
    // `tests/it/pdf.rs`.
    (s.status, s.details.iter().map(|d| d.message.clone()).collect())
}

#[test]
fn a_signature_chaining_through_an_ordinary_certificate_is_not_trusted() {
    let r = root();
    // The root vouches for a CA, which vouches for the signer: valid.
    let i = ca("Issuing CA", &r, None);
    let l = end_entity("Honest Signer", &i);
    let (status, details) = verdict(l, vec![i.cert.clone()], &r.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
    // The root vouched for an ordinary subscriber, who signed the "signer" certificate with the
    // subscriber's own key and embedded it: the root must not vouch for that.
    let subscriber = end_entity("Ordinary Subscriber", &r);
    let forged = end_entity("Forged Identity", &subscriber);
    let (status, details) = verdict(forged, vec![subscriber.cert.clone()], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("not a CA")), "{details:?}");
}

#[test]
fn a_v3_self_signed_certificate_without_constraints_is_not_a_root_that_issues() {
    // A self-signed v3 ID with no basicConstraints and no key usage (only a key identifier): it
    // may sign documents, but it may not vouch for anyone else.
    let id = issue("Self Signed V3 ID", None, FOREVER, &[ext("2.5.29.14", &der::octets(&[1, 2, 3, 4]))]);
    assert!(id.cert.is_self_signed() && !id.cert.has_basic_constraints && id.cert.version == 2);
    assert!(!id.cert.may_issue());
    let l = end_entity("Leaf", &id);
    assert_eq!(names(&build_chain(&l.cert, std::slice::from_ref(&id.cert), None)), ["Leaf"]);
}

const KEY_ENCIPHERMENT: u8 = 0x20;
const SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";

fn extended_key_usage(oids: &[&str]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = oids.iter().map(|o| der::oid(o)).collect();
    let refs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    der::seq(&[&der::oid("2.5.29.37"), &der::octets(&der::seq(&refs))])
}

fn signer_with(cn: &str, under: &Party, extensions: &[Vec<u8>]) -> Party {
    let mut all = vec![basic_constraints(false, None)];
    all.extend_from_slice(extensions);
    issue(cn, Some(under), FOREVER, &all)
}

#[test]
fn the_signer_certificate_must_allow_document_signing() {
    let r = root();
    let problem = |exts: &[Vec<u8>]| signer_with("Signer", &r, exts).cert.signing_problem();
    // Key usage without digitalSignature or nonRepudiation.
    assert!(problem(&[key_usage(KEY_CERT_SIGN)]).is_some_and(|w| w.message.contains("key usage")));
    assert!(problem(&[key_usage(KEY_ENCIPHERMENT)]).is_some());
    assert!(problem(&[key_usage(DIGITAL_SIGNATURE)]).is_none());
    // nonRepudiation alone is enough.
    assert!(problem(&[key_usage(0x40)]).is_none());
    // Extended key usage without a document-signing purpose.
    assert!(problem(&[extended_key_usage(&[SERVER_AUTH])]).is_some_and(|w| w.message.contains("extended key usage")));
    for ok in ["2.5.29.37.0", "1.3.6.1.5.5.7.3.4", "1.3.6.1.5.5.7.3.36", "1.2.840.113583.1.1.5", "1.3.6.1.4.1.311.10.3.12"] {
        assert!(problem(&[extended_key_usage(&[SERVER_AUTH, ok])]).is_none(), "{ok}");
    }
    // An unknown critical extension refuses; the same extension non-critical does not.
    let unknown = "1.2.3.4.5.6";
    assert!(problem(&[ext(unknown, &der::octets(&[0]))]).is_some_and(|w| w.message.contains(unknown)));
    assert!(problem(&[der::seq(&[&der::oid(unknown), &der::octets(&der::octets(&[0]))])]).is_none());
    // Critical extensions PdfCraft knows, or may accept as they are, don't refuse.
    assert!(problem(&[ext("2.5.29.32", &der::seq(&[]))]).is_none());
    // No extensions at all, and the digital IDs PdfCraft creates, may sign.
    assert!(issue("Plain", Some(&r), FOREVER, &[]).cert.signing_problem().is_none());
    let key = PrivateKey::generate_p256().unwrap();
    let name = Name::build("Own ID", "", "", "", "US");
    assert!(Certificate::self_signed(&name, &key, time(2026), 5, &[1]).unwrap().signing_problem().is_none());
}

#[test]
fn a_trusted_signer_whose_certificate_may_not_sign_is_not_valid() {
    let r = root();
    let (status, details) =
        verdict(signer_with("TLS Server", &r, &[key_usage(DIGITAL_SIGNATURE), extended_key_usage(&[SERVER_AUTH])]), vec![], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("not issued for signing documents")), "{details:?}");
    let (status, details) = verdict(signer_with("Encryption Only", &r, &[key_usage(KEY_ENCIPHERMENT)]), vec![], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("does not allow digital signatures")), "{details:?}");
    let (status, details) = verdict(signer_with("Document Signer", &r, &[key_usage(DIGITAL_SIGNATURE)]), vec![], &r.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
}

#[test]
fn a_smart_cards_certificates_that_may_not_sign_say_why_in_id_lists() {
    // A DoD CAC's four certificates, all under the same name; only the signing one may sign.
    let r = root();
    let clause = |exts: &[Vec<u8>]| signer_with("DOE.JANE.1234567890", &r, exts).cert.signing_problem_clause();
    let encryption = clause(&[key_usage(KEY_ENCIPHERMENT), extended_key_usage(&["1.3.6.1.5.5.7.3.4"])]);
    assert_eq!(encryption.as_deref(), Some("it does not allow digital signatures (its key usage has neither digital signature nor non-repudiation)"));
    let logon = clause(&[key_usage(DIGITAL_SIGNATURE), extended_key_usage(&["1.3.6.1.4.1.311.20.2.2", "1.3.6.1.5.5.7.3.2"])]);
    assert_eq!(logon.as_deref(), Some("it is not issued for signing documents (its extended key usage has no document-signing purpose)"));
    let card_auth = clause(&[key_usage(DIGITAL_SIGNATURE), extended_key_usage(&["2.16.840.1.101.3.6.8"])]);
    assert_eq!(card_auth, logon);
    let signing = clause(&[key_usage(DIGITAL_SIGNATURE | 0x40), extended_key_usage(&["1.3.6.1.5.5.7.3.4", "1.3.6.1.4.1.311.10.3.12"])]);
    assert_eq!(signing, None);
}

#[test]
fn unreadable_constraint_extensions_fail_closed() {
    // keyUsage present but not a BIT STRING: no usage at all, so no keyCertSign.
    let bad_usage = ext("2.5.29.15", &[0xff, 0x00]);
    let c = issue("Bad Usage CA", Some(&root()), FOREVER, &[basic_constraints(true, None), bad_usage]);
    assert_eq!(c.cert.key_usage, Some(0));
    assert!(!c.cert.may_issue());
    // pathLenConstraint present but negative: no CA may follow, not any number.
    let negative = ext("2.5.29.19", &der::seq(&[&der::boolean(true), &der::tlv(tag::INTEGER, &[0xff])]));
    let c = issue("Negative PathLen CA", Some(&root()), FOREVER, &[negative]);
    assert!(c.cert.is_ca);
    assert_eq!(c.cert.path_len, Some(0));
    // basicConstraints present but unparsable: present, not a CA, so not an "old v1 root".
    let garbage = ext("2.5.29.19", &[0x30, 0x05, 0x01]);
    let c = issue("Garbled Constraints", None, FOREVER, &[garbage]);
    assert!(c.cert.has_basic_constraints && !c.cert.is_ca && !c.cert.may_issue());
}

#[test]
fn an_unreadable_extended_key_usage_allows_no_purpose() {
    // An extendedKeyUsage that is present restricts the key even when it doesn't parse (#1807):
    // truncated, critical or not; a SET instead of a SEQUENCE; or a member that isn't an OID.
    // Each would otherwise read as "no restriction", or as only its readable members.
    let r = root();
    let any = der::oid("2.5.29.37.0");
    let truncated = [0x30, 0x05, 0x06];
    let malformed = [
        ext("2.5.29.37", &truncated),
        der::seq(&[&der::oid("2.5.29.37"), &der::octets(&truncated)]),
        ext("2.5.29.37", &der::tlv(tag::SET, &any)),
        ext("2.5.29.37", &der::seq(&[&der::int(1), &any])),
    ];
    for (i, eku) in malformed.into_iter().enumerate() {
        let signer = signer_with("Malformed EKU", &r, &[key_usage(DIGITAL_SIGNATURE), eku]);
        assert_eq!(signer.cert.extended_key_usage.as_deref(), Some(&[][..]), "case {i}");
        let (status, details) = verdict(signer, vec![], &r.cert);
        assert_eq!(status, Status::Unknown, "case {i}: {details:?}");
        assert!(details.iter().any(|d| d.contains("not issued for signing documents")), "case {i}: {details:?}");
    }
    // The same document-signing purpose, well formed, may sign.
    let (status, details) =
        verdict(signer_with("Good EKU", &r, &[key_usage(DIGITAL_SIGNATURE), extended_key_usage(&["2.5.29.37.0"])]), vec![], &r.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
}

/// A CA certificate with a critical extension PdfCraft does not enforce (here name constraints)
/// vouches for nothing: what that extension limits would go unchecked. A self-signed root's
/// extensions are the trust store's business, so such a root still anchors.
#[test]
fn an_issuer_with_an_unprocessed_critical_extension_does_not_vouch() {
    let name_constraints = ext("2.5.29.30", &der::seq(&[]));
    let r = root();
    let constrained =
        issue("Name Constrained CA", Some(&r), FOREVER, &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN), name_constraints.clone()]);
    let l = end_entity("Outside Its Names", &constrained);
    let pool = [constrained.cert.clone(), r.cert.clone()];
    let (chain, why) = build_chain_noted(&l.cert, &pool, None);
    assert_eq!(names(&chain), ["Outside Its Names"]);
    assert!(why.is_some_and(|w| w.message.contains("critical extensions")), "the reason is reported");
    let constrained_root =
        issue("Name Constrained Root", None, FOREVER, &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN), name_constraints]);
    let i = ca("Under Constrained Root", &constrained_root, None);
    let l = end_entity("Leaf", &i);
    let pool = [i.cert.clone(), constrained_root.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Under Constrained Root", "Name Constrained Root"]);
}

fn ca_extensions() -> [Vec<u8>; 2] {
    [basic_constraints(true, None), key_usage(KEY_CERT_SIGN)]
}

/// The last certificate of the chain from `leaf` through `pool`: where the anchor check looks.
fn chain_end(leaf: &Certificate, pool: &[Certificate]) -> Option<Vec<u8>> {
    build_chain(leaf, pool, None).last().map(|c| c.raw.clone())
}

/// Trusting a root trusts its name and key, however the certificate carrying them arrived
/// (RFC 5280 §6.1.1(d), issue #6221). A timestamp authority's token can carry a copy of its root
/// that an older root cross-signed; the chain built from the document's certificates reaches that
/// copy first, and it is still the trusted root.
#[test]
fn a_cross_signed_copy_of_a_trusted_root_is_that_root() {
    let g4 = issue("Root G4", None, FOREVER, &ca_extensions());
    let older = issue("Older Root", None, FOREVER, &ca_extensions());
    let cross = certify(&g4.name, &g4.key, (&older.key, &older.name), 99, FOREVER, &ca_extensions());
    assert_ne!(cross.raw, g4.cert.raw, "another certificate: issuer, serial and signature differ");
    let i = ca("Issuing CA", &g4, None);
    // The scenario: with the document's certificates ahead of the trust store, as validation pools
    // them, the chain ends at the cross-signed copy, not at the trusted certificate.
    let leaf = end_entity("Timestamp Signer", &i);
    let pool = [i.cert.clone(), cross.clone(), g4.cert.clone()];
    assert_eq!(chain_end(&leaf.cert, &pool), Some(cross.raw.clone()));
    let (status, details) = verdict(leaf, vec![i.cert.clone(), cross], &g4.cert);
    assert_eq!(status, Status::Valid, "only the root itself is trusted, not the root that cross-signed it: {details:?}");
    // The trusted certificate itself, with no copy embedded, still anchors.
    let (status, details) = verdict(end_entity("Plain Signer", &i), vec![i.cert.clone()], &g4.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
}

/// A copy of the anchor that drops a critical extension the anchor carries (here name
/// constraints, which PdfCraft does not process) does not stand in for the anchor: the
/// restriction is part of what the user trusted.
#[test]
fn a_copy_that_drops_the_anchors_critical_extension_is_not_the_anchor() {
    let name_constraints = ext("2.5.29.30", &der::seq(&[]));
    let constrained = issue("Constrained Root", None, FOREVER, &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN), name_constraints]);
    let i = ca("Issuing CA", &constrained, None);
    let (status, details) = verdict(end_entity("Honest Signer", &i), vec![i.cert.clone()], &constrained.cert);
    assert_eq!(status, Status::Valid, "control, no copy: {details:?}");
    let older = issue("Older Root", None, FOREVER, &ca_extensions());
    let copy = certify(&constrained.name, &constrained.key, (&older.key, &older.name), 96, FOREVER, &ca_extensions());
    let (status, details) = verdict(end_entity("Honest Signer", &i), vec![i.cert.clone(), copy], &constrained.cert);
    assert_eq!(status, Status::Unknown, "a copy without the anchor's name constraints: {details:?}");
}

/// A certificate with a trusted root's name and another key is an impostor, however it got there.
#[test]
fn a_trusted_roots_name_with_another_key_is_not_trusted() {
    let g4 = issue("Root G4", None, FOREVER, &ca_extensions());
    let older = issue("Older Root", None, FOREVER, &ca_extensions());
    // The same name (the same DER), a fresh key, vouched for by a root nobody trusts.
    let impostor = issue("Root G4", Some(&older), FOREVER, &ca_extensions());
    assert_eq!(impostor.cert.subject.raw, g4.cert.subject.raw);
    let i = ca("Issuing CA", &impostor, None);
    let leaf = end_entity("Impostor Signer", &i);
    let pool = [i.cert.clone(), impostor.cert.clone(), g4.cert.clone()];
    assert_eq!(chain_end(&leaf.cert, &pool), Some(impostor.cert.raw.clone()), "the chain reaches the impostor");
    let (status, details) = verdict(leaf, vec![i.cert.clone(), impostor.cert.clone()], &g4.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
}

/// A trusted root's key under another name is another identity: trust is in the name and the key.
#[test]
fn a_trusted_roots_key_under_another_name_is_not_trusted() {
    let g4 = issue("Root G4", None, FOREVER, &ca_extensions());
    let older = issue("Older Root", None, FOREVER, &ca_extensions());
    let other = name("Some Other Root");
    let renamed = certify(&other, &g4.key, (&older.key, &older.name), 98, FOREVER, &ca_extensions());
    assert_eq!(renamed.public_key, g4.cert.public_key);
    // An issuing CA the root's key signed under the other name.
    let i_name = name("Issuing CA");
    let i_key = PrivateKey::generate_p256().unwrap();
    let i_cert = certify(&i_name, &i_key, (&g4.key, &other), 7, FOREVER, &ca_extensions());
    let i = Party { name: i_name, key: i_key, cert: i_cert };
    let leaf = end_entity("Renamed Signer", &i);
    let pool = [i.cert.clone(), renamed.clone(), g4.cert.clone()];
    assert_eq!(chain_end(&leaf.cert, &pool), Some(renamed.raw.clone()), "the chain reaches the renamed copy");
    let (status, details) = verdict(leaf, vec![i.cert.clone(), renamed], &g4.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
}

/// A forged "copy" of `anchor`: its name and key, with fields of the forger's choosing, signed by
/// a throwaway key nobody has heard of. (Tests by tomcatt, from PR #6277 review.)
fn forged_copy(anchor: &Party, valid: (u32, u32)) -> Certificate {
    let nobody = PrivateKey::generate_p256().unwrap();
    certify(&anchor.name, &anchor.key, (&nobody, &name("Nobody")), 97, valid, &ca_extensions())
}

/// A trusted end-entity certificate may not issue. An embedded "copy" saying `cA` must not change
/// that.
#[test]
fn a_forged_copy_does_not_make_a_trusted_end_entity_a_ca() {
    let colleague = issue("Colleague", None, FOREVER, &[basic_constraints(false, None), key_usage(DIGITAL_SIGNATURE)]);
    let trusted = colleague.cert.clone();
    let (status, details) = verdict(end_entity("Someone Else", &colleague), vec![], &trusted);
    assert_eq!(status, Status::Unknown, "control, no forged copy: {details:?}");
    let forged = forged_copy(&colleague, FOREVER);
    let (status, details) = verdict(end_entity("Someone Else", &colleague), vec![forged], &trusted);
    assert_eq!(status, Status::Unknown, "a forged copy made an end entity a CA: {details:?}");
}

/// An expired anchor stays expired, whatever dates a forged copy claims.
#[test]
fn a_forged_copy_does_not_extend_an_expired_anchor() {
    let root = issue("Expired Root", None, (2000, 2010), &ca_extensions());
    let trusted = root.cert.clone();
    let (status, details) = verdict(end_entity("Late Signer", &root), vec![], &trusted);
    assert_eq!(status, Status::Unknown, "control, no forged copy: {details:?}");
    let forged = forged_copy(&root, FOREVER);
    let (status, details) = verdict(end_entity("Late Signer", &root), vec![forged], &trusted);
    assert_eq!(status, Status::Unknown, "a forged copy revived an expired anchor: {details:?}");
}
