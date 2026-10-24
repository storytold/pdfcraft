//! The layout engine: places a template's subforms, draws and fields on pages (XFA 3.3 part 2,
//! "Layout"). Supports positioned, top-to-bottom, left-to-right and table/row layouts, page
//! masters with content areas, `breakBefore`/`breakAfter`, and overflow leaders (header rows
//! repeated after a page break). Repeating subforms get their initial instances.
//!
//! Output is a list of pages of resolution-independent items; `pdf` turns them into PDF.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;

use crate::XfaError;
use crate::data::{DataNode, data_date_letters, data_path, data_to_iso, date_letters, is_on, iso_to_pattern};
use crate::model::{
    Access, Area, Border, Common, Draw, ExclGroup, Field, Font, HAlign, Layout, Margin, Node, Occur, PageArea, PageRole, Para, Placement, Presence,
    Rect, Script, SomIndices, Stroke, Subform, Template, Ui, VAlign, Value,
};
use crate::text::{self, Block, Face, Span};

/// Most pages a form may lay out to.
pub const MAX_PAGES: usize = 500;
/// Most items (text spans, lines, widgets…) across all pages.
const MAX_ITEMS: usize = 400_000;
/// Deepest nesting followed.
const MAX_DEPTH: usize = 64;
/// Most size measurements of one layout. Measurements of containers are memoized, so a real
/// form stays far below this; it bounds the work a hostile template can cause.
const MAX_MEASURES: usize = 2_000_000;
/// Most initial instances of one repeating subform.
const MAX_INSTANCES: usize = 50;
/// Largest picture taken from the data (decoded bytes), as for template pictures.
const MAX_DATA_IMAGE: usize = 32 << 20;
/// Most picture bytes one layout keeps, all copies together. Each laid-out picture holds its
/// own copy, so a large picture in repeating subforms would otherwise multiply into gigabytes
/// (a failed allocation aborts the app); a real form's logos and photos stay far below.
const MAX_IMAGE_BYTES: usize = 256 << 20;
/// Placeholder for the page count in text laid out before the count is known.
const PAGE_COUNT_MARK: char = '\u{E000}';

/// How far below its baseline an underline is drawn, in the text's size: an underline is the
/// level line pushed just before its span, this far below it (the PDF writer tells it apart from
/// a rule by this).
pub(crate) const UNDERLINE_DROP: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Fill {
        rect: Rect,
        color: [f64; 3],
    },
    Line {
        from: (f64, f64),
        to: (f64, f64),
        width: f64,
        color: [f64; 3],
        dashed: bool,
    },
    /// A box with rounded (or inverted) corners: its fill, and its edge drawn inside it.
    Rounded(Box<Rounded>),
    Text(Span),
    Image {
        rect: Rect,
        data: Arc<Vec<u8>>,
        content_type: String,
    },
    Widget(Box<Widget>),
}

/// A box with rounded corners ([`Item::Rounded`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Rounded {
    pub rect: Rect,
    /// Top-left, top-right, bottom-right and bottom-left radii, each at most half the box's
    /// shorter side.
    pub radii: [f64; 4],
    /// Corners whose curve goes into the box rather than around it, in the same order.
    pub inverted: [bool; 4],
    pub fill: Option<[f64; 3]>,
    /// The edge: its width, colour and whether it is dashed.
    pub edge: Option<(f64, [f64; 3], bool)>,
    /// A page break cut the box here: that side has no edge, and its corners are square.
    pub open_top: bool,
    pub open_bottom: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    Text,
    /// A text field whose characters are shown as asterisks.
    Password,
    /// A drop-down (`list_box` false) or list box: `options` are (saved value, shown text).
    Choice {
        options: Vec<(String, String)>,
        list_box: bool,
        multi: bool,
        editable: bool,
    },
    /// A signature field, to be signed in the app.
    Signature,
    /// An image field: a push button that picks a picture (the picture itself is page content).
    Image,
    /// A date field with its Acrobat format pattern (`yyyy-mm-dd`).
    Date(String),
    CheckBox {
        on: String,
        round: bool,
    },
    Radio {
        group: String,
        on: String,
        round: bool,
    },
    Button {
        caption: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderShape {
    None,
    Full,
    Underline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetBorder {
    pub color: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
    pub shape: BorderShape,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Reset,
    Print,
    SaveAs,
    Url(String),
    /// Any other click script: run by the XFA scripting engine.
    Script(String),
}

/// An interactive field to create as an AcroForm widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub rect: Rect,
    pub kind: WidgetKind,
    /// The PDF field name (unique in the form, no periods).
    pub name: String,
    /// The template's scripting object model path, for data round trips.
    pub som: String,
    pub tooltip: Option<String>,
    pub face: Face,
    pub h_align: HAlign,
    pub multiline: bool,
    pub max_chars: Option<usize>,
    pub read_only: bool,
    pub border: WidgetBorder,
    /// The field's value: saved (bound) values for choice lists, newline-separated when several.
    pub value: Option<String>,
    /// The template's default value (what a reset restores); data values are not defaults.
    pub default: Option<String>,
    pub action: Option<Action>,
    /// Check boxes: the on and off values of the data; radio buttons: the on value.
    pub items: Vec<String>,
    /// A text field that doesn't scroll: it takes no more text than it shows (`DoNotScroll`).
    pub no_scroll: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub pages: Vec<Page>,
    pub fields: usize,
    pub warnings: Vec<String>,
}

/// Inherited text defaults and the SOM path of the enclosing containers.
#[derive(Clone)]
struct Ctx {
    font: Font,
    para: Para,
    som: String,
}

impl Ctx {
    fn child(&self, c: &Common) -> Ctx {
        Ctx { font: self.font.apply(&c.font), para: self.para.apply(&c.para), som: self.som.clone() }
    }

    /// [`Self::child`] for object `c`, its SOM path extended by its name and `index` among the
    /// siblings of that name (an unnamed object adds nothing).
    fn with_som(&self, c: &Common, index: usize) -> Ctx {
        let mut out = self.child(c);
        if let Some(n) = &c.name {
            if !out.som.is_empty() {
                out.som.push('.');
            }
            out.som.push_str(&format!("{n}[{index}]"));
        }
        out
    }
}

/// (address of a subform or area in the template, available width bits, depth).
type MeasureKey = (usize, u64, usize);

/// A cached size, with the inherited text defaults it was measured with.
struct Measured {
    font: Font,
    para: Para,
    size: (f64, f64),
}

/// A point in the flow ([`Layouter::flow_mark`]): the page and content area, the height
/// reached there, and how many items the page had then.
#[derive(Clone, Copy, Debug)]
struct FlowMark {
    page: usize,
    area: usize,
    content: usize,
    y: f64,
    items: usize,
}

/// A flowed subform being laid out ([`Layouter::frames`]): where it sits across, its margins and
/// border, its part on the content area the flow is in, and its parts on earlier ones (page
/// index, the page's first item of the part, the part's box).
struct Frame {
    x: f64,
    w: f64,
    margin: Margin,
    border: Option<Border>,
    part: FlowMark,
    parts: Vec<(usize, usize, Rect)>,
    /// Something inside didn't fit its content area: an item cut across areas, or one placed past
    /// the area's bottom (a table row taller than an area, an overflow leader and the row after it
    /// that don't fit together). The border is left out ([`Layouter::finish_frame`]).
    lost: bool,
}

struct Cursor {
    area: usize,
    content: usize,
    rect: Rect,
    y: f64,
    /// Something from the flow was placed in this content area.
    placed: bool,
}

struct Layouter<'t> {
    tpl: &'t Template,
    data: Option<&'t DataNode>,
    /// Shared so a page's master items keep their addresses (the measurement cache keys on them).
    areas: Rc<Vec<PageArea>>,
    uses: Vec<usize>,
    pages: Vec<Page>,
    cur: Option<Cursor>,
    names: HashMap<String, usize>,
    /// Field values by `id`, for embedded fields in rich text.
    values: HashMap<String, String>,
    warnings: Vec<String>,
    warned: HashSet<String>,
    fields: usize,
    items: usize,
    /// Picture bytes laid out so far (see [`MAX_IMAGE_BYTES`]).
    image_bytes: usize,
    /// Container sizes by (address of the subform or area in the template, available width,
    /// depth), with the inherited font and paragraph they were measured with.
    measured: RefCell<HashMap<MeasureKey, Measured>>,
    /// Measurements made so far (see [`MAX_MEASURES`]).
    measures: Cell<usize>,
    /// The fields and draws of the block being placed: (first item, one past the last, top,
    /// bottom) on the last page. A page break keeps each whole (see `split_block`).
    leaves: Vec<(usize, usize, f64, f64)>,
    /// The flowed subforms being laid out, outermost first: a break between their children
    /// closes their parts on one content area and opens new ones on the next.
    frames: Vec<Frame>,
}

/// How many instances of a repeating subform to lay out: what the data holds once it holds
/// any (rows added or removed), else the template's initial count; never below one, never
/// above the maximum.
pub fn instance_count(occur: &Occur, from_data: usize) -> usize {
    let n = if from_data > 0 { from_data } else { occur.initial };
    n.min(occur.max.unwrap_or(usize::MAX)).clamp(1, MAX_INSTANCES)
}

/// Lay a template out, with the values and repeat counts of `data` (the `xfa:data` element).
pub fn layout(tpl: &Template, data: Option<&DataNode>) -> Result<Form, XfaError> {
    let areas = match &tpl.root.page_set {
        Some(ps) if !ps.areas.is_empty() => ps.areas.clone(),
        _ => vec![PageArea {
            name: None,
            width: 612.0,
            height: 792.0,
            content: vec![Rect::new(36.0, 36.0, 540.0, 720.0)],
            items: Vec::new(),
            occur_max: None,
            occur_min: 0,
        }],
    };
    let mut values = HashMap::new();
    // One copy of the root for the whole layout: measurements are cached by node address.
    let root_node = Node::Subform(Box::new(tpl.root.clone()));
    collect_values(std::slice::from_ref(&root_node), &mut values, 0);
    let mut l = Layouter {
        tpl,
        data,
        uses: vec![0; areas.len()],
        areas: Rc::new(areas),
        pages: Vec::new(),
        cur: None,
        names: HashMap::new(),
        values,
        warnings: Vec::new(),
        warned: HashSet::new(),
        fields: 0,
        items: 0,
        image_bytes: 0,
        measured: RefCell::new(HashMap::new()),
        measures: Cell::new(0),
        leaves: Vec::new(),
        frames: Vec::new(),
    };
    let ctx = Ctx { font: Font::default(), para: Para::default(), som: String::new() };
    l.new_page()?;
    if tpl.root.common.presence.occupies() {
        l.flow_subform(&root_node, None, &ctx, 0, 0)?;
    }
    l.satisfy_page_area_minimums()?;
    l.check_measures()?;
    let total = l.pages.len();
    let count = total.to_string();
    for p in &mut l.pages {
        for it in &mut p.items {
            if let Item::Text(s) = it
                && s.text.contains(PAGE_COUNT_MARK)
            {
                s.text = s.text.replace(PAGE_COUNT_MARK, &count);
            }
        }
    }
    Ok(Form { pages: l.pages, fields: l.fields, warnings: l.warnings })
}

