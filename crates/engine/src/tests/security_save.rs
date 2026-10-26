//! Security changes must stay coherent when undo crosses the last saved revision (#3276).
use super::*;
use pdfcraft_cos::{Algorithm, Document as CosDocument, NewEncryption, Object, SaveOptions, write_full};

/// A two-page file with a title and XMP metadata, protected with `user`/`owner` and no
/// restrictions. `inline` moves `/Encrypt` into the trailer, as some writers store it.
fn encrypted_fixture(algorithm: Algorithm, inline: bool) -> Arc<Vec<u8>> {
    let mut doc = CosDocument::open(Arc::new(fixture(2))).unwrap();
    pdfcraft_organize::set_info(&mut doc, "Title", "Security regression title").unwrap();
    let mut metadata = pdfcraft_cos::Dict::new();
    metadata.set(b"Type".to_vec(), Object::name("Metadata"));
    metadata.set(b"Subtype".to_vec(), Object::name("XML"));
    let metadata = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(metadata, b"<metadata>Security regression</metadata>".to_vec())));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"Metadata".to_vec(), Object::Ref(metadata))).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    if inline {
        let r = doc.trailer().reference(b"Encrypt").unwrap();
        let dict = doc.get(r).as_dict().cloned().unwrap();
        doc.trailer_mut().set(b"Encrypt".to_vec(), Object::Dict(dict));
    }
    Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())
}

/// Check the saved file, not the still-correct in-memory snapshot. A catalog/page-count
/// check alone accepts the broken reverse transition with two blank pages.
fn assert_saved_content(s: &Session, id: DocId, bytes: Arc<Vec<u8>>, password: Option<&str>, encrypted: bool) -> Result<(), String> {
    let check = s.check_save(id, &bytes);
    let plain_marker = bytes.windows(b"(Page 1)".len()).any(|w| w == b"(Page 1)");
    let mut reopened = Session::new();
    let restored = reopened.open("saved.pdf", None, bytes.clone(), password);
    let restored = restored.map_err(|e| format!("check_save={check:?}, plaintext marker={plain_marker}, reopen={e:?}"))?;
    let texts = page_texts(&reopened, restored);
    if check.is_err() || texts != ["Page 1", "Page 2"] {
        return Err(format!("check_save={check:?}, plaintext marker={plain_marker}, text={texts:?}"));
    }
    assert_eq!(reopened.get(restored).unwrap().info.encrypted, encrypted);
    assert_eq!(reopened.get(restored).unwrap().info_value("Title"), s.get(id).unwrap().info_value("Title"));
    fn metadata(doc: &CosDocument) -> Option<Vec<u8>> {
        let catalog = doc.get(doc.root()?);
        let stream = doc.resolve(catalog.as_dict()?.get(b"Metadata")?);
        let Object::Stream(stream) = stream.as_ref() else { return None };
        Some(stream.decoded().unwrap().to_vec())
    }
    assert_eq!(metadata(&reopened.get(restored).unwrap().editor.as_ref().unwrap().cos), metadata(&s.get(id).unwrap().editor.as_ref().unwrap().cos));
    if encrypted {
        assert!(!plain_marker, "encrypted saves must not expose page content");
        assert!(CosDocument::open_with_password(bytes, Some("wrong")).is_err());
    } else {
        // The fixture's content is stored unfiltered: without this, the check above could pass
        // only because the writer started compressing streams.
        assert!(plain_marker, "a plain save shows the page content");
    }
    Ok(())
}

