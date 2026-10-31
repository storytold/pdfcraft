//! Ask Agent: the person selects text, pages or a region and asks their own agent (over MCP or the
//! control channel) to work on the PDF. `hub.rs` and `ui.rs` are shared with the other apps; this
//! file is PdfCraft's part: the target as JSON, the agent's edits as one undo step, its closing note.
pub mod hub;
pub mod ui;

pub use hub::{AgentHub, HubError, Request, Snapshot, Status, Turn};
pub use ui::Action;

use crate::PdfCraftApp;
use egui::Rect;
use serde_json::json;

/// PDF-specific prompts. Choosing one only fills the editable instruction.
const QUICK_ACTIONS: &[ui::QuickAction] = &[
    ui::QuickAction {
        label: "Summarize",
        group: "",
        instruction: "Summarize the selected PDF content concisely, preserving the key facts and figures.",
    },
    ui::QuickAction {
        label: "Explain selection",
        group: "",
        instruction: "Explain the selected PDF content in plain language. Identify uncertainties without inventing details.",
    },
    ui::QuickAction {
        label: "Highlight key points",
        group: "",
        instruction: "Highlight the key results in the selected PDF content and add a short summary note.",
    },
    ui::QuickAction {
        label: "Add review note",
        group: "",
        instruction: "Review the selected PDF content and add a concise comment with issues or questions to check.",
    },
    ui::QuickAction {
        label: "Prepare fields",
        group: "Forms",
        instruction: "Inspect the selected PDF content for form fields and propose the field names and types. Ask me to confirm before creating or changing fields.",
    },
    ui::QuickAction {
        label: "Mark sensitive text",
        group: "Redaction",
        instruction: "Identify potentially sensitive text in the selected PDF content and mark it for my review. Do not apply redactions or permanently remove content.",
    },
];

/// Application state for Ask Agent.
#[derive(Default)]
pub struct AskAgent {
    pub hub: AgentHub,
    pub(crate) open: Option<Open>,
    pub(crate) draft: String,
    pub(crate) picked: Option<String>,
}

pub(crate) struct Open {
    pub(crate) id: u64,
    pub(crate) anchor: Rect,
    pub(crate) turns: Vec<Turn>,
    pub(crate) doc_id: pdfcraft_engine::DocId,
    pub(crate) initial_generation: u64,
    pub(crate) edits_count: usize,
    asked: String,
    target: serde_json::Value,
    commented: bool,
}

impl AskAgent {
    /// Create Ask Agent state attached to the given hub.
    pub fn new(hub: AgentHub) -> Self {
        Self { hub, open: None, draft: String::new(), picked: None }
    }

    /// True when the Ask Agent bubble is currently displayed.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The name an edit for `request` is authored with: "Agent (<name>)".
    pub fn author(&self, request: u64) -> Option<String> {
        self.hub.snapshot(request).and_then(|s| s.agent).map(|name| format!("Agent ({name})"))
    }

    /// Record an edit made on behalf of request_id.
    pub fn record_edit(&mut self, request_id: u64) {
        self.record_edits(request_id, 1);
    }

    /// Record `count` edits made on behalf of request_id: one per Undo step they created, so a
    /// batch of steps is counted step by step rather than as one tool call.
    pub fn record_edits(&mut self, request_id: u64, count: usize) {
        if let Some(open) = &mut self.open
            && open.id == request_id
        {
            open.edits_count += count;
        }
    }

    /// The document the open bubble for `request_id` works on.
    pub fn open_doc(&self, request_id: u64) -> Option<pdfcraft_engine::DocId> {
        self.open.as_ref().filter(|o| o.id == request_id).map(|o| o.doc_id)
    }

    /// Inspect current Ask Agent state for ui.inspect.
    pub fn inspect(&self) -> serde_json::Value {
        json!({
            "bubbleOpen": self.open.is_some(),
            "agents": self.hub.agents(),
            "currentId": self.open.as_ref().map(|o| o.id),
            "edits": self.open.as_ref().map_or(0, |o| o.edits_count),
        })
    }
}

