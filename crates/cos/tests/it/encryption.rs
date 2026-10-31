//! Encryption end to end: protect a document with every algorithm, save it, and read it back
//! with our reader and with an independent one (hayro-syntax's decryption). Also editing
//! encrypted documents incrementally, changing and removing passwords, and permissions.

use std::io::Write;
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Auth, CosError, Document, NewEncryption, ObjRef, Object, PdfString, SaveOptions, write_full, write_incremental};

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

/// The content stream's reference.
fn contents_of(doc: &Document) -> ObjRef {
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let pages = doc.get(catalog.reference(b"Pages").unwrap()).as_dict().cloned().unwrap();
    let page = doc.resolve(&pages.get(b"Kids").unwrap().as_array().unwrap()[0]).as_dict().cloned().unwrap();
    page.reference(b"Contents").unwrap()
}

/// The content stream's data as loaded, whatever its filters say.
fn raw_content(doc: &Document) -> String {
    match &*doc.get(contents_of(doc)) {
        Object::Stream(s) => String::from_utf8_lossy(&s.raw).trim().to_string(),
        _ => panic!("no content stream"),
    }
}

/// Gives the content stream the `/Filter` and `/DecodeParms` of `selector`.
fn select(doc: &mut Document, (filter, parms): (Object, Object)) {
    let contents = contents_of(doc);
    let Object::Stream(mut s) = doc.get(contents).as_ref().clone() else { panic!("no content stream") };
    s.dict.set(b"Filter".to_vec(), filter);
    s.dict.set(b"DecodeParms".to_vec(), parms);
    doc.set(contents, Object::Stream(s));
}

/// `/Crypt` naming `name`, in the `form`th spelling: all direct, then each part by reference.
/// `None` past the last.
fn selector(doc: &mut Document, form: usize, name: &str) -> Option<(Object, Object)> {
    let crypt = Object::name("Crypt");
    let parms = |n: Object| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Name".to_vec(), n);
        Object::Dict(d)
    };
    let named = parms(Object::name(name));
    Some(match form {
        0 => (crypt, named),
        1 => (crypt, Object::Ref(doc.add(named))),
        2 => (Object::Array(vec![crypt]), Object::Array(vec![Object::Ref(doc.add(named))])),
        3 => (Object::Array(vec![crypt]), Object::Ref(doc.add(Object::Array(vec![named])))),
        4 => (crypt, parms(Object::Ref(doc.add(Object::name(name))))),
        5 => (Object::Ref(doc.add(crypt)), named),
        6 => (Object::Ref(doc.add(Object::Array(vec![crypt]))), Object::Array(vec![named])),
        7 => (Object::Array(vec![Object::Ref(doc.add(crypt))]), Object::Array(vec![named])),
        // Behind a filter given by reference: places in the two arrays pair up all the same.
        8 => (Object::Array(vec![Object::Ref(doc.add(Object::name("ASCIIHexDecode"))), crypt]), Object::Array(vec![Object::Null, named])),
        // The two entries in different forms, which is how a lone filter is usually spelled.
        9 => (Object::Array(vec![crypt]), named),
        10 => (crypt, Object::Array(vec![named])),
        // The filter twice, which names the first's.
        11 => (Object::Array(vec![crypt.clone(), crypt]), Object::Array(vec![named.clone(), named])),
        // A reference to a reference.
        12 => {
            let name = doc.add(Object::name(name));
            let hop = doc.add(Object::Ref(name));
            (crypt, parms(Object::Ref(hop)))
        }
        _ => return None,
    })
}

fn holds(bytes: &[u8], text: &str) -> bool {
    String::from_utf8_lossy(bytes).contains(text)
}

/// A stream can select its crypt filter by reference at every step: the chain, an entry of it, the
/// parameters, an entry of those, the name (ISO 32000-2 §7.3.10, §7.4.10). Read as absent, the
/// filter was `/Identity`, so such a stream was saved in the clear and, in a file from elsewhere,
/// its ciphertext read as data. A full save renumbers: what it follows has the document's numbers.
#[test]
fn a_crypt_filter_selected_by_reference_is_used() {
    for alg in [Algorithm::Aes128, Algorithm::Aes256] {
        for form in 0.. {
            let mut doc = Document::open(Arc::new(protect(alg, "", "owner", -1))).unwrap();
            let Some(selected) = selector(&mut doc, form, "StdCF") else { break };
            select(&mut doc, selected);
            let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
            assert!(!holds(&saved, "Secret page"), "{alg:?} form {form}: saved encrypted");
            let back = Document::open(Arc::new(saved)).unwrap();
            assert_eq!(raw_content(&back), SECRET, "{alg:?} form {form}: read back");
            let full = write_full(&back, &SaveOptions::default()).unwrap();
            assert!(!holds(&full, "Secret page"), "{alg:?} form {form}: renumbered, encrypted");
            assert_eq!(raw_content(&Document::open(Arc::new(full)).unwrap()), SECRET, "{alg:?} form {form}: renumbered");
        }
    }
}

