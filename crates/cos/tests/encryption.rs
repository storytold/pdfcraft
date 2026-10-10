//! Encryption end to end: protect a document with every algorithm, save it, and read it back
//! with our reader and with an independent one (hayro-syntax's decryption). Also editing
//! encrypted documents incrementally, changing and removing passwords, and permissions.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Auth, CosError, Document, NewEncryption, ObjRef, Object, PdfString, SaveOptions, write_full, write_incremental};

const SECRET: &str = "BT /F1 18 Tf 20 100 Td (Secret page) Tj ET";

fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{SECRET}\nendstream", SECRET.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Metadata /Subtype /XML /Length 22 >>\nstream\n<x:xmpmeta>meta</x:xmpmeta>\nendstream".replace("22", "26"),
        "<< /Title (Quarterly numbers) >>".to_string(),
    ];
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn protect(alg: Algorithm, user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: alg,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn content(doc: &Document) -> String {
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let pages = doc.get(catalog.reference(b"Pages").unwrap()).as_dict().cloned().unwrap();
    let page = doc.resolve(&pages.get(b"Kids").unwrap().as_array().unwrap()[0]).as_dict().cloned().unwrap();
    match doc.resolve(page.get(b"Contents").unwrap()).as_ref() {
        Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).trim().to_string(),
        _ => panic!("no content stream"),
    }
}

fn title(doc: &Document) -> Option<String> {
    let info = doc.resolve(doc.trailer().get(b"Info")?);
    info.as_dict()?.get(b"Title").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text()))
}

const ALL: [Algorithm; 4] = [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256];

#[test]
fn every_algorithm_round_trips_and_hides_the_content() {
    for alg in ALL {
        let bytes = protect(alg, "user", "owner", -1);
        let raw = String::from_utf8_lossy(&bytes);
        assert!(!raw.contains("Secret page") && !raw.contains("Quarterly"), "{alg:?}: plaintext leaked");
        let bytes = Arc::new(bytes);
        assert!(matches!(Document::open(bytes.clone()), Err(CosError::NeedsPassword)), "{alg:?}");
        assert!(matches!(Document::open_with_password(bytes.clone(), Some("guess")), Err(CosError::WrongPassword)), "{alg:?}");
        let user = Document::open_with_password(bytes.clone(), Some("user")).unwrap();
        assert_eq!(user.security().unwrap().auth(), Auth::User);
        assert_eq!(content(&user), SECRET, "{alg:?}");
        assert_eq!(title(&user).as_deref(), Some("Quarterly numbers"), "{alg:?}");
        let owner = Document::open_with_password(bytes.clone(), Some("owner")).unwrap();
        assert_eq!(owner.security().unwrap().auth(), Auth::Owner);
        // Independent check: hayro decrypts our output. hayro-syntax 0.7 has no owner-password
        // authentication for R2–R4 (Algorithm 7), so for those qpdf is the owner-password oracle
        // (`qpdf_agrees`); hayro checks every user password and R6 owner passwords.
        for pw in if alg == Algorithm::Aes256 { &["user", "owner"][..] } else { &["user"][..] } {
            let hay = hayro_syntax::Pdf::new_with_password(bytes.as_ref().clone(), pw).unwrap_or_else(|e| panic!("{alg:?}/{pw}: {e:?}"));
            let page = hay.pages().first().unwrap().page_stream().map(|s| String::from_utf8_lossy(s).trim().to_string());
            assert_eq!(page.as_deref(), Some(SECRET), "{alg:?}/{pw}");
        }
        assert!(hayro_syntax::Pdf::new(bytes.as_ref().clone()).is_err(), "{alg:?}: hayro must not open it without a password");
    }
}

#[test]
fn empty_user_password_opens_but_reports_restrictions() {
    let bytes = Arc::new(protect(Algorithm::Aes256, "", "owner", 0b0100)); // print only
    let doc = Document::open(bytes.clone()).unwrap();
    let p = doc.permissions().unwrap();
    assert!(p.print() && !p.modify() && !p.copy() && !p.assemble());
    let owner = Document::open_with_password(bytes, Some("owner")).unwrap();
    assert!(owner.permissions().unwrap().modify(), "the owner may do everything");
    assert!(Document::open(Arc::new(fixture())).unwrap().permissions().is_none(), "unencrypted: no restrictions");
}

#[test]
fn incremental_edits_of_encrypted_documents_stay_encrypted() {
    for alg in ALL {
        let original = protect(alg, "user", "owner", -1);
        let mut doc = Document::open_with_password(Arc::new(original.clone()), Some("user")).unwrap();
        let info = doc.trailer().reference(b"Info").unwrap();
        doc.update_dict(info, |d| d.set(b"Title".to_vec(), Object::String(PdfString::text("Revised figures")))).unwrap();
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert_eq!(&saved[..original.len()], &original[..], "{alg:?}: incremental");
        assert!(!String::from_utf8_lossy(&saved).contains("Revised"), "{alg:?}: new string encrypted");
        let again = Document::open_with_password(Arc::new(saved.clone()), Some("user")).unwrap();
        assert_eq!(title(&again).as_deref(), Some("Revised figures"), "{alg:?}");
        assert_eq!(content(&again), SECRET);
        assert!(hayro_syntax::Pdf::new_with_password(saved, "user").is_ok(), "{alg:?}");
    }
}

