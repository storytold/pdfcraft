//! A page added after certification from one of the signed document's page templates (ISO
//! 32000-2 §12.7.7) is a change certification levels 2 and 3 permit ("instantiating page
//! templates", Table 257), and level 1 does not. Only such a page is: one that shows something
//! else, or a page tree that loses, reorders or miscounts pages, is still a page change.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_incremental};
use pdfcraft_sign::{Modification, SignOptions, Status, TrustStore, kind, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

/// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
fn write_pdf(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn r(num: u32) -> ObjRef {
    ObjRef { num, generation: 0 }
}

/// Two pages (3 and 10) and a hidden template, `More` (page 5), whose page holds the text field
/// `Extra` (7) and draws stream 8 with font 9; `Other` (11) is a second template that draws
/// something else.
fn form() -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /Names << /Templates 6 0 R >> /AcroForm << /Fields [7 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R /Resources << /Font << /F1 9 0 R >> >> >>",
        "<< /Length 40 >>\nstream\nBT /F1 12 Tf 20 20 Td (Visible) Tj ET\nendstream",
        "<< /Type /Template /MediaBox [0 0 200 300] /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [7 0 R] >>",
        "<< /Names [(More) 5 0 R (Other) 11 0 R] >>",
        "<< /FT /Tx /T (Extra) /V (x) /Subtype /Widget /Rect [20 100 180 120] /P 5 0 R >>",
        "<< /Length 41 >>\nstream\nBT /F1 12 Tf 20 20 Td (Template) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
        "<< /Type /Template /MediaBox [0 0 200 300] /Contents 12 0 R /Resources << /Font << /F1 9 0 R >> >> >>",
        "<< /Length 38 >>\nstream\nBT /F1 12 Tf 20 20 Td (Other) Tj ET\nendstream",
    ])
}

fn certified(p: u8) -> Vec<u8> {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let opts = SignOptions {
        page: 1,
        rect: Some([20.0, 100.0, 120.0, 150.0]),
        date: "D:20261002120000+01'00'".into(),
        certify: Some(p),
        ..SignOptions::default()
    };
    pdfcraft_sign::sign(&open(&form()), &id, &opts).unwrap()
}

fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

/// The certification signature of `bytes`: its status and modification verdict.
fn verdict(bytes: &[u8]) -> (Status, Modification) {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let s = signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed && s.certify.is_some()).unwrap();
    (s.status, s.modification)
}

fn assert_allowed(what: &str, bytes: &[u8]) {
    let (status, modification) = verdict(bytes);
    assert!(
        matches!(&modification, Modification::Allowed(kinds) if kinds.contains(&kind::PAGE_TEMPLATE)) && status == Status::Valid,
        "{what}: {status:?} {modification:?}"
    );
}

fn assert_refused(what: &str, bytes: &[u8], expected: &str) {
    let (status, modification) = verdict(bytes);
    assert!(
        matches!(&modification, Modification::Disallowed(kinds) if kinds.contains(&expected)) && status != Status::Valid,
        "{what}: {status:?} {modification:?}, expected {expected}"
    );
}

/// How a test page is made from template `More`.
#[derive(Clone, Copy, Default)]
struct Copy<'a> {
    /// The content stream's bytes, when not the template's.
    content: Option<&'a [u8]>,
    /// The `/TemplateInstantiated` written, if any.
    instantiated: Option<&'a str>,
    /// An annotation listed on the page instead of the copied widget.
    annots: Option<ObjRef>,
}

/// Instantiate template `More` as a writer does: copy its content stream and its widget (as the
/// renamed field `P2.More.Extra`, listed in the form), add a page drawing them with the
/// template's font, and put it into the page tree at `kids` (where `None` stands for the new
/// page), counted as `count` pages. Returns the new page.
fn spawn(doc: &mut Document, copy: Copy, kids: &[Option<u32>], count: i64) -> ObjRef {
    let template = open(&form());
    let Object::Stream(stream) = &*template.get(r(8)) else { panic!("the template draws stream 8") };
    let content = match copy.content {
        Some(bytes) => Stream::from_raw(Dict::new(), bytes.to_vec()),
        None => stream.clone(),
    };
    let content = doc.add(Object::Stream(content));
    let page = doc.add(Object::Null);
    let mut widget = doc.get(r(7)).as_dict().cloned().unwrap();
    widget.set(b"T".to_vec(), Object::String(PdfString::text("P2.More.Extra")));
    widget.set(b"P".to_vec(), Object::Ref(page));
    let widget = doc.add(Object::Dict(widget));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(r(9)));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Page"));
    d.set(b"Parent".to_vec(), Object::Ref(r(2)));
    d.set(b"MediaBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::Int).to_vec()));
    d.set(b"Contents".to_vec(), Object::Ref(content));
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(copy.annots.unwrap_or(widget))]));
    if let Some(name) = copy.instantiated {
        d.set(b"TemplateInstantiated".to_vec(), Object::name(name));
    }
    doc.set(page, Object::Dict(d));
    let kids: Vec<Object> = kids.iter().map(|k| Object::Ref(k.map_or(page, r))).collect();
    doc.update_dict(r(2), |n| {
        n.set(b"Kids".to_vec(), Object::Array(kids));
        n.set(b"Count".to_vec(), Object::Int(count));
    })
    .unwrap();
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm")
            && let Some(Object::Array(fields)) = form.get_mut(b"Fields")
        {
            fields.push(Object::Ref(widget));
        }
    })
    .unwrap();
    page
}

