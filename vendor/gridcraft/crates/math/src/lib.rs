//! GridCraft equations. LaTeX source is laid out by RaTeX (`ratex-parser`, `ratex-layout`; MIT)
//! into a [`MathList`]: plain glyph, rule and path data that the UI and the PDF writer paint
//! without calling RaTeX again.
//!
//! RaTeX is young (0.1.x), so it only ever runs inside this crate's entry points ([`layout`],
//! [`to_omml`], [`check`]). Each one caps its input (4 KB, nesting 32, no macro definitions, at most [`MAX_CELLS`] matrix cells) and runs under
//! `catch_unwind`, so a stray panic becomes a [`MathError`]. See the RaTeX gate in
//! `docs/decisions.md` and `tests/gate.rs`.
//!
//! Units are em: one em is the equation's font size. Coordinates run right and down from the
//! top-left of the list's box; the baseline is at `y = height`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod omml;

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ratex_font::FontId;
use ratex_types::color::Color;
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::path_command::PathCommand;

pub use omml::to_omml;

/// Longest source accepted, in bytes.
pub const MAX_SOURCE_BYTES: usize = 4096;
/// Deepest nesting of `{…}`, `\left…\right` and `\begin…\end` accepted.
pub const MAX_DEPTH: usize = 32;
/// Largest coordinate or size accepted in a layout, in em. `\rule{1e308em}{1em}` lays out; this
/// stops it from reaching a painter.
pub const MAX_EM: f32 = 10_000.0;
/// Most items one equation may produce.
pub const MAX_ITEMS: usize = 50_000;
/// Most matrix cells one equation may have, counting the empty cells that pad a short row to the
/// width of the widest one. A ragged matrix costs rows times the widest row, not what is typed.
pub const MAX_CELLS: usize = 2_500;

/// The error for a source over [`MAX_SOURCE_BYTES`]. It is one shared value so that a cache can
/// answer for such a source by its length alone, without reading it.
pub static SOURCE_TOO_LONG: std::sync::LazyLock<MathError> =
    std::sync::LazyLock::new(|| MathError::new(format!("An equation can be at most {MAX_SOURCE_BYTES} bytes long."), None));

/// Why an equation could not be laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError {
    /// The offending source, as a byte range on char boundaries, when known.
    pub span: Option<Range<usize>>,
    pub message: String,
}

impl MathError {
    fn new(message: impl Into<String>, span: Option<Range<usize>>) -> MathError {
        MathError { span, message: message.into() }
    }

    /// The 1-based character position of the error in `source`, when known.
    pub fn char_pos(&self, source: &str) -> Option<usize> {
        let start = self.span.as_ref()?.start;
        Some(source.char_indices().take_while(|(i, _)| *i < start).count() + 1)
    }
}

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MathError {}

/// A laid-out equation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MathList {
    pub items: Vec<Item>,
    /// Advance width, in em.
    pub width: f32,
    /// Height above the baseline, in em.
    pub height: f32,
    /// Depth below the baseline, in em.
    pub depth: f32,
}

impl MathList {
    /// Total height (height plus depth), in em.
    pub fn total_height(&self) -> f32 {
        self.height + self.depth
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Glyph(Glyph),
    Rule(Rule),
    Path(Path),
}

/// The typeface a glyph asks for. Painters map it onto the fonts they have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Face {
    #[default]
    Roman,
    Italic,
    Bold,
    BoldItalic,
    Sans,
    SansBold,
    SansItalic,
    Mono,
    /// A large operator or delimiter (TeX's Size1 to Size4 fonts): drawn stretched to the glyph's
    /// box rather than at the run's font size.
    Large(u8),
}

impl Face {
    pub fn is_italic(self) -> bool {
        matches!(self, Face::Italic | Face::BoldItalic | Face::SansItalic)
    }
    pub fn is_bold(self) -> bool {
        matches!(self, Face::Bold | Face::BoldItalic | Face::SansBold)
    }
}

/// One character.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// Left edge of the glyph's box.
    pub x: f32,
    /// The glyph's baseline.
    pub y: f32,
    /// Font size, in em of the equation (1 for the main line, smaller in scripts).
    pub size: f32,
    /// The character to draw.
    pub ch: char,
    /// A plainer character to draw when no font has `ch` (`R` for `ℝ`), before giving up.
    pub alt: Option<char>,
    pub face: Face,
    /// The glyph's box (TeX metrics), already scaled by `size`: advance, height above and depth
    /// below the baseline. A painter without the glyph draws a `[?]` box this size.
    pub width: f32,
    pub height: f32,
    pub depth: f32,
    /// An explicit colour (`\color`); `None` is the equation's own ("Automatic") colour.
    pub color: Option<[u8; 3]>,
}

