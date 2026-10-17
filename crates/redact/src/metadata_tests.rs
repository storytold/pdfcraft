//! Metadata comparison regressions for PR #2598 comments 6213443550 and 6216325944.
use super::*;

fn packet(payload: &str, attributes: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description rdf:about='' xmlns:z='https://example.org/ns/' {attributes}>{payload}</rdf:Description></rdf:RDF></x:xmpmeta>"
    )
}

fn collected(src: &[u8]) -> Vec<String> {
    let mut doc = one_page(src, "", vec![]);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let (_, data) = page_streams(&doc, &page.dict, 0).unwrap();
    let res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut rep = Report::default();
    let rects = [[-1_000_000.0, -1_000_000.0, 1_000_000.0, 1_000_000.0]];
    let mut scope = Scope::new(&rects, Mode::Apply, &mut rep);
    process(&mut doc, &mut scope, &data, &res, pdfcraft_content::Matrix::IDENTITY);
    scope.flush_run();
    scope.removed_fragments
}

fn redact_all(src: &[u8], key: &str, value: &str, packet: Option<&str>) -> Document {
    let mut doc = meta_doc(src, &[(key, value), ("Subject", "Spy stories")], packet, None);
    let rects = [[0.0, 0.0, 300.0, 300.0]];
    assert!(under(&mut doc, 0, &rects) > 0, "fixture has marked glyphs");
    mark(&mut doc, 0, &rects, "");
    apply(&mut doc, None).unwrap();
    assert_eq!(under(&mut doc, 0, &rects), 0, "page content was removed");
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(info_value(&saved, "Subject").as_deref(), Some("Spy stories"));
    saved
}

#[test]
fn tj_word_gaps_and_literal_spaces_clear_the_same_name() {
    for (label, src, fragment, survives) in [
        ("TJ -333", "BT /F1 10 Tf 10 100 Td [(James)-333(Bond)]TJ ET", "JamesBond", false),
        ("mixed kerns", "BT /F1 10 Tf 10 100 Td [(J)-200(ames)-333(Bond)]TJ ET", "JamesBond", false),
        ("rotated", "BT /F1 10 Tf 0 1 -1 0 100 100 Tm [(James)-333(Bond)]TJ ET", "JamesBond", false),
        ("literal space", "BT /F1 10 Tf 10 100 Td (James Bond)Tj ET", "James Bond", false),
    ] {
        assert_eq!(collected(src.as_bytes()), [fragment], "{label}");
        let packet = packet("<z:name>James Bond</z:name>", "");
        let saved = redact_all(src.as_bytes(), "Author", "James Bond", Some(&packet));
        let actual = info_value(&saved, "Author");
        assert_eq!(actual.as_deref(), survives.then_some("James Bond"), "the removed name must not survive");
        assert_eq!(xmp_text(&saved).is_some(), survives);
    }
}

#[test]
fn metadata_whitespace_is_normalized_without_broadening_name_matching() {
    for (label, text, key, value) in [
        ("partial phrase", "JAMES BOND IS A SPY", "Title", "Case: James Bond"),
        ("partial first name", "JAMES BOND IS A SPY", "Title", "Case: James"),
        ("reordered", "John Smith", "Author", "Smith, John"),
        ("double space metadata", "James Bond", "Author", "James  Bond"),
        ("tab metadata", "James Bond", "Author", "James\tBond"),
        ("newline metadata", "James Bond", "Author", "James\nBond"),
        ("double space page", "James  Bond", "Author", "James Bond"),
    ] {
        let src = format!("BT /F1 10 Tf 10 100 Td ({text})Tj ET");
        let packet = packet(&format!("<z:value>{value}</z:value>"), "");
        let saved = redact_all(src.as_bytes(), key, value, Some(&packet));
        let actual = info_value(&saved, key);
        let unsupported = label.starts_with("partial") || label == "reordered";
        assert_eq!(actual.as_deref(), unsupported.then_some(value), "whitespace normalizes; partial and reordered names remain unsupported");
        assert_eq!(xmp_text(&saved).as_deref(), unsupported.then_some(packet.as_str()));
    }
}

