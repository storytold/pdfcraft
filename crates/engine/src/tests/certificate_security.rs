//! The engine and renderer retain encrypted originals and never need the card again for edits.
use super::*;
use pdfcraft_cos::{CosError, EncryptDict, SaveOptions, SecurityHandler};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::cell::Cell;

fn encrypted(algorithm: pdfcraft_cos::Algorithm, metadata: bool) -> Arc<Vec<u8>> {
    let mut source = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    let mut info = pdfcraft_cos::Dict::new();
    info.set(b"Title".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString { bytes: b"Secret title".to_vec(), hex: false }));
    let info = source.add(info);
    source.trailer_mut().set(b"Info".to_vec(), pdfcraft_cos::Object::Ref(info));
    let mut stream = pdfcraft_cos::Dict::new();
    stream.set(b"Type".to_vec(), pdfcraft_cos::Object::name("EmbeddedFile"));
    let stream = source.add(pdfcraft_cos::Stream { dict: stream, raw: b"private attachment".to_vec().into() });
    let mut ef = pdfcraft_cos::Dict::new();
    ef.set(b"F".to_vec(), pdfcraft_cos::Object::Ref(stream));
    let mut spec = pdfcraft_cos::Dict::new();
    spec.set(b"Type".to_vec(), pdfcraft_cos::Object::name("Filespec"));
    spec.set(b"F".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString { bytes: b"notes.txt".to_vec(), hex: false }));
    spec.set(b"EF".to_vec(), pdfcraft_cos::Object::Dict(ef));
    let spec = source.add(spec);
    let mut tree = pdfcraft_cos::Dict::new();
    tree.set(
        b"Names".to_vec(),
        pdfcraft_cos::Object::Array(vec![
            pdfcraft_cos::Object::String(pdfcraft_cos::PdfString { bytes: b"notes.txt".to_vec(), hex: false }),
            pdfcraft_cos::Object::Ref(spec),
        ]),
    );
    let mut names = pdfcraft_cos::Dict::new();
    names.set(b"EmbeddedFiles".to_vec(), pdfcraft_cos::Object::Dict(tree));
    source.update_dict(source.root().unwrap(), |d| d.set(b"Names".to_vec(), pdfcraft_cos::Object::Dict(names))).unwrap();
    source
        .set_public_key_encryption(
            SecurityHandler::create_public_key(algorithm, vec![b"test envelope".to_vec()], &[7; 20], metadata).unwrap(),
            [9; 16],
        )
        .unwrap();
    Arc::new(pdfcraft_cos::write_full(&source, &SaveOptions::default()).unwrap())
}
fn authenticate(params: &EncryptDict, permissions: i32) -> Result<SecurityHandler, CosError> {
    SecurityHandler::open_public_key(params.clone(), &[7; 20], permissions).map_err(|e| CosError::Security(e.to_string()))
}

#[test]
fn certificate_security_opens_renders_and_preserves_the_original() {
    for algorithm in [pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256] {
        for metadata in [true, false] {
            let bytes = encrypted(algorithm, metadata);
            let calls = Cell::new(0);
            let resolver = |params: &EncryptDict, _: &[u8]| {
                calls.set(calls.get() + 1);
                authenticate(params, -1)
            };
            let mut session = Session::new();
            assert!(matches!(session.open("protected.pdf", None, bytes.clone(), None), Err(OpenError::NeedsCertificate)));
            let id = session.open_with_security_resolver("protected.pdf", None, bytes.clone(), &resolver).unwrap();
            let doc = session.get(id).unwrap();
            assert!(Arc::ptr_eq(&doc.bytes, &bytes));
            assert!(doc.editable());
            assert!(doc.info.encrypted);
            assert_eq!(doc.info.title.as_deref(), Some("Secret title"));
            assert_eq!(doc.info.pages.len(), 1);
            assert_eq!(doc.attachment_data(&doc.info.attachments[0]).unwrap(), b"private attachment");
            let mut renderer = PageRenderer::new(doc.display.clone(), doc.config.clone());
            let got = renderer.render(RenderRequest::default());
            assert!(got.error.is_none(), "{:?}", got.error);
            let mut clear = PageRenderer::new(Arc::new(fixture(1)), RenderConfig::default());
            let expected = clear.render(RenderRequest::default());
            assert_eq!(&*got.rgba, &*expected.rgba, "encrypted pixels differ");
            assert!(expected.rgba.iter().any(|v| *v != 255), "the reference page must contain ink");
            assert!(!doc.words().is_empty(), "authenticated text extraction must read the page");
            assert_eq!(calls.get(), 1);
            session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Private edit".into() }).unwrap();
            session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
            let saved = session.save_bytes(id).unwrap();
            assert!(saved.starts_with(&bytes));
            assert!(!String::from_utf8_lossy(&saved).contains("Private edit"));
            session.check_save(id, &saved).unwrap();
            session.mark_saved(id, saved.clone(), None).unwrap();
            session.undo(id).unwrap();
            session.redo(id).unwrap();
            session.revert(id).unwrap();
            session.reload(id, saved).unwrap();
            assert_eq!(calls.get(), 1, "editing and saving must not ask the original resolver again");
        }
    }
}

