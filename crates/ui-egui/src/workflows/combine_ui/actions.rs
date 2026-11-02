use super::files::{Preview, RowAction};
use super::parts::{moved_pages, removed_pages, step_to};
use super::view::expandable;
use super::{BTreeSet, PdfCraftApp};

impl PdfCraftApp {
    pub(crate) fn combine_apply(&mut self, action: RowAction) {
        let page_edit = matches!(action, RowAction::ClickPage(..) | RowAction::MovePages { .. } | RowAction::RemovePages(_));
        self.combine_apply_once(action);
        // One kind of selection at a time: files picked (by a click, the keys, a drop, Unlock…)
        // let go of the pages picked, so the trash never takes pages the user no longer means.
        if !self.combine_tab.selected.is_empty() {
            self.combine_tab.selected_pages.clear();
            self.combine_tab.page_anchor = None;
        }
        // A file's parts brought side by side (the file between them removed or moved away) are
        // one entry again, as in the same undo step.
        if !page_edit {
            self.combine_join_parts();
        }
    }

    fn combine_apply_once(&mut self, action: RowAction) {
        // The trash and Delete act on the selected pages while there are any.
        let action = match action {
            // (Files picked come first: the two are never both picked, but if they ever were, the
            // files the user sees picked are what goes.)
            RowAction::Remove if self.combine_tab.selected.is_empty() && !self.combine_tab.selected_pages.is_empty() => RowAction::RemovePages(None),
            a => a,
        };
        // Page edits work on the list as parts.
        let Some(action) = self.combine_page_action(action) else { return };
        // A file's card clicked (or the list's keys): the files are selected, no page.
        if matches!(action, RowAction::Click(..) | RowAction::SelectAll | RowAction::Step { .. }) {
            self.combine_tab.selected_pages.clear();
        }
        match action {
            RowAction::Click(..) | RowAction::SelectAll | RowAction::Step { .. } | RowAction::Unlock(..) => self.combine_select_action(action),
            RowAction::MoveUp
            | RowAction::MoveDown
            | RowAction::Remove
            | RowAction::RemoveFile(_)
            | RowAction::Drop { .. }
            | RowAction::DropAt { .. }
            | RowAction::Sort(_) => {
                self.combine_order_action(action);
            }
            // (Page edits were done above.)
            RowAction::ClickPage(..) | RowAction::MovePages { .. } | RowAction::RemovePages(_) => {}
            _ => self.combine_preview_action(action),
        }
    }

    /// Page edits work on the list as parts. Returns the action back when it is not one.
    fn combine_page_action(&mut self, action: RowAction) -> Option<RowAction> {
        match action {
            RowAction::ClickPage(id, nth, m) => {
                self.combine_click_page(id, nth, m);
                return None;
            }
            RowAction::MovePages { page, revision, gap } => {
                if revision != self.combine_tab.revision {
                    return None;
                }
                let moving =
                    if self.combine_tab.selected_pages.contains(&page) { self.combine_tab.selected_pages.clone() } else { BTreeSet::from([page]) };
                let parts = self.combine_parts();
                let next = &mut self.combine_tab.next_id;
                let mut new_id = || {
                    let id = *next;
                    *next = next.wrapping_add(1);
                    id
                };
                if let Some(moved) = moved_pages(&parts, &moving, gap, &mut new_id) {
                    self.combine_set_parts(moved);
                }
                return None;
            }
            RowAction::RemovePages(one) => {
                let removing = match one {
                    Some(page) => BTreeSet::from([page]),
                    None => self.combine_tab.selected_pages.clone(),
                };
                if removing.is_empty() {
                    return None;
                }
                let parts = self.combine_parts();
                let next = &mut self.combine_tab.next_id;
                let mut new_id = || {
                    let id = *next;
                    *next = next.wrapping_add(1);
                    id
                };
                // (Nothing to take out: no undo step, the redo history kept.)
                if let Some(left) = removed_pages(&parts, &removing, &mut new_id) {
                    self.combine_set_parts(left);
                }
                return None;
            }
            _ => {}
        }
        Some(action)
    }