/// Remove protection, save, undo, save: the file is encrypted again with its original passwords.
#[test]
fn undo_saved_security_removal_restores_encrypted_content() {
    let mut failures = Vec::new();
    for algorithm in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        for inline in [false, true] {
            for password in ["owner", "user"] {
                let mut s = Session::new();
                let original = encrypted_fixture(algorithm, inline);
                let id = s.open("encrypted.pdf", None, original.clone(), Some(password)).unwrap();
                s.apply(id, Edit::RemoveProtection).unwrap();
                let plain = s.save_bytes(id).unwrap();
                assert_saved_content(&s, id, plain.clone(), None, false).unwrap();
                s.mark_saved(id, plain, None).unwrap();
                s.undo(id).unwrap();
                assert_saved_content(&s, id, s.save_full_bytes(id).unwrap(), Some(password), true).unwrap();
                let restored = s.save_bytes(id).unwrap();
                if let Err(error) = assert_saved_content(&s, id, restored.clone(), Some(password), true) {
                    failures.push(format!("{algorithm:?}, inline={inline}, {password}: {error}"));
                    continue;
                }
                assert!(restored.starts_with(original.as_slice()), "appended to the encrypted original, not rewritten");
                // Both original passwords still work; saving also retains this session's rights.
                assert_saved_content(&s, id, restored.clone(), Some("user"), true).unwrap();
                assert_saved_content(&s, id, restored.clone(), Some("owner"), true).unwrap();
                s.mark_saved(id, restored.clone(), None).unwrap();
                assert!(!s.get(id).unwrap().dirty);
                assert_eq!(*s.save_bytes(id).unwrap(), *restored, "an unchanged save is byte-stable");
                s.redo(id).unwrap();
                let plain = s.save_bytes(id).unwrap();
                assert_saved_content(&s, id, plain.clone(), None, false).unwrap();
                s.mark_saved(id, plain, None).unwrap();
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Protect, save, undo, save: the file is plain again and readable without a password.
#[test]
fn undo_saved_protection_restores_plain_content() {
    let mut failures = Vec::new();
    for algorithm in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let (mut s, id) = session_with(2);
        let original = s.get(id).unwrap().bytes.clone();
        s.apply(id, Edit::Protect(Protection { algorithm, ..protection(Some("user"), Some("owner")) })).unwrap();
        let protected = s.save_bytes(id).unwrap();
        assert_saved_content(&s, id, protected.clone(), Some("owner"), true).unwrap();
        s.mark_saved(id, protected, None).unwrap();
        s.undo(id).unwrap();
        assert_saved_content(&s, id, s.save_full_bytes(id).unwrap(), None, false).unwrap();
        let restored = s.save_bytes(id).unwrap();
        if let Err(error) = assert_saved_content(&s, id, restored.clone(), None, false) {
            failures.push(format!("{algorithm:?}: {error}"));
            continue;
        }
        assert!(restored.starts_with(original.as_slice()), "{algorithm:?}: appended to the plain original");
        s.mark_saved(id, restored.clone(), None).unwrap();
        assert!(!s.get(id).unwrap().dirty);
        assert_eq!(*s.save_bytes(id).unwrap(), *restored);
        s.redo(id).unwrap();
        let protected = s.save_bytes(id).unwrap();
        assert_saved_content(&s, id, protected.clone(), Some("owner"), true).unwrap();
        s.mark_saved(id, protected, None).unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Undoing a saved change of algorithm, permissions, metadata encryption or key restores the
/// original security dictionary, even though the password strings never changed.
#[test]
fn undo_saved_rekey_with_the_same_passwords_restores_original_security() {
    let mut failures = Vec::new();
    // Identical password strings are not a security identity: changing algorithm, permission
    // bits, metadata encryption, or simply generating a fresh AES-256 key changes the output.
    for (algorithm, copy, encrypt_metadata) in [
        (Algorithm::Rc4_128, true, true),
        (Algorithm::Aes128, true, true),
        (Algorithm::Aes256, false, true),
        (Algorithm::Aes256, true, false),
        (Algorithm::Aes256, true, true),
    ] {
        let (mut s, id) = session_with(2);
        s.apply(id, Edit::Protect(protection(Some("user"), Some("owner")))).unwrap();
        let protected = s.save_bytes(id).unwrap();
        s.mark_saved(id, protected, None).unwrap();
        let original = s.get(id).unwrap().editor.as_ref().unwrap().cos.output_handler().unwrap().dict().clone();
        s.apply(id, Edit::Protect(Protection { algorithm, copy, encrypt_metadata, ..protection(Some("user"), Some("owner")) })).unwrap();
        let protected = s.save_bytes(id).unwrap();
        s.mark_saved(id, protected, None).unwrap();
        s.undo(id).unwrap();
        let restored = s.save_bytes(id).unwrap();
        let editor = s.get(id).unwrap().editor.as_ref().unwrap();
        assert_eq!(editor.keys, editor.saved_keys, "password strings alone cannot detect this transition");
        if let Err(error) = assert_saved_content(&s, id, restored.clone(), Some("owner"), true) {
            failures.push(format!("{algorithm:?}, copy={copy}, metadata={encrypt_metadata}: {error}"));
            continue;
        }
        let reopened = CosDocument::open_with_password(restored, Some("owner")).unwrap();
        assert_eq!(reopened.security().unwrap().dict(), &original);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A full save renumbers `/Encrypt` but keeps the encryption. Undo past it appends to that save,
/// keeping its revision, and leaves the restored dictionary unencrypted.
#[test]
fn undo_encrypted_compaction_restores_the_encrypt_dictionary_exemption() {
    let mut cos = CosDocument::open(Arc::new(fixture(3))).unwrap();
    cos.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let original = Arc::new(write_full(&cos, &SaveOptions::default()).unwrap());
    let original = CosDocument::open_with_password(original, Some("owner")).unwrap();
    let mut s = Session::new();
    let id = s.open("compact.pdf", None, original.bytes().clone(), Some("owner")).unwrap();
    s.apply(id, Edit::DeletePages { pages: vec![0] }).unwrap();
    let compact = s.save_full_bytes(id).unwrap();
    let compact_doc = CosDocument::open_with_password(compact.clone(), Some("owner")).unwrap();
    assert_eq!(original.security().unwrap().dict(), compact_doc.security().unwrap().dict());
    assert_ne!(original.trailer().reference(b"Encrypt"), compact_doc.trailer().reference(b"Encrypt"), "compaction renumbered /Encrypt");
    s.mark_saved(id, compact.clone(), None).unwrap();
    s.undo(id).unwrap();
    let restored = s.save_bytes(id).unwrap();
    s.check_save(id, &restored).unwrap();
    assert!(restored.starts_with(compact.as_slice()), "the same encryption still appends to the last save");
    assert!(!restored.windows(b"(Page 1)".len()).any(|w| w == b"(Page 1)"), "page content stays encrypted");
    assert!(CosDocument::open(restored.clone()).is_err() && CosDocument::open_with_password(restored.clone(), Some("wrong")).is_err());
    let mut reopened = Session::new();
    let id = reopened.open("restored.pdf", None, restored, Some("user")).unwrap();
    assert!(reopened.get(id).unwrap().info.encrypted);
    assert_eq!(page_texts(&reopened, id), ["Page 1", "Page 2", "Page 3"]);
}

/// A restricted user who unlocks with the owner password, removes protection, saves and undoes
/// gets the original restrictions back, not an unrestricted or owner-only file.
#[test]
fn undo_saved_removal_after_unlock_keeps_the_user_restrictions() {
    for algorithm in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let (mut s, id) = session_with(2);
        s.apply(id, Edit::Protect(Protection { algorithm, ..protection(Some("user"), Some("owner")) })).unwrap();
        let protected = s.save_bytes(id).unwrap();
        let p = CosDocument::open_with_password(protected.clone(), Some("user")).unwrap().permissions().unwrap();
        assert!(!p.owner && !p.copy() && !p.modify(), "{algorithm:?}: the fixture restricts the user");

        let mut s = Session::new();
        let id = s.open("restricted.pdf", None, protected, Some("user")).unwrap();
        assert!(matches!(s.apply(id, Edit::RemoveProtection), Err(EditError::NotPermitted(_))), "{algorithm:?}");
        s.unlock(id, "owner").unwrap();
        s.apply(id, Edit::RemoveProtection).unwrap();
        let plain = s.save_bytes(id).unwrap();
        assert_saved_content(&s, id, plain.clone(), None, false).unwrap();
        s.mark_saved(id, plain, None).unwrap();
        s.undo(id).unwrap();
        let restored = s.save_bytes(id).unwrap();
        assert_saved_content(&s, id, restored.clone(), Some("owner"), true).unwrap();
        let user = CosDocument::open_with_password(restored.clone(), Some("user")).unwrap();
        let p = user.permissions().unwrap();
        assert!(!p.owner && !p.copy() && !p.modify(), "{algorithm:?}: restrictions survive the undo and save");
        assert!(CosDocument::open_with_password(restored.clone(), Some("owner")).unwrap().permissions().unwrap().owner, "{algorithm:?}");
        s.mark_saved(id, restored.clone(), None).unwrap();
        assert!(!s.get(id).unwrap().dirty, "{algorithm:?}");
        assert_eq!(*s.save_bytes(id).unwrap(), *restored, "{algorithm:?}: an unchanged save is byte-stable");
    }
}

/// Undo across a save that kept the same security still appends to the last saved file.
#[test]
fn undo_ordinary_encrypted_edits_keeps_the_last_saved_revision() {
    let mut s = Session::new();
    let id = s.open("encrypted.pdf", None, encrypted_fixture(Algorithm::Aes256, false), Some("user")).unwrap();
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved.clone(), None).unwrap();
    s.undo(id).unwrap();
    let restored = s.save_bytes(id).unwrap();
    assert!(restored.starts_with(saved.as_slice()), "compatible encryption still appends to the latest saved bytes");
    assert_saved_content(&s, id, restored.clone(), Some("user"), true).unwrap();
    s.mark_saved(id, restored, None).unwrap();
    s.redo(id).unwrap();
    let rotated = s.save_bytes(id).unwrap();
    assert_saved_content(&s, id, rotated, Some("user"), true).unwrap();
}

/// Undoing a saved password change restores the original passwords; the new ones no longer open
/// the file.
#[test]
fn undo_saved_password_change_restores_original_passwords() {
    let mut s = Session::new();
    let id = s.open("encrypted.pdf", None, encrypted_fixture(Algorithm::Aes256, true), Some("owner")).unwrap();
    s.apply(id, Edit::Protect(protection(Some("new-user"), Some("new-owner")))).unwrap();
    let changed = s.save_bytes(id).unwrap();
    s.mark_saved(id, changed, None).unwrap();
    s.undo(id).unwrap();
    let restored = s.save_bytes(id).unwrap();
    assert_saved_content(&s, id, restored.clone(), Some("user"), true).unwrap();
    assert_saved_content(&s, id, restored.clone(), Some("owner"), true).unwrap();
    assert!(CosDocument::open_with_password(restored.clone(), Some("new-user")).is_err());
    assert!(CosDocument::open_with_password(restored, Some("new-owner")).is_err());
}

/// Undo past a security save back to a signed state, then save: the signature stays valid.
/// Deleting a signed field unsigns the document, so security can change before the undo.
#[test]
fn undo_past_a_security_save_keeps_a_restored_signature_valid() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(1)), None).unwrap();
    let signed = s.sign(id, &digital_id, SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() }).unwrap();
    s.mark_signed(id, signed.clone(), None).unwrap();
    let field = s.get(id).unwrap().signatures[0].field.clone();
    s.apply(id, Edit::DeleteField { name: field }).unwrap();
    assert!(!s.get(id).unwrap().is_signed());
    s.apply(id, Edit::Protect(protection(Some("user"), Some("owner")))).unwrap();
    let protected = s.save_bytes(id).unwrap();
    s.mark_saved(id, protected, None).unwrap();
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().is_signed(), "undo restores the signed snapshot");
    let restored = s.save_bytes(id).unwrap();
    assert_eq!(*restored, *signed, "the signed file as it was, with no revision after the signature");
    let mut u = Session::new().with_clock(|| 1_800_000_000);
    u.set_trusted_certificates(vec![digital_id.certificate.clone()]);
    let uid = u.open("restored.pdf", None, restored, None).unwrap();
    assert_eq!(u.get(uid).unwrap().signatures[0].status, SignatureStatus::Valid);
}

