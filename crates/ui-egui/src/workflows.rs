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
#[cfg(any(target_arch = "wasm32", test))]
pub(super) mod ocr_route;
pub(super) mod ocr_ui;
#[cfg(target_arch = "wasm32")]
pub(super) mod ocr_web;
pub(super) mod optimize_ui;
pub(super) mod print_ui;
pub(super) mod web_print;
