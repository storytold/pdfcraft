//! The AI tab: provider setup (BYO API endpoint or a one-click local model),
//! then document operations — translate the open document into a chosen
//! language, shown page by page next to the original.
//!
//! Everything slow runs on a worker thread and reports back over one channel;
//! `tick` drains it each frame. The panel always narrates what is happening —
//! the "smooth informed" flow the craft-ai spec asks for.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc::{Receiver, channel};

use craft_ai::fetch::{self, Event};
use craft_ai::{AiError, OpenAiCompat, Provider, ProviderInfo, by_id};

use egui::{Color32, Sense};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

/// Which backend the tab is configured for.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Api,
    Local,
}

/// Messages worker threads report back on.
enum Msg {
    /// `provider.test()` finished.
    Tested(Result<ProviderInfo, AiError>),
    /// Download progress event, forwarded.
    Progress(Event),
    /// GGUF + tokenizer are on disk; carries the model id.
    Downloaded(Result<String, AiError>),
    /// A local model finished loading into memory.
    #[cfg(feature = "ai-local")]
    Loaded(Result<Box<dyn Provider>, AiError>),
    /// One page of the document translated.
    Page(usize, String),
    /// The whole job finished; the provider comes back to the app.
    Translated(Result<Box<dyn Provider>, AiError>),
}

/// A finished (or in-flight) translation of one document.
pub struct Translation {
    pub lang: String,
    /// Pages translated so far; `page == text` pairs, sparse while running.
    pub pages: Vec<(usize, String)>,
    pub total: usize,
}

/// The tab's whole state. `provider` is `Some` only while ready to generate —
/// it is moved into the worker for a job and returned when done.
pub(crate) struct Ai {
    pub kind: Kind,
    pub endpoint: String,
    pub model: String,
    /// Kept in memory only unless `remember_token` is on.
    pub token: String,
    pub remember_token: bool,
    /// Target language for document translation (free text, e.g. "French").
    pub lang: String,
    /// Registry id of the picked local model.
    pub local_id: String,
    /// Registry id of the downloaded+loadable local model.
    pub local_ready: Option<String>,
    pub caps: Option<craft_ai::probe::MachineCaps>,
    provider: Option<Box<dyn Provider>>,
    rx: Option<Receiver<Msg>>,
    /// One-line narration of the current job ("Downloading… 412/986 MB").
    pub busy: Option<String>,
    /// Download progress (done, total, bytes/sec).
    pub dl: Option<(u64, u64, u64)>,
    /// Result of the last "Test connection" (shown beside the button).
    pub test_result: Option<Result<String, String>>,
    pub translation: Option<Translation>,
    /// Rolling activity log for the curious.
    pub log: Vec<String>,
    pub log_open: bool,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            kind: Kind::Api,
            endpoint: "http://localhost:11434/v1".into(),
            model: String::new(),
            token: String::new(),
            remember_token: false,
            lang: "English".into(),
            local_id: String::new(),
            local_ready: None,
            caps: None,
            provider: None,
            rx: None,
            busy: None,
            dl: None,
            test_result: None,
            translation: None,
            log: Vec::new(),
            log_open: false,
        }
    }
}

/// The languages offered in the picker; the field stays free-text.
pub const LANGUAGES: &[&str] = &[
    "English",
    "Arabic",
    "Chinese (Simplified)",
    "Chinese (Traditional)",
    "Dutch",
    "French",
    "German",
    "Hindi",
    "Italian",
    "Japanese",
    "Korean",
    "Polish",
    "Portuguese",
    "Russian",
    "Spanish",
    "Turkish",
    "Ukrainian",
];

