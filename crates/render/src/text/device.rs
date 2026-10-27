use super::layout::{Flow, along_baseline, arrange_flow, compatible, continues, from_upright, to_upright, unrolled};
use super::*;

/// Largest sine of the angle between a glyph's writing direction and a flow's for the glyph to be
/// laid out in that flow's frame. Turning an em box by θ moves its edges along the line by up to
/// h·sin θ, so below the layout's word-gap tolerance (`WORD_GAP` · h) the boxes still read as they
/// would unturned.
pub(super) const DIRECTION_TOLERANCE: f64 = WORD_GAP as f64;

/// A word gap is wider than the line's typical letter gap by this fraction of the line height.
pub(super) const WORD_GAP: f32 = 0.15;

/// A glyph with its writing direction to the nearest quarter turn (right, down, left, up) and as
/// a unit vector.
pub(super) type Drawn = (usize, [f64; 2], TextGlyph);

pub(super) struct TextDevice {
    /// Every glyph in content order.
    pub(super) glyphs: Vec<Drawn>,
    /// How many marked content sequences are open.
    depth: usize,
    /// The outermost open `/ActualText` replacement: its depth, text and the glyphs drawn inside
    /// it (with no text of their own yet).
    actual: Option<(usize, String, Vec<Drawn>)>,
    /// What the open `/ActualText` replacement paints besides glyphs (paths, images), as far as
    /// it can be seen: where its text reads when it marks no glyph (words drawn as outlines).
    painted: Option<Painted>,
    /// Whether the open `/ActualText` replacement paints anything an exclusion leaves out.
    paint_filtered: bool,
    /// The bounding boxes of the clip paths in force (view space), innermost last, each cut to the
    /// one before it: an outer bound on what can show.
    clips: Vec<Rect>,
    /// Every `/ActualText` replacement made on the page.
    pub(super) replaced: Vec<Replaced>,
    filter: TextFilter,
    excluded_depth: Option<usize>,
    actual_filtered: bool,
    partial_replacement: bool,
}

/// An `/ActualText` replacement: the glyph standing for it in `TextDevice::glyphs` (none for an
/// empty replacement), its text, and the text and box of each replaced glyph that has Unicode of
/// its own.
pub(super) struct Replaced {
    pub(super) at: Option<usize>,
    pub(super) text: String,
    pub(super) originals: Vec<(String, [f32; 4])>,
}

/// Grid cell size, in points, for finding the glyphs drawn at a place.
const CELL: f32 = 8.0;

/// Most grid cells searched around one place: a glyph so tall its tolerance spans more is skipped.
const MAX_CELLS: i64 = 4096;

/// Most work (grid cells visited plus glyphs compared) the redraw dedupe in `TextDevice::finish`
/// may do on one page. A hostile page stacking ~10^5 glyphs and ~10^5 replacements at one spot
/// would otherwise make ~10^10 comparisons; past the budget the remaining replacements keep
/// their unmarked redraws (the text reads twice, as it would without `/ActualText`).
const MAX_REDRAW_WORK: usize = 1 << 22;

/// Most replacement text bytes the redraw dedupe in `TextDevice::finish` copies on one page.
pub(super) const MAX_REDRAW_TEXT: usize = 1 << 22;

