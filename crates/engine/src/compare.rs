//! Compare files: the text differences between two open documents, a report, and the
//! differences marked as comments in the newer one.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::ObjRef;

pub use pdfcraft_compare::{Change, Comparison, Kind, Side};
// How far Office export reads into form XObjects; `OfficeExport::truncated_pages` lists the pages
// that draw more.
pub use pdfcraft_edit::{MAX_FORM_DEPTH, MAX_FORM_VISITS};

use crate::{DocId, Edit, EditError, Markup, NewAnnotation, NoteIcon, Session, Shape};

/// Most bytes of encoded pictures one Office export prepares, counting each image object once
/// (the Word and PowerPoint writers keep at most 512 MiB of them as well).
pub const MAX_EXPORT_IMAGE_BYTES: usize = 512 << 20;

/// Acrobat's compare colours: replaced blue, inserted green, deleted red.
pub fn colour(kind: Kind) -> crate::Rgb {
    match kind {
        Kind::Replaced => [0.2, 0.45, 0.95],
        Kind::Inserted => [0.2, 0.75, 0.3],
        Kind::Deleted => [0.9, 0.25, 0.25],
    }
}

impl crate::Document {
    /// Standards ▸ Verify PDF/A: the rules the document breaks for `level`.
    pub fn pdfa_verify(&self, level: pdfcraft_preflight::Level) -> Vec<pdfcraft_preflight::Issue> {
        self.editor.as_ref().map(|e| pdfcraft_preflight::verify(&e.cos, level)).unwrap_or_default()
    }

    /// The standards the document declares (Standards panel).
    pub fn standards(&self) -> pdfcraft_preflight::Declared {
        self.editor.as_ref().map(|e| pdfcraft_preflight::declared(&e.cos)).unwrap_or_default()
    }

    /// Every word of the document in reading order, with page and box.
    pub fn words(&self) -> Vec<pdfcraft_compare::Word> {
        let config = pdfcraft_render::RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        for (page, info) in self.info.pages.iter().enumerate() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(t) = res.text {
                out.extend(crate::js::page_words(&t, info).into_iter().map(|(text, rect)| pdfcraft_compare::Word { text, page, rect }));
            }
        }
        out
    }
}

