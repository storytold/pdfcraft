//! Encryption end to end: protect a document with every algorithm, save it, and read it back
//! with our reader and with an independent one (hayro-syntax's decryption). Also editing
//! encrypted documents incrementally, changing and removing passwords, and permissions.

use std::io::Write;
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Auth, CosError, Dict, Document, NewEncryption, ObjRef, Object, PdfString, SaveOptions, write_full, write_incremental};

const SECRET: &str = "BT /F1 18 Tf 20 100 Td (Secret page) Tj ET";

fn fixture() -> Vec<u8> {
    fixture_with("")
}

/// The fixture with `extra` added to the content stream's dictionary.
fn fixture_with(extra: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {}{extra} >>\nstream\n{SECRET}\nendstream", SECRET.len()),
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
fn the_permissions_password_unlocks_an_open_document() {
    for alg in ALL {
        // Only a permissions password: opens without asking, restricted (#995).
        let mut doc = Document::open(Arc::new(protect(alg, "", "perms", 0b0100))).unwrap();
        // Read something before unlocking: it must stay readable afterwards.
        assert_eq!(content(&doc), SECRET, "{alg:?}");
        assert_eq!(doc.unlock("").unwrap_err(), CosError::WrongPassword, "{alg:?}");
        assert_eq!(doc.unlock("nope").unwrap_err(), CosError::WrongPassword, "{alg:?}");
        assert!(!doc.permissions().unwrap().modify(), "{alg:?}: still restricted");
        doc.unlock("perms").unwrap();
        assert_eq!(doc.security().unwrap().auth(), Auth::Owner, "{alg:?}");
        assert!(doc.permissions().unwrap().modify(), "{alg:?}");
        assert_eq!(content(&doc), SECRET, "{alg:?}");
        assert_eq!(title(&doc).as_deref(), Some("Quarterly numbers"), "{alg:?}");
        // Unlocked, security can be removed.
        doc.remove_encryption();
        let plain = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(Document::open(Arc::new(plain)).unwrap().security().is_none(), "{alg:?}");
    }
    // Opened with a separate open password: that password doesn't unlock.
    let mut doc = Document::open_with_password(Arc::new(protect(Algorithm::Aes256, "open", "perms", 0b0100)), Some("open")).unwrap();
    assert_eq!(doc.unlock("open").unwrap_err(), CosError::WrongPassword);
    doc.unlock("perms").unwrap();
    assert!(doc.permissions().unwrap().modify());
    // Nothing to unlock: unencrypted, or already the owner.
    Document::open(Arc::new(fixture())).unwrap().unlock("anything").unwrap();
    doc.unlock("anything").unwrap();
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

/// The second independent oracle: qpdf must accept both passwords, identify the owner password,
/// and find no structural errors. For R2-R4 this is the *only* check of owner-password
/// authentication (Algorithm 7) that does not go through our own code, so it is the one place a
/// silent skip would cost the most -- see `pdfcraft-oracle`, which makes absence loud.
#[test]
fn qpdf_agrees() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let dir = private_temp_dir("pdfcraft-enc");
    for alg in ALL {
        let path = dir.join(format!("{alg:?}.pdf"));
        std::fs::File::create_new(&path).and_then(|mut f| f.write_all(&protect(alg, "user", "owner", -1))).unwrap();
        for (pw, expect) in [("user", "user password"), ("owner", "owner password")] {
            let out = qpdf.command().arg(format!("--password={pw}")).arg("--show-encryption").arg(&path).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains(&format!("Supplied password is {expect}")), "{alg:?}/{pw}: {text}");
            let check = qpdf.command().arg(format!("--password={pw}")).arg("--check").arg(&path).output().unwrap();
            assert!(check.status.success(), "{alg:?}/{pw}: {}", String::from_utf8_lossy(&check.stdout));
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A new directory for oracle files, private to this user: an unpredictable name (`RandomState`
/// is seeded by the operating system), created by this call or not at all.
fn private_temp_dir(prefix: &str) -> std::path::PathBuf {
    use std::hash::{BuildHasher, Hasher};
    let mut name = std::collections::hash_map::RandomState::new().build_hasher();
    name.write_u32(std::process::id());
    let dir = std::env::temp_dir().join(format!("{prefix}-{:016x}", name.finish()));
    #[cfg(unix)]
    let builder = {
        let mut builder = std::fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = std::fs::DirBuilder::new();
    builder.create(&dir).unwrap();
    dir
}

/// Oracle files go in a directory this test created: another local account cannot have
/// prepared it (or symlinks inside it) in advance.
#[test]
fn oracle_files_go_in_a_fresh_directory() {
    let (a, b) = (private_temp_dir("pdfcraft-enc-probe"), private_temp_dir("pdfcraft-enc-probe"));
    assert_ne!(a, b);
    assert_eq!(std::fs::read_dir(&a).unwrap().count(), 0);
    let _ = (std::fs::remove_dir(a), std::fs::remove_dir(b));
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

/// Only a signature dictionary keeps its `/Contents` in clear. An annotation that carries a
/// `/ByteRange` (or even `/Type /Sig`) is still an annotation, and its comment is encrypted.
#[test]
fn a_byte_range_does_not_exempt_an_annotation() {
    for (ty, subtype) in [(Some("Annot"), "Text"), (None, "FreeText"), (Some("Sig"), "Text")] {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let mut note = pdfcraft_cos::Dict::new();
        if let Some(ty) = ty {
            note.set(b"Type".to_vec(), Object::name(ty));
        }
        note.set(b"Subtype".to_vec(), Object::name(subtype));
        note.set(b"Contents".to_vec(), PdfString::literal(b"Private note".to_vec()));
        note.set(b"ByteRange".to_vec(), Object::Array(Vec::new()));
        let note = doc.add(Object::Dict(note));
        doc.update_dict(doc.root().unwrap(), |c| c.set(b"Test".to_vec(), Object::Ref(note))).unwrap();
        doc.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [3; 32],
        })
        .unwrap();
        let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Private note"), "{ty:?} {subtype}: comment encrypted");
        let back = Document::open(Arc::new(bytes)).unwrap();
        let note = back.get(back.root().unwrap()).as_dict().unwrap().reference(b"Test").unwrap();
        assert_eq!(back.get(note).as_dict().unwrap().get(b"Contents").unwrap().as_string().unwrap().bytes, b"Private note");
    }
}

/// Protecting a document puts every stream under the new handler. A stream that selects a
/// crypt filter of its own loses the selector: the new `/Encrypt` defines only `/StdCF`, so an
/// inherited name (or `/Identity`) would leave the stream in clear.
#[test]
fn protecting_drops_inherited_crypt_filter_selectors() {
    for selector in [" /Filter /Crypt /DecodeParms << /Name /MyCF >>", " /Filter [/Crypt] /DecodeParms [<< /Name /Identity >>]", " /Filter /Crypt"] {
        let mut doc = Document::open(Arc::new(fixture_with(selector))).unwrap();
        assert_eq!(content(&doc), SECRET, "{selector}");
        doc.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Secret page"), "{selector}: stream encrypted");
        assert_eq!(content(&Document::open(Arc::new(bytes.clone())).unwrap()), SECRET, "{selector}");
        assert!(hayro_syntax::Pdf::new(bytes).is_ok(), "{selector}");
    }
    // Under the handler the document was opened with, `/Identity` is the author's choice (a
    // stream meant to stay readable) and is kept.
    let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let pages = doc.get(catalog.reference(b"Pages").unwrap()).as_dict().cloned().unwrap();
    let page = doc.resolve(&pages.get(b"Kids").unwrap().as_array().unwrap()[0]).as_dict().cloned().unwrap();
    let contents = page.reference(b"Contents").unwrap();
    let Object::Stream(mut s) = doc.get(contents).as_ref().clone() else { panic!("no content stream") };
    s.dict.set(b"Filter".to_vec(), Object::name("Crypt"));
    doc.set(contents, Object::Stream(s));
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(String::from_utf8_lossy(&saved).contains("Secret page"), "written as the stream asks");
    assert_eq!(content(&Document::open(Arc::new(saved)).unwrap()), SECRET);
}

#[test]
fn a_nonpositive_crypt_filter_length_reads_as_the_default_key_length() {
    // A V4 crypt filter's /Length counts bytes (≤ 32) or bits. A zero or negative value is
    // meaningless and is read as if absent (128 bits), not multiplied into an overflow (#1816).
    let original = protect(Algorithm::Aes128, "", "owner", -1);
    for length in [i64::MIN, i64::MIN / 8 - 1, i64::MIN / 8, -1, 0, 16, 128] {
        let mut doc = Document::open(Arc::new(original.clone())).unwrap();
        let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
        doc.update_dict(encrypt, |e| {
            let Some(Object::Dict(cf)) = e.get(b"CF") else { panic!("no /CF") };
            let Some(Object::Dict(std_cf)) = cf.get(b"StdCF") else { panic!("no /StdCF") };
            let (mut cf, mut std_cf) = (cf.clone(), std_cf.clone());
            std_cf.set(b"Length".to_vec(), Object::Int(length));
            cf.set(b"StdCF".to_vec(), Object::Dict(std_cf));
            e.set(b"CF".to_vec(), Object::Dict(cf));
        })
        .unwrap();
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let again = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(content(&again), SECRET, "/Length {length}");
    }
}

#[test]
fn an_earlier_revision_opens_with_the_documents_own_credentials() {
    // The bytes a signature covers are an earlier revision of the file: the document that is
    // open can read them with its own key, so no password has to be kept or asked for again.
    for alg in ALL {
        let original = protect(alg, "user", "owner", -1);
        let mut doc = Document::open_with_password(Arc::new(original.clone()), Some("user")).unwrap();
        let info = doc.trailer().reference(b"Info").unwrap();
        doc.update_dict(info, |d| d.set(b"Title".to_vec(), Object::String(PdfString::text("Revised")))).unwrap();
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let current = Document::open_with_password(Arc::new(saved), Some("user")).unwrap();
        assert!(matches!(Document::open(Arc::new(original.clone())), Err(CosError::NeedsPassword)), "{alg:?}: not without a password");
        let earlier = current.open_with_same_credentials(Arc::new(original.clone())).unwrap();
        assert_eq!(content(&earlier), SECRET, "{alg:?}: the earlier revision is readable");
        assert_eq!(title(&earlier).as_deref(), Some("Quarterly numbers"), "{alg:?}: and it is the earlier one");
        // Another file, encrypted with other keys: the document's key is no use, and it says so.
        let other = {
            let mut doc = Document::open(Arc::new(fixture())).unwrap();
            doc.set_encryption(&NewEncryption {
                algorithm: alg,
                user_password: "user",
                owner_password: "owner",
                permissions: -1,
                encrypt_metadata: true,
                seed: [3; 32],
            })
            .unwrap();
            write_full(&doc, &SaveOptions::default()).unwrap()
        };
        assert!(matches!(current.open_with_same_credentials(Arc::new(other)), Err(CosError::NeedsPassword)), "{alg:?}");
        // An unencrypted file opens as it always does.
        assert_eq!(content(&current.open_with_same_credentials(Arc::new(fixture())).unwrap()), SECRET, "{alg:?}");
    }
}

/// `protect`, with the `/Encrypt` dictionary written inline in the trailer, as MuPDF writes it,
/// instead of as an indirect object.
fn protect_inline(alg: Algorithm, user: &str, owner: &str) -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: alg,
        user_password: user,
        owner_password: owner,
        permissions: -1,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    let dict = doc.get(doc.trailer().reference(b"Encrypt").unwrap()).as_dict().cloned().unwrap();
    doc.trailer_mut().set(b"Encrypt".to_vec(), Object::Dict(dict));
    write_full(&doc, &SaveOptions::default()).unwrap()
}

/// MuPDF writes `/Encrypt` inline in the trailer, so removing it changes no object. It is still
/// an unsaved change, and saving must write a file that opens without a password (#2661).
#[test]
fn removing_an_inline_encrypt_dictionary_leaves_an_unencrypted_file() {
    for alg in ALL {
        let bytes = Arc::new(protect_inline(alg, "user", "owner"));
        assert!(matches!(Document::open(bytes.clone()), Err(CosError::NeedsPassword)), "{alg:?}");
        let mut doc = Document::open_with_password(bytes, Some("owner")).unwrap();
        assert!(matches!(doc.trailer().get(b"Encrypt"), Some(Object::Dict(_))), "{alg:?}: inline");
        assert!(!doc.is_modified(), "{alg:?}");
        doc.remove_encryption();
        assert!(doc.is_modified(), "{alg:?}: removing security is an unsaved change");
        let plain = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let again = Document::open(Arc::new(plain.clone())).unwrap_or_else(|e| panic!("{alg:?}: {e:?}"));
        assert!(again.security().is_none() && !again.trailer().contains(b"Encrypt"), "{alg:?}");
        assert_eq!(content(&again), SECRET, "{alg:?}");
        assert!(hayro_syntax::Pdf::new(plain).is_ok(), "{alg:?}");
    }
}

/// A content stream that calls itself `/Type /XRef` is still encrypted: only reading trusts the label.
#[test]
fn a_stream_labelled_xref_is_encrypted_when_saved() {
    let mut doc = Document::open(Arc::new(fixture_with(" /Type /XRef"))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "u",
        owner_password: "o",
        permissions: -4,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(!bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()), "the labelled stream was written in the clear");
}

// Undo can restore a snapshot whose output security differs from the last saved file (#3276).
// Overlaying such a snapshot onto that file must be refused before anything changes.

/// Edit `/Title` in the document information dictionary, adding one if there is none: an unsaved
/// change to compare.
fn set_title(doc: &mut Document, title: &str) {
    let title = Object::String(PdfString { bytes: title.as_bytes().to_vec(), hex: false });
    match doc.trailer().reference(b"Info") {
        Some(info) => doc.update_dict(info, |d| d.set(b"Title".to_vec(), title)).unwrap(),
        None => {
            let mut info = Dict::new();
            info.set(b"Title".to_vec(), title);
            let info = doc.add(Object::Dict(info));
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
        }
    }
}

/// Protect again with the same passwords and no restrictions. `seed` picks the AES-256 key, and
/// the `/ID` when the document has none.
fn rekey(doc: &mut Document, alg: Algorithm, seed: u8) {
    doc.set_encryption(&NewEncryption {
        algorithm: alg,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [seed; 32],
    })
    .unwrap();
}

/// Replace the first file identifier, which is part of the key, and keep the second.
fn with_first_id(doc: &mut Document, id: &[u8]) {
    let Some(Object::Array(ids)) = doc.trailer().get(b"ID").cloned() else { panic!("no /ID") };
    let id = Object::String(PdfString { bytes: id.to_vec(), hex: true });
    doc.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![id, ids[1].clone()]));
}

/// Everything an overlay could change: the graph and trailer an incremental save would write.
fn state(doc: &Document) -> (Dict, Vec<u32>, Vec<u32>, bool, Vec<u8>) {
    let save = write_incremental(doc, &SaveOptions::default()).unwrap();
    (doc.trailer().clone(), doc.object_numbers(), doc.modified_objects(), doc.is_modified(), save)
}

/// Protect `fixture()` as [`protect`] does, without its `/Info` dictionary. A full save numbers
/// the trailer's `/Root`, `/Info` and `/Encrypt` first, in that order, so the same seed keeps the
/// dictionary, key and `/ID` but `/Encrypt` is written under another number.
fn protect_moved(alg: Algorithm) -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.trailer_mut().remove(b"Info");
    rekey(&mut doc, alg, 9);
    write_full(&doc, &SaveOptions::default()).unwrap()
}

