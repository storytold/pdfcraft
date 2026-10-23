use crate::FontResolverFn;
use crate::font::blob::{CffFontBlob, OpenTypeFontBlob};
use crate::font::generated::{glyph_names, metrics, standard, symbol, zapf_dings};
use crate::font::true_type::{Width, read_encoding, read_widths};
use crate::font::{
    Encoding, FontData, FontQuery, glyph_name_to_unicode, normalized_glyph_name, stretch_glyph,
    strip_subset_prefix,
};
use hayro_syntax::object::Dict;
use hayro_syntax::object::Name;
use hayro_syntax::object::dict::keys::{BASE_FONT, FONT_DESC, FONT_WEIGHT, ITALIC_ANGLE};
use kurbo::{Affine, BezPath, Shape};
use skrifa::GlyphId;
use skrifa::raw::TableProvider;
use std::cell::RefCell;
use std::collections::HashMap;

/// The 14 standard fonts of PDF.
#[derive(Copy, Clone, Debug)]
pub enum StandardFont {
    /// Helvetica.
    Helvetica,
    /// Helvetica Bold.
    HelveticaBold,
    /// Helvetica Oblique.
    HelveticaOblique,
    /// Helvetica Bold Oblique.
    HelveticaBoldOblique,
    /// Courier.
    Courier,
    /// Courier Bold.
    CourierBold,
    /// Courier Oblique.
    CourierOblique,
    /// Courier Bold Oblique.
    CourierBoldOblique,
    /// Times Roman.
    TimesRoman,
    /// Times Bold.
    TimesBold,
    /// Times Italic.
    TimesItalic,
    /// Times Bold Italic.
    TimesBoldItalic,
    /// Zapf Dingbats - a decorative symbol font.
    ZapfDingBats,
    /// Symbol - a mathematical symbol font.
    Symbol,
}

