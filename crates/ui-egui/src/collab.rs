//! Collaborative session wiring for the desktop app (feature `collab`, off by
//! default). The socket lives in `pdfcraft-collab-session`; this module starts
//! it from the environment, applies accepted operations to the open document,
//! turns edits into operations, and draws the participants window.
//!
//! Nothing here runs for a normal single-user session: without
//! `PDFCRAFT_COLLAB_URL`, `PDFCRAFT_COLLAB_TOKEN` and `PDFCRAFT_COLLAB_DOC`
//! the module does nothing.

use crate::PdfCraftApp;
use pdfcraft_collab_client_pdf::{apply, inverse_after, inverse_for, ops_for_edit};
use pdfcraft_collab_proto::{ActorId, ClientInfo, DocKey, ErrorCode, Op, OpId, PageRef, Presence, Role, Viewport};
use pdfcraft_collab_session::{Config as WireConfig, Event, Handle, spawn};
use pdfcraft_engine::{DocId, Edit};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long another participant's presence counts as current (ADR-0006).
const PRESENCE_TTL: Duration = Duration::from_secs(30);
/// The most presence frames per second one client sends (ADR-0006).
const PRESENCE_INTERVAL: Duration = Duration::from_millis(100);
/// A heartbeat while nothing moves, so a reader is not "inactive" (well under the TTL).
const PRESENCE_HEARTBEAT: Duration = Duration::from_secs(1);

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
    /// Recent presence per actor, with when it arrived: the TTL is local, since
    /// presence is never journaled or replayed.
    participants: BTreeMap<String, Seen>,
    color: String,
    status: String,
    error: Option<String>,
    history: Vec<HistoryGroup>,
    redo: Vec<HistoryGroup>,
    counter: u64,
    last_presence: Instant,
    /// What was last sent, so only a change (or the heartbeat) is sent again.
    sent_presence: Option<Presence>,
    committed: Option<(String, u64)>,
}

struct Seen {
    presence: Presence,
    at: Instant,
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
                    sent_presence: None,
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

    /// Replace the inverse of `op` once the server has answered. A deletion has
    /// no local inverse (the content is gone from this replica), so its undo is
    /// built from the accepted operation's copy token (RFC §6).
    fn patch_inverse(&mut self, op_id: &OpId, inverse: Op) {
        for group in self.history.iter_mut().chain(self.redo.iter_mut()) {
            let Some(index) = group.originals.iter().position(|op| op.op_id == *op_id) else { continue };
            if let Some(slot) = group.inverses.get_mut(index) {
                *slot = Some(inverse.clone());
            }
            return;
        }
    }

    /// Other participants' pointers this replica may draw: fresh (ADR-0006 TTL),
    /// on the page tree this replica is showing, with a position.
    fn cursors(&self, now: Instant) -> Vec<crate::canvas::RemoteCursor> {
        self.participants
            .values()
            .filter(|seen| now.duration_since(seen.at) < PRESENCE_TTL && seen.presence.page.epoch == self.epoch)
            .filter_map(|seen| {
                let cursor = seen.presence.cursor?;
                Some(crate::canvas::RemoteCursor {
                    actor: seen.presence.actor_id.as_str().to_owned(),
                    color: color_of_hex(&seen.presence.color),
                    page: usize::try_from(seen.presence.page.index).ok()?,
                    point: [cursor[0] as f64, cursor[1] as f64],
                })
            })
            .collect()
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
                if let Some(inverse) = inverse_after(&applied) {
                    collab.patch_inverse(&applied.op.op_id, inverse);
                }
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
                    collab.participants.insert(presence.actor_id.as_str().to_owned(), Seen { presence, at: Instant::now() });
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
        if !collab.connected || collab.doc != doc {
            // No session, no cursors: nothing must stay on the page from before.
            if let Some(view) = self.views.get_mut(index) {
                view.remote_cursors.clear();
            }
            return;
        }
        let now = Instant::now();
        // What the others show: fresh presence only, and only addresses that
        // still name a page in this replica's page tree.
        if let Some(view) = self.views.get_mut(index) {
            view.remote_cursors = collab.cursors(now);
        }
        let Some(actor) = collab.actor.clone() else { return };
        let Some(view) = self.views.get(index) else { return };
        // The pointer over the page in PDF user space: the page module resolved
        // it this frame, so this is the same point the user sees.
        let cursor = view.local_cursor.filter(|(page, _)| *page == view.current).map(|(_, point)| [point[0].round() as i64, point[1].round() as i64]);
        let presence = Presence {
            actor_id: actor,
            role: collab.role,
            page: PageRef { index: view.current.min(u32::MAX as usize) as u32, epoch: collab.epoch },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: f64::from(view.zoom) },
            cursor,
            selection: None,
            color: collab.color.clone(),
        };
        let moved = collab.sent_presence.as_ref().is_none_or(|sent| {
            sent.page != presence.page || sent.cursor != presence.cursor || sent.role != presence.role || sent.viewport.zoom != presence.viewport.zoom
        });
        let elapsed = collab.last_presence.elapsed();
        if !moved && elapsed < PRESENCE_HEARTBEAT {
            // Nothing to say; the next frame keeps the cursor clock running.
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        if elapsed < PRESENCE_INTERVAL {
            // Coalesce: only the last state is sent, when the interval is over.
            ctx.request_repaint_after(PRESENCE_INTERVAL - elapsed);
            return;
        }
        collab.last_presence = now;
        collab.sent_presence = Some(presence.clone());
        collab.handle.presence(presence);
        // Presence is ephemeral; keep the clock moving without redrawing pages.
        ctx.request_repaint_after(PRESENCE_INTERVAL);
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
            let now = Instant::now();
            for (actor, seen) in &collab.participants {
                let active = now.duration_since(seen.at) < PRESENCE_TTL;
                let where_ = if seen.presence.page.epoch == collab.epoch {
                    format!("page {}", seen.presence.page.index + 1)
                } else {
                    "another page set".to_owned()
                };
                let state = if active { "" } else { " · inactive" };
                ui.label(format!("{actor} · {} · {where_}{state}", role_name(seen.presence.role)));
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
        ErrorCode::Internal => "Server error",
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

/// A presence colour (`#RRGGBB`) as egui draws it. A malformed token is the
/// fallback colour, never a crash: the token comes from another client.
fn color_of_hex(color: &str) -> egui::Color32 {
    let hex = color.strip_prefix('#').unwrap_or(color);
    if hex.len() == 6
        && let Ok(value) = u32::from_str_radix(hex, 16)
    {
        return egui::Color32::from_rgb(((value >> 16) & 0xff) as u8, ((value >> 8) & 0xff) as u8, (value & 0xff) as u8);
    }
    egui::Color32::from_rgb(0x33, 0x66, 0xcc)
}
