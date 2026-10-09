use super::*;
use pdfcraft_cos::{Algorithm, NewEncryption, XrefEntry, write_full, write_incremental};
use std::sync::atomic::Ordering;

const STAMP: &str = "<< /Type /Sig /SubFilter /ETSI.RFC3161 /ByteRange [0 1 2 1] >>";

fn fixture(extra: &[&str]) -> Document {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R /Unknown 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"];
    objects.extend_from_slice(extra);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

fn stamp() -> Dict {
    pdfcraft_cos::Lexer::new(STAMP.as_bytes(), 0).object().unwrap().as_dict().unwrap().clone()
}

fn scans(cache: &DigestCache) -> usize {
    cache.discovery_scans.load(Ordering::Relaxed)
}

#[test]
fn unchanged_source_and_clones_skip_the_full_discovery_scan() {
    let doc = fixture(&[STAMP]);
    let cache = DigestCache::default();
    let expected = vec![ObjRef::new(3, 0)];
    assert_eq!(cache.timestamp_candidates(&doc), expected);
    assert_eq!(cache.timestamp_candidates(&doc), expected);
    assert_eq!(cache.timestamp_candidates(&doc.clone()), expected);
    assert_eq!(scans(&cache), 1);
    let listed = list_cached(&doc, doc.bytes(), &TrustStore::default(), &cache);
    assert_eq!(listed.len(), 1, "a timestamp needs no AcroForm");
    assert!(listed[0].doc_timestamp);
    assert_eq!(scans(&cache), 1);
}

#[test]
fn empty_discovery_is_cached_until_an_unknown_dictionary_becomes_a_timestamp() {
    let mut doc = fixture(&["<< /Unknown /Preserved >>"]);
    let cache = DigestCache::default();
    assert!(cache.timestamp_candidates(&doc).is_empty());
    assert!(cache.timestamp_candidates(&doc).is_empty());
    doc.update_dict(ObjRef::new(2, 0), |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    assert!(cache.timestamp_candidates(&doc).is_empty());
    doc.set(ObjRef::new(3, 0), stamp());
    assert_eq!(cache.timestamp_candidates(&doc), vec![ObjRef::new(3, 0)]);
    assert_eq!(scans(&cache), 1);
}

#[test]
fn dictionary_edits_additions_frees_and_undo_redo_reconcile_candidates() {
    let original = fixture(&[STAMP, "<< /Unknown /Preserved >>"]);
    let cache = DigestCache::default();
    let old = ObjRef::new(3, 0);
    let unknown = ObjRef::new(4, 0);
    assert_eq!(cache.timestamp_candidates(&original), vec![old]);
    let mut edited = original.clone();
    edited.update_dict(ObjRef::new(2, 0), |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    assert_eq!(cache.timestamp_candidates(&edited), vec![old]);
    edited
        .update_dict(old, |d| {
            d.remove(b"ByteRange");
        })
        .unwrap();
    edited.set(unknown, stamp());
    let added = edited.add(stamp());
    assert_eq!(cache.timestamp_candidates(&edited), vec![unknown, added]);
    let before_free = edited.clone();
    edited.free(unknown);
    edited.free(added);
    assert!(cache.timestamp_candidates(&edited).is_empty());
    assert_eq!(cache.timestamp_candidates(&before_free), vec![unknown, added]);
    assert_eq!(cache.timestamp_candidates(&original), vec![old]);
    assert!(cache.timestamp_candidates(&edited).is_empty());
    assert_eq!(scans(&cache), 1);
}

#[test]
fn first_discovery_on_an_edited_snapshot_does_not_hide_original_candidates() {
    let original = fixture(&[STAMP]);
    let mut edited = original.clone();
    edited.free(ObjRef::new(3, 0));
    let cache = DigestCache::default();
    assert!(cache.timestamp_candidates(&edited).is_empty());
    assert_eq!(cache.timestamp_candidates(&original), vec![ObjRef::new(3, 0)]);
    assert_eq!(scans(&cache), 1);
}

#[test]
fn field_membership_is_refreshed_even_when_discovery_is_cached() {
    let mut doc = fixture(&[STAMP]);
    let cache = DigestCache::default();
    assert!(list_cached(&doc, doc.bytes(), &TrustStore::default(), &cache)[0].doc_timestamp);
    let mut field = Dict::new();
    field.set(b"FT".to_vec(), Object::name("Sig"));
    field.set(b"T".to_vec(), Object::String(PdfString::literal("Approval")));
    field.set(b"V".to_vec(), Object::Ref(ObjRef::new(3, 0)));
    let field = doc.add(field);
    let mut form = Dict::new();
    form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(field)]));
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
    let listed = list_cached(&doc, doc.bytes(), &TrustStore::default(), &cache);
    assert_eq!(listed.len(), 1, "field values must not be duplicated as standalone timestamps");
    assert_eq!(listed[0].field, "Approval");
    assert!(!listed[0].doc_timestamp);
    doc.update_dict(doc.root().unwrap(), |d| {
        d.remove(b"AcroForm");
    })
    .unwrap();
    assert!(list_cached(&doc, doc.bytes(), &TrustStore::default(), &cache)[0].doc_timestamp);
    assert_eq!(scans(&cache), 1);
}

