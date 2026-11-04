//! The crate's integration tests, linked into one test binary.

// Compile winit's scroll policy against its real event type, without a compositor. The vendored
// source (`winit_wayland_scroll`) names it as `crate::event`.
#[cfg(target_os = "linux")]
mod event {
    pub use winit::event::TouchPhase;
}

mod vendor_graph;
mod winit_macos_emoji_picker;
mod winit_macos_ime_commit;
mod winit_macos_ime_discard;
mod winit_macos_korean_first_key;
mod winit_macos_korean_space;
mod winit_macos_title_bar_drag;
mod winit_wayland_scroll;
mod winit_wayland_text_input;
mod winit_windows_hidden_maximized;
mod winit_windows_ime_cursor;
mod winit_x11_dpi;
mod winit_x11_ime_focus;
