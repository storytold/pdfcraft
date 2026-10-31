//! Type 3 font synthesis for a vectorized master glyph (ClearScan stage 5).
//!
//! [`synthesize_type3`] writes one simple Type 3 font into a [`pdfcraft_cos::Document`]. Each
//! glyph is a PDF content stream of the outline from [`crate::vectorize`]: `d1` for the advance
//! and the ink box, then `m` / `l` / `c` / `h`, then `f`. The outlines are uncolored, so they
//! take the fill colour of the text that shows them. `d0` is the other width operator, for a
//! glyph procedure that sets its own colour; nothing here does.
//!
//! Glyph space is 1000 units per em, and [`em_pixels`] (cap height, else the ascent, else one
//! pixel) is the em. The page writer uses that same value for `Tf`. [`Type3Glyph::baseline`] is a
//! y in the prototype bitmap, y down. The bitmap's left edge is x = 0 and that baseline is y = 0
//! after the flip into PDF's y-up glyph space, so a font size equal to the em, in user units,
//! draws one prototype pixel as one user unit.
//! [`baseline_in_prototype`] turns a page baseline into that bitmap y.
//!
//! Codes run from 1. The encoding names them `/g1`, `/g2`, …. `/ToUnicode` maps each code to the
//! glyph's text in UTF-16BE, which is what a CMap destination is (a ligature is several code
//! units in one entry). A simple font has 255 drawing codes; a larger set is an error, so the
//! caller can split it across fonts. The dictionary follows ISO 32000-2 §9.6.5 and the ToUnicode
//! CMap §9.10.3. Nothing is taken from a font compiler.

use kurbo::{BezPath, PathEl};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::OcrError;
use crate::vectorize::GlyphOutline;

/// Drawing codes in one simple Type 3 font. Code 0 stays unused.
const MAX_GLYPHS: usize = 255;
/// A ligature is a few characters. Longer than this is not one glyph's text.
const MAX_TEXT_CHARS: usize = 32;
/// A mapped coordinate past this is not a glyph a page can show.
const MAX_COORD: f64 = 1.0e8;
/// Below this, a cap height or ascent is treated as missing when choosing the em.
const MIN_EM: f64 = 1.0e-3;

/// One master outline to embed. Coordinates are prototype pixels, x right, y down, the same
/// frame as [`crate::vectorize`].
#[derive(Clone, Debug)]
pub struct Type3Glyph {
    /// What the glyph reads as. Several characters is a ligature (`"fi"`). Empty text is drawn
    /// and left out of `/ToUnicode`.
    pub text: String,
    pub outline: GlyphOutline,
    /// The baseline, in prototype pixels, y down. It becomes y = 0 in the font.
    pub baseline: f64,
    /// Advance width in prototype pixels, from the bitmap's left edge.
    pub advance: f64,
}

/// Line metrics in prototype pixels. Heights are distances above the baseline; `descent` is a
/// distance below it. They are written on the font descriptor. The ink's own box is `/FontBBox`.
#[derive(Clone, Copy, Debug)]
pub struct Type3Metrics {
    pub ascent: f64,
    pub cap_height: f64,
    pub x_height: f64,
    pub descent: f64,
}

/// The font written into the document, and the character code of each input glyph.
#[derive(Clone, Debug)]
pub struct Type3Font {
    pub id: ObjRef,
    /// Codes in glyph order, from 1.
    pub codes: Vec<u8>,
}

/// Where a page baseline falls in a prototype bitmap, y down.
///
/// Clustering places every instance so its centroid sits on the prototype anchor. The baseline's
/// offset from that centroid, added to the anchor's y, is the bitmap y [`Type3Glyph::baseline`]
/// wants. `None` when the arithmetic is not finite.
pub fn baseline_in_prototype(anchor_y: f64, page_baseline: f64, centroid_y: f64) -> Option<f64> {
    let y = anchor_y + (page_baseline - centroid_y);
    y.is_finite().then_some(y)
}

