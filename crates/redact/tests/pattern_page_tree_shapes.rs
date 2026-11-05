//! Pattern redaction over unusual page trees -- cyclic, deep, wide, inconsistent, packed in
//! object streams, or reached only through `/Parent` -- and over Type 3 fonts reached other than
//! by `Tf` in the scope that paints. Each test reads the saved bytes back.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_redact::{RedactError, apply, marks};
use pdfcraft_testkit::{Pdf, stream_obj};

const SECRET: &[u8] = b"ADV2CELLSECRET";
const TEXT_CELL: &[u8] = b"BT /F1 20 Tf 40 120 Td (ADV2CELLSECRET) Tj ET\n";
const PATTERN: &str =
    "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 500 300] /XStep 500 /YStep 300 /Resources << /Font << /F1 4 0 R >> >>";
const FONT: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";
const RES: &str = "<< /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> >>";
const AREA: [f64; 4] = [30.0, 105.0, 215.0, 150.0];
const CONTENT: &[u8] = b"BT /F1 20 Tf 40 250 Td (PUBLIC) Tj ET\n/Pattern cs /P0 scn 35 110 170 35 re f\n";

fn open(objs: Vec<Vec<u8>>) -> Document {
    Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap()
}

fn mark(doc: &mut Document, page: usize) {
    let shape = Shape::Redact { quads: vec![rect_quad(AREA)], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: "Synthetic test".into() }, &Meta::default()).unwrap();
}

fn decoded_streams_contain(bytes: Vec<u8>, needle: &[u8]) -> bool {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    doc.object_numbers().into_iter().any(|num| {
        let object = doc.try_get(num).unwrap();
        matches!(&*object, Object::Stream(s) if s.decoded().unwrap_or_default().windows(needle.len()).any(|w| w == needle))
    })
}

fn saved_contains(doc: &Document, needle: &[u8]) -> bool {
    [true, false].into_iter().any(|object_streams| {
        let opts = SaveOptions { object_streams, ..SaveOptions::default() };
        decoded_streams_contain(write_full(doc, &opts).unwrap(), needle) || decoded_streams_contain(write_incremental(doc, &opts).unwrap(), needle)
    })
}

/// Round-trip through a full save with object streams, so every dictionary lives in one.
fn packed(doc: &Document) -> Document {
    Document::open(Arc::new(write_full(doc, &SaveOptions { object_streams: true, ..SaveOptions::default() }).unwrap())).unwrap()
}