/// The restored state can have unsaved edits of its own: they are appended to its source, in
/// that source's security, and the undone revision is still left out.
#[test]
fn undo_past_a_security_save_keeps_the_snapshot_edits() {
    for encrypted in [false, true] {
        let mut s = Session::new();
        let (original, password) =
            if encrypted { (encrypted_fixture(Algorithm::Aes256, false), Some("owner")) } else { (Arc::new(fixture(2)), None) };
        let id = s.open("a.pdf", None, original.clone(), password).unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let change = if encrypted { Edit::RemoveProtection } else { Edit::Protect(protection(Some("user"), Some("owner"))) };
        s.apply(id, change).unwrap();
        let changed = s.save_bytes(id).unwrap();
        s.mark_saved(id, changed, None).unwrap();
        s.undo(id).unwrap();
        let restored = s.save_bytes(id).unwrap();
        assert!(restored.starts_with(original.as_slice()) && restored.len() > original.len(), "{encrypted}: appended to the source");
        let mut reopened = Session::new();
        let rid = reopened.open("restored.pdf", None, restored.clone(), password).unwrap();
        let doc = reopened.get(rid).unwrap();
        assert_eq!((doc.info.encrypted, doc.info.pages[0].rotation), (encrypted, 90), "{encrypted}");
        assert_eq!(page_texts(&reopened, rid), ["Page 1", "Page 2"], "{encrypted}");
        assert_eq!(restored.windows(b"(Page 1)".len()).any(|w| w == b"(Page 1)"), !encrypted, "{encrypted}: plaintext only in a plain file");
    }
}

