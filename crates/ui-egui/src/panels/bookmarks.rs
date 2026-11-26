use super::right::PanelResult;
use super::{Nav, PdfCraftApp};
use craft::behavior::IconId;
use craft::behavior::{DisplayOrder, GesturePhase, ItemSource, Mixed, Selection};
use craft::keymap::CommandRef;
use craft::ui::buttons::IconButton;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{ActivateOn, Bind, CollectionEvent, Props, SelectMode, Show, ShowCollection, ShowContainer, request_rename};
use craft::ui::list::{HueTarget, List, ListRow, OpenSet};
use craft::ui::menu::{Menu, MenuItem, PanelRow};
use craft::ui::notices::Notice;
use craft::ui::primitives::IconSpec;
use craft::ui::style::DataColor;
use craft::ui::text::TextField;
use pdfcraft_render::OutlineItem;

/// What the user asked to do with a bookmark (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after the selected one (⌘B).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    /// Make the title bold or italic or not (`None` keeps that one).
    SetStyle(Vec<usize>, Option<bool>, Option<bool>),
    /// Colour the title; black is the default.
    SetColor(Vec<usize>, [f64; 3]),
    Delete(Vec<usize>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
    /// Copy the bookmark and everything under it to the bookmark clipboard.
    Copy(Vec<usize>),
    /// Copy it, then delete it (one undo step: only the delete changes the document).
    Cut(Vec<usize>),
    /// Paste the bookmark clipboard under `parent` at `index` (past the end appends).
    Paste {
        parent: Vec<usize>,
        index: usize,
    },
    /// Bookmarks from the tagged headings, under a new first "Untitled" bookmark.
    FromStructure,
    /// Open New bookmarks from text styles, to choose a style per bookmark level (#1693).
    FromTextStyles,
}

/// Outline nesting the panel lists; the engine bounds what it reads, this bounds what the panel walks.
const MAX_DEPTH: usize = 256;

/// The id of a document's bookmark list, so a new bookmark's rename can be requested from outside the panel.
pub(crate) fn list_id(doc: pdfcraft_engine::DocId) -> egui::Id {
    egui::Id::unique(("bookmark-list", doc.0))
}

/// Asks the bookmark list of `doc` to open the rename editor on the bookmark at `path`.
pub(crate) fn start_rename(ctx: &egui::Context, doc: pdfcraft_engine::DocId, path: &[usize]) {
    request_rename(ctx, list_id(doc), &path.to_vec());
}

/// The Bookmark options menu rows.
pub(super) fn options_menu(_app: &PdfCraftApp) -> Vec<PanelRow> {
    [
        "bookmark.from_structure",
        "bookmark.from_text_styles",
        "bookmark.paste",
        "bookmark.expand_all",
        "bookmark.collapse_all",
        "bookmark.expand_level",
    ]
    .into_iter()
    .map(|id| PanelRow::Command(CommandRef::new(id)))
    .collect()
}

/// The header's New bookmark button, shown while the document may be edited.
pub(super) fn header(app: &PdfCraftApp, ui: &mut egui::Ui) {
    let editable = app.active_ids().and_then(|(_, id)| app.session.get(id)).is_some_and(|d| d.allows_assembly() && d.read_only_reason.is_none());
    if editable {
        IconButton::command(IconSpec::svg(IconId::new("bookmark-plus")), CommandRef::new("bookmark.add")).show(ui);
    }
}

/// Opens the groups of `outline` shallower than `level` and closes the others: `usize::MAX` opens every group, `0`
/// closes every group.
pub(crate) fn expand_to(view: &mut crate::DocView, outline: &[OutlineItem], level: usize) {
    fn walk(items: &[OutlineItem], depth: usize, level: usize, path: &mut Vec<usize>, open: &mut OpenSet<Vec<usize>>) {
        if depth >= MAX_DEPTH {
            return;
        }
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            if !item.children.is_empty() {
                open.set(path, depth < level);
                walk(&item.children, depth + 1, level, path, open);
            }
            path.pop();
        }
    }
    // PLAN: assumed `DocView::bookmarks_open: OpenSet<Vec<usize>>` (pdf-canvas-w8); the outline's open state lives
    // on the view, not in the document.
    walk(outline, 0, level, &mut Vec::new(), &mut view.bookmarks_open);
}

