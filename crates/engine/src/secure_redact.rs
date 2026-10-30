//! Secure redacted copy: the redaction that cannot leave hidden data behind, because nothing
//! from the source document reaches the output except pixels.
//!
//! Applying redactions in place ([`pdfcraft_redact::apply`]) removes what the content
//! interpreter finds under each mark, then cleans every other place a copy of the text can be
//! (metadata, tags, patterns, layers …). A place nobody thought of stays. A secure copy instead:
//!
//! 1. renders every page of the source with its form fields and links but no comments, with the
//!    reading colours off, and reads each drawn page's Redact annotations from the same reading
//!    of the file ([`PageRenderer::page_redactions`]): one reader decides both what is drawn
//!    and where the boxes go;
//! 2. paints each mark's rectangles straight into those pixels, in the mark's fill colour (white
//!    for a mark with none), grown by [`GROW_PX`] device pixels for anti-aliased edges. The
//!    rectangles go to pixels through the transform the renderer drew the page with
//!    ([`pdfcraft_render::PageGeometry`]: crop box, `/Rotate` and `/UserUnit` as it resolved
//!    them), so a box covers what the page drew there. Nothing from the source is interpreted
//!    a second time to place the boxes, so nothing in it (inherited resources, colour spaces,
//!    fonts, page tree shapes) can move or hide them. The marks of page N go on picture N;
//! 3. writes a new document holding one image per page, each page the displayed size of its
//!    source page, with an invisible OCR text layer read from those pixels when OCR is installed;
//! 4. reads that document back and refuses it unless it holds only what this module writes
//!    ([`check`]): no `/Info`, no metadata, outline, annotations, forms, structure or layers,
//!    one revision, no object a page does not use.
//!
//! The source document and its marks stay as they were. What a secure copy cannot do is find
//! what nobody marked: a word left unmarked stays on the page as pixels, and OCR reads it again.
//! [`SecureReport::still_found`] lists the marked words that OCR found again elsewhere.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_render::{PRINT_TAG, PageGeometry, PageRenderer, Redaction, RenderConfig, RenderRequest, RequestKind};

use crate::{DocId, EditError, Session, guard};

/// Device pixels each mark grows by on every side, so the anti-aliased edge of a glyph next to
/// the box is covered too.
pub const GROW_PX: f32 = 2.0;
/// The resolution pages are rendered at unless asked otherwise.
pub const DEFAULT_DPI: f32 = 300.0;
/// The largest output a secure copy may build (compressed image data and text layers). A
/// document past it fails with [`SecureError::TooLarge`] instead of exhausting memory.
pub const MAX_OUTPUT_BYTES: usize = 1 << 30;
/// The most rectangles all marks together may have (about 32 MB of them). A document past it
/// fails with [`SecureError::TooManyMarks`] instead of exhausting memory.
pub const MAX_MARK_RECTS: usize = 1_000_000;
/// Marked words shorter than this are not looked for again in the output (they match
/// everywhere).
const MIN_WORD: usize = 4;

/// How to make a secure copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecureOptions {
    /// Render resolution, clamped to 72–600. Large pages render at less (the renderer's caps).
    pub dpi: f32,
    /// Add an invisible OCR text layer, when OCR is installed.
    pub ocr: bool,
}

impl Default for SecureOptions {
    fn default() -> Self {
        Self { dpi: DEFAULT_DPI, ocr: true }
    }
}

/// Why a secure copy was not made. Nothing is produced when any of these happens.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SecureError {
    #[error("there are no redaction marks to apply")]
    NoMarks,
    #[error(
        "{0} redaction mark(s) have no readable area, so what they cover can't be known; nothing was made. Remove those marks or mark the content again"
    )]
    UnreadableMark(usize),
    #[error(
        "a secure redacted copy can't draw overlay text yet ({0} mark(s) have it); remove the overlay text from the marks, or apply the redactions in the document"
    )]
    OverlayText(usize),
    #[error("page {page} could not be rendered, so it can't be redacted safely: {reason}")]
    Render { page: usize, reason: String },
    #[error("the document's page tree can't be matched to its marks safely: {0}")]
    PageTree(String),
    #[error("the document has more than {MAX_MARK_RECTS} redaction areas, more than a secure copy takes")]
    TooManyMarks,
    #[error("the secure copy would be larger than {} MiB; redact fewer pages at once or choose a lower resolution", MAX_OUTPUT_BYTES >> 20)]
    TooLarge,
    #[error("the secure copy failed its own check, so it was not made: {0}")]
    Check(String),
    #[error("the secure copy could not be made: {0}")]
    Internal(String),
    #[error("stopped")]
    Cancelled,
}