/// Edit, save, change security, save, then undo past both and save: the edit's saved revision
/// stays in the chain, in the original security; only the revision in the other security goes.
#[test]
fn undo_past_a_security_save_keeps_earlier_saves_of_the_same_security() {
    for encrypted in [false, true] {
        let (original, password) =
            if encrypted { (encrypted_fixture(Algorithm::Aes256, false), Some("owner")) } else { (Arc::new(fixture(2)), None) };
        let mut s = Session::new();
        let id = s.open("a.pdf", None, original.clone(), password).unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let edited = s.save_bytes(id).unwrap();
        assert!(edited.starts_with(original.as_slice()), "{encrypted}: the edit is an incremental save");
        s.mark_saved(id, edited.clone(), None).unwrap();
        let change = if encrypted { Edit::RemoveProtection } else { Edit::Protect(protection(Some("user"), Some("owner"))) };
        s.apply(id, change).unwrap();
        let changed = s.save_bytes(id).unwrap();
        s.mark_saved(id, changed, None).unwrap();
        s.undo(id).unwrap();
        s.undo(id).unwrap();
        let restored = s.save_bytes(id).unwrap();
        assert!(restored.starts_with(edited.as_slice()) && restored.len() > edited.len(), "{encrypted}: appended to the edit's save");
        let mut reopened = Session::new();
        let rid = reopened.open("restored.pdf", None, restored.clone(), password).unwrap();
        let doc = reopened.get(rid).unwrap();
        assert_eq!((doc.info.encrypted, doc.info.pages[0].rotation), (encrypted, 0), "{encrypted}: the rotation is undone");
        assert_eq!(page_texts(&reopened, rid), ["Page 1", "Page 2"], "{encrypted}");
        assert_eq!(restored.windows(b"(Page 1)".len()).any(|w| w == b"(Page 1)"), !encrypted, "{encrypted}: plaintext only in a plain file");
        s.mark_saved(id, restored.clone(), None).unwrap();
        assert!(!s.get(id).unwrap().dirty, "{encrypted}");
        assert_eq!(*s.save_bytes(id).unwrap(), *restored, "{encrypted}: an unchanged save is byte-stable");
    }
}

