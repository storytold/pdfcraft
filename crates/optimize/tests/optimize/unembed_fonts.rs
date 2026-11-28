//! Unembed fonts on documents built here: which fonts lose their programs, and that what a
//! substitute needs stays.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::unembed::{Outcome, Refusal, Request, plan, unembed};

/// A page whose resources list `fonts`, drawing a line of text in each.
fn text_page(doc: &mut Document, fonts: &[(&str, ObjRef)]) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut f = Dict::new();
    let mut content = String::new();
    for (n, r) in fonts {
        f.set(n.as_bytes().to_vec(), Object::Ref(*r));
        content.push_str(&format!("BT /{n} 12 Tf 72 700 Td (Hello) Tj ET\n"));
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(f));
    let c = doc.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    r
}

/// Stand-in program bytes: nothing parses them, only their size is measured.
fn program(doc: &mut Document, len: usize) -> ObjRef {
    let data: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
    doc.add(Object::Stream(Stream::from_raw(Dict::new(), data)))
}

fn descriptor(doc: &mut Document, name: &str, flags: i64, key: &str, program: ObjRef) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    d.set(b"FontName".to_vec(), Object::name(name));
    d.set(b"Flags".to_vec(), Object::Int(flags));
    d.set(b"FontBBox".to_vec(), Object::Array(vec![0.into(), (-200).into(), 1000.into(), 900.into()]));
    d.set(b"ItalicAngle".to_vec(), Object::Int(0));
    d.set(b"Ascent".to_vec(), Object::Int(900));
    d.set(b"Descent".to_vec(), Object::Int(-200));
    d.set(b"CapHeight".to_vec(), Object::Int(700));
    d.set(b"StemV".to_vec(), Object::Int(80));
    let set = doc.add(Object::Stream(Stream::from_raw(Dict::new(), vec![0xFF; 4])));
    d.set(b"CIDSet".to_vec(), Object::Ref(set));
    d.set(key.as_bytes().to_vec(), Object::Ref(program));
    doc.add(Object::Dict(d))
}

fn font(doc: &mut Document, subtype: &str, name: &str, encoding: Option<&str>, widths: bool, fd: ObjRef) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Font"));
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"BaseFont".to_vec(), Object::name(name));
    d.set(b"FirstChar".to_vec(), Object::Int(32));
    d.set(b"LastChar".to_vec(), Object::Int(34));
    if widths {
        d.set(b"Widths".to_vec(), Object::Array(vec![250.into(), 333.into(), 408.into()]));
    }
    if let Some(e) = encoding {
        d.set(b"Encoding".to_vec(), Object::name(e));
    }
    d.set(b"FontDescriptor".to_vec(), Object::Ref(fd));
    let cmap = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"%cmap".to_vec())));
    d.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    doc.add(Object::Dict(d))
}

/// A non-symbolic WinAnsi TrueType font with a program of `len` bytes: (font, descriptor).
fn simple(doc: &mut Document, name: &str, len: usize) -> (ObjRef, ObjRef) {
    let p = program(doc, len);
    let fd = descriptor(doc, name, 32, "FontFile2", p);
    (font(doc, "TrueType", name, Some("WinAnsiEncoding"), true, fd), fd)
}

fn saved(doc: &Document) -> Vec<u8> {
    write_full(doc, &SaveOptions::default()).unwrap()
}

fn dict(doc: &Document, r: ObjRef) -> Dict {
    doc.get(r).as_dict().unwrap().clone()
}

#[test]
fn a_standard_encoded_font_loses_its_program_and_keeps_its_metrics() {
    let mut doc = Document::new_empty();
    let (f, fd) = simple(&mut doc, "ABCDEF+Serifish", 40_000);
    text_page(&mut doc, &[("F1", f)]);
    let before = saved(&doc).len();
    let report = unembed(&mut doc, &Request::default()).unwrap();
    assert_eq!(report.unembedded(), 1);
    let after = saved(&doc);
    assert!(after.len() + 30_000 < before, "the program stays in the saved file: {before} -> {}", after.len());
    assert!(report.bytes_freed >= 40_000, "freed {} of a 40,000-byte program", report.bytes_freed);
    assert!(Document::open(Arc::new(after)).is_ok(), "the saved file does not open");
    let (font, desc) = (dict(&doc, f), dict(&doc, fd));
    assert!(!desc.contains(b"FontFile2") && !desc.contains(b"CIDSet"), "the program or its glyph set stays");
    assert_eq!(font.name(b"BaseFont"), desc.name(b"FontName"), "BaseFont and FontName no longer match");
    assert_eq!(font.name(b"BaseFont"), Some(&b"Serifish"[..]), "the subset tag stays on the name");
    for key in [&b"Widths"[..], b"ToUnicode", b"Encoding", b"FirstChar"] {
        assert!(font.contains(key), "unembedding dropped {}", String::from_utf8_lossy(key));
    }
    assert!(desc.contains(b"Ascent") && desc.contains(b"FontBBox"), "the substitute's metrics are gone");
}

