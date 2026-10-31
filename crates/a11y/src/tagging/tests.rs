//! Tagging: each operation on hand-written files (including unusual tree layouts and the cases
//! it must refuse), and random operation sequences that must keep the structure consistent and
//! the pages drawing what they drew.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};
use proptest::prelude::*;

use super::content::PageContent;
use super::*;

/// A PDF from object bodies (object n = bodies[n - 1]; object 1 is the catalog).
fn pdf(bodies: &[String]) -> Document {
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
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

fn r(n: u32) -> ObjRef {
    ObjRef::new(n, 0)
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
const IMAGE: &str =
    "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream";

/// What every page draws, marked content left out.
fn painting(doc: &Document) -> Vec<Vec<Vec<u8>>> {
    pdfcraft_model::pages(doc).iter().map(|p| PageContent::read(doc, p).painting()).collect()
}

fn content_text(doc: &Document, page: usize) -> String {
    let pages = pdfcraft_model::pages(doc);
    let p = &pages[page];
    let c = doc.resolve(p.dict.get(b"Contents").unwrap());
    match &*c {
        Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
        Object::Array(a) => a
            .iter()
            .map(|x| match &*doc.resolve(x) {
                Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn kinds(doc: &Document) -> Vec<(u32, String, usize)> {
    crate::tags(doc).into_iter().map(|t| (t.object.num, t.kind, t.depth)).collect()
}

/// Does any marked-content sequence open and close at different `q` depths, or outlive the
/// state saved around its start?
fn straddles(doc: &Document) -> bool {
    pdfcraft_model::pages(doc).iter().any(|p| {
        let content = PageContent::read(doc, p);
        let mut depth = 0i64;
        let mut open: Vec<i64> = Vec::new();
        for op in &content.ops {
            match op.op.as_slice() {
                b"q" => depth += 1,
                b"Q" => {
                    depth -= 1;
                    if open.last().is_some_and(|d| depth < *d) {
                        return true;
                    }
                }
                b"BMC" | b"BDC" => open.push(depth),
                b"EMC" if open.pop().is_some_and(|d| d != depth) => return true,
                _ => {}
            }
        }
        false
    })
}

fn consistent(doc: &Document) {
    assert_eq!(structure_problems(doc), Vec::<String>::new());
}

/// Save incrementally and open again.
fn reopened(doc: &Document) -> Document {
    let bytes = pdfcraft_cos::write_incremental(doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn saved(doc: &Document) -> Vec<u8> {
    pdfcraft_cos::write_incremental(doc, &pdfcraft_cos::SaveOptions::default()).unwrap()
}

/// An untagged page: a heading, a paragraph, a rule and an image.
fn untagged() -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R >> >> >>"
            .into(),
        stream(
            "BT /F1 18 Tf 20 260 Td (Title) Tj ET\n\
             BT /F1 10 Tf 20 230 Td (First line) Tj 0 -12 Td (Second line) Tj ET\n\
             0 0 1 rg 20 200 260 2 re f\n\
             q 100 0 0 50 20 120 cm /Im Do Q",
        ),
        FONT.into(),
        IMAGE.into(),
    ])
}

/// A tagged page: Document › [P (MCID 0), Sect › [P (MCID 1), Figure (MCID 2)]], an artifact,
/// untagged text and an untagged link.
fn tagged() -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 7 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Annots [14 0 R] \
         /Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R >> >> >>"
            .into(),
        stream(
            "/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Intro) Tj ET EMC\n\
             /P << /MCID 1 >> BDC BT /F1 12 Tf 20 230 Td (Body) Tj ET EMC\n\
             /Figure << /MCID 2 >> BDC q 100 0 0 50 20 120 cm /Im Do Q EMC\n\
             /Artifact BMC BT /F1 8 Tf 20 20 Td (Page 1) Tj ET EMC\n\
             BT /F1 12 Tf 20 90 Td (Loose text) Tj ET",
        ),
        FONT.into(),
        IMAGE.into(),
        "<< /Type /StructTreeRoot /K 8 0 R /ParentTree 13 0 R /ParentTreeNextKey 1 >>".into(),
        "<< /Type /StructElem /S /Document /P 7 0 R /K [9 0 R 10 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 8 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /Sect /P 8 0 R /Pg 3 0 R /K [11 0 R 12 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 10 0 R /K 1 >>".into(),
        "<< /Type /StructElem /S /Figure /P 10 0 R /K 2 >>".into(),
        "<< /Nums [0 [9 0 R 11 0 R 12 0 R]] >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [20 85 90 100] /Border [0 0 0] >>".into(),
    ])
}

#[test]
fn the_fixtures_are_consistent() {
    consistent(&tagged());
    consistent(&untagged());
}

#[test]
fn page_tagging_lists_units_marks_and_numbered_regions() {
    let doc = tagged();
    let page = page_tagging(&doc, 0).unwrap();
    let marks: Vec<UnitMark> = page.units.iter().map(|u| u.mark).collect();
    assert_eq!(
        marks,
        [
            UnitMark::Tagged { mcid: 0, tag: r(9) },
            UnitMark::Tagged { mcid: 1, tag: r(11) },
            UnitMark::Tagged { mcid: 2, tag: r(12) },
            UnitMark::Artifact,
            UnitMark::Untagged,
        ]
    );
    assert_eq!(page.units[0].text, "Intro");
    assert_eq!(page.units[2].kind, UnitKind::Image);
    assert_eq!(page.units[2].bbox, Some([20.0, 120.0, 120.0, 170.0]));
    let regions: Vec<(usize, u32, &str)> = page.regions.iter().map(|g| (g.order, g.tag.num, g.kind.as_str())).collect();
    assert_eq!(regions, [(1, 9, "P"), (2, 11, "P"), (3, 12, "Figure")]);
    assert_eq!(page.annotations.len(), 1);
    assert_eq!((page.annotations[0].subtype.as_str(), page.annotations[0].tag), ("Link", None));
    assert!(page.units[4].is_untagged() && !page.units[0].is_untagged());
    assert_eq!(page.read_only, None);
    // Text boxes come from the glyphs: Helvetica "Intro" at 12 pt from x = 20.
    let b = page.units[0].bbox.unwrap();
    assert!((b[0] - 20.0).abs() < 0.01 && b[2] > 40.0 && b[2] < 60.0 && b[1] < 260.0 && b[3] > 268.0, "{b:?}");
}

#[test]
fn tag_boxes_and_tag_at_go_both_ways() {
    let doc = tagged();
    // The Sect's box covers its paragraph and figure.
    let boxes = tag_boxes(&doc, r(10), 0);
    assert_eq!(boxes.len(), 2);
    assert_eq!(tag_at(&doc, 0, 50.0, 140.0), Some(r(12)));
    assert_eq!(tag_at(&doc, 0, 25.0, 263.0), Some(r(9)));
    assert_eq!(tag_at(&doc, 0, 280.0, 280.0), None);
    assert!(tag_boxes(&doc, r(5), 0).is_empty(), "not a tag");
}

#[test]
fn tagging_untagged_content_creates_the_tree_and_keeps_the_page() {
    let mut doc = untagged();
    let before = painting(&doc);
    let units = page_tagging(&doc, 0).unwrap().units;
    assert_eq!(units.len(), 5, "title, two lines, the rule and the image");
    let h1 = tag_selection(&mut doc, 0, &Selection::Units(vec![0]), "H1", None, None).unwrap();
    let p = tag_selection(&mut doc, 0, &Selection::Units(vec![1, 2]), "P", None, None).unwrap();
    let figure = tag_selection(&mut doc, 0, &Selection::Rect([10.0, 110.0, 130.0, 180.0]), "Figure", Some(" A logo "), None).unwrap();
    // The units now: H1, P, the rule, the figure.
    assert_eq!(page_tagging(&doc, 0).unwrap().units[2].mark, UnitMark::Untagged);
    assert_eq!(content_to_artifact(&mut doc, 0, &Selection::Units(vec![2])).unwrap(), 1);
    consistent(&doc);
    assert_eq!(painting(&doc), before, "the page draws what it drew");
    assert_eq!(kinds(&doc).iter().map(|(_, k, d)| (k.as_str(), *d)).collect::<Vec<_>>(), [("Document", 0), ("H1", 1), ("P", 1), ("Figure", 1)]);
    assert_eq!(crate::tags(&doc)[3].alt.as_deref(), Some("A logo"));
    let text = content_text(&doc, 0);
    assert!(text.contains("/H1 <</MCID 0>> BDC"), "{text}");
    // Both lines are in one text object, so it is wrapped whole, once.
    assert!(text.contains("/P <</MCID 1>> BDC\nBT /F1 10 Tf 20 230 Td"), "{text}");
    assert!(text.contains("/Artifact BMC\n0 0 1 rg 20 200 260 2 re f") || text.contains("/Artifact BMC\n20 200 260 2 re f"), "{text}");
    let page = page_tagging(&doc, 0).unwrap();
    assert!(page.units.iter().all(|u| !u.is_untagged()), "{:?}", page.units);
    assert_eq!(page.regions.iter().map(|g| g.tag).collect::<Vec<_>>(), [h1, p, figure]);
    let catalog = doc.get(doc.root().unwrap());
    assert_eq!(
        catalog.as_dict().unwrap().get(b"MarkInfo").and_then(|m| m.as_dict().and_then(|d| d.get(b"Marked").cloned())),
        Some(Object::Bool(true))
    );
    let again = reopened(&doc);
    consistent(&again);
    assert_eq!(kinds(&again), kinds(&doc));
    assert_eq!(painting(&again), before);
}

#[test]
fn retagging_moves_content_and_drops_the_emptied_tag() {
    let mut doc = tagged();
    let before = painting(&doc);
    let h2 = tag_selection(&mut doc, 0, &Selection::Units(vec![0]), "H2", None, None).unwrap();
    consistent(&doc);
    // The heading took the old paragraph's place, and the paragraph left with its only content.
    assert_eq!(kinds(&doc)[1], (h2.num, "H2".into(), 1));
    assert!(!kinds(&doc).iter().any(|(n, _, _)| *n == 9));
    // The MCID is kept: only ownership changed.
    assert_eq!(page_tagging(&doc, 0).unwrap().units[0].mark, UnitMark::Tagged { mcid: 0, tag: h2 });
    assert_eq!(painting(&doc), before);
    assert_eq!(content_text(&doc, 0), content_text(&tagged(), 0), "no content stream changed");
}

#[test]
fn artifacts_and_loose_text_join_the_structure_next_to_their_neighbours() {
    let mut doc = tagged();
    let before = painting(&doc);
    let note = tag_selection(&mut doc, 0, &Selection::Units(vec![3, 4]), "P", None, None).unwrap();
    consistent(&doc);
    assert_eq!(painting(&doc), before);
    // After the figure's section, the nearest tagged content before them.
    let order: Vec<u32> = kinds(&doc).iter().map(|(n, _, _)| *n).collect();
    assert_eq!(order, [8, 9, 10, 11, 12, note.num]);
    let text = content_text(&doc, 0).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("/P <</MCID 3>> BDC BT /F1 8 Tf") && !text.contains("/Artifact"), "{text}");
    assert!(text.contains("/P <</MCID 4>> BDC BT /F1 12 Tf 20 90 Td (Loose text) Tj ET EMC"), "{text}");
}

#[test]
fn tables_and_lists_are_built_from_the_layout() {
    let mut doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream(
            "BT /F1 10 Tf 20 260 Td (Name) Tj ET BT /F1 10 Tf 150 260 Td (Age) Tj ET\n\
             BT /F1 10 Tf 20 245 Td (Ada) Tj ET BT /F1 10 Tf 150 245 Td (36) Tj ET\n\
             BT /F1 10 Tf 20 150 Td (1.) Tj ET BT /F1 10 Tf 60 150 Td (First) Tj ET\n\
             BT /F1 10 Tf 20 135 Td (2.) Tj ET BT /F1 10 Tf 60 135 Td (Second) Tj ET",
        ),
        FONT.into(),
    ]);
    let before = painting(&doc);
    tag_selection(&mut doc, 0, &Selection::Rect([10.0, 240.0, 290.0, 275.0]), "Table", None, None).unwrap();
    tag_selection(&mut doc, 0, &Selection::Rect([10.0, 130.0, 290.0, 165.0]), "L", None, None).unwrap();
    consistent(&doc);
    assert_eq!(painting(&doc), before);
    let shape: Vec<(String, usize)> = kinds(&doc).into_iter().map(|(_, k, d)| (k, d)).collect();
    let expected: Vec<(&str, usize)> = vec![
        ("Document", 0),
        ("Table", 1),
        ("TR", 2),
        ("TD", 3),
        ("TD", 3),
        ("TR", 2),
        ("TD", 3),
        ("TD", 3),
        ("L", 1),
        ("LI", 2),
        ("Lbl", 3),
        ("LBody", 3),
        ("LI", 2),
        ("Lbl", 3),
        ("LBody", 3),
    ];
    assert_eq!(shape.iter().map(|(k, d)| (k.as_str(), *d)).collect::<Vec<_>>(), expected);
    let cells: Vec<String> = page_tagging(&doc, 0).unwrap().regions.iter().map(|g| format!("{}:{}", g.kind, g.text)).collect();
    assert_eq!(cells, ["TD:Name", "TD:Age", "TD:Ada", "TD:36", "Lbl:1.", "LBody:First", "Lbl:2.", "LBody:Second"]);
}

