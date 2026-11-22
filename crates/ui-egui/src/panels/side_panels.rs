//! The dock around the page: the Tools panel, the toolbar rail and the eleven side panels
//! ([`crate::RightPanel`]) as panels of one [`Workspace`], shown by a [`DockArea`].
//!
//! The workspace is the one record of which panels are open and where they sit. Settings written
//! by earlier versions (`side_panels`, `docking`) are read once by [`migrate`].

use craft::automation::dock as dock_rpc;
use craft::behavior::dock::{
    Action, Column, DockEdge, DockError, DockPlacement, Floating, Group, Layout, Node, PanelState, Permissions, SplitSize, Target, Workspace,
};
use craft::behavior::{Axis, DockSide, IconId};
use craft::keymap::CommandRef;
use craft::ui::buttons::IconButton;
use craft::ui::contract::{Bind, BodyCx, Props as _, Show as _};
use craft::ui::docking::{DockConfig, DockPanels, GroupAt, HidePanels, IconicMode, PanelSpec};
use craft::ui::menu::PanelRow;
use craft::ui::patterns::dock::DockArea;
use craft::ui::primitives::IconSpec;
use egui::Ui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{LeftPanel, Mode, PdfCraftApp, RightPanel};

/// Deepest split tree the old layout migration walks; the old renderer allowed far fewer.
const MAX_DEPTH: usize = 32;

/// A panel of the dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelId {
    /// The tool panels (All tools, a tool's options).
    Tools,
    /// The rail of panel buttons and page navigation.
    Toolbar,
    /// The page area: the document canvas, which no action closes or moves.
    Document,
    /// [`RightPanel::Comments`].
    Comments,
    /// [`RightPanel::Bookmarks`].
    Bookmarks,
    /// [`RightPanel::Pages`].
    Pages,
    /// [`RightPanel::Fields`].
    Fields,
    /// [`RightPanel::Layers`].
    Layers,
    /// [`RightPanel::Attachments`].
    Attachments,
    /// [`RightPanel::Signatures`].
    Signatures,
    /// [`RightPanel::Accessibility`].
    Accessibility,
    /// [`RightPanel::Tags`].
    Tags,
    /// [`RightPanel::Search`].
    Search,
    /// [`RightPanel::Compare`].
    Compare,
}

/// The side panels, in the order the rail lists them.
const SIDE: [PanelId; 11] = [
    PanelId::Comments,
    PanelId::Bookmarks,
    PanelId::Pages,
    PanelId::Fields,
    PanelId::Layers,
    PanelId::Attachments,
    PanelId::Signatures,
    PanelId::Accessibility,
    PanelId::Tags,
    PanelId::Search,
    PanelId::Compare,
];

impl PanelId {
    /// The name `ui.panel` and the panel's toggle command use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Toolbar => "toolbar",
            Self::Document => "document",
            Self::Comments => "comments",
            Self::Bookmarks => "bookmarks",
            Self::Pages => "pages",
            Self::Fields => "fields",
            Self::Layers => "layers",
            Self::Attachments => "attachments",
            Self::Signatures => "signatures",
            Self::Accessibility => "accessibility",
            Self::Tags => "tags",
            Self::Search => "search",
            Self::Compare => "compare",
        }
    }

    /// The panel `name` names.
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Tools, Self::Toolbar].into_iter().chain(SIDE).find(|panel| panel.name() == name)
    }

    /// The side panel this is, `None` for Tools and the toolbar.
    pub fn side(self) -> Option<RightPanel> {
        Some(match self {
            Self::Comments => RightPanel::Comments,
            Self::Bookmarks => RightPanel::Bookmarks,
            Self::Pages => RightPanel::Pages,
            Self::Fields => RightPanel::Fields,
            Self::Layers => RightPanel::Layers,
            Self::Attachments => RightPanel::Attachments,
            Self::Signatures => RightPanel::Signatures,
            Self::Accessibility => RightPanel::Accessibility,
            Self::Tags => RightPanel::Tags,
            Self::Search => RightPanel::Search,
            Self::Compare => RightPanel::Compare,
            Self::Tools | Self::Toolbar | Self::Document => return None,
        })
    }
}

impl From<RightPanel> for PanelId {
    /// The panel of side panel `panel`.
    fn from(panel: RightPanel) -> Self {
        match panel {
            RightPanel::Comments => Self::Comments,
            RightPanel::Bookmarks => Self::Bookmarks,
            RightPanel::Pages => Self::Pages,
            RightPanel::Fields => Self::Fields,
            RightPanel::Layers => Self::Layers,
            RightPanel::Attachments => Self::Attachments,
            RightPanel::Signatures => Self::Signatures,
            RightPanel::Accessibility => Self::Accessibility,
            RightPanel::Tags => Self::Tags,
            RightPanel::Search => Self::Search,
            RightPanel::Compare => Self::Compare,
        }
    }
}

impl From<LeftPanel> for PanelId {
    /// Every left panel is a view of the Tools panel.
    fn from(_: LeftPanel) -> Self {
        Self::Tools
    }
}

/// What may happen to `panel`. Tools and the toolbar never collapse to icons and never share their column; the page
/// area is protected; every other panel may close.
fn permissions(panel: &PanelId) -> Permissions {
    match panel {
        PanelId::Document => Permissions::PROTECTED,
        PanelId::Tools | PanelId::Toolbar => Permissions::ALL.collapses(false).shares_column(false),
        _ => Permissions::ALL,
    }
}

/// The edge of the rail's dock; the rail is on the start or the end side, never above or below.
fn rail_edge(side: DockSide) -> DockEdge {
    if side == DockSide::Start { DockEdge::Start } else { DockEdge::End }
}

/// Share of the window the start dock takes.
const START_SHARE: f32 = 0.2;
/// Share of the rest the page area takes beside the end dock.
const PAGE_SHARE: f32 = 0.8;

