//! The page's right-click menu, shown as a framework context menu from the tool host's `ContextRequest`.

use craft::behavior::IconId;
use craft::ui::canvas::ContextRequest;
use craft::ui::contract::overlay::Anchor;
use craft::ui::contract::{Props as _, Show as _, ShowContainer as _};
use craft::ui::display::Divider;
use craft::ui::menu::{Menu, MenuIcon, MenuIconRow, MenuItem};
use pdfcraft_engine::Edit;

use super::page_parts::Owner;
use super::{DocInfo, DocView};
use crate::{QuickTool, comments};

/// Default page right-click menu actions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PageMenuAction {
    RotateCCW,
    RotateCW,
    Print,
    SelectTool,
    HandTool,
    MarqueeZoom,
    Copy,
    Paste(usize, [f64; 2]),
    /// Paste the copied comments, centred at this point of the page.
    PasteComments(usize, [f64; 2]),
    AddComment(usize, [f64; 2]),
    AddBookmark,
    TakeSnapshot,
    EditPdf,
    AddText,
    AddImage,
    DeletePage(usize),
    ExportPdf,
    Find,
    DocProperties,
    FlattenComments,
}

pub(crate) struct PageMenuPerms {
    pub can_modify: bool,
    pub can_assemble: bool,
    pub can_annotate: bool,
    pub can_print: bool,
    pub can_copy: bool,
    pub can_copy_items: bool,
    pub can_paste: bool,
    /// Copied comments wait to be pasted, and comments may be added.
    pub can_paste_comments: bool,
    pub has_annotations: bool,
}

/// The prepare-mode field menu's choices.
pub(crate) enum FieldMenu {
    Properties,
    Duplicate(String),
}

/// What a row of the default menu does when picked.
pub(crate) enum RowPick {
    /// A page action the app runs.
    Page(PageMenuAction),
    /// A canvas action (Copy Image).
    Canvas(comments::CanvasAction),
}

/// One row of the default page menu.
pub(crate) enum PageMenuRow {
    /// The strip of icons: icon, label, enabled, action.
    Icons(Vec<(IconId, String, bool, PageMenuAction)>),
    Separator,
    Item {
        label: String,
        enabled: bool,
        pick: RowPick,
        /// The command whose chord the row shows at its end.
        chord_of: Option<&'static str>,
    },
}

/// What the page menu chose in a pass, and whether it closed.
#[derive(Default)]
pub(crate) struct PageMenuPick {
    pub page: Option<PageMenuAction>,
    pub field: Option<FieldMenu>,
    pub canvas: Option<comments::CanvasAction>,
    pub image: Option<crate::edit_text_ui::ImageAction>,
    /// The menu closed in this pass: the caller drops the request.
    pub closed: bool,
}

/// What the page menu reads from the document and the app.
pub(crate) struct PageMenuCx<'a> {
    pub doc: &'a pdfcraft_engine::Document,
    pub info: &'a DocInfo,
    pub prefs: &'a comments::CommentPrefs,
    pub rights: comments::MenuRights,
    pub recent: &'a comments::RecentColours,
    pub tool: QuickTool,
    pub preparing: bool,
    pub form: &'a std::sync::Arc<Vec<pdfcraft_engine::FormField>>,
    pub can_copy: bool,
    pub can_paste_image: bool,
    /// Comments were copied (in this document or another), so Paste has something to place.
    pub can_paste_comments: bool,
    /// The jobs and wake the picture search runs on.
    pub jobs: &'a std::sync::Arc<dyn craft::host::Jobs>,
    pub wake: &'a craft::host::Wake,
}

