//! Invented packet contents only; no real form or document is used here.

use std::sync::Arc;

use pdfcraft_cos::{Dict, ObjRef, PdfString, SaveOptions, Stream, write_incremental};

use super::*;

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const OPEN: &str = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\">";
const TEMPLATE: &str = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="master"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/>
  <medium short="612pt" long="792pt"/></pageArea></pageSet>
 <field name="entry" w="200pt" h="20pt"><ui><textEdit/></ui><value><text>default</text></value></field>
 <field name="fallback" w="200pt" h="20pt"><ui><textEdit/></ui><value><text>kept</text></value></field>
</subform></template>"#;
const DATA: &str = "<xfa:datasets><xfa:data><form><entry>Zoë &amp; Co</entry></form></xfa:data></xfa:datasets>";
const CLOSE: &str = "</xdp:xdp>";

/// Build a synthetic COS `/XFA` array with distinct compressed packet streams in supplied order.
/// Return their references so tests can verify that reads and layout preserve encoded bytes and
/// stream dictionaries.
fn array_document(packets: &[(&str, Vec<u8>)]) -> (Document, Vec<ObjRef>) {
    let mut doc = Document::new_empty();
    let mut items = Vec::new();
    let mut refs = Vec::new();
    for (name, bytes) in packets {
        let mut stream = Stream::flate(Dict::new(), bytes);
        // Match an already-stored PDF stream: the writer normally supplies /Length.
        stream.dict.set(b"Length".to_vec(), Object::Int(i64::try_from(stream.raw.len()).unwrap()));
        let stream = doc.add(Object::Stream(stream));
        refs.push(stream);
        items.push(Object::String(PdfString::literal(name.as_bytes().to_vec())));
        items.push(Object::Ref(stream));
    }
    let mut acro = Dict::new();
    acro.set(b"XFA".to_vec(), Object::Array(items));
    doc.update_dict(doc.root().unwrap(), |c| {
        c.set(b"AcroForm".to_vec(), Object::Dict(acro));
        c.set(b"NeedsRendering".to_vec(), Object::Bool(true));
    })
    .unwrap();
    (doc, refs)
}

/// Return the minimal multi-packet XDP used by declaration-assembly and layout regressions.
fn fixture() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("preamble", format!("{DECL}{OPEN}").into_bytes()),
        ("template", format!("{DECL}{TEMPLATE}").into_bytes()),
        ("datasets", format!("{DECL}{DATA}").into_bytes()),
        ("postamble", CLOSE.as_bytes().to_vec()),
    ]
}

/// Exercise all marker combinations so the shared PDF marker rule cannot drift between callers.
#[test]
fn layout_marker_rule_covers_all_boolean_combinations() {
    assert!(wants_layout(false, false));
    assert!(!wants_layout(false, true));
    assert!(wants_layout(true, false));
    assert!(wants_layout(true, true));
}

/// Malformed declarations remain errors for independently marked dynamic forms, while a marked
/// static form is still rejected by the normal classifier as `NotXfa`.
#[test]
fn malformed_packet_declaration_keeps_the_dynamic_layout_error() {
    let mut packets = fixture();
    packets[1].1 = format!("<?xml version=?>{TEMPLATE}").into_bytes();
    let (mut doc, _) = array_document(&packets);

    assert!(matches!(read_packets(&doc), Err(XfaError::Malformed(_))));
    assert!(!crate::is_dynamic(&doc));
    assert!(matches!(crate::render_into(&mut doc), Err(XfaError::Malformed(_))));

    let (mut static_doc, _) = array_document(&packets);
    static_doc
        .update_dict(static_doc.root().unwrap(), |catalog| {
            catalog.set(b"NeedsRendering".to_vec(), Object::Bool(false));
            if let Some(acro) = catalog.get_mut(b"AcroForm").and_then(|o| o.as_dict_mut()) {
                acro.set(b"Fields".to_vec(), Object::Array(vec![Object::Int(1)]));
            }
        })
        .unwrap();
    assert!(!crate::is_dynamic(&static_doc));
    assert!(matches!(crate::render_into(&mut static_doc), Err(XfaError::NotXfa)));
}

