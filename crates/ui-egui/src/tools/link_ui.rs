//! Edit a PDF ▸ Link: drag a rectangle to create a link, then choose its appearance and action
//! (go to a page, open a web page) in Link Properties; with the Link tool, existing links show
//! their outlines, a click selects one, a double-click edits it and Delete removes it.

use craft::behavior::kurbo::{BezPath, Point};
use craft::behavior::{CancelReason, Cursor, CursorIcon, PointerButton, PressPhase, PressState};
use craft::pages::ui::{PageBody, PlacedPage};
use craft::theme::Srgba;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::canvas_controls::SelectionOutline;
use craft::ui::choice::{Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::form::{Field, FieldGroup, Help};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::Select;
use craft::ui::primitives::Draw;
use craft::ui::runtime::requests::ColorRequest;
use craft::ui::text::{NumberField, TextField};
use egui::{Pos2, Rect};
use pdfcraft_engine::{Edit, LinkAction, LinkHighlight, LinkItem, LinkStyle, LinkZoom};
use pdfcraft_render::{DocInfo, LinkTarget};

use crate::PdfCraftApp;
use crate::canvas::PageSession;
use crate::canvas::page_overlays::LinkEvent;
use crate::colour_picker::PickerHost;
use crate::comments::{RecentColours, palette, to_rgb};

/// How far around a link (screen points) the Link tool finds it, so a thin one can be picked.
pub(crate) const LINK_TOOL_MARGIN: f32 = 2.0;

/// The least size (screen points, each side) a dragged rectangle has to become a link.
const MIN_LINK_SIDE: f32 = 4.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkView {
    pub selected: Option<(usize, usize)>,
    /// The link the last right-click on a page hit (page, index), if any: the canvas
    /// context menu shows the link items only for it, so a right-click elsewhere (on a
    /// comment, say) keeps its own menu even while a link stays selected.
    pub context: Option<(usize, usize)>,
    /// Ask the app to open Link Properties for a new rect (page, user rect) or an existing
    /// link (page, index).
    pub open_new: Option<(usize, [f64; 4])>,
    pub open_existing: Option<(usize, usize)>,
}

/// Link Properties' working copy.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkDraft {
    pub page: usize,
    pub rect: [f64; 4],
    /// `None` for a new link.
    pub index: Option<usize>,
    /// Keep an existing action that Link Properties cannot edit.
    pub preserve_action: bool,
    pub web: bool,
    pub url: String,
    /// 1-based target page.
    pub target: usize,
    /// How the target page is shown.
    pub zoom: LinkZoom,
    /// The link's action when the dialog opened (`None` for a new link). OK rewrites the action
    /// only when it changed, so an appearance edit keeps a destination's position and kind
    /// (`/XYZ left top`, `/FitR`…) that the dialog doesn't show.
    pub current: Option<LinkAction>,
    pub style: LinkStyle,
}

/// The draft of a new link: a web link to `https://`, with the default style.
impl Default for LinkDraft {
    fn default() -> Self {
        LinkDraft {
            page: 0,
            rect: [0.0; 4],
            index: None,
            preserve_action: false,
            web: true,
            url: "https://".into(),
            target: 1,
            zoom: LinkZoom::FitPage,
            current: None,
            style: LinkStyle::default(),
        }
    }
}

/// The zooms Link Properties offers for a page view.
const ZOOMS: [LinkZoom; 12] = [
    LinkZoom::FitPage,
    LinkZoom::FitWidth,
    LinkZoom::FitHeight,
    LinkZoom::FitVisible,
    LinkZoom::Percent(100.0),
    LinkZoom::Inherit,
    LinkZoom::Percent(50.0),
    LinkZoom::Percent(75.0),
    LinkZoom::Percent(125.0),
    LinkZoom::Percent(150.0),
    LinkZoom::Percent(200.0),
    LinkZoom::Percent(400.0),
];

fn zoom_label(zoom: LinkZoom) -> String {
    match zoom {
        LinkZoom::FitPage => tl!("Fit Page").to_string(),
        LinkZoom::FitWidth => tl!("Fit Width").to_string(),
        LinkZoom::FitHeight => tl!("Fit Height").to_string(),
        LinkZoom::FitVisible => tl!("Fit Visible").to_string(),
        LinkZoom::Inherit => tl!("Inherit Zoom").to_string(),
        LinkZoom::Percent(p) if (p - 100.0).abs() < 1e-9 => tl!("Actual Size").to_string(),
        // Another writer's factor can be anything up to f64::MAX, or a tiny fraction: exponent
        // form keeps it short.
        LinkZoom::Percent(p) if !(0.01..1e6).contains(&p) => format!("{p:.1e}%"),
        // A fractional one (another writer's 0.5% or 62.5%) shows up to two places, so it agrees
        // with what link_list reports instead of rounding to 0%.
        LinkZoom::Percent(p) if p.fract() != 0.0 => {
            let places = format!("{p:.2}");
            format!("{}%", places.trim_end_matches('0').trim_end_matches('.'))
        }
        LinkZoom::Percent(p) => format!("{p:.0}%"),
    }
}