/// Write `glyphs` as one Type 3 font. The document is left unchanged when this returns an error.
pub fn synthesize_type3(doc: &mut Document, glyphs: &[Type3Glyph], metrics: &Type3Metrics) -> Result<Type3Font, OcrError> {
    if glyphs.is_empty() {
        return Err(type3_err("needs at least one glyph"));
    }
    if glyphs.len() > MAX_GLYPHS {
        return Err(type3_err(format!(
            "holds {} glyphs; a simple Type 3 font draws at most {MAX_GLYPHS} (split the set across fonts)",
            glyphs.len()
        )));
    }
    let em = em_pixels(metrics)?;
    let units_per_pixel = 1000.0 / em;
    let mut built = Vec::with_capacity(glyphs.len());
    for (index, glyph) in glyphs.iter().enumerate() {
        built.push(build_glyph(index, glyph, units_per_pixel)?);
    }
    Ok(write_font(doc, &built, metrics, em))
}

fn type3_err(detail: impl Into<String>) -> OcrError {
    OcrError::Type3(detail.into())
}

/// The em in prototype pixels. [`synthesize_type3`] maps outlines with it, and the ClearScan
/// page sets the font size to this times the user units per pixel. A second copy of the
/// fallback would scale the outlines off the scan.
pub(crate) fn em_pixels(metrics: &Type3Metrics) -> Result<f64, OcrError> {
    check_metric("ascent", metrics.ascent)?;
    check_metric("cap height", metrics.cap_height)?;
    check_metric("x-height", metrics.x_height)?;
    check_metric("descent", metrics.descent)?;
    // The em is the body size the line actually showed. A missing cap height borrows the ascent,
    // and a line that showed neither still gets a one-pixel em so the matrix stays finite.
    let em = if metrics.cap_height >= MIN_EM {
        metrics.cap_height
    } else if metrics.ascent >= MIN_EM {
        metrics.ascent
    } else {
        1.0
    };
    Ok(em)
}

fn check_metric(name: &str, value: f64) -> Result<(), OcrError> {
    if value.is_finite() && value >= 0.0 { Ok(()) } else { Err(type3_err(format!("{name} must be a finite, non-negative pixel distance"))) }
}

struct BuiltGlyph {
    code: u8,
    name: String,
    width: f64,
    bbox: [f64; 4],
    source: Vec<u8>,
    text: String,
}

fn build_glyph(index: usize, glyph: &Type3Glyph, units_per_pixel: f64) -> Result<BuiltGlyph, OcrError> {
    let number = index.saturating_add(1);
    if !glyph.baseline.is_finite() {
        return Err(type3_err(format!("glyph {number} has a non-finite baseline")));
    }
    if !glyph.advance.is_finite() || glyph.advance < 0.0 {
        return Err(type3_err(format!("glyph {number} needs a finite, non-negative advance")));
    }
    if glyph.text.chars().count() > MAX_TEXT_CHARS {
        return Err(type3_err(format!("glyph {number} text is longer than {MAX_TEXT_CHARS} characters")));
    }
    let Ok(code) = u8::try_from(number) else {
        return Err(type3_err(format!("glyph {number} is past the last simple-font code")));
    };
    let space = GlyphSpace { baseline: glyph.baseline, units_per_pixel, number };
    let width = glyph.advance * units_per_pixel;
    if !width.is_finite() || width.abs() > MAX_COORD {
        return Err(type3_err(format!("glyph {number} advance does not fit in glyph space")));
    }
    let (body, ink, drew) = write_outline(&glyph.outline, &space)?;
    let bbox = outward(ink.unwrap_or([0.0, 0.0, width, 0.0]));
    let mut source = d1(width, bbox);
    source.push_str(&body);
    if drew {
        source.push_str("f\n");
    }
    Ok(BuiltGlyph { code, name: format!("g{code}"), width, bbox, source: source.into_bytes(), text: glyph.text.clone() })
}

/// Prototype pixels to glyph space: x right, y up, baseline at 0, 1000 units per em.
struct GlyphSpace {
    baseline: f64,
    units_per_pixel: f64,
    number: usize,
}

impl GlyphSpace {
    fn map(&self, x: f64, y: f64) -> Result<(f64, f64), OcrError> {
        let gx = x * self.units_per_pixel;
        let gy = (self.baseline - y) * self.units_per_pixel;
        if gx.is_finite() && gy.is_finite() && gx.abs() <= MAX_COORD && gy.abs() <= MAX_COORD {
            Ok((gx, gy))
        } else {
            Err(type3_err(format!("glyph {} outline has a coordinate that is not a finite Type 3 number", self.number)))
        }
    }
}