#[test]
fn xmp_matches_values_and_preserves_structure_only_matches() {
    for (label, text, payload, attributes, removed) in [
        ("tag name", "DC", "<z:dc>Harmless</z:dc>", "", false),
        ("attribute name", "ID", "<z:item>Harmless</z:item>", "z:id='safe'", false),
        ("short language value", "AGENT", "<z:lang>en</z:lang>", "", true),
        ("short version value", "RELEASE 1.7 APPROVED", "<z:version>1.7</z:version>", "", true),
        ("unrelated control", "ZX", "<z:item>Harmless</z:item>", "z:id='safe'", false),
    ] {
        let packet = packet(payload, attributes);
        let src = format!("BT /F1 10 Tf 10 100 Td ({text})Tj ET");
        if label.starts_with("short") {
            assert!(!packet.to_lowercase().contains(&text.to_lowercase()), "the whole removed run is absent");
        }
        let saved = redact_all(src.as_bytes(), "Author", "Ada", Some(&packet));
        let actual = xmp_text(&saved);
        assert_eq!(actual.is_none(), removed, "{label}");
        assert_eq!(info_value(&saved, "Author").as_deref(), Some("Ada"));
    }
}

#[test]
fn xmp_values_are_decoded_and_markup_is_not_a_value() {
    for (payload, attributes, removed) in [
        ("<z:name>&#74;ames&#32;Bond</z:name>", "", true),
        ("<z:name><![CDATA[James Bond]]></z:name>", "", true),
        ("<z:clean>Harmless</z:clean>", "z:name = 'James  Bond'", true),
        ("<!--James Bond--><z:clean>Harmless</z:clean>", "", true),
        ("<z:JamesBond/>", "", false),
    ] {
        let packet = packet(payload, attributes);
        let text = if removed { "James Bond" } else { "JamesBond" };
        let src = format!("BT /F1 10 Tf 10 100 Td ({text})Tj ET");
        let saved = redact_all(src.as_bytes(), "Author", "Ada", Some(&packet));
        assert_eq!(xmp_text(&saved).is_none(), removed);
    }
    let packet = packet("<z:clean>Harmless</z:clean>", "xmlns:dc='https://example.org/dc/' z:id = 'safe > tail'");
    for text in ["DC", "ID"] {
        let src = format!("BT /F1 10 Tf 10 100 Td ({text})Tj ET");
        assert!(xmp_text(&redact_all(src.as_bytes(), "Author", "Ada", Some(&packet))).is_some());
    }
    for broken in ["<broken>", "<!DOCTYPE x [<!ENTITY secret 'James Bond'>]><x>&secret;</x>"] {
        assert!(xmp_text(&redact_all(b"BT /F1 10 Tf 10 100 Td (James Bond)Tj ET", "Author", "Ada", Some(broken))).is_none());
    }
}

#[test]
fn optional_space_offsets_survive_unicode_lowercase_expansion() {
    let pattern = KernedFragment { text: "İB".into(), gaps: vec![2] }.normalized();
    assert!(pattern.matches(&normalize("İ B")));
    assert!(pattern.matches(&normalize("İB")));
    assert!(!pattern.matches(&normalize("B İ")));
    assert!(pattern.matches(&normalize("Case: İ B, unrelated")));
}

#[test]
fn optional_tj_spaces_preserve_literal_boundaries_and_complete_value_matching() {
    let src = b"BT /F1 10 Tf 10 100 Td [(J)-200(ames)-333(Bond) ( IS A SPY)]TJ ET";
    for (value, removed) in [("James Bond", true), ("JamesBond", true), ("Case: James Bond", false), ("Spy stories", false)] {
        let saved = redact_all(src, "Title", value, None);
        assert_eq!(info_value(&saved, "Title").is_none(), removed, "{value}");
    }
    // Literal spaces must never become optional merely because the run also contains a kern.
    let saved = redact_all(b"BT /F1 10 Tf 10 100 Td [(James Bond)-200(X)]TJ ET", "Author", "JamesBondX", None);
    assert_eq!(info_value(&saved, "Author").as_deref(), Some("JamesBondX"));
    for kern in [0, 333] {
        let src = format!("BT /F1 10 Tf 10 100 Td [(James){kern}(Bond)]TJ ET");
        let saved = redact_all(src.as_bytes(), "Author", "James Bond", None);
        assert_eq!(info_value(&saved, "Author").as_deref(), Some("James Bond"), "zero/backward kern {kern} is not a word gap");
    }
}