/// The Tools pane's default width. Production opened it at 272 points; the Comfortable density's 30-point icon buttons and
/// 10-point gaps need about 55 more for Format text's size field to keep room for its value beside the colour well, the list
/// buttons and the alignment menu (the field takes what the row leaves).
const TOOLS_WIDTH: f32 = 330.0;

/// The arrangement of a new install: Tools at the start edge, the side-panel rail at the window edge,
/// and the toolbar floating at the start of the page area. The side panels form one collapsed group.
pub fn default_workspace(rail_side: DockSide) -> Workspace<PanelId> {
    let single = |panel| Column::new(vec![Group::new(vec![panel], 0)]);
    // The rail is one tool column wide: its column keeps the toolbar's grid, never the theme's column width.
    // The Tools pane opens at production's width (272 points, the old `TOOLS_WIDTH`), not the theme's column width.
    let (tools, toolbar) = (single(PanelId::Tools).with_width(TOOLS_WIDTH), single(PanelId::Toolbar).with_steps(1));
    let side = Column::new(vec![Group::new(SIDE.to_vec(), 0)]).with_iconic(true);
    let (start, end) = if rail_edge(rail_side) == DockEdge::Start { (vec![side, tools], Vec::new()) } else { (vec![tools], vec![side]) };
    let page = Node::split(Axis::X, SplitSize::Ratio(PAGE_SHARE), Node::tabs(vec![PanelId::Document], 0), Node::dock(DockEdge::End, end));
    let root = Node::split(Axis::X, SplitSize::Ratio(START_SHARE), Node::dock(DockEdge::Start, start), page);
    let rect = egui::Rect::from_min_size(egui::pos2(TOOLS_WIDTH + 3.0, 8.0), egui::vec2(48.0, 400.0));
    let floating = Floating::new(toolbar, rect).with_id(1);
    Workspace::new(Layout::new(Some(root), vec![floating]))
}

/// The floating quick toolbar sits in the left margin of the page area, as production's does. The page area starts where
/// the Tools pane ends, so the toolbar moves by the pane's width when the pane closes and opens, and stays clear of the
/// page text. A toolbar the user has dragged elsewhere (not in the strip the pane's edge defines) stays where it is.
fn keep_toolbar_in_the_margin(app: &mut PdfCraftApp) {
    let open = app.dock.layout.contains(&PanelId::Tools);
    if open == app.toolbar_tools_open {
        return;
    }
    app.toolbar_tools_open = open;
    let Some(window) = app.dock.layout.floating.iter_mut().find(|window| window.contains(&PanelId::Toolbar)) else { return };
    let x = window.rect.min.x;
    let target = if open { (x < TOOLS_WIDTH).then_some(x + TOOLS_WIDTH) } else { (x >= TOOLS_WIDTH).then_some((x - TOOLS_WIDTH).max(0.0)) };
    if let Some(x) = target.filter(|x| x.is_finite()) {
        window.rect = window.rect.translate(egui::vec2(x - window.rect.min.x, 0.0));
    }
}

/// The window edge whose dock holds `panel`, `None` when it is closed or floating.
fn edge_of(workspace: &Workspace<PanelId>, panel: PanelId) -> Option<DockEdge> {
    let mut pending: Vec<(&Node<PanelId>, usize)> = workspace.layout.root.iter().map(|node| (node, 0)).collect();
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        match node {
            Node::Split { first, second, .. } => {
                pending.push((&**first, depth + 1));
                pending.push((&**second, depth + 1));
            }
            Node::Dock { side, columns, .. } => {
                if columns.iter().any(|column| column.panels().any(|p| *p == panel)) {
                    return Some(*side);
                }
            }
            _ => {}
        }
    }
    None
}

/// The columns of the dock at `edge`, from the window edge inward; empty when there is no such dock.
fn dock_columns(workspace: &Workspace<PanelId>, edge: DockEdge) -> &[Column<PanelId>] {
    let mut pending: Vec<(&Node<PanelId>, usize)> = workspace.layout.root.iter().map(|node| (node, 0)).collect();
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        match node {
            Node::Split { first, second, .. } => {
                pending.push((&**first, depth + 1));
                pending.push((&**second, depth + 1));
            }
            Node::Dock { side, columns, .. } if *side == edge => return columns,
            _ => {}
        }
    }
    &[]
}

/// Where a column moved to the `edge` dock goes: a dock holding columns takes it as a new column of its own (a new
/// dock is refused there), inward of the rail as in the default arrangement, else at the window edge.
fn edge_target(workspace: &Workspace<PanelId>, edge: DockEdge) -> Target<PanelId> {
    let columns = dock_columns(workspace, edge);
    if columns.iter().any(|column| column.panels().any(|p| *p == PanelId::Toolbar)) {
        return Target::at(PanelId::Toolbar, DockPlacement::column(true));
    }
    match columns.first().and_then(|column| column.panels().next()) {
        Some(outer) => Target::at(*outer, DockPlacement::column(false)),
        None => Target::new_dock(edge),
    }
}

fn edge_name(edge: Option<DockEdge>) -> &'static str {
    if edge == Some(DockEdge::End) { "right" } else { "left" }
}

/// The side panel that is open, if any: the front tab of the side panels' expanded column. Collapsed to its icons,
/// the column shows none.
fn open_side(workspace: &Workspace<PanelId>) -> Option<PanelId> {
    SIDE.into_iter().find(|panel| workspace.layout.panel_state(panel) == PanelState::Front)
}

/// The label of a side panel, which its icon in the collapsed column and its tab share: production's rail button name.
fn side_label(panel: RightPanel) -> String {
    let title = match panel {
        RightPanel::Comments => "Comments",
        RightPanel::Bookmarks => "Bookmarks",
        RightPanel::Pages => "Page thumbnails",
        RightPanel::Fields => "Form fields",
        RightPanel::Layers => "Layers",
        RightPanel::Attachments => "Attachments",
        RightPanel::Signatures => "Signatures",
        RightPanel::Accessibility => "Accessibility Checker",
        RightPanel::Tags => "Tags",
        RightPanel::Search => "Search",
        RightPanel::Compare => "Compare",
    };
    crate::i18n::t(title).to_owned()
}

