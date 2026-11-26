//! PDF 1.7 drawing for GridCraft print/export, with shared object and xref framing.
//!
//! - The 14 standard fonts (no embedding) with WinAnsiEncoding, plus approximate width tables
//!   ([`text_width`]) so callers can measure, align and wrap text. Characters outside
//!   WinAnsiEncoding (CJK and other scripts) are set in a predefined, non-embedded Adobe-GB1 CID
//!   font with the `UniGB-UTF16-H` CMap instead of being replaced by `?`. Symbol uses its own
//!   built-in encoding ([`symbol_code`]), for Greek letters and mathematical operators.
//! - Arabic is shaped and set in embedded Type3 subsets, with exact shaped advances.
//! - Vector drawing: rectangles, lines (solid or dashed), polygons, circles (Bézier), clipping.
//! - RGBA images as RGB image XObjects with a soft mask for alpha (run-length encoded).
//! - Coordinates are points with a **top-left origin** (y grows down); text `y` is the baseline.
//!
//! Streams are written uncompressed (images use the RunLengthDecode filter).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod arabic;
mod metrics;
mod symbol;

pub use symbol::{symbol_code, symbol_width};

/// Paragraph base direction. Context follows spreadsheet reading order: the first
/// non-whitespace character determines direction, including digits and punctuation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextDirection {
    #[default]
    Auto,
    Context,
    LeftToRight,
    RightToLeft,
}

impl TextDirection {
    /// Resolve once for a paragraph before wrapping it into visual lines.
    pub fn resolve(self, text: &str) -> Self {
        use unicode_bidi::{BidiClass, BidiInfo, bidi_class};
        let rtl = match self {
            Self::Auto => BidiInfo::new(text, None).paragraphs.first().is_some_and(|p| p.level.is_rtl()),
            Self::Context => text.chars().find(|ch| !ch.is_whitespace()).is_some_and(|ch| matches!(bidi_class(ch), BidiClass::R | BidiClass::AL)),
            explicit => return explicit,
        };
        if rtl { Self::RightToLeft } else { Self::LeftToRight }
    }

    fn level(self) -> Option<unicode_bidi::Level> {
        match self {
            Self::Auto => None,
            Self::LeftToRight | Self::Context => Some(unicode_bidi::Level::ltr()),
            Self::RightToLeft => Some(unicode_bidi::Level::rtl()),
        }
    }
}

/// A PDF export could not preserve the requested text or serialize its object envelope.
#[derive(Clone, Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

impl From<craft_pdf::classic::Error> for Error {
    fn from(error: craft_pdf::classic::Error) -> Self {
        Self(error.to_string())
    }
}

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

/// RGB colour, 0–255 per channel.
pub type Rgb = [u8; 3];

/// A point in page coordinates, measured from the top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A text baseline origin and its clockwise rotation in radians.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Baseline {
    pub origin: Point,
    pub angle: f32,
}

/// A page-space rectangle described by its top-left corner and extent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }
}

/// The standard 14 fonts minus ZapfDingbats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Font {
    #[default]
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
    /// Greek letters and mathematical symbols, in the font's own encoding.
    Symbol,
}

/// How many fonts a document can use.
const FONT_COUNT: usize = 13;

/// A font family for [`Font::styled`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Family {
    #[default]
    Sans,
    Serif,
    Mono,
}

impl Font {
    pub const ALL: [Font; FONT_COUNT] = [
        Font::Helvetica,
        Font::HelveticaBold,
        Font::HelveticaOblique,
        Font::HelveticaBoldOblique,
        Font::TimesRoman,
        Font::TimesBold,
        Font::TimesItalic,
        Font::TimesBoldItalic,
        Font::Courier,
        Font::CourierBold,
        Font::CourierOblique,
        Font::CourierBoldOblique,
        Font::Symbol,
    ];

