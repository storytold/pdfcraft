//! Opening a document reads its page tree only as far as it needs to (#5873). A long document's
//! pages don't stay cached between two reads, so every read of them all is a full parse of each,
//! and a page can carry tens of kilobytes of private data (InDesign's `/PieceInfo`).
//! Counted, not timed: `page_nodes_read_on_this_thread` counts the page-tree nodes
//! `pdfcraft_organize` read on this thread.

use std::sync::Arc;

use pdfcraft_engine::Session;
use pdfcraft_organize::page_nodes_read_on_this_thread as page_nodes_read;

/// `n` empty pages under one `/Pages` node.
fn pages(n: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 612 792] >>", kids.join(" ")),
    ];
    objs.extend((0..n).map(|_| "<< /Type /Page /Parent 2 0 R >>".to_string()));
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

/// Opening checks whether the catalog leads to any page before it would rebuild a missing page
/// tree (#1656). The check reads the tree only to the first page: counting the pages read all
/// fifty.
#[test]
fn opening_reads_the_page_tree_only_to_its_first_page() {
    let mut s = Session::new();
    let read = page_nodes_read();
    let id = s.open("pages.pdf", None, Arc::new(pages(50)), None).unwrap();
    assert_eq!(page_nodes_read() - read, 2, "the root and the first page");
    assert_eq!(s.get(id).unwrap().info.pages.len(), 50, "every page is there");
}