#[test]
fn new_tags_and_moves_across_parents_keep_parents_and_pages() {
    let mut doc = tagged();
    let before = painting(&doc);
    let sect = new_tag(&mut doc, "Sect", Destination::Into(Some(r(8)))).unwrap();
    consistent(&doc);
    // The figure inherits its page from the old section; moving it keeps that page explicit.
    move_tag_to(&mut doc, r(12), Destination::Into(Some(sect))).unwrap();
    consistent(&doc);
    assert_eq!(doc.get(r(12)).as_dict().unwrap().get(b"Pg").and_then(Object::as_ref), Some(r(3)));
    assert_eq!(doc.get(r(12)).as_dict().unwrap().get(b"P").and_then(Object::as_ref), Some(sect));
    move_tag_to(&mut doc, r(11), Destination::Before(r(9))).unwrap();
    move_tag_to(&mut doc, sect, Destination::At(None, 0)).unwrap();
    consistent(&doc);
    let order: Vec<(u32, usize)> = kinds(&doc).iter().map(|(n, _, d)| (*n, *d)).collect();
    assert_eq!(order, [(sect.num, 0), (12, 1), (8, 0), (11, 1), (9, 1), (10, 1)]);
    assert_eq!(move_tag_to(&mut doc, r(11), Destination::Before(r(9))), Err(TaggingError::AlreadyThere));
    assert_eq!(painting(&doc), before);
    // Reading order follows the tree.
    let regions: Vec<u32> = page_tagging(&doc, 0).unwrap().regions.iter().map(|g| g.tag.num).collect();
    assert_eq!(regions, [12, 11, 9]);
}

