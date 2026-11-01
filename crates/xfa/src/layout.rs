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

mod common;
mod draw;
mod field;
mod flow;
mod items;
mod measure;
mod pages;

use common::collect_values;
pub(crate) use common::date_pattern;
pub use common::formatted_value;
pub(crate) use items::UNDERLINE_DROP;
pub use items::{Action, BorderShape, Form, Item, Page, Rounded, Widget, WidgetBorder, WidgetKind};

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
