//! The GridCraft engine façade.
//!
//! Every user-visible action is a command with a stable id (`cell.set`, `home.bold`,
//! `data.sortAscending`…) and JSON parameters. The egui UI, the CLI, the control channel and MCP
//! all go through [`Session::execute`], so every action is scriptable and journaled.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod catalog;
pub mod cf;
pub mod chart_data;
mod chart_svg;
mod clip_html;
pub mod cmd;
pub mod display;
pub mod fill;
pub mod io;
pub mod links;
pub mod macros;
pub mod outline;
pub mod paper;
pub mod pivot;
pub mod pivot_calc;
pub mod recent;
pub mod record;
pub mod sample;
pub mod selection;
pub mod tables;
pub mod windows;

use std::sync::Arc;

use gridcraft_calc::Calc;
use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::Workbook;
use serde::Serialize;
use serde_json::Value as Json;

pub use cmd::{CommandInfo, CommandSpec, command_specs, find_command, register_test_command};
pub use gridcraft_calc as calc;
pub use gridcraft_core as core;
pub use gridcraft_formula as formula;
pub use gridcraft_math as math;
pub use gridcraft_model as model;
pub use gridcraft_numfmt as numfmt;
pub use selection::Selection;
pub use windows::{Target, TargetGuard, WindowState, WindowView};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no workbook open")]
    NoDocument,
    #[error("{0}")]
    Other(String),
    /// The command would replace something; run it again with `overwrite: true` to go ahead.
    #[error("confirm: {0} (run again with `overwrite: true`)")]
    Confirm(String),
    /// Active content (a macro) asks to run; run it again with `approve: true` to allow it.
    #[error("approve: {0} (run again with `approve: true`)")]
    Approve(String),
    #[error("internal error in `{0}` (the workbook was kept as it was): {1}")]
    Internal(String, String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    pub wb: Arc<Workbook>,
    pub selection: Selection,
    /// Estimate of storage this snapshot retains beyond the next workbook version.
    pub retained_bytes: usize,
}

/// An undo group opened with [`Session::group_begin`]. Hand it back to [`Session::group_end`]
/// (keep the changes as one undo step) or [`Session::group_rollback`] (undo them all).
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an undo group stays open until it is ended or rolled back"]
pub struct GroupToken {
    id: u64,
    doc: u64,
}

impl GroupToken {
    /// The uid of the document the group belongs to.
    pub fn doc(&self) -> u64 {
        self.doc
    }
}

/// An open undo group: what runs on `doc` until it ends becomes one undo step.
#[derive(Clone, Debug)]
struct OpenGroup {
    id: u64,
    doc: u64,
    label: String,
    /// Undo depth when the group began; the group's own steps are cut back to it.
    start: usize,
    wb: Arc<Workbook>,
    selection: Selection,
    /// The redo stack when the group began (a rollback puts it back).
    redo: Vec<HistoryEntry>,
    /// Where the recording was when the group began (`None` when nothing was recording): a
    /// rollback takes back the steps recorded since, as it takes back their edits.
    recorded: Option<(u64, u64)>,
}

/// Where the last [`Session::group_end`] or [`Session::group_rollback`] left its document: the
/// workbook and selection it ended at, the undo length then, and whether it made a step. A
/// command that ended the group tells its own later changes from the group's step by it, and
/// keeps the group's name only when the group's step is exactly its own (the entry holds the
/// workbook the step started from). Dropped when the outermost command finishes, so it keeps no
/// workbook alive between commands.
#[derive(Debug)]
struct GroupStep {
    doc: u64,
    after: Arc<Workbook>,
    selection: Selection,
    len: usize,
    stepped: bool,
}

/// The clipboard: a block of cells (with formulas and styles) or marching-ants reference.
#[derive(Clone, Debug)]
pub struct Clipboard {
    /// Source workbook snapshot, sheet and range.
    pub wb: Arc<Workbook>,
    pub sheet: usize,
    pub range: RangeRef,
    /// The range as copied, before whole rows/columns were trimmed to the used range
    /// (Insert Copied Cells inserts whole lines for a row or column copy).
    pub selected: RangeRef,
    pub cut: bool,
    /// Document uid of the source (a cut only moves within the same workbook).
    pub doc_uid: u64,
    /// Plain text (tab-separated) for the system clipboard.
    pub text: String,
    /// A multi-area copy: the areas as selected (for the marquee). `wb` then holds them slid
    /// together into one block at `range`. Empty for a single-range copy.
    pub areas: Vec<RangeRef>,
    /// Trust of the source document: pasting content from a file someone else made carries it.
    pub trust: Trust,
}

/// Whether a document's active content (macros) may run without asking the person first. There
/// is no default: whoever makes a document says where it came from ([`DocState::new`]), so a new
/// way of building one from bytes can't come out trusted by omission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Trust {
    /// Made in this session: a new workbook or a sample.
    Local,
    /// Opened from a file, or holding content pasted from one: active content asks before it
    /// runs.
    FromFile,
}

impl Trust {
    /// Whether active content may run without asking.
    pub fn allows_code(self) -> bool {
        self == Trust::Local
    }

    /// The trust of a document after it receives content from a document with trust `source`:
    /// content from an untrusted file leaves the receiver untrusted.
    pub fn receive(self, source: Trust) -> Trust {
        if source == Trust::FromFile { Trust::FromFile } else { self }
    }
}

/// In-cell edit mode as shown in the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Mode {
    #[default]
    Ready,
    Enter,
    Edit,
    Point,
}