impl Ai {
    /// What survives a restart: fields, not the live provider or jobs. The
    /// token is written only when `remember_token` is on — and the checkbox
    /// says it is stored unencrypted.
    pub(crate) fn persist_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "endpoint": self.endpoint,
            "model": self.model,
            "remember_token": self.remember_token,
            "token": self.remember_token.then_some(&self.token),
            "lang": self.lang,
            "local_id": self.local_id,
        })
    }

    /// Restore `persist_json` output; malformed values are ignored, as
    /// everywhere else in settings.
    pub(crate) fn restore(&mut self, v: &serde_json::Value) {
        if let Ok(k) = serde_json::from_value::<Kind>(v["kind"].clone()) {
            self.kind = k;
        }
        for (key, slot) in [
            ("endpoint", &mut self.endpoint),
            ("model", &mut self.model),
            ("token", &mut self.token),
            ("lang", &mut self.lang),
            ("local_id", &mut self.local_id),
        ] {
            if let Some(s) = v[key].as_str() {
                *slot = s.to_string();
            }
        }
        if let Some(b) = v["remember_token"].as_bool() {
            self.remember_token = b;
        }
    }

    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        // Keep the log readable on long sessions.
        const KEEP: usize = 400;
        if self.log.len() > KEEP {
            self.log.drain(..self.log.len() - KEEP);
        }
    }

    /// Is a provider ready to generate right now?
    pub fn ready(&self) -> bool {
        self.provider.is_some()
    }

    fn busy_with(&mut self, what: &str) {
        self.busy = Some(what.to_string());
        self.note(what);
    }

    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce(std::sync::mpsc::Sender<Msg>) + Send + 'static) {
        let (tx, rx) = channel::<Msg>();
        let ctx = ctx.clone();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            job(tx);
            // Wake the UI even if the last message races the drain.
            ctx.request_repaint();
        });
    }

    /// "Test connection" on the BYOT card — a handshake, no document text.
    pub(crate) fn test_endpoint(&mut self, ctx: &egui::Context) {
        let mut p = OpenAiCompat::new(self.endpoint.clone(), (!self.token.is_empty()).then(|| self.token.clone()), self.model.clone());
        self.test_result = None;
        self.busy_with("Testing the connection…");
        self.spawn(ctx, move |tx| {
            let _ = tx.send(Msg::Tested(p.test()));
        });
    }

    /// Adopt the BYOT fields as the active provider (after or without a test).
    fn use_api_provider(&mut self) {
        if self.endpoint.trim().is_empty() || self.model.trim().is_empty() {
            self.note("Endpoint and model are both required");
            return;
        }
        self.provider =
            Some(Box::new(OpenAiCompat::new(self.endpoint.clone(), (!self.token.is_empty()).then(|| self.token.clone()), self.model.clone())));
        self.note(format!("Provider ready: {} — document text will be sent to {}", self.model, self.endpoint));
    }

    /// "Detect this PC": probe once, pick the suggested model.
    pub(crate) fn detect(&mut self) {
        let Some(dir) = craft_ai::probe::models_dir() else {
            self.note("No usable cache folder on this system — local models unavailable");
            return;
        };
        let caps = craft_ai::probe::probe(&dir);
        if self.local_id.is_empty()
            && let Some(m) = craft_ai::suggest(&caps)
        {
            self.local_id = m.id.to_string();
            self.note(format!(
                "Suggested {} — fits your {:.0} GB RAM ({:.0} GB free disk; est. ~{:.0} tok/s)",
                m.name,
                caps.ram_gb,
                caps.disk_free_gb,
                caps.est_tokens_per_sec(m.size_mb),
            ));
        }
        self.caps = Some(caps);
    }

    /// Download the picked model (resumable; tokenizer included), then mark
    /// it loadable. Existing files are verified and reused.
    pub(crate) fn download_picked(&mut self, ctx: &egui::Context) {
        let Some(entry) = by_id(&self.local_id) else {
            self.note("Pick a model first");
            return;
        };
        let Some(dir) = craft_ai::probe::models_dir() else {
            self.note("No usable cache folder on this system");
            return;
        };
        self.busy_with(&format!("Downloading {} ({:.0} MB)…", entry.name, entry.size_mb));
        self.dl = Some((0, 0, 0));
        self.spawn(ctx, move |tx| {
            let report = |e: &Event| {
                let _ = tx.send(Msg::Progress(e.clone()));
            };
            let out = fetch::download(entry, &dir, report, None).and_then(|_| fetch::download_tokenizer(entry, &dir));
            let _ = tx.send(Msg::Downloaded(out.map(|_| entry.id.to_string())));
        });
    }

    /// Load a downloaded local model into memory — the slow part, on a worker.
    /// Needs the `ai-local` feature (candle); without it the button explains itself.
    pub(crate) fn load_downloaded(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "ai-local")]
        {
            let Some(id) = self.local_ready.clone().or_else(|| self.downloaded_ids().first().cloned()) else { return };
            let Some(entry) = by_id(&id) else { return };
            let Some(dir) = craft_ai::probe::models_dir() else { return };
            self.busy_with(&format!("Loading {} — this takes a moment…", entry.name));
            self.spawn(ctx, move |tx| {
                let gguf = dir.join(entry.file);
                let tok = fetch::tokenizer_path(entry, &dir);
                let r = craft_ai::local::LocalGguf::load(entry, &gguf, &tok).map(|m| Box::new(m) as Box<dyn Provider>);
                let _ = tx.send(Msg::Loaded(r));
            });
        }
        #[cfg(not(feature = "ai-local"))]
        {
            let _ = ctx;
            self.note("This build has no local inference (compiled without the `ai-local` feature) — the model still downloads fine");
        }
    }

    /// Models already sitting in the cache (for the "load again" path).
    fn downloaded_ids(&self) -> Vec<String> {
        craft_ai::probe::models_dir()
            .map(|dir| craft_ai::MODELS.iter().filter(|m| fetch::is_downloaded(m, &dir)).map(|m| m.id.to_string()).collect())
            .unwrap_or_default()
    }

    /// The active provider's display name, or none.
    fn provider_name(&self) -> Option<String> {
        self.provider.as_ref().map(|p| p.name())
    }

    fn provider_is_local(&self) -> bool {
        self.provider.as_ref().is_some_and(|p| p.is_local())
    }

    /// Forget the provider (and only it — settings stay for next time).
    fn reset_provider(&mut self) {
        self.provider = None;
        self.note("Provider cleared — pick or configure one below");
    }

    /// `ui.state`'s `ai` section — the headless `ai.status`/`ai.models.list`.
    /// The token is never in it.
    pub(crate) fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "endpoint": self.endpoint,
            "model": self.model,
            "lang": self.lang,
            "local_id": self.local_id,
            "ready": self.ready(),
            "provider": self.provider_name(),
            "provider_is_local": self.provider_is_local(),
            "busy": self.busy,
            "models": craft_ai::MODELS.iter().map(|m| serde_json::json!({
                "id": m.id,
                "name": m.name,
                "size_mb": m.size_mb,
                "license": m.license,
                "downloaded": self.downloaded_ids().iter().any(|d| d == m.id),
                "loaded": self.local_ready.as_deref() == Some(m.id) && self.provider_is_local(),
            })).collect::<Vec<_>>(),
            "translation": self.translation.as_ref().map(|t| serde_json::json!({
                "lang": t.lang,
                "done": t.pages.len(),
                "total": t.total,
            })),
        })
    }
}

