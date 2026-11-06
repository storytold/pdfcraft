//! Edit a PDF ▸ Link: drag a rectangle to create a link, then choose its appearance and action
//! (go to a page, open a web page) in Link Properties; with the Link tool, existing links show
//! their outlines, a click selects one, a double-click edits it and Delete removes it.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Edit, LinkAction, LinkHighlight, LinkItem, LinkStyle};
use pdfcraft_render::{DocInfo, LinkTarget};

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, Owner};
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, QuickTool, widgets};

const LINK_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// How far around a link (screen points) the Link tool finds it, so a thin one can be picked.
const LINK_TOOL_MARGIN: f32 = 2.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkView {
    /// A link rectangle being drawn: (page, start).
    pub drag: Option<(usize, Pos2)>,
    pub selected: Option<(usize, usize)>,
    /// The link the last right-click on a page hit (page, index), if any: the canvas
    /// context menu shows the link items only for it, so a right-click elsewhere (on a
    /// comment, say) keeps its own menu even while a link stays selected.
    pub context: Option<(usize, usize)>,
    /// Ask the app to open Link Properties for a new rect (page, user rect) or an existing
    /// link (page, index).
    pub open_new: Option<(usize, [f64; 4])>,
    pub open_existing: Option<(usize, usize)>,
    /// The last click on a link: (time, page, index), to see a second click as a double-click.
    last_click: Option<(f64, usize, usize)>,
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
    pub style: LinkStyle,
}

fn screen_rect(xf: &PageXform, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
    xf.user_rect(info, page, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])
}

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// The link under `p` on `page`: the one hit test of links, for the hand, the tip and the press.
/// Of stacked links, the topmost (the last in `/Annots`, drawn on top). Only the Select tool
/// follows links and the Link tool edits them (`Layer::reached_by`). The Link tool finds every link
/// the document has (`links`, the engine's), with a margin around each so a thin one can be
/// picked, and a drag from one draws a new link. The other tools find the links the renderer can
/// follow, which have a target (`info.links`). The tip is the link's target.
pub(crate) fn hit(xf: &PageXform, info: &DocInfo, page: usize, links: &[LinkItem], tool: QuickTool, p: Pos2) -> Option<Hit> {
    // A press off the page is no page's, so nothing there is a link's, the margin included.
    if !xf.rect.contains(p) {
        return None;
    }
    let (index, link) = if tool == QuickTool::Link {
        let index = links.iter().rfind(|l| l.page == page && screen_rect(xf, info, page, l.rect).expand(LINK_TOOL_MARGIN).contains(p))?.index;
        (index, info.links.iter().find(|l| l.page == page && l.index == index))
    } else {
        let link = info.links.iter().rfind(|l| l.page == page && xf.user_rect(info, page, l.rect).contains(p))?;
        (link.index, Some(link))
    };
    let tip = link.map(|l| target_label(info, &l.target));
    let declines_drag = tool == QuickTool::Link;
    Some(Hit { tip, declines_drag, ..Hit::new(Owner::Link { index }, Layer::Link, egui::CursorIcon::PointingHand) })
}

/// What a link's tip says it goes to.
fn target_label(info: &DocInfo, target: &LinkTarget) -> String {
    match target {
        LinkTarget::Page(n, _) => crate::i18n::fmt(tl!("Go to page {p}"), &[("p", info.pages.get(*n).map(|p| p.label.as_str()).unwrap_or("?"))]),
        LinkTarget::Uri(u) => u.clone(),
        LinkTarget::SetLayers { .. } => tl!("Set layer visibility").to_string(),
        LinkTarget::RemoteGoTo { file, .. } => file.clone(),
        LinkTarget::Named(n) => match n.as_str() {
            "NextPage" => tl!("Next page").to_string(),
            "PrevPage" => tl!("Previous page").to_string(),
            "FirstPage" => tl!("First page").to_string(),
            "LastPage" => tl!("Last page").to_string(),
            other => crate::i18n::fmt(tl!("{s} action"), &[("s", other)]),
        },
        LinkTarget::Hide { hide: true, .. } => tl!("Hide a field").to_string(),
        LinkTarget::Hide { hide: false, .. } => tl!("Show a field").to_string(),
        LinkTarget::ResetForm { .. } => tl!("Reset a form").to_string(),
        LinkTarget::Sequence(seq) => seq.first().map(|a| target_label(info, a)).unwrap_or_default(),
        LinkTarget::Other(s) => crate::i18n::fmt(tl!("{s} action"), &[("s", s)]),
    }
}

