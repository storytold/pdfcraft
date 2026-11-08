//! Collaborative session wiring for the desktop app (feature `collab`, off by
//! default). The socket lives in `pdfcraft-collab-session`; this module starts
//! it from the environment, applies accepted operations to the open document,
//! turns edits into operations, and draws the participants window.
//!
//! Nothing here runs for a normal single-user session: without
//! `PDFCRAFT_COLLAB_URL`, `PDFCRAFT_COLLAB_TOKEN` and `PDFCRAFT_COLLAB_DOC`
//! the module does nothing.

use crate::PdfCraftApp;
use pdfcraft_collab_client_pdf::{apply, inverse_for, ops_for_edit};
use pdfcraft_collab_proto::{ActorId, ClientInfo, DocKey, ErrorCode, Op, OpId, PageRef, Presence, Role, Viewport};
use pdfcraft_collab_session::{Config as WireConfig, Event, Handle, spawn};
use pdfcraft_engine::{DocId, Edit};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub(crate) struct CollabUi {
    handle: Handle,
    doc: DocId,
    doc_key: DocKey,
    actor: Option<ActorId>,
    /// Actor + a per-launch nonce: operation ids and comment names must not
    /// repeat when the app restarts inside a living room.
    client_id: String,
    role: Role,
    connected: bool,
    version: u64,
    epoch: u64,
    participants: BTreeMap<String, Presence>,
    color: String,
    status: String,
    error: Option<String>,
    history: Vec<HistoryGroup>,
    redo: Vec<HistoryGroup>,
    counter: u64,
    last_presence: Instant,
    committed: Option<(String, u64)>,
}

struct HistoryGroup {
    label: String,
    originals: Vec<Op>,
    inverses: Vec<Option<Op>>,
}

impl CollabUi {
    fn start(app: &mut PdfCraftApp) {
        let Some((_, doc)) = app.active_ids() else { return };
        let Ok(url) = std::env::var("PDFCRAFT_COLLAB_URL") else { return };
        let Ok(token) = std::env::var("PDFCRAFT_COLLAB_TOKEN") else { return };
        let Ok(key) = std::env::var("PDFCRAFT_COLLAB_DOC") else { return };
        let Ok(doc_key) = DocKey::new(key) else {
            app.notify_error("The collaboration document id is not valid");
            return;
        };
        let name = std::env::var("PDFCRAFT_COLLAB_NAME").unwrap_or_else(|_| "PdfCraft".into());
        let nonce =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| format!("{:x}", d.as_nanos())).unwrap_or_else(|_| "0".into());
        let info = ClientInfo { name, version: env!("CARGO_PKG_VERSION").into() };
        match spawn(WireConfig::new(url, token, doc_key.clone(), info)) {
            Ok(handle) => {
                app.collab = Some(Self {
                    handle,
                    doc,
                    doc_key,
                    actor: None,
                    client_id: format!("client-{nonce}"),
                    role: Role::Viewer,
                    connected: false,
                    version: 0,
                    epoch: 0,
                    participants: BTreeMap::new(),
                    color: "#3366cc".into(),
                    status: "Connecting…".into(),
                    error: None,
                    history: Vec::new(),
                    redo: Vec::new(),
                    counter: 0,
                    last_presence: Instant::now(),
                    committed: None,
                });
            }
            Err(e) => app.notify_error(format!("The collaboration session did not start: {e}")),
        }
    }

    fn next_op_id(&mut self) -> Option<OpId> {
        self.counter = self.counter.saturating_add(1);
        OpId::new(format!("{}-{}", self.client_id, self.counter)).ok()
    }
}

/// Start the session when the environment asks for one, drain its events and
/// send our presence. Called once per frame from `logic`.
impl PdfCraftApp {
    pub(crate) fn poll_collab(&mut self, ctx: &egui::Context) {
        if self.collab.is_none() && !self.collab_off {
            CollabUi::start(self);
        }
        let mut events = Vec::new();
        if let Some(collab) = self.collab.as_mut() {
            while let Some(event) = collab.handle.try_event() {
                events.push(event);
            }
        }
        for event in events {
            self.collab_event(event, ctx);
        }
        self.collab_presence(ctx);
    }