/// A filled rectangle (fraction bars, radical vinculums, table rules, box borders).
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub x: f32,
    /// Top edge.
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Option<[u8; 3]>,
}

/// A vector outline (stretchy braces, arrows, radicals), relative to `(x, y)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub x: f32,
    pub y: f32,
    pub commands: Vec<PathCmd>,
    pub fill: bool,
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Lays `latex` out. Errors carry the offending span when RaTeX reports one.
pub fn layout(latex: &str) -> Result<MathList, MathError> {
    check_caps(latex)?;
    guarded(|| {
        let nodes = parse(latex)?;
        let lb = ratex_layout::layout(&nodes, &ratex_layout::LayoutOptions::default());
        convert(&ratex_layout::to_display_list(&lb))
    })
}

/// Whether `latex` parses, without laying it out (cheap enough to run on every keystroke).
pub fn check(latex: &str) -> Result<(), MathError> {
    check_caps(latex)?;
    guarded(|| parse(latex).map(|_| ()))
}

/// The equation gallery (Insert › Equation): GridCraft's own examples, as (name, LaTeX).
pub const GALLERY: &[(&str, &str)] = &[
    ("Quadratic formula", "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"),
    ("Pythagoras", "a^2 + b^2 = c^2"),
    ("Binomial theorem", "(x + a)^n = \\sum_{k=0}^{n} \\binom{n}{k} x^k a^{n-k}"),
    ("Area of a circle", "A = \\pi r^2"),
    ("Euler's identity", "e^{i\\pi} + 1 = 0"),
    ("2\u{00D7}2 matrix", "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}"),
];

/// Runs `f` so that a panic inside it becomes an error.
fn guarded<T>(f: impl FnOnce() -> Result<T, MathError>) -> Result<T, MathError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(MathError::new("This equation could not be laid out.", None)))
}

