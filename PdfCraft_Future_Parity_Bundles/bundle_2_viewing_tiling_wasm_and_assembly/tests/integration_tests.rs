//! Comprehensive Integration Tests for Bundle 2: Viewing, Tiling, WASM, and Assembly.

use pdfcraft_tiling::{Rect2D, TilePyramidCache, ViewportCuller};
use pdfcraft_wasm_bridge::WasmPdfViewer;
use pdfcraft_a11y_bridge::{AccessibleRole, ScreenReaderTreeBuilder};
use pdfcraft_assembly::{BookmarkDestination, BookmarkNode, BookmarkTree, CrashRecoveryJournal, DestinationFit, PageAction, PageMetadata, PageOrientation, PageSelectionEngine};

#[test]
fn test_deep_zoom_tiled_rendering_pipeline() {
    let mut cache = TilePyramidCache::new(256, 64);
    let culler = ViewportCuller::new(256);

    // Zoom scale 16.0 (Level 4 = 1600% zoom)
    let visible_tiles = culler.visible_tiles(
        0,
        612.0,
        792.0,
        16.0,
        Rect2D::new(100.0, 100.0, 600.0, 600.0),
    );

    assert!(!visible_tiles.is_empty());
    for tile_coord in &visible_tiles {
        assert_eq!(tile_coord.zoom_level, 4);
    }
}

#[test]
fn test_wasm_browser_viewer_workflow() {
    let mut viewer = WasmPdfViewer::new();
    let pdf_bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Page >>\nendobj\n%%EOF";
    let count = viewer.load_document(pdf_bytes).expect("Failed to load into WASM");
    assert!(count >= 1);

    let tile = viewer.render_tile(0, 0, 0, 0);
    assert_eq!(tile.len(), 256 * 256 * 4);
}

#[test]
fn test_accessibility_screen_reader_bridge() {
    let mut builder = ScreenReaderTreeBuilder::new();
    let mut doc = builder.build_document_root(Some("Accessible Form Document"));
    let h1 = builder.build_heading(1, "Personal Details");
    let p = builder.build_paragraph("Please enter full legal name.");
    doc.add_child(h1);
    doc.add_child(p);

    assert_eq!(doc.children.len(), 2);
    assert_eq!(doc.children[0].role, AccessibleRole::Heading(1));
}

#[test]
fn test_assembly_journal_and_bookmark_navigation() {
    // 1. Crash recovery journal
    let mut journal = CrashRecoveryJournal::new();
    journal.record(PageAction::InsertPage { index: 0, width: 612, height: 792 }, 1000);
    journal.record(PageAction::RotatePage { index: 0, degrees: 90 }, 1050);
    assert!(journal.is_dirty());

    let mut pages = vec![10, 20];
    journal.replay_onto_pages(&mut pages);
    assert_eq!(pages.len(), 3);

    // 2. Bookmark hierarchy
    let mut tree = BookmarkTree::new();
    let root = BookmarkNode::new("Title Page")
        .with_destination(BookmarkDestination {
            target_page_index: 0,
            fit_mode: DestinationFit::FitPage,
        });
    tree.add_item(root);
    assert_eq!(tree.total_count(), 1);

    // 3. Page selection syntax
    let odd_pages = PageSelectionEngine::select_odd(6);
    assert_eq!(odd_pages, vec![0, 2, 4]);

    let parsed = PageSelectionEngine::parse_range_syntax("1-2, 5-end", 6);
    assert_eq!(parsed, vec![0, 1, 4, 5]);
}
