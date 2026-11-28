//! A replacement through the run sources must leave the rows it does not reach as they were.
use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_incremental};

use super::*;

fn page(content: &str) -> Document {
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

fn content(doc: &Document) -> String {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let list = match doc.resolve(p.dict.get(b"Contents").unwrap()).as_ref() {
        Object::Array(a) => a.clone(),
        _ => vec![p.dict.get(b"Contents").unwrap().clone()],
    };
    let mut out = Vec::new();
    for o in &list {
        if let Object::Stream(s) = &*doc.resolve(o) {
            out.extend(s.decoded().unwrap_or_default());
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn replace_keeps_untouched_rows() {
    let mut doc = page("BT /F1 10 Tf 12 TL 100 700 Td [(Hello) -300 (World)] TJ T* (second row here) Tj ET");
    let block = text_blocks(&doc, 0).unwrap().swap_remove(0);
    let start = block.editor_text.find("here").expect("the second row's word is in the paragraph");
    let (text, sources) = block.replaced(std::slice::from_ref(&(start..start + 4)), "there").expect("the range is on boundaries");
    let style = BlockStyle { sources, ..Default::default() };
    rewrite_block(&mut doc, 0, 0, Some(&text), &style).unwrap();
    let after = content(&doc);
    assert!(after.contains("-300"), "the first row keeps its TJ kerning: {after}");
}

/// The rewrite keeps every space typed, so a deleted word must not leave its two neighbouring
/// spaces behind; a run of spaces the paragraph had is left as it was.
#[test]
fn a_deleted_word_leaves_one_space() {
    let doc = page("BT /F1 10 Tf 100 700 Td (one two  three) Tj ET");
    let block = text_blocks(&doc, 0).unwrap().swap_remove(0);
    assert_eq!(block.editor_text, "one two  three");
    let two = block.editor_text.find("two").unwrap();
    let (text, _) = block.replaced(std::slice::from_ref(&(two..two + 3)), "").unwrap();
    assert_eq!(text, "one  three");
    let one = block.editor_text.find("one").unwrap();
    let (text, _) = block.replaced(std::slice::from_ref(&(one..one + 3)), "").unwrap();
    assert_eq!(text, " two  three");
    let (text, _) = block.replaced(std::slice::from_ref(&(two..two + 3)), "2").unwrap();
    assert_eq!(text, "one 2  three");
}