/// Drain the job channel once per frame.
pub(crate) fn tick(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let mut repaint = false;
    loop {
        let msg = match app.ai.rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(m)) => m,
            _ => break,
        };
        repaint = true;
        let ai = &mut app.ai;
        match msg {
            Msg::Tested(Ok(info)) => {
                ai.busy = None;
                let mut line = info.label;
                if !info.models.is_empty() {
                    line = format!("{line} — models: {}", info.models.iter().take(6).cloned().collect::<Vec<_>>().join(", "));
                }
                ai.test_result = Some(Ok(line.clone()));
                ai.note(format!("Connection OK: {line}"));
            }
            Msg::Tested(Err(e)) => {
                ai.busy = None;
                ai.test_result = Some(Err(e.to_string()));
                ai.note(format!("Connection failed: {e}"));
            }
            Msg::Progress(Event::Started { total, resumed_from }) => {
                ai.dl = Some((resumed_from, total, 0));
            }
            Msg::Progress(Event::Progress { done, total, bytes_per_sec }) => {
                ai.dl = Some((done, total, bytes_per_sec));
                ai.busy = Some(match total {
                    0 => format!("Downloading… {:.0} MB", done as f64 / 1e6),
                    t => format!("Downloading… {:.0}/{:.0} MB ({:.0}%)", done as f64 / 1e6, t as f64 / 1e6, done as f64 / t as f64 * 100.0),
                });
            }
            Msg::Progress(Event::Verifying) => ai.busy = Some("Verifying checksum…".into()),
            Msg::Progress(Event::Done { verified }) => {
                ai.note(if verified { "Checksum verified" } else { "Download finished (no checksum available — unverified)" });
            }
            Msg::Downloaded(Ok(id)) => {
                ai.busy = None;
                ai.dl = None;
                ai.local_ready = Some(id.clone());
                ai.note("Download complete — ready to load".to_string());
            }
            Msg::Downloaded(Err(e)) => {
                ai.busy = None;
                ai.dl = None;
                ai.note(format!("Download failed: {e}"));
            }
            #[cfg(feature = "ai-local")]
            Msg::Loaded(Ok(p)) => {
                ai.busy = None;
                ai.note(format!("Local model ready: {} — nothing leaves this PC", p.name()));
                ai.provider = Some(p);
            }
            #[cfg(feature = "ai-local")]
            Msg::Loaded(Err(e)) => {
                ai.busy = None;
                ai.note(format!("Model failed to load: {e}"));
            }
            Msg::Page(page, text) => {
                if let Some(t) = &mut ai.translation {
                    t.pages.push((page, text));
                    ai.busy = Some(format!("Translating… {}/{} pages", t.pages.len(), t.total));
                }
            }
            Msg::Translated(Ok(p)) => {
                ai.provider = Some(p);
                ai.busy = None;
                if let Some(t) = &ai.translation {
                    ai.note(format!("Translation finished: {} pages → {}", t.pages.len(), t.lang));
                }
            }
            Msg::Translated(Err(e)) => {
                ai.busy = None;
                ai.note(format!("Translation stopped: {e}"));
            }
        }
    }
    if repaint {
        ctx.request_repaint();
    }
}