/// The default rows, in order, for a press at `at` on `page`; `picture_under` is the picture index there.
pub(crate) fn default_rows(perms: &PageMenuPerms, page: usize, at: [f64; 2], picture_under: Option<usize>) -> Vec<PageMenuRow> {
    use PageMenuAction as A;
    let item = |label: String, enabled: bool, action: A| PageMenuRow::Item { label, enabled, pick: RowPick::Page(action), chord_of: None };
    let mut rows = Vec::new();
    if let Some(image) = picture_under {
        rows.push(PageMenuRow::Item {
            label: tl!("Copy Image").to_string(),
            enabled: true,
            pick: RowPick::Canvas(comments::CanvasAction::CopyImage(page, image)),
            chord_of: None,
        });
        rows.push(PageMenuRow::Item {
            label: tl!("Copy Image in Original Format").to_string(),
            enabled: true,
            pick: RowPick::Canvas(comments::CanvasAction::CopyImageFile(page, image)),
            chord_of: None,
        });
        rows.push(PageMenuRow::Separator);
    }
    rows.push(PageMenuRow::Icons(vec![
        (IconId::new("rotate_ccw"), tl!("Rotate view counterclockwise").to_string(), true, A::RotateCCW),
        (IconId::new("printer"), tl!("Print").to_string(), perms.can_print, A::Print),
        (IconId::new("pointer"), tl!("Select Tool").to_string(), true, A::SelectTool),
        (IconId::new("hand"), tl!("Hand Tool").to_string(), true, A::HandTool),
        (IconId::new("zoom_in"), tl!("Marquee Zoom").to_string(), true, A::MarqueeZoom),
    ]));
    rows.push(PageMenuRow::Separator);
    rows.push(PageMenuRow::Item {
        label: tl!("Copy").to_string(),
        enabled: perms.can_copy_items,
        pick: RowPick::Page(A::Copy),
        chord_of: Some("edit.copy"),
    });
    rows.push(PageMenuRow::Item {
        label: tl!("Paste").to_string(),
        enabled: perms.can_paste,
        pick: RowPick::Page(A::Paste(page, at)),
        chord_of: Some("edit.paste"),
    });
    if perms.can_paste_comments {
        rows.push(item(tl!("Paste comments").to_string(), true, A::PasteComments(page, at)));
    }
    rows.push(PageMenuRow::Separator);
    rows.push(item(tl!("Add comment").to_string(), perms.can_annotate, A::AddComment(page, at)));
    if perms.has_annotations {
        rows.push(item(tl!("Flatten comments").to_string(), perms.can_modify, A::FlattenComments));
    }
    rows.push(item(tl!("Add bookmark").to_string(), perms.can_assemble, A::AddBookmark));
    rows.push(item(tl!("Take a snapshot").to_string(), perms.can_copy, A::TakeSnapshot));
    rows.push(PageMenuRow::Separator);
    // PLAN: `MenuItem` has no emphasis prop, so "Edit this PDF" (the primary row) shows in the normal weight.
    rows.push(item(tl!("Edit this PDF").to_string(), perms.can_modify, A::EditPdf));
    rows.push(item(tl!("Add text").to_string(), perms.can_modify, A::AddText));
    rows.push(item(tl!("Add image").to_string(), perms.can_modify, A::AddImage));
    rows.push(item(tl!("Delete page").to_string(), perms.can_assemble, A::DeletePage(page)));
    rows.push(item(tl!("Export this PDF").to_string(), perms.can_copy, A::ExportPdf));
    rows.push(PageMenuRow::Separator);
    rows.push(item(tl!("Find text or tools").to_string(), true, A::Find));
    rows.push(item(tl!("Rotate view clockwise").to_string(), true, A::RotateCW));
    rows.push(item(tl!("Document properties…").to_string(), true, A::DocProperties));
    rows
}

/// Shows `rows` in the open menu and records what was picked.
fn show_rows(ui: &mut egui::Ui, rows: Vec<PageMenuRow>, pick: &mut PageMenuPick) {
    for row in rows {
        match row {
            PageMenuRow::Icons(icons) => {
                let strip: Vec<MenuIcon<'_>> =
                    icons.iter().map(|(icon, label, enabled, _)| MenuIcon::new(icon.clone(), label.clone()).enabled(*enabled)).collect();
                let out = MenuIconRow::new(tl!("Page").to_string(), &strip).show(ui);
                if let Some((_, _, _, action)) = out.picked.and_then(|i| icons.get(i)) {
                    pick.page = Some(action.clone());
                }
            }
            PageMenuRow::Separator => {
                Divider::horizontal().show(ui);
            }
            PageMenuRow::Item { label, enabled, pick: choice, chord_of } => {
                let mut item = MenuItem::new(label).enabled(enabled);
                // PLAN: assumed `crate::commands::chord_label(&egui::Context, &str) -> Option<String>`, the catalogue's
                // `Chord::display` of a command's chord (pdf-chrome-w9)
                if let Some(text) = chord_of.and_then(|id| crate::commands::chord_label(ui.ctx(), id)) {
                    item = item.shortcut(text);
                }
                if item.show(ui).ran {
                    match choice {
                        RowPick::Page(action) => pick.page = Some(action),
                        RowPick::Canvas(action) => pick.canvas = Some(action),
                    }
                }
            }
        }
    }
}

