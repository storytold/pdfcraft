use super::*;

#[test]
fn a_read_state_matches_a_clone_and_no_changed_document() {
    let mut doc = Document::new_empty();
    let a = doc.add(Object::Int(1));
    let state = doc.read_state();
    let snapshot = doc.clone();
    assert!(state.matches(&doc) && state.matches(&snapshot));
    // The same value set again is still a change: another allocation.
    doc.set(a, Object::Int(1));
    assert!(!state.matches(&doc) && state.matches(&snapshot));
    let edited = doc.read_state();
    doc.free(a);
    assert!(!edited.matches(&doc));
    let mut trailer_changed = snapshot.clone();
    trailer_changed.trailer_mut().set(b"Info".to_vec(), Object::Null);
    assert!(!state.matches(&trailer_changed));
    // Another document's state never matches, even with the same changes.
    let mut other = Document::new_empty();
    other.add(Object::Int(1));
    assert!(!other.read_state().matches(&snapshot));
}

/// Build a classic-xref file from object bodies (object i+1 = bodies[i]).
pub(crate) fn build(bodies: &[&str], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} {trailer} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn assert_compact_matches_materialized(doc: &Document) {
    assert!(matches!(doc.entries.as_ref(), XrefIndex::Compact { .. }), "valid xrefs should not retain a per-entry map");
    let (entries, _, revisions, _) = doc.read_xref_chain_materialized(&mut Vec::new()).unwrap();
    assert_eq!(revisions.as_slice(), doc.revisions());
    let max = entries.keys().next_back().copied().unwrap_or(0);
    assert_eq!(doc.entries.max_num(), max);
    for num in 0..=max {
        assert_eq!(doc.xref_entry(num), entries.get(&num).copied(), "object {num}");
    }
    let expected: Vec<_> = entries.iter().filter_map(|(&num, entry)| (!matches!(entry, XrefEntry::Free { .. })).then_some(num)).collect();
    assert_eq!(doc.object_numbers(), expected);
}

#[test]
fn compact_xref_tables_streams_and_revisions_match_the_materialized_reader() {
    let classic = Document::open(Arc::new(build(
        &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Value 9 >>"],
        "/Root 1 0 R",
    )))
    .unwrap();
    assert_compact_matches_materialized(&classic);

    let mut sparse = b"%PDF-1.7\n".to_vec();
    let root_offset = sparse.len();
    sparse.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 5 0 R >>\nendobj\n");
    let pages_offset = sparse.len();
    sparse.extend_from_slice(b"5 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let xref = sparse.len();
    sparse.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n1 1\n{root_offset:010} 00000 n \n5 1\n{pages_offset:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let sparse = Document::open(Arc::new(sparse)).unwrap();
    assert_eq!(sparse.object_numbers(), vec![1, 5]);
    assert_compact_matches_materialized(&sparse);

    let mut sparse_stream = b"%PDF-1.7\n".to_vec();
    let mut offsets = [0u32; 8];
    offsets[1] = sparse_stream.len() as u32;
    sparse_stream.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 5 0 R >>\nendobj\n");
    offsets[5] = sparse_stream.len() as u32;
    sparse_stream.extend_from_slice(b"5 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    offsets[6] = sparse_stream.len() as u32;
    sparse_stream.extend_from_slice(b"6 0 obj\n<< /Unused true >>\nendobj\n");
    offsets[7] = sparse_stream.len() as u32;
    let xref_row = |kind: u8, offset: u32, generation: u16| {
        let mut row = Vec::with_capacity(7);
        row.push(kind);
        row.extend_from_slice(&offset.to_be_bytes());
        row.extend_from_slice(&generation.to_be_bytes());
        row
    };
    let mut rows = Vec::new();
    rows.extend(xref_row(0, 0, u16::MAX));
    rows.extend(xref_row(1, offsets[1], 0));
    rows.extend(xref_row(1, offsets[5], 0));
    rows.extend(xref_row(1, offsets[6], 0));
    rows.extend(xref_row(1, offsets[7], 0));
    sparse_stream.extend_from_slice(
        format!("7 0 obj\n<< /Type /XRef /Size 8 /Root 1 0 R /W [1 4 2] /Index [0 2 5 3] /Length {} >>\nstream\n", rows.len()).as_bytes(),
    );
    sparse_stream.extend_from_slice(&rows);
    sparse_stream.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{}\n%%EOF\n", offsets[7]).as_bytes());
    let sparse_stream = Document::open(Arc::new(sparse_stream)).unwrap();
    assert_eq!(sparse_stream.object_numbers(), vec![1, 5, 6, 7]);
    assert_compact_matches_materialized(&sparse_stream);

    let stream_bytes = crate::write_full(&classic, &crate::SaveOptions::default()).unwrap();
    let stream = Document::open(Arc::new(stream_bytes)).unwrap();
    assert!(stream.revisions().last().is_some_and(|revision| revision.is_stream));
    assert_compact_matches_materialized(&stream);

    let mut edited = classic.clone();
    edited.update_dict(ObjRef::new(1, 0), |dict| dict.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
    edited.free(ObjRef::new(3, 0));
    let incremental = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
    let updated = Document::open(Arc::new(incremental)).unwrap();
    assert_eq!(updated.revisions().len(), 2);
    assert_compact_matches_materialized(&updated);
}

#[test]
fn malformed_size_uses_the_materialized_compatibility_path() {
    let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Value 9 >>"], "/Root 1 0 R");
    let marker = b"/Size 4";
    let at = bytes.windows(marker.len()).position(|window| window == marker).unwrap();
    bytes[at..at + marker.len()].copy_from_slice(b"/Size 2");
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(matches!(doc.entries.as_ref(), XrefIndex::Materialized(_)));
    assert_eq!(doc.next_num, 4);
    assert_eq!(doc.object_numbers(), vec![1, 2, 3]);
}

/// Entries the reader sets aside are remembered, so a caller comparing revisions can tell
/// that another reader may resolve the object differently.
#[test]
fn cross_reference_entries_that_are_set_aside_are_recorded() {
    let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
    let plain = Document::open(Arc::new(base.clone())).unwrap();
    assert!(plain.xref_oddities().is_empty());
    assert!((1..=3).all(|n| plain.xref_entry_is_exact(n)));
    let prev = String::from_utf8_lossy(&base).rfind("xref\n0 4").unwrap();
    let third = plain.xref_entry(3).unwrap();

    // A later section: object 3 "in use" at offset 0, object 2 listed twice with different
    // offsets, and object 0 at offset 0 (the usual producer slip, which names no object).
    let mut bytes = base.clone();
    let section = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 1\n0000000000 65535 n \n3 1\n0000000000 00000 n \n2 1\n0000000058 00000 n \n2 1\n0000000009 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.xref_oddities(), [(section as u64, 3), (section as u64, 2)]);
    // What the reader resolves is what it resolved before: the first entry it kept.
    assert_eq!(doc.xref_entry(3), Some(third));
    assert_eq!(doc.xref_entry(2), Some(XrefEntry::InFile { offset: 58, generation: 0 }));
}

/// An entry that misses its object still resolves (by searching) but is not exact.
#[test]
fn an_entry_that_misses_its_object_is_found_by_searching_but_is_not_exact() {
    let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
    let text = String::from_utf8_lossy(&base).into_owned();
    let at = text.find("3 0 obj").unwrap();
    let bytes = text.replacen(&format!("{at:010} 00000 n"), &format!("{:010} 00000 n", at + 2), 1).into_bytes();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.get(ObjRef { num: 3, generation: 0 }).as_string().map(|s| s.bytes.clone()), Some(b"three".to_vec()));
    assert!(!doc.xref_entry_is_exact(3));
    assert!(doc.xref_entry_is_exact(1) && doc.xref_entry_is_exact(2));
}

/// A classic table with `/Size` rows and object 0's free entry first, its subsection numbered
/// `1 n` instead of `0 n` (#2676): every row is one number off.
fn misnumbered_table(start: u32) -> Vec<u8> {
    let bodies = ["<< /Marker (first object) >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Type /Catalog /Pages 2 0 R >>"];
    let mut out = b"%PDF-1.7
"
    .to_vec();
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj
{body}
endobj
",
                i + 1
            )
            .as_bytes(),
        );
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n{start} 4\n0000000000 65535 f \n").as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 3 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn a_table_numbered_from_1_that_begins_with_object_0_is_read_from_0() {
    let doc = Document::open(Arc::new(misnumbered_table(1))).unwrap();
    let first = doc.get(ObjRef::new(1, 0)).as_dict().cloned().expect("object 1 is not free");
    assert!(first.contains(b"Marker"));
    assert_eq!(doc.root(), Some(ObjRef::new(3, 0)));
    assert!(doc.repair_log().iter().any(|l| l.contains("numbered from 1")), "{:?}", doc.repair_log());
    // A correct table reads the same and logs nothing.
    let fine = Document::open(Arc::new(misnumbered_table(0))).unwrap();
    assert!(fine.get(ObjRef::new(1, 0)).as_dict().is_some_and(|d| d.contains(b"Marker")));
    assert!(!fine.repair_log().iter().any(|l| l.contains("numbered from 1")));
}