#[cfg(test)]
thread_local! {
    /// Replacement text bytes given to redraws on this thread.
    pub(super) static REDRAW_TEXT_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn cell(v: f32) -> i64 {
    // Glyph boxes are finite (`draw_glyph` and `merge` drop the rest); `as` saturates.
    (v / CELL).floor() as i64
}

/// How far a redrawn glyph may sit from the one it redraws: a fifth of its height, as in `redraws`.
fn redraw_tolerance(rect: &[f32; 4]) -> f32 {
    0.2 * (rect[3] - rect[1]).max(0.1)
}

/// One glyph carrying `text`, boxed around `drawn` in their writing direction; `None` when they
/// span several lines or writing directions (one box would break selection and line grouping).
fn merge(drawn: &[Drawn], text: String) -> Option<Drawn> {
    let first = drawn.first()?;
    let (quarter, u) = (first.0, first.1);
    // Across the line: from the writing direction a quarter turn towards the line's bottom.
    let n = [-u[1], u[0]];
    let along = |p: &[f32; 2]| f64::from(p[0]) * u[0] + f64::from(p[1]) * u[1];
    let across = |p: &[f32; 2]| f64::from(p[0]) * n[0] + f64::from(p[1]) * n[1];
    let height = across(&first.2.quad[0]) - across(&first.2.quad[3]);
    let (mut s0, mut s1, mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for p in drawn.iter().flat_map(|d| d.2.quad.iter()) {
        (s0, s1) = (s0.min(along(p)), s1.max(along(p)));
        (t0, t1) = (t0.min(across(p)), t1.max(across(p)));
    }
    let one_line = drawn.iter().all(|d| compatible(u, d.1)) && t1 - t0 <= 2.0 * height.abs();
    if !(one_line && s0.is_finite() && s1.is_finite() && t0.is_finite() && t1.is_finite()) {
        return None;
    }
    // The edge towards the next line is the one the first glyph's quad starts on.
    let (near, far) = if height >= 0.0 { (t1, t0) } else { (t0, t1) };
    let point = |s: f64, t: f64| [(s * u[0] + t * n[0]) as f32, (s * u[1] + t * n[1]) as f32];
    let quad = [point(s0, near), point(s1, near), point(s1, far), point(s0, far)];
    let rect = quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
        [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
    });
    Some((quarter, u, TextGlyph { text, rect, quad, direction: first.2.direction }))
}

/// The widest or tallest box (view-space points) a glyph, or the paint standing in for one, may
/// have and still be placed: anything bigger is damage, not text.
const MAX_GLYPH_EXTENT: f64 = 10_000.0;

/// The quarter turn of direction `(dx, dy)` (view space, y down): 0 right, 1 down, 2 left, 3 up.
fn quarter_turn(dx: f64, dy: f64) -> usize {
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 { 0 } else { 2 }
    } else if dy > 0.0 {
        1
    } else {
        3
    }
}

/// What an `/ActualText` replacement paints besides glyphs, measured as [`merge`] measures
/// glyphs: along the writing direction of the first of it (its transform's x axis, `u`) and
/// across it.
struct Painted {
    quarter: usize,
    u: [f64; 2],
    along: [f64; 2],
    across: [f64; 2],
}

impl Painted {
    fn new(transform: Affine) -> Self {
        let [a, b, ..] = transform.as_coeffs();
        let length = a.hypot(b);
        let u = if length.is_finite() && length > 0.0 { [a / length, b / length] } else { [1.0, 0.0] };
        let empty = [f64::INFINITY, f64::NEG_INFINITY];
        Painted { quarter: quarter_turn(u[0], u[1]), u, along: empty, across: empty }
    }

    /// Grow the measure by the view-space box `b`.
    fn add(&mut self, b: Rect) {
        let n = [-self.u[1], self.u[0]];
        for (x, y) in [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)] {
            let (s, t) = (x * self.u[0] + y * self.u[1], x * n[0] + y * n[1]);
            self.along = [self.along[0].min(s), self.along[1].max(s)];
            self.across = [self.across[0].min(t), self.across[1].max(t)];
        }
    }

    /// A stand-in glyph with no text of its own, boxed around what was painted and read along its
    /// direction; `None` when that is too big to be the place of words.
    fn glyph(self) -> Option<Drawn> {
        let ([s0, s1], [t0, t1]) = (self.along, self.across);
        // Too big to be the place of words, or NaN.
        if !(s1 - s0 <= MAX_GLYPH_EXTENT && t1 - t0 <= MAX_GLYPH_EXTENT) {
            return None;
        }
        let (u, n) = (self.u, [-self.u[1], self.u[0]]);
        let point = |s: f64, t: f64| [(s * u[0] + t * n[0]) as f32, (s * u[1] + t * n[1]) as f32];
        // The edge towards the next line starts the quad, as a glyph's em box does.
        let quad = [point(s0, t1), point(s1, t1), point(s1, t0), point(s0, t0)];
        let rect = quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
            [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
        });
        rect.iter()
            .all(|v| v.is_finite())
            .then(|| (self.quarter, u, TextGlyph { text: String::new(), rect, quad, direction: [u[0] as f32, u[1] as f32] }))
    }
}