impl StandardFont {
    pub(crate) fn code_to_name(&self, code: u8) -> Option<&'static str> {
        match self {
            Self::Symbol => symbol::get(code),
            // Note that this font does not return postscript character names,
            // but instead has a custom encoding.
            Self::ZapfDingBats => zapf_dings::get(code),
            _ => standard::get(code),
        }
    }

    pub(crate) fn get_width(&self, mut name: &str) -> Option<f32> {
        // <https://github.com/apache/pdfbox/blob/129aafe26548c1ff935af9c55cb40a996186c35f/pdfbox/src/main/java/org/apache/pdfbox/pdmodel/font/PDSimpleFont.java#L340>
        if name == ".notdef" {
            return Some(250.0);
        }

        name = normalized_glyph_name(name);

        match self {
            Self::Helvetica => metrics::HELVETICA.get(name).copied(),
            Self::HelveticaBold => metrics::HELVETICA_BOLD.get(name).copied(),
            Self::HelveticaOblique => metrics::HELVETICA_OBLIQUE.get(name).copied(),
            Self::HelveticaBoldOblique => metrics::HELVETICA_BOLD_OBLIQUE.get(name).copied(),
            Self::Courier => metrics::COURIER.get(name).copied(),
            Self::CourierBold => metrics::COURIER_BOLD.get(name).copied(),
            Self::CourierOblique => metrics::COURIER_OBLIQUE.get(name).copied(),
            Self::CourierBoldOblique => metrics::COURIER_BOLD_OBLIQUE.get(name).copied(),
            Self::TimesRoman => metrics::TIMES_ROMAN.get(name).copied(),
            Self::TimesBold => metrics::TIMES_BOLD.get(name).copied(),
            Self::TimesItalic => metrics::TIMES_ITALIC.get(name).copied(),
            Self::TimesBoldItalic => metrics::TIMES_BOLD_ITALIC.get(name).copied(),
            Self::ZapfDingBats => metrics::ZAPF_DING_BATS.get(name).copied(),
            Self::Symbol => metrics::SYMBOL.get(name).copied(),
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Helvetica => "Helvetica",
            Self::HelveticaBold => "Helvetica Bold",
            Self::HelveticaOblique => "Helvetica Oblique",
            Self::HelveticaBoldOblique => "Helvetica Bold Oblique",
            Self::Courier => "Courier",
            Self::CourierBold => "Courier Bold",
            Self::CourierOblique => "Courier Oblique",
            Self::CourierBoldOblique => "Courier Bold Oblique",
            Self::TimesRoman => "Times Roman",
            Self::TimesBold => "Times Bold",
            Self::TimesItalic => "Times Italic",
            Self::TimesBoldItalic => "Times Bold Italic",
            Self::ZapfDingBats => "Zapf Dingbats",
            Self::Symbol => "Symbol",
        }
    }

    /// Return the postscrit name of the font.
    pub fn postscript_name(&self) -> &'static str {
        match self {
            Self::Helvetica => "Helvetica",
            Self::HelveticaBold => "Helvetica-Bold",
            Self::HelveticaOblique => "Helvetica-Oblique",
            Self::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Self::Courier => "Courier",
            Self::CourierBold => "Courier-Bold",
            Self::CourierOblique => "Courier-Oblique",
            Self::CourierBoldOblique => "Courier-BoldOblique",
            Self::TimesRoman => "Times-Roman",
            Self::TimesBold => "Times-Bold",
            Self::TimesItalic => "Times-Italic",
            Self::TimesBoldItalic => "Times-BoldItalic",
            Self::ZapfDingBats => "ZapfDingbats",
            Self::Symbol => "Symbol",
        }
    }

    pub(crate) fn is_bold(&self) -> bool {
        matches!(
            self,
            Self::HelveticaBold
                | Self::HelveticaBoldOblique
                | Self::CourierBold
                | Self::CourierBoldOblique
                | Self::TimesBold
                | Self::TimesBoldItalic
        )
    }

    pub(crate) fn is_italic(&self) -> bool {
        matches!(
            self,
            Self::HelveticaOblique
                | Self::HelveticaBoldOblique
                | Self::CourierOblique
                | Self::CourierBoldOblique
                | Self::TimesItalic
                | Self::TimesBoldItalic
        )
    }

    pub(crate) fn is_serif(&self) -> bool {
        matches!(
            self,
            Self::TimesRoman | Self::TimesBold | Self::TimesItalic | Self::TimesBoldItalic
        )
    }

    pub(crate) fn is_monospace(&self) -> bool {
        matches!(
            self,
            Self::Courier | Self::CourierBold | Self::CourierOblique | Self::CourierBoldOblique
        )
    }

    /// Return suitable font data for the given standard font.
    ///
    /// Currently, this will return the corresponding Foxit font, which is a set of permissibly
    /// licensed fonts that is also very light-weight.
    ///
    /// You can use the result of this method in your implementation of [`FontResolverFn`].
    ///
    /// [`FontResolverFn`]: crate::FontResolverFn
    #[cfg(feature = "embed-fonts")]
    pub fn get_font_data(&self) -> (FontData, u32) {
        use std::sync::Arc;

        let data = match self {
            Self::Helvetica => &include_bytes!("../../assets/FoxitSans.pfb")[..],
            Self::HelveticaBold => &include_bytes!("../../assets/FoxitSansBold.pfb")[..],
            Self::HelveticaOblique => &include_bytes!("../../assets/FoxitSansItalic.pfb")[..],
            Self::HelveticaBoldOblique => {
                &include_bytes!("../../assets/FoxitSansBoldItalic.pfb")[..]
            }
            Self::Courier => &include_bytes!("../../assets/FoxitFixed.pfb")[..],
            Self::CourierBold => &include_bytes!("../../assets/FoxitFixedBold.pfb")[..],
            Self::CourierOblique => &include_bytes!("../../assets/FoxitFixedItalic.pfb")[..],
            Self::CourierBoldOblique => {
                &include_bytes!("../../assets/FoxitFixedBoldItalic.pfb")[..]
            }
            Self::TimesRoman => &include_bytes!("../../assets/FoxitSerif.pfb")[..],
            Self::TimesBold => &include_bytes!("../../assets/FoxitSerifBold.pfb")[..],
            Self::TimesItalic => &include_bytes!("../../assets/FoxitSerifItalic.pfb")[..],
            Self::TimesBoldItalic => &include_bytes!("../../assets/FoxitSerifBoldItalic.pfb")[..],
            Self::ZapfDingBats => &include_bytes!("../../assets/FoxitDingbats.pfb")[..],
            Self::Symbol => {
                include_bytes!("../../assets/FoxitSymbol.pfb")
            }
        };

        (Arc::new(data), 0)
    }
}