#[test]
fn a_table_that_really_starts_at_1_keeps_its_numbers() {
    // Object 1 deleted for good (generation 65535) in a subsection that does start at 1: its
    // second row locates object 2, so nothing is shifted.
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let pages = bytes.len();
    bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let catalog = bytes.len();
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n1 3\n0000000000 65535 f \n{pages:010} 00000 n \n{catalog:010} 00000 n \ntrailer\n<< /Size 4 /Root 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.get(ObjRef::new(2, 0)).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Pages")));
    assert!(doc.get(ObjRef::new(1, 0)).as_dict().is_none(), "object 1 stays free");
    assert!(!doc.repair_log().iter().any(|l| l.contains("numbered from 1")));
}

/// A one-page scan: the page draws image 5 with content stream 4.
fn scanned_page() -> Vec<u8> {
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>",
            "<< /Length 21 >>\nstream\n10 0 0 10 0 0 cm /Im0 Do\nendstream",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\nA\nendstream",
        ],
        "/Root 1 0 R",
    )
}

/// `bytes` with its cross-reference table replaced by `table` (the lines after `xref`).
fn with_table(bytes: &[u8], table: &[String], size: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let at = text.rfind("\nxref\n").unwrap() + 1;
    let mut out = bytes[..at].to_vec();
    out.extend_from_slice(format!("xref\n{}\ntrailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n", table.join("\n")).as_bytes());
    out
}

/// The table's entry lines, object 0 first.
fn table_entries(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let at = text.rfind("\nxref\n").unwrap() + 1;
    text[at..].lines().skip(2).take_while(|l| !l.starts_with("trailer")).map(str::to_string).collect()
}

/// The classic off-by-one: the subsection says it starts at 0 but lists objects 1 to 5, so
/// the last object has no entry. Viewers rebuild the table and show the page; a reader that
/// took the missing entry for a null object dropped the image when pages were copied (#1628).
#[test]
fn an_off_by_one_xref_subsection_still_finds_the_last_object() {
    let base = scanned_page();
    let entries = table_entries(&base);
    let mut table = vec!["0 5".to_string()];
    table.extend(entries[1..6].iter().cloned());
    let doc = Document::open(Arc::new(with_table(&base, &table, 5))).unwrap();
    let image = doc.get(ObjRef::new(5, 0));
    assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "{image:?}");
    assert!(matches!(&*image, Object::Stream(_)));
    assert!(matches!(&*doc.get(ObjRef::new(4, 0)), Object::Stream(s) if &s.raw[..] == b"10 0 0 10 0 0 cm /Im0 Do"));
    assert!(doc.repair_log().iter().any(|l| l.contains("object headers")), "{:?}", doc.repair_log());
}

/// An "in use" entry at offset 0 says nothing about where the object is: the object is
/// found by its header, and the repair is recorded.
#[test]
fn an_object_listed_at_offset_zero_is_found_by_its_header() {
    let base = scanned_page();
    for num in [4usize, 5] {
        let mut table = vec!["0 6".to_string()];
        table.extend(table_entries(&base));
        table[num + 1] = "0000000000 00000 n ".into();
        let doc = Document::open(Arc::new(with_table(&base, &table, 6))).unwrap();
        assert!(matches!(&*doc.get(ObjRef::new(num as u32, 0)), Object::Stream(_)), "object {num}");
        assert!(doc.repair_log().iter().any(|l| l.contains("object headers")), "object {num}: {:?}", doc.repair_log());
    }
}

/// A valid table that simply does not list every number below `/Size` is not repaired: an
/// unlisted number is a null object (§7.3.10), even when an old body with that number is
/// still in the file.
#[test]
fn a_consistent_table_with_unlisted_numbers_is_taken_as_it_is() {
    let base = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(three)"], "/Root 1 0 R");
    let at = String::from_utf8_lossy(&base).rfind("\nxref\n").unwrap() + 1;
    let mut bytes = base[..at].to_vec();
    bytes.extend_from_slice(b"4 0 obj\n(stale)\nendobj\n");
    let mut table = vec!["0 4".to_string()];
    table.extend(table_entries(&base));
    let doc = Document::open(Arc::new(with_table(&[&bytes[..], &base[at..]].concat(), &table, 6))).unwrap();
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert_eq!(*doc.get(ObjRef::new(4, 0)), Object::Null);
    assert!(matches!(&*doc.get(ObjRef::new(3, 0)), Object::String(_)));
}

/// Filling the gaps of a damaged table never brings back an object that an incremental
/// update freed: its body is still in the file, but the newer section says it is gone.
#[test]
fn an_object_freed_by_an_update_stays_freed_when_the_table_is_repaired() {
    let base = scanned_page();
    // The original table lists the content stream at offset 0, so the gaps get filled.
    let mut table = vec!["0 6".to_string()];
    table.extend(table_entries(&base));
    table[5] = "0000000000 00000 n ".into();
    let mut bytes = with_table(&base, &table, 6);
    let prev = String::from_utf8_lossy(&bytes).rfind("\nxref\n").unwrap() + 1;
    let section = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n5 1\n0000000000 00001 f \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(matches!(&*doc.get(ObjRef::new(4, 0)), Object::Stream(_)), "the gap is filled");
    assert_eq!(*doc.get(ObjRef::new(5, 0)), Object::Null, "the freed image stays freed");
    assert_eq!(doc.xref_entry(5), Some(XrefEntry::Free { next_generation: 1 }));
}

/// Repairing a damaged table fills only the numbers the damage leaves out: an old body of a
/// number no section lists, under an oversized `/Size`, stays a null object.
#[test]
fn a_repaired_table_leaves_an_unlisted_number_null() {
    let base = scanned_page();
    let at = String::from_utf8_lossy(&base).rfind("\nxref\n").unwrap() + 1;
    let entries = table_entries(&base);
    let mut offset_zero = vec!["0 6".to_string()];
    offset_zero.extend(entries.iter().cloned());
    offset_zero[5] = "0000000000 00000 n ".into();
    let mut off_by_one = vec!["0 5".to_string()];
    off_by_one.extend(entries[1..6].iter().cloned());
    for (name, table, found) in [("offset 0", offset_zero, 4), ("off by one", off_by_one, 5)] {
        let bytes = [&base[..at], b"7 0 obj\n(stale)\nendobj\n", &base[at..]].concat();
        let doc = Document::open(Arc::new(with_table(&bytes, &table, 9))).unwrap();
        assert!(matches!(&*doc.get(ObjRef::new(found, 0)), Object::Stream(_)), "{name}: the gap is filled");
        assert_eq!(*doc.get(ObjRef::new(7, 0)), Object::Null, "{name}: {:?}", doc.repair_log());
        assert_eq!(doc.xref_entry(7), None, "{name}");
    }
}

