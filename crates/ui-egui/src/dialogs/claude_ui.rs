//! The AI assistant tools (Summarize, Ask about this document, Translate), answered by Claude.
//!
//! Requests go to the Anthropic Messages API when the person has an API key (`ANTHROPIC_API_KEY`,
//! or one saved from this dialog to `anthropic-api-key` in the settings folder). Without one they go
//! through Claude Code (`claude -p`), with no tools and no MCP servers, using the Claude plan it is
//! logged in to. The document's text is read on a worker thread and the reply streams into the
//! dialog. Nothing is sent until a button here is pressed. One request at a time; a new document, an edit or closing
//! the dialog stops it, so no answer is shown for text that has changed.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use pdfcraft_engine::{DocId, export::ExportSource};

use crate::{Dialog, PdfCraftApp, widgets};

const MODEL: &str = "claude-opus-5-5";
const KEY_FILE: &str = "anthropic-api-key";
/// Where people create an API key.
const KEYS_URL: &str = "https://console.anthropic.com/settings/keys";
/// About 3.5 MB of text is past the model's context window; say so instead of cutting it short.
const MAX_TEXT_CHARS: usize = 3_000_000;

/// How requests reach Claude.
#[derive(Clone)]
enum Backend {
    /// The Messages API, with the person's API key.
    Api(String),
    /// The `claude` command (Claude Code) and the Claude plan it is logged in to.
    ClaudeCode(std::path::PathBuf),
}

/// An API key wins; otherwise Claude Code, when it is installed.
fn backend() -> Option<Backend> {
    api_key().map(Backend::Api).or_else(|| claude_code().map(Backend::ClaudeCode))
}

/// The `claude` command: on `PATH`, else where its installers put it (an app opened from the Dock
/// or Finder gets a short `PATH`). Looked up once per run of the app.
fn claude_code() -> Option<std::path::PathBuf> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static FOUND: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
        FOUND.get_or_init(|| tool_dirs().into_iter().map(|d| d.join("claude")).find(|p| p.is_file())).clone()
    }
}

/// `PATH`, then the usual install folders for command-line tools.
#[cfg(not(target_arch = "wasm32"))]
fn tool_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.extend([".local/bin", ".claude/local", ".npm-global/bin"].map(|d| home.join(d)));
    }
    dirs
}

const SYSTEM: &str = "You work inside PdfCraft, a PDF editor. The user's message starts with the text of the PDF they \
have open, page by page, inside <document> tags; it was extracted from the PDF, so layout may be lost and tables may read \
oddly. Base everything on that text and say plainly when the document does not contain what is asked. Cite page numbers \
when it helps. Your reply is shown as plain text, so do not use Markdown: no #, ** or tables; use \u{2022} for bullet points \
and blank lines between paragraphs.";

/// Which tool the dialog shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeMode {
    Summary,
    Ask,
    Translate,
}

impl ClaudeMode {
    pub(crate) fn from_command(command: &str) -> Option<Self> {
        match command {
            "ai.summary" => Some(Self::Summary),
            "ai.ask" => Some(Self::Ask),
            "ai.translate" => Some(Self::Translate),
            _ => None,
        }
    }
}

/// What a worker has produced so far, shared with the dialog.
#[derive(Default)]
struct Reply {
    /// The document's text, once read, so later questions don't read it again.
    document: Option<Arc<String>>,
    text: String,
    /// Claude has started answering (reading the document is done).
    writing: bool,
    finished: bool,
    error: Option<String>,
    /// The answer stopped at the length limit.
    truncated: bool,
}

struct Job {
    mode: ClaudeMode,
    reply: Arc<Mutex<Reply>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One question and its answer.
struct Turn {
    question: String,
    answer: String,
}

pub(crate) struct Draft {
    /// The document and edit the results below belong to.
    key: Option<(DocId, u64)>,
    document: Option<Arc<String>>,
    summary: String,
    translation: String,
    turns: Vec<Turn>,
    question: String,
    language: String,
    key_input: String,
    job: Option<Job>,
    error: Option<String>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            key: None,
            document: None,
            summary: String::new(),
            translation: String::new(),
            turns: Vec::new(),
            question: String::new(),
            language: "English".into(),
            key_input: String::new(),
            job: None,
            error: None,
        }
    }
}