/// One page of a secure copy.
#[derive(Clone, Debug, PartialEq)]
pub struct SecurePage {
    /// The resolution the page was rendered at (less than asked for a very large page).
    pub dpi: f32,
    pub marks: usize,
    /// Words in the page's new OCR text layer.
    pub words: usize,
    /// The renderer stopped reading the page's content early (its content budget), so some of
    /// it is not in the picture.
    pub truncated: bool,
}

/// A marked word that OCR read again in the secure copy: most likely the same text unmarked
/// elsewhere. OCR can misread, so this is a prompt to look, not proof.
#[derive(Clone, Debug, PartialEq)]
pub struct StillFound {
    pub text: String,
    /// 0-based pages of the copy it was read on.
    pub pages: Vec<usize>,
}

/// What a secure copy holds.
#[derive(Clone, Debug, PartialEq)]
pub struct SecureReport {
    pub pages: Vec<SecurePage>,
    pub marks: usize,
    /// The copy has an OCR text layer (OCR was asked for and is installed).
    pub searchable: bool,
    pub still_found: Vec<StillFound>,
    pub bytes: usize,
}

/// A finished secure copy.
pub struct SecureCopy {
    pub bytes: Vec<u8>,
    pub report: SecureReport,
}

/// Everything a secure copy needs from a document, detached from the session so it can run on
/// a worker thread.
pub struct SecureJob {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    /// How many pages the renderer draws.
    pages: usize,
    /// The marks, each with the page (the renderer's) it was read from.
    marks: Vec<(usize, Redaction)>,
    /// The marked words to look for again in the output, lower case.
    words: Vec<String>,
    pub options: SecureOptions,
    /// The name the copy opens under.
    pub name: String,
}