/// The last object of a subsection numbered one too low is filled from that subsection,
/// whatever later happens to the row before it: an incremental save that rewrites object 4,
/// or object 4's own row at offset 0.
#[test]
fn the_last_object_of_a_shifted_subsection_is_filled_whatever_its_neighbour_says() {
    let base = scanned_page();
    let entries = table_entries(&base);
    let mut off_by_one = vec!["0 5".to_string()];
    off_by_one.extend(entries[1..6].iter().cloned());
    let shifted = with_table(&base, &off_by_one, 5);
    let prev = String::from_utf8_lossy(&shifted).rfind("\nxref\n").unwrap() + 1;
    let mut updated = shifted.clone();
    let body = updated.len();
    updated.extend_from_slice(b"4 0 obj\n<< /Length 21 >>\nstream\n10 0 0 10 0 0 cm /Im0 Do\nendstream\nendobj\n");
    let section = updated.len();
    updated.extend_from_slice(
        format!("xref\n4 1\n{body:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\nstartxref\n{section}\n%%EOF\n").as_bytes(),
    );
    let mut neighbour_at_zero = off_by_one.clone();
    neighbour_at_zero[5] = "0000000000 00000 n ".into();
    for (name, bytes) in [("updated", updated), ("row 4 at offset 0", with_table(&base, &neighbour_at_zero, 5))] {
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let image = doc.get(ObjRef::new(5, 0));
        assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "{name}: {:?}", doc.repair_log());
    }
}

/// The image the first page draws as `/Im1`, followed through the page tree.
fn first_page_image(doc: &Document) -> Arc<Object> {
    let catalog = doc.get(doc.root().unwrap());
    let pages = doc.resolve(catalog.as_dict().unwrap().get(b"Pages").unwrap());
    let page = doc.resolve(&pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0]);
    let resources = doc.resolve(page.as_dict().unwrap().get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    doc.resolve(xobjects.as_dict().unwrap().get(b"Im1").unwrap())
}

/// The opposite off-by-one (#1628): the subsection says it starts at 1, but its first entry
/// is object 0's free-list head, so every entry lands one number too high and object 1, the
/// image, reads as free. The offset the table gives object 2 holds `1 0 obj`, so the
/// subsection is read from 0, and the image survives a full save.
#[test]
fn a_subsection_from_one_that_opens_with_the_free_list_head_is_read_from_zero() {
    let base = build(
        &[
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\nABCDEFGHIJKL\nendstream",
            "<< /Length 29 >>\nstream\nq 72 0 0 72 0 0 cm /Im1 Do Q\nendstream",
            "<< /Type /Page /Parent 4 0 R /MediaBox [0 0 72 72] /Resources << /XObject << /Im1 1 0 R >> >> /Contents 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Catalog /Pages 4 0 R >>",
            "<< /Subject (synthetic) >>",
        ],
        "/Root 5 0 R /Info 6 0 R",
    );
    // /Size 7 is one short of the shifted numbers; /Size 8 covers them all. Junk before the
    // header moves every object past the offsets the table gives; it is not a comment, which
    // the lexer would skip on its way to `1 0 obj` anyway.
    for (size, junk) in [(7, ""), (8, ""), (7, "Content-Type: application/pdf\n\n")] {
        let text = String::from_utf8_lossy(&base).replace("\nxref\n0 7\n", "\nxref\n1 7\n").replace("/Size 7 ", &format!("/Size {size} "));
        let doc = Document::open(Arc::new(format!("{junk}{text}").into_bytes())).unwrap();
        let image = first_page_image(&doc);
        assert_eq!(image.as_dict().and_then(|d| d.name(b"Subtype")), Some(&b"Image"[..]), "/Size {size} {junk:?}: {image:?}");
        assert_eq!(doc.xref_entry(0), Some(XrefEntry::Free { next_generation: 65535 }), "/Size {size} {junk:?}");
        assert!(doc.repair_log().iter().any(|l| l.contains("from 0")), "/Size {size} {junk:?}: {:?}", doc.repair_log());
        let saved = Document::open(Arc::new(crate::write_full(&doc, &crate::SaveOptions::default()).unwrap())).unwrap();
        let image = first_page_image(&saved);
        assert!(matches!(&*image, Object::Stream(s) if &s.raw[..] == b"ABCDEFGHIJKL"), "/Size {size} {junk:?}: {image:?}");
    }
}