enum StandardFontFamily {
    Helvetica,
    Courier,
    Times,
}

pub(crate) fn select_standard_font(
    dict: &Dict<'_>,
    descriptor: &Dict<'_>,
) -> Option<(StandardFont, bool)> {
    let base_font = dict.get::<Name<'_>>(BASE_FONT)?;
    let name = strip_subset_prefix(base_font.as_str());

    // First try whether it matches literally.
    match name {
        "Helvetica" => return Some((StandardFont::Helvetica, true)),
        "Helvetica-Bold" => return Some((StandardFont::HelveticaBold, true)),
        "Helvetica-Oblique" => return Some((StandardFont::HelveticaOblique, true)),
        "Helvetica-BoldOblique" => return Some((StandardFont::HelveticaBoldOblique, true)),
        "Courier" => return Some((StandardFont::Courier, true)),
        "Courier-Bold" => return Some((StandardFont::CourierBold, true)),
        "Courier-Oblique" => return Some((StandardFont::CourierOblique, true)),
        "Courier-BoldOblique" => return Some((StandardFont::CourierBoldOblique, true)),
        "Times-Roman" => return Some((StandardFont::TimesRoman, true)),
        "Times-Bold" => return Some((StandardFont::TimesBold, true)),
        "Times-Italic" => return Some((StandardFont::TimesItalic, true)),
        "Times-BoldItalic" => return Some((StandardFont::TimesBoldItalic, true)),
        "Symbol" => return Some((StandardFont::Symbol, true)),
        "ZapfDingbats" => return Some((StandardFont::ZapfDingBats, true)),
        _ => {}
    }

    // Now, we bruteforce, trying to determine a suitable font based on the
    // keywords that appear in the name.
    let lower = name.to_ascii_lowercase();

    let is_bold = descriptor.get::<u32>(FONT_WEIGHT).is_some_and(|w| w >= 700)
        || lower.contains("bold")
        || lower.contains("demi");
    let is_italic = descriptor
        .get::<f32>(ITALIC_ANGLE)
        .is_some_and(|a| a != 0.0)
        || lower.contains("italic")
        || lower.contains("oblique");

    let (family, exact) = if lower.contains("helvetica") {
        (Some(StandardFontFamily::Helvetica), true)
    } else if lower.contains("arial") || lower.contains("sans") {
        (Some(StandardFontFamily::Helvetica), false)
    } else if lower.contains("courier") {
        (Some(StandardFontFamily::Courier), true)
    } else if lower.contains("mono") {
        (Some(StandardFontFamily::Courier), false)
    } else if lower.contains("times") {
        (Some(StandardFontFamily::Times), true)
    } else if lower.contains("serif") {
        (Some(StandardFontFamily::Times), false)
    } else if lower.contains("zapfdingbats") || lower.contains("dingbats") {
        return Some((StandardFont::ZapfDingBats, false));
    } else {
        (None, false)
    };

    let font = match (family?, is_bold, is_italic) {
        (StandardFontFamily::Helvetica, false, false) => StandardFont::Helvetica,
        (StandardFontFamily::Helvetica, true, false) => StandardFont::HelveticaBold,
        (StandardFontFamily::Helvetica, false, true) => StandardFont::HelveticaOblique,
        (StandardFontFamily::Helvetica, true, true) => StandardFont::HelveticaBoldOblique,
        (StandardFontFamily::Courier, false, false) => StandardFont::Courier,
        (StandardFontFamily::Courier, true, false) => StandardFont::CourierBold,
        (StandardFontFamily::Courier, false, true) => StandardFont::CourierOblique,
        (StandardFontFamily::Courier, true, true) => StandardFont::CourierBoldOblique,
        (StandardFontFamily::Times, false, false) => StandardFont::TimesRoman,
        (StandardFontFamily::Times, true, false) => StandardFont::TimesBold,
        (StandardFontFamily::Times, false, true) => StandardFont::TimesItalic,
        (StandardFontFamily::Times, true, true) => StandardFont::TimesBoldItalic,
    };

    Some((font, exact))
}

#[derive(Debug)]
pub(crate) enum StandardFontBlob {
    Cff(CffFontBlob),
    Otf(OpenTypeFontBlob, HashMap<String, GlyphId>),
}