#[test]
fn deleting_a_tag_untags_its_content_or_lifts_it_up() {
    let mut lifted = tagged();
    delete_tag(&mut lifted, r(10), true).unwrap();
    consistent(&lifted);
    assert_eq!(kinds(&lifted).iter().map(|(n, _, d)| (*n, *d)).collect::<Vec<_>>(), [(8, 0), (9, 1), (11, 1), (12, 1)]);
    // The children now name the document as parent and keep their page.
    assert_eq!(lifted.get(r(11)).as_dict().unwrap().get(b"P").and_then(Object::as_ref), Some(r(8)));
    assert_eq!(lifted.get(r(11)).as_dict().unwrap().get(b"Pg").and_then(Object::as_ref), Some(r(3)));
    let mut gone = tagged();
    let before = painting(&gone);
    delete_tag(&mut gone, r(10), false).unwrap();
    consistent(&gone);
    assert_eq!(painting(&gone), before, "the content stays on the page");
    let marks: Vec<UnitMark> = page_tagging(&gone, 0).unwrap().units.iter().map(|u| u.mark).collect();
    assert_eq!(marks[1], UnitMark::Orphan { mcid: 1 });
    assert_eq!(marks[2], UnitMark::Orphan { mcid: 2 });
    // Orphaned content can be tagged again; it keeps its MCID.
    let p = tag_selection(&mut gone, 0, &Selection::Units(vec![1]), "P", None, None).unwrap();
    consistent(&gone);
    assert_eq!(page_tagging(&gone, 0).unwrap().units[1].mark, UnitMark::Tagged { mcid: 1, tag: p });
    // Removing the document tag lifts its tags to the root; content directly in a tag can't go there.
    let mut root_level = tagged();
    delete_tag(&mut root_level, r(8), true).unwrap();
    consistent(&root_level);
    assert_eq!(kinds(&root_level).iter().map(|(n, _, d)| (*n, *d)).collect::<Vec<_>>(), [(9, 0), (10, 0), (11, 1), (12, 1)]);
    let copy = root_level.clone();
    assert_eq!(delete_tag(&mut root_level, r(9), true), Err(TaggingError::RootContent));
    assert_eq!(saved(&root_level), saved(&copy));
}

#[test]
fn deleting_a_paragraph_with_content_lifts_the_content_into_its_parent() {
    let mut doc = tagged();
    delete_tag(&mut doc, r(11), true).unwrap();
    consistent(&doc);
    // The section now owns MCID 1 itself.
    assert_eq!(page_tagging(&doc, 0).unwrap().units[1].mark, UnitMark::Tagged { mcid: 1, tag: r(10) });
}

#[test]
fn a_whole_subtree_becomes_an_artifact() {
    // Two content streams, a named property list and a marked-content reference on a page
    // the tag doesn't name.
    let mut doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 8 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R] /StructParents 0 \
         /Resources << /Font << /F1 13 0 R >> /Properties << /M1 << /MCID 1 >> >> >> >>"
            .into(),
        stream("/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Keep) Tj ET EMC"),
        stream("/Span /M1 BDC BT /F1 12 Tf 20 230 Td (Header) Tj ET EMC"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 7 0 R /StructParents 1 /Resources << /Font << /F1 13 0 R >> >> >>".into(),
        stream("/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Running head) Tj ET EMC"),
        "<< /Type /StructTreeRoot /K [9 0 R] /ParentTree 12 0 R >>".into(),
        "<< /Type /StructElem /S /Document /P 8 0 R /K [10 0 R 11 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 9 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /Div /P 9 0 R /Pg 3 0 R /K [1 << /Type /MCR /Pg 6 0 R /MCID 0 >>] >>".into(),
        "<< /Nums [0 [10 0 R 11 0 R] 1 [11 0 R]] >>".into(),
        FONT.into(),
    ]);
    consistent(&doc);
    let before = painting(&doc);
    let untouched = doc.get(r(4));
    assert_eq!(tag_to_artifact(&mut doc, r(11)).unwrap(), 2);
    consistent(&doc);
    assert_eq!(painting(&doc), before);
    assert_eq!(kinds(&doc).iter().map(|(n, _, _)| *n).collect::<Vec<_>>(), [9, 10]);
    let first = content_text(&doc, 0);
    assert!(first.contains("/P << /MCID 0 >> BDC") && first.contains("/Artifact BMC BT /F1 12 Tf 20 230"), "{first}");
    assert!(content_text(&doc, 1).starts_with("/Artifact BMC"), "{}", content_text(&doc, 1));
    // The untouched stream was not rewritten.
    let page = &pdfcraft_model::pages(&doc)[0];
    assert_eq!(page.dict.get(b"Contents"), Some(&Object::Array(vec![Object::Ref(r(4)), Object::Ref(r(5))])));
    assert_eq!(doc.get(r(4)), untouched);
    let units = page_tagging(&doc, 1).unwrap().units;
    assert_eq!(units[0].mark, UnitMark::Artifact);
}

