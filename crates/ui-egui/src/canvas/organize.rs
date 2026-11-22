use super::{DocView, Edit, Pos2, ViewAction};
use craft::behavior::{GesturePhase, IconId, MiddleButton, Selection};
use craft::keymap::CommandRef;
use craft::pages::ui::thumbnails::{ORGANIZE_ZOOM, PageThumbnails, ThumbnailArrangement, ThumbnailEvent};
use craft::theme::Variant;
use craft::ui::bars::{Bar, BarKind};
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::overlay::Anchor;
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::{Divider, Text};
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::style::DataColor;
use craft::ui::tile_grid::ThumbnailSlider;

/// What `organize_grid` is shown with: the `Ui` it fills and the app's own settings it reads and writes.
// PLAN: assumed this struct is the `cx` of the call `organize_grid(view, doc, cx)` (pdf-canvas-w8); it carries `app.organize_zoom` and
// `app.invert_middle_drag` because `view` is borrowed out of `app.views`.
pub(super) struct GridCx<'a> {
    pub ui: &'a mut egui::Ui,
    pub session: &'a pdfcraft_engine::Session,
    pub zoom: &'a mut f64,
    pub invert_middle: bool,
}

/// The tile actions in the order `ThumbnailEvent::Action::action` indexes them.
const ROTATE_CCW: usize = 0;
const ROTATE_CW: usize = 1;
const DELETE: usize = 2;

/// The grid's id: the press owner `PageThumbnails::id` takes and `cancel_page_drags` ends. One place builds it, so the
/// two cannot drift; the pane key outlives `document_replaced`.
fn organize_grid_id(view: &DocView) -> egui::Id {
    egui::Id::new(("organize-grid", view.view_key()))
}

/// The Pages panel grid's id, built the same way as [`organize_grid_id`].
pub(crate) fn pages_panel_grid_id(view: &DocView) -> egui::Id {
    egui::Id::new(("pages-panel-grid", view.view_key()))
}

impl DocView {
    /// Open Organize Pages on the page being read (issue #1910). An empty selection means the
    /// current page, so a selection left from earlier that does not include it is dropped, and
    /// the grid scrolls its cell into view. Every way into the grid goes through here; closing
    /// it is a plain `organize = false`.
    pub fn open_organize(&mut self) {
        self.organize = true;
        let current = self.view.current;
        if !self.selected.items.is_empty() && !self.selected.contains(&current) {
            self.clear_page_selection(Some(current));
        }
        self.grid_reveal = Some(current);
    }

    /// Pages an organize command acts on: the selection, or the current page.
    pub fn target_pages(&self) -> Vec<usize> {
        if self.selected.items.is_empty() {
            vec![self.view.current]
        } else {
            let mut pages = self.selected.items.clone();
            pages.sort_unstable();
            pages
        }
    }

    pub fn select_pages(&mut self, pages: &[usize]) {
        let mut items: Vec<usize> = pages.iter().copied().filter(|p| *p < self.page_count).collect();
        items.sort_unstable();
        items.dedup();
        let first = items.first().copied();
        self.selected = Selection::new(items, first, first);
        if let Some(first) = first {
            self.view.current = first;
        }
    }

    /// Drop the page selection; a later ⇧-click ranges from `anchor`.
    pub fn clear_page_selection(&mut self, anchor: Option<usize>) {
        self.selected = Selection::new(Vec::new(), None, anchor.filter(|a| *a < self.page_count));
    }

    /// Ends a page drag the Organize grid or the Pages panel holds open: its indices came from a document the edit,
    /// undo, redo or replacement has since changed.
    pub(crate) fn cancel_page_drags(&self, ctx: &egui::Context) {
        PageThumbnails::cancel_press(ctx, organize_grid_id(self));
        PageThumbnails::cancel_press(ctx, pages_panel_grid_id(self));
    }
}

/// The gap in the full page order that a move of `pages` (sorted) to `to` (counted without them) drops at.
fn gap_of(pages: &[usize], to: usize) -> usize {
    let mut gap = to;
    for p in pages {
        if *p < gap {
            gap += 1;
        } else {
            break;
        }
    }
    gap
}

