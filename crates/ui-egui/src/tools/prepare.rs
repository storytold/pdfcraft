//! Prepare a form (Acrobat's Prepare Form, execution plan M6.6): add fields with the field
//! tools (click for the default size, or drag a rectangle), select, move and resize them, see
//! their name tags, delete them, and edit them in Field Properties.
//!
//! While the Prepare a form panel is open the page shows every field with a light-blue fill and
//! its name; clicks select fields instead of filling them in, as in Acrobat. The pointer work is
//! [`tool::PrepareTool`]; this module keeps the selection, the field tools and the pure geometry.

use craft::behavior::geometry::MoveGesture;
use craft::behavior::item_marquee::MarqueeSelect;
use egui::{Pos2, Rect, vec2};
use pdfcraft_engine::form_scripts::{CalcOp, Calculate, DATE_PRESETS, Format, TIME_PRESETS, Validate, format_value};
use pdfcraft_engine::{BorderStyle, Edit, FieldFont, FieldLook, FieldProps, FormField, FormFieldKind, NewField};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageBoxDrag, PageSession, PageXform};

pub(crate) mod dialog;
mod draft;
mod properties;
mod tabs;
pub(crate) mod tool;
pub use dialog::FieldPropsDialog;

pub use draft::{ACTION_KINDS, ActionDraft, FieldDraft, FieldTab};
pub(crate) use draft::{da_font, da_size};

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

/// The pointer gesture in progress on the pages. It lives here and not in the tool because the
/// tool is built again every pass.
#[derive(Clone, Debug, Default, PartialEq)]
enum Gesture {
    #[default]
    Idle,
    /// Drawing a new field: the rectangle, and where the press began (a click places the default
    /// size with its top-left there).
    Draw { drag: PageBoxDrag, origin: Pos2 },
    /// Selecting fields inside a dragged rectangle.
    Marquee { marquee: MarqueeSelect<(String, usize)>, dragged: bool },
    /// Moving a field; the gesture runs in screen points.
    Move { page: usize, gesture: MoveGesture, widget: (String, usize) },
}

/// What a gesture asks of the page body; read once with [`PrepareView::take_outcome`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PrepareOutcome {
    /// Open Field Properties for the selected field.
    pub properties: bool,
    /// A field was placed: go back to the Select tool.
    pub placed: bool,
    /// A table of text fields was drawn: open its window for this page and area (view space).
    pub table: Option<(usize, [f64; 4])>,
    /// An edit produced by the tool (adding, moving, nudging or deleting fields).
    pub edit: Option<Edit>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrepareView {
    /// Show each fillable widget's position in the same order used by Tab and Shift+Tab.
    pub show_tab_numbers: bool,
    /// The selected field and which of its widgets (the anchor of Align and Match Size).
    pub selected: Option<(String, usize)>,
    /// More fields selected with Shift- or ⌘-click.
    pub also: Vec<(String, usize)>,
    /// Select the field an Add field edit creates (the form's length before it).
    pub select_added: Option<usize>,
    gesture: Gesture,
    outcome: PrepareOutcome,
}

