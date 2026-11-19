use crate::CMapResolverFn;
use crate::context::{
    Context, HeldContent, MAX_CACHED_TYPE3_PROCS, Type3Procs, content_left, hold_content,
    path_as_rect,
};
use crate::device::Device;
use crate::font::glyph_simulator::GlyphSimulator;
use crate::font::true_type::{Width, read_encoding, read_widths};
use crate::font::{
    Encoding, Glyph, Type3Glyph, UNITS_PER_EM, numbered_glyph_name_at_its_code,
    numbered_names_name_their_codes, read_to_unicode,
};
use crate::interpret::state::TextState;
use crate::soft_mask::SoftMask;
use crate::util::{BezPathExt, RectExt};
use crate::{BlendMode, FillRule, interpret};
use crate::{CacheKey, ClipPath, GlyphDrawMode, PathDrawMode};
use crate::{Image, Paint};
use hayro_cmap::{BfString, CMap};
use hayro_syntax::content::TypedIter;
use hayro_syntax::content::ops::TypedInstruction;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::{CHAR_PROCS, FONT_BBOX, FONT_MATRIX, RESOURCES};
use hayro_syntax::page::Resources;
use kurbo::{Affine, BezPath, Point, Rect};
use skrifa::GlyphId;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{OnceLock, atomic::Ordering};

#[derive(Debug)]
pub(crate) struct Type3<'a> {
    widths: Vec<Width>,
    missing_width: f32,
    /// PdfCraft patch (90): the width each code's glyph procedure gives with `d0`/`d1`, read
    /// the first time the code is shown in a font without `/Widths`. One cell per code.
    proc_widths: Vec<OnceCell<Option<f32>>>,
    encoding: Encoding,
    encodings: HashMap<u8, String>,
    dict: Dict<'a>,
    char_procs: HashMap<String, Stream<'a>>,
    glyph_simulator: GlyphSimulator,
    font_bbox: Rect,
    matrix: Affine,
    to_unicode: Option<CMap>,
    /// PdfCraft patch (87): glyph procedures already decoded, by glyph name, so a glyph shown
    /// again and again (a Type 3 font used as a fill pattern draws one glyph hundreds of
    /// thousands of times) isn't inflated again for every copy. Only procedures of at most
    /// [`MAX_CACHED_PROC`] bytes are kept, and the fonts of one interpreter cache keep at most
    /// `context::MAX_CACHED_TYPE3_PROCS` bytes together.
    decoded_procs: RefCell<HashMap<String, Rc<DecodedProc>>>,
    /// PdfCraft patch: whether the font's numbered glyph names all name their own codes
    /// (`numbered_names_name_their_codes`), worked out on first use.
    own_code_names: OnceLock<bool>,
}

/// PdfCraft patch (87): the largest decoded glyph procedure [`Type3`] keeps.
const MAX_CACHED_PROC: usize = 64 << 10;
/// PdfCraft patch (87): what a kept procedure costs the cache besides its bytes and its glyph
/// name (the map entry and the shared allocation's counts), so many empty procedures count too.
const CACHED_PROC_OVERHEAD: usize = 128;

#[derive(Debug)]
struct DecodedProc {
    bytes: Rc<[u8]>,
    shape: Option<(BezPath, FillRule)>,
}

/// Recognize an uncoloured glyph consisting of just one filled outline. There is no graphics
/// state to interpret or resource to resolve. All other procedures use the regular interpreter.
fn simple_outline(bytes: &[u8]) -> Option<(BezPath, FillRule)> {
    if bytes.len() > 8 * 1024 {
        return None;
    }
    let mut ops = TypedIter::new(bytes);
    if !matches!(ops.next(), Some(TypedInstruction::ShapeGlyph(_))) {
        return None;
    }
    let mut path = BezPath::new();
    let mut open = false;
    let fill = loop {
        if path.elements().len() >= 256 {
            return None;
        }
        match ops.next()? {
            TypedInstruction::MoveTo(m) => {
                path.move_to(outline_point(m.0.as_f64(), m.1.as_f64())?);
                open = true;
            }
            TypedInstruction::LineTo(l) if open => {
                path.line_to(outline_point(l.0.as_f64(), l.1.as_f64())?);
            }
            TypedInstruction::CubicTo(c) if open => {
                path.curve_to(
                    outline_point(c.0.as_f64(), c.1.as_f64())?,
                    outline_point(c.2.as_f64(), c.3.as_f64())?,
                    outline_point(c.4.as_f64(), c.5.as_f64())?,
                );
            }
            TypedInstruction::ClosePath(_) if open => {
                path.close_path();
                open = false;
            }
            TypedInstruction::FillPathNonZero(_) => break FillRule::NonZero,
            TypedInstruction::FillPathEvenOdd(_) => break FillRule::EvenOdd,
            _ => return None,
        }
    };
    let bounds = path.fast_bounding_box();
    // Degenerate fills and rectangles have special drawing rules in `fill_path_impl`.
    (ops.next().is_none()
        && bounds.width() > 0.0
        && bounds.height() > 0.0
        && path_as_rect(&path).is_none())
    .then_some((path, fill))
}

