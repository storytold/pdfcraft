//! Remove Hidden Information does the work of the categories it is asked for, and no more: a
//! textbook of 1,412 pages took over 15 minutes to lose its bookmarks while every page's content
//! was read for hidden text and layers it was not removing (#5873).

use super::*;
use crate::sanitize::{HIDDEN, Hidden, passes, remove_hidden, scan, scan_only};

/// Content passes and owner walks `work` makes on this thread.
fn passes_of<T>(work: impl FnOnce() -> T) -> (T, (usize, usize)) {
    let before = passes();
    let out = work();
    let after = passes();
    (out, (after.0 - before.0, after.1 - before.1))
}

/// Every category counted alone gives what the full scan gives it; lists come back in scan
/// order, once each.
#[test]
fn each_category_counts_alone_what_the_full_scan_counts() {
    let doc = hidden_fixture();
    let full = scan(&doc);
    assert_eq!(full.iter().map(|c| c.0).collect::<Vec<_>>(), HIDDEN, "the full scan lists every category in order");
    assert!(full.iter().all(|c| c.1 > 0), "the fixture holds something in every category: {full:?}");
    for &(h, n) in &full {
        assert_eq!(scan_only(&doc, &[h]), [(h, n)], "{h:?}");
    }
    // Listed out of order and twice, the categories come back in scan order, once each.
    let mut every: Vec<Hidden> = HIDDEN.into_iter().rev().collect();
    every.push(Hidden::Bookmarks);
    assert_eq!(scan_only(&doc, &every), full);
    assert!(scan_only(&doc, &[]).is_empty());
}

/// What each category costs: content passes for hidden text and layers, one shared object walk
/// for the owned entries, neither for the rest.
#[test]
fn a_category_reads_page_content_or_walks_every_object_only_when_it_needs_to() {
    let doc = hidden_fixture();
    assert_eq!(passes_of(|| scan(&doc)).1, (2, 1), "hidden text and hidden layers read the pages; four categories share one walk");
    assert_eq!(passes_of(|| scan_only(&doc, &[Hidden::Bookmarks, Hidden::Comments, Hidden::FormFields])).1, (0, 0));
    assert_eq!(passes_of(|| scan_only(&doc, &[Hidden::HiddenText])).1, (1, 0));
    assert_eq!(passes_of(|| scan_only(&doc, &[Hidden::Metadata, Hidden::PrivateData])).1, (0, 1), "one walk counts both");
}

/// Bookmarks alone: no content pass or walk, only the catalog edited, the rest still counted.
#[test]
fn removing_bookmarks_changes_the_catalog_alone() {
    let mut doc = hidden_fixture();
    let before = scan(&doc);
    // The fixture adds its /Info dictionary as an edit.
    let edited = doc.modified_objects();
    let (done, work) = passes_of(|| remove_hidden(&mut doc, &[Hidden::Bookmarks]));
    assert_eq!(done.unwrap(), [(Hidden::Bookmarks, 2)]);
    assert_eq!(work, (0, 0), "no page content read, no object walk");
    let root = doc.root().unwrap();
    let mut expected = [edited, vec![root.num]].concat();
    expected.sort_unstable();
    assert_eq!(doc.modified_objects(), expected, "the pages, their annotations and everything else stay as they were");
    let cat = doc.get(root).as_dict().cloned().unwrap();
    assert!(!cat.contains(b"Outlines") && !cat.contains(b"PageMode"));
    // Every other category still holds what it held.
    let after: Vec<(Hidden, usize)> = scan(&doc).into_iter().filter(|c| c.0 != Hidden::Bookmarks).collect();
    assert_eq!(after, before.into_iter().filter(|c| c.0 != Hidden::Bookmarks).collect::<Vec<_>>());
}

/// Hidden text alone: one pass to count it and one to remove it, no object walk.
#[test]
fn removing_hidden_text_reads_the_pages_for_it_alone() {
    let mut doc = hidden_fixture();
    let (done, work) = passes_of(|| remove_hidden(&mut doc, &[Hidden::HiddenText]));
    assert_eq!(done.unwrap(), [(Hidden::HiddenText, 13)]);
    assert_eq!(work, (2, 0), "one pass to count the hidden text, one to remove it");
    let c = content(&doc, 0);
    assert!(c.contains("(Visible)") && !c.contains("Hidden") && c.contains("(Layer)"), "{c}");
}

/// Comments alone still rewrite the pages' annotation lists, without reading their content.
#[test]
fn removing_comments_still_takes_them_off_every_page() {
    let mut doc = hidden_fixture();
    let (done, work) = passes_of(|| remove_hidden(&mut doc, &[Hidden::Comments]));
    assert_eq!(done.unwrap(), [(Hidden::Comments, 1)]);
    assert_eq!(work, (0, 0));
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let left: Vec<ObjRef> = annots_of(&doc, &p.dict).iter().filter_map(Object::as_ref).collect();
    // The square and its pop-up go; the widget, the link and the attachment stay.
    assert_eq!(left, [ObjRef::new(17, 0), ObjRef::new(18, 0), ObjRef::new(19, 0)]);
}

/// Each category alone, every pair and all of them remove what the full scan and a rewrite of
/// every page's annotation list removed: the same report and the same bytes in a full save.
#[test]
fn removal_writes_what_it_wrote_with_the_full_scan() {
    let mut cases: Vec<Vec<Hidden>> = HIDDEN.iter().map(|&h| vec![h]).collect();
    for (i, &a) in HIDDEN.iter().enumerate() {
        cases.extend(HIDDEN[i + 1..].iter().map(|&b| vec![a, b]));
    }
    cases.push(HIDDEN.to_vec());
    assert_eq!(cases.len(), 9 + 36 + 1);
    for which in cases {
        let mut before = hidden_fixture();
        let counts = scan(&before);
        let old = crate::sanitize::remove_counted(&mut before, &which, &counts, true);
        let mut after = hidden_fixture();
        let new = remove_hidden(&mut after, &which);
        assert_eq!(new, old, "{which:?}");
        let save = SaveOptions::default();
        assert!(write_full(&after, &save).unwrap() == write_full(&before, &save).unwrap(), "{which:?}: the saved files differ");
    }
}