impl Draft {
    /// Stop any request and forget the results (the dialog closed, or the document changed).
    pub(crate) fn cancel(&mut self) {
        let (language, key_input) = (std::mem::take(&mut self.language), std::mem::take(&mut self.key_input));
        *self = Self { language, key_input, ..Self::default() };
        if self.language.is_empty() {
            self.language = "English".into();
        }
    }

    fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Copy what the worker has written into the result it belongs to.
    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let Ok(reply) = job.reply.lock() else {
            self.error = Some(tl!("The request stopped unexpectedly.").into());
            self.job = None;
            return;
        };
        if let Some(document) = &reply.document {
            self.document = Some(document.clone());
        }
        let mut text = reply.text.clone();
        if reply.truncated {
            text.push_str("\n\n");
            text.push_str(tl!("[Stopped at the length limit.]"));
        }
        match job.mode {
            ClaudeMode::Summary => self.summary = text,
            ClaudeMode::Translate => self.translation = text,
            ClaudeMode::Ask => {
                if let Some(turn) = self.turns.last_mut() {
                    turn.answer = text;
                }
            }
        }
        if reply.finished {
            self.error = reply.error.clone();
            drop(reply);
            self.job = None;
        }
    }

    fn start(&mut self, mode: ClaudeMode, source: ExportSource, backend: Backend, ctx: &egui::Context, inline: bool) {
        self.error = None;
        let messages = match mode {
            ClaudeMode::Summary => {
                self.summary.clear();
                vec![(
                    "user",
                    "Summarize this document. Start with a short overview paragraph, then list the key points, then any \
                     dates, deadlines, amounts or action items it contains (leave that part out when there are none). Write \
                     in the document's language."
                        .to_string(),
                )]
            }
            ClaudeMode::Translate => {
                self.translation.clear();
                let language = self.language.trim();
                vec![(
                    "user",
                    format!(
                        "Translate the whole document into {language}. Keep its structure: headings, lists and paragraphs, \
                         and a line \"--- Page N ---\" where each page starts. Reply with the translation only."
                    ),
                )]
            }
            ClaudeMode::Ask => {
                let question = std::mem::take(&mut self.question).trim().to_string();
                self.turns.push(Turn { question, answer: String::new() });
                // Every earlier turn again, then the new question: the conversation only grows.
                let mut messages = Vec::new();
                for (i, turn) in self.turns.iter().enumerate() {
                    messages.push(("user", turn.question.clone()));
                    if i + 1 < self.turns.len() {
                        messages.push(("assistant", turn.answer.clone()));
                    }
                }
                messages
            }
        };
        let reply = Arc::new(Mutex::new(Reply::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job { mode, reply: reply.clone(), cancel: cancel.clone() });
        let document = self.document.clone();
        let ctx = ctx.clone();
        let work = move || {
            let result = run(source, document, &messages, &backend, &reply, &cancel, &ctx);
            if let Ok(mut r) = reply.lock() {
                if let Err(e) = result {
                    r.error = Some(e);
                }
                r.finished = true;
            }
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !inline {
            if let Err(error) = std::thread::Builder::new().name("claude".into()).spawn(work) {
                self.job = None;
                self.error = Some(error.to_string());
            }
            return;
        }
        let _ = inline;
        work();
    }
}

/// The page-by-page text of the document, or a reason it can't be sent.
fn document_text(source: ExportSource, cancel: &AtomicBool) -> Result<String, String> {
    let pages = source.pages;
    let mut exporter = pdfcraft_engine::export::Exporter::from_source(source);
    let mut out = String::new();
    for page in 0..pages {
        if cancel.load(Ordering::Relaxed) {
            return Err(String::new());
        }
        let text = exporter.text(page).unwrap_or_default();
        out.push_str(&format!("--- Page {} ---\n{}\n\n", page + 1, text.trim_end()));
    }
    if out.lines().all(|l| l.trim().is_empty() || l.starts_with("--- Page ")) {
        return Err(tl!("This PDF has no text to read. If it is a scan, run Recognize text (OCR) first.").into());
    }
    if out.chars().count() > MAX_TEXT_CHARS {
        return Err(tl!("This document is too long to send in one request. Extract the pages you need first.").into());
    }
    Ok(out)
}

/// The Messages API request: the document goes first and is cached, so follow-up questions about
/// the same document cost less.
fn request_body(document: &str, messages: &[(&str, String)]) -> serde_json::Value {
    use serde_json::json;
    let mut list = Vec::new();
    for (i, (role, text)) in messages.iter().enumerate() {
        let content = if i == 0 {
            json!([
                {"type": "text", "text": format!("<document>\n{document}</document>"), "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": text},
            ])
        } else {
            json!([{"type": "text", "text": text}])
        };
        list.push(json!({"role": role, "content": content}));
    }
    json!({
        "model": MODEL,
        "max_tokens": 64000,
        "stream": true,
        "system": SYSTEM,
        "output_config": {"effort": "medium"},
        // A request a safety check declines is retried on Anthropic's recommended other model.
        "fallbacks": "default",
        "messages": list,
    })
}

/// The single prompt Claude Code is given: the document, any earlier questions and answers, then
/// the request.
fn cli_prompt(document: &str, messages: &[(&str, String)]) -> String {
    let mut prompt = format!("<document>\n{document}</document>\n\n");
    let Some(((_, request), earlier)) = messages.split_last() else { return prompt };
    if !earlier.is_empty() {
        prompt.push_str("Our conversation about this document so far:\n\n");
        for (role, text) in earlier {
            prompt.push_str(if *role == "assistant" { "Your answer: " } else { "My question: " });
            prompt.push_str(text);
            prompt.push_str("\n\n");
        }
        prompt.push_str("My next question: ");
    }
    prompt.push_str(request);
    prompt
}

/// Apply one server-sent event's data to the reply.
fn apply_event(data: &str, reply: &mut Reply) -> Result<(), String> {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { return Ok(()) };
    apply_value(&event, reply)
}

/// Apply one streaming event (Messages API, or Claude Code's `stream_event`) to the reply.
fn apply_value(event: &serde_json::Value, reply: &mut Reply) -> Result<(), String> {
    match event["type"].as_str() {
        Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
            reply.text.push_str(event["delta"]["text"].as_str().unwrap_or_default());
        }
        Some("message_delta") => match event["delta"]["stop_reason"].as_str() {
            Some("max_tokens") => reply.truncated = true,
            Some("refusal") => return Err(tl!("Claude declined this request.").into()),
            _ => {}
        },
        Some("error") => return Err(api_error(event)),
        _ => {}
    }
    Ok(())
}

