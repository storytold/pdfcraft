//! Add Header and Footer, Add Watermark and Add Background dialogs (Acrobat: Edit a PDF ▸ …;
//! dialog snapshots d13, d14, d12d). Each is a hosted parameter dialog with the shared Page Range
//! Options and a preview of a page the range selects, with the marks laid out on it. "Update"
//! opens the same dialog in replace mode.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use craft::behavior::CanvasTransform;
use craft::host::queue::{JobQueue, Priority, QueueOptions, QueueOrder};
use craft::pages::PageRange;
use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::ui::contract::Show;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::display::Text;
use craft::ui::patterns::form_dialogs::{
    DialogFields, FieldValue, HostedParamDialog, PagerStep, PagerUnit, PanePager, Param, ParamDialog, ParamKind, ParamResult, PreviewState,
    SharedOverlay,
};
use craft::ui::primitives::ContentPainterCx;
use craft::ui::primitives::textures::{TextureState, ThumbRequest, ThumbSource, ThumbnailOptions, Thumbnails};
use craft::ui::runtime::env::{Label, UnitKind};
use craft::ui::runtime::requests::PathKind;
use craft::ui::style::DataColor;
use craft::ui::text::TextField;
use egui::{Color32, CornerRadius, Rect, pos2, vec2};
use pdfcraft_engine::{Backdrop, Background, DocId, Edit, FontFamily, HeaderFooter, MarkKind, Watermark};

use crate::PdfCraftApp;

/// How far a watermark may be moved from the page centre, in points.
const MAX_OFFSET: f64 = 20_000.0;

/// The preview page's longer side, in points.
const PREVIEW_SIDE: f32 = 190.0;

/// How many pages a job checks for a backdrop of their own, so a long document is checked in
/// steps and never stalls the dialog.
const BACKDROP_PAGES_PER_FRAME: usize = 24;

/// Which pages paint their own backdrop (it hides a background), for the document and edit
/// generation they were checked in. Filled a batch at a time by the backdrop job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackdropCheck {
    key: Option<(DocId, u64)>,
    /// Per page: `None` until checked.
    pages: Vec<Option<Option<Backdrop>>>,
}

/// What the check found among the pages a background goes on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackdropCount {
    /// Pages (0-based, ascending) with a white backdrop that can be replaced.
    pub fills: Vec<usize>,
    /// Pages (0-based, ascending) covered by a picture (scans): the background stays hidden there.
    pub images: Vec<usize>,
    /// Pages not checked yet.
    pub pending: usize,
}

impl BackdropCheck {
    /// Records the `finished` batches (pages, what each paints) of the document version `key` with
    /// `count` pages, sorts `pages` (0-based) and returns the next batch still to check: at most
    /// [`BACKDROP_PAGES_PER_FRAME`] pages, none when everything is checked.
    pub fn update(
        &mut self,
        key: (DocId, u64),
        count: usize,
        pages: &[usize],
        finished: Vec<(Vec<usize>, Vec<Option<Backdrop>>)>,
    ) -> (BackdropCount, Vec<usize>) {
        if self.key != Some(key) || self.pages.len() != count {
            self.key = Some(key);
            self.pages = vec![None; count];
        }
        for (batch, found) in finished {
            for (p, found) in batch.iter().zip(found) {
                if let Some(slot) = self.pages.get_mut(*p) {
                    *slot = Some(found);
                }
            }
        }
        let mut next = Vec::with_capacity(BACKDROP_PAGES_PER_FRAME);
        for &p in pages {
            if next.len() >= BACKDROP_PAGES_PER_FRAME {
                break;
            }
            if matches!(self.pages.get(p), Some(None)) && !next.contains(&p) {
                next.push(p);
            }
        }
        let mut n = BackdropCount::default();
        for &p in pages {
            let Some(slot) = self.pages.get(p) else { continue };
            match slot {
                None => n.pending += 1,
                Some(Some(Backdrop::Fill)) => n.fills.push(p),
                Some(Some(Backdrop::Image)) => n.images.push(p),
                Some(None) => {}
            }
        }
        for list in [&mut n.fills, &mut n.images] {
            list.sort_unstable();
            list.dedup();
        }
        (n, next)
    }

    /// What the check found on `page` (0-based) of this version of `doc`: `None` until it has
    /// looked, `Some(None)` for a page with no backdrop of its own.
    pub fn found(&self, doc: &pdfcraft_engine::Document, page: usize) -> Option<Option<Backdrop>> {
        if self.key != Some((doc.id, doc.edit_generation())) {
            return None;
        }
        self.pages.get(page).copied().flatten()
    }
}

impl MarksDraft {
    /// The dialog for `kind` is opening. Other settings carry over from last time, but replacing
    /// a page's own backdrop starts on: Update restores backdrops before it re-applies, so a
    /// page whose backdrop was replaced shows none and the checkbox stays hidden, and a choice
    /// left off from an earlier dialog would hide the background again unseen.
    pub fn opened(&mut self, kind: MarkKind) {
        if kind == MarkKind::Background {
            self.bg.replace_backdrop = true;
        }
    }
}

/// How many runs of pages a message lists before it stops with "…".
const LISTED_PAGE_RUNS: usize = 8;

/// After Remove background: the pages (0-based, ascending) whose own white backdrop stays off,
/// because they changed after the background was added (`pdfcraft_engine::EditNotice`).
pub(crate) fn backdrops_left_off_message(pages: &[usize]) -> String {
    pages_phrase(
        pages,
        tl!("Page {pages} changed after the background was added, so its own white backdrop stays off. It looks the same on white paper."),
        tl!("Pages {pages} changed after the background was added, so their own white backdrops stay off. They look the same on white paper."),
    )
}