/// Rendering must follow the successful-packet classifier, including when another change
/// extends it to configuration-required saved forms. Do not duplicate that policy here.
#[test]
fn saved_packet_forms_follow_classification_and_keep_static_or_signed_bytes() {
    for required in [false, true] {
        for signed in [false, true] {
            let mut packets = fixture();
            if required {
                packets.insert(
                    1,
                    (
                        "config",
                        format!("{DECL}<config><acrobat><acrobat7><dynamicRender>required</dynamicRender></acrobat7></acrobat></config>")
                            .into_bytes(),
                    ),
                );
            }
            let (mut doc, refs) = array_document(&packets);
            let mut signature = Dict::new();
            signature.set(b"FT".to_vec(), Object::name("Sig"));
            if signed {
                signature.set(b"V".to_vec(), Object::Dict(Dict::new()));
            }
            let signature = doc.add(Object::Dict(signature));
            doc.update_dict(doc.root().unwrap(), |catalog| {
                catalog.remove(b"NeedsRendering");
                let acro = catalog.get_mut(b"AcroForm").unwrap().as_dict_mut().unwrap();
                acro.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(signature)]));
            })
            .unwrap();
            let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
            let streams: Vec<_> = refs.iter().map(|r| doc.get(*r)).collect();
            let eligible = crate::is_dynamic(&doc);
            if signed || !required {
                assert!(!eligible, "static and signed saved forms remain ineligible");
            }
            match crate::render_into(&mut doc) {
                Ok(report) => {
                    assert!(eligible, "layout must follow is_dynamic");
                    assert_eq!((report.pages, report.fields), (1, 2));
                }
                Err(XfaError::NotXfa) => {
                    assert!(!eligible, "do not veto a successful-packet classification");
                    assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
                }
                other => panic!("unexpected layout result: {other:?}"),
            }
            assert_eq!(refs.iter().map(|r| doc.get(*r)).collect::<Vec<_>>(), streams);
        }
    }
}

/// Check that a synthetic assembled XDP lays out to one page and two fields with expected values.
fn assert_layout(xml: &str) {
    let form = crate::layout_xml(xml).expect("assembled XDP lays out");
    assert_eq!(form.pages.len(), 1);
    assert_eq!(form.fields, 2);
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    let widgets: Vec<_> = form
        .pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|i| if let crate::Item::Widget(w) = i { Some((w.som.as_str(), w.value.as_deref())) } else { None })
        .collect();
    assert_eq!(widgets, [("form[0].entry[0]", Some("Zoë & Co")), ("form[0].fallback[0]", Some("kept"))]);
}

