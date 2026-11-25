//! The host's colour picker for the Other… cell of the palettes: one request, one continuation.

use craft::color_ui::ui::{ColorPickerDialog, ColorPickerResult};
use craft::theme::Srgba;
use craft::ui::dialog::DialogHost;
use craft::ui::runtime::requests::ColorRequest;

use crate::PdfCraftApp;
use crate::comments::to_rgb;

/// What runs with the colour a picker was confirmed with, given the context of the pass that takes it.
type Then = Box<dyn FnOnce(&mut PdfCraftApp, &egui::Context, Srgba)>;

/// The dialog host a picker opens on.
pub(crate) enum PickerHost<'a> {
    /// The app's own dialog host.
    App,
    /// The host of another window that runs its own dialogs (Preferences); named so it can be passed without
    /// borrowing the app.
    Window(&'a mut DialogHost),
}

/// The continuations of the colour pickers that are open, by request key.
#[derive(Default)]
pub(crate) struct ColourPicks {
    pending: Vec<(String, Then)>,
}

impl ColourPicks {
    /// Opens a picker for `request` on `host` and keeps `then` for its answer. A picker already open keeps the
    /// input and the request is dropped.
    fn open(&mut self, host: &mut DialogHost, request: ColorRequest, then: Then) {
        let key = request.key.clone();
        if host.open_unless_open(ColorPickerDialog::from_request(request)).is_ok() {
            self.pending.retain(|(k, _)| *k != key);
            self.pending.push((key, then));
        }
    }

    /// The pickers of `host` that finished: the continuation of each confirmed one with its colour. A cancelled
    /// picker drops its continuation.
    pub(crate) fn take_answers(&mut self, host: &mut DialogHost) -> Vec<(Then, Srgba)> {
        let mut answers = Vec::new();
        while let Some((result, key)) = host.take_keyed::<ColorPickerDialog>() {
            let confirmed: Option<ColorPickerResult> = result.ok();
            let key = confirmed.as_ref().map(|r| r.key.clone()).or(key);
            let Some(at) = key.and_then(|key| self.pending.iter().position(|(k, _)| *k == key)) else { continue };
            let (_, then) = self.pending.remove(at);
            if let Some(r) = confirmed {
                answers.push((then, r.picked.color));
            }
        }
        answers
    }
}

impl PdfCraftApp {
    /// Opens the colour picker for `request` on `host` and runs `then` with the colour when it is confirmed.
    /// The picker never closes the menu or popover that asked.
    pub(crate) fn open_colour_picker(
        &mut self,
        host: PickerHost<'_>,
        request: ColorRequest,
        then: impl FnOnce(&mut PdfCraftApp, &egui::Context, Srgba) + 'static,
    ) {
        let host = match host {
            PickerHost::App => &mut self.dialogs,
            PickerHost::Window(h) => h,
        };
        self.colour_picks.open(host, request, Box::new(then));
    }
}

/// Runs the continuations of the pickers the app's dialog host finished, once per pass. Every colour picked
/// here came through Other…, so each is remembered as a recent colour first.
pub(crate) fn colour_picker_pass(app: &mut PdfCraftApp, ctx: &egui::Context) {
    for (then, color) in app.colour_picks.take_answers(&mut app.dialogs) {
        app.recent_colours.remember(to_rgb(color));
        then(app, ctx, color);
    }
}