/// A user-space rectangle of `placed`'s page on screen.
pub(crate) fn screen_rect(placed: &PlacedPage, info: &DocInfo, r: [f64; 4]) -> Option<Rect> {
    let page = info.pages.get(placed.page)?;
    let [x0, y0] = page.user_to_view(r[0] as f32, r[1] as f32);
    let [x1, y1] = page.user_to_view(r[2] as f32, r[3] as f32);
    Some(Rect::from_two_pos(placed.to_screen(Point::new(f64::from(x0), f64::from(y0))), placed.to_screen(Point::new(f64::from(x1), f64::from(y1)))))
}

/// The user-space point of `placed`'s page under the screen point `p`.
pub(crate) fn to_user(placed: &PlacedPage, info: &DocInfo, p: Pos2) -> Option<[f64; 2]> {
    let at = placed.to_page(p);
    let u = info.pages.get(placed.page)?.view_to_user(at.x as f32, at.y as f32);
    Some([f64::from(u[0]), f64::from(u[1])])
}

/// What a link's tip says it goes to.
pub(crate) fn target_label(info: &DocInfo, target: &LinkTarget) -> String {
    match target {
        LinkTarget::Page(n, _) => crate::i18n::fmt(tl!("Go to page {p}"), &[("p", info.pages.get(*n).map(|p| p.label.as_str()).unwrap_or("?"))]),
        LinkTarget::Uri(u) => u.clone(),
        LinkTarget::SetLayers { .. } => tl!("Set layer visibility").to_string(),
        LinkTarget::RemoteGoTo { file, .. } => file.clone(),
        LinkTarget::JavaScript(_) => crate::i18n::fmt(tl!("{s} action"), &[("s", "JavaScript")]),
        LinkTarget::Other(s) => crate::i18n::fmt(tl!("{s} action"), &[("s", s)]),
    }
}

/// The Link tool: a drag from free page space draws the rectangle of a new link, Delete removes the
/// selected link and Escape unselects it. A press on a link is the page overlay's (`PageLinks` in
/// edit mode, read by [`link_edit_press`]); a drag that starts on one passes to this tool.
pub(crate) struct LinkTool<'a> {
    session: &'a mut PageSession,
    pages: &'a [PlacedPage],
    info: &'a DocInfo,
    /// The canvas background's press of this pass: where a drawn rectangle began.
    press: PressState,
}

impl<'a> LinkTool<'a> {
    /// The tool over the pages on screen this pass.
    pub(crate) fn new(session: &'a mut PageSession, pages: &'a [PlacedPage], info: &'a DocInfo, press: PressState) -> Self {
        LinkTool { session, pages, info, press }
    }

    fn placed(&self, page: usize) -> Option<&PlacedPage> {
        self.pages.iter().find(|p| p.page == page)
    }

    /// The rectangle from the press to `pointer`, held to the page it began on.
    fn drawn(&self, pointer: Pos2) -> Option<(usize, Rect)> {
        let start = self.press.phase.start()?;
        if start.button != PointerButton::Primary {
            return None;
        }
        // A press off every page is no page's.
        let placed = self.pages.iter().find(|p| p.rect.contains(start.origin))?;
        Some((placed.page, Rect::from_two_pos(start.origin, pointer).intersect(placed.rect)))
    }
}