/// `one` or `many` with `{pages}` replaced by `pages` (0-based, ascending) as 1-based runs
/// ("1-3, 6"), so the message says which pages it means.
fn pages_phrase(pages: &[usize], one: &str, many: &str) -> String {
    // `count` 0 never matches a list that isn't empty, so the list is always written out.
    let runs = crate::combine_ui::range_of(pages, 0);
    let mut parts: Vec<&str> = runs.split(", ").take(LISTED_PAGE_RUNS + 1).collect();
    if parts.len() > LISTED_PAGE_RUNS {
        parts.truncate(LISTED_PAGE_RUNS);
        parts.push("…");
    }
    let template = if pages.len() == 1 { one } else { many };
    crate::i18n::fmt(template, &[("pages", &parts.join(", "))])
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarksDraft {
    pub hf: HeaderFooter,
    pub wm: Watermark,
    pub bg: Background,
    pub range: PageRange,
    /// Replace existing marks of this kind (Update).
    pub replace: bool,
    /// Which header/footer box Insert Page Number / Insert Date type into.
    pub focused_box: usize,
    pub page_format: usize,
    pub date_format: usize,
    /// Watermark: size the text to the page.
    pub fit: bool,
    /// Source ▸ File (watermark or background): the file picked, and the page of a PDF (1-based).
    pub use_file: bool,
    pub file: Option<(String, std::sync::Arc<Vec<u8>>)>,
    pub file_page: usize,
    /// Background: pages that paint their own backdrop.
    pub backdrops: BackdropCheck,
    /// The page (0-based) the preview was stepped to; `None` previews the current page. Reset
    /// each time the dialog opens.
    pub preview: Option<usize>,
}

impl Default for MarksDraft {
    fn default() -> Self {
        Self {
            hf: HeaderFooter::default(),
            wm: Watermark::default(),
            bg: Background::default(),
            range: PageRange::default(),
            replace: false,
            focused_box: 1,
            page_format: 0,
            date_format: 0,
            fit: true,
            use_file: false,
            file: None,
            file_page: 1,
            backdrops: BackdropCheck::default(),
            preview: None,
        }
    }
}

const PAGE_FORMATS: [&str; 5] = ["<<1>>", "<<1 of n>>", "<<1/n>>", "<<Page 1>>", "<<Page 1 of n>>"];
// New formats go last: `date_format` is an index into this list.
const DATE_FORMATS: [&str; 11] = [
    "<<m/d/yyyy>>",
    "<<mm/dd/yyyy>>",
    "<<d/m/yyyy>>",
    "<<yyyy-mm-dd>>",
    "<<m/d/yy>>",
    "<<mmmm d, yyyy>>",
    "<<yyyy/mm/dd>>",
    "<<yyyy/m/d>>",
    "<<dd.mm.yyyy>>",
    "<<d.m.yyyy>>",
    "<<dd.mm.yy>>",
];
const BOX_NAMES: [&str; 6] =
    ["Left Header Text", "Center Header Text", "Right Header Text", "Left Footer Text", "Center Footer Text", "Right Footer Text"];

/// The key of the dialog for `kind`, which the host pass finds it by.
fn dialog_key(kind: MarkKind) -> &'static str {
    match kind {
        MarkKind::HeaderFooter => "marks.header_footer",
        MarkKind::Watermark => "marks.watermark",
        MarkKind::Background => "marks.background",
        MarkKind::PrinterMarks => "marks.printer_marks",
    }
}

fn rgb32(c: [f64; 3]) -> Color32 {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// The tint of a page over a background of `color` at `opacity`: the colour shows where the page
/// is white and the content stays on top. A non-finite opacity is opaque, as the engine has it.
fn behind(color: [f64; 3], opacity: f64) -> Color32 {
    let a = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    rgb32(color.map(|c| 1.0 - a * (1.0 - c)))
}

/// The preview's tint for a background colour on a page whose own backdrop is `found` (`None`
/// when it has none, or the check hasn't reached it): no colour where the page will still hide
/// the background after OK, because a picture covers it (a scan) or it keeps its white backdrop
/// with Replace unticked.
fn background_tint(d: &MarksDraft, found: Option<Backdrop>) -> Color32 {
    let hidden = match found {
        Some(Backdrop::Image) => true,
        Some(Backdrop::Fill) => !d.bg.replace_backdrop,
        None => false,
    };
    if hidden { Color32::WHITE } else { behind(d.bg.color, d.bg.opacity) }
}

/// The page (0-based) the preview shows: the current one when the range selects it, otherwise the
/// first page the range selects (the current one again when it selects none).
fn preview_page(range: &PageRange, current: usize, count: usize, selection: &[usize]) -> usize {
    let pages = range.pages(count, current, selection);
    if pages.contains(&current) { current } else { pages.first().copied().unwrap_or(current) }
}

/// [`preview_page`] for the page stepped to (`stepped`), or the current page if none. A page
/// stepped to that the range leaves out is replaced by the page shown, so widening the range again
/// keeps that page rather than jumping back.
fn shown_preview(range: &PageRange, stepped: &mut Option<usize>, current: usize, count: usize, selection: &[usize]) -> usize {
    let shown = preview_page(range, stepped.unwrap_or(current), count, selection);
    if stepped.is_some() {
        *stepped = Some(shown);
    }
    shown
}

/// The page the preview steps to from `shown`: the range's next page after it (`forward`) or the
/// last one before it, or `None` at either end.
fn step_preview(range: &PageRange, shown: usize, count: usize, current: usize, selection: &[usize], forward: bool) -> Option<usize> {
    let pages = range.pages(count, current, selection);
    if forward { pages.into_iter().find(|&p| p > shown) } else { pages.into_iter().rev().find(|&p| p < shown) }
}

/// The first character of the dialog's text that applying would refuse (the standard fonts can't
/// draw it), as the engine decides it. Pictures draw no text.
fn undrawable(kind: MarkKind, d: &MarksDraft) -> Option<char> {
    match kind {
        MarkKind::HeaderFooter => d.hf.first_undrawable(),
        MarkKind::Watermark if !d.use_file => d.wm.first_undrawable(),
        MarkKind::Watermark | MarkKind::Background | MarkKind::PrinterMarks => None,
    }
}

fn mark_file(d: &MarksDraft) -> Option<pdfcraft_engine::MarkFile> {
    let (name, bytes) = d.file.clone().filter(|_| d.use_file)?;
    Some(pdfcraft_engine::MarkFile { name, bytes, page: d.file_page.max(1) - 1 })
}

/// The edit the dialog's OK makes.
pub fn edit(d: &MarksDraft, kind: MarkKind, count: usize, current: usize, selection: &[usize]) -> Edit {
    let pages = d.range.pages(count, current, selection);
    match kind {
        MarkKind::HeaderFooter => Edit::AddHeaderFooter { pages, settings: d.hf.clone(), replace: d.replace },
        MarkKind::Watermark => {
            let mut wm = d.wm.clone();
            if d.fit {
                wm.font_size = 0.0;
            }
            Edit::AddWatermark { pages, settings: wm, replace: d.replace, file: mark_file(d) }
        }
        MarkKind::Background => Edit::AddBackground { pages, settings: d.bg.clone(), replace: d.replace, file: mark_file(d) },
        MarkKind::PrinterMarks => Edit::AddPrinterMarks { pages, settings: pdfcraft_engine::PrinterMarks::default(), replace: d.replace },
    }
}

/// How the margin rows show lengths: the app's unit, which the draft (points) is converted to
/// and from at the dialog's edges.
#[derive(Clone, Copy)]
struct MarginUnits {
    /// Unit lengths per point.
    per: f64,
    kind: UnitKind,
    decimals: u32,
}

impl MarginUnits {
    /// The margin rows' units for the app's `unit`.
    fn of(unit: crate::persist::units::Unit) -> Self {
        use crate::persist::units::Unit;
        let kind = match unit {
            Unit::Inches => UnitKind::Inches,
            Unit::Millimetres => UnitKind::Millimetres,
            Unit::Centimetres => UnitKind::Centimetres,
            Unit::Points => UnitKind::Points,
        };
        Self { per: unit.per_point(), kind, decimals: unit.decimals() as u32 }
    }
}

/// The standard faces a header can be drawn in (#3754), by the id their choice row keeps.
const FACES: [(&str, FontFamily); 3] = [("helvetica", FontFamily::Helvetica), ("times", FontFamily::Times), ("courier", FontFamily::Courier)];

fn face_id(f: FontFamily) -> &'static str {
    FACES.iter().find(|(_, face)| *face == f).map_or("helvetica", |(id, _)| id)
}

fn colour_value(c: [f64; 3]) -> FieldValue {
    FieldValue::Color(c.map(|v| v as f32))
}

fn colour_of(f: &DialogFields, key: &str, or: [f64; 3]) -> [f64; 3] {
    f.color(key).map_or(or, |c| c.map(f64::from))
}

