//! Collaborative session wiring (feature `collab`, off by default). The
//! transport lives behind `pdfcraft-collab-session`'s [`Sink`]: the desktop runs
//! a worker thread with a blocking socket, the browser drives the page's
//! `WebSocket` from this frame loop. This module starts a session (from the
//! environment on the desktop, from the host page in the browser), applies
//! accepted operations to the open document, turns edits into operations, draws
//! the participants window and reports the state to the host page.
//!
//! Nothing here runs for a normal single-user session: without
//! `PDFCRAFT_COLLAB_URL`, `PDFCRAFT_COLLAB_TOKEN` and `PDFCRAFT_COLLAB_DOC` on
//! the desktop, and without a `pdfcraft:session` message in the browser, the
//! module does nothing.

use crate::PdfCraftApp;
use crate::canvas::{DocView, RemoteSelection, RemoteSelectionKind};
use crate::hooks::HostSession;
use pdfcraft_collab_client_pdf::{
    ProjectionError, SplitPart, apply, combine_parts_ops, inverse_after, inverse_for, ops_for_edit, split_pages_op, split_part,
};
use pdfcraft_collab_proto::{ActorId, Applied, ClientInfo, DocKey, ErrorCode, Op, OpId, PageRef, Presence, Role, Selection, Target, Viewport};
use pdfcraft_collab_session::{Config as WireConfig, Event, Sink};
use pdfcraft_engine::{DocId, Document, Edit};
use pdfcraft_render::DocInfo;
use std::collections::BTreeMap;
use web_time::{Duration, Instant, SystemTime};

/// How long another participant's presence counts as current (ADR-0006).
const PRESENCE_TTL: Duration = Duration::from_secs(30);
/// The most presence frames per second one client sends (ADR-0006).
const PRESENCE_INTERVAL: Duration = Duration::from_millis(100);
/// A heartbeat while nothing moves, so a reader is not "inactive" (well under the TTL).
const PRESENCE_HEARTBEAT: Duration = Duration::from_secs(1);

/// A session handed over by the host page (`pdfcraft:session`). The token is a
/// credential: it is never logged, never put in a URL and never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionParams {
    pub session_id: String,
    pub url: String,
    pub token: String,
    /// The role the cabinet assigned. The token decides the real one (ADR-0008
    /// §7); this only sets the first status line.
    pub role: Option<Role>,
    /// The name other participants see.
    pub name: String,
}

impl std::fmt::Debug for SessionParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionParams")
            .field("session_id", &self.session_id)
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("role", &self.role)
            .field("name", &self.name)
            .finish()
    }
}

impl SessionParams {
    /// The host page's session: its name, its socket URL, its token.
    pub fn from_host(session: &HostSession, name: String) -> Self {
        Self {
            session_id: session.session_id.clone(),
            url: session.url.clone(),
            token: session.token.clone(),
            role: session.role.as_deref().and_then(role_from_name),
            name,
        }
    }
}

/// What the host page (and the tests) can see of a session.
#[derive(Clone, Debug, PartialEq)]
pub struct CollabState {
    pub role: Role,
    pub version: u64,
    pub epoch: u64,
    pub connected: bool,
    pub status: String,
    pub error: Option<String>,
    pub participants: Vec<CollabParticipant>,
}

/// One other participant, as the host page shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct CollabParticipant {
    /// The actor id the server assigned: stable for the session.
    pub client_id: String,
    pub role: Role,
    /// A CSS colour (`#rrggbb`).
    pub color: String,
    /// The page they are on (1-based), when their presence is current.
    pub page: Option<u32>,
    /// Their presence is not older than [`PRESENCE_TTL`].
    pub active: bool,
    /// What they have selected, when it is current and on this page tree. Geometry and
    /// comment names only: the host page never receives the document's text.
    pub selection: Option<ParticipantSelection>,
}

/// A participant's selection in the host page's terms. The cabinet's markers are drawn
/// from it; the app already draws the same selection on its own canvas.
#[derive(Clone, Debug, PartialEq)]
pub enum ParticipantSelection {
    /// Selected comments by `/NM`, the one the participant acts on first.
    Annotations(Vec<String>),
    /// A text range on `page` (1-based): one rectangle per line piece in PDF user space,
    /// and the caret at the active end.
    Text { page: u32, rects: Vec<[i64; 4]>, caret: Option<[i64; 2]> },
}

impl CollabParticipant {
    pub fn role_label(&self) -> &'static str {
        role_name(self.role)
    }
}

/// Reports the session state (status and participants) to the host page.
pub type HostCollabStateFn = Box<dyn FnMut(&CollabState)>;
/// Asks the host page to commit (the cabinet owns the commit in the browser).
pub type HostCommitFn = Box<dyn FnMut()>;

pub(crate) struct CollabUi {
    /// The transport this platform has: a worker thread or the page's socket.
    sink: Box<dyn Sink>,
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
    /// A Combine confirmation still joining its parts, if one is (RFC §3.1.5).
    combine: Option<CombineBatch>,
    /// An undo/redo whose operations must be sent one at a time (structural).
    sequence: Option<OpSequence>,
    counter: u64,
    last_presence: Instant,
    /// What was last sent, so only a change (or the heartbeat) is sent again.
    sent_presence: Option<Presence>,
    /// The result of the last commit, ready to show.
    committed: Option<String>,
    /// What the host page was last told, so it hears a change and not a frame.
    reported: Option<ReportKey>,
    reported_presence: Option<Vec<PresenceKey>>,
    reported_at: Instant,
}

/// What a presence report is compared by: who is where, and what they have selected.
#[derive(Clone, PartialEq)]
struct PresenceKey {
    client_id: String,
    page: Option<u32>,
    active: bool,
    selection: Option<ParticipantSelection>,
}

/// What a session-state report is compared by. Page positions and selections are
/// excluded: they change with the pointer and belong to the presence report, which is
/// coalesced separately.
#[derive(Clone, PartialEq)]
struct ReportKey {
    role: Role,
    version: u64,
    epoch: u64,
    connected: bool,
    status: String,
    error: Option<String>,
    participants: Vec<(String, Role, bool)>,
}

struct Seen {
    presence: Presence,
    at: Instant,
}

impl Seen {
    /// Presence older than the TTL is a reader we stop drawing (ADR-0006).
    fn is_current(&self, now: Instant) -> bool {
        now.duration_since(self.at) < PRESENCE_TTL
    }
}

/// What another participant is selecting, as this replica may draw it. `None` when their
/// presence is stale, on another page tree, or carries no selection. `epoch` is this
/// replica's, so a selection addressed to a page set we have left is not drawn.
fn visible_selection(seen: &Seen, now: Instant, epoch: u64) -> Option<RemoteSelection> {
    if !seen.is_current(now) || seen.presence.page.epoch != epoch {
        return None;
    }
    let actor = seen.presence.actor_id.as_str().to_owned();
    let color = color_of_hex(&seen.presence.color);
    match seen.presence.selection.as_ref()? {
        Selection::Annotation { nm, also } => Some(RemoteSelection {
            actor,
            color,
            page: usize::try_from(seen.presence.page.index).ok()?,
            kind: if also.is_empty() {
                RemoteSelectionKind::Annotation(nm.clone())
            } else {
                RemoteSelectionKind::Annotations(std::iter::once(nm.clone()).chain(also.iter().cloned()).collect())
            },
            caret: None,
        }),
        Selection::Text { page, rect, quads, caret } => {
            if page.epoch != epoch {
                return None;
            }
            let rects = if quads.is_empty() { vec![*rect] } else { quads.clone() };
            Some(RemoteSelection {
                actor,
                color,
                page: usize::try_from(page.index).ok()?,
                kind: RemoteSelectionKind::Rects(rects.iter().map(|r| r.map(|v| v as f64)).collect()),
                caret: caret.map(|c| [c[0] as f64, c[1] as f64]),
            })
        }
    }
}

/// A participant's selection as the host page sees it: comment names, or a range's page,
/// per-line rectangles and caret. `None` when the frame carries no selection.
fn participant_selection(presence: &Presence) -> Option<ParticipantSelection> {
    match presence.selection.as_ref()? {
        Selection::Annotation { nm, also } => {
            Some(ParticipantSelection::Annotations(std::iter::once(nm.clone()).chain(also.iter().cloned()).collect()))
        }
        Selection::Text { page, rect, quads, caret } => {
            let rects = if quads.is_empty() { vec![*rect] } else { quads.clone() };
            Some(ParticipantSelection::Text { page: page.index.saturating_add(1), rects, caret: *caret })
        }
    }
}

/// What this client is selecting, in the form presence carries: the `/NM` of every selected
/// comment on the page in view, else the selection's per-line rectangles in user space and
/// its caret. `None` when nothing is selected. The payload is geometry and names only — never
/// the document's text or the comment's contents.
fn local_selection(view: &DocView, info: &DocInfo, epoch: u64) -> Option<Selection> {
    // Several comments selected together travel as one list: the primary first, then the
    // rest. Only comments on the page in view are named, because a presence frame names one
    // page; a comment on another page falls back to its rectangle, as before.
    let mut names: Vec<String> = Vec::new();
    for (page, index) in view.comments.selection() {
        if page != view.current {
            continue;
        }
        if let Some(nm) = info.annotations.iter().find(|a| a.page == page && a.index == index).and_then(|a| a.name.clone())
            && !names.contains(&nm)
        {
            names.push(nm);
        }
        if names.len() >= pdfcraft_collab_proto::MAX_SELECTION_NAMES {
            break;
        }
    }
    if let Some(nm) = names.first().cloned() {
        let also = names.split_off(1);
        return Some(Selection::Annotation { nm, also });
    }
    if let Some((page, index)) = view.comments.selected
        && let Some(annotation) = info.annotations.iter().find(|a| a.page == page && a.index == index)
        && let Some(rect) = annotation_selection_rect(annotation)
    {
        return Some(Selection::Text { page: PageRef { index: u32::try_from(page).ok()?, epoch }, rect, quads: Vec::new(), caret: None });
    }
    let (page, quads) = view.selection_quads(info)?;
    let page_ref = PageRef { index: u32::try_from(page).ok()?, epoch };
    Some(text_selection(quads, page_ref, view.selection_caret(info).filter(|(caret_page, _)| *caret_page == page).map(|(_, point)| point)))
}

