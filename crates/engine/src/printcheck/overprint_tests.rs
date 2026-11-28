use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::super::{inspect, overprint_finding, overprint_line};
use super::*;

fn dict(entries: &[(&str, Object)]) -> Dict {
    let mut d = Dict::new();
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v.clone());
    }
    d
}

fn gstate(op: bool) -> Object {
    Object::Dict(dict(&[("Type", Object::name("ExtGState")), ("OP", Object::Bool(op)), ("op", Object::Bool(op))]))
}

/// An `/ExtGState` resource category holding `Over`, which turns overprint on.
fn over() -> Object {
    Object::Dict(dict(&[("Over", gstate(true))]))
}

fn stream(doc: &mut Document, d: Dict, content: &str) -> ObjRef {
    doc.add(Object::Stream(Stream::from_raw(d, content.as_bytes().to_vec())))
}

fn form(doc: &mut Document, resources: Dict, content: &str) -> ObjRef {
    let bbox = Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()]);
    let d = dict(&[("Subtype", Object::name("Form")), ("BBox", bbox), ("Resources", Object::Dict(resources))]);
    stream(doc, d, content)
}

fn add_page(doc: &mut Document, resources: Dict, content: &str) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let contents = stream(doc, Dict::new(), content);
    let media = Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]);
    let p = dict(&[
        ("Type", Object::name("Page")),
        ("Parent", Object::Ref(pages)),
        ("MediaBox", media),
        ("Resources", Object::Dict(resources)),
        ("Contents", Object::Ref(contents)),
    ]);
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

fn walk(doc: &Document) -> Overprint {
    overprint_use(doc, &pdfcraft_model::pages(doc))
}

fn not_checked(text: &str) -> String {
    text.lines().find(|l| l.starts_with("Not checked")).unwrap().to_string()
}

#[test]
fn overprint_in_a_form_xobject_is_found_on_the_page_that_draws_it() {
    let mut doc = Document::new_empty();
    add_page(&mut doc, Dict::new(), "0 0 10 10 re f");
    let f = form(&mut doc, dict(&[("ExtGState", over())]), "/Over gs 0 0 5 5 re f");
    add_page(&mut doc, dict(&[("XObject", Object::Dict(dict(&[("F", Object::Ref(f))])))]), "/F Do");

    let report = inspect(&doc);
    assert_eq!(report.overprint.page, Some(2), "{:?}", report.overprint);
    assert_eq!(report.overprint.object, Some(f.to_string()));
    assert!(report.findings.iter().any(|x| x.id == "overprint" && x.page == Some(2)), "{:?}", report.findings);
    assert!(!not_checked(&report.text()).contains("overprint"), "{}", report.text());
}

#[test]
fn a_false_or_uninvoked_graphics_state_asks_for_nothing() {
    let mut doc = Document::new_empty();
    // `On` is listed in the page and in a form the page holds, but no drawn `gs` names it.
    let states = || Object::Dict(dict(&[("Off", gstate(false)), ("On", gstate(true))]));
    let f = form(&mut doc, dict(&[("ExtGState", states())]), "/On gs");
    let resources = dict(&[("ExtGState", states()), ("XObject", Object::Dict(dict(&[("F", Object::Ref(f))])))]);
    let p = add_page(&mut doc, resources, "/Off gs 0 0 10 10 re f");

    let report = inspect(&doc);
    assert!(!report.overprint.asks() && report.overprint.proven(), "{:?}", report.overprint);
    assert!(!report.findings.iter().any(|x| x.id == "overprint"), "{:?}", report.findings);
    assert!(!not_checked(&report.text()).contains("overprint"), "{}", report.text());

    // The same page, drawing the form: now it asks.
    let contents = doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
    doc.set(contents, Object::Stream(Stream::from_raw(Dict::new(), b"/Off gs /F Do".to_vec())));
    assert!(walk(&doc).asks());
}

