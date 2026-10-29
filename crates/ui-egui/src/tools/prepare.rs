//! Prepare a form (Acrobat's Prepare Form, execution plan M6.6): add fields with the field
//! tools (click for the default size, or drag a rectangle), select, move and resize them, see
//! their name tags, delete them, and edit them in Field Properties.
//!
//! While the Prepare a form panel is open the page shows every field with a light-blue fill and
//! its name; clicks select fields instead of filling them in, as in Acrobat.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, vec2};
use pdfcraft_engine::form_scripts::{CalcOp, Calculate, DATE_PRESETS, Format, TIME_PRESETS, Validate, format_value};
use pdfcraft_engine::{BorderStyle, Document, Edit, FieldFont, FieldLook, FieldProps, FormField, FormFieldKind, NewField};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::comments;
use crate::page_cursor::Layer;
use crate::theme;
use crate::widgets;

mod draft;
mod properties;
mod tabs;

pub use draft::{ACTION_KINDS, ActionDraft, FieldDraft, FieldTab};
pub(crate) use draft::{da_font, da_size};
pub(crate) use properties::body;

const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// Acrobat's field fill in Prepare Form (light blue).
pub(crate) const FIELD_FILL: Color32 = Color32::from_rgba_premultiplied(0x1B, 0x34, 0x54, 0x59);

/// The Add form components tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldTool {
    Text,
    /// Rows and columns of text fields in the area drawn, set up in a window first.
    TextTable,
    CheckBox,
    Radio,
    Combo,
    List,
    Button,
    Image,
    Date,
    Signature,
}

pub const FIELD_TOOLS: [FieldTool; 10] = [
    FieldTool::Text,
    FieldTool::TextTable,
    FieldTool::CheckBox,
    FieldTool::Radio,
    FieldTool::Combo,
    FieldTool::List,
    FieldTool::Button,
    FieldTool::Image,
    FieldTool::Date,
    FieldTool::Signature,
];

impl FieldTool {
    pub fn command(self) -> &'static str {
        match self {
            FieldTool::Text => "form.add.text",
            FieldTool::TextTable => "form.add.text_table",
            FieldTool::CheckBox => "form.add.checkbox",
            FieldTool::Radio => "form.add.radio",
            FieldTool::Combo => "form.add.combo",
            FieldTool::List => "form.add.list",
            FieldTool::Button => "form.add.button",
            FieldTool::Image => "form.add.image",
            FieldTool::Date => "form.add.date",
            FieldTool::Signature => "form.add.signature",
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        FIELD_TOOLS.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            FieldTool::Text => "Text field",
            FieldTool::TextTable => "Text field table",
            FieldTool::CheckBox => "Checkbox",
            FieldTool::Radio => "Radio button",
            FieldTool::Combo => "Drop-down list",
            FieldTool::List => "List box",
            FieldTool::Button => "Button",
            FieldTool::Image => "Image field",
            FieldTool::Date => "Date field",
            FieldTool::Signature => "Digital signature",
        }
    }

    /// The field a click or drag creates. Radio buttons dropped next to a selected radio group
    /// join it with the next free choice.
    pub fn new_field(self, join: Option<&FormField>) -> NewField {
        match self {
            // A table's cells are text fields.
            FieldTool::Text | FieldTool::TextTable => NewField::Text { multiline: false },
            FieldTool::CheckBox => NewField::CheckBox,
            FieldTool::Radio => match join.filter(|f| f.kind == FormFieldKind::Radio) {
                Some(g) => {
                    let taken: Vec<&str> = g.widgets.iter().filter_map(|w| w.on_state.as_deref()).collect();
                    let export =
                        (1..=u64::MAX).map(|i| format!("Choice{i}")).find(|c| !taken.contains(&c.as_str())).unwrap_or_else(|| "Choice".into());
                    NewField::Radio { group: Some(g.name.clone()), export }
                }
                None => NewField::Radio { group: None, export: "Choice1".into() },
            },
            FieldTool::Combo => NewField::Combo { options: Vec::new(), editable: false },
            FieldTool::List => NewField::List { options: Vec::new(), multi: false },
            FieldTool::Button => NewField::Button { caption: String::new() },
            FieldTool::Image => NewField::Image,
            FieldTool::Date => NewField::Date,
            FieldTool::Signature => NewField::Signature,
        }
    }

    /// Width and height in points of a field placed with a single click.
    pub fn default_size(self) -> (f64, f64) {
        match self {
            FieldTool::TextTable => (180.0, 110.0),
            FieldTool::CheckBox | FieldTool::Radio => (14.0, 14.0),
            FieldTool::List => (144.0, 54.0),
            FieldTool::Button => (72.0, 22.0),
            FieldTool::Image => (144.0, 108.0),
            FieldTool::Signature => (180.0, 36.0),
            FieldTool::Date => (108.0, 22.0),
            FieldTool::Text | FieldTool::Combo => (144.0, 22.0),
        }
    }
}