#[cfg(test)]
thread_local! {
    /// Test hook: makes the next guarded parse panic, to prove the guard holds.
    static INJECT_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn parse(latex: &str) -> Result<Vec<ratex_parser::ParseNode>, MathError> {
    #[cfg(test)]
    if INJECT_PANIC.with(|p| p.replace(false)) {
        panic!("injected");
    }
    ratex_parser::parse(latex).map_err(|e| {
        let span = e.loc.and_then(|l| {
            let (s, t) = (l.start.min(latex.len()), l.end.min(latex.len()));
            (latex.is_char_boundary(s) && latex.is_char_boundary(t)).then_some(s..t.max(s))
        });
        MathError::new(e.message, span)
    })
}

/// Control words that define or rebind macros. User macros have no use in an equation, and
/// RaTeX caps only how many expansions run, not how large they grow, so a self-referencing
/// definition whose argument doubles on every call keeps the expander busy without bound. They are
/// refused before RaTeX sees the source.
const MACRO_DEFINERS: &[&str] =
    &["def", "gdef", "edef", "xdef", "let", "futurelet", "global", "long", "newcommand", "renewcommand", "providecommand"];

/// One open `\begin…\end` environment, counted from the source text.
struct Grid {
    /// Rows ended so far (by `\\`, `\cr` or `\newline`).
    rows: usize,
    /// Cells in the current row.
    cells: usize,
    /// Cells in the widest row so far.
    widest: usize,
}

impl Grid {
    fn new() -> Grid {
        Grid { rows: 0, cells: 1, widest: 0 }
    }
    fn end_row(&mut self) {
        self.widest = self.widest.max(self.cells);
        self.rows += 1;
        self.cells = 1;
    }
    /// The padded grid RaTeX lays out and `to_omml` writes: every row as wide as the widest.
    fn padded_cells(&self) -> usize {
        (self.rows + 1).saturating_mul(self.widest.max(self.cells))
    }
}

/// The size, nesting, macro and matrix caps, checked before RaTeX sees the source.
fn check_caps(latex: &str) -> Result<(), MathError> {
    if latex.len() > MAX_SOURCE_BYTES {
        return Err(SOURCE_TOO_LONG.clone());
    }
    // `%` starts a comment, except inside the address of `\href` and `\url`, where RaTeX reads it
    // as a character. Which one it is decides what the rest of the line means, so the two are not
    // allowed together.
    if latex.contains('%') && (latex.contains("\\href") || latex.contains("\\url")) {
        return Err(MathError::new("A % can't be combined with a link in an equation.", latex.find('%').map(|i| i..i + 1)));
    }
    let mut depth = 0usize;
    let mut grids: Vec<Grid> = Vec::new();
    let mut cells_total = 0usize;
    let mut chars = latex.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let open = match c {
            '\\' => {
                // A control word (letters and `@`, as RaTeX's lexer reads it), or an escaped
                // character (`\{` is not a group).
                let mut word = String::new();
                while let Some(&(_, n)) = chars.peek() {
                    if n.is_ascii_alphabetic() || n == '@' {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    if let Some((_, n)) = chars.next()
                        && n == '\\'
                        && let Some(g) = grids.last_mut()
                    {
                        g.end_row();
                    }
                    continue;
                }
                if MACRO_DEFINERS.contains(&word.as_str()) {
                    return Err(MathError::new("Macro definitions are not supported in equations.", Some(i..i + 1 + word.len())));
                }
                match word.as_str() {
                    "verb" => {
                        // Verbatim text runs to the next copy of its delimiter, whatever it
                        // holds: no group, comment, row or definition starts inside it.
                        if chars.peek().is_some_and(|&(_, n)| n == '*') {
                            chars.next();
                        }
                        if let Some((_, delim)) = chars.next() {
                            for (_, n) in chars.by_ref() {
                                if n == delim {
                                    break;
                                }
                            }
                        }
                        None
                    }
                    // RaTeX reads `\newline` as `\\`, so it ends a row like `\cr` does.
                    "cr" | "newline" => {
                        if let Some(g) = grids.last_mut() {
                            g.end_row();
                        }
                        None
                    }
                    "begin" => {
                        if let Some(n) = alignat_columns(latex.get(i + 6..).unwrap_or_default())
                            && n > MAX_ALIGNAT_PAIRS
                        {
                            return Err(too_many_cells(i..i + 6));
                        }
                        grids.push(Grid::new());
                        Some(true)
                    }
                    "end" => {
                        if let Some(g) = grids.pop() {
                            cells_total = cells_total.saturating_add(g.padded_cells());
                            if cells_total > MAX_CELLS {
                                return Err(too_many_cells(i..i + 1 + word.len()));
                            }
                        }
                        Some(false)
                    }
                    "left" => Some(true),
                    "right" => Some(false),
                    _ => None,
                }
            }
            '&' | '@' => {
                // `@` makes an arrow cell and an empty cell beside it in a `CD` diagram.
                if let Some(g) = grids.last_mut() {
                    g.cells += if c == '@' { 2 } else { 1 };
                }
                None
            }
            '%' => {
                // A comment runs to the end of the line.
                for (_, n) in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
                None
            }
            '{' => Some(true),
            '}' => Some(false),
            _ => None,
        };
        match open {
            Some(true) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(MathError::new(format!("Equations can nest at most {MAX_DEPTH} levels deep."), Some(i..i + c.len_utf8())));
                }
            }
            Some(false) => depth = depth.saturating_sub(1),
            None => {}
        }
    }
    // An environment left open is a syntax error for the parser to report, but it still counts.
    for g in grids {
        cells_total = cells_total.saturating_add(g.padded_cells());
    }
    if cells_total > MAX_CELLS {
        return Err(too_many_cells(latex.len()..latex.len()));
    }
    Ok(())
}

/// Most column pairs `\begin{alignedat}{N}` may ask for. RaTeX allocates 2N column specs before
/// it lays anything out.
const MAX_ALIGNAT_PAIRS: usize = 50;

/// N of `{alignat}{N}`, `{alignat*}{N}` or `{alignedat}{N}` at the start of `rest` (just after
/// `\begin`). `usize::MAX` when the count is present but is not a plain digit string RaTeX reads
/// the same way (too long, or hidden by a comment, inner spaces or a macro): refused, not skipped.
/// An environment name that is computed (contains `\` or `{`) is refused the same way.
///
/// RaTeX reads the environment name and the count through its lexer, which drops comments and
/// spaces, so they are dropped here too before the two groups are compared.
fn alignat_columns(rest: &str) -> Option<usize> {
    // Enough for the longest name and any count that fits `usize`; a longer count is refused.
    const WINDOW: usize = 64;
    let mut kept = String::new();
    let mut chars = rest.chars();
    while kept.len() < WINDOW {
        match chars.next() {
            None => break,
            Some('%') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            Some(c) if c.is_whitespace() => {}
            Some(c) => kept.push(c),
        }
    }
    let group = kept.strip_prefix('{')?;
    // A name that does not close inside the window cannot be read, so it is refused.
    let Some((name, after)) = group.split_once('}') else {
        return Some(usize::MAX);
    };
    // A computed name (`align\@char{97}t` reads as `alignat`) cannot be resolved here: refused
    // too. Every real environment name is letters, optionally with a trailing `*`.
    if name.contains(['\\', '{']) {
        return Some(usize::MAX);
    }
    if !matches!(name, "alignat" | "alignat*" | "alignedat") {
        return None;
    }
    let Some(digits) = after.strip_prefix('{').and_then(|a| a.split_once('}')).map(|(d, _)| d) else {
        return Some(usize::MAX);
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Some(usize::MAX);
    }
    Some(digits.parse().unwrap_or(usize::MAX))
}