#[test]
fn a_walk_cut_short_is_not_proven() {
    let mut doc = Document::new_empty();
    add_page(&mut doc, dict(&[("ExtGState", over())]), "0 0 10 10 re f 0 0 5 5 re f /Over gs");
    let pages = pdfcraft_model::pages(&doc);

    let short = overprint_use_within(&doc, &pages, Limits { operators: 2, ..Limits::DOCUMENT });
    assert!(!short.asks() && !short.proven() && !short.unwalked.is_empty(), "{short:?}");
    assert!(overprint_finding(&short).is_some_and(|x| x.page.is_none() && x.message.contains("isn't proven")));
    assert!(overprint_line(&short).contains("not proven"));
    assert!(overprint_use(&doc, &pages).asks());
}

#[test]
fn requests_in_patterns_glyphs_masks_and_appearances_are_found() {
    let mut cases: Vec<(&str, Document)> = Vec::new();

    let mut doc = Document::new_empty();
    let tile = stream(&mut doc, dict(&[("PatternType", Object::Int(1)), ("Resources", Object::Dict(dict(&[("ExtGState", over())])))]), "/Over gs");
    add_page(&mut doc, dict(&[("Pattern", Object::Dict(dict(&[("T", Object::Ref(tile))])))]), "/Pattern cs /T scn 0 0 9 9 re f");
    cases.push(("tiling pattern", doc));

    let mut doc = Document::new_empty();
    let shading = doc.add(Object::Dict(dict(&[("PatternType", Object::Int(2)), ("ExtGState", gstate(true))])));
    add_page(&mut doc, dict(&[("Pattern", Object::Dict(dict(&[("S", Object::Ref(shading))])))]), "/Pattern cs /S scn 0 0 9 9 re f");
    cases.push(("shading pattern", doc));

    let mut doc = Document::new_empty();
    let glyph = stream(&mut doc, Dict::new(), "0 0 d0 /Over gs 0 0 1 1 re f");
    let font = doc.add(Object::Dict(dict(&[
        ("Subtype", Object::name("Type3")),
        ("CharProcs", Object::Dict(dict(&[("a", Object::Ref(glyph))]))),
        ("Resources", Object::Dict(dict(&[("ExtGState", over())]))),
    ])));
    add_page(&mut doc, dict(&[("Font", Object::Dict(dict(&[("T3", Object::Ref(font))])))]), "BT /T3 1 Tf (a) Tj ET");
    cases.push(("Type 3 glyph", doc));

    let mut doc = Document::new_empty();
    let group = form(&mut doc, dict(&[("ExtGState", over())]), "/Over gs");
    let mask = Object::Dict(dict(&[("S", Object::name("Luminosity")), ("G", Object::Ref(group))]));
    let masked = Object::Dict(dict(&[("SMask", mask)]));
    add_page(&mut doc, dict(&[("ExtGState", Object::Dict(dict(&[("M", masked)])))]), "/M gs");
    cases.push(("soft-mask group", doc));

    let mut doc = Document::new_empty();
    let on = form(&mut doc, dict(&[("ExtGState", over())]), "/Over gs");
    let ap = Object::Dict(dict(&[("N", Object::Dict(dict(&[("On", Object::Ref(on))])))]));
    let annot = doc.add(Object::Dict(dict(&[("Subtype", Object::name("Widget")), ("AP", ap)])));
    let p = add_page(&mut doc, Dict::new(), "");
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annot)]))).unwrap();
    cases.push(("appearance state", doc));

    for (what, doc) in &cases {
        let found = walk(doc);
        assert!(found.asks() && found.page == Some(1), "{what}: {found:?}");
    }
}

fn xobjects(entries: &[(&str, ObjRef)]) -> Object {
    Object::Dict(dict(&entries.iter().map(|(k, r)| (*k, Object::Ref(*r))).collect::<Vec<_>>()))
}

#[test]
fn forms_that_draw_each_other_end_with_a_proven_answer() {
    let mut doc = Document::new_empty();
    let a = form(&mut doc, Dict::new(), "");
    let b = form(&mut doc, Dict::new(), "");
    for (r, content) in [(a, "/A Do /B Do"), (b, "/A Do")] {
        let resources = dict(&[("XObject", xobjects(&[("A", a), ("B", b)]))]);
        let d = dict(&[("Subtype", Object::name("Form")), ("Resources", Object::Dict(resources))]);
        doc.set(r, Object::Stream(Stream::from_raw(d, content.as_bytes().to_vec())));
    }
    add_page(&mut doc, dict(&[("XObject", xobjects(&[("A", a)]))]), "/A Do");

    let found = walk(&doc);
    assert!(!found.asks() && found.proven(), "{found:?}");
}