/// Object 1 really freed: the subsection from 1 opens with a free entry, but the next entry's
/// offset holds object 2, not object 1. The table is taken as it is, and the old body of
/// object 1 still in the file stays freed.
#[test]
fn a_subsection_from_one_whose_next_offset_holds_object_two_is_taken_as_it_is() {
    let base = build(&["(freed)", "<< /Type /Catalog /Pages 3 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 2 0 R");
    let entries = table_entries(&base);
    let mut table = vec!["1 3".to_string(), "0000000000 65535 f ".to_string()];
    table.extend(entries[2..].iter().cloned());
    let text = String::from_utf8_lossy(&base).into_owned();
    let at = text.rfind("\nxref\n").unwrap() + 1;
    for junk in ["", "Content-Type: application/pdf\n\n"] {
        let mut bytes = junk.as_bytes().to_vec();
        bytes.extend_from_slice(&base[..at]);
        bytes.extend_from_slice(format!("xref\n{}\ntrailer\n<< /Size 4 /Root 2 0 R >>\nstartxref\n{at}\n%%EOF\n", table.join("\n")).as_bytes());
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(doc.xref_entry(1), Some(XrefEntry::Free { next_generation: 65535 }), "{junk:?}");
        assert_eq!(*doc.get(ObjRef::new(1, 0)), Object::Null, "{junk:?}: the freed object stays freed");
        assert!(matches!(&*doc.get(ObjRef::new(2, 0)), Object::Dict(d) if d.name(b"Type") == Some(&b"Catalog"[..])), "{junk:?}");
        assert!(!doc.repair_log().iter().any(|l| l.contains("from 0")), "{junk:?}: {:?}", doc.repair_log());
        assert!(!junk.is_empty() || doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    }
}

#[test]
fn opening_an_exhausted_classic_xref_repairs_and_allocates() {
    // The extra in-use row need not be loaded to exhaust the next object number.
    let bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\nxref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n4294967295 1\n0000000058 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n110\n%%EOF\n";
    assert_eq!(bytes.len(), 266);
    let mut doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
    assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
    assert!(doc.repair_log().iter().any(|line| line.contains("exhausted xref")));
    assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
}

#[test]
fn opening_an_exhausted_xref_stream_repairs_and_allocates() {
    let mut bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
    let offset = bytes.len();
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /XRef /Size 4 /Root 1 0 R /W [1 4 2] /Index [0 3 4294967295 1] /Length 28 >>\nstream\n");
    for (kind, position, generation) in [(0_u8, 0_u32, 65535_u16), (1, 9, 0), (1, 58, 0), (1, 58, 0)] {
        bytes.push(kind);
        bytes.extend_from_slice(&position.to_be_bytes());
        bytes.extend_from_slice(&generation.to_be_bytes());
    }
    bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{offset}\n%%EOF\n").as_bytes());
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
    assert!(doc.repair_log().iter().any(|line| line.contains("exhausted xref")));
    assert_eq!(doc.add(Object::Null), ObjRef::new(4, 0));
}

#[test]
fn exhausted_size_and_high_edits_allocate_without_overwriting_objects() {
    let ordinary = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    for size in ["4294967295", "4294967296", "9223372036854775807"] {
        let bytes = String::from_utf8(ordinary.clone()).unwrap().replace("/Size 3", &format!("/Size {size}"));
        let mut doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
        assert!(doc.repair_log().iter().any(|line| line.contains("exhausted trailer")));
        assert_eq!(doc.add(Object::Int(42)), ObjRef::new(3, 0));
        doc.set(ObjRef::new(u32::MAX, 0), Object::Int(99));
        doc.set(ObjRef::new(u32::MAX - 1, 0), Object::Int(98));
        assert_eq!(doc.add(Object::Int(43)), ObjRef::new(4, 0));
        assert_eq!(doc.get(ObjRef::new(3, 0)).as_int(), Some(42));
        assert_eq!(doc.get(ObjRef::new(u32::MAX, 0)).as_int(), Some(99));
        assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
    }
    let mut doc = Document::new_empty();
    doc.set(ObjRef::new(u32::MAX - 2, 0), Object::Null);
    assert_eq!(doc.add(Object::Null), ObjRef::new(u32::MAX - 1, 0));
    assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
    assert_eq!(doc.add(Object::Null), ObjRef::new(4, 0));
}

#[test]
fn exhausted_xref_controls_preserve_ordinary_ignored_and_repaired_entries() {
    let ordinary = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    for (extra, repaired) in
        [("", false), ("4294967295 1\n0000000000 00000 n \n", false), ("4294967295 2\n0000000058 00000 n \n0000000058 00000 n \n", true)]
    {
        let bytes = String::from_utf8(ordinary.clone()).unwrap().replacen("trailer\n", &format!("{extra}trailer\n"), 1);
        let mut doc = Document::open(Arc::new(bytes.into_bytes())).unwrap();
        assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
        assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
        assert_eq!(!doc.repair_log().is_empty(), repaired);
        if repaired {
            assert!(doc.repair_log().iter().any(|line| line.contains("xref") && line.contains("range")));
        }
        assert_eq!(doc.add(Object::Null), ObjRef::new(3, 0));
    }
}

#[test]
fn invalid_xref_object_ranges_are_reconstructed() {
    // Original in-memory PDF: an intact catalog followed by a damaged xref section.
    for stream in [false, true] {
        for (start, count) in [(i64::MAX, 2), (-1, 2), (4_294_967_296, 1), (4_294_967_295, 2)] {
            let mut bytes =
                b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
            let offset = bytes.len();
            if stream {
                bytes.extend_from_slice(
                    format!("3 0 obj\n<< /Type /XRef /Root 1 0 R /Size 4 /W [1 1 1] /Index [{start} {count}] /Length 6 >>\nstream\n").as_bytes(),
                );
                bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
                bytes.extend_from_slice(b"\nendstream\nendobj\n");
            } else {
                bytes.extend_from_slice(
                    format!("xref\n{start} {count}\n0000000000 65535 f \n0000000000 65535 f \ntrailer\n<< /Root 1 0 R /Size 3 >>\n").as_bytes(),
                );
            }
            bytes.extend_from_slice(format!("startxref\n{offset}\n%%EOF\n").as_bytes());
            let doc = Document::open(Arc::new(bytes)).unwrap();
            assert!(
                doc.repair_log().iter().any(|line| line.contains("xref") && line.contains("range")),
                "stream={stream}, start={start}, count={count}: {:?}",
                doc.repair_log()
            );
            assert_eq!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().name(b"Type"), Some(b"Catalog".as_slice()));
        }
    }
}

#[test]
fn objects_only_an_unreadable_older_section_locates_are_found_by_scanning() {
    // The newest section lists the catalog only; its /Prev points past the end of the file,
    // so the page tree is reachable only by scanning object headers, as readers do (#1656).
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (num, body) in
        [(1, "<< /Type /Catalog /Pages 2 0 R >>"), (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"), (3, "<< /Type /Page /Parent 2 0 R >>")]
    {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref\n0 2\n0000000000 65535 f \n{:010} 00000 n \ntrailer\n<< /Root 1 0 R /Size 4 /Prev 999999 >>\nstartxref\n{xref}\n%%EOF\n",
            offsets[0]
        )
        .as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.get(ObjRef::new(2, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(b"Pages".as_slice()));
    assert_eq!(doc.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(b"Page".as_slice()));
    assert!(doc.repair_log().iter().any(|l| l.contains("999999")), "{:?}", doc.repair_log());
}

#[test]
fn self_referential_length_does_not_overflow() {
    let bytes = build(
        &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Length 3 0 R >>\nstream\nabc\nendstream"],
        "/Root 1 0 R",
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    // Either recovered via the endstream search or reported — but never a stack overflow.
    let o = doc.get(ObjRef::new(3, 0));
    if let Object::Stream(s) = o.as_ref() {
        assert_eq!(&s.raw[..], b"abc");
    }
}

#[test]
fn mutually_referential_lengths_do_not_overflow() {
    let bytes = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [] /Count 0 >>",
            "<< /Length 4 0 R >>\nstream\nabc\nendstream",
            "<< /Length 3 0 R >>\nstream\nxyz\nendstream",
        ],
        "/Root 1 0 R",
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let _ = doc.get(ObjRef::new(3, 0));
    let _ = doc.get(ObjRef::new(4, 0));
}

#[test]
fn reads_classic_revision_and_edits_are_copy_on_write() {
    let bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.revisions().len(), 1);
    assert!(doc.repair_log().is_empty());
    let mut edited = doc.clone();
    edited.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
    assert!(edited.is_modified() && !doc.is_modified(), "clone is an independent snapshot");
    assert!(doc.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"Lang").is_none());
}

#[test]
fn object_numbers_merge_source_and_overlay_in_order() {
    let mut doc = Document::new_empty();
    doc.overlay.clear();
    doc.entries = Arc::new(XrefIndex::Materialized(Arc::new(BTreeMap::from([
        (0, XrefEntry::Free { next_generation: 65535 }),
        (2, XrefEntry::InFile { offset: 10, generation: 7 }),
        (4, XrefEntry::Free { next_generation: 9 }),
        (6, XrefEntry::InStream { stream: 80, index: 1 }),
        (8, XrefEntry::InFile { offset: 20, generation: 5 }),
        (10, XrefEntry::InFile { offset: 30, generation: 3 }),
        (u32::MAX, XrefEntry::InFile { offset: 40, generation: 1 }),
    ]))));
    doc.set(ObjRef::new(1, 2), Object::Int(1)); // before the first live source slot
    doc.set(ObjRef::new(2, 8), Object::Int(2)); // override, without duplication
    doc.set(ObjRef::new(4, 9), Object::Int(4)); // reuse a source free slot
    doc.free(ObjRef::new(6, 0)); // remove a compressed source object
    doc.free(ObjRef::new(7, 2)); // free a number absent from the source
    doc.free(ObjRef::new(10, 3));
    doc.set(ObjRef::new(11, 4), Object::Int(11));
    assert_eq!(doc.object_numbers(), vec![1, 2, 4, 8, 11, u32::MAX]);
    assert_eq!(doc.generation(2), 8);
    assert_eq!(doc.generation(4), 9);
    assert_eq!(doc.generation(6), 1);
    assert_eq!(doc.generation(8), 5);
    assert_eq!(doc.generation(10), 4);
    doc.overlay.clear();
    assert_eq!(doc.object_numbers(), vec![2, 6, 8, 10, u32::MAX]);
    doc.entries = Arc::new(XrefIndex::empty());
    assert!(doc.object_numbers().is_empty());
    doc.set(ObjRef::new(5, 7), Object::Int(5));
    assert_eq!(doc.object_numbers(), vec![5]);
}

#[test]
fn full_object_scans_preserve_edits_without_retaining_the_scan() {
    let mut bodies = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_owned(), "<< /Type /Pages /Kids [] /Count 0 >>".to_owned()];
    bodies.extend((0..400).map(|i| format!("<< /Value {i} >>")));
    let refs: Vec<_> = bodies.iter().map(String::as_str).collect();
    let mut doc = Document::open(Arc::new(build(&refs, "/Root 1 0 R"))).unwrap();
    doc.set(ObjRef::new(3, 0), Object::Int(900));
    doc.free(ObjRef::new(4, 0));
    let added = doc.add(Object::Int(901));
    let cached = doc.cache.lock().unwrap().len();
    let held: Vec<_> = doc.scan_objects().collect();
    assert_eq!(held.len(), 402);
    assert_eq!(doc.cache.lock().unwrap().len(), cached, "scanning must not populate the editor cache");
    assert!(doc.objstms.lock().unwrap().is_empty());
    assert!(!held.iter().any(|(r, _)| r.num == 4));
    assert_eq!(held.iter().find(|(r, _)| r.num == 3).unwrap().1.as_int(), Some(900));
    assert_eq!(held.iter().find(|(r, _)| *r == added).unwrap().1.as_int(), Some(901));
    assert_eq!(held.iter().find(|(r, _)| r.num == 400).unwrap().1.as_dict().unwrap().int(b"Value"), Some(397));
    // Previously returned objects survive cache eviction and dropping the source.
    drop(doc);
    assert_eq!(held.iter().find(|(r, _)| r.num == 128).unwrap().1.as_dict().unwrap().int(b"Value"), Some(125));
}

#[test]
fn temporary_reader_keeps_file_views_and_evicts_data_of_its_own() {
    // A stream read from the file is a view into the input: caching it costs no copy, so
    // even a large one stays cached (its /Length object too) instead of being re-parsed.
    let body = "x".repeat(ObjectReader::STREAM_BYTES + 1);
    let stream = format!("<< /Length 4 0 R >>\nstream\n{body}\nendstream");
    let length = body.len().to_string();
    let bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", &stream, &length], "/Root 1 0 R");
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let mut reader = doc.object_reader();
    let read = reader.get(ObjRef::new(3, 0));
    let Object::Stream(s) = &*read else { panic!("stream") };
    assert!(s.raw.shares(&reader.document.data), "no copy of the file's bytes");
    assert!(reader.document.cache.lock().unwrap().contains_key(&3), "a view costs nothing to keep");
    // Data of its own (decrypted streams, strings) counts: past the budget the cache is
    // cleared after the next read, including a failed one.
    let owned = Object::Stream(crate::Stream::from_raw(Dict::new(), vec![b'y'; ObjectReader::STREAM_BYTES + 1]));
    reader.document.cache.lock().unwrap().insert(3, Arc::new(owned));
    assert_eq!(*reader.get(ObjRef::new(99, 0)), Object::Null);
    assert!(reader.document.cache.lock().unwrap().is_empty(), "owned data over budget is evicted");
}

#[test]
fn temporary_reader_and_full_save_do_not_fill_snapshot_caches() {
    let mut source = Document::new_empty();
    let objects: Vec<_> = (0..260)
        .map(|i| {
            let mut value = vec![b'x'; 48 * 1024];
            value[..4].copy_from_slice(&(i as u32).to_le_bytes());
            Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
        })
        .collect();
    source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
    let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
    let doc = Document::open(bytes).unwrap();
    let root = doc.get(doc.root().unwrap());
    let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap();
    let snapshot = doc.clone();
    let cached = doc.cache.lock().unwrap().len();
    let streams = doc.objstms.lock().unwrap().len();
    let mut reader = doc.object_reader();
    let mut held = None;
    for (i, reference) in references.iter().enumerate() {
        let Object::Ref(reference) = reference else { panic!("indirect object") };
        let object = reader.get(*reference);
        assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
        if i == 0 {
            held = Some(object);
        }
        assert!(reader.document.cache.lock().unwrap().len() < ObjectReader::OBJECTS);
        let decoded = reader.document.objstms.lock().unwrap();
        assert!(decoded.len() < ObjectReader::OBJECTS);
        let bytes: usize = decoded.values().map(|s| s.data.capacity() + s.index.capacity() * std::mem::size_of::<Option<(u32, usize)>>()).sum();
        assert!(bytes <= ObjectReader::STREAM_BYTES || decoded.len() == 1, "only one oversized stream may remain");
    }
    drop(reader);
    assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes(), "owned results survive evictions");
    for object_streams in [false, true] {
        let saved = crate::write_full(&doc, &crate::SaveOptions { object_streams, ..Default::default() }).unwrap();
        assert!(Document::open(Arc::new(saved)).unwrap().repair_log().is_empty());
        assert_eq!(doc.cache.lock().unwrap().len(), cached, "full saves must not fill the editor cache");
        assert_eq!(doc.objstms.lock().unwrap().len(), streams);
        assert!(Arc::ptr_eq(&doc.cache, &snapshot.cache), "do not detach or clear caller snapshots");
    }
}

