//! View ▸ Side panels ▸ Assistant: summaries of, and questions about, the open PDF with a model
//! that already runs on this computer (`pdfcraft_assistant`).
//!
//! Off until the person turns it on: until then it makes no request of any kind. Reading the
//! pages and their origins is local and runs in the background; the model is asked only when the
//! person presses Summarise or Ask. Each answer lists the pages its verified quotations come from
//! (each a link to the page) and says whether that text was extracted or recognised by OCR.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText, Stroke};
use pdfcraft_assistant::{DocText, Endpoint, Kind, ModelInfo, Origin, Outcome, Settings, Stage, Task, context, provider};
use pdfcraft_engine::{DocId, Session};

use crate::theme::{self, Tokens};

/// Below this mean confidence (0-100) a page recognised now is illegible: the original's "low" band.
pub const MIN_OCR_CONFIDENCE: f32 = pdfcraft_assistant::pages::LOW_CONFIDENCE;

/// A background job and its result.
struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    fn spawn(name: &str, ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Result<Job<T>, String>
    where
        T: From<String>,
    {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                // A panic in the worker becomes an error message, never a crash of the window.
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .unwrap_or_else(|_| T::from("error interno del asistente".to_owned()));
                let _ = tx.send(out);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Job { rx })
    }

    /// The result once it's there; `Some(Err)` if the worker vanished.
    fn poll(&mut self) -> Option<Result<T, ()>> {
        match self.rx.try_recv() {
            Ok(v) => Some(Ok(v)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(())),
        }
    }
}

/// `Result<T, String>` that a worker can build from an error text.
struct Res<T>(Result<T, String>);

type OcrPages = Vec<(usize, String, Option<f32>)>;

impl<T> From<String> for Res<T> {
    fn from(e: String) -> Self {
        Res(Err(e))
    }
}

struct Running {
    job: Job<Res<Outcome>>,
    cancel: Arc<AtomicBool>,
    stage: Arc<Mutex<Option<Stage>>>,
    task: Task,
    model: String,
}

/// The panel's state, for the whole session. Only the chosen local server and model are
/// remembered between sessions ([`AssistantState::save_prefs`]); the external-provider permission
/// and any API key never are.
pub struct AssistantState {
    pub enabled: bool,
    spanish: bool,
    kind: Kind,
    url: String,
    model: String,
    api_key: String,
    /// The person allowed, for this session, sending text to the non-local `allowed_for` address.
    allowed_for: Option<String>,
    show_provider: bool,
    models: Option<Result<Vec<ModelInfo>, String>>,
    models_job: Option<Job<Res<Vec<ModelInfo>>>>,
    /// The document (and version of its bytes) the text below was read from.
    read_from: Option<(DocId, usize)>,
    reading: Option<Job<Res<DocText>>>,
    text: Option<Result<DocText, String>>,
    ocr: Option<Job<Res<OcrPages>>>,
    ocr_note: Option<String>,
    question: String,
    running: Option<Running>,
    /// Cancelled requests still closing their connection; their answers are discarded.
    stopping: Vec<Running>,
    /// The last answer: what was asked, which model answered, and the result.
    outcome: Option<(Task, String, Result<Outcome, String>)>,
    /// What the AI assistant tools asked for, waiting for the document to be read.
    pending: Option<Request>,
    focus_question: bool,
}

/// A request from the AI assistant tools (All tools ▸ AI assistant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Summary,
    Ask,
    Translate,
}

impl AssistantState {
    fn fresh() -> Self {
        AssistantState {
            enabled: false,
            spanish: true,
            kind: Kind::Ollama,
            url: "http://127.0.0.1:11434".into(),
            model: String::new(),
            api_key: String::new(),
            allowed_for: None,
            show_provider: false,
            models: None,
            models_job: None,
            read_from: None,
            reading: None,
            text: None,
            ocr: None,
            ocr_note: None,
            question: String::new(),
            running: None,
            stopping: Vec::new(),
            outcome: None,
            pending: None,
            focus_question: false,
        }
    }
}

impl Default for AssistantState {
    fn default() -> Self {
        Self::fresh().with_prefs()
    }
}

impl Drop for AssistantState {
    fn drop(&mut self) {
        self.cancel_running();
    }
}

/// Where the chosen model is remembered: the application settings folder.
fn prefs_path() -> Option<std::path::PathBuf> {
    eframe::storage_dir("PdfCraft").map(|d| d.join("assistant.json"))
}

