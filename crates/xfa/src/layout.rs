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
use crate::data::{DataNode, DataPath, data_date_letters, data_path, data_to_iso, date_letters, is_on, iso_to_pattern};
use crate::model::{
    Access, Area, Border, Common, Draw, ExclGroup, Field, Font, HAlign, Layout, Margin, Node, Occur, PageArea, PageRole, Para, Placement, Presence,
    Rect, Script, SomIndices, Stroke, Subform, Template, Ui, VAlign, Value, Views,
};
use crate::text::{self, Block, Face, Span};

mod common;
mod draw;
mod field;
mod flow;
mod items;
mod measure;
mod pages;

pub(crate) use common::date_pattern;
pub use common::formatted_value;
use common::{collect_names, collect_values};
pub(crate) use items::UNDERLINE_DROP;
pub use items::{Action, BorderShape, Form, Item, Page, Rounded, Turned, Widget, WidgetBorder, WidgetKind};

/// Most pages a form may lay out to.
pub const MAX_PAGES: usize = 500;
/// Most items (text spans, lines, widgets…) across all pages.
const MAX_ITEMS: usize = 400_000;
/// Deepest nesting followed.
const MAX_DEPTH: usize = 64;
/// Most size measurements of one layout. Measurements of containers are memoized, so a real
/// form stays far below this; it bounds the work a hostile template can cause.
const MAX_MEASURES: usize = 2_000_000;
/// Most characters the draw phase lays out in one layout, each pass counted. A draw re-measures
/// overflowing text up to six times to shrink it, and a repeating subform draws its text once
/// per instance, so large text drawn many times would otherwise cost seconds; a real form's
/// draws stay far below this. Reduced under test so the guard is exercised without laying out
/// hundreds of megabytes of text.
#[cfg(not(test))]
const MAX_DRAW_CHARS: usize = 256 << 20;
#[cfg(test)]
const MAX_DRAW_CHARS: usize = 1 << 20;
/// Most instances of one repeating subform laid out (a row the data adds, or the template's
/// initial count). `occur max="-1"` sets no upper boundary (XFA 3.3, "The occur element",
/// p. 786), so this only bounds the work of a hostile or runaway form: a long list or table
/// is laid out whole up to here, and the rows past it are reported ([`Layouter::warn_truncated`]).
/// It matches the largest `occur` count the parser reads and the most rows one script run may
/// add (`MAX_ADDED` in pdfcraft-js). Laying out and listing the fields grows faster than the
/// row count (one text field a row: 1,000 rows open in about 3 s in a debug build, 2,000 in
/// about 13 s), so a much larger bound would let one list stall the app. The layout budgets
/// ([`MAX_MEASURES`], [`MAX_ITEMS`], [`MAX_PAGES`]) still bound the form as a whole, however
/// its repeats nest.
pub(crate) const MAX_INSTANCES: usize = 1_000;
/// Largest picture taken from the data (decoded bytes), as for template pictures.
const MAX_DATA_IMAGE: usize = 32 << 20;
/// Most picture bytes one layout keeps, all copies together. Each laid-out picture holds its
/// own copy, so a large picture in repeating subforms would otherwise multiply into gigabytes
/// (a failed allocation aborts the app); a real form's logos and photos stay far below.
const MAX_IMAGE_BYTES: usize = 256 << 20;
/// Placeholder for the page count in text laid out before the count is known.
const PAGE_COUNT_MARK: char = '\u{E000}';

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

/// (address of a subform or area in the template, available width bits, depth, SOM path of the
/// instance measured when data drives the layout).
type MeasureKey = (usize, u64, usize, String);

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
    /// A piece of a field being split sits in the part on this content area, whether or not it
    /// drew anything there (a widget with no border or fill draws nothing below the break): the
    /// part counts as holding something, as one that got items does.
    held: bool,
}

struct Cursor {
    area: usize,
    content: usize,
    rect: Rect,
    y: f64,
    /// Something from the flow was placed in this content area.
    placed: bool,
}