/// A text range as presence carries it: the bounding box (what a peer that predates per-line
/// geometry draws), the per-line rectangles when they fit, and the caret at the active end.
fn text_selection(quads: Vec<[f64; 8]>, page: PageRef, caret: Option<[f64; 2]>) -> Selection {
    let rect = rect_of_quads(&quads).unwrap_or([0, 0, 0, 0]);
    let mut rects: Vec<[i64; 4]> = Vec::new();
    for quad in &quads {
        if rects.len() >= pdfcraft_collab_proto::MAX_SELECTION_QUADS {
            // Too many lines for one frame: the bounding box still says where the range is.
            rects.clear();
            break;
        }
        if let Some(rect) = quad_rect_i64(quad) {
            rects.push(rect);
        }
    }
    let caret = caret.map(|[x, y]| [round_unit_f64(x), round_unit_f64(y)]);
    Selection::Text { page, rect, quads: rects, caret }
}

/// One view-space quad as the `[x0, y0, x1, y1]` whole units presence carries. A quad whose
/// corners are not finite is skipped.
fn quad_rect_i64(q: &[f64; 8]) -> Option<[i64; 4]> {
    let mut rect: Option<[f64; 4]> = None;
    for [x, y] in [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]] {
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        rect = Some(match rect {
            None => [x, y, x, y],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        });
    }
    let [x0, y0, x1, y1] = rect?;
    Some([round_unit_f64(x0), round_unit_f64(y0), round_unit_f64(x1), round_unit_f64(y1)])
}

/// A user-space coordinate as the whole unit presence sends; non-finite input becomes 0
/// rather than a cast artefact.
fn round_unit_f64(value: f64) -> i64 {
    if value.is_finite() { value.round() as i64 } else { 0 }
}

/// A selected comment's geometry: its markup quads, else its rectangle.
fn annotation_selection_rect(annotation: &pdfcraft_render::Annotation) -> Option<[i64; 4]> {
    let quads: Vec<[f64; 8]> = annotation.quads.iter().map(|q| q.map(f64::from)).collect();
    rect_of_quads(&quads).or_else(|| {
        let [x0, y0, x1, y1] = annotation.rect;
        Some([round_unit(x0), round_unit(y0), round_unit(x1), round_unit(y1)])
    })
}