/// What the pointer is doing to a field.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// Drawing a new field from this screen point.
    Draw(Pos2),
    /// Selecting fields inside a dragged rectangle.
    Select(Pos2),
    Move,
    /// Resizing by a corner: (moves left edge, moves top edge) in screen terms.
    Corner(bool, bool),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrepareView {
    /// The selected field and which of its widgets (the anchor of Align and Match Size).
    pub selected: Option<(String, usize)>,
    /// More fields selected with Shift- or ⌘-click.
    pub also: Vec<(String, usize)>,
    /// Select the field an Add field edit creates (the form's length before it).
    pub select_added: Option<usize>,
    grab: Option<(usize, Grab, Pos2)>,
    marquee_before: Option<Vec<(String, usize)>>,
    /// Escape ends egui's drag too; ignore its release until the mouse is up.
    ignore_release: bool,
}

impl PrepareView {
    /// Cancel a preparation marquee without injecting the terminal pointer release that Escape does.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        cancel_marquee(self);
        // Unlike Escape, control cancellation never injects a terminal pointer release.
        self.ignore_release = false;
    }

    /// Unique field names: selecting several widgets of one radio group still edits one field.
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for (name, _) in self.selected.iter().chain(self.also.iter()) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    pub(crate) fn select(&mut self, key: (String, usize), extend: bool) {
        if !extend || self.selected.is_none() {
            self.selected = Some(key);
            self.also.clear();
        } else if self.selected.as_ref() == Some(&key) {
            self.selected = if self.also.is_empty() { None } else { Some(self.also.remove(0)) };
        } else if let Some(index) = self.also.iter().position(|k| *k == key) {
            self.also.remove(index);
        } else {
            self.also.push(key);
        }
    }
}