impl SecureJob {
    /// Pages to render, for progress.
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Make the copy. `ocr` reads the text layer (`None`: no text layer). `progress(done,
    /// total)` is called before each page; returning `false` stops with [`SecureError::Cancelled`].
    pub fn run(self, ocr: Option<&pdfcraft_ocr::Ocr>, mut progress: impl FnMut(usize, usize) -> bool) -> Result<SecureCopy, SecureError> {
        let total = self.pages;
        // The source itself, drawn without comments (form fields and links stay, as they print):
        // the same reading of the file the marks were read from, so picture N is page N.
        let config = RenderConfig { transparent: false, hide_comments: true, ..self.config.clone() };
        let mut page_r = PageRenderer::new(self.bytes.clone(), config);
        if page_r.page_count() != total {
            return Err(SecureError::PageTree("the document changed while the copy was being made".into()));
        }
        let dpi = if self.options.dpi.is_finite() { self.options.dpi.clamp(72.0, 600.0) } else { DEFAULT_DPI };
        let mut out = pdfcraft_cos::Document::new_empty();
        let pages_ref = page_tree(&out)?;
        let mut kids = Vec::with_capacity(total);
        let mut report =
            SecureReport { pages: Vec::with_capacity(total), marks: self.marks.len(), searchable: ocr.is_some(), still_found: Vec::new(), bytes: 0 };
        let mut found: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut size = 0usize;
        for page in 0..total {
            if !progress(page, total) {
                return Err(SecureError::Cancelled);
            }
            let render_error = |reason: String| SecureError::Render { page: page + 1, reason };
            let geometry = page_r.page_geometry(page, dpi / 72.0).ok_or_else(|| render_error("it has no page box to draw".into()))?;
            let scale = geometry.scale;
            let shot = page_r.render(RenderRequest { page, scale: dpi / 72.0, tag: PRINT_TAG, ..Default::default() });
            if let Some(reason) = shot.error {
                return Err(render_error(reason));
            }
            let (w, h) = (shot.width, shot.height);
            if (w, h) != (geometry.width, geometry.height) {
                return Err(render_error("the page was drawn differently than its geometry says".into()));
            }
            let mut rgba = shot.rgba.to_vec();
            crate::print_raster::over_white(&mut rgba, false);
            let mut marks = 0;
            for (_, m) in self.marks.iter().filter(|(on, _)| *on == page) {
                paint_mark(&mut rgba, &geometry, m)?;
                marks += 1;
            }
            let (words, (ow, oh)) = match ocr {
                Some(o) => {
                    let (small, ow, oh) = for_ocr(&rgba, w, h, scale);
                    let read = o.recognize(small.as_deref().unwrap_or(&rgba), ow, oh).map_err(|e| SecureError::Internal(e.to_string()))?;
                    (read.into_iter().flat_map(|l| l.words).collect(), (ow, oh))
                }
                None => (Vec::new(), (w, h)),
            };
            // The page as rendered: its pixels at the scale they were drawn at.
            let (pw, ph) = (f64::from(w) / f64::from(scale.max(1e-6)), f64::from(h) / f64::from(scale.max(1e-6)));
            let (sx, sy) = (pw / f64::from(ow.max(1)), ph / f64::from(oh.max(1)));
            let placed: Vec<pdfcraft_ocr::PlacedWord> =
                words.iter().map(|word| pdfcraft_ocr::PlacedWord::place(word, |x, y| [f64::from(x) * sx, ph - f64::from(y) * sy])).collect();
            for p in &placed {
                let text = p.text.to_lowercase();
                for wanted in self.words.iter().filter(|wanted| read_again(&text, wanted)) {
                    let pages = found.entry(wanted.clone()).or_default();
                    if pages.last() != Some(&page) {
                        pages.push(page);
                    }
                }
            }
            let kid = add_page(&mut out, pages_ref, pw, ph, w, h, &rgba, &placed, &mut size)?;
            kids.push(Object::Ref(kid));
            report.pages.push(SecurePage { dpi: scale * 72.0, marks, words: placed.len(), truncated: !shot.warnings.is_empty() });
        }
        progress(total, total);
        let count = kids.len() as i64;
        out.update_dict(pages_ref, |d| {
            d.set(b"Kids".to_vec(), Object::Array(kids));
            d.set(b"Count".to_vec(), Object::Int(count));
        })
        .map_err(|e| SecureError::Internal(e.to_string()))?;
        let bytes = guard(|| write_full(&out, &SaveOptions { object_streams: false, ..SaveOptions::default() }))
            .map_err(SecureError::Internal)?
            .map_err(|e| SecureError::Internal(e.to_string()))?;
        guard(|| check(&bytes)).map_err(SecureError::Internal)?.map_err(SecureError::Check)?;
        report.still_found = found.into_iter().map(|(text, pages)| StillFound { text, pages }).collect();
        report.bytes = bytes.len();
        Ok(SecureCopy { bytes, report })
    }
}

/// OCR read `wanted` (lower case) again in the word `read` (lower case): the whole of it, or
/// most of it (OCR misreads and drops letters, and the rest may be under a box): an alphanumeric
/// run of `read` of at least [`MIN_WORD`] characters and 70 % of `wanted` that `wanted` contains.
fn read_again(read: &str, wanted: &str) -> bool {
    if read.contains(wanted) {
        return true;
    }
    let need = (wanted.chars().count() * 7).div_ceil(10).max(MIN_WORD);
    read.split(|c: char| !c.is_alphanumeric()).any(|part| part.chars().count() >= need && wanted.contains(part))
}

/// The resolution OCR reads a page at: enough for text of 6 pt and up, and far less memory and
/// time than the picture's own resolution (OCR of a 300 dpi page took ten times as long and
/// several GB).
const OCR_DPI: f32 = 200.0;