/// The values the dialog for `kind` opens with, from the carried draft.
fn opening_fields(d: &MarksDraft, kind: MarkKind, m: &MarginUnits) -> DialogFields {
    let mut f = DialogFields::new();
    f.set("range", PageRangeOptions::field_value(&d.range));
    match kind {
        MarkKind::HeaderFooter => {
            f.set("font", FieldValue::Text(face_id(d.hf.font).into()));
            f.set("hf.size", FieldValue::Number(d.hf.font_size));
            f.set("hf.underline", FieldValue::Bool(d.hf.underline));
            f.set("marks.hf.colour", colour_value(d.hf.color));
            for (k, margin) in d.hf.margins.iter().enumerate() {
                f.set(format!("hf.margin.{k}"), FieldValue::Number(margin * m.per));
            }
            for (k, text) in d.hf.text.iter().enumerate() {
                f.set(format!("hf.text.{k}"), FieldValue::Text(text.clone()));
            }
            f.set("hf.focused", FieldValue::Int(d.focused_box as i64));
            f.set("page_format", FieldValue::Text(d.page_format.to_string()));
            f.set("date_format", FieldValue::Text(d.date_format.to_string()));
            f.set("hf.start", FieldValue::Int(i64::from(d.hf.start_number)));
            f.set("shrink", FieldValue::Bool(d.hf.shrink));
        }
        MarkKind::Watermark | MarkKind::Background => {
            f.set("source", FieldValue::Text(source_id(kind, d.use_file).into()));
            f.set("file.path", FieldValue::Text(d.file.as_ref().map(|x| x.0.clone()).unwrap_or_default()));
            f.set("file.is_pdf", FieldValue::Bool(d.file.as_ref().is_some_and(|x| x.1.starts_with(b"%PDF"))));
            f.set("file.page", FieldValue::Int(d.file_page as i64));
            if kind == MarkKind::Watermark {
                f.set("file.scale", FieldValue::Number(d.wm.scale * 100.0));
                f.set("wm.text", FieldValue::Text(d.wm.text.clone()));
                f.set("wm.fit", FieldValue::Bool(d.fit));
                f.set("wm.size", FieldValue::Number(if d.wm.font_size == 0.0 { 72.0 } else { d.wm.font_size }));
                f.set("marks.wm.colour", colour_value(d.wm.color));
                f.set("wm.rotation", FieldValue::Number(d.wm.rotation));
                f.set("wm.opacity", FieldValue::Number(d.wm.opacity * 100.0));
                f.set("wm.behind", FieldValue::Text(if d.wm.behind { "behind" } else { "above" }.into()));
                f.set("wm.offset.0", FieldValue::Number(d.wm.offset[0]));
                f.set("wm.offset.1", FieldValue::Number(d.wm.offset[1]));
            } else {
                f.set("file.scale", FieldValue::Number(d.bg.scale * 100.0));
                f.set("marks.bg.colour", colour_value(d.bg.color));
                f.set("bg.opacity", FieldValue::Number(d.bg.opacity * 100.0));
                f.set("replace_backdrop", FieldValue::Bool(d.bg.replace_backdrop));
            }
        }
        MarkKind::PrinterMarks => {}
    }
    f
}

fn source_id(kind: MarkKind, use_file: bool) -> &'static str {
    match (use_file, kind) {
        (true, _) => "file",
        (false, MarkKind::Background) => "colour",
        (false, _) => "text",
    }
}

/// Reads the dialog's values into the draft: the one place a field's key meets a draft member.
/// A key the dialog lacks leaves its member as it was.
fn read_fields(d: &mut MarksDraft, f: &DialogFields, kind: MarkKind, m: &MarginUnits) {
    if let Some(range) = PageRangeOptions::value(f, "range") {
        d.range = range;
    }
    match kind {
        MarkKind::HeaderFooter => {
            if let Some(face) = f.text("font").and_then(|id| FACES.iter().find(|(i, _)| *i == id)) {
                d.hf.font = face.1;
            }
            d.hf.font_size = f.number("hf.size").unwrap_or(d.hf.font_size);
            d.hf.underline = f.bool("hf.underline").unwrap_or(d.hf.underline);
            d.hf.color = colour_of(f, "marks.hf.colour", d.hf.color);
            for k in 0..4 {
                if let (Some(v), Some(slot)) = (f.number(&format!("hf.margin.{k}")), d.hf.margins.get_mut(k)) {
                    *slot = if m.per > 0.0 { v / m.per } else { v };
                }
            }
            for (k, slot) in d.hf.text.iter_mut().enumerate() {
                if let Some(text) = f.text(&format!("hf.text.{k}")) {
                    text.clone_into(slot);
                }
            }
            d.focused_box = f.int("hf.focused").map_or(d.focused_box, |k| (k.max(0) as usize).min(5));
            d.page_format = f.text("page_format").and_then(|k| k.parse().ok()).filter(|k| *k < PAGE_FORMATS.len()).unwrap_or(d.page_format);
            d.date_format = f.text("date_format").and_then(|k| k.parse().ok()).filter(|k| *k < DATE_FORMATS.len()).unwrap_or(d.date_format);
            d.hf.start_number = f.int("hf.start").map_or(d.hf.start_number, |n| n.clamp(1, 999_999) as u32);
            d.hf.shrink = f.bool("shrink").unwrap_or(d.hf.shrink);
        }
        MarkKind::Watermark | MarkKind::Background => {
            d.use_file = f.text("source") == Some("file");
            d.file_page = f.int("file.page").map_or(d.file_page, |p| p.clamp(1, 9999) as usize);
            let scale = f.number("file.scale").map(|p| p / 100.0);
            if kind == MarkKind::Watermark {
                d.wm.scale = scale.unwrap_or(d.wm.scale);
                if let Some(text) = f.text("wm.text") {
                    text.clone_into(&mut d.wm.text);
                }
                d.fit = f.bool("wm.fit").unwrap_or(d.fit);
                d.wm.font_size = f.number("wm.size").unwrap_or(d.wm.font_size);
                d.wm.color = colour_of(f, "marks.wm.colour", d.wm.color);
                d.wm.rotation = f.number("wm.rotation").unwrap_or(d.wm.rotation);
                d.wm.opacity = f.number("wm.opacity").map_or(d.wm.opacity, |p| p / 100.0);
                d.wm.behind = f.text("wm.behind").map_or(d.wm.behind, |b| b == "behind");
                for k in 0..2 {
                    if let (Some(v), Some(slot)) = (f.number(&format!("wm.offset.{k}")), d.wm.offset.get_mut(k)) {
                        *slot = v;
                    }
                }
            } else {
                d.bg.scale = scale.unwrap_or(d.bg.scale);
                d.bg.color = colour_of(f, "marks.bg.colour", d.bg.color);
                d.bg.opacity = f.number("bg.opacity").map_or(d.bg.opacity, |p| p / 100.0);
                // Not offered (no page paints a backdrop of its own) leaves the default: on.
                d.bg.replace_backdrop = f.bool("replace_backdrop").unwrap_or(true);
            }
        }
        MarkKind::PrinterMarks => {}
    }
}

/// What the dialog's build derives from its bound values in the pass that shows them: whether OK
/// can be pressed, and how many pages the range selects.
#[derive(Clone, Default)]
struct Notes {
    /// Whether OK can be pressed, and why not when the text can't be drawn.
    ready: bool,
    reason: Option<String>,
    /// How many pages the range selects.
    applies: usize,
}