#[derive(Clone, Debug)]
pub struct DocState {
    pub wb: Arc<Workbook>,
    pub calc: Calc,
    pub selection: Selection,
    pub undo: Vec<HistoryEntry>,
    pub redo: Vec<HistoryEntry>,
    pub path: Option<String>,
    pub saved: Arc<Workbook>,
    pub revision: u64,
    /// Identifies this open workbook for the rest of the session: it never changes while the
    /// workbook is open (saving, renaming, reordering) and is never reused for another one.
    pub uid: u64,
    pub title: String,
    /// Per-sheet selection memory.
    pub sheet_selections: Vec<Selection>,
    /// Allow Edit Ranges unlocked with their password in this session: (sheet name, title).
    pub unlocked_ranges: Vec<(String, String)>,
    /// The format `path`'s content was recognised as when its extension doesn't name one (an
    /// XLSX called `budget.dat`): Save writes it back in that format.
    pub content_kind: Option<io::FileKind>,
    /// Whether the scripts stored in this workbook may run (`automate.runScript`). A workbook
    /// that arrives already carrying scripts (opened from a file) starts untrusted until the
    /// user trusts it in this session (`automate.trustScripts`); one with no scripts starts
    /// trusted, since any script it gains later was written here.
    pub scripts_trusted: bool,
    /// Where the workbook's content came from: whether its active content may run without
    /// asking. Unlike `scripts_trusted`, this is provenance, not consent: it is set when the
    /// workbook is opened from a file, carried by content pasted from such a workbook, and never
    /// changed by a trust decision.
    pub trust: Trust,
    /// While a macro's footprint is measured: every cell a command wrote, by sheet name, even
    /// where it wrote what was already there.
    pub touched: Option<Vec<(String, CellRef)>>,
    /// While a macro step runs: the structural edits its command made (the sheet's name, the
    /// edit), so the macro's inputs, output and Repeat ranges follow their cells as formulas do.
    pub edits: Option<Vec<(String, gridcraft_formula::adjust::Edit)>>,
    /// The references `edit.goTo` with `record: true` went to in this document, newest first,
    /// without names, each area written with its sheet (`Sheet1!B14:C22,Sheet1!F19:G30`).
    pub go_to_recent: Vec<String>,
    /// The Distinct Count PivotTable value fields (sheet, pivot, value) an .xlsx save has already
    /// told the user about (#3336), so only a field not yet disclosed raises the notice again.
    pub distinct_disclosed: Vec<(usize, usize, usize)>,
}

static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const UNDO_LIMIT: usize = 100;
/// Past this many entries the journal drops its oldest half.
const JOURNAL_MAX_ENTRIES: usize = 10_000;
/// Past this many bytes (entries serialized as JSON) the journal drops its oldest entries until
/// it is under half. Entries keep their full params so they replay (recorded scripts,
/// `engine.journal`), and a `file.open` holds the whole file as base64: without a size cap a
/// few opens of a large workbook keep tens of MB alive.
pub(crate) const JOURNAL_MAX_BYTES: usize = 64 * 1024 * 1024;
const UNDO_BYTES_LIMIT: usize = 128 * 1024 * 1024;

/// Comment and hyperlink text of a sheet: a changed sheet's snapshot keeps its own copy of these
/// strings (an edit deep-copies them), so they count against the history budget with the cells.
fn sheet_text_bytes(sheet: &gridcraft_model::Sheet) -> usize {
    let comments = sheet.comments.values().fold(0usize, |sum, c| {
        // The vectors' own storage counts too: an empty reply still takes an element.
        let replies = c.replies.iter().fold(0usize, |n, (a, t)| n.saturating_add(a.len()).saturating_add(t.len()));
        let replies = replies.saturating_add(c.replies.capacity().saturating_mul(std::mem::size_of::<(String, String)>()));
        let dates = c.dates.iter().fold(0usize, |n, d| n.saturating_add(d.len()));
        let dates = dates.saturating_add(c.dates.capacity().saturating_mul(std::mem::size_of::<String>()));
        sum.saturating_add(std::mem::size_of::<gridcraft_model::Comment>() + c.author.len() + c.text.len() + replies + dates)
    });
    sheet.hyperlinks.values().fold(comments, |sum, h| {
        sum.saturating_add(std::mem::size_of::<gridcraft_model::Hyperlink>() + h.target.len() + h.tooltip.as_ref().map_or(0, String::len))
    })
}

fn history_retained_bytes(older: &Workbook, newer: &Workbook) -> usize {
    let mut bytes = std::mem::size_of::<Workbook>();
    for (index, sheet) in older.sheets.iter().enumerate() {
        // Moving or inserting sheets must not charge unchanged sheets at a new index.
        if newer.sheets.iter().any(|next| Arc::ptr_eq(sheet, next)) {
            continue;
        }
        bytes = bytes.saturating_add(std::mem::size_of::<gridcraft_model::Sheet>());
        let empty = gridcraft_model::CellStore::new();
        let next = newer.sheets.get(index).map_or(&empty, |next| &next.cells);
        bytes = bytes.saturating_add(sheet.cells.retained_bytes_against(next));
        bytes = bytes.saturating_add(sheet_text_bytes(sheet));
    }
    // Snapshots share the macro list until a change to it; a snapshot with its own list keeps it.
    if !older.macros.shares(&newer.macros) {
        bytes = bytes.saturating_add(older.macros.estimated_bytes());
    }
    bytes
}

fn push_history(history: &mut Vec<HistoryEntry>, label: String, wb: Arc<Workbook>, next: &Workbook, selection: Selection) {
    push_history_with_budget(history, label, wb, next, selection, UNDO_BYTES_LIMIT);
}

fn push_history_with_budget(
    history: &mut Vec<HistoryEntry>,
    label: String,
    wb: Arc<Workbook>,
    next: &Workbook,
    selection: Selection,
    byte_budget: usize,
) {
    push_entry(history, label, wb, next, selection);
    trim_to_budget(history, byte_budget);
}

/// Adds a step without applying the limits: inside a command they wait for the outermost
/// command to collapse its steps (see [`Session::trim_history`]).
fn push_entry(history: &mut Vec<HistoryEntry>, label: String, wb: Arc<Workbook>, next: &Workbook, selection: Selection) {
    let retained_bytes = history_retained_bytes(&wb, next);
    history.push(HistoryEntry { label, wb, selection, retained_bytes });
}

/// Drops the oldest steps past [`UNDO_LIMIT`] or `byte_budget`, always keeping the latest one
/// (even if that one edit exceeds the budget).
fn trim_to_budget(history: &mut Vec<HistoryEntry>, byte_budget: usize) {
    let mut total = history.iter().fold(0usize, |sum, entry| sum.saturating_add(entry.retained_bytes));
    while history.len() > 1 && (history.len() > UNDO_LIMIT || total > byte_budget) {
        total = total.saturating_sub(history.remove(0).retained_bytes);
    }
}