/// Every incompatible pairing returns `false` and leaves the target exactly as it was; the same
/// overlay with matching security does apply.
#[test]
fn an_overlay_with_different_output_security_is_refused_before_any_change() {
    let encrypted = Arc::new(protect(Algorithm::Aes256, "user", "owner", -1));
    let open_owner = || Document::open_with_password(encrypted.clone(), Some("owner")).unwrap();
    // Each target carries an unsaved edit, so a wrongful overlay would have something to change.
    let encrypted_target = || {
        let mut d = open_owner();
        set_title(&mut d, "Saved title");
        d
    };
    let plain_target = || {
        let mut d = Document::open(Arc::new(fixture())).unwrap();
        set_title(&mut d, "Saved title");
        d
    };
    let mut cases: Vec<(&str, Document, Document)> = Vec::new();
    let mut plain = Document::open(Arc::new(fixture())).unwrap();
    set_title(&mut plain, "Restored title");
    cases.push(("encrypted target, plain snapshot", encrypted_target(), plain));
    let mut protected = open_owner();
    set_title(&mut protected, "Restored title");
    cases.push(("plain target, encrypted snapshot", plain_target(), protected));
    let mut rekeyed = open_owner();
    rekey(&mut rekeyed, Algorithm::Aes256, 3);
    set_title(&mut rekeyed, "Restored title");
    cases.push(("same passwords, new key", encrypted_target(), rekeyed));
    let mut other_alg = open_owner();
    rekey(&mut other_alg, Algorithm::Rc4_128, 9);
    cases.push(("same passwords, other algorithm", encrypted_target(), other_alg));
    // Below revision 5 the first /ID is part of the key.
    let aes128 = Arc::new(protect(Algorithm::Aes128, "user", "owner", -1));
    let mut aes128_target = Document::open_with_password(aes128.clone(), Some("owner")).unwrap();
    set_title(&mut aes128_target, "Saved title");
    let mut other_id = Document::open_with_password(aes128, Some("owner")).unwrap();
    with_first_id(&mut other_id, b"another file id!");
    set_title(&mut other_id, "Restored title");
    cases.push(("AES-128, different first /ID", aes128_target, other_id));

    for (case, mut target, current) in cases {
        assert!(!target.has_same_output_security(&current) && !current.has_same_output_security(&target), "{case}");
        let before = state(&target);
        let result = target.overlay_changes_from(&current);
        assert!(matches!(result, Ok(false)), "{case}: {result:?}");
        assert!(before == state(&target), "{case}: a refused overlay changed the target");
    }

    // A target with a pending security change is refused even when the snapshot matches what
    // that change will write.
    let mut pending = Document::open(Arc::new(fixture())).unwrap();
    rekey(&mut pending, Algorithm::Aes256, 9);
    set_title(&mut pending, "Saved title");
    let matching = Document::open_with_password(Arc::new(protect(Algorithm::Aes256, "user", "owner", -1)), Some("owner")).unwrap();
    assert!(pending.has_same_output_security(&matching));
    let before = state(&pending);
    assert!(matches!(pending.overlay_changes_from(&matching), Ok(false)), "pending security change");
    assert!(before == state(&pending), "a refused overlay changed the target");

    // Control: the same overlay with matching security does change the target.
    let mut target = encrypted_target();
    let mut current = open_owner();
    set_title(&mut current, "Restored title");
    assert!(target.has_same_output_security(&current));
    let before = state(&target);
    assert!(target.overlay_changes_from(&current).unwrap());
    assert!(before != state(&target), "a compatible overlay applies the snapshot");
    assert_eq!(title(&target).as_deref(), Some("Restored title"));
}