/// The icon of a side panel's cell in its collapsed column, production's rail icon.
fn side_icon(panel: RightPanel) -> &'static str {
    match panel {
        RightPanel::Comments => "message-square-text",
        RightPanel::Bookmarks => "bookmark",
        RightPanel::Pages => "files",
        RightPanel::Fields => "text-cursor-input",
        RightPanel::Layers => "layers",
        RightPanel::Attachments => "paperclip",
        RightPanel::Signatures => "signature",
        RightPanel::Accessibility => "accessibility",
        RightPanel::Tags => "tag",
        RightPanel::Search => "search",
        RightPanel::Compare => "git-compare",
    }
}

/// The dock's panels over the app in one pass.
struct Panels<'a> {
    app: &'a mut PdfCraftApp,
    ctx: egui::Context,
}

impl DockPanels<PanelId> for Panels<'_> {
    fn spec(&self, panel: &PanelId) -> PanelSpec {
        if *panel == PanelId::Toolbar {
            return crate::hotbar::panel_spec(&*self.app, &self.ctx);
        }
        if *panel == PanelId::Document {
            return PanelSpec::new(crate::i18n::t("Document").to_owned()).canvas();
        }
        let command = CommandRef::with_params("view.panel", json!({ "panel": panel.name() }));
        match panel.side() {
            Some(side) => {
                let mut spec = PanelSpec::new(side_label(side)).icon(IconSpec::svg(IconId::new(side_icon(side)))).command(command);
                if side == RightPanel::Comments
                    && let Some(info) = self.app.active_ids().and_then(|(_, id)| self.app.session.get(id)).map(|doc| &doc.info)
                {
                    spec = spec.count(crate::comments_panel::count(info));
                    let replies = crate::comments_panel::reply_count(info);
                    if replies > 0 {
                        let text = crate::i18n::trn(crate::i18n::current(), replies as u64, "{n} reply", "{n} replies");
                        spec = spec.caption(text);
                    }
                }
                spec
            }
            None if self.app.left == LeftPanel::CalculationOrder => PanelSpec::new(crate::i18n::t("Calculation order").to_owned()).command(command),
            None => {
                // The panel is headed by what it shows: the tool's name, or All tools (production's heading).
                let title = match self.app.left {
                    LeftPanel::Tool(id) => super::catalog::group(id).map_or_else(|| crate::i18n::t("All tools"), |g| crate::i18n::t(g.label)),
                    _ => crate::i18n::t("All tools"),
                };
                let spec = PanelSpec::new(title.to_owned()).command(command);
                if super::tools::has_footer(self.app) { spec.footer() } else { spec }
            }
        }
    }

    fn footer(&mut self, panel: &PanelId, ui: &mut Ui, _cx: &mut BodyCx<'_>) {
        if *panel == PanelId::Tools {
            super::tools::footer(self.app, ui);
        }
    }

    fn rail_footer(&mut self, _: &PanelId, ui: &mut Ui, _: &mut BodyCx<'_>) {
        crate::chrome::view_rail::show(self.app, ui);
    }

    fn permissions(&self, panel: &PanelId) -> Permissions {
        permissions(panel)
    }

    fn body(&mut self, panel: &PanelId, ui: &mut Ui, cx: &mut BodyCx<'_>) {
        match panel {
            PanelId::Tools => crate::panels::left_panel(self.app, ui),
            // The rail and the page area are their groups' `group_view`.
            PanelId::Toolbar | PanelId::Document => {}
            side => {
                if let Some(side) = side.side() {
                    super::right::panel_body(self.app, ui, cx, side);
                }
            }
        }
    }

    fn group_view(&mut self, panels: &[PanelId], _active: usize, ui: &mut Ui, at: GroupAt, _cx: &mut BodyCx<'_>) -> Option<f32> {
        match at {
            GroupAt::Measure(width) if panels.contains(&PanelId::Toolbar) => Some(crate::hotbar::measure_rail(self.app, ui, width)),
            GroupAt::Show(rect) if panels.contains(&PanelId::Toolbar) => {
                crate::hotbar::show_rail(self.app, ui);
                Some(rect.height())
            }
            GroupAt::Show(rect) if panels.contains(&PanelId::Document) => {
                document_view(self.app, ui, rect);
                Some(rect.height())
            }
            _ => None,
        }
    }

    fn group_header(&mut self, panels: &[PanelId], active: usize, ui: &mut Ui, _cx: &mut BodyCx<'_>) {
        let Some(panel) = panels.get(active) else { return };
        match panel {
            // PLAN: assumed `calc_order_ui::header(ui)` (pdf-forms-w12).
            PanelId::Tools => {
                if self.app.left == LeftPanel::CalculationOrder {
                    crate::calc_order_ui::header(ui);
                } else if crate::organize_panel::showing(self.app) {
                    IconButton::command(IconSpec::svg(IconId::new("chevron-left")), CommandRef::new("tools.all")).show(ui);
                    IconButton::command(IconSpec::svg(IconId::new("x")), CommandRef::new("page.organize")).label(tl!("Close panel")).show(ui);
                } else if self.app.left == LeftPanel::AllTools {
                    IconButton::command(IconSpec::svg(IconId::new("maximize-2")), CommandRef::new("tools.expand")).show(ui);
                }
            }
            PanelId::Bookmarks => super::bookmarks::header(self.app, ui),
            PanelId::Comments => self.comments_header(ui),
            _ => {}
        }
    }

    fn panel_rows(&mut self, panel: &PanelId) -> Vec<PanelRow> {
        match panel {
            PanelId::Pages => super::pages::tools_menu(&*self.app),
            PanelId::Layers => super::layers::menu_rows(&*self.app),
            PanelId::Bookmarks => super::bookmarks::options_menu(&*self.app),
            _ => Vec::new(),
        }
    }
}

