//! An indirect reference identifies an object number AND its generation (ISO 32000-2 §7.3.10).

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
    object_file_with_entry(objects, None)
}

fn object_file_with_entry(objects: &[(u32, u16, &str)], changed: Option<(u32, u16, bool)>) -> Arc<Vec<u8>> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (num, generation, value) in objects {
        offsets.push((*num, bytes.len(), *generation));
        bytes.extend_from_slice(format!("{num} {generation} obj\n{value}\nendobj\n").as_bytes());
    }
    let xref = bytes.len();
    let size = objects.len() + 1;
    bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for (num, offset, generation) in offsets {
        let (generation, free) = changed.filter(|(n, _, _)| *n == num).map_or((generation, false), |(_, g, free)| (g, free));
        let (offset, kind) = if free { (0, 'f') } else { (offset, 'n') };
        bytes.extend_from_slice(format!("{offset:010} {generation:05} {kind} \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Arc::new(bytes)
}

fn contradictory_entry(free: bool) -> Arc<Vec<u8>> {
    object_file_with_entry(
        &[(1, 0, "<< /Type /Catalog /Pages 2 0 R /Stale 3 0 R /Current 3 1 R >>"), (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"), (3, 1, "42")],
        Some((3, if free { 2 } else { 0 }, free)),
    )
}

#[test]
fn contradictory_xref_generation_recovers_the_matching_header_and_records_repair() {
    for warm in [false, true] {
        let doc = Document::open(contradictory_entry(false)).unwrap();
        if warm {
            assert_eq!(*doc.try_get(3).unwrap(), Object::Int(42));
        }
        assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
        assert_eq!(*doc.resolve(&Object::Ref(ObjRef::new(3, 1))), Object::Int(42));
        // Preserve the existing xref-matching behavior, but never accept an unrelated generation.
        assert_eq!(*doc.get(ObjRef::new(3, 0)), Object::Int(42));
        assert_eq!(*doc.get(ObjRef::new(3, 2)), Object::Null);
        assert!(!doc.xref_entry_is_exact(3), "a contradictory generation is not an exact xref entry");
        assert!(
            doc.repair_log()
                .iter()
                .any(|line| { line.contains("object 3") && line.contains("xref generation 0") && line.contains("header generation 1") }),
            "{:?}",
            doc.repair_log()
        );
        let repairs = doc.repair_log().to_vec();
        for _ in 0..3 {
            assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
        }
        assert_eq!(doc.repair_log(), repairs);

        let mut edited = doc.clone();
        edited.free(ObjRef::new(3, 1));
        for generation in [0, 1, 2] {
            assert_eq!(*edited.get(ObjRef::new(3, generation)), Object::Null);
        }
        edited.set(ObjRef::new(3, 2), Object::Int(99));
        assert_eq!(*edited.get(ObjRef::new(3, 1)), Object::Null);
        assert_eq!(*edited.get(ObjRef::new(3, 2)), Object::Int(99));
        assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
    }
}

/// A damaged row may name generation zero while the retained object and catalog use one.
fn contradictory_dictionary() -> Arc<Vec<u8>> {
    object_file_with_entry(
        &[(1, 0, "<< /Type /Catalog /Pages 2 0 R /Current 3 1 R >>"), (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"), (3, 1, "<< /Value 42 >>")],
        Some((3, 0, false)),
    )
}

fn assert_repaired_dictionary(bytes: Vec<u8>, value: i32) {
    let bytes = Arc::new(bytes);
    let oracle = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    let catalog = oracle.xref().get::<HayroDict<'_>>(ObjectIdentifier::new(1, 0)).unwrap();
    assert_eq!(catalog.get::<HayroDict<'_>>(b"Current").unwrap().get::<i32>(b"Value"), Some(value));
    let back = Document::open(bytes).unwrap();
    let catalog = back.get(back.root().unwrap());
    let current = back.resolve(catalog.as_dict().unwrap().get(b"Current").unwrap());
    assert_eq!(current.as_dict().unwrap().int(b"Value"), Some(i64::from(value)));
}

#[test]
fn repaired_header_references_survive_full_saves_and_dictionary_edits() {
    let original = contradictory_dictionary();
    let doc = Document::open(original.clone()).unwrap();
    for object_streams in [false, true] {
        assert_repaired_dictionary(write_full(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap(), 42);
    }
    let mut edited = doc.clone();
    edited.update_dict(ObjRef::new(3, 1), |dict| dict.set(b"Value".to_vec(), Object::Int(77))).unwrap();
    assert_eq!(edited.generation(3), 1);
    let incremental = write_incremental(&edited, &SaveOptions::default()).unwrap();
    assert!(edited.full_save_required(), "repair-aware dictionary edits normalize on full rewrite");
    assert_repaired_dictionary(incremental, 77);
    for object_streams in [false, true] {
        assert_repaired_dictionary(write_full(&edited, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap(), 77);
    }
    let mut restored = doc.clone();
    restored.overlay_changes_from(&edited).unwrap();
    assert_repaired_dictionary(write_incremental(&restored, &SaveOptions::default()).unwrap(), 77);
}

#[test]
fn free_xref_slots_never_revive_retained_object_headers() {
    for warm in [false, true] {
        let doc = Document::open(contradictory_entry(true)).unwrap();
        assert!(matches!(doc.xref_entry(3), Some(XrefEntry::Free { next_generation: 2 })));
        if warm {
            assert_eq!(*doc.try_get(3).unwrap(), Object::Null);
        }
        for generation in [0, 1, 2] {
            assert_eq!(*doc.get(ObjRef::new(3, generation)), Object::Null);
        }
        assert_eq!(*doc.resolve(&Object::Ref(ObjRef::new(3, 1))), Object::Null);
        assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    }
    // Control: the retained bytes really are a readable object when its slot is live.
    let live = Document::open(contradictory_entry(false)).unwrap();
    assert_eq!(*live.try_get(3).unwrap(), Object::Int(42));
}

#[test]
fn incremental_generation_identity_is_visible_to_an_independent_reader() {
    let original = reused_object();
    let mut doc = Document::open(original.clone()).unwrap();
    doc.set(ObjRef::new(3, 1), Object::Int(77));
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes.starts_with(&original));
    let oracle = hayro_syntax::Pdf::new(Arc::new(bytes)).unwrap();
    let catalog = oracle.xref().get::<HayroDict<'_>>(ObjectIdentifier::new(1, 0)).unwrap();
    assert_eq!(catalog.get::<i32>(b"Current"), Some(77));
    assert_eq!(catalog.get::<i32>(b"Stale"), None);
}

#[test]
fn generation_repair_diagnostics_are_bounded_without_disabling_recovery() {
    let mut objects = vec![(1, 0, "<< /Type /Catalog /Pages 2 0 R >>"), (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>")];
    objects.extend((3..303).map(|num| (num, 1, "42")));
    let bytes = object_file(&objects);
    let bytes = String::from_utf8(bytes.as_ref().clone()).unwrap().replace("00001 n \n", "00000 n \n");
    for materialized in [false, true] {
        let bytes = if materialized { bytes.replace("/Size 303", "/Size 302") } else { bytes.clone() };
        let doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
        assert_eq!(doc.repair_log().len(), 257);
        assert!(doc.repair_log().last().unwrap().contains("omitted after 256 objects"));
        assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
        assert_eq!(*doc.get(ObjRef::new(302, 1)), Object::Int(42));
        assert_eq!(*doc.get(ObjRef::new(302, 2)), Object::Null);
        assert_eq!(doc.repair_log().len(), 257);
    }
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
fn restoring_snapshot_preserves_changes_to_nonzero_generation_objects() {
    for generation in [1, 2] {
        let mut saved = Document::open(reused_object()).unwrap();
        let mut current = saved.clone();
        let reference = ObjRef::new(3, generation);
        current.set(reference, Object::Int(99));
        current.update_dict(current.root().unwrap(), |catalog| catalog.set(b"Current".to_vec(), Object::Ref(reference))).unwrap();

        saved.overlay_changes_from(&current).unwrap();
        assert_eq!(*saved.get(reference), Object::Int(99), "generation {generation}");
        let back = Document::open(Arc::new(write_incremental(&saved, &SaveOptions::default()).unwrap())).unwrap();
        assert!(back.repair_log().is_empty());
        assert_eq!(*back.get(reference), Object::Int(99), "generation {generation}");
        let catalog = back.get(back.root().unwrap());
        assert_eq!(*back.resolve(catalog.as_dict().unwrap().get(b"Current").unwrap()), Object::Int(99));
    }
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

/// A newer valid row must not hide damage in a retained older revision.
fn shadowed_generation_disagreement(materialized: bool, shifted: bool) -> Arc<Vec<u8>> {
    let original = contradictory_dictionary();
    let text = std::str::from_utf8(&original).unwrap();
    let previous: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
    let mut bytes = (*original).clone();
    let offset = bytes.len();
    bytes.extend_from_slice(b"3 1 obj\n<< /Value 77 >>\nendobj\n");
    let xref = bytes.len();
    let size = if materialized { 3 } else { 4 };
    bytes.extend_from_slice(
        format!("xref\n3 1\n{offset:010} 00001 n \ntrailer\n<< /Size {size} /Root 1 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    if shifted {
        bytes.splice(..0, b"junk before the header\n".iter().copied());
    }
    Arc::new(bytes)
}

#[test]
fn retained_generation_disagreements_are_reported_in_materialized_mode_too() {
    for materialized in [false, true] {
        for shifted in [false, true] {
            let doc = Document::open(shadowed_generation_disagreement(materialized, shifted)).unwrap();
            assert_eq!(doc.revisions().len(), 2, "the xref chain is retained");
            assert!(!doc.reconstructed());
            assert_eq!(doc.get(ObjRef::new(3, 1)).as_dict().unwrap().int(b"Value"), Some(77));
            assert!(
                doc.repair_log().iter().any(|line| line.contains("object 3: xref generation 0 disagrees with object header generation 1")),
                "materialized={materialized}, shifted={shifted}: {:?}",
                doc.repair_log()
            );
        }
    }
}

#[test]
fn repair_aware_edits_keep_both_aliases_and_full_save_normalizes_them() {
    let mut doc = Document::open(contradictory_entry(false)).unwrap();
    let snapshot = doc.clone();
    doc.set_existing(ObjRef::new(3, 0), Object::Int(77)).unwrap();
    for generation in [0, 1] {
        assert_eq!(*doc.get(ObjRef::new(3, generation)), Object::Int(77));
        assert_eq!(*snapshot.get(ObjRef::new(3, generation)), Object::Int(42));
    }
    let state = doc.read_state();
    assert!(doc.set_existing(ObjRef::new(3, 2), Object::Int(99)).is_err());
    assert!(state.matches(&doc));
    assert_eq!(*doc.get(ObjRef::new(3, 2)), Object::Null);
    let mut restored = snapshot.clone();
    restored.overlay_changes_from(&doc).unwrap();
    for changed in [&doc, &restored] {
        assert!(changed.full_save_required());
        for bytes in [
            write_incremental(changed, &SaveOptions::default()).unwrap(),
            write_full(changed, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap(),
        ] {
            let bytes = Arc::new(bytes);
            let oracle = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
            let catalog = oracle.xref().get::<HayroDict<'_>>(ObjectIdentifier::new(1, 0)).unwrap();
            assert_eq!(catalog.get::<i32>(b"Current"), Some(77));
            assert_eq!(catalog.get::<i32>(b"Stale"), Some(77), "both source aliases stay live");
            let back = Document::open(bytes).unwrap();
            assert!(back.repair_log().is_empty(), "normalized output has no generation repairs");
        }
    }
    // Explicit replacement keeps its narrower, caller-supplied generation contract.
    doc.set(ObjRef::new(3, 2), Object::Int(99));
    assert_eq!(*doc.get(ObjRef::new(3, 0)), Object::Null);
    assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Null);
    assert_eq!(*doc.get(ObjRef::new(3, 2)), Object::Int(99));
}

#[test]
fn hybrid_generation_diagnostics_observe_rows_before_the_table_hides_them() {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (num, generation, body) in [(1, 0, "<< /Type /Catalog /Pages 2 0 R >>"), (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"), (3, 1, "42")] {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{num} {generation} obj\n{body}\nendobj\n").as_bytes());
    }
    let hybrid = bytes.len();
    offsets.push(hybrid);
    bytes.extend_from_slice(b"4 0 obj\n<< /Type /XRef /Size 5 /W [1 4 2] /Index [3 1] /Length 7 >>\nstream\n");
    bytes.push(1);
    bytes.extend_from_slice(&(offsets[2] as u32).to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for (i, offset) in offsets.into_iter().enumerate() {
        let generation = u16::from(i == 2);
        bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R /XRefStm {hybrid} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(!doc.reconstructed());
    assert!(doc.xref_oddities().iter().any(|(_, num)| *num == 3), "the conflicting hybrid row is set aside");
    assert_eq!(*doc.get(ObjRef::new(3, 1)), Object::Int(42));
    assert_eq!(*doc.get(ObjRef::new(3, 0)), Object::Null, "the table still determines accepted references");
    assert!(doc.repair_log().iter().any(|line| line.contains("object 3: xref generation 0 disagrees with object header generation 1")));
}

#[test]
fn discarded_xref_repair_notes_do_not_survive_object_header_reconstruction() {
    let bytes =
        String::from_utf8((*contradictory_dictionary()).clone()).unwrap().replace("/Root 1 0 R", "/Root 99 0 R").replace("/Size 4", "/Size 3");
    let doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
    assert!(doc.reconstructed());
    assert!(doc.repair_log().iter().all(|line| !line.contains("xref generation")), "{:?}", doc.repair_log());
}

#[test]
fn restoring_equal_values_still_restores_the_accepted_generation_aliases() {
    for edited in [false, true] {
        for value in [42, 77] {
            let mut current = Document::open(contradictory_entry(false)).unwrap();
            if edited {
                current.set_existing(ObjRef::new(3, 0), Object::Int(42)).unwrap();
            }
            let mut saved = Document::open(object_file(&[
                (1, 0, "<< /Type /Catalog /Pages 2 0 R /Stale 3 0 R /Current 3 1 R >>"),
                (2, 0, "<< /Type /Pages /Kids [] /Count 0 >>"),
                (3, 0, &value.to_string()),
            ]))
            .unwrap();
            assert_eq!(*saved.get(ObjRef::new(3, 1)), Object::Null);
            saved.overlay_changes_from(&current).unwrap();
            for generation in [0, 1] {
                assert_eq!(*saved.get(ObjRef::new(3, generation)), Object::Int(42), "edited={edited}, value={value}");
            }
            assert!(saved.full_save_required());
        }
    }
}

#[test]
fn repaired_page_tree_aliases_keep_one_identity_in_full_saves() {
    let source = object_file_with_entry(
        &[
            (1, 0, "<< /Type /Catalog /Pages 2 1 R /Stale 2 2 R >>"),
            (2, 1, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, 0, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>"),
        ],
        Some((2, 0, false)),
    );
    for edited in [false, true] {
        let mut doc = Document::open(source.clone()).unwrap();
        if edited {
            doc.update_dict(ObjRef::new(2, 0), |dict| dict.set(b"Edited".to_vec(), Object::Bool(true))).unwrap();
        }
        let mut restored = Document::open(source.clone()).unwrap();
        restored.overlay_changes_from(&doc).unwrap();
        for current in [&doc, &restored] {
            for object_streams in [false, true] {
                let opts = SaveOptions { object_streams, ..SaveOptions::default() };
                let mut saves = vec![write_full(current, &opts).unwrap()];
                if edited {
                    saves.push(write_incremental(current, &opts).unwrap());
                }
                for bytes in saves {
                    let back = Document::open(Arc::new(bytes)).unwrap();
                    let catalog = back.get(back.root().unwrap());
                    let catalog = catalog.as_dict().unwrap();
                    let pages_ref = catalog.reference(b"Pages").unwrap();
                    let pages = back.get(pages_ref);
                    let child_ref = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0].as_ref().unwrap();
                    let child = back.get(child_ref);
                    assert_eq!(child.as_dict().unwrap().reference(b"Parent"), Some(pages_ref), "edited={edited}, packed={object_streams}");
                    assert!(matches!(catalog.get(b"Stale"), Some(Object::Null)), "unrelated generations stay null");
                    assert!(back.repair_log().is_empty());
                }
            }
        }
    }
}

#[test]
fn restoring_explicit_sets_removes_old_generation_aliases() {
    for value in [42, 77] {
        let mut current = Document::open(contradictory_entry(false)).unwrap();
        current.set(ObjRef::new(3, 0), Object::Int(42));
        for repaired_overlay in [false, true] {
            let mut saved = Document::open(contradictory_entry(false)).unwrap();
            if repaired_overlay {
                saved.set_existing(ObjRef::new(3, 0), Object::Int(value)).unwrap();
            }
            saved.overlay_changes_from(&current).unwrap();
            assert_eq!(*saved.get(ObjRef::new(3, 0)), Object::Int(42));
            assert_eq!(*saved.get(ObjRef::new(3, 1)), Object::Null, "value={value}, overlay={repaired_overlay}");
        }
    }
}