impl Session {
    /// Compare files: the text differences from document `old` to document `new`. The result
    /// carries both documents' text, so both must allow copying it.
    pub fn compare(&self, old: DocId, new: DocId) -> Result<Comparison, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        a.check_copying()?;
        b.check_copying()?;
        Ok(pdfcraft_compare::compare(&a.words(), &b.words()))
    }

    /// [`Session::compare`], keeping the changes on `pages` of `new` (0-based; empty: every page).
    pub fn compare_pages(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Comparison, EditError> {
        let mut c = self.compare(old, new)?;
        if !pages.is_empty() {
            c.changes.retain(|ch| pages.iter().any(|page| (ch.new.page..=ch.new.last_page).contains(page)));
        }
        Ok(c)
    }

    /// Visual compare: regions where page n of `new` looks different from page n of `old`
    /// (rendered at `dpi`), as (page, user-space box in `new`).
    pub fn compare_visual(&self, old: DocId, new: DocId, dpi: f32) -> Result<Vec<(usize, [f64; 4])>, EditError> {
        let a = self.get(old).ok_or(EditError::NoDocument)?;
        let b = self.get(new).ok_or(EditError::NoDocument)?;
        let renderer = |d: &crate::Document| {
            pdfcraft_render::PageRenderer::new(
                d.bytes.clone(),
                pdfcraft_render::RenderConfig { password: d.password.as_deref().map(Arc::from), ..Default::default() },
            )
        };
        let (mut ra, mut rb) = (renderer(a), renderer(b));
        let scale = dpi.clamp(18.0, 150.0) / 72.0;
        let mut out = Vec::new();
        for (page, info) in b.info.pages.iter().enumerate().take(a.info.pages.len()) {
            let req = pdfcraft_render::RenderRequest { page, scale, ..Default::default() };
            let (x, y) = (ra.render(req), rb.render(req));
            if x.error.is_some() || y.error.is_some() {
                continue;
            }
            let s = y.width as f32 / info.width.max(1e-3);
            for r in pdfcraft_compare::visual_regions((&y.rgba, y.width, y.height), (&x.rgba, x.width, x.height), 24) {
                let p = info.view_to_user(r[0] as f32 / s, r[1] as f32 / s);
                let q = info.view_to_user(r[2] as f32 / s, r[3] as f32 / s);
                out.push((page, [p[0].min(q[0]) as f64, p[1].min(q[1]) as f64, p[0].max(q[0]) as f64, p[1].max(q[1]) as f64]));
            }
        }
        Ok(out)
    }

    /// The compare report as a new PDF (not opened).
    pub fn compare_report(&self, old: DocId, new: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        self.compare_report_of(old, new, &[])
    }

    /// [`Session::compare_report`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn compare_report_of(&self, old: DocId, new: DocId, pages: &[usize]) -> Result<Arc<Vec<u8>>, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let name = |id| self.get(id).map(|d| d.name.clone()).unwrap_or_default();
        self.create_report("Compare Report", &pdfcraft_compare::report(&c, &name(old), &name(new)))
    }

    /// Mark the differences in `new` as comments: highlights over replaced and inserted text
    /// (blue, green) and a note where text was deleted (red), authored "Compare". One undoable
    /// step; returns how many comments were added.
    pub fn mark_differences(&mut self, old: DocId, new: DocId) -> Result<usize, EditError> {
        self.mark_differences_of(old, new, &[])
    }

    /// [`Session::mark_differences`] of the changes on `pages` ([`Session::compare_pages`]).
    pub fn mark_differences_of(&mut self, old: DocId, new: DocId, pages: &[usize]) -> Result<usize, EditError> {
        let c = self.compare_pages(old, new, pages)?;
        let mut edits = Vec::new();
        for ch in &c.changes {
            let color = colour(ch.kind);
            let contents = match ch.kind {
                Kind::Replaced => format!("Replaced: \"{}\" with \"{}\"", ch.old.text, ch.new.text),
                Kind::Inserted => format!("Inserted: \"{}\"", ch.new.text),
                Kind::Deleted => format!("Deleted: \"{}\"", ch.old.text),
            };
            let shape = if ch.new.rects.is_empty() {
                let Some(r) = ch.new.near else { continue };
                Shape::Note { at: [r[0], r[3]], icon: NoteIcon::Note }
            } else {
                Shape::TextMarkup {
                    kind: Markup::Highlight,
                    quads: ch.new.rects.iter().map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]).collect(),
                }
            };
            let mut style = crate::Style::default_for(&shape);
            style.color = color;
            edits.push(Edit::AddAnnotation(NewAnnotation { page: ch.new.page, shape, style, contents, author: "Compare".into() }));
        }
        let n = edits.len();
        if n > 0 {
            self.apply(new, Edit::Batch { label: "Mark differences".into(), edits })?;
        }
        Ok(n)
    }
}

/// Export a PDF ▸ Word, PowerPoint, HTML or RTF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeFormat {
    Docx,
    Pptx,
    Html,
    /// CSS-free HTML that preserves text and inline images but drops colours and layout styling.
    HtmlSimple,
    Rtf,
    Markdown,
}

impl OfficeFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Pptx => "pptx",
            OfficeFormat::Html => "html",
            OfficeFormat::HtmlSimple => "html",
            OfficeFormat::Rtf => "rtf",
            OfficeFormat::Markdown => "md",
        }
    }

    pub fn from_extension(ext: &str) -> Option<OfficeFormat> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some(OfficeFormat::Docx),
            "pptx" => Some(OfficeFormat::Pptx),
            "html" | "htm" => Some(OfficeFormat::Html),
            "rtf" => Some(OfficeFormat::Rtf),
            "md" | "markdown" => Some(OfficeFormat::Markdown),
            _ => None,
        }
    }
}

/// A Word, PowerPoint, HTML, RTF or Markdown export ([`crate::Document::export_office_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfficeExport {
    /// The exported file.
    pub bytes: Vec<u8>,
    /// Pages (0-based) the export left something out of to stay bounded on hostile files: content
    /// past what one page may decode (its text is missing from `bytes`), forms nested deeper than
    /// [`MAX_FORM_DEPTH`] or more of them than [`MAX_FORM_VISITS`] (their text and images are
    /// missing), pictures past [`MAX_EXPORT_IMAGE_BYTES`] or past what
    /// one export may decode (those pictures are missing), or, on a page whose layout Word keeps,
    /// paths past [`pdfcraft_edit::MAX_PAGE_SHAPES`] or that couldn't be read, and shapes, points,
    /// fields or list items past what a Word page holds ([`pdfcraft_export::docx_checked`]: only
    /// those are missing; the page's text and images stay).
    pub truncated_pages: Vec<usize>,
}