/// The page picture (`w` × `h`, rendered at `scale` pixels per point) at [`OCR_DPI`]: a smaller
/// copy and its size, or `None` and the picture's own size when it is no larger.
fn for_ocr(rgba: &[u8], w: u32, h: u32, scale: f32) -> (Option<Vec<u8>>, u32, u32) {
    let k = OCR_DPI / 72.0 / scale.max(1e-3);
    if !(k.is_finite() && k < 0.95) {
        return (None, w, h);
    }
    let (ow, oh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let Some(img) = image::RgbaImage::from_raw(w, h, rgba.to_vec()) else { return (None, w, h) };
    let small = image::imageops::resize(&img, ow, oh, image::imageops::FilterType::Triangle);
    (Some(small.into_raw()), ow, oh)
}

/// Paint `mark`'s rectangles into `rgba` (opaque RGBA of `geometry`'s size), in its fill colour
/// (white when it has none: what is removed shows the paper). Each rectangle's corners go to
/// pixels through the renderer's transform; every pixel the box touches is painted, and
/// [`GROW_PX`] more on each side.
fn paint_mark(rgba: &mut [u8], geometry: &PageGeometry, mark: &Redaction) -> Result<(), SecureError> {
    let colour = mark.fill.unwrap_or([1.0; 3]).map(|v| (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 255.0).round() as u8);
    let (w, h) = (geometry.width as usize, geometry.height as usize);
    let grow = f64::from(GROW_PX);
    for &[x0, y0, x1, y1] in &mark.rects {
        let corners = [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|[x, y]| geometry.to_device(x, y));
        if corners.iter().flatten().any(|v| !v.is_finite()) {
            return Err(SecureError::UnreadableMark(1));
        }
        let lo = |i: usize| corners.iter().map(|c| c[i]).fold(f64::MAX, f64::min) - grow;
        let hi = |i: usize| corners.iter().map(|c| c[i]).fold(f64::MIN, f64::max) + grow;
        // Clamped to the raster before converting, so no value is out of range for usize.
        let span = |lo: f64, hi: f64, n: usize| (lo.floor().clamp(0.0, n as f64) as usize, hi.ceil().clamp(0.0, n as f64) as usize);
        let (xa, xb) = span(lo(0), hi(0), w);
        let (ya, yb) = span(lo(1), hi(1), h);
        for y in ya..yb {
            let row =
                rgba.get_mut(y * w * 4..(y + 1) * w * 4).ok_or_else(|| SecureError::Internal("the page raster is smaller than its size".into()))?;
            for px in row.as_chunks_mut::<4>().0.get_mut(xa..xb).unwrap_or_default() {
                *px = [colour[0], colour[1], colour[2], 255];
            }
        }
    }
    Ok(())
}

fn page_tree(doc: &pdfcraft_cos::Document) -> Result<ObjRef, SecureError> {
    doc.root()
        .and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages")))
        .ok_or_else(|| SecureError::Internal("the new document has no page tree".into()))
}

/// Add one image page of `pw` × `ph` points showing `rgba` (`w` × `h`, opaque), with `words` as
/// its invisible text layer.
#[allow(clippy::too_many_arguments)]
fn add_page(
    doc: &mut pdfcraft_cos::Document,
    parent: ObjRef,
    pw: f64,
    ph: f64,
    w: u32,
    h: u32,
    rgba: &[u8],
    words: &[pdfcraft_ocr::PlacedWord],
    size: &mut usize,
) -> Result<ObjRef, SecureError> {
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgba.len() / 4 * 3).map_err(|_| SecureError::TooLarge)?;
    for px in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut image = Dict::new();
    image.set(b"Type".to_vec(), Object::name("XObject"));
    image.set(b"Subtype".to_vec(), Object::name("Image"));
    image.set(b"Width".to_vec(), Object::Int(i64::from(w)));
    image.set(b"Height".to_vec(), Object::Int(i64::from(h)));
    image.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let image = Stream::flate(image, &rgb);
    drop(rgb);
    let mut content = format!("q {pw:.4} 0 0 {ph:.4} 0 0 cm /Im0 Do Q\n").into_bytes();
    if !words.is_empty() {
        content.extend(pdfcraft_ocr::text_layer(words));
    }
    let content = Stream::flate(Dict::new(), &content);
    *size = size.saturating_add(image.raw.len()).saturating_add(content.raw.len());
    if *size > MAX_OUTPUT_BYTES {
        return Err(SecureError::TooLarge);
    }
    let image = doc.add(Object::Stream(image));
    let content = doc.add(Object::Stream(content));
    let mut xobjects = Dict::new();
    xobjects.set(b"Im0".to_vec(), Object::Ref(image));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    if !words.is_empty() {
        let mut helv = Dict::new();
        helv.set(b"Type".to_vec(), Object::name("Font"));
        helv.set(b"Subtype".to_vec(), Object::name("Type1"));
        helv.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        helv.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let mut fonts = Dict::new();
        fonts.set(b"PCHelv".to_vec(), Object::Dict(helv));
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
    }
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(parent));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)]));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Ref(content));
    Ok(doc.add(page))
}