// Bytes of embedded field values copied out on this thread, for tests that bound layout work.
#[cfg(test)]
thread_local! {
    pub(crate) static EMBED_COPIED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn collect_values(nodes: &[Node], out: &mut HashMap<String, String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        match n {
            Node::Field(f) => {
                if let Some(id) = &f.common.id {
                    out.insert(id.clone(), f.value.plain().unwrap_or_default());
                }
            }
            Node::Subform(s) => {
                collect_values(&s.children, out, depth + 1);
                if let Some(ps) = &s.page_set {
                    for a in &ps.areas {
                        collect_values(&a.items, out, depth + 1);
                    }
                }
            }
            Node::Area(a) => collect_values(&a.children, out, depth + 1),
            Node::ExclGroup(_) | Node::Draw(_) => {}
        }
    }
}

fn clamp_opt(v: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut v = v;
    if let Some(m) = min {
        v = v.max(m);
    }
    if let Some(m) = max
        && m > 0.0
    {
        v = v.min(m);
    }
    v.max(0.0)
}

/// Does `node` flow (may be split across pages) rather than being placed as one block?
fn splittable(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if matches!(s.layout, Layout::Tb | Layout::Table) && s.common.h.is_none())
}

/// Top and bottom of an item a page break must not cut (text, a picture, a field); `None`
/// for fills and lines, which are cut at the break.
fn solid_extent(item: &Item) -> Option<(f64, f64)> {
    match item {
        Item::Text(s) => Some((s.baseline - s.face.size * 0.8, s.baseline + s.face.size * 0.2)),
        Item::Image { rect, .. } => Some((rect.y, rect.bottom())),
        Item::Widget(w) => Some((w.rect.y, w.rect.bottom())),
        Item::Fill { .. } | Item::Line { .. } | Item::Rounded(_) => None,
    }
}

/// `item` as the part above `cut` and the part below: solid items go whole to the side their
/// top is on, fills and lines crossing the cut are cut in two.
fn divide(item: Item, cut: f64) -> (Option<Item>, Option<Item>) {
    if let Some((top, _)) = solid_extent(&item) {
        return if top < cut - 0.01 { (Some(item), None) } else { (None, Some(item)) };
    }
    match item {
        Item::Fill { rect, color } => {
            if rect.bottom() <= cut + 0.01 {
                (Some(Item::Fill { rect, color }), None)
            } else if rect.y >= cut - 0.01 {
                (None, Some(Item::Fill { rect, color }))
            } else {
                let above = Rect::new(rect.x, rect.y, rect.w, cut - rect.y);
                let below = Rect::new(rect.x, cut, rect.w, rect.bottom() - cut);
                (Some(Item::Fill { rect: above, color }), Some(Item::Fill { rect: below, color }))
            }
        }
        Item::Line { from, to, width, color, dashed } => {
            let (lo, hi) = if from.1 <= to.1 { (from, to) } else { (to, from) };
            if lo.1 >= cut {
                (None, Some(Item::Line { from, to, width, color, dashed }))
            } else if hi.1 <= cut {
                (Some(Item::Line { from, to, width, color, dashed }), None)
            } else {
                // lo.1 < cut < hi.1, so the divisor is positive.
                let t = (cut - lo.1) / (hi.1 - lo.1);
                let mid = (lo.0 + (hi.0 - lo.0) * t, cut);
                (Some(Item::Line { from: lo, to: mid, width, color, dashed }), Some(Item::Line { from: mid, to: hi, width, color, dashed }))
            }
        }
        Item::Rounded(r) => {
            if r.rect.bottom() <= cut + 0.01 {
                (Some(Item::Rounded(r)), None)
            } else if r.rect.y >= cut - 0.01 {
                (None, Some(Item::Rounded(r)))
            } else {
                // Each part keeps the corners on its side, rounded no more than its own height
                // allows, and is open where it was cut.
                let [tl, tr, br, bl] = r.radii;
                let above = Rect::new(r.rect.x, r.rect.y, r.rect.w, cut - r.rect.y);
                let below = Rect::new(r.rect.x, cut, r.rect.w, r.rect.bottom() - cut);
                let fit = |v: f64, h: f64| v.min(h.max(0.0));
                let upper = Rounded { rect: above, radii: [fit(tl, above.h), fit(tr, above.h), 0.0, 0.0], open_bottom: true, ..(*r).clone() };
                let lower = Rounded { rect: below, radii: [0.0, 0.0, fit(br, below.h), fit(bl, below.h)], open_top: true, ..*r };
                (Some(Item::Rounded(Box::new(upper))), Some(Item::Rounded(Box::new(lower))))
            }
        }
        other => (Some(other), None),
    }
}

/// `item` moved by (`dx`, `dy`).
fn moved(mut item: Item, dx: f64, dy: f64) -> Item {
    match &mut item {
        Item::Fill { rect, .. } | Item::Image { rect, .. } => {
            rect.x += dx;
            rect.y += dy;
        }
        Item::Rounded(r) => {
            r.rect.x += dx;
            r.rect.y += dy;
        }
        Item::Line { from, to, .. } => {
            *from = (from.0 + dx, from.1 + dy);
            *to = (to.0 + dx, to.1 + dy);
        }
        Item::Text(s) => {
            s.x += dx;
            s.baseline += dy;
        }
        Item::Widget(w) => {
            w.rect.x += dx;
            w.rect.y += dy;
        }
    }
    item
}

fn is_row(node: &Node) -> bool {
    matches!(node, Node::Subform(s) if s.layout == Layout::Row)
}

/// The Acrobat date pattern for an XFA picture clause such as `date{YYYY-MM-DD}`.
pub(crate) fn date_pattern(picture: Option<&str>) -> String {
    picture.and_then(date_letters).unwrap_or_else(|| "yyyy-mm-dd".into())
}

/// What scripts read as a field's `formattedValue` when its display picture is `picture`: the
/// ISO date `raw` shown in a date picture (`date{YYYYMMDD}` turns `2026-10-20` into
/// `20261020`; of several alternatives, the first). `None` for other kinds of picture, for a
/// clause this can't read or show (month and weekday names) and for a value that isn't an ISO
/// date: the raw value stands then.
pub fn formatted_value(picture: &str, raw: &str) -> Option<String> {
    let first = picture.split('|').next()?.trim();
    let rest = first.strip_prefix("date")?;
    if !rest.starts_with(['{', '(']) {
        return None;
    }
    let pattern = date_pattern(Some(first));
    // A picture with parts this can't show yet (month or weekday names, `MMM` or `EEE`) would
    // leave them as letters: the raw value stands then.
    let shows = |t: &String| !t.chars().all(|c| c.is_ascii_alphabetic()) || matches!(t.as_str(), "yyyy" | "yy" | "mm" | "m" | "dd" | "d");
    if !crate::data::date_tokens(&pattern).iter().all(shows) {
        return None;
    }
    // Only the ISO date's own shape (`2026-10-20`): `2026-1-2` is not one, and stands.
    let b = raw.as_bytes();
    let iso = b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if !iso {
        return None;
    }
    let shown = iso_to_pattern(raw, &pattern);
    (shown != raw).then_some(shown)
}

/// The native PDF action a button's click script amounts to (a reset, print, save or link),
/// else the script itself to run; `None` without a click script.
fn button_action(scripts: &[Script]) -> Option<Action> {
    let s = scripts.iter().find(|s| s.activity == "click")?;
    let t = &s.text;
    // Only a single-statement idiom becomes a native PDF action; anything else is run by the
    // XFA scripting engine.
    let one = t.trim().trim_end_matches(';');
    let single = !one.contains(';') && !one.contains('\n');
    if !single {
        return (!t.trim().is_empty()).then(|| Action::Script(t.clone()));
    }
    if t.contains("resetData") {
        Some(Action::Reset)
    } else if t.contains("host.print") || t.contains("execMenuItem(\"Print\")") {
        Some(Action::Print)
    } else if t.contains("SaveAs") || t.contains("execMenuItem(\"Save\")") {
        Some(Action::SaveAs)
    } else if let Some(i) = t.find("launchURL(") {
        let rest = &t[i + "launchURL(".len()..];
        let q = rest.find(['"', '\''])?;
        let quote = rest[q..].chars().next()?;
        let body = &rest[q + 1..];
        let end = body.find(quote)?;
        Some(Action::Url(body[..end].to_string()))
    } else if t.trim().is_empty() {
        None
    } else {
        Some(Action::Script(t.clone()))
    }
}

/// A PDF field name: no periods (they separate hierarchy levels), no control characters.
fn clean_name(name: &str) -> String {
    let s: String = name.chars().filter(|c| !c.is_control()).map(|c| if c == '.' { '_' } else { c }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "field".into() } else { s }
}

