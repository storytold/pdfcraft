//! The PostScript `Device`: geometry, colour and clip emission (Layer A, Task 3).
//!
//! Emission model: PDF user space *is* PostScript user space (the initial transform is the
//! identity handoff — see the findings file), so nothing outside a draw ever changes the CTM.
//! Every draw is self-contained: `gsave`, the total CTM baked in via `concat`, the path
//! coordinates raw, the paint operator, `grestore`. Clip paths arrive from the interpreter
//! already transformed into that space, so they are emitted raw inside their own `gsave` and
//! released by the matching `grestore`. Text, images, shadings/patterns and transparency
//! degrade to fidelity notes here; Tasks 4–7 wire them.

// The device's first live constructor arrives with Task 7's `export()` assembly; until then
// only the tests build one, so the emission chain is dead code in the non-test lib target.
#![allow(dead_code)]

use std::marker::PhantomData;

use hayro::hayro_interpret::color::Color;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::{BlendMode, ClipPath, Device, FillRule, GlyphDrawMode, Image, Paint, PathDrawMode, SoftMask, StrokeProps};
use kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect};

use crate::FidelityNote;
use crate::psnum;

/// The shared output collector: operator bytes, fidelity notes, and the page the notes refer to.
#[derive(Default)]
pub(crate) struct Emitter {
    /// The PostScript program emitted so far.
    pub out: Vec<u8>,
    /// Every degradation recorded so far.
    pub notes: Vec<FidelityNote>,
    /// The 1-based page the emitter is currently inside, used to tag notes.
    pub page: Option<usize>,
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
    fn number(&mut self, v: f64) -> String {
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
}

/// The PostScript `Device` implementation.
pub(crate) struct PsDevice<'a> {
    emit: Emitter,
    /// Later tasks consume `&'a` PDF data (font programs, image streams); nothing is stored yet.
    _marker: PhantomData<&'a ()>,
}

impl PsDevice<'_> {
    /// A device emitting into a fresh buffer, tagging notes with `page` (1-based, if known).
    pub(crate) fn new(page: Option<usize>) -> Self {
        Self { emit: Emitter { out: Vec::new(), notes: Vec::new(), page }, _marker: PhantomData }
    }
}

/// Bake a draw transform in as a `concat` with its six coefficients.
fn concat(emit: &mut Emitter, transform: Affine) {
    for c in transform.as_coeffs() {
        emit.coord(c);
    }
    emit.op("concat");
}

