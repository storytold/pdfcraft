//! PDF export.
//!
//! Each slide is drawn by the DeckCraft renderer at print resolution (so the PDF looks exactly like
//! the slide, effects included) and overlaid with an invisible layer of real text from the text
//! layout engine with embedded font subsets, so the text can be searched, selected and copied.
//! Hyperlinks and shape actions become link annotations, slide titles become bookmarks.
//!
//! Layouts: one slide per page, notes pages (slide above its speaker notes) and handouts (1, 2, 3,
//! 4, 6 or 9 slides per page).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::collections::HashMap;

use craft_pdf::write::{self, Metadata, Writer, backend as krilla};

use deckcraft_fonts::FontDb;
use deckcraft_model::Presentation;
use deckcraft_model::text::{Action as ClickAction, safe_url};
use deckcraft_render::{PlacedText, RenderOpts};
use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::destination::XyzDestination;
use krilla::geom::{Point, Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::outline::{Outline, OutlineNode};
use krilla::page::PageSettings;
use krilla::paint::Fill;
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph, TextDirection};
use kurbo::{Affine, Rect};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("nothing to export (no slides in range)")]
    NoPages,
    #[error("PDF writer: {0}")]
    Write(String),
}

impl From<write::WriteError> for PdfError {
    fn from(error: write::WriteError) -> Self {
        Self::Write(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, PdfError>;

/// What each page shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PageLayout {
    /// One slide per page at the slide size.
    #[default]
    Slides,
    /// Portrait page: the slide above its notes.
    Notes,
    /// Portrait page with `per_page` slides (1, 2, 3, 4, 6 or 9).
    Handouts { per_page: u8 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PdfOptions {
    pub layout: PageLayout,
    /// Raster resolution of slide artwork (dots per inch), lowered for a slide whose raster would pass 64M pixels.
    pub dpi: f64,
    /// Slide indices to export (0-based); `None` = all.
    pub slides: Option<Vec<usize>>,
    pub include_hidden: bool,
    /// Invisible real-text layer (search, select, copy).
    pub text_layer: bool,
    /// Thin frame around each slide (notes/handouts).
    pub frame_slides: bool,
    /// Document title (defaults to the presentation title).
    pub title: Option<String>,
    /// Handout/notes page size in points (default US Letter portrait; see [`paper_size`]).
    pub paper: (f64, f64),
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            layout: PageLayout::Slides,
            dpi: 200.0,
            slides: None,
            include_hidden: false,
            text_layer: true,
            frame_slides: true,
            title: None,
            paper: (612.0, 792.0),
        }
    }
}

/// Paper sizes the Print page offers, as (id, label, width × height in points, portrait).
pub const PAPERS: [(&str, &str, (f64, f64)); 4] = [
    ("letter", "Letter (8.5 × 11 in)", (612.0, 792.0)),
    ("a4", "A4 (210 × 297 mm)", (595.28, 841.89)),
    ("legal", "Legal (8.5 × 14 in)", (612.0, 1008.0)),
    ("a3", "A3 (297 × 420 mm)", (841.89, 1190.55)),
];

/// The page size in points for a paper id from [`PAPERS`] (case-insensitive), turned on its side
/// when `landscape`. `None` for an unknown id.
pub fn paper_size(id: &str, landscape: bool) -> Option<(f64, f64)> {
    let (_, _, (w, h)) = PAPERS.iter().find(|p| p.0.eq_ignore_ascii_case(id.trim()))?;
    Some(if landscape { (*h, *w) } else { (*w, *h) })
}

/// The slide box on a notes page of `paper`: an inch of margin at the sides and the slide at most
/// half the page tall, so landscape paper still leaves room for the notes.
pub fn notes_slide_box(paper: (f64, f64), aspect: f64) -> Rect {
    let (pw, ph) = paper;
    let mut w = (pw - 2.0 * 72.0).max(1.0);
    let mut h = w / aspect.max(0.01);
    let max_h = (ph * 0.5 - 54.0).max(1.0);
    if h > max_h {
        h = max_h;
        w = h * aspect;
    }
    let x = (pw - w) / 2.0;
    Rect::new(x, 54.0, x + w, 54.0 + h)
}

/// Export `pres` as PDF bytes.
pub fn export(pres: &Presentation, opts: &PdfOptions) -> Result<Vec<u8>> {
    write::guard(|| export_inner(pres, opts))
}

fn plan_pages(pres: &Presentation, opts: &PdfOptions, slides: &[usize], sw: f64, sh: f64) -> Vec<Vec<(usize, Rect)>> {
    match opts.layout {
        PageLayout::Slides => slides.iter().map(|&i| vec![(i, Rect::new(0.0, 0.0, sw, sh))]).collect(),
        PageLayout::Notes => {
            let b = notes_frames(pres, opts.paper).0;
            slides.iter().map(|&i| vec![(i, b)]).collect()
        }
        PageLayout::Handouts { per_page } => {
            let boxes = handout_boxes(per_page, opts.paper, sw / sh);
            slides.chunks(boxes.len()).map(|c| c.iter().zip(&boxes).map(|(i, b)| (*i, *b)).collect()).collect()
        }
    }
}

fn build_outline(pres: &Presentation, pages: &[Vec<(usize, Rect)>]) -> Outline {
    let mut outline = Outline::new();
    for (p, v) in pages.iter().enumerate() {
        for (i, _) in v {
            let t = pres.slides[*i].title();
            let t = t.lines().next().unwrap_or("").trim().to_string();
            let label = if t.is_empty() { format!("Slide {}", i + 1) } else { format!("{}. {t}", i + 1) };
            outline.push_child(OutlineNode::new(label, XyzDestination::new(p, Point::from_xy(0.0, 0.0))));
        }
    }
    outline
}

fn export_inner(pres: &Presentation, opts: &PdfOptions) -> Result<Vec<u8>> {
    let slides: Vec<usize> = match &opts.slides {
        Some(v) => v.iter().copied().filter(|i| *i < pres.slides.len()).collect(),
        None => (0..pres.slides.len()).filter(|i| opts.include_hidden || !pres.slides[*i].hidden).collect(),
    };
    if slides.is_empty() {
        return Err(PdfError::NoPages);
    }
    let title = opts.title.clone().unwrap_or_else(|| pres.props.title.clone());
    let metadata = Metadata {
        creator: "DeckCraft".into(),
        producer: "DeckCraft".into(),
        title: (!title.trim().is_empty()).then_some(title),
        authors: if pres.props.author.trim().is_empty() { Vec::new() } else { vec![pres.props.author.clone()] },
        ..Default::default()
    };
    let mut writer = Writer::new(Default::default(), metadata);
    let pdf = writer.document_mut();

    let (sw, sh) = (pres.slide_size.width.max(1.0), pres.slide_size.height.max(1.0));
    let mut ex = Exporter { pres, opts, fonts: HashMap::new(), backdrops: Backdrops::default() };
    // Page plan: per page, the slides on it with their boxes (page points).
    let pages = plan_pages(pres, opts, &slides, sw, sh);
    // Bookmarks and slide → page lookup for internal links.
    let page_of: HashMap<usize, usize> = pages.iter().enumerate().flat_map(|(p, v)| v.iter().map(move |(i, _)| (*i, p))).collect();
    pdf.set_outline(build_outline(pres, &pages));

    for v in &pages {
        let (pw, ph) = match opts.layout {
            PageLayout::Slides => (sw, sh),
            _ => opts.paper,
        };
        let size = Size::from_wh(pw as f32, ph as f32).ok_or(PdfError::NoPages)?;
        let mut page = pdf.start_page_with(PageSettings::new(size));
        let mut annots = Vec::new();
        {
            let mut s = page.surface();
            for (i, b) in v {
                ex.slide(&mut s, *i, *b)?;
                annots.extend(ex.links(*i, *b, &page_of));
            }
            match opts.layout {
                PageLayout::Notes => {
                    if let Some((i, _)) = v.first() {
                        let notes = pres.slides[*i].notes_text();
                        ex.paragraphs(&mut s, &notes, notes_frames(pres, opts.paper).1, 12.0)?;
                    }
                    ex.page_texts(&mut s, pres.notes_master.as_deref(), pw, ph, v.first().map(|x| x.0 + 1).unwrap_or(1))?;
                }
                PageLayout::Handouts { per_page } => {
                    if per_page == 3 {
                        for (_, b) in v {
                            ex.note_lines(&mut s, Rect::new(b.x1 + 24.0, b.y0, pw - 54.0, b.y1));
                        }
                    }
                    ex.page_texts(&mut s, pres.handout_master.as_deref(), pw, ph, page_of.get(&v[0].0).map(|p| p + 1).unwrap_or(1))?;
                }
                PageLayout::Slides => {}
            }
            s.finish();
        }
        for a in annots {
            page.add_annotation(a);
        }
        page.finish();
    }
    writer.finish().map_err(Into::into)
}

/// A notes-size master's rect of placeholder `kind`, scaled to a page of `paper` points.
fn master_rect(pres: &Presentation, m: &deckcraft_model::Master, kind: deckcraft_model::PhType, paper: (f64, f64)) -> Option<Rect> {
    let x = m.shapes.iter().find(|s| s.ph_type() == Some(kind))?.xfrm?;
    let (kx, ky) = (paper.0 / pres.notes_size.width.max(1.0), paper.1 / pres.notes_size.height.max(1.0));
    let r = Rect::new(x.x * kx, x.y * ky, (x.x + x.w) * kx, (x.y + x.h) * ky);
    (r.width() > 1.0 && r.height() > 1.0).then_some(r)
}

/// A notes page's slide box and notes area on `paper`: where the notes master puts its slide image
/// and body placeholders, or the default layout when it has none.
pub fn notes_frames(pres: &Presentation, paper: (f64, f64)) -> (Rect, Rect) {
    let (pw, ph) = paper;
    let aspect = pres.slide_size.width.max(1.0) / pres.slide_size.height.max(1.0);
    let nm = pres.notes_master.as_deref();
    let slide = match nm.and_then(|m| master_rect(pres, m, deckcraft_model::PhType::SlideImage, paper)) {
        // The slide keeps its own shape, centred in the frame.
        Some(f) => {
            let (w, h) = if f.width() / f.height() > aspect { (f.height() * aspect, f.height()) } else { (f.width(), f.width() / aspect) };
            Rect::from_center_size(f.center(), (w, h))
        }
        None => notes_slide_box(paper, aspect),
    };
    let notes = nm
        .and_then(|m| master_rect(pres, m, deckcraft_model::PhType::Body, paper))
        .unwrap_or_else(|| Rect::new(72.0, slide.y1 + 36.0, pw - 72.0, ph - 54.0));
    (slide, notes)
}

/// How a master's header, date, footer or page number is printed: its own resolved formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: String,
    /// Points, before scaling to the paper.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: deckcraft_model::Rgba,
}

/// The resolved style of placeholder `sh`'s first run (or its paragraph's end properties) on
/// master `m`: run, list style, master text styles, theme fonts and colours.
pub fn master_text_style(pres: &Presentation, m: &deckcraft_model::Master, sh: &deckcraft_model::Shape) -> TextStyle {
    use deckcraft_model::resolve::{self, Ctx};
    let ctx = Ctx::for_master(pres, m);
    let para = sh.text.as_ref().and_then(|t| t.paragraphs.first()).cloned().unwrap_or_default();
    let own = para.runs.first().map(|r| r.props.clone()).unwrap_or_else(|| para.end_props.clone());
    let r = resolve::run(&ctx, sh, &para, &own);
    let family = r.font.as_deref().map(|f| if f.starts_with('+') { m.theme.font(f) } else { f.to_string() }).unwrap_or_default();
    let color = match &r.fill {
        Some(deckcraft_model::Fill::Solid { color }) => ctx.color(color, None),
        _ => deckcraft_model::Rgba::rgb(0x60, 0x60, 0x60),
    };
    TextStyle { family, size: r.size.unwrap_or(12.0).clamp(1.0, 400.0), bold: r.bold == Some(true), italic: r.italic == Some(true), color }
}

/// One laid-out piece of master text on a page of `paper` points: a run's text on one line, its
/// baseline start and its size, colour, weight and slant.
#[derive(Clone, Debug, PartialEq)]
pub struct TextPiece {
    pub at: (f64, f64),
    pub text: String,
    pub size: f64,
    pub color: deckcraft_model::Rgba,
    pub bold: bool,
    pub italic: bool,
}

/// The header, date, footer and page number of `m` for page `n`, laid out by the text engine
/// (wrapped, mixed formatting) and scaled to `paper`, for drawing without glyph access (the
/// Notes Page view).
pub fn master_text_pieces(pres: &Presentation, m: &deckcraft_model::Master, paper: (f64, f64), n: usize) -> Vec<TextPiece> {
    let (kx, ky) = (paper.0 / pres.notes_size.width.max(1.0), paper.1 / pres.notes_size.height.max(1.0));
    let mut out = vec![];
    for pt in deckcraft_render::place_master_texts(pres, m, n) {
        let mut para_chars: HashMap<usize, Vec<char>> = HashMap::new();
        for run in &pt.layout.runs {
            let Some(g) = run.glyphs.first() else { continue };
            let chars = para_chars.entry(run.para).or_insert_with(|| {
                pt.body.paragraphs.get(run.para).map(|p| p.runs.iter().flat_map(|r| r.content().chars()).collect()).unwrap_or_default()
            });
            let (a, b) = (run.chars.0.min(chars.len()), run.chars.1.min(chars.len()));
            let text: String = chars.get(a..b.max(a)).unwrap_or_default().iter().collect();
            if text.trim().is_empty() {
                continue;
            }
            let p = pt.transform * kurbo::Point::new(g.1, g.2);
            out.push(TextPiece {
                at: (p.x * kx, p.y * ky),
                text,
                size: run.size * ky,
                color: run.color,
                bold: run.fake_bold || run.face.weight >= 600.0,
                italic: run.fake_italic || run.face.italic,
            });
        }
    }
    out
}

/// The header, date, footer and page-number text a master puts on page `n`, with where it goes,
/// whether it's right-aligned and its style. Placeholders with no text are left out; the page
/// number is always `n`.
pub fn master_texts(pres: &Presentation, m: &deckcraft_model::Master, paper: (f64, f64), n: usize) -> Vec<(Rect, String, bool, TextStyle)> {
    use deckcraft_model::PhType;
    let mut out = vec![];
    for kind in [PhType::Header, PhType::Date, PhType::Footer, PhType::SlideNum] {
        let Some(r) = master_rect(pres, m, kind, paper) else { continue };
        let Some(sh) = m.shapes.iter().find(|s| s.ph_type() == Some(kind)) else { continue };
        let text = if kind == PhType::SlideNum { n.to_string() } else { sh.text.as_ref().map(|t| t.text()).unwrap_or_default().trim().to_string() };
        if text.is_empty() {
            continue;
        }
        let right = sh
            .text
            .as_ref()
            .and_then(|t| t.paragraphs.first())
            .and_then(|p| p.props.align)
            .map_or(matches!(kind, PhType::Date | PhType::SlideNum), |a| a == deckcraft_model::text::Align::Right);
        let mut style = master_text_style(pres, m, sh);
        // Sizes scale with the page, as the placeholders do.
        style.size *= paper.1 / pres.notes_size.height.max(1.0);
        out.push((r, text, right, style));
    }
    out
}

/// Slide boxes for a handout page (portrait paper, half-inch-ish margins).
pub fn handout_boxes(per_page: u8, paper: (f64, f64), aspect: f64) -> Vec<Rect> {
    let (pw, ph) = paper;
    let (mx, top, bottom) = (54.0, 54.0, 54.0);
    let (cols, rows, notes) = match per_page {
        0 | 1 => (1, 1, false),
        2 => (1, 2, false),
        3 => (1, 3, true),
        4 => (2, 2, false),
        6 => (2, 3, false),
        _ => (3, 3, false),
    };
    let gap = 18.0;
    let aw = if notes { (pw - 2.0 * mx) * 0.5 } else { pw - 2.0 * mx };
    let ah = ph - top - bottom - 18.0;
    let cw = (aw - gap * (cols as f64 - 1.0)) / cols as f64;
    let ch = (ah - gap * (rows as f64 - 1.0)) / rows as f64;
    let (w, h) = if cw / ch > aspect { (ch * aspect, ch) } else { (cw, cw / aspect) };
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let cx = mx + c as f64 * (cw + gap) + (cw - w) / 2.0;
            let cy = top + r as f64 * (ch + gap) + (ch - h) / 2.0;
            let cx = if notes { mx } else { cx };
            out.push(Rect::new(cx, cy, cx + w, cy + h));
        }
    }
    out
}

