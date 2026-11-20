//! The Destinations panel (Acrobat: View ▸ Show/Hide ▸ Side panels ▸ Destinations): the
//! document's named destinations (ISO 32000-2 §12.3.2.4), sorted by name or by page with the
//! Name and Page labels. A click goes to one; its menu goes to it, sends it to the view on screen
//! (Set destination), renames or deletes it; the panel's options make a new one at the view on
//! screen, which starts in rename mode as a new bookmark does. Renaming and deleting keep the
//! document's own links to it working (`pdfcraft_organize::rename_named_destination`).

use std::sync::Arc;

use egui::{Align, Layout, Sense, vec2};
use pdfcraft_engine::{DestinationRow, DocId, Edit};
use pdfcraft_render::DocInfo;

use super::Nav;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons};

/// What the Destinations panel asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum DestAction {
    /// Destinations panel ▸ New destination, at the view on screen.
    New,
    StartRename(String),
    /// Rename the first to the second.
    Rename(String, String),
    /// Send it to the view on screen.
    Set(String),
    Delete(String),
}

/// The panel's state between frames: the sort, the destination being renamed and the list, read
/// again only when the document changes.
#[derive(Default)]
pub struct DestinationsView {
    /// Sorted by page (Acrobat: click the Page label) instead of by name.
    pub by_page: bool,
    /// A destination being renamed: (its document, its name, the text so far).
    pub rename: Option<(DocId, String, String)>,
    /// The list, for a document at an edit generation.
    rows: Option<(DocId, u64, Arc<Vec<DestinationRow>>)>,
}

impl DestinationsView {
    /// The named destinations of document `id`, read once per change to it. Empty when its
    /// structure could not be read for editing.
    pub fn rows(&mut self, session: &pdfcraft_engine::Session, id: DocId) -> Arc<Vec<DestinationRow>> {
        let generation = session.get(id).map_or(0, |d| d.edit_generation());
        if let Some((doc, at, rows)) = &self.rows
            && *doc == id
            && *at == generation
        {
            return rows.clone();
        }
        let rows = Arc::new(session.named_destinations(id).unwrap_or_default());
        self.rows = Some((id, generation, rows.clone()));
        rows
    }

    /// End a rename whose destination is gone: `rows` are the destinations of document `id`, and
    /// the rename was started in another document, or for a name none of them has any more (an
    /// undo, a tool or another tab took it away). Left alone, the field would open again on a
    /// destination given that name later, or on one of that name in another document.
    fn drop_stale_rename(&mut self, id: DocId, rows: &[DestinationRow]) {
        if self.rename.as_ref().is_some_and(|(doc, name, _)| *doc != id || !rows.iter().any(|r| r.name == *name)) {
            self.rename = None;
        }
    }
}

/// The rows in the order the panel shows them: by name, or by page (then name), with those that
/// go to no page of this document last.
fn sorted(rows: &[DestinationRow], by_page: bool) -> Vec<&DestinationRow> {
    let mut out: Vec<&DestinationRow> = rows.iter().collect();
    if by_page {
        out.sort_by(|a, b| a.page.unwrap_or(usize::MAX).cmp(&b.page.unwrap_or(usize::MAX)).then_with(|| a.name.cmp(&b.name)));
    }
    out
}

/// What [`panel`] draws from and writes to.
pub(super) struct DestinationsPanel<'a> {
    pub(super) theme: &'a Tokens,
    /// The document `rows` are the destinations of.
    pub(super) id: DocId,
    pub(super) info: &'a DocInfo,
    pub(super) rows: &'a [DestinationRow],
    pub(super) state: &'a mut DestinationsView,
    pub(super) editable: bool,
    pub(super) nav: &'a mut Option<Nav>,
    pub(super) action: &'a mut Option<DestAction>,
}