/// One Align / Distribute / Same Size item.
#[allow(clippy::too_many_arguments)]
fn arrange_row(
    ui: &mut egui::Ui,
    view: &mut DocView,
    form: &std::sync::Arc<Vec<pdfcraft_engine::FormField>>,
    anchor: &(String, usize),
    others: &[(String, usize)],
    op: crate::prepare::Arrange,
    label: &str,
    can_modify: bool,
) {
    if MenuItem::new(tl!(label).to_string()).enabled(can_modify).show(ui).ran {
        view.session.pending_edit = crate::prepare::arrange(form, anchor, others, op);
    }
}

/// The selected field's menu while preparing a form.
fn field_rows(
    ui: &mut egui::Ui,
    view: &mut DocView,
    form: &std::sync::Arc<Vec<pdfcraft_engine::FormField>>,
    can_modify: bool,
    name: String,
    widget: usize,
    pick: &mut PageMenuPick,
) {
    use crate::prepare::Arrange as A;
    // Several fields: Align, Distribute, Set Fields to Same Size.
    if !view.session.prepare.also.is_empty() {
        let others = view.session.prepare.also.clone();
        let anchor = (name.clone(), widget);
        Menu::submenu_row_enabled(ui, tl!("Align").to_string(), can_modify, |ui, _| {
            for (op, label) in [
                (A::AlignLeft, "Left"),
                (A::AlignRight, "Right"),
                (A::AlignTop, "Top"),
                (A::AlignBottom, "Bottom"),
                (A::AlignCenterV, "Center vertically"),
                (A::AlignCenterH, "Center horizontally"),
            ] {
                arrange_row(ui, view, form, &anchor, &others, op, label, can_modify);
            }
        });
        Menu::submenu_row_enabled(ui, tl!("Distribute").to_string(), can_modify, |ui, _| {
            for (op, label) in [(A::DistributeH, "Horizontally"), (A::DistributeV, "Vertically")] {
                arrange_row(ui, view, form, &anchor, &others, op, label, can_modify);
            }
        });
        Menu::submenu_row_enabled(ui, tl!("Set Fields to Same Size").to_string(), can_modify, |ui, _| {
            for (op, label) in [(A::SameHeight, "Height"), (A::SameWidth, "Width"), (A::SameSize, "Both")] {
                arrange_row(ui, view, form, &anchor, &others, op, label, can_modify);
            }
        });
        Divider::horizontal().show(ui);
    }
    if MenuItem::new(tl!("Properties…").to_string()).show(ui).ran {
        pick.field = Some(FieldMenu::Properties);
    }
    if MenuItem::new(tl!("Duplicate…").to_string()).enabled(can_modify).show(ui).ran {
        pick.field = Some(FieldMenu::Duplicate(name));
    }
    Divider::horizontal().show(ui);
    let delete = if view.session.prepare.also.is_empty() { "Delete" } else { "Delete selected fields" };
    if MenuItem::new(tl!(delete).to_string()).enabled(can_modify).show(ui).ran {
        view.session.pending_edit = crate::prepare::delete_selected(&mut view.session);
    }
}

/// The menu of the link the press went to.
fn link_rows(ui: &mut egui::Ui, view: &mut DocView, can_modify: bool, page: usize, index: usize) {
    if MenuItem::new(tl!("Properties…").to_string()).show(ui).ran {
        view.session.links.open_existing = Some((page, index));
    }
    if MenuItem::new(tl!("Delete").to_string()).enabled(can_modify).show(ui).ran {
        view.session.pending_edit = Some(Edit::DeleteLink { page, index });
        view.session.links.selected = None;
        view.session.links.context = None;
    }
    Divider::horizontal().show(ui);
    if MenuItem::new(tl!("Remove all links").to_string()).enabled(can_modify).show(ui).ran {
        view.session.pending_edit = Some(Edit::RemoveLinks { pages: None });
        view.session.links.selected = None;
        view.session.links.context = None;
    }
}