/// The notes for the draft `d` read from the dialog's values; `has_file` is whether the file row
/// holds a name.
fn notes_of(kind: MarkKind, d: &MarksDraft, has_file: bool, g: &Geometry, selection: &[usize]) -> Notes {
    let applies = d.range.pages(g.count, g.current, selection).len();
    let refused = undrawable(kind, d);
    let ready = refused.is_none()
        && match kind {
            MarkKind::HeaderFooter => d.hf.text.iter().any(|x| !x.trim().is_empty()),
            MarkKind::Watermark => {
                if d.use_file {
                    has_file
                } else {
                    !d.wm.text.trim().is_empty()
                }
            }
            MarkKind::Background => !d.use_file || has_file,
            MarkKind::PrinterMarks => true,
        }
        && applies > 0;
    Notes { ready, reason: refused.map(|c| crate::content_ui::undrawable_message(c).to_string()), applies }
}

/// What the rows need to know about the document, fixed while the dialog is open.
#[derive(Clone, Copy)]
struct Geometry {
    count: usize,
    current: usize,
    units: MarginUnits,
}

fn choice(options: impl IntoIterator<Item = (String, String)>, default: &str) -> ParamKind {
    ParamKind::choice(options.into_iter().map(|(id, label)| (id, label.into())).collect(), Some(default.to_string()))
}

fn percent(key: &str, label: &'static str, default: f64) -> Param {
    Param::new(key, label, ParamKind::range(0.0, 100.0, default, Default::default()))
}

/// The six text boxes of Header & Footer: the box with focus is kept under `hf.focused`, which
/// Insert Page Number and Insert Date type into.
fn boxes_row() -> Param {
    let companions: Vec<String> = (0..6).map(|k| format!("hf.text.{k}")).chain(["hf.focused".to_string()]).collect();
    Param::custom("hf.boxes", tl!("Header and footer text"), |ui, fields, _cx| {
        let mut out: Option<egui::Response> = None;
        for k in 0..BOX_NAMES.len() {
            let key = format!("hf.text.{k}");
            let before = fields.text(&key).unwrap_or_default().to_owned();
            let mut text = before.clone();
            let r = ui
                .push_id(("hf-box", k), |ui| {
                    TextField::new(tl!(BOX_NAMES[k]), craft::ui::contract::Bind::bound(&mut text)).lines(2).show(ui).response
                })
                .inner;
            // The field's focus stop is the component: egui's focus sits on its text part.
            if craft::ui::runtime::focus::is_focused(ui.ctx(), r.id) {
                fields.set("hf.focused", FieldValue::Int(k as i64));
            }
            if text != before {
                fields.set(key, FieldValue::Text(text));
            }
            out = Some(match out {
                Some(o) => o.union(r),
                None => r,
            });
        }
        out.unwrap_or_else(|| ui.response())
    })
    .companions(companions)
    .full_width()
}

fn file_rows(rows: &mut Vec<Param>) {
    rows.push(
        Param::new("file.path", tl!("File"), ParamKind::text(""))
            .browse(PathKind::open_file(&["pdf", "png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"]))
            .hint(tl!("No file chosen"))
            .visible_when("source", "file"),
    );
    rows.push(Param::new("file.page", tl!("Page number"), ParamKind::int(Some(1), Some(9999), 1)).visible_when("file.is_pdf", true));
    rows.push(
        Param::new("file.scale", tl!("Scale relative to target page"), ParamKind::range(5.0, 100.0, 100.0, Default::default()))
            .visible_when("source", "file"),
    );
}

/// The rows of the dialog for `kind`.
fn rows(kind: MarkKind, g: &Geometry) -> Vec<Param> {
    let mut rows = Vec::new();
    match kind {
        MarkKind::HeaderFooter => {
            rows.push(Param::new("font", tl!("Font:"), choice(FACES.iter().map(|(id, f)| ((*id).to_string(), f.label().to_string())), "helvetica")));
            rows.push(Param::number("hf.size", tl!("Size"), 12.0, 0.5, 1).bounds(Some(4.0), Some(72.0)));
            rows.push(Param::new("hf.underline", tl!("Underline"), ParamKind::bool(false)));
            rows.push(Param::new("marks.hf.colour", tl!("Colour"), ParamKind::color(None)));
            let m = g.units;
            for (k, label) in [(0, tl!("Top margin")), (1, tl!("Bottom margin")), (2, tl!("Left margin")), (3, tl!("Right margin"))] {
                // Up to 10 inches, whatever the unit.
                rows.push(
                    Param::number(format!("hf.margin.{k}"), label, 0.0, m.kind.step(), m.decimals)
                        .unit(m.kind)
                        .bounds(Some(0.0), Some(720.0 * m.per)),
                );
            }
            rows.push(boxes_row());
            let list = |formats: &[&str]| {
                formats.iter().enumerate().map(|(k, f)| (k.to_string(), f.trim_matches(['<', '>']).to_string())).collect::<Vec<_>>()
            };
            rows.push(Param::new("page_format", tl!("Page number format"), choice(list(&PAGE_FORMATS), "0")));
            rows.push(Param::new("date_format", tl!("Date format"), choice(list(&DATE_FORMATS), "0")));
            rows.push(Param::new("hf.start", tl!("Start page number"), ParamKind::int(Some(1), Some(999_999), 1)));
            rows.push(Param::new("shrink", tl!("Shrink the page content to make room for the header and footer"), ParamKind::bool(false)));
        }
        MarkKind::Watermark => {
            let source = [("text".to_string(), tl!("Text").to_string()), ("file".to_string(), tl!("File").to_string())];
            rows.push(Param::new("source", tl!("Source"), ParamKind::radio(source.map(|(i, l)| (i, l.into())).to_vec(), Some("text".into()))));
            file_rows(&mut rows);
            rows.push(Param::new("wm.text", tl!("Text"), ParamKind::text("")).multiline(2).hint("CONFIDENTIAL").enabled_when("source", "text"));
            rows.push(Param::new("wm.fit", tl!("Fit to page"), ParamKind::bool(true)).enabled_when("source", "text"));
            rows.push(Param::number("wm.size", tl!("Size"), 72.0, 1.0, 0).bounds(Some(6.0), Some(300.0)).enabled_when("wm.fit", false));
            rows.push(Param::new("marks.wm.colour", tl!("Colour"), ParamKind::color(None)));
            rows.push(Param::new("wm.rotation", tl!("Rotation"), ParamKind::angle(0.0)));
            rows.push(percent("wm.opacity", tl!("Opacity"), 100.0));
            let place =
                [("behind".to_string(), tl!("Appear behind page").to_string()), ("above".to_string(), tl!("Appear on top of page").to_string())];
            rows.push(Param::new("wm.behind", tl!("Location"), ParamKind::radio(place.map(|(i, l)| (i, l.into())).to_vec(), Some("behind".into()))));
            for (k, label) in [(0, tl!("Horizontal offset from centre")), (1, tl!("Vertical offset from centre"))] {
                rows.push(
                    Param::number(format!("wm.offset.{k}"), label, 0.0, 1.0, 1).unit(UnitKind::Points).bounds(Some(-MAX_OFFSET), Some(MAX_OFFSET)),
                );
            }
        }
        MarkKind::Background => {
            let source = [("colour".to_string(), tl!("From colour").to_string()), ("file".to_string(), tl!("File").to_string())];
            rows.push(Param::new("source", tl!("Source"), ParamKind::radio(source.map(|(i, l)| (i, l.into())).to_vec(), Some("colour".into()))));
            rows.push(Param::new("marks.bg.colour", tl!("Colour"), ParamKind::color(None)).enabled_when("source", "colour"));
            file_rows(&mut rows);
            rows.push(percent("bg.opacity", tl!("Opacity"), 100.0));
            // Offered only once the backdrop check finds a white backdrop: its label names the pages, and the pass
            // sets both from the check's results (`backdrop_rows`).
            rows.push(Param::new("replace_backdrop", tl!("Replace it"), ParamKind::bool(true)).shown(false));
        }
        MarkKind::PrinterMarks => {}
    }
    rows.push(PageRangeOptions::param("range", tl!("Page Range Options"), g.count, g.current, PageRangeChoices::Acrobat));
    rows
}