impl AssistantState {
    /// The model and local server chosen in an earlier session, if any.
    fn with_prefs(mut self) -> Self {
        use std::io::Read as _;
        let Some(v) = prefs_path().and_then(|p| {
            let mut text = String::new();
            std::fs::File::open(p).ok()?.take(32_769).read_to_string(&mut text).ok()?;
            if text.len() > 32_768 {
                return None;
            }
            serde_json::from_str::<serde_json::Value>(&text).ok()
        }) else {
            return self;
        };
        self.restore_prefs(&v);
        self
    }

    fn restore_prefs(&mut self, v: &serde_json::Value) {
        self.spanish = v.get("language").and_then(|l| l.as_str()) != Some("en");
        let url = v.get("url").and_then(|u| u.as_str()).filter(|u| u.len() <= 4096).unwrap_or_default();
        // Remember a chosen subscription CLI, but never its session consent or any API key.
        let saved_kind = v.get("kind").and_then(|k| k.as_str());
        if matches!(saved_kind, Some("claude-code" | "codex")) && !url.trim().is_empty() {
            self.url = url.to_owned();
            self.kind = if saved_kind == Some("claude-code") { Kind::ClaudeCode } else { Kind::Codex };
            self.show_provider = true;
        } else if provider::is_local(url) == Ok(true) {
            self.url = url.to_owned();
            self.kind = if saved_kind == Some("openai") { Kind::OpenAi } else { Kind::Ollama };
        }
        if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
            self.model = m.chars().take(200).collect();
        }
    }

    /// Remember the chosen model (and the server, when it is on this computer).
    fn save_prefs(&self) {
        let Some(path) = prefs_path() else { return };
        let mut v = serde_json::json!({ "model": self.model, "language": if self.spanish { "es" } else { "en" } });
        if self.kind.is_cli() || self.local() == Ok(true) {
            v["url"] = serde_json::json!(self.url.trim());
            v["kind"] = serde_json::json!(match self.kind {
                Kind::ClaudeCode => "claude-code",
                Kind::Codex => "codex",
                Kind::OpenAi => "openai",
                Kind::Ollama => "ollama",
            });
        }
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, v.to_string())?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            log::warn!("assistant: could not remember the chosen model: {e}");
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint {
            kind: self.kind,
            base_url: self.url.trim().to_owned(),
            model: self.model.clone(),
            api_key: (!self.api_key.is_empty()).then(|| self.api_key.clone()),
        }
    }

    fn local(&self) -> Result<bool, String> {
        if self.kind.is_cli() { Ok(false) } else { provider::is_local(self.url.trim()) }
    }

    /// Whether requests may go to the current address now.
    fn may_send(&self) -> bool {
        if provider::missing_api_key(&self.endpoint()) {
            return false;
        }
        match self.local() {
            Ok(true) => true,
            Ok(false) => self.allowed_for.as_deref() == Some(self.url.trim()),
            Err(_) => false,
        }
    }

    fn refresh_models(&mut self, ctx: &egui::Context) {
        if !self.may_send() {
            self.models = None;
            return;
        }
        let ep = self.endpoint();
        self.models = None;
        self.models_job = Job::spawn("assistant-models", ctx, move || Res(provider::list_models(&ep))).ok();
    }

    fn start_reading(&mut self, ctx: &egui::Context, id: DocId, version: usize, bytes: Arc<Vec<u8>>, password: Option<String>) {
        self.read_from = Some((id, version));
        self.text = None;
        self.outcome = None;
        self.ocr_note = None;
        self.reading =
            Job::spawn("assistant-read", ctx, move || Res(pdfcraft_assistant::pages::read_document(bytes, password.as_deref(), &|| false))).ok();
    }

    fn poll(&mut self) {
        if let Some(job) = &mut self.models_job
            && let Some(r) = job.poll()
        {
            self.models_job = None;
            let r = r.map_or_else(|()| Err("se interrumpió la consulta de modelos".to_owned()), |r| r.0);
            if let Ok(list) = &r {
                // Prefer a model already in memory, then the smallest: nothing new is loaded
                // that need not be, and nothing is ever downloaded.
                if !list.iter().any(|m| m.name == self.model) {
                    let mut sorted: Vec<&ModelInfo> = list.iter().collect();
                    sorted.sort_by_key(|m| (!m.resident, m.size_bytes == 0, m.size_bytes));
                    self.model = sorted.first().map(|m| m.name.clone()).unwrap_or_default();
                }
            }
            self.models = Some(r);
        }
        if let Some(job) = &mut self.reading
            && let Some(r) = job.poll()
        {
            self.reading = None;
            self.text = Some(r.map_or_else(|()| Err("se interrumpió la lectura".to_owned()), |r| r.0));
        }
        if let Some(job) = &mut self.ocr
            && let Some(r) = job.poll()
        {
            self.ocr = None;
            match r.map_or_else(|()| Err("se interrumpió el OCR".to_owned()), |r| r.0) {
                Ok(pages) => {
                    if let Some(Ok(text)) = &mut self.text {
                        let (mut ok, mut bad) = (0, 0);
                        for (page, words, confidence) in pages {
                            text.apply_ocr(page, words, confidence, MIN_OCR_CONFIDENCE);
                            if text.page(page).is_some_and(|p| p.origin.readable()) {
                                ok += 1;
                            } else {
                                bad += 1;
                            }
                        }
                        self.ocr_note = Some(format!("OCR local: {ok} página(s) leídas, {bad} ilegible(s). El PDF no se modificó."));
                    }
                }
                Err(e) => self.ocr_note = Some(format!("El OCR no pudo leer las páginas: {e}")),
            }
        }
        // Cancelled requests finish on their own (at the next fragment); nothing waits for them.
        self.stopping.retain_mut(|r| r.job.poll().is_none());
        if let Some(run) = &mut self.running
            && let Some(r) = run.job.poll()
        {
            let task = run.task.clone();
            let model = run.model.clone();
            let cancelled = run.cancel.load(Ordering::Relaxed);
            self.running = None;
            if !cancelled {
                self.outcome = Some((task, model, r.map_or_else(|()| Err("se interrumpió la consulta".to_owned()), |r| r.0)));
            }
        }
    }

    /// Stop the running request: its connection is closed at the next fragment (so the model
    /// stops) and the panel is free for a new request at once.
    pub(crate) fn cancel_running(&mut self) {
        if let Some(run) = self.running.take() {
            run.cancel.store(true, Ordering::Relaxed);
            self.stopping.push(run);
        }
    }

    /// The AI assistant tools' entry point: choosing one of them turns the assistant on (that
    /// choice is the person's) and queues what was asked (`summary`, `ask` or `translate`).
    pub fn request(&mut self, what: &str) -> bool {
        let req = match what {
            "summary" => Request::Summary,
            "ask" => Request::Ask,
            "translate" => Request::Translate,
            _ => return false,
        };
        self.enabled = true;
        self.pending = Some(req);
        true
    }

    fn ask(&mut self, ctx: &egui::Context, task: Task) {
        let Some(Ok(text)) = &self.text else { return };
        let ep = self.endpoint();
        let doc = text.clone();
        let settings =
            Settings { allow_external: self.allowed_for.as_deref() == Some(self.url.trim()), spanish: self.spanish, ..Settings::default() };
        let cancel = Arc::new(AtomicBool::new(false));
        let stage = Arc::new(Mutex::new(None));
        let (c, s, t) = (cancel.clone(), stage.clone(), task.clone());
        let repaint = ctx.clone();
        let job = Job::spawn("assistant-ask", ctx, move || {
            Res(pdfcraft_assistant::run(&ep, &doc, &t, &settings, &c, |st| {
                if let Ok(mut g) = s.lock() {
                    *g = Some(st);
                }
                repaint.request_repaint();
            }))
        });
        match job {
            Ok(job) => {
                self.outcome = None;
                self.running = Some(Running {
                    job,
                    cancel,
                    stage,
                    task,
                    model: format!("{} · {} · {}", self.kind.label(), self.model, if self.spanish { "Español" } else { "English" }),
                });
            }
            Err(e) => self.outcome = Some((task, self.model.clone(), Err(format!("no se pudo iniciar la consulta: {e}")))),
        }
    }

    fn run_ocr(&mut self, ctx: &egui::Context, session: &Session, id: DocId, pages: Vec<usize>) {
        let Some(job) = session.ocr_job(id, &pages, pdfcraft_engine::ocr::OcrSettings::default()) else {
            self.ocr_note = Some("No se pudo preparar el OCR para este documento.".into());
            return;
        };
        let settings = job.settings.clone();
        self.ocr_note = None;
        self.ocr = Job::spawn("assistant-ocr", ctx, move || {
            Res(pdfcraft_engine::ocr::recognizers(&settings).map(|r| {
                job.run(&r, |_, _| true)
                    .into_iter()
                    .map(|p| (p.page, if p.skipped.is_some() { String::new() } else { p.text() }, p.mean_confidence()))
                    .collect()
            }))
        })
        .ok();
    }
}