fn write_outline(outline: &GlyphOutline, space: &GlyphSpace) -> Result<(String, Option<[f64; 4]>, bool), OcrError> {
    let mut body = String::new();
    let mut ink = None;
    let mut drew = false;
    for contour in outline.contours() {
        let (piece, bounds, piece_drew) = write_path(contour.path(), space)?;
        body.push_str(&piece);
        ink = union_box(ink, bounds);
        drew |= piece_drew;
    }
    Ok((body, ink, drew))
}

fn write_path(path: &BezPath, space: &GlyphSpace) -> Result<(String, Option<[f64; 4]>, bool), OcrError> {
    let mut body = String::new();
    let mut bounds = None;
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut drew = false;
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                let q = space.map(p.x, p.y)?;
                include(&mut bounds, q);
                push_op(&mut body, &[q.0, q.1], "m");
                current = q;
                start = q;
            }
            PathEl::LineTo(p) => {
                let q = space.map(p.x, p.y)?;
                include(&mut bounds, q);
                push_op(&mut body, &[q.0, q.1], "l");
                current = q;
                drew = true;
            }
            PathEl::QuadTo(control, end) => {
                let control = space.map(control.x, control.y)?;
                let end = space.map(end.x, end.y)?;
                let (c1, c2) = quad_to_cubic(current, control, end);
                include(&mut bounds, control);
                include(&mut bounds, c1);
                include(&mut bounds, c2);
                include(&mut bounds, end);
                push_op(&mut body, &[c1.0, c1.1, c2.0, c2.1, end.0, end.1], "c");
                current = end;
                drew = true;
            }
            PathEl::CurveTo(a, b, c) => {
                let a = space.map(a.x, a.y)?;
                let b = space.map(b.x, b.y)?;
                let c = space.map(c.x, c.y)?;
                include(&mut bounds, a);
                include(&mut bounds, b);
                include(&mut bounds, c);
                push_op(&mut body, &[a.0, a.1, b.0, b.1, c.0, c.1], "c");
                current = c;
                drew = true;
            }
            PathEl::ClosePath => {
                body.push_str("h\n");
                current = start;
                drew = true;
            }
        }
    }
    Ok((body, bounds, drew))
}

/// A quadratic is not a PDF path operator. The cubic with controls two thirds of the way to the
/// quadratic's control passes through the same curve.
fn quad_to_cubic(from: (f64, f64), control: (f64, f64), to: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    let toward = |origin: (f64, f64)| (origin.0 + (control.0 - origin.0) * (2.0 / 3.0), origin.1 + (control.1 - origin.1) * (2.0 / 3.0));
    (toward(from), toward(to))
}

fn push_op(body: &mut String, numbers: &[f64], operator: &str) {
    for (i, number) in numbers.iter().enumerate() {
        if i > 0 {
            body.push(' ');
        }
        body.push_str(&crate::pdf_num(*number));
    }
    body.push(' ');
    body.push_str(operator);
    body.push('\n');
}

fn include(bounds: &mut Option<[f64; 4]>, point: (f64, f64)) {
    match bounds {
        Some(box_) => {
            box_[0] = box_[0].min(point.0);
            box_[1] = box_[1].min(point.1);
            box_[2] = box_[2].max(point.0);
            box_[3] = box_[3].max(point.1);
        }
        None => *bounds = Some([point.0, point.1, point.0, point.1]),
    }
}

fn union_box(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Grow a box to whole glyph units so a curve on the boundary is not clipped by `d1`.
fn outward(bounds: [f64; 4]) -> [f64; 4] {
    [bounds[0].floor(), bounds[1].floor(), bounds[2].ceil(), bounds[3].ceil()]
}

fn d1(width: f64, bbox: [f64; 4]) -> String {
    format!(
        "{} 0 {} {} {} {} d1\n",
        crate::pdf_num(width),
        crate::pdf_num(bbox[0]),
        crate::pdf_num(bbox[1]),
        crate::pdf_num(bbox[2]),
        crate::pdf_num(bbox[3])
    )
}

fn write_font(doc: &mut Document, glyphs: &[BuiltGlyph], metrics: &Type3Metrics, em: f64) -> Type3Font {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(glyphs.len());
    let mut codes = Vec::with_capacity(glyphs.len());
    let mut font_box: Option<[f64; 4]> = None;
    for glyph in glyphs {
        let proc = doc.add(Object::Stream(Stream::flate(Dict::new(), &glyph.source)));
        charprocs.set(glyph.name.clone().into_bytes(), Object::Ref(proc));
        differences.push(Object::name(&glyph.name));
        widths.push(Object::Real(glyph.width));
        codes.push(glyph.code);
        font_box = union_box(font_box, Some(glyph.bbox));
    }
    let font_box = font_box.unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let last = glyphs.last().map(|g| i64::from(g.code)).unwrap_or(1);
    let cmap = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap(glyphs).as_bytes())));
    let descriptor = doc.add(Object::Dict(descriptor(metrics, em, font_box)));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"BaseFont".to_vec(), Object::name("ClearScan"));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FontBBox".to_vec(), reals(&font_box));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(descriptor));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(last));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    let id = doc.add(Object::Dict(font));
    Type3Font { id, codes }
}