#[test]
fn certificate_security_does_not_confuse_all_content_permissions_with_security_control() {
    let bytes = encrypted(pdfcraft_cos::Algorithm::Aes256, true);
    let resolver = |params: &EncryptDict, _: &[u8]| authenticate(params, -3); // Every bit except security-change bit 2.
    let mut session = Session::new();
    let id = session.open_with_security_resolver("restricted.pdf", None, bytes, &resolver).unwrap();
    let doc = session.get(id).unwrap();
    assert!(doc.allows_annotation());
    assert!(!doc.allows_security_change());
    assert!(!doc.can_unlock());
    assert!(session.unlock(id, "password").is_err());
    assert!(session.apply(id, Edit::RemoveProtection).is_err());
    assert!(session.apply(id, Edit::Batch { label: "Remove security".into(), edits: vec![Edit::RemoveProtection] }).is_err());
    assert!(session.apply(id, Edit::ConvertPdfA { level: pdfcraft_preflight::Level::A2b }).is_err());
}

#[test]
fn certificate_security_rejects_stale_context_and_keeps_unsaved_work() {
    let bytes = encrypted(pdfcraft_cos::Algorithm::Aes256, true);
    let resolver = |params: &EncryptDict, _: &[u8]| authenticate(params, -1);
    let mut session = Session::new();
    let id = session.open_with_security_resolver("protected.pdf", None, bytes, &resolver).unwrap();
    session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Unsaved".into() }).unwrap();
    let before = session.get(id).unwrap().bytes.clone();
    assert!(session.reload(id, Arc::new(fixture(1))).is_err(), "a reload must not silently remove certificate security");
    let changed = encrypted(pdfcraft_cos::Algorithm::Aes256, false);
    assert!(session.reload(id, changed.clone()).is_err());
    assert!(Arc::ptr_eq(&session.get(id).unwrap().bytes, &before));
    assert!(session.get(id).unwrap().dirty);
    let mut renderer = PageRenderer::new(changed, session.get(id).unwrap().config.clone());
    assert!(renderer.render(RenderRequest::default()).error.is_some());
    let snapshots = session.autosave_snapshots();
    assert_eq!(snapshots.len(), 1);
    assert!(snapshots[0].encrypted);
    assert!(!String::from_utf8_lossy(&snapshots[0].bytes).contains("Unsaved"));
    assert!(matches!(session.open("recovery.pdf", None, snapshots[0].bytes.clone(), None), Err(OpenError::NeedsCertificate)));
}

#[test]
fn certificate_security_owner_can_remove_protection_and_undo() {
    let bytes = encrypted(pdfcraft_cos::Algorithm::Aes256, true);
    let resolver = |params: &EncryptDict, _: &[u8]| authenticate(params, -1);
    let mut session = Session::new();
    let id = session.open_with_security_resolver("protected.pdf", None, bytes, &resolver).unwrap();
    session.apply(id, Edit::RemoveProtection).unwrap();
    assert!(!session.get(id).unwrap().info.encrypted);
    let clear = session.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(clear).unwrap().security().is_none());
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().info.encrypted);
    session.check_save(id, &session.save_bytes(id).unwrap()).unwrap();
    session.redo(id).unwrap();
    assert!(!session.get(id).unwrap().info.encrypted);
    session.undo(id).unwrap();
    session.apply(id, Edit::ConvertPdfA { level: pdfcraft_preflight::Level::A2b }).unwrap();
    assert!(!session.get(id).unwrap().info.encrypted);
    session.check_save(id, &session.save_bytes(id).unwrap()).unwrap();
}

#[test]
fn certificate_security_resolver_failure_does_not_replace_the_open_document() {
    let mut session = Session::new();
    let id = session.open("existing.pdf", None, Arc::new(fixture(1)), None).unwrap();
    session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Keep this".into() }).unwrap();
    let before = session.get(id).unwrap().bytes.clone();
    let resolver = |_: &EncryptDict, _: &[u8]| -> Result<SecurityHandler, CosError> { panic!("provider failed") };
    assert!(session.open_with_security_resolver("protected.pdf", None, encrypted(pdfcraft_cos::Algorithm::Aes256, true), &resolver).is_err());
    assert!(Arc::ptr_eq(&session.get(id).unwrap().bytes, &before));
    assert!(session.get(id).unwrap().dirty);
}