/// A full save can move the `/Encrypt` dictionary (renumbered by compaction, or written inline by
/// another tool) without changing the encryption. Undo across it still appends to the saved
/// file, and the update leaves unencrypted the dictionary its trailer names.
#[test]
fn an_overlay_takes_the_encrypt_dictionary_from_the_snapshot() {
    let place = |doc: &Document| match doc.trailer().get(b"Encrypt") {
        Some(Object::Ref(r)) => Some(r.num),
        _ => None,
    };
    for alg in ALL {
        let files =
            [("indirect", protect(alg, "user", "owner", -1)), ("moved", protect_moved(alg)), ("inline", protect_inline(alg, "user", "owner"))];
        let open = |bytes: &Vec<u8>| Document::open_with_password(Arc::new(bytes.clone()), Some("owner")).unwrap();
        for (saved_name, saved) in &files {
            for (snapshot_name, snapshot) in &files {
                if saved_name == snapshot_name {
                    continue;
                }
                let case = format!("{alg:?}: {snapshot_name} snapshot over the {saved_name} save");
                let mut base = open(saved);
                let mut current = open(snapshot);
                assert_ne!(place(&base), place(&current), "{case}");
                assert!(base.has_same_output_security(&current), "{case}");
                set_title(&mut current, "Restored title");
                assert!(base.overlay_changes_from(&current).unwrap(), "{case}");
                let bytes = write_incremental(&base, &SaveOptions::default()).unwrap();
                assert!(bytes.starts_with(saved), "{case}: appended to the saved file");
                assert!(!String::from_utf8_lossy(&bytes).contains("Secret page"), "{case}: plaintext leaked");
                let bytes = Arc::new(bytes);
                assert!(matches!(Document::open(bytes.clone()), Err(CosError::NeedsPassword)), "{case}");
                for password in ["user", "owner"] {
                    let reopened = Document::open_with_password(bytes.clone(), Some(password)).unwrap_or_else(|e| panic!("{case}/{password}: {e:?}"));
                    assert_eq!(place(&reopened), place(&current), "{case}");
                    assert_eq!(content(&reopened), SECRET, "{case}/{password}");
                    assert_eq!(title(&reopened).as_deref(), Some("Restored title"), "{case}/{password}");
                }
                // Independent check, as in `every_algorithm_round_trips_and_hides_the_content`.
                let hay = hayro_syntax::Pdf::new_with_password(bytes.as_ref().clone(), "user").unwrap_or_else(|e| panic!("{case}: {e:?}"));
                let page = hay.pages().first().unwrap().page_stream().map(|s| String::from_utf8_lossy(s).trim().to_string());
                assert_eq!(page.as_deref(), Some(SECRET), "{case}");
            }
        }
    }
}

