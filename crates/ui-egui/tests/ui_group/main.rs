//! One test binary for the UI tests that are safe to share a process (#454): each links the whole
//! UI and render stack, so grouping them saves a link unit apiece. Tests that hold a GPU device,
//! write screenshots, use process-ID temporary paths or probe installed fonts stay standalone
//! in `tests/`. A file in this folder is only built if it is listed here.

#[path = "../support/mod.rs"]
mod support;

mod animate;
mod bulk_fields;
mod column_select;
mod find_replace;
mod identity;
mod layer_actions;
mod links;
mod marks;
mod pageboxes;
mod pdf_links;
mod protect;
mod redact;
mod text_select;
mod theme;
mod updates;
mod xfa;