struct Exporter<'a> {
    pres: &'a Presentation,
    opts: &'a PdfOptions,
    fonts: HashMap<u32, Font>,
    backdrops: Backdrops,
}

fn tf(a: Affine) -> Transform {
    let c = a.as_coeffs();
    Transform::from_row(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32, c[4] as f32, c[5] as f32)
}

fn grey(v: u8, opacity: f32) -> Fill {
    Fill {
        paint: krilla::color::rgb::Color::new(v, v, v).into(),
        opacity: NormalizedF32::new(opacity.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE),
        ..Default::default()
    }
}

/// Premultiplied RGBA (the renderer's pixels) to the straight RGBA the writer stores. Runs of
/// pixels that are all fully transparent or fully opaque, most of a slide, are skipped in blocks.
fn straight(rgba: &mut [u8]) {
    let (blocks, rest) = rgba.as_chunks_mut::<64>();
    for block in blocks {
        if block.as_chunks::<4>().0.iter().fold(false, |m, px| m | (px[3].wrapping_sub(1) < 254)) {
            straight_px(block);
        }
    }
    straight_px(rest);
}

fn straight_px(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = (((*c as u32 * 255 + a / 2) / a).min(255)) as u8;
            }
        }
    }
}

/// A backdrop raster composited onto white (slides are normally opaque, and opaque blocks are skipped).
fn opaque(rgba: &mut [u8]) {
    let (blocks, rest) = rgba.as_chunks_mut::<64>();
    for block in blocks {
        if !block.as_chunks::<4>().0.iter().fold(true, |m, px| m & (px[3] == 255)) {
            opaque_px(block);
        }
    }
    opaque_px(rest);
}