fn too_many_cells(span: Range<usize>) -> MathError {
    MathError::new(format!("An equation can have at most {MAX_CELLS} matrix cells (rows times the widest row)."), Some(span))
}

fn color(c: &Color) -> Option<[u8; 3]> {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgb = [q(c.r), q(c.g), q(c.b)];
    (rgb != [0, 0, 0]).then_some(rgb)
}

/// A finite value within the caps, as `f32`.
fn em(v: f64) -> Result<f32, MathError> {
    if v.is_finite() && v.abs() <= f64::from(MAX_EM) { Ok(v as f32) } else { Err(MathError::new("This equation is too large to draw.", None)) }
}

fn convert(dl: &DisplayList) -> Result<MathList, MathError> {
    if dl.items.len() > MAX_ITEMS {
        return Err(MathError::new("This equation is too large to draw.", None));
    }
    let mut items = Vec::with_capacity(dl.items.len());
    for it in &dl.items {
        items.push(match it {
            DisplayItem::GlyphPath { x, y, scale, font, char_code, color: c } => {
                Item::Glyph(glyph(em(*x)?, em(*y)?, em(*scale)?, font, *char_code, color(c))?)
            }
            DisplayItem::Line { x, y, width, thickness, color: c, .. } => {
                let t = em(*thickness)?;
                Item::Rule(Rule { x: em(*x)?, y: em(*y)? - t / 2.0, width: em(*width)?, height: t, color: color(c) })
            }
            DisplayItem::Rect { x, y, width, height, color: c } => {
                Item::Rule(Rule { x: em(*x)?, y: em(*y)?, width: em(*width)?, height: em(*height)?, color: color(c) })
            }
            DisplayItem::Path { x, y, commands, fill, color: c } => {
                let mut cmds = Vec::with_capacity(commands.len());
                for pc in commands {
                    cmds.push(match *pc {
                        PathCommand::MoveTo { x, y } => PathCmd::MoveTo(em(x)?, em(y)?),
                        PathCommand::LineTo { x, y } => PathCmd::LineTo(em(x)?, em(y)?),
                        PathCommand::QuadTo { x1, y1, x, y } => PathCmd::QuadTo(em(x1)?, em(y1)?, em(x)?, em(y)?),
                        PathCommand::CubicTo { x1, y1, x2, y2, x, y } => PathCmd::CubicTo(em(x1)?, em(y1)?, em(x2)?, em(y2)?, em(x)?, em(y)?),
                        PathCommand::Close => PathCmd::Close,
                    });
                }
                Item::Path(Path { x: em(*x)?, y: em(*y)?, commands: cmds, fill: *fill, color: color(c) })
            }
        });
    }
    Ok(MathList { items, width: em(dl.width)?.max(0.0), height: em(dl.height)?, depth: em(dl.depth)? })
}

fn glyph(x: f32, y: f32, size: f32, font: &str, code: u32, color: Option<[u8; 3]>) -> Result<Glyph, MathError> {
    let fid = FontId::parse(font).unwrap_or(FontId::MainRegular);
    let face = match fid {
        FontId::MainBold | FontId::FrakturBold => Face::Bold,
        FontId::MainItalic | FontId::MathItalic => Face::Italic,
        FontId::MainBoldItalic | FontId::MathBoldItalic => Face::BoldItalic,
        FontId::SansSerifRegular => Face::Sans,
        FontId::SansSerifBold => Face::SansBold,
        FontId::SansSerifItalic => Face::SansItalic,
        FontId::TypewriterRegular => Face::Mono,
        FontId::Size1Regular => Face::Large(1),
        FontId::Size2Regular => Face::Large(2),
        FontId::Size3Regular => Face::Large(3),
        FontId::Size4Regular => Face::Large(4),
        _ => Face::Roman,
    };
    let raw = char::from_u32(code).unwrap_or('\u{FFFD}');
    // Blackboard, script and Fraktur letters sit at ASCII slots in TeX's fonts; draw the Unicode
    // letter, with the ASCII one as the fallback.
    let (ch, alt) = match (fid, raw) {
        (FontId::AmsRegular, c @ ('A'..='Z' | 'k')) => (styled(c, Style::DoubleStruck), Some(c)),
        (FontId::CaligraphicRegular | FontId::ScriptRegular, c @ ('A'..='Z' | 'a'..='z')) => (styled(c, Style::Script), Some(c)),
        (FontId::FrakturRegular, c @ ('A'..='Z' | 'a'..='z')) => (styled(c, Style::Fraktur), Some(c)),
        (FontId::FrakturBold, c @ ('A'..='Z' | 'a'..='z')) => (styled(c, Style::BoldFraktur), Some(c)),
        (_, c) if (0x1D400..=0x1D7FF).contains(&code) => (c, Some(ratex_font::katex_ttf_glyph_char(fid, code))),
        (_, c) => (c, None),
    };
    let m = ratex_font::font_and_metric_for_mathematical_alphanumeric(code)
        .and_then(|(f, mc)| ratex_font::get_char_metrics(f, mc))
        .or_else(|| ratex_font::get_char_metrics(fid, code))
        .or_else(|| ratex_font::get_char_metrics_with_fallback(fid, code));
    let (w, h, d) = m.map_or((0.5, 0.7, 0.0), |m| (m.width + m.italic, m.height, m.depth));
    Ok(Glyph { x, y, size, ch, alt, face, width: em(w)? * size, height: em(h)? * size, depth: em(d)? * size, color })
}