impl CanvasTool for LinkTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        if self.pages.iter().any(|p| p.rect.contains(pointer.pos)) { ToolAction::Claimed } else { ToolAction::Ignored }
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        let Some((page, r)) = self.drawn(pointer.pos) else { return };
        // A click that moved nothing draws nothing and leaves the page's selection empty.
        if r.width() < MIN_LINK_SIDE || r.height() < MIN_LINK_SIDE {
            self.session.links.selected = None;
            return;
        }
        let Some(placed) = self.placed(page) else { return };
        let (Some(a), Some(b)) = (to_user(placed, self.info, r.min), to_user(placed, self.info, r.max)) else { return };
        self.session.links.open_new = Some((page, [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]));
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {}

    fn claims(&self) -> ToolClaims {
        if self.session.links.selected.is_some() { ToolClaims::NONE.deletes() } else { ToolClaims::NONE }
    }

    fn key(&mut self, key: &ToolKey, _cx: &mut ToolContext<'_>) -> bool {
        match key {
            ToolKey::Delete { .. } => {
                let Some((page, index)) = self.session.links.selected.take() else { return false };
                self.session.pending_edit = Some(Edit::DeleteLink { page, index });
                true
            }
            ToolKey::Cancel => self.session.links.selected.take().is_some(),
            _ => false,
        }
    }

    fn paint(&self, _draw: &Draw<'_>, cx: &ToolContext<'_>) {
        // Only a press that is down shows the rectangle; the release pass has committed it.
        if !matches!(self.press.phase, PressPhase::Held { .. } | PressPhase::DragStart { .. } | PressPhase::Drag { .. }) {
            return;
        }
        let Some(pointer) = cx.pointer else { return };
        let Some((_, r)) = self.drawn(pointer.pos) else { return };
        cx.marching_ants(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], true);
    }

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        Cursor::System(CursorIcon::Crosshair)
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Link command")
    }
}

/// Applies the press the page's link parts reported with the Link tool: a click selects the link, a
/// second click opens its properties, a secondary press selects it for its menu.
pub(crate) fn link_edit_press(view: &mut LinkView, event: &LinkEvent) {
    match *event {
        LinkEvent::Select { page, index, clicks } => {
            view.selected = Some((page, index));
            if clicks >= 2 {
                view.open_existing = Some((page, index));
            }
        }
        LinkEvent::Context { page, index } => {
            view.selected = Some((page, index));
            view.context = Some((page, index));
        }
        // Following a link is the page's (`canvas` reads it into the output); it selects nothing.
        LinkEvent::Follow { .. } => {}
    }
}

/// Outlines of the document's links over the pages: the selected one solid, the others faint. Shown
/// under the Link tool only.
pub(crate) fn link_outlines(ui: &mut egui::Ui, body: &PageBody<'_>, view: &LinkView, links: &[LinkItem], info: &DocInfo) {
    // A pass with no tool run measures only; the outlines need the pages placed.
    let (mut selected, mut others) = (Vec::new(), Vec::new());
    for l in links {
        let Some(placed) = body.pages.iter().find(|p| p.page == l.page) else { continue };
        let Some(r) = screen_rect(placed, info, l.rect) else { continue };
        let mut path = BezPath::new();
        let [a, b, c, d] = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|p| body.view.to_doc(p));
        path.move_to(a);
        path.line_to(b);
        path.line_to(c);
        path.line_to(d);
        path.close_path();
        if view.selected == Some((l.page, l.index)) { selected.push(path) } else { others.push(path) }
    }
    SelectionOutline::faint(tl!("Links").to_string(), &others).show_at(ui, body.rect);
    SelectionOutline::solid(tl!("Selected link").to_string(), &selected).show_at(ui, body.rect);
}

/// Link Properties: the draft of a new or existing link over a form dialog; OK answers the edit it makes.
pub(crate) struct LinkPropsDialog {
    id: DialogId,
    draft: LinkDraft,
    /// The document's page count, which bounds the Go to a page view target.
    pages: usize,
    /// Whether the dialog is open: bound to the form's open state, so the host's dialog closes with it.
    open: bool,
    /// The custom-colour request the Color row raised, for the app to open the colour picker.
    colour_request: Option<ColorRequest>,
    /// The session's recent custom colours, which the Color row offers.
    recent: RecentColours,
}

impl LinkPropsDialog {
    /// The dialog editing `draft` in a document of `pages` pages.
    pub(crate) fn new(draft: LinkDraft, pages: usize, recent: RecentColours) -> Self {
        LinkPropsDialog { id: dialog_id("linkProps"), draft, pages, open: true, colour_request: None, recent }
    }

    /// Sets the link's colour: the colour picker's answer.
    pub(crate) fn set_colour(&mut self, colour: [f64; 3]) {
        self.draft.style.color = colour;
    }

    /// The custom-colour request raised since the last call.
    pub(crate) fn take_colour_request(&mut self) -> Option<ColorRequest> {
        self.colour_request.take()
    }
}