/// The page added after the two signed ones, as the template draws it.
fn spawned(signed: &[u8], copy: Copy) -> Vec<u8> {
    edit_after(signed, |doc| {
        spawn(doc, copy, &[Some(3), Some(10), None], 3);
    })
}

#[test]
fn a_page_from_a_signed_template_is_permitted_at_levels_2_and_3() {
    for p in [2, 3] {
        let signed = certified(p);
        assert_allowed(&format!("P={p} appended"), &spawned(&signed, Copy::default()));
        assert_allowed(&format!("P={p} named"), &spawned(&signed, Copy { instantiated: Some("More"), ..Copy::default() }));
        let first = edit_after(&signed, |doc| {
            spawn(doc, Copy::default(), &[None, Some(3), Some(10)], 3);
        });
        assert_allowed(&format!("P={p} before the signed pages"), &first);
    }
}

#[test]
fn a_page_from_a_template_is_no_change_level_1_permits() {
    assert_refused("P=1", &spawned(&certified(1), Copy::default()), kind::PAGE_TEMPLATE);
}

#[test]
fn a_page_that_shows_what_no_template_shows_is_a_page_change() {
    let signed = certified(2);
    let forged = spawned(&signed, Copy { content: Some(b"BT /F1 12 Tf 20 20 Td (Forged) Tj ET"), ..Copy::default() });
    assert_refused("other content", &forged, kind::PAGES);
    // The content of `More`, claimed to come from `Other`.
    assert_refused("another template named", &spawned(&signed, Copy { instantiated: Some("Other"), ..Copy::default() }), kind::PAGES);
    // A signed widget listed on the new page as well.
    assert_refused("a signed widget placed on it", &spawned(&signed, Copy { annots: Some(r(7)), ..Copy::default() }), kind::PAGES);
}

#[test]
fn reordering_removing_miscounting_or_turning_pages_beside_a_template_page_is_a_page_change() {
    let signed = certified(2);
    let tree = |kids: &'static [Option<u32>], count: i64| {
        edit_after(&signed, move |doc| {
            spawn(doc, Copy::default(), kids, count);
        })
    };
    assert_refused("signed pages reordered", &tree(&[Some(10), Some(3), None], 3), kind::PAGES);
    assert_refused("a signed page removed", &tree(&[Some(3), None], 2), kind::PAGES);
    assert_refused("miscounted", &tree(&[Some(3), Some(10), None], 4), kind::PAGES);
    assert_refused("listed twice", &tree(&[Some(3), Some(10), None, None], 4), kind::PAGES);
    // The node's inherited rotation turns every signed page, while the page is added.
    let turned = edit_after(&signed, |doc| {
        spawn(doc, Copy::default(), &[Some(3), Some(10), None], 3);
        doc.update_dict(r(2), |n| n.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    });
    assert_refused("signed pages turned through the tree", &turned, kind::PAGES);
}

/// A hidden template shown in place: its own page moves into the page tree and its name from
/// `/Templates` to `/Pages` (§12.7.7).
#[test]
fn a_hidden_template_shown_in_place_is_permitted_at_level_2() {
    let signed = certified(2);
    let show = |rename: bool| {
        edit_after(&signed, |doc| {
            doc.update_dict(r(5), |t| {
                t.set(b"Type".to_vec(), Object::name("Page"));
                t.set(b"Parent".to_vec(), Object::Ref(r(2)));
            })
            .unwrap();
            doc.update_dict(r(2), |n| {
                n.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r(3)), Object::Ref(r(10)), Object::Ref(r(5))]));
                n.set(b"Count".to_vec(), Object::Int(3));
            })
            .unwrap();
            let entry = |name: &str, num: u32| [Object::String(PdfString::literal(name.as_bytes().to_vec())), Object::Ref(r(num))];
            let mut hidden = Dict::new();
            hidden.set(b"Names".to_vec(), Object::Array(entry("Other", 11).to_vec()));
            let mut shown = Dict::new();
            shown.set(b"Names".to_vec(), Object::Array(entry(if rename { "Renamed" } else { "More" }, 5).to_vec()));
            let (hidden, shown) = (doc.add(Object::Dict(hidden)), doc.add(Object::Dict(shown)));
            let root = doc.root().unwrap();
            doc.update_dict(root, |c| {
                let mut names = Dict::new();
                names.set(b"Templates".to_vec(), Object::Ref(hidden));
                names.set(b"Pages".to_vec(), Object::Ref(shown));
                c.set(b"Names".to_vec(), Object::Dict(names));
            })
            .unwrap();
        })
    };
    assert_allowed("shown", &show(false));
    // Its name not moved but changed: not the template the signed revision named.
    assert_refused("shown under another name", &show(true), kind::PAGES);
}