/// The view-space box of `path` drawn with `transform`, from its control points (as redaction
/// measures a path), or `None` when it has no finite point.
fn control_box(path: &BezPath, transform: Affine) -> Option<Rect> {
    let mut b: Option<Rect> = None;
    let mut add = |p: kurbo::Point| {
        let q = transform * p;
        if q.is_finite() {
            b = Some(b.map_or_else(|| Rect::from_points(q, q), |r| r.union_pt(q)));
        }
    };
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => add(p),
            kurbo::PathEl::QuadTo(p1, p2) => [p1, p2].into_iter().for_each(&mut add),
            kurbo::PathEl::CurveTo(p1, p2, p3) => [p1, p2, p3].into_iter().for_each(&mut add),
            kurbo::PathEl::ClosePath => {}
        }
    }
    b
}

impl TextDevice {
    pub(super) fn new() -> Self {
        TextDevice {
            glyphs: Vec::new(),
            depth: 0,
            actual: None,
            painted: None,
            paint_filtered: false,
            clips: Vec::new(),
            replaced: Vec::new(),
            filter: TextFilter::default(),
            excluded_depth: None,
            actual_filtered: false,
            partial_replacement: false,
        }
    }

    /// Ends the open `/ActualText` replacement (ISO 32000-2 §14.9.4): its glyphs become one glyph
    /// carrying the replacement text, boxed around them all. Chrome (Skia) writes one around
    /// each Thai, Indic or Arabic cluster whose shaped glyphs (a tone mark moved clear of a vowel,
    /// a ligature) have no Unicode of their own, so without it those marks are lost and the
    /// words can't be found.
    ///
    /// A replacement spanning several lines or writing directions stays per glyph (each glyph's own
    /// Unicode). An empty one (a soft hyphen at a line break, say) stands for no text at all. One
    /// that marks no glyph reads where it paints, as far as that can be seen: words drawn as
    /// outlines or pictures.
    fn end_actual_text(&mut self) {
        let Some((_, text, mut drawn)) = self.actual.take() else { return };
        let painted = self.painted.take();
        let paint_filtered = std::mem::take(&mut self.paint_filtered);
        if std::mem::take(&mut self.actual_filtered) {
            // ActualText has no mapping from replacement words to individual glyphs. A mixed
            // span cannot be filtered honestly; a wholly excluded span can simply be omitted.
            self.partial_replacement |= !drawn.is_empty();
            return;
        }
        if drawn.is_empty()
            && !self.filter.painted
            && let Some(stand_in) = painted.and_then(Painted::glyph)
        {
            // The paint places it, so paint left out mixes it as excluded glyphs would.
            if paint_filtered {
                self.partial_replacement = true;
                return;
            }
            drawn.push(stand_in);
        }
        let Some(glyph) = merge(&drawn, text.clone()) else {
            self.glyphs.extend(drawn.into_iter().filter(|d| !d.2.text.chars().all(char::is_control)));
            return;
        };
        let originals = drawn.iter().filter(|d| !d.2.text.chars().all(char::is_control)).map(|d| (d.2.text.clone(), d.2.rect)).collect();
        let at = if text.chars().all(char::is_control) {
            None
        } else {
            self.glyphs.push(glyph);
            Some(self.glyphs.len() - 1)
        };
        self.replaced.push(Replaced { at, text, originals });
    }

    /// Record view-space box `b`, drawn with `transform`, as painted by the open `/ActualText`
    /// replacement, cut to the visible area; paint an exclusion leaves out (`excluded`) only marks
    /// the replacement.
    fn paint(&mut self, b: Rect, transform: Affine, excluded: bool) {
        let seen = match self.clips.last() {
            Some(c) => {
                let (x0, y0, x1, y1) = (b.x0.max(c.x0), b.y0.max(c.y0), b.x1.min(c.x1), b.y1.min(c.y1));
                // Clipped away entirely: nothing of it shows.
                if !(x0 <= x1 && y0 <= y1) {
                    return;
                }
                Rect::new(x0, y0, x1, y1)
            }
            None => b,
        };
        if excluded {
            self.paint_filtered = true;
        } else {
            self.painted.get_or_insert_with(|| Painted::new(transform)).add(seen);
        }
    }