#[test]
fn annotations_get_tags_and_parent_tree_entries() {
    let mut doc = tagged();
    let link = tag_annotation(&mut doc, 0, r(14), None).unwrap();
    consistent(&doc);
    let tag = doc.get(link);
    assert_eq!(tag.as_dict().unwrap().name(b"S"), Some(&b"Link"[..]));
    assert_eq!(doc.get(r(14)).as_dict().unwrap().int(b"StructParent"), Some(1));
    assert_eq!(doc.get(r(7)).as_dict().unwrap().int(b"ParentTreeNextKey"), Some(2));
    assert_eq!(page_tagging(&doc, 0).unwrap().annotations[0].tag, Some(link));
    assert_eq!(tag_annotation(&mut doc, 0, r(14), None), Err(TaggingError::AlreadyTagged));
    // Its tag holds an annotation, so it can't become an artifact; deleting it untags the link.
    assert_eq!(tag_to_artifact(&mut doc, link), Err(TaggingError::Annotation));
    delete_tag(&mut doc, link, false).unwrap();
    consistent(&doc);
    assert!(!doc.get(r(14)).as_dict().unwrap().contains(b"StructParent"));
    assert_eq!(page_tagging(&doc, 0).unwrap().annotations[0].tag, None);
}

#[test]
fn a_new_key_is_never_one_a_page_already_holds() {
    // The page holds /StructParents 0, but the parent tree lists nothing and the root has no
    // /ParentTreeNextKey: the link must not get key 0 too.
    let mut doc = tagged();
    doc.update_dict(r(13), |d| d.set(b"Nums".to_vec(), Object::Array(Vec::new()))).unwrap();
    doc.update_dict(r(7), |d| {
        d.remove(b"ParentTreeNextKey");
    })
    .unwrap();
    tag_annotation(&mut doc, 0, r(14), None).unwrap();
    assert_eq!(doc.get(r(14)).as_dict().unwrap().int(b"StructParent"), Some(1));
}

#[test]
fn an_object_two_tags_own_is_reported_whatever_tag_comes_first() {
    // The link is referenced from an inline Link tag, then from an indirect one.
    let mut doc = tagged();
    let dict = |entries: Vec<(&str, Object)>| {
        let mut d = pdfcraft_cos::Dict::new();
        for (k, v) in entries {
            d.set(k.as_bytes().to_vec(), v);
        }
        Object::Dict(d)
    };
    let objr = || dict(vec![("Type", Object::name("OBJR")), ("Obj", Object::Ref(r(14)))]);
    let indirect = doc.add(dict(vec![("Type", Object::name("StructElem")), ("S", Object::name("Link")), ("P", Object::Ref(r(8))), ("K", objr())]));
    let inline = dict(vec![("S", Object::name("Link")), ("K", objr())]);
    doc.update_dict(r(8), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(r(9)), Object::Ref(r(10)), inline, Object::Ref(indirect)])))
        .unwrap();
    assert!(structure_problems(&doc).iter().any(|p| p.contains("is owned by 2 tags")), "{:?}", structure_problems(&doc));
}

/// The same page as [`tagged`] with an unusual layout: a parent tree with intermediate nodes,
/// limits and indirect arrays, stored directly in the root; the root's /K a lone reference;
/// marked-content references instead of integers; and no /ParentTreeNextKey.
fn unusual() -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot << /Type /StructTreeRoot /K 8 0 R /ParentTree << /Kids [13 0 R 15 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 16 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> /XObject << /Im 6 0 R >> >> >>"
            .into(),
        stream(
            "/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Intro) Tj ET EMC\n\
             /P << /MCID 1 >> BDC BT /F1 12 Tf 20 230 Td (Body) Tj ET EMC\n\
             /Figure << /MCID 2 >> BDC q 100 0 0 50 20 120 cm /Im Do Q EMC\n\
             BT /F1 12 Tf 20 90 Td (Loose text) Tj ET",
        ),
        FONT.into(),
        IMAGE.into(),
        "<< /Unused true >>".into(),
        "<< /Type /StructElem /S /Document /K [9 0 R 10 0 R 17 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 8 0 R /K << /Type /MCR /Pg 3 0 R /MCID 0 >> >>".into(),
        "<< /Type /StructElem /S /Sect /P 8 0 R /Pg 3 0 R /K [11 0 R << /S /Div /K 12 0 R >>] >>".into(),
        "<< /Type /StructElem /S /P /P 10 0 R /K [<< /Type /MCR /MCID 1 >>] >>".into(),
        "<< /Type /StructElem /S /Figure /P 10 0 R /K 2 >>".into(),
        "<< /Limits [0 0] /Nums [0 14 0 R] >>".into(),
        "[9 0 R 11 0 R 12 0 R]".into(),
        "<< /Limits [1 1] /Nums [1 [17 0 R]] >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 18 0 R /StructParents 1 /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        "<< /Type /StructElem /S /P /P 8 0 R /Pg 16 0 R /K 0 >>".into(),
        stream("/P << /MCID 0 >> BDC BT /F1 12 Tf 20 260 Td (Page two) Tj ET EMC BT /F1 12 Tf 20 200 Td (More) Tj ET"),
    ])
}