/// Whether a draft can be accepted: a web link needs an address.
fn draft_ok(d: &LinkDraft) -> bool {
    d.preserve_action || !d.web || (d.url.trim().len() > 3 && d.url.trim() != "https://")
}

fn entries<T: Clone + PartialEq + std::fmt::Debug + Send + Sync + 'static>(rows: impl IntoIterator<Item = (T, String)>) -> Vec<Entry<T>> {
    rows.into_iter().map(|(value, label)| Entry::new(value, label)).collect()
}

/// A row of a labelled select over `rows`: the value picked this pass, if it is not `current`.
fn pick<T: Clone + PartialEq + std::fmt::Debug + Send + Sync + 'static>(
    ui: &mut egui::Ui,
    label: String,
    current: T,
    rows: Vec<(T, String)>,
) -> Option<T> {
    let list = entries(rows);
    let mut picked = None;
    Field::new(label.clone()).show(ui, |ui, _| {
        picked = Select::new(label.clone(), Bind::given(current.clone()), &list).show(ui).value().cloned();
    });
    picked.filter(|p| *p != current)
}

/// The Link Appearance group.
fn appearance(ui: &mut egui::Ui, d: &mut LinkDraft, request: &mut Option<ColorRequest>, recent: &RecentColours) {
    FieldGroup::new(tl!("Link Appearance").to_string()).show(ui, |ui, _| {
        let visible = [(true, tl!("Visible Rectangle").to_string()), (false, tl!("Invisible Rectangle").to_string())];
        if let Some(v) = pick(ui, tl!("Link Type").to_string(), d.style.visible, visible.to_vec()) {
            d.style.visible = v;
        }
        let highlights = LinkHighlight::ALL.into_iter().map(|h| (h, tl!(h.label()).to_string())).collect();
        if let Some(h) = pick(ui, tl!("Highlight Style").to_string(), d.style.highlight, highlights) {
            d.style.highlight = h;
        }
        if !d.style.visible {
            return;
        }
        let thickness = if d.style.width <= 1.0 {
            0
        } else if d.style.width <= 2.0 {
            1
        } else {
            2
        };
        let widths = vec![(0usize, tl!("Thin").to_string()), (1, tl!("Medium").to_string()), (2, tl!("Thick").to_string())];
        if let Some(w) = pick(ui, tl!("Line Thickness").to_string(), thickness, widths) {
            d.style.width = (w + 1) as f64;
        }
        let line = if d.style.dashed {
            1
        } else if d.style.underline {
            2
        } else {
            0
        };
        let lines = vec![(0usize, tl!("Solid").to_string()), (1, tl!("Dashed").to_string()), (2, tl!("Underline").to_string())];
        if let Some(l) = pick(ui, tl!("Line Style").to_string(), line, lines) {
            d.style.dashed = l == 1;
            d.style.underline = l == 2;
        }
        let out = palette(ui, tl!("Color"), Some(d.style.color), "link.colour", recent);
        // A repeat click on the bound colour changes nothing.
        if let Some(picked) = out.picked.filter(|c| *c != d.style.color) {
            d.style.color = picked;
        }
        if out.request.is_some() {
            *request = out.request;
        }
    });
}

/// The Link Action group.
fn action(ui: &mut egui::Ui, d: &mut LinkDraft, pages: usize) {
    FieldGroup::new(tl!("Link Action").to_string()).show(ui, |ui, _| {
        if d.preserve_action {
            Field::new(tl!("Action").to_string())
                .help(Help::Text(tl!("The existing action will be preserved.").to_string().into()))
                .show(ui, |_, _| {});
            return;
        }
        let web = d.web;
        RadioGroup::new(tl!("Link Action").to_string(), Bind::bound(&mut d.web)).show(ui, |ui, _| {
            Radio::new(true).label(tl!("Open a web page").to_string()).show(ui);
            let mut field = Field::new(tl!("Link URL").to_string());
            if web && !draft_ok_url(&d.url) {
                field = field.help(Help::Text(tl!("Type the web address.").to_string().into()));
            }
            field.show(ui, |ui, _| {
                TextField::new(tl!("Link URL").to_string(), Bind::bound(&mut d.url)).placeholder("https://example.org").enabled(web).show(ui);
            });
            Radio::new(false).label(tl!("Go to a page view").to_string()).show(ui);
            Field::new(tl!("Page").to_string()).show(ui, |ui, _| {
                let page = NumberField::new(tl!("Page").to_string(), Bind::given(Some(d.target as f64)))
                    .range(1.0..=pages.max(1) as f64)
                    .step(1.0)
                    .enabled(!web)
                    .show(ui);
                if let Some(n) = page.edit.events.landed().cloned().flatten() {
                    d.target = (n.round().max(1.0) as usize).min(pages.max(1));
                }
            });
            // The destination's own zoom, past what the dialog offers, stays a choice.
            let mut zooms = ZOOMS.to_vec();
            if !zooms.contains(&d.zoom) {
                zooms.push(d.zoom);
            }
            let current = zooms.iter().position(|z| *z == d.zoom).unwrap_or(0);
            let rows: Vec<(usize, String)> = zooms.iter().enumerate().map(|(i, z)| (i, zoom_label(*z))).collect();
            let list = entries::<usize>(rows);
            Field::new(tl!("Zoom").to_string()).show(ui, |ui, _| {
                let picked = Select::new(tl!("Zoom").to_string(), Bind::given(current), &list).enabled(!web).show(ui).value().copied();
                if let Some(z) = picked.and_then(|i| zooms.get(i)) {
                    d.zoom = *z;
                }
            });
        });
    });
}