impl StandardFontBlob {
    pub(crate) fn from_data(data: FontData, index: u32) -> Option<Self> {
        if let Some(blob) = CffFontBlob::new(data.clone()) {
            Some(Self::new_cff(blob))
        } else {
            OpenTypeFontBlob::new(data, index).map(Self::new_otf)
        }
    }

    pub(crate) fn new_cff(blob: CffFontBlob) -> Self {
        Self::Cff(blob)
    }

    pub(crate) fn new_otf(blob: OpenTypeFontBlob) -> Self {
        let glyph_names = blob.glyph_names();
        Self::Otf(blob, glyph_names)
    }
}

impl StandardFontBlob {
    pub(crate) fn name_to_glyph(&self, name: &str) -> Option<GlyphId> {
        match self {
            Self::Cff(blob) => blob.glyph_index_by_name(name),
            Self::Otf(_, glyph_names) => glyph_names.get(name).copied(),
        }
    }

    pub(crate) fn unicode_to_glyph(&self, code: u32) -> Option<GlyphId> {
        match self {
            Self::Cff(_) => None,
            Self::Otf(blob, _) => blob
                .font_ref()
                .cmap()
                .ok()
                .and_then(|c| c.map_codepoint(code)),
        }
    }

    pub(crate) fn advance_width(&self, glyph: GlyphId) -> Option<f32> {
        match self {
            Self::Cff(_) => None,
            Self::Otf(blob, _) => blob.glyph_metrics().advance_width(glyph),
        }
    }

    pub(crate) fn outline_glyph(&self, glyph: GlyphId) -> BezPath {
        // Standard fonts have empty outlines for these, but in Liberation Sans
        // they are a .notdef rectangle.
        if glyph == GlyphId::NOTDEF {
            return BezPath::new();
        }

        match self {
            Self::Cff(blob) => blob.outline_glyph(glyph),
            Self::Otf(blob, _) => blob.outline_glyph(glyph),
        }
    }
}

#[derive(Debug)]
pub(crate) struct StandardKind {
    base_font: StandardFont,
    base_font_blob: StandardFontBlob,
    encoding: Encoding,
    widths: Vec<Width>,
    missing_width: f32,
    fallback: bool,
    glyph_to_code: RefCell<HashMap<GlyphId, u8>>,
    encodings: HashMap<u8, String>,
}

impl StandardKind {
    pub(crate) fn new(dict: &Dict<'_>, resolver: &FontResolverFn) -> Option<Self> {
        let descriptor = dict.get::<Dict<'_>>(FONT_DESC).unwrap_or_default();
        let (font, exact) = select_standard_font(dict, &descriptor)?;
        Self::new_with_standard(dict, font, !exact, resolver)
    }

    pub(crate) fn new_with_standard(
        dict: &Dict<'_>,
        base_font: StandardFont,
        fallback: bool,
        resolver: &FontResolverFn,
    ) -> Option<Self> {
        let descriptor = dict.get::<Dict<'_>>(FONT_DESC).unwrap_or_default();
        let (widths, missing_width) = read_widths(dict, &descriptor)?;

        let (mut encoding, encoding_map) = read_encoding(dict);

        // See PDFJS-16464: Ignore encodings for non-embedded Type1 symbol fonts.
        if matches!(base_font, StandardFont::Symbol | StandardFont::ZapfDingBats) {
            encoding = Encoding::BuiltIn;
        }

        let (blob, index) = resolver(&FontQuery::Standard(base_font))?;
        let base_font_blob = StandardFontBlob::from_data(blob, index)?;

        Some(Self {
            base_font,
            base_font_blob,
            widths,
            missing_width,
            encodings: encoding_map,
            glyph_to_code: RefCell::new(HashMap::new()),
            fallback,
            encoding,
        })
    }

    pub(crate) fn code_to_ps_name(&self, code: u8) -> Option<&str> {
        let bf = self.base_font;

        self.encodings
            .get(&code)
            .map(String::as_str)
            .or_else(|| match self.encoding {
                Encoding::BuiltIn => bf.code_to_name(code),
                _ => self.encoding.map_code(code),
            })
    }