/// Past what one read may follow a reference reads as absent, and a crypt filter with no `/Name` is
/// `/Identity`. Together those would read this stream's ciphertext as data, so what can't be told
/// is the document's own filter, as a `/Filter` that can't be read is.
#[test]
fn a_crypt_filter_past_what_a_read_may_follow_is_the_documents_own() {
    let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let selected = selector(&mut doc, 1, "StdCF").unwrap();
    select(&mut doc, selected);
    // Streams each ahead of the next, whose nine filters by reference are all read: the first of
    // them, read cold, spends all that one read may follow.
    let mut spends = doc.add(Object::Int(0));
    for _ in 0..4 {
        let mut filters: Vec<Object> = (0..8).map(|i| Object::Ref(doc.add(Object::Int(i)))).collect();
        filters.push(Object::Ref(spends));
        let mut dict = pdfcraft_cos::Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(filters));
        spends = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(dict, Vec::new())));
    }
    let contents = contents_of(&doc);
    let Object::Stream(mut s) = doc.get(contents).as_ref().clone() else { panic!("no content stream") };
    s.dict.set(b"Type".to_vec(), Object::Ref(spends));
    doc.set(contents, Object::Stream(s));
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(!holds(&saved, "Secret page"), "saved encrypted");
    let back = Document::open(Arc::new(saved)).unwrap();
    assert_eq!(raw_content(&back), SECRET);
    let full = write_full(&back, &SaveOptions::default()).unwrap();
    assert!(!holds(&full, "Secret page"), "renumbered, encrypted");
    assert_eq!(raw_content(&Document::open(Arc::new(full)).unwrap()), SECRET, "renumbered");
}

/// `/Identity` selected by reference keeps a stream in the clear in a document that encrypts the
/// rest, in every spelling, on both saves.
#[test]
fn an_identity_crypt_filter_selected_by_reference_leaves_a_stream_alone() {
    for form in 0.. {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let Some(selected) = selector(&mut doc, form, "Identity") else { break };
        select(&mut doc, selected);
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(holds(&saved, "Secret page"), "form {form}: written as the stream asks");
        let back = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(raw_content(&back), SECRET, "form {form}");
        let full = write_full(&back, &SaveOptions::default()).unwrap();
        assert!(holds(&full, "Secret page"), "form {form}: renumbered");
        assert_eq!(raw_content(&Document::open(Arc::new(full)).unwrap()), SECRET, "form {form}: renumbered");
    }
}

/// New protection drops a crypt filter it doesn't define, whichever way the stream selected it. The
/// chain comes out direct. Parameters in the stream's own dictionary stay, encrypted with it; of an
/// array in an object of its own, which every stream can share, only what decoders read is copied.
#[test]
fn protecting_drops_a_crypt_filter_selected_by_reference() {
    let protected = |doc: &mut Document| {
        doc.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        write_full(doc, &SaveOptions::default()).unwrap()
    };
    for form in 0.. {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let Some(selected) = selector(&mut doc, form, "MyCF") else { break };
        select(&mut doc, selected);
        let bytes = protected(&mut doc);
        assert!(!holds(&bytes, "Secret page"), "form {form}: encrypted");
        assert!(!holds(&bytes, "MyCF"), "form {form}: no selector left in the stream");
        let back = Document::open(Arc::new(bytes.clone())).unwrap();
        assert_eq!(raw_content(&back), SECRET, "form {form}");
        let contents = back.get(contents_of(&back));
        let filter = contents.as_dict().unwrap().get(b"Filter");
        match form {
            8 => assert!(
                matches!(filter, Some(Object::Array(a)) if matches!(a.as_slice(), [Object::Ref(_)])),
                "form 8: the other filter stays: {filter:?}"
            ),
            _ => assert_eq!(filter, None, "form {form}"),
        }
        assert!(hayro_syntax::Pdf::new(bytes).is_ok(), "form {form}");
    }
    for shared in [false, true] {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let mut named = pdfcraft_cos::Dict::new();
        named.set(b"Name".to_vec(), Object::name("MyCF"));
        let mut other = pdfcraft_cos::Dict::new();
        other.set(b"Note".to_vec(), Object::String(PdfString::text("Private note")));
        other.set(b"Predictor".to_vec(), Object::Int(1));
        let parms = Object::Array(vec![Object::Dict(named), Object::Dict(other)]);
        let parms = if shared { Object::Ref(doc.add(parms)) } else { parms };
        select(&mut doc, (Object::Array(vec![Object::name("Crypt"), Object::name("ASCIIHexDecode")]), parms));
        let bytes = protected(&mut doc);
        assert!(!holds(&bytes, "Private note"), "shared: {shared}");
        let back = Document::open(Arc::new(bytes)).unwrap();
        let contents = back.get(contents_of(&back));
        let kept = contents.as_dict().unwrap().get(b"DecodeParms").unwrap().as_array().unwrap();
        assert_eq!(kept.len(), 1, "one filter left, one entry");
        let kept = kept[0].as_dict().unwrap();
        assert_eq!(kept.get(b"Predictor"), Some(&Object::Int(1)), "shared: {shared}");
        assert_eq!(kept.get(b"Note").is_some(), !shared, "shared: {shared}");
    }
}