/// A writer that only counts the bytes written to it.
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl DocState {
    /// A document for `wb`, which came from where `trust` says: [`Trust::FromFile`] for anything
    /// read from bytes (a file, a download, a recovery copy), [`Trust::Local`] only for a workbook
    /// made in this session.
    pub fn new(mut wb: Workbook, path: Option<String>, title: String, trust: Trust) -> DocState {
        let _ = gridcraft_model::PIVOT_INDEXER.set(pivot::index_cells);
        let mut calc = Calc::new();
        calc.recalc_all(&mut wb);
        Self::with_calc(wb, calc, path, title, trust)
    }
    /// A workbook as its file saved it: the dependency graph is built but nothing is
    /// evaluated, so every value is the one the file holds until an edit reaches it. `trust`
    /// is as for [`DocState::new`].
    pub fn from_saved(wb: Workbook, path: Option<String>, title: String, trust: Trust) -> DocState {
        let mut calc = Calc::new();
        calc.rebuild(&wb);
        Self::with_calc(wb, calc, path, title, trust)
    }
    fn with_calc(mut wb: Workbook, calc: Calc, path: Option<String>, title: String, trust: Trust) -> DocState {
        // Equations arrive without a layout (files never store one); painting reads only the cache.
        wb.refresh_equations();
        let scripts_trusted = !cmd::extra::has_scripts(&wb);
        let wb = Arc::new(wb);
        let sel = Selection::at(wb.active().map(|s| s.view_active).unwrap_or_default());
        DocState {
            saved: wb.clone(),
            wb,
            calc,
            selection: sel,
            undo: vec![],
            redo: vec![],
            path,
            revision: 1,
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            title,
            sheet_selections: vec![],
            unlocked_ranges: vec![],
            content_kind: None,
            scripts_trusted,
            trust,
            touched: None,
            edits: None,
            go_to_recent: vec![],
            distinct_disclosed: vec![],
        }
    }
    pub fn is_dirty(&self) -> bool {
        !Arc::ptr_eq(&self.wb, &self.saved)
    }
    pub fn sheet_index(&self) -> usize {
        self.wb.active_sheet()
    }
    /// Records the view state that is saved with the file (`sheetView topLeftCell`, the active
    /// cell): the first visible cell of the scrolling area per sheet index in `tops`, and the
    /// active sheet's active cell. View state never makes the workbook dirty.
    pub fn store_view(&mut self, tops: &[(usize, CellRef)]) {
        let active = self.selection.active;
        let si = self.wb.active_sheet();
        let changed = tops.iter().any(|(i, c)| self.wb.sheets.get(*i).is_some_and(|sh| sh.view_top_left != *c))
            || self.wb.sheets.get(si).is_some_and(|sh| sh.view_active != active);
        if !changed {
            return;
        }
        let dirty = self.is_dirty();
        let wb = Arc::make_mut(&mut self.wb);
        for (i, c) in tops {
            if let Some(sh) = wb.sheets.get_mut(*i)
                && sh.view_top_left != *c
            {
                Arc::make_mut(sh).view_top_left = *c;
            }
        }
        if let Some(sh) = wb.sheets.get_mut(si)
            && sh.view_active != active
        {
            Arc::make_mut(sh).view_active = active;
        }
        if !dirty {
            self.saved = self.wb.clone();
        }
    }
    pub fn display_title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.title.clone())
    }
}

/// Engine-side preferences.
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// After pressing Enter, move the selection: "down", "right", "up", "left", "none".
    pub enter_direction: String,
    pub autocomplete: bool,
    pub user_name: String,
    pub default_font: String,
    pub default_font_size: f32,
    pub sheets_in_new_workbook: usize,
    pub r1c1: bool,
    /// Words added with Add to Dictionary.
    pub user_dictionary: Vec<String>,
    /// View › Show › Formula Bar (an application option in Excel, not saved in the file).
    pub formula_bar: bool,
    /// The statistics the status bar shows for a selection (right-click the status bar).
    pub status_bar: StatusBarStats,
    /// Formulas › Recently Used: the functions most recently entered in a formula, newest first
    /// (at most [`recent::MAX`]; see [`recent`]). An application list, as in Excel.
    #[serde(deserialize_with = "recent::lenient")]
    pub recent_functions: Vec<String>,
}

/// The selection statistics shown on the status bar, set with `view.statusBar`. Excel turns on
/// Average, Count and Sum by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusBarStats {
    pub average: bool,
    pub count: bool,
    pub numerical_count: bool,
    pub minimum: bool,
    pub maximum: bool,
    pub sum: bool,
}

impl Default for StatusBarStats {
    fn default() -> Self {
        StatusBarStats { average: true, count: true, numerical_count: false, minimum: false, maximum: false, sum: true }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            enter_direction: "down".into(),
            autocomplete: true,
            user_name: "GridCraft User".into(),
            default_font: gridcraft_model::DEFAULT_FONT.into(),
            default_font_size: gridcraft_model::DEFAULT_FONT_SIZE,
            sheets_in_new_workbook: 1,
            r1c1: false,
            user_dictionary: vec![],
            formula_bar: true,
            status_bar: StatusBarStats::default(),
            recent_functions: recent::defaults(),
        }
    }
}

/// Requests from commands to the UI (open a dialog, show a message…).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum UiRequest {
    Dialog(String, Json),
    Message(String),
    /// Start editing the active cell (F2) with optional initial text.
    EditCell(Option<String>),
    /// Put a defined name at the caret of the cell being edited, in place of selected text; with
    /// no edit under way, start editing the active cell with `=` and the name.
    InsertName(String),
    OpenUrl(String),
}

/// Width in points of one line of text drawn in a cell font, measured by a front end that has
/// the font the grid draws with. With `exact` false the front end may answer with a cheaper
/// measure that is never narrower (AutoFit asks for it on columns with many distinct values).
/// Commands call it while they run, so a front end must not run commands while holding a lock
/// the measure takes (egui's: see `theme::cell_text_width` in the UI crate).
pub type TextWidth = Arc<dyn Fn(&str, &model::Font, bool) -> f32 + Send + Sync>;