/// An image object prepared for Office export: its file extension and encoded bytes, or why it is
/// left out (`Err(true)`: past [`MAX_EXPORT_IMAGE_BYTES`], or what the export may decode;
/// `Err(false)`: it can't be encoded).
type PreparedImage = Result<(&'static str, Arc<[u8]>), bool>;

/// A form field's widget as an exported field, its box on the visible page (`visible`).
fn export_field(
    cos: &pdfcraft_cos::Document,
    f: &pdfcraft_forms::Field,
    w: &pdfcraft_forms::Widget,
    visible: &dyn Fn([f64; 4]) -> [f64; 4],
    unit: f64,
    looks: &mut Looks,
) -> Option<pdfcraft_export::Field> {
    use pdfcraft_forms::{FieldKind, flags};
    let kind = match f.kind {
        FieldKind::Text => pdfcraft_export::FieldKind::Text {
            value: f.value.join(""),
            multiline: f.has(flags::MULTILINE),
            comb: f.is_comb().then_some(f.max_len).flatten(),
            max_len: f.max_len.filter(|n| *n > 0),
        },
        FieldKind::CheckBox => pdfcraft_export::FieldKind::Check { checked: button_on(f, w) },
        // A group's buttons are those the export keeps: shown, and on a page.
        FieldKind::Radio => pdfcraft_export::FieldKind::Radio {
            checked: button_on(f, w),
            buttons: f.widgets.iter().filter(|b| !b.hidden && b.page.is_some()).count(),
        },
        FieldKind::Combo | FieldKind::List => pdfcraft_export::FieldKind::Choice {
            value: f.display_value(),
            options: f.options.iter().map(|(_, shown)| shown.clone()).collect(),
            // A combo box that takes typed text as well as its choices.
            editable: f.kind == FieldKind::Combo && f.has(flags::EDIT),
        },
        FieldKind::PushButton | FieldKind::Signature => return None,
    };
    // The font size and border width are in the page's units, as its box is: `unit` scales them
    // as it scales the page's text and strokes (a page with `/UserUnit` 2 draws them twice as big).
    let (border, background) = widget_look(cos, w, looks);
    let border = border.map(|b| pdfcraft_export::Stroke { width: b.width * unit, ..b });
    let da = pdfcraft_forms::appearance::parse_da(&f.da);
    Some(pdfcraft_export::Field {
        name: f.name.clone(),
        rect: visible(w.rect),
        size: da.size * unit,
        font: dr_base_font(cos, &da.font),
        read_only: f.has(flags::READ_ONLY) || w.read_only,
        kind,
        border,
        background,
    })
}

/// Whether a check box or radio button `w` of `f` is on as the page shows it: its widget's
/// appearance state (`/AS`) is its on state, which is what a PDF viewer draws; for a widget with
/// no `/AS`, the field's value is.
fn button_on(f: &pdfcraft_forms::Field, w: &pdfcraft_forms::Widget) -> bool {
    let Some(on) = w.on_state.as_ref() else { return false };
    match w.state.as_ref() {
        Some(state) => state == on,
        None => f.value.iter().any(|v| v == on),
    }
}

/// The base font of the form's default resource font `name` (a field's `/DA` font, `Helv` for
/// `/Helvetica` say), or empty when the form has no such font; at most [`MAX_FONT_NAME_CHARS`]
/// characters of it.
fn dr_base_font(cos: &pdfcraft_cos::Document, name: &str) -> String {
    let base = || {
        let dr = cos.dict(pdfcraft_forms::acroform(cos)?.get(b"DR")?)?;
        let font = cos.dict(cos.dict(dr.get(b"Font")?)?.get(name.as_bytes())?)?;
        font.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).chars().take(MAX_FONT_NAME_CHARS).collect())
    };
    base().unwrap_or_default()
}