fn opaque_px(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let k = 255 - px[3];
        px[0] = px[0].saturating_add(k);
        px[1] = px[1].saturating_add(k);
        px[2] = px[2].saturating_add(k);
        px[3] = 255;
    }
}

/// Gives every fully transparent content pixel the backdrop's colour. The renderer leaves them
/// RGB 0,0,0; a viewer that smooths the image blends colour and mask separately, so the black
/// shows as a grey hairline around white pictures and shapes (#1455).
fn fill_clear(front: &mut [u8], back: &[u8]) {
    for (f, b) in front.as_chunks_mut::<4>().0.iter_mut().zip(back.as_chunks::<4>().0) {
        if f[3] == 0 {
            f[..3].copy_from_slice(&b[..3]);
        }
    }
}

/// Most bytes of opaque backdrop rasters kept to recognise a backdrop seen before.
const BACKDROP_CACHE_BYTES: usize = 32 << 20;

/// Backdrops already handed to the writer, most recent first, with their pixels. Slides that
/// share a layout render the same backdrop; reusing its image skips hashing and compressing the
/// raster again, and the writer stores it once either way (#845).
#[derive(Default)]
struct Backdrops(Vec<(Vec<u8>, u32, u32, Image)>);

impl Backdrops {
    fn image(&mut self, mut img: deckcraft_render::Image) -> Image {
        opaque(&mut img.pixels);
        let (w, h) = (img.width, img.height);
        if let Some(i) = self.0.iter().position(|(px, pw, ph, _)| (*pw, *ph) == (w, h) && *px == img.pixels) {
            let hit = self.0.remove(i);
            let image = hit.3.clone();
            self.0.insert(0, hit);
            return image;
        }
        if img.pixels.len() > BACKDROP_CACHE_BYTES {
            return Image::from_rgba8(img.pixels, w, h);
        }
        let image = Image::from_rgba8(img.pixels.clone(), w, h);
        self.0.insert(0, (img.pixels, w, h, image.clone()));
        let mut total = 0;
        self.0.retain(|(px, ..)| {
            total += px.len();
            total <= BACKDROP_CACHE_BYTES
        });
        image
    }
}

