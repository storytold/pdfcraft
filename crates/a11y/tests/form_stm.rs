//! Marked content in a form XObject, owned through a marked-content reference's `/Stm` (#3221).
//! The form numbers its marked content on its own, so MCID 0 in the form is not MCID 0 on the page.

use pdfcraft_a11y::{figures, mark_decorative, mark_tag_artifact, tag_content, tags};
use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, Stream, write_full};

const PAGE: &str = "/Figure <</MCID 0>> BDC 1 0 0 rg 72 600 60 60 re f EMC\n\
/P <</MCID 1>> BDC BT /F1 14 Tf 72 560 Td (Page paragraph) Tj ET EMC\n\
q 1 0 0 1 300 600 cm /FmF Do Q\n";
const FORM: &str = "/Figure <</MCID 0>> BDC 0 0 1 rg 0 0 60 60 re f EMC";

fn build(objects: &[(u32, String)]) -> Document {
    let mut raw = Vec::from(&b"%PDF-1.7\n"[..]);
    let size = objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
    let mut offsets = std::collections::HashMap::new();
    for (num, body) in objects {
        offsets.insert(*num, raw.len());
        raw.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = raw.len();
    raw.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for num in 1..size {
        match offsets.get(&num) {
            Some(off) => raw.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()),
            None => raw.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    raw.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    Document::open(std::sync::Arc::new(raw)).expect("valid synthetic PDF")
}

/// One page: a red page figure (MCID 0, element 20) and a paragraph (MCID 1, element 21) in the
/// page content, and a blue form figure drawn at (300, 600) from form 12 (MCID 0 in the form,
/// element 22, `/StructParents 1`). `form_kid` is the form figure's `/K`, `form` its content and
/// `parent_tree_form` the parent-tree value under key 1.
fn fixture(form: &str, form_kid: &str, parent_tree_form: &str) -> Document {
    build(&[
        (1, "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 3 0 R /MarkInfo << /Marked true >> /Lang (en) >>".into()),
        (2, "<< /Type /Pages /Kids [10 0 R] /Count 1 >>".into()),
        (3, "<< /Type /StructTreeRoot /K 4 0 R /ParentTree 5 0 R /ParentTreeNextKey 2 >>".into()),
        (4, "<< /Type /StructElem /S /Document /P 3 0 R /K [20 0 R 21 0 R 22 0 R] >>".into()),
        (5, format!("<< /Nums [0 [20 0 R 21 0 R] 1 {parent_tree_form}] >>")),
        (6, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into()),
        (
            10,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 11 0 R /StructParents 0 \
             /Resources << /Font << /F1 6 0 R >> /XObject << /FmF 12 0 R >> >> >>"
                .into(),
        ),
        (11, format!("<< /Length {} >>\nstream\n{PAGE}\nendstream", PAGE.len())),
        (
            12,
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 60 60] /StructParents 1 /Resources << /Font << /F1 6 0 R >> >> /Length {} >>\nstream\n{form}\nendstream",
                form.len()
            ),
        ),
        (20, "<< /Type /StructElem /S /Figure /P 4 0 R /Pg 10 0 R /K 0 /Alt (Page figure) >>".into()),
        (21, "<< /Type /StructElem /S /P /P 4 0 R /Pg 10 0 R /K 1 >>".into()),
        (22, format!("<< /Type /StructElem /S /Figure /P 4 0 R /Pg 10 0 R /K {form_kid} /Alt (Form figure) >>")),
    ])
}

fn stm_figures() -> Document {
    fixture(FORM, "<< /Type /MCR /MCID 0 /Stm 12 0 R /Pg 10 0 R >>", "[22 0 R]")
}

const PAGE_FIGURE: ObjRef = ObjRef::new(20, 0);
const FORM_FIGURE: ObjRef = ObjRef::new(22, 0);
const FORM_OBJ: ObjRef = ObjRef::new(12, 0);