#[test]
fn unusual_layouts_are_read_and_extended_in_place() {
    let doc = unusual();
    // The figure is inside an inline Div: it is listed, but its tag can't be moved.
    let page = page_tagging(&doc, 0).unwrap();
    assert_eq!(page.units[2].mark, UnitMark::Tagged { mcid: 2, tag: r(12) });
    let mut edited = doc.clone();
    assert_eq!(move_tag_to(&mut edited, r(12), Destination::Into(Some(r(8)))), Err(TaggingError::InlineParent(r(12))));
    // Retag, add, and untag on both pages: the existing leaves are edited and widened.
    let before = painting(&edited);
    let h1 = tag_selection(&mut edited, 0, &Selection::Units(vec![0]), "H1", None, None).unwrap();
    let loose = tag_selection(&mut edited, 0, &Selection::Units(vec![3]), "P", None, None).unwrap();
    let more = tag_selection(&mut edited, 1, &Selection::Units(vec![1]), "P", None, None).unwrap();
    move_tag_to(&mut edited, r(11), Destination::After(more)).unwrap();
    // The document tag takes the page-two paragraph's content, as a reference naming its page.
    delete_tag(&mut edited, r(17), true).unwrap();
    consistent(&edited);
    assert_eq!(painting(&edited), before);
    // The root and the parent tree became indirect objects; the leaf arrays stayed where they were.
    let catalog = edited.get(edited.root().unwrap());
    let root = catalog.as_dict().unwrap().get(b"StructTreeRoot").and_then(Object::as_ref).expect("indirect root");
    let pt = edited.get(root).as_dict().unwrap().get(b"ParentTree").and_then(Object::as_ref).expect("indirect parent tree");
    assert_eq!(edited.get(pt).as_dict().unwrap().get(b"Kids"), Some(&Object::Array(vec![Object::Ref(r(13)), Object::Ref(r(15))])));
    assert_eq!(edited.get(r(14)).as_array().unwrap().iter().filter_map(Object::as_ref).collect::<Vec<_>>(), [h1, r(11), r(12), loose]);
    let leaf = edited.get(r(15));
    let page_two = leaf.as_dict().unwrap().get(b"Nums").unwrap().as_array().unwrap()[1].as_array().unwrap().clone();
    assert_eq!(page_two, [Object::Ref(r(8)), Object::Ref(more)]);
    let again = reopened(&edited);
    consistent(&again);
    assert_eq!(kinds(&again), kinds(&edited));
}

#[test]
fn a_new_page_key_goes_into_the_last_leaf_and_widens_its_limits() {
    let mut doc = unusual();
    // Page 2 loses its key: tagging there allocates a fresh one.
    doc.update_dict(r(16), |d| {
        d.remove(b"StructParents");
    })
    .unwrap();
    doc.update_dict(r(17), |d| d.set(b"K".to_vec(), Object::Array(Vec::new()))).unwrap();
    let p = tag_selection(&mut doc, 1, &Selection::Units(vec![1]), "P", None, None).unwrap();
    consistent(&doc);
    assert_eq!(doc.get(r(16)).as_dict().unwrap().int(b"StructParents"), Some(2));
    let leaf = doc.get(r(15));
    let leaf = leaf.as_dict().unwrap();
    assert_eq!(leaf.get(b"Limits"), Some(&Object::Array(vec![Object::Int(1), Object::Int(2)])));
    let nums = leaf.get(b"Nums").unwrap().as_array().unwrap();
    assert_eq!(nums.len(), 4);
    assert_eq!(nums[2], Object::Int(2));
    assert_eq!(nums[3].as_array().unwrap()[1], Object::Ref(p));
}

#[test]
fn unsafe_requests_are_refused_and_change_nothing() {
    fn refused(doc: &Document, edit: impl FnOnce(&mut Document) -> Result<(), TaggingError>, expected: TaggingError) {
        let mut copy = doc.clone();
        assert_eq!(edit(&mut copy), Err(expected));
        assert_eq!(saved(&copy), saved(doc), "a refused edit changes nothing");
    }
    let doc = tagged();
    refused(&doc, |d| move_tag_to(d, r(10), Destination::Into(Some(r(11)))), TaggingError::Cycle);
    refused(&doc, |d| move_tag_to(d, r(10), Destination::Into(Some(r(10)))), TaggingError::Cycle);
    refused(&doc, |d| move_tag_to(d, r(5), Destination::Into(None)), TaggingError::NotATag(r(5)));
    refused(&doc, |d| move_tag_to(d, r(9), Destination::At(Some(r(10)), 9)), TaggingError::BadIndex(9));
    refused(&doc, |d| new_tag(d, "Banner", Destination::Into(None)).map(|_| ()), TaggingError::InvalidType("Banner".into()));
    refused(&doc, |d| new_tag(d, "bad type", Destination::Into(None)).map(|_| ()), TaggingError::InvalidType("bad type".into()));
    refused(&doc, |d| tag_selection(d, 0, &Selection::Units(vec![9]), "P", None, None).map(|_| ()), TaggingError::BadUnit(9));
    refused(&doc, |d| tag_selection(d, 0, &Selection::Rect([0.0, 0.0, 1.0, 1.0]), "P", None, None).map(|_| ()), TaggingError::NothingSelected);
    refused(&doc, |d| tag_selection(d, 3, &Selection::Units(vec![0]), "P", None, None).map(|_| ()), TaggingError::NoPage(4));
    refused(&doc, |d| tag_annotation(d, 0, r(9), None).map(|_| ()), TaggingError::NotAnAnnotation(1));
    // A tag listed twice.
    let mut shared = tagged();
    shared.update_dict(r(8), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(r(9)), Object::Ref(r(10)), Object::Ref(r(9))]))).unwrap();
    refused(&shared, |d| delete_tag(d, r(9), false), TaggingError::Shared(r(9)));
    refused(&shared, |d| tag_to_artifact(d, r(9)).map(|_| ()), TaggingError::Shared(r(9)));
    refused(
        &shared,
        |d| tag_selection(d, 0, &Selection::Units(vec![0]), "H1", None, None).map(|_| ()),
        TaggingError::UnitNotEditable { unit: 0, reason: TaggingError::Shared(r(9)).to_string() },
    );
    // A role-mapped type is fine; an unmapped one is not.
    let mut mapped = tagged();
    let mut role_map = pdfcraft_cos::Dict::new();
    role_map.set(b"Banner".to_vec(), Object::name("Div"));
    mapped.update_dict(r(7), |d| d.set(b"RoleMap".to_vec(), Object::Dict(role_map))).unwrap();
    assert!(new_tag(&mut mapped, "Banner", Destination::Into(None)).is_ok());
    consistent(&mapped);
    // A page whose content can't be decoded exactly.
    let broken = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".into(),
        "<< /Length 8 /Filter /FlateDecode >>\nstream\nnotflate\nendstream".into(),
    ]);
    let mut copy = broken.clone();
    assert!(matches!(
        content_to_artifact(&mut copy, 0, &Selection::Units(vec![0])),
        Err(TaggingError::PageNotEditable { .. }) | Err(TaggingError::BadUnit(0))
    ));
    assert_eq!(saved(&copy), saved(&broken));
    // Unbalanced marked content.
    let unbalanced = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".into(),
        stream("0 0 10 10 re f EMC"),
    ]);
    let mut copy = unbalanced.clone();
    assert!(matches!(tag_selection(&mut copy, 0, &Selection::Units(vec![0]), "P", None, None), Err(TaggingError::PageNotEditable { .. })));
    assert_eq!(saved(&copy), saved(&unbalanced));
    // A structure root that is not a dictionary is never replaced.
    let mut bad_root = untagged();
    bad_root.update_dict(r(1), |d| d.set(b"StructTreeRoot".to_vec(), Object::Int(3))).unwrap();
    refused(&bad_root, |d| tag_selection(d, 0, &Selection::Units(vec![0]), "P", None, None).map(|_| ()), TaggingError::BadRoot);
}