/// The rows of the menu, in the order of decisions: a field, a link, a picked picture, a comment, then the default rows.
fn rows(ui: &mut egui::Ui, view: &mut DocView, cx: &PageMenuCx<'_>, menu_for: Option<Owner>, pick: &mut PageMenuPick) {
    let can_modify = cx.rights.can_modify;
    let on_selected_field = match menu_for {
        Some(Owner::PrepareField { field, widget }) => {
            cx.form.get(field).is_some_and(|f| view.session.prepare.selected.as_ref().is_some_and(|(name, w)| *name == f.name && *w == widget))
        }
        _ => false,
    };
    if cx.preparing
        && on_selected_field
        && let Some((name, widget)) = view.session.prepare.selected.clone()
    {
        field_rows(ui, view, cx.form, can_modify, name, widget, pick);
        return;
    }
    if matches!(menu_for, Some(Owner::Link { .. }))
        && let Some((page, index)) = view.session.links.context
    {
        link_rows(ui, view, can_modify, page, index);
        return;
    }
    if cx.tool == QuickTool::EditText
        && let Some((page, Owner::Picture { index, .. })) = view.session.menu_for
        && let Some(sel) = view.session.image_selection.clone().filter(|s| (s.page, s.index) == (page, index))
    {
        let images = cx.doc.page_images(sel.page);
        // Paste lands where the menu opened, on the picture's page: another page's point means nothing there.
        let at = view.session.comments.context_at.filter(|(page, _)| *page == sel.page).map(|(_, at)| at);
        let rotation = (view.view.rotation % 360 + cx.info.pages.get(sel.page).map_or(0, |p| p.rotation % 360)) % 360;
        let menu = crate::edit_text_ui::PictureMenu { images: &images, page: sel.page, hit: sel.index, can_paste: cx.can_paste_image, at, rotation };
        pick.image = crate::edit_text_ui::picture_menu(ui, &mut view.session, &menu);
        return;
    }
    // The comment the press went to, when it is the one selected, or connected lines being drawn.
    let on_selected_comment =
        matches!(view.session.menu_for, Some((page, Owner::Comment { index, .. })) if view.session.comments.is_selected((page, index)));
    let drawing_points = matches!(cx.tool, QuickTool::Comment(t) if t.clicks_points()) && !view.session.comments.tool.drawing().is_empty();
    if on_selected_comment || drawing_points {
        let tool = if let QuickTool::Comment(t) = cx.tool { Some(t) } else { None };
        pick.canvas = comments::context_rows(
            ui,
            comments::CommentMenuCx {
                view,
                info: cx.info,
                deletable: &|page, index| cx.doc.annotation_deletable(page, index),
                prefs: cx.prefs,
                rights: cx.rights,
                recent: cx.recent,
                tool,
            },
        );
        return;
    }
    let (page, at) = view.session.comments.context_at.unwrap_or((view.view.current, [0.0, 0.0]));
    // On a page still being read, the picture rows appear when the answer comes.
    let picture_under = view.session.comments.context_at.and_then(|(page, at)| {
        view.session
            .pictures
            .get(cx.jobs, cx.wake, cx.doc, page)
            .and_then(|pictures| pdfcraft_engine::picture_at(pictures, at))
            .map(|(index, _)| *index)
    });
    // A text selection whose pages are still being read copies once they are.
    let has_text = view.selected_text().is_some() || view.selection_waits_for_text().is_some();
    let can_copy_items = cx.can_copy && (view.session.image_selection.is_some() || has_text || picture_under.is_some());
    let perms = PageMenuPerms {
        can_modify,
        can_assemble: cx.doc.allows_assembly(),
        can_annotate: cx.rights.allowed,
        can_print: cx.doc.allows_printing(),
        can_copy: cx.can_copy,
        can_copy_items,
        can_paste: cx.can_paste_image && can_modify,
        can_paste_comments: cx.rights.allowed && cx.can_paste_comments,
        has_annotations: !cx.info.annotations.is_empty(),
    };
    show_rows(ui, default_rows(&perms, page, at, picture_under), pick);
}

/// Shows the page menu every pass. The menu opens itself on the opener's context trigger and keeps its own open
/// state; `request` (the tool host's, present in the pass of the secondary press) only says where it hangs. The
/// crop handles' right click is the crop tool's, which claims the press, so no request arrives for it.
pub(crate) fn show(
    ui: &mut egui::Ui,
    request: Option<&ContextRequest>,
    opener: &egui::Response,
    menu_for: Option<Owner>,
    view: &mut DocView,
    cx: &PageMenuCx<'_>,
) -> PageMenuPick {
    let mut pick = PageMenuPick::default();
    // A right-click a page control kept for itself (the picture being cropped) opens no menu: the menu does not run in
    // the pass of its trigger.
    if std::mem::take(&mut view.session.context_held) {
        return pick;
    }
    let mut menu = Menu::context(tl!("Page").to_string(), opener);
    if let Some(request) = request {
        menu = menu.anchor(Anchor::Point(request.at));
    }
    let shown = menu.show(ui, |ui, _| rows(ui, view, cx, menu_for, &mut pick));
    pick.closed = shown.output.closed.is_some();
    pick
}