#[test]
fn reopened_incremental_and_renumbered_sources_get_fresh_discovery() {
    let mut doc = fixture(&["<< /Ignored true >>", STAMP]);
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"Unknown".to_vec(), Object::Ref(ObjRef::new(4, 0)))).unwrap();
    let cache = DigestCache::default();
    assert_eq!(cache.timestamp_candidates(&doc), vec![ObjRef::new(4, 0)]);
    let incremental = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_ne!(doc.source_identity(), incremental.source_identity());
    assert_eq!(cache.timestamp_candidates(&incremental), vec![ObjRef::new(4, 0)]);
    let full = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let renumbered = full.get(full.root().unwrap()).as_dict().unwrap().reference(b"Unknown").unwrap();
    assert_ne!(renumbered.num, 4);
    assert_eq!(cache.timestamp_candidates(&full), vec![renumbered]);
    let reopened_same_bytes = Document::open(full.bytes().clone()).unwrap();
    assert_ne!(full.source_identity(), reopened_same_bytes.source_identity());
    assert_eq!(cache.timestamp_candidates(&reopened_same_bytes), vec![renumbered]);
    assert_eq!(scans(&cache), 4);
}

#[test]
fn discovery_retains_neither_document_bytes_nor_old_sources() {
    let cache = DigestCache::default();
    let doc = fixture(&[STAMP]);
    let bytes = Arc::downgrade(doc.bytes());
    let source = doc.source_identity();
    cache.timestamp_candidates(&doc);
    drop(doc);
    assert!(bytes.upgrade().is_none());
    for _ in 0..64 {
        let next = fixture(&["<< /Unknown true >>"]);
        assert_ne!(source, next.source_identity());
        assert!(cache.timestamp_candidates(&next).is_empty());
    }
    assert_eq!(scans(&cache), 65);
}

#[test]
fn reconstructed_and_unreadable_objects_are_not_cached_as_complete() {
    let doc = fixture(&[STAMP]);
    let mut damaged = doc.bytes().as_ref().clone();
    damaged[0] = b'!';
    let repaired = Document::open(Arc::new(damaged)).unwrap();
    assert!(!repaired.repair_log().is_empty());
    let cache = DigestCache::default();
    for _ in 0..2 {
        assert_eq!(cache.timestamp_candidates(&repaired), vec![ObjRef::new(3, 0)]);
    }
    // A single invalid keyword immediately before endobj is deliberately recovered as
    // Null by parse_indirect. Use a stream without endstream to exercise a real read error.
    let unreadable = fixture(&[STAMP, "<< /Length 0 >>\nstream\nunterminated"]);
    assert!(unreadable.repair_log().is_empty());
    assert!(unreadable.try_get(4).is_err());
    assert!(unreadable.scan_objects_checked().any(|(r, object)| r.num == 4 && object.is_err()));
    for _ in 0..2 {
        assert_eq!(cache.timestamp_candidates(&unreadable), vec![ObjRef::new(3, 0)]);
        assert!(cache.discovery.lock().unwrap().is_none(), "incomplete discovery must not be published");
    }
    assert_eq!(scans(&cache), 4);
    assert!(cache.discovery.lock().unwrap().is_none());
}

