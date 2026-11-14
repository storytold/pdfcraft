//! PdfCraft patch (107): which accent a glyph's outline draws, for rules that check a reading
//! against the glyph itself.
//!
//! [`classify`] looks at a glyph's contours (1000 units per em, y up, the baseline at 0) and
//! names the mark it draws above its base letter: an acute, a caron, a double acute, a macron, a
//! ring, a dot … or nothing, and whether something reaches well below the baseline (an ogonek, a
//! cedilla, a comma below). [`matches`] says whether that is the mark a character carries, from
//! the character's canonical decomposition (Unicode's combining marks: U+0301 is the acute,
//! U+030C the caron, U+030B the double acute, U+0328 the ogonek …), tabulated here for the
//! letters of the code pages the rules compare.
//!
//! The shapes are told apart by geometry alone, the way a reader tells them apart: a contour
//! above the base letter's top; two of them side by side (a dieresis when both are round and
//! upright, a double acute otherwise); one with a hole (a ring); one wide and flat (a macron, or
//! a breve when its middle sags); one whose top is wide and bottom narrow (a caron) or the
//! reverse (a circumflex); one leaning right (an acute) or left (a grave); a small square one (a
//! dot). It is coarse on purpose: the rules that use it ask it about many glyphs of one font and
//! act only when the answers agree, and a glyph it can't place is no vote either way.

use kurbo::{BezPath, PathEl};

/// The mark a glyph draws above its base letter (or what it draws instead), as [`classify`]
/// reads it and as a character carries it ([`expected`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// No mark above the base letter (`ł`, `ą`, `ð`: their marks, if any, are elsewhere).
    Nothing,
    /// The whole glyph sits above the baseline: a superior figure or letter (`³`, `º`).
    Superior,
    Acute,
    Grave,
    Circumflex,
    Caron,
    Breve,
    Tilde,
    Macron,
    Dieresis,
    DoubleAcute,
    Ring,
    Dot,
    /// A tall, narrow mark right of the letter's middle: the apostrophe of `ď`, `ť`, `ľ`.
    Apostrophe,
    /// A tall, narrow mark that isn't right of the middle: an acute or an apostrophe.
    AcuteOrApostrophe,
    /// Three or more marks, or one this classifier can't name.
    Other,
}

/// What [`classify`] reads from a glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The mark above the base letter.
    pub(crate) mark: Mark,
    /// Whether the outline reaches more than 60 units below the baseline.
    pub(crate) below: bool,
}

/// The most points [`classify`] looks at in one glyph; a larger outline is not classified.
const MAX_POINTS: usize = 4096;

/// One contour's points and bounding box.
struct Contour {
    points: Vec<(f64, f64)>,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Contour {
    fn new(points: Vec<(f64, f64)>) -> Option<Self> {
        let first = *points.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for &(x, y) in &points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Some(Self {
            points,
            x0,
            y0,
            x1,
            y1,
        })
    }

    /// Whether this contour's box lies inside `other`'s (to a unit).
    fn inside(&self, other: &Self) -> bool {
        self.x0 >= other.x0 - 1.0
            && self.y0 >= other.y0 - 1.0
            && self.x1 <= other.x1 + 1.0
            && self.y1 <= other.y1 + 1.0
    }
}

/// The contours of `path`, every point of each (on the curve and off it); `None` when the path
/// has more than [`MAX_POINTS`] points or a point that isn't finite.
fn contours(path: &BezPath) -> Option<Vec<Contour>> {
    let mut out = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut total = 0_usize;
    for element in path.elements() {
        let points: &[kurbo::Point] = match element {
            PathEl::MoveTo(p) => {
                if let Some(c) = Contour::new(std::mem::take(&mut current)) {
                    out.push(c);
                }
                std::slice::from_ref(p)
            }
            PathEl::LineTo(p) => std::slice::from_ref(p),
            PathEl::QuadTo(a, b) => &[*a, *b][..],
            PathEl::CurveTo(a, b, c) => &[*a, *b, *c][..],
            PathEl::ClosePath => {
                if let Some(c) = Contour::new(std::mem::take(&mut current)) {
                    out.push(c);
                }
                &[]
            }
        };
        total = total.saturating_add(points.len());
        if total > MAX_POINTS {
            return None;
        }
        for p in points {
            if !(p.x.is_finite() && p.y.is_finite()) {
                return None;
            }
            current.push((p.x, p.y));
        }
    }
    if let Some(c) = Contour::new(current) {
        out.push(c);
    }
    Some(out)
}

/// The mean of `values`, or nothing for none.
fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (mut sum, mut n) = (0.0_f64, 0_u32);
    for v in values {
        sum += v;
        n = n.saturating_add(1);
    }
    (n > 0).then(|| sum / f64::from(n))
}