impl Panels<'_> {
    fn comments_header(&mut self, ui: &mut Ui) {
        let Some((index, id)) = self.app.active_ids() else { return };
        let enabled: Vec<&str> = ["comment.hide_all", "comment.summarize", "comment.export", "comment.import", "comment.flatten"]
            .into_iter()
            .filter(|id| pdfcraft_engine::commands::command(id).is_some_and(|spec| self.app.command_enabled(spec)))
            .collect();
        let command = {
            let Some(doc) = self.app.session.get(id) else { return };
            crate::comments_panel::header(ui, doc, &mut self.app.views[index], &self.app.comment_prefs, |id| enabled.contains(&id))
        };
        if let Some(command) = command {
            self.app.run_command(&ui.ctx().clone(), command);
        }
    }
}

/// The document panel's view in `rect`: the active document alone on black in full screen, the OCR Verify screen
/// (its four panes under the window's tabs, menus and toolbar, which stay, #6485), the expanded All tools view, or
/// the page area.
fn document_view(app: &mut PdfCraftApp, ui: &mut Ui, rect: egui::Rect) {
    app.settle_document_view();
    if app.full_screen_document() {
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        if let Some(i) = app.active {
            match crate::compare_ui::side_by_side(app, i) {
                Some((old, new)) => crate::compare_ui::compare_area(app, old, new, ui),
                None => crate::canvas::document_area(app, i, ui),
            }
        }
    } else if app.ocr_verify.open {
        crate::ocr_verify::show(app, ui);
    } else if app.mode == Mode::AllTools && app.all_tools_full && !app.hidden_interface().window_ui {
        crate::all_tools_ui::show(app, ui);
    } else {
        crate::panes::page_area(app, ui);
    }
}

/// The permissions alone, for the automation method that applies a dock action without drawing the dock.
struct Rules;

impl DockPanels<PanelId> for Rules {
    fn spec(&self, panel: &PanelId) -> PanelSpec {
        PanelSpec::new(panel.name())
    }

    fn permissions(&self, panel: &PanelId) -> Permissions {
        permissions(panel)
    }