#[test]
fn indirect_length_edits_and_frees_take_the_full_scan_fallback() {
    let original = fixture(&[STAMP, "<< /Length 5 0 R >>\nstream\nabc\nendstream", "3"]);
    let cache = DigestCache::default();
    assert_eq!(cache.timestamp_candidates(&original), vec![ObjRef::new(3, 0)]);
    let mut edited = original.clone();
    edited.set(ObjRef::new(5, 0), Object::Int(0));
    assert_eq!(cache.timestamp_candidates(&edited), vec![ObjRef::new(3, 0)]);
    assert!(cache.discovery.lock().unwrap().is_none());
    edited.free(ObjRef::new(5, 0));
    assert_eq!(cache.timestamp_candidates(&edited), vec![ObjRef::new(3, 0)]);
    assert_eq!(cache.timestamp_candidates(&original), vec![ObjRef::new(3, 0)]);
    assert_eq!(scans(&cache), 4);
}

#[test]
fn object_stream_changes_cannot_reuse_old_candidate_membership() {
    let original = fixture(&[STAMP]);
    let original = Document::open(Arc::new(write_full(&original, &SaveOptions::default()).unwrap())).unwrap();
    let cache = DigestCache::default();
    let candidates = cache.timestamp_candidates(&original);
    assert_eq!(candidates.len(), 1);
    let Some(XrefEntry::InStream { stream, .. }) = original.xref_entry(candidates[0].num) else { panic!("fixture must pack the timestamp") };
    let stream_ref = ObjRef::new(stream, original.generation(stream));
    let old_stream = original.get(stream_ref);
    let Object::Stream(old_stream) = old_stream.as_ref() else { panic!("missing object stream") };
    let mut decoded = old_stream.decoded().unwrap();
    let at = decoded.windows(b"ETSI.RFC3161".len()).position(|s| s == b"ETSI.RFC3161").unwrap();
    decoded[at] = b'X';
    let mut edited = original.clone();
    edited.set(stream_ref, Stream::flate(old_stream.dict.clone(), &decoded));
    assert!(cache.timestamp_candidates(&edited).is_empty());
    assert!(cache.discovery.lock().unwrap().is_none());
    edited.free(stream_ref);
    assert!(cache.timestamp_candidates(&edited).is_empty());
    assert_eq!(cache.timestamp_candidates(&original), candidates);
    assert_eq!(scans(&cache), 4);
}