    pub(crate) fn map_code(&self, code: u8) -> GlyphId {
        let result = self
            .code_to_ps_name(code)
            .and_then(|c| {
                self.base_font_blob
                    .name_to_glyph(c)
                    .or_else(|| {
                        // If the font doesn't have a POST table, try to map via unicode instead.
                        glyph_names::get(c).and_then(|c| {
                            self.base_font_blob
                                .unicode_to_glyph(c.chars().nth(0).unwrap() as u32)
                        })
                    })
                    // PdfCraft patch: an accented letter the stand-in lacks is built from its
                    // parts (see `ACCENTED_LETTERS`).
                    .or_else(|| self.composite_glyph(code, c))
            })
            .unwrap_or(GlyphId::NOTDEF);
        self.glyph_to_code.borrow_mut().insert(result, code);

        result
    }

    /// PdfCraft patch: the glyph id standing for the letter `name` built from its base letter and
    /// accent, when the stand-in has both. Ids from [`COMPOSITE_GLYPH_BASE`] up are one per
    /// character code, so the outline is found again from the code alone.
    fn composite_glyph(&self, code: u8, name: &str) -> Option<GlyphId> {
        let (base, accent, _) = accented_parts(name)?;
        self.base_font_blob.name_to_glyph(base)?;
        self.base_font_blob.name_to_glyph(accent)?;
        Some(GlyphId::new(COMPOSITE_GLYPH_BASE + u32::from(code)))
    }

    /// PdfCraft patch: the outline of a glyph from [`Self::composite_glyph`] — the base letter,
    /// then the accent centred on it. An accent above a letter taller than `x` is raised by the
    /// difference (the stand-in's accents sit on lowercase letters) and, in an italic font,
    /// moved along the slant; an ogonek hangs from the letter's right edge; a comma below is
    /// the font's comma, smaller, under the baseline.
    fn composite_outline(&self, glyph: GlyphId) -> Option<BezPath> {
        let code = u8::try_from(glyph.to_u32().checked_sub(COMPOSITE_GLYPH_BASE)?).ok()?;
        let (base, accent, place) = accented_parts(self.code_to_ps_name(code)?)?;
        let outline = |name: &str| {
            self.base_font_blob
                .name_to_glyph(name)
                .map(|g| self.base_font_blob.outline_glyph(g))
        };
        let mut path = outline(base)?;
        let mut accent = outline(accent)?;
        if path.elements().is_empty() || accent.elements().is_empty() {
            return Some(path);
        }
        if place == Place::CommaBelow {
            accent = Affine::scale(COMMA_BELOW_SCALE) * accent;
        }
        let (b, a) = (path.bounding_box(), accent.bounding_box());
        let (dx, dy) = match place {
            Place::Below => (b.center().x - a.center().x, 0.0),
            Place::CommaBelow => {
                // Its top a little below the baseline (or the letter's overshoot below it), and
                // in an italic font moved back along the slant by as much as it is lowered.
                let dy = b.y0.min(0.0) - COMMA_BELOW_GAP * a.height() - a.y1;
                let slant = if self.base_font.is_italic() {
                    dy * ITALIC_ACCENT_SLANT
                } else {
                    0.0
                };
                (b.center().x - a.center().x + slant, dy)
            }
            Place::Ogonek => (b.x1 - a.x1, 0.0),
            Place::Above => {
                let x_top = outline("x")
                    .filter(|x| !x.elements().is_empty())
                    .map_or(b.y1, |x| x.bounding_box().y1);
                // Overshoot of round lowercase letters (o, g, s) over x is not a taller letter.
                let raise = if b.y1 > x_top * 1.1 {
                    b.y1 - x_top
                } else {
                    0.0
                };
                let slant = if self.base_font.is_italic() {
                    raise * ITALIC_ACCENT_SLANT
                } else {
                    0.0
                };
                (b.center().x - a.center().x + slant, raise)
            }
        };
        if !(dx.is_finite() && dy.is_finite()) {
            return Some(path);
        }
        path.extend(Affine::translate((dx, dy)) * accent);
        Some(path)
    }