/// Link tool input on one page: `free` when nobody holds the press and it began on this page,
/// `owner` the link that holds it (by its `/Annots` index), and `context` the link a right-click is
/// on. A drag from either draws a new link. Returns `true` when the gesture belongs to the tool.
pub(crate) fn page_input(cx: PageToolContext<'_>, free: bool, owner: Option<usize>, context: Option<usize>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let pointer = ptr.pos;
    let lv = &mut view.links;
    if let Some((dp, start)) = lv.drag
        && dp == page
    {
        if resp.drag_stopped() || !ptr.primary_down {
            lv.drag = None;
            let end = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, end).intersect(xf.rect);
            if r.width() >= 4.0 && r.height() >= 4.0 {
                let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
                lv.open_new = Some((page, [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]));
            }
        }
        return true;
    }
    // A drag that nobody holds, or from a link (whose hit lets the drag go), draws a new link from
    // where its press began on this page. egui may see the drag only once the pointer is past the
    // page's edge.
    if resp.drag_started()
        && (free || owner.is_some())
        && let Some(origin) = ptr.origin.filter(|o| xf.rect.contains(*o))
    {
        lv.drag = Some((page, origin));
        return true;
    }
    if !pointer.is_some_and(|p| xf.rect.contains(p)) {
        return false;
    }
    // Over a link, its hit claims the hand.
    view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    if let Some(index) = context {
        lv.selected = Some((page, index));
        lv.context = Some((page, index));
        return true;
    }
    // The tool's own click: on a link, or one nobody holds.
    if (resp.clicked() || resp.double_clicked()) && (free || owner.is_some()) {
        let now = ui.input(|i| i.time);
        // A second click on the same link soon after the first is a double-click.
        let double = resp.double_clicked() || owner.is_some_and(|k| lv.last_click.is_some_and(|(t, p, l)| p == page && l == k && now - t < 0.5));
        lv.last_click = owner.map(|k| (now, page, k));
        lv.selected = owner.map(|k| (page, k));
        if double && let Some(k) = owner {
            lv.open_existing = Some((page, k));
            lv.last_click = None;
        }
        return true;
    }
    resp.is_pointer_button_down_on()
}

/// Outlines of the page's links (selected one bold) and the rectangle being drawn.
pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, links: &[LinkItem], view: &DocView, pointer: Option<Pos2>) {
    for l in links.iter().filter(|l| l.page == page) {
        let r = screen_rect(xf, info, page, l.rect);
        let selected = view.links.selected == Some((page, l.index));
        painter.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(if selected { 2.0 } else { 1.0 }, LINK_BLUE.gamma_multiply(if selected { 1.0 } else { 0.6 })),
            egui::StrokeKind::Outside,
        );
    }
    if let (Some((dp, start)), Some(p)) = (view.links.drag, pointer)
        && dp == page
    {
        let r = Rect::from_two_pos(start, p);
        painter.rect_filled(r, CornerRadius::ZERO, LINK_BLUE.gamma_multiply(0.08));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, LINK_BLUE), egui::StrokeKind::Inside);
    }
}

/// Delete removes the selected link; Escape clears the selection.
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView) {
    if view.links.selected.is_none() || ctx.egui_wants_keyboard_input() {
        return;
    }
    let (del, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
    if del && let Some((page, index)) = view.links.selected.take() {
        view.pending_edit = Some(Edit::DeleteLink { page, index });
    } else if esc {
        view.links.selected = None;
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
                    LinkAction::Page(p) => p + 1,
                    _ => 1,
                },
                style: l.style,
            },
            None => LinkDraft {
                page,
                rect: rect.unwrap_or_default(),
                index: None,
                preserve_action: false,
                web: true,
                url: "https://".into(),
                target: 1,
                style: LinkStyle::default(),
            },
        };
        self.link_draft = Some(draft);
        self.dialog = Some(crate::Dialog::LinkProps);
    }

    /// Create links from URLs in the text of every page.
    pub fn links_from_urls(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let found = self.session.find_urls(id);
        if found.is_empty() {
            self.notify_tr("No unlinked web addresses were found in the text");
            return;
        }
        let n = found.len();
        if self.apply_edit(Edit::AddLinks { links: found, style: LinkStyle::default() }) {
            if n == 1 {
                self.notify_tr("Created 1 link");
            } else {
                self.notify_fmt("Created {n} links", &[("n", &n.to_string())]);
            }
        }
    }

    /// Remove every link in the active document.
    pub fn remove_all_links(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.links().len());
        if count == 0 {
            self.notify_tr("No links to remove");
            return;
        }
        if self.apply_edit(Edit::RemoveLinks { pages: None }) {
            if count == 1 {
                self.notify_tr("Removed 1 link");
            } else {
                self.notify_fmt("Removed {n} links", &[("n", &count.to_string())]);
            }
        }
    }
}