fn screen_rect(xf: &PageXform, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
    xf.user_rect(info, page, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])
}

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Where a table goes, in view space, for a screen rect drawn with the table tool: a click (a rect
/// under 4 pixels either way) puts the default size's top-left corner at the pointer, moved in from
/// the page's edges as far as it takes to stay on the page.
fn table_rect(xf: &PageXform, r: Rect) -> [f64; 4] {
    let (x0, y0) = xf.screen_to_view(r.min);
    if r.width() < 4.0 || r.height() < 4.0 {
        let (pw, ph) = (f64::from(xf.pw), f64::from(xf.ph));
        let (dw, dh) = FieldTool::TextTable.default_size();
        // Smaller than the default only on a page smaller than it.
        let (w, h) = (dw.min(pw), dh.min(ph));
        let (x, y) = (f64::from(x0).min(pw - w).max(0.0), f64::from(y0).min(ph - h).max(0.0));
        return [x, y, x + w, y + h];
    }
    let (x1, y1) = xf.screen_to_view(r.max);
    [x0, y0, x1, y1].map(f64::from)
}

/// A screen rect → a normalised user-space rect.
fn user_rect(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

fn handles(r: Rect) -> [(Pos2, bool, bool); 4] {
    [(r.left_top(), true, true), (r.right_top(), false, true), (r.left_bottom(), true, false), (r.right_bottom(), false, false)]
}

/// The rect while a move or resize is in progress. With `shift` held a corner keeps the field's
/// proportions, as it does for comments and stamps.
fn dragged(r: Rect, grab: Grab, delta: egui::Vec2, shift: bool) -> Rect {
    match grab {
        Grab::Move => r.translate(delta),
        Grab::Corner(left, top) => {
            let handle = (if left { -1 } else { 1 }, if top { -1 } else { 1 });
            comments::resized(r, handle, delta, comments::corner_ratio(r, None, shift))
        }
        Grab::Draw(_) | Grab::Select(_) => r,
    }
}

pub(crate) struct Outcome {
    /// The click or drag belonged to Prepare a form.
    pub consumed: bool,
    /// Open Field Properties for the selected field.
    pub properties: bool,
    /// A field was placed: go back to the Select tool.
    pub placed: bool,
    /// A table of text fields was drawn: open its window for this page and area (view space).
    pub table: Option<(usize, [f64; 4])>,
}

/// Pointer input on one page while preparing a form.
pub(crate) fn page_input(cx: PageToolContext<'_>, form: &[FormField], tool: Option<FieldTool>, allowed: bool, view: &mut DocView) -> Outcome {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let mut out = Outcome { consumed: false, properties: false, placed: false, table: None };
    let pointer = ptr.pos;
    let prep = &mut view.prepare;

    if prep.marquee_before.is_some() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cancel_marquee(prep);
        out.consumed = true;
        return out;
    }
    if prep.ignore_release {
        if !ptr.primary_down {
            prep.ignore_release = false;
        }
        out.consumed = true;
        return out;
    }

    // A grab in progress on this page.
    if let Some((gp, grab, start)) = prep.grab
        && gp == page
    {
        out.consumed = true;
        if resp.drag_stopped() || !ptr.primary_down {
            prep.grab = None;
            let end = pointer.unwrap_or(start);
            match grab {
                Grab::Select(from) => {
                    prep.marquee_before = None;
                    let rect = Rect::from_two_pos(from, end).intersect(xf.rect);
                    for f in form {
                        for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page)) {
                            if screen_rect(xf, info, page, w.rect).intersects(rect) {
                                let key = (f.name.clone(), wi);
                                if prep.selected.is_none() {
                                    prep.selected = Some(key);
                                } else if prep.selected.as_ref() != Some(&key) && !prep.also.contains(&key) {
                                    prep.also.push(key);
                                }
                            }
                        }
                    }
                }
                Grab::Draw(from) => {
                    let Some(tool) = tool else { return out };
                    if tool == FieldTool::TextTable {
                        out.table = Some((page, table_rect(xf, Rect::from_two_pos(from, end).intersect(xf.rect))));
                        out.placed = true;
                        return out;
                    }
                    let r = Rect::from_two_pos(from, end).intersect(xf.rect);
                    let rect = user_rect(xf, info, page, r);
                    let rect = if r.width() < 4.0 || r.height() < 4.0 {
                        // A click: the default size, top-left at the pointer.
                        let at = to_user(xf, info, page, from);
                        let (w, h) = tool.default_size();
                        [at[0], at[1] - h, at[0] + w, at[1]]
                    } else {
                        rect
                    };
                    let join = prep.selected.as_ref().and_then(|(n, _)| form.iter().find(|f| &f.name == n));
                    prep.select_added = Some(form.len());
                    view.pending_edit = Some(Edit::AddField { page, rect, kind: tool.new_field(join), name: None });
                    out.placed = true;
                }
                Grab::Move | Grab::Corner(..) => {
                    let Some((name, wi)) = prep.selected.clone() else { return out };
                    let Some(w) = form.iter().find(|f| f.name == name).and_then(|f| f.widgets.get(wi)) else { return out };
                    let delta = end - start;
                    if delta.length() >= 1.0 {
                        let shift = ui.input(|i| i.modifiers.shift);
                        let r = dragged(screen_rect(xf, info, page, w.rect), grab, delta, shift);
                        let rect = user_rect(xf, info, page, r);
                        view.pending_edit =
                            Some(Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, rect)), ..Default::default() }) });
                    }
                }
            }
        }
        return out;
    }

    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return out };
    if !allowed {
        return out;
    }
    if let Some(tool) = tool {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        if resp.drag_started() || resp.clicked() {
            let origin = ptr.origin.filter(|o| xf.rect.contains(*o)).unwrap_or(p);
            prep.grab = Some((page, Grab::Draw(origin), origin));
            out.consumed = true;
            if resp.clicked() {
                // A click without a drag: finish at once.
                prep.grab = None;
                if tool == FieldTool::TextTable {
                    out.table = Some((page, table_rect(xf, Rect::from_min_size(p, egui::Vec2::ZERO))));
                    out.placed = true;
                    return out;
                }
                let at = to_user(xf, info, page, p);
                let (w, h) = tool.default_size();
                let join = prep.selected.as_ref().and_then(|(n, _)| form.iter().find(|f| &f.name == n));
                prep.select_added = Some(form.len());
                view.pending_edit = Some(Edit::AddField { page, rect: [at[0], at[1] - h, at[0] + w, at[1]], kind: tool.new_field(join), name: None });
                out.placed = true;
            }
        }
        return out;
    }

    // Right-click a field: select it (the context menu acts on it).
    if resp.secondary_clicked()
        && let Some((f, wi)) = form.iter().find_map(|f| {
            f.widgets.iter().enumerate().find(|(_, w)| w.page == Some(page) && screen_rect(xf, info, page, w.rect).contains(p)).map(|(i, _)| (f, i))
        })
    {
        let key = (f.name.clone(), wi);
        if let Some(i) = prep.also.iter().position(|k| *k == key) {
            // Part of the selection: it becomes the anchor the others line up with.
            prep.also.remove(i);
            if let Some(old) = prep.selected.replace(key) {
                prep.also.push(old);
            }
        } else if prep.selected.as_ref() != Some(&key) {
            prep.selected = Some(key);
            prep.also.clear();
        }
    }
    // The Select tool: corner handles of the selection, then fields.
    let selected_rect = prep
        .selected
        .as_ref()
        .and_then(|(n, wi)| form.iter().find(|f| &f.name == n)?.widgets.get(*wi).filter(|w| w.page == Some(page)))
        .map(|w| screen_rect(xf, info, page, w.rect));
    let corner = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(p) <= 6.0));
    let hit = form.iter().find_map(|f| {
        f.widgets.iter().enumerate().find(|(_, w)| w.page == Some(page) && screen_rect(xf, info, page, w.rect).contains(p)).map(|(i, _)| (f, i))
    });
    if let Some((_, left, top)) = corner {
        view.cursor.claim(Layer::Handle, if left == top { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
    } else if hit.is_some() {
        view.cursor.claim(Layer::Field, egui::CursorIcon::Move);
    }
    if resp.drag_started() {
        let origin = ptr.origin.unwrap_or(p);
        if let Some((_, left, top)) = selected_rect.and_then(|r| handles(r).into_iter().find(|(c, ..)| c.distance(origin) <= 6.0)) {
            prep.grab = Some((page, Grab::Corner(left, top), origin));
            out.consumed = true;
        } else if let Some((f, wi)) = form.iter().find_map(|f| {
            f.widgets
                .iter()
                .enumerate()
                .find(|(_, w)| w.page == Some(page) && screen_rect(xf, info, page, w.rect).contains(origin))
                .map(|(i, _)| (f, i))
        }) {
            prep.selected = Some((f.name.clone(), wi));
            prep.grab = Some((page, Grab::Move, origin));
            out.consumed = true;
        } else {
            prep.marquee_before = Some(prep.selected.iter().chain(prep.also.iter()).cloned().collect());
            if !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                prep.selected = None;
                prep.also.clear();
            }
            prep.grab = Some((page, Grab::Select(origin), origin));
            out.consumed = true;
        }
        return out;
    }
    if resp.double_clicked() && hit.is_some() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
        out.consumed = true;
        out.properties = true;
        return out;
    }
    if resp.clicked() {
        let extend = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        match hit {
            // Shift/⌘-click adds or removes a field from the selection.
            Some((f, wi)) => {
                prep.select((f.name.clone(), wi), extend);
                out.consumed = true;
            }
            None if corner.is_none() => {
                prep.selected = None;
                prep.also.clear();
            }
            None => out.consumed = true,
        }
    }
    out.consumed |= hit.is_some() && resp.is_pointer_button_down_on();
    out
}