#[test]
fn a_form_without_resources_uses_those_of_the_stream_that_draws_it() {
    let mut doc = Document::new_empty();
    let bare = dict(&[("Subtype", Object::name("Form"))]);
    let f = stream(&mut doc, bare, "/Over gs");
    add_page(&mut doc, dict(&[("ExtGState", over()), ("XObject", xobjects(&[("F", f)]))]), "/F Do");

    let found = walk(&doc);
    assert!(found.asks() && found.object == Some(f.to_string()), "{found:?}");
}

#[test]
fn the_font_restored_by_q_and_q_decides_whether_glyphs_are_walked() {
    let page = |content: &str| {
        let mut doc = Document::new_empty();
        let glyph = stream(&mut doc, Dict::new(), "0 0 d0 /Over gs 0 0 1 1 re f");
        let t3 = doc.add(Object::Dict(dict(&[
            ("Subtype", Object::name("Type3")),
            ("CharProcs", Object::Dict(dict(&[("a", Object::Ref(glyph))]))),
            ("Resources", Object::Dict(dict(&[("ExtGState", over())]))),
        ])));
        let plain = doc.add(Object::Dict(dict(&[("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica"))])));
        add_page(&mut doc, dict(&[("Font", xobjects(&[("T3", t3), ("P", plain)]))]), content);
        walk(&doc)
    };

    let restored = page("BT /T3 1 Tf q /P 1 Tf Q (a) Tj ET");
    assert!(restored.asks(), "{restored:?}");
    let plain = page("BT /P 1 Tf q /T3 1 Tf Q (a) Tj ET");
    assert!(!plain.asks() && plain.proven(), "{plain:?}");
}

#[test]
fn every_cap_and_undecodable_stream_leaves_the_answer_unproven() {
    let mut doc = Document::new_empty();
    let inner = form(&mut doc, dict(&[("ExtGState", over())]), "/Over gs");
    let outer = form(&mut doc, dict(&[("XObject", xobjects(&[("I", inner)]))]), "/I Do");
    add_page(&mut doc, dict(&[("XObject", xobjects(&[("O", outer)]))]), "/O Do");
    let pages = pdfcraft_model::pages(&doc);
    assert!(overprint_use(&doc, &pages).asks());

    let capped = [
        ("nests deeper than", Limits { depth: 1, ..Limits::DOCUMENT }),
        ("streams, on page", Limits { streams: 1, ..Limits::DOCUMENT }),
        ("of decoded content", Limits { bytes: 4, ..Limits::DOCUMENT }),
    ];
    for (why, limits) in capped {
        let short = overprint_use_within(&doc, &pages, limits);
        assert!(!short.asks() && !short.proven(), "{why}: {short:?}");
        assert!(short.unwalked.iter().any(|r| r.contains(why)), "{why}: {short:?}");
    }

    for (filter, why) in [("FlateDecode", "damaged partway"), ("NoSuchDecode", "can't be decoded")] {
        let mut doc = Document::new_empty();
        let d = dict(&[("Subtype", Object::name("Form")), ("Filter", Object::name(filter))]);
        let broken = stream(&mut doc, d, "not deflate data");
        add_page(&mut doc, dict(&[("XObject", xobjects(&[("B", broken)]))]), "/B Do");
        let undecoded = walk(&doc);
        assert!(!undecoded.asks() && !undecoded.proven(), "{filter}: {undecoded:?}");
        assert!(undecoded.unwalked.iter().any(|r| r.contains(why)), "{filter}: {undecoded:?}");
    }

    let mut doc = Document::new_empty();
    add_page(&mut doc, Dict::new(), &"q ".repeat(MAX_SAVES + 1));
    let nested = walk(&doc);
    assert!(!nested.proven(), "{nested:?}");
    assert!(nested.unwalked.iter().any(|r| r.contains(&format!("nest deeper than {MAX_SAVES}"))), "{nested:?}");
}