/// Ok means confidential; Err must be a pattern refusal that leaves the document unchanged.
/// Returns whether it applied.
fn confidential_or_refused(mut doc: Document, page: usize) -> bool {
    mark(&mut doc, page);
    assert!(saved_contains(&doc, SECRET), "the fixture must carry the secret");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    match apply(&mut doc, None) {
        Ok(report) => {
            assert_eq!(report.paths_removed, 1, "{report:?}");
            assert!(!saved_contains(&doc, SECRET), "Apply succeeded but the saved file still holds the pattern cell's text ({report:?})");
            true
        }
        Err(error) => {
            assert!(matches!(error, RedactError::Pattern { .. }), "{error}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert_eq!(marks(&doc).len(), 1);
            false
        }
    }
}

fn catalog() -> Vec<u8> {
    b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()
}

/// The root `/Pages` node carries a `/Parent` to a node outside the `/Kids` tree that names the
/// pattern. A full save follows that `/Parent`, so the node and the cell are written.
#[test]
fn a_node_reached_only_through_the_roots_parent_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Parent 7 0 R /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Kids [] /Count 0 /Resources {RES} >>").into_bytes(),
    ]);
    confidential_or_refused(doc, 0);
}
/// As above, one level down: an intermediate node in `/Kids` whose `/Parent` leads elsewhere.
#[test]
fn a_node_reached_only_through_an_intermediate_parent_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [8 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 8 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Kids [] /Count 0 /Resources {RES} >>").into_bytes(),
        format!("<< /Type /Pages /Parent 7 0 R /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
    ]);
    confidential_or_refused(doc, 0);
}
/// `/Parent` cycles: the root's parent is itself, and the page's chain loops through it.
#[test]
fn a_parent_cycle_is_confidential_or_refused() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Parent 2 0 R /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    confidential_or_refused(doc, 0);
}
/// A page 5,000 levels below the root that holds the pattern (past `resource_source`'s 4,096).
#[test]
fn a_very_deep_tree_is_confidential_or_refused() {
    const DEPTH: usize = 5000;
    let mut objs = vec![
        catalog(),
        format!("<< /Type /Pages /Kids [7 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        format!("<< /Type /Page /Parent {} 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>", 7 + DEPTH - 1).into_bytes(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ];
    for i in 0..DEPTH {
        let num = 7 + i;
        let parent = if i == 0 { 2 } else { num - 1 };
        let kid = if i + 1 == DEPTH { 3 } else { num + 1 };
        objs.push(format!("<< /Type /Pages /Parent {parent} 0 R /Kids [{kid} 0 R] /Count 1 >>").into_bytes());
    }
    confidential_or_refused(doc_or(objs), 0);
}
fn doc_or(objs: Vec<Vec<u8>>) -> Document {
    open(objs)
}
/// The shadowed-root and missing-parent shapes again, with every dictionary in object streams.
#[test]
fn page_tree_shapes_in_object_streams_are_confidential_or_refused() {
    let shadowed = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [7 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 7 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
    ]);
    assert!(confidential_or_refused(packed(&shadowed), 0), "the shadowed root applies");
    let missing = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    confidential_or_refused(packed(&missing), 0);
}
/// The shadowed root's `/Resources` object is also a form's resources, drawn outside the mark on
/// the redacted page: unlinking from the root must not reach through to the form.
#[test]
fn a_root_resource_dictionary_shared_with_a_form_is_confidential_or_refused() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 /Resources 9 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 7 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", b"BT /F1 20 Tf 40 250 Td (PUBLIC) Tj ET\n/Pattern cs /P0 scn 35 110 170 35 re f\nq 1 0 0 1 300 200 cm /Fm Do Q\n"),
        stream_obj(PATTERN, TEXT_CELL),
        b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Resources << /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> /XObject << /Fm 8 0 R >> >> >>".to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 100 50] /Resources 9 0 R", b"0 0 1 rg 0 0 50 20 re f\n"),
        RES.as_bytes().to_vec(),
    ]);
    confidential_or_refused(doc, 0);
}
/// A document of 17,000 pages whose first page names a pattern: only that page has patterns.
/// Redacting it should apply as it does in a short document.
#[test]
fn a_long_document_with_a_pattern_on_one_page_still_applies() {
    const PAGES: usize = 17_000;
    let kids: String = (0..PAGES).map(|i| format!("{} 0 R ", if i == 0 { 3 } else { 7 + i - 1 })).collect();
    let mut objs = vec![
        catalog(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>").into_bytes(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources {RES} >>").into_bytes(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ];
    for _ in 1..PAGES {
        objs.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] >>".to_vec());
    }
    let mut doc = open(objs);
    mark(&mut doc, 0);
    let result = apply(&mut doc, None);
    assert!(result.is_ok(), "a long document refuses: {result:?}");
    assert!(!saved_contains(&doc, SECRET));
}
/// Control for the above: the same with 1,000 pages.
#[test]
fn control_a_shorter_document_with_a_pattern_on_one_page_applies() {
    const PAGES: usize = 1000;
    let kids: String = (0..PAGES).map(|i| format!("{} 0 R ", if i == 0 { 3 } else { 7 + i - 1 })).collect();
    let mut objs = vec![
        catalog(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>").into_bytes(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources {RES} >>").into_bytes(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ];
    for _ in 1..PAGES {
        objs.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] >>".to_vec());
    }
    let mut doc = open(objs);
    mark(&mut doc, 0);
    assert!(apply(&mut doc, None).is_ok());
    assert!(!saved_contains(&doc, SECRET));
}