/// Align, Center, Distribute and Set Fields to Same Size (Prepare a form, several fields
/// selected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    AlignLeft,
    AlignRight,
    AlignTop,
    AlignBottom,
    /// Centres on one vertical line (the anchor's).
    AlignCenterH,
    /// Centres on one horizontal line.
    AlignCenterV,
    DistributeH,
    DistributeV,
    SameWidth,
    SameHeight,
    SameSize,
}

impl Arrange {
    pub fn label(self) -> &'static str {
        match self {
            Arrange::AlignLeft | Arrange::AlignRight | Arrange::AlignTop | Arrange::AlignBottom | Arrange::AlignCenterH | Arrange::AlignCenterV => {
                "Align fields"
            }
            Arrange::DistributeH | Arrange::DistributeV => "Distribute fields",
            Arrange::SameWidth | Arrange::SameHeight | Arrange::SameSize => "Match field sizes",
        }
    }
}

/// The edits that arrange the selected widgets (user space; the anchor stays put).
pub fn arrange(form: &[FormField], anchor: &(String, usize), others: &[(String, usize)], op: Arrange) -> Option<Edit> {
    let rect_of = |k: &(String, usize)| form.iter().find(|f| f.name == k.0).and_then(|f| f.widgets.get(k.1)).map(|w| w.rect);
    let a = rect_of(anchor)?;
    let mut items: Vec<((String, usize), [f64; 4])> = others.iter().filter_map(|k| rect_of(k).map(|r| (k.clone(), r))).collect();
    if items.is_empty() {
        return None;
    }
    let (aw, ah) = (a[2] - a[0], a[3] - a[1]);
    let mut moved: Vec<((String, usize), [f64; 4])> = Vec::new();
    match op {
        Arrange::DistributeH | Arrange::DistributeV => {
            // Even gaps between all selected fields (the outermost stay).
            items.push((anchor.clone(), a));
            if items.len() < 3 {
                return None;
            }
            let horiz = op == Arrange::DistributeH;
            items.sort_by(|x, y| if horiz { x.1[0].total_cmp(&y.1[0]) } else { y.1[3].total_cmp(&x.1[3]) });
            let size = |r: &[f64; 4]| if horiz { r[2] - r[0] } else { r[3] - r[1] };
            let (first, last) = (items[0].1, items[items.len() - 1].1);
            let span = if horiz { last[2] - first[0] } else { first[3] - last[1] };
            let total: f64 = items.iter().map(|(_, r)| size(r)).sum();
            let gap = (span - total) / (items.len() - 1) as f64;
            let mut at = if horiz { first[0] } else { first[3] };
            for (k, r) in &items {
                let s = size(r);
                let nr = if horiz { [at, r[1], at + s, r[3]] } else { [r[0], at - s, r[2], at] };
                at = if horiz { at + s + gap } else { at - s - gap };
                moved.push((k.clone(), nr));
            }
        }
        _ => {
            for (k, r) in items {
                let (w, h) = (r[2] - r[0], r[3] - r[1]);
                let nr = match op {
                    Arrange::AlignLeft => [a[0], r[1], a[0] + w, r[3]],
                    Arrange::AlignRight => [a[2] - w, r[1], a[2], r[3]],
                    Arrange::AlignTop => [r[0], a[3] - h, r[2], a[3]],
                    Arrange::AlignBottom => [r[0], a[1], r[2], a[1] + h],
                    Arrange::AlignCenterH => {
                        let cx = (a[0] + a[2]) / 2.0;
                        [cx - w / 2.0, r[1], cx + w / 2.0, r[3]]
                    }
                    Arrange::AlignCenterV => {
                        let cy = (a[1] + a[3]) / 2.0;
                        [r[0], cy - h / 2.0, r[2], cy + h / 2.0]
                    }
                    Arrange::SameWidth => [r[0], r[1], r[0] + aw, r[3]],
                    Arrange::SameHeight => [r[0], r[3] - ah, r[2], r[3]],
                    _ => [r[0], r[3] - ah, r[0] + aw, r[3]],
                };
                moved.push((k, nr));
            }
        }
    }
    let edits: Vec<Edit> = moved
        .into_iter()
        .filter(|(k, nr)| rect_of(k).is_some_and(|r| r.iter().zip(nr).any(|(x, y)| (x - y).abs() > 1e-6)))
        .map(|((name, wi), rect)| Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, rect)), ..Default::default() }) })
        .collect();
    (!edits.is_empty()).then(|| Edit::Batch { label: op.label().into(), edits })
}

