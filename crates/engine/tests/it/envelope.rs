//! CAdES envelope files (`.pdf.p7m`, issue #1449; `.p7s`, issue #2129) at the engine open
//! path: an attached one opens as the PDF it wraps, read-only, with the signer identified and
//! an extraction action; a detached one opened from disk resolves the document beside it and
//! is verified over that file's bytes; a detached signature without its document, or a non-PDF
//! document, is an actionable error. Fixtures are the OpenSSL-made files in pdfcraft-sign
//! (tests/data/README.md there).

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document as CosDocument, NewEncryption, SaveOptions, write_full};
use pdfcraft_engine::{Edit, EditError, FileReadRefusal, Session};
use pdfcraft_render::OpenError;
use pdfcraft_sign::pkcs12;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The on-disk path of a sign fixture, for tests where the engine itself reads beside a `.p7s`.
fn data_path(name: &str) -> String {
    format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A scratch directory for tests that place a detached signature and its document beside each
/// other on disk. Emptied first, so reruns start clean.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-engine-envelope-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

/// An R2–R4 (RC4-128) PDF whose owner password withholds every permission, built like the
/// engine's `owner_password_of_older_revisions_opens_the_viewer_too`: opening it with the
/// owner password recovers the user password for rendering, and only the owner credential
/// keeps the owner's rights on a copy.
fn restricted_r4_pdf(user: &str, owner: &str) -> Vec<u8> {
    let mut doc = CosDocument::open(Arc::new(data("openssl-signed.pdf"))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Rc4_128,
        user_password: user,
        owner_password: owner,
        permissions: 0,
        encrypt_metadata: true,
        seed: [4; 32],
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
    assert_eq!(env.source.as_deref(), Some("/somewhere/cades-envelope.pdf.p7m"), "the file it was opened from is remembered");
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

/// An encrypted PDF inside an envelope, unlocked with its password, extracts without asking
/// for that password again: the copy is the same encrypted bytes, and the password that opened
/// the envelope opens it too (#1449).
#[test]
fn extracted_encrypted_original_opens_with_the_stored_password() {
    let inner = encrypted_pdf("secret");
    let wrapped = envelope_over(&inner);
    let mut s = Session::new();
    let id = s.open("locked.pdf.p7m", None, wrapped, Some("secret")).unwrap();
    let copy = s.extract_original(id).unwrap();
    let d = s.get(copy).unwrap();
    assert!(d.editable(), "the extracted copy is editable, not blocked on its password");
    assert!(d.envelope.is_none() && d.read_only_reason.is_none(), "a plain document: {:?}", d.read_only_reason);
    assert!(d.password.as_deref().is_some_and(|p| p == "secret"), "the copy keeps the password it needs");
    assert_eq!(&d.bytes[..], &inner[..], "byte-identical to the encrypted PDF that was signed");
    assert_eq!(d.info.pages.len(), 1, "the copy's pages are read through its password");
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

/// An encrypted original opened with the OWNER password of an R2–R4 file extracts with the
/// owner's rights, not as a re-restricted copy: `Document::password` carries the user
/// password owner authentication recovers (for rendering), so extraction must use the
/// original credential it retains instead. Before the fix the copy opened with the recovered
/// user password and every permission stayed withheld (#1449 review).
#[test]
fn extracted_original_opened_with_the_owner_password_keeps_owner_rights() {
    let inner = restricted_r4_pdf("u", "o");
    let wrapped = envelope_over(&inner);
    let mut s = Session::new();
    let id = s.open("locked.pdf.p7m", None, wrapped, Some("o")).unwrap();
    // The envelope document itself renders through the recovered user password.
    assert_eq!(s.get(id).unwrap().password.as_deref(), Some("u"));
    let copy = s.extract_original(id).unwrap();
    let d = s.get(copy).unwrap();
    let summary = d.security_summary().expect("still encrypted");
    assert!(summary.owner, "the copy opens with the owner's rights: {:?}", summary);
    assert!(d.allows_modification(), "the withheld permissions are the owner's to lift: {:?}", summary);
    assert_eq!(&d.bytes[..], &inner[..], "byte-identical to the encrypted PDF that was signed");
}

/// A detached signature runtime-made over `content` with the test ID, its content digest
/// computed the way the caller of `sign_detached` does.
fn detached_signature_over(content: &[u8]) -> Vec<u8> {
    let id = test_id();
    let alg = id.key.preferred_digest();
    pdfcraft_sign::cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &alg.digest(&[content])).unwrap()
}

#[test]
fn detached_pair_opens_read_only_verified() {
    let mut s = Session::new();
    // The pair is the committed fixture: the .p7s and the PDF beside it, both on disk.
    let path = data_path("openssl-signed.pdf.p7s");
    let id = s.open("openssl-signed.pdf.p7s", Some(path), Arc::new(data("openssl-signed.pdf.p7s")), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.editable(), "the pair opens read-only");
    assert!(d.path.is_none(), "the signature's path never attaches: {:?}", d.path);
    assert_eq!(d.name, "openssl-signed.pdf", "the document is the sibling beside the signature");
    assert!(d.repair_log().is_empty(), "the sibling is opened as it is: {:?}", d.repair_log());
    let env = d.envelope.as_ref().expect("a detached pair carries EnvelopeInfo");
    assert!(env.detached, "opened from the sibling, not from inside the envelope");
    assert!(env.digest_ok, "the signature is verified over the sibling's bytes");
    assert_eq!(env.signer, "PdfCraft Test");
    assert_eq!(env.signers, 1);
    // Edits are refused through the same read-only path the attached envelope uses.
    let err = s.apply(id, Edit::SetInfo { key: "Title".into(), value: "x".into() }).unwrap_err();
    assert!(matches!(err, EditError::ReadOnly(ref reason) if reason.contains("detached CAdES")), "{err}");
}

#[test]
fn missing_sibling_error_names_the_expected_file() {
    let mut s = Session::new();
    let path = data_path("cades-detached.p7s");
    let err = s.open("cades-detached.p7s", Some(path), Arc::new(data("cades-detached.p7s")), None).unwrap_err();
    // One message for a missing and an unreadable sibling alike, so opening a .p7s cannot be
    // used to probe the directory around it.
    assert!(matches!(err, OpenError::Unsupported(ref m) if m == "cades-detached.p7s signs cades-detached, which was not found beside it"), "{err}");
}

#[test]
fn misnamed_detached_envelope_does_not_read_itself() {
    let dir = scratch("misnamed");
    let bytes = data("cades-detached.p7s");
    std::fs::write(dir.join("odd-name.p7m"), &bytes).unwrap();
    let path = dir.join("odd-name.p7m").to_string_lossy().into_owned();
    let mut s = Session::new();
    // A detached signature that is not named .p7s is not looking for a sibling at all, even
    // when it was opened from disk.
    let err = s.open("odd-name.p7m", Some(path), Arc::new(bytes), None).unwrap_err();
    assert!(
        matches!(err, OpenError::Unsupported(ref m) if m == "this is a detached CAdES signature; the signed document is not inside the file"),
        "{err}"
    );
}

#[test]
fn sibling_that_is_not_a_pdf_is_refused() {
    let dir = scratch("not-a-pdf");
    let payload: &[u8] = b"<?xml version=\"1.0\"?><note/>";
    let sig = detached_signature_over(payload);
    std::fs::write(dir.join("signed-xml.p7s"), &sig).unwrap();
    std::fs::write(dir.join("signed-xml"), payload).unwrap();
    let path = dir.join("signed-xml.p7s").to_string_lossy().into_owned();
    let mut s = Session::new();
    let err = s.open("signed-xml.p7s", Some(path), Arc::new(sig), None).unwrap_err();
    assert!(matches!(err, OpenError::Unsupported(ref m) if m == "the detached signature's document is XML data, not a PDF"), "{err}");
}

#[test]
fn tampered_sibling_reports_digest_mismatch() {
    let dir = scratch("tampered");
    let pdf = data("openssl-signed.pdf");
    // Flip the version digit after the %PDF- magic: still an openable document, but no longer
    // the bytes that were signed.
    let mut tampered = pdf.clone();
    tampered[7] = b'8';
    let sig = detached_signature_over(&pdf);
    std::fs::write(dir.join("openssl-signed.pdf.p7s"), &sig).unwrap();
    std::fs::write(dir.join("openssl-signed.pdf"), &tampered).unwrap();
    let path = dir.join("openssl-signed.pdf.p7s").to_string_lossy().into_owned();
    let mut s = Session::new();
    let id = s.open("openssl-signed.pdf.p7s", Some(path), Arc::new(sig), None).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.editable(), "a pair with a tampered sibling is still read-only");
    assert!(d.path.is_none());
    let env = d.envelope.as_ref().expect("a detached pair carries EnvelopeInfo");
    assert!(env.detached);
    assert!(!env.digest_ok, "the flipped byte is caught");
    let err = s.apply(id, Edit::SetInfo { key: "Title".into(), value: "x".into() }).unwrap_err();
    assert!(matches!(err, EditError::ReadOnly(_)), "{err}");
}

#[test]
fn bytes_only_detached_open_keeps_today_error() {
    let mut s = Session::new();
    // Without a path there is nowhere to look for the sibling: the plain detached error stands.
    let err = s.open("cades-detached.p7s", None, Arc::new(data("cades-detached.p7s")), None).unwrap_err();
    assert!(
        matches!(err, OpenError::Unsupported(ref m) if m == "this is a detached CAdES signature; the signed document is not inside the file"),
        "{err}"
    );
}

#[test]
fn sibling_read_is_confined_by_the_hosts_guard() {
    let dir = scratch("guard");
    let pdf = data("openssl-signed.pdf");
    let sig = detached_signature_over(&pdf);
    std::fs::write(dir.join("openssl-signed.pdf.p7s"), &sig).unwrap();
    std::fs::write(dir.join("openssl-signed.pdf"), &pdf).unwrap();
    let path = dir.join("openssl-signed.pdf.p7s").to_string_lossy().into_owned();
    // The host's guard refuses the sibling, as one resolving out of its root would be: the
    // pair does not open, and the refusal is the host's own message.
    let mut refused = Session::new().with_file_read_guard(Arc::new(|p: &std::path::Path| {
        Err(FileReadRefusal::Refused(format!("{} is outside the allowed directory /root", p.display())))
    }));
    let err = refused.open("openssl-signed.pdf.p7s", Some(path.clone()), Arc::new(sig.clone()), None).unwrap_err();
    assert!(matches!(err, OpenError::Unsupported(ref m) if m.contains("outside the allowed directory")), "{err}");
    // A guard that opens the sibling leaves the pair opening as it does without one.
    let mut allowed =
        Session::new().with_file_read_guard(Arc::new(|p: &std::path::Path| std::fs::File::open(p).map_err(|_| FileReadRefusal::Unreadable)));
    let id = allowed.open("openssl-signed.pdf.p7s", Some(path), Arc::new(sig), None).unwrap();
    assert!(allowed.get(id).unwrap().envelope.as_ref().is_some_and(|e| e.detached));
}

#[test]
fn extract_original_refuses_detached_pairs() {
    let dir = scratch("extract-detached");
    let pdf = data("openssl-signed.pdf");
    let sig = detached_signature_over(&pdf);
    std::fs::write(dir.join("openssl-signed.pdf.p7s"), &sig).unwrap();
    std::fs::write(dir.join("openssl-signed.pdf"), &pdf).unwrap();
    let path = dir.join("openssl-signed.pdf.p7s").to_string_lossy().into_owned();
    let mut s = Session::new();
    let id = s.open("openssl-signed.pdf.p7s", Some(path), Arc::new(sig), None).unwrap();
    assert_eq!(s.extract_original(id).unwrap_err(), "a detached signature has nothing to extract; open the PDF itself to edit it.");
    // Nothing opened: the pair is still the only document.
    assert_eq!(s.docs().len(), 1);
}

/// The guard performs the confined open and the engine reads from the handle it gets back
/// (#2202 review): once the guard has opened the sibling, the path can be unlinked and the
/// pair still opens, verified over the bytes of that handle. This fails if the engine
/// resolves the sibling's path again after the guard — the time-of-check-to-time-of-use
/// window in which a swapped link could steer the read outside the host's root.
#[test]
fn detached_sibling_is_read_from_the_guards_handle() {
    let dir = scratch("guard-handle");
    let pdf = data("openssl-signed.pdf");
    let sig = detached_signature_over(&pdf);
    std::fs::write(dir.join("openssl-signed.pdf.p7s"), &sig).unwrap();
    std::fs::write(dir.join("openssl-signed.pdf"), &pdf).unwrap();
    let path = dir.join("openssl-signed.pdf.p7s").to_string_lossy().into_owned();
    // The host's guarded open, after which the sibling disappears from disk: nothing that
    // resolves the path again can reach it.
    let mut gone = Session::new().with_file_read_guard(Arc::new(|p: &std::path::Path| {
        let handle = std::fs::File::open(p).map_err(|_| FileReadRefusal::Unreadable)?;
        std::fs::remove_file(p).unwrap();
        Ok(handle)
    }));
    let id = gone.open("openssl-signed.pdf.p7s", Some(path), Arc::new(sig), None).unwrap();
    let d = gone.get(id).unwrap();
    let env = d.envelope.as_ref().expect("the pair opens from the guard's handle");
    assert!(env.detached);
    assert!(env.digest_ok, "verified over the bytes the guard opened, not a later re-read of the path");
    assert_eq!(d.name, "openssl-signed.pdf");
    assert!(!dir.join("openssl-signed.pdf").exists(), "the sibling really is gone: the engine read the handle, not the path");
}
