//! The AI assistant panel (M13): Summarize, Ask about this document and Translate, answered by
//! the model set in Preferences ▸ AI assistant. Off by default.
//!
//! The desktop app supplies how to send a request ([`PdfCraftApp::ai_provider`]), so this crate
//! has no network code; without one (the web build, tests) the panel says the assistant needs
//! the desktop app. Requests and answers are built and read by `pdfcraft_engine::ai`.

use std::sync::Arc;

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ai::{self, AiSettings, Task};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, RightPanel, icons, widgets};

/// Sends one chat-completions request: (endpoint URL, API key or "", JSON body) → the response
/// body. Blocking; it runs on its own thread.
pub type AiProvider = Arc<dyn Fn(&str, &str, &serde_json::Value) -> Result<String, String> + Send + Sync>;

/// One question and its answer.
pub(crate) struct Exchange {
    pub(crate) doc: DocId,
    pub(crate) label: String,
    pub(crate) answer: Result<String, String>,
    /// The document was longer than what was sent.
    pub(crate) cut: bool,
    /// Pages the answer cites (1-based).
    pub(crate) cited: Vec<usize>,
}

pub(crate) struct Running {
    pub(crate) doc: DocId,
    pub(crate) label: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) rx: std::sync::mpsc::Receiver<Exchange>,
}

pub(crate) struct AiState {
    pub(crate) exchanges: Vec<Exchange>,
    pub(crate) running: Option<Running>,
    pub(crate) question: String,
    pub(crate) language: String,
}

impl Default for AiState {
    fn default() -> Self {
        Self { exchanges: Vec::new(), running: None, question: String::new(), language: "English".into() }
    }
}

/// What the panel asks for.
pub(crate) enum PanelAction {
    Run(Task),
    Cancel,
    Clear,
    Setup,
    GoTo(usize),
}

impl PdfCraftApp {
    /// `ai.summary`, `ai.ask`, `ai.translate`: open the panel, and start the task when it needs
    /// no more input. Without a configured provider, Preferences opens instead.
    pub(crate) fn ai_command(&mut self, id: &str) {
        self.right = Some(RightPanel::Assistant);
        if self.ai_provider.is_none() {
            self.notify("The AI assistant needs the desktop app");
            return;
        }
        if !self.ai_settings.is_ready() {
            self.dialog = Some(crate::Dialog::Preferences);
            self.notify("Set up an AI provider first (Preferences ▸ AI assistant)");
            return;
        }
        match id {
            "ai.summary" => self.ai_start(Task::Summarize),
            "ai.translate" => self.ai_start(Task::Translate(self.ai.language.clone())),
            // Ask waits for the question in the panel.
            _ => self.ai.question.clear(),
        }
    }

    /// Send `task` about the active document.
    pub(crate) fn ai_start(&mut self, task: Task) {
        let Some((index, id)) = self.active_ids() else { return };
        let Some(provider) = self.ai_provider.clone() else { return };
        if self.ai.running.is_some() {
            self.notify("The assistant is still answering");
            return;
        }
        let settings = self.ai_settings.clone();
        let endpoint = match settings.endpoint() {
            Ok(e) if settings.is_ready() => e,
            Ok(_) | Err(_) => {
                self.dialog = Some(crate::Dialog::Preferences);
                return;
            }
        };
        let Some(doc) = self.session.get(id) else { return };
        let src = doc.export_source();
        // Translate works on the page being read (a whole document rarely fits one answer).
        let pages: Vec<usize> = match &task {
            Task::Translate(_) => vec![self.views.get(index).map_or(0, |v| v.current).min(src.pages.saturating_sub(1))],
            _ => (0..src.pages).collect(),
        };
        let label = match &task {
            Task::Translate(_) => format!("{} (page {})", task.label(), pages.first().map_or(1, |p| p + 1)),
            _ => task.label(),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            let shown = label.clone();
            let work = move || {
                let exchange = match pdfcraft_engine::guard(|| ask(&src, &pages, &task, &settings, &endpoint, &provider)) {
                    Ok(Ok((answer, cut))) => {
                        let cited = ai::cited_pages(&answer, src.pages);
                        Exchange { doc: id, label: shown, answer: Ok(answer), cut, cited }
                    }
                    Ok(Err(e)) => Exchange { doc: id, label: shown, answer: Err(e), cut: false, cited: Vec::new() },
                    Err(m) => {
                        Exchange { doc: id, label: shown, answer: Err(format!("an internal error stopped it ({m})")), cut: false, cited: Vec::new() }
                    }
                };
                // The receiver may be gone (cancelled, or the app quit): nothing to report to then.
                let _ = tx.send(exchange);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            };
            if std::thread::Builder::new().name("pdfcraft-ai".into()).spawn(work).is_err() {
                self.notify("Couldn't start the assistant");
                return;
            }
            self.ai.running = Some(Running { doc: id, label, rx });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (src, pages, label, settings, endpoint, provider);
        }
    }