/// Show the Ask Agent chip or bubble and process resulting actions.
pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if app.modal_open() || app.palette_open {
        return;
    }
    let Some(open) = &app.ask_agent.open else {
        let anchor = app.active.and_then(|i| app.views.get(i)).and_then(|v| {
            let meaningful = v.selected_text().is_some() || !v.selected.is_empty() || v.forms.focus.is_some();
            meaningful.then(|| v.page_screen_rect(v.current)).flatten()
        });
        if let Some(anchor) = anchor
            && !app.ask_agent.hub.agents().is_empty()
            && ui::chip(ctx, anchor)
        {
            open_at(app, anchor);
        }
        return;
    };
    let (anchor, snapshot) = (open.anchor, app.ask_agent.hub.snapshot(open.id));
    if let Some(s) = snapshot.as_ref().filter(|s| s.status == Status::Answered) {
        note_once(app, s);
    }
    let agents = app.ask_agent.hub.agents();
    let (history, instruction) = app.ask_agent.open.as_ref().map(|open| (open.turns.as_slice(), open.asked.as_str())).unwrap_or((&[], ""));
    let view = ui::View { snapshot: snapshot.as_ref(), agents: &agents, actions: QUICK_ACTIONS, history, instruction };
    if let Some(action) = ui::bubble(ctx, anchor, &view, &mut app.ask_agent.draft, &mut app.ask_agent.picked) {
        handle_action(app, action);
    }
}

fn open_at(app: &mut PdfCraftApp, anchor: Rect) {
    let Some((_, doc_id)) = app.active_ids() else { return };
    let initial_generation = app.session.get(doc_id).map(|d| d.edit_generation()).unwrap_or(0);
    let target = serde_json::Value::Null;
    app.ask_agent.open =
        Some(Open { id: 0, anchor, turns: vec![], doc_id, initial_generation, edits_count: 0, asked: String::new(), target, commented: false });
}

/// The agent's answer, once, as a note on the page it worked on, authored like its edits.
fn note_once(app: &mut PdfCraftApp, s: &Snapshot) {
    let Some(open) = app.ask_agent.open.as_mut().filter(|o| !o.commented) else { return };
    open.commented = true;
    let (Some(answer), Some(author)) = (s.answer.clone(), app.ask_agent.author(s.id)) else { return };
    let Some(open) = &app.ask_agent.open else { return };
    let page = open.target["pages"][0].as_u64().unwrap_or(1);
    let at = open.target["rects"][0]["rect"].as_array().map(|r| [r[2].clone(), r[1].clone()]).map_or(json!([36, 36]), |p| json!(p));
    let contents = format!("Instruction: {}\n\nAgent response: {}", open.asked, answer);
    let args = json!({"doc": open.doc_id.0, "page": page, "type": "note", "at": at, "contents": contents, "author": author});
    if app.run_tool("comment_add", &args, None).is_ok() {
        app.ask_agent.record_edit(s.id);
    }
}

fn handle_action(app: &mut PdfCraftApp, act: Action) {
    match act {
        Action::Send(text) => {
            let Some((sel, doc, _, _)) = capture_target(app) else { return };
            let agent = app.ask_agent.picked.clone();
            let hub = app.ask_agent.hub.clone();
            let Some(open) = &mut app.ask_agent.open else { return };
            // After an answer, this is a follow-up: the thread so far goes with it.
            if let Some(answer) = hub.snapshot(open.id).and_then(|s| s.answer) {
                open.turns.push(Turn { instruction: std::mem::take(&mut open.asked), answer });
                hub.close(open.id);
            }
            if open.target.is_null() {
                open.target = sel.clone();
            }
            open.asked = text.clone();
            open.commented = false;
            open.id = hub.ask(Request { id: 0, instruction: text, history: open.turns.clone(), selection: sel, document: doc, agent });
        }
        Action::Keep | Action::Close => {
            if let Some(open) = app.ask_agent.open.take() {
                if open.edits_count > 1 {
                    let _ = app.session.group_undo(open.doc_id, open.edits_count, pdfcraft_engine::Label::of("Ask Agent"));
                }
                app.ask_agent.hub.close(open.id);
            }
        }
        Action::Undo => {
            let Some(open) = app.ask_agent.open.take() else { return };
            app.ask_agent.hub.close(open.id);
            let current = app.session.get(open.doc_id).map(|d| d.edit_generation()).unwrap_or(0);
            if current != open.initial_generation + open.edits_count as u64 {
                app.notify_error("The document changed since the agent's edits; use Undo in the Edit menu");
                return;
            }
            for _ in 0..open.edits_count {
                let _ = app.session.undo(open.doc_id);
            }
            if let Some(doc) = app.session.get(open.doc_id)
                && let Some(view) = app.views.iter_mut().find(|v| v.id == open.doc_id)
            {
                view.document_changed(&doc.info);
            }
        }
    }
}

