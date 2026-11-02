//! Windows supplies the read-only association query and the explicit Settings action.
//! Other frontends have no integration and never ask. Neither callback sets a default.

use std::sync::Arc;

use crate::{PdfCraftApp, theme, widgets};

/// Host operations, injectable in tests without reading or changing OS settings.
#[derive(Clone)]
pub struct DefaultPdfIntegration {
    /// Read whether PdfCraft is already associated with PDFs. Runs on a worker thread.
    pub is_default: Arc<dyn Fn() -> Result<bool, String> + Send + Sync>,
    /// Open the Windows Settings page after an explicit user action.
    pub open_settings: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    /// False for the opt-in automation host: opening test PDFs must not interrupt it.
    pub automatic_prompt: bool,
}

#[derive(Default)]
pub(crate) struct Prompt {
    checked: bool,
    pending: bool,
    pub(crate) open: bool,
    #[cfg(not(target_arch = "wasm32"))]
    receiver: Option<std::sync::mpsc::Receiver<Result<bool, String>>>,
}

impl PdfCraftApp {
    /// Called only after opening existing PDF bytes, including a successful password retry.
    pub(crate) fn pdf_opened_for_default_prompt(&mut self) {
        if self.default_pdf.checked || !self.ask_default_pdf {
            return;
        }
        let Some(integration) = self.default_pdf_integration.clone().filter(|i| i.automatic_prompt) else { return };
        self.default_pdf.checked = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            let worker = std::thread::Builder::new().name("default-pdf-query".into()).spawn(move || {
                let result = pdfcraft_engine::guard(|| (integration.is_default)()).and_then(|r| r);
                let _ = tx.send(result);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            match worker {
                Ok(_) => self.default_pdf.receiver = Some(rx),
                Err(e) => log::warn!("default PDF query could not start: {e}"),
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = integration;
    }

    pub(crate) fn poll_default_pdf(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.default_pdf.receiver {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("default PDF query stopped".into()),
            };
            self.default_pdf.receiver = None;
            match result {
                Ok(false) => self.default_pdf.pending = true,
                Ok(true) => {}
                Err(e) => log::warn!("default PDF association could not be checked: {e}"),
            }
        }
        if !self.ask_default_pdf {
            self.default_pdf.pending = false;
        }
        // Do not cover a password, recovery, script, save, or other existing dialog.
        if self.default_pdf.pending && !self.modal_open() && !self.palette_open {
            self.default_pdf.pending = false;
            self.default_pdf.open = true;
        }
    }

    /// Preferences and `ui.command app.default_pdf_settings`; never writes associations.
    pub fn open_default_pdf_settings(&mut self) {
        let Some(integration) = &self.default_pdf_integration else { return };
        let result = pdfcraft_engine::guard(|| (integration.open_settings)()).and_then(|r| r);
        if let Err(error) = result {
            self.notify_fmt("Couldn't open Windows default app settings: {error}", &[("error", &error)]);
        }
    }

    fn resolve_default_pdf(&mut self, open_settings: bool) {
        self.default_pdf.open = false;
        if open_settings {
            self.open_default_pdf_settings();
        }
    }
}

pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.default_pdf.open {
        return;
    }
    let mut answer = None;
    let modal = widgets::show_modal(ctx, "default-pdf", |ui| {
        ui.set_width(410.0);
        let tokens = theme::Tokens::get(ctx);
        // Keep the opt-out box distinct in both themes within the shared dialog styling.
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0, tokens.text_muted);
        widgets.inactive.bg_fill = tokens.hover;
        widgets.hovered.bg_stroke = egui::Stroke::new(1.0, tokens.text);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let close = ui.add(egui::Button::image(crate::icons::image("x", 18.0, theme::Tokens::get(ctx).icon)).frame(false));
            close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tl!("Close")));
            if close.on_hover_text(tl!("Close")).clicked() {
                answer = Some(false);
            }
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(tl!("Make PdfCraft your default PDF app?")).font(theme::semibold(17.0))).wrap());
            });
        });
        ui.add_space(8.0);
        ui.label(tl!("Choose PdfCraft for .pdf files in Windows Settings."));
        ui.add_space(8.0);
        // Update the saved preference immediately, including when the app window itself closes.
        let mut dont_ask = !app.ask_default_pdf;
        if ui.add(crate::widgets::checkbox(&mut dont_ask, tl!("Do not ask again"))).changed() {
            app.ask_default_pdf = !dont_ask;
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Yes"), true).clicked() {
                answer = Some(true);
            }
            if widgets::pill_button(ui, tl!("Later"), false).clicked() {
                answer = Some(false);
            }
        });
    });
    if modal.should_close() {
        answer = Some(false);
    }
    if let Some(open_settings) = answer {
        app.resolve_default_pdf(open_settings);
    }
}