const FORM: &str = "/P << /MCID 0 >> BDC 0 0 5 5 re f EMC";

#[test]
fn content_in_a_form_xobject_is_left_alone() {
    let doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>".into(),
        stream("q 1 0 0 1 10 10 cm /Fm Do Q"),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 50 50] /StructParents 0 /Length {} >>\nstream\n{FORM}\nendstream", FORM.len()),
        "<< /Type /StructTreeRoot /K 7 0 R /ParentTree 9 0 R /ParentTreeNextKey 1 >>".into(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K 8 0 R >>".into(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K << /Type /MCR /Stm 5 0 R /MCID 0 >> >>".into(),
        "<< /Nums [0 [8 0 R]] >>".into(),
    ]);
    let page = page_tagging(&doc, 0).unwrap();
    assert_eq!(page.units[0].mark, UnitMark::FormStructure);
    let mut copy = doc.clone();
    assert!(matches!(tag_selection(&mut copy, 0, &Selection::Units(vec![0]), "Figure", None, None), Err(TaggingError::UnitNotEditable { .. })));
    assert_eq!(tag_to_artifact(&mut copy, r(8)), Err(TaggingError::FormContent));
    assert_eq!(saved(&copy), saved(&doc));
    // Deleting its tag clears the form's parent-tree entry.
    delete_tag(&mut copy, r(8), false).unwrap();
    consistent(&copy);
}

#[test]
fn a_content_stream_two_pages_share_is_copied_not_changed() {
    let mut doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>".into(),
        stream("BT /F1 12 Tf 20 260 Td (Same) Tj ET"),
        FONT.into(),
    ]);
    let shared = doc.get(r(5));
    tag_selection(&mut doc, 0, &Selection::Units(vec![0]), "P", None, None).unwrap();
    consistent(&doc);
    assert_eq!(doc.get(r(5)), shared, "the shared stream is untouched");
    assert!(content_text(&doc, 0).contains("MCID 0"));
    assert!(!content_text(&doc, 1).contains("MCID"));
    assert_eq!(page_tagging(&doc, 1).unwrap().units[0].mark, UnitMark::Untagged);
}

#[test]
fn a_parent_tree_without_the_pages_entry_is_rebuilt_and_noted() {
    let mut doc = tagged();
    doc.update_dict(r(13), |d| d.set(b"Nums".to_vec(), Object::Array(Vec::new()))).unwrap();
    assert!(!structure_problems(&doc).is_empty());
    tag_selection(&mut doc, 0, &Selection::Units(vec![4]), "P", None, None).unwrap();
    consistent(&doc);
    assert!(doc.repair_log().iter().any(|l| l.contains("parent tree")), "{:?}", doc.repair_log());
}

#[test]
fn text_objects_are_split_only_where_they_must_be() {
    // One text object holding two lines: tagging one line wraps just its operator, inside BT.
    let mut doc = untagged();
    tag_selection(&mut doc, 0, &Selection::Units(vec![1]), "P", None, None).unwrap();
    tag_selection(&mut doc, 0, &Selection::Units(vec![2]), "P", None, None).unwrap();
    consistent(&doc);
    let text = content_text(&doc, 0);
    assert!(text.contains("Td \n/P <</MCID 0>> BDC\n(First line) Tj\nEMC\n 0 -12 Td"), "{text}");
    assert_eq!(painting(&doc), painting(&untagged()));
}

#[test]
fn marked_content_never_straddles_a_saved_graphics_state() {
    let mut doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        // Two paths, the first inside a saved state; a text object that restores a state it
        // didn't save (invalid, but drawn by viewers).
        stream("q 0.5 g 10 10 50 50 re f Q 70 10 50 50 re f\nq BT /F1 12 Tf 20 200 Td (Inside) Tj Q ET"),
        FONT.into(),
    ]);
    assert!(!straddles(&doc));
    let before = painting(&doc);
    tag_selection(&mut doc, 0, &Selection::Units(vec![0, 1]), "Figure", Some("Two squares"), None).unwrap();
    tag_selection(&mut doc, 0, &Selection::Units(vec![2]), "P", None, None).unwrap();
    consistent(&doc);
    assert_eq!(painting(&doc), before);
    assert!(!straddles(&doc), "{}", content_text(&doc, 0));
    let text = content_text(&doc, 0);
    // One sequence per path, and the text wrapped inside its text object.
    assert_eq!(text.matches("/Figure <<").count(), 2, "{text}");
    assert!(text.contains("Td \n/P <</MCID 2>> BDC\n(Inside) Tj\nEMC\n Q ET"), "{text}");
    // A path that saves a state it restores only after painting can't be wrapped at all.
    let crossing = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".into(),
        stream("10 10 m q 50 50 l S Q"),
    ]);
    let mut copy = crossing.clone();
    assert!(matches!(
        tag_selection(&mut copy, 0, &Selection::Units(vec![0]), "Figure", None, None),
        Err(TaggingError::UnitNotEditable { unit: 0, .. })
    ));
    assert_eq!(saved(&copy), saved(&crossing));
}

#[test]
fn an_artifact_never_swallows_another_tags_content() {
    let doc = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("/P << /MCID 0 >> BDC BT /F1 12 Tf 20 200 Td (Outer ) Tj /Span << /MCID 1 >> BDC (inner) Tj EMC ET EMC"),
        FONT.into(),
        "<< /Type /StructTreeRoot /K 7 0 R /ParentTree 10 0 R /ParentTreeNextKey 1 >>".into(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R 9 0 R] >>".into(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
        "<< /Type /StructElem /S /Span /P 7 0 R /Pg 3 0 R /K 1 >>".into(),
        "<< /Nums [0 [8 0 R 9 0 R]] >>".into(),
    ]);
    consistent(&doc);
    let page = page_tagging(&doc, 0).unwrap();
    assert!(page.units[0].nested);
    let mut copy = doc.clone();
    assert_eq!(tag_to_artifact(&mut copy, r(8)), Err(TaggingError::NestedContent(1)));
    assert!(matches!(content_to_artifact(&mut copy, 0, &Selection::Units(vec![0])), Err(TaggingError::UnitNotEditable { unit: 0, .. })));
    assert!(matches!(tag_selection(&mut copy, 0, &Selection::Units(vec![0]), "H1", None, None), Err(TaggingError::UnitNotEditable { unit: 0, .. })));
    assert_eq!(saved(&copy), saved(&doc));
    // The inner tag on its own can go.
    tag_to_artifact(&mut copy, r(9)).unwrap();
    consistent(&copy);
}