/// Keys each kind of object in a secure copy may have.
const TRAILER: &[&[u8]] = &[b"Root", b"Size", b"ID"];
const CATALOG: &[&[u8]] = &[b"Type", b"Pages"];
const PAGES: &[&[u8]] = &[b"Type", b"Kids", b"Count"];
const PAGE: &[&[u8]] = &[b"Type", b"Parent", b"MediaBox", b"Resources", b"Contents"];
const RESOURCES: &[&[u8]] = &[b"XObject", b"Font"];
const IMAGE: &[&[u8]] = &[b"Type", b"Subtype", b"Width", b"Height", b"ColorSpace", b"BitsPerComponent", b"Filter", b"Length"];
const CONTENT: &[&[u8]] = &[b"Filter", b"Length"];
const FONT: &[&[u8]] = &[b"Type", b"Subtype", b"BaseFont", b"Encoding"];

/// Read `bytes` back and accept them only if they hold exactly what [`SecureJob::run`] writes:
/// a catalog, one page tree, image pages with one image each and an optional standard
/// Helvetica text layer, in one revision, with no object that a page doesn't use. Any other
/// key or object is reported by name.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    let doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).map_err(|e| format!("it can't be read back: {e}"))?;
    if doc.revisions().len() != 1 || bytes.windows(5).filter(|w| *w == b"%%EOF").count() != 1 {
        return Err("it has more than one revision".into());
    }
    if doc.security().is_some() {
        return Err("it is encrypted".into());
    }
    let only = |what: &str, d: &Dict, allowed: &[&[u8]]| -> Result<(), String> {
        match d.iter().find(|(k, _)| !allowed.contains(&k.as_ref())) {
            Some((k, _)) => Err(format!("its {what} has /{}", String::from_utf8_lossy(k.as_ref()))),
            None => Ok(()),
        }
    };
    let name_is = |d: &Dict, key: &[u8], want: &[u8]| d.name(key) == Some(want);
    only("trailer", doc.trailer(), TRAILER)?;
    let mut seen: HashSet<u32> = HashSet::new();
    let mut take = |r: ObjRef, what: &str| -> Result<Arc<Object>, String> {
        if !seen.insert(r.num) {
            return Err(format!("its {what} (object {}) is used twice", r.num));
        }
        doc.try_get(r.num).map_err(|e| format!("its {what} (object {}) can't be read: {e}", r.num))
    };
    let root = doc.root().ok_or("it has no catalog")?;
    let catalog = take(root, "catalog")?;
    let catalog = dict_of(&catalog).ok_or("its catalog is not a dictionary")?;
    only("catalog", catalog, CATALOG)?;
    let pages_ref = catalog.reference(b"Pages").ok_or("its catalog has no page tree")?;
    let pages = take(pages_ref, "page tree")?;
    let pages = dict_of(&pages).ok_or("its page tree is not a dictionary")?;
    only("page tree", pages, PAGES)?;
    let kids = pages.get(b"Kids").and_then(Object::as_array).ok_or("its page tree has no pages")?;
    for kid in kids {
        let r = kid.as_ref().ok_or("a page is not an indirect object")?;
        let page = take(r, "page")?;
        let page = dict_of(&page).ok_or("a page is not a dictionary")?;
        only("page", page, PAGE)?;
        if !name_is(page, b"Type", b"Page") || page.reference(b"Parent") != Some(pages_ref) {
            return Err(format!("object {} is not a page of the page tree", r.num));
        }
        let content_ref = page.reference(b"Contents").ok_or("a page has no content stream")?;
        let content = take(content_ref, "page content")?;
        let content = stream_of(&content).ok_or("a page's content is not a stream")?;
        only("page content", &content.dict, CONTENT)?;
        let res = page.get(b"Resources").and_then(dict_of).ok_or("a page has no resources")?;
        only("page resources", res, RESOURCES)?;
        let xobjects = res.get(b"XObject").and_then(dict_of).ok_or("a page has no image")?;
        if xobjects.len() != 1 {
            return Err("a page has more than one XObject".into());
        }
        for (_, x) in xobjects.iter() {
            let r = x.as_ref().ok_or("a page's image is not an indirect object")?;
            let image = take(r, "image")?;
            let image = stream_of(&image).ok_or("a page's XObject is not a stream")?;
            only("image", &image.dict, IMAGE)?;
            if !name_is(&image.dict, b"Subtype", b"Image") || !name_is(&image.dict, b"ColorSpace", b"DeviceRGB") {
                return Err(format!("object {} is not an RGB image", r.num));
            }
        }
        if let Some(fonts) = res.get(b"Font") {
            let fonts = dict_of(fonts).ok_or("a page's fonts are not a dictionary")?;
            for (_, f) in fonts.iter() {
                let f = dict_of(f).ok_or("a font is not a direct dictionary")?;
                only("font", f, FONT)?;
                if !name_is(f, b"BaseFont", b"Helvetica") || !name_is(f, b"Subtype", b"Type1") || !name_is(f, b"Encoding", b"WinAnsiEncoding") {
                    return Err("a font is not the standard Helvetica".into());
                }
            }
        }
    }
    let extra: Vec<u32> = doc.object_numbers().into_iter().filter(|n| *n != 0 && !seen.contains(n)).collect();
    if let Some(n) = extra.first() {
        return Err(format!("object {n} is not used by any page ({} such object(s))", extra.len()));
    }
    Ok(())
}