    fn body(&mut self, _panel: &PanelId, _ui: &mut Ui, _cx: &mut BodyCx<'_>) {}
}

/// A visible panel in the navigation rail anchors the fixed view controls. If all panels have
/// moved away or are hidden, the shell keeps the controls available on its own rail.
pub(crate) fn rail_anchor(app: &PdfCraftApp) -> Option<PanelId> {
    if app.active.is_none() || app.full_screen_document() || app.hidden_interface().window_ui || (app.mode == Mode::AllTools && app.all_tools_full) {
        return None;
    }
    let open = open_side(&app.dock);
    let checked = app.active_ids().is_some_and(|(_, id)| app.a11y.report.as_ref().is_some_and(|(d, _)| *d == id));
    dock_columns(&app.dock, rail_edge(app.rail_side)).iter().flat_map(|column| column.panels()).copied().find(|panel| {
        SIDE.contains(panel)
            && match panel {
                PanelId::Accessibility => checked || open == Some(*panel),
                PanelId::Search | PanelId::Compare => open == Some(*panel),
                _ => true,
            }
    })
}

/// Draws the dock. Call after the top bars and before the central panel.
///
/// The area gets a copy of the workspace and reports every change of the pass; they are applied to `app.dock` here,
/// the area's own actions first, then the ones the bodies requested. A pass applies only what it reported.
pub fn dock(app: &mut PdfCraftApp, ui: &mut Ui) {
    let active = app.active.is_some();
    let read = app.mode == Mode::Read;
    // The expanded All tools view covers the panels but keeps the rail.
    let expanded = app.mode == Mode::AllTools && app.all_tools_full;
    // Full screen shows the document alone, and a document can hide the toolbars, the menu and the panels around it
    // in its tab (ISO 32000-2 Table 147); the tab strip stays, and F8 brings the rest back (`viewer_prefs`).
    let bare = app.full_screen_document() || app.hidden_interface().window_ui;
    let organizing = app.active_ids().and_then(|(index, _)| app.views.get(index)).is_some_and(|view| view.organize);
    let toolbar = active && !app.hotbar_hidden() && !organizing && !expanded;
    let anchor = rail_anchor(app);
    keep_toolbar_in_the_margin(app);
    let workspace = app.dock.clone();
    // The rail lists the Accessibility Checker once the document has a report, and Search and Compare only while
    // they show; their commands open them.
    let open = open_side(&workspace);
    let checked = app.active_ids().is_some_and(|(_, id)| app.a11y.report.as_ref().is_some_and(|(d, _)| *d == id));
    let listed = |panel: &PanelId| match panel {
        PanelId::Accessibility => checked || open == Some(*panel),
        PanelId::Search | PanelId::Compare => open == Some(*panel),
        _ => true,
    };
    let inset = app.notice_inset;
    let mut panels = Panels { ctx: ui.ctx().clone(), app: &mut *app };
    // Home and Combine have no side panel and no toolbar, and Read mode hides the tools and the quick toolbar, not the
    // navigation panels. A document that hides the window controls keeps the quick toolbar on its pages; full screen
    // hides it (`hotbar_hidden`), and so does the expanded All tools view, which takes the pages' place.
    let shown = |panel: &PanelId| match panel {
        PanelId::Document => true,
        PanelId::Toolbar => toolbar,
        _ if bare => false,
        PanelId::Tools => !read && !expanded,
        _ => active && !expanded && listed(panel),
    };
    // The side panels' icons are production's rail: they stay beside the expanded column, a click on one opens its
    // panel docked, and a click on the open panel's icon closes it.
    let config = DockConfig::default().native_windows(false).iconic_mode(IconicMode::Rail).titled_headers(true).tab_close_buttons(true);
    let area = DockArea::new("Dock", Bind::given(workspace), &mut panels, config).hide(HidePanels::Except(&shown)).top_inset(inset);
    let output = match anchor {
        Some(panel) => area.rail_footer(panel).show(ui),
        None => area.show(ui),
    };
    let before = right_of(&app.dock);
    for action in output.actions.into_iter().chain(output.requested) {
        // The area drops what the workspace refuses; one refused here is stale, not the user's mistake.
        drop(app.dock.apply(action, false, permissions));
    }
    let after = right_of(&app.dock);
    if after != before {
        app.right_panel_chosen(before, after);
    }
    if let Some(panel) = open_side(&app.dock).and_then(PanelId::side) {
        app.last_right = panel;
    }
}

/// What `ui.panel` and `ui.state` report.
pub fn state(app: &PdfCraftApp) -> Value {
    let side = open_side(&app.dock);
    json!({
        "tools_side": edge_name(edge_of(&app.dock, PanelId::Tools)),
        "tools_open": app.dock.layout.contains(&PanelId::Tools),
        "rail_side": edge_name(Some(rail_edge(app.rail_side))),
        "side_open": side.is_some(),
        "side_panel": side.and_then(PanelId::side).map(|r| format!("{r:?}")),
    })
}

/// The reply when the dock does not allow a change to a panel.
fn refused() -> String {
    crate::i18n::t("The dock does not allow that change to this panel").to_owned()
}

/// Opens or closes `panel` in the workspace, the one write path for panel visibility.
fn show_panel(app: &mut PdfCraftApp, panel: PanelId, visible: bool) -> Result<(), String> {
    set_shown(&mut app.dock, panel, visible).map_err(|_| refused())
}

/// Shows or hides `panel` in `workspace`. A side panel stays in the layout as an icon of its column: showing it
/// expands the column with it in front, hiding it collapses the column to its icons. One closed from its tab comes
/// back to its recorded place first.
fn set_shown(workspace: &mut Workspace<PanelId>, panel: PanelId, visible: bool) -> Result<(), DockError> {
    let fallback = (panel != PanelId::Toolbar && workspace.layout.contains(&PanelId::Toolbar)).then_some(PanelId::Toolbar);
    if panel.side().is_none() {
        return workspace.set_visible(panel, visible, fallback, false, permissions);
    }
    if visible {
        workspace.set_visible(panel, true, fallback, false, permissions)?;
        return workspace.apply(Action::set_iconic(panel, false), false, permissions).map(drop);
    }
    if workspace.layout.panel_state(&panel) != PanelState::Front {
        return Ok(());
    }
    workspace.apply(Action::set_iconic(panel, true), false, permissions).map(drop)
}

/// The side panel `workspace` has open, `None` when every side panel is closed.
pub(crate) fn right_of(workspace: &Workspace<PanelId>) -> Option<RightPanel> {
    open_side(workspace).and_then(PanelId::side)
}

/// Opens or closes `panel`, telling the user when the dock refuses.
pub(crate) fn set_panel_visible(app: &mut PdfCraftApp, panel: PanelId, visible: bool) {
    if let Err(error) = show_panel(app, panel, visible) {
        app.notify(error);
    }
}

/// Whether the left panel `panel` is the one the tool panels show and the dock has them open.
pub(crate) fn panel_visible(app: &PdfCraftApp, panel: LeftPanel) -> bool {
    app.left == panel && app.dock.layout.contains(&PanelId::Tools)
}

/// Moves the column holding `panel` to the `edge` dock.
fn move_to_edge(app: &mut PdfCraftApp, panel: PanelId, edge: DockEdge) -> Result<(), String> {
    if !app.dock.layout.contains(&panel) {
        return Err(format!("open the {} panel first", panel.name()));
    }
    if edge_of(&app.dock, panel) == Some(edge) {
        return Ok(());
    }
    let to = if SIDE.contains(&panel) && edge_of(&app.dock, PanelId::Toolbar) == Some(edge) {
        Target::at(PanelId::Toolbar, DockPlacement::column(false))
    } else {
        edge_target(&app.dock, edge)
    };
    app.dock.apply(Action::move_column(panel, to), false, permissions).map(drop).map_err(|_| refused())
}

/// Puts the rail, and the side panels that share its edge, on `side`.
pub(crate) fn set_rail_side(app: &mut PdfCraftApp, side: DockSide) -> Result<(), String> {
    let edge = rail_edge(side);
    for panel in [PanelId::Toolbar].into_iter().chain(SIDE) {
        if app.dock.layout.contains(&panel) {
            move_to_edge(app, panel, edge)?;
        }
    }
    app.rail_side = side;
    Ok(())
}

/// Whether the tools panel is docked at the end edge; `false` at the start edge and while it is closed.
pub(crate) fn tools_on_right(app: &PdfCraftApp) -> bool {
    edge_of(&app.dock, PanelId::Tools) == Some(DockEdge::End)
}

/// Docks the tools panel at the end (`on_right`) or the start edge. A closed panel has no edge to move: it stays closed.
pub(crate) fn set_tools_on_right(app: &mut PdfCraftApp, on_right: bool) -> Result<(), String> {
    if !app.dock.layout.contains(&PanelId::Tools) {
        return Ok(());
    }
    move_to_edge(app, PanelId::Tools, if on_right { DockEdge::End } else { DockEdge::Start })
}

fn edge_param(value: &str, key: &str) -> Result<DockEdge, String> {
    match value {
        "left" => Ok(DockEdge::Start),
        "right" => Ok(DockEdge::End),
        _ => Err(format!("{key} must be left or right")),
    }
}

/// `ui.panel {panel, open?, edge?}`, `ui.panel {rail_side}` and `ui.dock.reset` (`reset`).
///
/// A request is checked whole before anything changes. Panel sizes are not parameters: they are the dock's, set by
/// dragging a divider or by `ui.dock.apply`.
pub fn command(app: &mut PdfCraftApp, reset: bool, params: &Value) -> Result<Value, String> {
    if reset {
        app.dock = default_workspace(app.rail_side);
        return Ok(json!({ "side_panels": state(app) }));
    }
    if params.get("width").is_some_and(|width| !width.is_null()) {
        return Err("panel sizes are set by dragging a divider or by ui.dock.apply".into());
    }
    let rail = match params.get("rail_side") {
        None | Some(Value::Null) => None,
        Some(Value::String(side)) if side == "toggle" => {
            Some(if rail_edge(app.rail_side) == DockEdge::Start { DockSide::End } else { DockSide::Start })
        }
        Some(value) => {
            let edge = edge_param(value.as_str().unwrap_or_default(), "rail_side")?;
            Some(if edge == DockEdge::Start { DockSide::Start } else { DockSide::End })
        }
    };
    let panel = match params.get("panel") {
        None | Some(Value::Null) if rail.is_some() => None,
        value => {
            let name = value.and_then(Value::as_str).ok_or("panel must be \"tools\", \"toolbar\", \"side\" or a side panel's name")?;
            Some(if name == "side" {
                PanelId::from(app.last_right)
            } else {
                PanelId::parse(name).ok_or("panel must be \"tools\", \"toolbar\", \"side\" or a side panel's name")?
            })
        }
    };
    let open = match params.get("open") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_bool().ok_or("open must be a boolean")?),
    };
    let edge = match params.get("edge") {
        None | Some(Value::Null) => None,
        Some(value) => Some(edge_param(value.as_str().unwrap_or_default(), "edge")?),
    };
    if let Some(panel) = panel {
        if let Some(open) = open {
            show_panel(app, panel, open)?;
        }
        if let Some(edge) = edge {
            move_to_edge(app, panel, edge)?;
        }
    }
    if let Some(side) = rail {
        set_rail_side(app, side)?;
    }
    Ok(json!({ "side_panels": state(app) }))
}