/// What the backdrop check found, in the Background dialog: pages that paint their own white backdrop are offered
/// the Replace box, named in its label; pages a picture covers (scans) are said to keep hiding the background; pages
/// not checked yet show "Checking pages…".
fn backdrop_rows(dialog: &mut HostedParamDialog, n: &BackdropCount) {
    let mut notes: Vec<Label> = Vec::new();
    if n.pending > 0 {
        notes.push(tl!("Checking pages…").into());
    }
    let offered = !n.fills.is_empty();
    dialog.set_param_shown("replace_backdrop", offered);
    if offered {
        let label = pages_phrase(
            &n.fills,
            tl!("Page {pages} paints its own white backdrop, which would hide the background. Replace it"),
            tl!("Pages {pages} paint their own white backdrop, which would hide the background. Replace it"),
        );
        dialog.set_param_label("replace_backdrop", label);
        notes.push(tl!("Removing the background puts the page's own backdrop back.").into());
    }
    if !n.images.is_empty() {
        let text = pages_phrase(
            &n.images,
            tl!("A picture covers page {pages} (a scan?), so the background won't show there."),
            tl!("Pictures cover pages {pages} (scans?), so the background won't show there."),
        );
        notes.push(text.into());
    }
    dialog.set_notes(notes);
}

/// The dialog's title: "Add …" or "Update …".
fn title(kind: MarkKind, replace: bool) -> String {
    let verb = if replace { tl!("Update") } else { tl!("Add") };
    match kind {
        MarkKind::HeaderFooter => crate::i18n::fmt(tl!("{verb} Header and Footer"), &[("verb", verb)]),
        MarkKind::Watermark => crate::i18n::fmt(tl!("{verb} Watermark"), &[("verb", verb)]),
        MarkKind::Background => crate::i18n::fmt(tl!("{verb} Background"), &[("verb", verb)]),
        // Printer marks have a dialog of their own (printer_marks_ui); this one is never opened for them.
        MarkKind::PrinterMarks => tl!("Add printer marks").to_string(),
    }
}

