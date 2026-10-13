//! The PostScript `Device`: geometry, colour and clip emission (Layer A, Task 3).
//!
//! Emission model: PDF user space *is* PostScript user space (the initial transform is the
//! identity handoff — see the findings file), so nothing outside a draw ever changes the CTM.
//! Every draw is self-contained: `gsave`, the total CTM baked in via `concat`, the path
//! coordinates raw, the paint operator, `grestore`. Clip paths arrive from the interpreter
//! already transformed into that space, so they are emitted raw inside their own `gsave` and
//! released by the matching `grestore`. Shadings/patterns go through `shading.rs`; text and
//! images through `fonts.rs`/`images.rs`. Transparency has no Level 3-agnostic operator form,
//! so it degrades to fidelity notes (Task 7 policy: one note per page, first occurrence
//! only — normal pages stay note-free).

use std::collections::HashSet;
use std::marker::PhantomData;

use hayro::hayro_interpret::color::Color;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::page::Resources;
use hayro::hayro_interpret::pattern::Pattern;
use hayro::hayro_interpret::{BlendMode, ClipPath, Device, FillRule, GlyphDrawMode, Image, Paint, PathDrawMode, SoftMask, StrokeProps};
use kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect};

use crate::FidelityNote;
use crate::fonts::{self, FontTable, TextStrategy};
use crate::images;
use crate::psnum;
use crate::shading;

/// The shared output collector: operator bytes, fidelity notes, and the page the notes refer to.
#[derive(Default)]
pub(crate) struct Emitter {
    /// The PostScript program emitted so far.
    pub(crate) out: Vec<u8>,
    /// Every degradation recorded so far.
    pub(crate) notes: Vec<FidelityNote>,
    /// The 1-based page the emitter is currently inside, used to tag notes.
    pub(crate) page: Option<usize>,
    /// Note texts already recorded for this page (the first-occurrence-per-page policy).
    noted: HashSet<String>,
}

impl Emitter {
    /// Append one operator (or comment) line.
    pub(crate) fn op(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
        self.out.push(b'\n');
    }

    /// Append one number operand followed by a space. Non-finite values have no PostScript
    /// operand at all: they are replaced with `0` and the repair is recorded.
    pub(crate) fn coord(&mut self, v: f64) {
        let n = self.number(v);
        self.out.extend_from_slice(n.as_bytes());
        self.out.push(b' ');
    }

    /// [`psnum::num`] with the non-finite repair applied and recorded.
    pub(crate) fn number(&mut self, v: f64) -> String {
        match psnum::num(v) {
            Some(s) => s,
            None => {
                self.note("non-finite coordinate replaced with 0");
                "0".to_string()
            }
        }
    }

    /// Record a degradation against the current page.
    pub(crate) fn note(&mut self, text: &str) {
        self.notes.push(FidelityNote { page: self.page, text: text.to_string() });
    }

    /// Record a degradation once per page (first occurrence only). The transparency policy:
    /// the interpreter re-announces the blend mode/mask before every draw, and one note per
    /// page per degradation is what a reader wants — not one per painted rectangle.
    pub(crate) fn note_once(&mut self, text: &str) {
        if self.noted.insert(text.to_string()) {
            self.note(text);
        }
    }
}

/// The PostScript `Device` implementation.
pub(crate) struct PsDevice<'a> {
    pub(crate) emit: Emitter,
    /// Per-font text strategies, built from the page's font resources.
    pub(crate) fonts: FontTable,
    /// A text run waiting for its `show`: glyphs batch here until a font,
    /// strategy, transform-basis or paint change flushes them.
    run: Option<TextRun<'a>>,
    /// Later tasks consume `&'a` PDF data (font programs, image streams); nothing is stored yet.
    _marker: PhantomData<&'a ()>,
}

/// One pending `show`: consecutive glyphs of one font, one linear transform
/// basis and one paint, positioned by their observed glyph transforms.
struct TextRun<'a> {
    font_key: u128,
    /// Which 256-code slice of the font the codes live in (its PostScript name).
    slice: usize,
    /// The run's linear part `[a b c d]`: the CTM every glyph of the run shares
    /// exactly (text-matrix advances are translations, which compose exactly).
    basis: [f64; 4],
    /// The paint the run was started with, re-emitted at flush time.
    paint: Paint<'a>,
    /// The first glyph's origin, in the run's user space (1000-unit glyph space).
    origin: Point,
    /// The assigned codes, in draw order.
    codes: Vec<u8>,
    /// Displacement from each glyph's origin to the next, in user space —
    /// exactly what `xyshow` consumes.
    offsets: Vec<(f64, f64)>,
    /// The last glyph's origin, for the next offset.
    last: Point,
}

