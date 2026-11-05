//! A document that encrypts only its embedded files, authorised when one is opened (`/AuthEvent
//! /EFOpen`, ISO 32000-2 §7.6.6, Table 25): it opens without a password, its pages read as stored,
//! and its embedded files keep their encrypted bytes, flagged as such, through every save.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, CosError, Document, NewEncryption, ObjRef, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_crypt::StreamKind;

const PAGE: &str = "BT /F1 12 Tf 20 100 Td (Visible page) Tj ET";
const ATTACHED: &[u8] = b"the attached file's own words";
const PASSWORD: &str = "attach pw";
const EMBEDDED: ObjRef = ObjRef { num: 6, generation: 0 };
const CONTENT: ObjRef = ObjRef { num: 4, generation: 0 };

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// One page in the clear and one attachment encrypted with `/StdCF`, whose `/AuthEvent` is
/// `auth_event`: AES-256 (V5, R6) or AES-128 (V4, R4), `/StmF` and `/StrF` `/Identity`.
fn fixture(algorithm: Algorithm, auth_event: &str) -> Vec<u8> {
    let id0 = [7u8; 16];
    let params =
        NewEncryption { algorithm, user_password: PASSWORD, owner_password: "owner pw", permissions: -4, encrypt_metadata: true, seed: [3; 32] };
    let handler = pdfcraft_crypt::create(&params, &id0).unwrap();
    let d = handler.dict();
    let cipher = handler.encrypt_stream(EMBEDDED.num, 0, ATTACHED, StreamKind::EmbeddedFile);
    let (version, cfm, r6) = match algorithm {
        Algorithm::Aes256 => ("/V 5 /R 6", "/AESV3 /Length 32", format!("/OE <{}> /UE <{}> /Perms <{}>", hex(&d.oe), hex(&d.ue), hex(&d.perms))),
        _ => ("/V 4 /R 4 /Length 128", "/AESV2 /Length 16", String::new()),
    };
    let encrypt = format!(
        "<< /Filter /Standard {version} /O <{}> /U <{}> {r6} /P {} /CF << /StdCF << /AuthEvent /{auth_event} /CFM {cfm} >> >> /StmF /Identity /StrF /Identity /EFF /StdCF >>",
        hex(&d.o),
        hex(&d.u),
        d.p
    );
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(note.txt) 5 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{PAGE}\nendstream", PAGE.len()).into_bytes(),
        b"<< /Type /Filespec /F (note.txt) /UF (note.txt) /EF << /F 6 0 R >> >>".to_vec(),
    ];
    let mut stream = format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", cipher.len()).into_bytes();
    stream.extend_from_slice(&cipher);
    stream.extend_from_slice(b"\nendstream");
    objs.push(stream);
    objs.push(encrypt.into_bytes());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    let trailer = format!(
        "trailer\n<< /Size {} /Root 1 0 R /Encrypt 7 0 R /ID [<{}> <{}>] >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1,
        hex(&id0),
        hex(&id0)
    );
    out.extend_from_slice(trailer.as_bytes());
    out
}

fn stream(doc: &Document, r: ObjRef) -> pdfcraft_cos::Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        other => panic!("object {} isn't a stream: {other:?}", r.num),
    }
}

/// The attachment as the password reads it from `bytes`.
fn attachment_with_password(bytes: Vec<u8>) -> Vec<u8> {
    let doc = Document::open_with_password(Arc::new(bytes), Some(PASSWORD)).unwrap();
    assert!(!doc.embedded_files_locked());
    let embedded = doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap();
    assert!(embedded.get(b"Names").is_some());
    stream(&doc, EMBEDDED).decoded().unwrap()
}

#[test]
fn a_document_encrypting_only_its_embedded_files_opens_without_a_password() {
    let bytes = fixture(Algorithm::Aes256, "EFOpen");
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    assert!(doc.embedded_files_locked());
    // Not a repair: the specification has it open so.
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert!(doc.trailer().get(b"Encrypt").is_some());
    // The page reads as stored; the attachment keeps its encrypted bytes and says so.
    let content = stream(&doc, CONTENT);
    assert_eq!(content.decoded().unwrap(), PAGE.as_bytes());
    assert!(!doc.reads_as_stored(&content));
    let embedded = stream(&doc, EMBEDDED);
    assert!(doc.reads_as_stored(&embedded));
    assert_ne!(embedded.decoded().unwrap(), ATTACHED);
    // The password still opens it, and reads the attachment.
    assert_eq!(attachment_with_password(bytes.clone()), ATTACHED);
    // A wrong password is still refused.
    assert!(matches!(Document::open_with_password(Arc::new(bytes), Some("nope")), Err(CosError::WrongPassword)));
}

#[test]
fn a_crypt_filter_authorised_when_the_document_opens_still_needs_the_password() {
    let bytes = fixture(Algorithm::Aes256, "DocOpen");
    assert!(matches!(Document::open(Arc::new(bytes.clone())), Err(CosError::NeedsPassword)));
    assert_eq!(attachment_with_password(bytes), ATTACHED);
}

#[test]
fn saves_keep_the_encrypted_embedded_files_as_stored() {
    for algorithm in [Algorithm::Aes256, Algorithm::Aes128] {
        let mut doc = Document::open(Arc::new(fixture(algorithm, "EFOpen"))).unwrap();
        assert!(doc.embedded_files_locked(), "{algorithm:?}");
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"PageMode".to_vec(), Object::name("UseAttachments"))).unwrap();
        // An incremental save keeps /Encrypt, and the attachment decrypts with the password.
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert_eq!(attachment_with_password(saved), ATTACHED, "{algorithm:?}");
        // A full save renumbers: AES-256 keys don't depend on the number, AES-128 keys do.
        match algorithm {
            Algorithm::Aes256 => {
                let full = write_full(&doc, &SaveOptions::default()).unwrap();
                let reopened = Document::open(Arc::new(full.clone())).unwrap();
                assert!(reopened.embedded_files_locked());
                let doc = Document::open_with_password(Arc::new(full), Some(PASSWORD)).unwrap();
                let names = doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap();
                let tree = doc.resolve(names.get(b"Names").unwrap()).as_dict().cloned().unwrap();
                let files = doc.resolve(tree.get(b"EmbeddedFiles").unwrap()).as_dict().cloned().unwrap();
                let list = doc.resolve(files.get(b"Names").unwrap()).as_array().cloned().unwrap();
                let spec = doc.resolve(&list[1]).as_dict().cloned().unwrap();
                let ef = doc.resolve(spec.get(b"EF").unwrap()).as_dict().cloned().unwrap();
                let file = match &*doc.resolve(ef.get(b"F").unwrap()) {
                    Object::Stream(s) => s.decoded().unwrap(),
                    other => panic!("{other:?}"),
                };
                assert_eq!(file, ATTACHED);
            }
            _ => assert!(matches!(write_full(&doc, &SaveOptions::default()), Err(CosError::Security(_))), "{algorithm:?}"),
        }
    }
}

#[test]
fn security_cannot_change_without_the_embedded_files_password() {
    let mut doc = Document::open(Arc::new(fixture(Algorithm::Aes256, "EFOpen"))).unwrap();
    doc.remove_encryption();
    assert!(matches!(write_full(&doc, &SaveOptions::default()), Err(CosError::Security(_))));
    assert!(matches!(write_incremental(&doc, &SaveOptions::default()), Err(CosError::Security(_))));
}