#[derive(Clone, Copy)]
enum Style {
    DoubleStruck,
    Script,
    Fraktur,
    BoldFraktur,
}

/// The Mathematical Alphanumeric Symbols letter for `c` (Unicode 15, chapter 22.2), using the
/// Letterlike Symbols block where Unicode put a letter before the alphanumeric block existed.
fn styled(c: char, style: Style) -> char {
    let letterlike = match (style, c) {
        (Style::DoubleStruck, 'C') => Some('\u{2102}'),
        (Style::DoubleStruck, 'H') => Some('\u{210D}'),
        (Style::DoubleStruck, 'N') => Some('\u{2115}'),
        (Style::DoubleStruck, 'P') => Some('\u{2119}'),
        (Style::DoubleStruck, 'Q') => Some('\u{211A}'),
        (Style::DoubleStruck, 'R') => Some('\u{211D}'),
        (Style::DoubleStruck, 'Z') => Some('\u{2124}'),
        (Style::Script, 'B') => Some('\u{212C}'),
        (Style::Script, 'E') => Some('\u{2130}'),
        (Style::Script, 'F') => Some('\u{2131}'),
        (Style::Script, 'H') => Some('\u{210B}'),
        (Style::Script, 'I') => Some('\u{2110}'),
        (Style::Script, 'L') => Some('\u{2112}'),
        (Style::Script, 'M') => Some('\u{2133}'),
        (Style::Script, 'R') => Some('\u{211B}'),
        (Style::Script, 'e') => Some('\u{212F}'),
        (Style::Script, 'g') => Some('\u{210A}'),
        (Style::Script, 'o') => Some('\u{2134}'),
        (Style::Fraktur, 'C') => Some('\u{212D}'),
        (Style::Fraktur, 'H') => Some('\u{210C}'),
        (Style::Fraktur, 'I') => Some('\u{2111}'),
        (Style::Fraktur, 'R') => Some('\u{211C}'),
        (Style::Fraktur, 'Z') => Some('\u{2128}'),
        _ => None,
    };
    if let Some(l) = letterlike {
        return l;
    }
    let (upper, lower) = match style {
        Style::DoubleStruck => (0x1D538, 0x1D552),
        Style::Script => (0x1D49C, 0x1D4B6),
        Style::Fraktur => (0x1D504, 0x1D51E),
        Style::BoldFraktur => (0x1D56C, 0x1D586),
    };
    let code = match c {
        'A'..='Z' => upper + (c as u32 - 'A' as u32),
        'a'..='z' => lower + (c as u32 - 'a' as u32),
        _ => return c,
    };
    char::from_u32(code).unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(l: &MathList) -> String {
        l.items.iter().filter_map(|i| if let Item::Glyph(g) = i { Some(g.ch) } else { None }).collect()
    }

    #[test]
    fn boxes_of_a_simple_line() {
        let l = layout("x+1").unwrap();
        assert_eq!(glyphs(&l), "x+1");
        assert!(l.width > 1.5 && l.width < 3.0, "{}", l.width);
        assert!(l.height > 0.4 && l.height < 1.0);
        let Some(Item::Glyph(x)) = l.items.first() else { panic!("a glyph first") };
        assert_eq!(x.face, Face::Italic, "a letter is a math italic");
        assert!((x.y - l.height).abs() < 1e-4, "on the baseline");
        assert!(x.width > 0.3 && x.height > 0.3);
        let one = l.items.iter().find_map(|i| if let Item::Glyph(g) = i { (g.ch == '1').then_some(g) } else { None }).unwrap();
        assert_eq!(one.face, Face::Roman);
    }

    #[test]
    fn fractions_roots_and_scripts() {
        let f = layout("\\frac{a}{b}").unwrap();
        let rule = f.items.iter().find_map(|i| if let Item::Rule(r) = i { Some(r) } else { None }).expect("a fraction bar");
        assert!(rule.width > 0.3 && rule.height > 0.0 && rule.height < 0.1);
        let (a, b) = match &f.items.iter().filter_map(|i| if let Item::Glyph(g) = i { Some(g.clone()) } else { None }).collect::<Vec<_>>()[..] {
            [a, b] => (a.clone(), b.clone()),
            other => panic!("{other:?}"),
        };
        assert!(a.y < rule.y && b.y > rule.y + rule.height, "numerator above, denominator below");
        assert!(f.depth > 0.3);
        let s = layout("x^2").unwrap();
        let g: Vec<&Glyph> = s.items.iter().filter_map(|i| if let Item::Glyph(g) = i { Some(g) } else { None }).collect();
        assert!(g[1].size < 1.0 && g[1].y < g[0].y, "the exponent is smaller and raised");
        let r = layout("\\sqrt{x}").unwrap();
        assert!(glyphs(&r).contains('x'));
        assert!(r.items.len() >= 2);
        let big = layout("\\displaystyle\\sum_{i=1}^n i").unwrap();
        assert!(big.items.iter().any(|i| matches!(i, Item::Glyph(Glyph { ch: '∑', face: Face::Large(_), .. }))));
    }

    #[test]
    fn styled_letters_map_to_unicode_with_a_plain_fallback() {
        let l = layout("\\mathbb{R}\\mathbb{Q}\\mathcal{L}\\mathfrak{g}").unwrap();
        let g: Vec<(char, Option<char>)> = l.items.iter().filter_map(|i| if let Item::Glyph(g) = i { Some((g.ch, g.alt)) } else { None }).collect();
        assert_eq!(g, vec![('ℝ', Some('R')), ('ℚ', Some('Q')), ('ℒ', Some('L')), ('𝔤', Some('g'))]);
        assert_eq!(styled('A', Style::DoubleStruck), '𝔸');
        assert_eq!(styled('b', Style::Script), '𝒷');
        assert_eq!(styled('7', Style::Fraktur), '7');
    }

    #[test]
    fn colours_other_than_black_are_kept() {
        let l = layout("{\\color{red}x}y").unwrap();
        let c: Vec<Option<[u8; 3]>> = l.items.iter().filter_map(|i| if let Item::Glyph(g) = i { Some(g.color) } else { None }).collect();
        assert_eq!(c, vec![Some([255, 0, 0]), None]);
    }

    #[test]
    fn errors_point_at_the_source() {
        let e = layout("a + \\frac{1}{").unwrap_err();
        assert!(!e.message.is_empty());
        let e = layout("x + \\foo").unwrap_err();
        assert_eq!(e.span, Some(4..8), "{e:?}");
        assert_eq!(e.char_pos("x + \\foo"), Some(5));
        let e = layout("é + \\foo").unwrap_err();
        assert_eq!(e.char_pos("é + \\foo"), Some(5), "positions count characters, not bytes");
        assert!(check("\\frac{a}{b}").is_ok());
        assert!(check("\\frac{a}{").is_err());
    }

    #[test]
    fn hostile_input_is_refused_without_panicking() {
        let e = layout(&"x".repeat(MAX_SOURCE_BYTES + 1)).unwrap_err();
        assert!(e.message.contains("4096"));
        assert!(layout(&"x".repeat(MAX_SOURCE_BYTES)).is_ok());
        let deep = format!("{}x{}", "{".repeat(33), "}".repeat(33));
        assert_eq!(layout(&deep).unwrap_err().span, Some(32..33));
        assert!(layout(&format!("{}x{}", "\\left(".repeat(40), "\\right)".repeat(40))).is_err());
        // Escaped braces and comments are not groups.
        assert!(check_caps(&"\\{".repeat(100)).is_ok());
        assert!(check_caps(&format!("x % {}\n", "{".repeat(100))).is_ok());
        assert!(layout("\\rule{99999999em}{1em}").unwrap_err().message.contains("too large"));
        assert!(layout("\\kern99999999em x").is_err());
        assert!(layout("\\def\\a{\\a\\a}\\a").is_err());
        for s in ["", " ", "\\", "{", "}", "^", "_", "&", "\\\\", "\u{0}", "\u{FFFF}", "\u{1F600}", "$", "#", "\\begin{matrix}", "\\end{matrix}"] {
            let _ = layout(s);
            let _ = check(s);
            let _ = to_omml(s, 11.0, None);
        }
    }

    /// Runs `f` on its own thread and fails if it has not returned within `secs` seconds.
    fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let _ = tx.send(f());
            })
            .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
            Ok(v) => v,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("did not return within {secs} s"),
            Err(e) => panic!("the worker thread died: {e}"),
        }
    }

    #[test]
    fn macro_definitions_are_refused_before_the_expander_runs() {
        let defs = [
            "\\def\\a#1{\\a{#1#1}}\\a{x}",
            "\\gdef\\a#1{\\a{#1#1#1#1}}\\a{x}",
            "\\edef\\a#1{\\a{#1#1}}\\a{x}",
            "\\xdef\\a#1{{#1}{#1}}\\a{x}",
            "\\newcommand{\\a}[1]{\\a{#1#1}}\\a{x}",
            "\\renewcommand{\\a}[1]{\\a{#1#1}}\\a{x}",
            "\\providecommand{\\a}[1]{\\a{#1#1}}\\a{x}",
            "\\let\\a\\frac \\a{1}{2}",
            "\\futurelet\\a\\frac \\a{1}{2}",
            "\\global\\def\\a#1{\\a{#1#1}}\\a{x}",
            "\\long\\def\\a#1{\\a{#1#1}}\\a{x}",
            "{\\def\\a#1{\\a{#1#1}}}\\a{x}",
        ];
        for src in defs {
            // `check_caps` alone, so this cannot hang even if the refusal regresses.
            let e = check_caps(src).unwrap_err();
            assert!(e.message.contains("Macro definitions"), "{src}: {e}");
            assert_eq!(e.span.as_ref().map(|r| &src[r.clone()]).map(|w| w.starts_with('\\')), Some(true));
            let s = src.to_string();
            let (c, l, o) = within(10, move || (check(&s).is_err(), layout(&s).is_err(), to_omml(&s, 11.0, None).is_err()));
            assert!(c && l && o, "{src}");
        }
        // Names that merely start with a definer, escaped backslashes and comments are not macros.
        for ok in ["\\default", "\\letter", "\\\\def", "x % \\def\\a\n+1", "\\longrightarrow"] {
            assert!(check_caps(ok).is_ok(), "{ok}");
        }
        assert!(layout("\\longrightarrow x").is_ok());
        // Text the lexer reads differently from a plain scan must not hide a definition: a
        // verbatim `%` is no comment, a `%` in a link address is no comment, `@` joins a word.
        for src in [
            "\\verb|%|\\def\\a#1{\\a{#1#1}}\\a{x}",
            "\\verb\\x\\\\def\\a#1{\\a{#1#1}}\\a{x}",
            "%\\verb|\n\\def\\a#1{\\a{#1#1}}\\a{x}",
            "\\href{a%b}{x}\\def\\a#1{\\a{#1#1}}\\a{x}",
            "\\url{a%}\\def\\a#1{\\a{#1#1}}\\a{x}",
        ] {
            assert!(check_caps(src).is_err(), "{src}");
        }
        assert!(check_caps("\\verb|\\def| x").is_ok(), "verbatim text is not a definition");
        assert!(check_caps("\\def@x").is_ok(), "`\\def@x` is one unknown word to the lexer");
    }

    #[test]
    fn ragged_matrices_are_capped_by_their_padded_size() {
        let ragged = format!("\\begin{{matrix}}{}{}\\end{{matrix}}", "a\\\\".repeat(650), "a&".repeat(999) + "a");
        assert!(ragged.len() < MAX_SOURCE_BYTES);
        let e = check_caps(&ragged).unwrap_err();
        assert!(e.message.contains("matrix cells"), "{e}");
        let r = ragged.clone();
        let (l, o) = within(10, move || (layout(&r).unwrap_err(), to_omml(&r, 11.0, None).unwrap_err()));
        assert!(l.message.contains("matrix cells") && o.message.contains("matrix cells"));
        // Without the environment words there is no grid to count; the parser rejects it.
        // Ordinary and wide-but-regular matrices still work, and so does a trailing `\\`.
        let wide = format!("\\begin{{pmatrix}}{}\\end{{pmatrix}}", vec!["a&".repeat(39) + "a"; 40].join("\\\\"));
        assert!(wide.len() < MAX_SOURCE_BYTES);
        assert!(layout(&wide).is_ok());
        assert!(to_omml(&wide, 11.0, None).is_ok());
        assert!(layout("\\begin{pmatrix} a & b \\\\ c & d \\\\ \\end{pmatrix}").is_ok());
        // Diagram arrows and `alignedat` column counts size a grid without any `&`.
        let cd = format!("\\begin{{CD}}{}{}\\end{{CD}}", "A\\\\".repeat(900), "@>>>".repeat(300));
        assert!(cd.len() < MAX_SOURCE_BYTES);
        assert!(check_caps(&cd).is_err_and(|e| e.message.contains("matrix cells")));
        let r = "\\begin{alignedat}{99999999}a\\end{alignedat}";
        assert!(check_caps(r).is_err_and(|e| e.message.contains("matrix cells")));
        assert!(check_caps("\\begin{alignedat}{2}a&b&c&d\\end{alignedat}").is_ok());
        // A comment, inner spaces or a macro in the count do not hide it from the cap.
        for hidden in [
            "\\begin{alignedat}%\n{99999999}a\\end{alignedat}",
            "\\begin %c\n {alignedat}{99999999}a\\end{alignedat}",
            "\\begin{alignedat}{9 9}a\\end{alignedat}",
            "\\begin{alignedat}{9%x\n9}a\\end{alignedat}",
            "\\begin{align%x\nedat}{99}a\\end{alignedat}",
            "\\begin{alignedat}{9\\relax9}a\\end{alignedat}",
        ] {
            assert!(check_caps(hidden).is_err_and(|e| e.message.contains("matrix cells")), "{hidden}");
        }
        // A computed environment name reads as `alignat` to the parser, so it is capped too.
        for name in ["align\\@char{97}t", "alignedat\\relax", "al\\@char{105}gnat*"] {
            let over = format!("\\begin{{{name}}}{{51}}a\\end{{alignat}}");
            assert!(check_caps(&over).is_err_and(|e| e.message.contains("matrix cells")), "{over}");
        }
        let long = format!("\\begin{{{}}}{{51}}a\\end{{alignat}}", "a".repeat(80));
        assert!(check_caps(&long).is_err_and(|e| e.message.contains("matrix cells")));
        assert!(check_caps("\\begin{alignat}{50}a\\end{alignat}").is_ok());
        assert!(check_caps("\\begin{alignedat} {2} % two pairs\na&b&c&d\\end{alignedat}").is_ok());
        assert!(layout("\\begin{CD}A @>f>> B \\\\ @VVV @VVV \\\\ C @>>> D\\end{CD}").is_ok());
        assert!(check_caps("\\operatorname{curl} 50\\%").is_ok());
        let two = format!("{0}{0}", "\\begin{matrix}a&b\\\\c&d\\end{matrix}");
        assert!(check_caps(&two).is_ok());
    }

    #[test]
    fn an_injected_panic_is_an_error_not_a_crash() {
        INJECT_PANIC.with(|p| p.set(true));
        let e = layout("x").unwrap_err();
        assert_eq!(e.message, "This equation could not be laid out.");
        assert!(layout("x").is_ok(), "the next call is unaffected");
    }

    #[test]
    fn the_gallery_lays_out_and_converts() {
        for (name, src) in GALLERY {
            assert!(layout(src).is_ok_and(|l| !l.items.is_empty()), "{name}");
            assert!(to_omml(src, 11.0, None).is_ok(), "{name}");
        }
    }

    #[test]
    fn empty_source_is_an_empty_list() {
        let l = layout("").unwrap();
        assert!(l.items.is_empty());
    }

    #[test]
    fn newline_rows_count_toward_the_matrix_cap() {
        // RaTeX reads `\newline` as `\\`, so it ends a row like `\cr` does.
        for brk in ["\\newline ", "\\cr ", "\\\\"] {
            let src = format!("\\begin{{matrix}}{}{}\\end{{matrix}}", format!("a{brk}").repeat(250), "a&".repeat(700) + "a");
            assert!(src.len() < MAX_SOURCE_BYTES);
            let e = check_caps(&src).unwrap_err();
            assert!(e.message.contains("matrix cells"), "{brk}: {e}");
            let s = src.clone();
            let (l, o) = within(10, move || (layout(&s).unwrap_err(), to_omml(&s, 11.0, None).unwrap_err()));
            assert!(l.message.contains("matrix cells") && o.message.contains("matrix cells"), "{brk}");
        }
        assert!(check_caps("\\begin{matrix}a&b\\newline c&d\\end{matrix}").is_ok());
        assert!(layout("\\begin{matrix}a&b\\newline c&d\\end{matrix}").is_ok());
    }
}
