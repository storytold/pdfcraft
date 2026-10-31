#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use adw::{gio, glib, prelude::*};
use libadwaita as adw;

mod app;
mod capture;
mod document;
mod tool_form;

fn main() -> glib::ExitCode {
    let application = adw::Application::builder().application_id("ai.storyteller.pdfcraft.gtk").flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    application.connect_startup(app::startup);
    application.connect_activate(app::activate);
    application.connect_open(app::open);
    application.run()
}
