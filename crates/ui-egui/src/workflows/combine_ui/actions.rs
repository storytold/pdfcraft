use super::BTreeSet;
use super::files::{CombineKey, Preview, RowAction};
use super::parts::{moved_pages, removed_pages};
use super::view::expandable;
use crate::PdfCraftApp;
use craft::behavior::selection::Selection;

impl PdfCraftApp {
    pub(crate) fn combine_apply(&mut self, action: RowAction) {
        self.combine_apply_once(action);
        // One kind of selection at a time: the latest pick's kind wins, so the trash never takes
        // pages (or files) the user no longer means.
        self.combine_keep_one_kind();
    }

    fn combine_apply_once(&mut self, action: RowAction) {
        // The trash and Delete act on the selected pages while no file is selected.
        let action = match action {
            RowAction::Remove if self.combine_file_ids().is_empty() && !self.combine_page_keys().is_empty() => RowAction::RemovePages(None),
            a => a,
        };
        // Page edits work on the list as parts.
        let Some(action) = self.combine_page_action(action) else { return };
        match action {
            RowAction::MoveUp
            | RowAction::MoveDown
            | RowAction::Remove
            | RowAction::RemoveFile(_)
            | RowAction::DropAt { .. }
            | RowAction::Sort(_)
            | RowAction::Unlock(_) => {
                self.combine_order_action(action);
            }
            // (Page edits were done above.)
            RowAction::MovePages { .. } | RowAction::RemovePages(_) => {}
            _ => self.combine_preview_action(action),
        }
    }

    /// Page edits work on the list as parts. Returns the action back when it is not one.
    fn combine_page_action(&mut self, action: RowAction) -> Option<RowAction> {
        match action {
            RowAction::MovePages { page, revision, gap } => {
                if revision != self.combine_tab.revision {
                    return None;
                }
                let picked = self.combine_page_keys();
                let moving = if picked.contains(&page) { picked } else { BTreeSet::from([page]) };
                let parts = self.combine_parts();
                let next = &mut self.combine_tab.next_id;
                let mut new_id = || {
                    let id = *next;
                    *next = next.wrapping_add(1);
                    id
                };
                if let Some(moved) = moved_pages(&parts, &moving, gap, &mut new_id) {
                    self.combine_set_parts(moved, tl!("Pages"));
                }
                return None;
            }
            RowAction::RemovePages(one) => {
                let removing = match one {
                    Some(page) => BTreeSet::from([page]),
                    None => self.combine_page_keys(),
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
                    self.combine_set_parts(left, tl!("Remove page"));
                }
                return None;
            }
            _ => {}
        }
        Some(action)
    }

    /// Moving, removing, dropping, sorting files, and asking for the passwords of locked ones.
    fn combine_order_action(&mut self, action: RowAction) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let selected = self.combine_file_ids();
        match action {
            RowAction::MoveUp | RowAction::MoveDown => {
                let up = matches!(action, RowAction::MoveUp);
                let flags: Vec<bool> = ids.iter().map(|id| selected.contains(id)).collect();
                let movable = if up { flags.iter().skip_while(|s| **s).any(|s| *s) } else { flags.iter().rev().skip_while(|s| **s).any(|s| *s) };
                if !movable {
                    return;
                }
                let label = if up { tl!("Move up") } else { tl!("Move down") };
                self.combine_edit(label, |app| {
                    let list = &mut app.combine_draft;
                    let sel = &selected;
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
                    app.combine_tab.sort = None;
                    app.combine_join_parts();
                });
            }
            RowAction::Remove => {
                let Some(first) = ids.iter().position(|id| selected.contains(id)) else { return };
                self.combine_edit(tl!("Remove"), |app| {
                    app.combine_draft.retain(|f| !selected.contains(&f.id));
                    // The file that took the first removed one's place is selected next.
                    let next = app.combine_draft.get(first).or(app.combine_draft.last()).map(|f| CombineKey::File(f.id));
                    app.combine_tab.selection = Selection::new(next.into_iter().collect(), next, next);
                    app.combine_join_parts();
                });
            }
            RowAction::RemoveFile(id) => {
                if !ids.contains(&id) {
                    return;
                }
                self.combine_edit(tl!("Remove"), |app| {
                    app.combine_draft.retain(|f| f.id != id);
                    // The rest of the selection stays as it was.
                    let sel = &mut app.combine_tab.selection;
                    sel.items.retain(|k| k.entry() != id);
                    sel.lead = sel.lead.filter(|k| k.entry() != id);
                    sel.anchor = sel.anchor.filter(|k| k.entry() != id);
                    app.combine_join_parts();
                });
            }
            RowAction::DropAt { file, revision, gap } => {
                // The list changed under the drag (e.g. undo): the gap no longer means the same.
                if revision != self.combine_tab.revision || !ids.contains(&file) {
                    return;
                }
                let moving: BTreeSet<u64> = if selected.contains(&file) { selected } else { BTreeSet::from([file]) };
                self.combine_move_to_gap(moving, gap);
            }
            RowAction::Sort(key) => {
                let ascending = !matches!(self.combine_tab.sort, Some((k, true)) if k == key);
                self.combine_edit(key.label(), |app| {
                    app.combine_sort(key, ascending);
                    app.combine_join_parts();
                });
            }
            RowAction::Unlock(row) => {
                let locked: BTreeSet<u64> = self.combine_draft.iter().filter(|f| f.lock.is_some()).map(|f| f.id).collect();
                let of_selection = || ids.iter().copied().filter(|x| selected.contains(x) && locked.contains(x)).collect::<Vec<u64>>();
                let rows: Vec<u64> = match row {
                    Some(id) if !ids.contains(&id) => return,
                    // A selected row brings the rest of the selection.
                    Some(id) if selected.contains(&id) => of_selection(),
                    Some(id) => {
                        self.combine_tab.selection = Selection::single(CombineKey::File(id));
                        vec![id]
                    }
                    None => of_selection(),
                };
                self.combine_ask_password(rows);
            }
            _ => {}
        }
    }

    /// Previewing and expanding files.
    fn combine_preview_action(&mut self, action: RowAction) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
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
                    // Pages stay picked only while they show.
                    self.combine_drop_page_keys(Some(id));
                }
            }
            RowAction::ExpandAll(on) => {
                self.combine_tab.expanded =
                    if on { self.combine_draft.iter_mut().filter_map(|f| expandable(f).then_some(f.id)).collect() } else { BTreeSet::new() };
                if !on {
                    self.combine_drop_page_keys(None);
                }
            }
            RowAction::PreviewSelected => {
                // The lead (file or page card) if it is selected, or the first selected file in
                // the list's order.
                let sel = &self.combine_tab.selection;
                let first = ids.iter().find_map(|id| sel.items.iter().find(|k| k.entry() == *id)).copied();
                let Some(key) = sel.lead.filter(|k| sel.items.contains(k)).or(first) else { return };
                let (id, at) = match key {
                    CombineKey::File(id) => (id, 0),
                    CombineKey::Page(id, nth) => (id, nth),
                };
                // Only what its card would offer a magnifier for (unlocked, readable, measured).
                let shows = self.combine_draft.iter().any(|f| f.id == id && f.lock.is_none() && f.problem.is_none() && !f.sizes.is_empty());
                if shows {
                    self.combine_tab.preview = Some(Preview { file: id, at, page: None, pages: None });
                }
            }
            _ => {}
        }
    }
}