fn capture_target(app: &PdfCraftApp) -> Option<(serde_json::Value, serde_json::Value, pdfcraft_engine::DocId, u64)> {
    let v = app.active.and_then(|i| app.views.get(i))?;
    let d = app.session.get(v.id)?;
    let pages: Vec<usize> = if let Some((p, _)) = v.selection_quads(&d.info) {
        vec![p + 1]
    } else if !v.selected.is_empty() {
        v.selected.iter().map(|p| p + 1).collect()
    } else {
        vec![v.current + 1]
    };
    let rects = v
        .selection_quads(&d.info)
        .map(|(p, qs)| {
            qs.into_iter()
                .map(|q| {
                    let (min_x, max_x) =
                        [q[0], q[2], q[4], q[6]].into_iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), v| (mn.min(v), mx.max(v)));
                    let (min_y, max_y) =
                        [q[1], q[3], q[5], q[7]].into_iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), v| (mn.min(v), mx.max(v)));
                    json!({ "page": p + 1, "rect": [min_x, min_y, max_x, max_y] })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let text = v.selected_text().unwrap_or_default();
    let selection = json!({ "doc": d.id.0, "pages": pages, "rects": rects, "text": text });
    let document =
        json!({ "id": format!("pdf:{}", d.id.0), "title": d.name, "saved": d.path.is_some(), "revision": d.edit_generation().to_string() });
    Some((selection, document, d.id, d.edit_generation()))
}

impl PdfCraftApp {
    pub(crate) fn ask_agent_show(&mut self, ctx: &egui::Context) {
        show(self, ctx);
    }

    pub(crate) fn ask_agent_input(&mut self, ctx: &egui::Context) {
        if self.ask_agent.is_open() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            handle_action(self, Action::Close);
        }
    }

    pub(crate) fn ask_agent_inspect(&self) -> serde_json::Value {
        self.ask_agent.inspect()
    }

    /// `ui.askAgent {instruction?, close?}`: open the bubble at the current page, and send right
    /// away when an instruction is given. Keep and Undo are the person's, in the window only.
    pub(crate) fn ask_agent_command(&mut self, p: &serde_json::Value) -> Result<serde_json::Value, String> {
        if ["accept", "discard", "keep", "undo"].iter().any(|k| p.get(*k).is_some()) {
            return Err("HUMAN_REVIEW_REQUIRED: Keep and Undo are the person's, in the window".into());
        }
        if p.get("close").and_then(serde_json::Value::as_bool) == Some(true) {
            handle_action(self, Action::Close);
            return Ok(json!({ "open": false }));
        }
        if self.ask_agent.open.is_none() {
            let anchor = self.active.and_then(|i| self.views.get(i)).and_then(|v| v.page_screen_rect(v.current));
            open_at(self, anchor.unwrap_or_else(|| Rect::from_min_size(egui::pos2(96.0, 160.0), egui::vec2(1.0, 16.0))));
        }
        if self.ask_agent.open.is_none() {
            return Err("STALE_TARGET: no document open".into());
        }
        if let Some(text) = p.get("instruction").and_then(serde_json::Value::as_str) {
            handle_action(self, Action::Send(text.to_string()));
        }
        Ok(json!({ "open": true, "id": self.ask_agent.open.as_ref().map(|o| o.id) }))
    }
}