fn decoded(doc: &Document, obj: &Object) -> Vec<u8> {
    match &*doc.resolve(obj) {
        Object::Stream(s) => s.decoded().unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A content stream's marked-content operators, as `(tag, mcid)`.
type Marks = Vec<(String, Option<i64>)>;

/// The marked-content operators of the page content and of the form.
fn marks(doc: &Document) -> (Marks, Marks) {
    let read = |data: Vec<u8>| {
        pdfcraft_content::parse(&data)
            .ops
            .into_iter()
            .filter(|op| op.is("BDC") || op.is("BMC"))
            .map(|op| {
                let tag = String::from_utf8_lossy(op.name(0).unwrap_or_default()).into_owned();
                let mcid = op.operands.get(1).and_then(Object::as_dict).and_then(|d| d.get(b"MCID")).and_then(Object::as_int);
                (tag, mcid)
            })
            .collect::<Vec<_>>()
    };
    let page = pdfcraft_model::pages(doc)[0].dict.clone();
    let contents = page.get(b"Contents").expect("contents").clone();
    let form = page
        .get(b"Resources")
        .and_then(|r| doc.resolve(r).as_dict().cloned())
        .and_then(|r| r.get(b"XObject").and_then(|x| x.as_dict().cloned()))
        .and_then(|x| x.get(b"FmF").cloned())
        .expect("form");
    (read(decoded(doc, &contents)), read(decoded(doc, &form)))
}

fn parent_tree(doc: &Document) -> Vec<Object> {
    let nums = doc.get(ObjRef::new(5, 0)).as_dict().and_then(|d| d.get(b"Nums").cloned()).expect("nums");
    nums.as_array().cloned().unwrap_or_default().iter().map(|o| (*doc.resolve(o)).clone()).collect()
}

fn reopened(doc: &Document) -> Document {
    Document::open(std::sync::Arc::new(write_full(doc, &SaveOptions::default()).expect("saves"))).expect("reopens")
}

/// What every save keeps: the marked content, and the one figure left with its content. A full
/// save renumbers objects, so these checks go by content.
fn assert_only_the_form_figure_became_an_artifact(doc: &Document) {
    let (page, form) = marks(doc);
    assert_eq!(page, [("Figure".into(), Some(0)), ("P".into(), Some(1))], "the page's own marked content stays as it was");
    assert_eq!(form, [("Artifact".into(), None)], "the form's figure is the one that becomes an artifact");
    let listed = figures(doc);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].alt.as_deref(), Some("Page figure"));
    assert_eq!(listed[0].bbox, Some([72.0, 600.0, 132.0, 660.0]), "the page figure keeps its content");
    assert_eq!(tags(doc).iter().filter(|t| t.alt.as_deref() == Some("Form figure")).count(), 0);
}

/// Before saving: the parent tree, by object number, and the form's drawing.
fn assert_the_form_entry_was_cleared(doc: &Document) {
    assert_only_the_form_figure_became_an_artifact(doc);
    assert_eq!(figures(doc)[0].obj, PAGE_FIGURE);
    let tree = parent_tree(doc);
    assert_eq!(tree[1], Object::Array(vec![Object::Ref(PAGE_FIGURE), Object::Ref(ObjRef::new(21, 0))]), "the page's entry is untouched");
    assert_eq!(tree[3], Object::Array(vec![Object::Null]), "the form's entry no longer names the removed figure");
    assert!(!tags(doc).iter().any(|t| t.object == FORM_FIGURE));
    // The drawing itself is unchanged.
    let after = pdfcraft_content::parse(&decoded(doc, &Object::Ref(FORM_OBJ))).ops;
    assert!(after.iter().any(|op| op.is("re") && op.nums::<4>() == Some([0.0, 0.0, 60.0, 60.0])));
}

#[test]
fn decorative_on_a_form_figure_changes_the_form_not_the_page() {
    let mut doc = stm_figures();
    mark_decorative(&mut doc, FORM_FIGURE).expect("decorative");
    assert_the_form_entry_was_cleared(&doc);
    assert_only_the_form_figure_became_an_artifact(&reopened(&doc));
}

#[test]
fn artifact_on_a_form_tag_changes_the_form_not_the_page() {
    let mut doc = stm_figures();
    mark_tag_artifact(&mut doc, FORM_FIGURE).expect("artifact");
    assert_the_form_entry_was_cleared(&doc);
    assert_only_the_form_figure_became_an_artifact(&reopened(&doc));
}

#[test]
fn figures_locate_a_form_figure_where_its_form_draws_it() {
    let doc = stm_figures();
    let listed = figures(&doc);
    let page = listed.iter().find(|f| f.obj == PAGE_FIGURE).expect("page figure");
    let form = listed.iter().find(|f| f.obj == FORM_FIGURE).expect("form figure");
    assert_eq!(page.bbox, Some([72.0, 600.0, 132.0, 660.0]));
    assert_eq!(form.bbox, Some([300.0, 600.0, 360.0, 660.0]), "the blue square at the form's placement");
}