#[test]
fn streams_without_length_share_one_endstream_search() {
    let n = 2_000;
    let mut file = b"%PDF-1.7\n".to_vec();
    for k in 1..=n {
        file.extend_from_slice(format!("{k} 0 obj\n<< >>\nstream\nxx\n").as_bytes());
    }
    file.extend_from_slice(b"endstream\nendobj\n");
    file.extend_from_slice(
        format!("{0} 0 obj\n<< /Type /Catalog /Pages {1} 0 R >>\nendobj\n{1} 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n", n + 1, n + 2)
            .as_bytes(),
    );
    file.extend_from_slice(format!("trailer\n<< /Root {} 0 R /Size {} >>\n%%EOF\n", n + 1, n + 3).as_bytes());
    let len = file.len();
    crate::parser::ENDSTREAM_SCAN.with(|c| c.set(0));
    let doc = Document::open(Arc::new(file)).unwrap();
    for k in [1, n / 2, n] {
        assert!(matches!(doc.try_get(k).as_deref(), Ok(Object::Stream(_))), "object {k}");
    }
    let scanned = crate::parser::ENDSTREAM_SCAN.with(|c| c.get());
    assert!(scanned <= 4 * len, "{scanned} bytes searched in a {len}-byte file");
}

#[test]
fn editor_cache_counts_deeply_nested_strings() {
    let mut nested = Object::String(crate::PdfString::literal(vec![b'x'; 1 << 20]));
    for _ in 0..8 {
        nested = Object::Array(vec![nested]);
    }
    let mut source = Document::new_empty();
    let objects: Vec<_> = (0..48).map(|_| Object::Ref(source.add(nested.clone()))).collect();
    source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
    let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
    let doc = Document::open(bytes).unwrap();
    assert_eq!(owned_bytes(&nested, &doc.data, 0), 1 << 20);
    let root = doc.get(doc.root().unwrap());
    for reference in root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap() {
        let Object::Ref(reference) = reference else { panic!("indirect object") };
        drop(doc.get(*reference));
        let cache = doc.cache.lock().unwrap();
        let owned = cache.values().fold(0usize, |n, o| n.saturating_add(owned_bytes(o, &doc.data, 0)));
        let strings = cache.values().filter(|o| o.as_array().is_some()).count();
        assert!(strings << 20 <= Document::EDITOR_CACHE_BYTES + (1 << 20), "{strings} MiB strings cached, {owned} bytes counted");
    }
}