    /// Selecting files, and asking for the passwords of locked ones.
    fn combine_select_action(&mut self, action: RowAction) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let n = ids.len();
        let index_of = |id: Option<u64>| id.and_then(|id| ids.iter().position(|x| *x == id));
        let tab = &mut self.combine_tab;
        match action {
            RowAction::Click(i, m) => {
                let Some(&id) = ids.get(i) else { return };
                if m.shift {
                    let from = index_of(tab.anchor).unwrap_or(i);
                    let range = ids.get(from.min(i)..=from.max(i)).unwrap_or_default();
                    if !m.command {
                        tab.selected.clear();
                    }
                    tab.selected.extend(range.iter().copied());
                    tab.cursor = Some(id);
                } else if m.command {
                    if !tab.selected.remove(&id) {
                        tab.selected.insert(id);
                    }
                    (tab.anchor, tab.cursor) = (Some(id), Some(id));
                } else {
                    tab.selected = BTreeSet::from([id]);
                    (tab.anchor, tab.cursor) = (Some(id), Some(id));
                }
            }
            RowAction::SelectAll => {
                tab.selected = ids.iter().copied().collect();
            }
            RowAction::Step { down, by, extend } => {
                // With files shown as their pages, Up and Down go a row of cards (from the last
                // of a file's cards down, from its first up) to the file there.
                let places = &tab.grid_places;
                let rows_of_cards = by > 1 && places.len() > n;
                let next = match index_of(tab.cursor) {
                    Some(c) if rows_of_cards => {
                        let from = if down { places.iter().rposition(|f| *f == c) } else { places.iter().position(|f| *f == c) };
                        from.and_then(|at| places.get(step_to(at, places.len(), by, down)).copied()).unwrap_or(c)
                    }
                    Some(c) => step_to(c, n, by, down),
                    None if down => 0,
                    None => n.saturating_sub(1),
                };
                let Some(&id) = ids.get(next) else { return };
                if extend {
                    let from = index_of(tab.anchor).unwrap_or(next);
                    tab.selected = ids.get(from.min(next)..=from.max(next)).unwrap_or_default().iter().copied().collect();
                } else {
                    tab.selected = BTreeSet::from([id]);
                    tab.anchor = Some(id);
                }
                tab.cursor = Some(id);
                tab.reveal = Some(id);
            }
            RowAction::Unlock(row, at) => {
                let locked: BTreeSet<u64> = self.combine_draft.iter().filter(|f| f.lock.is_some()).map(|f| f.id).collect();
                let rows: Vec<u64> = match row.and_then(|i| ids.get(i).copied()) {
                    // A selected row brings the rest of the selection.
                    Some(id) if tab.selected.contains(&id) => {
                        ids.iter().copied().filter(|x| tab.selected.contains(x) && locked.contains(x)).collect()
                    }
                    Some(id) => {
                        tab.selected = BTreeSet::from([id]);
                        (tab.anchor, tab.cursor) = (Some(id), Some(id));
                        vec![id]
                    }
                    None => ids.iter().copied().filter(|x| tab.selected.contains(x) && locked.contains(x)).collect(),
                };
                self.combine_ask_password(rows, at);
            }
            _ => {}
        }
    }

    /// Moving, removing, dropping and sorting files.
    fn combine_order_action(&mut self, action: RowAction) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let tab = &mut self.combine_tab;
        match action {
            RowAction::MoveUp | RowAction::MoveDown => {
                let up = matches!(action, RowAction::MoveUp);
                let selected: Vec<bool> = ids.iter().map(|id| tab.selected.contains(id)).collect();
                let movable =
                    if up { selected.iter().skip_while(|s| **s).any(|s| *s) } else { selected.iter().rev().skip_while(|s| **s).any(|s| *s) };
                if !movable {
                    return;
                }
                self.combine_record();
                let list = &mut self.combine_draft;
                let sel = &self.combine_tab.selected;
                // Each selected file steps past its unselected neighbour; blocks move together.
                if up {
                    for i in 1..list.len() {
                        if sel.contains(&list[i].id) && !sel.contains(&list[i - 1].id) {
                            list.swap(i, i - 1);
                        }
                    }
                } else {
                    for i in (0..list.len().saturating_sub(1)).rev() {
                        if sel.contains(&list[i].id) && !sel.contains(&list[i + 1].id) {
                            list.swap(i, i + 1);
                        }
                    }
                }
                self.combine_tab.sort = None;
            }
            RowAction::Remove => {
                let Some(first) = ids.iter().position(|id| tab.selected.contains(id)) else { return };
                self.combine_record();
                let sel = std::mem::take(&mut self.combine_tab.selected);
                self.combine_draft.retain(|f| !sel.contains(&f.id));
                // The file that took the first removed one's place is selected next.
                let next = self.combine_draft.get(first).or(self.combine_draft.last()).map(|f| f.id);
                self.combine_tab.selected = next.into_iter().collect();
                (self.combine_tab.anchor, self.combine_tab.cursor) = (next, next);
            }
            RowAction::RemoveFile(id) => {
                if !ids.contains(&id) {
                    return;
                }
                self.combine_record();
                self.combine_draft.retain(|f| f.id != id);
                // The rest of the selection stays as it was.
                self.combine_tab.selected.remove(&id);
                if self.combine_tab.anchor == Some(id) {
                    self.combine_tab.anchor = None;
                }
                if self.combine_tab.cursor == Some(id) {
                    self.combine_tab.cursor = None;
                }
            }
            RowAction::Drop { from, to } => {
                let (Some(&dragged), Some(&target)) = (ids.get(from), ids.get(to)) else { return };
                // Dragging a selected file takes the whole selection along.
                let moving: BTreeSet<u64> = if tab.selected.contains(&dragged) { tab.selected.clone() } else { BTreeSet::from([dragged]) };
                // Dropped on the selection itself: nothing to do.
                if moving.contains(&target) {
                    return;
                }
                // Below the row when dragged down, above it when dragged up.
                self.combine_move_to_gap(moving, if from < to { to + 1 } else { to });
            }
            RowAction::DropAt { file, revision, gap } => {
                // The list changed under the drag (e.g. undo): the gap no longer means the same.
                if revision != tab.revision || !ids.contains(&file) {
                    return;
                }
                let moving: BTreeSet<u64> = if tab.selected.contains(&file) { tab.selected.clone() } else { BTreeSet::from([file]) };
                self.combine_move_to_gap(moving, gap);
            }
            RowAction::Sort(key) => {
                let ascending = !matches!(tab.sort, Some((k, true)) if k == key);
                self.combine_record();
                self.combine_sort(key, ascending);
            }
            _ => {}
        }
    }

    /// Previewing and expanding files.
    fn combine_preview_action(&mut self, action: RowAction) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let tab = &mut self.combine_tab;
        match action {
            RowAction::Preview(id) => {
                if ids.contains(&id) {
                    self.combine_tab.preview = Some(Preview { file: id, at: 0, page: None, pages: None });
                }
            }
            RowAction::PreviewAt(id, at) => {
                if ids.contains(&id) {
                    self.combine_tab.preview = Some(Preview { file: id, at, page: None, pages: None });
                }
            }
            RowAction::Expand(id, on) => {
                if on {
                    if self.combine_draft.iter_mut().any(|f| f.id == id && expandable(f)) {
                        self.combine_tab.expanded.insert(id);
                    }
                } else {
                    self.combine_tab.expanded.remove(&id);
                    // Its card may be far from where its pages were: brought into view.
                    self.combine_tab.reveal = Some(id);
                }
            }
            RowAction::ExpandAll(on) => {
                self.combine_tab.expanded =
                    if on { self.combine_draft.iter_mut().filter_map(|f| expandable(f).then_some(f.id)).collect() } else { BTreeSet::new() };
                if !on {
                    self.combine_tab.reveal = self.combine_tab.cursor;
                }
            }
            RowAction::PreviewSelected => {
                // The file the keyboard is on, or the first selected; with pages selected, the
                // page card that has the keyboard, or the first selected page.
                let first_page = tab.selected_pages.iter().min_by_key(|(id, nth)| (ids.iter().position(|x| x == id), *nth)).copied();
                let page = tab.card_focus_at.filter(|p| tab.selected_pages.contains(p)).or(first_page);
                let id = tab
                    .cursor
                    .filter(|c| tab.selected.contains(c))
                    .or_else(|| ids.iter().copied().find(|id| tab.selected.contains(id)))
                    .or(page.map(|(id, _)| id));
                // Only what its card would offer a magnifier for (unlocked, readable, measured).
                let shows = |id: u64| self.combine_draft.iter().any(|f| f.id == id && f.lock.is_none() && f.problem.is_none() && !f.sizes.is_empty());
                if let Some(id) = id.filter(|id| shows(*id)) {
                    // From the page card that has the keyboard (or the selected page), if it's one
                    // of this file's.
                    let at = self.combine_tab.card_focus_at.or(page).filter(|(file, _)| *file == id).map_or(0, |(_, nth)| nth);
                    self.combine_tab.preview = Some(Preview { file: id, at, page: None, pages: None });
                }
            }
            _ => {}
        }
    }
}