#[test]
fn fonts_a_substitute_cannot_stand_in_for_keep_their_programs() {
    let mut doc = Document::new_empty();
    let cases = [
        ("Type3", 32, Some("WinAnsiEncoding"), true, Outcome::Refused(Refusal::Type3)),
        ("Type0", 32, None, true, Outcome::Refused(Refusal::Composite)),
        ("TrueType", 4, None, true, Outcome::Refused(Refusal::SymbolicEncoding)),
        ("Type1", 32, Some("WinAnsiEncoding"), false, Outcome::Refused(Refusal::NoWidths)),
        ("TrueType", 4, Some("WinAnsiEncoding"), true, Outcome::Unembed),
    ];
    let mut fonts = Vec::new();
    for (i, (subtype, flags, encoding, widths, want)) in cases.into_iter().enumerate() {
        let p = program(&mut doc, 1000);
        let fd = descriptor(&mut doc, &format!("Font{i}"), flags, "FontFile2", p);
        fonts.push((format!("F{i}"), font(&mut doc, subtype, &format!("Font{i}"), encoding, widths, fd), fd, want));
    }
    let named: Vec<(&str, ObjRef)> = fonts.iter().map(|(n, f, _, _)| (n.as_str(), *f)).collect();
    text_page(&mut doc, &named);
    let report = unembed(&mut doc, &Request::default()).unwrap();
    for (name, f, fd, want) in &fonts {
        let got = report.fonts.iter().find(|p| p.font == Some(*f)).map(|p| p.outcome);
        assert_eq!(got, Some(*want), "{name}");
        assert_eq!(dict(&doc, *fd).contains(b"FontFile2"), *want != Outcome::Unembed, "{name}");
    }
}

#[test]
fn a_descriptor_shared_with_a_font_that_stays_blocks_both() {
    let mut doc = Document::new_empty();
    let p = program(&mut doc, 1000);
    let fd = descriptor(&mut doc, "Shared", 32, "FontFile", p);
    let a = font(&mut doc, "Type1", "Shared", Some("WinAnsiEncoding"), true, fd);
    let b = font(&mut doc, "Type1", "SharedToo", Some("WinAnsiEncoding"), true, fd);
    text_page(&mut doc, &[("A", a), ("B", b)]);
    let one = Request { fonts: Some(vec!["Shared".into()]), keep_subset_tag: false };
    let report = unembed(&mut doc, &one).unwrap();
    assert_eq!(report.unembedded(), 0);
    assert!(report.fonts.iter().any(|f| f.font == Some(a) && f.outcome == Outcome::Refused(Refusal::DescriptorShared)));
    assert!(dict(&doc, fd).contains(b"FontFile"), "a font that was not asked for lost its program");
    let both = Request { fonts: Some(vec!["Shared".into(), "SharedToo".into()]), keep_subset_tag: false };
    assert_eq!(unembed(&mut doc, &both).unwrap().unembedded(), 2);
    assert!(!dict(&doc, fd).contains(b"FontFile"));
}

#[test]
fn a_program_another_font_still_uses_is_not_counted_as_freed() {
    let mut doc = Document::new_empty();
    let p = program(&mut doc, 5000);
    let fd1 = descriptor(&mut doc, "One", 32, "FontFile2", p);
    let fd2 = descriptor(&mut doc, "Two", 4, "FontFile2", p);
    let one = font(&mut doc, "TrueType", "One", Some("WinAnsiEncoding"), true, fd1);
    let two = font(&mut doc, "TrueType", "Two", None, true, fd2);
    text_page(&mut doc, &[("A", one), ("B", two)]);
    let report = unembed(&mut doc, &Request::default()).unwrap();
    let first = report.fonts.iter().find(|f| f.font == Some(one)).unwrap();
    assert_eq!(first.outcome, Outcome::Unembed);
    assert!(!first.freed && report.bytes_freed == 0, "a program the symbolic font still draws with was counted as freed");
    assert!(dict(&doc, fd2).contains(b"FontFile2"));
}

#[test]
fn a_plan_changes_nothing_and_reports_names_no_font_has() {
    let mut doc = Document::new_empty();
    let (f, fd) = simple(&mut doc, "QWERTY+Sansish", 2000);
    text_page(&mut doc, &[("F1", f)]);
    let before = saved(&doc);
    let request = Request { fonts: Some(vec!["Sansish".into(), "Missing".into()]), keep_subset_tag: true };
    let preview = plan(&doc, &request);
    assert_eq!(saved(&doc), before, "planning changed the document");
    assert_eq!(preview.unmatched, vec!["Missing".to_string()]);
    let done = unembed(&mut doc, &request).unwrap();
    assert!(done.unembedded() > 0 && done.unembedded() == preview.unembedded());
    assert_eq!(dict(&doc, f).name(b"BaseFont"), dict(&doc, fd).name(b"FontName"));
    assert_eq!(dict(&doc, f).name(b"BaseFont"), Some(&b"QWERTY+Sansish"[..]), "keep_subset_tag took the tag off");
}

#[test]
fn a_font_written_directly_in_the_resources_is_refused_not_lost() {
    let mut doc = Document::new_empty();
    let (f, fd) = simple(&mut doc, "Direct", 2000);
    let page = text_page(&mut doc, &[("F1", f)]);
    let inline = dict(&doc, f);
    doc.update_dict(page, |d| {
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Dict(inline));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    let report = unembed(&mut doc, &Request::default()).unwrap();
    assert_eq!(report.unembedded(), 0);
    assert!(report.fonts.iter().any(|p| p.font.is_none() && p.outcome == Outcome::Refused(Refusal::FontNotIndirect)));
    assert!(dict(&doc, fd).contains(b"FontFile2"));
}
