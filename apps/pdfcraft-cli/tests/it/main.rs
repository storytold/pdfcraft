//! The crate's integration tests, linked into one test binary.

mod audit;
mod automation_roots;
mod batch;
mod batch_exit;
mod check;
mod cpu_override;
mod edit;
mod extract;
mod help;
mod info_repairs;
mod info_standards;
mod mcp_compact;
mod options;
mod owner_password;
mod render;
mod root_arguments;
mod run_report;
mod run_root;
mod split;
mod stderr;
mod stdout;
mod text;
mod ui_control;
mod version_resource;
mod xfa;

/// Nextest remaps binaries when running an archive; ordinary Cargo uses the build-time path.
fn binary() -> std::ffi::OsString {
    std::env::var_os("NEXTEST_BIN_EXE_pdfcraft_cli").unwrap_or_else(|| env!("CARGO_BIN_EXE_pdfcraft-cli").into())
}