#[test]
fn processing_instruction_data_is_metadata_but_its_target_is_not() {
    for (payload, removed) in [
        ("<?audit James Bond?><z:clean>Harmless</z:clean>", true),
        ("<?JamesBond harmless?><z:clean>Harmless</z:clean>", false),
        ("<?audit?><z:clean>Harmless</z:clean>", false),
    ] {
        let packet = packet(payload, "");
        let src = if removed { b"BT /F1 10 Tf 10 100 Td (James Bond)Tj ET".as_slice() } else { b"BT /F1 10 Tf 10 100 Td (JamesBond)Tj ET" };
        let saved = redact_all(src, "Author", "Ada", Some(&packet));
        assert_eq!(xmp_text(&saved).is_none(), removed, "{payload}");
    }
    let wrapped =
        format!("<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>{}<?xpacket end='w'?>", packet("<z:clean>Harmless</z:clean>", ""));
    for word in ["begin", "end", "W5M0MpCehiHzreSzNTczkc9d"] {
        let src = format!("BT /F1 10 Tf 10 100 Td ({word})Tj ET");
        let saved = redact_all(src.as_bytes(), "Author", "Ada", Some(&wrapped));
        assert_eq!(xmp_text(&saved).as_deref(), Some(wrapped.as_str()), "xpacket wrapper must survive redacting '{word}'");
    }
}

#[test]
fn excessive_removed_text_clears_metadata_instead_of_truncating_needles() {
    // The explicit character spacing keeps every synthetic glyph at the same origin.
    // Each repositioning separates runs without making a huge page or a large fixture file.
    for (label, run_len, runs) in [("run length", 16_385, 1), ("run count", 2, 1_025), ("total bytes", 16_384, 17)] {
        let mut src = String::from("BT /F1 10 Tf -5 Tc ");
        let run = "A".repeat(run_len);
        for i in 0..runs {
            let x = if i % 2 == 0 { 10 } else { 200 };
            src.push_str(&format!("1 0 0 1 {x} 100 Tm ({run})Tj "));
        }
        src.push_str("ET");
        let clean = packet("<z:clean>Harmless</z:clean>", "");
        let mut doc = meta_doc(src.as_bytes(), &[("Author", "Ada"), ("Subject", "Spy stories")], Some(&clean), Some(&clean));
        let rects = [[0.0, 0.0, 300.0, 300.0]];
        mark(&mut doc, 0, &rects, "");
        let report = apply(&mut doc, None).unwrap();
        assert!(report.metadata_cleared_on_limit, "{label}");
        assert_eq!(report.glyphs, run_len * runs, "{label}");
        let saved = reopen(&doc);
        assert_eq!(info_value(&saved, "Author"), None, "{label}: incomplete evidence must clear all metadata");
        assert_eq!(info_value(&saved, "Subject"), None, "{label}");
        assert!(xmp_text(&saved).is_none(), "{label}");
        assert!(pdfcraft_model::pages(&saved)[0].dict.get(b"Metadata").is_none(), "{label}");
        assert_eq!((report.info_entries_cleared, report.xmp_removed), (2, 2), "{label}");
        assert_no_secret(&doc, b"Ada");
        assert_no_secret(&doc, b"Spy stories");
        assert_no_secret(&doc, b"Harmless");
    }
}

#[test]
fn repetitive_optional_gap_matching_has_a_shared_fail_closed_work_limit() {
    let run = format!("{}b", "a".repeat(4_096));
    let value = format!("{}c", "a".repeat(4_096));
    let clean = packet("<z:clean>Harmless</z:clean>", "");
    let mut doc = meta_doc(b"", &[("Title", &value), ("Author", "Ada")], Some(&clean), Some(&clean));
    let mut report = Report::default();
    let pattern = KernedFragment { text: run.clone(), gaps: vec![1] };
    clear_marked_metadata(&mut doc, &[run], &[pattern], &mut report).unwrap();
    assert!(report.metadata_cleared_on_limit);
    let saved = reopen(&doc);
    assert_eq!(info_value(&saved, "Title"), None, "exhaustion must not be treated as no match");
    assert_eq!(info_value(&saved, "Author"), None, "the exhausted budget applies to the whole operation");
    assert!(xmp_text(&saved).is_none());
    assert!(pdfcraft_model::pages(&saved)[0].dict.get(b"Metadata").is_none());
}

#[test]
fn compressed_metadata_is_visible_to_the_secret_scan() {
    let mut doc = meta_doc(b"", &[("Title", "HIDDEN-COMPRESSED-TITLE")], None, None);
    let r = doc.trailer().reference(b"Info").unwrap();
    doc.update_dict(r, |d| {
        d.set(b"Subject".to_vec(), Object::String(pdfcraft_cos::PdfString { bytes: vec![0xFE, 0xFF, 0, b'S', 0, b'E', 0, b'C'], hex: false }));
    })
    .unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(b"HIDDEN-COMPRESSED-TITLE".len()).any(|w| w == b"HIDDEN-COMPRESSED-TITLE"), "the control really is compressed");
    assert!(secret_is_present(&doc, b"HIDDEN-COMPRESSED-TITLE"));
    assert!(secret_is_present(&doc, &[0, b'S', 0, b'E', 0, b'C']), "PDF string bytes must be read before serialization escapes them");
}