#[test]
fn editor_cache_stays_bounded() {
    let mut source = Document::new_empty();
    let objects: Vec<_> = (0..(Document::EDITOR_CACHE_OBJECTS + 64))
        .map(|i| {
            let mut value = vec![b'x'; 1024];
            value[..4].copy_from_slice(&(i as u32).to_le_bytes());
            Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
        })
        .collect();
    source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
    let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
    let doc = Document::open(bytes).unwrap();
    let root = doc.get(doc.root().unwrap());
    let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap().clone();
    let mut held = None;
    for (i, reference) in references.iter().enumerate() {
        let Object::Ref(reference) = reference else { panic!("indirect object") };
        let object = doc.get(*reference);
        assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
        if i == 0 {
            held = Some(object);
        }
        assert!(doc.cache.lock().unwrap().len() <= Document::EDITOR_CACHE_OBJECTS, "editing must not pin the whole object graph");
        // Eviction keeps the newcomer.
        assert!(doc.cache.lock().unwrap().contains_key(&reference.num), "eviction must keep the newcomer");
    }
    // Returned objects survive eviction, through both handles of the shared cache.
    assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
    let snapshot = doc.clone();
    let first = references[0].as_ref().unwrap();
    assert_eq!(&snapshot.get(first).as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
}

#[test]
fn editor_cache_byte_bound_fires() {
    // 40 × 1 MB strings: far below the count cap, above the 32 MB byte budget. Each
    // newcomer exceeds EDITOR_LARGE_OBJECT, so every insert takes a full accounting pass.
    let mut source = Document::new_empty();
    let objects: Vec<_> = (0..40)
        .map(|i| {
            let mut value = vec![b'y'; 1024 * 1024];
            value[..4].copy_from_slice(&(i as u32).to_le_bytes());
            Object::Ref(source.add(Object::String(crate::PdfString::literal(value))))
        })
        .collect();
    source.update_dict(source.root().unwrap(), |d| d.set(b"Extension".to_vec(), Object::Array(objects))).unwrap();
    let bytes = Arc::new(crate::write_full(&source, &crate::SaveOptions::default()).unwrap());
    let doc = Document::open(bytes).unwrap();
    let root = doc.get(doc.root().unwrap());
    let references = root.as_dict().unwrap().get(b"Extension").unwrap().as_array().unwrap().clone();
    let mut held = None;
    for (i, reference) in references.iter().enumerate() {
        let Object::Ref(reference) = reference else { panic!("indirect object") };
        let object = doc.get(*reference);
        assert_eq!(&object.as_string().unwrap().bytes[..4], &(i as u32).to_le_bytes());
        if i == 0 {
            held = Some(object);
        }
        // Eviction keeps the newcomer.
        assert!(doc.cache.lock().unwrap().contains_key(&reference.num), "eviction must keep the newcomer");
    }
    assert!(doc.cache.lock().unwrap().len() < references.len(), "the byte budget must evict even below the count cap");
    let last = references[references.len() - 1].as_ref().unwrap();
    assert!(doc.cache.lock().unwrap().contains_key(&last.num), "eviction must keep the newcomer");
    assert_eq!(&held.unwrap().as_string().unwrap().bytes[..4], &0u32.to_le_bytes());
}

#[test]
fn a_stream_limited_document_cannot_be_saved() {
    let bytes = Arc::new(crate::write_full(&Document::new_empty(), &crate::SaveOptions::default()).unwrap());
    let doc = Document::open_with_stream_limit(bytes.clone(), None, 1 << 20).unwrap();
    for saved in [crate::write_full(&doc, &crate::SaveOptions::default()), crate::write_incremental(&doc, &crate::SaveOptions::default())] {
        assert!(matches!(saved, Err(CosError::ReadOnlyLimit)), "{saved:?}");
    }
    let doc = Document::open(bytes).unwrap();
    assert!(crate::write_full(&doc, &crate::SaveOptions::default()).is_ok());
}

#[test]
fn inspection_stream_limit_applies_before_loading_compressed_objects() {
    let mut doc = Document::new_empty();
    let large = doc.add(Object::String(crate::PdfString::literal(vec![b'x'; 4096])));
    doc.update_dict(doc.root().unwrap(), |catalog| catalog.set(b"Extension".to_vec(), Object::Ref(large))).unwrap();
    let bytes = Arc::new(crate::write_full(&doc, &crate::SaveOptions::default()).unwrap());
    assert!(Document::open(bytes.clone()).is_ok());
    assert!(Document::open_with_stream_limit(bytes, None, 512).is_err());
}

/// A /Prev chain that loops back on itself stops at the loop (and says so).
#[test]
fn looping_revision_chains_stop() {
    let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    // An update whose /Prev points at itself.
    let at = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 1\n0000000000 65535 f \ntrailer << /Size 3 /Root 1 0 R /Prev {at} >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
    assert!(doc.repair_log().iter().any(|l| l.contains("loops")), "{:?}", doc.repair_log());
}

/// A hybrid-reference file: the classic table leaves object 3 out; the `/XRefStm` stream
/// says it lives in object stream 4.
#[test]
fn hybrid_reference_files_read_the_xref_stream_too() {
    let doc = Document::open(Arc::new(object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0))).unwrap();
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert_compact_matches_materialized(&doc);
    assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
}

/// A hybrid-reference file whose classic table leaves object 3 out, and whose `/XRefStm`
/// stream puts it at position `member` of object stream 4, which holds `header` (the number
/// and offset pairs) and then `members`.
fn object_stream_file(header: &str, members: &str, member: u8) -> Vec<u8> {
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offs = [0usize; 6];
    let mut obj = |out: &mut Vec<u8>, n: usize, body: &[u8]| {
        offs[n] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, 2, b"<< /Type /Pages /Kids [] /Count 0 /Extra 3 0 R >>");
    let count = header.split_whitespace().count() / 2;
    let inner = format!("{header}{members}");
    let mut stm = format!("<< /Type /ObjStm /N {count} /First {} /Length {} >>\nstream\n", header.len(), inner.len()).into_bytes();
    stm.extend_from_slice(inner.as_bytes());
    stm.extend_from_slice(b"\nendstream");
    obj(&mut out, 4, &stm);
    let data = [2u8, 0, 4, member];
    let mut xs = format!("<< /Type /XRef /Size 6 /W [1 2 1] /Index [3 1] /Length {} >>\nstream\n", data.len()).into_bytes();
    xs.extend_from_slice(&data);
    xs.extend_from_slice(b"\nendstream");
    obj(&mut out, 5, &xs);
    let table = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for (n, off) in offs.iter().enumerate().skip(1) {
        let line = if n == 3 { "0000000000 65535 f \n".to_string() } else { format!("{off:010} 00000 n \n") };
        out.extend_from_slice(line.as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size 6 /Root 1 0 R /XRefStm {} >>\nstartxref\n{table}\n%%EOF\n", offs[5]).as_bytes());
    out
}

/// Many revisions naming one hybrid /XRefStm read it once.
#[test]
fn a_hybrid_stream_named_by_many_revisions_is_read_once() {
    let mut out = object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0);
    let stream = find(&out, b"5 0 obj", 0, out.len()).unwrap();
    let mut prev = rfind(&out, b"\nxref\n", 0).unwrap() + 1;
    let revisions = 200;
    for _ in 0..revisions {
        let at = out.len();
        out.extend_from_slice(format!("xref\ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {prev} >>\nstartxref\n{at}\n%%EOF\n").as_bytes());
        prev = at;
    }
    let doc = Document::open(Arc::new(out)).unwrap();
    SECTION_READS.with(|c| c.set(0));
    let (entries, ..) = doc.read_xref_chain_materialized(&mut Vec::new()).unwrap();
    let reads = SECTION_READS.with(|c| c.get());
    assert!(reads <= revisions + 2, "{reads} sections read for {} revisions", revisions + 1);
    assert!(matches!(entries.get(&3), Some(XrefEntry::InStream { stream: 4, .. })), "{:?}", entries.get(&3));
    assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
    assert_compact_matches_materialized(&doc);
}

/// An older section that names an /XRefStm a newer section already read still has its own
/// table compared with that stream, so an entry it sets aside is recorded for it.
#[test]
fn a_repeated_hybrid_stream_records_each_sections_conflicts() {
    let mut out = object_stream_file("3 0 ", "<< /Hidden (yes) >>", 0);
    let stream = find(&out, b"5 0 obj", 0, out.len()).unwrap();
    let catalog = find(&out, b"1 0 obj", 0, out.len()).unwrap();
    let base = rfind(&out, b"\nxref\n", 0).unwrap() + 1;
    // This section's table places object 3 at the catalog; the stream puts it in object stream 4.
    let older = out.len();
    out.extend_from_slice(
        format!("xref\n3 1\n{catalog:010} 00000 n \ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {base} >>\nstartxref\n{older}\n%%EOF\n")
            .as_bytes(),
    );
    let newer = out.len();
    out.extend_from_slice(format!("xref\ntrailer << /Size 6 /Root 1 0 R /XRefStm {stream} /Prev {older} >>\nstartxref\n{newer}\n%%EOF\n").as_bytes());
    let doc = Document::open(Arc::new(out)).unwrap();
    assert_eq!(doc.xref_oddities(), [(older as u64, 3)]);
    assert_eq!(text_of(&doc, 3, b"Hidden").as_deref(), Some("yes"));
}

fn text_of(doc: &Document, num: u32, key: &[u8]) -> Option<String> {
    doc.get(ObjRef::new(num, 0)).as_dict().and_then(|d| d.get(key).and_then(|v| v.as_string().map(|s| s.to_text())))
}

/// An object stream member numbered 2^32 + 3 is not object 3 (#1815): the number used to be
/// cast to `u32`, which dropped the high bits.
#[test]
fn object_stream_numbers_beyond_u32_are_not_truncated() {
    let control = Document::open(Arc::new(object_stream_file("3 0 ", "<< /Member (yes) >>", 0))).unwrap();
    assert_eq!(text_of(&control, 3, b"Member").as_deref(), Some("yes"));
    let doc = Document::open(Arc::new(object_stream_file("4294967299 0 ", "<< /Member (yes) >>", 0))).unwrap();
    assert!(matches!(*doc.get(ObjRef::new(3, 0)), Object::Null), "{:?}", doc.get(ObjRef::new(3, 0)));
    assert!(!doc.xref_entry_is_exact(3));
}

/// Member starts are checked rather than cast, so they cannot wrap on any target (#1815).
#[test]
fn object_stream_member_starts_are_checked() {
    assert_eq!(objstm_member(3, 4, Some(10), 20), Some((3, 14)));
    assert_eq!(objstm_member(3, 9, Some(10), 20), Some((3, 19)), "the last byte");
    assert_eq!(objstm_member(3, 10, Some(10), 20), None, "at the end of the data");
    assert_eq!(objstm_member(3, 1, Some(usize::MAX), 20), None, "/First plus the offset overflows");
    assert_eq!(objstm_member(3, i64::MAX, Some(10), 20), None);
    assert_eq!(objstm_member(3, -1, Some(10), 20), None);
    assert_eq!(objstm_member(3, 0, None, 20), None, "/First does not fit in usize");
    assert_eq!(objstm_member(i64::from(u32::MAX), 0, Some(0), 1), Some((u32::MAX, 0)));
    assert_eq!(objstm_member(i64::from(u32::MAX) + 1, 0, Some(0), 1), None);
    assert_eq!(objstm_member(-1, 0, Some(0), 1), None);
}

/// A member at a negative offset is unusable, not the stream's first member (#1815): the
/// offset used to be clamped to zero, which read the object at `/First`.
#[test]
fn object_stream_negative_offsets_are_refused() {
    let doc = Document::open(Arc::new(object_stream_file("7 0 3 -1 ", "<< /First (yes) >>", 1))).unwrap();
    assert!(matches!(*doc.get(ObjRef::new(3, 0)), Object::Null), "{:?}", doc.get(ObjRef::new(3, 0)));
    let control = Document::open(Arc::new(object_stream_file("7 0 3 19 ", "<< /First (yes) >> << /Second (yes) >>", 1))).unwrap();
    assert_eq!(text_of(&control, 3, b"Second").as_deref(), Some("yes"));
}

#[test]
fn revision_ends_split_incremental_updates() {
    let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    let first = bytes.len();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    assert_eq!(doc.revision_ends(), vec![first]);
    let mut edited = doc.clone();
    edited.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(crate::PdfString::literal("en")))).unwrap();
    bytes = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let ends = doc.revision_ends();
    assert_eq!(ends.len(), 2, "{ends:?}");
    assert!(ends[0] >= first && ends[0] < bytes.len() && ends[1] == bytes.len(), "{ends:?} of {}", bytes.len());
    // The first revision opens on its own, without the update.
    let old = Document::open(Arc::new(bytes[..ends[0]].to_vec())).unwrap();
    assert!(old.get(ObjRef::new(1, 0)).as_dict().unwrap().get(b"Lang").is_none());
}