#[test]
fn figures_follow_the_form_matrix() {
    let mut doc = stm_figures();
    let mut form = match &*doc.get(FORM_OBJ) {
        Object::Stream(s) => s.dict.clone(),
        _ => unreachable!("the fixture's form is a stream"),
    };
    form.remove(b"Length");
    form.set(b"Matrix".to_vec(), Object::Array([2, 0, 0, 2, 10, 0].map(Object::Int).to_vec()));
    doc.set(FORM_OBJ, Object::Stream(Stream::flate(form, FORM.as_bytes())));
    let form = figures(&doc).into_iter().find(|f| f.obj == FORM_FIGURE).expect("form figure");
    assert_eq!(form.bbox, Some([310.0, 600.0, 430.0, 720.0]));
}

/// A form that draws itself is walked once, not forever; a form drawn from that form still counts.
#[test]
fn figures_stop_at_a_form_drawing_itself_and_reach_nested_forms() {
    let mut doc = stm_figures();
    let inner = doc.add(Object::Stream(Stream::flate(
        {
            let mut d = pdfcraft_cos::Dict::new();
            d.set(b"Subtype".to_vec(), Object::name("Form"));
            d.set(b"BBox".to_vec(), Object::Array([0, 0, 10, 10].map(Object::Int).to_vec()));
            d
        },
        b"/Figure <</MCID 3>> BDC 0 0 10 10 re f EMC",
    )));
    let mut form = match &*doc.get(FORM_OBJ) {
        Object::Stream(s) => s.dict.clone(),
        _ => unreachable!("the fixture's form is a stream"),
    };
    form.remove(b"Length");
    let mut xobjects = pdfcraft_cos::Dict::new();
    xobjects.set(b"Me".to_vec(), Object::Ref(FORM_OBJ));
    xobjects.set(b"In".to_vec(), Object::Ref(inner));
    let mut res = pdfcraft_cos::Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    form.set(b"Resources".to_vec(), Object::Dict(res));
    doc.set(FORM_OBJ, Object::Stream(Stream::flate(form, format!("{FORM} /Me Do q 1 0 0 1 100 0 cm /In Do Q").as_bytes())));
    // The form figure now also owns MCID 3 of the inner form.
    doc.update_dict(FORM_FIGURE, |d| {
        let mut second = pdfcraft_cos::Dict::new();
        second.set(b"Type".to_vec(), Object::name("MCR"));
        second.set(b"MCID".to_vec(), Object::Int(3));
        second.set(b"Stm".to_vec(), Object::Ref(inner));
        let first = d.get(b"K").cloned().unwrap_or(Object::Null);
        d.set(b"K".to_vec(), Object::Array(vec![first, Object::Dict(second)]));
    })
    .unwrap();
    let form = figures(&doc).into_iter().find(|f| f.obj == FORM_FIGURE).expect("form figure");
    assert_eq!(form.bbox, Some([300.0, 600.0, 410.0, 660.0]), "the blue square and the inner form's square at (400, 600)");
}

#[test]
fn tag_content_reads_a_form_paragraph_from_the_form() {
    let doc = fixture("/P <</MCID 0>> BDC BT /F1 12 Tf 0 10 Td (Form para) Tj ET EMC", "<< /Type /MCR /MCID 0 /Stm 12 0 R /Pg 10 0 R >>", "[22 0 R]");
    let items = tag_content(&doc, FORM_FIGURE).expect("content");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].mcid, 0);
    assert_eq!(items[0].page, Some(0));
    assert_eq!(items[0].stream, Some(FORM_OBJ));
    assert_eq!(items[0].text, "Form para");
    let page = tag_content(&doc, ObjRef::new(21, 0)).expect("page paragraph");
    assert_eq!(page[0].stream, None);
    assert_eq!(page[0].text, "Page paragraph");
}