/// `ui.dock.apply {action}`: one dock action applied to the workspace under the panels' permissions.
pub fn dock_method(app: &mut PdfCraftApp, method: &str, params: &Value) -> Option<Result<Value, String>> {
    dock_rpc::dock_method(&mut app.dock, &Rules, false, method, params)
}

/// The dock's layout for `ui.inspect`.
pub fn dock_inspect(app: &PdfCraftApp) -> Value {
    dock_rpc::inspect(&app.dock)
}

/// What [`migrate`] read from settings written before the dock.
pub(crate) struct Migrated {
    /// The arrangement the old layout stood for.
    pub workspace: Workspace<PanelId>,
    /// The side the old rail was on.
    pub rail_side: DockSide,
}

/// The old layout's facts, from either old format.
struct Old {
    tools_right: bool,
    rail: DockSide,
    tools_open: bool,
    tools_width: Option<f64>,
    side_open: bool,
    side_width: Option<f64>,
}

/// Where the old docking tree held a panel.
struct Found {
    /// The panel's group held only it: a tab group or stack of its own.
    alone: bool,
    /// The innermost split around the panel's own group.
    split: Option<Split>,
}

struct Split {
    horizontal: bool,
    /// The panel is in the split's second (right or bottom) child.
    second: bool,
    /// The split's fixed size, when it is the panel's side.
    size: Option<f64>,
}

/// The pixel size of `size` (`{"FixedFirst": w}` or `{"FixedSecond": w}`) when it fixes the side
/// the panel is on.
fn fixed_size(size: Option<&Value>, second: bool) -> Option<f64> {
    let key = if second { "FixedSecond" } else { "FixedFirst" };
    size?.get(key)?.as_f64()
}

fn group_ids(node: &Value) -> Option<Vec<&str>> {
    if let Some(tabs) = node.get("Tabs") {
        return Some(tabs.get("panels")?.as_array()?.iter().filter_map(Value::as_str).collect());
    }
    let entries = node.get("Stack")?.get("entries")?.as_array()?;
    Some(entries.iter().filter_map(|entry| entry.get("panel")?.as_str()).collect())
}

fn find(node: &Value, panel: &str, depth: usize) -> Option<Found> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(ids) = group_ids(node) {
        return ids.contains(&panel).then(|| Found { alone: ids.iter().all(|id| *id == panel), split: None });
    }
    let split = node.get("Split")?;
    for (key, second) in [("first", false), ("second", true)] {
        if let Some(mut found) = split.get(key).and_then(|child| find(child, panel, depth + 1)) {
            // A panel sharing its group was placed by its group, not by a split.
            if found.alone && found.split.is_none() {
                let horizontal = split.get("axis").and_then(Value::as_str) == Some("Horizontal");
                found.split = Some(Split { horizontal, second, size: fixed_size(split.get("size"), second) });
            }
            return Some(found);
        }
    }
    None
}

/// Whether `node`'s splits nest deeper than [`find`] walks.
fn too_deep(node: &Value, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    node.get("Split").is_some_and(|split| ["first", "second"].into_iter().any(|key| split.get(key).is_some_and(|child| too_deep(child, depth + 1))))
}

/// The old layout's saved place of a closed panel: whether it was on the right and its width.
fn hidden_place<'a>(old: &'a Value, panel: &str) -> Option<&'a Value> {
    old.get("hidden")?.as_array()?.iter().find_map(|entry| {
        let pair = entry.as_array()?;
        (pair.first()?.as_str()? == panel).then(|| pair.get(1)).flatten()
    })
}

fn floating_contains(old: &Value, panel: &str) -> bool {
    old.get("layout").and_then(|l| l.get("floating")).and_then(Value::as_array).is_some_and(|groups| {
        groups.iter().any(|group| group.get("panels").and_then(Value::as_array).is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(panel))))
    })
}

