//! Several document tabs picked at once (#4590): Ctrl-click (⌘-click on macOS) adds or removes a
//! tab, Shift-click takes the run from the last tab picked, and dragging one of them moves them
//! all.

use super::{DocId, DocView, PdfCraftApp};

/// The tabs picked with Ctrl/⌘- or Shift-click, by document, and the active document they were
/// picked with. Once another document becomes active by any other route (a plain click, Ctrl+Tab,
/// opening or closing a file), the active tab is the whole selection again.
#[derive(Clone, Debug, Default)]
pub(crate) struct TabSelection {
    ids: Vec<DocId>,
    /// Where a Shift-click's run starts: the tab last clicked without Shift.
    anchor: Option<DocId>,
    /// The active document when `ids` was set.
    active: Option<DocId>,
}

impl PdfCraftApp {
    /// The selected tabs' documents, in tab order: the active one and, while it stays active,
    /// the others picked with it. Empty on the Home tab.
    pub fn selected_tabs(&self) -> Vec<DocId> {
        let Some((_, active)) = self.active_ids() else { return Vec::new() };
        let s = &self.tab_selection;
        if s.active != Some(active) {
            return vec![active];
        }
        self.views.iter().map(|v| v.id).filter(|id| *id == active || s.ids.contains(id)).collect()
    }

    /// Clicking the tab at `index`: alone it shows that document and selects only its tab;
    /// `command` (Ctrl, ⌘ on macOS) adds the tab to the selection or takes it out; `shift`
    /// selects the run of tabs from the last one clicked without Shift.
    pub(crate) fn click_tab(&mut self, index: usize, command: bool, shift: bool) {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return };
        let mut ids = self.selected_tabs();
        let mut anchor = Some(id);
        let mut active = Some(index);
        if shift {
            // The run starts where the last click without Shift was, even one that took that tab
            // out of the selection, and a Shift-click leaves it there.
            let current = self.tab_selection.active == self.active_ids().map(|(_, a)| a);
            let open = |a: &DocId| self.views.iter().any(|v| v.id == *a);
            let start = self.tab_selection.anchor.filter(|a| current && open(a)).or_else(|| self.active_ids().map(|(_, a)| a)).unwrap_or(id);
            let from = self.views.iter().position(|v| v.id == start).unwrap_or(index);
            let run: Vec<DocId> = self.views.get(from.min(index)..=from.max(index)).unwrap_or_default().iter().map(|v| v.id).collect();
            // Ctrl/⌘ as well keeps what was picked before; Shift alone replaces it.
            if command {
                ids.extend(run.into_iter().filter(|r| !ids.contains(r)).collect::<Vec<_>>());
            } else {
                ids = run;
            }
            anchor = Some(start);
        } else if command {
            // The last selected tab stays (there is always an active document), though the
            // click still starts the next Shift-click's run there.
            if ids.len() == 1 && ids.contains(&id) {
                active = self.active;
            } else if let Some(at) = ids.iter().position(|s| *s == id) {
                ids.remove(at);
                // The active tab taken out hands over to its neighbour in the selection.
                if self.active == Some(index) {
                    let next = ids.get(at).or_else(|| ids.last()).copied();
                    active = next.and_then(|n| self.views.iter().position(|v| v.id == n));
                } else {
                    active = self.active;
                }
            } else {
                ids.push(id);
            }
        } else {
            ids = vec![id];
        }
        self.active = active;
        let active = self.active_ids().map(|(_, a)| a);
        self.tab_selection = TabSelection { ids, anchor, active };
    }

    /// Forgets the tabs picked with the active one once another document is shown by any route
    /// but a click on the strip (Ctrl+Tab, opening or closing a file, Home), so coming back to
    /// it later selects its tab alone. Run once a frame.
    pub(crate) fn forget_stale_tab_selection(&mut self) {
        let active = self.active_ids().map(|(_, id)| id);
        if self.tab_selection.active != active {
            self.tab_selection = TabSelection { ids: Vec::new(), anchor: active, active };
        }
    }

    /// The tabs a drag of the tab at `index` moves, by index: the whole selection when the tab
    /// is one of several selected, or that tab alone.
    pub(crate) fn dragged_tabs(&self, index: usize) -> Vec<usize> {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return Vec::new() };
        let selected = self.selected_tabs();
        if selected.len() < 2 || !selected.contains(&id) {
            return vec![index];
        }
        self.views.iter().enumerate().filter(|(_, v)| selected.contains(&v.id)).map(|(i, _)| i).collect()
    }

    /// Moves the tabs of documents `moving`, kept together in their order on the strip, so they
    /// sit after the first `to` of the other tabs. The same document stays active, and the
    /// selection stays as it was. A `to` past the end puts them last.
    pub fn move_tabs(&mut self, moving: &[DocId], to: usize) {
        let active = self.active_ids().map(|(_, id)| id);
        let (group, mut rest): (Vec<DocView>, Vec<DocView>) = std::mem::take(&mut self.views).into_iter().partition(|v| moving.contains(&v.id));
        let tail = rest.split_off(to.min(rest.len()));
        rest.extend(group);
        rest.extend(tail);
        self.views = rest;
        if let Some(id) = active {
            self.active = self.views.iter().position(|v| v.id == id);
        }
    }
}
