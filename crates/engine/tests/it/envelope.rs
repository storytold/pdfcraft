//! CAdES envelope files (`.pdf.p7m`, issue #1449) at the engine open path: one opens as the
//! PDF it wraps, read-only, with the signer identified and an extraction action; a detached
//! signature or a non-PDF content is an actionable error. Fixtures are the OpenSSL-made files
//! in pdfcraft-sign (tests/data/README.md there).

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document as CosDocument, NewEncryption, SaveOptions, write_full};
use pdfcraft_engine::{Edit, EditError, Session};
use pdfcraft_render::OpenError;
use pdfcraft_sign::pkcs12;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The test digital ID the fixtures were made with (see sign/tests/data/README.md).
fn test_id() -> pkcs12::DigitalId {
    pkcs12::open(&data("rsa-aes.p12"), "test").unwrap()
}

/// An honest attached CAdES envelope over `content`, signed with the test ID.
fn envelope_over(content: &[u8]) -> Arc<Vec<u8>> {
    let id = test_id();
    Arc::new(
        pdfcraft_sign::cms::sign_encapsulated(&id.key, &id.certificate, &id.chain, id.key.preferred_digest(), pdfcraft_sign::cms::DATA, content)
            .unwrap(),
    )
}

/// A password-protected PDF built at test time with the cos writer's encryption: no
/// encrypted PDF fixture is committed in the workspace (every crate builds these on the fly).
fn encrypted_pdf(user: &str) -> Vec<u8> {
    let mut doc = CosDocument::open(Arc::new(data("openssl-signed.pdf"))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: user,
        owner_password: "owner-secret",
        permissions: -1,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

#[test]
fn envelope_opens_read_only_without_repairs() {
    let mut s = Session::new();
    // The .p7m sits on disk, but its path must not attach: Save may never write over the envelope.
    let id =
        s.open("cades-envelope.pdf.p7m", Some("/somewhere/cades-envelope.pdf.p7m".into()), Arc::new(data("cades-envelope.pdf.p7m")), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.read_only_reason.as_deref().is_some_and(|r| r.contains("CAdES")), "{:?}", d.read_only_reason);
    assert!(!d.editable(), "the envelope document is read-only");
    assert!(d.path.is_none(), "the envelope's path never becomes the document's: {:?}", d.path);
    assert_eq!(d.name, "cades-envelope.pdf", "one trailing .p7m comes off the display name");
    assert!(d.repair_log().is_empty(), "the embedded PDF is opened as it is: {:?}", d.repair_log());
    let env = d.envelope.as_ref().expect("an envelope document carries EnvelopeInfo");
    assert_eq!(env.signer, "PdfCraft Test");
    let time = env.signing_time.as_deref().expect("the OpenSSL fixture carries a signing time");
    assert_eq!(time.len(), 20, "YYYY-MM-DD HH:MM UTC: {time}");
    assert!(time.ends_with(" UTC") && time.as_bytes()[4] == b'-' && time.as_bytes()[10] == b' ', "{time}");
    assert!(env.digest_ok, "the fixture's content matches what was signed");
    assert_eq!(env.signers, 1);
    assert!(env.quirks.is_empty(), "OpenSSL writes DER: {:?}", env.quirks);
    let pages = d.info.pages.len();
    let direct = s.open("plain.pdf", None, Arc::new(data("openssl-signed.pdf")), None).unwrap();
    assert_eq!(pages, s.get(direct).unwrap().info.pages.len(), "the wrapped PDF's pages are the document's");
}

#[test]
fn edits_are_refused() {
    let mut s = Session::new();
    let id = s.open("cades-envelope.pdf.p7m", None, Arc::new(data("cades-envelope.pdf.p7m")), None).unwrap();
    let err = s.apply(id, Edit::SetInfo { key: "Title".into(), value: "x".into() }).unwrap_err();
    assert!(matches!(err, EditError::ReadOnly(ref reason) if reason.contains("CAdES")), "{err}");
}

#[test]
fn detached_is_an_actionable_error() {
    let mut s = Session::new();
    let err = s.open("cades-detached.p7s", None, Arc::new(data("cades-detached.p7s")), None).unwrap_err();
    assert!(matches!(err, OpenError::Unsupported(ref m) if m.contains("detached")), "{err}");
}

#[test]
fn xml_content_is_an_actionable_error() {
    let mut s = Session::new();
    let err = s.open("cades-xml.p7m", None, Arc::new(data("cades-xml.p7m")), None).unwrap_err();
    assert!(matches!(err, OpenError::Unsupported(ref m) if m == "the CAdES envelope contains XML data, not a PDF"), "{err}");
}

#[test]
fn envelope_with_damaged_inner_pdf_opens_read_only_with_repairs() {
    // The envelope is intact; the PDF inside was cut off after 5 000 bytes (its cross-reference
    // table is gone). It still opens as its (repaired) pages, read-only, and the damage surfaces.
    let cut = data("openssl-signed.pdf")[..5_000].to_vec();
    let mut s = Session::new();
    let id = s.open("damaged.pdf.p7m", None, envelope_over(&cut), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.read_only_reason.as_deref().is_some_and(|r| r.contains("CAdES")), "{:?}", d.read_only_reason);
    assert!(!d.editable());
    assert!(!d.info.warnings.is_empty(), "the inner damage surfaces: {:?}", d.info.warnings);
    assert_eq!(d.info.pages.len(), 1, "the repaired pages are what the document shows");
}

#[test]
fn envelope_with_encrypted_inner_pdf_asks_for_its_password() {
    let wrapped = envelope_over(&encrypted_pdf("secret"));
    let mut s = Session::new();
    let err = s.open("locked.pdf.p7m", None, wrapped.clone(), None).unwrap_err();
    assert!(matches!(err, OpenError::NeedsPassword), "{err}");
    let err = s.open("locked.pdf.p7m", None, wrapped.clone(), Some("nope")).unwrap_err();
    assert!(matches!(err, OpenError::WrongPassword), "{err}");
    // The right password flows through to the inner open.
    let id = s.open("locked.pdf.p7m", None, wrapped, Some("secret")).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.editable() && d.read_only_reason.as_deref().is_some_and(|r| r.contains("CAdES")));
}

#[test]
fn extracted_original_is_byte_identical() {
    let mut s = Session::new();
    let id = s.open("cades-envelope.pdf.p7m", None, Arc::new(data("cades-envelope.pdf.p7m")), None).unwrap();
    let copy = s.extract_original(id).unwrap();
    let d = s.get(copy).unwrap();
    assert!(d.editable(), "the extracted copy is editable");
    assert!(d.path.is_none(), "the copy is unsaved");
    assert_eq!(&d.bytes[..], &data("openssl-signed.pdf")[..], "byte-identical to the embedded PDF");
    // The envelope document is unchanged and still read-only.
    assert!(!s.get(id).unwrap().editable());
}

#[test]
fn extract_refuses_plain_documents() {
    let mut s = Session::new();
    let id = s.open("plain.pdf", None, Arc::new(data("openssl-signed.pdf")), None).unwrap();
    assert_eq!(s.extract_original(id).unwrap_err(), "this document is not inside a CAdES envelope");
    assert_eq!(s.extract_original(pdfcraft_engine::DocId(999)).unwrap_err(), "no such document");
}

/// The `file.extract_original` command is registered for envelope documents only, so menus,
/// the palette and the control channel never offer it — and never run it — on a plain one.
#[test]
fn extract_command_is_enabled_only_for_envelope_documents() {
    use pdfcraft_engine::commands;
    let spec = commands::command("file.extract_original").expect("registered");
    assert_eq!(spec.menu, Some("File"));
    let mut s = Session::new();
    assert!(!commands::is_enabled(spec, &s, None), "no document, no extraction");
    let plain = s.open("plain.pdf", None, Arc::new(data("openssl-signed.pdf")), None).unwrap();
    assert!(!commands::is_enabled(spec, &s, Some(plain)), "a plain document offers no extraction");
    let env = s.open("cades-envelope.pdf.p7m", None, Arc::new(data("cades-envelope.pdf.p7m")), None).unwrap();
    assert!(commands::is_enabled(spec, &s, Some(env)));
    // The extracted copy is a plain document: the command switches off once the work is done.
    let copy = s.extract_original(env).unwrap();
    assert!(!commands::is_enabled(spec, &s, Some(copy)));
}

#[test]
fn save_as_hands_out_the_signed_envelope_not_the_pdf_inside() {
    let mut s = Session::new();
    let envelope = Arc::new(data("cades-envelope.pdf.p7m"));
    let id = s.open("cades-envelope.pdf.p7m", None, envelope.clone(), None).unwrap();
    // The read-only save path (Save As, `doc_save`) returns the file as opened: the signed
    // envelope. Returning the working PDF would let a save over the `.p7m` silently replace
    // the signed envelope with the unsigned document inside it.
    let saved = s.save_bytes(id).unwrap();
    assert_eq!(&saved[..], &envelope[..], "the envelope's own bytes");
    assert_ne!(&saved[..], &data("openssl-signed.pdf")[..], "never the unsigned PDF inside");
    assert_eq!(&s.get(id).unwrap().saved_bytes()[..], &envelope[..], "the envelope is also the saved base");
    // The working file stays the PDF everything else processes (pages, printing, combining).
    assert_eq!(&s.get(id).unwrap().bytes[..], &data("openssl-signed.pdf")[..]);
}