/// A form figure whose parent-tree entry is the element itself, not an array, as some producers
/// write it: the entry is cleared all the same.
#[test]
fn artifact_clears_a_form_entry_written_as_the_element() {
    let mut doc = fixture(FORM, "<< /Type /MCR /MCID 0 /Stm 12 0 R /Pg 10 0 R >>", "22 0 R");
    mark_tag_artifact(&mut doc, FORM_FIGURE).expect("artifact");
    assert_eq!(parent_tree(&doc)[3], Object::Null);
    let (page, form) = marks(&doc);
    assert_eq!(page, [("Figure".into(), Some(0)), ("P".into(), Some(1))]);
    assert_eq!(form, [("Artifact".into(), None)]);
}

/// The tag owns content both on the page and in a form: everything it owns becomes an artifact.
#[test]
fn artifact_on_a_tag_owning_page_and_form_content_changes_both() {
    let mut doc = fixture(FORM, "[1 << /Type /MCR /MCID 0 /Stm 12 0 R >>]", "[22 0 R]");
    // Element 21 no longer owns the page paragraph: 22 does, along with the form's figure.
    doc.update_dict(ObjRef::new(4, 0), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(PAGE_FIGURE), Object::Ref(FORM_FIGURE)]))).unwrap();
    mark_tag_artifact(&mut doc, FORM_FIGURE).expect("artifact");
    let (page, form) = marks(&doc);
    assert_eq!(page, [("Figure".into(), Some(0)), ("Artifact".into(), None)]);
    assert_eq!(form, [("Artifact".into(), None)]);
}

/// When the form doesn't hold the content its reference names, nothing changes: not the page,
/// not the form, not the tags.
#[test]
fn artifact_refuses_when_the_form_lacks_the_content_and_changes_nothing() {
    let mut doc = fixture(FORM, "[1 << /Type /MCR /MCID 7 /Stm 12 0 R >>]", "[22 0 R]");
    let before = marks(&doc);
    assert_eq!(mark_tag_artifact(&mut doc, FORM_FIGURE), Err(pdfcraft_a11y::ArtifactError::ContentNotFound));
    assert_eq!(marks(&doc), before);
    assert!(tags(&doc).iter().any(|t| t.object == FORM_FIGURE));
}

/// A form damaged partway can't be rewritten without losing what wasn't read.
#[test]
fn decorative_refuses_a_damaged_form() {
    let mut doc = stm_figures();
    let mut dict = match &*doc.get(FORM_OBJ) {
        Object::Stream(s) => s.dict.clone(),
        _ => unreachable!("the fixture's form is a stream"),
    };
    // A stored Flate block holding the form's content, then a block of an invalid type and more
    // drawing that can't be read.
    let len = u16::try_from(FORM.len()).unwrap();
    let mut data = vec![0x78, 0x01, 0x00];
    data.extend_from_slice(&len.to_le_bytes());
    data.extend_from_slice(&(!len).to_le_bytes());
    data.extend_from_slice(FORM.as_bytes());
    data.push(0x07);
    data.extend_from_slice(b"0 g 0 0 10 10 re f");
    dict.remove(b"Length");
    dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
    doc.set(FORM_OBJ, Object::Stream(Stream::from_raw(dict, data)));
    let before = (marks(&doc), doc.get(FORM_OBJ));
    assert_eq!(mark_decorative(&mut doc, FORM_FIGURE), Err(pdfcraft_a11y::AltError::Damaged));
    assert_eq!(mark_tag_artifact(&mut doc, FORM_FIGURE), Err(pdfcraft_a11y::ArtifactError::Damaged));
    assert_eq!((marks(&doc), doc.get(FORM_OBJ)), before);
    assert!(tags(&doc).iter().any(|t| t.object == FORM_FIGURE));
}

/// A `/Stm` naming the page's own content stream is the page's content.
#[test]
fn stm_naming_the_page_stream_is_the_page() {
    let mut doc = fixture(FORM, "<< /Type /MCR /MCID 1 /Stm 11 0 R >>", "[22 0 R]");
    doc.update_dict(ObjRef::new(4, 0), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(PAGE_FIGURE), Object::Ref(FORM_FIGURE)]))).unwrap();
    assert_eq!(tag_content(&doc, FORM_FIGURE).expect("content")[0].text, "Page paragraph");
    mark_tag_artifact(&mut doc, FORM_FIGURE).expect("artifact");
    let (page, form) = marks(&doc);
    assert_eq!(page, [("Figure".into(), Some(0)), ("Artifact".into(), None)]);
    assert_eq!(form, [("Figure".into(), Some(0))]);
}