/// A panel of the old layout: on the right edge, open, and its width where the layout stated it
/// in points. A floating or tabbed panel counts as on the left, as the old Tools side reading did.
fn old_panel(old: &Value, panel: &str) -> (bool, bool, Option<f64>) {
    let root = old.get("layout").and_then(|l| l.get("root"));
    if let Some(found) = root.and_then(|root| find(root, panel, 0)) {
        return match found.split {
            Some(split) if split.horizontal => (split.second, true, split.size),
            _ => (false, true, None),
        };
    }
    if floating_contains(old, panel) {
        return (false, true, None);
    }
    let Some(place) = hidden_place(old, panel) else { return (false, false, None) };
    let right = place.get("placement").and_then(|p| p.get("Split")).and_then(Value::as_str) == Some("Right");
    let left = place.get("placement").and_then(|p| p.get("Split")).and_then(Value::as_str) == Some("Left");
    let size = if right || left { fixed_size(place.get("size"), right) } else { None };
    (right, false, size)
}

/// The `side_panels` value of version 1.
fn read_side_panels(value: &Value) -> Option<Old> {
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let width = |key: &str| value.get(key).and_then(Value::as_f64);
    Some(Old {
        tools_right: text("tools_side") == Some("right"),
        rail: if text("rail_side") == Some("left") { DockSide::Start } else { DockSide::End },
        tools_open: value.get("tools_open").and_then(Value::as_bool).unwrap_or(true),
        tools_width: width("tools_width"),
        side_open: true,
        side_width: width("side_width"),
    })
}

/// The old `docking` value (craft-ui's `Workspace`) as plain JSON. `None` when it has no document view, which the
/// old layout always held.
fn read_docking(saved: &Value) -> Option<Old> {
    let old = saved.get("docking")?;
    let root = old.get("layout")?.get("root")?;
    // A tree deeper than the walk reads is cut off whole, not read as one that lacks its panels.
    if too_deep(root, 0) {
        return None;
    }
    find(root, "canvas", 0)?;
    let (tools_right, tools_open, tools_width) = old_panel(old, "tools");
    let (_, side_open, side_width) = old_panel(old, "inspector");
    Some(Old {
        tools_right,
        rail: DockSide::End,
        tools_open: saved.get("docking_left_open").and_then(Value::as_bool).unwrap_or(tools_open),
        tools_width,
        side_open,
        side_width,
    })
}

/// The workspace `old` stands for, with its pixel widths when `widths`.
fn build(old: &Old, right: Option<RightPanel>, widths: bool) -> Option<Workspace<PanelId>> {
    let mut workspace = default_workspace(old.rail);
    let apply = |workspace: &mut Workspace<PanelId>, action| workspace.apply(action, false, permissions).ok();
    if old.tools_right {
        let to = edge_target(&workspace, DockEdge::End);
        let moved = workspace.apply(Action::move_column(PanelId::Tools, to), false, permissions);
        moved.ok()?;
    }
    let side = right.filter(|_| old.side_open).map(PanelId::from);
    if let Some(side) = side {
        set_shown(&mut workspace, side, true).ok()?;
    }
    if !old.tools_open {
        workspace.set_visible(PanelId::Tools, false, None, false, permissions).ok()?;
    }
    if widths {
        if let (true, Some(width)) = (old.tools_open, old.tools_width) {
            apply(&mut workspace, Action::resize_column(PanelId::Tools, width as f32))?;
        }
        if let (Some(side), Some(width)) = (side, old.side_width) {
            apply(&mut workspace, Action::resize_column(side, width as f32))?;
        }
    }
    Some(workspace)
}

