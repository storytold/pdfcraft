#[cfg(target_os = "macos")]
pub(super) mod macos_association;
#[cfg(target_os = "macos")]
pub(crate) mod macos_default_app;
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod open_worker;
pub mod ui_scale;
pub mod updates;