fn pill(ui: &mut egui::Ui, text: &str, fill: Color32, fg: Color32) {
    egui::Frame::NONE.fill(fill).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).font(theme::medium(11.0)).color(fg));
    });
}

fn warn_box(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(0xd9, 0x8a, 0x00, 0x22))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0xc0, 0x7a, 0x00)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).font(theme::regular(12.0)).color(t.text));
        });
}

fn origin_colors(t: &Tokens, origin: &Origin) -> (Color32, Color32) {
    if origin.is_ocr() { (Color32::from_rgba_unmultiplied(0x9b, 0x59, 0xb6, 0x30), t.text) } else { (t.accent_soft, t.text) }
}

fn provider_ui(ui: &mut egui::Ui, t: &Tokens, s: &mut AssistantState, ctx: &egui::Context) {
    // Where requests go.
    let local = s.local();
    ui.horizontal_wrapped(|ui| {
        let host = if s.kind.is_cli() {
            "suscripción del usuario".into()
        } else {
            provider::host_of(s.url.trim()).unwrap_or_else(|| "dirección no válida".into())
        };
        match local {
            Ok(true) => pill(ui, "local", Color32::from_rgba_unmultiplied(0x2e, 0x9e, 0x5b, 0x40), t.text),
            Ok(false) => pill(ui, "EXTERNO", Color32::from_rgba_unmultiplied(0xd0, 0x30, 0x30, 0x50), t.text),
            Err(_) => pill(ui, "inválida", Color32::from_rgba_unmultiplied(0xd0, 0x30, 0x30, 0x50), t.text),
        }
        let kind = s.kind.label();
        ui.label(RichText::new(format!("{kind} · {host}")).color(t.text_muted));
        if ui.small_button(if s.show_provider { "Ocultar" } else { "Cambiar" }).clicked() {
            s.show_provider = !s.show_provider;
        }
    });
    if s.show_provider {
        egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let before = (s.kind, s.url.clone());
                ui.label(RichText::new("Con su sesión existente, sin clave API").strong());
                crate::widgets::radio_value(ui, &mut s.kind, Kind::Codex, "Codex · cuenta ChatGPT");
                crate::widgets::radio_value(ui, &mut s.kind, Kind::ClaudeCode, "Claude Code · cuenta Claude");
                ui.separator();
                ui.label("Otras opciones");
                crate::widgets::radio_value(ui, &mut s.kind, Kind::Ollama, "Ollama · modelo local");
                crate::widgets::radio_value(ui, &mut s.kind, Kind::OpenAi, "API externa · clave propia y facturación aparte");
                if s.kind.is_cli() {
                    ui.label("No ingrese una clave API. Se usa la sesión del programa ya instalado.");
                    ui.collapsing("Ubicación del programa (avanzado)", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.url).desired_width(f32::INFINITY));
                    });
                } else {
                    ui.label("Dirección del proveedor");
                    ui.add(egui::TextEdit::singleline(&mut s.url).hint_text("http://127.0.0.1:11434").desired_width(f32::INFINITY));
                }
                if before.0 != s.kind && before.1 == s.url {
                    s.url = match s.kind { Kind::Ollama => "http://127.0.0.1:11434".into(), Kind::OpenAi => "https://api.openai.com/v1".into(), _ => pdfcraft_assistant::cli::default_program(s.kind) };
                    s.model.clear(); s.api_key.clear();
                }
                if (s.kind, s.url.clone()) != before {
                    // A new address needs its own permission, and its own model list.
                    s.cancel_running();
                    s.api_key.clear();
                    s.allowed_for = None;
                    s.models = None;
                    s.models_job = None;
                    s.outcome = None;
                    s.save_prefs();
                }
                if s.local() == Ok(false) {
                    warn_box(
                        ui,
                        t,
                        if s.kind.is_cli() { "El CLI enviará el texto seleccionado al proveedor de su cuenta. Se usa su sesión existente, sin claves API heredadas ni fallback de pago. Puede consumir su cuota. Autorice sólo documentos que pueda compartir." } else { "El texto se enviará al proveedor elegido. Una API puede generar cargos en su cuenta. La clave no se guarda. Autorice sólo documentos que pueda compartir." },
                    );
                    if !s.kind.is_cli() && ui.add(egui::TextEdit::singleline(&mut s.api_key).password(true).hint_text("clave de API (no se guarda)").desired_width(f32::INFINITY)).changed() { s.models = None; }
                    let url = s.url.trim().to_owned();
                    let mut allowed = s.allowed_for.as_deref() == Some(url.as_str());
                    if ui.add(crate::widgets::checkbox(&mut allowed, format!("Autorizo enviar el texto de mis documentos a {}", if s.kind.is_cli() { s.kind.label().into() } else { provider::host_of(&url).unwrap_or_default() }))).changed() {
                        if !allowed { s.cancel_running(); }
                        s.allowed_for = allowed.then_some(url);
                        s.models = None;
                    }
                }
                if ui.button(if s.kind.is_cli() { "Usar esta sesión" } else { "Buscar modelos" }).clicked() {
                    s.refresh_models(ctx);
                }
            });
    }
    if s.models.is_none() && s.models_job.is_none() && s.may_send() && (s.kind.is_cli() || s.local() == Ok(true)) {
        s.refresh_models(ctx);
    }
    // Model.
    match &s.models {
        None if s.models_job.is_some() => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Buscando modelos instalados…");
            });
        }
        None if !s.may_send() => {
            ui.label(RichText::new(if provider::missing_api_key(&s.endpoint()) { "Falta una clave API de OpenAI. Su sesión de Codex se usa eligiendo Codex (OpenAI), sin clave API. No se realiza ninguna solicitud." } else { "Marque la autorización para habilitar las consultas. Todavía no se envió texto." }).color(t.text_muted));
        }
        None => {
            ui.label("Pulse Buscar modelos cuando haya completado la configuración.");
        }
        Some(Err(e)) => {
            warn_box(ui, t, &format!("No se pudo consultar el proveedor: {e}\nRevise la configuración y pulse Reintentar."));
            if ui.small_button("Reintentar").clicked() {
                s.refresh_models(ctx);
            }
        }
        Some(Ok(list)) if list.is_empty() => warn_box(ui, t, "El proveedor no devolvió modelos. Esta app no descarga modelos."),
        Some(Ok(list)) => {
            let list = list.clone();
            let before = s.model.clone();
            // Every installed model, in view: the person picks which one answers.
            egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Modelo de IA").font(theme::semibold(13.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Actualizar lista").on_hover_text("Vuelve a leer los modelos ya instalados (no descarga ninguno)").clicked() {
                                s.refresh_models(ctx);
                            }
                        });
                    });
                    ui.label(RichText::new(format!("{} opciones disponibles. Elija cuál responde:", list.len())).color(t.text_muted).small());
                    ui.add_space(2.0);
                    for m in &list {
                        let mut detail = Vec::new();
                        if m.size_bytes > 0 {
                            detail.push(format!("{:.1} GB", m.size_bytes as f64 / 1e9));
                        }
                        if m.resident {
                            detail.push("ya en memoria: responde antes".to_owned());
                        }
                        ui.horizontal(|ui| {
                            crate::widgets::radio_value(ui, &mut s.model, m.name.clone(), RichText::new(&m.name).font(theme::medium(12.5)));
                            if !detail.is_empty() {
                                ui.label(RichText::new(detail.join(" · ")).color(t.text_faint).small());
                            }
                        });
                    }
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(if s.kind.is_cli() { "Se usa el modelo predeterminado del programa. Las consultas pueden consumir su cuota de suscripción; no se utiliza una clave API." } else { "Elija un modelo del proveedor. Ollama no descarga modelos; una API externa puede generar cargos." })
                            .color(t.text_faint)
                            .small(),
                    );
                });
            if s.model != before {
                s.save_prefs();
            }
        }
    }
    if !s.model.is_empty() && s.models.as_ref().is_some_and(Result::is_ok) {
        ui.label(RichText::new(format!("Responderá: {} · {}", s.kind.label(), s.model)).color(t.text_muted));
    }
    ui.separator();
}