/// The groups of `outline` open as the file says: only a positive `/Count` opens one, at every level (ISO 32000-2
/// §12.3.3, Table 151). A view starts with them so; the reader's opening and closing changes them from there.
pub(crate) fn opened_as_filed(outline: &[OutlineItem]) -> OpenSet<Vec<usize>> {
    fn walk(items: &[OutlineItem], depth: usize, path: &mut Vec<usize>, open: &mut OpenSet<Vec<usize>>) {
        if depth >= MAX_DEPTH {
            return;
        }
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            if !item.children.is_empty() {
                open.set(path, item.open);
                walk(&item.children, depth + 1, path, open);
            }
            path.pop();
        }
    }
    let mut open = OpenSet::default();
    walk(outline, 0, &mut Vec::new(), &mut open);
    open
}

/// How a bookmark's title is drawn: bold at depth 0 and where the file says, italic where it says, in its own colour
/// (ISO 32000-2 §12.3.3, Tables 151 and 152). The row keeps the colour readable.
fn title_style(item: &OutlineItem, depth: usize) -> (bool, bool, Option<DataColor>) {
    let color = item.color.map(|rgb| {
        // Each component is within 0–1, so the rounded value fits a byte.
        let [r, g, b] = rgb.map(|c| (c * 255.0).round() as u8);
        DataColor::new(egui::Color32::from_rgb(r, g, b))
    });
    (item.bold || depth == 0, item.italic, color)
}

fn item_at<'a>(outline: &'a [OutlineItem], path: &[usize]) -> Option<&'a OutlineItem> {
    let (&first, rest) = path.split_first()?;
    let mut item = outline.get(first)?;
    for &i in rest {
        item = item.children.get(i)?;
    }
    Some(item)
}

fn groups(items: &[OutlineItem], path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if path.len() >= MAX_DEPTH {
        return;
    }
    for (i, item) in items.iter().enumerate() {
        path.push(i);
        if !item.children.is_empty() {
            out.push(path.clone());
            groups(&item.children, path, out);
        }
        path.pop();
    }
}

struct Row<'a> {
    path: Vec<usize>,
    item: &'a OutlineItem,
    open: bool,
}

/// The outline as a tree's display order: the rows under open groups.
struct Rows<'a>(Vec<Row<'a>>);

impl<'a> Rows<'a> {
    fn collect(items: &'a [OutlineItem], path: &mut Vec<usize>, open: &dyn Fn(&[usize]) -> bool, out: &mut Vec<Row<'a>>) {
        if path.len() >= MAX_DEPTH {
            return;
        }
        for (i, item) in items.iter().enumerate() {
            path.push(i);
            let shown = open(path);
            out.push(Row { path: path.clone(), item, open: shown });
            if shown {
                Self::collect(&item.children, path, open, out);
            }
            path.pop();
        }
    }
}

impl DisplayOrder<Vec<usize>> for Rows<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn key(&self, at: usize) -> Vec<usize> {
        self.0.get(at).map(|r| r.path.clone()).unwrap_or_default()
    }
    fn depth(&self, at: usize) -> usize {
        self.0.get(at).map_or(0, |r| r.path.len().saturating_sub(1))
    }
    fn has_children(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|r| !r.item.children.is_empty())
    }
    fn open(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|r| r.open)
    }
    /// The title: what the rename field starts from and type-ahead matches.
    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.0.get(at).map(|r| std::borrow::Cow::Borrowed(r.item.title.as_str()))
    }
}

impl ItemSource<Vec<usize>> for Rows<'_> {}

