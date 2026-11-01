//! Multi-page comparison checks: page insertions, deletions, and moves realign corresponding
//! pages for both text and visual comparison.

use std::sync::Arc;

use pdfcraft_engine::Session;
use pdfcraft_engine::compare::{Kind, PageKind};

fn build_doc(pages: &[&str]) -> Arc<Vec<u8>> {
    let n = pages.len();
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 600 800] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for (i, text) in pages.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 14 Tf 40 700 Td ({text}) Tj ET");
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Arc::new(out)
}

#[test]
fn rewritten_middle_page_is_visually_compared() {
    let mut s = Session::new();
    let old = s.open("old.pdf", None, build_doc(&["Introduction", "Original payment conditions", "Conclusion"]), None).unwrap();
    let new = s.open("new.pdf", None, build_doc(&["Introduction", "Entirely revised terms", "Conclusion"]), None).unwrap();
    assert_eq!(s.compare(old, new).unwrap().page_pairs, [(0, 0), (1, 1), (2, 2)]);
    let visual = s.compare_visual(old, new, 72.0).unwrap();
    assert!(!visual.is_empty() && visual.iter().all(|(page, _)| *page == 1), "{visual:?}");
}

#[test]
fn compare_realigns_pages_after_insertions_deletions_and_moves() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let page_a = "Section One overview of the master service agreement and definitions";
    let page_b1 = "Section Two payment is due within thirty days of invoice receipt";
    let page_b2 = "Section Two payment is due within fortyfive days of invoice receipt";
    let page_c = "Section Three governing law arbitration and final signatures";
    let page_x = "Appendix X supplemental regulatory compliance schedule";

    let old = s.open("old.pdf", None, build_doc(&[page_a, page_b1, page_c]), None).unwrap();

    // 1. Inserted page: [A, X, B2, C]
    let new_ins = s.open("new_ins.pdf", None, build_doc(&[page_a, page_x, page_b2, page_c]), None).unwrap();

    let ci = s.compare(old, new_ins).unwrap();
    assert_eq!(ci.page_pairs, [(0, 0), (1, 2), (2, 3)]);
    assert_eq!(ci.page_count(PageKind::Inserted), 1);
    let text_ins: Vec<(Kind, usize, &str, usize, &str)> =
        ci.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
    assert_eq!(text_ins, [(Kind::Inserted, 0, "", 1, page_x), (Kind::Replaced, 1, "thirty", 2, "fortyfive"),]);
    // Visual compare aligns old page 1 with new page 2 (changed) and old page 2 with new page 3 (unchanged).
    let vis_ins = s.compare_visual(old, new_ins, 72.0).unwrap();
    assert!(!vis_ins.is_empty() && vis_ins.iter().all(|(p, _)| *p == 2), "{vis_ins:?}");

    // 2. Deleted page: old_with_x [A, X, B1, C] -> new_del [A, B2, C]
    let old_with_x = s.open("old_with_x.pdf", None, build_doc(&[page_a, page_x, page_b1, page_c]), None).unwrap();
    let new_del = s.open("new_del.pdf", None, build_doc(&[page_a, page_b2, page_c]), None).unwrap();

    let cd = s.compare(old_with_x, new_del).unwrap();
    assert_eq!(cd.page_pairs, [(0, 0), (2, 1), (3, 2)]);
    assert_eq!(cd.page_count(PageKind::Deleted), 1);
    let vis_del = s.compare_visual(old_with_x, new_del, 72.0).unwrap();
    assert!(!vis_del.is_empty() && vis_del.iter().all(|(p, _)| *p == 1), "{vis_del:?}");

    // 3. Moved page: [A, B1, C] -> [A, C, B2]
    let new_mov = s.open("new_mov.pdf", None, build_doc(&[page_a, page_c, page_b2]), None).unwrap();

    let cm = s.compare(old, new_mov).unwrap();
    assert_eq!(cm.page_pairs, [(0, 0), (2, 1), (1, 2)]);
    assert_eq!(cm.page_count(PageKind::Moved), 1);
    let text_mov: Vec<(Kind, usize, &str, usize, &str)> =
        cm.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
    assert_eq!(text_mov, [(Kind::Replaced, 1, "thirty", 2, "fortyfive")]);
    // Visual compare aligns moved page C (old 2 -> new 1, unchanged) and moved page B (old 1 -> new 2, changed).
    let vis_mov = s.compare_visual(old, new_mov, 72.0).unwrap();
    assert!(!vis_mov.is_empty() && vis_mov.iter().all(|(p, _)| *p == 2), "{vis_mov:?}");
}