    /// Ends the page. Some producers draw text twice, the copy for an outline, shadow or
    /// glow, and mark only one drawing with `/ActualText` (Canva). The unmarked redraw of a
    /// replacement's glyphs (the same text at the same places, in content order) takes the same
    /// replacement, so the two read alike and line layout drops the copy as a redraw.
    pub(super) fn finish(&mut self) {
        self.end_actual_text();
        if self.replaced.is_empty() {
            return;
        }
        let mut taken = vec![false; self.glyphs.len()];
        for at in self.replaced.iter().filter_map(|r| r.at) {
            if let Some(t) = taken.get_mut(at) {
                *t = true;
            }
        }
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if !taken.get(i).copied().unwrap_or(true) {
                grid.entry((cell(g.2.rect[0]), cell(g.2.rect[1]))).or_default().push(i);
            }
        }
        // Redraws found: the glyph range and the replacement standing for it.
        let mut redraws: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
        let mut budget = MAX_REDRAW_WORK;
        'replacements: for (k, r) in self.replaced.iter().enumerate() {
            let Some((_, rect)) = r.originals.first() else { continue };
            let tol = redraw_tolerance(rect);
            let (x0, x1, y0, y1) = (cell(rect[0] - tol), cell(rect[0] + tol), cell(rect[1] - tol), cell(rect[1] + tol));
            if x1.saturating_sub(x0).saturating_add(1).saturating_mul(y1.saturating_sub(y0).saturating_add(1)) > MAX_CELLS {
                continue;
            }
            // Checking one candidate walks its run twice (free, then same).
            let candidate_cost = r.originals.len().saturating_mul(2).saturating_add(1);
            for key in (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))) {
                let Some(rest) = budget.checked_sub(1) else { break 'replacements };
                budget = rest;
                for &i in grid.get(&key).map_or(&[][..], Vec::as_slice) {
                    let Some(rest) = budget.checked_sub(candidate_cost) else { break 'replacements };
                    budget = rest;
                    let Some(range) = i.checked_add(r.originals.len()).map(|end| i..end) else { continue };
                    let Some(run) = self.glyphs.get(range.clone()) else { continue };
                    let free = taken.get(range.clone()).is_some_and(|t| t.iter().all(|t| !t));
                    let same = run.iter().zip(&r.originals).all(|(g, (text, o))| {
                        let tol = redraw_tolerance(o);
                        g.2.text == *text && (g.2.rect[0] - o[0]).abs() < tol && (g.2.rect[1] - o[1]).abs() < tol
                    });
                    if free && same {
                        if let Some(t) = taken.get_mut(range.clone()) {
                            t.fill(true);
                        }
                        redraws.push((range, k));
                    }
                }
            }
        }
        if redraws.is_empty() {
            return;
        }
        redraws.sort_by_key(|(range, _)| range.start);
        let glyphs = std::mem::take(&mut self.glyphs);
        let mut redraws = redraws.into_iter().peekable();
        let mut text_left = MAX_REDRAW_TEXT;
        let mut i = 0;
        while let Some(g) = glyphs.get(i) {
            let Some((range, k)) = redraws.next_if(|(range, _)| range.start == i) else {
                self.glyphs.push(g.clone());
                i += 1;
                continue;
            };
            let run = glyphs.get(range.clone()).unwrap_or_default();
            let replacement = self.replaced.get(k).map_or("", |r| r.text.as_str());
            // Each redraw takes its own copy of the replacement, and one long replacement can
            // have many redraws: past the budget a redraw keeps its glyphs, as past the work budget.
            let Some(left) = text_left.checked_sub(replacement.len()) else {
                self.glyphs.extend_from_slice(run);
                i = range.end.max(i + 1);
                continue;
            };
            text_left = left;
            let text = replacement.to_owned();
            #[cfg(test)]
            REDRAW_TEXT_BYTES.with(|c| c.set(c.get() + text.len()));
            if !text.chars().all(char::is_control) {
                match merge(run, text) {
                    Some(glyph) => self.glyphs.push(glyph),
                    None => self.glyphs.extend_from_slice(run),
                }
            }
            i = range.end.max(i + 1);
        }
    }
}