/// The Bookmarks panel body: the search field, then the outline as a tree. Navigation goes in `nav`, the actions in
/// `bookmark`.
pub(super) fn body(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> PanelResult {
    let mut result = PanelResult::default();
    let Some((index, id)) = app.active_ids() else { return result };
    let Some(doc) = app.session.get(id) else { return result };
    let editable = doc.allows_assembly() && doc.read_only_reason.is_none();
    let can_paste = app.bookmark_clipboard.is_some();
    let info = &doc.info;
    let outline = info.outline.as_slice();
    let Some(view) = app.views.get_mut(index) else { return result };
    let current = view.current();

    if outline.is_empty() {
        // Centred in the panel with a glyph above, as the old panel showed an empty list.
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            craft::ui::display::Icon::new(IconSpec::svg(IconId::new("bookmark-plus"))).show(ui);
            ui.add_space(8.0);
            craft::ui::display::Text::new(tl!("This document has no bookmarks.").to_string()).show(ui);
        });
        return result;
    }

    // The query is the search field's kept value, inside the document's id scope so each document keeps its own.
    let query = ui
        .push_id(view.id.0, |ui| {
            // A kept binding raises no event: the shown value after the pass is the query.
            match TextField::search(tl!("Search"), Bind::<String>::kept()).show(ui).shown {
                Some(Mixed::Same(text)) => text.as_str().to_owned(),
                _ => String::new(),
            }
        })
        .inner;
    let searching = !query.trim().is_empty();

    // While searching every group lists open without writing the outline's open state, so the filter sees every
    // bookmark.
    let open_set = &view.bookmarks_open;
    let open = |path: &[usize]| searching || open_set.is_open(&path.to_vec());
    let mut rows = Vec::new();
    Rows::collect(outline, &mut Vec::new(), &open, &mut rows);
    let rows = Rows(rows);

    let list_id = list_id(id);
    let mut held = Selection::<Vec<usize>>::default();
    let row_key = |key: &Vec<usize>| item_at(outline, key).map(|i| vec![i.title.clone().into()]).unwrap_or_default();
    let list = List::<Vec<usize>>::new(tl!("Bookmarks"))
        .id(list_id)
        .tree(true)
        .select(SelectMode::Single)
        .activate(ActivateOn::Click)
        .rename(editable)
        .selection(Bind::bound(&mut held))
        .empty(tl!("No matches."))
        .filter(&query, row_key);

    let out = list.show_items(ui, &rows, &mut |path, icx| {
        let Some(item) = item_at(outline, path) else { return ListRow::new("") };
        let depth = path.len().saturating_sub(1);
        let (strong, italic, color) = title_style(item, depth);
        let mut r = ListRow::new(item.title.clone()).state(icx.row.clone()).depth(depth).strong(strong).italic(italic);
        if !item.children.is_empty() {
            r = r.group(open(path));
        }
        if let Some(color) = color {
            r = r.hue(color, HueTarget::Label);
        }
        if let Some(page) = item.page.and_then(|p| info.pages.get(p)) {
            r = r.trailing(page.label.clone());
        }
        r
    });

    let menu_id = list_id.with("target");
    let mut target: Option<Vec<usize>> = ui.data(|d| d.get_temp::<Option<Vec<usize>>>(menu_id)).flatten();
    for event in out.events {
        match event {
            CollectionEvent::Activate(path) => {
                if let Some(item) = item_at(outline, &path) {
                    result.nav = match (item.page, &item.action) {
                        (Some(p), _) => Some(Nav::Dest(p, item.view)),
                        // A web address, another PDF or a layer change, as a link runs it.
                        (None, Some(action)) => Some(Nav::Action(action.clone())),
                        (None, None) => None,
                    };
                }
            }
            CollectionEvent::Open(change) if !searching => {
                let mut all = Vec::new();
                groups(outline, &mut Vec::new(), &mut all);
                view.bookmarks_open.apply(&change.key, change.open, change.all, all.iter());
            }
            CollectionEvent::Rename(edit) => {
                let ended = matches!(edit.phase(), Some(GesturePhase::Commit) | None);
                if let (true, Some(rename)) = (ended, edit.value()) {
                    let title = rename.name.trim();
                    if !title.is_empty() && item_at(outline, &rename.key).is_some_and(|i| i.title != title) {
                        result.bookmark = Some(BmAction::Rename(rename.key.clone(), title.to_owned()));
                    }
                }
            }
            CollectionEvent::ContextMenu(request) if editable => target = Some(request.key),
            _ => {}
        }
    }

    if let Some(path) = target.clone() {
        let menu = Menu::context(tl!("Bookmark"), &out.response).show(ui, |ui, _| {
            let depth = path.len().saturating_sub(1);
            let i = path.last().copied().unwrap_or(0);
            let siblings = match path.split_last() {
                Some((_, parent)) if !parent.is_empty() => item_at(outline, parent).map_or(0, |p| p.children.len()),
                _ => outline.len(),
            };
            // Held apart from `result` so the style and colour submenus can set the choice too.
            let picked = std::cell::Cell::new(None);
            let pick = |ui: &mut egui::Ui, label: &'static str, enabled: bool, action: BmAction| {
                if MenuItem::new(label).enabled(enabled).show(ui).ran {
                    picked.set(Some(action));
                }
            };
            if MenuItem::new(tl!("Rename")).show(ui).ran {
                request_rename(ui.ctx(), list_id, &path);
            }
            let set_page = crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current + 1).to_string())]);
            if MenuItem::new(set_page).show(ui).ran {
                picked.set(Some(BmAction::SetToCurrentPage(path.clone())));
            }
            if let Some(item) = item_at(outline, &path) {
                // The title's style and colour (ISO 32000-2 §12.3.3, Tables 151 and 152).
                let style = MenuItem::new(tl!("Style")).show(ui);
                let mut styled = None;
                Menu::submenu(tl!("Style"), &style.response).show(ui, |ui, _| {
                    let (mut bold, mut italic) = (item.bold, item.italic);
                    if Checkbox::new(Bind::bound(&mut bold)).label(tl!("Bold")).show(ui).value().is_some() {
                        styled = Some(BmAction::SetStyle(path.clone(), Some(bold), None));
                    }
                    if Checkbox::new(Bind::bound(&mut italic)).label(tl!("Italic")).show(ui).value().is_some() {
                        styled = Some(BmAction::SetStyle(path.clone(), None, Some(italic)));
                    }
                });
                let colour = MenuItem::new(tl!("Colour")).show(ui);
                Menu::submenu(tl!("Colour"), &colour.response).show(ui, |ui, _| {
                    let current = item.color.map(|c| c.map(f64::from));
                    if let Some(c) = crate::comments::colours::palette(ui, tl!("Colour"), current, "bookmark.colour", &app.recent_colours).picked {
                        styled = Some(BmAction::SetColor(path.clone(), c));
                    }
                });
                if styled.is_some() {
                    picked.set(styled);
                }
            }
            pick(ui, tl!("Move up"), i > 0, BmAction::MoveUp(path.clone()));
            pick(ui, tl!("Move down"), i + 1 < siblings, BmAction::MoveDown(path.clone()));
            pick(ui, tl!("Indent"), i > 0, BmAction::Indent(path.clone()));
            pick(ui, tl!("Outdent"), depth > 0, BmAction::Outdent(path.clone()));
            pick(ui, tl!("Cut"), true, BmAction::Cut(path.clone()));
            pick(ui, tl!("Copy"), true, BmAction::Copy(path.clone()));
            let parent = path.get(..depth).unwrap_or_default().to_vec();
            pick(ui, tl!("Paste"), can_paste, BmAction::Paste { parent, index: i.saturating_add(1) });
            pick(ui, tl!("Delete"), true, BmAction::Delete(path.clone()));
            if let Some(action) = picked.take() {
                result.bookmark = Some(action);
            }
        });
        if menu.output.closed.is_some() {
            target = None;
        }
    }
    // PLAN: the open menu's target is egui temp data; the panel's component memory would hold it (as `history.rs`
    // does) once pdf-chrome-w13 passes the body its `BodyCx`.
    ui.data_mut(|d| d.insert_temp(menu_id, target));

    if info.outline_more {
        Notice::new(
            craft::theme::variant::Tone::Neutral,
            tl!("Some bookmarks are not shown here: the panel lists a limited number, nested a limited depth."),
        )
        .show(ui);
    }
    result
}