fn document_text_ui(ui: &mut egui::Ui, t: &Tokens, s: &mut AssistantState, ctx: &egui::Context, session: &Session, id: DocId) -> Option<usize> {
    let mut go = None;
    // What the assistant can read.
    let mut want_ocr: Option<Vec<usize>> = None;
    match (&s.text, &s.reading) {
        (_, Some(_)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Leyendo el texto de las páginas (en esta Mac)…");
            });
        }
        (Some(Err(e)), None) => warn_box(ui, t, &format!("No se pudo leer el documento: {e}")),
        (Some(Ok(text)), None) => {
            let n = text.pages.len();
            let extracted = text.count(|o| *o == Origin::Extracted);
            let ocr = text.count(Origin::is_ocr);
            let scans = text.scanned_without_text();
            let illegible = text.count(|o| matches!(o, Origin::Illegible { .. }));
            let empty = text.count(|o| matches!(o, Origin::NoText { pictures: false } | Origin::Failed(_)));
            ui.label(RichText::new(format!("{n} páginas · {extracted} con texto extraído · {ocr} con OCR · {} escaneadas sin texto · {illegible} ilegibles · {empty} vacías o con error", scans.len())).color(t.text_muted));
            if text.unread_pages > 0 {
                warn_box(ui, t, &format!("El documento es muy largo: {} páginas finales no se leyeron.", text.unread_pages));
            }
            if !scans.is_empty() {
                warn_box(
                    ui,
                    t,
                    &format!(
                        "Páginas escaneadas sin texto: {}. El asistente no puede leerlas ni citarlas hasta reconocer su texto.",
                        context::list(&scans)
                    ),
                );
                if s.ocr.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reconociendo texto con el OCR de la app (en esta Mac)…");
                    });
                } else if pdfcraft_engine::ocr::available() {
                    if ui.button("Reconocer esas páginas con OCR local (no modifica el PDF)").clicked() {
                        want_ocr = Some(scans.clone());
                    }
                } else {
                    ui.label(
                        RichText::new("El OCR no está disponible en esta copia (faltan sus modelos), así que esas páginas quedan sin leer.")
                            .color(t.text_muted),
                    );
                }
            }
            let illegible_pages: Vec<usize> = text.pages.iter().filter(|p| matches!(p.origin, Origin::Illegible { .. })).map(|p| p.page).collect();
            if !illegible_pages.is_empty() {
                warn_box(ui, t, &format!("El OCR no pudo leer las páginas {}: quedan fuera de las respuestas.", context::list(&illegible_pages)));
            }
            if let Some(note) = &s.ocr_note {
                ui.label(RichText::new(note).color(t.text_muted).small());
            }
            egui::CollapsingHeader::new("Origen del texto por página").id_salt("assistant-origins").show(ui, |ui| {
                for p in text.pages.iter().take(500) {
                    ui.horizontal(|ui| {
                        if ui.link(format!("p. {}", p.page + 1)).clicked() {
                            go = Some(p.page);
                        }
                        ui.label(RichText::new(p.origin.label()).color(t.text_muted).small());
                        if p.truncated {
                            ui.label(RichText::new("(texto incompleto)").color(t.text_faint).small());
                        }
                    });
                }
            });
        }
        (None, None) => {}
    }
    if let Some(pages) = want_ocr {
        s.run_ocr(ctx, session, id, pages);
    }
    ui.separator();

    go
}

