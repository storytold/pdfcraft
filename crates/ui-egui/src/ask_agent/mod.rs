//! Ask Agent: the person selects text, pages or a region and asks their own agent (over MCP or the
//! control channel) to work on the PDF. `hub.rs` and `ui.rs` are shared with the other apps; this
//! file is PdfCraft's part: the target as JSON, the agent's edits as one undo step, its closing note.
pub mod hub;
pub(crate) mod target;
#[cfg(test)]
mod tests;
mod transactions;
pub mod ui;

pub use hub::{AgentHub, HubError, Request, Snapshot, Status, Target, Turn};
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
    pub(crate) expected_generation: u64,
    pub(crate) initial_depth: usize,
    pub(crate) undo_safe: bool,
    pub(crate) last_agent: Option<String>,
    pub(crate) scope_valid: bool,
    pub(crate) edits_count: usize,
    asked: String,
    pub(crate) target: serde_json::Value,
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
        let open = self.open.as_ref().filter(|open| open.id == request);
        let interrupted = open.filter(|open| !open.scope_valid).and_then(|open| open.last_agent.clone());
        interrupted
            .or_else(|| self.hub.snapshot(request).and_then(|snapshot| snapshot.agent))
            .or_else(|| open.and_then(|open| open.last_agent.clone()))
            .map(|name| format!("Agent ({name})"))
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
            open.edits_count = open.edits_count.saturating_add(count);
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
    for doc in app.session.docs() {
        if let Some(current) = target::for_doc(&app.session, doc.id) {
            app.ask_agent.hub.invalidate(&current);
        }
    }
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
    if let Some(s) = snapshot.as_ref().filter(|s| matches!(s.status, Status::Answered | Status::Failed)) {
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
    app.ask_agent.open = Some(Open {
        id: 0,
        anchor,
        turns: vec![],
        doc_id,
        expected_generation: initial_generation,
        initial_depth: app.session.get(doc_id).map_or(0, |d| d.undo_depth()),
        undo_safe: true,
        last_agent: None,
        scope_valid: true,
        edits_count: 0,
        asked: String::new(),
        target,
        commented: false,
    });
}

/// The agent's answer, once, as a note on the page it worked on, authored like its edits.
fn note_once(app: &mut PdfCraftApp, s: &Snapshot) {
    let Some(open) = app.ask_agent.open.as_ref().filter(|o| !o.commented && o.id == s.id) else { return };
    if !transactions::undo_matches(&app.session, open) {
        return;
    }
    let answer = s.review_text();
    if answer.is_empty() {
        return;
    }
    let Some(author) = app.ask_agent.author(s.id) else { return };
    let page = open.target["pages"][0].as_u64().unwrap_or(1);
    let at = open.target["rects"][0]["rect"]
        .as_array()
        .and_then(|r| {
            let doc = app.session.get(open.doc_id)?;
            let info = doc.info.pages.get(usize::try_from(page).ok()?.checked_sub(1)?)?;
            Some(info.user_to_view(r.get(2)?.as_f64()? as f32, r.get(1)?.as_f64()? as f32))
        })
        .unwrap_or([36.0, 36.0]);
    let doc_id = open.doc_id;
    let before = app.session.get(doc_id).map(|d| (d.undo_depth(), d.edit_generation()));
    let contents = format!("Instruction: {}\n\nAgent response: {}", open.asked, answer);
    let args = json!({"doc":doc_id.0,"page":page,"type":"note","at":at,"contents":contents,"author":author});
    if app.run_tool("comment_add", &args, None).is_ok() {
        let after = app.session.get(doc_id);
        if let Some(open) = app.ask_agent.open.as_mut() {
            open.commented = true;
            // A note is one edit even when the engine evicts an older history entry.
            // Do not advance a supposedly safe checkpoint if that step cannot be counted.
            if let (Some((depth, generation)), Some(after)) = (before, after) {
                let created = after.undo_depth().saturating_sub(depth);
                open.edits_count = open.edits_count.saturating_add(created);
                if created != 1 || generation.checked_add(1) != Some(after.edit_generation()) {
                    open.undo_safe = false;
                }
                open.expected_generation = after.edit_generation();
            } else {
                open.undo_safe = false;
            }
        }
    }
}

