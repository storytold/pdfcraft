//! PdfCraft patch (111): glyph outlines fitted to the pixel grid at the size they are drawn
//! (storytold/pdfcraft#730).
//!
//! An outline is otherwise read once at 1000 units per em and filled at whatever size the page
//! shows it, so baselines, x-heights and stems land between device pixels: small text comes out
//! soft, its thin strokes grey and broken, and each line a little different from the next. Here
//! an outline is read hinted for the size it is drawn, as text engines draw text on screen: a CFF
//! program by its own PostScript hints, a TrueType or Type 1 program by skrifa's automatic hinter
//! (a TrueType font that needs its instructions to draw correct contours still runs them). The
//! target is the light one, which moves points only vertically, so where a glyph sits along the
//! line and how far it advances stay as the PDF says.
//!
//! The hinted outline is scaled back to 1000 units per em, so it is drawn with the transform
//! the unhinted one would have been drawn with; the renderer decides when that transform allows
//! it. The program is read for hinting through skrifa's font model (`skrifa::Font`), which hints
//! standalone CFF and Type 1 programs, while the rest of the crate reads it through the older
//! API skrifa keeps; [`plausible`] keeps the unhinted outline when the two disagree.

use crate::font::UNITS_PER_EM;
use kurbo::{BezPath, Shape};
use skrifa::{DrawMemory, Font, GlyphId, HintOptions, PathSink, RasterTarget, ScalerFont};
use std::sync::{Mutex, OnceLock, PoisonError};

type FontData = std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>;

/// The smallest size, in device pixels per em, that is hinted. Below it a glyph is a few pixels
/// tall and fitting it to the grid would close its counters.
pub const MIN_HINTED_PPEM: f32 = 4.0;

/// The largest size, in device pixels per em, that is hinted. Above it the softness of an
/// unhinted edge is a small part of a stem, and every zoom level would set up hinting for its
/// own sizes for nothing.
pub const MAX_HINTED_PPEM: f32 = 64.0;

/// Sizes hinted per font that are kept set up. A page uses a few sizes of each font (body text,
/// headings, notes, sub- and superscripts, a table's), drawn in whatever order its content has;
/// setting up another is cheap for the automatic and PostScript hinters, but a TrueType font that
/// needs its instructions runs its programs again, so a page's sizes should all fit. Each kept
/// scaler is a few kilobytes.
const KEPT_SIZES: usize = 12;

/// Which of skrifa's hinters a font program is fitted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    /// The font's own PostScript hints: CFF programs, which carry them for this.
    PostScript,
    /// The automatic hinter: TrueType programs, whose instructions in a subset are often broken
    /// and are run only when the font needs them, and Type 1 programs, which skrifa hints only
    /// this way.
    Auto,
}

/// Hinted outlines of one font program.
pub(crate) struct Hinter {
    data: FontData,
    index: u32,
    engine: Engine,
    /// The program read by skrifa 0.50, on first use; `None` when it can't read it.
    font: OnceLock<Option<Font>>,
    /// Scalers set up for the sizes drawn last, most recent first, by the size's bits.
    scalers: Mutex<Vec<(u32, ScalerFont)>>,
}

impl std::fmt::Debug for Hinter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hinter")
            .field("index", &self.index)
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

impl Hinter {
    pub(crate) fn new(data: FontData, index: u32, engine: Engine) -> Self {
        Self {
            data,
            index,
            engine,
            font: OnceLock::new(),
            scalers: Mutex::new(Vec::new()),
        }
    }

    /// The outline of `glyph` fitted to the grid at `ppem` device pixels per em, in units of
    /// 1/1000 em like the unhinted outline. `None` when the size isn't hinted, or the program
    /// or the glyph can't be read or hinted; the unhinted outline is drawn then.
    pub(crate) fn outline(&self, glyph: u32, ppem: f32) -> Option<BezPath> {
        if !(MIN_HINTED_PPEM..=MAX_HINTED_PPEM).contains(&ppem) {
            return None;
        }
        let font = self
            .font
            .get_or_init(|| Font::new(self.data.clone(), self.index))
            .as_ref()?;
        let mut scalers = self.scalers.lock().unwrap_or_else(PoisonError::into_inner);
        let key = ppem.to_bits();
        match scalers.iter().position(|(size, _)| *size == key) {
            Some(0) => {}
            Some(at) => {
                let kept = scalers.remove(at);
                scalers.insert(0, kept);
            }
            None => {
                let options = match self.engine {
                    Engine::PostScript => HintOptions::interpreter(),
                    Engine::Auto => HintOptions::auto(),
                }
                .with_target(RasterTarget::Light);
                scalers.insert(0, (key, ScalerFont::hinted(font.clone(), ppem, options)));
                scalers.truncate(KEPT_SIZES);
            }
        }
        let (_, scaler) = scalers.first()?;
        let outline = scaler.outline(GlyphId::new(glyph))?;
        let mut sink = Sink {
            path: BezPath::new(),
            scale: f64::from(UNITS_PER_EM / ppem),
        };
        outline.draw(DrawMemory::Scratch, &mut sink).ok()?;
        Some(sink.path)
    }
}

/// Collects an outline drawn in device pixels as a path in units of 1/1000 em.
struct Sink {
    path: BezPath,
    scale: f64,
}

impl Sink {
    fn point(&self, x: f32, y: f32) -> (f64, f64) {
        (f64::from(x) * self.scale, f64::from(y) * self.scale)
    }

    /// Whether a subpath is open; segments before the first move are dropped, as the unhinted
    /// outline's pen drops them.
    fn started(&self) -> bool {
        !self.path.elements().is_empty()
    }
}

impl PathSink for Sink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.point(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        if self.started() {
            self.path.line_to(self.point(x, y));
        }
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        if self.started() {
            self.path.quad_to(self.point(cx0, cy0), self.point(x, y));
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        if self.started() {
            self.path
                .curve_to(self.point(cx0, cy0), self.point(cx1, cy1), self.point(x, y));
        }
    }

    fn close(&mut self) {
        if self.started() {
            self.path.close_path();
        }
    }
}

/// Whether `hinted` can stand in for `unhinted` at `ppem`: both have ink or neither has, and
/// each edge of the hinted outline's bounds lies within two device pixels of the unhinted one's.
///
/// The hinters move an edge by a pixel at most. A larger difference means the font model read
/// the program differently from the reader that draws everything else (another glyph, a
/// different matrix) or the hints went wrong, and then the unhinted outline is drawn: a glyph
/// slightly soft is better than a wrong one.
pub(crate) fn plausible(hinted: &BezPath, unhinted: &BezPath, ppem: f32) -> bool {
    let inked = |path: &BezPath| path.elements().len() > 1;
    match (inked(hinted), inked(unhinted)) {
        (false, false) => return true,
        (true, true) => {}
        _ => return false,
    }
    let (h, u) = (hinted.bounding_box(), unhinted.bounding_box());
    let reach = 2.0 * f64::from(UNITS_PER_EM / ppem);
    [h.x0 - u.x0, h.y0 - u.y0, h.x1 - u.x1, h.y1 - u.y1]
        .iter()
        .all(|d| d.is_finite() && d.abs() <= reach)
}