/// An empty field a page break is splitting ([`Layouter::splitting`]): its widget's SOM path, its
/// box where it was drawn, and its pieces so far (page index, the page's first item of the piece,
/// the piece's box).
struct Splitting {
    som: String,
    rect: Rect,
    pieces: Vec<(usize, usize, Rect)>,
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
    /// Every PDF field name handed out so far.
    taken: HashSet<String>,
    /// The names the template's fields, groups and groups' members take as written
    /// ([`common::collect_names`]), which a made-up `_2`, `_3` name must not take.
    template_names: HashSet<String>,
    /// Field values by `id`, for embedded fields in rich text.
    values: HashMap<String, common::EmbedSource>,
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
    /// Characters the draw phase has laid out, each pass counted (see [`MAX_DRAW_CHARS`]).
    drawn_chars: Cell<usize>,
    /// The fields and draws of the block being placed: (first item, one past the last, top,
    /// bottom) on the last page. A page break keeps each whole (see `split_block`).
    leaves: Vec<(usize, usize, f64, f64)>,
    /// The empty field a page break is splitting (see `flow_block` and `split_block`).
    splitting: Option<Splitting>,
    /// The items `draw_field` drew for the border of the field drawn last: (first, one past the last).
    field_border: Option<(usize, usize)>,
    /// Where what is being drawn shows, as the objects it is inside say ([`Common::views`]).
    views: Views,
    /// The flowed subforms being laid out, outermost first: a break between their children
    /// closes their parts on one content area and opens new ones on the next.
    frames: Vec<Frame>,
    /// What scripts set on page-master content, applied to each page's copy by its own SOM path.
    overrides: Option<&'t crate::script::Overrides>,
    /// The pages' copies of master content that overrides changed, kept for the whole layout so
    /// their addresses stay unique (the measurement cache keys on them).
    master_copies: Vec<Rc<Vec<Node>>>,
}

/// How many instances of a repeating subform to lay out: what the data holds once it holds
/// any (rows added or removed), else the template's initial count; never below one, never
/// above the maximum.
pub fn instance_count(occur: &Occur, from_data: usize) -> usize {
    let n = if from_data > 0 { from_data } else { occur.initial };
    n.min(occur.max.unwrap_or(usize::MAX)).clamp(1, MAX_INSTANCES)
}

/// How many instances of `sf` the layout makes under the container at `parent_som`: its `occur`
/// and the instances `data` holds ([`instance_count`]). The traversal counts them the same way.
pub(crate) fn subform_instances(tpl: &Template, data: Option<&DataNode>, sf: &Subform, parent_som: &str) -> usize {
    let from_data = match (&sf.common.name, data) {
        (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(tpl, parent_som), name),
        _ => 0,
    };
    instance_count(&sf.occur, from_data)
}

/// Lay a template out, with the values and repeat counts of `data` (the `xfa:data` element).
pub fn layout(tpl: &Template, data: Option<&DataNode>) -> Result<Form, XfaError> {
    layout_inner(tpl, data, None)
}

/// [`layout`], with what scripts set (`ov`) applied to page-master content page by page: a
/// field hidden on the second page made from a page area is hidden there only. The rest of the
/// template takes `ov` before this ([`crate::script::apply_overrides`]).
pub fn layout_with_overrides(tpl: &Template, data: Option<&DataNode>, ov: &crate::script::Overrides) -> Result<Form, XfaError> {
    layout_inner(tpl, data, Some(ov))
}

fn layout_inner(tpl: &Template, data: Option<&DataNode>, overrides: Option<&crate::script::Overrides>) -> Result<Form, XfaError> {
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
    let values = collect_values(tpl, data);
    // One copy of the root for the whole layout: measurements are cached by node address.
    let root_node = Node::Subform(Box::new(tpl.root.clone()));
    let mut template_names = HashSet::new();
    collect_names(std::slice::from_ref(&root_node), &mut template_names, 0);
    let mut l = Layouter {
        tpl,
        data,
        uses: vec![0; areas.len()],
        areas: Rc::new(areas),
        pages: Vec::new(),
        cur: None,
        names: HashMap::new(),
        taken: HashSet::new(),
        template_names,
        values,
        warnings: Vec::new(),
        warned: HashSet::new(),
        fields: 0,
        items: 0,
        image_bytes: 0,
        measured: RefCell::new(HashMap::new()),
        measures: Cell::new(0),
        drawn_chars: Cell::new(0),
        leaves: Vec::new(),
        splitting: None,
        field_border: None,
        views: Views::ALL,
        frames: Vec::new(),
        overrides,
        master_copies: Vec::new(),
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
        fill_page_count(&mut p.items, &count);
    }
    // Tab goes the way the template's traversal says.
    crate::traverse::order_widgets(&root_node, tpl, data, &mut l.pages);
    #[cfg(test)]
    LAID_OUT_ITEMS.with(|c| c.set(l.items));
    Ok(Form { pages: l.pages, fields: l.fields, warnings: l.warnings })
}

/// Puts the page count, known once every page is, where `items` left room for it.
fn fill_page_count(items: &mut [Item], count: &str) {
    for it in items {
        match it {
            Item::Text(s) if s.text.contains(PAGE_COUNT_MARK) => s.text = s.text.replace(PAGE_COUNT_MARK, count),
            Item::Turned(t) => fill_page_count(&mut t.items, count),
            Item::Only { item, .. } => fill_page_count(std::slice::from_mut(item.as_mut()), count),
            _ => {}
        }
    }
}

// Bytes of embedded field values copied out on this thread, for tests that bound layout work.
#[cfg(test)]
thread_local! {
    pub(crate) static EMBED_COPIED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// The item count the last layout on this thread ended with, against `MAX_ITEMS`.
#[cfg(test)]
thread_local! {
    pub(crate) static LAID_OUT_ITEMS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