/// How much of a base font's name an exported field keeps: any real font's name, and a hostile
/// one can't grow each field.
const MAX_FONT_NAME_CHARS: usize = 127;

/// A colour array of `/MK` (ISO 32000-2 Table 192): none (transparent), gray, RGB or CMYK
/// components, as plain sRGB; `None` for none or anything else.
fn mk_colour(cos: &pdfcraft_cos::Document, o: Option<&pdfcraft_cos::Object>) -> Option<[f64; 3]> {
    let a = o.map(|o| cos.resolve(o))?;
    let v: Vec<f64> = a.as_array()?.iter().map(|x| cos.resolve(x).as_f64()).collect::<Option<_>>()?;
    let unit = |x: f64| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    match v.as_slice() {
        [g] => Some([unit(*g); 3]),
        [r, g, b] => Some([unit(*r), unit(*g), unit(*b)]),
        [c, m, y, k] => {
            let k = unit(*k);
            Some([(1.0 - unit(*c)) * (1.0 - k), (1.0 - unit(*m)) * (1.0 - k), (1.0 - unit(*y)) * (1.0 - k)])
        }
        _ => None,
    }
}

/// Most bytes of a widget's appearance read to tell whether it is drawn round.
const MAX_LOOK_BYTES: usize = 64 * 1024;

/// Most bytes of appearances one export reads to tell widgets' shapes, all widgets together.
const MAX_LOOKS_BYTES: usize = 16 << 20;

/// What the appearances an export has read say about their widgets' shapes, by appearance stream
/// (thousands of check boxes share two), and how many more bytes it may read: past
/// [`MAX_LOOKS_BYTES`] a widget's shape is the form reader's.
struct Looks {
    /// Per stream: drawn round or not, or `None` for one that draws nothing (or wasn't read).
    shapes: HashMap<ObjRef, Option<bool>>,
    bytes: usize,
}

impl Looks {
    fn new() -> Self {
        Self::within(MAX_LOOKS_BYTES)
    }

    /// Looks that read at most `bytes` bytes of appearances.
    fn within(bytes: usize) -> Self {
        Looks { shapes: HashMap::new(), bytes }
    }

    /// Whether appearance stream `r` draws a round shape, `None` when it draws nothing, can't be
    /// read, or is past what the export may read; each stream is read once.
    fn shape(&mut self, cos: &pdfcraft_cos::Document, r: ObjRef) -> Option<bool> {
        if let Some(shape) = self.shapes.get(&r) {
            return *shape;
        }
        let shape = if self.bytes == 0 {
            None
        } else {
            let limit = MAX_LOOK_BYTES.min(self.bytes);
            let data = match &*cos.resolve(&pdfcraft_cos::Object::Ref(r)) {
                pdfcraft_cos::Object::Stream(s) => s.decoded_within(limit).ok(),
                _ => None,
            };
            // A stream that couldn't be read within the limit cost the limit.
            self.bytes = self.bytes.saturating_sub(data.as_ref().map_or(limit, Vec::len));
            // A stream draws when it paints a path, shows text, or draws an XObject, shading or
            // image.
            let draws = |op: &[u8]| {
                matches!(op, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"Tj" | b"TJ" | b"'" | b"\"" | b"Do" | b"sh" | b"EI")
            };
            data.map(|d| pdfcraft_content::parse(&d).ops)
                .filter(|ops| ops.iter().any(|op| draws(&op.op)))
                .map(|ops| ops.iter().any(|op| matches!(op.op.as_slice(), b"c" | b"v" | b"y")))
        };
        self.shapes.insert(r, shape);
        shape
    }
}