/// Whether `PDFCRAFT_TEXT_DEBUG` is set. `draw_glyph` runs once per glyph and `getenv` scans the
/// whole environment under a lock, so read it once.
fn text_debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some())
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, path: &BezPath, transform: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        // A path is no text, but it places an `/ActualText` replacement that marks no glyph.
        if self.actual.is_some()
            && let Some(b) = control_box(path, transform)
        {
            self.paint(b, transform, self.excluded_depth.is_some());
        }
    }
    fn push_clip_path(&mut self, clip: &ClipPath) {
        // Clip paths arrive in view space. NaN edges (a damaged clip) leave that edge unclipped.
        let b = clip.path.bounding_box();
        let visible = match self.clips.last() {
            Some(c) => Rect::new(c.x0.max(b.x0), c.y0.max(b.y0), c.x1.min(b.x1), c.y1.min(b.y1)),
            None => b,
        };
        self.clips.push(visible);
    }
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, mode: &GlyphDrawMode) {
        if self.excluded_depth.is_some() || (self.filter.images && matches!(mode, GlyphDrawMode::Invisible)) {
            self.actual_filtered |= self.actual.is_some();
            return;
        }
        // Inside an `/ActualText` replacement only the glyph's box matters.
        let replaced = self.actual.is_some();
        let text = match glyph.as_unicode() {
            Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => c.to_string(),
            Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => s,
            None if replaced => String::new(),
            None => {
                if text_debug() {
                    eprintln!(
                        "NOUNICODE {} {:?}",
                        if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" },
                        (transform * glyph_transform).as_coeffs()
                    );
                }
                return;
            }
        };
        if !replaced && text.chars().all(char::is_control) {
            return;
        }
        // Glyph space → view space. Glyph space uses 1000 units per em for outline glyphs; the em
        // box spans descender (−200) to ascender (800), the advance gives the width.
        let t = transform * glyph_transform;
        if text_debug() {
            let kind = if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" };
            eprintln!("{kind} {text:?} transform={:?} glyph_transform={:?}", transform.as_coeffs(), glyph_transform.as_coeffs());
        }
        let advance = match glyph {
            Glyph::Outline(g) => {
                g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0))
            }
            Glyph::Type3(g) => g.advance_width().filter(|a| a.is_finite() && *a > 0.0).map(f64::from).unwrap_or(600.0),
        };
        // The baseline direction in view space (y down).
        let coefficients = t.as_coeffs();
        let (dx, dy) = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            // PDF vertical fonts advance along negative glyph-space y (WMode 1), while
            // keeping the glyph outline upright. Its x axis is not the writing direction.
            (-coefficients[2], -coefficients[3])
        } else {
            (coefficients[0], coefficients[1])
        };
        let dir = quarter_turn(dx, dy);
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let corners = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            [(em.x0, em.y1), (em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1)]
        } else {
            [(em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1), (em.x0, em.y1)]
        }
        .map(|(x, y)| t * kurbo::Point::new(x, y));
        let b = corners.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
            Rect::new(b.x0.min(p.x), b.y0.min(p.y), b.x1.max(p.x), b.y1.max(p.y))
        });
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite())
            || b.width() > MAX_GLYPH_EXTENT
            || b.height() > MAX_GLYPH_EXTENT
        {
            return;
        }
        let length = dx.hypot(dy);
        let direction = if length.is_finite() && length > 0.0 { [dx / length, dy / length] } else { [1.0, 0.0] };
        // A glyph whose Unicode is several characters (a ligature such as "ffi") stays one glyph
        // with its own box: the PDF gives no position for the characters inside it.
        let glyph = TextGlyph {
            text,
            rect: [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32],
            quad: corners.map(|p| [p.x as f32, p.y as f32]),
            direction: direction.map(|d| d as f32),
        };
        match &mut self.actual {
            Some((_, _, drawn)) => drawn.push((dir, direction, glyph)),
            None => self.glyphs.push((dir, direction, glyph)),
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        // A picture of words, like an outline, places a replacement that marks no glyph; with
        // images excluded, it is not counted.
        if self.actual.is_none() {
            return;
        }
        // The transform places the picture's pixels, not a unit square.
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| transform * kurbo::Point::new(x, y));
        let [a, b, c, d] = corners;
        if corners.iter().all(|p| p.is_finite()) {
            self.paint(Rect::from_points(a, c).union_pt(b).union_pt(d), transform, self.excluded_depth.is_some() || self.filter.images);
        }
    }
    fn pop_clip_path(&mut self) {
        self.clips.pop();
    }
    fn pop_transparency_group(&mut self) {}
    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        self.depth = self.depth.saturating_add(1);
        if self.excluded_depth.is_none() && self.filter.excludes(tag, mcid) {
            self.excluded_depth = Some(self.depth);
        }
    }
    fn actual_text(&mut self, text: &[u8]) {
        // A replacement nested in another is already covered by the outer one.
        if self.actual.is_none() && self.excluded_depth.is_none() {
            self.actual = Some((self.depth, pdfcraft_cos::PdfString::literal(text).to_text(), Vec::new()));
        }
    }
    fn end_marked_content(&mut self) {
        if self.actual.as_ref().is_some_and(|(depth, _, _)| *depth == self.depth) {
            self.end_actual_text();
        }
        if self.excluded_depth == Some(self.depth) {
            self.excluded_depth = None;
        }
        self.depth = self.depth.saturating_sub(1);
    }
    fn begin_annotations(&mut self) {
        // A replacement the page content never closed ends with it: the annotations drawn next
        // are not part of it.
        self.end_actual_text();
    }
}