/// The message of an API error body (`{"type":"error","error":{"type":…,"message":…}}`).
fn api_error(body: &serde_json::Value) -> String {
    let message = body["error"]["message"].as_str().unwrap_or("unknown error");
    match body["error"]["type"].as_str() {
        Some("authentication_error") => format!("{} ({message})", tl!("The API key was not accepted.")),
        Some("rate_limit_error") | Some("overloaded_error") => {
            format!("{} ({message})", tl!("Claude is busy right now. Try again in a minute."))
        }
        _ => message.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(
    source: ExportSource,
    document: Option<Arc<String>>,
    messages: &[(&str, String)],
    backend: &Backend,
    reply: &Mutex<Reply>,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    let document = match document {
        Some(d) => d,
        None => Arc::new(document_text(source, cancel)?),
    };
    if let Ok(mut r) = reply.lock() {
        r.document = Some(document.clone());
        r.writing = true;
    }
    ctx.request_repaint();
    match backend {
        Backend::Api(key) => run_api(&document, messages, key, reply, cancel, ctx),
        Backend::ClaudeCode(claude) => run_claude_code(claude, &document, messages, reply, cancel, ctx),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_api(
    document: &str,
    messages: &[(&str, String)],
    api_key: &str,
    reply: &Mutex<Reply>,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    use std::io::BufRead as _;
    use std::time::Duration;

    let body = request_body(document, messages).to_string();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let response = agent
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2026-10-21")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .header("content-type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("{} ({e})", tl!("Could not reach Claude. Check your internet connection.")))?;
    let status = response.status();
    let mut body = response.into_body();
    if !status.is_success() {
        let text = body.read_to_string().unwrap_or_default();
        return Err(serde_json::from_str::<serde_json::Value>(&text).map_or_else(|_| format!("HTTP {status}"), |v| api_error(&v)));
    }
    let reader = std::io::BufReader::new(body.into_reader());
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(mut r) = reply.lock() else { return Ok(()) };
        apply_event(data.trim_start(), &mut r)?;
        drop(r);
        ctx.request_repaint();
    }
    Ok(())
}

/// Run `claude -p` with no tools, no MCP servers, no slash commands and no saved session, in the
/// temporary folder (so no project's CLAUDE.md applies), and stream its answer.
#[cfg(not(target_arch = "wasm32"))]
fn run_claude_code(
    claude: &std::path::Path,
    document: &str,
    messages: &[(&str, String)],
    reply: &Mutex<Reply>,
    cancel: &AtomicBool,
    ctx: &egui::Context,
) -> Result<(), String> {
    use std::io::{BufRead as _, Read as _, Write as _};
    use std::process::{Command, Stdio};

    // An app opened from the Dock has a short PATH; give Claude Code the usual tool folders.
    let path = std::env::join_paths(claude.parent().map(std::path::Path::to_path_buf).into_iter().chain(tool_dirs())).unwrap_or_default();
    let mut child = Command::new(claude)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--no-session-persistence"])
        .args(["--tools", "", "--strict-mcp-config", "--disable-slash-commands", "--system-prompt", SYSTEM])
        .current_dir(std::env::temp_dir())
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} ({e})", tl!("Could not start Claude Code.")))?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let Some(stdout) = stdout else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(tl!("Could not start Claude Code.").into());
    };
    let prompt = cli_prompt(document, messages);
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    let (outcome, errors) = std::thread::scope(|scope| {
        scope.spawn(|| {
            if let Some(mut stdin) = stdin {
                // Fails only when Claude Code has stopped, which the reading below reports.
                let _ = stdin.write_all(prompt.as_bytes());
            }
        });
        let errors = scope.spawn(|| {
            let mut text = String::new();
            if let Some(stderr) = stderr {
                let _ = stderr.take(16 * 1024).read_to_string(&mut text);
            }
            text
        });
        // Closing the dialog stops Claude Code even while it is still thinking.
        scope.spawn(|| {
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        let outcome = (|| -> Result<Option<String>, String> {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                match value["type"].as_str() {
                    Some("stream_event") => {
                        let Ok(mut r) = reply.lock() else { return Ok(None) };
                        apply_value(&value["event"], &mut r)?;
                        drop(r);
                        ctx.request_repaint();
                    }
                    Some("result") if value["is_error"].as_bool() == Some(true) => {
                        return Ok(Some(value["result"].as_str().unwrap_or("Claude Code reported an error.").to_string()));
                    }
                    _ => {}
                }
            }
            Ok(None)
        })();
        finished.store(true, Ordering::Relaxed);
        if let Ok(mut child) = child.lock() {
            if outcome.is_err() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        (outcome, errors.join().unwrap_or_default())
    });
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let status = child.into_inner().ok().and_then(|mut c| c.try_wait().ok().flatten());
    if let Some(error) = outcome? {
        return Err(claude_code_error(&error));
    }
    let empty = reply.lock().map_or(true, |r| r.text.is_empty());
    if empty && status.is_some_and(|s| !s.success()) {
        let detail = errors.trim();
        return Err(claude_code_error(if detail.is_empty() { "Claude Code stopped without an answer." } else { detail }));
    }
    Ok(())
}

/// Claude Code's message, with what to do when it isn't logged in.
fn claude_code_error(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("log in") || lower.contains("login") || lower.contains("logged in") {
        format!("{} ({message})", tl!("Claude Code is not logged in. Open Terminal, run claude and log in, then try again."))
    } else {
        message.to_string()
    }
}

#[cfg(target_arch = "wasm32")]
fn run(
    _: ExportSource,
    _: Option<Arc<String>>,
    _: &[(&str, String)],
    _: &Backend,
    _: &Mutex<Reply>,
    _: &AtomicBool,
    _: &egui::Context,
) -> Result<(), String> {
    Err(tl!("Claude is available in the desktop app only.").into())
}

fn key_path() -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::portable::data_dir().map(|d| d.join(KEY_FILE)).or_else(|| eframe::storage_dir("PdfCraft").map(|d| d.join(KEY_FILE)))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// `ANTHROPIC_API_KEY`, else the key saved from this dialog.
fn api_key() -> Option<String> {
    let from_env = std::env::var("ANTHROPIC_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    from_env.or_else(|| {
        let saved = std::fs::read_to_string(key_path()?).ok()?;
        Some(saved.trim().to_string()).filter(|k| !k.is_empty())
    })
}

/// Save the key readable only by this user.
fn save_key(key: &str) -> Result<(), String> {
    let path = key_path().ok_or_else(|| tl!("There is no settings folder to save the key in.").to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut file, key.trim().as_bytes()).map_err(|e| e.to_string())
}

fn forget_key() {
    if let Some(path) = key_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Text that can be selected and copied, in a scrolling box.
fn output(ui: &mut egui::Ui, id: &str, text: &str) {
    egui::ScrollArea::vertical().id_salt(id).max_height(380.0).stick_to_bottom(true).show(ui, |ui| {
        ui.add(egui::Label::new(text).selectable(true).wrap());
    });
}

fn copy_button(ui: &mut egui::Ui, text: &str) {
    if !text.is_empty() && widgets::pill_button(ui, tl!("Copy"), false).clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

fn key_setup(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.label(tl!("Claude needs Claude Code or an Anthropic API key. To use your Claude plan, install Claude Code, run claude in Terminal once to log in, then restart PdfCraft. Or paste an API key from the Anthropic Console here (usage is billed to that account)."));
    ui.hyperlink_to(tl!("Get an API key"), KEYS_URL);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(widgets::line(&mut draft.key_input).password(true).hint_text("sk-ant-…").desired_width(320.0));
        let ready = !draft.key_input.trim().is_empty();
        if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Save key"), true)).inner.clicked() {
            match save_key(&draft.key_input) {
                Ok(()) => draft.key_input.clear(),
                Err(e) => draft.error = Some(e),
            }
        }
    });
}

