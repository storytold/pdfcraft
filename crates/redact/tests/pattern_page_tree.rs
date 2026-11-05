//! Pattern redaction through the page tree and through glyphs: synthetic files whose pattern
//! cells could stay reachable, or whose surviving paint could lose its pattern, after Apply.
//! Each test reads the saved bytes back (full and incremental, with and without object
//! streams) and searches every decoded stream.

use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_redact::{PatternProblem, RedactError, apply, marks};
use pdfcraft_testkit::{Pdf, stream_obj};

const SECRET: &[u8] = b"ADV2CELLSECRET";
const TEXT_CELL: &[u8] = b"BT /F1 20 Tf 40 120 Td (ADV2CELLSECRET) Tj ET\n";
const PATTERN: &str =
    "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 500 300] /XStep 500 /YStep 300 /Resources << /Font << /F1 4 0 R >> >>";
const FONT: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";
const RES: &str = "<< /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> >>";
const AREA: [f64; 4] = [30.0, 105.0, 215.0, 150.0];
/// Public text outside the mark, and a pattern fill wholly under it.
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

/// The needle in any decoded stream, with and without object streams, and in an incremental save.
fn saved_contains(doc: &Document, needle: &[u8]) -> bool {
    [true, false].into_iter().any(|object_streams| {
        let opts = SaveOptions { object_streams, ..SaveOptions::default() };
        decoded_streams_contain(write_full(doc, &opts).unwrap(), needle) || decoded_streams_contain(write_incremental(doc, &opts).unwrap(), needle)
    })
}

/// A successful apply must leave no trace of the cell; a refusal must leave the document as it was.
fn assert_confidential_or_refused(mut doc: Document) {
    mark(&mut doc, 0);
    assert!(saved_contains(&doc, SECRET), "the fixture must carry the secret");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    match apply(&mut doc, None) {
        Ok(report) => {
            assert_eq!(report.paths_removed, 1, "{report:?}");
            assert!(!saved_contains(&doc, SECRET), "Apply succeeded but the saved file still holds the pattern cell's text ({report:?})");
        }
        Err(error) => {
            assert!(matches!(error, RedactError::Pattern { .. }), "{error}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
            assert_eq!(marks(&doc).len(), 1);
        }
    }
}

fn catalog() -> Vec<u8> {
    b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()
}

/// The page inherits its resources, but its `/Parent` does not lead back to the node it inherits
/// from (missing here). Readers resolve inheritance down the `/Kids` tree, so the page draws with
/// the root's `/Pattern`; that root keeps its `/Resources` after the page's copy is pruned.
#[test]
fn a_page_whose_parent_link_is_missing_does_not_keep_the_pruned_cell_in_the_page_tree() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    assert_confidential_or_refused(doc);
}

/// As above with a `/Parent` that names a different, resource-less `/Pages` node.
#[test]
fn a_page_whose_parent_link_disagrees_with_the_tree_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 7 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(),
    ]);
    assert_confidential_or_refused(doc);
}

/// Two ancestors both hold `/Resources` naming the pattern; the page inherits from the nearer one,
/// which is retired once the page gets its own pruned copy. The root's dictionary, which no page
/// inherits, still names the cell.
#[test]
fn a_shadowed_ancestor_resource_dictionary_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [7 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 7 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
    ]);
    // The tree is consistent and no page inherits the root's dictionary any more, so the pruned
    // pattern is unlinked from it and the redaction applies rather than refusing.
    let mut doc = doc;
    mark(&mut doc, 0);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.patterns_removed, 1, "{report:?}");
    assert!(!saved_contains(&doc, SECRET), "the root's shadowed /Resources still names the cell ({report:?})");
}

/// Control: the same shape with only the nearer ancestor holding resources applies cleanly.
#[test]
fn control_a_single_inheriting_ancestor_is_retired_with_the_cell() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [7 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 7 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
    ]);
    assert_confidential_or_refused(doc);
}

/// A hostile chain of tiling patterns, each cell selecting the next, read from a file: the guard
/// must stop with an error (or apply), never overflow the stack.
#[test]
fn a_hostile_chain_of_nested_patterns_is_bounded() {
    const N: usize = 3000;
    let mut objs = vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources {RES} >>").into_bytes(),
        FONT.to_vec(),
        // The fill outside the mark keeps /P0 in use, so the guard must inspect the chain.
        stream_obj("", b"/Pattern cs /P0 scn 35 110 170 35 re f /P0 scn 300 200 100 40 re f\n"),
    ];
    for i in 0..N {
        let num = 6 + i;
        let (res, body) = if i + 1 < N {
            (format!("/Resources << /Pattern << /Q {} 0 R >> >>", num + 1), b"/Pattern cs /Q scn 0 0 10 10 re f\n".to_vec())
        } else {
            ("/Resources << /Font << /F1 4 0 R >> >>".to_string(), TEXT_CELL.to_vec())
        };
        objs.push(stream_obj(&format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 {res}"), body));
    }
    let mut doc = open(objs);
    mark(&mut doc, 0);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let error = apply(&mut doc, None).unwrap_err();
    assert!(matches!(error, RedactError::Pattern { reason: PatternProblem::Limit | PatternProblem::Content, .. }), "{error}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}