    /// PostScript base font name.
    pub fn base_name(self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::HelveticaBold => "Helvetica-Bold",
            Font::HelveticaOblique => "Helvetica-Oblique",
            Font::HelveticaBoldOblique => "Helvetica-BoldOblique",
            Font::TimesRoman => "Times-Roman",
            Font::TimesBold => "Times-Bold",
            Font::TimesItalic => "Times-Italic",
            Font::TimesBoldItalic => "Times-BoldItalic",
            Font::Courier => "Courier",
            Font::CourierBold => "Courier-Bold",
            Font::CourierOblique => "Courier-Oblique",
            Font::CourierBoldOblique => "Courier-BoldOblique",
            Font::Symbol => "Symbol",
        }
    }

    fn index(self) -> usize {
        Font::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// The face of `family` with the given weight and slant.
    pub fn styled(family: Family, bold: bool, italic: bool) -> Font {
        match (family, bold, italic) {
            (Family::Sans, false, false) => Font::Helvetica,
            (Family::Sans, true, false) => Font::HelveticaBold,
            (Family::Sans, false, true) => Font::HelveticaOblique,
            (Family::Sans, true, true) => Font::HelveticaBoldOblique,
            (Family::Serif, false, false) => Font::TimesRoman,
            (Family::Serif, true, false) => Font::TimesBold,
            (Family::Serif, false, true) => Font::TimesItalic,
            (Family::Serif, true, true) => Font::TimesBoldItalic,
            (Family::Mono, false, false) => Font::Courier,
            (Family::Mono, true, false) => Font::CourierBold,
            (Family::Mono, false, true) => Font::CourierOblique,
            (Family::Mono, true, true) => Font::CourierBoldOblique,
        }
    }

    /// Picks a standard family for a font name: serif faces map to Times, monospaced ones to
    /// Courier, everything else to Helvetica.
    pub fn family_for(name: &str) -> Family {
        let n = name.to_ascii_lowercase();
        const SERIF: [&str; 10] = ["times", "cambria", "georgia", "garamond", "serif", "book", "palatino", "baskerville", "tinos", "caladea"];
        const MONO: [&str; 8] = ["courier", "consolas", "mono", "menlo", "lucida console", "cousine", "code", "typewriter"];
        if MONO.iter().any(|m| n.contains(m)) {
            Family::Mono
        } else if SERIF.iter().any(|s| n.contains(s)) && !n.contains("sans") {
            Family::Serif
        } else {
            Family::Sans
        }
    }

    /// Whether this font can show `c` by itself (in its own encoding; for the text faces,
    /// without the CID fallback).
    pub fn can_encode(self, c: char) -> bool {
        match self {
            Font::Symbol => symbol_code(c).is_some(),
            _ => c == '?' || win_ansi(c) != b'?',
        }
    }

    /// Advance width of a character in 1/1000 em.
    pub fn char_width(self, c: char) -> u16 {
        if self == Font::Symbol {
            return symbol_code(c).and_then(symbol_width).unwrap_or(500);
        }
        let table: Option<&[u16; 95]> = match self {
            Font::Helvetica | Font::HelveticaOblique => Some(&metrics::HELVETICA),
            Font::HelveticaBold | Font::HelveticaBoldOblique => Some(&metrics::HELVETICA_BOLD),
            Font::TimesRoman | Font::TimesItalic => Some(&metrics::TIMES),
            Font::TimesBold | Font::TimesBoldItalic => Some(&metrics::TIMES_BOLD),
            _ => None,
        };
        if needs_cid(c) {
            return CID_WIDTH;
        }
        let Some(table) = table else { return 600 };
        let ascii = |c: char| -> Option<u16> {
            let i = (c as u32).checked_sub(32)? as usize;
            table.get(i).copied()
        };
        if let Some(w) = ascii(c) {
            return w;
        }
        match c {
            '—' | '…' | '‰' | '™' => 1000,
            _ => metrics::fold(c).and_then(ascii).unwrap_or(match self {
                Font::TimesRoman | Font::TimesItalic | Font::TimesBold | Font::TimesBoldItalic => 500,
                _ => 556,
            }),
        }
    }
}

/// Whether [`text_width_with_direction`] is the sum of `text`'s character widths: not shaped,
/// not reordered right to left and holding no bidi controls. A prefix's width is then a running sum.
pub fn width_is_char_sum(font: Font, text: &str, direction: TextDirection) -> bool {
    let direction = if direction == TextDirection::Context { direction.resolve(text) } else { direction };
    !arabic::shaped(font, text) && arabic::is_logical_order(text, direction)
}

/// Width of `text` in points when set in `font` at `size` points.
/// Arabic uses shaped advances. If its font cannot render the text, measurement returns zero
/// and drawing that text causes [`PdfDoc::finish`] to return the rendering error.
pub fn text_width(font: Font, size: f32, text: &str) -> f32 {
    text_width_with_direction(font, size, text, TextDirection::Auto)
}

/// Width measured with the same base direction as drawing.
pub fn text_width_with_direction(font: Font, size: f32, text: &str, direction: TextDirection) -> f32 {
    let direction = if direction == TextDirection::Context { direction.resolve(text) } else { direction };
    if arabic::shaped(font, text) {
        return arabic::layout(font, text, direction).map(|layout| layout.width * size / 1000.0).unwrap_or(0.0);
    }
    // u64: a u32 sum overflows for ~8 million characters.
    let Ok(visual) = arabic::latin_visual(text, direction) else { return 0.0 };
    let units: u64 = visual.chars().map(|c| u64::from(font.char_width(c))).sum();
    units as f32 * size / 1000.0
}

/// Unicode → WinAnsiEncoding (CP1252) byte; unmappable characters become `?`.
pub fn win_ansi(c: char) -> u8 {
    let u = c as u32;
    if (0x20..0x7F).contains(&u) || (0xA0..=0xFF).contains(&u) {
        return u as u8;
    }
    match c {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        '\t' => b' ',
        _ => b'?',
    }
}

/// Index of the CID font in a page's font flags (after the [`Font::ALL`] faces).
const CID_FONT: usize = Font::ALL.len();
/// Number of font resources a page can use.
const FONT_SLOTS: usize = CID_FONT + 1;
/// Base font of the CID font used for characters WinAnsiEncoding lacks.
const CID_BASE_FONT: &str = "STSong-Light";
/// Default glyph width of the CID font (full-width), in 1/1000 em.
const CID_WIDTH: u16 = 1000;

/// True when `c` has no WinAnsiEncoding byte and is drawn with the CID font.
fn needs_cid(c: char) -> bool {
    !c.is_control() && c != '?' && win_ansi(c) == b'?'
}

/// A PDF hex string `< … >` of `text` in UTF-16BE (the `UniGB-UTF16-H` encoding).
fn pdf_utf16_string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() * 4 + 2);
    s.push('<');
    for unit in text.encode_utf16() {
        let _ = write!(s, "{unit:04X}");
    }
    s.push('>');
    s
}

/// A PDF literal string `( … )` for `text` in WinAnsiEncoding.
fn pdf_string(text: &str) -> String {
    pdf_string_in(text, false)
}