/// Emit `newpath` plus the path's elements as `moveto`/`lineto`/`curveto`/`closepath`.
/// Quadratic Béziers have no PostScript operator and are elevated exactly to the equivalent
/// cubic (degree elevation is lossless, so it is not a degradation).
fn path_ops(emit: &mut Emitter, path: &BezPath) {
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

    let mut line = String::from("[");
    for (i, d) in props.dash_array.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&emit.number(f64::from(*d)));
    }
    line.push_str("] ");
    line.push_str(&emit.number(f64::from(props.dash_offset)));
    line.push_str(" setdash");
    emit.op(&line);

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
/// and `setrgbcolor` otherwise; alpha is drawn opaque and the drop recorded. Pattern/shading
/// paints are Task 6's; until then they record a note and emit no colour operator.
fn paint_ops(emit: &mut Emitter, paint: &Paint<'_>) {
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
        Paint::Pattern(_) => emit.note("paint not yet emitted (pattern/shading)"),
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
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        if mask.is_some() {
            self.emit.note("transparency ignored (soft mask)");
        }
    }

    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        if !matches!(blend_mode, BlendMode::Normal) {
            self.emit.note(&format!("transparency ignored (blend {blend_mode:?})"));
        }
    }

    fn draw_path(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'a>, draw_mode: &PathDrawMode) {
        if path.is_empty() {
            return;
        }
        self.emit.op("gsave");
        concat(&mut self.emit, transform);
        if let PathDrawMode::Stroke(props) = draw_mode {
            stroke_ops(&mut self.emit, props);
        }
        paint_ops(&mut self.emit, paint);
        path_ops(&mut self.emit, path);
        paint_op(&mut self.emit, draw_mode);
        self.emit.op("grestore");
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        // The interpreter delivers clip paths already transformed; emit them raw.
        self.emit.op("gsave");
        path_ops(&mut self.emit, &clip_path.path);
        match clip_path.fill {
            FillRule::NonZero => self.emit.op("clip"),
            FillRule::EvenOdd => self.emit.op("eoclip"),
        }
    }

    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, blend_mode: BlendMode) {
        if opacity < 1.0 || mask.is_some() || !matches!(blend_mode, BlendMode::Normal) {
            self.emit.note("transparency ignored (transparency group)");
        }
    }

    fn draw_glyph(&mut self, _glyph: &Glyph<'a>, _transform: Affine, _glyph_transform: Affine, _paint: &Paint<'a>, _draw_mode: &GlyphDrawMode) {
        // Task 5 replaces this with Type 1/Type 42 text and vectorized fallbacks.
        self.emit.note("glyph skipped (text not implemented yet)");
    }

    fn draw_image(&mut self, _image: Image<'a, '_>, _transform: Affine) {
        // Task 4 replaces this with filter passthrough and re-encoding.
        self.emit.note("image skipped (not implemented yet)");
    }

    fn pop_clip_path(&mut self) {
        self.emit.op("grestore");
    }

    fn pop_transparency_group(&mut self) {}

    fn draw_rect(&mut self, rect: &Rect, transform: Affine, paint: &Paint<'a>, draw_mode: &PathDrawMode) {
        self.emit.op("gsave");
        concat(&mut self.emit, transform);
        if let PathDrawMode::Stroke(props) = draw_mode {
            stroke_ops(&mut self.emit, props);
        }
        paint_ops(&mut self.emit, paint);
        rect_ops(&mut self.emit, rect);
        paint_op(&mut self.emit, draw_mode);
        self.emit.op("grestore");
    }

    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        let mut line = format!("%%BeginMarkedContent: {}", ascii_safe(tag));
        if let Some(mcid) = mcid {
            line.push_str(&format!(" mcid {mcid}"));
        }
        self.emit.op(&line);
    }

    fn end_marked_content(&mut self) {
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
             1 0 0 1 0 0 concat\n\
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
        assert!(out.contains("1 0 0 1 5 7 concat\n"), "translate as concat: {out}");
        assert!(out.contains("10 20 moveto\n") && out.contains("30 40 lineto\n"), "raw coords: {out}");

        let mut dev = device();
        let general = Affine::new([2.0, 1.0, 0.0, 3.0, 4.0, 5.0]);
        dev.draw_path(&line_path(), general, &red(), &PathDrawMode::Fill(FillRule::NonZero));
        assert!(output(&dev).contains("2 1 0 3 4 5 concat\n"), "all six coefficients: {}", output(&dev));
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
             1 0 0 1 0 0 concat\n\
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

    /// The transparency stubs emit no operators and only note actual degradations (the default
    /// blend mode, absent mask and full-opacity group are not degradations).
    #[test]
    fn transparency_stubs_note_and_emit_nothing() {
        let mut dev = device();
        dev.set_blend_mode(BlendMode::Multiply);
        dev.set_blend_mode(BlendMode::Normal);
        dev.set_soft_mask(None);
        dev.push_transparency_group(1.0, None, BlendMode::Normal);
        dev.pop_transparency_group();
        assert_eq!(output(&dev), "", "defaults emit nothing: {}", output(&dev));
        assert_eq!(dev.emit.notes, vec![FidelityNote { page: None, text: "transparency ignored (blend Multiply)".into() }]);

        let mut dev = device();
        dev.push_transparency_group(0.5, None, BlendMode::Normal);
        assert!(dev.emit.notes.iter().any(|n| n.text.contains("transparency group")), "group opacity noted: {:?}", dev.emit.notes);
    }
}