/// What a render job is for: the document version, the kind of mark taken away, the page and the
/// scale (×1000) it is drawn at.
type RenderKey = (DocId, u64, &'static str, usize, u32);

/// The dialog's preview work while it is open: the backdrop check, the pages of an Update drawn
/// without the marks they replace, and what the rows are told. Dropping it (the dialog closed)
/// cancels its jobs.
pub struct MarksPreview {
    kind: MarkKind,
    backdrops: JobQueue<(DocId, u64, Vec<usize>), Result<Vec<Option<Backdrop>>, String>>,
    renders: JobQueue<RenderKey, Result<egui::ColorImage, String>>,
    /// The document without its marks of `kind`, made once per document version: `None` inside
    /// when it has none or can't be made.
    unmarked: Option<((DocId, u64), Option<Arc<Mutex<pdfcraft_engine::UnmarkedPages>>>)>,
    pictures: HashMap<RenderKey, egui::TextureHandle>,
    failed: HashSet<RenderKey>,
}

fn queue<K: Clone + Eq + std::hash::Hash + Send + 'static, R: Send + 'static>(
    jobs: Arc<dyn craft::host::Jobs>,
    wake: craft::host::Wake,
    name: &'static str,
) -> JobQueue<K, R> {
    JobQueue::new(jobs, wake, QueueOptions { lanes: QueueOptions::default_lanes(), name, order: QueueOrder::default() })
}

impl MarksPreview {
    fn new(kind: MarkKind, jobs: Arc<dyn craft::host::Jobs>, wake: craft::host::Wake) -> Self {
        Self {
            kind,
            backdrops: queue(jobs.clone(), wake.clone(), "marks-backdrops"),
            renders: queue(jobs, wake, "marks-preview"),
            unmarked: None,
            pictures: HashMap::new(),
            failed: HashSet::new(),
        }
    }

    /// Whether the preview has nothing left to wait for: the pages are checked and drawn (the
    /// automation state reads it).
    pub fn is_ready(&self) -> bool {
        let (b, r) = (self.backdrops.counts(), self.renders.counts());
        b.queued + b.running + b.finished + r.queued + r.running + r.finished == 0
    }

    /// Takes the finished backdrop batches of this document version, records them, starts the next
    /// batch and returns what the check found among `pages`.
    fn check_backdrops(&mut self, d: &mut MarksDraft, doc: &pdfcraft_engine::Document, pages: &[usize]) -> BackdropCount {
        let key = (doc.id, doc.edit_generation());
        let mut finished = Vec::new();
        while let Some(done) = self.backdrops.try_recv() {
            let (id, generation, batch) = done.key;
            if (id, generation) != key {
                continue;
            }
            // A batch that could not be read counts as having no backdrop, so it isn't asked again.
            let found = done.result.unwrap_or_else(|_| vec![None; batch.len()]);
            finished.push((batch, found));
        }
        let (n, next) = d.backdrops.update(key, doc.info.pages.len(), pages, finished);
        self.backdrops.reprioritize(|k, p| ((k.0, k.1) == key).then_some(p));
        if !next.is_empty() {
            let job_key = (key.0, key.1, next.clone());
            if !self.backdrops.contains(&job_key) {
                let source = doc.export_source();
                self.backdrops.submit(job_key, Priority::VISIBLE, move |_| pdfcraft_engine::export::page_backdrops(&source, &next));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.backdrops.run_inline(1);
        n
    }

    /// The picture of `page` without the marks of this dialog's kind, for Update: `None` when the
    /// thumbnail already is that (Add, no such marks, the pages can't be prepared, or `page` failed
    /// to render). `near` are the pages a step goes to, rendered ahead.
    fn unmarked_picture(&mut self, ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize, near: &[usize]) -> Option<PreviewState> {
        let key = (doc.id, doc.edit_generation());
        if self.unmarked.as_ref().map(|u| u.0) != Some(key) {
            self.pictures.clear();
            self.failed.clear();
            self.renders.reprioritize(|_, _| None);
            let pages = doc.without_marks(self.kind).ok().flatten().map(|p| Arc::new(Mutex::new(p)));
            self.unmarked = Some((key, pages));
        }
        let pages = self.unmarked.as_ref()?.1.clone()?;
        while let Some(done) = self.renders.try_recv() {
            match done.result {
                Ok(image) => {
                    self.pictures.insert(done.key, ctx.load_texture("marks-preview-unmarked", image, egui::TextureOptions::LINEAR));
                }
                Err(_) => {
                    self.failed.insert(done.key);
                }
            }
        }
        let label = dialog_key(self.kind);
        // At the preview's size in pixels.
        let want = |p: usize| {
            let size = doc.info.pages.get(p)?;
            let scale = PREVIEW_SIDE * ctx.pixels_per_point() / size.width.max(size.height).max(1.0);
            Some(((doc.id, doc.edit_generation(), label, p, (scale * 1000.0) as u32), scale))
        };
        let (shown, _) = want(page)?;
        let ahead: Vec<(RenderKey, f32)> = near.iter().filter_map(|&p| want(p)).collect();
        let wanted: Vec<RenderKey> = std::iter::once(shown).chain(ahead.iter().map(|(k, _)| *k)).collect();
        for (key, scale) in std::iter::once(want(page)?).chain(ahead) {
            if self.pictures.contains_key(&key) || self.failed.contains(&key) || self.renders.contains(&key) {
                continue;
            }
            let priority = if key == shown { Priority::VISIBLE } else { Priority::NEAR };
            let pages = Arc::clone(&pages);
            let page = key.3;
            self.renders.submit(key, priority, move |cancel| {
                if cancel.is_cancelled() {
                    return Err("cancelled".to_string());
                }
                let mut pages = pages.lock().map_err(|_| "the pages are unavailable".to_string())?;
                let out = pages.render_page(page, scale).map_err(|e| e.to_string())?;
                Ok(crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba))
            });
        }
        self.renders.reprioritize(|k, p| wanted.contains(k).then_some(p));
        #[cfg(target_arch = "wasm32")]
        self.renders.run_inline(1);
        self.pictures.retain(|k, _| wanted.contains(k));
        if self.failed.contains(&shown) {
            return None;
        }
        Some(match self.pictures.get(&shown) {
            Some(handle) => PreviewState::Ready(egui::load::SizedTexture::from_handle(handle)),
            None => PreviewState::Pending(None),
        })
    }
}

/// The picture of `page` from the page thumbnails, with the pages next to it asked for ahead.
fn thumbnail_picture(ctx: &egui::Context, source: &impl ThumbSource<usize>, page: usize, near: &[usize]) -> PreviewState {
    let ppp = ctx.pixels_per_point();
    let request = ThumbRequest::for_cell(PREVIEW_SIDE, ppp);
    // The store's own size cap covers the preview at this screen's density.
    let store = Thumbnails::<usize>::in_context(ctx, "marks-preview", ThumbnailOptions::new().max_side(request.side));
    for &p in near {
        let _ = store.state(ctx, source, &p, &request, Priority::NEAR);
    }
    match store.state(ctx, source, &page, &request, Priority::VISIBLE) {
        TextureState::Ready(texture) => PreviewState::Ready(texture.sized()),
        TextureState::Failed { .. } => PreviewState::Failed(tl!("The page can't be previewed").into()),
        other => match other.shown() {
            Some(texture) => PreviewState::Ready(texture.sized()),
            None => PreviewState::Pending(None),
        },
    }
}

/// The marks laid out on the preview's page: what the preview closure drew over the picture.
fn overlay(kind: MarkKind, d: &MarksDraft, size: (f64, f64)) -> SharedOverlay {
    let hf = d.hf.clone();
    let (wm, fit, use_file) = (d.wm.clone(), d.fit, d.use_file);
    let file_name = d.file.as_ref().map(|f| f.0.clone());
    let (page_w, page_h) = (size.0.max(1.0), size.1.max(1.0));
    Arc::new(move |cx: &ContentPainterCx<'_>, _: CanvasTransform, r: Rect| {
        let p = &cx.painter;
        let s = (f64::from(r.width()) / page_w) as f32;
        match kind {
            MarkKind::HeaderFooter => {
                if hf.shrink {
                    // The band the page content shrinks into, centred across.
                    let (top, bottom) = hf.bands();
                    let k = ((page_h - top - bottom) / page_h).clamp(0.0, 1.0) as f32;
                    let band = Rect::from_min_max(pos2(r.left(), r.top() + top as f32 * s), pos2(r.right(), r.bottom() - bottom as f32 * s));
                    let content = Rect::from_center_size(band.center(), vec2(r.width() * k, band.height().max(0.0)));
                    p.rect_filled(content, CornerRadius::ZERO, Color32::from_gray(232));
                }
                for (k, text) in hf.text.iter().enumerate() {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let shown = text.replace("<<", "").replace(">>", "");
                    let y = if k < 3 { r.top() + hf.margins[0] as f32 * s } else { r.bottom() - hf.margins[1] as f32 * s };
                    let (x, align) = match k % 3 {
                        0 => (r.left() + hf.margins[2] as f32 * s, egui::Align2::LEFT_CENTER),
                        1 => (r.center().x, egui::Align2::CENTER_CENTER),
                        _ => (r.right() - hf.margins[3] as f32 * s, egui::Align2::RIGHT_CENTER),
                    };
                    // Courier previews in the monospaced UI face, the others proportional.
                    let size = (hf.font_size as f32 * s).max(5.0);
                    let font = if hf.font == FontFamily::Courier { egui::FontId::monospace(size) } else { egui::FontId::proportional(size) };
                    p.text(pos2(x, y), align, shown, font, rgb32(hf.color));
                }
            }
            MarkKind::Watermark if use_file => {
                if let Some(name) = &file_name {
                    p.rect_stroke(r.shrink(6.0), CornerRadius::ZERO, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                    p.text(r.center(), egui::Align2::CENTER_CENTER, name.clone(), egui::FontId::proportional(10.0), Color32::GRAY);
                }
            }
            MarkKind::Watermark => {
                let text = wm.text.trim();
                if text.is_empty() {
                    return;
                }
                let size = if fit {
                    (r.width().hypot(r.height()) * 0.5 / (text.len().max(1) as f32 * 0.55)).clamp(6.0, 80.0)
                } else {
                    (wm.font_size as f32 * s).max(5.0)
                };
                let galley = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(size), rgb32(wm.color).gamma_multiply(wm.opacity as f32));
                let angle = -(wm.rotation as f32).to_radians();
                let half = galley.size() / 2.0;
                let (sn, cs) = angle.sin_cos();
                let offset = vec2(half.x * cs - half.y * sn, half.x * sn + half.y * cs);
                let centre = r.center() + vec2(wm.offset[0] as f32 * s, -wm.offset[1] as f32 * s);
                p.add(egui::epaint::TextShape::new(centre - offset, galley, Color32::BLACK).with_angle(angle));
            }
            MarkKind::Background if use_file => {
                if let Some(name) = &file_name {
                    p.rect_stroke(r.shrink(6.0), CornerRadius::ZERO, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                    p.text(r.center(), egui::Align2::CENTER_CENTER, name.clone(), egui::FontId::proportional(10.0), Color32::GRAY);
                }
            }
            // A colour is the page's tint, not the overlay's.
            MarkKind::Background | MarkKind::PrinterMarks => {}
        }
    })
}

impl PdfCraftApp {
    /// Opens the dialog for `kind`: Add, or Update when `replace`. The draft carries over from the
    /// last opening; the preview starts at the current page.
    pub(crate) fn open_marks(&mut self, ctx: &egui::Context, kind: MarkKind, replace: bool) {
        if kind == MarkKind::PrinterMarks {
            return;
        }
        let Some((index, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views.get(index).map_or(0, |v| v.current());
        let selection = self.selected_pages(index);
        self.marks_draft.replace = replace;
        self.marks_draft.opened(kind);
        self.marks_draft.preview = None;
        let units = MarginUnits::of(self.units);
        let geometry = Geometry { count, current, units };
        let fields = opening_fields(&self.marks_draft, kind, &units);
        let base = self.marks_draft.clone();
        let heading = title(kind, replace);
        // Passed inline: a closure made apart from the call gets one lifetime for its argument and result.
        // PLAN: assumed `self.dialogs: DialogHost`, `self.host`/`self.wake` (pdf-hub-w1 note 11)
        let dialog = HostedParamDialog::new(ctx, dialog_key(kind), fields, move |fields: &mut DialogFields| {
            let mut draft = base.clone();
            read_fields(&mut draft, fields, kind, &units);
            let has_file = fields.text("file.path").is_some_and(|p| !p.is_empty());
            let notes = notes_of(kind, &draft, has_file, &geometry, &selection);
            let applies = notes.applies;
            let refusal = notes.reason.clone();
            let mut dialog = ParamDialog::new(dialog_key(kind), heading.clone(), rows(kind, &geometry))
                .values(craft::ui::contract::Bind::bound(fields))
                .previews(true)
                .ok_enabled(notes.ready)
                .extra_buttons(if kind == MarkKind::HeaderFooter {
                    vec![("insert.page".to_string(), tl!("Insert Page Number").into()), ("insert.date".to_string(), tl!("Insert Date").into())]
                } else {
                    Vec::new()
                })
                .below(move |ui, _| {
                    let applies = if applies == 1 {
                        tl!("Applies to 1 page").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Applies to {n} pages"), &[("n", &applies.to_string())])
                    };
                    if let Some(reason) = &refusal {
                        Text::new(reason.clone()).tone(craft::theme::variant::Tone::Bad).show(ui);
                    }
                    Text::new(applies).show(ui);
                    if replace {
                        Text::new(tl!("Replaces the existing one on those pages.")).show(ui);
                    }
                });
            if let Some(reason) = &notes.reason {
                dialog = dialog.ok_reason(reason.clone().into());
            }
            dialog
        });
        self.dialogs.open(dialog);
        self.marks_preview = Some(MarksPreview::new(kind, self.host.jobs_shared().clone(), self.wake.clone()));
    }

    /// The pages (0-based) selected in view `index`.
    fn selected_pages(&self, index: usize) -> Vec<usize> {
        self.views.get(index).map(|v| v.selected.items.iter().copied().collect()).unwrap_or_default()
    }

    /// One pass of the open marks dialog: answers its requests, reads its values into the draft,
    /// keeps the preview current and applies the edit once OK answered.
    pub(crate) fn marks_pass(&mut self, ctx: &egui::Context) {
        let Some(mut preview) = self.marks_preview.take() else { return };
        let kind = preview.kind;
        let id = dialog_id(dialog_key(kind));
        if let Some(result) = self.dialogs.take_by_id::<HostedParamDialog>(id) {
            if let DialogResult::Ok(answer) = result {
                self.finish_marks(ctx, kind, &answer);
            }
            return;
        }
        if !self.dialogs.stack().contains(id) {
            return;
        }
        let Some((index, doc_id)) = self.active_ids() else {
            self.dialogs.cancel_by_id(ctx, id);
            self.marks_preview = Some(preview);
            return;
        };

        let (fields, colour, path, action, step) = {
            let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) else {
                return;
            };
            (dialog.fields().clone(), dialog.pick_color(), dialog.pick_path(), dialog.take_action(), dialog.take_preview_step())
        };
        if let Some(request) = colour {
            let key = request.key.clone();
            self.open_colour_picker(crate::colour_picker::PickerHost::App, request, move |app: &mut PdfCraftApp, _: &egui::Context, colour| {
                if let Some(dialog) = app.dialogs.get_mut_by_id::<HostedParamDialog>(dialog_id(dialog_key(kind))) {
                    dialog.answer_color(key, colour);
                }
            });
        }
        if let Some(request) = path {
            let serial = self.dialogs.open_serial(id);
            let mut pick = craft::host::PickRequest::open("marks");
            for filter in &request.filters {
                let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
                pick = pick.filter(filter.name.clone(), &extensions);
            }
            if let PathKind::OpenFile { extensions, .. } = &request.kind {
                let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
                if !extensions.is_empty() {
                    pick = pick.filter(tl!("Supported files"), &extensions);
                }
            }
            let target = self.pick_target(Some(doc_id));
            self.pick_file(pick, target, move |app: &mut PdfCraftApp, _: &egui::Context, picked| {
                // The file Browse chose, read through the host's file service as every pick is.
                match app.read_picked(picked).map(|files| files.into_iter().next()) {
                    Ok(Some((name, bytes))) => app.marks_picked(kind, serial, name, bytes),
                    Ok(None) => {}
                    Err((name, e)) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
                }
            });
        }
        if let Some(action) = action {
            let (formats, key): (&[&str], _) = if action == "insert.page" { (&PAGE_FORMATS, "page_format") } else { (&DATE_FORMATS, "date_format") };
            let format = fields.text(key).and_then(|k| k.parse::<usize>().ok()).and_then(|k| formats.get(k));
            let focused = fields.int("hf.focused").map_or(0, |k| k.clamp(0, 5));
            if let Some(format) = format {
                let box_key = format!("hf.text.{focused}");
                let text = format!("{}{format}", fields.text(&box_key).unwrap_or_default());
                if let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) {
                    dialog.set_field(box_key, FieldValue::Text(text));
                }
            }
        }

        let Some(doc) = self.session.get(doc_id) else {
            self.marks_preview = Some(preview);
            return;
        };
        let count = doc.info.pages.len();
        let current = self.views.get(index).map_or(0, |v| v.current());
        let selection = self.selected_pages(index);
        let source = crate::canvas::pictures::DocPages::new(&self.session, doc_id).revisions(&self.views[index].revisions);
        let units = MarginUnits::of(self.units);
        let d = &mut self.marks_draft;
        read_fields(d, &fields, kind, &units);

        if let Some(step) = step {
            let shown = shown_preview(&d.range, &mut d.preview, current, count, &selection);
            let forward = step == PagerStep::Next;
            d.preview = step_preview(&d.range, shown, count, current, &selection, forward).or(d.preview);
        }
        let pages = d.range.pages(count, current, &selection);
        let target = shown_preview(&d.range, &mut d.preview, current, count, &selection);
        let size = doc.info.pages.get(target).map(|p| (f64::from(p.width), f64::from(p.height)));
        let back = step_preview(&d.range, target, count, current, &selection, false);
        let next = step_preview(&d.range, target, count, current, &selection, true);

        let backdrops = (kind == MarkKind::Background).then(|| preview.check_backdrops(d, doc, &pages));
        let near: Vec<usize> = back.into_iter().chain(next).collect();
        let picture = match preview.unmarked_picture_for(ctx, doc, d.replace, target, &near) {
            Some(state) => state,
            None => thumbnail_picture(ctx, &source, target, &near),
        };
        let tint =
            (kind == MarkKind::Background && !d.use_file).then(|| DataColor::new(background_tint(d, d.backdrops.found(doc, target).flatten())));

        let pager = PanePager { shown: target + 1, count, shown_label: None, back: back.is_some(), next: next.is_some(), unit: PagerUnit::Page };
        let overlay = size.map(|size| overlay(kind, d, size));
        if let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) {
            dialog.set_preview(Some(picture));
            dialog.set_preview_pager(Some(pager));
            dialog.set_preview_overlay(overlay);
            dialog.set_preview_tint(tint);
            if let Some(n) = &backdrops {
                backdrop_rows(dialog, n);
            }
        }
        self.marks_preview = Some(preview);
    }

    /// OK answered: the values go into the draft and the edit is applied.
    fn finish_marks(&mut self, ctx: &egui::Context, kind: MarkKind, answer: &ParamResult) {
        let Some((index, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let current = self.views.get(index).map_or(0, |v| v.current());
        let selection = self.selected_pages(index);
        let units = MarginUnits::of(self.units);
        read_fields(&mut self.marks_draft, &answer.fields, kind, &units);
        let edit = edit(&self.marks_draft, kind, count, current, &selection);
        self.apply_edit(ctx, edit);
    }

    /// The file Browse picked, for the dialog that asked: dropped when that dialog is closed, or
    /// was opened again since (`serial`).
    fn marks_picked(&mut self, kind: MarkKind, serial: Option<u64>, name: String, bytes: Vec<u8>) {
        let id = dialog_id(dialog_key(kind));
        if self.marks_preview.is_none() || self.dialogs.open_serial(id) != serial {
            return;
        }
        let is_pdf = bytes.starts_with(b"%PDF");
        self.marks_draft.file = Some((name.clone(), Arc::new(bytes)));
        if let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) {
            dialog.set_field("file.path", FieldValue::Text(name));
            dialog.set_field("file.is_pdf", FieldValue::Bool(is_pdf));
        }
    }
}

impl MarksPreview {
    /// [`Self::unmarked_picture`] when the dialog is Update.
    fn unmarked_picture_for(
        &mut self,
        ctx: &egui::Context,
        doc: &pdfcraft_engine::Document,
        replace: bool,
        page: usize,
        near: &[usize],
    ) -> Option<PreviewState> {
        if replace { self.unmarked_picture(ctx, doc, page, near) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_applying_would_refuse_is_caught_in_the_dialog() {
        let mut d = MarksDraft::default();
        d.hf.text[5] = "Страница <<1>> из <<n>> / Page <<1>> of <<n>>".into();
        assert_eq!(undrawable(MarkKind::HeaderFooter, &d), Some('С'), "#2013: refused before OK, not after");
        d.hf.text[5] = "Page <<1>> of <<n>> — café".into();
        assert_eq!(undrawable(MarkKind::HeaderFooter, &d), None);
        d.wm.text = "ΑΠΟΡΡΗΤΟ".into();
        assert_eq!(undrawable(MarkKind::Watermark, &d), Some('Α'));
        d.use_file = true;
        assert_eq!(undrawable(MarkKind::Watermark, &d), None, "a picture watermark draws no text");
        assert_eq!(undrawable(MarkKind::Background, &d), None);
    }

    /// The runs a phrase lists, from "Pages 1-3, 6".
    fn runs(phrase: &str) -> Vec<String> {
        phrase.split(", ").map(|r| r.trim_start_matches("Pages ").trim_start_matches("Page ").to_string()).collect()
    }

    #[test]
    fn backdrop_messages_name_their_pages() {
        let phrase = |pages: &[usize]| pages_phrase(pages, "{pages}", "{pages}");
        // Pages arrive 0-based and are named 1-based.
        for n in [0usize, 4, 9] {
            let said = phrase(&[n]);
            assert!(said.contains(&(n + 1).to_string()) && !said.split(['-', ',', ' ']).any(|w| w == n.to_string()), "{said}");
        }
        assert_eq!(runs(&phrase(&[0, 1, 2])).len(), 1, "consecutive pages are one run");
        assert_eq!(runs(&phrase(&[0, 2, 4])).len(), 3);
        let long: Vec<usize> = (0..LISTED_PAGE_RUNS * 3).step_by(2).collect();
        let said = phrase(&long);
        assert!(said.ends_with('…'), "a long list ends with the cut mark: {said}");
        assert_eq!(said.trim_end_matches(", …").split(", ").count(), LISTED_PAGE_RUNS, "and shows exactly the limit");
        let exact: Vec<usize> = (0..LISTED_PAGE_RUNS * 2).step_by(2).collect();
        let said = phrase(&exact);
        assert!(!said.contains('…'), "exactly the limit has no cut mark: {said}");
        assert_eq!(said.split(", ").count(), LISTED_PAGE_RUNS);
        assert_eq!(pages_phrase(&[3], "one {pages}", "many {pages}"), "one 4");
        assert_eq!(pages_phrase(&[3, 5], "one {pages}", "many {pages}"), "many 4, 6");
    }

    #[test]
    fn a_background_tints_the_page_it_is_behind() {
        assert_eq!(behind([1.0, 0.0, 0.0], 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(behind([1.0, 0.0, 0.0], 0.0), Color32::WHITE);
        assert_eq!(behind([0.0, 0.0, 0.0], 0.5), Color32::from_rgb(128, 128, 128));
        assert_eq!(behind([0.2, 0.4, 0.6], f64::NAN), behind([0.2, 0.4, 0.6], 1.0), "opaque, as the engine has it");
    }

    #[test]
    fn the_preview_shows_a_background_only_where_ok_lets_it_show() {
        let mut d = MarksDraft::default();
        let colour = behind(d.bg.color, d.bg.opacity);
        assert_ne!(colour, Color32::WHITE);
        assert_eq!(background_tint(&d, None), colour, "no backdrop of its own");
        assert_eq!(background_tint(&d, Some(Backdrop::Fill)), colour, "its white backdrop is replaced");
        assert_eq!(background_tint(&d, Some(Backdrop::Image)), Color32::WHITE, "a picture covers it");
        d.bg.replace_backdrop = false;
        assert_eq!(background_tint(&d, Some(Backdrop::Fill)), Color32::WHITE, "its white backdrop stays and hides it");
        assert_eq!(background_tint(&d, None), colour, "unticked changes nothing without a backdrop");
    }

    #[test]
    fn the_watermark_edit_carries_the_offset() {
        let mut d = MarksDraft::default();
        let units = MarginUnits { per: 1.0, kind: UnitKind::Points, decimals: 1 };
        let mut fields = opening_fields(&d, MarkKind::Watermark, &units);
        fields.set("wm.text", FieldValue::Text("DRAFT".into()));
        fields.set("wm.offset.0", FieldValue::Number(-72.0));
        fields.set("wm.offset.1", FieldValue::Number(36.0));
        read_fields(&mut d, &fields, MarkKind::Watermark, &units);
        match edit(&d, MarkKind::Watermark, 3, 0, &[]) {
            Edit::AddWatermark { settings, .. } => assert_eq!(settings.offset, [-72.0, 36.0]),
            other => panic!("expected AddWatermark, got {other:?}"),
        }
    }
}