/// Fields with their light-blue fill and name tags, the selection with its handles, and the
/// rectangle being drawn or dragged to the pages' `pointer`.
pub(crate) fn paint_page(cx: PageToolContext<'_>, form: &[FormField], view: &DocView) {
    let PageToolContext { ui, xf, page, info, ptr, .. } = cx;
    let painter = ui.painter();
    let pointer = ptr.pos;
    let prep = &view.prepare;
    let grab = prep.grab.filter(|(gp, ..)| *gp == page);
    let zoom = xf.zoom().clamp(0.5, 3.0);
    for f in form {
        for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page)) {
            let mut r = screen_rect(xf, info, page, w.rect);
            let selected = prep.selected.as_ref().is_some_and(|(n, i)| n == &f.name && *i == wi);
            let also = prep.also.iter().any(|(n, i)| n == &f.name && *i == wi);
            if selected && let (Some((_, g, start)), Some(p)) = (grab, pointer) {
                r = dragged(r, g, p - start, ui.input(|i| i.modifiers.shift));
            }
            painter.rect_filled(r, CornerRadius::ZERO, FIELD_FILL);
            painter.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(if also { 2.0 } else { 1.0 }, if selected || also { SELECT_BLUE } else { Color32::from_gray(90) }),
                egui::StrokeKind::Inside,
            );
            // The name tag: white text on black, clipped to the field.
            // The full name, as Acrobat shows it: MASTER.first, not first (#2696).
            let label = f.name.as_str();
            let font = theme::regular((9.0 * zoom).clamp(7.0, 13.0));
            let galley = painter.layout_no_wrap(label.to_string(), font, Color32::WHITE);
            let size = galley.size() + vec2(6.0, 2.0);
            let tag = Rect::from_center_size(r.center(), size);
            let clip = painter.clip_rect().intersect(r);
            let p2 = painter.with_clip_rect(clip);
            p2.rect_filled(tag, CornerRadius::same(1), if selected { SELECT_BLUE } else { Color32::BLACK });
            p2.galley(tag.min + vec2(3.0, 1.0), galley, Color32::WHITE);
            if selected {
                painter.rect_stroke(r.expand(1.0), CornerRadius::ZERO, Stroke::new(2.0, SELECT_BLUE), egui::StrokeKind::Outside);
                for (c, ..) in handles(r) {
                    painter.circle(c, 3.5, Color32::WHITE, Stroke::new(1.5, SELECT_BLUE));
                }
            }
        }
    }
    if let (Some((_, Grab::Draw(from) | Grab::Select(from), _)), Some(p)) = (grab, pointer) {
        let r = Rect::from_two_pos(from, p).intersect(xf.rect);
        painter.rect_filled(r, CornerRadius::ZERO, FIELD_FILL);
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Inside);
    }
}