/// A dictionary that is not a stream's.
fn dict_of(o: &Object) -> Option<&Dict> {
    match o {
        Object::Dict(d) => Some(d),
        _ => None,
    }
}

fn stream_of(o: &Object) -> Option<&Stream> {
    match o {
        Object::Stream(s) => Some(s),
        _ => None,
    }
}

/// The renderer's page count, the marks with their pages, and how many marks have no area.
type Marks = (usize, Vec<(usize, Redaction)>, usize);

impl Session {
    /// Capture what a secure redacted copy of document `id` needs. Refused when it has no
    /// marks, a mark without area or with overlay text, or when its security settings don't
    /// allow redaction (changing it) or copying its text and images.
    pub fn secure_copy_job(&self, id: DocId, options: SecureOptions) -> Result<SecureJob, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(p) = doc.editor.as_ref().and_then(|e| e.cos.permissions()) {
            crate::edit::check_permission(&crate::Edit::ApplyRedactions { pages: None }, &p)?;
        }
        doc.check_copying()?;
        let src = doc.export_source();
        // The pictures and the marks come from one reading of the file, the renderer's: each page
        // it draws, the Redact annotations of that very page dictionary. No page is matched by
        // number or identity across two readers, so a page listed twice, written in place, or
        // that another reader walks or resolves differently gets exactly its own marks.
        let (pages, marks, unreadable) = guard(|| -> Result<Marks, SecureError> {
            let r = PageRenderer::new(src.bytes.clone(), src.config.clone());
            let pages = r.page_count();
            let (mut marks, mut unreadable) = (Vec::new(), 0usize);
            let mut budget = MAX_MARK_RECTS;
            for page in 0..pages {
                let found = r.page_redactions(page, &mut budget).ok_or_else(|| SecureError::PageTree(format!("page {} can't be read", page + 1)))?;
                if found.exhausted {
                    return Err(SecureError::TooManyMarks);
                }
                unreadable += found.unreadable;
                marks.extend(found.marks.into_iter().map(|m| (page, m)));
            }
            Ok((pages, marks, unreadable))
        })
        .map_err(EditError::Invalid)??;
        if unreadable > 0 {
            return Err(SecureError::UnreadableMark(unreadable).into());
        }
        if marks.is_empty() {
            return Err(SecureError::NoMarks.into());
        }
        let overlays = marks.iter().filter(|(_, m)| m.overlay).count();
        if overlays > 0 {
            return Err(SecureError::OverlayText(overlays).into());
        }
        let words = guard(|| marked_words(&src.bytes, &src.config, &marks)).map_err(EditError::Invalid)?;
        let stem = doc.display_name();
        let stem = stem.strip_suffix(".pdf").or_else(|| stem.strip_suffix(".PDF")).unwrap_or(&stem);
        Ok(SecureJob { bytes: src.bytes, config: src.config, pages, marks, words, options, name: format!("{stem} (redacted).pdf") })
    }

    /// Make a secure redacted copy of document `id` and open it as a new, unsaved document.
    /// The source and its marks stay as they were. Reads text with OCR when `options.ocr` and
    /// OCR is installed.
    pub fn secure_redacted_copy(&mut self, id: DocId, options: SecureOptions) -> Result<(DocId, SecureReport), EditError> {
        let job = self.secure_copy_job(id, options)?;
        let ocr = if options.ocr && crate::ocr::available() { crate::ocr::engine().ok() } else { None };
        let name = job.name.clone();
        let copy = job.run(ocr.as_deref(), |_, _| true)?;
        let new = self.open_new(name, Arc::new(copy.bytes)).map_err(|e| EditError::Invalid(e.to_string()))?;
        Ok((new, copy.report))
    }
}

