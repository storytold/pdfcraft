//! Provenance-only: reads the retained `vendor/` copy, not the resolved dependency, so it is
//! not runtime coverage. `vendor_graph.rs` checks the resolved graph identity.
//! Retained vendor provenance for winit patch (12) (vendor/README.md); `vendor_graph.rs` checks the resolved graph.

/// Regression test: the vendored winit lets title-bar drags reach the app while the empty tab strip still
/// moves the window.
#[test]
fn vendor_provenance_winit_lets_macos_title_bar_drags_reach_the_app() {
    // On macOS the tab strip is the title bar: a transparent title bar over a full-size content
    // view. NSView answers YES to `mouseDownCanMoveWindow` for a view that isn't opaque, so AppKit
    // moved the whole window for a drag that began on a document tab, and tabs could not be
    // reordered. vendor/winit answers with the window's `movable_by_window_background` (NO unless
    // asked); the empty strip still moves the window through `ViewportCommand::StartDrag`. A
    // re-vendored copy without the patch would silently bring the bug back: re-apply the patch, or
    // drop the copy once winit has the fix.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let view = std::fs::read_to_string(root.join("vendor/winit/src/platform_impl/macos/view.rs")).unwrap();
    let (_, method) = view.split_once("#[method(mouseDownCanMoveWindow)]").expect("a title-bar drag moves the window again");
    let body = method.split_once("\n        }\n").map_or(method, |(body, _)| body);
    assert!(body.contains("movable_by_window_background"), "mouseDownCanMoveWindow no longer follows the window's setting");
    // The strip still moves the window itself, from its empty background.
    let chrome = std::fs::read_to_string(root.join("crates/ui-egui/src/chrome.rs")).unwrap();
    assert!(chrome.contains("ViewportCommand::StartDrag"), "the empty tab strip no longer moves the window");
}