#[cfg(not(target_arch = "wasm32"))]
mod workflows {
    use super::*;
    use crate::certificate_security::{self, CertificateProtection, CertificateRecipient};
    use crate::sign::{Certificate, DigestAlg, DigitalId, Name, PrivateKey, Time, der};
    use aws_lc_rs::rsa::{Pkcs1PrivateDecryptingKey, PrivateDecryptingKey};
    use zeroize::Zeroizing;

    fn identity(serial: u8) -> DigitalId {
        let key = PrivateKey::generate_rsa(2048).unwrap();
        let cert =
            Certificate::self_signed(&Name::build("Encryption test", "", "", "", ""), &key, Time::from_unix(1_700_000_000), 20, &[serial]).unwrap();
        let fields = der::Tlv::parse_all(&cert.raw).unwrap().children().unwrap();
        let tbs = fields[0].children().unwrap();
        let mut values: Vec<Vec<u8>> = tbs.iter().filter(|v| v.tag != der::tag::ctx(3)).map(|v| v.raw.to_vec()).collect();
        let usage = der::seq(&[&der::oid("2.5.29.15"), &der::octets(&der::tlv(der::tag::BIT_STRING, &[5, 0x20]))]);
        values.push(der::explicit(3, &der::seq(&[&usage])));
        let tbs = der::seq(&values.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let signature = key.sign(DigestAlg::Sha256, &tbs).unwrap();
        let certificate = Certificate::parse(&der::seq(&[&tbs, fields[1].raw, &der::bit_string(&signature)])).unwrap();
        DigitalId { key, certificate, chain: Vec::new(), friendly_name: None }
    }
    fn decrypt(id: &DigitalId, encrypted: &[u8]) -> Result<Zeroizing<Vec<u8>>, crate::sign::SignError> {
        let key = Pkcs1PrivateDecryptingKey::new(PrivateDecryptingKey::from_pkcs8(id.key.pkcs8()).unwrap()).unwrap();
        let mut out = Zeroizing::new(vec![0; 512]);
        let n = key.decrypt(encrypted, &mut out).unwrap().len();
        out.truncate(n);
        Ok(out)
    }
    fn protection(ids: &[(&DigitalId, i32)]) -> CertificateProtection {
        CertificateProtection {
            recipients: ids
                .iter()
                .map(|(id, permissions)| CertificateRecipient { certificate: id.certificate.raw.clone(), permissions: *permissions })
                .collect(),
            algorithm: Algorithm::Aes256,
            encrypt_metadata: true,
        }
    }

    #[test]
    fn certificate_workflow_encrypts_for_two_recipients_and_retains_their_distinct_rights() {
        let owner = identity(1);
        let restricted = identity(2);
        for algorithm in [Algorithm::Aes128, Algorithm::Aes256] {
            for metadata in [true, false] {
                let (mut session, id) = session_with(1);
                let mut config = protection(&[(&owner, -1), (&restricted, 1 << 5)]);
                config.algorithm = algorithm;
                config.encrypt_metadata = metadata;
                session.apply(id, Edit::ProtectCertificates(config)).unwrap();
                let saved = session.save_bytes(id).unwrap();
                assert!(!String::from_utf8_lossy(&saved).contains("Page 1"));
                session.check_save(id, &saved).unwrap();
                session.mark_saved(id, saved.clone(), None).unwrap();
                session.undo(id).unwrap();
                assert!(!session.get(id).unwrap().info.encrypted);
                session.redo(id).unwrap();
                assert!(session.get(id).unwrap().info.encrypted);
                for (recipient, security_control) in [(&owner, true), (&restricted, false)] {
                    let calls = Cell::new(0);
                    let resolver = |params: &EncryptDict, _: &[u8]| {
                        certificate_security::authenticate(params, &recipient.certificate, |key| {
                            calls.set(calls.get() + 1);
                            decrypt(recipient, key)
                        })
                    };
                    let mut opened = Session::new();
                    let doc = opened.open_with_security_resolver("encrypted.pdf", None, saved.clone(), &resolver).unwrap();
                    assert_eq!(calls.get(), 1);
                    assert_eq!(opened.get(doc).unwrap().allows_security_change(), security_control);
                    let mut renderer = PageRenderer::new(saved.clone(), opened.get(doc).unwrap().config.clone());
                    let shot = renderer.render(RenderRequest::default());
                    assert!(shot.error.is_none(), "{:?}", shot.error);
                    assert!(shot.rgba.iter().any(|p| *p != 255));
                    opened
                        .apply(
                            doc,
                            Edit::AddAnnotation(NewAnnotation {
                                page: 0,
                                shape: Shape::Note { at: [10.0, 10.0], icon: NoteIcon::Comment },
                                contents: "Allowed annotation".into(),
                                style: Style::default(),
                                author: String::new(),
                            }),
                        )
                        .unwrap();
                    opened.check_save(doc, &opened.save_bytes(doc).unwrap()).unwrap();
                    assert_eq!(calls.get(), 1);
                    if !security_control {
                        assert!(opened.apply(doc, Edit::RemoveProtection).is_err());
                    }
                }
            }
        }
    }

    #[test]
    fn certificate_workflow_replaces_recipients_and_can_undo_a_password_change() {
        let original = identity(1);
        let replacement = identity(2);
        let (mut session, id) = session_with(1);
        session.apply(id, Edit::ProtectCertificates(protection(&[(&original, -1)]))).unwrap();
        let first = session.save_bytes(id).unwrap();
        session.mark_saved(id, first.clone(), None).unwrap();
        session.apply(id, Edit::ProtectCertificates(protection(&[(&replacement, -1)]))).unwrap();
        let second = session.save_bytes(id).unwrap();
        let calls = Cell::new(0);
        let old_resolver = |params: &EncryptDict, _: &[u8]| {
            certificate_security::authenticate(params, &original.certificate, |key| {
                calls.set(calls.get() + 1);
                decrypt(&original, key)
            })
        };
        assert!(pdfcraft_cos::Document::open_with_security_resolver(second.clone(), &old_resolver).is_err());
        assert_eq!(calls.get(), 0, "a removed recipient must not use a private key");
        let resolver =
            |params: &EncryptDict, _: &[u8]| certificate_security::authenticate(params, &replacement.certificate, |key| decrypt(&replacement, key));
        assert!(pdfcraft_cos::Document::open_with_security_resolver(second, &resolver).is_ok());
        session.undo(id).unwrap();
        session.check_save(id, &session.save_bytes(id).unwrap()).unwrap();
        assert!(pdfcraft_cos::Document::open_with_security_resolver(session.save_bytes(id).unwrap(), &old_resolver).is_ok());
        session.redo(id).unwrap();
        session.apply(id, Edit::Protect(Protection { open_password: Some("password".into()), ..Default::default() })).unwrap();
        assert!(pdfcraft_cos::Document::open_with_password(session.save_bytes(id).unwrap(), Some("password")).is_ok());
        session.undo(id).unwrap();
        session.check_save(id, &session.save_bytes(id).unwrap()).unwrap();
        assert!(!session.get(id).unwrap().words().is_empty());
    }

    #[test]
    fn certificate_workflow_rejects_invalid_edits_without_losing_current_work() {
        let owner = identity(1);
        let conflicting = identity(1);
        let (mut session, id) = session_with(1);
        session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Keep me".into() }).unwrap();
        let before = session.get(id).unwrap().bytes.clone();
        for invalid in
            [protection(&[]), protection(&[(&owner, 0)]), protection(&[(&owner, -1), (&owner, -1)]), protection(&[(&owner, -1), (&conflicting, -1)])]
        {
            assert!(session.apply(id, Edit::ProtectCertificates(invalid)).is_err());
            assert!(Arc::ptr_eq(&session.get(id).unwrap().bytes, &before));
            assert!(session.get(id).unwrap().dirty);
        }
        let mut cx = EditCtx::new(Some(1_800_000_000), 0);
        cx.rng = |_| Err(getrandom::Error::UNSUPPORTED);
        let mut source = pdfcraft_cos::Document::open(before).unwrap();
        assert!(run_edit(&mut source, &Edit::ProtectCertificates(protection(&[(&owner, -1)])), &mut cx).is_err());
        assert!(source.output_handler().is_none());
    }

    #[test]
    fn certificate_workflow_refuses_ambiguous_or_missing_envelopes_before_private_key_use() {
        let owner = identity(1);
        let other = identity(2);
        let (mut session, id) = session_with(1);
        session.apply(id, Edit::ProtectCertificates(protection(&[(&owner, -1)]))).unwrap();
        let mut params = session.get(id).unwrap().editor.as_ref().unwrap().cos.output_handler().unwrap().dict().clone();
        let used = Cell::new(0);
        let decrypt = |_: &[u8]| {
            used.set(used.get() + 1);
            Err(crate::sign::SignError::Crypto("must not reach a private key".into()))
        };
        assert!(certificate_security::authenticate(&params, &other.certificate, decrypt).is_err());
        let mut invalid = params.clone();
        invalid.v = 99;
        assert!(certificate_security::authenticate(&invalid, &owner.certificate, decrypt).is_err());
        let envelope = params.public_key.as_ref().unwrap().recipients[0].clone();
        params.public_key.as_mut().unwrap().recipients.push(envelope);
        assert!(certificate_security::authenticate(&params, &owner.certificate, decrypt).is_err());
        assert_eq!(used.get(), 0);
    }
}