/// A PDF literal string for `text` in WinAnsiEncoding, or in Symbol's encoding (where a
/// character Symbol lacks becomes `?`).
fn pdf_string_in(text: &str, symbol: bool) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push('(');
    for c in text.chars() {
        let b = if symbol { symbol_code(c).unwrap_or(b'?') } else { win_ansi(c) };
        match b {
            b'(' | b')' | b'\\' => {
                s.push('\\');
                s.push(b as char);
            }
            0x20..=0x7E => s.push(b as char),
            _ => {
                let _ = write!(s, "\\{b:03o}");
            }
        }
    }
    s.push(')');
    s
}

/// URI actions use byte strings. Percent-encode non-ASCII UTF-8 before PDF string escaping.
fn pdf_uri_string(uri: &str) -> String {
    let mut ascii = String::with_capacity(uri.len());
    for byte in uri.bytes() {
        if byte.is_ascii() {
            ascii.push(byte as char);
        } else {
            let _ = write!(ascii, "%{byte:02X}");
        }
    }
    pdf_string(&ascii)
}

/// A number for a content stream: finite, short.
fn n(v: f32) -> String {
    let v = if v.is_finite() { v.clamp(-1.0e7, 1.0e7) } else { 0.0 };
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Appends `v / 255` with at most three decimals, exactly what `n(v as f32 / 255.0)` gives, but
/// without a float round trip or an allocation per channel.
fn push_unit(s: &mut String, v: u8) {
    let m = (u32::from(v) * 1000 + 127) / 255;
    match m {
        0 => s.push('0'),
        1000 => s.push('1'),
        _ => {
            let _ = write!(s, "0.{m:03}");
            while s.ends_with('0') {
                s.pop();
            }
        }
    }
}

/// Appends a colour operator line without the trailing newline: `r g b rg` (fill) or `r g b RG`.
fn push_rgb(s: &mut String, c: Rgb, stroke: bool) {
    for v in c {
        push_unit(s, v);
        s.push(' ');
    }
    s.push_str(if stroke { "RG" } else { "rg" });
}

/// True when every value is finite (NaN and infinities are rejected).
fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// A positive, finite extent (a width, height or radius).
fn extent(v: f32) -> bool {
    v > 0.0 && v.is_finite()
}

/// A stroke width: negative, NaN and infinite become 0 (the thinnest line).
fn stroke_width(w: f32) -> f32 {
    if w.is_finite() { w.max(0.0) } else { 0.0 }
}

/// The entries of a dash array (`[on, off, …]`, at most 8). A pattern the PDF specification
/// forbids (any non-finite entry, or no entry that survives rounding to a positive number) is
/// replaced by an empty array, i.e. a solid line.
fn dash_values(dash: &[f32]) -> String {
    let d: Vec<f32> = dash.iter().take(8).map(|v| if v.is_finite() { v.max(0.0) } else { f32::NAN }).collect();
    if !finite(&d) || !d.iter().any(|v| (v.min(1.0e7) * 1000.0).round() > 0.0) {
        return String::new();
    }
    d.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" ")
}

struct ImageData {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    /// `None` when every pixel is fully opaque (no soft mask is written).
    alpha: Option<Vec<u8>>,
    /// Hash of all of the above, to find duplicates quickly; equality still compares the bytes.
    hash: u64,
}

impl ImageData {
    fn new(width: u32, height: u32, rgb: Vec<u8>, alpha: Option<Vec<u8>>) -> ImageData {
        let mut h = DefaultHasher::new();
        width.hash(&mut h);
        height.hash(&mut h);
        rgb.hash(&mut h);
        alpha.hash(&mut h);
        ImageData { width, height, rgb, alpha, hash: h.finish() }
    }

    /// Same pixels (a hash match alone is never trusted).
    fn same(&self, o: &ImageData) -> bool {
        self.hash == o.hash && self.width == o.width && self.height == o.height && self.rgb == o.rgb && self.alpha == o.alpha
    }
}

/// The part of the graphics state the writer tracks so that a style operator is only emitted
/// when it changes. `None` = not set yet on this page (or since the last restore that undid it).
#[derive(Clone, Debug, Default, PartialEq)]
struct Style {
    fill: Option<Rgb>,
    stroke: Option<Rgb>,
    /// Formatted line width.
    width: Option<String>,
    /// Formatted dash array contents.
    dash: Option<String>,
    /// Line join; PDF's initial value is 0 (miter).
    join: u8,
}

enum LinkTarget {
    Uri(String),
    Page { index: usize, x: f32, y: f32 },
}

struct LinkAnnotation {
    /// PDF coordinates, with the origin at the bottom left.
    rect: [f32; 4],
    target: LinkTarget,
}

/// A page under construction. Obtain one with [`PdfDoc::add_page`].
pub struct Page {
    width: f32,
    height: f32,
    content: String,
    fonts: [bool; FONT_SLOTS],
    images: Vec<ImageData>,
    /// Indexes into `images`, by content hash; a hash match is confirmed with [`ImageData::same`].
    image_index: HashMap<u64, Vec<usize>>,
    /// Pixel comparisons made while looking up duplicate pictures (test instrumentation).
    #[cfg(test)]
    image_comparisons: usize,
    /// Style operators already in effect at the end of `content`.
    style: Style,
    /// One entry per accepted [`Page::save`], restored by [`Page::restore`].
    saved: Vec<Style>,
    /// `save` calls refused at the nesting limit; the same number of `restore` calls are ignored.
    refused: u32,
    links: Vec<LinkAnnotation>,
    arabic: arabic::Subset,
    error: Option<Error>,
    missing_arabic_font: bool,
}