/// The user and owner passwords unlock the same file key: either opening may be overlaid on the
/// other, and doing so neither elevates nor drops the user's restrictions in the saved file.
#[test]
fn output_security_does_not_depend_on_the_authentication_level() {
    let no_print_or_copy = !(4 | 16);
    for alg in ALL {
        let bytes = Arc::new(protect(alg, "user", "owner", no_print_or_copy));
        let user = Document::open_with_password(bytes.clone(), Some("user")).unwrap();
        let owner = Document::open_with_password(bytes.clone(), Some("owner")).unwrap();
        assert_eq!((user.security().unwrap().auth(), owner.security().unwrap().auth()), (Auth::User, Auth::Owner), "{alg:?}");
        assert!(user.has_same_output_security(&owner) && owner.has_same_output_security(&user), "{alg:?}");
        let mut unlocked = Document::open_with_password(bytes.clone(), Some("user")).unwrap();
        unlocked.unlock("owner").unwrap();
        assert!(unlocked.has_same_output_security(&user), "{alg:?}: unlocking keeps the output security");

        // A snapshot edited as user, overlaid on the owner's opening (and the reverse).
        for (base, password) in [("owner", "user"), ("user", "owner")] {
            let mut target = Document::open_with_password(bytes.clone(), Some(base)).unwrap();
            let mut current = Document::open_with_password(bytes.clone(), Some(password)).unwrap();
            set_title(&mut current, "Edited");
            assert!(target.overlay_changes_from(&current).unwrap(), "{alg:?}");
            let saved = Arc::new(write_incremental(&target, &SaveOptions::default()).unwrap());
            assert!(!String::from_utf8_lossy(&saved).contains("Secret page"), "{alg:?}: plaintext leaked");
            assert!(matches!(Document::open(saved.clone()), Err(CosError::NeedsPassword)), "{alg:?}");
            let reopened = Document::open_with_password(saved.clone(), Some("user")).unwrap();
            assert_eq!(title(&reopened).as_deref(), Some("Edited"), "{alg:?}");
            assert_eq!(content(&reopened), SECRET, "{alg:?}");
            let p = reopened.permissions().unwrap();
            assert!(!p.print() && !p.copy() && p.modify(), "{alg:?}, {base} base: user restrictions survive the save");
            let p = Document::open_with_password(saved, Some("owner")).unwrap().permissions().unwrap();
            assert!(p.print() && p.copy(), "{alg:?}: the owner keeps full rights");
        }
    }
}

