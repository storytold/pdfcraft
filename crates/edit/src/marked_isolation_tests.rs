use super::*;

fn fixture(parts: &[&[u8]]) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let streams = parts.iter().map(|data| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), data.to_vec()))))).collect();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    page.set(b"Contents".to_vec(), Object::Array(streams));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

fn placed(parts: &[&[u8]]) -> (Document, Vec<Object>) {
    let mut doc = fixture(parts);
    stamp(&mut doc, 0, "Probe", b"q Q\n".to_vec()).unwrap();
    let page = page_list(&doc).remove(0);
    let list = contents(&mut doc, &page).unwrap();
    (doc, list)
}

fn close(parts: &[&[u8]]) -> Vec<u8> {
    let (doc, list) = placed(parts);
    let object = &list[list.len() - 2];
    assert_eq!(stream_tag(&doc, object).as_deref(), Some("Wrap"));
    let bytes = stream_bytes(&doc, object).unwrap();
    assert_eq!(wrapper_role(&doc, object), Some(false));
    bytes
}

#[test]
fn closes_only_marked_operators_not_strings_names_comments_or_image_samples() {
    for tail in
        [b"/Span << /Text (BMC BDC EMC) >> DP".as_slice(), b"/BMC /EMC DP", b"% BMC BDC EMC", b"BI /W 11 /H 1 /BPC 8 /CS /G ID BMC BDC EMC EI\n"]
    {
        let mut input = b"/OC /L1 BDC\n".to_vec();
        input.extend_from_slice(tail);
        assert_eq!(close(&[&input]), b"\nEMC\nQ %PdfCraft\n", "{tail:?}");
    }
}

#[test]
fn safe_suffix_stays_outside_marked_content_across_balanced_graphics() {
    let (doc, list) = placed(&[b"q /OC /L1 BDC Q", b"50 Tz"]);
    assert_eq!(stream_bytes(&doc, &list[0]).unwrap(), WRAP_OPEN);
    assert_eq!(stream_bytes(&doc, &list[1]).unwrap(), b"q /OC /L1 BDC Q");
    assert_eq!(stream_bytes(&doc, &list[list.len() - 2]).unwrap(), b"\nEMC\nQ %PdfCraft\n");
    assert_eq!(close(&[b"q /OC /L1 BDC Q"]), b"\nEMC\nQ %PdfCraft\n");
}

#[test]
fn known_marked_depth_survives_unknown_graphics_and_tolerant_operands() {
    for (input, expected) in [
        (b"/OC /L1 BDC q".as_slice(), b"\nEMC\nQ\nQ %PdfCraft\n".as_slice()),
        (b"Q /OC /L1 BDC", b"\nEMC\nQ %PdfCraft\n"),
        (b"/OC /L1 BDC BT", b"\nEMC\nQ %PdfCraft\n"),
        (b"/OC /L1 BDC 0 0 m", b"\nEMC\nQ %PdfCraft\n"),
        (b"/OC /L1 BDC 123 BMC /Artifact BDC", b"\nEMC\nQ %PdfCraft\n"),
        (b"EMC 123 /OC /L1 BDC 123 /Artifact BMC", b"\nEMC\nEMC\nQ %PdfCraft\n"),
        (b"/OC /L1 BDC 123 EMC", b"\nQ %PdfCraft\n"),
        (b"/Span 123 BDC", b"\nEMC\nQ %PdfCraft\n"),
    ] {
        assert_eq!(close(&[input]), expected, "{input:?}");
    }
}

#[test]
fn closes_cross_stream_begins_without_closing_balanced_sequences_twice() {
    assert_eq!(close(&[b"/OC", b"/L1 BDC"]), b"\nEMC\nQ %PdfCraft\n");
    assert_eq!(close(&[b"/OC /L1 BDC", b"EMC"]), WRAP_CLOSE);
    assert_eq!(close(&[b"/OC /L1 BDC % EMC", b"EMC\n"]), b"\nEMC\nQ %PdfCraft\n");
}

#[test]
fn recognizes_only_canonical_new_closes_and_retains_legacy_closes() {
    for bytes in [WRAP_CLOSE, OLD_WRAP_CLOSE, b"\nQ\nQ %PdfCraft\n", b"\nEMC\nQ %PdfCraft\n", b"\nEMC\nEMC\nQ\nQ %PdfCraft\n"] {
        assert!(is_wrap_close(bytes), "{bytes:?}");
    }
    for bytes in [
        b"\nEMCQ %PdfCraft\n".as_slice(),
        b"\nEMC\nQQ %PdfCraft\n",
        b"\nQ\nEMC\nQ %PdfCraft\n",
        b"EMC\nQ %PdfCraft\n",
        b"\nEMC 1 w\nQ %PdfCraft\n",
        b"\nEMC\nQ %PdfCraft\nq Q",
    ] {
        assert!(!is_wrap_close(bytes), "{bytes:?}");
    }
}

#[test]
fn depth_budget_bounds_emitted_closures_and_overflow_stays_unknown() {
    let maximum = b"/Artifact BMC\n".repeat(ISOLATION_MAX_DEPTH);
    let mut expected = b"\nEMC".repeat(ISOLATION_MAX_DEPTH);
    expected.extend_from_slice(WRAP_CLOSE);
    assert_eq!(close(&[&maximum]), expected);
    let mut combined = maximum.clone();
    combined.extend_from_slice(&b"q\n".repeat(ISOLATION_MAX_DEPTH));
    let mut expected = b"\nEMC".repeat(ISOLATION_MAX_DEPTH);
    expected.extend_from_slice(&b"\nQ".repeat(ISOLATION_MAX_DEPTH));
    expected.extend_from_slice(WRAP_CLOSE);
    assert_eq!(close(&[&combined]), expected);
    assert!(expected.len() < 64 * 1024, "maximum generated close remains recognizable for removal");
    let mut doc = fixture(&[&combined]);
    let page = page_list(&doc).remove(0);
    let original = contents(&mut doc, &page).unwrap();
    place_tagged(&mut doc, &page, MarkKind::HeaderFooter.tag(), b"q Q\n".to_vec(), false).unwrap();
    assert_eq!(remove_marks(&mut doc, &[0], MarkKind::HeaderFooter).unwrap(), 1);
    assert_eq!(contents(&mut doc, &page).unwrap(), original, "maximum combined close can be removed without nesting wrappers");
    for suffix in [b"/Artifact BMC\n".as_slice(), b"/Artifact BMC\nEMC\n", b"/Artifact BMC\nEMC\nEMC\n"] {
        let mut over = maximum.clone();
        over.extend_from_slice(suffix);
        assert_eq!(close(&[&over]), WRAP_CLOSE, "overflow cannot resume speculative depth tracking after EMC");
    }
}