/// Most bytes of content per page before further drawing is dropped (keeps hostile inputs from
/// exhausting memory).
const MAX_PAGE_CONTENT: usize = 64 * 1024 * 1024;
/// Largest image (pixels) embedded.
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
/// Deepest `q` nesting a PDF consumer must support (the PDF specification's implementation limits).
const MAX_SAVE_DEPTH: usize = 28;
/// Line join styles, as the `j` operator takes them.
const JOIN_MITER: u8 = 0;
const JOIN_ROUND: u8 = 1;
const MAX_PAGE_LINKS: usize = 4096;
const MAX_LINK_URI_BYTES: usize = 4096;

impl Page {
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn height(&self) -> f32 {
        self.height
    }
    fn y(&self, y: f32) -> f32 {
        self.height - y
    }
    fn full(&self) -> bool {
        self.content.len() > MAX_PAGE_CONTENT
    }
    fn op(&mut self, s: &str) {
        if !self.full() {
            self.content.push_str(s);
            self.content.push('\n');
        }
    }

    /// Appends a fill colour operator to `s` unless that colour is already in effect.
    fn set_fill(&mut self, s: &mut String, c: Rgb) {
        if self.style.fill != Some(c) {
            push_rgb(s, c, false);
            s.push('\n');
            self.style.fill = Some(c);
        }
    }

    /// Appends the stroke colour, width, dash and join operators to `s` that differ from the
    /// style already in effect.
    fn set_stroke(&mut self, s: &mut String, c: Rgb, width: f32, dash: &str, join: u8) {
        if self.style.stroke != Some(c) {
            push_rgb(s, c, true);
            s.push('\n');
            self.style.stroke = Some(c);
        }
        let w = n(stroke_width(width));
        if self.style.width.as_deref() != Some(w.as_str()) {
            let _ = writeln!(s, "{w} w");
            self.style.width = Some(w);
        }
        if self.style.dash.as_deref() != Some(dash) {
            let _ = writeln!(s, "[{dash}] 0 d");
            self.style.dash = Some(dash.to_string());
        }
        if self.style.join != join {
            let _ = writeln!(s, "{join} j");
            self.style.join = join;
        }
    }

    fn link_rect(&self, rect: Rect) -> Option<[f32; 4]> {
        let Rect { x, y, width, height } = rect;
        if !finite(&[x, y, width, height]) || !extent(width) || !extent(height) || self.links.len() >= MAX_PAGE_LINKS {
            return None;
        }
        let (right, bottom) = (x + width, y + height);
        if !finite(&[right, bottom]) {
            return None;
        }
        let (left, right) = (x.max(0.0), right.min(self.width));
        let (top, bottom) = (y.max(0.0), bottom.min(self.height));
        (right > left && bottom > top).then(|| [left, self.y(bottom), right, self.y(top)])
    }