    fn collab_event(&mut self, event: Event, ctx: &egui::Context) {
        let Some((index, doc)) = self.active_ids() else { return };
        let Some(collab) = self.collab.as_mut() else { return };
        if collab.doc != doc {
            return;
        }
        match event {
            Event::Joined { welcome, .. } => {
                collab.actor = Some(welcome.actor_id.clone());
                collab.role = welcome.role;
                // The actor names the person; the nonce keeps this launch's ids unique.
                let nonce = collab.client_id.rsplit_once('-').map_or("0", |(_, nonce)| nonce);
                collab.client_id = format!("{}-{nonce}", welcome.actor_id.as_str());
                collab.connected = true;
                collab.version = welcome.version;
                collab.epoch = welcome.epoch;
                collab.color = color_of(welcome.actor_id.as_str());
                collab.status = format!("Connected as {} · {}", welcome.actor_id.as_str(), role_name(welcome.role));
                collab.error = None;
            }
            Event::Applied { applied, .. } => {
                collab.version = applied.version;
                collab.epoch = applied.epoch;
                match apply(&mut self.session, doc, &applied.op) {
                    Ok(_) => {
                        if let Some(document) = self.session.get(doc)
                            && let Some(view) = self.views.get_mut(index)
                        {
                            view.document_changed(&document.info);
                        }
                    }
                    Err(e) => collab.error = Some(format!("A shared change could not be applied: {e}")),
                }
                ctx.request_repaint();
            }
            Event::Rejected(rejected) => {
                for group in collab.history.iter_mut().chain(collab.redo.iter_mut()) {
                    group.originals.retain(|op| op.op_id != rejected.op_id);
                }
                collab.history.retain(|group| !group.originals.is_empty());
                collab.redo.retain(|group| !group.originals.is_empty());
                if rejected.code != ErrorCode::Lease {
                    collab.error = Some(format!("{}: {}", code_name(rejected.code), rejected.message));
                }
            }
            Event::Snapshot { version, epoch, base, tail } => {
                collab.version = version;
                collab.epoch = epoch;
                collab.history.clear();
                collab.redo.clear();
                if base.is_some() {
                    collab.error = Some("The server compacted the journal; reopen the document to catch up.".into());
                } else {
                    for op in tail {
                        if let Err(e) = apply(&mut self.session, doc, &op) {
                            collab.error = Some(format!("A snapshot operation could not be applied: {e}"));
                            break;
                        }
                    }
                    if let Some(document) = self.session.get(doc)
                        && let Some(view) = self.views.get_mut(index)
                    {
                        view.document_changed(&document.info);
                    }
                }
            }
            Event::Presence(presence) => {
                if collab.actor.as_ref() != Some(&presence.actor_id) {
                    collab.participants.insert(presence.actor_id.as_str().to_owned(), presence);
                }
            }
            Event::Committed(result) => {
                collab.committed = Some((result.sha256.clone(), result.bytes));
                collab.status = format!("Committed version {version}", version = result.version);
            }
            Event::Disconnected => collab.status = "Reconnecting…".into(),
            Event::Gap | Event::Retry { .. } => collab.status = "Catching up…".into(),
            Event::Error(error) => {
                collab.error = Some(format!("{}: {}", code_name(error.code), error.message));
                if error.code == ErrorCode::Auth {
                    collab.connected = false;
                }
            }
            Event::Notice(text) => collab.error = Some(text),
        }
    }

    fn collab_presence(&mut self, ctx: &egui::Context) {
        let Some((index, doc)) = self.active_ids() else { return };
        let Some(collab) = self.collab.as_mut() else { return };
        if !collab.connected || collab.doc != doc || collab.last_presence.elapsed() < Duration::from_millis(100) {
            return;
        }
        let Some(actor) = collab.actor.clone() else { return };
        let Some(view) = self.views.get(index) else { return };
        let presence = Presence {
            actor_id: actor,
            role: collab.role,
            page: PageRef { index: view.current.min(u32::MAX as usize) as u32, epoch: collab.epoch },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: f64::from(view.zoom) },
            cursor: None,
            selection: None,
            color: collab.color.clone(),
        };
        collab.last_presence = Instant::now();
        collab.handle.presence(presence);
        // Presence is ephemeral; keep the clock moving without redrawing pages.
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    /// Send a user edit to the server instead of applying it locally. `None`
    /// means the edit is not ours to handle (no session for this document).
    pub(crate) fn collab_apply_edit(&mut self, edit: &Edit) -> Option<bool> {
        let (_, doc) = self.active_ids()?;
        let collab = self.collab.as_mut()?;
        if collab.doc != doc {
            return None;
        }
        if !collab.connected {
            collab.error = Some("Waiting for the collaboration session".into());
            return Some(false);
        }
        if !collab.role.can_write() {
            collab.error = Some("This role cannot change the document".into());
            return Some(false);
        }
        let structural = matches!(
            edit,
            Edit::RotatePages { .. }
                | Edit::MovePages { .. }
                | Edit::DeletePages { .. }
                | Edit::InsertBlankPage { .. }
                | Edit::DuplicatePages { .. }
                | Edit::InsertPagesFrom { .. }
        );
        if structural && !collab.role.can_structural() {
            collab.error = Some("This role cannot change pages".into());
            return Some(false);
        }
        let Some(document) = self.session.get(doc) else { return Some(false) };
        let mut counter = collab.counter;
        let ops = {
            let client_id = collab.client_id.clone();
            ops_for_edit(document, edit, collab.doc_key.clone(), &client_id, collab.version, collab.epoch, &mut counter)
        };
        collab.counter = counter;
        let ops = match ops {
            Ok(ops) => ops,
            Err(e) => {
                collab.error = Some(e.to_string());
                return Some(false);
            }
        };
        let mut inverses = Vec::with_capacity(ops.len());
        let Some(document) = self.session.get(doc) else { return Some(false) };
        for op in &ops {
            let Some(id) = collab.next_op_id() else {
                collab.error = Some("Could not allocate an operation id".into());
                return Some(false);
            };
            inverses.push(inverse_for(document, op, id).ok().flatten());
        }
        for op in &ops {
            if !collab.handle.enqueue(op.clone()) {
                collab.error = Some("The collaboration session is closed".into());
                return Some(false);
            }
        }
        collab.history.push(HistoryGroup { label: edit.label().to_string(), originals: ops, inverses });
        collab.redo.clear();
        Some(true)
    }

