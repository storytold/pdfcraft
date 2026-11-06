//! Replacing in several paragraphs of one page: each is checked against the page as it is after
//! the earlier rewrites, and a paragraph that still reads as it was found is replaced.
use std::sync::Arc;

use super::*;
use crate::{DocId, Session};

fn page_with(content: &str) -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
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

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("order.pdf", None, Arc::new(bytes), None).expect("opens");
    (s, id)
}

#[test]
fn replacing_in_every_paragraph_of_a_page_still_replaces_each_one() {
    let (mut s, id) = open(page_with("BT /F1 12 Tf 72 700 Td (red apple) Tj 0 -60 Td (red pear) Tj 0 -60 Td (red plum) Tj ET"));
    let q = Replace { find: "red".into(), with: "green".into(), ..Default::default() };
    let plan = s.get(id).unwrap().plan_replace(&[0], &q, Which::All, None).unwrap();
    assert_eq!(plan.replaced, 3, "refusals: {:?}", plan.refusals);
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    let edit = plan.edit.expect("an edit");
    s.apply(id, edit).unwrap();
    let texts: Vec<String> = s.get(id).unwrap().text_blocks(0).into_iter().map(|b| b.text).collect();
    assert_eq!(texts, ["green apple", "green pear", "green plum"]);
}