/// Every stream of a file can share one large object as its `/DecodeParms` or `/Filter`, which
/// costs nothing until new protection drops a crypt filter from each. Copied whole, a 1 MB file
/// came out at 1 GB.
#[test]
fn protecting_copies_no_large_object_into_every_stream() {
    let pad = Object::Array(vec![Object::Int(1); 100_000]);
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    let mut large = pdfcraft_cos::Dict::new();
    large.set(b"Pad".to_vec(), pad.clone());
    let parms = doc.add(Object::Array(vec![Object::Null, Object::Dict(large)]));
    let filter = doc.add(Object::Array(vec![Object::name("Crypt"), Object::Name(vec![b'A'; 100_000]), pad]));
    let mut streams = Vec::new();
    for _ in 0..200 {
        let mut by_parms = pdfcraft_cos::Dict::new();
        by_parms.set(b"Filter".to_vec(), Object::Array(vec![Object::name("Crypt"), Object::name("ASCIIHexDecode")]));
        by_parms.set(b"DecodeParms".to_vec(), Object::Ref(parms));
        streams.push(Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(by_parms, b"41>".to_vec())))));
        let mut by_filter = pdfcraft_cos::Dict::new();
        by_filter.set(b"Filter".to_vec(), Object::Ref(filter));
        streams.push(Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(by_filter, b"41>".to_vec())))));
    }
    doc.update_dict(doc.root().unwrap(), |c| c.set(b"Test".to_vec(), Object::Array(streams))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(bytes.len() < 1 << 20, "{} bytes", bytes.len());
}

/// A file can name one large string thousands of times in `/ID`, which holds two. An encrypted file
/// keeps its `/ID`, so each copy would be in the file a save writes.
#[test]
fn a_file_identifier_of_many_references_is_two_strings() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes128,
        user_password: "",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    let large = doc.add(Object::String(PdfString { bytes: vec![0xAB; 100_000], hex: true }));
    doc.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![Object::Ref(large); 500]));
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(bytes.len() < 1 << 20, "{} bytes", bytes.len());
}

/// What a file can do with those references: point a stream's selector at the stream itself, or at
/// nothing. They read as absent, so a stream with `/Crypt` is `/Identity` (ISO 32000-2 §7.4.10),
/// and one that names no filter follows the document's. Loading ends either way.
#[test]
fn a_crypt_filter_reference_to_itself_or_nothing_reads_as_absent() {
    let named = |n: Object| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Name".to_vec(), n);
        Object::Dict(d)
    };
    for (what, encrypted) in [
        ("/Filter to itself", true),
        ("/Filter nowhere", true),
        ("/DecodeParms to itself", false),
        ("/DecodeParms nowhere", false),
        ("/Name to itself", false),
        ("/Name nowhere", false),
    ] {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let me = Object::Ref(contents_of(&doc));
        let nowhere = Object::Ref(ObjRef::new(9_999, 0));
        let crypt = Object::name("Crypt");
        select(
            &mut doc,
            match what {
                "/Filter to itself" => (me, Object::Null),
                "/Filter nowhere" => (nowhere, Object::Null),
                "/DecodeParms to itself" => (crypt, me),
                "/DecodeParms nowhere" => (crypt, nowhere),
                "/Name to itself" => (crypt, named(me)),
                _ => (crypt, named(nowhere)),
            },
        );
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert_eq!(!holds(&saved, "Secret page"), encrypted, "{what}");
        assert_eq!(raw_content(&Document::open(Arc::new(saved)).unwrap()), SECRET, "{what}");
    }
}