    pub(crate) fn outline_glyph(&self, glyph: GlyphId) -> BezPath {
        let composite = self.composite_outline(glyph);
        let is_composite = composite.is_some();
        let path = composite.unwrap_or_else(|| self.base_font_blob.outline_glyph(glyph));

        // If the font is not embedded, we might need to stretch it so that
        // it matches the metrics of the actual underlying font blob.

        // PdfCraft patch: a composite id isn't a glyph of the blob, so its width comes from the
        // standard metrics of its name.
        let blob_width = if is_composite {
            None
        } else {
            self.base_font_blob.advance_width(glyph)
        };
        if let Some(code) = self.glyph_to_code.borrow().get(&glyph).copied()
            && let Some(actual_width) = blob_width.or_else(|| {
                self.code_to_ps_name(code)
                    .and_then(|name| self.base_font.get_width(name))
            })
        {
            // From my experiments: Most PDF viewers, if they detect a font is a
            // standard font, they completely ignore the widths array, even if
            // different widths are indicated there. So only if it's an unknown
            // font do we check the widths array. Otherwise, we always use the
            // base font metrics.
            let should_width = if self.fallback {
                if let Some(Width::Value(w)) = self.widths.get(code as usize).copied() {
                    w
                } else {
                    return path;
                }
            } else if let Some(w) = self
                .code_to_ps_name(code)
                .and_then(|name| self.base_font.get_width(name))
            {
                w
            } else {
                return path;
            };

            return stretch_glyph(path, should_width, actual_width);
        }

        path
    }

    pub(crate) fn glyph_width(&self, code: u8) -> Option<f32> {
        match self.widths.get(code as usize).copied() {
            Some(Width::Value(w)) => Some(w),
            Some(Width::Missing) => Some(self.missing_width),
            None => self
                .code_to_ps_name(code)
                .and_then(|c| self.base_font.get_width(c)),
        }
    }

    pub(crate) fn char_code_to_unicode(&self, code: u8) -> Option<char> {
        // PdfCraft patch: an Annex D.2 alias code reads as the ordinary character; see
        // `Encoding::text_alias`. `code_to_ps_name` is shared with glyph lookup and stays as it
        // is, so the alias is applied here, after `/Differences` has had its say.
        if !self.encodings.contains_key(&code)
            && let Some(name) = self.encoding.text_alias(code)
        {
            return glyph_name_to_unicode(name);
        }
        self.code_to_ps_name(code).and_then(glyph_name_to_unicode)
    }

    pub(crate) fn is_italic(&self) -> bool {
        self.base_font.is_italic()
    }

    pub(crate) fn is_bold(&self) -> bool {
        self.base_font.is_bold()
    }

    pub(crate) fn is_serif(&self) -> bool {
        self.base_font.is_serif()
    }

    pub(crate) fn is_monospace(&self) -> bool {
        self.base_font.is_monospace()
    }
}

/// PdfCraft patch: the first glyph id of the letters [`StandardKind`] builds from parts, far
/// above the glyph count of any font (one id per character code from here).
const COMPOSITE_GLYPH_BASE: u32 = 0x0100_0000;

/// PdfCraft patch: how far an accent raised over a capital moves right per unit of height in
/// an italic stand-in (tan 15°; the standard italics lean 12° to 16°).
const ITALIC_ACCENT_SLANT: f64 = 0.27;

/// PdfCraft patch: the size of a comma accent against the font's comma. A full-size comma
/// moved under the baseline reaches past the descenders and into the line below.
const COMMA_BELOW_SCALE: f64 = 0.75;

/// PdfCraft patch: the gap between the baseline and a comma accent, as a share of its height.
const COMMA_BELOW_GAP: f64 = 0.15;

/// Where an accent goes on its base letter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Above,
    Below,
    Ogonek,
    /// The font's comma, smaller, under the letter (the stand-ins have no `commaaccent`).
    CommaBelow,
}