/// A committed drag: a move, or with the copy modifier a duplicate at the gap.
fn commit_reorder(view: &mut DocView, mut pages: Vec<usize>, to: usize, copy: bool) {
    pages.sort_unstable();
    pages.dedup();
    let Some(first) = pages.first().copied() else { return };
    let count = pages.len();
    if copy {
        // The copies land at the gap as it is: nothing leaves its place.
        let gap = gap_of(&pages, to);
        view.session.pending_edit = Some(Edit::DuplicatePages { pages, at: Some(gap) });
        view.selected = Selection::new((gap..gap + count).collect(), Some(gap), Some(gap));
        return;
    }
    let contiguous = pages.windows(2).all(|w| w[1] == w[0] + 1);
    // A contiguous run dropped where it is moves nothing.
    if contiguous && to == first {
        return;
    }
    view.session.pending_edit = Some(Edit::MovePages { pages, to });
    view.selected = Selection::new((to..to + count).collect(), Some(to), Some(to));
}

/// The page menu's rows: Extract, Split, Replace, a rule, Cut, Copy, Paste after.
fn page_menu(ui: &mut egui::Ui, view: &mut DocView, editable: bool) {
    if MenuItem::new(tl!("Extract pages")).show(ui).ran {
        view.pending_action = Some(ViewAction::Extract);
    }
    if MenuItem::new(tl!("Split document…")).show(ui).ran {
        view.pending_action = Some(ViewAction::SplitBeforeSelected);
    }
    if MenuItem::new(tl!("Replace pages…")).enabled(editable).show(ui).ran {
        view.pending_action = Some(ViewAction::ReplacePages);
    }
    Divider::horizontal().show(ui);
    if MenuItem::new(tl!("Cut")).enabled(editable).show(ui).ran {
        view.pending_action = Some(ViewAction::CopyPages { cut: true });
    }
    if MenuItem::new(tl!("Copy")).show(ui).ran {
        view.pending_action = Some(ViewAction::CopyPages { cut: false });
    }
    if MenuItem::new(tl!("Paste after")).enabled(editable).show(ui).ran {
        view.pending_action = Some(ViewAction::PastePages);
    }
}