impl<'a> PsDevice<'a> {
    /// A device emitting into a fresh buffer, tagging notes with `page` (1-based, if known).
    /// The unit-test constructor: `export()` always supplies the page's font resources
    /// (and the document's running font table) via [`Self::from_table`].
    #[cfg(test)]
    pub(crate) fn new(page: Option<usize>) -> Self {
        Self {
            emit: Emitter { out: Vec::new(), notes: Vec::new(), page, noted: HashSet::new() },
            fonts: FontTable::default(),
            run: None,
            _marker: PhantomData,
        }
    }

    /// A device whose text strategies come from `resources` (the page's own —
    /// the same dicts the interpreter resolves fonts through, so the glyph font
    /// cache keys index the table) plus the document's running `FontTable`, so
    /// font decisions and slice names carry across the pages of one export.
    pub(crate) fn from_table(page: Option<usize>, resources: &Resources<'_>, fonts: FontTable) -> Self {
        let mut fonts = fonts;
        fonts.add_resources(resources);
        Self { emit: Emitter { out: Vec::new(), notes: Vec::new(), page, noted: HashSet::new() }, fonts, run: None, _marker: PhantomData }
    }

    /// Emit any pending text run. Called before every other operator (program
    /// order), and once when the page is done; safe to call again.
    pub(crate) fn finish(&mut self) {
        self.flush_text();
    }

    /// Emit the pending run as one `show`/`xyshow` inside the usual
    /// `gsave`…`grestore` wrap.
    fn flush_text(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        let Some(name) = self.fonts.slice_names(run.font_key).get(run.slice).cloned() else {
            self.emit.note("text skipped (font entry missing)");
            return;
        };
        self.emit.op("gsave");
        concat(&mut self.emit, Affine::new([run.basis[0], run.basis[1], run.basis[2], run.basis[3], 0.0, 0.0]));
        // The prolog font's natural matrix is 1/1000 (Type 1 programs), so scaling
        // by 1000 makes the effective glyph matrix the identity on 1000-unit glyph
        // space, which is exactly the space the run basis maps. (`scalefont` takes
        // the font itself, not its key — verified against the Ghostscript oracle.)
        self.emit.op(&format!("/{name} findfont 1000 scalefont setfont"));
        paint_ops(&mut self.emit, &run.paint);
        self.emit.coord(run.origin.x);
        self.emit.coord(run.origin.y);
        self.emit.op("moveto");
        let string = fonts::ps_string(&run.codes);
        if run.codes.len() == 1 {
            self.emit.op(&format!("{string} show"));
        } else {
            // `xyshow` takes one displacement pair per glyph, each replacing
            // that glyph's font advance (verified against the Ghostscript
            // oracle); the final pair is never used, so it is zero. The array
            // wraps to stay near the DSC line-length convention.
            let mut array = String::from("[");
            let mut line_len = 1usize;
            for (dx, dy) in &run.offsets {
                let pair = format!("{} {} ", self.emit.number(*dx), self.emit.number(*dy));
                if line_len + pair.len() > 200 {
                    array.push('\n');
                    line_len = 0;
                }
                array.push_str(&pair);
                line_len += pair.len();
            }
            array.push_str("0 0]");
            self.emit.op(&format!("{string} {array} xyshow"));
        }
        self.emit.op("grestore");
    }

    /// Add one glyph to the pending run, or start a new one (flushing the old).
    fn push_run_glyph(&mut self, font_key: u128, slice: usize, code: u8, total: Affine, paint: &Paint<'a>) {
        let [a, b, c, d, e, f] = total.as_coeffs();
        let basis = [a, b, c, d];
        // The glyph origin in the run's user space: the inverse basis applied to
        // the transform's translation. A singular basis inverts to non-finite
        // coordinates, which `psnum` repairs (with a note) at emission time.
        let user = Affine::new([a, b, c, d, 0.0, 0.0]).inverse() * Point::new(e, f);
        if let Some(run) = self.run.as_mut()
            && run.font_key == font_key
            && run.slice == slice
            && run.basis == basis
            && same_paint(&run.paint, paint)
        {
            run.offsets.push((user.x - run.last.x, user.y - run.last.y));
            run.last = user;
            run.codes.push(code);
            return;
        }
        self.flush_text();
        self.run = Some(TextRun { font_key, slice, basis, paint: paint.clone(), origin: user, codes: vec![code], offsets: Vec::new(), last: user });
    }
}

/// Whether two paints render identically (the run batching test). Patterns
/// never compare equal: their equality is not observable through the public API.
fn same_paint(a: &Paint<'_>, b: &Paint<'_>) -> bool {
    match (a, b) {
        (Paint::Color(x), Paint::Color(y)) => Color::to_rgba(x).components() == Color::to_rgba(y).components(),
        _ => false,
    }
}

impl Drop for PsDevice<'_> {
    fn drop(&mut self) {
        // A pending run must reach the program even if nobody called `finish`.
        self.flush_text();
    }
}

