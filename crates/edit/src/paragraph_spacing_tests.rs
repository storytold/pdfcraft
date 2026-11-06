//! The paragraph spacing: no default space between source rows, and a set spacing that
//! replaces the space a paragraph already had instead of adding to it.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_incremental};

use super::*;

/// One page drawing `content` with Helvetica as /F1.
pub(crate) fn page(content: &str) -> Document {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

/// The steps from each row down to the next, in points.
pub(crate) fn steps(doc: &Document) -> Vec<f64> {
    let b: Vec<f64> = text::text_lines(&reopen(doc), 0).unwrap().iter().map(|l| l.rect[1]).collect();
    b.windows(2).map(|w| w[0] - w[1]).collect()
}

pub(crate) fn close(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.5)
}

/// Choosing a family takes the single-style path. Three rows of 10 pt text 12 pt apart (the
/// default 1.2 line step) stay 12 pt apart: no spacing is set, so none is added. Half the size
/// before every row, as before, made the steps 17 and 17.
#[test]
fn choosing_a_family_keeps_row_steps() {
    let mut doc = page("BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (Two) Tj 0 -12 Td (Three) Tj ET");
    let style = text::BlockStyle { family: Some((Family::Times, false, false)), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &style).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 12.0]), "{s:?}");
}

/// Rows of 10 pt text at baselines 700, 688, 672 and 660: a 12 pt row step, and a 4 pt gap
/// before the third row (step 16, still one paragraph: a gap of 1.5 rows is the next one). A
/// 10 pt spacing replaces that gap: the third row moves
/// down 6 pt to 666 and the fourth with it to 654, so the steps are 12, 22, 12. Applied again
/// to the result the gap is already 10, so nothing moves. Before, the spacing was added on top
/// of the gap and before every row: 22, 30, 22 the first time, and growing each time after.
/// A 0 pt spacing takes the gap away: 12, 12, 12.
#[test]
fn reapplying_paragraph_spacing_replaces_the_gap() {
    let source = "BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (Two) Tj 0 -16 Td (Three) Tj 0 -12 Td (Four) Tj ET";
    let draft = Some("One\nTwo\nThree\nFour");
    let ten = text::BlockStyle { paragraph_spacing: Some(10.0), ..Default::default() };
    let mut doc = page(source);
    text::rewrite_block(&mut doc, 0, 0, draft, &ten).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 22.0, 12.0]), "first time: {s:?}");
    // The 22 pt step is wider than a row step and a half, so the reopened page reads the third
    // row as the next paragraph: the spacing applied again to the first one changes nothing.
    let mut doc = reopen(&doc);
    let first = text::text_blocks(&doc, 0).unwrap()[0].editor_text.clone();
    text::rewrite_block(&mut doc, 0, 0, Some(&first), &ten).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 22.0, 12.0]), "again: {s:?}");
    let mut doc = page(source);
    text::rewrite_block(&mut doc, 0, 0, draft, &text::BlockStyle { paragraph_spacing: Some(0.0), ..Default::default() }).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 12.0, 12.0]), "none: {s:?}");
}