/// Most pixels one slide raster may have; it is RGBA, and the backdrop and the content are two rasters.
const MAX_RASTER_PIXELS: f64 = 64_000_000.0;

/// Most ruled note lines one handout area gets (a letter page holds about 30).
const MAX_NOTE_LINES: usize = 500;

/// Render scale for a slide of `slide` units drawn `box_width` points wide at `dpi`, with the total pixel count capped.
fn raster_scale(box_width: f64, slide: (f64, f64), dpi: f64) -> f64 {
    let scale = (box_width / 72.0 * dpi.clamp(36.0, 600.0)) / slide.0;
    let pixels = slide.0 * scale * slide.1 * scale;
    if pixels > MAX_RASTER_PIXELS { scale * (MAX_RASTER_PIXELS / pixels).sqrt() } else { scale }
}

impl Exporter<'_> {
    fn font(&mut self, face: &deckcraft_fonts::FontFace) -> Result<Font> {
        if let Some(font) = self.fonts.get(&face.id()) {
            return Ok(font.clone());
        }
        let font = write::font(face.data(), face.index(), None)?;
        self.fonts.insert(face.id(), font.clone());
        Ok(font)
    }

    /// Slide `i` drawn into `b` (page points).
    fn slide(&mut self, s: &mut Surface, i: usize, b: Rect) -> Result<()> {
        let pres = self.pres;
        let (sw, sh) = (pres.slide_size.width.max(1.0), pres.slide_size.height.max(1.0));
        let k = b.width() / sw;
        // Raster at the requested dpi relative to the box size on paper.
        let scale = raster_scale(b.width(), (sw, sh), self.opts.dpi);
        // One clock reading for the raster and the text layer, so a date field cannot differ between them.
        let ropts = RenderOpts { scale, threads: 4, now: deckcraft_text::datetime::DateTime::now_local(), ..Default::default() };
        // Two rasters per slide: the backdrop (background, master and layout graphics) is the same
        // on every slide that shares its layout, so the writer stores it once, and the slide's own
        // shapes draw over it (#845).
        let mut back = deckcraft_render::render_slide_part(pres, i, deckcraft_render::SlidePart::Backdrop, &ropts);
        opaque(&mut back.pixels);
        let front = deckcraft_render::render_slide_part(pres, i, deckcraft_render::SlidePart::Content, &ropts);
        let (w, h) = (back.width, back.height);
        if w > 0
            && h > 0
            && let Some(size) = Size::from_wh(b.width() as f32, b.height() as f32)
        {
            s.push_transform(&Transform::from_translate(b.x0 as f32, b.y0 as f32));
            // Taken, not copied: a print-resolution slide is ~16 MB.
            let mut rgba = front.pixels;
            let content = rgba.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
            if content {
                fill_clear(&mut rgba, &back.pixels);
                straight(&mut rgba);
            }
            s.draw_image(self.backdrops.image(back), size);
            if content {
                s.draw_image(Image::from_rgba8(rgba, w, h), size);
            }
            s.pop();
        }
        if self.opts.frame_slides && self.opts.layout != PageLayout::Slides {
            let p = kurbo::Shape::to_path(&b, 0.1);
            if let Some(path) = to_path(&p) {
                s.set_fill(None);
                s.set_stroke(Some(krilla::paint::Stroke {
                    paint: krilla::color::rgb::Color::new(0x80, 0x80, 0x80).into(),
                    width: 0.5,
                    ..Default::default()
                }));
                s.draw_path(&path);
                s.set_stroke(None);
            }
        }
        if self.opts.text_layer {
            let view = Affine::translate((b.x0, b.y0)) * Affine::scale(k);
            for pt in deckcraft_render::place_slide_with(pres, i, &ropts).texts {
                self.text_layer(s, &pt, view, false)?;
            }
            let _ = sh;
        }
        Ok(())
    }

    /// Invisible glyphs exactly over the rasterised text.
    fn text_layer(&mut self, s: &mut Surface, pt: &PlacedText, view: Affine, visible: bool) -> Result<()> {
        // Each paragraph's characters, gathered once: a long paragraph wraps into many runs.
        let mut para_chars: HashMap<usize, Vec<char>> = HashMap::new();
        for run in &pt.layout.runs {
            if run.glyphs.is_empty() || run.size <= 0.0 {
                continue;
            }
            let chars = para_chars.entry(run.para).or_insert_with(|| {
                pt.body.paragraphs.get(run.para).map(|p| p.runs.iter().flat_map(|r| r.content().chars()).collect()).unwrap_or_default()
            });
            let (a, b) = (run.chars.0.min(chars.len()), run.chars.1.min(chars.len()));
            let text: String = chars.get(a..b.max(a)).unwrap_or_default().iter().collect();
            if text.trim().is_empty() {
                continue;
            }
            let font = self.font(&run.face)?;
            let n = run.glyphs.len();
            let ranges = glyph_text_ranges(&text, run.chars, &run.cells, n);
            let (x0, y0) = (run.glyphs[0].1, run.glyphs[0].2);
            let kg: Vec<KrillaGlyph> = run
                .glyphs
                .iter()
                .enumerate()
                .map(|(gi, g)| {
                    let next = run.glyphs.get(gi + 1).map_or(g.1 + run.face.advance(g.0) * run.size / run.face.upem.max(1.0), |nx| nx.1);
                    let range = ranges.get(gi).cloned().unwrap_or(text.len()..text.len());
                    KrillaGlyph::new(GlyphId::new(g.0), ((next - g.1) / run.size) as f32, 0.0, ((g.2 - y0) / run.size) as f32, 0.0, range, None)
                })
                .collect();
            s.push_transform(&tf(view * pt.transform * Affine::translate((x0, y0))));
            s.set_fill(Some(if visible {
                let c = run.color;
                Fill {
                    paint: krilla::color::rgb::Color::new(c.r, c.g, c.b).into(),
                    opacity: NormalizedF32::new((f64::from(c.a) / 255.0 * run.alpha) as f32).unwrap_or(NormalizedF32::ONE),
                    ..Default::default()
                }
            } else {
                grey(0, 0.0)
            }));
            s.draw_glyphs(Point::from_xy(0.0, 0.0), &kg, font, &text, run.size as f32, false);
            s.set_fill(None);
            s.pop();
        }
        Ok(())
    }

    fn links(&self, i: usize, b: Rect, page_of: &HashMap<usize, usize>) -> Vec<Annotation> {
        let pres = self.pres;
        let k = b.width() / pres.slide_size.width.max(1.0);
        let view = Affine::translate((b.x0, b.y0)) * Affine::scale(k);
        let mut out = Vec::new();
        for l in deckcraft_render::place_slide(pres, i).links {
            let r = view.transform_rect_bbox(l.rect).intersect(b);
            let Some(rect) = krilla::geom::Rect::from_ltrb(r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32) else { continue };
            let goto = |slide: Option<usize>| {
                slide.and_then(|s| page_of.get(&s)).map(|p| Target::Destination(XyzDestination::new(*p, Point::from_xy(0.0, 0.0)).into()))
            };
            let target = match &l.link.action {
                // Only the links the slide show opens (web and mail): a reader of the file gets the rest
                // as plain text.
                ClickAction::Url { url } if safe_url(url) => Some(Target::Action(Action::Link(LinkAction::new(url.clone())))),
                ClickAction::Slide { slide } => goto(pres.slides.iter().position(|x| x.id == *slide)),
                ClickAction::NextSlide => goto(Some(i + 1)),
                ClickAction::PreviousSlide => goto(i.checked_sub(1)),
                ClickAction::FirstSlide => goto(Some(0)),
                ClickAction::LastSlide => goto(pres.slides.len().checked_sub(1)),
                _ => None,
            };
            if let Some(t) = target {
                let alt = if l.link.tooltip.is_empty() { None } else { Some(l.link.tooltip.clone()) };
                out.push(Annotation::new_link(LinkAnnotation::new(rect, t), alt));
            }
        }
        out
    }

    fn ui_font(&mut self) -> Result<(Font, std::sync::Arc<deckcraft_fonts::FontFace>)> {
        let face = FontDb::global().face(deckcraft_fonts::DEFAULT_FAMILY, "Regular");
        let f = self.font(&face)?;
        Ok((f, face))
    }

    /// Wrapped plain text (notes).
    fn paragraphs(&mut self, s: &mut Surface, text: &str, area: Rect, size: f64) -> Result<()> {
        let (font, face) = self.ui_font()?;
        let k = size / face.upem.max(1.0);
        let width = |w: &str| w.chars().map(|c| face.advance(face.glyph_for(c)) * k).sum::<f64>();
        let lh = size * 1.3;
        let mut y = area.y0 + size;
        s.set_fill(Some(grey(0x20, 1.0)));
        let space = width(" ");
        'outer: for para in text.split('\n') {
            // The line's width is kept as words are added, not measured again for each word.
            let mut line = String::new();
            let mut line_w = 0.0;
            for word in para.split(' ') {
                let word_w = width(word);
                let cand_w = if line.is_empty() { word_w } else { line_w + space + word_w };
                if cand_w > area.width() && !line.is_empty() {
                    if y > area.y1 {
                        break 'outer;
                    }
                    s.draw_text(Point::from_xy(area.x0 as f32, y as f32), font.clone(), size as f32, &line, false, TextDirection::Auto);
                    y += lh;
                    line = word.to_string();
                    line_w = word_w;
                } else {
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(word);
                    line_w = cand_w;
                }
            }
            if y > area.y1 {
                break;
            }
            if !line.is_empty() {
                s.draw_text(Point::from_xy(area.x0 as f32, y as f32), font.clone(), size as f32, &line, false, TextDirection::Auto);
            }
            y += lh;
        }
        s.set_fill(None);
        Ok(())
    }

    /// Ruled lines for handwritten notes (3-per-page handouts).
    fn note_lines(&self, s: &mut Surface, r: Rect) {
        // Counted, not stepped: at huge coordinates adding 22 leaves y unchanged, and a huge page
        // would want millions of rules.
        let first = r.y0 + 18.0;
        let count = ((r.y1 - first) / 22.0).floor() + 1.0;
        if count.is_nan() || count < 1.0 {
            return;
        }
        s.set_stroke(Some(krilla::paint::Stroke {
            paint: krilla::color::rgb::Color::new(0xB0, 0xB0, 0xB0).into(),
            width: 0.5,
            ..Default::default()
        }));
        for i in 0..(count.min(MAX_NOTE_LINES as f64) as usize) {
            let y = first + 22.0 * i as f64;
            let mut p = kurbo::BezPath::new();
            p.move_to((r.x0, y));
            p.line_to((r.x1, y));
            if let Some(path) = to_path(&p) {
                s.draw_path(&path);
            }
        }
        s.set_stroke(None);
    }

    /// A notes or handout page's header, date, footer and page number from its master; just the
    /// page number when the deck has no such master.
    fn page_texts(&mut self, s: &mut Surface, master: Option<&deckcraft_model::Master>, pw: f64, ph: f64, n: usize) -> Result<()> {
        let Some(m) = master else { return self.footer(s, pw, ph, n) };
        // Laid out by the text engine in the placeholders, scaled from the notes size to the page:
        // wrapping, alignment and mixed formatting as the master has them.
        let pres = self.pres;
        let view = Affine::scale_non_uniform(pw / pres.notes_size.width.max(1.0), ph / pres.notes_size.height.max(1.0));
        for pt in deckcraft_render::place_master_texts(pres, m, n) {
            self.text_layer(s, &pt, view, true)?;
        }
        Ok(())
    }
    fn footer(&mut self, s: &mut Surface, pw: f64, ph: f64, n: usize) -> Result<()> {
        let (font, _) = self.ui_font()?;
        s.set_fill(Some(grey(0x60, 1.0)));
        s.draw_text(Point::from_xy((pw - 72.0) as f32, (ph - 30.0) as f32), font, 9.0, &n.to_string(), false, TextDirection::Auto);
        s.set_fill(None);
        Ok(())
    }
}