/// Extract the text of one page. Panics inside the interpreter are the caller's to catch.
pub(crate) fn extract_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Option<PageText> {
    extract_page_filtered(pdf, page, settings, TextFilter::default()).ok()
}

pub(crate) fn extract_page_filtered(pdf: &Pdf, page: usize, settings: &InterpreterSettings, filter: TextFilter) -> Result<PageText, String> {
    let pages = pdf.pages();
    let p = pages.get(page).ok_or("the page could not be read")?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // Annotation appearances are included: form-field values and comment text are searchable,
    // as in Acrobat, and match what other extractors report.
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, p.xref(), settings);
    let mut dev = TextDevice::new();
    dev.filter = filter;
    interpret_page(p, &mut ctx, &mut dev);
    // A replacement the content never closed ends with the page.
    dev.finish();
    if dev.partial_replacement {
        return Err("a replacement text span mixes included and excluded content; disable the exclusions to count this page".into());
    }
    let glyphs = dev.glyphs;
    // Text on a curved path: glyphs that each continue the previous one's line while the
    // direction turns. Each such run is one line.
    let turned = |u: [f64; 2]| u[0].abs().min(u[1].abs()) > DIRECTION_TOLERANCE;
    let mut in_curve = vec![false; glyphs.len()];
    // Turned text: curved runs, and straight flows along one writing direction. The two are kept
    // apart so finding a glyph's direction scans only the directions (a few dozen at most: each
    // differs from the others by more than the tolerance), not every curved run.
    let mut curves: Vec<Vec<usize>> = Vec::new();
    let mut straight: Vec<([f64; 2], Vec<usize>)> = Vec::new();
    let mut start = 0;
    for end in 1..=glyphs.len() {
        if end < glyphs.len() && continues(&glyphs[end - 1], &glyphs[end]) {
            continue;
        }
        let run = &glyphs[start..end];
        if run.iter().any(|g| turned(g.1)) && run.iter().any(|g| !compatible(run[0].1, g.1)) {
            in_curve[start..end].fill(true);
            curves.push((start..end).collect());
        }
        start = end;
    }
    let mut quarters: [Vec<usize>; 4] = Default::default();
    for (i, (quarter, u, _)) in glyphs.iter().enumerate() {
        if in_curve[i] {
            continue;
        }
        if !turned(*u) {
            if let Some(q) = quarters.get_mut(*quarter) {
                q.push(i);
            }
        } else if let Some((_, ids)) = straight.iter_mut().find(|(d, _)| compatible(*d, *u)) {
            ids.push(i);
        } else {
            straight.push((*u, vec![i]));
        }
    }
    // A horizontal heading must not force a vertical body back into horizontal line grouping.
    // Group each writing direction independently, then join flows from their topmost position.
    let upright = |quarters: [Vec<usize>; 4]| {
        let mut flows: Vec<Flow> = quarters
            .into_iter()
            .enumerate()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(quarter, ids)| {
                let rects = ids.iter().map(|&i| to_upright(glyphs[i].2.rect, quarter as u8, w, h)).collect();
                arrange_flow(&glyphs, ids, rects, Some(quarter as u8))
            })
            .collect();
        flows.sort_by(|a, b| a.top.total_cmp(&b.top));
        flows
    };
    let flows = upright(quarters);
    let mut extra: Vec<Flow> = curves
        .into_iter()
        .map(|ids| (None, ids))
        .chain(straight.into_iter().map(|(u, ids)| (Some(u), ids)))
        .map(|(u, ids)| {
            let rects = match u {
                // Turned text is laid out along its own baseline.
                Some(u) => ids.iter().map(|&i| along_baseline(&glyphs[i].2.quad, u).map(|c| c as f32)).collect(),
                None => unrolled(ids.iter().map(|&i| &glyphs[i])),
            };
            arrange_flow(&glyphs, ids, rects, None)
        })
        .collect();
    // Turned text takes the place in the reading order where laying it out among the nearest
    // quarter turn's text puts its first glyph.
    let mut rank = Vec::new();
    if !extra.is_empty() {
        rank = vec![usize::MAX; glyphs.len()];
        let mut nearest: [Vec<usize>; 4] = Default::default();
        for (i, g) in glyphs.iter().enumerate() {
            if let Some(q) = nearest.get_mut(g.0) {
                q.push(i);
            }
        }
        for (r, i) in upright(nearest).into_iter().flat_map(|f| f.ids).enumerate() {
            rank[i] = r;
        }
    }
    let first = |ids: &[usize]| ids.iter().filter_map(|&i| rank.get(i).copied()).min().unwrap_or(usize::MAX);
    extra.sort_by_key(|f| first(&f.ids));
    let mut extra = extra.into_iter().peekable();
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(|g| Some(g.2)).collect();
    let mut text = PageText::default();
    let mut emit = |text: &mut PageText, ids: &[usize], line_of: &[u32], space_before: &[bool], quarter: Option<u8>| {
        let offset = text.line_of.last().map_or(0, |line| line.saturating_add(1));
        for ((&i, &line), &space) in ids.iter().zip(line_of).zip(space_before) {
            let Some(mut g) = slots.get_mut(i).and_then(Option::take) else { continue };
            if let Some(quarter) = quarter {
                g.rect = from_upright(to_upright(g.rect, quarter, w, h), quarter, w, h);
            }
            text.glyphs.push(g);
            text.line_of.push(line.saturating_add(offset));
            text.space_before.push(space);
        }
    };
    for flow in flows {
        let mut k = 0;
        while k < flow.ids.len() {
            let line = flow.line_of[k];
            let end = flow.line_of[k..].iter().position(|l| *l != line).map_or(flow.ids.len(), |n| k + n);
            let before = first(&flow.ids[k..end]);
            while let Some(f) = extra.next_if(|f| first(&f.ids) < before) {
                emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
            }
            emit(&mut text, &flow.ids[k..end], &vec![0; end - k], &flow.space_before[k..end], flow.quarter);
            k = end;
        }
    }
    for f in extra {
        emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
    }
    Ok(text)
}