/// Draw the panel's list: the Name and Page labels (each sorts by itself), then a row per
/// destination.
pub(super) fn panel(ui: &mut egui::Ui, p: DestinationsPanel<'_>) {
    let DestinationsPanel { theme: t, id: doc, info, rows, state, editable, nav, action } = p;
    state.drop_stale_rename(doc, rows);
    if rows.is_empty() {
        super::right::empty(ui, t, "navigation", "This document has no named destinations.");
        return;
    }
    let current = state.by_page;
    let mut sort = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label = |text: &str| egui::Button::new(egui::RichText::new(text).font(theme::semibold(12.0)).color(t.text_muted)).frame(false);
        // The sort's own label is marked with a small chevron after it. It is an icon: the
        // interface fonts have no triangle glyph, and a glyph no font has is a panic.
        let mark = |ui: &mut egui::Ui| {
            let (_, rect) = ui.allocate_space(vec2(12.0, 12.0));
            icons::paint(ui, rect, "chevron-down", 12.0, t.text_muted);
        };
        if ui.add(label(tl!("Name"))).on_hover_text(tl!("Sort by name")).clicked() {
            sort = Some(false);
        }
        if !current {
            mark(ui);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Right to left: the mark comes first so that it follows the label.
            if current {
                mark(ui);
            }
            if ui.add(label(tl!("Page"))).on_hover_text(tl!("Sort by page")).clicked() {
                sort = Some(true);
            }
        });
    });
    if let Some(by_page) = sort {
        state.by_page = by_page;
    }
    for row in sorted(rows, state.by_page) {
        let id = ui.scope_id().with(("destination", &row.name));
        if let Some((_, old, text)) = state.rename.as_mut()
            && *old == row.name
        {
            // Inline rename: Enter (or clicking away) commits, Escape cancels.
            let (mut commit, mut cancel) = (false, false);
            ui.horizontal(|ui| {
                crate::widgets::begin_dialog(ui);
                let resp = super::bookmarks::rename_field(ui, text, id.with("rename"));
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                } else if resp.lost_focus() {
                    commit = true;
                }
            });
            if cancel || commit {
                let new = text.trim();
                let renamed = (commit && !new.is_empty() && new != row.name).then(|| new.to_string());
                state.rename = None;
                if let Some(new) = renamed {
                    *action = Some(DestAction::Rename(row.name.clone(), new));
                }
            }
            continue;
        }
        let size = vec2(ui.available_width(), 28.0);
        // A document can name a hundred thousand places: a row scrolled out of sight only keeps
        // its place in the list.
        if !ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, size)) {
            ui.allocate_space(size);
            continue;
        }
        let page = row.page.and_then(|p| info.pages.get(p).map(|page| (p, page)));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &row.name));
        theme::Press::track(ui, &resp).wash(ui, rect, 6, false);
        let label = page.map(|(_, page)| page.label.as_str()).unwrap_or("\u{2013}");
        let label_galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), theme::regular(11.0), t.text_faint));
        let label_w = label_galley.size().x;
        ui.painter().galley(rect.right_center() - vec2(6.0 + label_w, label_galley.size().y / 2.0), label_galley, t.text_faint);
        crate::widgets::paint_left(
            ui,
            rect.left_center() + vec2(8.0, 0.0),
            &row.name,
            theme::regular(13.0),
            t.text,
            (rect.width() - label_w - 24.0).max(0.0),
        );
        let go = |nav: &mut Option<Nav>| {
            if let Some((p, _)) = page {
                *nav = Some(Nav::Dest(p, row.view));
            }
        };
        if theme::hand(resp.clone()).clicked() {
            go(nav);
        }
        resp.context_menu(|ui| {
            if ui.add_enabled(page.is_some(), egui::Button::new(tl!("Go to destination"))).clicked() {
                go(nav);
                ui.close();
            }
            if editable {
                for (label, a) in [
                    (tl!("Set destination"), DestAction::Set(row.name.clone())),
                    (tl!("Rename"), DestAction::StartRename(row.name.clone())),
                    (tl!("Delete"), DestAction::Delete(row.name.clone())),
                ] {
                    if ui.button(label).clicked() {
                        *action = Some(a);
                        ui.close();
                    }
                }
            }
        });
    }
}