/// The bounding rectangle of a selection's quadrilaterals, in user space, rounded to the
/// whole units presence carries. Non-finite corners are skipped; no quads means no selection.
fn rect_of_quads(quads: &[[f64; 8]]) -> Option<[i64; 4]> {
    let mut rect: Option<[f64; 4]> = None;
    for q in quads {
        for [x, y] in [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]] {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            rect = Some(match rect {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
    }
    let [x0, y0, x1, y1] = rect?;
    Some([round_unit(x0 as f32), round_unit(y0 as f32), round_unit(x1 as f32), round_unit(y1 as f32)])
}

/// A user-space coordinate as the whole unit presence sends; non-finite input becomes 0
/// rather than a cast artefact.
fn round_unit(value: f32) -> i64 {
    if value.is_finite() { value.round() as i64 } else { 0 }
}

struct HistoryGroup {
    label: String,
    originals: Vec<Op>,
    inverses: Vec<Option<Op>>,
}

/// The history label of the Split dialog's operation. A split with this label
/// is a user's Split (the pages that leave are theirs to keep); a split sent as
/// an undo or a rollback is somebody's inverse and opens nothing.
const SPLIT_LABEL: &str = "Split pages";
/// The history label of one Combine confirmation: all its parts are one undo step.
const COMBINE_LABEL: &str = "Combine pages";

/// One "Combine files" confirmation in a room: its parts join one at a time,
/// because a structural operation is accepted only at the head of the version.
/// The order is the list's order, and a refusal of any part takes the parts
/// already joined out again in reverse order, so the confirmation is all or
/// nothing (RFC §3.1.5).
struct CombineBatch {
    /// Parts not sent yet, in confirmation order.
    pending: std::collections::VecDeque<Op>,
    /// The part whose answer is awaited.
    in_flight: Option<Op>,
    /// Where each part that joined sits, oldest first: the split that takes it
    /// out again is built from this once the answer is in.
    joined: Vec<JoinedPart>,
    /// Files the confirmation names (one file may be several parts).
    files: usize,
    /// The operation that opened the history group, so a rollback can drop it.
    group_op: OpId,
    /// The confirmation is being taken back; `pending` holds its splits.
    rollback: bool,
    /// A retry asked for a snapshot: the splits wait for it, or they would be
    /// stale again with the old base version.
    after_snapshot: bool,
}

#[derive(Clone, Copy)]
struct JoinedPart {
    at: usize,
    count: usize,
}

/// Operations that must go out one at a time: a structural operation is
/// accepted only at the head of the version, so an undo of a multi-part
/// confirmation sends its splits in sequence, each addressed at the head the
/// previous answer left.
struct OpSequence {
    pending: std::collections::VecDeque<Op>,
    in_flight: Option<Op>,
}

/// What a Combine confirmation needs after an accepted part.
enum CombineAdvance {
    /// Not this confirmation's operation, or none in flight.
    Idle,
    /// Send the operation (the next part, or the next split of a rollback).
    Send(Op),
    /// The last part joined; the count for the message.
    Finished(usize),
    /// The rollback finished.
    RolledBack,
}

/// The name of the document a split's part is kept in: the shared document's
/// stem and the pages that left, as the single-user Split names its parts.
fn part_name(document: &str, part: &SplitPart) -> String {
    let stem = document.strip_suffix(".pdf").or_else(|| document.strip_suffix(".PDF")).unwrap_or(document);
    if part.to == part.from.saturating_add(1) {
        format!("{stem} (page {}).pdf", part.from + 1)
    } else {
        format!("{stem} (pages {}-{}).pdf", part.from + 1, part.to)
    }
}

/// Address an operation, and the page its target names, at `version`/`epoch`:
/// what the session does when it queues an intent, and what a sequence does for
/// each step after the previous answer moved the head on.
fn readdress(op: &mut Op, version: u64, epoch: u64) {
    op.base_version = version;
    op.epoch = epoch;
    match &mut op.target {
        Target::Page(page) => page.epoch = epoch,
        Target::Annotation { page, .. } => page.epoch = epoch,
        _ => {}
    }
}

impl CollabUi {
    /// Start the session the desktop environment asks for, if it does. A
    /// refused configuration stops the attempts: the environment does not
    /// change while the app runs, so retrying every frame would only repeat the
    /// same error.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_from_env(app: &mut PdfCraftApp) {
        let Ok(url) = std::env::var("PDFCRAFT_COLLAB_URL") else { return };
        let Ok(token) = std::env::var("PDFCRAFT_COLLAB_TOKEN") else { return };
        let Ok(key) = std::env::var("PDFCRAFT_COLLAB_DOC") else { return };
        // The app opens the document named on the command line first; until it
        // is there a session has nothing to join.
        if app.active_ids().is_none() {
            return;
        }
        let name = std::env::var("PDFCRAFT_COLLAB_NAME").unwrap_or_else(|_| "PdfCraft".into());
        let params = SessionParams { session_id: key.clone(), url, token, role: None, name };
        if !Self::join(app, params) {
            app.collab_off = true;
        }
    }

    /// Start (or replace) a session for the open document. `false` when the
    /// session could not start (nothing open, a malformed id or address).
    pub(crate) fn join(app: &mut PdfCraftApp, params: SessionParams) -> bool {
        let Some((_, doc)) = app.active_ids() else {
            app.notify_error("Open the document before joining a collaboration session");
            return false;
        };
        let fallback = DocKey::new(params.session_id.clone());
        let doc_key = match doc_key_from_token(&params.token, &params.session_id).or_else(|| fallback.ok()) {
            Some(key) => key,
            None => {
                app.notify_error("The collaboration document id is not valid");
                return false;
            }
        };
        if params.url.is_empty() {
            app.notify_error("The collaboration session has no server address");
            return false;
        }
        let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| format!("{:x}", d.as_nanos())).unwrap_or_else(|_| "0".into());
        let info = ClientInfo { name: params.name, version: env!("CARGO_PKG_VERSION").into() };
        match pdfcraft_collab_session::open(WireConfig::new(params.url, params.token, doc_key.clone(), info)) {
            Ok(sink) => {
                if let Some(mut previous) = app.collab.take() {
                    previous.sink.close();
                }
                app.collab = Some(Self {
                    sink,
                    doc,
                    doc_key,
                    actor: None,
                    client_id: format!("client-{nonce}"),
                    role: params.role.unwrap_or(Role::Viewer),
                    connected: false,
                    version: 0,
                    epoch: 0,
                    participants: BTreeMap::new(),
                    color: "#3366cc".into(),
                    status: "Connecting…".into(),
                    error: None,
                    history: Vec::new(),
                    redo: Vec::new(),
                    combine: None,
                    sequence: None,
                    counter: 0,
                    last_presence: Instant::now(),
                    sent_presence: None,
                    committed: None,
                    reported: None,
                    reported_presence: None,
                    reported_at: Instant::now(),
                });
            }
            Err(error) => {
                app.notify_error(format!("The collaboration session did not start: {error}"));
                return false;
            }
        }
        true
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

    /// True when the operation is a user's Split (the dialog action), not an
    /// undo or a rollback that also takes pages out.
    fn is_dialog_split(&self, op_id: &OpId) -> bool {
        self.history.iter().chain(self.redo.iter()).any(|group| group.label == SPLIT_LABEL && group.originals.iter().any(|op| op.op_id == *op_id))
    }

    /// Advance a Combine confirmation with an accepted operation: keep where the
    /// part joined, then send the next part or report the confirmation done.
    fn combine_advance(&mut self, applied: &Applied) -> CombineAdvance {
        let Some(batch) = self.combine.as_mut() else { return CombineAdvance::Idle };
        if !batch.in_flight.as_ref().is_some_and(|op| op.op_id == applied.op_id) {
            return CombineAdvance::Idle;
        }
        batch.in_flight = None;
        if !batch.rollback {
            let at = applied.op.payload.get("at").and_then(|at| at.as_u64()).and_then(|at| usize::try_from(at).ok());
            let count = applied.op.payload.get("count").and_then(|count| count.as_u64()).and_then(|count| usize::try_from(count).ok());
            if let (Some(at), Some(count)) = (at, count) {
                batch.joined.push(JoinedPart { at, count });
            }
        }
        let rollback = batch.rollback;
        let next = batch.pending.pop_front().map(|mut op| {
            // The head moved with the answer: address the next part where the
            // previous one left, like the session will when it queues it.
            readdress(&mut op, applied.version, applied.epoch);
            op
        });
        if let Some(op) = &next {
            batch.in_flight = Some(op.clone());
        }
        let finished = if next.is_none() { Some(if rollback { None } else { Some(batch.files) }) } else { None };
        if finished.is_some() {
            self.combine = None;
        }
        match (next, finished) {
            (Some(op), _) => CombineAdvance::Send(op),
            (None, Some(None)) => CombineAdvance::RolledBack,
            (None, Some(Some(files))) => CombineAdvance::Finished(files),
            (None, None) => CombineAdvance::Idle,
        }
    }

    /// True when `op_id` (as a string) is the confirmation's in-flight part or
    /// one of the parts still queued.
    fn combine_has(&self, op_id: &str) -> bool {
        self.combine.as_ref().is_some_and(|batch| {
            batch.in_flight.as_ref().is_some_and(|op| op.op_id.as_str() == op_id) || batch.pending.iter().any(|op| op.op_id.as_str() == op_id)
        })
    }

    fn combine_is_rolling_back(&self) -> bool {
        self.combine.as_ref().is_some_and(|batch| batch.rollback)
    }

    /// True when a retry asked for a snapshot and the rollback splits wait for it.
    fn combine_waiting_for_snapshot(&self) -> bool {
        self.combine.as_ref().is_some_and(|batch| batch.rollback && batch.after_snapshot)
    }

    /// Turn the confirmation into a rollback: nothing else is sent, the history
    /// group goes away, and the splits of the parts that joined are built once
    /// the version is known (`collab_combine_rollback`).
    fn combine_mark_rollback(&mut self, error: String, after_snapshot: bool) {
        let Some(group_op) = self.combine.as_ref().map(|batch| batch.group_op.clone()) else { return };
        let Some(batch) = self.combine.as_mut() else { return };
        if batch.rollback {
            return;
        }
        batch.rollback = true;
        batch.after_snapshot = after_snapshot;
        batch.pending.clear();
        batch.in_flight = None;
        self.error = Some(error);
        self.history.retain(|group| group.originals.iter().all(|op| op.op_id != group_op));
        self.redo.clear();
    }

    /// Advance an undo/redo sequence with an accepted operation: address the
    /// next one at the head this answer left and return it to send. `None` when
    /// the operation is not part of the sequence or nothing is left.
    fn sequence_advance(&mut self, applied: &Applied) -> Option<Op> {
        let (next, done) = {
            let sequence = self.sequence.as_mut()?;
            if !sequence.in_flight.as_ref().is_some_and(|op| op.op_id == applied.op_id) {
                return None;
            }
            sequence.in_flight = None;
            let next = sequence.pending.pop_front();
            (next, sequence.pending.is_empty())
        };
        let Some(mut op) = next else {
            if done {
                self.sequence = None;
            }
            return None;
        };
        readdress(&mut op, applied.version, applied.epoch);
        if let Some(sequence) = self.sequence.as_mut() {
            sequence.in_flight = Some(op.clone());
        }
        Some(op)
    }

    /// True when `op_id` is the sequence's in-flight operation or one still
    /// queued.
    fn sequence_has(&self, op_id: &str) -> bool {
        self.sequence.as_ref().is_some_and(|sequence| {
            sequence.in_flight.as_ref().is_some_and(|op| op.op_id.as_str() == op_id) || sequence.pending.iter().any(|op| op.op_id.as_str() == op_id)
        })
    }

    /// Other participants' pointers this replica may draw: fresh (ADR-0006 TTL),
    /// on the page tree this replica is showing, with a position.
    fn cursors(&self, now: Instant) -> Vec<crate::canvas::RemoteCursor> {
        self.participants
            .values()
            .filter(|seen| seen.is_current(now) && seen.presence.page.epoch == self.epoch)
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

    /// Other participants' selections this replica may draw: the same freshness and
    /// page-tree rules as the pointers. A selection is presence, never an operation.
    fn selections(&self, now: Instant) -> Vec<RemoteSelection> {
        self.participants.values().filter_map(|seen| visible_selection(seen, now, self.epoch)).collect()
    }

    /// The session as the host page and the tests see it.
    fn state(&self, now: Instant) -> CollabState {
        let participants = self
            .participants
            .iter()
            .map(|(actor, seen)| {
                let active = now.duration_since(seen.at) < PRESENCE_TTL;
                let current = active && seen.presence.page.epoch == self.epoch;
                CollabParticipant {
                    client_id: actor.clone(),
                    role: seen.presence.role,
                    color: seen.presence.color.clone(),
                    page: current.then_some(seen.presence.page.index.saturating_add(1)),
                    active,
                    selection: current.then(|| participant_selection(&seen.presence)).flatten(),
                }
            })
            .collect();
        CollabState {
            role: self.role,
            version: self.version,
            epoch: self.epoch,
            connected: self.connected,
            status: self.status.clone(),
            error: self.error.clone(),
            participants,
        }
    }
}

/// Start the session when the environment (desktop) or the host page (browser)
/// asks for one, drain its events and send our presence. Called once per frame
/// from `logic`.
impl PdfCraftApp {
    pub(crate) fn poll_collab(&mut self, ctx: &egui::Context) {
        self.poll_host_collab();
        if self.collab.is_none() && !self.collab_off {
            #[cfg(not(target_arch = "wasm32"))]
            CollabUi::start_from_env(self);
        }
        let mut events = Vec::new();
        if let Some(collab) = self.collab.as_mut() {
            while let Some(event) = collab.sink.try_event() {
                events.push(event);
            }
        }
        for event in events {
            self.collab_event(event, ctx);
        }
        self.collab_presence(ctx);
        self.report_collab();
        // The browser has no thread to wake egui: the socket's callbacks only
        // queue what arrived, so an open session keeps the frame loop running.
        // Without one the app stays on its own repaint schedule (a single-user
        // web session must not tick ten times a second for nothing).
        #[cfg(target_arch = "wasm32")]
        if self.collab.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Take a session the host page handed over (`pdfcraft:session`) and the
    /// cabinet's commit answers (`pdfcraft:committed`).
    pub(crate) fn poll_host_collab(&mut self) {
        let sessions = self.host_session_inbox.lock().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
        for session in sessions {
            if self.active_ids().is_none() {
                // The host sends `open` and `session` together; the file is opened
                // later in this frame, so the session waits for the next one
                // instead of being refused for a document that is about to exist.
                if let Ok(mut queue) = self.host_session_inbox.lock() {
                    queue.push(session);
                }
                continue;
            }
            let name = self.comment_prefs.author.clone();
            let params = SessionParams::from_host(&session, name);
            CollabUi::join(self, params);
        }
        let committed = self.host_committed_inbox.lock().map(|mut queue| std::mem::take(&mut *queue)).unwrap_or_default();
        for (version, etag) in committed {
            self.collab_committed(version, &etag);
        }
    }

    /// Start a session from the host page (the web shell calls this directly
    /// when it prefers not to go through the inbox).
    pub fn collab_join(&mut self, params: SessionParams) {
        CollabUi::join(self, params);
    }

    /// The cabinet committed the session: it owns the stored document now.
    pub fn collab_committed(&mut self, version: u64, etag: &str) {
        if let Some(collab) = self.collab.as_mut() {
            collab.committed = Some(format!("version {version} · {etag}"));
            collab.status = format!("Committed version {version}");
        }
    }

    /// Commit: to the host page when the cabinet owns the commit, to the sync
    /// server otherwise.
    pub fn collab_commit(&mut self) {
        if let Some(hook) = self.host_commit.as_mut() {
            hook();
            if let Some(collab) = self.collab.as_mut() {
                collab.status = "Commit requested from the cabinet".into();
            }
            return;
        }
        if let Some(collab) = self.collab.as_mut() {
            collab.sink.commit(None);
        }
    }

    /// What the host page shows in its participants panel, if a session is open.
    pub fn collab_state(&self) -> Option<CollabState> {
        self.collab.as_ref().map(|collab| collab.state(Instant::now()))
    }

    /// Tell the host page a change: `session-state` for the status line and the
    /// participant list, `presence` for page positions (coalesced, ADR-0006).
    fn report_collab(&mut self) {
        let Some(collab) = self.collab.as_mut() else { return };
        if self.host_collab_state.is_none() && self.host_collab_presence.is_none() {
            return;
        }
        let now = Instant::now();
        let state = collab.state(now);
        let key = ReportKey {
            role: state.role,
            version: state.version,
            epoch: state.epoch,
            connected: state.connected,
            status: state.status.clone(),
            error: state.error.clone(),
            participants: state.participants.iter().map(|p| (p.client_id.clone(), p.role, p.active)).collect(),
        };
        if collab.reported.as_ref() != Some(&key) {
            collab.reported = Some(key);
            if let Some(hook) = self.host_collab_state.as_mut() {
                hook(&state);
            }
        }
        let presence: Vec<PresenceKey> = state
            .participants
            .iter()
            .map(|p| PresenceKey { client_id: p.client_id.clone(), page: p.page, active: p.active, selection: p.selection.clone() })
            .collect();
        if collab.reported_presence.as_ref() != Some(&presence) && now.duration_since(collab.reported_at) >= PRESENCE_INTERVAL {
            collab.reported_presence = Some(presence);
            collab.reported_at = now;
            if let Some(hook) = self.host_collab_presence.as_mut() {
                hook(&state);
            }
        }
    }

    fn collab_event(&mut self, event: Event, ctx: &egui::Context) {
        let Some((index, doc)) = self.active_ids() else { return };
        let Some(collab) = self.collab.as_mut() else { return };
        if collab.doc != doc {
            return;
        }
        let mut taken: Option<(String, std::sync::Arc<Vec<u8>>)> = None;
        let mut finished: Option<usize> = None;
        let mut rollback_now = false;
        let mut rollback_after_snapshot = false;
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
            Event::Applied { applied, own } => {
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
                // A split the user asked for takes a part out of the room: keep
                // the pages by opening the part as a document of its own, in the
                // background so the shared document keeps the focus. Splits sent
                // as an undo or a rollback are inverses; they open nothing.
                if own
                    && collab.is_dialog_split(&applied.op_id)
                    && let Some(part) = split_part(&applied)
                    && let Some(name) = self.session.get(doc).map(|document| part_name(&document.name, &part))
                {
                    taken = Some((name, std::sync::Arc::new(part.bytes)));
                }
                match collab.combine_advance(&applied) {
                    CombineAdvance::Send(op) => {
                        if collab.sink.enqueue(op) {
                            collab.status = "Joining the parts…".into();
                        } else {
                            collab.error = Some("The collaboration session is closed".into());
                            collab.combine = None;
                        }
                    }
                    CombineAdvance::Finished(files) => finished = Some(files),
                    CombineAdvance::RolledBack | CombineAdvance::Idle => {}
                }
                if let Some(op) = collab.sequence_advance(&applied)
                    && !collab.sink.enqueue(op)
                {
                    collab.error = Some("The collaboration session is closed".into());
                    collab.sequence = None;
                }
                ctx.request_repaint();
            }
            Event::Rejected(rejected) => {
                for group in collab.history.iter_mut().chain(collab.redo.iter_mut()) {
                    group.originals.retain(|op| op.op_id != rejected.op_id);
                }
                collab.history.retain(|group| !group.originals.is_empty());
                collab.redo.retain(|group| !group.originals.is_empty());
                if collab.combine_has(rejected.op_id.as_str()) {
                    if collab.combine_is_rolling_back() {
                        // A split taking the confirmation back was refused too:
                        // leave the room as it is and say so.
                        collab.error = Some("The combine could not be fully taken back; check the pages".into());
                        collab.combine = None;
                    } else {
                        let why = format!("{}: {} The combine was taken back; try again.", code_name(rejected.code), rejected.message);
                        collab.combine_mark_rollback(why, false);
                        rollback_now = true;
                    }
                } else if collab.sequence_has(rejected.op_id.as_str()) {
                    // A step of an undo/redo sequence was refused: the rest
                    // cannot follow, so the sequence stops here.
                    collab.sequence = None;
                    if rejected.code != ErrorCode::Lease {
                        collab.error = Some(format!("{}: {}", code_name(rejected.code), rejected.message));
                    }
                } else if rejected.code != ErrorCode::Lease {
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
                // A retry asked for this snapshot: now the rollback splits have a
                // head to be built on.
                if collab.combine_waiting_for_snapshot() {
                    if let Some(batch) = collab.combine.as_mut() {
                        batch.after_snapshot = false;
                    }
                    rollback_after_snapshot = true;
                }
            }
            Event::Presence(presence) => {
                if collab.actor.as_ref() != Some(&presence.actor_id) {
                    collab.participants.insert(presence.actor_id.as_str().to_owned(), Seen { presence, at: Instant::now() });
                }
            }
            Event::Committed(result) => {
                collab.committed = Some(format!("{} · {} bytes", result.sha256, result.bytes));
                collab.status = format!("Committed version {version}", version = result.version);
            }
            Event::Disconnected => {
                collab.status = "Reconnecting…".into();
                collab.connected = false;
            }
            Event::Gap => collab.status = "Catching up…".into(),
            Event::Retry { op_id } => {
                collab.status = "Catching up…".into();
                if collab.combine_has(&op_id) {
                    if collab.combine_is_rolling_back() {
                        // A split taking the confirmation back went stale too:
                        // stop here and say what is left.
                        collab.combine = None;
                        collab.error = Some("The combine could not be fully taken back; check the pages".into());
                    } else {
                        // A structural operation cannot be re-addressed blindly:
                        // the page set may have moved. Take the parts that joined
                        // out again, after the snapshot the retry asked for, so
                        // the confirmation is all or nothing.
                        collab.combine_mark_rollback("The document changed; the combine was taken back. Try again.".into(), true);
                    }
                } else if collab.sequence_has(&op_id) {
                    // A structural step of an undo/redo cannot be re-addressed
                    // blindly: stop the sequence and say so.
                    collab.sequence = None;
                    collab.error = Some("The document changed; the undo did not finish. Try again.".into());
                }
            }
            Event::Error(error) => {
                collab.error = Some(format!("{}: {}", code_name(error.code), error.message));
                if error.code == ErrorCode::Auth {
                    collab.connected = false;
                    self.collab_off = true;
                }
            }
            Event::Notice(text) => collab.error = Some(text),
        }
        if let Some(files) = finished {
            self.notify_fmt("Combined {n} files into the shared document; Undo takes them out", &[("n", &files.to_string())]);
        }
        if rollback_now || rollback_after_snapshot {
            self.collab_combine_rollback(doc);
        }
        if let Some((name, bytes)) = taken {
            self.open_created_quiet(&name, bytes, "The pages taken out opened in a document of their own");
        }
    }

    /// Build and send the splits that take a rolled-back Combine confirmation
    /// out of the document, once the version they must be built on is known:
    /// immediately for a refusal, after the snapshot for a retry. One split at a
    /// time, like the parts themselves (RFC §3.1.5).
    fn collab_combine_rollback(&mut self, doc: DocId) {
        let Some(collab) = self.collab.as_ref() else { return };
        let Some(batch) = collab.combine.as_ref() else { return };
        if !batch.rollback || batch.after_snapshot {
            return;
        }
        let joined = batch.joined.clone();
        let Some(document) = self.session.get(doc) else { return };
        let Some(collab) = self.collab.as_mut() else { return };
        let mut counter = collab.counter;
        let mut ops = Vec::with_capacity(joined.len());
        for part in joined.iter().rev() {
            match split_pages_op(document, part.at, part.count, collab.doc_key.clone(), &collab.client_id, collab.version, collab.epoch, &mut counter)
            {
                Ok(op) => ops.push(op),
                Err(e) => collab.error = Some(format!("The combine could not be taken back: {e}")),
            }
        }
        collab.counter = counter;
        let Some(batch) = collab.combine.as_mut() else { return };
        batch.after_snapshot = false;
        let mut queue: std::collections::VecDeque<Op> = ops.into_iter().collect();
        let first = queue.pop_front();
        let sent = first.as_ref().is_some_and(|op| collab.sink.enqueue(op.clone()));
        if sent {
            batch.pending = queue;
            batch.in_flight = first.clone();
        }
        let failed = first.is_some() && !sent;
        if failed {
            collab.error = Some("The collaboration session is closed".into());
        }
        if first.is_none() || failed {
            collab.combine = None;
        } else {
            collab.status = "Taking the combine back…".into();
        }
    }

    fn collab_presence(&mut self, ctx: &egui::Context) {
        let Some((index, doc)) = self.active_ids() else { return };
        let Some(collab) = self.collab.as_mut() else { return };
        if !collab.connected || collab.doc != doc {
            // No session, no cursors or selections: nothing must stay on the page from before.
            if let Some(view) = self.views.get_mut(index) {
                view.remote_cursors.clear();
                view.remote_selections.clear();
            }
            return;
        }
        let now = Instant::now();
        // What the others show: fresh presence only, and only addresses that
        // still name a page in this replica's page tree.
        if let Some(view) = self.views.get_mut(index) {
            view.remote_cursors = collab.cursors(now);
            view.remote_selections = collab.selections(now);
        }
        let Some(actor) = collab.actor.clone() else { return };
        let Some(view) = self.views.get(index) else { return };
        // The pointer over the page in PDF user space: the page module resolved
        // it this frame, so this is the same point the user sees.
        let cursor = view.local_cursor.filter(|(page, _)| *page == view.current).map(|(_, point)| [point[0].round() as i64, point[1].round() as i64]);
        // The selection travels as a `/NM` or a rectangle, never as text: presence is not
        // an operation and must not be a way to read the document's contents.
        let selection = self.session.get(view.id).and_then(|document| local_selection(view, &document.info, collab.epoch));
        let presence = Presence {
            actor_id: actor,
            role: collab.role,
            page: PageRef { index: view.current.min(u32::MAX as usize) as u32, epoch: collab.epoch },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: f64::from(view.zoom) },
            cursor,
            selection,
            color: collab.color.clone(),
        };
        let moved = collab.sent_presence.as_ref().is_none_or(|sent| {
            sent.page != presence.page
                || sent.cursor != presence.cursor
                || sent.selection != presence.selection
                || sent.role != presence.role
                || sent.viewport.zoom != presence.viewport.zoom
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
        collab.sink.presence(presence);
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
                | Edit::ReplacePages { .. }
                | Edit::InsertPagesFrom { .. }
                // Bookmarks and named destinations change the document's
                // structure, not just a comment: a commenter may not touch them
                // (TASK §6), and the room refuses the operation anyway.
                | Edit::AddBookmark { .. }
                | Edit::AddBookmarkWith { .. }
                | Edit::RenameBookmark { .. }
                | Edit::UpdateBookmark { .. }
                | Edit::DeleteBookmark { .. }
                | Edit::MoveBookmark { .. }
                | Edit::SetBookmarkPage { .. }
                | Edit::SetBookmarkColor { .. }
                | Edit::SetBookmarkStyle { .. }
                | Edit::ImportBookmarks { .. }
                | Edit::BookmarksFromStructure
                | Edit::BookmarksFromTextStyles { .. }
                | Edit::SetNamedDestination { .. }
                | Edit::DeleteNamedDestination { .. }
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
            if !collab.sink.enqueue(op.clone()) {
                collab.error = Some("The collaboration session is closed".into());
                return Some(false);
            }
        }
        collab.history.push(HistoryGroup { label: edit.label().to_string(), originals: ops, inverses });
        collab.redo.clear();
        Some(true)
    }

    /// Whether the open session shares this document. The Split and Combine
    /// dialogs ask before they fall back to their single-user file workflow.
    pub(crate) fn collab_shares(&self, doc: DocId) -> bool {
        self.collab.as_ref().is_some_and(|collab| collab.doc == doc)
    }

    /// The document a session shares, if one is open. `None` without a
    /// session: the caller then keeps its single-user behaviour.
    pub(crate) fn collab_document(&self) -> Option<DocId> {
        self.collab.as_ref().map(|collab| collab.doc)
    }

    /// Take `count` pages out of the shared document, starting at the 0-based
    /// `from` (RFC §3.1.5). `None` when no session shares this document, so
    /// the caller runs its file workflow; `Some(sent)` otherwise. The part is
    /// cut by the server, so the intent carries the range alone and the undo
    /// is patched from the accepted operation (`inverse_after`).
    pub(crate) fn collab_split_pages(&mut self, doc: DocId, from: usize, count: usize) -> Option<bool> {
        self.collab_send_pages_op(doc, SPLIT_LABEL, |document, key, client_id, version, epoch, counter| {
            split_pages_op(document, from, count, key, client_id, version, epoch, counter)
        })
    }

    /// Join `parts` into the shared document at the 0-based `at` (RFC §3.1.5):
    /// Combine Files inside a room, as one confirmation. The parts are sent one
    /// at a time in the order given (a structural operation is accepted only at
    /// the head of the version), and the whole confirmation is one history group,
    /// so Undo takes it back in one step. `None` when no session shares this
    /// document, so the caller runs its file workflow.
    pub(crate) fn collab_combine_parts(&mut self, doc: DocId, at: usize, parts: &[std::sync::Arc<Vec<u8>>], files: usize) -> Option<bool> {
        let collab = self.collab.as_mut()?;
        if collab.doc != doc {
            return None;
        }
        if !collab.connected {
            collab.error = Some("Waiting for the collaboration session".into());
            return Some(false);
        }
        if !collab.role.can_structural() {
            collab.error = Some("This role cannot change pages".into());
            return Some(false);
        }
        if collab.combine.is_some() {
            collab.error = Some("The previous combine is still joining its parts".into());
            return Some(false);
        }
        let document = self.session.get(doc)?;
        let mut counter = collab.counter;
        let ops = {
            let refs: Vec<&[u8]> = parts.iter().map(|part| part.as_slice()).collect();
            let client_id = collab.client_id.clone();
            combine_parts_ops(document, at, &refs, collab.doc_key.clone(), &client_id, collab.version, collab.epoch, &mut counter)
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
        for op in &ops {
            let Some(id) = collab.next_op_id() else {
                collab.error = Some("Could not allocate an operation id".into());
                return Some(false);
            };
            inverses.push(self.session.get(doc).and_then(|document| inverse_for(document, op, id).ok().flatten()));
        }
        let mut queue: std::collections::VecDeque<Op> = ops.iter().cloned().collect();
        let Some(first) = queue.pop_front() else { return Some(false) };
        if !collab.sink.enqueue(first.clone()) {
            collab.error = Some("The collaboration session is closed".into());
            return Some(false);
        }
        let group_op = first.op_id.clone();
        collab.history.push(HistoryGroup { label: COMBINE_LABEL.to_string(), originals: ops, inverses });
        collab.redo.clear();
        collab.combine = Some(CombineBatch {
            pending: queue,
            in_flight: Some(first),
            joined: Vec::new(),
            files,
            group_op,
            rollback: false,
            after_snapshot: false,
        });
        Some(true)
    }

    /// Send one structural page operation the way [`Self::collab_apply_edit`]
    /// sends an edit: refused before anything is sent when the session is not
    /// ready or the role cannot change pages, and the inverse is computed here
    /// when it can be (a combine's split); a split's combine is patched from the
    /// accepted operation.
    fn collab_send_pages_op(
        &mut self,
        doc: DocId,
        label: &str,
        build: impl FnOnce(&Document, DocKey, &str, u64, u64, &mut u64) -> Result<Op, ProjectionError>,
    ) -> Option<bool> {
        let collab = self.collab.as_mut()?;
        if collab.doc != doc {
            return None;
        }
        if !collab.connected {
            collab.error = Some("Waiting for the collaboration session".into());
            return Some(false);
        }
        if !collab.role.can_structural() {
            collab.error = Some("This role cannot change pages".into());
            return Some(false);
        }
        let document = self.session.get(doc)?;
        let mut counter = collab.counter;
        let op = {
            let client_id = collab.client_id.clone();
            build(document, collab.doc_key.clone(), &client_id, collab.version, collab.epoch, &mut counter)
        };
        collab.counter = counter;
        let op = match op {
            Ok(op) => op,
            Err(e) => {
                collab.error = Some(e.to_string());
                return Some(false);
            }
        };
        let inverse = self.session.get(doc).and_then(|document| collab.next_op_id().and_then(|id| inverse_for(document, &op, id).ok().flatten()));
        if !collab.sink.enqueue(op.clone()) {
            collab.error = Some("The collaboration session is closed".into());
            return Some(false);
        }
        collab.history.push(HistoryGroup { label: label.to_string(), originals: vec![op], inverses: vec![inverse] });
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
        // One structural step at a time: a part of a confirmation (or of an
        // undo) changes the head under the next one, so nothing else may take
        // the history until it has answered.
        if collab.combine.is_some() || collab.sequence.is_some() {
            collab.status = "Wait for the previous change to finish".into();
            return true;
        }
        let group = if undo { collab.history.pop() } else { collab.redo.pop() };
        let Some(mut group) = group else {
            collab.status = if undo { "Nothing to undo".into() } else { "Nothing to redo".into() };
            return true;
        };
        let ops = if undo {
            // A group's inverses run in reverse: a confirmation's parts joined
            // in order, so the last one leaves first and the positions of the
            // parts before it are still the ones they joined at.
            let available: Vec<Op> = group.inverses.iter().flatten().rev().cloned().collect();
            if available.is_empty() {
                collab.status = format!("{} cannot be undone", group.label);
                collab.history.push(group);
                return true;
            }
            available
        } else {
            group.originals.clone()
        };
        // A structural group (a Combine confirmation's splits) cannot go out in
        // one burst: each is accepted only at the head the previous one left.
        // The rest wait in a sequence and are addressed when their turn comes.
        let mut queue: std::collections::VecDeque<Op> = std::collections::VecDeque::with_capacity(ops.len());
        for mut op in ops {
            let Some(id) = collab.next_op_id() else { break };
            op.op_id = id;
            queue.push_back(op);
        }
        let Some(mut first) = queue.pop_front() else { return true };
        readdress(&mut first, collab.version, collab.epoch);
        if !collab.sink.enqueue(first.clone()) {
            collab.error = Some("The collaboration session is closed".into());
            if undo {
                collab.history.push(group);
            } else {
                collab.redo.push(group);
            }
            return true;
        }
        let mut all = Vec::with_capacity(queue.len().saturating_add(1));
        all.push(first.clone());
        all.extend(queue.iter().cloned());
        group.originals = all;
        if undo {
            collab.redo.push(group);
        } else {
            collab.history.push(group);
        }
        collab.sequence = Some(OpSequence { pending: queue, in_flight: Some(first) });
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
            if let Some(result) = &collab.committed {
                ui.weak(result);
            }
        });
        if commit {
            self.collab_commit();
        }
    }
}

/// The document a session belongs to: the token's `fileId` claim, because the
/// server refuses a `hello` whose document is not the token's file. The host's
/// session id is the fallback for a token we cannot read — the server will then
/// refuse the session, which is the honest outcome, not a guessed room.
fn doc_key_from_token(token: &str, session_id: &str) -> Option<DocKey> {
    use base64::Engine as _;
    let file = token
        .split('.')
        .nth(1)
        .filter(|payload| !payload.is_empty())
        .and_then(|payload| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|decoded| serde_json::from_slice::<serde_json::Value>(&decoded).ok())
        .and_then(|claims| claims.get("fileId").and_then(|file| file.as_str()).filter(|file| !file.is_empty()).map(str::to_owned));
    file.and_then(|file| DocKey::new(file).ok()).or_else(|| DocKey::new(session_id).ok())
}

fn role_from_name(name: &str) -> Option<Role> {
    match name {
        "owner" => Some(Role::Owner),
        "editor" => Some(Role::Editor),
        "commenter" => Some(Role::Commenter),
        "viewer" => Some(Role::Viewer),
        _ => None,
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
        ErrorCode::Busy => "A save is in progress; try again",
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

#[cfg(test)]
mod tests {
    use super::{
        ActorId, BTreeMap, CollabUi, DocId, DocKey, Duration, Event, Instant, Op, PRESENCE_TTL, PageRef, ParticipantSelection, PdfCraftApp, Presence,
        RemoteSelectionKind, Role, Seen, Sink, doc_key_from_token, local_selection, rect_of_quads, role_from_name, role_name, text_selection,
        visible_selection,
    };
    use pdfcraft_collab_proto::{Selection, Target, Viewport};

    fn presence(selection: Option<Selection>) -> Result<Presence, Box<dyn std::error::Error>> {
        Ok(Presence {
            actor_id: ActorId::new("ada")?,
            role: Role::Editor,
            page: PageRef { index: 1, epoch: 2 },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: Some([5, 6]),
            selection,
            color: "#3366cc".into(),
        })
    }

    /// A selection is presence: it is drawn while the sender's presence is fresh, and it
    /// disappears with the cursor's TTL (ADR-0006). Nothing here touches a document.
    #[test]
    fn a_remote_selection_expires_with_its_presence() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let selection = Selection::Text { page: PageRef { index: 1, epoch: 2 }, rect: [10, 20, 30, 40], quads: Vec::new(), caret: Some([30, 40]) };
        let fresh = Seen { presence: presence(Some(selection.clone()))?, at: now };
        let drawn = visible_selection(&fresh, now, 2).expect("a fresh selection is drawn");
        assert_eq!(drawn.page, 1);
        assert_eq!(drawn.kind, RemoteSelectionKind::Rects(vec![[10.0, 20.0, 30.0, 40.0]]));
        assert_eq!(drawn.caret, Some([30.0, 40.0]), "the caret travels with the range");
        assert_eq!(drawn.actor, "ada");
        assert!(visible_selection(&fresh, now, 3).is_none(), "a selection from another page tree is not drawn");
        let stale_at = now.checked_sub(PRESENCE_TTL + Duration::from_millis(1)).expect("an instant before now");
        let stale = Seen { presence: presence(Some(selection))?, at: stale_at };
        assert!(visible_selection(&stale, now, 2).is_none(), "a stale selection disappears with the cursor");
        Ok(())
    }

    /// The page's own epoch is checked too, not only the one in the frame's `page`: a
    /// range addressed to a page set we have left is not drawn even when the sender's
    /// frame page was current.
    #[test]
    fn a_text_selection_from_another_epoch_is_not_drawn() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let selection = Selection::Text { page: PageRef { index: 1, epoch: 5 }, rect: [0, 0, 1, 1], quads: Vec::new(), caret: None };
        let seen = Seen { presence: presence(Some(selection))?, at: now };
        assert!(visible_selection(&seen, now, 2).is_none(), "the range's own epoch is stale");
        Ok(())
    }

    #[test]
    fn a_selected_comment_travels_by_its_name() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let seen = Seen { presence: presence(Some(Selection::Annotation { nm: "ada-hl-1".into(), also: Vec::new() }))?, at: now };
        let drawn = visible_selection(&seen, now, 2).expect("a fresh selection is drawn");
        assert_eq!(drawn.page, 1);
        assert_eq!(drawn.kind, RemoteSelectionKind::Annotation("ada-hl-1".into()));
        assert_eq!(drawn.caret, None);
        Ok(())
    }

    /// Several comments selected together are drawn as one selection that names them
    /// all, so the canvas looks up each `/NM`'s geometry.
    #[test]
    fn several_selected_comments_travel_as_a_list() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let selection = Selection::Annotation { nm: "ada-hl-1".into(), also: vec!["ada-hl-2".into(), "bo-note".into()] };
        let seen = Seen { presence: presence(Some(selection))?, at: now };
        let drawn = visible_selection(&seen, now, 2).expect("a fresh selection is drawn");
        assert_eq!(drawn.kind, RemoteSelectionKind::Annotations(vec!["ada-hl-1".into(), "ada-hl-2".into(), "bo-note".into()]));
        Ok(())
    }

    /// A text range is drawn as one rectangle per line piece: the box between lines no
    /// longer covers text the sender did not select.
    #[test]
    fn a_text_range_travels_as_one_rect_per_line() -> Result<(), Box<dyn std::error::Error>> {
        let now = Instant::now();
        let selection = Selection::Text {
            page: PageRef { index: 1, epoch: 2 },
            rect: [10, 70, 50, 100],
            quads: vec![[10, 90, 50, 100], [10, 70, 30, 80]],
            caret: Some([30, 70]),
        };
        let seen = Seen { presence: presence(Some(selection))?, at: now };
        let drawn = visible_selection(&seen, now, 2).expect("a fresh selection is drawn");
        assert_eq!(drawn.kind, RemoteSelectionKind::Rects(vec![[10.0, 90.0, 50.0, 100.0], [10.0, 70.0, 30.0, 80.0]]));
        assert_eq!(drawn.caret, Some([30.0, 70.0]));
        Ok(())
    }

    /// A text range becomes one bounding rectangle in user space (what a peer that
    /// predates per-line geometry draws); one page's column select is the same shape.
    #[test]
    fn a_text_range_travels_as_its_bounding_rect() {
        let quads = [[10.0, 100.0, 50.0, 100.0, 10.0, 90.0, 50.0, 90.0], [10.0, 80.0, 30.0, 80.0, 10.0, 70.0, 30.0, 70.0]];
        assert_eq!(rect_of_quads(&quads), Some([10, 70, 50, 100]));
        assert_eq!(rect_of_quads(&[]), None);
        assert_eq!(rect_of_quads(&[[f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]]), Some([0, 0, 0, 0]));
    }

    /// The wire form of a range: the bounding box, one rectangle per line piece, and
    /// the caret; a range with more pieces than a frame holds keeps the box alone.
    #[test]
    fn a_text_selection_is_sent_as_quads_and_a_caret() {
        let quads = [[10.0, 100.0, 50.0, 100.0, 10.0, 90.0, 50.0, 90.0], [10.0, 80.0, 30.0, 80.0, 10.0, 70.0, 30.0, 70.0]];
        let sent = text_selection(quads.to_vec(), PageRef { index: 1, epoch: 2 }, Some([30.5, 69.4]));
        assert_eq!(
            sent,
            Selection::Text {
                page: PageRef { index: 1, epoch: 2 },
                rect: [10, 70, 50, 100],
                quads: vec![[10, 90, 50, 100], [10, 70, 30, 80]],
                caret: Some([31, 69]),
            }
        );
        let many = vec![quads[0]; pdfcraft_collab_proto::MAX_SELECTION_QUADS + 1];
        let sent = text_selection(many, PageRef { index: 0, epoch: 0 }, None);
        match sent {
            Selection::Text { quads, rect, .. } => {
                assert!(quads.is_empty(), "the box alone instead of a frame over the limit");
                assert_eq!(rect, [10, 90, 50, 100]);
            }
            other => panic!("a text selection, got {other:?}"),
        }
    }

    /// The session state the host page receives carries who is on which page and what
    /// they have selected — names and geometry only. A stale participant claims neither.
    #[test]
    fn the_host_report_carries_a_participants_selection() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, fixture(2)).expect("the fixture opens");
        let id = app.active_ids().map(|(_, id)| id).expect("a document");
        session_on(&mut app, id);
        let now = Instant::now();
        {
            let collab = app.collab.as_mut().expect("a session");
            collab.epoch = 2;
            let selection = Selection::Annotation { nm: "bo-1".into(), also: vec!["bo-2".into()] };
            collab.participants.insert("bo".into(), Seen { presence: presence(Some(selection))?, at: now });
        }
        let state = app.collab_state().expect("a session");
        let bo = state.participants.first().expect("the participant is reported");
        assert_eq!(bo.client_id, "bo");
        assert_eq!(bo.page, Some(2), "the frame's 0-based page is reported 1-based");
        assert!(bo.active);
        assert_eq!(bo.selection, Some(ParticipantSelection::Annotations(vec!["bo-1".into(), "bo-2".into()])));

        // A text range reports its per-line rectangles and the caret, on the range's page.
        let selection = Selection::Text {
            page: PageRef { index: 1, epoch: 2 },
            rect: [10, 20, 30, 40],
            quads: vec![[10, 30, 30, 40], [10, 20, 20, 30]],
            caret: Some([20, 20]),
        };
        {
            let collab = app.collab.as_mut().expect("a session");
            collab.participants.get_mut("bo").expect("the participant").presence.selection = Some(selection);
        }
        let state = app.collab_state().expect("a session");
        assert_eq!(
            state.participants[0].selection,
            Some(ParticipantSelection::Text { page: 2, rects: vec![[10, 30, 30, 40], [10, 20, 20, 30]], caret: Some([20, 20]) })
        );

        // Past the TTL the participant is inactive and claims no page or selection.
        {
            let collab = app.collab.as_mut().expect("a session");
            let stale = now.checked_sub(PRESENCE_TTL + Duration::from_millis(1)).expect("an instant before now");
            collab.participants.get_mut("bo").expect("the participant").at = stale;
        }
        let state = app.collab_state().expect("a session");
        let bo = &state.participants[0];
        assert!(!bo.active);
        assert_eq!(bo.page, None);
        assert_eq!(bo.selection, None, "a stale selection is not reported");
        Ok(())
    }

    /// What the app puts in a frame: several selected comments become one list with the
    /// primary first, and a text selection becomes its per-line rectangles and a caret.
    #[test]
    fn the_local_selection_lists_comments_and_per_line_quads() {
        use crate::canvas::{DocView, ViewDefaults};
        use pdfcraft_render::{DocInfo, PageInfo, PageText, TextGlyph};
        use std::sync::Arc;
        let info = DocInfo {
            pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
            annotations: vec![comment(0, 0, "ada-1"), comment(0, 1, "ada-2"), comment(0, 2, "ada-3")],
            ..Default::default()
        };
        let mut view = DocView::new(DocId(1), &info, ViewDefaults::default());
        view.current = 0;
        // Two comments on the page in view travel as a list, the one the handles act on first.
        view.comments.selected = Some((0, 1));
        view.comments.also = vec![(0, 2)];
        assert_eq!(
            local_selection(&view, &info, 0),
            Some(Selection::Annotation { nm: "ada-2".into(), also: vec!["ada-3".into()] }),
            "the selected comments, not their text"
        );

        // A comment on another page falls back to its rectangle; an epoch is this replica's.
        view.comments.selected = Some((0, 0));
        view.comments.also.clear();
        view.current = 1;
        let selected = local_selection(&view, &info, 7).expect("the off-page comment still says where it is");
        assert!(matches!(selected, Selection::Text { page: PageRef { index: 0, epoch: 7 }, .. }), "got {selected:?}");

        // A text selection: one rectangle per line piece, and the caret at the active end.
        view.comments.selected = None;
        view.current = 0;
        let glyph = |x: f32, y: f32| TextGlyph { text: "x".into(), rect: [x, y, x + 10.0, y + 10.0], quad: [[x, y]; 4], direction: [1.0, 0.0] };
        view.texts.insert(
            0,
            Arc::new(PageText {
                glyphs: vec![glyph(10.0, 10.0), glyph(20.0, 10.0), glyph(10.0, 30.0)],
                line_of: vec![0, 0, 1],
                space_before: vec![false; 3],
                truncated: false,
            }),
        );
        view.select_text(0, 0, 2);
        let sent = local_selection(&view, &info, 0).expect("a text selection is sent");
        match sent {
            Selection::Text { quads, caret, .. } => {
                assert_eq!(quads.len(), 2, "one rectangle per line: {quads:?}");
                assert!(caret.is_some(), "the caret travels with the range");
            }
            other => panic!("a text selection, got {other:?}"),
        }
    }

    /// A comment on `page` with `/NM` `name` and an index in `/Annots`, for the local
    /// selection tests above.
    fn comment(page: usize, index: usize, name: &str) -> pdfcraft_render::Annotation {
        pdfcraft_render::Annotation {
            page,
            subtype: "Text".into(),
            author: Some("ada".into()),
            contents: Some("a private note".into()),
            modified: None,
            name: Some(name.into()),
            in_reply_to: None,
            parent: None,
            grouped: false,
            rect: [10.0, 10.0, 20.0, 20.0],
            color: None,
            index,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: None,
            fill_sign: false,
            no_rotate: false,
            points: Vec::new(),
            text_box: None,
            popup: None,
        }
    }

    /// A token is read for its `fileId` only: the server still verifies it, and
    /// the document a session joins must be the file the token names.
    #[test]
    fn the_document_comes_from_the_tokens_file_claim() {
        // {"fileId":"file-123","sub":"ada"}
        let payload = "eyJmaWxlSWQiOiJmaWxlLTEyMyIsInN1YiI6ImFkYSJ9";
        let token = format!("header.{payload}.signature");
        let key = doc_key_from_token(&token, "session-9").expect("a document");
        assert_eq!(key.as_str(), "file-123");
    }

    #[test]
    fn an_unreadable_token_falls_back_to_the_session_id() {
        assert_eq!(doc_key_from_token("not-a-token", "file-123").map(|key| key.as_str().to_owned()), Some("file-123".to_owned()));
        assert!(doc_key_from_token("", "").is_none());
    }

    #[test]
    fn a_role_name_round_trips() {
        for role in [Role::Owner, Role::Editor, Role::Commenter, Role::Viewer] {
            assert_eq!(role_from_name(role_name(role)), Some(role));
        }
        assert_eq!(role_from_name("chair"), None);
    }

    /// An `n`-page document; page `i` shows "Page i+1".
    fn fixture(n: usize) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        for i in 0..n {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
            let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
            objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// What the UI hands a session: the operations it would have sent, kept for
    /// the assertions. No socket, no server.
    struct RecordedSink {
        ops: std::sync::Arc<std::sync::Mutex<Vec<Op>>>,
    }

    impl Sink for RecordedSink {
        fn enqueue(&mut self, op: Op) -> bool {
            self.ops.lock().map(|mut ops| ops.push(op)).is_ok()
        }
        fn presence(&mut self, _presence: Presence) -> bool {
            true
        }
        fn commit(&mut self, _message: Option<String>) -> bool {
            true
        }
        fn resync(&mut self) -> bool {
            true
        }
        fn try_event(&mut self) -> Option<Event> {
            None
        }
        fn close(&mut self) {}
    }

    /// Put an editor's session on `doc` and return the operations it sends.
    fn session_on(app: &mut PdfCraftApp, doc: DocId) -> std::sync::Arc<std::sync::Mutex<Vec<Op>>> {
        let ops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let now = Instant::now();
        app.collab = Some(CollabUi {
            sink: Box::new(RecordedSink { ops: ops.clone() }),
            doc,
            doc_key: DocKey::new("file-1").expect("a document key"),
            actor: Some(ActorId::new("ada").expect("an actor")),
            client_id: "ada-1".into(),
            role: Role::Editor,
            connected: true,
            version: 0,
            epoch: 0,
            participants: BTreeMap::new(),
            color: "#3366cc".into(),
            status: String::new(),
            error: None,
            history: Vec::new(),
            redo: Vec::new(),
            combine: None,
            sequence: None,
            counter: 0,
            last_presence: now,
            sent_presence: None,
            committed: None,
            reported: None,
            reported_presence: None,
            reported_at: now,
        });
        ops
    }

    /// The Split dialog's action in a session is one `page.split`; the accepted
    /// operation removes the range and the undo joins the part back. This is the
    /// dialog's path (not the file export), and the history is what Undo uses.
    #[test]
    fn a_shared_split_sends_one_operation_and_undo_joins_the_part() {
        use base64::Engine as _;
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, fixture(4)).expect("the fixture opens");
        let id = app.active_ids().map(|(_, id)| id).expect("a document");
        let ops = session_on(&mut app, id);

        assert_eq!(app.collab_split_pages(id, 1, 3), Some(true), "a shared split is one operation");
        let sent = ops.lock().expect("the queue").clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].payload["action"], "page.split");
        assert_eq!(sent[0].payload["count"], 3);
        assert_eq!(sent[0].target, Target::Page(PageRef { index: 1, epoch: 0 }), "the first page of the part, in the epoch it was in");
        let inverses = &app.collab.as_ref().expect("a session").history[0].inverses;
        assert!(inverses[0].is_none(), "the part is cut by the server, so the undo waits for the accepted operation");

        // The room cuts the part from its own document and accepts the operation.
        let part = fixture(3);
        let mut accepted = sent[0].clone();
        accepted.payload["from"] = serde_json::json!(1);
        accepted.payload["to"] = serde_json::json!(4);
        accepted.payload["count"] = serde_json::json!(3);
        accepted.payload["kept"] = serde_json::json!(1);
        accepted.payload["bytes"] = serde_json::json!(base64::engine::general_purpose::STANDARD.encode(&part));
        let ctx = egui::Context::default();
        let applied = pdfcraft_collab_proto::Applied {
            op_id: accepted.op_id.clone(),
            actor_id: ActorId::new("ada").expect("an actor"),
            version: 1,
            epoch: 1,
            op: accepted,
            result: serde_json::json!({"part": {"sha256": "0", "len": part.len()}}),
        };
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 1, "the range left the replica");
        // The part is not lost: the accepted operation carries it, and it opens
        // as a document of its own in the background (the shared document keeps
        // the focus, so Undo still belongs to the session).
        let part = app.views.iter().map(|view| view.id).find(|other| *other != id).expect("the part opened in a tab");
        let part_doc = app.session.get(part).expect("the part document");
        assert_eq!(part_doc.info.pages.len(), 3, "the pages taken out are all in the part");
        assert_eq!(part_doc.name, "t (pages 2-4).pdf");
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(id), "the shared document keeps the focus");
        let inverse = app.collab.as_ref().expect("a session").history[0].inverses[0].clone().expect("the accepted split carries its part");
        assert_eq!(inverse.payload["action"], "page.combine", "the undo is the operation that joins the part back");
        assert_eq!(inverse.payload["count"], 3);
        assert_eq!(inverse.target, Target::Page(PageRef { index: 1, epoch: 1 }), "the part goes back where it left, in the epoch the split left");

        // Undo sends that combine; the accepted join restores the page set.
        ops.lock().expect("the queue").clear();
        assert!(app.collab_history_step(true), "the session owns the undo");
        let joined = ops.lock().expect("the queue").clone();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].payload["action"], "page.combine");
        let mut accepted = joined[0].clone();
        accepted.payload["at"] = serde_json::json!(1);
        accepted.payload["count"] = serde_json::json!(3);
        let applied = pdfcraft_collab_proto::Applied {
            op_id: accepted.op_id.clone(),
            actor_id: ActorId::new("ada").expect("an actor"),
            version: 2,
            epoch: 2,
            op: accepted,
            result: serde_json::json!({"at": 1, "count": 3, "pages": 4}),
        };
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 4, "Undo brings the pages back");
    }

    /// A split with no second part is refused before it is sent, and a role
    /// without page access is refused with a message rather than a file write.
    #[test]
    fn a_shared_split_stops_before_the_server() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, fixture(4)).expect("the fixture opens");
        let id = app.active_ids().map(|(_, id)| id).expect("a document");
        let ops = session_on(&mut app, id);

        // The projection refuses a range the room would refuse, and nothing is queued.
        assert_eq!(app.collab_split_pages(id, 0, 4), Some(false), "a split that takes everything is refused");
        assert!(ops.lock().expect("the queue").is_empty());
        assert!(app.collab.as_ref().expect("a session").error.is_some());

        // Another document is not this session's: the caller keeps its file workflow.
        let other = app.session.open_new("other.pdf", fixture(2).into()).expect("a second document");
        assert_eq!(app.collab_split_pages(other, 0, 1), None);
        assert!(ops.lock().expect("the queue").is_empty());
    }

    /// The room's accepted form of an operation: the intent with what the
    /// server normalised (a combine's `at`, a split's `from`/`to`/`bytes`).
    fn accepted(op: &Op, version: u64, epoch: u64, extra: serde_json::Value) -> pdfcraft_collab_proto::Applied {
        let mut op = op.clone();
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                op.payload[key] = value.clone();
            }
        }
        pdfcraft_collab_proto::Applied {
            op_id: op.op_id.clone(),
            actor_id: ActorId::new("ada").expect("an actor"),
            version,
            epoch,
            op,
            result: serde_json::json!({}),
        }
    }

    /// Combine Files in a room sends the parts one at a time in the list's
    /// order (a structural operation is accepted only at the head), the whole
    /// confirmation is one history group, and Undo takes the parts out in
    /// reverse order so every position stays the one it joined at.
    #[test]
    fn a_shared_combine_joins_its_parts_one_at_a_time_and_undo_takes_them_back() {
        use std::sync::Arc;
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, fixture(4)).expect("the fixture opens");
        let id = app.active_ids().map(|(_, id)| id).expect("a document");
        let ops = session_on(&mut app, id);
        let parts = [Arc::new(fixture(2)), Arc::new(fixture(1))];

        assert_eq!(app.collab_combine_parts(id, 4, &parts, 2), Some(true), "the confirmation is accepted");
        let sent = ops.lock().expect("the queue").clone();
        assert_eq!(sent.len(), 1, "only the first part goes out; the next waits for its answer");
        assert_eq!(sent[0].payload["action"], "page.combine");
        assert_eq!(sent[0].payload["count"], 2);
        assert_eq!(sent[0].target, Target::Page(PageRef { index: 4, epoch: 0 }));
        assert_eq!(app.collab.as_ref().expect("a session").history.len(), 1, "the confirmation is one undo step");

        let ctx = egui::Context::default();
        let applied = accepted(&sent[0], 1, 1, serde_json::json!({"at": 4, "count": 2}));
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 6, "the first part joined");
        let sent = ops.lock().expect("the queue").clone();
        assert_eq!(sent.len(), 2, "the second part follows on the answer");
        assert_eq!(sent[1].target, Target::Page(PageRef { index: 6, epoch: 1 }), "where the first part ends, on the head it left");

        let applied = accepted(&sent[1], 2, 2, serde_json::json!({"at": 6, "count": 1}));
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 7, "both parts joined");
        assert!(app.collab.as_ref().expect("a session").combine.is_none(), "the confirmation is done");

        // Undo sends the splits one at a time — a structural operation is
        // accepted only at the head the previous one left — in reverse order,
        // so every position stays the one the part joined at.
        ops.lock().expect("the queue").clear();
        assert!(app.collab_history_step(true), "the session owns the undo");
        let undo = ops.lock().expect("the queue").clone();
        assert_eq!(undo.len(), 1, "one split goes out; the next waits for its answer");
        assert_eq!(undo[0].target, Target::Page(PageRef { index: 6, epoch: 2 }), "the last part leaves first");
        assert_eq!(undo[0].payload["count"], 1);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 7, "nothing is applied before the room accepts it");

        let applied = accepted(&undo[0], 3, 3, serde_json::json!({"from": 6, "to": 7}));
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 6, "the last part left");
        let undo = ops.lock().expect("the queue").clone();
        assert_eq!(undo.len(), 2, "then the split of the first part");
        assert_eq!(undo[1].target, Target::Page(PageRef { index: 4, epoch: 3 }), "at the index it joined, on the head the last split left");
        assert_eq!(undo[1].payload["count"], 2);

        let applied = accepted(&undo[1], 4, 4, serde_json::json!({"from": 4, "to": 6}));
        app.collab_event(Event::Applied { applied, own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 4, "the confirmation is fully undone");
        assert!(app.collab.as_ref().expect("a session").sequence.is_none(), "the undo sequence is done");
    }

    /// A part refused `stale` (the document moved under the confirmation) takes
    /// the parts already joined out again after the snapshot the retry asks
    /// for, so the confirmation is all or nothing, and the list can be sent
    /// again on the new head.
    #[test]
    fn a_stale_part_rolls_the_shared_combine_back() {
        use base64::Engine as _;
        use std::sync::Arc;
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, fixture(4)).expect("the fixture opens");
        let id = app.active_ids().map(|(_, id)| id).expect("a document");
        let ops = session_on(&mut app, id);
        let parts = [Arc::new(fixture(2)), Arc::new(fixture(1))];

        assert_eq!(app.collab_combine_parts(id, 4, &parts, 2), Some(true));
        let first = ops.lock().expect("the queue")[0].clone();
        let ctx = egui::Context::default();
        app.collab_event(Event::Applied { applied: accepted(&first, 1, 1, serde_json::json!({"at": 4, "count": 2})), own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 6);
        let second = ops.lock().expect("the queue")[1].clone();

        // The room answers the second part with `stale` (retryable); the client
        // asks for a snapshot, and the confirmation waits for it.
        app.collab_event(Event::Retry { op_id: second.op_id.as_str().to_owned() }, &ctx);
        assert!(app.collab.as_ref().expect("a session").error.is_some(), "the refusal is reported");
        assert!(app.collab.as_ref().expect("a session").combine_is_rolling_back());

        // The snapshot moves the head on; now the split that takes the joined
        // part out can be built on it.
        ops.lock().expect("the queue").clear();
        app.collab_event(Event::Snapshot { version: 2, epoch: 2, base: None, tail: Vec::new() }, &ctx);
        let rollback = ops.lock().expect("the queue").clone();
        assert_eq!(rollback.len(), 1, "one split for the part that joined");
        assert_eq!(rollback[0].payload["action"], "page.split");
        assert_eq!(rollback[0].payload["count"], 2);
        assert_eq!(rollback[0].target, Target::Page(PageRef { index: 4, epoch: 2 }), "where the part joined, on the new head");

        // The room accepts the split: the document is back at four pages and the
        // confirmation has nothing left to do.
        let mut full = rollback[0].clone();
        full.payload["from"] = serde_json::json!(4);
        full.payload["to"] = serde_json::json!(6);
        full.payload["bytes"] = serde_json::json!(base64::engine::general_purpose::STANDARD.encode(fixture(2)));
        app.collab_event(Event::Applied { applied: accepted(&full, 3, 3, serde_json::json!({})), own: true }, &ctx);
        assert_eq!(app.session.get(id).expect("the document").info.pages.len(), 4, "the joined part is taken out again");
        assert!(app.collab.as_ref().expect("a session").combine.is_none(), "the rollback is done");
        assert!(app.collab.as_ref().expect("a session").history.is_empty(), "a rolled-back confirmation is not undoable");
    }
}