/// Translate the active document with the configured provider. Extracts the
/// text on the UI thread (the session isn't Send), then hands plain text to a
/// worker — the document itself never leaves the app.
pub(crate) fn translate_active(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((_, id)) = app.active_ids() else {
        app.ai.note("Open a document first");
        return;
    };
    let Some(mut provider) = app.ai.provider.take() else {
        app.ai.note("Set up a provider first (API or local model)");
        return;
    };
    if app.ai.busy.is_some() {
        app.ai.provider = Some(provider);
        return;
    }
    let Some(doc) = app.session.get(id) else { return };
    let pages = doc.info.pages.len();
    let lang = app.ai.lang.clone();
    app.ai.busy_with(&format!("Extracting text from {pages} page(s)…"));
    let mut texts: Vec<(usize, String)> = Vec::with_capacity(pages);
    {
        let mut ex = pdfcraft_engine::export::Exporter::new(doc);
        for p in 0..pages {
            match ex.text(p) {
                Ok(t) if !t.trim().is_empty() => texts.push((p, t)),
                Ok(_) => {}
                Err(e) => app.ai.note(format!("Page {}: no text extracted ({e})", p + 1)),
            }
        }
    }
    if texts.is_empty() {
        app.ai.provider = Some(provider);
        app.ai.busy = None;
        app.ai.note("This document has no extractable text (scanned PDFs need OCR first)");
        return;
    }
    app.ai.translation = Some(Translation { lang: lang.clone(), pages: Vec::new(), total: texts.len() });
    app.ai.busy_with(&format!("Translating {} page(s) to {lang}…", texts.len()));
    app.ai.spawn(ctx, move |tx| {
        let grammar = craft_ai::PdfGrammar;
        for (page, text) in &texts {
            let segs = craft_ai::segment(text, &grammar);
            match craft_ai::translate_segments(provider.as_mut(), &segs, &lang) {
                Ok(parts) => {
                    let _ = tx.send(Msg::Page(*page, parts.concat()));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Translated(Err(e)));
                    return;
                }
            }
        }
        let _ = tx.send(Msg::Translated(Ok(provider)));
    });
}

// ---------- the panel ----------

/// Green for "ready", red for failures — the same hues status notices use.
const OK: Color32 = Color32::from_rgb(0x2F, 0x9E, 0x44);
const ERR: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x3E);

