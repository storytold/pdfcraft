//! The crate's integration tests, linked into one test binary. `perf.rs` keeps its own process:
//! its frame-time budgets would charge each measurement for the other tests running beside it.

mod support;

/// Tests that render pixels (`h.render()`) take this first, so only one wgpu device exists at a
/// time. Lavapipe crashes when another test creates its Vulkan instance meanwhile (segfault in
/// `vkCreateSampler` during a concurrent `vkCreateInstance`). On machines without a GPU, wgpu falls
/// back to Microsoft's WARP, whose ARM64 pixel-shader JIT crashes (access violation in
/// `d3d10warp!PixelJitProgram::ClassifyVars`) when two devices compile at once, as on GitHub's
/// Windows 11 ARM64 runner. Hold it for the whole test (declared before the harness, so it's
/// released after the device is dropped).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding it leaves nothing to clean up.
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod accessibility;
mod actions;
mod animate;
mod bookmark_position;
mod bookmarks;
mod bulk_fields;
mod buttons;
mod camera;
mod column_select;
mod combine_grid;
mod commands;
mod comments;
mod compare;
mod content;
mod control;
mod create;
mod date_format;
mod dialog_chrome;
mod document_links;
mod editing;
mod export;
mod fill_sign;
mod folders;
mod fonts;
mod forms;
mod identity;
mod javascript;
mod layer_actions;
mod links;
mod marks;
mod object_selection;
mod ocr;
mod organize_drag;
mod pageboxes;
mod panel_snapshots;
mod pdf_links;
mod pickers;
mod preferences_layout;
mod prepare;
mod print;
mod protect;
mod reading_colors;
mod recovery;
mod redact;
mod reload;
mod signing;
mod stamps;
mod text_select;
mod theme;
mod theme_button;
mod trust_signer;
mod ui;
mod updates;
mod view;
mod viewer;
mod xfa;