/// Delete the selected field and every field selected with it (#95), as one undoable step, and
/// clear the selection. Each selection is one widget, so selecting one radio button of a group,
/// or one copy of a duplicated field, deletes only that one, as in Acrobat (#4609); a field's
/// last widget takes the field with it.
pub(crate) fn delete_selected(view: &mut DocView) -> Option<Edit> {
    let first = view.prepare.selected.take()?;
    let mut picked = vec![first];
    for w in std::mem::take(&mut view.prepare.also) {
        if !picked.contains(&w) {
            picked.push(w);
        }
    }
    // A field's later widgets first, so the indices of the ones still to go don't shift.
    picked.sort_by_key(|w| std::cmp::Reverse(w.1));
    if picked.len() == 1 {
        return picked.pop().map(|(name, widget)| Edit::DeleteWidget { name, widget });
    }
    let label = format!("Delete {} fields", picked.len());
    Some(Edit::Batch { label, edits: picked.into_iter().map(|(name, widget)| Edit::DeleteWidget { name, widget }).collect() })
}

/// The arrow keys move the selected field a point, Shift and an arrow ten points, as in Acrobat,
/// as the object nudge does (`object_ui::nudge_keys`) and as dragging it does (#2608). The keys
/// are consumed before the view turns the page with them; one press is one undo step.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    // `consume_key` lets an extra Alt through, and Alt+↑/↓ belongs to Calculation order's move.
    if ctx.egui_wants_keyboard_input() || view.prepare.grab.is_some() || ctx.input(|i| i.modifiers.alt) {
        return;
    }
    let Some((name, wi)) = view.prepare.selected.clone() else { return };
    let Some(w) = doc.form.iter().find(|f| f.name == name).and_then(|f| f.widgets.get(wi)) else { return };
    let Some(page) = w.page else { return };
    let Some(xf) = view.page_xform(page) else { return };
    use egui::{Key, Modifiers};
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        for (key, direction) in [
            (Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
            (Key::ArrowRight, egui::vec2(1.0, 0.0)),
            (Key::ArrowUp, egui::vec2(0.0, -1.0)),
            (Key::ArrowDown, egui::vec2(0.0, 1.0)),
        ] {
            // Shift first: a plain key pattern also matches a Shift press.
            if i.consume_key(Modifiers::SHIFT, key) {
                step += direction * 10.0;
            } else if i.consume_key(Modifiers::NONE, key) {
                step += direction;
            }
        }
    });
    if step == egui::Vec2::ZERO {
        return;
    }
    // A point in view space whatever the zoom and whatever way the view is turned; the page's own
    // rotation turns it into the user-space move the widget's rect is stored in.
    let c = xf.rect.center();
    let unit = |d: egui::Vec2| {
        let (a, b) = (xf.screen_to_view(c), xf.screen_to_view(c + d));
        let v = egui::vec2(b.0 - a.0, b.1 - a.1);
        if v.length() > 0.0 { v / v.length() } else { egui::Vec2::ZERO }
    };
    let delta = unit(egui::vec2(1.0, 0.0)) * step.x + unit(egui::vec2(0.0, 1.0)) * step.y;
    let Ok(offset) = doc.object_move_offset(page, [f64::from(delta.x), f64::from(delta.y)]) else { return };
    let r = w.rect;
    let rect = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
    view.pending_edit = Some(Edit::SetFieldProps { name, props: Box::new(FieldProps { rect: Some((wi, rect)), ..Default::default() }) });
}