/// Whether a typed web address is one a link can take.
fn draft_ok_url(url: &str) -> bool {
    url.trim().len() > 3 && url.trim() != "https://"
}

impl StackDialog for LinkPropsDialog {
    type Result = Edit;

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut egui::Ui, _: &DialogStack) -> Option<DialogResult<Edit>> {
        let title = if self.draft.index.is_some() { tl!("Link Properties") } else { tl!("Create Link") };
        let LinkPropsDialog { draft, pages, open, colour_request, recent, .. } = self;
        let form = FormDialog::new(title.to_string(), Bind::bound(&mut *draft))
            .opened(Bind::bound(open))
            .ok_reason(tl!("Type the web address.").to_string());
        let shown = form.show(ui, |ui, _, d| {
            appearance(ui, d, colour_request, recent);
            action(ui, d, *pages);
            draft_ok(d)
        });
        match (shown.accepted, shown.closed) {
            (Some(d), _) => Some(DialogResult::Ok(edit_for(&d))),
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }

    fn key(&self) -> Option<&str> {
        Some("linkProps")
    }
}

impl PdfCraftApp {
    /// Open Link Properties for a new link area or an existing link.
    pub(crate) fn open_link_props(&mut self, page: usize, rect: Option<[f64; 4]>, index: Option<usize>) {
        let Some((_, id)) = self.active_ids() else { return };
        let existing = index.and_then(|i| self.session.get(id).and_then(|d| d.links().iter().find(|l| l.page == page && l.index == i).cloned()));
        let draft = match existing {
            Some(l) => LinkDraft {
                page,
                rect: l.rect,
                index,
                preserve_action: matches!(l.action, LinkAction::Other(_)),
                web: matches!(l.action, LinkAction::Uri(_)),
                url: match &l.action {
                    LinkAction::Uri(u) => u.clone(),
                    _ => "https://".into(),
                },
                target: match l.action {
                    LinkAction::Page { page, .. } => page + 1,
                    _ => 1,
                },
                zoom: match l.action {
                    LinkAction::Page { zoom, .. } => zoom,
                    _ => LinkZoom::FitPage,
                },
                current: Some(l.action.clone()),
                style: l.style,
            },
            None => LinkDraft { page, rect: rect.unwrap_or_default(), ..LinkDraft::default() },
        };
        let pages = self.session.get(id).map_or(1, |d| d.info.pages.len());
        crate::dialogs::host::open(self, LinkPropsDialog::new(draft, pages, self.recent_colours.clone()));
    }

    /// One pass of Link Properties for the app: opens the colour picker the Color row asked for, and
    /// applies the edit OK answered.
    // PLAN: pdf-hub-w2's `dialogs/host.rs::results` calls `app.link_props_pass(ctx)`; the pass needs no context, so it calls it without.
    pub(crate) fn link_props_pass(&mut self, ctx: &egui::Context) {
        if let Some(request) = self.dialogs.get_mut::<LinkPropsDialog>().and_then(|d| d.take_colour_request()) {
            self.open_colour_picker(PickerHost::App, request, |app: &mut PdfCraftApp, _: &egui::Context, picked: Srgba| {
                if let Some(d) = app.dialogs.get_mut::<LinkPropsDialog>() {
                    d.set_colour(to_rgb(picked));
                }
            });
        }
        if let Some(DialogResult::Ok(edit)) = self.dialogs.take::<LinkPropsDialog>() {
            self.apply_edit(ctx, edit);
        }
    }

