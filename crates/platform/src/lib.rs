//! pdfcraft-platform — L7 operating-system services (`file_names` among them) shared by the desktop
//! app (`ui-egui`) and the automation tools (`automation`), which sit on the same layer and can't
//! depend on each other.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod desktop_theme;
pub mod file_names;
pub mod staging;
