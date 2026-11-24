use super::right::PanelResult;
use super::{Nav, PdfCraftApp};
use craft::behavior::GesturePhase;
use craft::keymap::CommandRef;
use craft::pages::ui::{PageThumbnails, ThumbnailEvent, moved_page};
use craft::ui::contract::{Props, Show, ShowContainer};
use craft::ui::display::Text;
use craft::ui::menu::{Menu, MenuItem, PanelRow};
use craft::ui::style::DataColor;

/// The Pages panel's tools menu rows: insert a blank page or pages from a file, rotate, extract and delete. Each
/// runs its command on the selected pages (the current page when none is), so it does what Pages ▸ … does.
pub(super) fn tools_menu(_app: &PdfCraftApp) -> Vec<PanelRow> {
    // The commands are enabled by the catalog's own gate (`doc.allows_assembly()`, no dialog).
    ["page.insert_blank", "page.insert", "page.rotate_ccw", "page.rotate", "page.extract", "page.delete"]
        .into_iter()
        .map(|id| PanelRow::Command(CommandRef::new(id)))
        .collect()
}

/// The page thumbnails. A click goes to the page; ⌘/Ctrl-click and ⇧-click pick several pages (the selection page
/// commands and Print act on) without moving the document. The context menu offers Extract, Cut, Copy and Paste on
/// the picked pages; dragging moves them to the gap they are dropped on.
pub(super) fn body(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> PanelResult {
    let mut result = PanelResult::default();
    let Some((index, id)) = app.active_ids() else { return result };
    let modal = app.modal_open() || app.palette_open;
    let Some(doc) = app.session.get(id) else { return result };
    let editable = doc.allows_assembly() && doc.read_only_reason.is_none() && !modal;
    let count = doc.info.pages.len();
    let Some(view) = app.views.get_mut(index) else { return result };

    if view.selected.items.len() > 1 {
        let count = view.selected.items.len().to_string();
        Text::new(crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &count)])).show(ui);
    }

    let source = crate::canvas::pictures::DocPages::new(&app.session, id);
    let before = view.thumbnail_zoom();
    let mut zoom = f64::from(before);
    let current = view.current();
    let grid_id = crate::canvas::organize::pages_panel_grid_id(view);
    let dragging = craft::ui::runtime::drag::active::<craft::ui::contract::ListDrag<usize>>(ui.ctx()).is_some_and(|drag| drag.list == grid_id);
    let reveal = (!dragging && (view.panel_followed != Some(current) || view.panel_zoom_followed != before)).then_some(current);
    view.panel_followed = Some(current);
    view.panel_zoom_followed = before;
    let output = PageThumbnails::new(tl!("Pages"), &source, &mut view.selected, current, DataColor::new(egui::Color32::WHITE))
        .id(grid_id)
        .picks_beside_current(true)
        .editable(editable)
        .reveal(reveal)
        .size(&mut zoom)
        .show(ui);
    let zoom = zoom as f32;
    if zoom != before {
        view.set_thumbnail_zoom(zoom);
    }

    view.pages_focus_id = Some(output.response.id);
    view.pages_panel = editable.then_some(output.response.rect);
    view.panel_gap = if editable { output.file_gap } else { None };

    let menu_id = output.response.id.with("page-menu");
    let mut target: Option<usize> = ui.data(|d| d.get_temp::<Option<usize>>(menu_id)).flatten();
    for event in output.events {
        match event {
            ThumbnailEvent::Go(page) => result.nav = Some(Nav::Page(page)),
            ThumbnailEvent::Reorder(edit) => {
                let ended = matches!(edit.phase(), Some(GesturePhase::Commit) | None);
                let Some(mv) = edit.value().filter(|_| ended && editable) else { continue };
                let mut pages = mv.keys.clone();
                pages.sort_unstable();
                pages.dedup();
                let Some(&first) = pages.first() else { continue };
                let contiguous = pages.windows(2).all(|w| w[1] == w[0] + 1);
                // A drop where the pages already are changes nothing.
                if mv.copy || (contiguous && mv.to == first) {
                    continue;
                }
                let to = mv.to;
                view.select_pages(&(to..to + pages.len()).collect::<Vec<_>>());
                view.view.current = moved_page(mv, current, count);
                result.edit = Some(pdfcraft_engine::Edit::MovePages { pages, to });
            }
            ThumbnailEvent::Delete(_) => {
                // The menu bar owns keys while its menu is open, even over the thumbnails.
                if editable && !craft::ui::menu::bar::is_open(ui.ctx()) {
                    result.edit = delete_pages(view, count);
                }
            }
            ThumbnailEvent::ContextMenu(request) => target = Some(request.key),
            ThumbnailEvent::FilesDropped { insert, files, .. } => {
                view.panel_gap = Some(insert);
                view.dropped_files = Some((insert, files));
            }
            _ => {}
        }
    }

    if let Some(page) = target {
        let menu = Menu::context(tl!("Pages"), &output.response).show(ui, |ui, _| {
            if MenuItem::new(tl!("Extract pages")).show(ui).ran {
                view.pending_action = Some(crate::canvas::ViewAction::Extract);
            }
            if MenuItem::new(tl!("Split document…")).show(ui).ran {
                if view.selected.items.is_empty() {
                    view.select_pages(&[page]);
                }
                view.pending_action = Some(crate::canvas::ViewAction::SplitBeforeSelected);
            }
            if MenuItem::new(tl!("Replace pages…")).enabled(editable).show(ui).ran {
                view.pending_action = Some(crate::canvas::ViewAction::ReplacePages);
            }
            if MenuItem::new(tl!("Cut")).enabled(editable).show(ui).ran {
                view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: true });
            }
            if MenuItem::new(tl!("Copy")).show(ui).ran {
                view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: false });
            }
            if MenuItem::new(tl!("Paste after")).enabled(editable).show(ui).ran {
                view.pending_action = Some(crate::canvas::ViewAction::PastePages);
            }
            // Never every page: a document keeps at least one.
            let some_left = view.target_pages().len() < count;
            if MenuItem::new(tl!("Delete pages")).enabled(editable && some_left).show(ui).ran {
                result.edit = delete_pages(view, count);
            }
        });
        if menu.output.closed.is_some() {
            target = None;
        }
    }
    // PLAN: the open menu's target is egui temp data; the panel's component memory would hold it (as `history.rs`
    // does) once pdf-chrome-w13 passes the body its `BodyCx`.
    ui.data_mut(|d| d.insert_temp(menu_id, target));
    result
}

/// Queue deleting the selection (or the current page) of a document of `n` pages, unless that
/// would leave no page. The selection goes; the current page stays on the page it showed, or on
/// the page that follows the deleted ones. The edit is returned for the caller to queue.
fn delete_pages(view: &mut crate::DocView, n: usize) -> Option<pdfcraft_engine::Edit> {
    let mut pages = view.target_pages();
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() || pages.len() >= n {
        return None;
    }
    // Deleted pages before it shift it up; if it was deleted itself, the page after the deleted
    // run lands where it was (or the last page, when the deletion ran to the end).
    let current = view.current();
    let before = pages.iter().filter(|p| **p < current).count();
    view.view.current = current.saturating_sub(before).min(n.saturating_sub(pages.len() + 1));
    view.clear_page_selection(None);
    Some(pdfcraft_engine::Edit::DeletePages { pages })
}