fn ask_ui(ui: &mut egui::Ui, t: &Tokens, s: &mut AssistantState, ctx: &egui::Context, current_page: usize) {
    // Ask.
    let ready = matches!(&s.text, Some(Ok(_))) && s.running.is_none() && s.ocr.is_none() && !s.model.is_empty() && s.may_send();
    // A request from the AI assistant tools (Summarize, Translate) runs as soon as it can.
    if ready {
        match s.pending.take() {
            Some(Request::Summary) => s.ask(ctx, Task::Summary),
            Some(Request::Translate) => {
                s.ask(ctx, Task::Translate { from: current_page, language: if s.spanish { "español".into() } else { "inglés".into() } })
            }
            Some(Request::Ask) => s.focus_question = true,
            None => {}
        }
    }
    if let Some(p) = &s.pending {
        let what = match p {
            Request::Summary => "el resumen",
            Request::Translate => "la traducción",
            Request::Ask => "la pregunta",
        };
        ui.label(RichText::new(format!("Preparando {what}: falta leer el documento o elegir un modelo disponible.")).color(t.text_muted));
    }
    ui.add_enabled_ui(ready, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button(if s.spanish { "Resumir el documento" } else { "Summarize document" }).clicked() {
                s.ask(ctx, Task::Summary);
            }
            if ui
                .button(format!("{} (p. {})", if s.spanish { "Traducir al español" } else { "Translate into English" }, current_page + 1))
                .on_hover_text("La página que se ve y las siguientes que entren en una consulta")
                .clicked()
            {
                s.ask(ctx, Task::Translate { from: current_page, language: if s.spanish { "español".into() } else { "inglés".into() } });
            }
        });
        let field = ui.add(
            egui::TextEdit::multiline(&mut s.question)
                .hint_text(if s.spanish { "Pregunta sobre el documento…" } else { "Ask about this document…" })
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut s.focus_question) {
            field.request_focus();
        }
        if ui.add_enabled(!s.question.trim().is_empty(), egui::Button::new(if s.spanish { "Preguntar" } else { "Ask" })).clicked() {
            let q = s.question.trim().to_owned();
            s.ask(ctx, Task::Question(q));
        }
    });
}

