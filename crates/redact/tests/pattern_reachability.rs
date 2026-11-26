//! Pattern redaction against references no walk follows -- a direct `/Parent`, `/Kids` on a
//! page, a stray resource entry -- Type 3 fonts reached in other ways, and hostile wide trees.
//! Each test reads the saved bytes back.

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

fn glyph_pixel(doc: &Document, x: u32, row: u32) -> Vec<u8> {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    let i = ((row * out.width + x) * 4) as usize;
    out.rgba[i..i + 3].to_vec()
}

const T3: &[u8] = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 40 40] /FontMatrix [1 0 0 1 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [40] >>";
const BLUE_CELL: &[u8] = b"0 0 1 rg 0 0 500 300 re f\n";

/// The root's `/Parent` is a direct dictionary, not a reference: a full save writes it inline
/// with the root, and the pattern it names with it.
#[test]
fn a_direct_parent_dictionary_on_the_root_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Parent << /Type /Pages /Kids [] /Count 0 /Resources {RES} >> /Resources {RES} >>")
            .into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    confidential_or_refused(doc, 0);
}
/// A leaf (`/Type /Page`) that also carries `/Kids`: readers stop at the leaf, a full save does
/// not, and the hidden node names the pattern.
#[test]
fn a_node_hidden_under_a_leaf_s_kids_does_not_keep_the_pruned_cell() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources {RES} /Kids [7 0 R] >>").into_bytes(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("<< /Type /Pages /Parent 3 0 R /Kids [] /Count 0 /Resources {RES} >>").into_bytes(),
    ]);
    confidential_or_refused(doc, 0);
}
/// A stray reference to the cell from a resource category the guard does not inspect
/// (`/Shading`): the class #3999 tracks, shown here for patterns.
#[test]
fn a_stray_resource_reference_to_the_cell_does_not_keep_it() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> /Shading << /Sh 6 0 R >> >> >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    confidential_or_refused(doc, 0);
}
fn type3_page(page_res_extra: &str, content: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R /T3 7 0 R >> /Pattern << /P0 6 0 R >> {page_res_extra} >> >>").into_bytes(),
        FONT.to_vec(),
        stream_obj("", content),
        stream_obj(PATTERN, BLUE_CELL),
        T3.to_vec(),
        stream_obj("", b"40 0 d0 /Pattern cs /P0 scn 0 0 40 40 re f\n"),
    ];
    objs.extend(extra);
    open(objs)
}
fn assert_glyph_kept(mut doc: Document) {
    let before = glyph_pixel(&doc, 320, 50);
    assert_eq!(before, vec![0, 0, 255], "the glyph paints with the page pattern before the redaction");
    mark(&mut doc, 0);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(glyph_pixel(&doc, 320, 50), before, "the unmarked glyph lost its pattern paint ({report:?})");
}
/// The Type 3 font is shown in a form without `/Resources`, which draws through the page's.
#[test]
fn a_type3_glyph_in_a_resourceless_form_keeps_the_page_pattern() {
    assert_glyph_kept(type3_page(
        "/XObject << /Fm 9 0 R >>",
        b"/Fm Do\n/Pattern cs /P0 scn 35 110 170 35 re f\n",
        vec![stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 500 300]", b"BT /T3 1 Tf 300 230 Td (a) Tj ET\n")],
    ));
}
/// The Type 3 font is shown inside a pattern cell the page paints outside the mark: the cell
/// carries text, so the guard refuses rather than dropping anything.
#[test]
fn a_type3_glyph_inside_a_pattern_cell_is_refused() {
    let mut doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R /P1 9 0 R >> >> >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", b"/Pattern cs /P1 scn 300 200 100 40 re f\n/Pattern cs /P0 scn 35 110 170 35 re f\n"),
        stream_obj(PATTERN, BLUE_CELL),
        T3.to_vec(),
        stream_obj("", b"40 0 d0 /Pattern cs /P0 scn 0 0 40 40 re f\n"),
        stream_obj("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 500 300] /XStep 500 /YStep 300 /Resources << /Font << /T3 7 0 R >> >>", b"BT /T3 1 Tf 300 230 Td (a) Tj ET\n"),
    ]);
    mark(&mut doc, 0);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let error = apply(&mut doc, None).unwrap_err();
    assert!(matches!(error, RedactError::Pattern { .. }), "{error}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
}
/// A hostile wide tree: `K` `/Pages` nodes that all share one `/Kids` array of `M` references
/// back to themselves, beside the real page. Time and memory are measured from outside
/// (`PDF_WIDE_K`, `PDF_WIDE_M`). Ignored by default.
#[test]
#[ignore]
fn hostile_wide_tree_probe() {
    let k: usize = std::env::var("PDF_WIDE_K").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let m: usize = std::env::var("PDF_WIDE_M").ok().and_then(|v| v.parse().ok()).unwrap_or(500_000);
    let mut array = String::with_capacity(m * 9);
    for i in 0..m {
        array.push_str(&format!("{} 0 R ", 10 + i % k));
    }
    let mut objs = vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources {RES} >>").into_bytes(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        format!("[{array}]").into_bytes(),
        b"null".to_vec(),
        b"null".to_vec(),
    ];
    for _ in 0..k {
        objs.push(b"<< /Type /Pages /Kids 7 0 R /Count 0 >>".to_vec());
    }
    let mut doc = open(objs);
    mark(&mut doc, 0);
    let start = std::time::Instant::now();
    let result = apply(&mut doc, None);
    eprintln!("WIDE k={k} m={m} apply={:?} in {:?}", result.as_ref().map(|r| r.patterns_removed), start.elapsed());
}
/// The Type 3 font has `/Resources null`, which readers treat as absent.
#[test]
fn a_type3_font_with_null_resources_keeps_the_page_pattern() {
    let mut doc = type3_page("", b"BT /T3 1 Tf 300 230 Td (a) Tj ET\n/Pattern cs /P0 scn 35 110 170 35 re f\n", vec![]);
    doc.update_dict(pdfcraft_cos::ObjRef::new(7, 0), |d| d.set(b"Resources".to_vec(), Object::Null)).unwrap();
    assert_glyph_kept(doc);
}
/// Known limit (fidelity only, never a leak: a missed use can only prune more). Kept annotation
/// appearances are not interpreted for pattern use, so a Type 3 font without resources shown in
/// one does not keep the page's patterns. ISO 32000 resolves the glyph's names in the
/// appearance's own resources, where `/P0` does not exist, so only a lenient renderer draws it.
/// The Type 3 font is shown by a kept annotation's appearance stream (outside the mark).
#[test]
#[ignore = "known limit: kept annotation appearances are not interpreted, so a resourceless Type 3 glyph in one does not keep the page pattern; ISO 32000 resolves it in the appearance's own resources, which pdfcraft's renderer does not"]
fn a_type3_glyph_in_an_annotation_appearance_renders_the_same() {
    let mut doc = type3_page(
        "",
        b"/Pattern cs /P0 scn 35 110 170 35 re f\n",
        vec![
            stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 40 40] /Resources << /Font << /T3 7 0 R >> >>", b"BT /T3 1 Tf 0 0 Td (a) Tj ET\n"),
            b"<< /Type /Annot /Subtype /Stamp /Rect [300 230 340 270] /F 4 /AP << /N 9 0 R >> >>".to_vec(),
        ],
    );
    doc.update_dict(pdfcraft_cos::ObjRef::new(3, 0), |d| {
        d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(pdfcraft_cos::ObjRef::new(10, 0))]))
    })
    .unwrap();
    let before = glyph_pixel(&doc, 320, 50);
    mark(&mut doc, 0);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(glyph_pixel(&doc, 320, 50), before, "the annotation renders differently ({report:?}, before {before:?})");
}
/// A form wholly under the mark is removed; its own `/Pattern` cell carries text. Nothing prunes
/// that pattern, so the backstop never looks, and a stray reference (`/Shading`) keeps the cell.
#[test]
fn a_removed_form_s_pattern_does_not_survive_through_a_stray_reference() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> /XObject << /Fm 7 0 R >> /Shading << /Sh 6 0 R >> >> >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", b"BT /F1 20 Tf 40 250 Td (PUBLIC) Tj ET\nq 1 0 0 1 35 110 cm /Fm Do Q\n"),
        stream_obj(PATTERN, TEXT_CELL),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 170 35] /Resources << /Pattern << /P0 6 0 R >> >>", b"/Pattern cs /P0 scn 0 0 170 35 re f\n"),
    ]);
    let mut doc = doc;
    mark(&mut doc, 0);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    match apply(&mut doc, None) {
        Ok(report) => assert!(!saved_contains(&doc, SECRET), "Apply succeeded but the removed form's cell is in the saved file ({report:?})"),
        Err(error) => {
            assert!(matches!(error, RedactError::Pattern { .. }), "{error}");
            assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
        }
    }
}
/// `/P0` names an indirect object whose value is a reference to the cell; the cell is also named
/// by a stray reference. The backstop checks the number it pruned, not what it points at.
#[test]
fn a_pruned_pattern_named_through_a_reference_object_does_not_dodge_the_backstop() {
    let doc = open(vec![
        catalog(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> /Pattern << /P0 7 0 R >> /Shading << /Sh 6 0 R >> >> >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
        b"6 0 R".to_vec(),
    ]);
    confidential_or_refused(doc, 0);
}
/// The backstop's reachability is a full save's: after Apply an incremental save must be that
/// full save, byte for byte, never an appended revision over the old bytes.
#[test]
fn an_incremental_save_after_apply_is_the_full_save() {
    let doc = open(vec![
        catalog(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources {RES} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R >>".to_vec(),
        FONT.to_vec(),
        stream_obj("", CONTENT),
        stream_obj(PATTERN, TEXT_CELL),
    ]);
    let mut doc = doc;
    mark(&mut doc, 0);
    apply(&mut doc, None).unwrap();
    for object_streams in [true, false] {
        // A redacted rewrite gets a new identifier from fresh entropy; pin it so the two saves
        // can be compared byte for byte.
        let opts = SaveOptions { object_streams, id_entropy: Some([7; 16]), ..SaveOptions::default() };
        assert_eq!(write_incremental(&doc, &opts).unwrap(), write_full(&doc, &opts).unwrap());
    }
    // A later edit after a save-and-reopen starts from the clean file.
    let reopened = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert!(!reopened.bytes().windows(SECRET.len()).any(|w| w == SECRET));
}
