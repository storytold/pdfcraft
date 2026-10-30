pub(super) mod a11y_ui;
pub(super) mod actions_ui;
pub(super) mod combine_grid;
pub(super) mod combine_ui;
pub(super) mod compare_ui;
pub(super) mod create_multiple_ui;
pub(super) mod create_ui;
pub(super) mod export_ui;
pub(super) mod files;
pub(super) mod js_ui;
/// Recognize text in files and folders; the web build has no folders.
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod ocr_batch_ui;
pub(super) mod ocr_ui;
pub(super) mod optimize_ui;
pub(super) mod print_ui;
pub(super) mod web_print;