/// Bake a draw transform in as a `concat`. PostScript's `concat` takes a *matrix object*,
/// so the six coefficients ride inside `[a b c d e f]` (six bare operands are a typecheck
/// on every conforming interpreter, verified against the Ghostscript oracle).
fn concat(emit: &mut Emitter, transform: Affine) {
    let mut line = String::from("[");
    for c in transform.as_coeffs() {
        line.push_str(&emit.number(c));
        line.push(' ');
    }
    line.pop();
    line.push_str("] concat");
    emit.op(&line);
}

/// Emit `newpath` plus the path's elements as `moveto`/`lineto`/`curveto`/`closepath`.
/// Quadratic Béziers have no PostScript operator and are elevated exactly to the equivalent
/// cubic (degree elevation is lossless, so it is not a degradation).
pub(crate) fn path_ops(emit: &mut Emitter, path: &BezPath) {
    emit.op("newpath");
    let mut current = Point::ORIGIN;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                emit.coord(p.x);
                emit.coord(p.y);
                emit.op("moveto");
                current = *p;
            }
            PathEl::LineTo(p) => {
                emit.coord(p.x);
                emit.coord(p.y);
                emit.op("lineto");
                current = *p;
            }
            PathEl::QuadTo(p1, p2) => {
                let c1 = current + (2.0 / 3.0) * (*p1 - current);
                let c2 = *p2 + (2.0 / 3.0) * (*p1 - *p2);
                for p in [&c1, &c2, p2] {
                    emit.coord(p.x);
                    emit.coord(p.y);
                }
                emit.op("curveto");
                current = *p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                for p in [p1, p2, p3] {
                    emit.coord(p.x);
                    emit.coord(p.y);
                }
                emit.op("curveto");
                current = *p3;
            }
            PathEl::ClosePath => emit.op("closepath"),
        }
    }
}

/// Emit the rectangle's four corners as plain path operators (the shortcut the interpreter uses
/// for axis-aligned rectangles).
fn rect_ops(emit: &mut Emitter, rect: &Rect) {
    emit.op("newpath");
    let corners = [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x1, rect.y1), (rect.x0, rect.y1)];
    let [first, rest @ ..] = corners;
    emit.coord(first.0);
    emit.coord(first.1);
    emit.op("moveto");
    for (x, y) in rest {
        emit.coord(x);
        emit.coord(y);
        emit.op("lineto");
    }
    emit.op("closepath");
}

/// Emit the stroke properties. Line width, dash and phase go through the number formatter; cap
/// and join map onto PostScript's integer codes (0/1/2). Negative widths and miter limits are
/// rangecheck errors in PostScript, and mean "thinner than renderable"/"invalid" in PDF, so they
/// clamp instead of failing downstream interpreters.
fn stroke_ops(emit: &mut Emitter, props: &StrokeProps) {
    emit.coord(f64::from(props.line_width).max(0.0));
    emit.op("setlinewidth");

    // PLRM `setdash` requires nonnegative elements with a positive sum (a violating pattern is
    // a rangecheck, which could truncate the page on a conforming interpreter); PDF calls
    // negative elements invalid and an all-zero pattern a solid line. Normalize rather than
    // pass PDF data through: negative elements clamp to 0 (noted), an empty or all-zero
    // pattern emits a solid line (the phase is meaningless then), and a negative phase is
    // reduced modulo the pattern length — visually identical, never negative.
    let clamped_sum: f64 = props.dash_array.iter().map(|d| f64::from(*d).max(0.0)).sum();
    if clamped_sum > 0.0 {
        let mut line = String::from("[");
        for (i, d) in props.dash_array.iter().enumerate() {
            let d = f64::from(*d);
            if d < 0.0 {
                emit.note("negative dash element replaced with 0");
            }
            if i > 0 {
                line.push(' ');
            }
            line.push_str(&emit.number(if d < 0.0 { 0.0 } else { d }));
        }
        line.push_str("] ");
        let phase = f64::from(props.dash_offset);
        let phase = if phase < 0.0 { phase.rem_euclid(clamped_sum) } else { phase };
        line.push_str(&emit.number(phase));
        line.push_str(" setdash");
        emit.op(&line);
    } else {
        emit.op("[] 0 setdash");
    }

    let join = match props.line_join {
        Join::Miter => "0",
        Join::Round => "1",
        Join::Bevel => "2",
    };
    emit.op(&format!("{join} setlinejoin"));

    let cap = match props.line_cap {
        Cap::Butt => "0",
        Cap::Round => "1",
        Cap::Square => "2",
    };
    emit.op(&format!("{cap} setlinecap"));

    emit.coord(f64::from(props.miter_limit).max(1.0));
    emit.op("setmiterlimit");
}