impl Layouter<'_> {
    fn warn_once(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.warned.insert(m.clone()) && self.warnings.len() < 100 {
            self.warnings.push(m);
        }
    }

    fn page_no(&self) -> usize {
        self.pages.len()
    }

    /// The text an embedded field `id` shows, at most `max` bytes of a field value (the page
    /// number and count are a few bytes; the text layout cuts them).
    fn embed(&self, id: &str, max: usize) -> String {
        let v = match self.tpl.page_roles.iter().find(|(i, _)| i == id).map(|(_, r)| *r) {
            Some(PageRole::Number) => self.page_no().to_string(),
            Some(PageRole::Count) => PAGE_COUNT_MARK.to_string(),
            None => self.values.get(id).map(|v| text::prefix(v, max).to_string()).unwrap_or_default(),
        };
        #[cfg(test)]
        EMBED_COPIED.with(|c| c.set(c.get().saturating_add(v.len() as u64)));
        v
    }

    /// `TooLarge` once the measurement budget is spent (sizes measured after that are guesses).
    fn check_measures(&self) -> Result<(), XfaError> {
        if self.measures.get() > MAX_MEASURES {
            return Err(XfaError::TooLarge("the form is nested too deeply to lay out".into()));
        }
        Ok(())
    }

    fn push(&mut self, item: Item) -> Result<(), XfaError> {
        self.check_measures()?;
        self.items += 1;
        if self.items > MAX_ITEMS {
            return Err(XfaError::TooLarge("the form has too many objects to lay out".into()));
        }
        if let Some(p) = self.pages.last_mut() {
            p.items.push(item);
        }
        Ok(())
    }

    fn push_block(&mut self, b: Block) -> Result<(), XfaError> {
        for s in b.spans {
            if s.face.underline {
                let w = s.face.width(&s.text);
                let y = s.baseline + s.face.size * UNDERLINE_DROP;
                self.push(Item::Line { from: (s.x, y), to: (s.x + w, y), width: (s.face.size * 0.06).max(0.3), color: s.face.color, dashed: false })?;
            }
            if !s.text.trim().is_empty() {
                self.push(Item::Text(s))?;
            }
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── pages

    fn new_page(&mut self) -> Result<(), XfaError> {
        let idx =
            (0..self.areas.len()).find(|&i| self.areas[i].occur_max.is_none_or(|m| self.uses[i] < m)).unwrap_or(self.areas.len().saturating_sub(1));
        self.new_page_of(idx)
    }

    /// Once the body has been laid out, give every page area that has been used fewer times than
    /// its `occur min` the pages it still needs, in template order, after the body's pages.
    fn satisfy_page_area_minimums(&mut self) -> Result<(), XfaError> {
        let areas = Rc::clone(&self.areas);
        for (idx, area) in areas.iter().enumerate() {
            while self.uses.get(idx).is_some_and(|u| *u < area.occur_min) {
                self.new_page_of(idx)?;
            }
        }
        Ok(())
    }

    /// Start a page from page area `idx`.
    fn new_page_of(&mut self, idx: usize) -> Result<(), XfaError> {
        if self.pages.len() >= MAX_PAGES {
            return Err(XfaError::TooLarge(format!("the form lays out to more than {MAX_PAGES} pages")));
        }
        let areas = Rc::clone(&self.areas);
        let Some(area) = areas.get(idx) else {
            return Err(XfaError::Malformed("the template has no page area".into()));
        };
        if let Some(u) = self.uses.get_mut(idx) {
            *u += 1;
        }
        self.pages.push(Page { width: area.width, height: area.height, items: Vec::new() });
        let content = area.content.first().copied().unwrap_or(Rect::new(0.0, 0.0, area.width, area.height));
        self.cur = Some(Cursor { area: idx, content: 0, rect: content, y: content.y, placed: false });
        // Master page content: positioned draws and fields.
        let ctx = Ctx { font: Font::default(), para: Para::default(), som: area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_default() };
        let page_rect = Rect::new(0.0, 0.0, area.width, area.height);
        let sibs = SomIndices::of(&area.items);
        for (i, item) in area.items.iter().enumerate() {
            self.draw_positioned_child(item, page_rect, &ctx, 1, sibs.at(i))?;
        }
        Ok(())
    }

    /// Move to the next content area, or the next page.
    fn next_area(&mut self) -> Result<(), XfaError> {
        if let Some(c) = &self.cur
            && let Some(area) = self.areas.get(c.area)
            && let Some(next) = area.content.get(c.content + 1).copied()
        {
            let i = c.content + 1;
            self.cur = Some(Cursor { area: c.area, content: i, rect: next, y: next.y, placed: false });
            return Ok(());
        }
        self.new_page()
    }

    /// The current content area, how far down it the flow is, and whether anything has been
    /// placed in it; before the first page, an empty letter-size page from its top.
    fn cursor(&self) -> (Rect, f64, bool) {
        match &self.cur {
            Some(c) => (c.rect, c.y, c.placed),
            None => (Rect::new(0.0, 0.0, 612.0, 792.0), 0.0, false),
        }
    }

    /// Where the flow is now ([`FlowMark`]); `None` before the first page.
    fn flow_mark(&self) -> Option<FlowMark> {
        let c = self.cur.as_ref()?;
        Some(FlowMark { page: self.pages.len(), area: c.area, content: c.content, y: c.y, items: self.pages.last().map_or(0, |p| p.items.len()) })
    }

    /// Whether the flow is still in the content area `m` was taken in.
    fn on_area(&self, m: &FlowMark) -> bool {
        self.pages.len() == m.page && self.cur.as_ref().is_some_and(|c| c.area == m.area && c.content == m.content)
    }

    /// Whether page `page` (an index) got anything from item `at` on.
    fn has_items_from(&self, page: usize, at: usize) -> bool {
        self.pages.get(page).is_some_and(|p| p.items.len() > at)
    }

    /// Break the flowed subforms being laid out ([`Self::frames`]) across content areas, as
    /// another viewer breaks such a section on a synthetic form: each one's part here ends after
    /// its content and its bottom margin (the innermost first, each inside the next), the flow
    /// moves to the next content area, and each starts a new part there after its top margin
    /// (the outermost first). The top margins take only what the `need` points of what comes next
    /// leave of the new area: a child that fits a fresh area but not after the margins would
    /// otherwise run past its bottom, out of its border. `to_page` moves to a new page instead (a
    /// child's own page break), which breaks the subforms around it the same way.
    fn break_frames(&mut self, need: f64, to_page: bool) -> Result<(), XfaError> {
        let (rect, mut y, _) = self.cursor();
        let page = self.pages.len().saturating_sub(1);
        let here: Vec<bool> = self.frames.iter().map(|f| self.on_area(&f.part)).collect();
        let filled: Vec<bool> = self.frames.iter().map(|f| self.has_items_from(page, f.part.items)).collect();
        for ((f, here), filled) in self.frames.iter_mut().zip(here).zip(filled).rev() {
            y = (y + f.margin.bottom).min(rect.bottom());
            if !here {
                f.lost = true;
            } else if filled && y - f.part.y >= 0.01 {
                f.parts.push((page, f.part.items, Rect::new(f.x, f.part.y, f.w, y - f.part.y)));
            }
        }
        if to_page {
            self.new_page()?;
        } else {
            self.next_area()?;
        }
        let (rect, y, _) = self.cursor();
        let mut room = (rect.bottom() - y - need).max(0.0);
        for i in 0..self.frames.len() {
            let Some(mark) = self.flow_mark() else { break };
            let Some(f) = self.frames.get_mut(i) else { break };
            f.part = mark;
            // `max` before `min`: a NaN margin counts as none and leaves `room` a number.
            let top = f.margin.top.max(0.0).min(room);
            room = (room - top).max(0.0);
            self.advance(top);
        }
        Ok(())
    }

    /// Close flowed subform `f` once its content is placed: its last part ends where the flow
    /// is, and its border (edges and fill) is drawn round each part, closed on every side, under
    /// that part's content. One holding something that didn't fit its content area ([`Frame::lost`])
    /// is drawn without its border, and the layout says so.
    fn finish_frame(&mut self, mut f: Frame) -> Result<(), XfaError> {
        let Some(b) = f.border.take() else { return Ok(()) };
        if b.fill().is_none() && b.visible_edges() == [false; 4] {
            return Ok(());
        }
        let (area, end, _) = self.cursor();
        // The bottom margin stays in the content area, as at a break; content that itself ran
        // past it has already cost the section its border ([`Self::overflowed`]).
        let end = end.min(area.bottom()).max(end - f.margin.bottom);
        let page = self.pages.len().saturating_sub(1);
        if !self.on_area(&f.part) {
            f.lost = true;
        } else if self.has_items_from(page, f.part.items) && end - f.part.y >= 0.01 {
            f.parts.push((page, f.part.items, Rect::new(f.x, f.part.y, f.w, end - f.part.y)));
        }
        if f.lost {
            self.warn_once("a bordered section holding an item that doesn't fit on its page is drawn without its border");
            return Ok(());
        }
        // The last part first: a part's place on its page stays where it was found.
        for (page, at, rect) in f.parts.into_iter().rev() {
            self.insert_border(&b, rect, page, at)?;
        }
        Ok(())
    }

    /// Draw border `b` round `rect` on page `page` (an index), under what that page got from item
    /// `at` on.
    fn insert_border(&mut self, b: &Border, rect: Rect, page: usize, at: usize) -> Result<(), XfaError> {
        let before = self.pages.last().map_or(0, |p| p.items.len());
        self.draw_border(b, rect)?;
        let drawn: Vec<Item> = match self.pages.last_mut() {
            Some(p) => {
                let from = before.min(p.items.len());
                p.items.drain(from..).collect()
            }
            None => return Ok(()),
        };
        if let Some(target) = self.pages.get_mut(page) {
            let at = at.min(target.items.len());
            target.items.splice(at..at, drawn);
        }
        Ok(())
    }

    /// Move the cursor down (margins and spacing: the area still counts as empty).
    fn advance(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
        }
    }

    /// Move past something that was placed.
    fn place(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
            c.placed = true;
        }
        let (_, y, _) = self.cursor();
        self.overflowed(y);
    }

    /// Something was placed down to `bottom`: past the content area's bottom, the flowed subforms
    /// it is inside can't hold it in a part, and are drawn without their border.
    fn overflowed(&mut self, bottom: f64) {
        let (rect, _, _) = self.cursor();
        if bottom > rect.bottom() + 0.01 {
            for f in &mut self.frames {
                f.lost = true;
            }
        }
    }

    /// Make room for `h`, with `after` more drawn before it should it move (a table's overflow
    /// leader): stay, or move to the next content area / page. Returns whether a break happened.
    /// Something that fits a fresh area breaks the flowed subforms it is in with it
    /// ([`Self::break_frames`]), a table's rows included; one taller than an area just moves on,
    /// and those subforms are drawn without their border ([`Self::finish_frame`]).
    fn make_room(&mut self, h: f64, after: f64) -> Result<bool, XfaError> {
        let (rect, y, placed) = self.cursor();
        if y + h <= rect.bottom() + 0.01 || !placed {
            return Ok(false);
        }
        if h <= rect.h + 0.01 {
            self.break_frames(h + after, false)?;
        } else {
            self.next_area()?;
        }
        Ok(true)
    }

    /// Place `node` as one block of size (`w`, `h`) at the cursor: in this content area when it fits,
    /// in the next when it fits there, and across as many as it takes when it is taller than a
    /// content area (a positioned or fixed-height subform holding more than a page).
    fn flow_block(&mut self, node: &Node, x: f64, (w, h): (f64, f64), ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        let (rect, y, _) = self.cursor();
        if y + h > rect.bottom() + 0.01 && h <= rect.h + 0.01 {
            self.make_room(h, 0.0)?;
        }
        let (rect, y, _) = self.cursor();
        let first = self.pages.last().map_or(0, |p| p.items.len());
        self.leaves.clear();
        self.draw_node(node, Rect::new(x, y, w, h), ctx, depth, sib)?;
        let page_h = self.pages.last().map_or(f64::INFINITY, |p| p.height);
        // What was drawn, when it is lower than the block's own height says (content that
        // outgrows a fixed height) and ends off the page.
        let drawn = self
            .pages
            .last()
            .and_then(|p| p.items.get(first..))
            .unwrap_or_default()
            .iter()
            .filter_map(solid_extent)
            .map(|e| e.1)
            .fold(y + h, f64::max);
        if y + h <= rect.bottom() + 0.01 && drawn <= page_h + 0.01 {
            self.leaves.clear();
            self.place(h);
            return Ok(());
        }
        self.split_block(first, y, drawn)
    }

    /// Remember that the items the last page got since `start` are one field or draw at
    /// `rect`, to be kept together by a page break.
    fn keep_whole(&mut self, start: usize, rect: Rect) {
        let end = self.pages.last().map_or(0, |p| p.items.len());
        if end > start {
            self.leaves.push((start, end, rect.y, rect.bottom()));
        }
    }

    fn item_count(&self) -> usize {
        self.pages.last().map_or(0, |p| p.items.len())
    }

    /// Spread the items the last page holds from index `first` on (a block drawn from `top` to
    /// `end`, past the bottom of its content area) over this and the following content areas.
    /// Breaks fall between text, pictures and fields, never through one that fits in an area;
    /// fills and lines are cut at the break.
    fn split_block(&mut self, first: usize, top: f64, end: f64) -> Result<(), XfaError> {
        let items: Vec<Item> = match self.pages.last_mut() {
            Some(p) if first <= p.items.len() => p.items.split_off(first),
            _ => return Ok(()),
        };
        // Each item with the extent of the field or draw it belongs to: such a leaf moves
        // whole (its border with its text), unless it is taller than a content area.
        let area_h = self.cursor().0.h;
        let mut extents: Vec<Option<(f64, f64)>> = vec![None; items.len()];
        for (start, end, top, bottom) in std::mem::take(&mut self.leaves) {
            if bottom - top <= area_h + 0.01
                && let Some(slots) = start.checked_sub(first).and_then(|s| extents.get_mut(s..end.saturating_sub(first)))
            {
                slots.fill(Some((top, bottom)));
            }
        }
        let mut rest: Vec<(Item, Option<(f64, f64)>)> = items.into_iter().zip(extents).collect();
        let origin_x = self.cursor().0.x;
        // An item's place on the current page is its drawn place moved by (dx, -shift).
        let (mut dx, mut shift) = (0.0, 0.0);
        let mut seg_top = top;
        let mut first_segment = true;
        loop {
            let (rect, _, placed) = self.cursor();
            let limit = rect.bottom() + shift;
            let mut cut = limit;
            if rect.h < 1.0 {
                // No room to split into: leave the rest here.
                cut = f64::INFINITY;
            } else {
                // The lowest break at or above the bottom that crosses no solid item: going
                // through them from the lowest top up, a break moved to an item's top can't
                // cross one already passed.
                let mut solid: Vec<(f64, f64)> =
                    rest.iter().filter_map(|(item, kept)| kept.or_else(|| solid_extent(item))).filter(|e| e.0 < limit - 0.01).collect();
                solid.sort_by(|a, b| b.0.total_cmp(&a.0));
                for (t, b) in solid {
                    if t < cut - 0.01 && b > cut + 0.01 {
                        cut = t;
                    }
                }
                if cut <= seg_top + 0.01 {
                    // An item taller than what is left. Start it in a fresh area if this one
                    // already holds something; otherwise cut through it.
                    cut = if first_segment && placed && seg_top > rect.y + 0.01 { seg_top } else { limit };
                }
            }
            let mut here = Vec::new();
            let mut below = Vec::new();
            for (item, kept) in rest {
                if let Some((top, _)) = kept {
                    if top < cut - 0.01 {
                        here.push(item)
                    } else {
                        below.push((item, kept))
                    }
                    continue;
                }
                match divide(item, cut) {
                    (Some(a), None) => here.push(a),
                    (None, Some(b)) => below.push((b, None)),
                    (Some(a), Some(b)) => {
                        here.push(a);
                        below.push((b, None));
                    }
                    (None, None) => {}
                }
            }
            if let Some(p) = self.pages.last_mut() {
                p.items.extend(here.into_iter().map(|it| moved(it, dx, -shift)));
            }
            if below.is_empty() {
                if let Some(c) = &mut self.cur {
                    c.y = (end - shift).min(c.rect.bottom()).max(c.y);
                    c.placed = true;
                }
                self.overflowed(end - shift);
                return Ok(());
            }
            rest = below;
            seg_top = cut;
            first_segment = false;
            self.next_area()?;
            let (rect, _, _) = self.cursor();
            shift = cut - rect.y;
            dx = rect.x - origin_x;
        }
    }

    fn page_has_flow(&self) -> bool {
        self.cur.as_ref().is_some_and(|c| c.placed || c.content > 0)
    }

    // ───────────────────────────────────────────────────────────────────────── flow

    /// Flow `node` (a subform; anything else is ignored) into the content areas.
    fn flow_subform(&mut self, node: &Node, avail: Option<(f64, f64)>, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let Node::Subform(sf) = node else { return Ok(()) };
        self.check_measures()?;
        if sf.break_before_page && self.page_has_flow() {
            self.break_frames(0.0, true)?;
        }
        // Data adds instances of a repeating subform (rows added in another viewer).
        let instances = self.instances(node, &ctx.som);
        self.warn_truncated(node, &ctx.som, instances);
        let ctx_parent = ctx;
        for i in 0..instances {
            // Every instance spends budget, so nested repeats that place nothing still stop.
            self.measures.set(self.measures.get().saturating_add(1));
            self.check_measures()?;
            let ctx = ctx_parent.with_som(&sf.common, sib + i);
            let (rect, _, _) = self.cursor();
            let (x, w) = avail.unwrap_or((rect.x, rect.w));
            let w = clamp_opt(sf.common.w.unwrap_or(w), sf.common.min_w, sf.common.max_w).min(w.max(1.0));
            if !matches!(sf.layout, Layout::Tb | Layout::Table) || sf.common.h.is_some() {
                // Not splittable: one block.
                let (_, h) = self.size_of(node, w, ctx_parent, depth);
                self.flow_block(node, x, (w, h), ctx_parent, depth, sib + i)?;
                continue;
            }
            let m = sf.common.margin;
            // Its border and fill go round what it takes from here, a part on each content area.
            let framed = match self.flow_mark() {
                Some(part) => {
                    self.frames.push(Frame { x, w, margin: m, border: sf.common.border.clone(), part, parts: Vec::new(), lost: false });
                    true
                }
                None => false,
            };
            self.advance(m.top);
            let inner_x = x + m.left;
            let inner_w = (w - m.horizontal()).max(1.0);
            match sf.layout {
                Layout::Table => self.flow_table(sf, inner_x, inner_w, &ctx, depth)?,
                _ => {
                    let sibs = SomIndices::of(&sf.children);
                    for (ci, child) in sf.children.iter().enumerate() {
                        if !child.common().presence.occupies() {
                            continue;
                        }
                        let csib = sibs.at(ci);
                        if let Node::Subform(_) = child
                            && splittable(child)
                        {
                            self.flow_subform(child, Some((inner_x, inner_w)), &ctx, depth + 1, csib)?;
                            continue;
                        }
                        let (cw, ch) = self.size_of(child, inner_w, &ctx, depth + 1);
                        self.flow_block(child, inner_x, (cw, ch), &ctx, depth + 1, csib)?;
                    }
                }
            }
            self.advance(m.bottom);
            if framed && let Some(frame) = self.frames.pop() {
                self.finish_frame(frame)?;
            }
        }
        if sf.break_after_page {
            self.break_frames(0.0, true)?;
        }
        Ok(())
    }

    fn columns(&self, sf: &Subform, inner_w: f64) -> Vec<f64> {
        if !sf.column_widths.is_empty() {
            return sf.column_widths.clone();
        }
        let cells = sf.children.iter().filter_map(|c| if let Node::Subform(r) = c { Some(r.children.len()) } else { None }).max().unwrap_or(1).max(1);
        vec![inner_w / cells as f64; cells]
    }

    fn flow_table(&mut self, sf: &Subform, x: f64, inner_w: f64, ctx: &Ctx, depth: usize) -> Result<(), XfaError> {
        let cols = self.columns(sf, inner_w);
        let leader =
            sf.overflow_leader.as_ref().and_then(|name| sf.children.iter().position(|c| c.common().name.as_deref() == Some(name) && is_row(c)));
        // The overflow leader starts each part after the first, before the row that moved.
        let lead = leader.and_then(|li| match sf.children.get(li) {
            Some(Node::Subform(l)) => Some((li, l.as_ref())),
            _ => None,
        });
        let lead_h = lead.map_or(0.0, |(_, l)| self.row_height(l, &cols, inner_w, ctx, depth + 1));
        let sibs = SomIndices::of(&sf.children);
        for (ci, child) in sf.children.iter().enumerate() {
            if !child.common().presence.occupies() {
                continue;
            }
            let csib = sibs.at(ci);
            if let Node::Subform(row) = child
                && row.layout == Layout::Row
            {
                // Rows added as data (in another viewer) repeat the row.
                let instances = self.instances(child, &ctx.som);
                self.warn_truncated(child, &ctx.som, instances);
                for inst in 0..instances {
                    let h = self.row_height(row, &cols, inner_w, ctx, depth + 1);
                    let led = lead.filter(|(li, _)| *li != ci);
                    if self.make_room(h, led.map_or(0.0, |_| lead_h))?
                        && let Some((li, l)) = led
                    {
                        let (_, y, _) = self.cursor();
                        self.draw_row(l, &cols, Rect::new(x, y, inner_w, lead_h), ctx, depth + 1, sibs.at(li))?;
                        self.place(lead_h);
                    }
                    let (_, y, _) = self.cursor();
                    self.draw_row(row, &cols, Rect::new(x, y, inner_w, h), ctx, depth + 1, csib + inst)?;
                    self.place(h);
                }
                continue;
            }
            if splittable(child)
                && let Node::Subform(_) = child
            {
                self.flow_subform(child, Some((x, inner_w)), ctx, depth + 1, csib)?;
                continue;
            }
            let (cw, ch) = self.size_of(child, inner_w, ctx, depth + 1);
            self.flow_block(child, x, (cw, ch), ctx, depth + 1, csib)?;
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── sizing

    /// The size a node takes when given `avail_w` of width. Containers are measured once per
    /// (node, width, depth): measuring a width-less container measures its content twice (for
    /// its width, then its height), which would otherwise be exponential in the nesting.
    /// How many instances of `node` to lay out under the container at `parent_som`: a
    /// repeating subform's `occur` and data instances ([`instance_count`]); one for anything else.
    fn instances(&self, node: &Node, parent_som: &str) -> usize {
        let Node::Subform(sf) = node else { return 1 };
        let from_data = match (&sf.common.name, self.data) {
            (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
            _ => 0,
        };
        instance_count(&sf.occur, from_data)
    }

    /// The data holds more instances of `node` than `laid_out`: say so, since calculations
    /// then see only those.
    fn warn_truncated(&mut self, node: &Node, parent_som: &str, laid_out: usize) {
        let Node::Subform(sf) = node else { return };
        let from_data = match (&sf.common.name, self.data) {
            (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
            _ => 0,
        };
        if from_data > laid_out {
            self.warn_once(format!(
                "{}: the data holds {from_data} rows; only the first {laid_out} are laid out (calculations see those)",
                sf.common.name.as_deref().unwrap_or("a subform")
            ));
        }
    }

    /// The width and height `node` takes in `avail_w`: its own size, else what its content needs
    /// (containers are measured once per width and depth, and measuring stops at its caps).
    fn size_of(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        let spent = self.measures.get().saturating_add(1);
        self.measures.set(spent);
        if depth > MAX_DEPTH || spent > MAX_MEASURES {
            return (c.w.unwrap_or(avail_w), c.h.unwrap_or(0.0));
        }
        // Only containers recurse; their addresses are stable for the whole layout (the
        // template is borrowed, the root and page areas are held by the layouter).
        let key = match node {
            Node::Subform(s) => Some((&**s as *const Subform as usize, avail_w.to_bits(), depth)),
            Node::Area(a) => Some((&**a as *const Area as usize, avail_w.to_bits(), depth)),
            _ => None,
        };
        if let Some(key) = key
            && let Ok(cache) = self.measured.try_borrow()
            && let Some(m) = cache.get(&key)
            && m.font == ctx.font
            && m.para == ctx.para
        {
            return m.size;
        }
        let size = self.measure(node, avail_w, ctx, depth);
        if let Some(key) = key
            && let Ok(mut cache) = self.measured.try_borrow_mut()
        {
            cache.insert(key, Measured { font: ctx.font.clone(), para: ctx.para.clone(), size });
        }
        size
    }

    /// The size (width, height) of `node` within `avail_w`, uncached ([`Self::size_of`] caches it).
    /// Without a width, a container, a text draw or a button is as wide as its content (at most
    /// `avail_w`) and anything else takes `avail_w`; without a height, it is as tall as its content.
    fn measure(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        // The node's own SOM: its repeating children count their instances under it, and a
        // growing text field finds its value at it. Measurements are cached per template node,
        // so instance 0 stands for all of them.
        let ctx = ctx.with_som(c, 0);
        let som = ctx.som.clone();
        // A container without a width is as wide as its content (a radio group beside its
        // question, say), and so is a text label or a button (as wide as its caption), within
        // what is available; any other field without one takes what is available.
        let w = match c.w {
            Some(w) => w,
            None => match node {
                Node::Subform(_) | Node::ExclGroup(_) | Node::Area(_) => {
                    let inner = (avail_w - c.margin.horizontal()).max(1.0);
                    (self.content_width(node, inner, &ctx, depth) + c.margin.horizontal()).min(avail_w)
                }
                Node::Draw(d) => self.natural_width(&d.value, &ctx.font, &ctx.para).map_or(avail_w, |t| (t + c.margin.horizontal()).min(avail_w)),
                Node::Field(f) if f.ui == Ui::Button => f
                    .caption
                    .as_ref()
                    .and_then(|cap| self.natural_width(&cap.value, &ctx.font.apply(&cap.font), &ctx.para.apply(&cap.para)))
                    .map_or(avail_w, |t| (t + c.margin.horizontal() + f.ui_margin.horizontal()).min(avail_w)),
                Node::Field(_) => avail_w,
            },
        };
        let w = clamp_opt(w, c.min_w, c.max_w);
        let h = match c.h {
            Some(h) => h,
            None => {
                let inner_w = (w - c.margin.horizontal()).max(1.0);
                let content = match node {
                    Node::Draw(d) => self.value_height(&d.value, inner_w, &ctx),
                    Node::Field(f) => self.field_height(f, inner_w, &ctx),
                    Node::ExclGroup(g) => self.group_height(g, inner_w, &ctx, depth),
                    Node::Area(a) => self.positioned_height(&a.children, inner_w, &ctx, depth),
                    Node::Subform(s) => match s.layout {
                        Layout::Positioned => self.positioned_height(&s.children, inner_w, &ctx, depth),
                        // Each instance of a repeating child takes its height again.
                        Layout::Tb => s
                            .children
                            .iter()
                            .filter(|k| k.common().presence.occupies())
                            .map(|k| self.size_of(k, inner_w, &ctx, depth + 1).1 * self.instances(k, &som) as f64)
                            .sum(),
                        Layout::LrTb | Layout::RlTb => self.pack_instances(&s.children, inner_w, &ctx, depth, &som).1,
                        Layout::Table => {
                            let cols = self.columns(s, inner_w);
                            s.children
                                .iter()
                                .filter(|k| k.common().presence.occupies())
                                .map(|k| match k {
                                    Node::Subform(r) if r.layout == Layout::Row => self.row_height(r, &cols, inner_w, &ctx, depth + 1),
                                    _ => self.size_of(k, inner_w, &ctx, depth + 1).1,
                                })
                                .sum()
                        }
                        Layout::Row => self.row_height(s, &self.columns(s, inner_w), inner_w, &ctx, depth + 1),
                    },
                };
                content + c.margin.vertical()
            }
        };
        // A flowed subform is as tall as what it holds: other viewers do not stretch one to
        // the `minH` a designer left on it.
        let min_h = match node {
            Node::Subform(s) if c.h.is_none() && s.layout != Layout::Positioned => None,
            _ => c.min_h,
        };
        (w, clamp_opt(h, min_h, c.max_h))
    }

    /// The natural width of a container's content within `avail_w`.
    fn content_width(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let group: Vec<Node>;
        let (children, layout): (&[Node], Layout) = match node {
            Node::Subform(s) => {
                if s.layout == Layout::Table && !s.column_widths.is_empty() {
                    return s.column_widths.iter().sum::<f64>().min(avail_w);
                }
                (&s.children, s.layout)
            }
            Node::ExclGroup(g) => {
                // Fields are leaves: measuring these copies caches nothing.
                group = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                (&group, g.layout)
            }
            Node::Area(a) => (&a.children, Layout::Positioned),
            _ => return avail_w,
        };
        let visible = children.iter().filter(|k| k.common().presence.occupies());
        match layout {
            Layout::LrTb | Layout::RlTb | Layout::Row => {
                let sum: f64 = visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).sum();
                if sum <= avail_w + 0.05 { sum } else { avail_w }
            }
            Layout::Positioned => visible
                .map(|k| {
                    let (w, h) = self.size_of(k, avail_w, ctx, depth + 1);
                    k.common().top_left(w, h).0 + w
                })
                .fold(0.0, f64::max)
                .min(avail_w),
            Layout::Tb | Layout::Table => visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).fold(0.0, f64::max).min(avail_w),
        }
    }

    /// How wide text `v` is unwrapped, in `font` and `para`, with a hair to spare so that laying
    /// it out at that width keeps it on its lines; `None` for what isn't text, or text that draws
    /// nothing.
    fn natural_width(&self, v: &Value, font: &Font, para: &Para) -> Option<f64> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return None,
        };
        // No text (a bordered spacer or rule): nothing to size to, so it keeps what it has room for.
        let w = text::natural_width(&rich, para, &Face::of(font), &|id, max| self.embed(id, max));
        (w > 0.0).then_some(w + 0.01)
    }

    /// How tall value `v` is laid out `w` wide: text wrapped in the context's font and paragraph,
    /// an image at its aspect ratio, and nothing for lines, rectangles and empty values.
    fn value_height(&self, v: &Value, w: f64, ctx: &Ctx) -> f64 {
        match v {
            Value::Text(t) => text::measure_height(&text::plain(t), w, &ctx.para, &Face::of(&ctx.font), &|id, max| self.embed(id, max)),
            Value::Rich(r) => text::measure_height(r, w, &ctx.para, &Face::of(&ctx.font), &|id, max| self.embed(id, max)),
            Value::Image { data, .. } => crate::image::size(data).map_or(w, |(iw, ih)| if iw > 0 { w * ih as f64 / iw as f64 } else { w }),
            Value::Line { .. } | Value::Rectangle(_) | Value::Empty => 0.0,
        }
    }

    /// Where field `f`'s caption goes and how much room it takes from the field's box, or `None`
    /// when it has no caption that takes room (a button's is drawn inside its box).
    fn caption_reserve(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> Option<(Placement, f64)> {
        let cap = f.caption.as_ref()?;
        if !cap.presence.occupies() || f.ui == Ui::Button {
            return None;
        }
        if cap.value.is_empty() && cap.reserve.is_none() {
            return None;
        }
        // A check box has no text for an inline caption to run with: it is not shown.
        if f.ui == Ui::CheckButton && cap.placement == Placement::Inline {
            return None;
        }
        let placement = if cap.placement == Placement::Inline { Placement::Left } else { cap.placement };
        let font = ctx.font.apply(&cap.font);
        let para = ctx.para.apply(&cap.para);
        let reserve = cap.reserve.unwrap_or_else(|| match placement {
            Placement::Top | Placement::Bottom => {
                let rich = match &cap.value {
                    Value::Rich(r) => r.clone(),
                    other => text::plain(&other.plain().unwrap_or_default()),
                };
                text::measure_height(&rich, inner_w, &para, &Face::of(&font), &|id, max| self.embed(id, max))
            }
            _ => (Face::of(&font).width(&cap.value.plain().unwrap_or_default()) + 2.0).min(inner_w * 0.6),
        });
        Some((placement, reserve))
    }

    /// The height of field `f`'s content within `inner_w` (inside its margins): its widget (a check
    /// box, a button's caption, or one line of text or three when multi-line, more for a text field
    /// that grows) plus a caption above or below it, or the taller of the two for one beside it.
    fn field_height(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> f64 {
        let font = ctx.font.apply(&f.common.font);
        let lines = if f.multiline { 3.0 } else { 1.0 };
        let ui = match f.ui {
            Ui::CheckButton => f.check_size.unwrap_or(10.0).max(text::line_height(font.size)),
            // A button shows its caption: as tall as the caption's text in its own font.
            Ui::Button if f.caption.as_ref().is_some_and(|c| !c.value.is_empty()) => {
                let cap = f.caption.as_ref().map_or(0.0, |c| {
                    let rich = match &c.value {
                        Value::Rich(r) => r.clone(),
                        other => text::plain(&other.plain().unwrap_or_default()),
                    };
                    let w = (inner_w - f.ui_margin.horizontal()).max(1.0);
                    text::measure_height(&rich, w, &ctx.para.apply(&c.para), &Face::of(&ctx.font.apply(&c.font)), &|id, max| self.embed(id, max))
                });
                cap + f.ui_margin.vertical()
            }
            _ => (text::line_height(font.size) * lines).max(self.grown_text_height(f, inner_w, ctx)) + 4.0 + f.ui_margin.vertical(),
        };
        match self.caption_reserve(f, inner_w, ctx) {
            Some((Placement::Top | Placement::Bottom, r)) => ui + r,
            Some((placement, r)) => {
                // A check box's caption without a reserve is drawn in all the box leaves.
                let w = match auto_check_reserve(f, placement, inner_w, None) {
                    Some(auto) => auto,
                    None => inner_w - r,
                };
                let cap = f.caption.as_ref().map(|c| {
                    let rich = match &c.value {
                        Value::Rich(r) => r.clone(),
                        other => text::plain(&other.plain().unwrap_or_default()),
                    };
                    text::measure_height(&rich, w.max(1.0), &ctx.para.apply(&c.para), &Face::of(&ctx.font.apply(&c.font)), &|id, max| {
                        self.embed(id, max)
                    })
                });
                ui.max(cap.unwrap_or(0.0))
            }
            None => ui,
        }
    }

    /// The face a field's text is laid out in: at its horizontal scale when the layout draws
    /// the text itself (a non-interactive field), as designed when the forms layer draws it in
    /// a widget, whose appearances have no horizontal scaling.
    fn field_text_face(f: &Field, font: &Font) -> Face {
        let face = Face::of(font);
        if f.access == Access::NonInteractive { face } else { face.unscaled() }
    }

    /// The height the text of a multi-line text field takes (its value in the data, else the
    /// template's), for a field without a fixed height, which grows to hold its text. `ctx`
    /// is the field's own (its SOM ends in the field). Zero for anything else.
    fn grown_text_height(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> f64 {
        if !f.grows_with_text() || f.common.name.is_none() {
            return 0.0;
        }
        let value = self.data.and_then(|d| d.value_at(&data_path(self.tpl, &ctx.som))).filter(|t| !t.trim().is_empty()).or_else(|| f.value.plain());
        let Some(value) = value.filter(|t| !t.trim().is_empty()) else { return 0.0 };
        // Beside a caption the text has what the caption leaves; a widget's text has 2pt of
        // padding on each side, a non-interactive field's (drawn as text) none, so it wraps
        // where it is drawn.
        let beside = match self.caption_reserve(f, inner_w, ctx) {
            Some((Placement::Top | Placement::Bottom, _)) | None => 0.0,
            Some((_, r)) => r,
        };
        let padding = if f.access == Access::NonInteractive { 0.0 } else { 4.0 };
        let w = (inner_w - beside - f.ui_margin.horizontal() - padding).max(1.0);
        let font = ctx.font.apply(&f.common.font);
        text::measure_height(&text::plain(value.trim_end()), w, &ctx.para, &Self::field_text_face(f, &font), &|_, _| String::new())
    }

    /// The height of an exclusion group's fields at `inner_w`, by the group's layout: stacked
    /// top to bottom, at their own positions, or packed in rows.
    fn group_height(&self, g: &ExclGroup, inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        match g.layout {
            Layout::Tb => nodes.iter().filter(|k| k.common().presence.occupies()).map(|k| self.size_of(k, inner_w, ctx, depth + 1).1).sum(),
            Layout::Positioned => self.positioned_height(&nodes, inner_w, ctx, depth),
            _ => self.packed_height(&nodes, inner_w, ctx, depth),
        }
    }

    fn positioned_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        children
            .iter()
            .filter(|k| k.common().presence.occupies())
            .map(|k| {
                let c = k.common();
                let (w, h) = self.size_of(k, (inner_w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1);
                c.top_left(w, h).1 + h
            })
            .fold(0.0, f64::max)
    }

    /// Left-to-right packing: rows of children, wrapped at the available width, as (child,
    /// instance, place). A repeating subform is packed once per instance it has under the
    /// container at `som`.
    fn pack_instances(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize, som: &str) -> (Vec<(usize, usize, Rect)>, f64) {
        let mut out = Vec::new();
        let (mut x, mut y, mut line_h) = (0.0, 0.0, 0.0);
        for (i, k) in children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let (w, h) = self.size_of(k, inner_w, ctx, depth + 1);
            for instance in 0..self.instances(k, som) {
                if x > 0.0 && x + w > inner_w + 0.05 {
                    y += line_h;
                    x = 0.0;
                    line_h = 0.0;
                }
                out.push((i, instance, Rect::new(x, y, w, h)));
                x += w;
                line_h = line_h.max(h);
            }
        }
        (out, y + line_h)
    }

    /// [`Self::pack_instances`] for children that don't repeat (the fields of a group).
    fn pack(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let (placed, h) = self.pack_instances(children, inner_w, ctx, depth, "");
        (placed.into_iter().map(|(i, _, r)| (i, r)).collect(), h)
    }

    fn packed_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.pack(children, inner_w, ctx, depth).1
    }

    /// Cell rectangles of a row (relative to the row's inner box) and the row height.
    fn cells(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let ctx = ctx.child(&row.common);
        let mut out = Vec::new();
        let mut col = 0usize;
        let mut x = 0.0;
        let mut h: f64 = 0.0;
        for (i, k) in row.children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let span = k.common().col_span.max(1);
            let w = if col < cols.len() { cols.iter().skip(col).take(span).sum::<f64>() } else { k.common().w.unwrap_or((inner_w - x).max(1.0)) };
            let (_, ch) = self.size_of(k, w, &ctx, depth + 1);
            out.push((i, Rect::new(x, 0.0, w, ch)));
            h = h.max(ch);
            x += w;
            col += span;
        }
        let h = match row.common.h {
            Some(rh) => rh,
            None => h + row.common.margin.vertical(),
        };
        (out, clamp_opt(h, row.common.min_h, row.common.max_h))
    }

    fn row_height(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.cells(row, cols, inner_w, ctx, depth).1
    }

    // ───────────────────────────────────────────────────────────────────────── drawing

    fn draw_row(&mut self, row: &Subform, cols: &[f64], rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        if let Some(b) = &row.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&row.common, sib);
        let inner = rect.inset(&row.common.margin);
        let (cells, _) = self.cells(row, cols, inner.w, &ctx, depth);
        let sibs = SomIndices::of(&row.children);
        for (i, r) in cells {
            if let Some(k) = row.children.get(i) {
                // Cells stretch to the row height.
                self.draw_node(k, Rect::new(inner.x + r.x, inner.y, r.w, inner.h), &ctx, depth + 1, sibs.at(i))?;
            }
        }
        Ok(())
    }

    fn draw_positioned_child(&mut self, k: &Node, inner: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        let c = k.common();
        if !c.presence.occupies() {
            return Ok(());
        }
        let (w, h) = self.size_of(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1);
        let (x, y) = c.top_left(w, h);
        self.draw_node(k, Rect::new(inner.x + x, inner.y + y, w, h), ctx, depth + 1, sib)
    }

    fn draw_node(&mut self, node: &Node, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let c = node.common();
        if c.presence == Presence::Invisible {
            return Ok(());
        }
        match node {
            Node::Draw(d) => {
                let start = self.item_count();
                self.draw_draw(d, rect, ctx)?;
                // Text and pictures stay with their border; a bare line or rectangle is cut.
                if matches!(d.value, Value::Text(_) | Value::Rich(_) | Value::Image { .. }) {
                    self.keep_whole(start, rect);
                }
                Ok(())
            }
            Node::Field(f) => {
                let start = self.item_count();
                self.draw_field(f, rect, ctx, None, sib)?;
                self.keep_whole(start, rect);
                Ok(())
            }
            Node::ExclGroup(g) => self.draw_group(g, rect, ctx, depth, sib),
            Node::Area(a) => {
                let ctx = ctx.with_som(&a.common, sib);
                let sibs = SomIndices::of(&a.children);
                for (i, k) in a.children.iter().enumerate() {
                    self.draw_positioned_child(k, rect, &ctx, depth + 1, sibs.at(i))?;
                }
                Ok(())
            }
            Node::Subform(s) => {
                // Every drawn instance spends budget, as a flowed one does, so nested repeats that
                // place nothing still stop.
                self.measures.set(self.measures.get().saturating_add(1));
                self.check_measures()?;
                if let Some(b) = &s.common.border {
                    self.draw_border(b, rect)?;
                }
                let parent_ctx = ctx;
                let ctx = ctx.with_som(&s.common, sib);
                let inner = rect.inset(&s.common.margin);
                let sibs = SomIndices::of(&s.children);
                match s.layout {
                    Layout::Positioned => {
                        for (i, k) in s.children.iter().enumerate() {
                            self.draw_positioned_child(k, inner, &ctx, depth + 1, sibs.at(i))?;
                        }
                    }
                    Layout::Tb => {
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                            // A repeating subform here (a table in an area of a positioned page,
                            // say) gets its instances, as in a flowed one, one under another.
                            for instance in 0..self.instances(k, &ctx.som) {
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, sibs.at(i) + instance)?;
                                y += h;
                            }
                        }
                    }
                    Layout::LrTb | Layout::RlTb => {
                        // A repeating subform here (rows of fields under a heading) gets its
                        // instances, as in the other layouts.
                        let (placed, _) = self.pack_instances(&s.children, inner.w, &ctx, depth, &ctx.som);
                        for (i, instance, r) in placed {
                            if let Some(k) = s.children.get(i) {
                                let x = if s.layout == Layout::RlTb { inner.right() - r.x - r.w } else { inner.x + r.x };
                                self.draw_node(k, Rect::new(x, inner.y + r.y, r.w, r.h), &ctx, depth + 1, sibs.at(i) + instance)?;
                            }
                        }
                    }
                    Layout::Table => {
                        let cols = self.columns(s, inner.w);
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let csib = sibs.at(i);
                            if let Node::Subform(r) = k
                                && r.layout == Layout::Row
                            {
                                let h = self.row_height(r, &cols, inner.w, &ctx, depth + 1);
                                self.draw_row(r, &cols, Rect::new(inner.x, y, inner.w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            } else {
                                let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            }
                        }
                    }
                    Layout::Row => {
                        let cols = self.columns(s, inner.w);
                        self.draw_row(s, &cols, rect, parent_ctx, depth, sib)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Draw border `b` around `rect`: its fill, then each shown edge as a line inside the box, or
    /// the whole box as one rounded shape when its corners are rounded.
    fn draw_border(&mut self, b: &Border, rect: Rect) -> Result<(), XfaError> {
        let vis = b.visible_edges();
        // Rounded corners: the box is one shape, filled and edged along its curves, when its
        // edges are all alike (all shown and the same, or all hidden). Edges that differ keep
        // square corners.
        let radii = b.radii(rect.w, rect.h);
        let alike = b.edges.iter().all(|e| *e == b.edges[0]);
        if radii.iter().any(|r| *r > 0.0) && (vis == [true; 4] && alike || vis == [false; 4]) {
            let e = b.edges[0];
            let edge = vis[0].then(|| {
                let color = if e.stroke == Stroke::Relief { [0.55, 0.55, 0.55] } else { e.color.0 };
                (e.thickness.clamp(0.1, 20.0), color, matches!(e.stroke, Stroke::Dashed | Stroke::Dotted))
            });
            let shape = Rounded {
                rect,
                radii,
                inverted: b.corners.map(|c| c.inverted),
                fill: b.fill().map(|f| f.0),
                edge,
                open_top: false,
                open_bottom: false,
            };
            return self.push(Item::Rounded(Box::new(shape)));
        }
        if let Some(fill) = b.fill() {
            self.push(Item::Fill { rect, color: fill.0 })?;
        }
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
        // Top, right, bottom, left; lines sit inside the box.
        let segs = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x0, y1), (x1, y1)), ((x0, y0), (x0, y1))];
        for (i, seg) in segs.iter().enumerate() {
            if !vis[i] {
                continue;
            }
            let e = b.edges[i];
            let t = e.thickness.clamp(0.1, 20.0);
            let (dx, dy) = match i {
                0 => (0.0, t / 2.0),
                1 => (-t / 2.0, 0.0),
                2 => (0.0, -t / 2.0),
                _ => (t / 2.0, 0.0),
            };
            let color = if e.stroke == Stroke::Relief { [0.55, 0.55, 0.55] } else { e.color.0 };
            self.push(Item::Line {
                from: (seg.0.0 + dx, seg.0.1 + dy),
                to: (seg.1.0 + dx, seg.1.1 + dy),
                width: t,
                color,
                dashed: matches!(e.stroke, Stroke::Dashed | Stroke::Dotted),
            })?;
        }
        Ok(())
    }

    fn draw_value_text(&mut self, v: &Value, rect: Rect, font: &Font, para: &Para) -> Result<(), XfaError> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return Ok(()),
        };
        let face = Face::of(font);
        // Text that doesn't fit the height it was given (the template's face is narrower than
        // the standard one drawn in its place, so it wraps to more lines) is drawn smaller,
        // down to 70%, rather than over what lies below.
        let fits = |k: f64| text::measure_height(&rich, rect.w / k, para, &face, &|id, max| self.embed(id, max)) <= rect.h / k + 1.0;
        let scale = if rect.h < 4.0 || fits(1.0) { 1.0 } else { [0.95, 0.9, 0.85, 0.8, 0.75].into_iter().find(|k| fits(*k)).unwrap_or(0.7) };
        let mut block = text::layout(&rich, Rect::new(rect.x, rect.y, rect.w / scale, rect.h / scale), para, &face, &|id, max| self.embed(id, max));
        if scale < 1.0 {
            for s in &mut block.spans {
                s.x = rect.x + (s.x - rect.x) * scale;
                s.baseline = rect.y + (s.baseline - rect.y) * scale;
                s.face.size *= scale;
            }
        }
        self.push_block(block)
    }

    fn draw_draw(&mut self, d: &Draw, rect: Rect, ctx: &Ctx) -> Result<(), XfaError> {
        if let Some(b) = &d.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.child(&d.common);
        let inner = rect.inset(&d.common.margin);
        match &d.value {
            Value::Text(_) | Value::Rich(_) => self.draw_value_text(&d.value, inner, &ctx.font, &ctx.para),
            Value::Image { content_type, data } => self.draw_image(data, content_type, inner),
            Value::Line { slope_up, edge } => {
                if !edge.visible() {
                    return Ok(());
                }
                let (from, to) = if *slope_up {
                    ((inner.x, inner.bottom()), (inner.right(), inner.y))
                } else {
                    ((inner.x, inner.y), (inner.right(), inner.bottom()))
                };
                self.push(Item::Line { from, to, width: edge.thickness.clamp(0.1, 20.0), color: edge.color.0, dashed: edge.stroke == Stroke::Dashed })
            }
            Value::Rectangle(b) => self.draw_border(b, inner),
            Value::Empty => Ok(()),
        }
    }

    fn draw_group(&mut self, g: &ExclGroup, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if let Some(b) = &g.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&g.common, sib);
        // The group's data node holds the selected button's on value.
        let selected: Option<String> =
            self.data.and_then(|d| d.text_at(&data_path(self.tpl, &ctx.som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let inner = rect.inset(&g.common.margin);
        let group = self.unique_name(g.common.name.as_deref().unwrap_or("group"));
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        let placed: Vec<(usize, Rect)> = match g.layout {
            Layout::Tb => {
                let mut y = 0.0;
                let mut out = Vec::new();
                for (i, k) in nodes.iter().enumerate() {
                    if !k.common().presence.occupies() {
                        continue;
                    }
                    let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                    out.push((i, Rect::new(0.0, y, w, h)));
                    y += h;
                }
                out
            }
            Layout::Positioned => nodes
                .iter()
                .enumerate()
                .filter(|(_, k)| k.common().presence.occupies())
                .map(|(i, k)| {
                    let c = k.common();
                    let (w, h) = self.size_of(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), &ctx, depth + 1);
                    let (x, y) = c.top_left(w, h);
                    (i, Rect::new(x, y, w, h))
                })
                .collect(),
            _ => self.pack(&nodes, inner.w, &ctx, depth).0,
        };
        let sibs = SomIndices::of(&nodes);
        for (i, r) in placed {
            if let Some(f) = g.fields.get(i) {
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| (i + 1).to_string());
                let tip = f.tooltip.clone().or_else(|| g.tooltip.clone());
                let chosen = selected.as_deref() == Some(on.as_str());
                self.draw_field(f, Rect::new(inner.x + r.x, inner.y + r.y, r.w, r.h), &ctx, Some((&group, &on, tip, chosen)), sibs.at(i))?;
            }
        }
        Ok(())
    }

    /// A picture scaled to fit `inner`, keeping its shape, from its top left.
    fn draw_image(&mut self, data: &[u8], content_type: &str, inner: Rect) -> Result<(), XfaError> {
        self.image_bytes = self.image_bytes.saturating_add(data.len());
        if self.image_bytes > MAX_IMAGE_BYTES {
            return Err(XfaError::TooLarge("the form's pictures are too large together to lay out".into()));
        }
        let fitted = match crate::image::size(data) {
            Some((iw, ih)) if iw > 0 && ih > 0 && inner.w > 0.0 && inner.h > 0.0 => {
                let k = (inner.w / iw as f64).min(inner.h / ih as f64);
                Rect::new(inner.x, inner.y, iw as f64 * k, ih as f64 * k)
            }
            _ => inner,
        };
        self.push(Item::Image { rect: fitted, data: Arc::new(data.to_vec()), content_type: content_type.to_string() })
    }

    /// A field name made from `base` that no earlier field of the form has: the first keeps it,
    /// later ones get `_2`, `_3`, ….
    fn unique_name(&mut self, base: &str) -> String {
        let base = clean_name(base);
        let n = self.names.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}_{n}") }
    }

    /// Draw field `f` at `rect`: its border (a button's is its widget's box), its caption, then its
    /// widget, or its value as plain text when it is non-interactive, a barcode, or a text field
    /// taller than its content area. `radio` is set for a group's radio button; `sib` is its SOM index.
    fn draw_field(
        &mut self,
        f: &Field,
        rect: Rect,
        ctx: &Ctx,
        radio: Option<(&str, &str, Option<String>, bool)>,
        sib: usize,
    ) -> Result<(), XfaError> {
        if let Some(b) = &f.common.border
            && f.ui != Ui::Button
        {
            self.draw_border(b, rect)?;
        }
        let fctx = ctx.child(&f.common);
        let inner = rect.inset(&f.common.margin);
        let (mut ui, mut cap_rect) = (inner, None);
        if let Some((placement, reserve)) = self.caption_reserve(f, inner.w, ctx) {
            let reserve = auto_check_reserve(f, placement, inner.w, Some(inner.h)).unwrap_or(reserve);
            let r = reserve.max(0.0);
            match placement {
                Placement::Top => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y + r, inner.w, inner.h - r);
                }
                Placement::Bottom => {
                    cap_rect = Some(Rect::new(inner.x, inner.bottom() - r.min(inner.h), inner.w, r.min(inner.h)));
                    ui = Rect::new(inner.x, inner.y, inner.w, inner.h - r);
                }
                Placement::Right => {
                    cap_rect = Some(Rect::new(inner.right() - r.min(inner.w), inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x, inner.y, inner.w - r, inner.h);
                }
                _ => {
                    cap_rect = Some(Rect::new(inner.x, inner.y, r.min(inner.w), inner.h));
                    ui = Rect::new(inner.x + r, inner.y, inner.w - r, inner.h);
                }
            }
        }
        if let (Some(cr), Some(cap)) = (cap_rect, &f.caption)
            && cap.presence == Presence::Visible
        {
            let font = ctx.font.apply(&cap.font);
            let para = ctx.para.apply(&cap.para);
            self.draw_value_text(&cap.value, cr, &font, &para)?;
        }
        let som_path = || {
            if fctx.som.is_empty() {
                format!("{}[{sib}]", f.common.name.as_deref().unwrap_or("field"))
            } else {
                format!("{}.{}[{sib}]", fctx.som, f.common.name.as_deref().unwrap_or("field"))
            }
        };
        match f.ui {
            Ui::Unknown => {
                self.warn_once("a field of an unknown kind is left blank");
                return Ok(());
            }
            Ui::ImageEdit => {
                // The picture the data holds (base64, as Designer saves it) or the template's.
                let field_name = f.common.name.clone().unwrap_or_else(|| "an image field".into());
                let text: Vec<u8> = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(|t| t.bytes().filter(|b| !b.is_ascii_whitespace()).collect())
                    .unwrap_or_default();
                let from_data = if text.is_empty() {
                    None
                } else if text.len() > MAX_DATA_IMAGE / 3 * 4 + 4 {
                    self.warn_once(format!("{field_name}: the picture in the data is larger than 32 MB and is left out"));
                    None
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(&text) {
                        Ok(d) if crate::image::kind(&d).is_some() => Some(d),
                        Ok(_) => {
                            self.warn_once(format!("{field_name}: the picture in the data is not a JPEG, PNG, GIF, BMP or TIFF and is left out"));
                            None
                        }
                        Err(_) => {
                            self.warn_once(format!("{field_name}: the data holds no readable picture (not base64) and is left out"));
                            None
                        }
                    }
                };
                let box_ = ui.inset(&f.ui_margin);
                match (&from_data, &f.value) {
                    (Some(d), _) => self.draw_image(d, "", box_)?,
                    (None, Value::Image { content_type, data }) => self.draw_image(data, content_type, box_)?,
                    _ => {}
                }
                // Then the field itself, which clicking gives a new picture.
            }
            Ui::Barcode => {
                // The value as text, where the bars would be.
                self.warn_once("barcode fields show their value as text, not as bars");
                let value = self
                    .data
                    .and_then(|d| d.text_at(&data_path(self.tpl, &som_path())))
                    .map(str::to_string)
                    .or_else(|| f.value.plain())
                    .unwrap_or_default();
                if !value.trim().is_empty() {
                    let mut para = fctx.para.clone();
                    para.h_align = HAlign::Center;
                    self.draw_value_text(&Value::Text(value), ui.inset(&f.ui_margin), &fctx.font, &para)?;
                }
                return Ok(());
            }
            _ => {}
        }
        if f.access == Access::NonInteractive && f.ui != Ui::Button {
            // Shown as text, not as a field.
            return self.draw_value_text(&f.value, ui.inset(&f.ui_margin), &fctx.font, &fctx.para);
        }
        // A text field that grew past what a content area holds can't be one widget: its text
        // is drawn as lines the page breaks fall between, and stays what the data holds.
        if f.grows_with_text() && rect.h > self.cursor().0.h + 0.01 {
            let value =
                self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som_path()))).filter(|t| !t.trim().is_empty()).or_else(|| f.value.plain());
            if let Some(value) = value.filter(|t| !t.trim().is_empty()) {
                let name = f.common.name.clone().unwrap_or_else(|| "a text field".into());
                self.warn_once(format!("{name}: its text is longer than a page; it is shown across pages and can't be edited here"));
                let m = Margin {
                    top: f.ui_margin.top + 2.0,
                    bottom: f.ui_margin.bottom + 2.0,
                    left: f.ui_margin.left + 2.0,
                    right: f.ui_margin.right + 2.0,
                };
                let para = Para { v_align: VAlign::Top, ..fctx.para.clone() };
                // In the face it was measured in, so it takes the lines it was given.
                let face = Self::field_text_face(f, &fctx.font);
                let block = text::layout(&text::plain(value.trim_end()), ui.inset(&m), &para, &face, &|_, _| String::new());
                return self.push_block(block);
            }
        }
        let face = match (&f.ui, &f.caption) {
            // Buttons show their caption in the caption's font.
            (Ui::Button, Some(cap)) => Face::of(&ctx.font.apply(&cap.font)).unscaled(),
            _ => Face::of(&fctx.font).unscaled(),
        };
        let h_align = match (&f.ui, &f.caption) {
            (Ui::Button, Some(cap)) => ctx.para.apply(&cap.para).h_align,
            _ => fctx.para.h_align,
        };
        let name = self.unique_name(f.common.name.as_deref().unwrap_or("field"));
        let som = som_path();
        let read_only = matches!(f.access, Access::ReadOnly | Access::Protected);
        // The data's value wins over the template's default.
        let data_value: Option<String> =
            self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        // A choice list's default is a saved value too.
        let template_default = f.value.plain().map(|v| f.saved_value(v.trim())).filter(|v| !v.is_empty());
        let plain_value = data_value.clone().or_else(|| template_default.clone());
        let (kind, rect, border, value, tooltip) = match f.ui {
            Ui::CheckButton => {
                let s = f.check_size.unwrap_or(10.0).min(ui.w.max(1.0)).min(ui.h.max(1.0));
                // Without a caption the box sits at the right of the content area, as if a
                // caption on its left took the rest, so the paragraph's alignment has no room to
                // act on; beside a caption that alignment places it. The box's own margin doesn't
                // move it.
                let x = match (cap_rect, fctx.para.h_align) {
                    (None, _) | (Some(_), HAlign::Right) => ui.right() - s,
                    (Some(_), HAlign::Center) => ui.x + (ui.w - s) / 2.0,
                    (Some(_), HAlign::Left | HAlign::Justify) => ui.x,
                };
                let y = match fctx.para.v_align {
                    VAlign::Top => ui.y,
                    VAlign::Middle => ui.y + (ui.h - s) / 2.0,
                    VAlign::Bottom => ui.bottom() - s,
                };
                let r = Rect::new(x, y, s, s);
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "1".into());
                let (kind, tip, checked) = match radio {
                    Some((group, on_value, tip, chosen)) => (
                        WidgetKind::Radio { group: group.to_string(), on: on_value.to_string(), round: f.check_round },
                        tip.or_else(|| f.tooltip.clone()),
                        chosen,
                    ),
                    None => (
                        WidgetKind::CheckBox { on: on.clone(), round: f.check_round },
                        f.tooltip.clone(),
                        plain_value.as_deref().is_some_and(|v| is_on(v, &on)),
                    ),
                };
                let border = widget_border(f.ui_border.as_ref(), true);
                (kind, r, border, checked.then(|| on.clone()), tip)
            }
            Ui::Button => {
                let caption = f.caption.as_ref().and_then(|c| c.value.plain()).unwrap_or_default().trim().to_string();
                // A button's box is its field border, around the whole field (its margins pad
                // the caption, not the box); without one it is invisible (a link over text, say).
                let border = match &f.common.border {
                    Some(b) => widget_border(Some(b), false),
                    None => WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 },
                };
                (WidgetKind::Button { caption }, rect, border, None, f.tooltip.clone())
            }
            Ui::DateTimeEdit => {
                let pattern = date_pattern(f.picture.as_deref());
                // The data holds the date in the field's data picture, or else as an ISO date; the
                // field shows it in its own pattern.
                let data_pattern = data_date_letters(f);
                let shown = plain_value
                    .as_ref()
                    .map(|v| if data_value.is_some() { iso_to_pattern(&data_to_iso(v, data_pattern.as_deref()), &pattern) } else { v.clone() });
                (WidgetKind::Date(pattern), ui, widget_border(f.ui_border.as_ref(), false), shown, f.tooltip.clone())
            }
            Ui::ChoiceList => {
                let options: Vec<(String, String)> = f
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, shown)| (f.item_values.get(i).cloned().unwrap_or_else(|| shown.clone()), shown.clone()))
                    .collect();
                // The data holds saved values (several on separate lines); shown text is taken
                // too, as a user typing into an editable list leaves it.
                let value = plain_value.as_ref().map(|v| f.saved_value(v));
                let kind = WidgetKind::Choice { options, list_box: f.choice.list_box, multi: f.choice.multi, editable: f.choice.editable };
                (kind, ui, widget_border(f.ui_border.as_ref(), false), value, f.tooltip.clone())
            }
            Ui::Signature => (WidgetKind::Signature, ui, widget_border(f.ui_border.as_ref(), false), None, f.tooltip.clone()),
            Ui::ImageEdit => {
                let none = WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 };
                (WidgetKind::Image, ui, none, None, f.tooltip.clone())
            }
            Ui::PasswordEdit => (WidgetKind::Password, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
            _ => (WidgetKind::Text, ui, widget_border(f.ui_border.as_ref(), false), plain_value, f.tooltip.clone()),
        };
        if rect.w < 1.0 || rect.h < 1.0 {
            return Ok(());
        }
        let action = if f.ui == Ui::Button { button_action(&f.scripts) } else { None };
        let items: Vec<String> = match &kind {
            WidgetKind::CheckBox { on, .. } => vec![on.clone(), f.items.get(1).cloned().unwrap_or_default()],
            WidgetKind::Radio { on, .. } => vec![on.clone()],
            _ => Vec::new(),
        };
        self.fields += 1;
        self.push(Item::Widget(Box::new(Widget {
            rect,
            kind,
            name,
            som,
            tooltip,
            face,
            h_align,
            multiline: f.multiline,
            max_chars: f.max_chars,
            read_only,
            border,
            value,
            default: template_default,
            action,
            items,
            no_scroll: f.no_scroll,
        })))
    }
}