/// Link Properties. Returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, d: &mut LinkDraft, pages: usize, t: &Tokens) -> (bool, bool) {
    ui.set_width(460.0);
    ui.label(egui::RichText::new(if d.index.is_some() { tl!("Link Properties") } else { tl!("Create Link") }).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::section_title(ui, tl!("Link Appearance"));
    egui::Grid::new("link-appearance").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Link Type:"));
        craft_ui::ComboBox::from_id_salt("link-type")
            .selected_text(if d.style.visible { tl!("Visible Rectangle") } else { tl!("Invisible Rectangle") })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.style.visible, true, tl!("Visible Rectangle"));
                ui.selectable_value(&mut d.style.visible, false, tl!("Invisible Rectangle"));
            });
        ui.end_row();
        ui.label(tl!("Highlight Style:"));
        craft_ui::ComboBox::from_id_salt("link-hl").selected_text(tl!(d.style.highlight.label())).show_ui(ui, |ui| {
            for h in LinkHighlight::ALL {
                ui.selectable_value(&mut d.style.highlight, h, tl!(h.label()));
            }
        });
        ui.end_row();
        if d.style.visible {
            ui.label(tl!("Line Thickness:"));
            craft_ui::ComboBox::from_id_salt("link-w")
                .selected_text(if d.style.width <= 1.0 {
                    tl!("Thin")
                } else if d.style.width <= 2.0 {
                    tl!("Medium")
                } else {
                    tl!("Thick")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.style.width, 1.0, tl!("Thin"));
                    ui.selectable_value(&mut d.style.width, 2.0, tl!("Medium"));
                    ui.selectable_value(&mut d.style.width, 3.0, tl!("Thick"));
                });
            ui.end_row();
            ui.label(tl!("Line Style:"));
            let mut style = if d.style.dashed {
                1
            } else if d.style.underline {
                2
            } else {
                0
            };
            craft_ui::ComboBox::from_id_salt("link-s").selected_text(tl!(["Solid", "Dashed", "Underline"][style])).show_ui(ui, |ui| {
                for (k, l) in ["Solid", "Dashed", "Underline"].iter().enumerate() {
                    ui.selectable_value(&mut style, k, tl!(l));
                }
            });
            d.style.dashed = style == 1;
            d.style.underline = style == 2;
            ui.end_row();
            ui.label(tl!("Color:"));
            if let Some(c) = crate::comments::swatch_grid(ui, Some(d.style.color)) {
                d.style.color = c;
            }
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    widgets::section_title(ui, tl!("Link Action"));
    if d.preserve_action {
        ui.label(tl!("The existing action will be preserved."));
    } else {
        widgets::radio_value(ui, &mut d.web, true, tl!("Open a web page"));
        ui.add_enabled_ui(d.web, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.add(crate::widgets::line(&mut d.url).desired_width(360.0).hint_text("https://example.org")).accessible_name(tl!("Link URL"));
            });
        });
        widgets::radio_value(ui, &mut d.web, false, tl!("Go to a page view"));
        ui.add_enabled_ui(!d.web, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(tl!("Page"));
                ui.add(egui::DragValue::new(&mut d.target).range(1..=pages.max(1)));
                ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &pages.to_string())]));
            });
        });
    }
    let ok = d.preserve_action || !d.web || (d.url.trim().len() > 3 && d.url.trim() != "https://");
    if !ok {
        ui.label(egui::RichText::new(tl!("Type the web address.")).small().color(t.text_faint));
    }
    ui.add_space(12.0);
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// The edit for a confirmed draft.
pub(crate) fn edit_for(d: &LinkDraft) -> Edit {
    match d.index {
        None => Edit::AddLink {
            page: d.page,
            rect: d.rect,
            action: if d.web { LinkAction::Uri(d.url.trim().to_string()) } else { LinkAction::Page(d.target.saturating_sub(1)) },
            style: d.style,
        },
        Some(index) => Edit::SetLink {
            page: d.page,
            index,
            rect: None,
            action: (!d.preserve_action)
                .then(|| if d.web { LinkAction::Uri(d.url.trim().to_string()) } else { LinkAction::Page(d.target.saturating_sub(1)) }),
            style: Some(d.style),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkDraft, edit_for};
    use pdfcraft_engine::{Edit, LinkStyle};

    #[test]
    fn appearance_edit_preserves_an_existing_unsupported_action() {
        let style = LinkStyle::default();
        let draft = LinkDraft { page: 0, rect: [0.0; 4], index: Some(3), preserve_action: true, web: false, url: String::new(), target: 1, style };
        assert_eq!(edit_for(&draft), Edit::SetLink { page: 0, index: 3, rect: None, action: None, style: Some(style) });
    }
}
