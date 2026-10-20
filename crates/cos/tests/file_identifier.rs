//! The file identifier (`/ID`, ISO 32000-2 §14.4) of a full save. An ordinary save keeps the
//! identifier the file has and writes the same bytes twice. A rewrite the document asked for (after
//! redaction or sanitizing) gives the file a new pair. An encrypted file keeps its first string,
//! because its file key is derived from it.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_testkit::Pdf;

const ID_HEX: &str = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";
const ID_BYTES: [u8; 16] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];

/// One page whose content holds a secret, with `/ID` in the trailer when `with_id` is set.
fn source(with_id: bool) -> Vec<u8> {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
        .stream("", "BT /F1 12 Tf 20 100 Td (TOPSECRET) Tj ET")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    if with_id { pdf.trailer_entry(ID_HEX).build() } else { pdf.build() }
}

fn reopen(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).unwrap()
}

/// The strings of the trailer's `/ID` (empty when there is none).
fn id_strings(doc: &Document) -> Vec<Vec<u8>> {
    match doc.trailer().get(b"ID") {
        Some(Object::Array(a)) => a.iter().map(|s| s.as_string().map(|s| s.bytes.clone()).unwrap_or_default()).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn an_ordinary_save_keeps_the_identifier_and_writes_the_same_bytes() {
    let doc = reopen(source(true));
    let opts = SaveOptions::default();
    let saved = write_full(&doc, &opts).unwrap();
    assert_eq!(saved, write_full(&doc, &opts).unwrap(), "unchanged inputs produce identical bytes");
    assert_eq!(id_strings(&reopen(saved)), vec![ID_BYTES.to_vec(), ID_BYTES.to_vec()]);
    let appended = reopen(write_incremental(&doc, &opts).unwrap());
    assert_eq!(id_strings(&appended), vec![ID_BYTES.to_vec(), ID_BYTES.to_vec()]);
}

#[test]
fn an_ordinary_save_of_a_file_without_an_identifier_creates_the_same_one_every_time() {
    let doc = reopen(source(false));
    let opts = SaveOptions::default();
    let saved = write_full(&doc, &opts).unwrap();
    assert_eq!(saved, write_full(&doc, &opts).unwrap(), "unchanged inputs produce identical bytes");
    let ids = id_strings(&reopen(saved));
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|s| s.len() == 16), "{ids:?}");
}

#[test]
fn a_rewrite_the_document_asked_for_gets_a_new_identifier() {
    let mut doc = reopen(source(true));
    doc.require_full_save_with_new_id();
    assert!(doc.full_save_required() && doc.new_id_required());
    let opts = SaveOptions::default();
    for saved in [write_incremental(&doc, &opts).unwrap(), write_full(&doc, &opts).unwrap()] {
        let ids = id_strings(&reopen(saved));
        assert_eq!(ids.len(), 2);
        for id in &ids {
            assert_eq!(id.len(), 16);
            assert_ne!(id.as_slice(), ID_BYTES.as_slice(), "the copy must not claim the source's identifier");
        }
    }
    // Every save is a new identity, not the same one again.
    let first = id_strings(&reopen(write_full(&doc, &opts).unwrap()));
    let second = id_strings(&reopen(write_full(&doc, &opts).unwrap()));
    assert_ne!(first, second);
}

#[test]
fn a_rewrite_without_an_identifier_still_gets_one() {
    let mut doc = reopen(source(false));
    doc.require_full_save_with_new_id();
    let ids = id_strings(&reopen(write_full(&doc, &SaveOptions::default()).unwrap()));
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|s| s.len() == 16), "{ids:?}");
}

#[test]
fn caller_entropy_makes_the_new_identifier_reproducible() {
    let mut doc = reopen(source(true));
    doc.require_full_save_with_new_id();
    let with = |entropy: u8| SaveOptions { id_entropy: Some([entropy; 16]), ..SaveOptions::default() };
    let first = write_full(&doc, &with(1)).unwrap();
    assert_eq!(first, write_full(&doc, &with(1)).unwrap(), "the same entropy gives the same bytes");
    let other = write_full(&doc, &with(2)).unwrap();
    assert_ne!(id_strings(&reopen(first)), id_strings(&reopen(other)), "different entropy gives a different identifier");
}

#[test]
fn an_encrypted_rewrite_keeps_the_first_identifier_string_its_key_depends_on() {
    for algorithm in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = reopen(source(true));
        doc.set_encryption(&NewEncryption {
            algorithm,
            user_password: "user",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [5; 32],
        })
        .unwrap();
        let protected = write_full(&doc, &SaveOptions::default()).unwrap();
        let before = Document::open_with_password(Arc::new(protected), Some("user")).unwrap();

        let mut doc = before.clone();
        doc.require_full_save_with_new_id();
        let out = write_incremental(&doc, &SaveOptions::default()).unwrap();
        // The output opens with the same password only if its first identifier string is unchanged.
        let back = Document::open_with_password(Arc::new(out), Some("user")).unwrap();
        let (old, new) = (id_strings(&before), id_strings(&back));
        assert_eq!(new.len(), 2, "{algorithm:?}");
        assert_eq!(new.first(), old.first(), "{algorithm:?}: the key depends on the first string");
        assert_ne!(new.get(1), old.get(1), "{algorithm:?}: the second string is new");
    }
}