fn reals(values: &[f64]) -> Object {
    Object::Array(values.iter().copied().map(Object::Real).collect())
}

/// Descriptor numbers are thousandths of an em. `descent` is stored negative, below the baseline.
/// `StemV` has no measured stem in a traced outline; a quarter of the x-height keeps the required
/// key finite and positive.
fn descriptor(metrics: &Type3Metrics, em: f64, font_box: [f64; 4]) -> Dict {
    let thousandths = |pixels: f64| pixels / em * 1000.0;
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("ClearScan"));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontBBox".to_vec(), reals(&font_box));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    descriptor.set(b"Ascent".to_vec(), Object::Real(thousandths(metrics.ascent)));
    descriptor.set(b"Descent".to_vec(), Object::Real(-thousandths(metrics.descent)));
    descriptor.set(b"CapHeight".to_vec(), Object::Real(thousandths(metrics.cap_height)));
    descriptor.set(b"XHeight".to_vec(), Object::Real(thousandths(metrics.x_height)));
    descriptor.set(b"StemV".to_vec(), Object::Real(thousandths(metrics.x_height).mul_add(0.25, 0.0).max(1.0)));
    descriptor
}

fn cmap(glyphs: &[BuiltGlyph]) -> String {
    let mut entries = Vec::new();
    for glyph in glyphs {
        if glyph.text.is_empty() {
            continue;
        }
        let hex: String = glyph.text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
        entries.push(format!("<{code:02X}> <{hex}>\n", code = glyph.code));
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", block.len()));
        for entry in block {
            cmap.push_str(entry);
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binarize::Bitmap;
    use crate::vectorize::{VectorizeParams, vectorize};
    use pdfcraft_cos::{Document, Object, SaveOptions, Stream, write_full};
    use pdfcraft_fonts::pdf::Metrics;
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
    use std::sync::Arc;

    fn solid(width: u32, height: u32) -> Bitmap {
        Bitmap::from_ink(width, height, vec![1; (width * height) as usize]).expect("size")
    }

    fn ink_at(width: u32, height: u32, ink_at: impl Fn(u32, u32) -> bool) -> Bitmap {
        let mut ink = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                if ink_at(x, y)
                    && let Some(flag) = ink.get_mut((y * width + x) as usize)
                {
                    *flag = 1;
                }
            }
        }
        Bitmap::from_ink(width, height, ink).expect("size")
    }

    fn glyph(text: &str, bitmap: &Bitmap, baseline: f64) -> Type3Glyph {
        Type3Glyph { text: text.into(), outline: vectorize(bitmap, &VectorizeParams::default()), baseline, advance: f64::from(bitmap.width()) }
    }

    fn body_metrics() -> Type3Metrics {
        Type3Metrics { ascent: 8.0, cap_height: 6.0, x_height: 3.0, descent: 2.0 }
    }

    #[test]
    fn the_em_is_the_cap_height_then_the_ascent_then_one_pixel() {
        let body = body_metrics();
        assert_eq!(em_pixels(&body).expect("cap"), 6.0);
        assert_eq!(em_pixels(&Type3Metrics { cap_height: 0.0, ..body }).expect("ascent"), 8.0);
        assert_eq!(em_pixels(&Type3Metrics { cap_height: 0.0, ascent: 0.0, ..body }).expect("pixel"), 1.0);
        assert!(em_pixels(&Type3Metrics { x_height: f64::NAN, ..body }).is_err());
    }

    fn font_of(doc: &Document) -> pdfcraft_cos::Dict {
        let root = doc.trailer().reference(b"Root").expect("catalog");
        let catalog = doc.dict(&Object::Ref(root)).expect("catalog dict");
        let pages = doc.dict(&Object::Ref(catalog.reference(b"Pages").expect("pages"))).expect("pages dict");
        let kids = doc.resolve(pages.get(b"Kids").expect("kids"));
        let page_ref = kids.as_array().expect("kids")[0].as_ref().expect("page");
        let page = doc.dict(&Object::Ref(page_ref)).expect("page");
        let resources = doc.dict(page.get(b"Resources").expect("resources")).expect("resources");
        let fonts = doc.dict(resources.get(b"Font").expect("font")).expect("font dict");
        doc.dict(&Object::Ref(fonts.reference(b"F").expect("F"))).expect("type 3")
    }

    fn attach(doc: &mut Document, font: ObjRef, content: &str, width: f64, height: f64) {
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
        let pages = {
            let root = doc.trailer().reference(b"Root").expect("catalog");
            let catalog = doc.dict(&Object::Ref(root)).expect("catalog");
            catalog.reference(b"Pages").expect("pages")
        };
        let mut resources_font = Dict::new();
        resources_font.set(b"F".to_vec(), Object::Ref(font));
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(resources_font));
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(width), Object::Real(height)]));
        page.set(b"Contents".to_vec(), Object::Ref(contents));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        let page = doc.add(Object::Dict(page));
        doc.update_dict(pages, |dict| {
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            dict.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("pages");
    }

    fn saved(doc: &Document) -> Vec<u8> {
        write_full(doc, &SaveOptions::default()).expect("save")
    }

    fn proc_source(doc: &Document, font: &pdfcraft_cos::Dict, name: &[u8]) -> String {
        let procs = doc.dict(font.get(b"CharProcs").expect("procs")).expect("procs");
        let obj = doc.get(procs.reference(name).expect("char proc"));
        let Object::Stream(stream) = &*obj else { panic!("char proc is not a stream") };
        String::from_utf8(stream.decoded().expect("decode")).expect("utf-8 operators")
    }

    fn number(doc: &Document, dict: &pdfcraft_cos::Dict, key: &[u8]) -> f64 {
        doc.resolve(dict.get(key).unwrap_or(&Object::Null)).as_f64().unwrap_or(f64::NAN)
    }

    #[test]
    fn baseline_offset_follows_the_centroid() {
        assert_eq!(baseline_in_prototype(10.0, 36.0, 30.0), Some(16.0));
        assert_eq!(baseline_in_prototype(f64::NAN, 1.0, 1.0), None);
    }

    #[test]
    fn a_rectangle_keeps_its_metrics_and_reads_back() {
        let bitmap = solid(4, 6);
        let mut doc = Document::new_empty();
        let written = synthesize_type3(&mut doc, &[glyph("A", &bitmap, 6.0)], &body_metrics()).expect("font");
        assert_eq!(written.codes, vec![1]);
        attach(&mut doc, written.id, "BT /F 6 Tf 1 0 0 1 10 10 Tm <01> Tj ET\n", 64.0, 64.0);
        let bytes = saved(&doc);
        let doc = Document::open(Arc::new(bytes.clone())).expect("reopen");
        let font = font_of(&doc);
        assert_eq!(font.name(b"Subtype"), Some(b"Type3".as_slice()));
        assert_eq!(font.name(b"BaseFont"), Some(b"ClearScan".as_slice()));
        let matrix = doc.resolve(font.get(b"FontMatrix").expect("matrix"));
        let matrix: Vec<f64> = matrix.as_array().expect("matrix")[..].iter().map(|n| doc.resolve(n).as_f64().unwrap_or(f64::NAN)).collect();
        assert_eq!(matrix, vec![0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.base_font, "ClearScan");
        assert_eq!(metrics.decode(&[1]), "A");
        assert!((metrics.width(1) - 4.0 / 6.0).abs() < 1.0e-6, "advance {}", metrics.width(1));
        let descriptor = doc.dict(font.get(b"FontDescriptor").expect("descriptor")).expect("descriptor");
        assert!((number(&doc, &descriptor, b"Ascent") - 8.0 / 6.0 * 1000.0).abs() < 1.0e-6);
        assert!((number(&doc, &descriptor, b"CapHeight") - 1000.0).abs() < 1.0e-6);
        assert!((number(&doc, &descriptor, b"XHeight") - 500.0).abs() < 1.0e-6);
        assert!((number(&doc, &descriptor, b"Descent") - (-2.0 / 6.0 * 1000.0)).abs() < 1.0e-6);
        assert!(number(&doc, &descriptor, b"StemV") > 0.0);
        let source = proc_source(&doc, &font, b"g1");
        assert!(source.contains(" d1\n"), "{source}");
        assert!(source.contains(" l\n") && source.contains("h\n") && source.ends_with("f\n"), "{source}");
        assert!(!source.contains(" c\n"), "a rectangle is not a curve: {source}");
        assert!(!source.contains('e') && !source.contains('E'), "exponent in {source}");
        let names = doc.resolve(doc.dict(font.get(b"Encoding").expect("enc")).expect("enc").get(b"Differences").expect("diff"));
        let names = names.as_array().expect("differences");
        assert_eq!(names[0].as_int(), Some(1));
        assert_eq!(names[1].as_name(), Some(b"g1".as_slice()));
        // The ink sits on the baseline and rises one em. The line's descent is in the descriptor, not the box.
        let bbox = doc.resolve(font.get(b"FontBBox").expect("bbox"));
        let bbox: Vec<f64> = bbox.as_array().expect("bbox")[..].iter().map(|n| doc.resolve(n).as_f64().unwrap_or(f64::NAN)).collect();
        assert!(bbox[1] >= -1.0 && bbox[3] > 500.0, "font box {bbox:?}");
        let rendered = render(&bytes, RequestKind::Pixels);
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        assert!(dark(&rendered, 12, 13), "interior should be ink");
        assert!(light(&rendered, 12, 20), "above the cap");
        assert!(light(&rendered, 12, 6), "below the baseline");
        assert!(light(&rendered, 4, 13), "before the advance");
    }

    #[test]
    fn a_ligature_maps_to_several_characters_and_a_hole_stays_open() {
        let letter = solid(4, 6);
        let mut doc = Document::new_empty();
        let written = synthesize_type3(&mut doc, &[glyph("A", &letter, 6.0), glyph("fi", &letter, 6.0)], &body_metrics()).expect("font");
        assert_eq!(written.codes, vec![1, 2]);
        attach(&mut doc, written.id, "BT /F 6 Tf 1 0 0 1 8 30 Tm <01> Tj 20 0 Td <02> Tj ET\n", 64.0, 64.0);
        let bytes = saved(&doc);
        let doc = Document::open(Arc::new(bytes.clone())).expect("reopen");
        let font = font_of(&doc);
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.decode(&[1, 2]), "Afi");
        let names = doc.resolve(doc.dict(font.get(b"Encoding").expect("enc")).expect("enc").get(b"Differences").expect("diff"));
        assert_eq!(names.as_array().expect("differences").get(2).and_then(Object::as_name), Some(b"g2".as_slice()));
        let text = render(&bytes, RequestKind::Text);
        assert!(text.error.is_none(), "{:?}", text.error);
        let plain = text.text.expect("text").plain_text();
        assert!(plain.contains('A') && plain.contains("fi"), "extracted {plain:?}");

        // Cap height is the bitmap height, so font size 8 draws one bitmap pixel as one user unit.
        let frame = ink_at(10, 8, |x, y| !((3..7).contains(&x) && (3..5).contains(&y)));
        let metrics = Type3Metrics { ascent: 8.0, cap_height: 8.0, x_height: 4.0, descent: 0.0 };
        let mut doc = Document::new_empty();
        let written = synthesize_type3(&mut doc, &[glyph("O", &frame, 8.0)], &metrics).expect("frame");
        attach(&mut doc, written.id, "BT /F 8 Tf 1 0 0 1 8 8 Tm <01> Tj ET\n", 64.0, 64.0);
        let rendered = render(&saved(&doc), RequestKind::Pixels);
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        // Bitmap pixel centres, placed with the baseline on user y = 8.
        // The device pixel covering that bitmap point: user units are 1:1 with pixels here.
        let at = |px: f64, py: f64| ((8.0 + px).floor() as u32, (8.0 + (8.0 - py)).floor() as u32);
        let (fx, fy) = at(1.5, 1.5);
        let (hx, hy) = at(4.5, 3.5);
        assert!(dark(&rendered, fx, fy), "frame pixel ({fx}, {fy}) {:?}", sample(&rendered, fx, fy));
        assert!(light(&rendered, hx, hy), "hole pixel ({hx}, {hy}) {:?}", sample(&rendered, hx, hy));
    }

    #[test]
    fn a_disk_is_curves_and_a_descender_crosses_the_baseline() {
        let disk = ink_at(16, 16, |x, y| {
            let dx = f64::from(x) + 0.5 - 8.0;
            let dy = f64::from(y) + 0.5 - 8.0;
            dx * dx + dy * dy <= 36.0
        });
        let descender = ink_at(4, 6, |_, y| y >= 4);
        let mut doc = Document::new_empty();
        let written = synthesize_type3(&mut doc, &[glyph("o", &disk, 16.0), glyph("g", &descender, 4.0)], &body_metrics()).expect("font");
        attach(&mut doc, written.id, "BT /F 6 Tf <01> Tj ET\n", 32.0, 32.0);
        let doc = Document::open(Arc::new(saved(&doc))).expect("reopen");
        let font = font_of(&doc);
        let disk_source = proc_source(&doc, &font, b"g1");
        assert!(disk_source.contains(" c\n"), "a disk is a curve: {disk_source}");
        let bbox = doc.resolve(font.get(b"FontBBox").expect("bbox"));
        let bottom = bbox.as_array().expect("bbox").get(1).and_then(|n| doc.resolve(n).as_f64()).unwrap_or(0.0);
        assert!(bottom < 0.0, "descender box bottom {bottom}");
    }

    #[test]
    fn an_inkless_glyph_still_has_a_width_and_hostile_input_is_refused() {
        let blank = Bitmap::from_ink(4, 2, vec![0; 8]).expect("blank");
        let mut doc = Document::new_empty();
        let space = Type3Glyph { text: " ".into(), outline: vectorize(&blank, &VectorizeParams::default()), baseline: 2.0, advance: 4.0 };
        let written = synthesize_type3(&mut doc, &[space], &body_metrics()).expect("space");
        attach(&mut doc, written.id, "BT /F 6 Tf <01> Tj ET\n", 16.0, 16.0);
        let doc = Document::open(Arc::new(saved(&doc))).expect("reopen");
        let font = font_of(&doc);
        let metrics = Metrics::from_dict(&doc, &font);
        assert_eq!(metrics.decode(&[1]), " ");
        assert!((metrics.width(1) - 4.0 / 6.0).abs() < 1.0e-6);
        let source = proc_source(&doc, &font, b"g1");
        assert!(source.contains("d1\n") && !source.contains("\nf\n"), "{source}");

        let mut doc = Document::new_empty();
        let err = synthesize_type3(&mut doc, &[], &body_metrics()).unwrap_err();
        assert!(err.to_string().contains("at least one"), "{err}");
        let one = glyph("a", &solid(1, 1), 1.0);
        let many = vec![one.clone(); 256];
        let err = synthesize_type3(&mut doc, &many, &body_metrics()).unwrap_err();
        assert!(err.to_string().contains("255"), "{err}");
        let err = synthesize_type3(&mut doc, &[Type3Glyph { baseline: f64::NAN, ..one.clone() }], &body_metrics()).unwrap_err();
        assert!(err.to_string().contains("baseline"), "{err}");
        let err = synthesize_type3(&mut doc, &[Type3Glyph { advance: -1.0, ..one.clone() }], &body_metrics()).unwrap_err();
        assert!(err.to_string().contains("advance"), "{err}");
        let err = synthesize_type3(&mut doc, &[one], &Type3Metrics { cap_height: -1.0, ..body_metrics() }).unwrap_err();
        assert!(err.to_string().contains("cap height"), "{err}");
        // The refusals added no page, so the empty document still saves.
        assert!(saved(&doc).starts_with(b"%PDF"));
    }

    fn render(bytes: &[u8], kind: RequestKind) -> pdfcraft_render::RenderedPage {
        let mut renderer = PageRenderer::new(Arc::new(bytes.to_vec()), RenderConfig::default());
        renderer.render(RenderRequest { page: 0, kind, scale: 1.0, ..RenderRequest::default() })
    }

    fn sample(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> [u8; 4] {
        let height = page.height;
        let width = page.width;
        let row = height.saturating_sub(1).saturating_sub(y_pdf);
        let index = (row * width + x) as usize * 4;
        let bytes: &[u8] = &page.rgba;
        match bytes.get(index..index + 4) {
            Some([r, g, b, a]) => [*r, *g, *b, *a],
            _ => [255, 255, 255, 255],
        }
    }

    fn dark(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> bool {
        sample(page, x, y_pdf)[0] < 80
    }

    fn light(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> bool {
        sample(page, x, y_pdf)[0] > 200
    }
}
