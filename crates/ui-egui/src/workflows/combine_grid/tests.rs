use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft::host::queue::Cancel;
use craft::ui::primitives::textures::LoadedPicture;
use web_time::Instant;

use super::source::{self, OpenDocuments};
use super::*;
use crate::combine_ui::moved_order;

#[test]
fn moves_to_a_gap_keep_order_and_report_no_ops() {
    let ids = [10, 11, 12, 13, 14];
    let set = |v: &[u64]| v.iter().copied().collect::<BTreeSet<u64>>();
    // One file to the start, to the end, and between others.
    assert_eq!(moved_order(&ids, &set(&[13]), 0), Some(vec![13, 10, 11, 12, 14]));
    assert_eq!(moved_order(&ids, &set(&[10]), 5), Some(vec![11, 12, 13, 14, 10]));
    assert_eq!(moved_order(&ids, &set(&[10]), 3), Some(vec![11, 12, 10, 13, 14]));
    // A discontiguous selection gathers at the gap, in list order.
    assert_eq!(moved_order(&ids, &set(&[10, 12, 14]), 2), Some(vec![11, 10, 12, 14, 13]));
    assert_eq!(moved_order(&ids, &set(&[11, 13]), 5), Some(vec![10, 12, 14, 11, 13]));
    // Dropping a file next to itself, or a block inside itself, changes nothing.
    assert_eq!(moved_order(&ids, &set(&[12]), 2), None);
    assert_eq!(moved_order(&ids, &set(&[12]), 3), None);
    assert_eq!(moved_order(&ids, &set(&[11, 12, 13]), 2), None);
    // Nothing to move, unknown files and gaps past the end are harmless.
    assert_eq!(moved_order(&ids, &set(&[]), 1), None);
    assert_eq!(moved_order(&ids, &set(&[99]), 1), None);
    assert_eq!(moved_order(&ids, &set(&[10]), 99), Some(vec![11, 12, 13, 14, 10]));
}

/// The grid counts its rows (a header is one); the list counts entries and every page of every entry,
/// collapsed files' pages included. A page dropped by its grid position would land among another file's
/// pages and split that file.
#[test]
fn a_reorder_lands_in_the_lists_own_indexes_not_the_grids() {
    use CombineKey::{File, Page};
    // A has 3 pages and is collapsed; B has 2 and is expanded: rows [A, B header, B0, B1] over items
    // [A0 A1 A2 B0 B1].
    let rows = [File(1), File(2), Page(2, 0), Page(2, 1)];
    let (ids, items) = ([1, 2], [3, 2]);
    let moved = |key: CombineKey, to: usize| ReorderMove::new(key, vec![key], to, false);

    // B1 between the header and B0: before B0, which is item 3 (a grid position 2 would be among A's).
    assert_eq!(landing(&rows, &moved(Page(2, 1), 2), &ids, &items), Landing::Pages { gap: 3 });
    // B1 before A: item 0.
    assert_eq!(landing(&rows, &moved(Page(2, 1), 0), &ids, &items), Landing::Pages { gap: 0 });
    // B1 after the last row that remains: the end of every page.
    assert_eq!(landing(&rows, &moved(Page(2, 1), 3), &ids, &items), Landing::Pages { gap: 5 });
    // B0 onto B's header: that entry's first item.
    assert_eq!(landing(&rows, &moved(Page(2, 0), 1), &ids, &items), Landing::Pages { gap: 3 });

    // A file lands before the entry of the row now at `to`: B's header, then a page of B, then the end.
    assert_eq!(landing(&rows, &moved(File(1), 0), &ids, &items), Landing::Files { gap: 1 });
    assert_eq!(landing(&rows, &moved(File(1), 2), &ids, &items), Landing::Files { gap: 1 });
    assert_eq!(landing(&rows, &moved(File(1), 3), &ids, &items), Landing::Files { gap: 2 });
}

/// A PDF of `pages` pages of `w` × `h` points.
fn pdf(pages: usize, w: u32, h: u32) -> Arc<Vec<u8>> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 {w} {h}] >>", kids.join(" ")));
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 4 + 2 * i));
        let body = "0.2 0.4 0.8 rg 0 0 10 10 re f";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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
    Arc::new(out)
}

/// Regression: each page of an expanded file read (parsed) the whole file again. The job is what
/// `CombineThumbs::render` hands the queue.
#[test]
fn the_next_page_of_a_file_reuses_its_open_document_until_it_closes() {
    let documents = Arc::new(Mutex::new(OpenDocuments::default()));
    let bytes = pdf(3, 200, 300);
    let cancel = Cancel::new();
    for page in 0..2 {
        let picture = source::page_job(documents.clone(), bytes.clone(), 1, None, page, 0.15)(&cancel);
        assert!(matches!(picture, Ok(LoadedPicture::Image(_))), "page {page} is drawn");
    }
    assert_eq!(source::lock(&documents).opened(), 1, "the file was read once for both pages");
    assert_eq!(source::lock(&documents).kept(), 1, "its document stays open a moment");
    // Read the other way (a new password) it is another document.
    let other = source::page_job(documents.clone(), bytes.clone(), 2, None, 2, 0.15)(&cancel);
    assert!(matches!(other, Ok(LoadedPicture::Image(_))));
    assert_eq!(source::lock(&documents).opened(), 2, "a file read another way is read again");
    // Unused, they close after a while.
    source::lock(&documents).sweep(Instant::now() + OpenDocuments::KEEP_OPEN + Duration::from_millis(1));
    assert_eq!(source::lock(&documents).kept(), 0, "the open documents close after the keep-open time");
}

/// Combine thumbnails are on-screen rasters: like the viewer, they may draw a non-embedded
/// CJK font with an installed face, and they keep the file's password.
#[test]
fn thumbnails_use_the_on_screen_configuration() {
    let config = thumbnail_config(Some(Arc::from("secret")));
    assert!(config.system_fonts, "Combine thumbnails draw non-embedded CJK text like the viewer");
    assert_eq!(config.password.as_deref(), Some("secret"));
    assert!(!RenderConfig::default().system_fonts, "anything else stays off by default");
}