fn outline_point(x: f64, y: f64) -> Option<Point> {
    let range = -1.0e7..=1.0e7;
    (range.contains(&x) && range.contains(&y)).then_some(Point::new(x, y))
}

impl<'a> Type3<'a> {
    pub(crate) fn new(dict: &Dict<'a>, cmap_resolver: &CMapResolverFn) -> Option<Self> {
        let (encoding, encodings) = read_encoding(dict);
        let (widths, missing_width) = read_widths(dict, dict)?;
        let font_bbox = dict
            .get::<hayro_syntax::object::Rect>(FONT_BBOX)
            .unwrap_or(hayro_syntax::object::Rect::ZERO)
            .to_kurbo();

        let matrix = Affine::new(
            dict.get::<[f64; 6]>(FONT_MATRIX)
                .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
        );

        let char_procs = {
            let mut procs = HashMap::new();
            let dict = dict.get::<Dict<'_>>(CHAR_PROCS).unwrap_or_default();

            for name in dict.keys() {
                if let Some(prog) = dict.get::<Stream<'_>>(name.as_ref()) {
                    procs.insert(name.as_str().to_string(), prog);
                }
            }

            procs
        };

        let to_unicode = read_to_unicode(dict, cmap_resolver);

        Some(Self {
            glyph_simulator: GlyphSimulator::new(),
            encoding,
            font_bbox,
            char_procs,
            widths,
            missing_width,
            proc_widths: vec![OnceCell::new(); usize::from(u8::MAX) + 1],
            encodings,
            matrix,
            dict: dict.clone(),
            to_unicode,
            decoded_procs: RefCell::new(HashMap::new()),
            own_code_names: OnceLock::new(),
        })
    }

    /// PdfCraft patch (87): the decoded procedure of glyph `name`, held within the page's
    /// content budget while the glyph is painted, whether it was decoded now or kept from an
    /// earlier copy, so a page skips the same glyphs either way once the budget is spent. It is
    /// decoded the first time and kept, when small enough and while `procs` has room, for the
    /// next copies. Only a procedure decoded with more budget left than [`MAX_CACHED_PROC`] is
    /// kept, so one the budget may have cut short isn't.
    fn decoded_proc(
        &self,
        name: &str,
        program: &Stream<'a>,
        procs: &Type3Procs,
    ) -> Option<(Rc<DecodedProc>, HeldContent)> {
        let kept = self.decoded_procs.borrow().get(name).cloned();
        if let Some(proc) = kept {
            let held = hold_content(proc.bytes.len())?;
            return Some((proc, held));
        }
        let left = content_left();
        // PdfCraft patch: see `context::MAX_PAGE_CONTENT`.
        let decoded = crate::context::decode_content(program)?;
        procs.decodes.set(procs.decodes.get().saturating_add(1));
        let bytes: Rc<[u8]> = Rc::from(decoded.as_ref());
        drop(decoded);
        let held = hold_content(bytes.len())?;
        let cost = bytes
            .len()
            .saturating_add(name.len())
            .saturating_add(CACHED_PROC_OVERHEAD);
        let base_total = procs.held.get().saturating_add(cost);
        if left > MAX_CACHED_PROC
            && bytes.len() <= MAX_CACHED_PROC
            && base_total <= MAX_CACHED_TYPE3_PROCS
        {
            let mut total = base_total;
            let shape = simple_outline(&bytes).filter(|(path, _)| {
                // Account for spare capacity in the path's vector as well as its elements.
                let size = size_of_val(path.elements()).saturating_mul(2);
                let with_shape = total.saturating_add(size);
                if with_shape > MAX_CACHED_TYPE3_PROCS {
                    return false;
                }
                total = with_shape;
                true
            });
            let proc = Rc::new(DecodedProc { bytes, shape });
            self.decoded_procs
                .borrow_mut()
                .insert(name.to_string(), proc.clone());
            procs.held.set(total);
            return Some((proc, held));
        }
        Some((Rc::new(DecodedProc { bytes, shape: None }), held))
    }

    pub(crate) fn map_code(&self, code: u8) -> GlyphId {
        self.encodings
            .get(&code)
            .map(|s| s.as_str())
            .or_else(|| self.encoding.map_code(code))
            .map(|g| self.glyph_simulator.string_to_glyph(g))
            .unwrap_or(GlyphId::NOTDEF)
    }

    pub(crate) fn glyph_width(&self, code: u8) -> f32 {
        // PdfCraft patch (90): /Widths, /FirstChar and /LastChar are required for Type 3 fonts
        // (Table 110), but a font without them gave every glyph no advance and its text was drawn on one spot. The glyph
        // procedure's `d0`/`d1` operator states the glyph's width too (9.6.4), so such a font
        // takes that width, as PDFium does. A font with /Widths is read as before: a code
        // outside FirstChar..LastChar has width 0 (Table 110), as in Poppler and MuPDF.
        let w = match self.widths.get(code as usize).copied() {
            Some(Width::Value(w)) => w,
            _ if self.widths.is_empty() => self.proc_width(code).unwrap_or(self.missing_width),
            _ => self.missing_width,
        };
        (w * self.matrix.as_coeffs()[0] as f32) * UNITS_PER_EM
    }

    /// PdfCraft patch (90): whether the font has no `/Widths` (or `/FirstChar`/`/LastChar`), so
    /// that `glyph_width` falls back on the glyph procedures' `d0`/`d1` widths.
    pub(crate) fn widths_missing(&self) -> bool {
        self.widths.is_empty()
    }

    /// PdfCraft patch (90): the horizontal width `wx` that the glyph procedure for `code`
    /// sets with `d0` or `d1`, in glyph space. `None` when the code has no procedure, the
    /// procedure sets no width or its width isn't finite. A procedure the page's content
    /// budget can't decode isn't remembered, so a later page may still read it.
    fn proc_width(&self, code: u8) -> Option<f32> {
        let cell = self.proc_widths.get(usize::from(code))?;
        if let Some(width) = cell.get() {
            return *width;
        }
        let name = self.glyph_simulator.glyph_to_string(self.map_code(code))?;
        let program = self.char_procs.get(&name)?;
        let decoded = crate::context::decode_content(program)?;
        let mut iter = TypedIter::new(decoded.as_ref());
        let mut width = None;
        while let Some(op) = iter.next() {
            match op {
                TypedInstruction::ColorGlyph(d0) => width = Some(d0.0.as_f32()),
                TypedInstruction::ShapeGlyph(d1) => width = Some(d1.0.as_f32()),
                _ => continue,
            }
            break;
        }
        let width = width.filter(|w| w.is_finite());
        // Setting can't fail here: the cell was empty above and nothing in between fills it.
        let _ = cell.set(width);
        width
    }

    pub(crate) fn char_code_to_unicode(&self, char_code: u32) -> Option<BfString> {
        // PdfCraft patch: upstream only consulted ToUnicode. Many Type3 fonts (e.g. TeX/dvips
        // bitmap fonts) have none; fall back to the /Encoding glyph names, then to printable
        // ASCII codes, as other viewers do for text extraction.
        self.to_unicode
            .as_ref()
            .and_then(|t| t.lookup_bf_string(char_code))
            .or_else(|| {
                let code = u8::try_from(char_code).ok()?;
                let name = self
                    .encodings
                    .get(&code)
                    .map(|s| s.as_str())
                    // PdfCraft patch: an Annex D.2 alias code reads as the ordinary character;
                    // see `Encoding::text_alias`.
                    .or_else(|| self.encoding.text_alias(code))
                    .or_else(|| self.encoding.map_code(code));
                name.and_then(crate::font::glyph_name_to_unicode)
                    .or_else(|| (0x20..0x7f).contains(&code).then_some(code as char))
                    .map(BfString::Char)
                    // PdfCraft patch (100): above 126, a name that only numbers its glyph reads
                    // as the code's Latin-1 character when the number is that code and every
                    // numbered name in the font names its own code (`C174` at 174 reads `®`),
                    // as in a Type 1 font.
                    .or_else(|| {
                        let own_codes = *self.own_code_names.get_or_init(|| {
                            numbered_names_name_their_codes(
                                (0..=u8::MAX).filter_map(|c| Some((c, self.code_to_name(c)?))),
                            )
                        });
                        name.filter(|_| own_codes)
                            .and_then(|name| numbered_glyph_name_at_its_code(name, code))
                    })
            })
    }

    /// PdfCraft patch: the glyph name `/Encoding` gives a code, as `map_code` reads it.
    fn code_to_name(&self, code: u8) -> Option<&str> {
        self.encodings
            .get(&code)
            .map(String::as_str)
            .or_else(|| self.encoding.map_code(code))
    }

    pub(crate) fn render_glyph(
        &self,
        glyph: &Type3Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        device: &mut impl Device<'a>,
    ) -> Option<()> {
        // PdfCraft patch: a glyph procedure may show text in this same font (through
        // inherited resources), recursing until the stack overflowed.
        if glyph.nesting_depth > crate::context::MAX_PAINT_NESTING
            || !crate::context::take_nested_paint(glyph.nesting_depth)
        {
            return None;
        }
        let root_transform =
            transform * glyph_transform * self.matrix * Affine::scale(UNITS_PER_EM as f64);
        let name = self.glyph_simulator.glyph_to_string(glyph.glyph_id)?;
        let program = self.char_procs.get(&name)?;
        let (decoded, _held) = self.decoded_proc(&name, program, &glyph.cache.type3_procs)?;
        if let Some((path, fill)) = &decoded.shape
            && matches!(paint, Paint::Color(_))
            && glyph.state.graphics_state.soft_mask.is_none()
        {
            if !glyph
                .settings
                .cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                device.set_soft_mask(None);
                device.draw_path(path, root_transform, paint, &PathDrawMode::Fill(*fill));
            }
            return Some(());
        }

        let mut state = glyph.state.clone();
        state.ctm = root_transform;

        // Not sure if this is mentioned anywhere, but I do think we need to reset the text state
        // (though the graphics state itself should be preserved).
        state.text_state = TextState::default();

        let iter = TypedIter::new(&decoded.bytes);

        let is_shape_glyph = {
            let mut iter = iter.clone();
            let mut is_shape_glyph = true;

            while let Some(op) = iter.next() {
                match op {
                    TypedInstruction::ShapeGlyph(_) => {
                        break;
                    }
                    TypedInstruction::ColorGlyph(_) => {
                        is_shape_glyph = false;
                        break;
                    }
                    _ => {}
                }
            }

            is_shape_glyph
        };

        // PdfCraft patch: context clips and shading bounds are in device space; FontBBox is in glyph space.
        let mut context = Context::new_with(
            state.ctm,
            root_transform.transform_rect_bbox(self.font_bbox),
            &glyph.cache,
            glyph.xref,
            glyph.settings.clone(),
            state,
            glyph.nesting_depth,
        );

        let mut resources = Resources::from_parent(
            self.dict.get(RESOURCES).unwrap_or_default(),
            glyph.parent_resources.clone(),
        );

        // Technically not valid, but also support by Adobe Acrobat. See PDFBOX-5294.
        if let Some(procs_resources) = program.dict().get::<Dict<'_>>(RESOURCES) {
            resources = Resources::from_parent(procs_resources, resources);
        }

        if is_shape_glyph {
            let mut device = Type3ShapeGlyphDevice::new(device, paint.clone());
            interpret(iter, &resources, &mut context, &mut device);
        } else {
            interpret(iter, &resources, &mut context, device);
        }

        Some(())
    }
}

