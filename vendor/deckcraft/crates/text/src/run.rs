use super::{Arc, FontFace, Point, Rect, Rgba};

/// A run of glyphs in one face, size and colour.
#[derive(Clone)]
pub struct GlyphRun {
    pub face: Arc<FontFace>,
    /// Font size in points (after autofit scaling).
    pub size: f64,
    pub color: Rgba,
    /// Glyph id and pen position of its origin (baseline), in shape-local points.
    pub glyphs: Vec<(u32, f64, f64)>,
    /// Per glyph: the paragraph character it draws (index, char); empty for bullets.
    pub cells: Vec<(usize, char)>,
    /// Draw a heavier stroke because the family has no bold face.
    pub fake_bold: bool,
    /// Slant because the family has no italic face.
    pub fake_italic: bool,
    pub outline: Option<(Rgba, f64)>,
    /// Paragraph and character range this run covers (for hyperlinks, selection colours).
    pub para: usize,
    pub chars: (usize, usize),
    pub link: bool,
    /// Opacity of the fill on top of `color`'s own alpha (0 for no fill).
    pub alpha: f64,
    pub gradient: Option<std::sync::Arc<deckcraft_model::style::Gradient>>,
    /// Shadow and glow behind the glyphs (`a:rPr/a:effectLst`).
    pub effects: Option<RunEffects>,
    /// East Asian vertical text (`eaVert`): each glyph stands upright, turned a quarter turn
    /// back about the centre of its em box ([`GlyphRun::upright_centre`]) inside text that is
    /// laid out turned a quarter turn.
    pub upright: bool,
}

/// Share of the size the ideographic em box's centre sits above the baseline (CJK fonts put the
/// em box from 0.88 em above to 0.12 em below it).
pub const UPRIGHT_CENTRE: f64 = 0.38;

impl GlyphRun {
    /// The point an upright glyph at pen position (`x`, `y`) turns about: the centre of its em box,
    /// `advance` wide (`None` when the run is not upright).
    pub fn upright_centre(&self, gid: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        if !self.upright {
            return None;
        }
        let adv = self.face.advance(gid) * self.size / self.face.upem.max(1.0);
        Some((x + adv / 2.0, y - self.size * UPRIGHT_CENTRE))
    }
}

/// A text run's shadow and glow, colours resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunEffects {
    pub shadow: Option<TextShadow>,
    /// Glow colour (with its alpha) and radius in points.
    pub glow: Option<(Rgba, f64)>,
}

/// An outer shadow: colour (with its alpha), blur radius and offset in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextShadow {
    pub color: Rgba,
    pub blur: f64,
    pub dx: f64,
    pub dy: f64,
}

impl std::fmt::Debug for GlyphRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphRun").field("family", &self.face.family).field("size", &self.size).field("glyphs", &self.glyphs.len()).finish()
    }
}

/// An underline, strikethrough or highlight box.
#[derive(Clone, Debug)]
pub struct Deco {
    pub rect: Rect,
    pub color: Rgba,
    /// Highlights draw behind text.
    pub behind: bool,
    /// Paragraph the decoration belongs to (by-paragraph animation).
    pub para: usize,
    /// How the line fills `rect`.
    pub line: DecoLine,
}

/// How an underline fills its box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DecoLine {
    /// The whole box.
    #[default]
    Solid,
    /// Dashes or dots across the box's height, from its left end.
    Dashed(Dash),
    /// A zigzag between the box's top and bottom: `stroke` wide, one wave every `period`.
    Wave { stroke: f64, period: f64 },
}

/// Dash patterns of the dotted and dashed underlines, in units of the line's thickness. Measured
/// on PowerPoint's export: dots and their gaps as long as the line is thick, dashes three
/// thicknesses long with gaps of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dash {
    Dot,
    Dash,
    Long,
    DotDash,
    DotDotDash,
}

impl Dash {
    /// On, off, on, off… lengths in thicknesses.
    fn pattern(self) -> &'static [f64] {
        match self {
            Dash::Dot => &[1.0, 1.0],
            Dash::Dash => &[3.0, 2.0],
            Dash::Long => &[6.0, 2.0],
            Dash::DotDash => &[3.0, 2.0, 1.0, 2.0],
            Dash::DotDotDash => &[3.0, 2.0, 1.0, 2.0, 1.0, 2.0],
        }
    }
}