/// A lone parameter dictionary belongs to the first filter, so `/Crypt` behind another filter has
/// none of its own and is `/Identity` (ISO 32000-2 §7.4.1), whatever that dictionary says.
#[test]
fn a_lone_parameter_dictionary_is_the_first_filters() {
    let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let mut parms = pdfcraft_cos::Dict::new();
    parms.set(b"Name".to_vec(), Object::name("StdCF"));
    let first = Object::Array(vec![Object::name("Crypt"), Object::name("ASCIIHexDecode")]);
    let second = Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("Crypt")]);
    select(&mut doc, (first, Object::Dict(parms.clone())));
    assert!(!holds(&write_incremental(&doc, &SaveOptions::default()).unwrap(), "Secret page"), "first: it names StdCF");
    select(&mut doc, (second, Object::Dict(parms)));
    assert!(holds(&write_incremental(&doc, &SaveOptions::default()).unwrap(), "Secret page"), "second: it names nothing");
}

/// A lone parameter dictionary is the first filter's, so dropping a `/Crypt` behind that filter
/// leaves it, and dropping one in front of it takes it away.
#[test]
fn protecting_keeps_the_parameters_of_the_filter_before_a_crypt_filter() {
    let protected = |filters: Vec<Object>| {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let mut parms = pdfcraft_cos::Dict::new();
        parms.set(b"Name".to_vec(), Object::name("MyCF"));
        select(&mut doc, (Object::Array(filters), Object::Dict(parms)));
        doc.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
        let back = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        back.get(contents_of(&back)).as_dict().unwrap().contains(b"DecodeParms")
    };
    let (hex, crypt) = (Object::name("ASCIIHexDecode"), Object::name("Crypt"));
    assert!(protected(vec![hex.clone(), crypt.clone()]), "the first filter's, which stays");
    assert!(!protected(vec![crypt, hex]), "/Crypt's, which goes");
}

/// Dropping `/Crypt` takes its entry out of `/DecodeParms`, and an array too short to have one is
/// left as it is, given directly or by reference.
#[test]
fn protecting_leaves_a_parameter_array_too_short_for_the_crypt_filter() {
    for by_reference in [false, true] {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        let short = Object::Array(vec![Object::Null]);
        let parms = if by_reference { Object::Ref(doc.add(short)) } else { short };
        select(&mut doc, (Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("Crypt")]), parms));
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
        assert!(!holds(&bytes, "Secret page"), "by reference: {by_reference}");
        let back = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(raw_content(&back), SECRET, "by reference: {by_reference}");
        let contents = back.get(contents_of(&back));
        let filter = contents.as_dict().unwrap().get(b"Filter").unwrap().as_array().unwrap();
        assert_eq!(filter.len(), 1, "only /Crypt goes");
        assert_eq!(filter[0].as_name(), Some(b"ASCIIHexDecode".as_slice()), "the other filter stays as it was");
    }
}

/// A chain of up to [`pdfcraft_filters::MAX_CHAIN`] filters and `/Crypt` is read; a longer one
/// names no crypt filter, so its entries aren't followed for every stream sharing it.
#[test]
fn a_chain_longer_than_any_decoder_takes_names_no_crypt_filter() {
    let chain = |others: usize| {
        let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
        let mut filters = vec![Object::name("ASCIIHexDecode"); others];
        filters.push(Object::name("Crypt"));
        let array = doc.add(Object::Array(filters));
        select(&mut doc, (Object::Ref(array), Object::Null));
        holds(&write_incremental(&doc, &SaveOptions::default()).unwrap(), "Secret page")
    };
    assert!(chain(pdfcraft_filters::MAX_CHAIN), "the longest chain read: /Identity, in the clear");
    assert!(!chain(pdfcraft_filters::MAX_CHAIN + 1), "one more: the document's filter");
}

