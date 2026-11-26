//! Windows on open workbooks.
//!
//! Every open workbook has one or more windows (View › New Window adds another on the same
//! workbook). A window has its own selection, active sheet and per-sheet selections. The active
//! window's view lives in its [`DocState`] (`selection`, `sheet_selections`) and in the workbook's
//! active sheet ([`gridcraft_model::workbook::Workbook::active_sheet`]), where every command reads
//! it; each other window keeps its view in [`WindowState`] until it is activated. Swapping a view
//! in or out never copies the workbook or makes it dirty: the active sheet is set in place through
//! the shared `Arc`, and selections live outside the workbook.

use serde_json::{Value as Json, json};

use crate::{DocState, EngineError, Result, Selection, Session};

/// The view state one window keeps while another window on its workbook is showing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowView {
    pub selection: Selection,
    pub sheet: usize,
    /// The active sheet's name, so a sheet inserted, deleted or moved meanwhile in another
    /// window still finds it.
    pub sheet_name: String,
    pub sheet_selections: Vec<Selection>,
}

/// One window on an open workbook.
#[derive(Clone, Debug)]
pub struct WindowState {
    /// Identifies the window for the rest of the session; never reused.
    pub id: u64,
    /// The [`DocState::uid`] of the workbook it shows.
    pub doc_uid: u64,
    /// View › Hide.
    pub hidden: bool,
    /// `None` while this window's view is the one in the document (the active window, or the
    /// workbook's most recently active window); exactly one window per workbook has `None`.
    view: Option<WindowView>,
    /// Activation order: higher was active more recently.
    last_active: u64,
}

impl WindowView {
    fn capture(d: &DocState) -> WindowView {
        WindowView {
            selection: d.selection.clone(),
            sheet: d.wb.active_sheet(),
            sheet_name: d.wb.sheets.get(d.wb.active_sheet()).map(|s| s.name.clone()).unwrap_or_default(),
            sheet_selections: d.sheet_selections.clone(),
        }
    }

    fn apply(self, d: &mut DocState) {
        let sheets = &d.wb.sheets;
        let at = if sheets.get(self.sheet).is_some_and(|s| s.name == self.sheet_name) {
            self.sheet
        } else {
            sheets.iter().position(|s| s.name == self.sheet_name).unwrap_or(self.sheet)
        };
        d.wb.set_active_sheet(at.min(sheets.len().saturating_sub(1)));
        d.selection = self.selection;
        d.sheet_selections = self.sheet_selections;
    }
}

/// Which window a command from a script or agent acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// The active window.
    #[default]
    Active,
    /// A window by [`WindowState::id`].
    Window(u64),
    /// A workbook by [`DocState::uid`]: its most recently active window.
    Doc(u64),
}

impl Target {
    /// Reads `window` or `doc` from a JSON object (control channel envelopes, MCP arguments).
    /// Neither present is [`Target::Active`]; a value that isn't an id is an error.
    pub fn from_json(p: &Json) -> Result<Target> {
        let id = |k: &str| -> Result<Option<u64>> {
            match p.get(k) {
                None | Some(Json::Null) => Ok(None),
                Some(v) => v.as_u64().map(Some).ok_or_else(|| EngineError::Other(format!("`{k}` must be a window or workbook id number"))),
            }
        };
        Ok(match (id("window")?, id("doc")?) {
            (Some(w), _) => Target::Window(w),
            (None, Some(d)) => Target::Doc(d),
            (None, None) => Target::Active,
        })
    }
}

/// What [`Session::leave_target`] restores.
#[derive(Debug)]
#[must_use]
pub struct TargetGuard {
    id: u64,
    back: Option<u64>,
}

impl Session {
    /// The open windows, in the order they were opened.
    pub fn windows(&self) -> &[WindowState] {
        &self.windows
    }

    /// The active window's id (`None` with no workbook open).
    pub fn active_window(&self) -> Option<u64> {
        self.active_window
    }

    pub fn window(&self, id: u64) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// A workbook's windows in opening order.
    fn windows_of(&self, doc: u64) -> impl Iterator<Item = &WindowState> {
        self.windows.iter().filter(move |w| w.doc_uid == doc)
    }