#[test]
fn changing_and_removing_passwords() {
    let bytes = protect(Algorithm::Rc4_128, "old", "old-owner", -1);
    // Change: decrypt with the old handler, encrypt with the new one.
    let mut doc = Document::open_with_password(Arc::new(bytes), Some("old-owner")).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "new",
        owner_password: "new-owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [3; 32],
    })
    .unwrap();
    let changed = Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap()); // falls back to a full save
    assert!(Document::open_with_password(changed.clone(), Some("old")).is_err());
    let doc = Document::open_with_password(changed.clone(), Some("new")).unwrap();
    assert_eq!(content(&doc), SECRET);
    assert_eq!(doc.security().unwrap().dict().r, 6);
    // Remove.
    let mut doc = Document::open_with_password(changed, Some("new-owner")).unwrap();
    doc.remove_encryption();
    let plain = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(String::from_utf8_lossy(&plain).contains("Secret page"));
    let doc = Document::open(Arc::new(plain.clone())).unwrap();
    assert!(doc.security().is_none() && !doc.trailer().contains(b"Encrypt"));
    assert_eq!(title(&doc).as_deref(), Some("Quarterly numbers"));
    assert!(hayro_syntax::Pdf::new(plain).is_ok());
}

#[test]
fn unencrypted_metadata_is_left_readable() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "",
        owner_password: "o",
        permissions: -1,
        encrypt_metadata: false,
        seed: [5; 32],
    })
    .unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("<x:xmpmeta>meta</x:xmpmeta>"), "search engines can read XMP");
    assert!(!String::from_utf8_lossy(&bytes).contains("Secret page"));
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let meta = doc.get(catalog.reference(b"Metadata").unwrap());
    let Object::Stream(s) = meta.as_ref() else { panic!() };
    assert_eq!(s.decoded().unwrap(), b"<x:xmpmeta>meta</x:xmpmeta>");
    let _ = ObjRef::new(1, 0);
}

/// Second independent oracle when qpdf is installed: it must accept both passwords, identify
/// the owner password, and find no structural errors.
#[test]
fn qpdf_agrees() {
    if std::process::Command::new("qpdf").arg("--version").output().is_err() {
        eprintln!("qpdf not installed; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-enc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for alg in ALL {
        let path = dir.join(format!("{alg:?}.pdf"));
        std::fs::write(&path, protect(alg, "user", "owner", -1)).unwrap();
        for (pw, expect) in [("user", "user password"), ("owner", "owner password")] {
            let out = std::process::Command::new("qpdf").arg(format!("--password={pw}")).arg("--show-encryption").arg(&path).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains(&format!("Supplied password is {expect}")), "{alg:?}/{pw}: {text}");
            let check = std::process::Command::new("qpdf").arg(format!("--password={pw}")).arg("--check").arg(&path).output().unwrap();
            assert!(check.status.success(), "{alg:?}/{pw}: {}", String::from_utf8_lossy(&check.stdout));
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A signature dictionary's `/Contents` is never encrypted (§7.6.2): signers patch it in place
/// after hashing the file around it. Its other strings, and the `/Contents` of anything that
/// is not a signature dictionary, are encrypted as usual, and reading leaves it untouched.
#[test]
fn signature_contents_stay_unencrypted() {
    for alg in ALL {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let hex = |b: &[u8]| Object::String(PdfString { bytes: b.to_vec(), hex: true });
        let byte_range = Object::Array([0, 10, 20, 30].into_iter().map(Object::Int).collect());
        let mut sig = pdfcraft_cos::Dict::new();
        sig.set(b"Type".to_vec(), Object::name("Sig"));
        sig.set(b"Contents".to_vec(), hex(&[0x30, 0x82, 0xDE, 0xAD]));
        sig.set(b"ByteRange".to_vec(), byte_range.clone());
        sig.set(b"Name".to_vec(), PdfString::literal(b"Signer Name".to_vec()));
        let sig = doc.add(Object::Dict(sig));
        // No /Type: still a signature dictionary by its /Contents and /ByteRange.
        let mut untyped = pdfcraft_cos::Dict::new();
        untyped.set(b"Contents".to_vec(), hex(&[0x30, 0x82, 0xBE, 0xEF]));
        untyped.set(b"ByteRange".to_vec(), byte_range);
        let untyped = doc.add(Object::Dict(untyped));
        let mut note = pdfcraft_cos::Dict::new();
        note.set(b"Type".to_vec(), Object::name("Annot"));
        note.set(b"Contents".to_vec(), PdfString::literal(b"Private note".to_vec()));
        let note = doc.add(Object::Dict(note));
        doc.update_dict(doc.root().unwrap(), |c| {
            c.set(b"Test".to_vec(), Object::Array(vec![Object::Ref(sig), Object::Ref(untyped), Object::Ref(note)]))
        })
        .unwrap();
        doc.set_encryption(&NewEncryption {
            algorithm: alg,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [3; 32],
        })
        .unwrap();
        let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
        let raw = String::from_utf8_lossy(&bytes);
        assert!(raw.contains("<3082DEAD>") && raw.contains("<3082BEEF>"), "{alg:?}: signature /Contents written as is");
        assert!(!raw.contains("Signer Name") && !raw.contains("Private note"), "{alg:?}: other strings are encrypted");
        let back = Document::open(Arc::new(bytes)).unwrap();
        let test = back.get(back.root().unwrap()).as_dict().unwrap().get(b"Test").unwrap().as_array().unwrap().clone();
        let string = |i: usize, key: &[u8]| {
            let o = back.resolve(&test[i]);
            o.as_dict().unwrap().get(key).unwrap().as_string().unwrap().bytes.clone()
        };
        assert_eq!(string(0, b"Contents"), [0x30, 0x82, 0xDE, 0xAD], "{alg:?}: read back without decrypting");
        assert_eq!(string(0, b"Name"), b"Signer Name", "{alg:?}");
        assert_eq!(string(1, b"Contents"), [0x30, 0x82, 0xBE, 0xEF], "{alg:?}");
        assert_eq!(string(2, b"Contents"), b"Private note", "{alg:?}");
    }
}