/// Emit the paint's colour operators. `Color` exposes only its 8-bit RGBA view — the same values
/// the raster renderer shows — so a flat colour is emitted as `setgray` when the triple is equal
/// and `setrgbcolor` otherwise; alpha is drawn opaque and the drop recorded. Pattern paints
/// degrade to their flat stand-in (`shading.rs` owns that policy; here there is no path or
/// clip to shade through — this is the text-run and stencil-mask position).
/// (`images.rs` reuses this for stencil masks, which paint with the current colour.)
pub(crate) fn paint_ops(emit: &mut Emitter, paint: &Paint<'_>) {
    match paint {
        Paint::Color(color) => {
            let [r, g, b, a] = Color::to_rgba(color).components();
            if r == g && g == b {
                emit.coord(f64::from(r));
                emit.op("setgray");
            } else {
                emit.coord(f64::from(r));
                emit.coord(f64::from(g));
                emit.coord(f64::from(b));
                emit.op("setrgbcolor");
            }
            if a < 1.0 {
                emit.note("transparency ignored (paint alpha)");
            }
        }
        Paint::Pattern(_) => shading::pattern_approx(paint, emit),
    }
}

/// A paint that renders nothing (PDF alpha 0 — Separation "None" is invisible). The
/// draw is skipped rather than degraded, because skipping is exactly what the raster
/// renderer shows.
fn paint_invisible(paint: &Paint<'_>) -> bool {
    match paint {
        Paint::Color(c) => Color::to_rgba(c).components()[3] <= 0.0,
        Paint::Pattern(p) => match &**p {
            Pattern::Shading(sp) => sp.opacity <= 0.0,
            // A tiling pattern approximates to a flat gray, which is visible even
            // when the pattern was meant to be transparent; `opacity` is not carried.
            Pattern::Tiling(_) => false,
        },
    }
}

/// The paint + path + painting-operator tail of a draw, routing pattern paints through
/// the Task 6 shading machinery. `build` emits the path operators. Flat colours keep the
/// Task 3 flow (colour, path, paint operator). A shading *fill* becomes the path plus
/// the fill-rule clip and `shfill`; a shading *stroke* cannot clip-to-path and degrades
/// to its flat colour; a tiling pattern degrades everywhere (the recorded decision).
fn paint_draw(emit: &mut Emitter, transform: Affine, paint: &Paint<'_>, draw_mode: &PathDrawMode, build: impl FnOnce(&mut Emitter)) {
    match paint {
        Paint::Color(_) => {
            paint_ops(emit, paint);
            build(emit);
            paint_op(emit, draw_mode);
        }
        Paint::Pattern(p) => match (&**p, draw_mode) {
            (Pattern::Shading(_), PathDrawMode::Fill(rule)) => {
                build(emit);
                match rule {
                    FillRule::NonZero => emit.op("clip"),
                    FillRule::EvenOdd => emit.op("eoclip"),
                }
                // Back to page space: the shading's own matrix and BBox clip arrive
                // there (the interpreter pre-concats the root transform into them).
                concat_unless_identity(emit, transform.inverse());
                shading::shading_paint(paint, emit);
            }
            (Pattern::Shading(_), PathDrawMode::Stroke(_)) => {
                shading::shading_stroke_approx(paint, emit);
                build(emit);
                paint_op(emit, draw_mode);
            }
            (Pattern::Tiling(_), _) => {
                shading::shading_paint(paint, emit);
                build(emit);
                paint_op(emit, draw_mode);
            }
        },
    }
}

/// `concat` the matrix unless it is the identity, keeping the program free of no-op
/// lines (the draw `concat` itself always emits, per the Task 3 tests).
fn concat_unless_identity(emit: &mut Emitter, transform: Affine) {
    if transform != Affine::IDENTITY {
        concat(emit, transform);
    }
}

/// The painting operator for a draw mode: `fill`/`eofill` for fills, `stroke` for strokes.
fn paint_op(emit: &mut Emitter, draw_mode: &PathDrawMode) {
    match draw_mode {
        PathDrawMode::Fill(FillRule::NonZero) => emit.op("fill"),
        PathDrawMode::Fill(FillRule::EvenOdd) => emit.op("eofill"),
        PathDrawMode::Stroke(_) => emit.op("stroke"),
    }
}

/// One printable-ASCII line fragment: every other byte becomes `?` (the `dsc::note` rules).
fn ascii_safe(bytes: &[u8]) -> String {
    bytes.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '?' }).collect()
}

