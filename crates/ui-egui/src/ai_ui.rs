//! The optional AI assistant: Preferences ▸ AI assistant, and the Summarize / Ask about this
//! document / Translate dialog.
//!
//! Off by default. Nothing is sent anywhere until the user turns it on, sets a provider and runs
//! one of the three commands; then the document's text goes to that provider's endpoint and
//! nowhere else. The desktop app supplies how to send ([`PdfCraftApp::ai_transport`]), so this
//! crate has no network code; without one (the web build) the commands say so. The API key is
//! never written to the settings: it is typed for the session, or read from an environment
//! variable the user names.

use std::sync::Arc;

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ai::{self, Answer, Api, Provider, Task};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, widgets};

/// Sends the assistant's requests (set by the desktop app).
pub type AiTransport = Arc<dyn ai::Transport>;

/// The longest environment variable name kept.
const MAX_ENV_NAME: usize = 128;

/// Preferences ▸ AI assistant, as kept in the settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiPrefs {
    pub enabled: bool,
    pub api: Api,
    pub endpoint: String,
    pub model: String,
    /// The environment variable the key is read from when none is typed for the session.
    pub key_env: String,
}

impl AiPrefs {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled,
            "api": self.api.as_str(),
            "endpoint": self.endpoint,
            "model": self.model,
            "key_env": self.key_env,
        })
    }

    /// Settings are untrusted: unknown values keep the defaults and text is cut to a sane length.
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let text = |key: &str, max: usize| {
            v[key].as_str().map(|s| s.trim().chars().filter(|c| !c.is_control()).take(max).collect::<String>()).unwrap_or_default()
        };
        AiPrefs {
            enabled: v["enabled"].as_bool().unwrap_or(false),
            api: v["api"].as_str().and_then(Api::parse).unwrap_or_default(),
            endpoint: text("endpoint", ai::MAX_ENDPOINT_CHARS),
            model: text("model", ai::MAX_MODEL_CHARS),
            key_env: text("key_env", MAX_ENV_NAME),
        }
    }
}

/// What the assistant dialog is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Summary,
    Ask,
    Translate,
}

impl Kind {
    pub fn from_command(id: &str) -> Option<Kind> {
        match id {
            "ai.summary" => Some(Kind::Summary),
            "ai.ask" => Some(Kind::Ask),
            "ai.translate" => Some(Kind::Translate),
            _ => None,
        }
    }
}

type Outcome = Result<Answer, String>;

/// The assistant dialog's state.
#[derive(Default)]
pub struct Assistant {
    pub kind: Kind,
    pub question: String,
    /// The language to translate into, as the user names it.
    pub language: String,
    /// The last answer (or why there is none), for document `doc`.
    pub answer: Option<Outcome>,
    pub doc: Option<DocId>,
    running: Option<std::sync::mpsc::Receiver<Outcome>>,
}

impl Assistant {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }
}