/// A stream's `/Type` by reference picks its handling like one given directly: metadata left
/// readable under `/EncryptMetadata false`, an embedded file under `/EFF`.
#[test]
fn a_stream_type_given_by_reference_picks_its_crypt_filter() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let meta = catalog.reference(b"Metadata").unwrap();
    let ty = doc.add(Object::name("Metadata"));
    let Object::Stream(mut s) = doc.get(meta).as_ref().clone() else { panic!("no metadata stream") };
    s.dict.set(b"Type".to_vec(), Object::Ref(ty));
    doc.set(meta, Object::Stream(s));
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
    assert!(holds(&bytes, "<x:xmpmeta>meta</x:xmpmeta>"), "search engines can read XMP");

    let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
    doc.update_dict(encrypt, |e| e.set(b"EFF".to_vec(), Object::name("Identity"))).unwrap();
    let mut doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let contents = contents_of(&doc);
    let ty = doc.add(Object::name("EmbeddedFile"));
    let Object::Stream(mut s) = doc.get(contents).as_ref().clone() else { panic!("no content stream") };
    s.dict.set(b"Type".to_vec(), Object::Ref(ty));
    doc.set(contents, Object::Stream(s));
    assert!(holds(&write_incremental(&doc, &SaveOptions::default()).unwrap(), "Secret page"), "/EFF /Identity leaves it alone");
}

/// What makes a dictionary a signature's can be given by reference, and what is null or refers to
/// nothing is absent: `/Contents` stays as written in each of these.
#[test]
fn a_signature_dictionary_is_known_in_every_form() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    let ty = doc.add(Object::name("Sig"));
    let range = Object::Array([0, 10, 20, 30].into_iter().map(Object::Int).collect());
    let by_reference = doc.add(range.clone());
    let nowhere = Object::Ref(ObjRef::new(9_999, 0));
    let forms: [(Vec<(&str, Object)>, &str); 5] = [
        (vec![("Type", Object::Ref(ty))], "DEAD"),
        (vec![("ByteRange", Object::Ref(by_reference))], "BEEF"),
        (vec![("Type", Object::Null), ("ByteRange", range.clone())], "CAFE"),
        (vec![("Type", nowhere.clone()), ("ByteRange", range.clone())], "F00D"),
        (vec![("Subtype", nowhere), ("ByteRange", range)], "D00D"),
    ];
    let mut signatures = Vec::new();
    for (entries, contents) in forms {
        let mut sig = pdfcraft_cos::Dict::new();
        for (key, value) in entries {
            sig.set(key.as_bytes().to_vec(), value);
        }
        let bytes = (0..contents.len()).step_by(2).map(|i| u8::from_str_radix(&contents[i..i + 2], 16).unwrap()).collect();
        sig.set(b"Contents".to_vec(), Object::String(PdfString { bytes, hex: true }));
        signatures.push(Object::Ref(doc.add(Object::Dict(sig))));
    }
    // Nested in another object, it follows nothing, but known directly it is one all the same.
    let mut nested = pdfcraft_cos::Dict::new();
    nested.set(b"Type".to_vec(), Object::name("Sig"));
    nested.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0xAB, 0xBA], hex: true }));
    let mut field = pdfcraft_cos::Dict::new();
    field.set(b"V".to_vec(), Object::Dict(nested));
    signatures.push(Object::Ref(doc.add(Object::Dict(field))));
    // Neither of what makes one: a string /Contents alone is anybody's, so it's encrypted.
    let mut other = pdfcraft_cos::Dict::new();
    other.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0xAB, 0xCD], hex: true }));
    signatures.push(Object::Ref(doc.add(Object::Dict(other))));
    doc.update_dict(doc.root().unwrap(), |c| c.set(b"Test".to_vec(), Object::Array(signatures))).unwrap();
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
    for contents in ["DEAD", "BEEF", "CAFE", "F00D", "D00D", "ABBA"] {
        assert!(holds(&bytes, &format!("<{contents}>")), "{contents}: written as is");
    }
    assert!(!holds(&bytes, "<ABCD>"), "without a /ByteRange it is no signature");
}

/// What a test needs to write an update of a classic file protected with AES-128.
struct Classic {
    bytes: Vec<u8>,
    root: ObjRef,
    pages: ObjRef,
    info: ObjRef,
    encrypt: ObjRef,
    id: Vec<u8>,
    dict: pdfcraft_cos::Dict,
}

fn classic_file() -> Classic {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes128,
        user_password: "",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [9; 32],
    })
    .unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let encrypt = doc.trailer().reference(b"Encrypt").unwrap();
    let id = doc.trailer().get(b"ID").unwrap().as_array().unwrap()[0].as_string().unwrap().bytes.clone();
    let dict = doc.get(encrypt).as_dict().cloned().unwrap();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    Classic { root: doc.root().unwrap(), pages, info: doc.trailer().reference(b"Info").unwrap(), bytes, encrypt, id, dict }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// `bytes` with an update appended: `objects`, and a trailer of `trailer` entries, in which `@n`