fn send(app: &mut PdfCraftApp, text: String) -> Result<(), String> {
    let (captured, document, doc_id, generation) = capture_target(app).ok_or("STALE_TARGET: no document open")?;
    let open = app.ask_agent.open.as_ref().ok_or("STALE_TARGET: no open Ask Agent request")?;
    if doc_id != open.doc_id {
        return Err("STALE_TARGET: switch back to the document where Ask Agent was opened before sending".into());
    }
    let hub = app.ask_agent.hub.clone();
    let previous = hub.snapshot(open.id);
    if previous.as_ref().is_some_and(|s| matches!(s.status, Status::Queued | Status::Working { .. })) {
        return Err("HELD: the current request is still waiting or working".into());
    }
    if open.id != 0 && !open.scope_valid {
        return Err("STALE_TARGET: the edited selection cannot be followed; keep or close this request and select it again".into());
    }
    if open.id != 0 && !transactions::undo_matches(&app.session, open) {
        return Err("STALE_TARGET: the document changed outside this request; keep or close it before asking again".into());
    }
    let mut history = open.turns.clone();
    if let Some(previous) = previous.as_ref().filter(|s| matches!(s.status, Status::Answered | Status::Failed)) {
        history.push(Turn { instruction: open.asked.clone(), answer: previous.review_text() });
    }
    let selection = if open.target.is_null() { captured } else { open.target.clone() };
    let id = hub
        .ask(Request {
            id: 0,
            instruction: text.clone(),
            history: history.clone(),
            selection: selection.clone(),
            document,
            agent: app.ask_agent.picked.clone(),
            target: Target { document: format!("pdf:{}", doc_id.0), revision: generation },
        })
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
    let depth = app.session.get(doc_id).map_or(0, |d| d.undo_depth());
    if let Some(open) = app.ask_agent.open.as_mut() {
        if open.id == 0 {
            open.initial_depth = depth;
            open.expected_generation = generation;
            open.undo_safe = true;
        } else {
            hub.close(open.id);
        }
        open.id = id;
        open.target = selection;
        open.turns = history;
        open.asked = text;
        open.commented = false;
    }
    Ok(())
}

fn handle_action(app: &mut PdfCraftApp, act: Action) {
    match act {
        Action::Send(text) => {
            if let Err(error) = send(app, text.clone()) {
                app.ask_agent.draft = text;
                app.notify_error(&error);
            }
        }
        Action::Keep | Action::Close => {
            if let Some(open) = app.ask_agent.open.take() {
                if open.edits_count > 1 && transactions::undo_matches(&app.session, &open) {
                    let _ = app.session.group_undo(open.doc_id, open.edits_count, pdfcraft_engine::Label::of("Ask Agent"));
                }
                app.ask_agent.hub.close(open.id);
            }
        }
        Action::Undo => {
            let Some(open) = app.ask_agent.open.take() else { return };
            app.ask_agent.hub.close(open.id);
            if !transactions::undo_matches(&app.session, &open) {
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
    let quads =
        v.selection_quads(&d.info).map(|(p, qs)| qs.into_iter().map(|q| json!({"page":p+1,"quad":q})).collect::<Vec<_>>()).unwrap_or_default();
    let selection = json!({ "doc": d.id.0, "pages": pages, "rects": rects, "quads":quads,"text": text });
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
            if self.ask_agent.open.as_ref().is_some_and(|open| open.id != 0) {
                return Err("HUMAN_REVIEW_REQUIRED: closing this review keeps its edits; use Keep, Undo or Close in the window yourself".into());
            }
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
            send(self, text.to_string())?;
        }
        Ok(json!({ "open": true, "id": self.ask_agent.open.as_ref().map(|o| o.id) }))
    }
}