/// The panel header's options button: New destination.
pub(super) fn options(ui: &mut egui::Ui, action: &mut Option<DestAction>) {
    let more = icons::button(ui, "ellipsis", 26.0, false, tl!("Destination options"));
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(200.0);
        if ui.button(tl!("New destination")).clicked() {
            *action = Some(DestAction::New);
            ui.close();
        }
    });
}

/// "Untitled", or "Untitled 2", "Untitled 3"… when that is taken: a new destination's first name.
fn untitled(rows: &[DestinationRow]) -> String {
    let taken = |name: &str| rows.iter().any(|r| r.name == name);
    let mut name = "Untitled".to_string();
    let mut n = 2;
    while taken(&name) {
        name = format!("Untitled {n}");
        n += 1;
    }
    name
}

impl PdfCraftApp {
    /// Carry out a Destinations-panel action; the changes are undoable edits.
    pub fn destination_action(&mut self, action: DestAction) {
        let Some((i, id)) = self.active_ids() else { return };
        // Where the reader is: the selection's corner, else the top of the window, as a new
        // bookmark goes.
        let here = |app: &Self| {
            let current = app.views.get(i).map_or(0, |v| v.current);
            let (page, [left, top]) =
                app.session.get(id).and_then(|d| app.views.get(i)?.bookmark_position(&d.info)).unwrap_or((current, [None, None]));
            (page, left.map(f64::from), top.map(f64::from))
        };
        match action {
            DestAction::New => {
                let name = untitled(&self.destinations.rows(&self.session, id));
                let (page, left, top) = here(self);
                if self.apply_edit(Edit::AddDestination { name: name.clone(), page, left, top }) {
                    // Like Acrobat: the new destination is named at once.
                    self.destinations.rename = Some((id, name.clone(), name));
                }
            }
            DestAction::StartRename(name) => self.destinations.rename = Some((id, name.clone(), name)),
            DestAction::Rename(name, new_name) => {
                self.apply_edit(Edit::RenameDestination { name, new_name });
            }
            DestAction::Set(name) => {
                let (page, left, top) = here(self);
                self.apply_edit(Edit::SetDestination { name, page, left, top });
            }
            DestAction::Delete(name) => {
                self.apply_edit(Edit::DeleteDestination { name });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DestinationRow, DestinationsView, DocId, sorted, untitled};
    use pdfcraft_render::DestView;

    fn row(name: &str, page: Option<usize>) -> DestinationRow {
        DestinationRow { name: name.into(), page, view: DestView::Top }
    }

    #[test]
    fn by_page_sorts_by_page_then_name_with_pageless_ones_last() {
        let rows = [row("b", Some(2)), row("a", None), row("c", Some(0)), row("a2", Some(2))];
        let names = |by_page| sorted(&rows, by_page).iter().map(|r| r.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(true), ["c", "a2", "b", "a"]);
        // By name is the order the engine lists them in.
        assert_eq!(names(false), ["b", "a", "c", "a2"]);
    }

    #[test]
    fn a_rename_ends_when_its_destination_or_its_document_is_gone() {
        let renaming = || DestinationsView { rename: Some((DocId(1), "a".into(), "a2".into())), ..Default::default() };
        // Still there, among others: the rename goes on with the text typed so far.
        let mut view = renaming();
        view.drop_stale_rename(DocId(1), &[row("b", None), row("a", Some(0))]);
        assert_eq!(view.rename, Some((DocId(1), "a".into(), "a2".into())));
        // Deleted, or renamed by something else: no row has the name.
        let mut view = renaming();
        view.drop_stale_rename(DocId(1), &[row("b", None)]);
        assert_eq!(view.rename, None);
        // Another document with a destination of the same name does not take the rename over.
        let mut view = renaming();
        view.drop_stale_rename(DocId(2), &[row("a", Some(0))]);
        assert_eq!(view.rename, None);
    }

    #[test]
    fn a_new_destination_takes_the_first_free_untitled_name() {
        assert_eq!(untitled(&[]), "Untitled");
        assert_eq!(untitled(&[row("Untitled", None), row("Untitled 2", None), row("Untitled 4", None)]), "Untitled 3");
    }
}