#[test]
fn decode_limits_and_password_contexts_cannot_share_discovery() {
    let mut doc = fixture(&[STAMP]);
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes128,
        user_password: "user",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [29; 32],
    })
    .unwrap();
    let bytes = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
    assert!(Document::open_with_password(bytes.clone(), Some("wrong")).is_err());
    let user = Document::open_with_password(bytes.clone(), Some("user")).unwrap();
    let owner = Document::open_with_password(bytes.clone(), Some("owner")).unwrap();
    let limited = Document::open_with_stream_limit(bytes, Some("owner"), 1024 * 1024).unwrap();
    assert_ne!(user.source_identity(), owner.source_identity());
    assert_ne!(owner.source_identity(), limited.source_identity());
    let cache = DigestCache::default();
    for opened in [&user, &owner, &limited] {
        assert_eq!(cache.timestamp_candidates(opened).len(), 1);
        assert_eq!(cache.timestamp_candidates(opened).len(), 1);
    }
    assert_eq!(scans(&cache), 3);
    let mut unprotected = owner.clone();
    unprotected.remove_encryption();
    assert_eq!(cache.timestamp_candidates(&unprotected).len(), 1);
    assert!(cache.discovery.lock().unwrap().is_none());
    let reopened = Document::open(Arc::new(write_full(&unprotected, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(cache.timestamp_candidates(&reopened).len(), 1);
    assert_eq!(scans(&cache), 5);
}

#[test]
fn candidate_cache_is_bounded_without_truncating_results() {
    let extras = vec![STAMP; DISCOVERY_CACHE_LIMIT + 1];
    let doc = fixture(&extras);
    let cache = DigestCache::default();
    for _ in 0..2 {
        assert_eq!(cache.timestamp_candidates(&doc).len(), extras.len());
        assert!(cache.discovery.lock().unwrap().is_none());
    }
    assert_eq!(scans(&cache), 2);
}

#[test]
fn overlay_cache_is_bounded_and_new_documents_take_the_fallback() {
    let mut doc = fixture(&[STAMP]);
    for _ in 0..=DISCOVERY_CACHE_LIMIT {
        doc.add(Dict::new());
    }
    let cache = DigestCache::default();
    for _ in 0..2 {
        assert_eq!(cache.timestamp_candidates(&doc), vec![ObjRef::new(3, 0)]);
        assert!(cache.discovery.lock().unwrap().is_none());
    }
    let mut fresh = Document::new_empty();
    let timestamp = fresh.add(stamp());
    for _ in 0..2 {
        assert_eq!(cache.timestamp_candidates(&fresh), vec![timestamp]);
        assert!(cache.discovery.lock().unwrap().is_none());
    }
    assert_eq!(scans(&cache), 4);
}

/// Local, read-only corpus diagnostic; intentionally separate from ordinary timing.
/// The source PDF is never copied into the repository or written by this probe.
#[test]
#[ignore = "set PDFCRAFT_SIGNATURE_PROBE_SOURCE to a local PDF; parent coordinates resource use"]
fn real_file_discovery_cache_diagnostic() {
    let path = std::env::var_os("PDFCRAFT_SIGNATURE_PROBE_SOURCE").expect("PDFCRAFT_SIGNATURE_PROBE_SOURCE");
    let bytes = Arc::new(std::fs::read(path).unwrap());
    let doc = Document::open(bytes.clone()).unwrap();
    let mut objects = 0usize;
    let mut candidates = 0usize;
    let mut read_errors = 0usize;
    let mut failed_numbers = Vec::new();
    for (reference, object) in doc.scan_objects_checked() {
        objects += 1;
        match object {
            Ok(object) => candidates += usize::from(is_document_timestamp(&object)),
            Err(_) => {
                read_errors += 1;
                if failed_numbers.len() < 8 {
                    failed_numbers.push(reference.num);
                }
            }
        }
    }
    eprintln!(
        "signature-discovery-input\tbytes={}\tobjects={objects}\tcandidates={candidates}\tread_errors={read_errors}\tfirst_failed_numbers={failed_numbers:?}\trepairs={}\trevisions={}",
        bytes.len(),
        doc.repair_log().len(),
        doc.revisions().len()
    );
    let cache = DigestCache::default();
    let trust = TrustStore::default();
    let first = list_cached(&doc, &bytes, &trust, &cache);
    let cached_candidates = cache.discovery.lock().unwrap().as_ref().map(|entry| entry.candidates.len());
    eprintln!("signature-discovery-initial\tfull_scans={}\tcached_candidates={cached_candidates:?}\tsignatures={}", scans(&cache), first.len());
    assert_eq!(read_errors, 0, "input has unreadable objects; complete discovery must not be published");
    assert_eq!(cached_candidates, Some(candidates), "input did not qualify for discovery caching");
    assert_eq!(scans(&cache), 1);
    for _ in 0..3 {
        assert_eq!(list_cached(&doc, &bytes, &trust, &cache).len(), first.len());
    }
    eprintln!("signature-discovery-unchanged\tfull_scans={}", scans(&cache));
    assert_eq!(scans(&cache), 1);
    let mut edited = doc.clone();
    edited.update_dict(edited.root().unwrap(), |d| d.set(b"PdfcraftSignatureCacheProbe".to_vec(), Object::Int(1))).unwrap();
    assert_eq!(list_cached(&edited, &bytes, &trust, &cache).len(), first.len());
    assert_eq!(list_cached(&doc, &bytes, &trust, &cache).len(), first.len());
    assert_eq!(list_cached(&edited, &bytes, &trust, &cache).len(), first.len());
    eprintln!("signature-discovery-edit-undo-redo\tfull_scans={}", scans(&cache));
    assert_eq!(scans(&cache), 1);
    let added = edited.add(stamp());
    assert_eq!(list_cached(&edited, &bytes, &trust, &cache).len(), first.len() + 1);
    edited.free(added);
    assert_eq!(list_cached(&edited, &bytes, &trust, &cache).len(), first.len());
    assert_eq!(list_cached(&doc, &bytes, &trust, &cache).len(), first.len());
    eprintln!("signature-discovery-add-free-undo\tfull_scans={}", scans(&cache));
    assert_eq!(scans(&cache), 1);
    let reopened = Document::open(bytes.clone()).unwrap();
    assert_eq!(list_cached(&reopened, &bytes, &trust, &cache).len(), first.len());
    eprintln!("signature-discovery-reopen\tfull_scans={}", scans(&cache));
    assert_eq!(scans(&cache), 2);
}
