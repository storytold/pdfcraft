#[cfg(target_os = "macos")]
pub(super) mod macos_association;
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod open_worker;
pub mod ui_scale;
pub mod updates;