/// Reads the layout of settings written before the dock: `side_panels`, else the older `docking`, with the side panel
/// that was open in `docking_right`. Nothing in it is an error: a malformed or unknown value gives `None`, and the
/// caller takes [`default_workspace`]. A width the dock refuses is dropped and the rest kept.
pub(crate) fn migrate(saved: &Value) -> Option<Migrated> {
    let old = match saved.get("side_panels") {
        Some(value) => read_side_panels(value)?,
        None => read_docking(saved)?,
    };
    let right = saved.get("docking_right").and_then(|v| serde_json::from_value::<Option<RightPanel>>(v.clone()).ok()).flatten();
    let workspace = build(&old, right, true).or_else(|| build(&old, right, false))?;
    Some(Migrated { workspace, rail_side: old.rail })
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn new_workspace_floats_the_toolbar() {
        for side in [DockSide::Start, DockSide::End] {
            let workspace = default_workspace(side);
            assert!(workspace.layout.validate().is_ok());
            assert_eq!(edge_of(&workspace, PanelId::Toolbar), None);
            assert!(workspace.layout.floating.iter().any(|window| window.contains(&PanelId::Toolbar)));
        }
    }

    fn tabs(panels: &[&str]) -> Value {
        json!({"Tabs": {"panels": panels, "active": 0}})
    }

    fn split(size: Value, first: Value, second: Value) -> Value {
        json!({"Split": {"axis": "Horizontal", "size": size, "first": first, "second": second}})
    }

    fn workspace(root: Value, floating: Value, hidden: Value) -> Value {
        json!({"docking": {"layout": {"root": root, "floating": floating}, "hidden": hidden}})
    }

    fn with_side(mut saved: Value, panel: &str) -> Value {
        saved["docking_right"] = json!(panel);
        saved
    }

    /// An old layout migrates to the panels' edges and open state, whatever the old widths were.
    #[test]
    fn an_old_saved_layout_migrates_to_the_dock_workspace() {
        let hidden_place = |side: &str, size: f64| json!({"floating": null, "anchor": "canvas", "placement": {"Split": side}, "size": {"FixedSecond": size}, "stack": null, "sibling": ["canvas"]});
        // (case, saved, Tools open, Tools edge, side panel open)
        let cases = [
            (
                "docked default",
                with_side(
                    workspace(
                        split(
                            json!({"FixedFirst": 300.0}),
                            tabs(&["tools"]),
                            split(json!({"FixedSecond": 400.0}), tabs(&["canvas"]), tabs(&["inspector"])),
                        ),
                        json!([]),
                        json!([]),
                    ),
                    "Bookmarks",
                ),
                true,
                DockEdge::Start,
                true,
            ),
            (
                "tools on the right",
                workspace(
                    split(
                        json!({"FixedSecond": 330.0}),
                        tabs(&["canvas"]),
                        split(json!({"FixedSecond": 290.0}), tabs(&["inspector"]), tabs(&["tools"])),
                    ),
                    json!([]),
                    json!([]),
                ),
                true,
                DockEdge::End,
                false,
            ),
            (
                "floated tools",
                workspace(
                    split(json!({"FixedSecond": 360.0}), tabs(&["canvas"]), tabs(&["inspector"])),
                    json!([{"panels": ["tools"], "active": 0, "rect": [10.0, 20.0, 300.0, 400.0]}]),
                    json!([]),
                ),
                true,
                DockEdge::Start,
                false,
            ),
            (
                "tools tabbed with the inspector",
                with_side(
                    workspace(split(json!({"FixedSecond": 350.0}), tabs(&["canvas"]), tabs(&["inspector", "tools"])), json!([]), json!([])),
                    "Pages",
                ),
                true,
                DockEdge::Start,
                true,
            ),
            (
                "hidden inspector and tools",
                with_side(
                    workspace(
                        tabs(&["canvas"]),
                        json!([]),
                        json!([["inspector", hidden_place("Right", 380.0)], ["tools", hidden_place("Right", 300.0)]]),
                    ),
                    "Bookmarks",
                ),
                false,
                DockEdge::End,
                false,
            ),
            (
                "version 1 value",
                with_side(
                    json!({"side_panels": {"version": 1, "tools_side": "right", "tools_open": true, "tools_width": 300.0, "side_width": 400.0}}),
                    "Pages",
                ),
                true,
                DockEdge::End,
                true,
            ),
        ];
        for (name, saved, tools_open, edge, side_open) in cases {
            let migrated = migrate(&saved).unwrap_or_else(|| panic!("{name}: not migrated"));
            assert_eq!(migrated.workspace.layout.contains(&PanelId::Tools), tools_open, "{name}: Tools open");
            if tools_open {
                assert_eq!(edge_of(&migrated.workspace, PanelId::Tools), Some(edge), "{name}: Tools edge");
            }
            assert_eq!(open_side(&migrated.workspace).is_some(), side_open, "{name}: side panel open");
            assert_eq!(migrated.rail_side, DockSide::End, "{name}: rail side");
        }
        let left = migrate(&json!({"side_panels": {"version": 1, "rail_side": "left"}})).expect("migrates");
        assert_eq!(left.rail_side, DockSide::Start);
    }

    #[test]
    fn malformed_values_give_the_default_workspace() {
        let default = default_workspace(DockSide::End);
        let take = |saved: &Value| migrate(saved).map_or_else(|| default.clone(), |m| m.workspace);
        for saved in [
            json!({"docking": "not a layout"}),
            json!({"docking": {"layout": [1, 2]}}),
            json!({"docking": {"layout": {"root": {"Split": {"axis": 3, "size": {"FixedFirst": "wide"}, "first": null}}}}}),
            json!({"side_panels": {"version": 2, "tools_side": "right"}}),
            json!({"side_panels": "right"}),
        ] {
            assert_eq!(take(&saved), default, "{saved}");
        }
        // Deep trees are cut off rather than walked.
        let mut deep = tabs(&["tools"]);
        for _ in 0..200 {
            deep = split(json!({"Ratio": 0.5}), tabs(&["canvas"]), deep);
        }
        assert_eq!(take(&workspace(deep, json!([]), json!([]))), default);
    }

    fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = PdfCraftApp::for_unit_tests();
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                setup(&mut app);
                app
            }
        });
        h.run_steps(6);
        h
    }

    fn toolbar_x(h: &Harness<'static, PdfCraftApp>) -> f32 {
        h.state().dock.layout.floating.iter().find(|window| window.contains(&PanelId::Toolbar)).map_or(f32::NAN, |window| window.rect.min.x)
    }

    /// L-310: the floating quick toolbar stays in the page area's left margin: closing the Tools pane carries it to the
    /// window's start with the page area, and opening the pane puts it back beside the pane, never over the page text.
    #[test]
    fn the_toolbar_follows_the_page_areas_left_edge() {
        let mut closed = harness(|app| app.set_left_open(false));
        assert!(toolbar_x(&closed) < TOOLS_WIDTH * 0.5, "left closed: the toolbar at {}", toolbar_x(&closed));
        closed.state_mut().set_left_open(true);
        closed.run_steps(4);
        assert!(toolbar_x(&closed) > TOOLS_WIDTH, "left open again: the toolbar at {}", toolbar_x(&closed));
        let open = harness(|_| {});
        assert!(toolbar_x(&open) > TOOLS_WIDTH, "the default arrangement keeps the toolbar beside the Tools pane");
    }

    fn tools_x(h: &Harness<'static, PdfCraftApp>) -> f32 {
        h.get_by_label("Expand all tools").rect().center().x
    }

    /// A panel moved to the other edge is there again in a new app restored from the saved settings.
    #[test]
    fn saved_value_round_trips() {
        let default = harness(|_| {});
        assert!(tools_x(&default) < 700.0, "Tools start at the left");
        let mut moved = harness(|app| {
            command(app, false, &json!({"panel": "tools", "edge": "right"})).unwrap();
        });
        moved.run_steps(3);
        assert!(tools_x(&moved) > 700.0);
        let snapshot = moved.state().settings_snapshot();
        let restored = harness(move |app| app.apply_settings(&snapshot));
        assert!(tools_x(&restored) > 700.0);
    }
}