/// Draw the panel. Returns a page (0-based) to show, when a page link was clicked.
pub fn panel(ui: &mut egui::Ui, t: &Tokens, s: &mut AssistantState, session: &Session, id: DocId, current_page: usize) -> Option<usize> {
    let ctx = ui.ctx().clone();
    s.poll();
    let mut go = None;
    let doc = session.get(id)?;
    if !doc.allows_copying() || doc.redaction_marks() > 0 {
        s.cancel_running();
        s.text = None;
        s.outcome = None;
        s.pending = None;
        ui.label("The assistant cannot read a copy-restricted document or a document with unapplied redactions.");
        return None;
    }
    if s.read_from.is_some_and(|(read_id, version)| read_id != id || version != Arc::as_ptr(&doc.bytes) as usize) {
        s.cancel_running();
        s.reading = None;
        s.ocr = None;
        s.text = None;
        s.outcome = None;
    }
    if !s.enabled {
        ui.add_space(6.0);
        ui.label(RichText::new("Asistente de IA (optativo)").font(theme::semibold(14.0)));
        ui.add_space(4.0);
        ui.label(RichText::new(
            "Resume el PDF abierto y responde preguntas sobre él con Ollama local, Claude Code, Codex o un proveedor OpenAI compatible elegido por usted. \
             Cada respuesta indica las páginas de donde sale y si el texto es extraído u OCR; sólo se muestran citas verificadas contra el texto.",
        ).color(t.text_muted));
        ui.add_space(6.0);
        ui.label(RichText::new("Ollama puede procesar localmente. Claude Code, Codex y una API externa envían texto al proveedor: requieren autorización por sesión. No se descargan modelos ni se inicia sesión automáticamente.").color(t.text_muted).small());
        ui.add_space(10.0);
        if ui.button("Activar asistente").clicked() {
            s.enabled = true;
            s.refresh_models(&ctx);
        }
        return None;
    }

    // The document's text: read again when the document changes.
    let version = Arc::as_ptr(&doc.bytes) as usize;
    if s.read_from != Some((id, version)) && s.reading.is_none() && s.running.is_none() {
        s.start_reading(&ctx, id, version, doc.bytes.clone(), doc.password.clone());
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let before_language = s.spanish;
        ui.label(RichText::new("Idioma de respuestas / Answer language").strong());
        ui.add_enabled_ui(s.running.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                crate::widgets::radio_value(ui, &mut s.spanish, true, "Español");
                crate::widgets::radio_value(ui, &mut s.spanish, false, "English");
            });
        });
        if before_language != s.spanish { s.outcome = None; s.save_prefs(); }
        ui.label(RichText::new(if s.spanish { "Las citas conservan el idioma original del PDF." } else { "Quotations keep the PDF's original language." }).small().color(t.text_muted));
        provider_ui(ui, t, s, &ctx);
        if let Some(page) = document_text_ui(ui, t, s, &ctx, session, id) { go = Some(page); }
        ask_ui(ui, t, s, &ctx, current_page);
        let mut cancel = false;
        if let Some(run) = &s.running {
            let stage = run.stage.lock().ok().and_then(|g| g.clone());
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(match stage {
                    Some(Stage::Queued(secs)) => format!("Esperando candado local (otra tarea usa los modelos): {secs} s"),
                    Some(Stage::Answering) => format!("{} está respondiendo…", run.model),
                    None => "Preparando la consulta…".to_owned(),
                });
                cancel = ui.small_button("Cancelar").clicked();
            });
        }
        if cancel {
            s.cancel_running();
        }
        if !s.stopping.is_empty() {
            ui.label(
                RichText::new("Consulta cancelada. La respuesta se descartará y se cerrará la conexión o el proceso de esta consulta. La cancelación no garantiza devolución de cuota del proveedor.")
                    .color(t.text_faint)
                    .small(),
            );
        }

        // The answer.
        if let Some((task, model, outcome)) = &s.outcome {
            ui.separator();
            let title = match task {
                Task::Summary => "Resumen".to_owned(),
                Task::Question(q) => format!("Pregunta: {q}"),
                Task::Translate { language, .. } => format!("Traducción al {language}"),
            };
            ui.label(RichText::new(title).font(theme::semibold(13.5)));
            match outcome {
                Err(e) => warn_box(ui, t, e),
                Ok(o) => {
                    let p = &o.prompt;
                    let mut notes = vec![format!("Texto enviado: páginas {}", context::list(&p.included))];
                    if !p.partial.is_empty() {
                        notes.push(format!("en parte: {}", context::list(&p.partial)));
                    }
                    if !p.omitted.is_empty() {
                        notes.push(format!("no enviadas por longitud: {}", context::list(&p.omitted)));
                    }
                    if !p.unreadable.is_empty() {
                        notes.push(format!("sin texto legible: {}", context::list(&p.unreadable)));
                    }
                    notes.push(format!(
                        "modelo {model} · {} · espera {:.0} s · respuesta {:.0} s",
                        if o.local { "en esta Mac" } else { "PROVEEDOR EXTERNO" },
                        o.queue_seconds,
                        o.answer_seconds
                    ));
                    ui.label(RichText::new(notes.join(" · ")).color(t.text_faint).small());
                    let a = &o.answer;
                    if let Some(raw) = &a.unparsed {
                        warn_box(ui, t, &format!("La respuesta del modelo no tenía el formato pedido, así que no se muestra como respuesta verificada. Comienzo: {raw}"));
                    }
                    if a.not_found && a.points.iter().all(|x| !x.supported()) {
                        warn_box(ui, t, "El modelo indica que el texto enviado no contiene la respuesta.");
                    }
                    let text = match &s.text {
                        Some(Ok(text)) => Some(text),
                        _ => None,
                    };
                    for (page, translated) in &a.translations {
                        egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                if ui.link(format!("p. {}", page + 1)).on_hover_text("Ir a la página").clicked() {
                                    go = Some(*page);
                                }
                                if let Some(origin) = text.and_then(|d| d.page(*page)).map(|p| p.origin.clone()) {
                                    let (fill, fg) = origin_colors(t, &origin);
                                    pill(ui, &format!("traducido de {}", origin.label()), fill, fg);
                                }
                            });
                            ui.add(egui::Label::new(RichText::new(translated).color(t.text)).selectable(true));
                        });
                    }
                    if matches!(task, Task::Translate { .. }) {
                        if a.translations.is_empty() && a.unparsed.is_none() {
                            warn_box(ui, t, "El modelo no devolvió traducciones de las páginas enviadas.");
                        }
                        ui.label(
                            RichText::new("Traducción automática hecha por el proveedor elegido: no se puede verificar palabra por palabra. Para citar, use el texto original de la página.")
                                .color(t.text_faint)
                                .small(),
                        );
                    }
                    for (i, point) in a.points.iter().enumerate() {
                        egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(format!("{}.", i + 1)).font(theme::semibold(12.5)));
                                if point.supported() {
                                    let pages: Vec<String> = point.pages().iter().map(|p| format!("p. {}", p + 1)).collect();
                                    ui.label(RichText::new(format!("Fuente: {}", pages.join(", "))).color(t.text_muted).small());
                                } else {
                                    pill(ui, "sin cita verificada: no tomar como dato del documento", Color32::from_rgba_unmultiplied(0xd0, 0x30, 0x30, 0x40), t.text);
                                }
                            });
                            ui.label(RichText::new(&point.statement).color(if point.supported() { t.text } else { t.text_muted }));
                            for q in &point.quotes {
                                ui.horizontal_wrapped(|ui| {
                                    if ui.link(format!("p. {}", q.page + 1)).on_hover_text("Ir a la página").clicked() {
                                        go = Some(q.page);
                                    }
                                    let (fill, fg) = origin_colors(t, &q.origin);
                                    pill(ui, &q.origin.label(), fill, fg);
                                    if let Some(c) = q.claimed_page {
                                        ui.label(RichText::new(format!("(el modelo indicó p. {c})")).color(t.text_faint).small());
                                    }
                                });
                                ui.label(RichText::new(format!("«{}»", q.text)).italics().color(t.text_muted));
                            }
                        });
                    }
                    if !a.rejected.is_empty() {
                        egui::CollapsingHeader::new(format!("Citas descartadas por no figurar en el texto ({})", a.rejected.len())).id_salt("assistant-rejected").show(ui, |ui| {
                            for r in &a.rejected {
                                let page = r.claimed_page.map(|p| format!("p. {p}")).unwrap_or_else(|| "sin página".into());
                                ui.label(RichText::new(format!("{page}: «{}» — {}", r.text, r.reason)).color(t.text_faint).small());
                            }
                        });
                    }
                    if !matches!(task, Task::Translate { .. }) {
                        ui.label(RichText::new("Las frases numeradas son la síntesis del modelo; sólo las citas entre « » fueron comprobadas contra el texto de la página.").color(t.text_faint).small());
                    }
                }
            }
        }
        ui.add_space(8.0);
        if ui.small_button("Desactivar asistente").clicked() {
            s.cancel_running();
            s.enabled = false;
        }
    });
    if s.reading.is_some() || s.ocr.is_some() || s.running.is_some() || s.models_job.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restores_language_and_cli_but_never_consent_or_keys() {
        let mut s = AssistantState::fresh();
        s.restore_prefs(&serde_json::json!({ "kind": "codex", "url": "codex", "model": "default", "language": "en", "enabled": true, "allowed_for": "codex", "api_key": "synthetic" }));
        assert_eq!(s.kind, Kind::Codex);
        assert!(!s.spanish);
        assert!(!s.enabled);
        assert!(s.allowed_for.is_none());
        assert!(s.api_key.is_empty());
    }
    #[test]
    fn refuses_external_or_oversized_saved_endpoints() {
        let mut s = AssistantState::fresh();
        s.restore_prefs(&serde_json::json!({ "kind": "openai", "url": "https://example.com/v1", "model": "m".repeat(300) }));
        assert_eq!(s.url, "http://127.0.0.1:11434");
        assert_eq!(s.model.len(), 200);
        s.restore_prefs(&serde_json::json!({ "kind": "codex", "url": "x".repeat(5000) }));
        assert_eq!(s.kind, Kind::Ollama);
    }
}