/// The words under the marks (lower case, at least [`MIN_WORD`] characters), read from the
/// page's own text layer.
fn marked_words(bytes: &Arc<Vec<u8>>, config: &RenderConfig, marks: &[(usize, Redaction)]) -> Vec<String> {
    let mut r = PageRenderer::new(bytes.clone(), config.clone());
    let mut out: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = marks.iter().map(|(page, _)| *page).collect();
    pages.sort_unstable();
    pages.dedup();
    for page in pages {
        // The text layer is in view space: points, y down, after /Rotate, which is the renderer's
        // raster at scale 1.
        let (Some(view), Some(text)) =
            (r.page_geometry(page, 1.0), r.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() }).text)
        else {
            continue;
        };
        let k = f64::from(view.scale.max(1e-6));
        for (_, m) in marks.iter().filter(|(on, _)| *on == page) {
            for rect in &m.rects {
                let corners = [[rect[0], rect[1]], [rect[2], rect[1]], [rect[0], rect[3]], [rect[2], rect[3]]].map(|[x, y]| view.to_device(x, y));
                let lo = |i: usize| (corners.iter().map(|c| c[i]).fold(f64::MAX, f64::min) / k) as f32;
                let hi = |i: usize| (corners.iter().map(|c| c[i]).fold(f64::MIN, f64::max) / k) as f32;
                let glyphs = text.glyphs_in([lo(0), lo(1), hi(0), hi(1)]);
                for word in text.column_text(&glyphs).split(|c: char| !c.is_alphanumeric()) {
                    let word = word.to_lowercase();
                    if word.chars().count() >= MIN_WORD && !out.contains(&word) {
                        out.push(word);
                    }
                }
            }
        }
    }
    out
}

impl From<SecureError> for EditError {
    fn from(e: SecureError) -> Self {
        EditError::SecureCopy(e)
    }
}

#[cfg(test)]
mod tests;