#[derive(Default)]
pub struct Session {
    docs: Vec<DocState>,
    /// Index in `docs` of the active window's workbook.
    active: usize,
    /// Windows on the open workbooks ([`windows`]); every workbook has at least one.
    windows: Vec<WindowState>,
    active_window: Option<u64>,
    /// Last window id handed out.
    next_window: u64,
    /// Window activation counter (most recently used order).
    activation: u64,
    pub clipboard: Option<Clipboard>,
    pub prefs: Prefs,
    pub mode: Mode,
    /// Commands run so far, without their passwords ([`cmd::journal_params`]).
    pub journal: Vec<(String, Json)>,
    /// Serialized size of each journal entry, measured once when it is pushed, and their sum.
    journal_sizes: Vec<usize>,
    journal_bytes: usize,
    /// While a dry run runs (the footprint, `macro.check`), its commands are not journaled: the
    /// dry run is put back, and journaling it would evict the person's own oldest entries.
    pub(crate) journal_paused: u32,
    pub ui_requests: Vec<UiRequest>,
    /// Format painter source style (sticky when double-clicked).
    pub format_painter: Option<(Arc<Workbook>, usize, RangeRef, bool)>,
    /// Find state (last search).
    pub last_find: Option<Json>,
    /// View › Show › Ruler is off (rulers in Page Layout view; an application option).
    pub hide_ruler: bool,
    /// Journal position where Automate › Record Actions started.
    pub recording: Option<usize>,
    /// The recording buffer while a recorder runs ([`Session::start_recording`]): top-level
    /// commands with the place they ran in, kept apart from the journal.
    pub recorder: Option<record::Recorder>,
    /// How many `execute` calls are running (0 between top-level commands).
    depth: u32,
    /// The steps the last successful `automate.runScript` or `macro.run` ran while a recorder was
    /// on, for the recorder to keep with that step (see [`record::Recorder::ran`]); taken by the
    /// recording hook.
    pub(crate) script_ran: Option<Vec<(String, Json)>>,
    /// Macros the person allowed to run in this session: (document uid, the macro's content as
    /// canonical JSON, from [`macros::fingerprint`]).
    pub macro_approvals: std::collections::HashSet<(u64, String)>,
    /// The macro being debugged, between debugger actions.
    pub macro_debug: Option<macros::debug::Debugger>,
    /// Where the macro recorder started (beside `recorder`, its buffer).
    pub macro_recording: Option<macros::recorder::RecordingStart>,
    /// Draw tab tool: "" (select), "pen" or "eraser"; pen colour and width.
    pub draw_tool: String,
    pub draw_color: String,
    pub draw_width: f32,
    /// Review › Show Comments: the Comments pane is open.
    pub comments_pane: bool,
    /// Review › Notes › Show All Notes: every note is displayed. An application display setting
    /// (like Excel's comment indicator option), so it's not saved in the file.
    pub show_all_notes: bool,
    /// Whether `link.open` may open web and e-mail addresses without the user's confirmation
    /// (#2181). Host code sets it; no command does.
    pub link_policy: links::LinkPolicy,
    /// Open undo groups, innermost last.
    groups: Vec<OpenGroup>,
    next_group: u64,
    /// Counts group begins and ends, so `execute` can tell that a command opened or closed one.
    group_epoch: u64,
    /// Where the last group ended, so `execute` can tell a command's own changes from its group's.
    group_step: Option<GroupStep>,
    /// For each running command, innermost last: its document and that document's undo length
    /// when it began. Steps before that length are not touched while the command runs (see
    /// [`Session::bound_tail`]).
    marks: Vec<(u64, usize)>,
    /// Measures cell text for AutoFit Column Width. Without it (CLI, MCP) AutoFit uses
    /// [`cmd::format::approx_text_width`].
    pub text_width: Option<TextWidth>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }
    pub fn documents(&self) -> &[DocState] {
        &self.docs
    }
    pub fn active_index(&self) -> usize {
        self.active
    }
    /// Makes the workbook at `i` active, in its most recently active window (unless the active
    /// window already shows it).
    pub fn set_active(&mut self, i: usize) {
        let Some(uid) = self.docs.get(i).map(|d| d.uid) else { return };
        if self.active == i && self.active_window.and_then(|w| self.window(w)).is_some_and(|w| w.doc_uid == uid) {
            return;
        }
        if let Some(w) = self.doc_window(uid) {
            self.activate_window(w);
        }
    }
    pub fn active(&self) -> Option<&DocState> {
        self.docs.get(self.active)
    }
    /// Where the open workbook with this [`DocState::uid`] is in [`Session::documents`].
    pub fn document_index(&self, uid: u64) -> Option<usize> {
        self.docs.iter().position(|d| d.uid == uid)
    }
    pub fn active_mut(&mut self) -> Option<&mut DocState> {
        self.docs.get_mut(self.active)
    }
    pub fn doc(&self) -> Result<&DocState> {
        self.active().ok_or(EngineError::NoDocument)
    }
    pub fn doc_mut(&mut self) -> Result<&mut DocState> {
        self.active_mut().ok_or(EngineError::NoDocument)
    }
    /// The warning an edit of `at` on `sheet` gets from sheet protection, or `None` when the edit
    /// is allowed. Front ends ask before opening an editor, so typing into a locked cell never
    /// leaves a draft that can't be entered (and so blocks saving).
    pub fn locked_cell_message(&self, sheet: usize, at: CellRef) -> Option<String> {
        let d = self.active()?;
        let sh = d.wb.sheet(sheet)?;
        if !sh.is_protected() || !d.wb.styles.get(sh.style_id(at)).protection.locked {
            return None;
        }
        cmd::locked_cell_error(sh, at, &d.unlocked_ranges)
    }
    /// Opens a workbook in a new window, which becomes the active one.
    pub fn add_document(&mut self, d: DocState) -> usize {
        let uid = d.uid;
        self.docs.push(d);
        let w = self.push_window(uid);
        self.activate_window(w);
        self.docs.len() - 1
    }
    /// Closes a workbook and its windows; the most recently active remaining window comes forward.
    pub fn close_document(&mut self, i: usize) {
        if i < self.docs.len() {
            let d = self.docs.remove(i);
            self.groups.retain(|g| g.doc != d.uid);
            // A macro being debugged in this workbook ends with it; its group just went.
            if self.macro_debug.as_ref().is_some_and(|m| m.doc == d.uid) {
                self.macro_debug = None;
            }
            self.windows.retain(|w| w.doc_uid != d.uid);
            if self.active_window.is_some_and(|w| self.window(w).is_none()) {
                self.activate_recent();
            } else if let Some(di) = self.active_window.and_then(|w| self.window(w)).and_then(|w| self.document_index(w.doc_uid)) {
                self.active = di;
            }
        }
    }
    /// A new blank workbook titled `BookN`.
    pub fn new_workbook(&mut self) -> usize {
        let n = (1..).find(|n| !self.docs.iter().any(|d| d.title == format!("Book{n}") && d.path.is_none())).unwrap_or(1);
        let mut wb = Workbook::new();
        wb.props.author = self.prefs.user_name.clone();
        for _ in 1..self.prefs.sheets_in_new_workbook.clamp(1, 255) {
            let name = wb.next_sheet_name();
            wb.sheets.push(Arc::new(gridcraft_model::Sheet::new(name)));
        }
        self.add_document(DocState::new(wb, None, format!("Book{n}"), Trust::Local))
    }

    /// Journals a command, then trims the oldest entries past either cap. A recording in progress
    /// keeps pointing at the same entry (the steps the trim dropped are lost).
    fn push_journal(&mut self, id: &str, params: Json) {
        // Counted as it is written, without building the text: a paste's params can be MBs.
        let size = |e: &(String, Json)| {
            let mut counter = ByteCounter(0);
            serde_json::to_writer(&mut counter, e).map_or(0, |()| counter.0)
        };
        // `journal` is public: if it was changed from outside, measure it again.
        if self.journal_sizes.len() != self.journal.len() {
            self.journal_sizes = self.journal.iter().map(size).collect();
            self.journal_bytes = self.journal_sizes.iter().sum();
        }
        let entry = (id.to_string(), params);
        let entry_size = size(&entry);
        self.journal.push(entry);
        self.journal_sizes.push(entry_size);
        self.journal_bytes = self.journal_bytes.saturating_add(entry_size);
        let len = self.journal.len();
        let mut n = if len > JOURNAL_MAX_ENTRIES { len - JOURNAL_MAX_ENTRIES / 2 } else { 0 };
        let mut kept = self.journal_bytes.saturating_sub(self.journal_sizes.iter().take(n).sum());
        if self.journal_bytes > JOURNAL_MAX_BYTES {
            // The newest entry stays even when it is larger than the cap on its own.
            while kept > JOURNAL_MAX_BYTES / 2 && n + 1 < len {
                kept = kept.saturating_sub(self.journal_sizes.get(n).copied().unwrap_or(0));
                n += 1;
            }
        }
        if n > 0 {
            self.journal.drain(..n);
            self.journal_sizes.drain(..n);
            self.journal_bytes = kept;
            self.recording = self.recording.map(|r| r.saturating_sub(n));
        }
    }

    /// Cuts the journal back to its first `keep` entries, with its size bookkeeping.
    pub(crate) fn truncate_journal(&mut self, keep: usize) {
        if self.journal_sizes.len() == self.journal.len() {
            self.journal_sizes.truncate(keep);
            self.journal_bytes = self.journal_sizes.iter().sum();
        }
        self.journal.truncate(keep);
        self.recording = self.recording.map(|r| r.min(self.journal.len()));
    }

    pub fn commands(&self) -> Vec<CommandInfo> {
        command_specs().iter().map(|c| c.info(self)).collect()
    }

    /// Runs a command by id. Undoable commands record a history step when the workbook changed,
    /// unless an undo group is open on the document (the group then makes the step).
    /// Panics inside commands are caught and reported; the workbook is restored.
    pub fn execute(&mut self, id: &str, params: Json) -> Result<Json> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if let Err(why) = (spec.enabled)(self) {
            return Err(EngineError::Disabled(id.to_string(), why));
        }
        // A debugger whose undo group something else ended locks nothing any more (`macro.debug`
        // itself says that the run had ended).
        if id != "macro.debug" {
            macros::debug::drop_if_ended(self);
        }
        let query = !spec.journal && !spec.undoable;
        // While a macro is being debugged its undo group is open, and its workbook is locked:
        // between steps only the debugger, queries and commands that look around run there
        // (`macros::debug::LOOKS_AROUND`). Anything else waits, undoable or not (saving, view
        // modes, stopping Record Actions, which saves a script), also when another command runs
        // it. Only the run's own steps go ahead. Other workbooks stay editable.
        let locked = self.active().is_some_and(|d| self.macro_debug.as_ref().is_some_and(|m| m.locks(d.uid) && !m.stepping()));
        if locked && !macros::debug::runs_while_locked(id, query) {
            return Err(EngineError::Disabled(id.to_string(), macros::debug::BUSY.into()));
        }
        let before = self.active().map(|d| (d.uid, d.wb.clone(), d.selection.clone()));
        // A dry run inside this command that a panic cuts short leaves journaling paused and its
        // touch log open; the panic branch puts both back.
        let paused_before = self.journal_paused;
        let params = if params.is_null() { Json::Object(Default::default()) } else { params };
        let (first_group, group_epoch) = (self.next_group, self.group_epoch);
        // Whether a group was open on the document before the command, and how long its history was.
        let start = before.as_ref().and_then(|(uid, ..)| self.docs.iter().find(|d| d.uid == *uid).map(|d| (self.in_group(*uid), d.undo.len())));
        // A command that panics leaves the history as it found it: the steps commands nested in it
        // made or undid, and the redo history they cleared, come back. A length is not enough,
        // because a nested `edit.undo` removes entries. A query runs nothing that changes the
        // history (`every_query_leaves_the_history_alone` runs each one), so it needs no
        // copy; front ends run queries all the time.
        let history_before = before.as_ref().filter(|_| !query).and_then(|(uid, ..)| {
            #[cfg(test)]
            tests::HISTORY_COPIES.with(|n| n.set(n.get().saturating_add(1)));
            self.docs.iter().find(|d| d.uid == *uid).map(|d| (d.undo.clone(), d.redo.clone()))
        });
        // The groups open on the document before the command (usually none), for the same reason.
        let groups_before: Vec<(u64, String, Arc<Workbook>, Selection)> = match &before {
            Some((uid, ..)) => {
                self.groups.iter().filter(|g| g.doc == *uid).map(|g| (g.id, g.label.clone(), g.wb.clone(), g.selection.clone())).collect()
            }
            None => Vec::new(),
        };
        // The recorder keeps top-level calls with the place they ran in.
        let place = (self.depth == 0 && self.recorder.is_some()).then(|| match self.active() {
            Some(d) => (d.uid, d.wb.active_sheet(), d.wb.active().map(|sh| sh.name.clone()).unwrap_or_default(), d.selection.clone()),
            None => (0, 0, String::new(), Selection::default()),
        });
        let doc_uid = before.as_ref().map(|(uid, ..)| *uid);
        let requests_before = self.ui_requests.len();
        let marked = doc_uid.zip(start).map(|(uid, (_, len))| (uid, len));
        self.marks.extend(marked);
        self.depth = self.depth.saturating_add(1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, &params)));
        self.depth = self.depth.saturating_sub(1);
        if marked.is_some() {
            self.marks.pop();
        }
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(ToString::to_string)
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                // Restore the workbook and its history as they were, and drop the undo groups the
                // command opened.
                self.groups.retain(|g| g.id < first_group);
                let group_open = before.as_ref().is_some_and(|(uid, ..)| self.groups.iter().any(|g| g.doc == *uid));
                if let Some((uid, wb, sel)) = &before
                    && let Some(d) = self.docs.iter_mut().find(|d| d.uid == *uid)
                {
                    d.wb = wb.clone();
                    d.selection = sel.clone();
                    if let Some((undo, redo)) = history_before {
                        d.undo = undo;
                        d.redo = redo;
                    }
                    // A group open before the command and ended or rolled back inside it stays
                    // ended (its token went with the command), and keeps a step from its start to
                    // the restored workbook, so Undo still returns to the state before the group.
                    if !group_open
                        && let Some((_, label, wb, selection)) = groups_before.into_iter().min_by_key(|g| g.0)
                        && !Arc::ptr_eq(&wb, &d.wb)
                    {
                        push_entry(&mut d.undo, label, wb, &d.wb, selection);
                        d.redo.clear();
                    }
                    d.calc.rebuild(&d.wb);
                }
                if self.journal_paused != paused_before {
                    self.journal_paused = paused_before;
                    for d in self.docs.iter_mut() {
                        d.touched = None;
                    }
                }
                if self.depth == 0 {
                    // Only the `macro.debug` call itself can lose the run's place: a command nested
                    // in a step that panics is an error of that step, which the run handles.
                    macros::debug::recover(self);
                    self.trim_history();
                    self.group_step = None;
                }
                return Err(EngineError::Internal(id.to_string(), msg));
            }
        };
        // A command that fails leaves none of its own undo groups open (a `?` between
        // `group_begin` and `group_end`): each is rolled back as `group_rollback` does. Only a
        // command that succeeds keeps a group it opened (a debugger's start).
        if result.is_err() {
            self.rollback_groups_from(first_group);
        }
        if spec.journal && result.is_ok() {
            // A command that started recording is not part of the recording; one that stopped it
            // finds no recorder. A menu-style call that only opened a dialog (`file.open` with no
            // file, `home.formatCells` with no style) is not recorded either: the dialog's own
            // call is, so playing the recording back runs the command once, with its parameters.
            // A call that opened a dialog and changed the workbook too (a script with an edit before
            // a menu-style step) is recorded: the edit happened, and the dialog's confirming call
            // is a step of its own.
            let opened_dialog = self.ui_requests.get(requests_before..).is_some_and(|new| new.iter().any(|q| matches!(q, UiRequest::Dialog(..))));
            let changed = before.as_ref().is_some_and(|(uid, wb, _)| self.docs.iter().any(|d| d.uid == *uid && !Arc::ptr_eq(&d.wb, wb)));
            if let Some((doc, sheet, sheet_name, selection)) = place.filter(|_| (!opened_dialog || changed) && !record::NOT_RECORDED.contains(&id))
                && let Some(rec) = self.recorder.as_mut()
            {
                // A toggle (Bold) is recorded with the state it set, as Excel records
                // `Bold = True`: playing it back sets that state rather than flipping it again.
                let params = match &result {
                    Ok(r) if params.is_object() => record::with_toggle_state(id, cmd::journal_params(&params), r),
                    _ => cmd::journal_params(&params),
                };
                let kept = rec.push(record::RecordedStep { doc, sheet, sheet_name, selection, id: id.to_string(), params });
                // A script or macro run is kept with the steps it ran, so editing or deleting the
                // script or macro before the recording stops does not change what was recorded.
                if let Some(ran) = self.script_ran.take()
                    && kept
                    && matches!(id, "automate.runScript" | "macro.run")
                {
                    rec.attach_ran(ran);
                }
            }
            if self.journal_paused == 0 {
                self.push_journal(id, cmd::journal_params(&params));
                recent::note(&mut self.prefs.recent_functions, id, &params);
            }
        }
        // Inside an undo group the group owns the step. A command that ended a group open before
        // it leaves the group its step and makes one of its own for what it changed after the end.
        // A command that opened and closed its own groups makes one step of its own, which
        // replaces the steps those groups made.
        let epoch_changed = self.group_epoch != group_epoch;
        let in_group_now = before.as_ref().is_some_and(|(uid, ..)| self.in_group(*uid));
        let was_open = start.is_some_and(|(was_open, _)| was_open);
        if spec.undoable
            && let Some((uid, wb, sel)) = before
            && let Some(d) = self.docs.iter_mut().find(|d| d.uid == uid)
            && !Arc::ptr_eq(&d.wb, &wb)
        {
            let label = spec.label.trim_end_matches('…').to_string();
            if in_group_now {
                // A group this command opened and left open (a debugger's start) takes the
                // command's start as its own, so what the command changed before opening it is
                // part of the group's step and not a step of its own.
                if !was_open
                    && let Some((_, len)) = start
                    && let Some(g) = self.groups.iter_mut().filter(|g| g.doc == uid && g.id >= first_group).min_by_key(|g| g.id)
                {
                    g.start = len;
                    g.wb = wb;
                    g.selection = sel;
                }
            } else if epoch_changed && was_open {
                // The group it ended made its step; changes after the end, whether direct or by
                // nested commands, are the command's own step from where the group ended.
                if let Some(step) = self.group_step.as_ref().filter(|g| g.doc == uid)
                    && (d.undo.len() > step.len || !Arc::ptr_eq(&step.after, &d.wb))
                {
                    d.undo.truncate(step.len);
                    push_entry(&mut d.undo, label, step.after.clone(), &d.wb, step.selection.clone());
                    d.redo.clear();
                }
            } else {
                let mut label = label;
                if epoch_changed && let Some((_, len)) = start {
                    // A command whose only step is its group's keeps that name ("Run Macro: Totals").
                    // That is so only when the one entry the command added is a group's step from
                    // the command's own start to its own result: an entry a nested command made,
                    // a group step of an earlier command (undone since), or a group's step that
                    // the command edited past all name the step after the command instead.
                    if d.undo.len() == len + 1
                        && let Some(last) = d.undo.last()
                        && Arc::ptr_eq(&last.wb, &wb)
                        && self.group_step.as_ref().is_some_and(|g| g.doc == uid && g.stepped && Arc::ptr_eq(&g.after, &d.wb))
                    {
                        label = last.label.clone();
                    }
                    d.undo.truncate(len);
                }
                push_entry(&mut d.undo, label, wb, &d.wb, sel);
                d.redo.clear();
            }
            // Inside a group the redo history waits for the group: `group_end` clears it when the
            // group becomes a step, and `group_rollback` puts it back as it was.
            d.revision += 1;
        }
        // The history limit applies once the outermost command has collapsed its steps, so the
        // lengths it compared stay true. Meanwhile the steps made inside it are bounded on their own.
        if self.depth == 0 {
            self.trim_history();
            self.group_step = None;
        } else if let Some(uid) = doc_uid {
            self.bound_tail(uid);
        }
        result
    }

    /// Runs `f` as one undo step labelled `label` on the active document: whatever commands `f`
    /// runs, one Undo brings back the workbook as it was before. If `f` fails, every change it made
    /// is rolled back and its error is returned. Groups nest; the outermost one makes the step.
    pub fn grouped<R>(&mut self, label: &str, f: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
        let token = self.group_begin(label)?;
        let doc = token.doc();
        // A panic in `f` rolls the group back before it goes on, so a caller that catches it
        // (outside `execute`, which drops a command's groups itself) finds no group left open.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(Ok(r)) => match self.group_end(token) {
                Ok(_) => Ok(r),
                // `f` closed the group's document (`file.close`, or `file.open` taking the place of
                // an untouched new workbook), and the group went with it: nothing is left to undo.
                Err(_) if !self.docs.iter().any(|d| d.uid == doc) => Ok(r),
                Err(e) => Err(e),
            },
            Ok(Err(e)) => {
                // The error from `f` is the one to report. A rollback fails only when the group
                // has already ended, and then there is nothing left to undo.
                let _ = self.group_rollback(token);
                Err(e)
            }
            Err(panic) => {
                let _ = self.group_rollback(token);
                std::panic::resume_unwind(panic)
            }
        }
    }

    /// Opens an undo group on the active document that can span several calls (for example a
    /// debugger that runs one step per call). Until [`Session::group_end`] or
    /// [`Session::group_rollback`], undoable commands on that document add no undo steps of their
    /// own, and Undo and Redo are unavailable there.
    pub fn group_begin(&mut self, label: &str) -> Result<GroupToken> {
        let d = self.doc()?;
        let g = OpenGroup {
            id: self.next_group,
            doc: d.uid,
            label: label.trim_end_matches('…').to_string(),
            start: d.undo.len(),
            wb: d.wb.clone(),
            selection: d.selection.clone(),
            redo: d.redo.clone(),
            recorded: self.recorder.as_ref().map(record::Recorder::mark),
        };
        let token = GroupToken { id: g.id, doc: g.doc };
        self.next_group = self.next_group.saturating_add(1);
        self.group_epoch = self.group_epoch.wrapping_add(1);
        self.groups.push(g);
        Ok(token)
    }

    /// Ends an undo group. If the workbook changed, the group becomes one undo step, unless it is
    /// nested in another open group on the same document, which then owns the step. Groups opened
    /// inside it and still open end with it. Returns whether the workbook changed.
    pub fn group_end(&mut self, token: GroupToken) -> Result<bool> {
        let g = self.take_group(&token)?;
        let outer = self.in_group(g.doc);
        let Some(d) = self.docs.iter_mut().find(|d| d.uid == g.doc) else { return Ok(false) };
        // Steps pushed directly while the group was open fold into the group's step.
        d.undo.truncate(g.start);
        let changed = !Arc::ptr_eq(&d.wb, &g.wb);
        if changed && !outer {
            push_entry(&mut d.undo, g.label, g.wb, &d.wb, g.selection);
            d.redo.clear();
            d.revision += 1;
        }
        if !outer {
            self.group_step =
                Some(GroupStep { doc: g.doc, after: d.wb.clone(), selection: d.selection.clone(), len: d.undo.len(), stepped: changed });
        }
        // Inside a command, the outermost command applies the history limit when it finishes.
        if self.depth == 0 {
            self.trim_history();
        }
        Ok(changed)
    }

    /// Stops the running action on the document with uid `doc`: every undo group open on it is
    /// rolled back as [`Session::group_rollback`] does, putting back the workbook, selection, undo
    /// and redo history as they were when the outermost one began. Their tokens are then spent
    /// (ending or rolling one back reports that it has already ended). Returns whether a group
    /// was open. `automate.stopAction` runs it on the active document.
    pub fn abort_groups(&mut self, doc: u64) -> bool {
        let Some(id) = self.groups.iter().filter(|g| g.doc == doc).map(|g| g.id).min() else { return false };
        // Rolling back the outermost group ends the ones opened inside it.
        self.group_rollback(GroupToken { id, doc }).is_ok()
    }

    /// Rolls back every open undo group numbered `first` or later (the groups a failed command
    /// opened), outermost first on each document.
    fn rollback_groups_from(&mut self, first: u64) {
        let opened: Vec<(u64, u64)> = self.groups.iter().filter(|g| g.id >= first).map(|g| (g.id, g.doc)).collect();
        for (id, doc) in opened {
            // A group inside one rolled back before it is already gone.
            if self.groups.iter().any(|g| g.id == id) {
                let _ = self.group_rollback(GroupToken { id, doc });
            }
        }
    }

    /// Bounds the undo steps pushed while a command runs (a script of many steps, a subtotal per
    /// group): at most [`UNDO_LIMIT`] of them within [`UNDO_BYTES_LIMIT`], dropping the oldest.
    /// Only steps after the latest mark go: a running command's start, an open group's start and
    /// the end of the last group are positions the command or group collapses its steps by when
    /// it finishes, so what is before them stays as it is until [`Session::trim_history`].
    fn bound_tail(&mut self, uid: u64) {
        let Some(d) = self.docs.iter_mut().find(|d| d.uid == uid) else { return };
        let marks = self.marks.iter().filter(|(doc, _)| *doc == uid).map(|(_, len)| *len);
        let groups = self.groups.iter().filter(|g| g.doc == uid).map(|g| g.start);
        let step = self.group_step.as_ref().filter(|g| g.doc == uid).map(|g| g.len);
        let mark = marks.chain(groups).chain(step).max().unwrap_or(0).min(d.undo.len());
        let mut bytes = d.undo.iter().skip(mark).fold(0usize, |sum, e| sum.saturating_add(e.retained_bytes));
        while d.undo.len() > mark + 1 && (d.undo.len() - mark > UNDO_LIMIT || bytes > UNDO_BYTES_LIMIT) {
            bytes = bytes.saturating_sub(d.undo.remove(mark).retained_bytes);
        }
    }

    /// Drops the oldest undo steps past [`UNDO_LIMIT`] or [`UNDO_BYTES_LIMIT`]. A document with
    /// an open group waits for the group to end, because the group remembers where its steps start.
    fn trim_history(&mut self) {
        let groups = &self.groups;
        for d in self.docs.iter_mut().filter(|d| !groups.iter().any(|g| g.doc == d.uid)) {
            trim_to_budget(&mut d.undo, UNDO_BYTES_LIMIT);
        }
    }

    /// Ends an undo group by putting back the workbook, selection, undo and redo history as they
    /// were when it began.
    pub fn group_rollback(&mut self, token: GroupToken) -> Result<()> {
        let g = self.take_group(&token)?;
        let Some(d) = self.docs.iter_mut().find(|d| d.uid == g.doc) else { return Ok(()) };
        if !Arc::ptr_eq(&d.wb, &g.wb) {
            d.wb = g.wb;
            d.calc.rebuild(&d.wb);
        }
        d.selection = g.selection;
        d.undo.truncate(g.start);
        d.redo = g.redo;
        d.revision += 1;
        // The steps recorded while the group was open go with their edits (all of them when the
        // recording started inside the group).
        if let Some(rec) = self.recorder.as_mut() {
            rec.cut_back(g.recorded.unwrap_or((0, 0)), g.doc);
        }
        // The record of the last group step stays when the document is back exactly where that
        // step left it (this group began there); otherwise the rollback is where the group ended.
        let kept = self.group_step.as_ref().is_some_and(|st| st.doc == g.doc && st.len == g.start && Arc::ptr_eq(&st.after, &d.wb));
        if !kept && !self.groups.iter().any(|o| o.doc == g.doc) {
            self.group_step = Some(GroupStep { doc: g.doc, after: d.wb.clone(), selection: d.selection.clone(), len: g.start, stepped: false });
        }
        Ok(())
    }

    /// Starts recording top-level commands into an empty buffer, replacing any recording in
    /// progress. Queries, failed commands and commands run by other commands are not recorded.
    pub fn start_recording(&mut self) {
        self.recorder = Some(record::Recorder::default());
    }

    /// Stops recording and returns what was recorded (`None` when nothing was recording).
    pub fn stop_recording(&mut self) -> Option<record::Recorder> {
        self.recorder.take()
    }

    /// Whether the recording buffer is collecting commands.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// How many commands are running: 0 between top-level commands, 1 inside one, 2 inside a
    /// command run by a command, and so on.
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Runs a command as a command in progress would (one level deeper), for tests of what a
    /// nested call may do.
    #[cfg(test)]
    pub(crate) fn execute_nested(&mut self, id: &str, params: Json) -> Result<Json> {
        self.depth = self.depth.saturating_add(1);
        let r = self.execute(id, params);
        self.depth = self.depth.saturating_sub(1);
        r
    }

    /// Whether the token's undo group is still open (nothing has ended or rolled it back).
    pub fn group_is_open(&self, token: &GroupToken) -> bool {
        self.groups.iter().any(|g| g.id == token.id)
    }

    /// Whether an undo group is open on the document with this uid.
    pub fn in_group(&self, doc: u64) -> bool {
        self.groups.iter().any(|g| g.doc == doc)
    }

    /// Removes the token's group and any group on the same document opened after it.
    fn take_group(&mut self, token: &GroupToken) -> Result<OpenGroup> {
        let ended = || EngineError::Other("That undo group has already ended.".into());
        let at = self.groups.iter().position(|g| g.id == token.id).ok_or_else(ended)?;
        let mut later = self.groups.split_off(at).into_iter();
        let g = later.next().ok_or_else(ended)?;
        self.groups.extend(later.filter(|o| o.doc != g.doc));
        self.group_epoch = self.group_epoch.wrapping_add(1);
        Ok(g)
    }

    pub fn run(&mut self, id: &str, params: Json) -> std::result::Result<Json, String> {
        self.execute(id, params).map_err(|e| e.to_string())
    }

    pub fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.ui_requests)
    }

    /// Runs a command for a caller with no UI (MCP, CLI): queued UI requests are dropped, and a command that only
    /// queued a file dialog because it got no file (`file.open`, `file.saveAs`, `file.save` on a workbook with no
    /// file yet, `data.getData`) is an error instead of a silent success.
    pub fn execute_headless(&mut self, id: &str, params: Json) -> Result<Json> {
        let r = self.execute(id, params);
        let requests = self.take_ui_requests();
        let asked_for_file = requests.iter().any(|q| matches!(q, UiRequest::Dialog(name, _) if name == "open" || name == "saveAs"));
        // A call that only opened a dialog is not recorded (see `execute`), so the recording
        // has no step to take back here.
        if r.is_ok() && asked_for_file {
            let what = match id {
                "file.open" => "`path` or `base64`",
                "data.getData" | "data.fromTextCsv" => "`path` or `text`",
                _ => "`path`",
            };
            let msg = format!("missing {what}: programmatic calls never open dialogs");
            return Err(EngineError::BadParams { cmd: id.to_string(), msg });
        }
        r
    }
}

/// Convenience: the active workbook's active sheet.
pub fn active_sheet(s: &Session) -> Option<&gridcraft_model::Sheet> {
    s.active().and_then(|d| d.wb.active())
}

pub fn cell(r: u32, c: u32) -> CellRef {
    CellRef::new(r, c)
}

#[cfg(test)]
mod tests;