    /// The window title: the workbook's name when it has one window, "Name:1", "Name:2"… in
    /// opening order when it has several.
    pub fn window_title(&self, id: u64) -> Option<String> {
        let w = self.window(id)?;
        let d = self.docs.iter().find(|d| d.uid == w.doc_uid)?;
        let name = d.display_title();
        let mut n = 0;
        let mut at = 0;
        for x in self.windows_of(w.doc_uid) {
            n += 1;
            if x.id == id {
                at = n;
            }
        }
        Some(if n > 1 { format!("{name}:{at}") } else { name })
    }

    /// The most recently active window of a workbook, preferring one that isn't hidden.
    pub fn doc_window(&self, doc: u64) -> Option<u64> {
        self.windows_of(doc).max_by_key(|w| (!w.hidden, w.last_active)).map(|w| w.id)
    }

    /// Resolves a [`Target`] to a window id.
    pub fn target_window(&self, t: Target) -> Result<u64> {
        match t {
            Target::Active => self.active_window.ok_or(EngineError::NoDocument),
            Target::Window(id) => self.window(id).map(|w| w.id).ok_or_else(|| EngineError::Other(format!("no open window has id {id}"))),
            Target::Doc(uid) => self.doc_window(uid).ok_or_else(|| EngineError::Other(format!("no open workbook has id {uid}"))),
        }
    }

    pub(crate) fn push_window(&mut self, doc_uid: u64) -> u64 {
        self.next_window = self.next_window.saturating_add(1);
        let id = self.next_window;
        self.windows.push(WindowState { id, doc_uid, hidden: false, view: None, last_active: 0 });
        id
    }

    /// Makes a window active, swapping its view into its workbook. Unknown ids are ignored.
    pub fn activate_window(&mut self, id: u64) {
        self.enter_window(id, true);
    }

    /// Makes a window the one commands act on while the interface draws it, without counting as
    /// an activation: the most recently active window of each workbook stays the same.
    pub fn show_window(&mut self, id: u64) {
        self.enter_window(id, false);
    }

    fn enter_window(&mut self, id: u64, recent: bool) {
        let Some(wi) = self.windows.iter().position(|w| w.id == id) else { return };
        let Some(w) = self.windows.get(wi) else { return };
        let doc = w.doc_uid;
        if w.view.is_some()
            && let Some(di) = self.document_index(doc)
        {
            let live = self.windows.iter().position(|x| x.doc_uid == doc && x.view.is_none());
            let incoming = self.windows.get_mut(wi).and_then(|w| w.view.take());
            if let Some(d) = self.docs.get_mut(di) {
                if let Some(li) = live
                    && let Some(lw) = self.windows.get_mut(li)
                {
                    lw.view = Some(WindowView::capture(d));
                }
                if let Some(v) = incoming {
                    v.apply(d);
                }
            }
        }
        if recent {
            self.activation = self.activation.saturating_add(1);
            let stamp = self.activation;
            if let Some(w) = self.windows.get_mut(wi) {
                w.last_active = stamp;
            }
        }
        self.active_window = Some(id);
        if let Some(di) = self.document_index(doc) {
            self.active = di;
        }
    }

    /// Activates the most recently active window that isn't hidden, else any window, else none.
    pub(crate) fn activate_recent(&mut self) {
        let next = self.windows.iter().max_by_key(|w| (!w.hidden, w.last_active)).map(|w| w.id);
        match next {
            Some(id) => self.activate_window(id),
            None => {
                self.active_window = None;
                self.active = 0;
            }
        }
    }

    /// View › New Window: another window on the active window's workbook, showing what it shows.
    pub fn new_window(&mut self) -> Result<u64> {
        let current = self.active_window.ok_or(EngineError::NoDocument)?;
        let doc = self.window(current).map(|w| w.doc_uid).ok_or(EngineError::NoDocument)?;
        let view = self.doc().map(WindowView::capture)?;
        // The new window takes over the live view (identical at this point); the old one keeps a copy.
        if let Some(w) = self.windows.iter_mut().find(|w| w.doc_uid == doc && w.view.is_none()) {
            w.view = Some(view);
        }
        let id = self.push_window(doc);
        self.activate_window(id);
        Ok(id)
    }

