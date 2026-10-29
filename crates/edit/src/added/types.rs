use super::layout::norm;
use super::{Content, MarkKind, ObjRef, Std14};

pub(super) const TAG: &str = "Added";

/// The most character spacing an added text takes either way, in points: as far as the font size goes.
pub const CHAR_SPACING_LIMIT: f64 = 500.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Helvetica,
    Times,
    Courier,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Helvetica => "Helvetica",
            Family::Times => "Times",
            Family::Courier => "Courier",
        }
    }

    /// The standard-14 font name for this family and style.
    pub fn base_font(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Helvetica, false, false) => "Helvetica",
            (Family::Helvetica, true, false) => "Helvetica-Bold",
            (Family::Helvetica, false, true) => "Helvetica-Oblique",
            (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
            (Family::Times, false, false) => "Times-Roman",
            (Family::Times, true, false) => "Times-Bold",
            (Family::Times, false, true) => "Times-Italic",
            (Family::Times, true, true) => "Times-BoldItalic",
            (Family::Courier, false, false) => "Courier",
            (Family::Courier, true, false) => "Courier-Bold",
            (Family::Courier, false, true) => "Courier-Oblique",
            (Family::Courier, true, true) => "Courier-BoldOblique",
        }
    }

    pub(super) fn from_base(base: &[u8]) -> (Family, bool, bool) {
        let s = String::from_utf8_lossy(base);
        let family = if s.starts_with("Times") {
            Family::Times
        } else if s.starts_with("Courier") {
            Family::Courier
        } else {
            Family::Helvetica
        };
        (family, s.contains("Bold"), s.contains("Italic") || s.contains("Oblique"))
    }

    /// Advance width of `s` at `size` points, from the published standard-14 metrics.
    ///
    /// `italic` matters: the item is written out as the oblique/italic face (see [`Family::base_font`]),
    /// whose published widths differ from the upright face (Times-Italic `A` is 611, Times-Roman 722).
    pub fn width(self, s: &str, size: f64, bold: bool, italic: bool) -> f64 {
        self.std14(bold, italic).text_width(s, size)
    }

    /// The standard-14 face this family means at the given style.
    pub fn std14(self, bold: bool, italic: bool) -> Std14 {
        match (self, bold, italic) {
            (Family::Courier, false, false) => Std14::Courier,
            (Family::Courier, true, false) => Std14::CourierBold,
            (Family::Courier, false, true) => Std14::CourierOblique,
            (Family::Courier, true, true) => Std14::CourierBoldOblique,
            (Family::Times, false, false) => Std14::TimesRoman,
            (Family::Times, true, false) => Std14::TimesBold,
            (Family::Times, false, true) => Std14::TimesItalic,
            (Family::Times, true, true) => Std14::TimesBoldItalic,
            (Family::Helvetica, false, false) => Std14::Helvetica,
            (Family::Helvetica, true, false) => Std14::HelveticaBold,
            (Family::Helvetica, false, true) => Std14::HelveticaOblique,
            (Family::Helvetica, true, true) => Std14::HelveticaBoldOblique,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    /// Lines (but the last) stretched to the box's width.
    Justify,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedText {
    /// The text box in display space; its width wraps the text and its top is the first line's
    /// top. The height follows the text.
    pub rect: [f64; 4],
    pub text: String,
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub size: f64,
    pub color: [f64; 3],
    pub align: Align,
    /// Extra space after every character, in points: the PDF's character spacing (`Tc`).
    pub char_spacing: f64,
}

impl Default for AddedText {
    fn default() -> Self {
        AddedText {
            rect: [0.0; 4],
            text: String::new(),
            family: Family::Helvetica,
            bold: false,
            italic: false,
            size: 12.0,
            color: [0.0; 3],
            align: Align::Left,
            char_spacing: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedImage {
    /// Display space.
    pub rect: [f64; 4],
    /// The image XObject.
    pub image: ObjRef,
    /// Quarter turns counter-clockwise (0–3).
    pub rotation: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// The fraction trimmed from each side of the image: left, bottom, right, top (see
    /// [`AddedImage::crop_valid`]).
    pub crop: [f64; 4],
}

impl AddedImage {
    pub fn new(rect: [f64; 4], image: ObjRef) -> Self {
        AddedImage { rect, image, rotation: 0, flip_h: false, flip_v: false, crop: [0.0; 4] }
    }

    /// The least of the image's width and of its height that a crop keeps.
    pub const MIN_CROP_KEEP: f64 = 0.05;

    /// How far a sum of two sides may pass the most a crop trims: `0.05 + 0.9` is
    /// `0.9500000000000001` in floating point and still keeps 5 %.
    const CROP_ROUNDING: f64 = 1e-9;

    /// Whether `crop` trims every side by a fraction from 0 to under 1 and keeps at least
    /// [`AddedImage::MIN_CROP_KEEP`] across each axis: one side may trim 95 % when its opposite
    /// trims nothing.
    pub fn crop_valid(crop: [f64; 4]) -> bool {
        let [l, b, r, t] = crop;
        let most = 1.0 - Self::MIN_CROP_KEEP;
        crop.iter().all(|v| v.is_finite() && (0.0..1.0).contains(v)) && l + r <= most + Self::CROP_ROUNDING && b + t <= most + Self::CROP_ROUNDING
    }

    /// `crop` brought into [`AddedImage::crop_valid`] range, for one read from a file: each side
    /// in range, and an axis that trims too much keeps its two sides' proportion.
    pub fn clamp_crop(crop: [f64; 4]) -> [f64; 4] {
        let most = 1.0 - Self::MIN_CROP_KEEP;
        let [l, b, r, t] = crop.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        // Scaled from the halves, whose sum can't overflow, so the pair keeps its proportion;
        // a pair within the limit keeps each side within it too.
        let fit = |a: f64, c: f64| {
            let half = a / 2.0 + c / 2.0;
            if half > (most + Self::CROP_ROUNDING) / 2.0 { (a / 2.0 / half * most, c / 2.0 / half * most) } else { (a, c) }
        };
        let ((l, r), (b, t)) = (fit(l, r), fit(b, t));
        [l, b, r, t]
    }

    /// The matrix from image space (the unit square) to display space: crop, flip, rotate,
    /// then fill the box.
    pub(super) fn matrix(&self) -> pdfcraft_content::Matrix {
        use pdfcraft_content::Matrix as M;
        let [l, b, r, t] = Self::clamp_crop(self.crop);
        let (cw, ch) = (1.0 - l - r, 1.0 - b - t);
        let mut m = M([1.0 / cw, 0.0, 0.0, 1.0 / ch, -l / cw, -b / ch]);
        if self.flip_h {
            m = m.then(&M([-1.0, 0.0, 0.0, 1.0, 1.0, 0.0]));
        }
        if self.flip_v {
            m = m.then(&M([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]));
        }
        for _ in 0..self.rotation % 4 {
            // A quarter turn counter-clockwise within the unit square.
            m = m.then(&M([0.0, 1.0, -1.0, 0.0, 1.0, 0.0]));
        }
        let [x0, y0, x1, y1] = norm(self.rect);
        m.then(&M([x1 - x0, 0.0, 0.0, y1 - y0, x0, y0]))
    }
}

/// An added item on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub page: usize,
    /// The item's content stream.
    pub obj: ObjRef,
    pub content: Content,
}

/// PdfCraft-owned page content discovered in one page/content traversal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkerScan {
    pub marks: Vec<MarkKind>,
    pub added: Vec<Added>,
}