/// stands for the offset of object `n`. An object with nothing in it gets an entry of the
/// cross-reference table that points at nothing.
fn updated(bytes: &[u8], objects: &[(u32, Vec<u8>)], trailer: &str) -> Vec<u8> {
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    let prev: usize = String::from_utf8_lossy(&bytes[at + 9..]).split_whitespace().next().unwrap().parse().unwrap();
    let mut out = bytes.to_vec();
    let mut table = String::from("xref\n");
    let mut trailer = trailer.to_string();
    for (num, body) in objects {
        if body.is_empty() {
            table.push_str(&format!("{num} 1\n0000000010 00000 n \n"));
            continue;
        }
        trailer = trailer.replace(&format!("@{num}"), &out.len().to_string());
        table.push_str(&format!("{num} 1\n{:010} 00000 n \n", out.len()));
        out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(table.as_bytes());
    out.extend_from_slice(format!("trailer\n<< {trailer} /Size 2000 /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// The `/Encrypt` dictionary of `file`, with `/O`, `/StmF` and `/StrF` as given.
fn encrypt_with(file: &Classic, o: &str, filter: &str) -> Vec<u8> {
    let string = |key: &[u8]| hex(&file.dict.get(key).unwrap().as_string().unwrap().bytes);
    format!(
        "<< /Filter /Standard /V 4 /R 4 /Length 128 /P {} /O {o} /U <{}> /CF << /StdCF << /CFM /AESV2 /Length 16 /AuthEvent /DocOpen >> >> /StmF /{filter} /StrF /{filter} >>",
        file.dict.int(b"P").unwrap(),
        string(b"U")
    )
    .into_bytes()
}

/// An object stream can have its parameters by reference, and a stream's crypt filter can be
/// selected by a member of it: legal together, though each is rare. What reads the stream first
/// goes through the object stream, and members must come out of it as they do read directly. They
/// didn't, while a read made for a selector wasn't allowed to follow its own: the object stream was
/// kept undecoded, its members lost for good, and a full save wrote nulls without saying so.
#[test]
fn an_object_stream_reached_from_a_selector_keeps_its_members() {
    let file = classic_file();
    let parms = "[<< /Name /Identity >> null]";
    let header = format!("902 0 903 {} ", parms.len() + 1);
    let members = format!("{header}{parms} << /Keep (yes) >>");
    let (mut rows, mut before) = (vec![1u8], 0u8);
    for b in members.bytes() {
        rows.push(b.wrapping_sub(before));
        before = b;
    }
    let coded = pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), &rows).raw;
    let mut object_stream =
        format!("<< /Type /ObjStm /N 2 /First {} /Filter /FlateDecode /DecodeParms 911 0 R /Length {} >>\nstream\n", header.len(), coded.len())
            .into_bytes();
    object_stream.extend_from_slice(&coded);
    object_stream.extend_from_slice(b"\nendstream");
    let data = [2u8, 0x03, 0x8E, 0, 2, 0x03, 0x8E, 1];
    let mut xref = format!("<< /Type /XRef /Size 2000 /W [1 2 1] /Index [902 2] /Length {} >>\nstream\n", data.len()).into_bytes();
    xref.extend_from_slice(&data);
    xref.extend_from_slice(b"\nendstream");
    let string = |key: &[u8]| format!("<{}>", hex(&file.dict.get(key).unwrap().as_string().unwrap().bytes));
    let objects = [
        (file.root.num, format!("<< /Type /Catalog /Pages {} 0 R /A 900 0 R /B 903 0 R >>", file.pages.num).into_bytes()),
        (file.encrypt.num, encrypt_with(&file, &string(b"O"), "Identity")),
        (900, b"<< /Filter [/Crypt /ASCIIHexDecode] /DecodeParms 902 0 R /Length 1 >>\nstream\n>\nendstream".to_vec()),
        (910, object_stream),
        (911, format!("<< /Predictor 11 /Columns {} >>", members.len()).into_bytes()),
        (912, xref),
    ];
    let trailer = format!("/Root {} 0 R /Encrypt {} 0 R /ID [<{id}> <{id}>] /XRefStm @912", file.root.num, file.encrypt.num, id = hex(&file.id));
    let bytes = updated(&file.bytes, &objects, &trailer);
    let keep =
        |doc: &Document, r: &Object| doc.resolve(r).as_dict().and_then(|d| d.get(b"Keep")).and_then(Object::as_string).map(|s| s.bytes.clone());
    let member = Object::Ref(ObjRef::new(903, 0));
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    assert_eq!(keep(&doc, &member).as_deref(), Some(b"yes".as_slice()), "read first");
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let _ = doc.get(ObjRef::new(900, 0));
    assert_eq!(keep(&doc, &member).as_deref(), Some(b"yes".as_slice()), "read after the stream selecting from it");
    let saved = write_full(&Document::open(Arc::new(bytes)).unwrap(), &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved)).unwrap();
    let catalog = back.get(back.root().unwrap());
    assert_eq!(keep(&back, catalog.as_dict().unwrap().get(b"B").unwrap()).as_deref(), Some(b"yes".as_slice()), "after a full save");
}

/// `walk` goes as deep as an object nests, so following a reference from down there adds that depth
/// again for each object it loads: this file, under 10 KB, overflowed a 2 MiB stack and took the
/// process with it. Only the object itself follows references, and it does so in very little stack.
#[test]
fn nested_dictionaries_load_nothing_on_a_small_stack() {
    let file = classic_file();
    let first = 960u32;
    let objects: Vec<(u32, Vec<u8>)> = (first..first + 40)
        .map(|num| {
            let open = "[".repeat(120);
            let close = "]".repeat(120);
            (num, format!("{open}<< /Contents () /Type {} 0 R >>{close}", num + 1).into_bytes())
        })
        .collect();
    let trailer = format!("/Root {} 0 R /Encrypt {} 0 R /ID [<{id}> <{id}>]", file.root.num, file.encrypt.num, id = hex(&file.id));
    let bytes = updated(&file.bytes, &objects, &trailer);
    let reading = std::thread::Builder::new().stack_size(1 << 20).spawn(move || {
        let doc = Document::open(Arc::new(bytes)).unwrap();
        doc.get(ObjRef::new(first, 0)).as_array().is_some()
    });
    assert_eq!(reading.unwrap().join().ok(), Some(true));
}

/// The object itself can follow references, a ladder of them as long as loading may nest, and that
/// fits a small stack too.
#[test]
fn a_ladder_of_signature_checks_fits_a_small_stack() {
    let file = classic_file();
    let first = 960u32;
    let objects: Vec<(u32, Vec<u8>)> =
        (first..first + 40).map(|num| (num, format!("<< /Contents () /Type {} 0 R >>", num + 1).into_bytes())).collect();
    let trailer = format!("/Root {} 0 R /Encrypt {} 0 R /ID [<{id}> <{id}>]", file.root.num, file.encrypt.num, id = hex(&file.id));
    let bytes = updated(&file.bytes, &objects, &trailer);
    let reading = std::thread::Builder::new().stack_size(1 << 20).spawn(move || {
        let doc = Document::open(Arc::new(bytes)).unwrap();
        doc.get(ObjRef::new(first, 0)).as_dict().is_some()
    });
    assert_eq!(reading.unwrap().join().ok(), Some(true));
}

/// The strings of `/Encrypt` are never encrypted (ISO 32000-2 §7.6.2), and can be objects of their
/// own. Read as ordinary strings they were decrypted, and a full save encrypted them again under
/// another number, so the file it wrote no longer opened with its own password.
#[test]
fn encrypt_strings_given_by_reference_stay_as_stored() {
    let file = classic_file();
    let string = |key: &[u8]| hex(&file.dict.get(key).unwrap().as_string().unwrap().bytes);
    let replaced = format!(
        "<< /Filter /Standard /V 4 /R 4 /Length 128 /P {} /O 900 0 R /U 901 0 R /CF << /StdCF << /CFM /AESV2 /Length 16 /AuthEvent /DocOpen >> >> /StmF /StdCF /StrF /StdCF >>",
        file.dict.int(b"P").unwrap()
    );
    let objects = [
        (file.encrypt.num, replaced.into_bytes()),
        (900, format!("<{}>", string(b"O")).into_bytes()),
        (901, format!("<{}>", string(b"U")).into_bytes()),
    ];
    let trailer =
        format!("/Root {} 0 R /Info {} 0 R /Encrypt {} 0 R /ID [<{id}> <{id}>]", file.root.num, file.info.num, file.encrypt.num, id = hex(&file.id));
    let source = Document::open(Arc::new(updated(&file.bytes, &objects, &trailer))).unwrap();
    assert_eq!(content(&source), SECRET, "the file reads");
    assert_eq!(title(&source).as_deref(), Some("Quarterly numbers"));
    for object_streams in [false, true] {
        let saved = write_full(&source, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap();
        let back = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(content(&back), SECRET, "object streams: {object_streams}");
        assert_eq!(title(&back).as_deref(), Some("Quarterly numbers"), "object streams: {object_streams}");
    }
}

/// `/ID` has to be direct in an encrypted file, but one that isn't has its key made from the same
/// bytes, and a full save writes it direct.
#[test]
fn a_file_identifier_given_by_reference_is_read() {
    let file = classic_file();
    let id = hex(&file.id);
    let reference = |entries: &str, objects: &[(u32, Vec<u8>)]| {
        let trailer = format!("/Root {} 0 R /Encrypt {} 0 R /ID {entries}", file.root.num, file.encrypt.num);
        updated(&file.bytes, objects, &trailer)
    };
    let whole = reference("900 0 R", &[(900, format!("[<{id}> <{id}>]").into_bytes())]);
    let entries = reference("[900 0 R 901 0 R]", &[(900, format!("<{id}>").into_bytes()), (901, format!("<{id}>").into_bytes())]);
    for file in [whole, entries] {
        let source = Document::open(Arc::new(file)).unwrap();
        assert_eq!(content(&source), SECRET);
        let back = Document::open(Arc::new(write_full(&source, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(content(&back), SECRET);
        assert!(matches!(back.trailer().get(b"ID"), Some(Object::Array(_))), "written direct");
    }
}

/// A direct `/Encrypt` can refer to objects. A full save dropped those it found only there, and kept
/// its references as they were under the old numbers, so nothing in the file read afterwards.
#[test]
fn a_direct_encrypt_dictionary_keeps_what_it_refers_to() {
    for object_streams in [false, true] {
        let mut doc = Document::open(Arc::new(fixture())).unwrap();
        doc.set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes128,
            user_password: "",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [9; 32],
        })
        .unwrap();
        let mut dict = doc.get(doc.trailer().reference(b"Encrypt").unwrap()).as_dict().cloned().unwrap();
        let filter = doc.add(Object::name("StdCF"));
        let filters = doc.add(dict.get(b"CF").unwrap().clone());
        dict.set(b"StmF".to_vec(), Object::Ref(filter));
        dict.set(b"CF".to_vec(), Object::Ref(filters));
        doc.trailer_mut().set(b"Encrypt".to_vec(), Object::Dict(dict));
        let options = SaveOptions { object_streams, ..SaveOptions::default() };
        let saved = write_full(&doc, &options).unwrap();
        let back = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(content(&back), SECRET, "object streams: {object_streams}");
        let again = write_full(&back, &options).unwrap();
        assert_eq!(content(&Document::open(Arc::new(again)).unwrap()), SECRET, "object streams: {object_streams}, saved again");
    }
}

/// How a stream is treated can be given by reference, and what that loads can be a stream too. A
/// file can make a ladder of those, where each rung reads the rung below, then two trees wider than
/// the object cache, which push that rung out of it, then the rung below again. Each rung would
/// take twice what the one below does, if one read could follow any number of references.
#[test]
fn selectors_that_lead_to_selectors_load_at_once() {
    let mut doc = Document::open(Arc::new(protect(Algorithm::Aes128, "", "owner", -1))).unwrap();
    let stream_of = |doc: &mut Document, filters: Vec<Object>| {
        let mut dict = pdfcraft_cos::Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(filters));
        doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(dict, Vec::new())))
    };
    let tree = |doc: &mut Document| {
        let mut level: Vec<ObjRef> = (0..6_561).map(|i| doc.add(Object::Int(i))).collect();
        while level.len() > 1 {
            let chunks: Vec<Vec<Object>> = level.chunks(9).map(|c| c.iter().map(|r| Object::Ref(*r)).collect()).collect();
            level = chunks.into_iter().map(|c| stream_of(doc, c)).collect();
        }
        Object::Ref(level[0])
    };
    let (first, second) = (tree(&mut doc), tree(&mut doc));
    let mut top = doc.add(Object::Int(0));
    // Twelve rungs of two, and the trees' five levels, stay inside the 32 that loading may nest.
    for _ in 0..12 {
        let (a, b) = (stream_of(&mut doc, vec![Object::Ref(top)]), stream_of(&mut doc, vec![Object::Ref(top)]));
        top = stream_of(&mut doc, vec![Object::Ref(a), first.clone(), second.clone(), Object::Ref(b)]);
    }
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let doc = Document::open(Arc::new(saved)).unwrap();
        let _ = done.send(doc.get(top).as_dict().is_some());
    });
    // With the budget it takes milliseconds, and without it longer than anyone would wait, so a
    // margin this wide can't fail on a busy machine.
    assert_eq!(finished.recv_timeout(std::time::Duration::from_secs(120)), Ok(true), "loading must not fan out");
}