/// PdfCraft patch: the stand-ins for the standard fonts (PDFium's Foxit fonts) carry only the
/// glyphs of the Standard, WinAnsi and Mac Roman encodings, so a font that isn't embedded and
/// names other Latin letters in its /Differences (`/Gbreve`, `/Idotaccent`, `/Scedilla`…, as
/// iText 2 writes Turkish text with Times New Roman left unembedded) drew nothing for them. Each of
/// these Latin Extended-A letters is a base letter the stand-ins have plus one of their accents,
/// as Type 1 `seac` composites and the composites of the standard-14 AFM files are built.
/// The comma-below letters of Romanian (ș ț) and Latvian (ķ ļ ņ ŗ Ģ) are the base letter and the
/// font's comma, since the stand-ins have no `commaaccent` either; without them a form laid out
/// from its XFA template drew Romanian ș as a blank and ț with a cedilla. Letters whose accent
/// isn't a plain one (ď ľ ť, whose caron is an apostrophe beside the letter, and ģ, whose comma
/// is turned and above it) aren't here.
const ACCENTED_LETTERS: &[(&str, &str, &str, Place)] = &[
    ("Amacron", "A", "macron", Place::Above),
    ("amacron", "a", "macron", Place::Above),
    ("Abreve", "A", "breve", Place::Above),
    ("abreve", "a", "breve", Place::Above),
    ("Aogonek", "A", "ogonek", Place::Ogonek),
    ("aogonek", "a", "ogonek", Place::Ogonek),
    ("Cacute", "C", "acute", Place::Above),
    ("cacute", "c", "acute", Place::Above),
    ("Ccircumflex", "C", "circumflex", Place::Above),
    ("ccircumflex", "c", "circumflex", Place::Above),
    ("Cdotaccent", "C", "dotaccent", Place::Above),
    ("cdotaccent", "c", "dotaccent", Place::Above),
    ("Ccaron", "C", "caron", Place::Above),
    ("ccaron", "c", "caron", Place::Above),
    ("Dcaron", "D", "caron", Place::Above),
    ("Emacron", "E", "macron", Place::Above),
    ("emacron", "e", "macron", Place::Above),
    ("Ebreve", "E", "breve", Place::Above),
    ("ebreve", "e", "breve", Place::Above),
    ("Edotaccent", "E", "dotaccent", Place::Above),
    ("edotaccent", "e", "dotaccent", Place::Above),
    ("Eogonek", "E", "ogonek", Place::Ogonek),
    ("eogonek", "e", "ogonek", Place::Ogonek),
    ("Ecaron", "E", "caron", Place::Above),
    ("ecaron", "e", "caron", Place::Above),
    ("Gcircumflex", "G", "circumflex", Place::Above),
    ("gcircumflex", "g", "circumflex", Place::Above),
    ("Gbreve", "G", "breve", Place::Above),
    ("gbreve", "g", "breve", Place::Above),
    ("Gdotaccent", "G", "dotaccent", Place::Above),
    ("Gcommaaccent", "G", "comma", Place::CommaBelow),
    ("gdotaccent", "g", "dotaccent", Place::Above),
    ("Hcircumflex", "H", "circumflex", Place::Above),
    ("hcircumflex", "h", "circumflex", Place::Above),
    ("Itilde", "I", "tilde", Place::Above),
    ("itilde", "dotlessi", "tilde", Place::Above),
    ("Imacron", "I", "macron", Place::Above),
    ("imacron", "dotlessi", "macron", Place::Above),
    ("Ibreve", "I", "breve", Place::Above),
    ("ibreve", "dotlessi", "breve", Place::Above),
    ("Iogonek", "I", "ogonek", Place::Ogonek),
    ("iogonek", "i", "ogonek", Place::Ogonek),
    ("Idotaccent", "I", "dotaccent", Place::Above),
    ("Jcircumflex", "J", "circumflex", Place::Above),
    ("Kcommaaccent", "K", "comma", Place::CommaBelow),
    ("kcommaaccent", "k", "comma", Place::CommaBelow),
    ("Lacute", "L", "acute", Place::Above),
    ("lacute", "l", "acute", Place::Above),
    ("Lcommaaccent", "L", "comma", Place::CommaBelow),
    ("lcommaaccent", "l", "comma", Place::CommaBelow),
    ("Nacute", "N", "acute", Place::Above),
    ("nacute", "n", "acute", Place::Above),
    ("Ncommaaccent", "N", "comma", Place::CommaBelow),
    ("ncommaaccent", "n", "comma", Place::CommaBelow),
    ("Ncaron", "N", "caron", Place::Above),
    ("ncaron", "n", "caron", Place::Above),
    ("Omacron", "O", "macron", Place::Above),
    ("omacron", "o", "macron", Place::Above),
    ("Obreve", "O", "breve", Place::Above),
    ("obreve", "o", "breve", Place::Above),
    ("Ohungarumlaut", "O", "hungarumlaut", Place::Above),
    ("ohungarumlaut", "o", "hungarumlaut", Place::Above),
    ("Racute", "R", "acute", Place::Above),
    ("racute", "r", "acute", Place::Above),
    ("Rcommaaccent", "R", "comma", Place::CommaBelow),
    ("rcommaaccent", "r", "comma", Place::CommaBelow),
    ("Rcaron", "R", "caron", Place::Above),
    ("rcaron", "r", "caron", Place::Above),
    ("Sacute", "S", "acute", Place::Above),
    ("sacute", "s", "acute", Place::Above),
    ("Scircumflex", "S", "circumflex", Place::Above),
    ("scircumflex", "s", "circumflex", Place::Above),
    ("Scedilla", "S", "cedilla", Place::Below),
    ("scedilla", "s", "cedilla", Place::Below),
    ("Scommaaccent", "S", "comma", Place::CommaBelow),
    ("scommaaccent", "s", "comma", Place::CommaBelow),
    ("Tcedilla", "T", "cedilla", Place::Below),
    ("tcedilla", "t", "cedilla", Place::Below),
    ("Tcommaaccent", "T", "comma", Place::CommaBelow),
    ("tcommaaccent", "t", "comma", Place::CommaBelow),
    ("Tcaron", "T", "caron", Place::Above),
    ("Utilde", "U", "tilde", Place::Above),
    ("utilde", "u", "tilde", Place::Above),
    ("Umacron", "U", "macron", Place::Above),
    ("umacron", "u", "macron", Place::Above),
    ("Ubreve", "U", "breve", Place::Above),
    ("ubreve", "u", "breve", Place::Above),
    ("Uring", "U", "ring", Place::Above),
    ("uring", "u", "ring", Place::Above),
    ("Uhungarumlaut", "U", "hungarumlaut", Place::Above),
    ("uhungarumlaut", "u", "hungarumlaut", Place::Above),
    ("Uogonek", "U", "ogonek", Place::Ogonek),
    ("uogonek", "u", "ogonek", Place::Ogonek),
    ("Wcircumflex", "W", "circumflex", Place::Above),
    ("wcircumflex", "w", "circumflex", Place::Above),
    ("Ycircumflex", "Y", "circumflex", Place::Above),
    ("ycircumflex", "y", "circumflex", Place::Above),
    ("Zacute", "Z", "acute", Place::Above),
    ("zacute", "z", "acute", Place::Above),
    ("Zdotaccent", "Z", "dotaccent", Place::Above),
    ("zdotaccent", "z", "dotaccent", Place::Above),
];