/// Security-aware undo saves still strip typed Password-field values, both when writing onto
/// the snapshot's source and when overlaying it onto an earlier compatible save (#3276, #3926).
#[test]
fn undo_past_a_security_save_keeps_password_field_values_out_of_the_file() {
    let mut form = CosDocument::open(Arc::new(form_fixture())).unwrap();
    form.update_dict(pdfcraft_cos::ObjRef::new(4, 0), |d| d.set(b"Ff".to_vec(), Object::Int(8192))).unwrap();
    let original = Arc::new(write_full(&form, &SaveOptions::default()).unwrap());
    for earlier_save in [false, true] {
        let mut s = Session::new();
        let id = s.open("form.pdf", None, original.clone(), None).unwrap();
        s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("hunter2".into()) }).unwrap();
        let target = if earlier_save {
            s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
            let saved = s.save_bytes(id).unwrap();
            s.mark_saved(id, saved.clone(), None).unwrap();
            saved
        } else {
            original.clone()
        };
        s.apply(id, Edit::Protect(protection(Some("user"), Some("owner")))).unwrap();
        let protected = s.save_bytes(id).unwrap();
        s.mark_saved(id, protected, None).unwrap();
        s.undo(id).unwrap();
        if earlier_save {
            s.undo(id).unwrap();
        }
        let value = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "name").unwrap().value.clone();
        assert_eq!(value(&s), ["hunter2"], "undo restores the typed value in memory");
        let restored = s.save_bytes(id).unwrap();
        s.check_save(id, &restored).unwrap();
        assert!(restored.starts_with(&target) && restored.len() > target.len(), "{earlier_save}: keeps the compatible source");
        assert!(!appears_in_pdf(&restored, "hunter2"), "{earlier_save}: the password value stays out of the file");
        assert!(!appears_in_pdf(&restored, "*******"), "{earlier_save}: the typed length stays out of the file");
        let mut reopened = Session::new();
        let rid = reopened.open("restored.pdf", None, restored.clone(), None).unwrap();
        assert!(reopened.get(rid).unwrap().form.iter().find(|f| f.name == "name").unwrap().value.iter().all(|v| v.is_empty()));
        s.mark_saved(id, restored, None).unwrap();
        assert_eq!(value(&s), ["hunter2"], "recording the save keeps the typed value in memory");
    }
}

