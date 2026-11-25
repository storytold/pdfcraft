//! Update checks are turned off in PdfChocolate: it never asks a server for a newer release, and
//! offers no Check for updates command or preference. The framework's updater stays in the app's
//! services (it is part of the craft facade) but is never started, so it has nothing to do.

use crate::PdfCraftApp;

impl PdfCraftApp {
    /// One frame of the update flow: nothing, since no check is ever started.
    pub(crate) fn update_logic(&mut self, _ctx: &egui::Context) {}
}