    /// Adds an invisible clickable area for an external URI. Invalid or excessive links are ignored.
    pub fn link_uri(&mut self, x: f32, y: f32, w: f32, h: f32, uri: &str) {
        let allowed = uri
            .split_once(':')
            .is_some_and(|(scheme, rest)| matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "mailto") && !rest.is_empty());
        if !allowed || uri.len() > MAX_LINK_URI_BYTES || uri.chars().any(char::is_control) {
            return;
        }
        if let Some(rect) = self.link_rect(Rect::new(x, y, w, h)) {
            self.links.push(LinkAnnotation { rect, target: LinkTarget::Uri(uri.to_string()) });
        }
    }

    /// Adds a link to a location on another page of this document (zero-based page index).
    pub fn link_page(&mut self, rect: Rect, page: usize, target: Point) {
        if !finite(&[target.x, target.y]) {
            return;
        }
        if let Some(rect) = self.link_rect(rect) {
            self.links.push(LinkAnnotation { rect, target: LinkTarget::Page { index: page, x: target.x, y: target.y } });
        }
    }

    /// Saves the graphics state (pair with [`Page::restore`]). Nesting is limited to 28 levels, the
    /// depth every PDF consumer must support; a `save` beyond that is ignored, and so is the
    /// `restore` that would have matched it.
    pub fn save(&mut self) {
        if self.saved.len() >= MAX_SAVE_DEPTH {
            self.refused = self.refused.saturating_add(1);
            return;
        }
        self.saved.push(self.style.clone());
        self.content.push_str("q\n");
    }
    /// Restores the graphics state saved by [`Page::save`]; extra calls are ignored.
    pub fn restore(&mut self) {
        if self.refused > 0 {
            self.refused -= 1;
        } else if let Some(style) = self.saved.pop() {
            self.style = style;
            self.content.push_str("Q\n");
        }
    }
    /// Intersects the clip with a rectangle (until the next [`Page::restore`]).
    pub fn clip_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = format!("{} {} {} {} re W n", n(x), n(self.y(y + h)), n(w.max(0.0)), n(h.max(0.0)));
        self.op(&s);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        if !(extent(w) && extent(h) && finite(&[x, y])) || self.full() {
            return;
        }
        let mut s = String::new();
        self.set_fill(&mut s, color);
        let _ = write!(s, "{} {} {} {} re f", n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }

    pub fn rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: Rgb) {
        if !finite(&[x, y, w, h]) || self.full() {
            return;
        }
        let mut s = String::new();
        self.set_stroke(&mut s, color, width, "", JOIN_MITER);
        let _ = write!(s, "{} {} {} {} re S", n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }

    pub fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Rgb) {
        self.stroke_line_dashed(Point::new(x0, y0), Point::new(x1, y1), width, color, &[]);
    }

    /// A line with a dash pattern (`[on, off, …]` in points; empty = solid).
    pub fn stroke_line_dashed(&mut self, start: Point, end: Point, width: f32, color: Rgb, dash: &[f32]) {
        if !finite(&[start.x, start.y, end.x, end.y]) || self.full() {
            return;
        }
        let mut s = String::new();
        self.set_stroke(&mut s, color, width, &dash_values(dash), JOIN_MITER);
        let _ = write!(s, "{} {} m {} {} l S", n(start.x), n(self.y(start.y)), n(end.x), n(self.y(end.y)));
        self.op(&s);
    }

    /// An open polyline (round joins).
    pub fn polyline(&mut self, pts: &[[f32; 2]], width: f32, color: Rgb, dash: &[f32]) {
        if pts.len() < 2 || !pts.iter().all(|p| finite(p)) || self.full() {
            return;
        }
        let mut s = String::new();
        self.set_stroke(&mut s, color, width, &dash_values(dash), JOIN_ROUND);
        self.path(&mut s, pts, false);
        s.push_str(" S");
        self.op(&s);
    }

    fn path(&self, s: &mut String, pts: &[[f32; 2]], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(s, "{} {} {} ", n(p[0]), n(self.y(p[1])), if i == 0 { "m" } else { "l" });
        }
        if close {
            s.push('h');
        }
    }

    /// A closed polygon, filled and/or stroked (round joins).
    pub fn polygon(&mut self, pts: &[[f32; 2]], fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if pts.len() < 2 || (fill.is_none() && stroke.is_none()) || !pts.iter().all(|p| finite(p)) || self.full() {
            return;
        }
        let mut s = String::new();
        if let Some(f) = fill {
            self.set_fill(&mut s, f);
        }
        if let Some((c, w)) = stroke {
            self.set_stroke(&mut s, c, w, "", JOIN_ROUND);
        }
        self.path(&mut s, pts, true);
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }

    /// A circle approximated by four cubic Béziers.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if !extent(r) || !finite(&[cx, cy]) || (fill.is_none() && stroke.is_none()) || self.full() {
            return;
        }
        const K: f32 = 0.552_284_8;
        let k = r * K;
        let y = self.y(cy);
        let mut s = String::new();
        if let Some(f) = fill {
            self.set_fill(&mut s, f);
        }
        if let Some((c, w)) = stroke {
            self.set_stroke(&mut s, c, w, "", JOIN_MITER);
        }
        let _ = write!(
            s,
            "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h",
            n(cx + r),
            n(y),
            n(cx + r),
            n(y + k),
            n(cx + k),
            n(y + r),
            n(cx),
            n(y + r),
            n(cx - k),
            n(y + r),
            n(cx - r),
            n(y + k),
            n(cx - r),
            n(y),
            n(cx - r),
            n(y - k),
            n(cx - k),
            n(y - r),
            n(cx),
            n(y - r),
            n(cx + k),
            n(y - r),
            n(cx + r),
            n(y - k),
            n(cx + r),
            n(y)
        );
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }

    /// Arabic ink extents above and below the baseline, in points. Standard font text returns
    /// `None`. Font errors are recorded for [`PdfDoc::finish`], just as when drawing text.
    pub fn text_vertical_extents(&mut self, font: Font, size: f32, text: &str) -> Option<(f32, f32)> {
        self.text_vertical_extents_with_direction(font, size, text, TextDirection::Auto)
    }

    /// Arabic ink extents with an explicit paragraph base direction.
    pub fn text_vertical_extents_with_direction(&mut self, font: Font, size: f32, text: &str, direction: TextDirection) -> Option<(f32, f32)> {
        if !arabic::shaped(font, text) {
            return None;
        }
        match arabic::vertical_extents(font, text, direction) {
            Ok((ascent, descent)) => Some((ascent * size / 1000.0, descent * size / 1000.0)),
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }

    /// Text with its baseline starting at `(x, y)`.
    pub fn text(&mut self, x: f32, y: f32, size: f32, font: Font, color: Rgb, text: &str) {
        self.text_with_direction(Point::new(x, y), size, font, color, text, TextDirection::Auto);
    }

    /// Text drawn with an explicit paragraph base direction.
    pub fn text_with_direction(&mut self, origin: Point, size: f32, font: Font, color: Rgb, text: &str, direction: TextDirection) {
        self.text_rotated_with_direction(Baseline { origin, angle: 0.0 }, size, font, color, text, direction);
    }

    /// Text rotated by `angle` radians (clockwise on the page, like the y-down coordinate
    /// system) about its baseline origin.
    pub fn text_rotated(&mut self, origin: Point, size: f32, font: Font, color: Rgb, text: &str, angle: f32) {
        self.text_rotated_with_direction(Baseline { origin, angle }, size, font, color, text, TextDirection::Auto);
    }

    /// Rotated text with an explicit paragraph base direction.
    pub fn text_rotated_with_direction(&mut self, baseline: Baseline, size: f32, font: Font, color: Rgb, text: &str, direction: TextDirection) {
        let Baseline { origin, angle } = baseline;
        if text.is_empty() || !extent(size) || !finite(&[origin.x, origin.y]) {
            return;
        }
        let direction = if direction == TextDirection::Context { direction.resolve(text) } else { direction };
        let (sn, cs) = if angle.is_finite() { angle.sin_cos() } else { (0.0, 1.0) };
        // Clockwise in y-down = counter-clockwise negated in PDF's y-up space.
        let matrix = format!("{} {} {} {} {} {}", n(cs), n(-sn), n(sn), n(cs), n(origin.x), n(self.y(origin.y)));
        let size = size.min(10_000.0);
        let shaping = arabic::shaping(font, text);
        self.missing_arabic_font |= shaping == arabic::Shaping::NoFont;
        if shaping == arabic::Shaping::Shaped {
            self.shaped_text(&matrix, size, font, color, text, direction);
        } else {
            self.standard_text(&matrix, size, font, color, text, direction);
        }
    }

    fn shaped_text(&mut self, matrix: &str, size: f32, font: Font, color: Rgb, text: &str, direction: TextDirection) {
        if self.saved.len() >= MAX_SAVE_DEPTH {
            return;
        }
        let body = match self.arabic.text(font, text, size, direction) {
            Ok(body) => body,
            Err(error) => {
                self.error.get_or_insert(error);
                return;
            }
        };
        for slot in [Some(font.index()), self.arabic.uses_cid.then_some(CID_FONT)].into_iter().flatten() {
            if let Some(f) = self.fonts.get_mut(slot) {
                *f = true;
            }
        }
        let mut s = format!("q\n{matrix} cm\n");
        push_rgb(&mut s, color, false);
        s.push(' ');
        push_rgb(&mut s, color, true);
        let _ = write!(s, "\n{body}\nQ");
        self.text_op(&s);
    }

    fn standard_text(&mut self, matrix: &str, size: f32, font: Font, color: Rgb, text: &str, direction: TextDirection) {
        let visual = match arabic::latin_visual(text, direction) {
            Ok(visual) => visual,
            Err(error) => {
                self.error.get_or_insert(error);
                return;
            }
        };
        let fi = font.index();
        let size = n(size);
        let mut runs = String::new();
        // Runs of WinAnsi text use `font`; runs it cannot encode use the CID font. Each `Tj`
        // advances the text position, so the runs follow each other on the baseline.
        // Symbol text is one run in Symbol's own encoding (the CID font has no maths symbols).
        let symbol = font == Font::Symbol;
        let uses_cid = |c: char| !symbol && needs_cid(c);
        let mut rest: &str = &visual;
        while let Some(first) = rest.chars().next() {
            let cid = uses_cid(first);
            let end = rest.char_indices().find(|(_, c)| uses_cid(*c) != cid).map_or(rest.len(), |(i, _)| i);
            let (run, tail) = rest.split_at(end);
            let (slot, string) = if cid { (CID_FONT, pdf_utf16_string(run)) } else { (fi, pdf_string_in(run, symbol)) };
            if let Some(f) = self.fonts.get_mut(slot) {
                *f = true;
            }
            let first_run = runs.is_empty();
            let _ = writeln!(runs, "/F{} {} Tf", slot + 1, size);
            if first_run {
                let _ = writeln!(runs, "{matrix} Tm");
            }
            let _ = writeln!(runs, "{string} Tj");
            rest = tail;
        }
        let mut s = String::new();
        self.set_fill(&mut s, color);
        let _ = write!(s, "BT\n{runs}ET");
        let s = if visual != text { format!("/Span << /ActualText {} >> BDC\n{s}\nEMC", arabic::unicode_string(text, true)) } else { s };
        self.text_op(&s);
    }

    fn text_op(&mut self, s: &str) {
        if self.content.len().saturating_add(s.len()).saturating_add(1) > MAX_PAGE_CONTENT {
            self.error.get_or_insert_with(|| Error("PDF page text exceeds the size limit".into()));
        } else {
            self.op(s);
        }
    }

    /// Draws an RGBA image (`pw`×`ph` pixels, row-major, straight alpha) into the box
    /// `(x, y, w, h)`. Malformed or oversized buffers are ignored.
    pub fn image_rgba(&mut self, destination: Rect, rgba: &[u8], pixel_size: [u32; 2]) {
        let Rect { x, y, width: w, height: h } = destination;
        let [pw, ph] = pixel_size;
        let px = pw as u64 * ph as u64;
        if pw == 0 || ph == 0 || px > MAX_IMAGE_PIXELS || (rgba.len() as u64) < px * 4 || !(extent(w) && extent(h) && finite(&[x, y])) {
            return;
        }
        // The image is drawn inside its own q/Q, which counts towards the nesting limit.
        if self.saved.len() >= MAX_SAVE_DEPTH {
            return;
        }
        let px = px as usize;
        let mut rgb = Vec::with_capacity(px * 3);
        // The alpha plane is only allocated once a non-opaque pixel shows up.
        let mut alpha: Option<Vec<u8>> = None;
        for (i, p) in rgba.as_chunks::<4>().0.iter().take(px).enumerate() {
            rgb.extend_from_slice(&[p[0], p[1], p[2]]);
            match &mut alpha {
                Some(a) => a.push(p[3]),
                None if p[3] != 255 => {
                    let mut a = Vec::with_capacity(px);
                    a.resize(i, 255);
                    a.push(p[3]);
                    alpha = Some(a);
                }
                None => {}
            }
        }
        let img = ImageData::new(pw, ph, rgb, alpha);
        // The same picture drawn again on this page shares one XObject.
        let candidates = self.image_index.entry(img.hash).or_default();
        #[cfg(test)]
        let comparisons = &mut self.image_comparisons;
        let known = candidates.iter().copied().find(|&i| {
            #[cfg(test)]
            {
                *comparisons += 1;
            }
            self.images.get(i).is_some_and(|o| o.same(&img))
        });
        let idx = match known {
            Some(i) => i,
            None => {
                candidates.push(self.images.len());
                self.images.push(img);
                self.images.len() - 1
            }
        };
        let s = format!("q {} 0 0 {} {} {} cm /Im{} Do Q", n(w), n(h), n(x), n(self.y(y + h)), idx + 1);
        self.op(&s);
    }
}