/// The border and background a PDF viewer draws for a widget: `/MK /BC` in `/BS /W` points
/// (1 by default; dashed for `/S /D`) and `/MK /BG`. None for a widget drawn round (its
/// appearance curves, as a radio button's circle does; without an appearance, a radio button),
/// which a square Word frame can't draw, nor for a border of no width.
fn widget_look(cos: &pdfcraft_cos::Document, w: &pdfcraft_forms::Widget, looks: &mut Looks) -> (Option<pdfcraft_export::Stroke>, Option<[f64; 3]>) {
    let Some(d) = cos.dict(&pdfcraft_cos::Object::Ref(w.obj)) else { return (None, None) };
    let Some(mk) = d.get(b"MK").and_then(|o| cos.dict(o)) else { return (None, None) };
    // The shape is the one the widget shows: the normal appearance of its appearance state
    // (`/AS`), or its only one (a stream is always an indirect object). A state that draws nothing
    // (a radio button's empty off state), or none at all, leaves it to the form reader, which
    // takes every radio button as round.
    let stream_ref = |o: &pdfcraft_cos::Object| match o {
        pdfcraft_cos::Object::Ref(r) => Some(*r),
        _ => None,
    };
    let shown = d.get(b"AP").and_then(|o| cos.dict(o)).and_then(|ap| ap.get(b"N").cloned()).and_then(|n| match &*cos.resolve(&n) {
        pdfcraft_cos::Object::Dict(states) => w.state.as_ref().and_then(|state| states.get(state.as_bytes())).and_then(stream_ref),
        _ => stream_ref(&n),
    });
    let round = shown.and_then(|r| looks.shape(cos, r)).unwrap_or(w.round);
    if round {
        return (None, None);
    }
    let bs = d.get(b"BS").and_then(|o| cos.dict(o));
    let width = bs.as_ref().and_then(|b| b.get(b"W")).and_then(|o| cos.resolve(o).as_f64()).filter(|w| w.is_finite()).unwrap_or(1.0);
    let dashed = bs.as_ref().is_some_and(|b| b.name(b"S") == Some(b"D".as_slice()));
    let border = mk_colour(cos, mk.get(b"BC")).filter(|_| width > 0.0).map(|color| pdfcraft_export::Stroke { color, width: width.min(12.0), dashed });
    (border, mk_colour(cos, mk.get(b"BG")))
}

impl crate::Document {
    /// The pages as paragraphs and images (for Word, HTML and RTF export), including what
    /// form XObjects draw.
    pub fn export_pages(&self) -> Vec<pdfcraft_export::Page> {
        self.export_pages_checked(true, MAX_EXPORT_IMAGE_BYTES).0
    }