impl PdfCraftApp {
    /// The provider as set in Preferences, with the session's key or the one in the named
    /// environment variable.
    pub fn ai_provider(&self) -> Result<Provider, String> {
        let prefs = &self.ai_prefs;
        let mut key = Some(self.ai_key.trim().to_string()).filter(|k| !k.is_empty());
        let name = prefs.key_env.trim();
        if key.is_none() && !name.is_empty() {
            if name.len() > MAX_ENV_NAME || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(tl!("The environment variable name may only have letters, digits and underscores").to_string());
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                key = std::env::var(name).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
            }
            if key.is_none() {
                return Err(crate::i18n::fmt(tl!("The environment variable {name} is not set"), &[("name", name)]));
            }
        }
        let provider = Provider::new(prefs.api, prefs.endpoint.clone(), prefs.model.clone()).with_key(key);
        provider.url().map_err(|e| e.to_string())?;
        Ok(provider)
    }

    /// Summarize / Ask about this document / Translate: open the assistant, or say what it needs.
    pub fn open_assistant(&mut self, kind: Kind) {
        if !self.ai_prefs.enabled {
            self.dialog = Some(Dialog::Preferences);
            self.notify_tr("The AI assistant is off. Turn it on and choose a provider in Preferences.");
            return;
        }
        if self.ai_transport.is_none() {
            self.notify_tr("The AI assistant isn't available in this build");
            return;
        }
        let doc = self.active_ids().map(|(_, id)| id);
        // An answer belongs to the document and the task it was given for.
        if !self.assistant.is_running() && (self.assistant.doc != doc || self.assistant.kind != kind) {
            self.assistant.answer = None;
        }
        if !self.assistant.is_running() {
            self.assistant.kind = kind;
            self.assistant.doc = doc;
        }
        self.dialog = Some(Dialog::Assistant);
    }

    /// Send the active document's text with the dialog's task, in the background.
    pub fn start_assistant(&mut self) {
        if self.assistant.is_running() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let Some(transport) = self.ai_transport.clone() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let task = match self.assistant.kind {
            Kind::Summary => Task::Summarize,
            Kind::Ask => Task::Ask { question: self.assistant.question.clone() },
            Kind::Translate => Task::Translate { language: self.assistant.language.clone() },
        };
        self.assistant.doc = Some(id);
        let provider = match self.ai_provider() {
            Ok(p) => p,
            Err(e) => {
                self.assistant.answer = Some(Err(e));
                return;
            }
        };
        let source = doc.export_source();
        let ctx = self.ctx.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let work = move || {
            // Last-resort guard: a panic while reading the pages is an error, not a lost thread.
            let outcome = pdfcraft_engine::guard(|| {
                let all: Vec<usize> = (0..source.pages).collect();
                let pages = pdfcraft_engine::export::Exporter::from_source(source).assistant_pages(&all)?;
                ai::run(&provider, transport.as_ref(), &task, &pages).map_err(|e| e.to_string())
            })
            .unwrap_or_else(Err);
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(outcome);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        self.assistant.answer = None;
        self.assistant.running = Some(rx);
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-ai".into()).spawn(work).is_err() {
            self.assistant.running = None;
            self.assistant.answer = Some(Err(tl!("Couldn't start the request").to_string()));
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.poll_assistant();
    }

    /// Pick up a finished request (each frame).
    pub(crate) fn poll_assistant(&mut self) {
        let Some(rx) = &self.assistant.running else { return };
        let outcome = match rx.try_recv() {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tl!("The request stopped unexpectedly").to_string()),
        };
        self.assistant.running = None;
        self.assistant.answer = Some(outcome);
    }
}

/// Preferences ▸ AI assistant.
pub(crate) fn preferences_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("AI assistant")).font(theme::semibold(13.0)));
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.checkbox(&mut app.ai_prefs.enabled, tl!("Enable the AI assistant"));
        ui.label(
            egui::RichText::new(tl!("Off by default. When it is on, Summarize, Ask about this document and Translate send the document's text to the endpoint you set here, and nowhere else. The key is never saved with your settings."))
                .small()
                .color(t.text_muted),
        );
        if !app.ai_prefs.enabled {
            return;
        }
        ui.add_space(4.0);
        let prefs = &mut app.ai_prefs;
        egui::Grid::new("ai-provider").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(tl!("Provider type"));
            egui::ComboBox::from_id_salt("ai-api").selected_text(prefs.api.label()).width(320.0).show_ui(ui, |ui| {
                for api in Api::ALL {
                    ui.selectable_value(&mut prefs.api, api, api.label());
                }
            });
            ui.end_row();
            let label = ui.label(tl!("Endpoint URL"));
            let hint = if prefs.api == Api::Anthropic { "https://api.anthropic.com" } else { "http://localhost:11434/v1" };
            ui.add(egui::TextEdit::singleline(&mut prefs.endpoint).desired_width(320.0).char_limit(ai::MAX_ENDPOINT_CHARS).hint_text(hint).id_salt("ai-endpoint"))
                .labelled_by(label.id);
            ui.end_row();
            let label = ui.label(tl!("Model"));
            ui.add(egui::TextEdit::singleline(&mut prefs.model).desired_width(320.0).char_limit(ai::MAX_MODEL_CHARS).id_salt("ai-model")).labelled_by(label.id);
            ui.end_row();
            let label = ui.label(tl!("API key (this session only)"));
            ui.add(egui::TextEdit::singleline(&mut app.ai_key).password(true).desired_width(320.0).char_limit(ai::MAX_KEY_CHARS).id_salt("ai-key"))
                .labelled_by(label.id);
            ui.end_row();
            let label = ui.label(tl!("Or the environment variable holding it"));
            ui.add(egui::TextEdit::singleline(&mut prefs.key_env).desired_width(320.0).char_limit(MAX_ENV_NAME).id_salt("ai-key-env")).labelled_by(label.id);
            ui.end_row();
        });
        // Say what is still wrong once something has been typed.
        if !app.ai_prefs.endpoint.trim().is_empty()
            && let Err(e) = app.ai_provider()
        {
            ui.label(egui::RichText::new(e).small().color(t.text_muted));
        }
    });
}