impl<'a> Device<'a> for PsDevice<'a> {
    /// No PostScript operator form: a soft mask degrades to one note per page.
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        if mask.is_some() {
            self.emit.note_once("transparency ignored (soft mask)");
        }
    }

    /// `Normal` (the default everywhere) is not a degradation; anything else records one
    /// note per page naming the mode.
    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        if !matches!(blend_mode, BlendMode::Normal) {
            self.emit.note_once(&format!("transparency ignored (blend {blend_mode:?})"));
        }
    }

    fn draw_path(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'a>, draw_mode: &PathDrawMode) {
        if path.is_empty() {
            return;
        }
        self.flush_text();
        if paint_invisible(paint) {
            return;
        }
        self.emit.op("gsave");
        concat(&mut self.emit, transform);
        if let PathDrawMode::Stroke(props) = draw_mode {
            stroke_ops(&mut self.emit, props);
        }
        paint_draw(&mut self.emit, transform, paint, draw_mode, |emit| path_ops(emit, path));
        self.emit.op("grestore");
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        // The interpreter delivers clip paths already transformed; emit them raw.
        self.flush_text();
        self.emit.op("gsave");
        path_ops(&mut self.emit, &clip_path.path);
        match clip_path.fill {
            FillRule::NonZero => self.emit.op("clip"),
            FillRule::EvenOdd => self.emit.op("eoclip"),
        }
    }

    /// Transparency groups composite; Level 3 PostScript has no portable equivalent, so the
    /// group's contents draw as if at full opacity, with one note per page when the group
    /// actually carried opacity, a mask or a non-Normal blend.
    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, blend_mode: BlendMode) {
        if opacity < 1.0 || mask.is_some() || !matches!(blend_mode, BlendMode::Normal) {
            self.emit.note_once("transparency ignored (transparency group)");
        }
    }

    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, draw_mode: &GlyphDrawMode) {
        // Invisible in either sense (render mode 3, or an alpha-0 paint) draws nothing.
        if paint_invisible(paint) {
            return;
        }
        // The total map from 1000-unit glyph space to the page, the same product
        // the raster renderer draws with (findings (b)).
        let total = transform * glyph_transform;
        match glyph {
            // Type 3 glyphs re-interpret their PDF drawing instructions through
            // this device as ordinary path calls — faithful, no font machinery.
            Glyph::Type3(g) => {
                self.flush_text();
                g.interpret(self, transform, glyph_transform, paint);
            }
            Glyph::Outline(outline) => match draw_mode {
                // Invisible text renders nothing (as in the raster renderer).
                GlyphDrawMode::Invisible => {}
                GlyphDrawMode::Fill => {
                    let strategy = self.fonts.strategy(glyph);
                    for note in self.fonts.drain_notes() {
                        self.emit.note(&note);
                    }
                    if matches!(strategy, TextStrategy::Type1Passthrough) {
                        let key = outline.font_cache_key();
                        let gid = outline.glyph_id().to_u32();
                        // A Type 1 code whose glyph is not in the parsed program
                        // would show the wrong glyph; that glyph vectorizes.
                        if strategy == TextStrategy::Type1Passthrough && self.fonts.type1_glyph_name(key, gid).is_none() {
                            self.emit.note("text vectorized (glyph missing from font program)");
                            self.draw_path(&outline.outline(), total, paint, &PathDrawMode::Fill(FillRule::NonZero));
                            return;
                        }
                        let code = self.fonts.char_code(key, gid);
                        let slice = self.fonts.slice_index(key, gid).unwrap_or(0);
                        self.push_run_glyph(key, slice, code, total, paint);
                    } else {
                        self.draw_path(&outline.outline(), total, paint, &PathDrawMode::Fill(FillRule::NonZero));
                    }
                }
                // Stroked (and fill-then-stroke) text becomes outline paths:
                // `show` fills, so text-as-text cannot express these modes.
                GlyphDrawMode::Stroke(props) => {
                    self.draw_path(&outline.outline(), total, paint, &PathDrawMode::Stroke(props.clone()));
                }
            },
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        // Images keep the Task 3 discipline: `gsave`, the total CTM baked in via `concat`,
        // the image machinery, `grestore`. The interpreter's `transform` already maps
        // image pixel space (row 0 first, y-flipped) onto the page, which is why the
        // PostScript ImageMatrix inside `images.rs` is the identity.
        self.flush_text();
        self.emit.op("gsave");
        concat(&mut self.emit, transform);
        images::image_ops(&image, &mut self.emit);
        self.emit.op("grestore");
    }

    fn pop_clip_path(&mut self) {
        self.flush_text();
        self.emit.op("grestore");
    }

    /// The matching pop of [`Self::push_transparency_group`]: nothing to undo, since the
    /// push emitted nothing (any degradation was noted there, once per page).
    fn pop_transparency_group(&mut self) {}

    fn draw_rect(&mut self, rect: &Rect, transform: Affine, paint: &Paint<'a>, draw_mode: &PathDrawMode) {
        self.flush_text();
        if paint_invisible(paint) {
            return;
        }
        self.emit.op("gsave");
        concat(&mut self.emit, transform);
        if let PathDrawMode::Stroke(props) = draw_mode {
            stroke_ops(&mut self.emit, props);
        }
        paint_draw(&mut self.emit, transform, paint, draw_mode, |emit| rect_ops(emit, rect));
        self.emit.op("grestore");
    }

    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        self.flush_text();
        let mut line = format!("%%BeginMarkedContent: {}", ascii_safe(tag));
        if let Some(mcid) = mcid {
            line.push_str(&format!(" mcid {mcid}"));
        }
        self.emit.op(&line);
    }

    fn end_marked_content(&mut self) {
        self.flush_text();
        self.emit.op("%%EndMarkedContent");
    }
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::color::{AlphaColor, Color};
    use hayro::hayro_interpret::{BlendMode, ClipPath, Device, FillRule, Paint, PathDrawMode, StrokeProps};
    use kurbo::{Affine, BezPath, Cap, Join, Rect};

    use super::PsDevice;
    use crate::FidelityNote;

    /// A fresh device over an empty emitter, page tag unset.
    fn device() -> PsDevice<'static> {
        PsDevice::new(None)
    }

    /// The emitted program as a string, for substring assertions.
    fn output(dev: &PsDevice) -> String {
        String::from_utf8_lossy(&dev.emit.out).into_owned()
    }

    /// A two-point path: (10, 20) → (30, 40).
    fn line_path() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((10.0, 20.0));
        path.line_to((30.0, 40.0));
        path
    }

    /// Opaque red.
    fn red() -> Paint<'static> {
        Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 1.0])))
    }

    /// A path is emitted as `newpath` + `moveto`/`lineto` and painted with `fill`, all inside
    /// `gsave`/`grestore`.
    #[test]
    fn new_path_becomes_moveto_lineto() {
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert_eq!(
            output(&dev),
            "gsave\n\
             [1 0 0 1 0 0] concat\n\
             1 0 0 setrgbcolor\n\
             newpath\n\
             10 20 moveto\n\
             30 40 lineto\n\
             fill\n\
             grestore\n"
        );
        // The even-odd fill rule paints with `eofill`.
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::EvenOdd));
        assert!(output(&dev).contains("\neofill\n"), "even-odd fill: {}", output(&dev));
    }

    /// A quadratic Bézier is elevated exactly to the equivalent cubic: with start (10, 20),
    /// control (20, 30) and end (30, 40), the cubic controls are (16.6667, 26.6667) and
    /// (23.3333, 33.3333).
    #[test]
    fn quad_curve_elevates_to_curveto() {
        let mut path = BezPath::new();
        path.move_to((10.0, 20.0));
        path.quad_to((20.0, 30.0), (30.0, 40.0));
        let mut dev = device();
        dev.draw_path(&path, Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("16.6667 26.6667 23.3333 33.3333 30 40 curveto\n"), "exact degree elevation: {}", output(&dev));
    }

    /// The draw transform is baked into the program as a `concat` with the total CTM; the path
    /// coordinates stay raw.
    #[test]
    fn transform_bakes_via_concat() {
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::translate((5.0, 7.0)), &red(), &PathDrawMode::Fill(FillRule::NonZero));
        let out = output(&dev);
        assert!(out.contains("[1 0 0 1 5 7] concat\n"), "translate as concat: {out}");
        assert!(out.contains("10 20 moveto\n") && out.contains("30 40 lineto\n"), "raw coords: {out}");

        let mut dev = device();
        let general = Affine::new([2.0, 1.0, 0.0, 3.0, 4.0, 5.0]);
        dev.draw_path(&line_path(), general, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("[2 1 0 3 4 5] concat\n"), "all six coefficients: {}", output(&dev));
    }

    /// Pushing a clip is `gsave` + the (already transformed) path + `clip`; popping restores.
    #[test]
    fn clip_push_pop_is_gsave_clip_grestore() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();

        let mut dev = device();
        dev.push_clip_path(&ClipPath { path: path.clone(), fill: FillRule::NonZero });
        dev.pop_clip_path();
        assert_eq!(
            output(&dev),
            "gsave\n\
             newpath\n\
             0 0 moveto\n\
             10 0 lineto\n\
             10 10 lineto\n\
             closepath\n\
             clip\n\
             grestore\n"
        );

        let mut dev = device();
        dev.push_clip_path(&ClipPath { path, fill: FillRule::EvenOdd });
        assert!(output(&dev).contains("\neoclip\n"), "even-odd clip: {}", output(&dev));
    }

    /// `draw_rect` (the shortcut the interpreter uses for axis-aligned rectangles) emits plain
    /// path construction operators, in both fill and stroke modes.
    #[test]
    fn rect_shortcut_emits_path_ops() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);

        let mut dev = device();
        dev.draw_rect(&rect, Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert_eq!(
            output(&dev),
            "gsave\n\
             [1 0 0 1 0 0] concat\n\
             1 0 0 setrgbcolor\n\
             newpath\n\
             10 20 moveto\n\
             30 20 lineto\n\
             30 40 lineto\n\
             10 40 lineto\n\
             closepath\n\
             fill\n\
             grestore\n"
        );

        let mut dev = device();
        dev.draw_rect(&rect, Affine::IDENTITY, &red(), &PathDrawMode::Stroke(StrokeProps::default()));
        let out = output(&dev);
        assert!(out.contains("1 setlinewidth\n"), "default width: {out}");
        assert!(out.contains("closepath\nstroke\n"), "stroke after the path: {out}");
    }

    /// Non-finite coordinates never reach the program: each becomes `0` and records a note
    /// tagged with the current page.
    #[test]
    fn non_finite_coord_emits_zero_and_note() {
        let mut path = BezPath::new();
        path.move_to((f64::NAN, 20.0));
        path.line_to((30.0, f64::INFINITY));

        let mut dev = PsDevice::new(Some(4));
        dev.draw_path(&path, Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        let out = output(&dev);
        assert!(out.contains("0 20 moveto\n") && out.contains("30 0 lineto\n"), "{out}");
        let lowered = out.to_lowercase();
        assert!(!lowered.contains("nan") && !lowered.contains("inf"), "no non-finite operand: {out}");
        assert_eq!(
            dev.emit.notes,
            vec![
                FidelityNote { page: Some(4), text: "non-finite coordinate replaced with 0".into() },
                FidelityNote { page: Some(4), text: "non-finite coordinate replaced with 0".into() },
            ]
        );
    }

    /// A flat `Paint::Color` maps to the closest device colour operator: an equal RGB triple is
    /// `setgray`, anything else `setrgbcolor`. (A source CMYK/grey colour space is not visible
    /// through `Color`'s public API — `to_rgba()` is the only view, the same 8-bit RGBA the
    /// raster renderer shows — so every flat colour arrives as one of these two.)
    #[test]
    fn flat_paint_emits_setgray_and_setrgbcolor() {
        let mut dev = device();
        let gray = Paint::Color(Color::from_rgba(AlphaColor::new([0.5, 0.5, 0.5, 1.0])));
        dev.draw_path(&line_path(), Affine::IDENTITY, &gray, &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("0.502 setgray\n"), "gray via setgray (128/255 after to_rgba): {}", output(&dev));

        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("1 0 0 setrgbcolor\n"), "rgb via setrgbcolor: {}", output(&dev));

        // Alpha is not representable in PostScript: the colour is still emitted (drawn opaque),
        // and the drop is recorded.
        let mut dev = device();
        let half_alpha = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 0.5])));
        dev.draw_path(&line_path(), Affine::IDENTITY, &half_alpha, &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("1 0 0 setrgbcolor\n"), "colour emitted despite alpha: {}", output(&dev));
        assert!(dev.emit.notes.iter().any(|n| n.text.contains("alpha")), "alpha drop recorded: {:?}", dev.emit.notes);
    }

    /// Stroke properties map onto `setlinewidth`/`setdash`/`setlinejoin`/`setlinecap` (plus
    /// `setmiterlimit`); a solid line is an explicit empty dash.
    #[test]
    fn stroke_props_map_to_operators() {
        let props = StrokeProps {
            line_width: 2.0,
            line_cap: Cap::Round,
            line_join: Join::Bevel,
            miter_limit: 5.0,
            dash_array: [3.0f32, 2.0].into_iter().collect(),
            dash_offset: 1.0,
        };
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(props));
        assert!(
            output(&dev).contains("2 setlinewidth\n[3 2] 1 setdash\n2 setlinejoin\n1 setlinecap\n5 setmiterlimit\n"),
            "props before the path: {}",
            output(&dev)
        );

        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(StrokeProps::default()));
        assert!(output(&dev).contains("[] 0 setdash\n"), "solid line as empty dash: {}", output(&dev));
    }

    /// An all-zero dash array means a solid line in PDF, and a zero-sum pattern is a
    /// rangecheck on conforming PostScript interpreters: it must normalize to `[] 0 setdash`.
    #[test]
    fn all_zero_dash_array_is_solid() {
        let props = StrokeProps { dash_array: [0.0f32, 0.0].into_iter().collect(), ..Default::default() };
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(props));
        let out = output(&dev);
        assert!(out.contains("[] 0 setdash\n"), "solid, phase meaningless: {out}");
        assert!(!out.contains("[0 0]"), "zero-sum pattern is a rangecheck: {out}");
    }

    /// Negative dash elements are invalid PDF and rangecheck in PostScript; they clamp to 0
    /// (recorded), keeping the pattern's alternating shape valid.
    #[test]
    fn negative_dash_element_clamps_with_note() {
        let props = StrokeProps { dash_array: [-3.0f32, 2.0].into_iter().collect(), dash_offset: 1.0, ..Default::default() };
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(props));
        assert!(output(&dev).contains("[0 2] 1 setdash\n"), "negative element clamped, pattern kept: {}", output(&dev));
        assert_eq!(dev.emit.notes, vec![FidelityNote { page: None, text: "negative dash element replaced with 0".into() }]);
    }

    /// A negative dash phase is reduced modulo the pattern length (visually identical) so the
    /// program never carries a negative phase; a solid line ignores the phase entirely.
    #[test]
    fn negative_dash_phase_reduces_modulo_pattern() {
        let props = StrokeProps { dash_array: [3.0f32, 2.0].into_iter().collect(), dash_offset: -1.0, ..Default::default() };
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(props));
        // -1 mod (3+2) = 4.
        assert!(output(&dev).contains("[3 2] 4 setdash\n"), "phase reduced: {}", output(&dev));

        let props = StrokeProps { dash_array: [3.0f32, 2.0].into_iter().collect(), dash_offset: -6.0, ..Default::default() };
        let mut dev = device();
        dev.draw_path(&line_path(), Affine::IDENTITY, &red(), &PathDrawMode::Stroke(props));
        // -6 mod 5 = 4.
        assert!(output(&dev).contains("[3 2] 4 setdash\n"), "phase reduced twice: {}", output(&dev));
        assert!(!output(&dev).contains("-6 setdash"), "no negative phase: {}", output(&dev));
    }

    /// Alpha-0 flat paint is invisible (PDF Separation "None" paints nothing): the draw
    /// is skipped entirely — no operators, no transparency note (skipping is exactly
    /// what the raster renderer shows, not a degradation). Alpha between 0 and 1 keeps
    /// the Task 3 behaviour (emitted opaque + note).
    #[test]
    fn alpha_zero_paint_skips_draw() {
        let mut dev = device();
        let invisible = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 0.0])));
        dev.draw_path(&line_path(), Affine::IDENTITY, &invisible, &PathDrawMode::Fill(FillRule::NonZero));
        dev.draw_rect(&Rect::new(0.0, 0.0, 10.0, 10.0), Affine::IDENTITY, &invisible, &PathDrawMode::Stroke(StrokeProps::default()));
        assert_eq!(output(&dev), "", "invisible paints draw nothing: {}", output(&dev));
        assert_eq!(dev.emit.notes, vec![], "and note nothing: {:?}", dev.emit.notes);

        let mut dev = device();
        let half = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 0.5])));
        dev.draw_path(&line_path(), Affine::IDENTITY, &half, &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("1 0 0 setrgbcolor\n"), "partial alpha still emits: {}", output(&dev));
    }

    /// Marked content becomes DSC-style comment lines (printable ASCII only), not operators.
    #[test]
    fn marked_content_becomes_dsc_comments() {
        let mut dev = device();
        dev.begin_marked_content(b"P", None);
        dev.begin_marked_content(b"Span", Some(12));
        dev.begin_marked_content(b"caf\xe9", None);
        dev.end_marked_content();
        dev.end_marked_content();
        dev.end_marked_content();
        assert_eq!(
            output(&dev),
            "%%BeginMarkedContent: P\n\
             %%BeginMarkedContent: Span mcid 12\n\
             %%BeginMarkedContent: caf?\n\
             %%EndMarkedContent\n\
             %%EndMarkedContent\n\
             %%EndMarkedContent\n"
        );
    }

    /// The transparency methods emit no operators and only note actual degradations (the
    /// default blend mode, absent mask and full-opacity group are not degradations) — once
    /// per page, however often the interpreter re-announces them.
    #[test]
    fn transparency_stubs_note_and_emit_nothing() {
        let mut dev = device();
        dev.set_blend_mode(BlendMode::Multiply);
        dev.set_blend_mode(BlendMode::Multiply);
        dev.set_blend_mode(BlendMode::Normal);
        dev.set_soft_mask(None);
        dev.push_transparency_group(1.0, None, BlendMode::Normal);
        dev.pop_transparency_group();
        assert_eq!(output(&dev), "", "defaults emit nothing: {}", output(&dev));
        assert_eq!(dev.emit.notes, vec![FidelityNote { page: None, text: "transparency ignored (blend Multiply)".into() }]);

        let mut dev = device();
        dev.push_transparency_group(0.5, None, BlendMode::Normal);
        dev.push_transparency_group(0.5, None, BlendMode::Normal);
        let group_notes = dev.emit.notes.iter().filter(|n| n.text.contains("transparency group")).count();
        assert_eq!(group_notes, 1, "one group note per page, first occurrence only: {:?}", dev.emit.notes);
    }
}