// ---------------------------------------------------------------------------------------------
// Random operation sequences

#[derive(Clone, Debug)]
enum Item {
    /// Tagged content of kind 0 P, 1 Figure, 2 H1, under section `group` (0: the document).
    Tagged {
        kind: u8,
        group: u8,
    },
    Text,
    Path,
    Artifact,
    Image,
}

#[derive(Clone, Debug)]
struct Spec {
    pages: Vec<Vec<Item>>,
    /// 0: flat /Nums with direct arrays, 1: indirect arrays, 2: /Kids with /Limits, 3: a direct parent tree.
    layout: u8,
    single_root: bool,
    mcr: bool,
    next_key: bool,
    links: bool,
    /// Sections per page carry `/Pg`, and their paragraphs inherit it.
    inherit: bool,
}

fn item() -> impl Strategy<Value = Item> {
    prop_oneof![
        3 => (0u8..3, 0u8..3).prop_map(|(kind, group)| Item::Tagged { kind, group }),
        2 => Just(Item::Text),
        1 => Just(Item::Path),
        1 => Just(Item::Artifact),
        1 => Just(Item::Image),
    ]
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        prop::collection::vec(prop::collection::vec(item(), 0..7), 1..4),
        0u8..4,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(pages, layout, single_root, mcr, next_key, links, inherit)| Spec {
            pages,
            layout,
            single_root,
            mcr,
            next_key,
            links,
            inherit,
        })
}

/// Build a consistent document for a spec.
fn build(spec: &Spec) -> Document {
    let mut objects: Vec<String> = vec![String::new(); 4];
    let add = |objects: &mut Vec<String>, body: String| {
        objects.push(body);
        objects.len() as u32
    };
    // 1 catalog, 2 pages, 3 font, 4 image.
    objects[2] = FONT.into();
    objects[3] = IMAGE.into();
    let tagged_any = spec.pages.iter().flatten().any(|i| matches!(i, Item::Tagged { .. }));
    let root = if tagged_any { add(&mut objects, String::new()) } else { 0 };
    let document = if tagged_any { add(&mut objects, String::new()) } else { 0 };
    // Sections: two shared ones, or (inheriting pages) two per page that carry the page.
    let mut sections: Vec<(u32, Option<u32>, Vec<u32>)> = Vec::new();
    let mut section_at: std::collections::HashMap<(usize, u8), usize> = std::collections::HashMap::new();
    let mut doc_kids: Vec<u32> = Vec::new();
    let mut page_refs = Vec::new();
    let mut arrays: Vec<(usize, Vec<u32>)> = Vec::new();
    for (p, items) in spec.pages.iter().enumerate() {
        let page = add(&mut objects, String::new());
        page_refs.push(page);
        let mut ops = Vec::new();
        let mut owners = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let y = 280 - (i as i32) * 35;
            match item {
                Item::Tagged { kind, group } => {
                    let mcid = owners.len();
                    let (s, body) = match kind {
                        1 => ("Figure", format!("q 40 0 0 20 20 {y} cm /Im Do Q")),
                        2 => ("H1", format!("BT /F1 16 Tf 20 {y} Td (Head {p}.{i}) Tj ET")),
                        _ => ("P", format!("BT /F1 10 Tf 20 {y} Td (Para {p}.{i}) Tj ET")),
                    };
                    ops.push(format!("/{s} << /MCID {mcid} >> BDC {body} EMC"));
                    let section = (*group != 0).then(|| {
                        let slot = (if spec.inherit { p } else { 0 }, *group);
                        match section_at.get(&slot) {
                            Some(i) => *i,
                            None => {
                                let obj = add(&mut objects, String::new());
                                sections.push((obj, spec.inherit.then_some(page), Vec::new()));
                                doc_kids.push(obj);
                                section_at.insert(slot, sections.len() - 1);
                                sections.len() - 1
                            }
                        }
                    });
                    let parent = section.map_or(document, |i| sections[i].0);
                    let k = if spec.mcr { format!("<< /Type /MCR /Pg {page} 0 R /MCID {mcid} >>") } else { mcid.to_string() };
                    let inherits = spec.inherit && section.is_some();
                    let pg = if spec.mcr || inherits { String::new() } else { format!("/Pg {page} 0 R") };
                    let element = add(&mut objects, format!("<< /Type /StructElem /S /{s} /P {parent} 0 R {pg} /K {k} >>"));
                    match section {
                        Some(i) => sections[i].2.push(element),
                        None => doc_kids.push(element),
                    }
                    owners.push(element);
                }
                Item::Text => ops.push(format!("BT /F1 10 Tf 20 {y} Td (Text {p}.{i}) Tj ET")),
                Item::Path => ops.push(format!("0.5 g 20 {y} 200 3 re f")),
                Item::Artifact => ops.push(format!("/Artifact BMC 0 g 20 {y} 100 1 re f EMC")),
                Item::Image => ops.push(format!("q 30 0 0 15 150 {y} cm /Im Do Q")),
            }
        }
        let content = add(&mut objects, stream(&ops.join("\n")));
        let annots = if spec.links {
            let link = add(&mut objects, format!("<< /Type /Annot /Subtype /Link /Rect [200 {} 280 {}] >>", 10 + p, 30 + p));
            format!("/Annots [{link} 0 R]")
        } else {
            String::new()
        };
        let key = if owners.is_empty() { String::new() } else { format!("/StructParents {p}") };
        objects[page as usize - 1] = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents {content} 0 R {key} {annots} /Resources << /Font << /F1 3 0 R >> /XObject << /Im 4 0 R >> >> >>"
        );
        if !owners.is_empty() {
            arrays.push((p, owners));
        }
    }
    let kids: Vec<String> = page_refs.iter().map(|p| format!("{p} 0 R")).collect();
    objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), page_refs.len());
    if !tagged_any {
        objects[0] = "<< /Type /Catalog /Pages 2 0 R >>".into();
        return pdf(&objects);
    }
    let list = |v: &[u32]| v.iter().map(|x| format!("{x} 0 R")).collect::<Vec<_>>().join(" ");
    for (sect, page, kids) in &sections {
        let pg = page.map(|p| format!("/Pg {p} 0 R")).unwrap_or_default();
        objects[*sect as usize - 1] = format!("<< /Type /StructElem /S /Sect /P {document} 0 R {pg} /K [{}] >>", list(kids));
    }
    objects[document as usize - 1] = format!("<< /Type /StructElem /S /Document /P {root} 0 R /K [{}] >>", list(&doc_kids));
    let value = |objects: &mut Vec<String>, owners: &[u32]| -> String {
        if spec.layout == 1 {
            let a = add(objects, format!("[{}]", list(owners)));
            format!("{a} 0 R")
        } else {
            format!("[{}]", list(owners))
        }
    };
    let parent_tree = match spec.layout {
        2 => {
            let mut leaves = Vec::new();
            for (p, owners) in &arrays {
                let v = value(&mut objects, owners);
                leaves.push(add(&mut objects, format!("<< /Limits [{p} {p}] /Nums [{p} {v}] >>")));
            }
            if leaves.is_empty() {
                leaves.push(add(&mut objects, "<< /Limits [0 0] /Nums [] >>".into()));
            }
            format!("<< /Kids [{}] >>", list(&leaves))
        }
        _ => {
            let mut nums = Vec::new();
            for (p, owners) in &arrays {
                let v = value(&mut objects, owners);
                nums.push(format!("{p} {v}"));
            }
            format!("<< /Nums [{}] >>", nums.join(" "))
        }
    };
    let parent_tree = if spec.layout == 3 { parent_tree } else { format!("{} 0 R", add(&mut objects, parent_tree)) };
    let k = if spec.single_root { format!("{document} 0 R") } else { format!("[{document} 0 R]") };
    let next = if spec.next_key { format!("/ParentTreeNextKey {}", spec.pages.len()) } else { String::new() };
    objects[root as usize - 1] = format!("<< /Type /StructTreeRoot /K {k} /ParentTree {parent_tree} {next} >>");
    objects[0] = format!("<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot {root} 0 R >>");
    pdf(&objects)
}