/// qpdf's `xref-compressed-in-compressed.pdf` in miniature: object 3 is an object stream
/// holding object 4, and the cross-reference stream says object 3 itself lives in stream 99.
fn compressed_in_compressed_fixture() -> Document {
    let mut out = b"%PDF-1.5\n".to_vec();
    let at = |out: &mut Vec<u8>, num: u32, body: &str| {
        let off = out.len();
        out.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
        off as u16
    };
    let o1 = at(&mut out, 1, "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>");
    let o2 = at(&mut out, 2, "<< /Type /Pages /Kids [] /Count 0 >>");
    let inner = "<< /Type /Outlines /Count 0 >>";
    at(&mut out, 3, &format!("<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n4 0 {inner}\nendstream", 4 + inner.len()));
    // /W [1 2 1]: object 3 is said to live in stream 99 (which does not exist), 4 in stream 3.
    let rows: Vec<[u8; 4]> =
        vec![[0, 0, 0, 255], [1, 0, o1 as u8, 0], [1, 0, o2 as u8, 0], [2, 0, 99, 0], [2, 0, 3, 0], [1, (out.len() >> 8) as u8, out.len() as u8, 0]];
    let rows: Vec<u8> = rows.concat();
    let xref = out.len();
    out.extend_from_slice(
        format!("5 0 obj\n<< /Type /XRef /W [1 2 1] /Index [0 6] /Size 6 /Root 1 0 R /Length {} >>\nstream\n", rows.len()).as_bytes(),
    );
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// An object stream whose own cross-reference entry says it is compressed inside another
/// stream (which the format forbids, and which does not exist here: qpdf's
/// `xref-compressed-in-compressed.pdf`, #2367) could not be loaded, so every object in it
/// read as null and a full save wrote `/Outlines null`. The stream is now found by its own
/// header, as a plain object with a wrong offset is.
#[test]
fn an_object_stream_marked_as_compressed_is_found_by_its_header() {
    let doc = compressed_in_compressed_fixture();
    assert_eq!(doc.revisions().len(), 1, "the chain itself reads: {:?}", doc.repair_log());
    let outlines = doc.get(ObjRef::new(4, 0));
    assert_eq!(outlines.as_dict().and_then(|d| d.name(b"Type")), Some(&b"Outlines"[..]), "{outlines:?}");
    let saved = crate::write_full(&doc, &crate::SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved)).unwrap();
    let catalog = back.get(back.root().unwrap());
    let outlines = back.resolve(catalog.as_dict().unwrap().get(b"Outlines").unwrap());
    assert_eq!(outlines.as_dict().and_then(|d| d.name(b"Type")), Some(&b"Outlines"[..]), "the full save keeps it: {outlines:?}");
}

/// The header fallback is for file entries only: once the stream is freed (or replaced) in
/// the overlay, its objects must not come back from the file's copy of it.
#[test]
fn a_freed_object_stream_is_not_found_by_its_header() {
    let doc = compressed_in_compressed_fixture();
    assert!(doc.get(ObjRef::new(4, 0)).as_dict().is_some());
    let mut edited = doc.clone();
    edited.free(ObjRef::new(3, 0));
    let fresh = edited.object_reader().document;
    assert!(matches!(*fresh.get(ObjRef::new(4, 0)), Object::Null), "{:?}", fresh.get(ObjRef::new(4, 0)));
    let mut replaced = doc.clone();
    replaced.set(ObjRef::new(3, 0), Object::Int(0));
    let fresh = replaced.object_reader().document;
    assert!(matches!(*fresh.get(ObjRef::new(4, 0)), Object::Null), "{:?}", fresh.get(ObjRef::new(4, 0)));
}

/// The same once the free is saved: the revision's table says the stream is free, and
/// neither reading the file back nor rewriting it in full brings its objects back.
#[test]
fn a_container_freed_in_a_saved_revision_is_not_found_by_its_header() {
    let mut edited = compressed_in_compressed_fixture();
    edited.free(ObjRef::new(3, 0));
    let incremental = crate::write_incremental(&edited, &crate::SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(incremental)).unwrap();
    assert_eq!(reopened.revisions().len(), 2);
    assert!(matches!(reopened.xref_entry(3), Some(XrefEntry::Free { .. })));
    assert!(matches!(*reopened.get(ObjRef::new(4, 0)), Object::Null), "{:?}", reopened.get(ObjRef::new(4, 0)));
    for object_streams in [true, false] {
        let opts = crate::SaveOptions { object_streams, ..Default::default() };
        let full = crate::write_full(&reopened, &opts).unwrap();
        let back = Document::open(Arc::new(full)).unwrap();
        let catalog = back.get(back.root().unwrap());
        let outlines = catalog.as_dict().unwrap().get(b"Outlines").map(|o| back.resolve(o));
        assert!(outlines.as_ref().is_none_or(|o| matches!(**o, Object::Null)), "the full save keeps it out: {outlines:?}");
    }
}

