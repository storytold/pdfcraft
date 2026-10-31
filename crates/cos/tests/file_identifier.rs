//! The file identifier (`/ID`, ISO 32000-2 §14.4) of a full save. An ordinary save keeps the
//! identifier the file has and writes the same bytes twice. A rewrite the document asked for (after
//! redaction or sanitizing) gives the file a new pair. An encrypted file keeps its first string,
//! because its file key is derived from it.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, Object, PdfString, SaveOptions, write_full, write_incremental};
use pdfcraft_testkit::Pdf;

const ID_HEX: &str = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";
const ID_BYTES: [u8; 16] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];

/// One page whose content holds a secret, with `entry` added to the trailer (for example an `/ID`).
fn source_with(entry: Option<&str>) -> Vec<u8> {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
        .stream("", "BT /F1 12 Tf 20 100 Td (TOPSECRET) Tj ET")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    match entry {
        Some(entry) => pdf.trailer_entry(entry).build(),
        None => pdf.build(),
    }
}

/// [`source_with`] with the standard identifier, or none.
fn source(with_id: bool) -> Vec<u8> {
    source_with(with_id.then_some(ID_HEX))
}

/// A one-page file whose 70,000-byte content stream ends in `last`. Two of these built with
/// different last bytes have the same length and differ only past the first 64 KiB.
fn large_source(last: u8) -> Vec<u8> {
    let mut content = vec![b' '; 70_000];
    content.push(last);
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .build()
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

/// The `/ID` shapes whose key the reader takes as the empty string: none at all, one entry, and a
/// first entry that is not a string.
fn unusable_ids() -> Vec<(&'static str, Option<Object>)> {
    let empty = Object::String(PdfString::literal(Vec::new()));
    vec![
        ("absent", None),
        ("one entry", Some(Object::Array(vec![empty.clone()]))),
        ("first entry not a string", Some(Object::Array(vec![Object::Int(7), empty]))),
    ]
}

/// An encrypted file whose key is derived from the empty string, with its trailer `/ID` replaced by `id`.
fn protected_with_id(algorithm: Algorithm, id: Option<Object>) -> Vec<u8> {
    let mut doc = reopen(source_with(Some("/ID [<> <>]")));
    doc.set_encryption(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .unwrap();
    match id {
        Some(id) => doc.trailer_mut().set(b"ID".to_vec(), id),
        None => {
            doc.trailer_mut().remove(b"ID");
        }
    }
    write_full(&doc, &SaveOptions::default()).unwrap()
}

#[test]
fn an_encrypted_rewrite_without_a_usable_identifier_still_gets_a_new_one() {
    // The key is derived from the first string of /ID, and a missing /ID, a one-entry /ID and a
    // first entry that is not a string all count as the empty string. The rewrite keeps that
    // (empty) first string, so the file still opens with its password, and writes a new second one.
    for algorithm in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        for (shape, id) in unusable_ids() {
            let before = Document::open_with_password(Arc::new(protected_with_id(algorithm, id)), Some("user")).unwrap();
            let mut doc = before.clone();
            doc.require_full_save_with_new_id();
            let out = write_incremental(&doc, &SaveOptions::default()).unwrap();
            let back = Document::open_with_password(Arc::new(out), Some("user")).unwrap();
            let ids = id_strings(&back);
            let previous = id_strings(&before);
            assert_eq!(ids.len(), 2, "{algorithm:?}, {shape}: the rewrite writes a pair");
            assert!(ids.first().is_some_and(|s| s.is_empty()), "{algorithm:?}, {shape}: the first string is the one the key uses");
            assert_eq!(ids.get(1).map(Vec::len), Some(16), "{algorithm:?}, {shape}");
            assert_ne!(ids.get(1), previous.get(1), "{algorithm:?}, {shape}: the second string is new");
        }
    }
}

/// The identifier of [`fixed_entropy_rewrite`]. It was computed once with the FNV-1a formula in the
/// writer; it must not change with the platform or the toolchain.
const GOLDEN_FIXED_ENTROPY_ID: [u8; 16] = [0x32, 0x7d, 0x91, 0x46, 0xb8, 0x6d, 0xb3, 0xe1, 0x97, 0xc3, 0x5e, 0xdc, 0x58, 0x48, 0x3f, 0xba];

/// The standard fixture rewritten for a new identifier, with `id_entropy` of 0x42.
fn fixed_entropy_rewrite() -> Vec<u8> {
    let mut doc = reopen(source(true));
    doc.require_full_save_with_new_id();
    write_full(&doc, &SaveOptions { id_entropy: Some([0x42; 16]), ..SaveOptions::default() }).unwrap()
}

#[test]
fn a_fixed_entropy_gives_the_same_identifier_on_every_platform() {
    // The identifier is FNV-1a over fixed-width big-endian fields and the written file, so the same
    // entropy and bytes give the same 16 bytes on every platform and toolchain. The value also pins
    // this writer's output for the fixture.
    let golden = GOLDEN_FIXED_ENTROPY_ID.to_vec();
    assert_eq!(id_strings(&reopen(fixed_entropy_rewrite())), vec![golden.clone(), golden]);
}

#[test]
fn the_identifier_depends_on_the_whole_rewritten_file() {
    // Same caller entropy and same length; the two rewrites differ only in one byte of a stream
    // that lies past the first 64 KiB of the output. Their identifiers must still differ.
    let opts = SaveOptions { id_entropy: Some([9; 16]), ..SaveOptions::default() };
    let rewrite = |last: u8| {
        let mut doc = reopen(large_source(last));
        doc.require_full_save_with_new_id();
        write_full(&doc, &opts).unwrap()
    };
    let (a, b) = (rewrite(b'A'), rewrite(b'B'));
    assert_eq!(a.len(), b.len(), "the two rewrites have the same length");
    let first_difference = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(usize::MAX);
    assert!(first_difference > 64 * 1024, "the rewrites first differ at byte {first_difference}, inside the first 64 KiB");
    assert_ne!(id_strings(&reopen(a)), id_strings(&reopen(b)), "content past 64 KiB must change the identifier");
}

/// Where [`stand_in_for_a_fresh_process`] writes its rewrite.
const ID_PROBE: &str = "PDFCRAFT_TEST_ID_PROBE";

/// The child half of [`separate_fresh_processes_get_different_identifiers`]: it runs alone, in its
/// own process, and writes a default-path rewrite of the standard fixture to the file the parent
/// named. Without the variable it is a plain no-op.
#[test]
fn stand_in_for_a_fresh_process() {
    let Some(path) = std::env::var_os(ID_PROBE) else { return };
    let mut doc = reopen(source(true));
    doc.require_full_save_with_new_id();
    std::fs::write(path, write_full(&doc, &SaveOptions::default()).unwrap()).unwrap();
}

#[test]
fn separate_fresh_processes_get_different_identifiers() {
    // The default identifier is meant to differ between fresh instances of the whole save pipeline,
    // not only between two calls in one process (which the counter already separates). On the native
    // test target a fresh instance is a fresh process, whose hash keys the operating system seeds;
    // the fix under test runs on WebAssembly, where no browser test harness exists — there the
    // browser's `crypto.getRandomValues` stands in for this seeding, and this test cannot see it.
    let exe = std::env::current_exe().unwrap();
    let probe = |name: &str| {
        let path = std::env::temp_dir().join(format!("pdfcraft-id-probe-{}-{name}.pdf", std::process::id()));
        let out = std::process::Command::new(&exe)
            .args(["--exact", "stand_in_for_a_fresh_process", "--nocapture", "--"])
            .env(ID_PROBE, &path)
            .output()
            .expect("run this test's own binary as a child");
        assert!(out.status.success(), "{name}: the child failed: {}", String::from_utf8_lossy(&out.stderr));
        let saved = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: the child wrote its file: {e}"));
        let ids = id_strings(&reopen(saved));
        assert_eq!(ids.len(), 2, "{name}");
        assert!(ids.iter().all(|s| s.len() == 16), "{name}: {ids:?}");
        let _ = std::fs::remove_file(&path);
        ids
    };
    let (first, second) = (probe("a"), probe("b"));
    assert_ne!(first, second, "two fresh processes must not share an identifier");
}
