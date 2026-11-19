//! Where a tag's content is drawn (#7070): a table whose cells hold text, an image, a path, content
//! inside a form XObject and a link annotation, over two pages.

use pdfcraft_a11y::{TagBox, TagError, tag_boxes, tag_page_bounds};
use pdfcraft_cos::{Document, ObjRef};

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

fn build_pdf(objects: &[(u32, String)]) -> Document {
    let mut raw = Vec::from(&b"%PDF-1.7\n"[..]);
    let max_num = objects.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut offsets = std::collections::HashMap::new();
    for (num, body) in objects {
        offsets.insert(*num, raw.len());
        raw.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = raw.len();
    let size = max_num + 1;
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

/// Page 1: a text cell, an image cell, a path cell, a cell whose content is inside a form XObject
/// (`/Stm`) and a cell holding a link annotation (`/OBJR`, its page from the annotation's `/P`).
/// Page 2: one cell, an inline `TD` dictionary.
fn table() -> Document {
    let page1 = "/TD << /MCID 0 >> BDC BT /F1 12 Tf 100 700 Td (Cell) Tj ET EMC\n\
                 /TD << /MCID 1 >> BDC q 40 0 0 30 200 690 cm /Im1 Do Q EMC\n\
                 /TD << /MCID 2 >> BDC 0 0 1 rg 300 690 50 20 re f EMC\n\
                 q 1 0 0 1 400 680 cm /Fm1 Do Q";
    build_pdf(&[
        (1, "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R /MarkInfo << /Marked true >> >>".into()),
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [22 0 R] \
             /Resources << /Font << /F1 7 0 R >> /XObject << /Im1 20 0 R /Fm1 21 0 R >> >> >>"
                .into(),
        ),
        (4, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R >>".into()),
        (5, stream("", page1)),
        (6, stream("", "/TD << /MCID 0 >> BDC 50 50 100 20 re f EMC")),
        (7, FONT.into()),
        (9, "<< /Type /StructTreeRoot /K 10 0 R >>".into()),
        (10, "<< /S /Table /P 9 0 R /K [11 0 R 12 0 R] >>".into()),
        (11, "<< /S /TR /P 10 0 R /Pg 3 0 R /K [13 0 R 14 0 R 15 0 R 16 0 R 17 0 R] >>".into()),
        (12, "<< /S /TR /P 10 0 R /Pg 4 0 R /K [<< /S /TD /K 0 >>] >>".into()),
        (13, "<< /S /TD /P 11 0 R /K 0 >>".into()),
        (14, "<< /S /TD /P 11 0 R /K 1 >>".into()),
        (15, "<< /S /TD /P 11 0 R /K [2] >>".into()),
        (16, "<< /S /TD /P 11 0 R /K << /Type /MCR /Stm 21 0 R /MCID 0 >> >>".into()),
        (17, "<< /S /TD /P 11 0 R /K << /Type /OBJR /Obj 22 0 R >> >>".into()),
        (20, stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "0")),
        (21, stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30]", "/P << /MCID 0 >> BDC 0 0 20 20 re f EMC")),
        // The rectangle's corners are given the wrong way round on purpose.
        (22, "<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [500 720 450 700] /StructParent 0 >>".into()),
    ])
}

fn boxes(doc: &Document, num: u32) -> Vec<TagBox> {
    tag_boxes(doc, ObjRef::new(num, 0)).expect("a tag")
}

fn only(doc: &Document, num: u32) -> [f64; 4] {
    let b = boxes(doc, num);
    assert_eq!(b.len(), 1, "tag {num}: {b:?}");
    assert_eq!(b[0].page, 0);
    b[0].rect
}

#[test]
fn a_text_cell_is_outlined_where_its_glyphs_are() {
    let [x0, y0, x1, y1] = only(&table(), 13);
    assert!((x0 - 100.0).abs() < 1e-6, "starts at the text origin: {x0}");
    // "Cell" in 12 pt Helvetica: well over 15 pt and under 30 pt wide.
    assert!(x1 > 115.0 && x1 < 130.0, "ends after the glyphs' advance: {x1}");
    assert!(y0 < 700.0 && y0 > 695.0, "reaches below the baseline: {y0}");
    assert!(y1 > 707.0 && y1 < 713.0, "reaches up to the ascent: {y1}");
}

#[test]
fn an_image_cell_and_a_path_cell_are_outlined_where_they_paint() {
    let doc = table();
    assert_eq!(only(&doc, 14), [200.0, 690.0, 240.0, 720.0]);
    assert_eq!(only(&doc, 15), [300.0, 690.0, 350.0, 710.0]);
}

#[test]
fn form_content_and_an_annotation_are_outlined_where_the_page_places_them() {
    let doc = table();
    assert_eq!(only(&doc, 16), [400.0, 680.0, 430.0, 710.0], "the form's box where the page draws it");
    assert_eq!(only(&doc, 17), [450.0, 700.0, 500.0, 720.0], "the annotation's rectangle, normalised");
}

#[test]
fn a_row_is_outlined_by_its_cells_and_a_table_by_its_rows_one_box_per_page() {
    let doc = table();
    let row: Vec<[f64; 4]> = boxes(&doc, 11).into_iter().map(|b| b.rect).collect();
    assert_eq!(row.len(), 5, "one box per cell, in structure order: {row:?}");
    assert_eq!(row[1..], [[200.0, 690.0, 240.0, 720.0], [300.0, 690.0, 350.0, 710.0], [400.0, 680.0, 430.0, 710.0], [450.0, 700.0, 500.0, 720.0]]);
    let table = boxes(&doc, 10);
    assert_eq!(table.len(), 6);
    assert_eq!(table.last(), Some(&TagBox { page: 1, rect: [50.0, 50.0, 150.0, 70.0] }), "the inline cell on page 2");
    let pages = tag_page_bounds(&table);
    assert_eq!(pages.len(), 2, "{pages:?}");
    let [x0, y0, x1, y1] = pages[0].rect;
    assert_eq!((pages[0].page, x0, y0, x1, y1), (0, 100.0, 680.0, 500.0, 720.0));
    assert_eq!(pages[1], TagBox { page: 1, rect: [50.0, 50.0, 150.0, 70.0] });
}

#[test]
fn what_is_not_a_tag_is_refused() {
    assert_eq!(tag_boxes(&table(), ObjRef::new(22, 0)), Err(TagError::NotATag(ObjRef::new(22, 0))));
}

#[test]
fn a_cycle_in_the_tree_ends_and_deep_nesting_is_refused() {
    let tree = |kids: &str, extra: Vec<(u32, String)>| {
        let mut objects = vec![
            (1, "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R >>".to_owned()),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R >>".into()),
            (5, stream("", "/P << /MCID 0 >> BDC 10 10 20 20 re f EMC")),
            (9, "<< /Type /StructTreeRoot /K 10 0 R >>".into()),
            (10, "<< /S /Div /Pg 3 0 R /K [0 11 0 R] >>".into()),
            (11, format!("<< /S /Div /K {kids} >>")),
        ];
        objects.extend(extra);
        build_pdf(&objects)
    };
    // A child that lists its parent, and itself, again.
    assert_eq!(boxes(&tree("[10 0 R 11 0 R]", Vec::new()), 10), [TagBox { page: 0, rect: [10.0, 10.0, 30.0, 30.0] }]);
    // A chain of 600 indirect arrays, each holding the next.
    let chain = (20..620).map(|n| (n, format!("[{} 0 R]", n + 1))).collect();
    assert_eq!(tag_boxes(&tree("20 0 R", chain), ObjRef::new(10, 0)), Err(TagError::ContentLimit));
}

#[test]
fn an_annotation_rectangle_with_a_non_number_is_not_outlined() {
    let doc = build_pdf(&[
        (1, "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R >>".into()),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [11 0 R] >>".into()),
        (9, "<< /Type /StructTreeRoot /K 10 0 R >>".into()),
        (10, "<< /S /Link /Pg 3 0 R /K << /Type /OBJR /Obj 11 0 R >> >>".into()),
        // Skipping the null would read the box as [1 3 4 5].
        (11, "<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [1 null 3 4 5] >>".into()),
    ]);
    assert_eq!(boxes(&doc, 10), []);
}

#[test]
fn an_annotation_without_a_page_in_the_tree_is_outlined_on_its_own_page() {
    let doc = build_pdf(&[
        (1, "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 9 0 R >>".into()),
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into()),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into()),
        (4, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [11 0 R] >>".into()),
        (9, "<< /Type /StructTreeRoot /K 10 0 R >>".into()),
        // Neither the tag nor the object reference names a page: the annotation's /P does.
        (10, "<< /S /Link /K << /Type /OBJR /Obj 11 0 R >> >>".into()),
        (11, "<< /Type /Annot /Subtype /Link /P 4 0 R /Rect [10 20 30 40] >>".into()),
    ]);
    assert_eq!(boxes(&doc, 10), [TagBox { page: 1, rect: [10.0, 20.0, 30.0, 40.0] }]);
}