    /// Undo/redo through inverse operations (ADR-0010). Returns false when no
    /// session owns this document, so the local history runs instead.
    pub(crate) fn collab_history_step(&mut self, undo: bool) -> bool {
        let Some((_, doc)) = self.active_ids() else { return true };
        let Some(collab) = self.collab.as_mut() else { return false };
        if collab.doc != doc {
            return false;
        }
        if !collab.connected {
            return true;
        }
        let group = if undo { collab.history.pop() } else { collab.redo.pop() };
        let Some(mut group) = group else {
            collab.status = if undo { "Nothing to undo".into() } else { "Nothing to redo".into() };
            return true;
        };
        let ops = if undo {
            let available: Vec<Op> = group.inverses.iter().flatten().cloned().collect();
            if available.is_empty() {
                collab.status = format!("{} cannot be undone", group.label);
                collab.history.push(group);
                return true;
            }
            available
        } else {
            group.originals.clone()
        };
        let mut sent = Vec::with_capacity(ops.len());
        for mut op in ops {
            let Some(id) = collab.next_op_id() else { break };
            op.op_id = id;
            if collab.handle.enqueue(op.clone()) {
                sent.push(op);
            }
        }
        if sent.is_empty() {
            return true;
        }
        group.originals = sent;
        if undo {
            collab.redo.push(group);
        } else {
            collab.history.push(group);
        }
        true
    }

    /// The participants and status window. Drawn over the workspace only while
    /// a session is open.
    pub(crate) fn show_collab(&mut self, ctx: &egui::Context) {
        let Some(collab) = self.collab.as_mut() else { return };
        let mut commit = false;
        egui::Window::new("Collaboration").resizable(false).collapsible(true).anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0]).show(ctx, |ui| {
            ui.label(format!("{} · v{} · epoch {}", role_name(collab.role), collab.version, collab.epoch));
            ui.label(&collab.status);
            if let Some(error) = &collab.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.separator();
            ui.label("Participants");
            if collab.participants.is_empty() {
                ui.weak("Only you");
            }
            for (actor, presence) in &collab.participants {
                ui.label(format!("{actor} · {} · page {}", role_name(presence.role), presence.page.index + 1));
            }
            ui.separator();
            if collab.role.can_structural() && ui.button("Commit to the cabinet").clicked() {
                commit = true;
            }
            if let Some((sha, bytes)) = &collab.committed {
                ui.weak(format!("{sha} · {bytes} bytes"));
            }
        });
        if commit {
            collab.handle.commit(None);
        }
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Editor => "editor",
        Role::Commenter => "commenter",
        Role::Viewer => "viewer",
    }
}

fn code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Auth => "Not authorised",
        ErrorCode::Lease => "Another editor is changing pages",
        ErrorCode::Stale => "Out of date",
        ErrorCode::Conflict => "Conflict",
        ErrorCode::Forbidden => "Not allowed",
        ErrorCode::Invalid => "Invalid request",
        ErrorCode::Limit => "Limit reached",
        ErrorCode::Unsupported => "Not shared yet",
    }
}

fn color_of(actor: &str) -> String {
    let mut hash: u32 = 2_166_136_261;
    for byte in actor.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("#{:06x}", hash & 0x00ff_ffff)
}