/// PdfCraft patch: the comma-below letters of [`ACCENTED_LETTERS`] by character. The glyph list
/// names the Latvian ones `Kcedilla`… (their comma is a cedilla in Unicode's names), has no name
/// for Ț ț, and gives `Tcommaaccent` and `tcommaaccent` to Ţ ţ, which keep their cedilla.
const COMMA_BELOW_BY_CHAR: &[(char, &str)] = &[
    ('\u{0122}', "Gcommaaccent"),
    ('\u{0136}', "Kcommaaccent"),
    ('\u{0137}', "kcommaaccent"),
    ('\u{013B}', "Lcommaaccent"),
    ('\u{013C}', "lcommaaccent"),
    ('\u{0145}', "Ncommaaccent"),
    ('\u{0146}', "ncommaaccent"),
    ('\u{0156}', "Rcommaaccent"),
    ('\u{0157}', "rcommaaccent"),
    ('\u{0218}', "Scommaaccent"),
    ('\u{0219}', "scommaaccent"),
    ('\u{021A}', "Tcommaaccent"),
    ('\u{021B}', "tcommaaccent"),
];

/// PdfCraft patch: the base letter, accent and placement of an accented letter in
/// [`ACCENTED_LETTERS`], by glyph name (`uni011E` and `u011E` forms too). A comma-below letter
/// is found by its character ([`COMMA_BELOW_BY_CHAR`]), so `/uni0136` and `/Kcedilla` are Ķ too.
fn accented_parts(name: &str) -> Option<(&'static str, &'static str, Place)> {
    let name = normalized_glyph_name(name);
    let by_name = |n: &str| {
        ACCENTED_LETTERS
            .iter()
            .find(|(letter, ..)| *letter == n)
            .map(|&(_, base, accent, place)| (base, accent, place))
    };
    by_name(name).or_else(|| {
        let c = glyph_name_to_unicode(name)?;
        match COMMA_BELOW_BY_CHAR.iter().find(|(letter, _)| *letter == c) {
            Some((_, comma_below)) => by_name(comma_below),
            None => by_name(glyph_names::get_reverse(c)?),
        }
    })
}