impl PrepareView {
    /// What the last gesture asked for, taken so it is read once.
    pub(crate) fn take_outcome(&mut self) -> PrepareOutcome {
        std::mem::take(&mut self.outcome)
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

fn tab_badge_rect(widget: Rect, page: Rect, size: egui::Vec2) -> Option<Rect> {
    if !page.is_positive() || !size.is_finite() || size.x < 0.0 || size.y < 0.0 || size.x > page.width() || size.y > page.height() {
        return None;
    }
    let max_x = page.right() - size.x;
    let max_y = page.bottom() - size.y;
    let min = egui::pos2((widget.left() + 3.0).clamp(page.left(), max_x), (widget.top() + 3.0).clamp(page.top(), max_y));
    Some(Rect::from_min_size(min, size))
}

fn place_tab_badge(widget: Rect, page: Rect, size: egui::Vec2, placed: &[Rect]) -> Option<Rect> {
    let preferred = tab_badge_rect(widget, page, size)?;
    if placed.iter().all(|badge| !badge.intersects(preferred)) {
        return Some(preferred);
    }

    let gap = 2.0;
    let step = size + vec2(gap, gap);
    let columns = (((page.width() - size.x) / step.x).floor().max(0.0) as usize).saturating_add(1);
    let rows = (((page.height() - size.y) / step.y).floor().max(0.0) as usize).saturating_add(1);
    let attempts = columns.saturating_mul(rows).min(placed.len().saturating_add(1));
    for slot in 0..attempts {
        let col = slot % columns;
        let row = slot / columns;
        let min = page.min + vec2(col as f32 * step.x, row as f32 * step.y);
        let candidate = Rect::from_min_size(min, size);
        if placed.iter().all(|badge| !badge.intersects(candidate)) {
            return Some(candidate);
        }
    }
    None
}

/// A right-click on widget `widget` of field `field` (the top hit there): the field is selected,
/// so the menu acts on it. Part of a selection of several, it becomes the anchor the others line
/// up with. Not while a gesture runs: the release acts on the selection the gesture began with.
pub(crate) fn context(form: &[FormField], field: usize, widget: usize, session: &mut PageSession) {
    let Some(f) = form.get(field) else { return };
    let key = (f.name.clone(), widget);
    let prep = &mut session.prepare;
    if !matches!(prep.gesture, Gesture::Idle) {
        return;
    }
    if let Some(i) = prep.also.iter().position(|k| *k == key) {
        prep.also.remove(i);
        if let Some(old) = prep.selected.replace(key) {
            prep.also.push(old);
        }
    } else if prep.selected.as_ref() != Some(&key) {
        prep.selected = Some(key);
        prep.also.clear();
    }
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

/// Delete the selected field and every field selected with it (#95), as one undoable step, and
/// clear the selection. Each selection is one widget, so selecting one radio button of a group,
/// or one copy of a duplicated field, deletes only that one, as in Acrobat (#4609); a field's
/// last widget takes the field with it.
pub(crate) fn delete_selected(session: &mut PageSession) -> Option<Edit> {
    delete_selected_from(&mut session.prepare)
}

pub(crate) fn delete_selected_from(prepare: &mut PrepareView) -> Option<Edit> {
    let first = prepare.selected.take()?;
    let mut picked = vec![first];
    for w in std::mem::take(&mut prepare.also) {
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

/// Select every widget on the current page.
pub(crate) fn select_all(view: &mut DocView, form: &[FormField]) {
    let current = view.current();
    let mut selected =
        form.iter().flat_map(|f| f.widgets.iter().enumerate().filter(move |(_, w)| w.page == Some(current)).map(|(wi, _)| (f.name.clone(), wi)));
    view.session.prepare.selected = selected.next();
    view.session.prepare.also = selected.collect();
}

/// After an Add field edit: select the new field (the form's last).
pub(crate) fn after_refresh(session: &mut PageSession, form: &[FormField]) {
    session.prepare.also.retain(|(name, wi)| form.iter().any(|f| &f.name == name && *wi < f.widgets.len()));
    if let Some(n) = session.prepare.select_added.take() {
        if form.len() > n
            && let Some(f) = form.last()
        {
            session.prepare.selected = Some((f.name.clone(), 0));
        } else if let Some((name, _)) = &session.prepare.selected
            && let Some(f) = form.iter().find(|f| &f.name == name)
        {
            // A radio button joined the selected group: select the new button.
            session.prepare.selected = Some((f.name.clone(), f.widgets.len().saturating_sub(1)));
        }
    }
    if let Some((name, wi)) = &session.prepare.selected
        && !form.iter().any(|f| &f.name == name && *wi < f.widgets.len())
    {
        session.prepare.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn tab_badge_rect_tracks_dragged_rect_and_clamps_to_page_edges() {
        let page = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 80.0));
        let widget = Rect::from_min_size(pos2(30.0, 40.0), vec2(20.0, 15.0));
        let size = vec2(24.0, 14.0);
        let original = tab_badge_rect(widget, page, size).unwrap();
        let moved = widget.translate(vec2(12.0, 8.0));
        assert_eq!(tab_badge_rect(moved, page, size).unwrap().min, original.min + vec2(12.0, 8.0));

        let near_edge = Rect::from_min_size(pos2(100.0, 95.0), vec2(8.0, 8.0));
        let badge = tab_badge_rect(near_edge, page, size).unwrap();
        assert!(page.contains_rect(badge));
    }

    #[test]
    fn tab_badge_is_omitted_when_it_cannot_fit_on_the_page() {
        let page = Rect::from_min_size(pos2(10.0, 20.0), vec2(20.0, 12.0));
        let widget = Rect::from_min_size(pos2(10.0, 20.0), vec2(4.0, 4.0));
        assert_eq!(tab_badge_rect(widget, page, vec2(24.0, 14.0)), None);
    }

    #[test]
    fn tab_badges_near_the_right_edge_do_not_overlap() {
        let page = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 80.0));
        let size = vec2(24.0, 14.0);
        let first_widget = Rect::from_min_size(pos2(104.0, 45.0), vec2(8.0, 8.0));
        let second_widget = Rect::from_min_size(pos2(106.0, 45.0), vec2(8.0, 8.0));
        let first = place_tab_badge(first_widget, page, size, &[]).unwrap();
        let second = place_tab_badge(second_widget, page, size, &[first]).unwrap();
        assert!(!first.intersects(second), "{first:?} overlaps {second:?}");
        assert!(page.contains_rect(first) && page.contains_rect(second));
    }
}