/// Organize pages: the page operations bar over a thumbnail grid (Acrobat's Organize Pages view). Click picks,
/// ⌘-click toggles, ⇧-click extends; double-click opens the page.
pub(super) fn organize_grid(view: &mut DocView, doc: &pdfcraft_engine::Document, cx: &mut GridCx<'_>) {
    let info = &doc.info;
    let editable = doc.allows_assembly();
    let dirty = doc.dirty;
    let n = info.pages.len();
    let targets = view.target_pages();
    let (first, last) = (targets.first().copied().unwrap_or(0), targets.last().copied().unwrap_or(0));
    let label = match view.selected.items.len() {
        0 => crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(view.view.current + 1).to_string()), ("n", &n.to_string())]),
        1 => tl!("1 page selected").to_string(),
        k => crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &k.to_string())]),
    };
    let (mut earlier, mut later, mut save, mut close) = (false, false, false, false);
    let zoom = &mut *cx.zoom;
    Bar::new(tl!("Organize pages"), BarKind::Tool)
        .trailing(|ui, _| {
            ThumbnailSlider::new(tl!("Page size"), Bind::bound(&mut *zoom), ORGANIZE_ZOOM).show(ui);
            // What the grid shows is what is saved.
            // PLAN: assumed `Props::tooltip(impl Into<Tooltip>)` and `TooltipSpec -> Tooltip`.
            let hint = craft::ui::contract::TooltipSpec::new(tl!("Save these pages as one PDF")).disabled_text(tl!("No changes to save"));
            save |= Button::new(tl!("Save pages")).variant(Variant::PRIMARY).enabled(dirty).tooltip(hint).show(ui).clicked();
            close |= Button::new(tl!("Close")).variant(Variant::GHOST).icon(IconId::new("x")).show(ui).clicked();
        })
        .show(cx.ui, |ui, _| {
            Text::new(label.clone()).show(ui);
            earlier = IconButton::new(IconId::new("chevron-left")).label(tl!("Move earlier")).enabled(editable && first > 0).show(ui).clicked();
            later = IconButton::new(IconId::new("chevron-right")).label(tl!("Move later")).enabled(editable && last + 1 < n).show(ui).clicked();
        });
    if earlier {
        view.session.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first - 1 });
    }
    if later {
        view.session.pending_edit = Some(Edit::MovePages { pages: targets, to: first + 1 });
    }
    if save {
        view.pending_action = Some(ViewAction::Save);
    }
    if close {
        view.organize = false;
    }

    let pages = super::pictures::DocPages::new(cx.session, view.id);
    let actions = [
        (IconId::new("rotate_ccw"), CommandRef::new("page.rotate_ccw")),
        (IconId::new("rotate_cw"), CommandRef::new("page.rotate")),
        (IconId::new("trash-2"), CommandRef::new("page.delete")),
    ];
    // Each action names the page it acts on, so a screen reader can tell one tile's buttons from the next.
    let action_label = |action: usize, page: usize| {
        let template = match action {
            ROTATE_CCW => tl!("Rotate page {label} counterclockwise"),
            ROTATE_CW => tl!("Rotate page {label} clockwise"),
            _ => tl!("Delete page {label}"),
        };
        let label = info.pages.get(page).map_or_else(|| (page + 1).to_string(), |p| p.label.clone());
        crate::i18n::fmt(template, &[("label", &label)]).into()
    };
    let insert_label = |at: usize| {
        match info.pages.get(at) {
            Some(p) => crate::i18n::fmt(tl!("Insert a file before page {label}"), &[("label", &p.label)]),
            None => tl!("Insert a file at the end").to_string(),
        }
        .into()
    };
    let middle = if cfg!(target_os = "linux") { MiddleButton::AutoScroll } else { MiddleButton::Pan };
    let current = view.view.current;
    let reveal = view.grid_reveal;
    if reveal.is_some() {
        cx.ui.ctx().data_mut(|data| data.insert_temp(organize_grid_id(view).with("reveal-drawn"), true));
    }
    let id = organize_grid_id(view);
    // PLAN: assumed `PageThumbnails::invert_middle(bool)` beside `middle_button` (pdf-canvas-w6).
    let out = PageThumbnails::new(tl!("Pages"), &pages, &mut view.selected, current, DataColor::new(egui::Color32::WHITE))
        .id(id)
        .arrangement(ThumbnailArrangement::Organize)
        .editable(editable)
        .size(&mut *cx.zoom)
        .actions(&actions)
        .action_label(&action_label)
        .insert_label(&insert_label)
        .middle_button(middle)
        .invert_middle(cx.invert_middle)
        .reveal(reveal)
        .show(cx.ui);
    view.middle_scrolling = out.auto_scrolling;

    // The menu is open from the request until a row runs or it is dismissed.
    // PLAN: assumed the open anchor needs a remembered slot; egui's temp data keyed by the grid stands in for it.
    let menu_id = id.with("menu");
    let mut opened = None;
    for event in out.events {
        match event {
            ThumbnailEvent::Go(page) => view.go_to_page(page),
            // A drag ends as a commit, a keyboard move as a set; proposals during the drag move nothing.
            ThumbnailEvent::Reorder(edit) if matches!(edit.phase(), Some(GesturePhase::Commit) | None) => {
                if let Some(mv) = edit.value() {
                    commit_reorder(view, mv.keys.clone(), mv.to, mv.copy);
                }
            }
            ThumbnailEvent::Delete(pages) => view.session.pending_edit = Some(Edit::DeletePages { pages }),
            ThumbnailEvent::Activate(page) => {
                view.organize = false;
                view.go_to_page(page);
            }
            ThumbnailEvent::ContextMenu(request) => opened = Some(request.at),
            ThumbnailEvent::Insert(at) => view.pending_action = Some(ViewAction::InsertFromFileAt(at)),
            ThumbnailEvent::FilesDropped { insert, files, .. } => {
                view.grid_gap = Some(insert);
                view.dropped_files = Some((insert, files));
            }
            ThumbnailEvent::Action { page, action, .. } => {
                view.session.pending_edit = match action {
                    ROTATE_CCW => Some(Edit::RotatePages { pages: vec![page], degrees: -90 }),
                    ROTATE_CW => Some(Edit::RotatePages { pages: vec![page], degrees: 90 }),
                    DELETE => Some(Edit::DeletePages { pages: vec![page] }),
                    _ => view.session.pending_edit.take(),
                };
            }
            _ => {}
        }
    }
    if opened.is_some() {
        cx.ui.data_mut(|d| d.insert_temp(menu_id, opened));
    }
    if let Some(at) = cx.ui.data(|d| d.get_temp::<Option<Pos2>>(menu_id)).flatten() {
        let mut open = true;
        Menu::context(tl!("Page"), &out.response)
            .anchor(Anchor::Point(at))
            .open(Bind::bound(&mut open))
            .show(cx.ui, |ui, _| page_menu(ui, view, editable));
        if !open {
            cx.ui.data_mut(|d| d.insert_temp::<Option<Pos2>>(menu_id, None));
        }
    }
}

/// Retire a reveal only after the pass that drew it is known to have survived.
pub(crate) fn finish_reveal(view: &mut DocView, ctx: &egui::Context) {
    let id = organize_grid_id(view).with("reveal-drawn");
    let drawn = ctx.data_mut(|data| data.remove_temp::<bool>(id)).unwrap_or(false);
    if drawn && !ctx.will_discard() {
        view.grid_reveal = None;
    }
}