/// Two later declarations must both be handled, and envelope fragments must stay in context.
#[test]
fn packet_leading_declarations_allow_complete_xdp_layout() {
    let packets = fixture();
    assert!(roxmltree::Document::parse(OPEN).is_err());
    assert!(roxmltree::Document::parse(CLOSE).is_err());
    let raw = packets.iter().map(|(_, b)| String::from_utf8(b.clone()).unwrap() + "\n").collect::<String>();
    assert!(matches!(roxmltree::Document::parse(&raw), Err(roxmltree::Error::UnexpectedDeclaration(_))));
    let one_removed = raw.replacen(&format!("{DECL}{TEMPLATE}"), TEMPLATE, 1);
    assert!(matches!(roxmltree::Document::parse(&one_removed), Err(roxmltree::Error::UnexpectedDeclaration(_))));
    let (mut doc, refs) = array_document(&packets);
    let before: Vec<_> = refs.iter().map(|r| doc.get(*r)).collect();
    let assembled = read_packets(&doc).unwrap().unwrap();
    // This is the regression assertion: before the fix it fails with UnexpectedDeclaration.
    let parsed = roxmltree::Document::parse(&assembled.xdp).expect("production packet reader assembles valid XDP");
    assert_eq!(parsed.root_element().tag_name().namespace(), Some("http://ns.adobe.com/xdp/"));
    assert_eq!(assembled.xdp, format!("{DECL}{OPEN}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
    assert_layout(&assembled.xdp);
    assert_eq!(refs.iter().map(|r| doc.get(*r)).collect::<Vec<_>>(), before, "compressed stream bytes and dictionaries stay intact");
    let report = crate::render_into(&mut doc).expect("the same packets render into PDF pages");
    assert_eq!((report.pages, report.fields), (1, 2));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(refs.iter().map(|r| doc.get(*r)).collect::<Vec<_>>(), before, "layout does not rewrite packet streams");
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(saved)).unwrap();
    assert_eq!(refs.iter().map(|r| reopened.get(*r)).collect::<Vec<_>>(), before, "saved compressed bytes and dictionaries match");
    assert_layout(&read_packets(&reopened).unwrap().unwrap().xdp);
}

/// Accept legal XML declaration whitespace and pseudo-attribute order, then verify full layout.
#[test]
fn declarations_accept_only_xml_whitespace_and_ordered_literal_values() {
    for decl in [
        "<?xml version='1.0'?>",
        "<?xml\tversion = \"1.0\"\r\nencoding='UTF-8' standalone = 'yes' ?>",
        "<?xml\nversion='1.1' standalone='no'?>",
        "<?xml\rversion='1.23' encoding='x-test_1.2'?>",
    ] {
        let packets = [
            ("preamble", format!("{decl}{OPEN}").into_bytes()),
            ("template", format!("{decl}{TEMPLATE}").into_bytes()),
            ("datasets", format!("{decl}{DATA}").into_bytes()),
            ("postamble", format!("{decl}{CLOSE}").into_bytes()),
        ];
        let (doc, _) = array_document(&packets);
        let xml = read_packets(&doc).unwrap().unwrap().xdp;
        assert_eq!(xml, format!("{decl}{OPEN}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
        assert_layout(&xml);
    }
}

/// Preserve declaration-free packet text and a standalone template packet without normalization.
#[test]
fn packets_without_declarations_and_a_template_alone_keep_their_text() {
    let packets =
        [("preamble", OPEN), ("template", TEMPLATE), ("datasets", DATA), ("postamble", CLOSE)].map(|(name, text)| (name, text.as_bytes().to_vec()));
    let (doc, _) = array_document(&packets);
    let xml = read_packets(&doc).unwrap().unwrap().xdp;
    assert_eq!(xml, format!("{OPEN}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
    assert_layout(&xml);
    let alone = format!("{DECL}{TEMPLATE}");
    let (doc, _) = array_document(&[("template", alone.as_bytes().to_vec())]);
    assert_eq!(read_packets(&doc).unwrap().unwrap().xdp, alone + "\n");
}

/// Keep declaration-shaped text inside comments, CDATA and non-XML processing instructions.
#[test]
fn processing_instructions_comments_and_cdata_are_preserved() {
    let extra = format!(
        "<?xml-stylesheet type='text/xsl' href='test.xsl'?><?xml-model href='test.rng'?>\n<!--{DECL}-->\n<note><![CDATA[{DECL} & literal]]></note>"
    );
    let packets = [
        ("preamble", format!("{DECL}{OPEN}").into_bytes()),
        ("note", extra.as_bytes().to_vec()),
        ("template", format!("{DECL}{TEMPLATE}").into_bytes()),
        ("datasets", format!("{DECL}{DATA}").into_bytes()),
        ("postamble", CLOSE.as_bytes().to_vec()),
    ];
    let (doc, _) = array_document(&packets);
    let xml = read_packets(&doc).unwrap().unwrap().xdp;
    assert_eq!(xml, format!("{DECL}{OPEN}\n{extra}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
    let parsed = roxmltree::Document::parse(&xml).unwrap();
    assert!(parsed.descendants().any(|n| n.is_comment() && n.text() == Some(DECL)));
    assert!(parsed.descendants().any(|n| n.has_tag_name("note") && n.text() == Some(&format!("{DECL} & literal"))));
    assert_eq!(parsed.descendants().filter_map(|n| n.pi()).map(|p| p.target).collect::<Vec<_>>(), ["xml-stylesheet", "xml-model"]);
    assert_layout(&xml);
}

/// Do not remove declaration-shaped prefixes when a packet continues an open lexical section.
#[test]
fn declaration_shaped_text_in_continued_lexical_sections_is_preserved() {
    for (start, end) in [("<!--", "-->"), ("<note><![CDATA[", "]]></note>"), ("<?observer", "")] {
        let packets = [
            ("preamble", format!("{DECL}{OPEN}").into_bytes()),
            ("section-start", start.as_bytes().to_vec()),
            ("section-end", format!("{DECL}{end}").into_bytes()),
            ("template", format!("{DECL}{TEMPLATE}").into_bytes()),
            ("datasets", format!("{DECL}{DATA}").into_bytes()),
            ("postamble", CLOSE.as_bytes().to_vec()),
        ];
        let (doc, _) = array_document(&packets);
        let xml = read_packets(&doc).unwrap().unwrap().xdp;
        assert_eq!(xml, format!("{DECL}{OPEN}\n{start}\n{DECL}{end}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
        assert_layout(&xml);
    }
}

/// Keep a boundary declaration inside a quoted attribute and let the XML parser reject bad markup.
#[test]
fn a_packet_boundary_in_an_attribute_does_not_repair_invalid_markup() {
    // '>' in the attribute is literal; it must not put the scanner back in XML content.
    let start = "<note attr='>";
    let end = format!("{DECL}'>text</note>");
    let (doc, _) = array_document(&[
        ("preamble", format!("{DECL}{OPEN}{start}").into_bytes()),
        ("attribute", end.as_bytes().to_vec()),
        ("template", TEMPLATE.as_bytes().to_vec()),
        ("postamble", CLOSE.as_bytes().to_vec()),
    ]);
    let xml = read_packets(&doc).unwrap().unwrap().xdp;
    assert_eq!(xml, format!("{DECL}{OPEN}{start}\n{end}\n{TEMPLATE}\n{CLOSE}\n"));
    assert!(crate::parse(&xml).is_err(), "literal '<' in an attribute is still malformed");
}

/// Reject inner, repeated, misplaced, or otherwise unrelated malformed XML rather than repairing it.
#[test]
fn inner_declarations_and_unrelated_xml_errors_are_not_repaired() {
    for extra in [
        format!("<note>{DECL}</note>"),
        format!(" \n{DECL}<note/>"),
        format!("<!--before-->{DECL}<note/>"),
        format!("<?observer test?>{DECL}<note/>"),
        format!("{DECL}{DECL}<note/>"),
        "<unbound:note/>".into(),
        "<note>mismatched</other>".into(),
        "<!DOCTYPE note [<!ENTITY x 'no'>]><note/>".into(),
    ] {
        let (doc, _) = array_document(&[
            ("preamble", format!("{DECL}{OPEN}").into_bytes()),
            ("template", TEMPLATE.as_bytes().to_vec()),
            ("extra", extra.as_bytes().to_vec()),
            ("postamble", CLOSE.as_bytes().to_vec()),
        ]);
        let xml = read_packets(&doc).unwrap().unwrap().xdp;
        assert!(crate::parse(&xml).is_err(), "unrelated malformed input must fail: {extra}");
        if !extra.starts_with(DECL) {
            assert!(xml.contains(&extra), "interior syntax must stay intact");
        }
    }
}

/// Reject malformed declaration grammar both in the first packet and at later packet boundaries.
#[test]
fn malformed_leading_declarations_are_rejected_in_first_and_later_packets() {
    for decl in [
        "<?xml?>",
        "<?xml version='1.0'",
        "<?xml ?>",
        "<?xml encoding='UTF-8'?>",
        "<?xml version=1.0?>",
        "<?xml version='1.0\"?>",
        "<?xml version='1.'?>",
        "<?xml version='2.0'?>",
        "<?xml version='1.a'?>",
        "<?xml version='1.&#48;'?>",
        "<?xml version='1.0'version='1.0'?>",
        "<?xml version='1.0' version='1.0'?>",
        "<?xml version='1.0' encoding=''?>",
        "<?xml version='1.0' encoding='8UTF'?>",
        "<?xml version='1.0' encoding='UTF 8'?>",
        "<?xml version='1.0' encoding='UTF&amp;8'?>",
        "<?xml version='1.0' standalone='maybe'?>",
        "<?xml version='1.0' standalone='yes' encoding='UTF-8'?>",
        "<?xml version='1.0' unknown='x'?>",
        "<?xml version='1.0'\u{000B}?>",
        "<?xml version='1.0'\u{00A0}?>",
        "<?xml version='1.0' ? >",
    ] {
        for first in [true, false] {
            let (doc, _) = array_document(&[
                ("preamble", format!("{}{OPEN}", if first { decl } else { DECL }).into_bytes()),
                ("template", format!("{}{TEMPLATE}", if first { "" } else { decl }).into_bytes()),
                ("postamble", CLOSE.as_bytes().to_vec()),
            ]);
            assert!(matches!(read_packets(&doc), Err(XfaError::Malformed(_))), "must reject {decl:?}, first={first}");
        }
    }
}

/// Decode supported UTF-8/UTF-16 byte-order variants and preserve each original stored stream.
#[test]
fn packet_decoding_keeps_utf8_utf16_boms_and_original_bytes() {
    for encoding in [
        Encoding::Utf8 { bom: false },
        Encoding::Utf8 { bom: true },
        Encoding::Utf16Le { bom: false },
        Encoding::Utf16Le { bom: true },
        Encoding::Utf16Be { bom: false },
        Encoding::Utf16Be { bom: true },
    ] {
        let label = if matches!(encoding, Encoding::Utf8 { .. }) { "UTF-8" } else { "UTF-16" };
        let packets = fixture()
            .into_iter()
            .map(|(name, bytes)| {
                let text = String::from_utf8(bytes).unwrap().replace("UTF-8", label);
                (name, encode(&text, encoding))
            })
            .collect::<Vec<_>>();
        let (mut doc, refs) = array_document(&packets);
        let before: Vec<_> = refs.iter().map(|r| doc.get(*r)).collect();
        let xml = read_packets(&doc).unwrap().unwrap().xdp;
        assert_eq!(xml, format!("{}{OPEN}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n", DECL.replace("UTF-8", label)));
        assert_layout(&xml);
        crate::render_into(&mut doc).unwrap();
        let reopened = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(refs.iter().map(|r| reopened.get(*r)).collect::<Vec<_>>(), before, "original {encoding:?} stream storage survives save");
        for (r, (_, bytes)) in refs.iter().zip(&packets) {
            let object = reopened.get(*r);
            let Object::Stream(stream) = &*object else { panic!("fixture stream disappeared") };
            assert_eq!(&stream.decoded().unwrap(), bytes);
            assert_eq!(decode(bytes).unwrap().1, encoding);
        }
    }
}

/// Assemble independently encoded packets in their declared order after per-packet decoding.
#[test]
fn differently_encoded_packets_assemble_in_original_order() {
    let encodings = [Encoding::Utf8 { bom: true }, Encoding::Utf16Le { bom: true }, Encoding::Utf16Be { bom: false }, Encoding::Utf8 { bom: false }];
    let packets = fixture()
        .into_iter()
        .zip(encodings)
        .map(|((name, bytes), encoding)| {
            let label = if matches!(encoding, Encoding::Utf8 { .. }) { "UTF-8" } else { "UTF-16" };
            let text = String::from_utf8(bytes).unwrap().replace("UTF-8", label);
            (name, encode(&text, encoding))
        })
        .collect::<Vec<_>>();
    let (doc, _) = array_document(&packets);
    let xml = read_packets(&doc).unwrap().unwrap().xdp;
    assert_eq!(xml, format!("{DECL}{OPEN}\n{TEMPLATE}\n{DATA}\n{CLOSE}\n"));
    assert_layout(&xml);
}

/// Leave whole-XDP streams byte-for-text unchanged, including an illegal inner declaration.
#[test]
fn whole_xdp_streams_are_not_normalized() {
    for inner in ["", DECL] {
        let xml = format!("{DECL}{OPEN}{inner}{TEMPLATE}{DATA}{CLOSE}");
        let doc = Document::open(Arc::new(crate::fixtures::shell(&xml))).unwrap();
        assert_eq!(read_packets(&doc).unwrap().unwrap().xdp, xml);
        if inner.is_empty() {
            assert_layout(&xml);
        } else {
            assert!(matches!(roxmltree::Document::parse(&xml), Err(roxmltree::Error::UnexpectedDeclaration(_))));
        }
    }
}

/// Exercise every UTF-8 character-boundary truncation without allowing the declaration scanner to panic.
#[test]
fn declaration_truncations_and_non_ascii_values_do_not_panic() {
    for input in [DECL, "<?xml version='1.0' encoding='ünicode'?>", "<?xml version='1.0' standalone='日本'?>"] {
        for end in input.char_indices().map(|(p, _)| p).chain(std::iter::once(input.len())) {
            // Exercise the same bounded reader on every character-boundary truncation.
            let (doc, _) = array_document(&[("template", format!("{}{TEMPLATE}", &input[..end]).into_bytes())]);
            let _ = read_packets(&doc);
        }
    }
}
