//! PdfCraft Android entry point.
//!
//! This is the library target declared in `Cargo.toml` (`crate-type = ["cdylib", "rlib"]`). On
//! Android it compiles to `libpdfcraft.so`, whose exported `android_main` is invoked by
//! `android.app.NativeActivity` through `android-activity`'s glue (the same backend winit 0.30 /
//! eframe 0.36 use). On every other platform this crate is empty; the desktop binary lives in
//! `main.rs`.

#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use android_activity::AndroidApp;
use pdfcraft_ui_egui::PdfCraftApp;

/// NativeActivity JNI entry point.
///
/// The signature matches `android-activity`'s `extern "Rust" { fn android_main(app: AndroidApp) }`
/// contract: it must be exported under the unmangled name `android_main` with the Rust ABI. eframe
/// takes the `AndroidApp` and builds the winit event loop with `with_android_app`.
///
/// `#[unsafe(no_mangle)]` is the one unsafe op this crate performs (there is no safe way to export
/// an unmangled symbol); the crate-level `unsafe_code = "deny"` plus this scoped allow keeps the
/// rest of the workspace `forbid`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // TODO(android): attach an `android_logger` sink so `log::*` output reaches `adb logcat`.

    // Android has no system title bar to replace, so the desktop window strategy (integrated
    // titlebar, `with_fullsize_content_view`, icon, drag-and-drop, app id) is skipped: the app is
    // a fullscreen activity. `min_inner_size` still bounds freeform (multi-window) resizing; see
    // the Android manifest.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("PdfCraft").with_inner_size([1440.0, 920.0]).with_min_inner_size([820.0, 520.0]),
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    // The same shell as the desktop: a fresh `PdfCraftApp`. Desktop services (file dialogs,
    // clipboard, browser opener, update check, …) are absent here and the editor degrades
    // gracefully (no file dialog, no clipboard, no native menu).
    let result = eframe::run_native("PdfCraft", options, Box::new(|_cc| Ok(Box::new(PdfCraftApp::new()))));
    if let Err(error) = result {
        log::error!("PdfCraft Android failed to start: {error}");
    }
}