/// Small accent-coloured "link" text that clicks (as "View more" in Tools).
fn link(ui: &mut egui::Ui, t: &Tokens, text: &str) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).color(t.accent_text).font(theme::medium(12.5))).selectable(false).sense(Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn card(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(t.card)
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .show(ui, add);
}

/// The AI panel body (called from `panels::right_panel`).
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let ctx = ui.ctx().clone();
    let mut action: Option<Action> = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // ---- provider state -------------------------------------------------
        let ai = &mut app.ai;
        if ai.ready() {
            card(ui, &t, |ui| {
                ui.horizontal(|ui| {
                    icons::paint(ui, egui::Rect::from_min_size(ui.cursor().min, egui::vec2(16.0, 16.0)), "circle-check", 16.0, OK);
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(ai.provider_name().unwrap_or_default()).font(theme::medium(12.5)));
                });
                ui.label(
                    egui::RichText::new(if ai.provider_is_local() {
                        tl!("Local — nothing leaves this PC").to_string()
                    } else {
                        tl!("Cloud — document text is sent to the endpoint").to_string()
                    })
                    .small()
                    .color(t.text_muted),
                );
                if link(ui, &t, tl!("Change provider")).clicked() {
                    action = Some(Action::ResetProvider);
                }
            });
            ui.add_space(8.0);
        } else {
            // ---- BYOT card ---------------------------------------------------
            card(ui, &t, |ui| {
                let ai = &mut app.ai;
                ui.label(egui::RichText::new(tl!("Use an API provider")).font(theme::semibold(13.0)));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tl!("OpenAI, Azure, Ollama, LM Studio — any OpenAI-compatible endpoint")).small().color(t.text_muted));
                ui.add_space(6.0);
                egui::Grid::new("ai-byot").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label(tl!("Endpoint:"));
                    ui.add(egui::TextEdit::singleline(&mut ai.endpoint).desired_width(190.0).hint_text("https://api.openai.com/v1"));
                    ui.end_row();
                    ui.label(tl!("Model:"));
                    ui.add(egui::TextEdit::singleline(&mut ai.model).desired_width(190.0).hint_text("gpt-4o-mini"));
                    ui.end_row();
                    ui.label(tl!("API token:"));
                    ui.add(
                        egui::TextEdit::singleline(&mut ai.token).desired_width(190.0).password(true).hint_text(tl!("optional for local endpoints")),
                    );
                    ui.end_row();
                });
                ui.add(widgets::checkbox(&mut ai.remember_token, tl!("Remember token in app settings (stored unencrypted)")));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(ai.busy.is_none(), egui::Button::new(tl!("Test connection"))).clicked() {
                        action = Some(Action::Test);
                    }
                    if ui.add_enabled_ui(ai.busy.is_none(), |ui| widgets::pill_button(ui, tl!("Use this provider"), true)).inner.clicked() {
                        action = Some(Action::UseApi);
                    }
                });
                if let Some(res) = &ai.test_result {
                    let (txt, col) = match res {
                        Ok(m) => (m.clone(), OK),
                        Err(m) => (m.clone(), ERR),
                    };
                    ui.label(egui::RichText::new(txt).small().color(col));
                }
            });
            ui.add_space(8.0);

            // ---- local model card -------------------------------------------
            card(ui, &t, |ui| {
                let ai = &mut app.ai;
                ui.label(egui::RichText::new(tl!("Run on this PC")).font(theme::semibold(13.0)));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tl!("One click downloads a curated open model — everything then runs offline")).small().color(t.text_muted),
                );
                ui.add_space(6.0);
                if let Some(c) = &ai.caps {
                    ui.label(
                        egui::RichText::new(format!(
                            "This PC: {:.0} GB RAM · {} threads · {:.0} GB free disk · AVX2 {}",
                            c.ram_gb,
                            c.threads,
                            c.disk_free_gb,
                            if c.avx2 { "yes" } else { "no" }
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                }
                if ui.add_enabled(ai.busy.is_none(), egui::Button::new(tl!("Detect this PC"))).clicked() {
                    action = Some(Action::Detect);
                }
                if ai.caps.is_some() {
                    ui.add_space(4.0);
                    let chosen = craft_ai::MODELS.iter().position(|m| m.id == ai.local_id);
                    let model_label = ui.label(tl!("Model:"));
                    let combo = craft_ui::ComboBox::from_id_salt("ai-model")
                        .selected_text(chosen.map_or(tl!("Pick a model").to_string(), |i| craft_ai::MODELS[i].name.to_string()))
                        .show_ui(ui, |ui| {
                            for m in craft_ai::MODELS {
                                let fits = ai.caps.as_ref().is_none_or(|c| craft_ai::fits(c, m));
                                ui.add_enabled_ui(fits, |ui| {
                                    ui.selectable_value(
                                        &mut ai.local_id,
                                        m.id.to_string(),
                                        format!("{} — {:.0} MB · {}", m.name, m.size_mb, m.license),
                                    )
                                    .on_hover_text(m.langs);
                                });
                            }
                        });
                    combo.response.labelled_by(model_label.id);
                    let downloaded = ai.downloaded_ids();
                    let picked_downloaded = downloaded.contains(&ai.local_id);
                    ui.add_space(4.0);
                    if let Some((done, total, _bps)) = ai.dl {
                        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(frac).show_percentage());
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(ai.busy.is_none() && !ai.local_id.is_empty() && !picked_downloaded, egui::Button::new(tl!("Download")))
                            .clicked()
                        {
                            action = Some(Action::Download);
                        }
                        if ui.add_enabled(ai.busy.is_none() && picked_downloaded, egui::Button::new(tl!("Load model"))).clicked() {
                            action = Some(Action::LoadLocal);
                        }
                    });
                }
            });
        }

        // ---- operations -----------------------------------------------------
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tl!("Translate document")).font(theme::semibold(13.0)));
        ui.add_space(4.0);
        let ai = &mut app.ai;
        ui.horizontal(|ui| {
            let lang_label = ui.label(tl!("To:"));
            let combo = craft_ui::ComboBox::from_id_salt("ai-lang").selected_text(ai.lang.clone()).width(150.0).show_ui(ui, |ui| {
                for l in LANGUAGES {
                    ui.selectable_value(&mut ai.lang, l.to_string(), *l);
                }
            });
            combo.response.labelled_by(lang_label.id);
            if ui
                .add_enabled_ui(ai.ready() && ai.busy.is_none() && app.active.is_some(), |ui| widgets::pill_button(ui, tl!("Translate"), true))
                .inner
                .on_hover_text(if app.active.is_none() { tl!("Open a document first").to_string() } else { String::new() })
                .clicked()
            {
                action = Some(Action::Translate);
            }
        });

        // ---- status + result ------------------------------------------------
        if let Some(busy) = &ai.busy {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(egui::RichText::new(busy).small().color(t.text_muted));
            });
        }
        if let Some(tr) = &ai.translation {
            ui.add_space(8.0);
            ui.separator();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Translated to {lang}"), &[("lang", &tr.lang)])).font(theme::semibold(12.5)));
            let mut pages = tr.pages.clone();
            pages.sort_by_key(|(p, _)| *p);
            for (page, text) in &pages {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(tl!("Page {n}"), &[("n", &(*page + 1).to_string())]))
                        .font(theme::medium(11.0))
                        .color(t.text_faint),
                );
                ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.0))).selectable(true).wrap());
            }
        }

        // ---- activity log ----------------------------------------------------
        ui.add_space(10.0);
        let ai = &mut app.ai;
        if link(ui, &t, if ai.log_open { tl!("Hide activity log") } else { tl!("Activity log") }).clicked() {
            ai.log_open = !ai.log_open;
        }
        if ai.log_open {
            egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                for line in &ai.log {
                    ui.label(egui::RichText::new(line).small().color(t.text_muted));
                }
            });
        }
    });

    // Deferred actions (cannot borrow ai while the scroll closure owns it).
    match action {
        Some(Action::Test) => app.ai.test_endpoint(&ctx),
        Some(Action::UseApi) => app.ai.use_api_provider(),
        Some(Action::Detect) => app.ai.detect(),
        Some(Action::Download) => app.ai.download_picked(&ctx),
        Some(Action::LoadLocal) => app.ai.load_downloaded(&ctx),
        Some(Action::ResetProvider) => app.ai.reset_provider(),
        Some(Action::Translate) => translate_active(app, &ctx),
        None => {}
    }
}

enum Action {
    Test,
    UseApi,
    Detect,
    Download,
    LoadLocal,
    ResetProvider,
    Translate,
}
