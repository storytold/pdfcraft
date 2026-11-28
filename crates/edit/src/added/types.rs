use super::layout::norm;
use super::{Content, MarkKind, ObjRef, Std14, n};

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
        self.std14(bold, italic).latin_text_width(s, size)
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

/// A device colour added text is painted in instead of RGB, for print files: a text's black in
/// CMYK stays 100 K rather than a DeviceRGB black the printer separates into rich black.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    /// A DeviceGray level, from 0 (black) to 1 (white): painted with `g`.
    Gray(f64),
    /// DeviceCMYK `[c, m, y, k]`, each from 0 to 1 (`[0, 0, 0, 1]` is 100 K): painted with `k`.
    Cmyk([f64; 4]),
}

impl Ink {
    /// Whether every component is a number from 0 to 1.
    pub fn valid(self) -> bool {
        self.components().iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }

    /// The ink as its operator paints it: each component to the thousandth that `g` and `k` are
    /// written with (as RGB's `rg`), so the item, what it reads back as and the page agree.
    pub fn painted(self) -> Ink {
        let round = |v: f64| (v * 1000.0).round() / 1000.0;
        match self {
            Ink::Gray(g) => Ink::Gray(round(g)),
            Ink::Cmyk(c) => Ink::Cmyk(c.map(round)),
        }
    }

    fn components(&self) -> &[f64] {
        match self {
            Ink::Gray(g) => std::slice::from_ref(g),
            Ink::Cmyk(c) => c,
        }
    }

    /// How the ink looks on screen: gray is the same level in each of red, green and blue
    /// (ISO 32000-2 §10.4.2.2), CMYK takes each colour plus black from 1 (§10.4.2.5).
    pub fn rgb(self) -> [f64; 3] {
        match self {
            Ink::Gray(g) => [g.clamp(0.0, 1.0); 3],
            Ink::Cmyk([c, m, y, k]) => [c, m, y].map(|v| 1.0 - (v + k).clamp(0.0, 1.0)),
        }
    }

    /// The operator that makes this ink the fill colour, without a line end.
    pub(super) fn fill_op(self) -> String {
        match self {
            Ink::Gray(g) => format!("{} g", n(g.clamp(0.0, 1.0))),
            Ink::Cmyk(c) => {
                let [c, m, y, k] = c.map(|v| v.clamp(0.0, 1.0));
                format!("{} {} {} {} k", n(c), n(m), n(y), n(k))
            }
        }
    }
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
    /// The RGB colour: what the text is painted in, unless it has an [`AddedText::ink`] this is
    /// the look of.
    pub color: [f64; 3],
    /// A gray or CMYK ink the text is painted in, with `color` its look ([`Ink::rgb`]). Set it
    /// with [`AddedText::set_ink`] and the colour with [`AddedText::set_color`], which drops an
    /// ink the colour no longer shows; one left behind by setting `color` directly paints nothing
    /// ([`AddedText::painted_ink`]) until [`AddedText::drop_stale_ink`] forgets it.
    pub ink: Option<Ink>,
    pub align: Align,
    /// Extra space after every character, in points: the PDF's character spacing (`Tc`).
    pub char_spacing: f64,
    /// Paragraphs (lines typed with Enter) aligned other than [`AddedText::align`], by index;
    /// `None` and paragraphs past the end follow `align` ([`crate::paragraph_align`]).
    pub aligns: Vec<Option<Align>>,
}

impl AddedText {
    /// Paragraph `i`'s alignment.
    pub fn paragraph_align(&self, i: usize) -> Align {
        crate::paragraph_align::align_of(&self.aligns, i, self.align)
    }

    /// Paint the text in `ink` (its look becomes [`AddedText::color`]), or in RGB with `None`. An
    /// ink in range is kept as painted ([`Ink::painted`]); one out of range stays as given, for
    /// saving to refuse.
    pub fn set_ink(&mut self, ink: Option<Ink>) {
        let ink = ink.map(|i| if i.valid() { i.painted() } else { i });
        if let Some(i) = ink {
            self.color = i.rgb();
        }
        self.ink = ink;
    }

    /// Paint the text in RGB `rgb`. Its ink stays only when `rgb` is the ink's own look; once
    /// gone it stays gone, so picking that look again later paints in RGB.
    pub fn set_color(&mut self, rgb: [f64; 3]) {
        self.color = rgb;
        self.drop_stale_ink();
    }

    /// Forget an ink `color` no longer shows, so two styles that paint the same compare equal.
    pub fn drop_stale_ink(&mut self) {
        self.ink = self.painted_ink();
    }

    /// The ink the text is painted in, or `None` for RGB: the one it has while `color` is still
    /// its look.
    pub fn painted_ink(&self) -> Option<Ink> {
        // Within rounding: a look read back from a file, or from a colour picker, is the same.
        self.ink.filter(|i| i.rgb().iter().zip(self.color).all(|(a, b)| (a - b).abs() < 1e-6))
    }

    /// The operator that sets the text's fill colour, without a line end.
    pub(super) fn fill_op(&self) -> String {
        match self.painted_ink() {
            Some(ink) => ink.fill_op(),
            None => crate::marks::rgb(self.color),
        }
    }
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
            ink: None,
            align: Align::Left,
            aligns: Vec::new(),
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