/// The spread of `values` (largest minus smallest), or nothing for none.
fn spread(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut range: Option<(f64, f64)> = None;
    for v in values {
        range = Some(range.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
    }
    range.map(|(lo, hi)| hi - lo)
}

/// The accent a glyph's outline draws (see the module documentation), or nothing for an empty
/// outline, one too large to look at, or one with a point that isn't finite.
pub(crate) fn classify(path: &BezPath) -> Option<Shape> {
    let all = contours(path)?;
    let lowest = all.iter().map(|c| c.y0).reduce(f64::min)?;
    let below = lowest < -60.0;
    let shape = |mark| Some(Shape { mark, below });
    if lowest > 200.0 {
        return shape(Mark::Superior);
    }
    // The base letter: the contours that reach down to the baseline region.
    let base: Vec<&Contour> = all.iter().filter(|c| c.y0 < 150.0).collect();
    let Some(base_top) = base.iter().map(|c| c.y1).reduce(f64::max) else {
        return shape(Mark::Superior);
    };
    let base_left = base.iter().map(|c| c.x0).reduce(f64::min).unwrap_or(0.0);
    let base_right = base.iter().map(|c| c.x1).reduce(f64::max).unwrap_or(0.0);
    // Marks start above the base letter's top; a contour inside another is a hole.
    let is_hole = |i: usize, c: &Contour| {
        all.iter()
            .enumerate()
            .any(|(j, other)| i != j && c.inside(other))
    };
    let (mut marks, mut holes) = (Vec::new(), 0_usize);
    for (i, c) in all.iter().enumerate() {
        if c.y0 > base_top - 10.0 && c.y0 >= 150.0 {
            if is_hole(i, c) {
                holes = holes.saturating_add(1);
            } else {
                marks.push(c);
            }
        }
    }
    match marks.as_slice() {
        [] => shape(Mark::Nothing),
        [a, b] => {
            let (a, b) = if b.x0 < a.x0 { (b, a) } else { (a, b) };
            if roundish(a) && roundish(b) && !(slant(a) > 0.3 && slant(b) > 0.3) {
                shape(Mark::Dieresis)
            } else {
                shape(Mark::DoubleAcute)
            }
        }
        [mark] => shape(single_mark(mark, holes > 0, base_left, base_right)),
        _ => shape(Mark::Other),
    }
}

/// Whether a mark's box is about as tall as it is wide (a dot of a dieresis).
fn roundish(c: &Contour) -> bool {
    let w = (c.x1 - c.x0).max(1.0);
    let h = (c.y1 - c.y0).max(1.0);
    (0.75..=1.3).contains(&(h / w))
}

/// How far right a mark's highest point lies of its lowest, as a share of its width: an acute
/// leans right.
fn slant(c: &Contour) -> f64 {
    let w = (c.x1 - c.x0).max(1.0);
    let mut lo = (0.0, f64::INFINITY);
    let mut hi = (0.0, f64::NEG_INFINITY);
    for &(x, y) in &c.points {
        if y < lo.1 {
            lo = (x, y);
        }
        if y > hi.1 {
            hi = (x, y);
        }
    }
    (hi.0 - c.x0) / w - (lo.0 - c.x0) / w
}

/// Whether a mark's ends are higher than its middle, which comes down to its bottom (`dip`
/// times its height higher, at least): a breve.
fn sags(c: &Contour, w: f64, h: f64, dip: f64) -> bool {
    let left = mean(
        c.points
            .iter()
            .filter(|p| p.0 - c.x0 < 0.15 * w)
            .map(|p| p.1),
    );
    let right = mean(
        c.points
            .iter()
            .filter(|p| p.0 - c.x0 > 0.85 * w)
            .map(|p| p.1),
    );
    let middle = c
        .points
        .iter()
        .filter(|p| p.0 - c.x0 > 0.35 * w && p.0 - c.x0 < 0.65 * w)
        .map(|p| p.1);
    let middle_mean = mean(middle.clone());
    let middle_low = middle.reduce(f64::min);
    match (left, right, middle_mean, middle_low) {
        (Some(l), Some(r), Some(m), Some(low)) => l.min(r) > m + dip * h && low <= c.y0 + 0.1 * h,
        _ => false,
    }
}

/// The one mark above a base letter, by its shape.
fn single_mark(c: &Contour, has_hole: bool, base_left: f64, base_right: f64) -> Mark {
    let w = (c.x1 - c.x0).max(1.0);
    let h = (c.y1 - c.y0).max(1.0);
    if has_hole {
        return Mark::Ring;
    }
    let low = || c.points.iter().filter(|p| p.1 <= c.y0 + 0.2 * h);
    let high = || c.points.iter().filter(|p| p.1 >= c.y1 - 0.2 * h);
    // Both are non-empty: they hold the lowest and the highest point.
    let lx = mean(low().map(|p| p.0)).map_or(0.5, |x| (x - c.x0) / w);
    let hx = mean(high().map(|p| p.0)).map_or(0.5, |x| (x - c.x0) / w);
    let lsp = spread(low().map(|p| p.0)).unwrap_or(0.0) / w;
    let hsp = spread(high().map(|p| p.0)).unwrap_or(0.0) / w;
    if w < 0.6 * h && h > 120.0 {
        return if c.x0 > (base_left + base_right) / 2.0 {
            Mark::Apostrophe
        } else {
            Mark::AcuteOrApostrophe
        };
    }
    if h < 0.35 * w && w > 1.8 * h {
        if sags(c, w, h, 0.25) {
            return Mark::Breve;
        }
        let wavy = spread(
            c.points
                .iter()
                .filter(|p| p.0 - c.x0 > 0.25 * w && p.0 - c.x0 < 0.75 * w)
                .map(|p| p.1),
        )
        .is_some_and(|s| s > 0.6 * h && h > 0.15 * w);
        return if wavy { Mark::Tilde } else { Mark::Macron };
    }
    if hsp > 0.55 && lsp < 0.5 && (0.3..=0.7).contains(&lx) {
        return Mark::Caron;
    }
    if lsp > 0.55 && hsp < 0.5 && (0.3..=0.7).contains(&hx) {
        return Mark::Circumflex;
    }
    if hx < 0.45 && lx > 0.55 {
        return Mark::Grave;
    }
    if hx > 0.55 && lx < 0.45 {
        return Mark::Acute;
    }
    if (w - h).abs() < 0.35 * w.max(h) && w < 200.0 {
        return Mark::Dot;
    }
    if sags(c, w, h, 0.15) {
        return Mark::Breve;
    }
    if w > 1.6 * h {
        return Mark::Tilde;
    }
    Mark::Other
}

/// The mark a character carries above its base letter, and whether it carries one below the
/// baseline (`None`: either way, for letters with a descender of their own), for the letters and
/// signs of the code pages the rules compare; nothing for any other character, which makes no
/// vote. From each character's canonical decomposition: an ogonek, a cedilla and a comma below
/// are below, and leave nothing above. Fractions (`½`) make no vote: their denominator reaches
/// the baseline, and the spacing accents some code pages have at their codes (`˝`) float.
pub(crate) fn expected(c: char) -> Option<(Mark, Option<bool>)> {
    Some(match c {
        // The comma above of ģ looks like an acute.
        'ģ' => (Mark::Acute, None),
        'Á' | 'É' | 'Í' | 'Ó' | 'Ú' | 'Ý' | 'á' | 'é' | 'í' | 'ó' | 'ú' | 'ý' | 'Ć' | 'ć' | 'Ĺ'
        | 'ĺ' | 'Ń' | 'ń' | 'Ŕ' | 'ŕ' | 'Ś' | 'ś' | 'Ź' | 'ź' => {
            (Mark::Acute, Some(false))
        }
        'Ă' | 'ă' | 'Ğ' | 'ğ' => (Mark::Breve, Some(false)),
        'Č' | 'č' | 'Ď' | 'Ě' | 'ě' | 'Ň' | 'ň' | 'Ř' | 'ř' | 'Š' | 'š' | 'Ť' | 'Ž' | 'ž' => {
            (Mark::Caron, Some(false))
        }
        // Drawn with a caron or, beside the letter's stem, an apostrophe.
        'ď' | 'ľ' | 'Ľ' | 'ť' => (Mark::Apostrophe, Some(false)),
        'Â' | 'Ê' | 'Î' | 'Ô' | 'Û' | 'â' | 'ê' | 'î' | 'ô' | 'û' => {
            (Mark::Circumflex, Some(false))
        }
        'Ä' | 'Ë' | 'Ï' | 'Ö' | 'Ü' | 'ä' | 'ë' | 'ï' | 'ö' | 'ü' | 'ÿ' | 'Ÿ' => {
            (Mark::Dieresis, Some(false))
        }
        'Ė' | 'ė' | 'İ' | 'Ż' | 'ż' => (Mark::Dot, Some(false)),
        'Ő' | 'ő' | 'Ű' | 'ű' => (Mark::DoubleAcute, Some(false)),
        'À' | 'È' | 'Ì' | 'Ò' | 'Ù' | 'à' | 'è' | 'ì' | 'ò' | 'ù' => {
            (Mark::Grave, Some(false))
        }
        'Ā' | 'ā' | 'Ē' | 'ē' | 'Ī' | 'ī' | 'Ō' | 'ō' | 'Ū' | 'ū' => {
            (Mark::Macron, Some(false))
        }
        '£' | '¥' | 'µ' | '¿' | 'Þ' | 'þ' => (Mark::Nothing, None),
        'Æ' | 'Ð' | 'ß' | 'æ' | 'ð' | 'Đ' | 'đ' | 'ı' | 'Ł' | 'ł' | 'Œ' | 'œ' | 'ƒ' => {
            (Mark::Nothing, Some(false))
        }
        // The slash of `Ø` often reaches below the baseline.
        'Ø' | 'ø' => (Mark::Nothing, None),
        'Ç' | 'ç' | 'Ą' | 'ą' | 'Ę' | 'ę' | 'Į' | 'į' | 'Ķ' | 'ķ' | 'Ļ' | 'ļ' | 'Ņ' | 'ņ' | 'Ŗ'
        | 'ŗ' | 'Ş' | 'ş' | 'Ţ' | 'ţ' | 'Ų' | 'ų' | 'Ģ' => (Mark::Nothing, Some(true)),
        'Å' | 'å' | 'Ů' | 'ů' => (Mark::Ring, Some(false)),
        'ª' | '²' | '³' | '¹' | 'º' => (Mark::Superior, Some(false)),
        'Ã' | 'Ñ' | 'Õ' | 'ã' | 'ñ' | 'õ' => (Mark::Tilde, Some(false)),
        _ => return None,
    })
}

/// Whether a glyph of `shape` draws `c` as far as its marks tell: nothing when the character
/// carries no mark this module tabulates ([`expected`]). A caron and a breve count as each other
/// (small carons are often round), a tall narrow mark as an acute or an apostrophe, and the
/// apostrophe of `ď`, `ť`, `ľ` and `Ľ` may be a caron.
pub(crate) fn matches(shape: Shape, c: char) -> Option<bool> {
    let (mark, below) = expected(c)?;
    let caron_like = |m: Mark| matches!(m, Mark::Caron | Mark::Breve);
    let above = mark == shape.mark
        || (caron_like(mark) && caron_like(shape.mark))
        || (mark == Mark::Acute && shape.mark == Mark::AcuteOrApostrophe)
        || (mark == Mark::Apostrophe
            && (shape.mark == Mark::AcuteOrApostrophe || caron_like(shape.mark)));
    Some(above && below.is_none_or(|b| b == shape.below))
}