/// Compatible output security means the same security dictionary and file key, and below
/// revision 5 the same first file identifier. Each differs alone here; where the dictionary is
/// stored is not compared, and unencrypted documents always match.
#[test]
fn output_security_compares_dictionary_key_and_first_id() {
    let plain = || Document::open(Arc::new(fixture())).unwrap();
    assert!(plain().has_same_output_security(&plain()), "two unencrypted documents");

    for alg in ALL {
        let encrypted = Arc::new(protect(alg, "user", "owner", -1));
        let open = || Document::open_with_password(encrypted.clone(), Some("owner")).unwrap();
        assert!(open().has_same_output_security(&open()), "{alg:?}: the same file opened twice");
        assert!(!plain().has_same_output_security(&open()) && !open().has_same_output_security(&plain()), "{alg:?}");

        // Only the first /ID differs. AES-256 (revision 6) draws its key at random instead.
        let mut other_id = open();
        with_first_id(&mut other_id, b"another file id!");
        assert_eq!(open().has_same_output_security(&other_id), alg == Algorithm::Aes256, "{alg:?}: first /ID");
        // The second /ID changes on every save and is never part of the key.
        let Some(Object::Array(ids)) = open().trailer().get(b"ID").cloned() else { panic!("no /ID") };
        let mut second = open();
        second.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![ids[0].clone(), Object::String(PdfString { bytes: b"x".to_vec(), hex: true })]));
        assert!(open().has_same_output_security(&second), "{alg:?}: second /ID");

        // Protected again with the same passwords: AES-256 draws a new key, and another algorithm
        // is another dictionary. (Below revision 5 the same algorithm derives the same key from
        // the passwords and /ID, so whether it matches is not what this checks.)
        for other in ALL.into_iter().filter(|&other| other != alg || alg == Algorithm::Aes256) {
            let mut rekeyed = open();
            rekey(&mut rekeyed, other, 3);
            assert!(!open().has_same_output_security(&rekeyed), "{alg:?} protected again as {other:?}");
        }

        // Only where the dictionary is stored differs: renumbered, or inline.
        let moved = Document::open_with_password(Arc::new(protect_moved(alg)), Some("owner")).unwrap();
        let inline = Document::open_with_password(Arc::new(protect_inline(alg, "user", "owner")), Some("owner")).unwrap();
        assert_ne!(open().trailer().get(b"Encrypt"), moved.trailer().get(b"Encrypt"), "{alg:?}");
        assert!(matches!(inline.trailer().get(b"Encrypt"), Some(Object::Dict(_))), "{alg:?}");
        for other in [&moved, &inline] {
            assert_eq!(open().security().unwrap().dict(), other.security().unwrap().dict(), "{alg:?}");
            assert!(open().has_same_output_security(other) && other.has_same_output_security(&open()), "{alg:?}: dictionary place");
        }
    }

    // Only the dictionary differs: AES-256 draws its file key from the seed, so new permissions
    // or metadata encryption keep the key and /ID.
    for (permissions, encrypt_metadata) in [(!(4 | 16), true), (-1, false)] {
        let mut a = plain();
        rekey(&mut a, Algorithm::Aes256, 5);
        let mut b = plain();
        b.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "user",
            owner_password: "owner",
            permissions,
            encrypt_metadata,
            seed: [5; 32],
        })
        .unwrap();
        let (sa, sb) = (a.output_handler().unwrap(), b.output_handler().unwrap());
        assert_eq!(sa.file_key(), sb.file_key(), "{permissions}, {encrypt_metadata}");
        assert_ne!(sa.dict(), sb.dict(), "{permissions}, {encrypt_metadata}");
        assert!(!a.has_same_output_security(&b), "{permissions}, {encrypt_metadata}: security dictionary");
    }
}