    /// Create links from URLs in the text of every page.
    pub fn links_from_urls(&mut self, ctx: &egui::Context) {
        let Some((_, id)) = self.active_ids() else { return };
        let found = self.session.find_urls(id);
        if found.is_empty() {
            self.notify_tr("No unlinked web addresses were found in the text");
            return;
        }
        let n = found.len();
        if self.apply_edit(ctx, Edit::AddLinks { links: found, style: LinkStyle::default() }) {
            if n == 1 {
                self.notify_tr("Created 1 link");
            } else {
                self.notify_fmt("Created {n} links", &[("n", &n.to_string())]);
            }
        }
    }

    /// Remove every link in the active document.
    pub fn remove_all_links(&mut self, ctx: &egui::Context) {
        let Some((_, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.links().len());
        if count == 0 {
            self.notify_tr("No links to remove");
            return;
        }
        if self.apply_edit(ctx, Edit::RemoveLinks { pages: None }) {
            if count == 1 {
                self.notify_tr("Removed 1 link");
            } else {
                self.notify_fmt("Removed {n} links", &[("n", &count.to_string())]);
            }
        }
    }
}

/// The action a confirmed draft sets.
fn action_for(d: &LinkDraft) -> LinkAction {
    if d.web { LinkAction::Uri(d.url.trim().to_string()) } else { LinkAction::Page { page: d.target.saturating_sub(1), zoom: d.zoom } }
}

/// The edit for a confirmed draft.
pub(crate) fn edit_for(d: &LinkDraft) -> Edit {
    match d.index {
        None => Edit::AddLink { page: d.page, rect: d.rect, action: action_for(d), style: d.style },
        Some(index) => Edit::SetLink {
            page: d.page,
            index,
            rect: None,
            action: (!d.preserve_action).then(|| action_for(d)).filter(|a| d.current.as_ref() != Some(a)),
            style: Some(d.style),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkDraft, edit_for, zoom_label};
    use pdfcraft_engine::{Edit, LinkAction, LinkStyle, LinkZoom};

    fn draft(current: LinkAction) -> LinkDraft {
        LinkDraft {
            page: 0,
            rect: [0.0; 4],
            index: Some(3),
            preserve_action: matches!(current, LinkAction::Other(_)),
            web: false,
            url: String::new(),
            target: 1,
            zoom: LinkZoom::FitPage,
            current: Some(current),
            style: LinkStyle::default(),
        }
    }

    #[test]
    fn appearance_edit_preserves_an_existing_unsupported_action() {
        let style = LinkStyle::default();
        let draft = draft(LinkAction::Other("named destination".into()));
        assert_eq!(edit_for(&draft), Edit::SetLink { page: 0, index: 3, rect: None, action: None, style: Some(style) });
    }

    #[test]
    fn only_a_changed_destination_is_rewritten() {
        // A page view the dialog shows as it is: OK leaves the destination (and its position) alone.
        let mut d = draft(LinkAction::Page { page: 0, zoom: LinkZoom::Percent(200.0) });
        d.zoom = LinkZoom::Percent(200.0);
        assert!(matches!(edit_for(&d), Edit::SetLink { action: None, .. }));
        d.zoom = LinkZoom::FitWidth;
        assert!(matches!(edit_for(&d), Edit::SetLink { action: Some(LinkAction::Page { page: 0, zoom: LinkZoom::FitWidth }), .. }));
        d.zoom = LinkZoom::Percent(200.0);
        d.target = 2;
        assert!(matches!(edit_for(&d), Edit::SetLink { action: Some(LinkAction::Page { page: 1, .. }), .. }));
    }

    #[test]
    fn a_zoom_label_stays_short_for_any_factor() {
        assert_eq!(zoom_label(LinkZoom::Percent(200.0)), "200%");
        // The destination's own zoom, past what PdfCraft writes, is shown as it is.
        assert_eq!(zoom_label(LinkZoom::Percent(10000.0)), "10000%");
        assert_eq!(zoom_label(LinkZoom::Percent(f64::MAX)), "1.8e308%");
        // A fractional zoom keeps its places instead of rounding to 0%.
        assert_eq!(zoom_label(LinkZoom::Percent(0.5)), "0.5%");
        assert_eq!(zoom_label(LinkZoom::Percent(62.5)), "62.5%");
        assert_eq!(zoom_label(LinkZoom::Percent(33.333)), "33.33%");
        assert_eq!(zoom_label(LinkZoom::Percent(0.001)), "1.0e-3%");
    }
}
