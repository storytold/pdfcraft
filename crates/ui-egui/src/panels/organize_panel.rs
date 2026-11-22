//! Organize pages, laid out as Acrobat's panel: the pages selected (typed, or picked by kind from
//! its menu); page options as a row of icon buttons (rotate both ways, delete, extract) and Insert,
//! Replace, Split and Bates numbering; page setup (boxes, crop, transitions, templates, printing);
//! and page labels and properties. It shows whenever the page grid does, so every way into
//! Organize (the tool, the Edit panel, the command, `--organize`) gets the same controls, and the
//! grid's own toolbar keeps only moving pages, their size, Save pages and Close.
//!
//! The page actions queue on the view (`pending_edit`, `pending_action`) exactly as the grid's
//! toolbar did, so they run, refuse and undo the same way.

use craft::behavior::{IconId, Selection};
use craft::pages::ui::PageRangeField;
use craft::ui::buttons::IconButton;
use craft::ui::contract::{ActivateOn, Bind, CollectionEvent, Entry, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::layout_components::Row;
use craft::ui::contract::overlay::Anchor;
use craft::ui::menu::Menu;
use craft::ui::list::{List, ListRow, RowInk};
use craft::ui::primitives::IconSpec;
use pdfcraft_engine::Edit;

use super::tools::{Rows, caption_row};
use crate::canvas::ViewAction;
use crate::{LeftPanel, PdfCraftApp};

/// Whether the left panel is Organize pages: the tool is chosen, or the active view shows the grid.
pub(crate) fn showing(app: &PdfCraftApp) -> bool {
    app.left == LeftPanel::Tool("organize") || app.active.and_then(|i| app.views.get(i)).is_some_and(|v| v.organize)
}

/// The panel of the workspace Organize was opened from, shown again once Organize closes (by
/// its Close, the grid's Close, or opening a page from the grid): Edit's panel under the Edit
/// tab, as Acrobat goes back to the tool you came from, and All tools otherwise.
pub(crate) fn workspace_panel(mode: crate::Mode) -> LeftPanel {
    match mode {
        crate::Mode::Edit => LeftPanel::Tool("edit"),
        crate::Mode::Convert => LeftPanel::Tool("export"),
        crate::Mode::Sign => LeftPanel::Tool("fill_sign"),
        _ => LeftPanel::AllTools,
    }
}

/// What a row does once clicked.
enum Act {
    Edit(Box<Edit>),
    View(ViewAction),
    Command(&'static str),
}

/// One line of the panel's list.
enum Line {
    Heading(&'static str),
    Row {
        icon: &'static str,
        label: &'static str,
        act: Option<Act>,
        /// Whether the row works now: needs page assembly allowed.
        enabled: bool,
        /// Not in PdfCraft yet: shown dimmed, as Acrobat lists it, and says it is planned when hovered.
        later: bool,
        /// Starts a group: room above it.
        space: bool,
    },
}

/// The menu of the page range field: pages by kind.
fn page_menu(info: &pdfcraft_render::DocInfo, n: usize) -> Vec<Entry<Vec<usize>>> {
    use pdfcraft_engine::{PageOrientation as O, PageParity as P, filter_pages};
    let all: Vec<usize> = (0..n).collect();
    let mut entries: Vec<Entry<Vec<usize>>> = [
        ("All pages", P::Both, O::Both),
        ("Even pages", P::Even, O::Both),
        ("Odd pages", P::Odd, O::Both),
        ("Landscape pages", P::Both, O::Landscape),
        ("Portrait pages", P::Both, O::Portrait),
    ]
    .into_iter()
    .map(|(label, parity, orient)| Entry::new(filter_pages(info, &all, parity, orient), tl!(label).to_owned()))
    .collect();
    entries.push(Entry::new(Vec::new(), tl!("None").to_owned()));
    entries
}

/// The Organize pages body. Its Back (`tools.all`) and Close belong to the dock's group header.
// PLAN: the panel's own Close (it also closed the page grid and went back to the workspace's panel) is now the
// dock's Close; pdf-chrome-w13's group header owns closing the grid with it.
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some((index, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let info = &doc.info;
    let editable = doc.allows_assembly();
    let n = info.pages.len();
    let mut act: Option<Act> = None;
    let Some(view) = app.views.get_mut(index) else { return };

    caption_row(ui, tl!("Pages selected"), true);
    let current = view.current();
    let selected: Vec<usize> = view.selected.items.clone();
    let entries = page_menu(info, n);
    let picked = PageRangeField::new(tl!("Pages selected").to_owned(), &selected, n, current).menu(&entries).show(ui).picked;
    if let Some(pages) = picked {
        view.select_pages(&pages);
    }

    let targets = view.target_pages();
    let last = targets.last().copied().unwrap_or(0);
    caption_row(ui, tl!("Page options"), true);
    // A document keeps at least one page.
    let can_delete = editable && targets.len() < n;
    Row::new().show(ui, |ui, _| {
        let buttons = [
            ("rotate-ccw", "Rotate counterclockwise", editable),
            ("rotate-cw", "Rotate clockwise", editable),
            ("trash-2", "Delete pages (Delete)", can_delete),
            ("file-output", "Extract pages to a new document", editable),
        ];
        for (at, (icon, label, enabled)) in buttons.into_iter().enumerate() {
            if !IconButton::new(IconId::new(icon)).label(tl!(label).to_owned()).enabled(enabled).show(ui).clicked() {
                continue;
            }
            act = Some(match at {
                0 => Act::Edit(Box::new(Edit::RotatePages { pages: targets.clone(), degrees: -90 })),
                1 => Act::Edit(Box::new(Edit::RotatePages { pages: targets.clone(), degrees: 90 })),
                2 => Act::Edit(Box::new(Edit::DeletePages { pages: targets.clone() })),
                _ => Act::View(ViewAction::Extract),
            });
        }
    });

    // Insert: a blank page, pages from a file, or copies of the selected pages.
    let crop = info.pages.get(last).map_or([0.0, 0.0, 612.0, 792.0], |p| p.crop);
    let (w, h) = ((crop[2] - crop[0]).abs().max(1.0) as f64, (crop[3] - crop[1]).abs().max(1.0) as f64);
    let row = |icon, label, act, enabled, later, space| Line::Row { icon, label, act, enabled, later, space };
    let mut lines = vec![
        // Production's Insert is one row that opens a menu of the three ways to insert.
        row("file-plus-2", "Insert", None, editable, false, false),
        row("replace", "Replace", Some(Act::View(ViewAction::ReplacePages)), editable, false, true),
        row("scissors", "Split", Some(Act::View(ViewAction::Split)), editable, false, false),
        row("hash", "Bates numbering", Some(Act::Command("edit.bates")), true, false, false),
        row("square-dashed-mouse-pointer", "Set page boxes", Some(Act::Command("page.boxes")), true, false, true),
        row("crop", "Crop pages", Some(Act::Command("page.crop")), true, false, false),
        row("presentation", "Page transitions", Some(Act::Command("page.transitions")), true, true, false),
        row("file-text", "Page templates", None, true, true, false),
        row("printer", "Print pages", Some(Act::Command("print.dialog")), true, false, false),
        row("tag", "Page labels", Some(Act::Command("page.number")), true, false, true),
        row("info", "Page properties", Some(Act::Command("page.properties")), true, false, false),
    ];
    let out = List::<usize>::new(tl!("Page actions").to_owned())
        .select(SelectMode::Single)
        .activate(ActivateOn::Click)
        .selection(Bind::given(Selection::default()))
        .show_items(ui, &Rows(lines.len()), &mut |at, _| match lines.get(*at) {
            Some(Line::Heading(title)) => ListRow::new(tl!(title).to_owned()).heading().ink(RowInk::Dimmed),
            Some(Line::Row { icon, label, enabled, later, space, .. }) => {
                let mut row = ListRow::new(tl!(label).to_owned()).icon(IconSpec::svg(IconId::new(icon))).enabled(*enabled).space_above(*space);
                if *label == "Insert" {
                    row = row.row_menu();
                }
                if *later { row.ink(RowInk::Dimmed).tooltip(tl!("Planned").to_owned()) } else { row }
            }
            None => ListRow::spacer(),
        });
    // The Insert menu hangs where it was asked for (the row's More button, a right click or a plain click on the
    // row) and stays until a pick or a click elsewhere closes it.
    let menu_id = ui.id().with("insert-menu-at");
    let mut menu_at: Option<egui::Pos2> = ui.ctx().data(|d| d.get_temp::<Option<egui::Pos2>>(menu_id)).flatten();
    for event in &out.events {
        match event {
            CollectionEvent::ContextMenu(request) => menu_at = Some(request.at),
            CollectionEvent::Activate(at) if matches!(lines.get(*at), Some(Line::Row { label: "Insert", enabled: true, .. })) => {
                menu_at = Some(ui.ctx().pointer_latest_pos().unwrap_or_else(|| out.response.rect.left_top()));
            }
            _ => {}
        }
    }
    if let Some(at) = menu_at {
        let entries = [
            Entry::new(0u8, tl!("Insert a blank page after the selection").to_owned()),
            Entry::new(1u8, tl!("Insert pages from a file…").to_owned()),
            Entry::new(2u8, tl!("Duplicate pages").to_owned()),
        ];
        let mut open = true;
        let picked = Menu::context(tl!("Insert").to_owned(), &out.response).anchor(Anchor::Point(at)).open(Bind::bound(&mut open)).pick(ui, &entries).picked;
        match picked {
            Some(0) => act = Some(Act::Edit(Box::new(Edit::InsertBlankPage { at: last.saturating_add(1), width: w, height: h }))),
            Some(1) => act = Some(Act::View(ViewAction::InsertFromFile)),
            Some(2) => act = Some(Act::Command("page.duplicate")),
            _ => {}
        }
        if !open || picked.is_some() {
            menu_at = None;
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(menu_id, menu_at));
    for event in out.events {
        if let CollectionEvent::Activate(at) = event
            && let Some(Line::Row { act: slot, enabled: true, later: false, .. }) = lines.get_mut(at)
            && let Some(done) = slot.take()
        {
            act = Some(done);
        }
    }

    match act {
        Some(Act::Edit(edit)) => view.session.pending_edit = Some(*edit),
        Some(Act::View(action)) => view.pending_action = Some(action),
        Some(Act::Command(command)) => app.run_command(ui.ctx(), command),
        None => {}
    }
}
