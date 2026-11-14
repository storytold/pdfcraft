//! Scan & OCR ▸ Straighten page / Straighten all pages: the report of what was straightened and what needs
//! looking at by hand. The batch runs behind it: [`PdfCraftApp::deskew_pass`] puts one measured page back per
//! pass and writes the report the dialog shows, so the window keeps painting (and the dialog can be closed)
//! while the pages are worked through.

use std::sync::{Arc, Mutex};

use craft::ui::contract::{Show as _, ShowContainer as _};
use craft::ui::dialog::{Dialog, DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::Text;
use craft::ui::layout_components::Stack;
use craft::ui::sections::{DialogRole, FooterButton};
use egui::Ui;

use crate::PdfCraftApp;

/// The straighten report on the dialog host. It cannot see the app, so the text it shows is the app's, shared
/// through `text` and updated by the pass as the run advances.
pub(crate) struct DeskewReport {
    id: DialogId,
    text: Arc<Mutex<String>>,
}

impl DeskewReport {
    pub(crate) fn new(text: Arc<Mutex<String>>) -> Self {
        Self { id: dialog_id("deskew-report"), text }
    }
}

impl StackDialog for DeskewReport {
    type Result = ();

    fn id(&self) -> DialogId {
        self.id
    }

    fn key(&self) -> Option<&str> {
        Some("deskew-report")
    }

    fn show(&mut self, ui: &mut Ui, _stack: &DialogStack) -> Option<DialogResult<()>> {
        let text = self.text.lock().map(|t| t.clone()).unwrap_or_default();
        let shown = Dialog::new(tl!("Straighten pages"))
            .buttons([FooterButton::new(tl!("Done"), DialogRole::Accept)])
            .show(ui, |ui, _| {
                Stack::new().show(ui, |ui, _| {
                    Text::new(text.clone()).show(ui);
                });
            });
        shown.output.result(|| ())
    }
}

impl PdfCraftApp {
    /// Opens the straighten report with `text` as it stands. The pass keeps it up to date while the run goes.
    pub(crate) fn open_deskew_report(&mut self, text: String) {
        *self.deskew_text.lock().expect("the report lock is not poisoned") = text;
        let text = self.deskew_text.clone();
        crate::dialogs::host::open(self, DeskewReport::new(text));
    }

    /// One step of the straighten run: the batch puts one measured page back, the single-page report does
    /// nothing. The dialog redraws as the text changes, and the run ends when every page has had its turn.
    pub(crate) fn deskew_pass(&mut self, ctx: &egui::Context) {
        let Some(job) = self.deskew_job.as_mut() else { return };
        job.step(&mut self.session);
        let text = job.text();
        *self.deskew_text.lock().expect("the report lock is not poisoned") = text;
        if job.done() {
            self.deskew_job = None;
        } else {
            ctx.request_repaint();
        }
    }
}