    /// [`Self::export_pages`], and the pages whose forms were left partly unread or whose
    /// pictures went past `image_budget` bytes ([`OfficeExport::truncated_pages`]). Without
    /// `images` no picture is prepared.
    pub(crate) fn export_pages_checked(&self, images: bool, image_budget: usize) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(editor) = self.editor.as_ref() else { return (Vec::new(), Vec::new()) };
        self.export_pages_within(images, image_budget, &mut pdfcraft_cos::ImageBudget::new(&editor.cos))
    }

    /// [`Self::export_pages_checked`], its pictures decoded within `image_work`, which is shared
    /// by the whole export: a picture past it is left out like one past `image_budget`.
    pub(crate) fn export_pages_within(
        &self,
        images: bool,
        image_budget: usize,
        image_work: &mut pdfcraft_cos::ImageBudget,
    ) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        self.export_pages_with(images, image_budget, image_work, true)
    }

    /// [`Self::export_pages_within`]; with `art` also the vector paths and form fields, which
    /// only Word uses (a limit that cuts the paths short truncates the page too).
    fn export_pages_with(
        &self,
        images: bool,
        image_budget: usize,
        image_work: &mut pdfcraft_cos::ImageBudget,
        art: bool,
    ) -> (Vec<pdfcraft_export::Page>, Vec<usize>) {
        let Some(cos) = self.editor.as_ref().map(|e| &e.cos) else { return (Vec::new(), Vec::new()) };
        let mut truncated = Vec::new();
        // Each image object is encoded once and shared by every place that draws it.
        let mut encoded: HashMap<ObjRef, PreparedImage> = HashMap::new();
        let mut image_bytes = image_budget;
        // What widgets' appearances say about their shapes, read once per stream for the export.
        let mut looks = Looks::new();
        // Form fields by page (0-based), read once: Word keeps them as content controls.
        let mut fields_by_page: HashMap<usize, Vec<(&pdfcraft_forms::Field, &pdfcraft_forms::Widget)>> = HashMap::new();
        let fields = if art { pdfcraft_forms::fields(cos) } else { Vec::new() };
        for f in &fields {
            if matches!(f.kind, pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature) {
                continue;
            }
            for w in f.widgets.iter().filter(|w| !w.hidden) {
                if let Some(p) = w.page {
                    fields_by_page.entry(p).or_default().push((f, w));
                }
            }
        }
        // In the page's tab order: a protected Word form tabs through its fields in body order.
        for on_page in fields_by_page.values_mut() {
            on_page.sort_by_key(|(_, w)| w.tab);
        }
        let pages = self
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(i, info)| {
                // Export uses the visible page in points, y up. Raw PDF coordinates include
                // CropBox offsets and precede /Rotate and /UserUnit.
                let visible_rect = |r: [f64; 4]| {
                    let a = info.user_to_view(r[0] as f32, r[1] as f32);
                    let b = info.user_to_view(r[2] as f32, r[3] as f32);
                    [a[0].min(b[0]) as f64, (info.height - a[1].max(b[1])) as f64, a[0].max(b[0]) as f64, (info.height - a[1].min(b[1])) as f64]
                };
                let crop_width = f64::from(info.crop[2] - info.crop[0]);
                let display_width = f64::from(if info.rotation % 180 == 0 { info.width } else { info.height });
                let unit = if crop_width > 0.0 && crop_width.is_finite() && display_width.is_finite() { display_width / crop_width } else { 1.0 };
                // Word also places a positioned page's text line by line. A page whose text can't be
                // read at all (its content is past the decode budget) lost it, and is listed.
                let unread = || pdfcraft_edit::ReadingBlocks { truncated: true, ..Default::default() };
                let (reading, lines) = if art {
                    pdfcraft_edit::reading_blocks_and_lines_checked(cos, i).unwrap_or_else(|_| (unread(), Vec::new()))
                } else {
                    (pdfcraft_edit::reading_blocks_checked(cos, i).unwrap_or_else(|_| unread()), Vec::new())
                };
                let forms_unread = reading.truncated;
                let to_block = |b: pdfcraft_edit::TextBlock| {
                    let f = b.base_font.to_ascii_lowercase();
                    // `text` is trimmed; the editor text keeps the source's own spaces.
                    let space_around = [b.editor_text.starts_with(char::is_whitespace), b.editor_text.ends_with(char::is_whitespace)];
                    pdfcraft_export::Block {
                        space_around,
                        font: pdfcraft_edit::strip_subset_tag(&b.base_font).to_string(),
                        text: b.text,
                        rect: visible_rect(b.rect),
                        size: b.size * unit,
                        bold: f.contains("bold") || f.contains("black") || f.contains("heavy"),
                        italic: f.contains("italic") || f.contains("oblique"),
                        color: b.color,
                        rotation: b.rotation - f64::from(info.rotation),
                    }
                };
                let blocks = reading.blocks.into_iter().filter(|b| !b.text.trim().is_empty()).map(to_block).collect();
                let lines = lines.into_iter().filter(|b| !b.text.trim().is_empty()).map(to_block).collect();
                let placed = if images { pdfcraft_edit::reading_images(cos, i).unwrap_or_default() } else { Vec::new() };
                let mut over_budget = false;
                let images = placed
                    .iter()
                    .filter_map(|im| {
                        let object = im.object?;
                        let prepared = encoded.entry(object).or_insert_with(|| {
                            let refused = image_work.refused();
                            let (ext, bytes) =
                                pdfcraft_create::image_file_within(cos, object, &mut *image_work).map_err(|_| image_work.refused() > refused)?;
                            image_bytes = image_bytes.checked_sub(bytes.len()).ok_or(true)?;
                            Ok((if ext == "jpg" { "jpg" } else { "png" }, Arc::from(bytes)))
                        });
                        over_budget |= matches!(prepared, Err(true));
                        let (ext, bytes) = prepared.clone().ok()?;
                        Some(pdfcraft_export::Image {
                            ext,
                            bytes,
                            rect: visible_rect(im.rect),
                            size: [im.matrix[0].hypot(im.matrix[1]) * unit, im.matrix[2].hypot(im.matrix[3]) * unit],
                            rotation: im.matrix[1].atan2(im.matrix[0]).to_degrees() - f64::from(info.rotation),
                            // A page's /Rotate turns the page; only the placement can mirror it.
                            flip: im.matrix[0] * im.matrix[3] - im.matrix[1] * im.matrix[2] < 0.0,
                        })
                    })
                    .collect();
                // A page whose paths can't be read at all (its content is past the decode budget)
                // lost them as surely as one cut short.
                let art_read = if art {
                    pdfcraft_edit::reading_shapes(cos, i).unwrap_or_else(|_| pdfcraft_edit::ReadingShapes { truncated: true, ..Default::default() })
                } else {
                    pdfcraft_edit::ReadingShapes::default()
                };
                // Paths left unread matter only to a page Word positions; the writer decides that.
                let art_truncated = art_read.truncated;
                if forms_unread || over_budget {
                    truncated.push(i);
                }
                // The page colour (Edit a PDF ▸ Background, a producer's page fill) for formats that keep one.
                let background = pdfcraft_edit::page_background(cos, i).ok().flatten();
                // Points on the visible page, y up (as the text and image boxes above).
                let visible_point = |p: [f64; 2]| {
                    let v = info.user_to_view(p[0] as f32, p[1] as f32);
                    [f64::from(v[0]), f64::from(info.height - v[1])]
                };
                let shapes = art_read
                    .shapes
                    .into_iter()
                    .map(|s| pdfcraft_export::Shape {
                        paths: s.subpaths.into_iter().map(|p| (p.points.into_iter().map(visible_point).collect(), p.closed)).collect(),
                        stroke: s.stroke.map(|st| pdfcraft_export::Stroke { color: st.color, width: st.width * unit, dashed: st.dashed }),
                        fill: s.fill,
                    })
                    .collect();
                // A Word page holds at most MAX_DOCX_PAGE_SHAPES fields: the rest aren't prepared
                // (nor their appearances read), and the page is listed as cut short.
                let fields = fields_by_page.get(&i).map_or_else(Vec::new, |on_page| {
                    if on_page.len() > pdfcraft_export::MAX_DOCX_PAGE_SHAPES {
                        truncated.push(i);
                    }
                    on_page
                        .iter()
                        .take(pdfcraft_export::MAX_DOCX_PAGE_SHAPES)
                        .filter_map(|(f, w)| export_field(cos, f, w, &visible_rect, unit, &mut looks))
                        .collect()
                });
                pdfcraft_export::Page {
                    width: info.width as f64,
                    height: info.height as f64,
                    blocks,
                    images,
                    background,
                    shapes,
                    fields,
                    lines,
                    art_truncated,
                }
            })
            .collect();
        (pages, truncated)
    }

    /// The document as a Word, PowerPoint, HTML or RTF file.
    pub fn export_office(&self, format: OfficeFormat) -> Vec<u8> {
        self.export_office_checked(format).bytes
    }

    /// [`Self::export_office`], and the pages whose text it had to leave out
    /// ([`OfficeExport::truncated_pages`]).
    pub fn export_office_checked(&self, format: OfficeFormat) -> OfficeExport {
        // RTF and Markdown leave pictures out.
        let (pages, mut truncated_pages) = match self.editor.as_ref() {
            Some(editor) => self.export_pages_with(
                !matches!(format, OfficeFormat::Rtf | OfficeFormat::Markdown),
                MAX_EXPORT_IMAGE_BYTES,
                &mut pdfcraft_cos::ImageBudget::new(&editor.cos),
                matches!(format, OfficeFormat::Docx),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let title = self.info.title.clone().unwrap_or_else(|| self.name.trim_end_matches(".pdf").to_string());
        let bytes = match format {
            OfficeFormat::Docx => {
                // The Word writer says which pages it had to cut short (shapes, points, fields, list items).
                let (bytes, dropped) = pdfcraft_export::docx_checked(&pages, &title);
                truncated_pages.extend(dropped);
                truncated_pages.sort_unstable();
                truncated_pages.dedup();
                bytes
            }
            OfficeFormat::Pptx => pdfcraft_export::pptx(&pages, &title),
            OfficeFormat::Html => pdfcraft_export::html(&pages, &title).into_bytes(),
            OfficeFormat::HtmlSimple => pdfcraft_export::html_simple(&pages, &title).into_bytes(),
            OfficeFormat::Rtf => pdfcraft_export::rtf(&pages).into_bytes(),
            OfficeFormat::Markdown => pdfcraft_export::markdown(&pages).into_bytes(),
        };
        OfficeExport { bytes, truncated_pages }
    }
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