#[derive(Clone, Debug)]
enum Step {
    Tag { page: usize, picks: Vec<usize>, kind: u8 },
    Artifact { page: usize, picks: Vec<usize> },
    New { kind: u8, parent: usize },
    Move { tag: usize, anchor: usize, how: u8 },
    Delete { tag: usize, keep: bool },
    ToArtifact { tag: usize },
    Annot { page: usize },
}

fn step() -> impl Strategy<Value = Step> {
    let picks = prop::collection::vec(0usize..64, 1..4);
    prop_oneof![
        4 => (0usize..8, picks.clone(), 0u8..7).prop_map(|(page, picks, kind)| Step::Tag { page, picks, kind }),
        2 => (0usize..8, picks).prop_map(|(page, picks)| Step::Artifact { page, picks }),
        2 => (0u8..4, 0usize..64).prop_map(|(kind, parent)| Step::New { kind, parent }),
        3 => (0usize..64, 0usize..64, 0u8..4).prop_map(|(tag, anchor, how)| Step::Move { tag, anchor, how }),
        2 => (0usize..64, any::<bool>()).prop_map(|(tag, keep)| Step::Delete { tag, keep }),
        1 => (0usize..64).prop_map(|tag| Step::ToArtifact { tag }),
        1 => (0usize..8).prop_map(|page| Step::Annot { page }),
    ]
}

fn run(doc: &mut Document, step: &Step) -> Option<Result<(), TaggingError>> {
    let pages = pdfcraft_model::pages(doc).len();
    let tags: Vec<ObjRef> = crate::tags(doc).into_iter().map(|t| t.object).collect();
    let tag = |i: usize| tags.get(i % tags.len().max(1)).copied();
    Some(match step {
        Step::Tag { page, picks, kind } => {
            let page = page % pages;
            let units = page_tagging(doc, page).ok()?.units.len();
            if units == 0 {
                return None;
            }
            let picks = picks.iter().map(|p| p % units).collect();
            let kind = ["P", "H1", "H2", "Figure", "Table", "L", "Span"][usize::from(*kind) % 7];
            tag_selection(doc, page, &Selection::Units(picks), kind, (kind == "Figure").then_some("A picture"), None).map(|_| ())
        }
        Step::Artifact { page, picks } => {
            let page = page % pages;
            let units = page_tagging(doc, page).ok()?.units.len();
            if units == 0 {
                return None;
            }
            content_to_artifact(doc, page, &Selection::Units(picks.iter().map(|p| p % units).collect())).map(|_| ())
        }
        Step::New { kind, parent } => {
            let kind = ["Sect", "Div", "P", "H3"][usize::from(*kind) % 4];
            new_tag(doc, kind, Destination::Into(tag(*parent))).map(|_| ())
        }
        Step::Move { tag: t, anchor, how } => {
            let (t, a) = (tag(*t)?, tag(*anchor)?);
            let dest = match how {
                0 => Destination::Before(a),
                1 => Destination::After(a),
                2 => Destination::Into(Some(a)),
                _ => Destination::At(Some(a), 0),
            };
            move_tag_to(doc, t, dest)
        }
        Step::Delete { tag: t, keep } => delete_tag(doc, tag(*t)?, *keep),
        Step::ToArtifact { tag: t } => tag_to_artifact(doc, tag(*t)?).map(|_| ()),
        Step::Annot { page } => {
            let page = page % pages;
            let annot = page_tagging(doc, page).ok()?.annotations.first()?.object;
            tag_annotation(doc, page, annot, None).map(|_| ())
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// Whatever sequence of edits runs, each one either succeeds and leaves the structure
    /// consistent with every page drawing exactly what it drew, or is refused and changes
    /// nothing; and the result survives an incremental save.
    #[test]
    fn random_edits_keep_the_structure_consistent_and_the_pages_unchanged(spec in spec(), steps in prop::collection::vec(step(), 1..10)) {
        let mut doc = build(&spec);
        prop_assert_eq!(structure_problems(&doc), Vec::<String>::new(), "the fixture is consistent");
        let drawn = painting(&doc);
        for step in &steps {
            let before = saved(&doc);
            match run(&mut doc, step) {
                None => {}
                Some(Ok(())) => {
                    prop_assert_eq!(structure_problems(&doc), Vec::<String>::new(), "after {:?}", step);
                    prop_assert_eq!(&painting(&doc), &drawn, "after {:?}", step);
                    prop_assert!(!straddles(&doc), "after {:?}", step);
                }
                Some(Err(e)) => prop_assert_eq!(saved(&doc), before, "{:?} was refused ({}) but changed the file", step, e),
            }
        }
        let again = reopened(&doc);
        prop_assert_eq!(structure_problems(&again), Vec::<String>::new());
        prop_assert_eq!(kinds(&again), kinds(&doc));
        prop_assert_eq!(painting(&again), drawn);
    }
}