/// The assistant dialog. Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let kind = app.assistant.kind;
    let title = match kind {
        Kind::Summary => tl!("Summarize"),
        Kind::Ask => tl!("Ask about this document"),
        Kind::Translate => tl!("Translate"),
    };
    ui.horizontal(|ui| {
        ui.add(crate::icons::image("sparkles", 20.0, t.accent));
        ui.label(egui::RichText::new(title).font(theme::semibold(18.0)));
    });
    ui.label(
        egui::RichText::new(crate::i18n::fmt(
            tl!("Sends this document's text to {endpoint} (model {model})."),
            &[("endpoint", app.ai_prefs.endpoint.trim()), ("model", app.ai_prefs.model.trim())],
        ))
        .small()
        .color(t.text_muted),
    );
    ui.add_space(6.0);
    let running = app.assistant.is_running();
    let mut enter = false;
    match kind {
        Kind::Summary => {}
        Kind::Ask => {
            let input = ui.add_enabled(
                !running,
                egui::TextEdit::multiline(&mut app.assistant.question)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .char_limit(ai::MAX_QUESTION_CHARS)
                    .hint_text(tl!("Your question"))
                    .id_salt("ai-question"),
            );
            enter = input.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
            ui.add_space(6.0);
        }
        Kind::Translate => {
            ui.horizontal(|ui| {
                let label = ui.label(tl!("Translate into"));
                let input = ui
                    .add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut app.assistant.language)
                            .desired_width(220.0)
                            .char_limit(ai::MAX_LANGUAGE_CHARS)
                            .id_salt("ai-language"),
                    )
                    .labelled_by(label.id);
                enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            });
            ui.add_space(6.0);
        }
    }
    let mut text_to_copy = None;
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(320.0).id_salt("ai-answer").show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(160.0);
            match &app.assistant.answer {
                _ if running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Waiting for the AI provider…"));
                    });
                }
                None => {
                    ui.label(egui::RichText::new(tl!("The answer appears here.")).color(t.text_muted));
                }
                Some(Ok(answer)) => {
                    if answer.coverage.truncated {
                        ui.label(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("Only the first {n} of {t} pages with text fit in the request."),
                                &[("n", &answer.coverage.pages_sent.to_string()), ("t", &answer.coverage.pages_total.to_string())],
                            ))
                            .small()
                            .color(t.text_muted),
                        );
                    }
                    // The model's words, shown as plain text: never interpreted, never translated.
                    ui.add(egui::Label::new(&answer.text).selectable(true).wrap());
                    text_to_copy = Some(answer.text.clone());
                }
                Some(Err(e)) => {
                    ui.label(crate::i18n::fmt(tl!("The AI assistant couldn't answer: {e}"), &[("e", e)]));
                }
            }
        });
    });
    ui.add_space(8.0);
    let (mut go, mut close, mut prefs, mut copy) = (false, false, false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let primary = match kind {
                Kind::Summary => tl!("Summarize"),
                Kind::Ask => tl!("Ask"),
                Kind::Translate => tl!("Translate"),
            };
            // Stable ids: the answer above changes how many widgets come first.
            go = ui.push_id("ai-go", |ui| ui.add_enabled_ui(!running, |ui| widgets::pill_button(ui, primary, true)).inner).inner.clicked();
            close = ui.push_id("ai-close", |ui| widgets::pill_button(ui, tl!("Close"), false)).inner.clicked();
            if text_to_copy.is_some() {
                copy = ui.push_id("ai-copy", |ui| widgets::pill_button(ui, tl!("Copy"), false)).inner.clicked();
            }
            prefs = ui.push_id("ai-prefs", |ui| widgets::pill_button(ui, tl!("Preferences…"), false)).inner.clicked();
        })
    });
    if copy && let Some(text) = text_to_copy {
        ui.ctx().copy_text(text);
    }
    if go || (enter && !running) {
        app.start_assistant();
    }
    if prefs {
        app.dialog = Some(Dialog::Preferences);
        return false;
    }
    close
}