    /// Pick up a finished answer (each frame).
    pub(crate) fn poll_ai(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.ai.running {
            let exchange = match r.rx.try_recv() {
                Ok(e) => e,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Exchange {
                    doc: r.doc,
                    label: r.label.clone(),
                    answer: Err("the request stopped unexpectedly".into()),
                    cut: false,
                    cited: Vec::new(),
                },
            };
            self.ai.running = None;
            self.ai.exchanges.push(exchange);
        }
    }

    pub(crate) fn ai_action(&mut self, index: usize, action: PanelAction) {
        match action {
            PanelAction::Run(task) => self.ai_start(task),
            // The request finishes in the background; its answer is dropped.
            PanelAction::Cancel => self.ai.running = None,
            PanelAction::Clear => {
                let id = self.views.get(index).map(|v| v.id);
                self.ai.exchanges.retain(|e| Some(e.doc) != id);
            }
            PanelAction::Setup => self.dialog = Some(crate::Dialog::Preferences),
            PanelAction::GoTo(page) => {
                if let Some(v) = self.views.get_mut(index) {
                    v.go_to_page(page.saturating_sub(1));
                }
            }
        }
    }
}

/// Extract the text, send the request, read the answer. Returns the answer and whether the
/// document text was cut to fit.
fn ask(
    src: &pdfcraft_engine::export::ExportSource,
    pages: &[usize],
    task: &Task,
    settings: &AiSettings,
    endpoint: &str,
    provider: &AiProvider,
) -> Result<(String, bool), String> {
    let mut ex = pdfcraft_engine::export::Exporter::from_source(src.clone());
    let mut texts = Vec::new();
    let mut total = 0usize;
    for &p in pages {
        // A page whose text can't be read is skipped rather than failing the whole request.
        let text = ex.text(p).unwrap_or_default();
        total = total.saturating_add(text.len());
        texts.push((p, text));
        // No need to extract pages that won't be sent.
        if total > ai::MAX_DOCUMENT_CHARS.saturating_mul(4) {
            break;
        }
    }
    let skipped = texts.len() < pages.len();
    if texts.iter().all(|(_, t)| t.trim().is_empty()) {
        return Err("this document has no text to read. If it is scanned, run Scan & OCR ▸ Recognize text first".into());
    }
    let (document, cut) = ai::document_text(&texts, ai::MAX_DOCUMENT_CHARS);
    let body = ai::request_body(settings, &ai::messages(task, &document));
    let response = provider(endpoint, settings.api_key.trim(), &body)?;
    Ok((ai::parse_response(&response)?, cut || skipped))
}