impl Deco {
    /// The filled boxes of a solid or dashed line (dashes start at the box's left end). A wavy
    /// line gives a thin box along its middle, for drawing that can't stroke a path.
    pub fn pieces(&self) -> Vec<Rect> {
        let r = self.rect;
        match self.line {
            DecoLine::Solid => vec![r],
            DecoLine::Wave { stroke, .. } => {
                let c = (r.y0 + r.y1) / 2.0;
                vec![Rect::new(r.x0, c - stroke / 2.0, r.x1, c + stroke / 2.0)]
            }
            DecoLine::Dashed(d) => {
                let unit = r.height().max(0.1);
                let pat = d.pattern();
                let mut out = Vec::new();
                let mut x = r.x0;
                let mut k = 0usize;
                while x < r.x1 && out.len() < 10_000 {
                    let len = pat.get(k % pat.len()).copied().unwrap_or(1.0) * unit;
                    if k.is_multiple_of(2) {
                        out.push(Rect::new(x, r.y0, (x + len).min(r.x1), r.y1));
                    }
                    x += len;
                    k += 1;
                }
                out
            }
        }
    }

    /// The zigzag of a wavy underline, left to right from the box's top, and its stroke width;
    /// None for other lines.
    pub fn wave(&self) -> Option<(Vec<Point>, f64)> {
        let DecoLine::Wave { stroke, period } = self.line else { return None };
        let r = self.rect;
        let half = (period / 2.0).max(0.1);
        let n = ((r.width().max(0.0) / half).ceil() as usize).min(20_000);
        let pts = (0..=n)
            .map(|i| {
                let x = (r.x0 + i as f64 * half).min(r.x1);
                Point::new(x, if i % 2 == 0 { r.y0 } else { r.y1 })
            })
            .collect();
        Some((pts, stroke))
    }
}

/// An underline style (`a:rPr/@u`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Underline {
    Single,
    Double,
    Heavy,
    /// Dotted or dashed, heavy or not.
    Dashed(Dash, bool),
    /// Wavy, heavy or not.
    Wavy(bool),
}

impl Underline {
    pub(crate) fn from_xml(u: &str) -> Option<Self> {
        Some(match u {
            "none" => return None,
            // PowerPoint underlines the spaces of a "words" underline too.
            "words" => Underline::Single,
            // At text sizes PowerPoint's double wave reads as two straight lines.
            "dbl" | "wavyDbl" => Underline::Double,
            "heavy" => Underline::Heavy,
            "dotted" => Underline::Dashed(Dash::Dot, false),
            "dottedHeavy" => Underline::Dashed(Dash::Dot, true),
            "dash" => Underline::Dashed(Dash::Dash, false),
            "dashHeavy" => Underline::Dashed(Dash::Dash, true),
            "dashLong" => Underline::Dashed(Dash::Long, false),
            "dashLongHeavy" => Underline::Dashed(Dash::Long, true),
            "dotDash" => Underline::Dashed(Dash::DotDash, false),
            "dotDashHeavy" => Underline::Dashed(Dash::DotDash, true),
            "dotDotDash" => Underline::Dashed(Dash::DotDotDash, false),
            "dotDotDashHeavy" => Underline::Dashed(Dash::DotDotDash, true),
            "wavy" => Underline::Wavy(false),
            "wavyHeavy" => Underline::Wavy(true),
            _ => Underline::Single,
        })
    }

    /// The boxes of this underline below the baseline (top, bottom, fill), from the font's
    /// underline position `p` and thickness `t` at the run's size. Measured on PowerPoint's export
    /// (Arial 96 pt: p = 10.2, t = 7.0): single at p..p+t; heavy from p-t to p+0.55t; double as two
    /// lines 0.55t thick spanning p-t..p+t; wavy a thin zigzag from p-0.6t to p+1.1t, a wave every
    /// 3.9t.
    pub(crate) fn boxes(self, p: f64, t: f64) -> Vec<(f64, f64, DecoLine)> {
        let heavy = (p - t, p + 0.55 * t);
        match self {
            Underline::Single => vec![(p, p + t, DecoLine::Solid)],
            Underline::Heavy => vec![(heavy.0, heavy.1, DecoLine::Solid)],
            Underline::Double => vec![(p - t, p - 0.45 * t, DecoLine::Solid), (p + 0.45 * t, p + t, DecoLine::Solid)],
            Underline::Dashed(d, false) => vec![(p, p + t, DecoLine::Dashed(d))],
            Underline::Dashed(d, true) => vec![(heavy.0, heavy.1, DecoLine::Dashed(d))],
            Underline::Wavy(h) => {
                let stroke = (t * 0.15).max(0.4) * if h { 2.0 } else { 1.0 };
                vec![(p - 0.6 * t, p + 1.1 * t, DecoLine::Wave { stroke, period: t * 3.9 })]
            }
        }
    }
}