#[test]
fn fragment_collection_limit_is_shared_across_selected_pages() {
    let src = format!("BT /F1 10 Tf {} ET", "1 0 0 1 10 100 Tm (AB)Tj 1 0 0 1 200 100 Tm (AB)Tj ".repeat(300));
    for selected in [vec![0], vec![0, 1]] {
        let clean = packet("<z:clean>Harmless</z:clean>", "");
        let mut doc = meta_doc(src.as_bytes(), &[("Author", "Ada"), ("Subject", "Spy stories")], Some(&clean), Some(&clean));
        let first = pdfcraft_model::pages(&doc).swap_remove(0);
        let second = doc.add(Object::Dict(first.dict));
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        doc.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(first.obj), Object::Ref(second)]));
            d.set(b"Count".to_vec(), Object::Int(2));
        })
        .unwrap();
        for pi in &selected {
            mark(&mut doc, *pi, &[[0.0, 0.0, 300.0, 300.0]], "");
        }
        let report = apply(&mut doc, Some(&selected)).unwrap();
        let saved = reopen(&doc);
        assert_eq!(report.metadata_cleared_on_limit, selected.len() == 2);
        assert_eq!(info_value(&saved, "Author").is_none(), selected.len() == 2);
        assert_eq!(xmp_text(&saved).is_none(), selected.len() == 2);
        assert_eq!(report.glyphs, 1_200 * selected.len());
        if selected.len() == 2 {
            assert_eq!((report.info_entries_cleared, report.xmp_removed), (2, 3));
            assert!(pdfcraft_model::pages(&saved).iter().all(|p| p.dict.get(b"Metadata").is_none()));
        }
    }
}

#[test]
fn a_later_packet_exhausts_the_same_budget_and_retires_earlier_clean_metadata() {
    let run = format!("{}b", "a".repeat(4_096));
    let value = format!("{}c", "a".repeat(4_096));
    let clean = packet("<z:clean>Harmless</z:clean>", "");
    let costly = packet(&format!("<z:value>{value}</z:value>"), "");
    let mut doc = meta_doc(b"", &[("Author", "Ada")], Some(&clean), Some(&costly));
    let mut report = Report::default();
    let pattern = KernedFragment { text: run.clone(), gaps: vec![1] };
    clear_marked_metadata(&mut doc, &[run], &[pattern], &mut report).unwrap();
    assert!(report.metadata_cleared_on_limit);
    assert_eq!((report.info_entries_cleared, report.xmp_removed), (1, 2));
    let saved = reopen(&doc);
    assert_eq!(info_value(&saved, "Author"), None);
    assert!(xmp_text(&saved).is_none());
    assert!(pdfcraft_model::pages(&saved)[0].dict.get(b"Metadata").is_none());
}

#[test]
fn exact_collection_boundaries_keep_unrelated_metadata() {
    for (run_len, runs) in [(16_384, 1), (2, 1_024), (16_384, 16)] {
        let run = "A".repeat(run_len);
        let mut src = String::from("BT /F1 10 Tf -5 Tc ");
        for i in 0..runs {
            src.push_str(&format!("1 0 0 1 {} 100 Tm ({run})Tj ", if i % 2 == 0 { 10 } else { 200 }));
        }
        src.push_str("ET");
        let mut doc = meta_doc(src.as_bytes(), &[("Author", "Someone")], None, None);
        mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert!(!report.metadata_cleared_on_limit, "{run_len} bytes by {runs} runs");
        assert_eq!(info_value(&reopen(&doc), "Author").as_deref(), Some("Someone"));
    }
}

#[test]
fn metadata_values_and_packet_decoding_share_the_operation_allowance() {
    for packet_limit in [false, true] {
        let large = "Z".repeat(if packet_limit { 2 << 20 } else { (1 << 20) + 1 });
        let clean = packet("<z:clean>Harmless</z:clean>", "");
        let costly = packet(&format!("<z:value>{large}</z:value>"), "");
        let mut doc = meta_doc(b"", &[("Title", if packet_limit { "Ada" } else { &large })], Some(&clean), Some(&costly));
        let mut report = Report::default();
        clear_marked_metadata(&mut doc, &["AB".into()], &[], &mut report).unwrap();
        assert!(report.metadata_cleared_on_limit);
        assert_eq!((report.info_entries_cleared, report.xmp_removed), (1, 2));
        let saved = reopen(&doc);
        assert_eq!(info_value(&saved, "Title"), None);
        assert!(xmp_text(&saved).is_none());
        assert!(pdfcraft_model::pages(&saved)[0].dict.get(b"Metadata").is_none());
    }
}