/// An optimized save packs objects into object streams under new numbers. Undo past it restores
/// a graph that puts other objects at those numbers: objects that look unchanged but sat in such
/// a stream must still be readable from the next save.
#[test]
fn undo_past_an_optimized_save_keeps_the_objects_it_packed() {
    let mut encrypted = CosDocument::open(Arc::new(fixture(3))).unwrap();
    encrypted
        .set_encryption(&NewEncryption {
            algorithm: Algorithm::Aes256,
            user_password: "user",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .unwrap();
    let encrypted = Arc::new(write_full(&encrypted, &SaveOptions::default()).unwrap());
    for (bytes, password) in [(Arc::new(fixture(3)), None), (encrypted, Some("owner"))] {
        let mut s = Session::new();
        let id = s.open("a.pdf", None, bytes, password).unwrap();
        s.apply(id, Edit::DeletePages { pages: vec![2] }).unwrap();
        let compact = s.save_full_bytes(id).unwrap();
        let packed = CosDocument::open_with_password(compact.clone(), password).unwrap();
        s.mark_saved(id, compact, None).unwrap();
        s.undo(id).unwrap();
        // The premise: an object the snapshot keeps unchanged sits in a stream whose number the
        // snapshot reuses for something else.
        let current = &s.get(id).unwrap().editor.as_ref().unwrap().cos;
        let numbers = current.object_numbers();
        let at_risk: Vec<u32> = numbers
            .iter()
            .copied()
            .filter(|&n| matches!(packed.xref_entry(n), Some(pdfcraft_cos::XrefEntry::InStream { stream, .. }) if numbers.contains(&stream)))
            .filter(|&n| {
                let r = pdfcraft_cos::ObjRef::new(n, current.generation(n));
                *packed.get(r) == *current.get(r)
            })
            .collect();
        // Only the encrypted file sets this up; the plain one checks there is no regression.
        if password.is_some() {
            assert!(!at_risk.is_empty(), "no unchanged packed object in a reused stream");
        }
        let restored = s.save_bytes(id).unwrap();
        s.check_save(id, &restored).unwrap();
        let saved = CosDocument::open_with_password(restored.clone(), password).unwrap();
        assert!(saved.repair_log().is_empty(), "{password:?}: {:?}", saved.repair_log());
        let current = &s.get(id).unwrap().editor.as_ref().unwrap().cos;
        for num in current.object_numbers() {
            let r = pdfcraft_cos::ObjRef::new(num, current.generation(num));
            // The source's obsolete cross-reference stream is not document content. Compaction
            // left its number unused, so Save's drop_unlinked option omits the unreferenced
            // addition instead of copying that old index into the new revision (#4226).
            let source_xref = matches!(current.xref_entry(num), Some(pdfcraft_cos::XrefEntry::InFile { offset, .. })
                if current.revisions().iter().any(|revision| revision.is_stream && revision.xref_offset == offset));
            if source_xref && matches!(packed.xref_entry(num), None | Some(pdfcraft_cos::XrefEntry::Free { .. })) {
                assert_eq!(*saved.get(r), Object::Null, "{password:?}: an obsolete unlinked cross-reference stream");
                continue;
            }
            assert_eq!(*saved.get(r), *current.get(r), "{password:?}: object {num}");
        }
        let mut reopened = Session::new();
        let rid = reopened.open("restored.pdf", None, restored, password).unwrap();
        assert_eq!(page_texts(&reopened, rid), ["Page 1", "Page 2", "Page 3"], "{password:?}");
    }
}