/// Delete removes the selected fields; Escape clears the selection.
pub(crate) fn select_all(view: &mut DocView, form: &[FormField]) {
    let mut selected =
        form.iter().flat_map(|f| f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(view.current)).map(|(wi, _)| (f.name.clone(), wi)));
    view.prepare.selected = selected.next();
    view.prepare.also = selected.collect();
}

fn cancel_marquee(prep: &mut PrepareView) {
    prep.grab = None;
    prep.ignore_release = true;
    if let Some(before) = prep.marquee_before.take() {
        let mut before = before.into_iter();
        prep.selected = before.next();
        prep.also = before.collect();
    }
}

pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if view.prepare.marquee_before.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cancel_marquee(&mut view.prepare);
        return;
    }
    if view.prepare.selected.is_none() {
        return;
    }
    let (del, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
    if del {
        view.pending_edit = delete_selected(view);
    } else if esc {
        view.prepare.selected = None;
        view.prepare.also.clear();
    }
}

/// After an Add field edit: select the new field (the form's last).
pub(crate) fn after_refresh(view: &mut DocView, form: &[FormField]) {
    view.prepare.also.retain(|(name, wi)| form.iter().any(|f| &f.name == name && *wi < f.widgets.len()));
    if let Some(n) = view.prepare.select_added.take() {
        if form.len() > n
            && let Some(f) = form.last()
        {
            view.prepare.selected = Some((f.name.clone(), 0));
        } else if let Some((name, _)) = &view.prepare.selected
            && let Some(f) = form.iter().find(|f| &f.name == name)
        {
            // A radio button joined the selected group: select the new button.
            view.prepare.selected = Some((f.name.clone(), f.widgets.len().saturating_sub(1)));
        }
    }
    if let Some((name, wi)) = &view.prepare.selected
        && !form.iter().any(|f| &f.name == name && *wi < f.widgets.len())
    {
        view.prepare.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn corner_drags_stretch_freely_and_keep_proportions_with_shift() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
        let free = dragged(r, Grab::Corner(false, false), vec2(24.0, 0.0), false);
        assert_eq!(free, Rect::from_min_max(pos2(100.0, 100.0), pos2(220.0, 132.0)));
        for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
            let d = vec2(if left { -48.0 } else { 48.0 }, 0.0);
            let out = dragged(r, Grab::Corner(left, top), d, true);
            assert!((out.width() / out.height() - 3.0).abs() < 0.001, "{left} {top}: {out:?}");
            // The opposite corner stays put.
            assert_eq!(if left { out.max.x } else { out.min.x }, if left { r.max.x } else { r.min.x });
            assert_eq!(if top { out.max.y } else { out.min.y }, if top { r.max.y } else { r.min.y });
        }
        assert_eq!(dragged(r, Grab::Move, vec2(5.0, 6.0), true), r.translate(vec2(5.0, 6.0)));
    }
}
