//! An indirect reference identifies an object number AND its generation (ISO 32000-2 §7.3.10).
//! Backported from #1143, commit da54b14f085c930350d9b23839bd404cf4c3a625.

use std::sync::Arc;

use hayro_syntax::object::{Dict as HayroDict, ObjectIdentifier};
use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, XrefEntry, write_full, write_incremental};

/// Object 3 was reused: only 3 1 R refers to the current value, never 3 0 R.
fn reused_object() -> Arc<Vec<u8>> {
    object_file(&[
        (1, 0, "<< /Type /Catalog /Pages 2 0 R /Stale 3 0 R /Current 3 1 R >>"),
        (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"),
        (3, 1, "42"),
    ])
}

fn object_file(objects: &[(u32, u16, &str)]) -> Arc<Vec<u8>> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (num, generation, value) in objects {
        offsets.push((bytes.len(), generation));
        bytes.extend_from_slice(format!("{num} {generation} obj\n{value}\nendobj\n").as_bytes());
    }
    let xref = bytes.len();
    let size = objects.len() + 1;
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for (offset, generation) in offsets {
        bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Arc::new(bytes)
}

#[test]
fn references_require_the_current_generation_even_with_a_warm_cache() {
    let bytes = reused_object();
    let oracle = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    assert_eq!(oracle.xref().get::<i32>(ObjectIdentifier::new(3, 0)), None);
    assert_eq!(oracle.xref().get::<i32>(ObjectIdentifier::new(3, 1)), Some(42));

    let doc = Document::open(bytes).unwrap();
    assert!(doc.repair_log().is_empty());
    assert_eq!(*doc.get(ObjRef::new(3, 0)), Object::Null);
    assert_eq!(*doc.try_get(3).unwrap(), Object::Int(42));
    assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
    assert_eq!(*doc.get(ObjRef::new(3, 0)), Object::Null);
    assert_eq!(*doc.resolve(&Object::Ref(ObjRef::new(3, 0))), Object::Null);
    assert_eq!(*doc.get(ObjRef::new(3, 2)), Object::Null);
}

#[test]
fn overlay_generation_changes_do_not_retarget_old_references_or_clones() {
    let mut doc = Document::open(reused_object()).unwrap();
    let snapshot = doc.clone();
    let old = ObjRef::new(3, 1);
    let new = ObjRef::new(3, 2);
    doc.free(old);
    assert_eq!(*doc.get(old), Object::Null);
    doc.set(new, Object::Int(99));
    assert_eq!(*doc.get(old), Object::Null);
    assert_eq!(*doc.get(new), Object::Int(99));
    assert_eq!(*snapshot.get(old), Object::Int(42));
    assert_eq!(*snapshot.get(new), Object::Null);

    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved)).unwrap();
    assert!(back.repair_log().is_empty());
    assert_eq!(*back.get(old), Object::Null);
    assert_eq!(*back.get(new), Object::Int(99));
}

#[test]
fn full_save_does_not_turn_stale_references_into_live_ones() {
    let doc = Document::open(reused_object()).unwrap();
    for object_streams in [false, true] {
        let saved = write_full(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap();
        let saved = Arc::new(saved);
        let oracle = hayro_syntax::Pdf::new(saved.clone()).unwrap();
        let catalog = oracle.xref().get::<HayroDict<'_>>(ObjectIdentifier::new(1, 0)).unwrap();
        assert_eq!(catalog.get::<i32>(b"Current"), Some(42));
        assert_eq!(catalog.get::<i32>(b"Stale"), None);
        let back = Document::open(saved).unwrap();
        assert!(back.repair_log().is_empty());
        let catalog = back.get(back.root().unwrap());
        let catalog = catalog.as_dict().unwrap();
        assert_eq!(catalog.get(b"Stale"), Some(&Object::Null));
        assert_eq!(*back.resolve(catalog.get(b"Current").unwrap()), Object::Int(42));
    }
}

#[test]
fn compressed_objects_only_accept_generation_zero() {
    let doc = Document::open(reused_object()).unwrap();
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved)).unwrap();
    let catalog = back.get(back.root().unwrap());
    let current = catalog.as_dict().unwrap().reference(b"Current").unwrap();
    assert_eq!(current.generation, 0);
    assert!(matches!(back.xref_entry(current.num), Some(XrefEntry::InStream { .. })));
    assert_eq!(*back.get(current), Object::Int(42));
    assert_eq!(*back.get(ObjRef::new(current.num, 1)), Object::Null);
}

#[test]
fn indirect_stream_lengths_require_the_referenced_generation() {
    for generation in [0, 1] {
        // The embedded endstream is data when /Length resolves, but the existing missing-
        // length recovery stops there when the reference is stale.
        let stream = format!("<< /Length 4 {generation} R >>\nstream\nA\nendstream\nB\nendstream");
        let doc = Document::open(object_file(&[
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"),
            (3, 0, &stream),
            (4, 1, "13"),
        ]))
        .unwrap();
        let object = doc.get(ObjRef::new(3, 0));
        let Object::Stream(stream) = &*object else { panic!("expected stream") };
        assert_eq!(&stream.raw[..], if generation == 1 { b"A\nendstream\nB".as_slice() } else { b"A".as_slice() });
    }
}

/// Copying edits onto a saved document reads each object at its live generation. Asking for
/// generation 0 read a generation-1 object as Null on both sides, so an edit was skipped, and a
/// reused number was copied over as Null.
#[test]
fn overlaying_edits_keeps_objects_whose_generation_is_not_zero() {
    let reused = ObjRef::new(3, 1);
    let mut saved = Document::open(reused_object()).unwrap();
    let mut current = saved.clone();
    current.set(reused, Object::Int(7));
    saved.overlay_changes_from(&current).unwrap();
    assert_eq!(*saved.get(reused), Object::Int(7), "an edit at generation 1 is copied");

    let mut saved = Document::open(reused_object()).unwrap();
    let mut current = saved.clone();
    let next = ObjRef::new(3, 2);
    current.free(reused);
    current.set(next, Object::Int(99));
    saved.overlay_changes_from(&current).unwrap();
    assert_eq!(*saved.get(next), Object::Int(99), "a reused number keeps its value, not Null");
}
