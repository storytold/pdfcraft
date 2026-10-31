//! Provenance-only: reads the retained `vendor/` copy, not the resolved dependency, so it is
//! not runtime coverage. `vendor_graph.rs` checks the resolved graph identity.
//! Retained vendor provenance for winit patch (4) (vendor/README.md); `vendor_graph.rs` checks the resolved graph.

/// Regression test: the vendored winit maximizes a hidden Windows window only when it is shown.
#[test]
fn vendor_provenance_winit_maximizes_a_hidden_windows_window_only_when_it_is_shown() {
    // eframe creates PdfCraft's window hidden, restores the saved maximized state and shows the
    // window once its first frame is drawn. winit 0.30.13 applies the maximized state with
    // `ShowWindow(SW_MAXIMIZE)`, which shows a hidden window, so a window closed maximized came back
    // unpainted (white) before its first frame and could lose its focus. vendor/winit maximizes a
    // hidden window when it is shown, with one `SW_MAXIMIZE` (rust-windowing/winit#4587). A
    // re-vendored copy without the patch would silently bring the bug back: re-apply the patch, or
    // drop the copy once a winit 0.30 release has the fix.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let state = std::fs::read_to_string(root.join("vendor/winit/src/platform_impl/windows/window_state.rs")).unwrap();
    let shown_maximized = "if new.contains(WindowFlags::MAXIMIZED) && diff.contains(WindowFlags::VISIBLE) {\n                    SW_MAXIMIZE";
    assert!(state.contains(shown_maximized), "a hidden window shown while maximized is shown, then maximized again");
    let maximized_once_visible = "&& new.contains(WindowFlags::VISIBLE)\n            && !diff.contains(WindowFlags::VISIBLE)\n        {";
    assert!(state.contains(maximized_once_visible), "maximizing a hidden window shows it again");
}