    /// Closes one window. The workbook stays open while it has another window; closing its last
    /// window closes the workbook (unsaved changes are the caller's to ask about).
    pub fn close_window(&mut self, id: u64) {
        let Some(wi) = self.windows.iter().position(|w| w.id == id) else { return };
        let doc = self.windows.get(wi).map(|w| w.doc_uid).unwrap_or_default();
        if self.windows_of(doc).count() <= 1 {
            if let Some(di) = self.document_index(doc) {
                self.close_document(di);
            }
            return;
        }
        let closed = self.windows.remove(wi);
        if closed.view.is_none() {
            // Its view was the one in the document: the workbook's next window takes it over.
            if let Some(next) = self.windows.iter().filter(|w| w.doc_uid == doc).max_by_key(|w| w.last_active).map(|w| w.id) {
                let incoming = self.windows.iter_mut().find(|w| w.id == next).and_then(|w| w.view.take());
                if let Some(v) = incoming
                    && let Some(d) = self.docs.iter_mut().find(|d| d.uid == doc)
                {
                    v.apply(d);
                }
            }
        }
        if self.active_window == Some(id) {
            self.activate_recent();
        }
    }

    /// View › Hide on a window; the most recent visible window comes forward.
    pub fn hide_window(&mut self, id: u64) {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else { return };
        w.hidden = true;
        if self.active_window == Some(id) && self.windows.iter().any(|w| !w.hidden) {
            self.activate_recent();
        }
    }

    /// View › Unhide: shows a hidden window and activates it.
    pub fn unhide_window(&mut self, id: u64) {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else { return };
        w.hidden = false;
        self.activate_window(id);
    }

    /// The most recently hidden window.
    pub fn last_hidden_window(&self) -> Option<u64> {
        self.windows.iter().filter(|w| w.hidden).max_by_key(|w| w.last_active).map(|w| w.id)
    }

    /// The windows as scripts and the Switch Windows lists see them:
    /// `[{window, doc, index, title, dirty, active, hidden}]`, `index` being the workbook's
    /// position in [`Session::documents`].
    pub fn window_list(&self) -> Json {
        Json::Array(
            self.windows
                .iter()
                .filter_map(|w| {
                    let index = self.document_index(w.doc_uid)?;
                    let d = self.docs.get(index)?;
                    Some(json!({
                        "window": w.id,
                        "doc": w.doc_uid,
                        "index": index,
                        "title": self.window_title(w.id).unwrap_or_default(),
                        "dirty": d.is_dirty(),
                        "active": self.active_window == Some(w.id),
                        "hidden": w.hidden,
                    }))
                })
                .collect(),
        )
    }

    /// Runs `f` with the target window active, then brings back the window that was active, so a
    /// script acting on another window doesn't change what the user is looking at. When `f`
    /// itself switched windows (`window.activate`, `file.new`, a close), its choice stands.
    pub fn with_target<T>(&mut self, t: Target, f: impl FnOnce(&mut Session) -> T) -> Result<T> {
        let guard = self.enter_target(t)?;
        let r = f(self);
        self.leave_target(guard);
        Ok(r)
    }

    /// The first half of [`Session::with_target`], for callers whose closure can't borrow the
    /// session alone (the UI runs commands through its app): activates the target window and
    /// returns what [`Session::leave_target`] needs to bring the previous one back.
    pub fn enter_target(&mut self, t: Target) -> Result<Option<TargetGuard>> {
        if t == Target::Active {
            return Ok(None);
        }
        let id = self.target_window(t)?;
        let back = self.active_window;
        self.activate_window(id);
        Ok(Some(TargetGuard { id, back }))
    }

    pub fn leave_target(&mut self, guard: Option<TargetGuard>) {
        let Some(TargetGuard { id, back: Some(back) }) = guard else { return };
        if self.active_window == Some(id) && back != id && self.window(back).is_some() {
            self.activate_window(back);
        }
    }

    /// [`Session::execute`] on a target window.
    pub fn execute_in(&mut self, t: Target, id: &str, params: Json) -> Result<Json> {
        self.with_target(t, |s| s.execute(id, params))?
    }

    /// [`Session::execute_headless`] on a target window.
    pub fn execute_headless_in(&mut self, t: Target, id: &str, params: Json) -> Result<Json> {
        self.with_target(t, |s| s.execute_headless(id, params))?
    }
}

/// Commands that take `window`/`doc` as their own parameters rather than as a target.
pub fn is_window_command(id: &str) -> bool {
    id.starts_with("window.") || matches!(id, "view.newWindow" | "view.hideWindow" | "view.unhideWindow" | "view.switchWindows")
}