/// Document metadata.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub creator: String,
}

/// A PDF document under construction.
#[derive(Default)]
pub struct PdfDoc {
    pages: Vec<Page>,
    pub info: Info,
}

/// PackBits-style run-length encoding (PDF `RunLengthDecode`).
fn run_length(data: &[u8]) -> Vec<u8> {
    craft_stream_codecs::runlength::encode(data)
}

/// Longest Info string written, in characters; longer values are cut on a character boundary.
const MAX_INFO_CHARS: usize = 1024;

/// A PDF text string (document Info entries) for `text`: a literal `( … )` for printable ASCII,
/// otherwise UTF-16BE with a byte-order mark as a hex string (ISO 32000-1 7.9.2.2).
fn pdf_text_string(text: &str) -> String {
    let text: String = text.chars().take(MAX_INFO_CHARS).collect();
    if text.chars().all(|c| matches!(c, ' '..='~')) {
        return pdf_string(&text);
    }
    let mut s = String::with_capacity(text.len() * 4 + 6);
    s.push_str("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(s, "{unit:04X}");
    }
    s.push('>');
    s
}

impl PdfDoc {
    pub fn new() -> PdfDoc {
        PdfDoc::default()
    }

    /// Adds a page of `width_pt` × `height_pt` points and returns it for drawing.
    pub fn add_page(&mut self, width_pt: f32, height_pt: f32) -> &mut Page {
        let width = if width_pt.is_finite() { width_pt.clamp(3.0, 14_400.0) } else { 612.0 };
        let height = if height_pt.is_finite() { height_pt.clamp(3.0, 14_400.0) } else { 792.0 };
        self.pages.push(Page {
            width,
            height,
            content: String::new(),
            fonts: [false; FONT_SLOTS],
            images: Vec::new(),
            image_index: HashMap::new(),
            #[cfg(test)]
            image_comparisons: 0,
            links: Vec::new(),
            style: Style::default(),
            saved: Vec::new(),
            refused: 0,
            arabic: arabic::Subset::default(),
            error: None,
            missing_arabic_font: false,
        });
        let i = self.pages.len() - 1;
        &mut self.pages[i]
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The page at `i`, to keep drawing on it.
    pub fn page_mut(&mut self, i: usize) -> Option<&mut Page> {
        self.pages.get_mut(i)
    }

    /// Whether some Arabic text was set in the standard and CID fonts, which have no Arabic glyphs,
    /// because no Arabic face was found for its weight.
    pub fn missing_arabic_font(&self) -> bool {
        self.pages.iter().any(|page| page.missing_arabic_font)
    }

    /// Serializes the document. A document without pages gets one blank Letter page.
    ///
    /// Every object is written straight into the output buffer (nothing is staged in an
    /// intermediate list of bodies); only the xref table needs the offset of each object id, and
    /// the ids are contiguous from 1. Objects are therefore in the file in the order they are
    /// numbered during writing (fonts, then per page its new images, content stream and page
    /// dictionary), with the catalog, page tree and info dictionary last.
    ///
    /// Returns an error if the Arabic font is unusable, text exceeds a size limit, or the
    /// shared envelope writer refuses object counts, offsets or an allocation.
    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        if let Some(error) = self.pages.iter().find_map(|page| page.error.clone()) {
            return Err(error);
        }
        if self.pages.is_empty() {
            self.add_page(612.0, 792.0);
        }
        for page in &mut self.pages {
            page.refused = 0;
            while !page.saved.is_empty() {
                page.restore();
            }
        }
        // Object numbering: 1 catalog, 2 page tree, 3 info, then fonts, then per page.
        let mut used = [false; FONT_SLOTS];
        for p in &self.pages {
            for (u, f) in used.iter_mut().zip(p.fonts.iter()) {
                *u |= *f;
            }
        }
        let mut next = 4u32;
        let mut font_ids = [0u32; FONT_SLOTS];
        for (i, u) in used.iter().enumerate() {
            if *u {
                font_ids[i] = next;
                // The CID font also needs its descendant font and font descriptor.
                next += if i == CID_FONT { 3 } else { 1 };
            }
        }
        // Page contents are moved out so each one is freed as soon as it is in the output.
        let contents: Vec<String> = self.pages.iter_mut().map(|p| std::mem::take(&mut p.content)).collect();
        let page_links: Vec<Vec<LinkAnnotation>> = self.pages.iter_mut().map(|p| std::mem::take(&mut p.links)).collect();
        let reserve = contents.iter().fold(4096usize, |acc, c| acc.saturating_add(c.len()).saturating_add(1024));
        let mut out = craft_pdf::classic::Writer::with_capacity(reserve)?;
        let mut kids = Vec::new();
        let mut annotations = Vec::new();
        let page_count = self.pages.len();
        let mut font_dict = String::new();
        for (i, id) in font_ids.iter().enumerate() {
            if *id != 0 {
                let _ = write!(font_dict, "/F{} {} 0 R ", i + 1, id);
                if i == CID_FONT {
                    let (cid_id, desc_id) = (id + 1, id + 2);
                    out.object(
                        *id,
                        &format!(
                            "<< /Type /Font /Subtype /Type0 /BaseFont /{CID_BASE_FONT} /Encoding /UniGB-UTF16-H /DescendantFonts [{cid_id} 0 R] >>"
                        ),
                    )?;
                    out.object(cid_id,
                        &format!(
                            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{CID_BASE_FONT} /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /FontDescriptor {desc_id} 0 R /DW {CID_WIDTH} >>"
                        ),
                    )?;
                    out.object(desc_id,
                        &format!(
                            "<< /Type /FontDescriptor /FontName /{CID_BASE_FONT} /Flags 6 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>"
                        ),
                    )?;
                    continue;
                }
                let name = Font::ALL.get(i).copied().unwrap_or_default().base_name();
                let enc = if name.starts_with("Symbol") { "" } else { " /Encoding /WinAnsiEncoding" };
                out.object(*id, &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name}{enc} >>"))?;
            }
        }
        // Images already written, by content hash: (page, image, object id). A hash match is
        // confirmed by comparing the pixels before an object is shared.
        let mut written: HashMap<u64, Vec<(usize, usize, u32)>> = HashMap::new();
        for (pi, ((page, content), links)) in self.pages.iter().zip(contents).zip(page_links).enumerate() {
            let arabic_fonts = page.arabic.write_objects(&mut next, &mut out)?;
            let page_fonts = format!("{font_dict}{arabic_fonts}");
            let mut xobjects = String::new();
            for (k, img) in page.images.iter().enumerate() {
                let known = written.get(&img.hash).and_then(|c| {
                    c.iter().find_map(|(p, i, id)| {
                        let other = self.pages.get(*p)?.images.get(*i)?;
                        img.same(other).then_some(*id)
                    })
                });
                let img_id = match known {
                    Some(id) => id,
                    None => {
                        let img_id = next;
                        next += 1;
                        let mut dict = format!("/Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8", img.width, img.height);
                        let smask_id = img.alpha.as_ref().map(|_| {
                            let id = next;
                            next += 1;
                            id
                        });
                        dict.push_str(" /ColorSpace /DeviceRGB");
                        if let Some(id) = smask_id {
                            let _ = write!(dict, " /SMask {id} 0 R");
                        }
                        dict.push_str(" /Filter /RunLengthDecode");
                        out.stream(img_id, &dict, &run_length(&img.rgb))?;
                        if let (Some(id), Some(a)) = (smask_id, &img.alpha) {
                            let dict = format!(
                                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /RunLengthDecode",
                                img.width, img.height
                            );
                            out.stream(id, &dict, &run_length(a))?;
                        }
                        written.entry(img.hash).or_default().push((pi, k, img_id));
                        img_id
                    }
                };
                let _ = write!(xobjects, "/Im{} {} 0 R ", k + 1, img_id);
            }
            let content_id = next;
            let page_id = next + 1;
            next += 2;
            out.stream(content_id, "", content.as_bytes())?;
            drop(content);
            let xo = if xobjects.is_empty() { String::new() } else { format!(" /XObject << {xobjects}>>") };
            let mut link_refs = String::new();
            for link in links {
                if matches!(&link.target, LinkTarget::Page { index, .. } if *index >= page_count) {
                    continue;
                }
                let id = next;
                next += 1;
                let _ = write!(link_refs, "{id} 0 R ");
                annotations.push((id, link));
            }
            let annots = if link_refs.is_empty() { String::new() } else { format!(" /Annots [{link_refs}]") };
            out.object(
                page_id,
                &format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << {}>>{} >> /Contents {} 0 R{} >>",
                    n(page.width),
                    n(page.height),
                    page_fonts,
                    xo,
                    content_id,
                    annots
                ),
            )?;
            kids.push(page_id);
        }
        for (id, link) in annotations {
            let [x0, y0, x1, y1] = link.rect;
            let action = match link.target {
                LinkTarget::Uri(uri) => format!("/A << /S /URI /URI {} >>", pdf_uri_string(&uri)),
                LinkTarget::Page { index, x, y } => {
                    let Some(&page_id) = kids.get(index) else { continue };
                    let height = self.pages.get(index).map_or(0.0, |p| p.height);
                    format!("/Dest [{page_id} 0 R /XYZ {} {} null]", n(x), n(height - y))
                }
            };
            out.object(id, &format!("<< /Type /Annot /Subtype /Link /Rect [{} {} {} {}] /Border [0 0 0] {action} >>", n(x0), n(y0), n(x1), n(y1)))?;
        }
        out.object(1, "<< /Type /Catalog /Pages 2 0 R >>")?;
        let kids_s: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
        out.object(2, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids_s.join(" "), kids.len()))?;
        let creator = if self.info.creator.is_empty() { "GridCraft" } else { &self.info.creator };
        let mut info = format!("<< /Producer {} /Creator {}", pdf_text_string("GridCraft PDF"), pdf_text_string(creator));
        if !self.info.title.is_empty() {
            let _ = write!(info, " /Title {}", pdf_text_string(&self.info.title));
        }
        if !self.info.author.is_empty() {
            let _ = write!(info, " /Author {}", pdf_text_string(&self.info.author));
        }
        info.push_str(" >>");
        out.object(3, &info)?;

        out.finish(1, Some(3)).map_err(Error::from)
    }
}

#[cfg(test)]
mod tests;