impl CacheKey for Type3<'_> {
    fn cache_key(&self) -> u128 {
        self.dict.cache_key()
    }
}

struct Type3ShapeGlyphDevice<'a, 'b, T: Device<'a>> {
    inner: &'b mut T,
    paint: Paint<'a>,
}

impl<'a, 'b, T: Device<'a>> Type3ShapeGlyphDevice<'a, 'b, T> {
    pub(crate) fn new(device: &'b mut T, paint: Paint<'a>) -> Self {
        Self {
            inner: device,
            paint,
        }
    }
}

// Only filling, stroking of paths and stencil masks are allowed.
impl<'a, T: Device<'a>> Device<'a> for Type3ShapeGlyphDevice<'a, '_, T> {
    fn set_soft_mask(&mut self, m: Option<SoftMask<'a>>) {
        self.inner.set_soft_mask(m);
    }

    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        _: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        self.inner
            .draw_path(path, transform, &self.paint, draw_mode);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.inner.push_clip_path(clip_path);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}

    fn draw_glyph(
        &mut self,
        g: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        p: &Paint<'a>,
        draw_mode: &GlyphDrawMode,
    ) {
        self.inner
            .draw_glyph(g, transform, glyph_transform, p, draw_mode);
    }

    fn pop_clip_path(&mut self) {
        self.inner.pop_clip_path();
    }

    fn pop_transparency_group(&mut self) {}

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        if let Image::Stencil(mut s) = image {
            s.paint = self.paint.clone();
            self.inner.draw_image(Image::Stencil(s), transform);
        }
    }

    fn set_blend_mode(&mut self, _: BlendMode) {}
}
