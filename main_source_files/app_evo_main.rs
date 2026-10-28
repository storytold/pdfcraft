#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[path = "../../pdfcraft/src/main.rs"]
mod app;

fn main() -> eframe::Result {
    app::entry(true)
}