/// The AI assistant panel for document `id`.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, app_state: PanelState<'_>, id: DocId) -> Option<PanelAction> {
    let PanelState { state, settings, available } = app_state;
    let mut action = None;
    if !available {
        ui.label(egui::RichText::new("The AI assistant needs the PdfCraft desktop app.").color(t.text_muted));
        return None;
    }
    if !settings.is_ready() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("sparkles", 36.0, t.text_faint));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Bring your own model").font(theme::semibold(14.0)));
            ui.label(
                egui::RichText::new(
                    "Summarize, ask questions and translate with a model you choose: any OpenAI-compatible endpoint, \
                     such as vLLM, llama.cpp, Ollama or LM Studio. Off until you set it up.",
                )
                .color(t.text_muted),
            );
            ui.add_space(10.0);
            if widgets::pill_button(ui, "Set up AI assistant…", true).clicked() {
                action = Some(PanelAction::Setup);
            }
        });
        return action;
    }
    let busy = state.running.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("Model: {}", settings.model.trim())).small().color(t.text_faint));
        if ui.small_button("Change…").clicked() {
            action = Some(PanelAction::Setup);
        }
    });
    ui.add_space(4.0);
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            if widgets::pill_button(ui, "Summarize", false).clicked() {
                action = Some(PanelAction::Run(Task::Summarize));
            }
            if widgets::pill_button(ui, "Translate page into", false).clicked() && !state.language.trim().is_empty() {
                action = Some(PanelAction::Run(Task::Translate(state.language.clone())));
            }
            ui.add(egui::TextEdit::singleline(&mut state.language).desired_width(80.0).char_limit(60).hint_text("Language"));
        });
        ui.add_space(6.0);
        let edit = ui.add(
            egui::TextEdit::multiline(&mut state.question)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .char_limit(ai::MAX_PROMPT_CHARS)
                .hint_text("Ask about this document… (Enter to send, Shift+Enter for a new line)"),
        );
        let enter = edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can_ask = !state.question.trim().is_empty();
                if (ui.add_enabled_ui(can_ask, |ui| widgets::pill_button(ui, "Ask", true)).inner.clicked() || enter) && can_ask {
                    let q = state.question.trim().to_string();
                    state.question.clear();
                    action = Some(PanelAction::Run(Task::Ask(q)));
                }
            });
        });
    });
    if let Some(r) = state.running.as_ref().filter(|r| r.doc == id) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(format!("{} — waiting for the model…", r.label)).color(t.text_muted));
            if ui.small_button("Cancel").clicked() {
                action = Some(PanelAction::Cancel);
            }
        });
    }
    let mine: Vec<&Exchange> = state.exchanges.iter().filter(|e| e.doc == id).collect();
    if !mine.is_empty() {
        ui.add_space(8.0);
        ui.separator();
    }
    // Newest first, so a new answer appears under the controls.
    for e in mine.iter().rev() {
        ui.add_space(6.0);
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(icons::image("sparkles", 14.0, t.accent));
                ui.add(egui::Label::new(egui::RichText::new(&e.label).font(theme::semibold(13.0))).wrap());
            });
            ui.add_space(4.0);
            match &e.answer {
                Ok(text) => {
                    ui.add(egui::Label::new(text.as_str()).selectable(true).wrap());
                    if e.cut {
                        ui.label(
                            egui::RichText::new("The document was too long to send whole; the answer covers its beginning.")
                                .small()
                                .color(t.text_faint),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        for &p in e.cited.iter().take(20) {
                            if ui.small_button(format!("p. {p}")).on_hover_text(format!("Go to page {p}")).clicked() {
                                action = Some(PanelAction::GoTo(p));
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::button(ui, "copy", 24.0, false, "Copy answer").clicked() {
                                ui.ctx().copy_text(text.clone());
                            }
                        });
                    });
                }
                Err(err) => {
                    ui.label(egui::RichText::new(format!("Couldn't get an answer: {err}")).color(ui.visuals().error_fg_color));
                }
            }
        });
    }
    if !mine.is_empty() {
        ui.add_space(8.0);
        if ui.small_button("Clear conversation").clicked() {
            action = Some(PanelAction::Clear);
        }
        ui.label(egui::RichText::new("Answers come from your model and can be wrong; check them against the document.").small().color(t.text_faint));
    }
    action
}

/// What the panel reads and edits.
pub(crate) struct PanelState<'a> {
    pub(crate) state: &'a mut AiState,
    pub(crate) settings: &'a AiSettings,
    pub(crate) available: bool,
}

/// Preferences ▸ AI assistant.
pub(crate) fn preferences(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(egui::RichText::new("AI assistant").font(theme::semibold(13.0)));
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let s = &mut app.ai_settings;
        ui.checkbox(&mut s.enabled, "Use an AI provider for Summarize, Ask and Translate");
        ui.add_enabled_ui(s.enabled, |ui| {
            egui::Grid::new("ai-provider").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let l = ui.label("Endpoint URL");
                ui.add(egui::TextEdit::singleline(&mut s.base_url).desired_width(260.0).char_limit(2_048).hint_text("http://localhost:8080/v1"))
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label("Model");
                ui.add(egui::TextEdit::singleline(&mut s.model).desired_width(260.0).char_limit(256).hint_text("the model id the server lists"))
                    .labelled_by(l.id);
                ui.end_row();
                let l = ui.label("API key");
                ui.add(egui::TextEdit::singleline(&mut s.api_key).password(true).desired_width(260.0).char_limit(2_048).hint_text("optional"))
                    .labelled_by(l.id);
                ui.end_row();
            });
            if s.enabled
                && !s.base_url.trim().is_empty()
                && let Err(e) = s.endpoint()
            {
                ui.label(egui::RichText::new(e).small().color(ui.visuals().error_fg_color));
            }
        });
        ui.label(
            egui::RichText::new(
                "Any OpenAI-compatible chat-completions endpoint (vLLM, llama.cpp, Ollama, LM Studio, or a hosted service). \
                 The text of the document you ask about is sent to it; nothing is sent until you ask. \
                 The settings, including the API key, are stored in PdfCraft's settings file on this computer.",
            )
            .small()
            .color(t.text_muted),
        );
    });
}