/// Draw the dialog. Returns `true` when it should close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, mode: ClaudeMode, next: &mut Option<Dialog>) -> bool {
    ui.heading(tl!("Claude"));
    ui.horizontal_wrapped(|ui| {
        for (m, label) in
            [(ClaudeMode::Summary, tl!("Summarize")), (ClaudeMode::Ask, tl!("Ask about this document")), (ClaudeMode::Translate, tl!("Translate"))]
        {
            if widgets::mode_tab(ui, label, mode == m).clicked() {
                *next = Some(Dialog::Claude(m));
            }
        }
    });
    ui.separator();
    let Some((_, id)) = app.active_ids() else { return true };
    let Some(doc) = app.session.get(id) else { return true };
    let draft = &mut app.claude_draft;
    if let Err(error) = doc.check_copying() {
        draft.cancel();
        ui.label(error.to_string());
        return false;
    }
    let key = (id, doc.edit_generation());
    if draft.key != Some(key) {
        draft.cancel();
        draft.key = Some(key);
    }
    draft.poll();
    let Some(backend) = backend() else {
        key_setup(ui, draft);
        if let Some(error) = &draft.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        return false;
    };
    let mut go = false;
    match mode {
        ClaudeMode::Summary => {
            if draft.summary.is_empty() && !draft.running() {
                ui.label(tl!("Claude reads the document and writes a summary of it."));
            } else {
                output(ui, "claude-summary", &draft.summary);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = if draft.summary.is_empty() { tl!("Summarize this document") } else { tl!("Summarize again") };
                go = ui.add_enabled_ui(!draft.running(), |ui| widgets::pill_button(ui, label, true)).inner.clicked();
                copy_button(ui, &draft.summary);
            });
        }
        ClaudeMode::Translate => {
            ui.horizontal(|ui| {
                ui.label(tl!("Into"));
                ui.add(widgets::line(&mut draft.language).desired_width(180.0));
                let ready = !draft.running() && !draft.language.trim().is_empty();
                go = ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Translate"), true)).inner.clicked();
                copy_button(ui, &draft.translation);
            });
            if !draft.translation.is_empty() || draft.running() {
                ui.add_space(6.0);
                output(ui, "claude-translation", &draft.translation);
            }
        }
        ClaudeMode::Ask => {
            if draft.turns.is_empty() {
                ui.label(tl!("Ask anything about this document. Follow-up questions keep the conversation."));
            } else {
                egui::ScrollArea::vertical().id_salt("claude-ask").max_height(380.0).stick_to_bottom(true).show(ui, |ui| {
                    for turn in &draft.turns {
                        ui.strong(&turn.question);
                        ui.add(egui::Label::new(&turn.answer).selectable(true).wrap());
                        ui.add_space(10.0);
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let field = ui.add(widgets::line(&mut draft.question).hint_text(tl!("Your question")).desired_width(360.0));
                let ready = !draft.running() && !draft.question.trim().is_empty();
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Ask"), true)).inner.clicked();
                go = ready && (entered || clicked);
                if !draft.turns.is_empty() && !draft.running() && widgets::pill_button(ui, tl!("New conversation"), false).clicked() {
                    draft.turns.clear();
                }
            });
        }
    }
    if let Some(job) = &draft.job
        && job.mode == mode
    {
        let writing = job.reply.lock().is_ok_and(|r| r.writing);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if writing { tl!("Claude is writing…") } else { tl!("Reading the document…") });
        });
    }
    if let Some(error) = &draft.error
        && !error.is_empty()
    {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if go {
        draft.start(mode, doc.export_source(), backend.clone(), ui.ctx(), app.run_inline);
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        match backend {
            Backend::Api(_) => {
                ui.small(crate::i18n::fmt(tl!("Sends the document's text to Anthropic ({model}) using your API key."), &[("model", MODEL)]));
            }
            Backend::ClaudeCode(_) => {
                ui.small(tl!("Sends the document's text to Claude through Claude Code. It counts toward your Claude plan's usage limits."));
            }
        }
        if matches!(backend, Backend::Api(_)) && std::env::var_os("ANTHROPIC_API_KEY").is_none() && ui.small_button(tl!("Remove saved key")).clicked()
        {
            forget_key();
            app.claude_draft.cancel();
        }
    });
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_events_build_the_answer() {
        let mut reply = Reply::default();
        for data in [
            r#"{"type":"message_start","message":{"id":"msg_1"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":" world"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#,
            "not json",
        ] {
            apply_event(data, &mut reply).unwrap();
        }
        assert_eq!(reply.text, "Hello world");
        assert!(reply.truncated);
        let error = apply_event(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &mut reply);
        assert!(error.unwrap_err().contains("Overloaded"));
        assert!(apply_event(r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#, &mut reply).is_err());
    }

    #[test]
    fn the_document_leads_the_first_message_and_is_cached() {
        let body = request_body("--- Page 1 ---\nHi\n", &[("user", "Q1".into()), ("assistant", "A1".into()), ("user", "Q2".into())]);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["fallbacks"], "default");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages[0]["content"][0]["text"].as_str().unwrap().starts_with("<document>"));
        assert_eq!(messages[0]["content"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(messages[0]["content"][1]["text"], "Q1");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["content"][0]["text"], "Q2");
    }

    #[test]
    fn claude_code_gets_the_document_then_the_conversation() {
        let first = cli_prompt("--- Page 1 ---\nHi\n", &[("user", "Summarize.".into())]);
        assert_eq!(first, "<document>\n--- Page 1 ---\nHi\n</document>\n\nSummarize.");
        let later = cli_prompt("D", &[("user", "Q1".into()), ("assistant", "A1".into()), ("user", "Q2".into())]);
        assert!(later.ends_with("My question: Q1\n\nYour answer: A1\n\nMy next question: Q2"), "{later}");
        assert!(claude_code_error("Not logged in · Please run /login").contains("not logged in"));
        let mut reply = Reply::default();
        let line: serde_json::Value = serde_json::from_str(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}}"#,
        )
        .unwrap();
        apply_value(&line["event"], &mut reply).unwrap();
        assert_eq!(reply.text, "Hi");
    }

    #[test]
    fn ai_commands_open_their_tab() {
        assert_eq!(ClaudeMode::from_command("ai.summary"), Some(ClaudeMode::Summary));
        assert_eq!(ClaudeMode::from_command("ai.ask"), Some(ClaudeMode::Ask));
        assert_eq!(ClaudeMode::from_command("ai.translate"), Some(ClaudeMode::Translate));
        assert_eq!(ClaudeMode::from_command("file.open"), None);
    }
}