/// Byte range in `text` (the run's characters `chars`) of each of `n` glyphs, for the ToUnicode map.
/// `cells` holds each glyph's shaping-cluster head (paragraph character index); a cluster runs to the
/// next head in the run, so a ligature maps to all its characters and every glyph of a cluster shares
/// its range (krilla then writes the text once).
fn glyph_text_ranges(text: &str, chars: (usize, usize), cells: &[(usize, char)], n: usize) -> Vec<std::ops::Range<usize>> {
    let starts: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let byte = |ci: usize| ci.checked_sub(chars.0).and_then(|i| starts.get(i).copied()).unwrap_or(text.len());
    let mut heads: Vec<usize> = cells.iter().map(|c| c.0).collect();
    heads.sort_unstable();
    heads.dedup();
    let end = |ci: usize| heads.get(heads.partition_point(|&h| h <= ci)).copied().unwrap_or(chars.1);
    (0..n).map(|gi| cells.get(gi).map_or(text.len()..text.len(), |&(ci, _)| byte(ci)..byte(end(ci)))).collect()
}

fn to_path(bp: &kurbo::BezPath) -> Option<krilla::geom::Path> {
    use kurbo::PathEl;
    let mut pb = krilla::geom::PathBuilder::new();
    for el in bp.elements() {
        match *el {
            PathEl::MoveTo(p) => pb.move_to(p.x as f32, p.y as f32),
            PathEl::LineTo(p) => pb.line_to(p.x as f32, p.y as f32),
            PathEl::QuadTo(a, p) => pb.quad_to(a.x as f32, a.y as f32, p.x as f32, p.y as f32),
            PathEl::CurveTo(a, b, p) => pb.cubic_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32, p.x as f32, p.y as f32),
            PathEl::ClosePath => pb.close(),
        }
    }
    pb.finish()
}

#[cfg(test)]
mod tests;