/// The room a caption without a reserve takes beside a check box: all the box leaves of a
/// content area `w` wide (and `h` tall, when known), the box at the size it can be drawn there.
/// `None` for any other field, or a caption with a reserve of its own.
fn auto_check_reserve(f: &Field, placement: Placement, w: f64, h: Option<f64>) -> Option<f64> {
    if f.ui != Ui::CheckButton || f.caption.as_ref().is_none_or(|c| c.reserve.is_some()) {
        return None;
    }
    let s = f.check_size.unwrap_or(10.0).min(w.max(1.0)).min(h.map_or(f64::INFINITY, |h| h.max(1.0)));
    let rest = match (placement, h) {
        (Placement::Top | Placement::Bottom, Some(h)) => h - s,
        (Placement::Top | Placement::Bottom, None) => return None,
        _ => w - s,
    };
    Some(rest.max(0.0))
}

/// The widget's own border from the `<ui>` border element. Without one, a check box gets a
/// solid box, as Designer draws it, and any other field has no box of its own.
fn widget_border(b: Option<&Border>, check: bool) -> WidgetBorder {
    let Some(b) = b else {
        if !check {
            return WidgetBorder { color: None, fill: None, shape: BorderShape::None, width: 0.0 };
        }
        return WidgetBorder { color: Some([0.0, 0.0, 0.0]), fill: Some([1.0, 1.0, 1.0]), shape: BorderShape::Full, width: 0.5 };
    };
    let vis = b.visible_edges();
    let shape = match vis {
        [false, false, false, false] => BorderShape::None,
        [false, false, true, false] => BorderShape::Underline,
        _ => BorderShape::Full,
    };
    let edge = (0..4).find(|&i| vis[i]).map(|i| b.edges[i]).unwrap_or_default();
    WidgetBorder {
        color: (shape != BorderShape::None).then_some(edge.color.0),
        fill: b.fill().map(|c| c.0),
        shape,
        width: edge.thickness.clamp(0.1, 12.0),
    }
}