/// A trailer whose /Prev is written as the reference `N 0 R` (tiff2pdf did this; the files
/// are JHOVE's PDF-HUL-157 cases, #2366) used to end the cross-reference chain silently, so
/// every object of the earlier sections read as null: a four-page file showed one page, and
/// a full save wrote the three missing kids as null. The offset is read from the reference.
#[test]
fn a_prev_written_as_a_reference_still_reaches_the_earlier_sections() {
    let base =
        build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"], "/Root 1 0 R");
    let prev = String::from_utf8_lossy(&base).rfind("xref\n0 4").unwrap();
    let update = |prev_entry: &str| {
        let mut bytes = base.clone();
        let at = bytes.len();
        bytes.extend_from_slice(b"4 0 obj\n<< /Title (Update) >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n4 1\n{at:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R /Info 4 0 R /Prev {prev_entry} >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        Document::open(Arc::new(bytes)).unwrap()
    };
    let good = update(&prev.to_string());
    let bad = update(&format!("{prev} 0 R"));
    assert!(good.repair_log().is_empty(), "{:?}", good.repair_log());
    assert_eq!(bad.revisions().len(), 2, "{:?}", bad.repair_log());
    assert_eq!(bad.repair_log().len(), 1, "a note about the /Prev: {:?}", bad.repair_log());
    assert_eq!(bad.xref_entry(3), good.xref_entry(3), "the page of the first section is reached");
    assert_eq!(bad.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(&b"Page"[..]));
    assert_eq!(
        bad.get(ObjRef::new(4, 0)).as_dict().and_then(|d| d.get(b"Title").and_then(|t| t.as_string().map(|s| s.to_text()))),
        Some("Update".into())
    );
}

/// iText's `indirectPrev.pdf` in miniature: the update's trailer gives `/Prev 4 0 R`, and
/// object 4, listed in that update, holds the offset of the first section. Read as offset 4
/// (inside the header), the chain broke and the file was rebuilt without its revisions.
#[test]
fn a_prev_reference_to_an_object_holding_the_offset_reads_that_offset() {
    let mut bytes =
        build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R >>"], "/Root 1 0 R");
    let prev = String::from_utf8_lossy(&bytes).rfind("xref\n0 4").unwrap();
    let mut at = Vec::new();
    for (num, body) in
        [(2, "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string()), (4, prev.to_string()), (5, "<< /Type /Page /Parent 2 0 R >>".into())]
    {
        at.push(bytes.len());
        bytes.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref\n2 1\n{:010} 00000 n \n4 2\n{:010} 00000 n \n{:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev 4 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            at[0], at[1], at[2]
        )
        .as_bytes(),
    );
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.revisions().len(), 2, "{:?}", doc.repair_log());
    assert_eq!(doc.repair_log().len(), 1, "a note about the /Prev, and no rebuild: {:?}", doc.repair_log());
    assert!(doc.repair_log()[0].contains(&format!("the offset {prev}")), "{:?}", doc.repair_log());
    assert_eq!(doc.get(ObjRef::new(3, 0)).as_dict().and_then(|d| d.name(b"Type")), Some(&b"Page"[..]));
    assert_compact_matches_materialized(&doc);
}

/// A file whose only trailers are cross-reference stream dictionaries, with its `startxref`
/// out of reach (here a second file appended after it, cut off before its own
/// cross-reference data, as in #2365), is rebuilt from its object headers. The rebuild used
/// to look only for the `trailer` keyword, so /Info and /ID were lost, and a full save after
/// editing one Info field dropped every other.
#[test]
fn reconstruction_takes_the_trailer_from_a_cross_reference_stream() {
    let table = build(
        &["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Title (Kept) /Creator (Me) >>"],
        "/Root 1 0 R /Info 3 0 R",
    );
    let packed = crate::write_full(&Document::open(Arc::new(table)).unwrap(), &crate::SaveOptions::default()).unwrap();
    let text = String::from_utf8_lossy(&packed);
    assert!(text.contains("/XRef") && !text.contains("trailer"), "a cross-reference stream is the only trailer");
    let mut bytes = packed.clone();
    let appended = format!("%PDF-1.3\n90 0 obj\n<< /Length 5000 >>\nstream\n{}\nendstream\nendobj\n", "x".repeat(5000));
    bytes.extend_from_slice(appended.as_bytes());
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.revisions().is_empty(), "rebuilt: {:?}", doc.repair_log());
    let info = doc.trailer().reference(b"Info").expect("the cross-reference stream's /Info");
    let field = |d: &Document, key: &[u8]| d.get(info).as_dict().unwrap().get(key).unwrap().as_string().unwrap().bytes.clone();
    assert_eq!(field(&doc, b"Title"), b"Kept");
    assert!(doc.trailer().get(b"ID").is_some(), "and its /ID");
    let mut edited = doc.clone();
    edited.update_dict(info, |d| d.set(b"Subject".to_vec(), Object::String(crate::PdfString::literal("new")))).unwrap();
    let saved = crate::write_full(&edited, &crate::SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved)).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    let info = back.trailer().reference(b"Info").expect("the full save keeps /Info");
    let field = |key: &[u8]| back.get(info).as_dict().unwrap().get(key).unwrap().as_string().unwrap().bytes.clone();
    assert_eq!((field(b"Title"), field(b"Creator"), field(b"Subject")), (b"Kept".to_vec(), b"Me".to_vec(), b"new".to_vec()));
}

/// A rebuild prefers a trailer whose /Root is the catalog (as `root_is_catalog` judges it): a
/// later cross-reference stream whose /Root points at the Info dictionary does not replace an
/// earlier good trailer, or a full save would write the Info dictionary as /Root.
#[test]
fn reconstruction_skips_a_trailer_whose_root_is_not_the_catalog() {
    let mut bytes =
        build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "<< /Title (Kept) >>"], "/Root 1 0 R /Info 3 0 R");
    bytes.extend_from_slice(b"4 0 obj\n<< /Type /XRef /Size 5 /W [1 1 1] /Root 3 0 R /Length 3 >>\nstream\nabc\nendstream\nendobj\n");
    let appended = format!("%PDF-1.3\n90 0 obj\n<< /Length 5000 >>\nstream\n{}\nendstream\nendobj\n", "x".repeat(5000));
    bytes.extend_from_slice(appended.as_bytes());
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.repair_log().iter().any(|l| l.contains("rebuilt")), "{:?}", doc.repair_log());
    assert_eq!(doc.trailer().reference(b"Root").map(|r| r.num), Some(1), "the catalog, not the Info dictionary");
    assert_eq!(doc.trailer().reference(b"Info").map(|r| r.num), Some(3));
}

#[test]
fn damaged_xref_is_reconstructed() {
    let mut bytes = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "/Root 1 0 R");
    let n = bytes.len();
    bytes[n - 12..n - 6].copy_from_slice(b"999999"); // point startxref into nowhere
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.revisions().is_empty());
    assert!(!doc.repair_log().is_empty());
    assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
}

#[test]
fn a_decompression_bomb_in_an_xref_stream_is_refused_and_the_file_reconstructed() {
    // 32 MiB of rows behind two FlateDecode filters (a few hundred bytes in the file) for
    // /W [1 1 1]: at 3 bytes a row that is more than 8,388,607 objects' worth, so it is not
    // decoded (in full: the limit applies while inflating) and the objects are found by
    // reconstruction instead.
    let bomb = pdfcraft_filters::encode_flate(&pdfcraft_filters::encode_flate(&vec![0u8; 32 << 20]));
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let o1 = bytes.len();
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let o2 = bytes.len();
    bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let x = bytes.len();
    bytes.extend_from_slice(
        format!("3 0 obj\n<< /Type /XRef /Size 4 /W [1 1 1] /Root 1 0 R /Filter [/FlateDecode /FlateDecode] /Length {} >>\nstream\n", bomb.len())
            .as_bytes(),
    );
    bytes.extend_from_slice(&bomb);
    bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{x}\n%%EOF\n").as_bytes());
    assert!(o1 < o2 && o2 < x);
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(doc.root(), Some(ObjRef::new(1, 0)));
    assert!(doc.repair_log().iter().any(|l| l.contains("limit")), "{:?}", doc.repair_log());
}

#[test]
fn a_stream_whose_length_is_itself_and_never_ends_loads_quickly() {
    // From the nightly fuzz job: `6 0 obj << /Length 6 0 R >> stream` with no `endstream`
    // after it. Every parse attempt failed, and each attempt resolved the length by loading
    // object 6 again, retrying the same ways down to the nesting limit: about 2^32 parses.
    // Objects being loaded are now a seen-set, lengths are memoised, and the second parse
    // only runs when the header is shifted.
    let mut bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
    bytes.extend_from_slice(b"6 0 obj\n<< /Length 6 0 R >>\nstream\nno end marker follows\n");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(doc) = Document::open(Arc::new(bytes)) {
            let _ = doc.try_get(6);
        }
        let _ = tx.send(());
    });
    rx.recv_timeout(std::time::Duration::from_secs(20)).expect("loading must not retry exponentially");
}
