//! A content document (XHTML) as the blocks the layout sets: paragraphs of styled runs, images,
//! rules, simple tables and page breaks, each with its computed style. Styles come from a small
//! user-agent sheet (what browsers give `h1`, `p`, `blockquote`…), the book's style sheets and
//! `style=""`, in CSS's order; inheritance follows CSS for the properties that inherit.

use std::collections::HashMap;

use super::css::{self, Declaration, Element, Sheet};

/// Points per CSS pixel.
pub const PT_PER_PX: f64 = 0.75;
/// The default font size (16 px).
pub const BASE_SIZE: f64 = 12.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VAlign {
    #[default]
    Baseline,
    Super,
    Sub,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transform {
    #[default]
    None,
    Upper,
    Lower,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    #[default]
    Inline,
    Block,
    ListItem,
    Table,
    Row,
    Cell,
    None,
}

/// What a run of text looks like.
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    /// Points.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Lowercase family names in preference order, generic families (`serif`…) included.
    pub families: Vec<String>,
    pub color: [f64; 3],
    pub underline: bool,
    pub strike: bool,
    pub valign: VAlign,
    pub small_caps: bool,
}

/// The computed style of an element.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub run: RunStyle,
    pub display: Display,
    pub align: Align,
    pub rtl: bool,
    /// Points.
    pub indent: f64,
    /// Line height as a multiple of the font size.
    pub line_height: f64,
    /// Top, right, bottom, left, in points (margin and padding together).
    pub margin: [f64; 4],
    /// Left and right space the enclosing blocks' margins take, in points.
    pub inset: [f64; 2],
    pub break_before: bool,
    pub break_after: bool,
    pub pre: bool,
    pub transform: Transform,
    pub list: ListStyle,
    /// `position: absolute` with `left`/`top`/`width`/`height` in points (fixed-layout pages).
    pub absolute: Option<[Option<f64>; 4]>,
    /// `width`/`height` in points, for images.
    pub size: [Option<f64>; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListStyle {
    #[default]
    Disc,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    None,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            run: RunStyle {
                size: BASE_SIZE,
                bold: false,
                italic: false,
                families: vec!["serif".into()],
                color: [0.0; 3],
                underline: false,
                strike: false,
                valign: VAlign::Baseline,
                small_caps: false,
            },
            display: Display::Block,
            align: Align::Start,
            rtl: false,
            indent: 0.0,
            line_height: 1.25,
            margin: [0.0; 4],
            inset: [0.0; 2],
            break_before: false,
            break_after: false,
            pre: false,
            transform: Transform::None,
            list: ListStyle::Disc,
            absolute: None,
            size: [None, None],
        }
    }
}

/// A piece of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: RunStyle,
    /// A link's target: a container path with `#fragment`, or an external URL.
    pub link: Option<String>,
}

/// What the layout sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A paragraph. `anchors` are ids that point at it.
    Text { runs: Vec<Run>, style: Style, anchors: Vec<String>, heading: Option<u8> },
    /// An image (a container path), with its alternate text.
    Image { path: String, alt: String, style: Style, anchors: Vec<String> },
    /// A horizontal rule.
    Rule { style: Style },
    /// A table: rows of cells, each cell its paragraphs.
    Table { rows: Vec<Vec<Vec<Block>>>, style: Style, anchors: Vec<String> },
    /// A forced page break.
    PageBreak,
    /// Vertical space an enclosing block's margin asks for (collapses with its neighbours').
    Gap(f64),
    /// Ids with no content of their own (an empty `<a id>`), placed where they are.
    Anchor(Vec<String>),
    /// An absolutely positioned box of blocks (fixed-layout pages).
    Positioned { rect: [Option<f64>; 4], blocks: Vec<Block> },
}

/// The user-agent sheet: what an HTML element looks like before the book's CSS.
const UA: &str = "
html, body, div, section, article, aside, nav, header, footer, main, figure, figcaption, address, details, summary,
dl, dt, dd, center, hgroup, ol, ul, li, p, h1, h2, h3, h4, h5, h6, pre, blockquote, hr, table, tr, td, th, caption, img, svg { }
p { margin: 1em 0 }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold; page-break-before: auto }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold }
h4 { margin: 1.33em 0; font-weight: bold }
h5 { font-size: 0.83em; margin: 1.67em 0; font-weight: bold }
h6 { font-size: 0.67em; margin: 2.33em 0; font-weight: bold }
blockquote, figure { margin: 1em 40px }
ul, ol { margin: 1em 0; padding-left: 40px }
li ul, li ol { margin: 0 }
ol { list-style-type: decimal }
dd { margin-left: 40px }
pre, code, kbd, samp, tt { font-family: monospace }
pre { white-space: pre; margin: 1em 0 }
b, strong, th, dt { font-weight: bold }
i, em, cite, var, dfn, address { font-style: italic }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
a { text-decoration: none }
sup { vertical-align: super; font-size: smaller }
sub { vertical-align: sub; font-size: smaller }
small { font-size: smaller }
big { font-size: larger }
center, th, caption { text-align: center }
hr { margin: 0.5em 0 }
td, th { padding: 2px 4px }
";

fn block_tag(tag: &str) -> Option<Display> {
    Some(match tag {
        "html" | "body" | "div" | "section" | "article" | "aside" | "nav" | "header" | "footer" | "main" | "figure" | "figcaption" | "address"
        | "details" | "summary" | "dl" | "dt" | "dd" | "center" | "hgroup" | "ol" | "ul" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre"
        | "blockquote" | "hr" | "caption" | "form" | "fieldset" | "legend" => Display::Block,
        "li" => Display::ListItem,
        "table" => Display::Table,
        "tr" => Display::Row,
        "td" | "th" => Display::Cell,
        "head" | "script" | "style" | "title" | "meta" | "link" | "noscript" | "template" | "audio" | "video" | "object" | "embed" | "iframe"
        | "math" | "rt" | "rp" => Display::None,
        _ => return None,
    })
}

/// A CSS length in points; `em` against `font` (the element's font size, or its parent's for
/// `font-size`), `%` against `percent_of`.
pub fn length(value: &str, font: f64, percent_of: f64) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    if v == "0" || v == "auto" {
        return Some(0.0);
    }
    let num_end = v.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))).unwrap_or(v.len());
    let n: f64 = v[..num_end].parse().ok()?;
    let unit = &v[num_end..];
    let pt = match unit {
        "px" => n * PT_PER_PX,
        "pt" => n,
        "em" => n * font,
        "rem" => n * BASE_SIZE,
        "ex" => n * font * 0.5,
        "%" => n * percent_of / 100.0,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "pc" => n * 12.0,
        "" => n * PT_PER_PX,
        _ => return None,
    };
    pt.is_finite().then_some(pt)
}

/// A CSS colour as RGB 0–1.
pub fn color(value: &str) -> Option<[f64; 3]> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let digits: Vec<u8> = hex.chars().filter_map(|c| c.to_digit(16).map(|d| d as u8)).collect();
        return match digits.len() {
            3 | 4 => Some([digits[0], digits[1], digits[2]].map(|d| f64::from(d * 17) / 255.0)),
            6 | 8 => Some([0, 2, 4].map(|i| f64::from(digits[i] * 16 + digits[i + 1]) / 255.0)),
            _ => None,
        };
    }
    if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")).and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<f64> = inner
            .split([',', ' ', '/'])
            .filter(|p| !p.is_empty())
            .take(3)
            .filter_map(|p| match p.strip_suffix('%') {
                Some(pc) => pc.parse::<f64>().ok().map(|x| x / 100.0),
                None => p.parse::<f64>().ok().map(|x| x / 255.0),
            })
            .collect();
        return (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]].map(|x| x.clamp(0.0, 1.0)));
    }
    let named = match v.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "maroon" => [128, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "navy" => [0, 0, 128],
        "gray" | "grey" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "dimgray" | "dimgrey" => [105, 105, 105],
        "purple" => [128, 0, 128],
        "teal" => [0, 128, 128],
        "olive" => [128, 128, 0],
        "orange" => [255, 165, 0],
        "brown" => [165, 42, 42],
        "darkred" => [139, 0, 0],
        "darkblue" => [0, 0, 139],
        "darkgreen" => [0, 100, 0],
        _ => return None,
    };
    Some(named.map(|c| f64::from(c) / 255.0))
}

/// Apply one declaration to `s` (whose inherited values come from `parent`).
fn apply(s: &mut Style, parent: &Style, prop: &str, value: &str) {
    let v = value.trim().to_ascii_lowercase();
    let font = s.run.size;
    match prop {
        "font-size" => {
            let p = parent.run.size;
            s.run.size = match v.as_str() {
                "xx-small" => 7.0,
                "x-small" => 7.5,
                "small" => 10.0,
                "medium" => 12.0,
                "large" => 13.5,
                "x-large" => 18.0,
                "xx-large" => 24.0,
                "smaller" => p / 1.2,
                "larger" => p * 1.2,
                _ => length(&v, p, p).filter(|x| *x > 0.0).unwrap_or(s.run.size),
            }
            .clamp(2.0, 400.0);
        }
        "font-weight" => s.run.bold = v == "bold" || v == "bolder" || v.parse::<u32>().is_ok_and(|w| w >= 600),
        "font-style" => s.run.italic = v == "italic" || v == "oblique",
        "font-variant" => s.run.small_caps = v.contains("small-caps"),
        "font-family" => s.run.families = value.split(',').map(|f| css::unquote(f).to_ascii_lowercase()).filter(|f| !f.is_empty()).collect(),
        "font" => {
            // The shorthand's parts this layout uses: style, weight, size[/line-height], family.
            for word in value.split_whitespace() {
                let w = word.to_ascii_lowercase();
                match w.as_str() {
                    "italic" | "oblique" => s.run.italic = true,
                    "bold" | "bolder" => s.run.bold = true,
                    _ if w.parse::<u32>().is_ok_and(|n| n >= 600) => s.run.bold = true,
                    _ => {}
                }
            }
            if let Some(size) = value.split_whitespace().find_map(|w| {
                let size = w.split('/').next().unwrap_or(w);
                size.chars().next().filter(char::is_ascii_digit).and(length(size, parent.run.size, parent.run.size))
            }) {
                s.run.size = size.clamp(2.0, 400.0);
            }
            if let Some(i) = value.find([',', '"', '\'']).or_else(|| value.rfind(' ').map(|i| i + 1)) {
                let families = &value[value[..i].rfind(' ').map_or(0, |j| j + 1)..];
                s.run.families = families.split(',').map(|f| css::unquote(f).to_ascii_lowercase()).filter(|f| !f.is_empty()).collect();
            }
        }
        "color" => {
            if let Some(c) = color(&v) {
                s.run.color = c;
            }
        }
        "text-align" => {
            s.align = match v.as_str() {
                "left" => Align::Left,
                "right" => Align::Right,
                "center" => Align::Center,
                "justify" => Align::Justify,
                "end" => Align::End,
                _ => Align::Start,
            }
        }
        "direction" => s.rtl = v == "rtl",
        "text-indent" => s.indent = length(&v, font, 0.0).unwrap_or(0.0),
        "line-height" => {
            s.line_height = match v.as_str() {
                "normal" => 1.25,
                _ => match v.parse::<f64>() {
                    Ok(n) => n,
                    Err(_) => length(&v, font, font).map_or(1.25, |pt| pt / font.max(0.1)),
                },
            }
            .clamp(0.5, 5.0);
        }
        "margin" | "padding" => {
            let parts: Vec<f64> = v.split_whitespace().map(|p| length(p, font, 0.0).unwrap_or(0.0)).collect();
            let [t, r, b, l] = match parts.as_slice() {
                [a] => [*a, *a, *a, *a],
                [a, b] => [*a, *b, *a, *b],
                [a, b, c] => [*a, *b, *c, *b],
                [a, b, c, d, ..] => [*a, *b, *c, *d],
                [] => return,
            };
            let add = prop == "padding";
            for (i, x) in [t, r, b, l].into_iter().enumerate() {
                s.margin[i] = if add { s.margin[i] + x } else { x };
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            let side = match prop.rsplit('-').next() {
                Some("top") => 0,
                Some("right") => 1,
                Some("bottom") => 2,
                _ => 3,
            };
            let x = length(&v, font, 0.0).unwrap_or(0.0);
            s.margin[side] = if prop.starts_with("padding") { s.margin[side] + x } else { x };
        }
        "page-break-before" | "break-before" => s.break_before = matches!(v.as_str(), "always" | "page" | "left" | "right" | "recto" | "verso"),
        "page-break-after" | "break-after" => s.break_after = matches!(v.as_str(), "always" | "page" | "left" | "right" | "recto" | "verso"),
        "display" => {
            s.display = match v.as_str() {
                "none" => Display::None,
                "inline" | "inline-block" => Display::Inline,
                "list-item" => Display::ListItem,
                "table" => Display::Table,
                "table-row" => Display::Row,
                "table-cell" => Display::Cell,
                _ => Display::Block,
            }
        }
        "text-decoration" | "text-decoration-line" => {
            s.run.underline = v.contains("underline");
            s.run.strike = v.contains("line-through");
        }
        "vertical-align" => {
            s.run.valign = match v.as_str() {
                "super" => VAlign::Super,
                "sub" => VAlign::Sub,
                _ => VAlign::Baseline,
            }
        }
        "white-space" => s.pre = v.starts_with("pre"),
        "text-transform" => {
            s.transform = match v.as_str() {
                "uppercase" => Transform::Upper,
                "lowercase" => Transform::Lower,
                _ => Transform::None,
            }
        }
        "list-style-type" | "list-style" => {
            s.list = if v.contains("none") {
                ListStyle::None
            } else if v.contains("lower-alpha") || v.contains("lower-latin") {
                ListStyle::LowerAlpha
            } else if v.contains("upper-alpha") || v.contains("upper-latin") {
                ListStyle::UpperAlpha
            } else if v.contains("lower-roman") {
                ListStyle::LowerRoman
            } else if v.contains("upper-roman") {
                ListStyle::UpperRoman
            } else if v.contains("decimal") {
                ListStyle::Decimal
            } else {
                ListStyle::Disc
            }
        }
        "position" => s.absolute = (v == "absolute" || v == "fixed").then_some([None; 4]),
        "left" | "top" | "width" | "height" => {
            let i = ["left", "top", "width", "height"].iter().position(|p| *p == prop).unwrap_or(0);
            // A percentage of the containing block is unknown here: for a size, leave it to the natural
            // size (fitted to the page later) rather than a zero-sized box.
            let x = length(&v, font, 0.0).filter(|_| v != "auto" && !(i >= 2 && v.ends_with('%')));
            if i >= 2 {
                s.size[i - 2] = x;
            }
            if let Some(abs) = &mut s.absolute {
                abs[i] = x;
            }
        }
        _ => {}
    }
}

/// The style an element of `tag` inherits from `parent` before any rule applies.
fn inherited(parent: &Style, tag: &str) -> Style {
    Style {
        run: RunStyle { underline: parent.run.underline, strike: parent.run.strike, valign: VAlign::Baseline, ..parent.run.clone() },
        display: block_tag(tag).unwrap_or(Display::Inline),
        align: parent.align,
        rtl: parent.rtl,
        indent: parent.indent,
        line_height: parent.line_height,
        margin: [0.0; 4],
        inset: if parent.display == Display::Inline {
            parent.inset
        } else {
            [parent.inset[0] + parent.margin[3], parent.inset[1] + parent.margin[1]]
        },
        break_before: false,
        break_after: false,
        pre: parent.pre,
        transform: parent.transform,
        list: parent.list,
        absolute: None,
        size: [None, None],
    }
}

/// Style sheets in cascade order (the user-agent sheet first) and a cache of their rules by
/// element shape.
pub struct Cascade {
    sheets: Vec<Sheet>,
}

impl Cascade {
    pub fn new(book: Vec<Sheet>) -> Self {
        let mut sheets = vec![css::parse(UA, "")];
        sheets.extend(book);
        Cascade { sheets }
    }

    /// The declarations that apply to the last element of `path`, in the order to apply them.
    fn declarations(&self, path: &[Element]) -> Vec<Declaration> {
        // (important, specificity, source order, declaration)
        type Hit<'d> = (bool, (u32, u32, u32), usize, &'d Declaration);
        let mut hits: Vec<Hit> = Vec::new();
        let mut order = 0usize;
        for sheet in &self.sheets {
            for rule in &sheet.rules {
                order += 1;
                let Some(spec) = rule.selectors.iter().filter(|s| css::matches(s, path)).map(|s| s.specificity).max() else { continue };
                for d in &rule.declarations {
                    hits.push((d.2, spec, order, d));
                }
            }
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        hits.into_iter().map(|h| h.3.clone()).collect()
    }
}

/// Elements nested deeper than this are not walked one by one: their text is kept, in the style
/// of the element at the limit. The walk recurses, so this bounds the stack.
pub const MAX_DEPTH: usize = 256;
/// A list's `start` above this counts from here (a marker can't be worth a huge number).
pub const MAX_LIST_START: usize = 1_000_000;
/// Roman numerals are written up to this; past it the marker is decimal, as CSS does.
const MAX_ROMAN: usize = 3999;

/// The blocks of one content document. `path` is its container path (for links and images).
pub struct Builder<'a> {
    cascade: &'a Cascade,
    path: &'a str,
    blocks: Vec<Block>,
    /// The paragraph being filled.
    runs: Vec<Run>,
    para_style: Option<Style>,
    anchors: Vec<String>,
    heading: Option<u8>,
    /// Elements and their computed styles, outermost first.
    stack: Vec<(Element, Style)>,
    link: Option<String>,
    /// Counters of the lists being filled.
    lists: Vec<usize>,
    /// Notes for the repair log.
    pub notes: Vec<String>,
}

impl<'a> Builder<'a> {
    pub fn new(cascade: &'a Cascade, path: &'a str) -> Self {
        Builder {
            cascade,
            path,
            blocks: Vec::new(),
            runs: Vec::new(),
            para_style: None,
            anchors: Vec::new(),
            heading: None,
            stack: Vec::new(),
            link: None,
            lists: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn style_of(&self, node: roxmltree::Node, element: &Element) -> Style {
        let parent = self.stack.last().map(|s| s.1.clone()).unwrap_or_default();
        let mut s = inherited(&parent, &element.tag);
        let mut path: Vec<Element> = self.stack.iter().map(|(e, _)| e.clone()).collect();
        path.push(element.clone());
        let mut decls = self.cascade.declarations(&path);
        // Presentational attributes, then style="" (which wins over the sheets but not !important).
        if let Some(a) = node.attribute("align") {
            decls.insert(0, ("text-align".into(), a.into(), false));
        }
        if node.attribute("dir") == Some("rtl") {
            decls.insert(0, ("direction".into(), "rtl".into(), false));
        } else if node.attribute("dir") == Some("ltr") {
            decls.insert(0, ("direction".into(), "ltr".into(), false));
        }
        if let Some(inline) = node.attribute("style") {
            let important_at = decls.iter().position(|d| d.2).unwrap_or(decls.len());
            let inline = css::declarations(inline);
            for (i, d) in inline.into_iter().enumerate() {
                decls.insert(important_at + i, d);
            }
        }
        // font-size first: em lengths in the rest are against the element's own size.
        for (p, v, _) in decls.iter().filter(|d| d.0 == "font-size" || d.0 == "font") {
            apply(&mut s, &parent, p, v);
        }
        for (p, v, _) in decls.iter().filter(|d| d.0 != "font-size" && d.0 != "font") {
            apply(&mut s, &parent, p, v);
        }
        s
    }

    /// End the paragraph being filled.
    fn flush(&mut self) {
        let has_text = self.runs.iter().any(|r| !r.text.trim().is_empty() || (self.para_style.as_ref().is_some_and(|s| s.pre) && !r.text.is_empty()));
        if has_text {
            let style = self.para_style.clone().unwrap_or_else(|| self.stack.last().map(|s| s.1.clone()).unwrap_or_default());
            let runs = std::mem::take(&mut self.runs);
            self.blocks.push(Block::Text { runs, style, anchors: std::mem::take(&mut self.anchors), heading: self.heading });
        } else {
            self.runs.clear();
        }
        self.para_style = None;
    }

    fn text(&mut self, text: &str) {
        let Some((_, style)) = self.stack.last() else { return };
        let pre = style.pre;
        let mut t = if pre { text.replace("\r\n", "\n") } else { text.split_whitespace().collect::<Vec<_>>().join(" ") };
        if !pre && !text.is_empty() {
            // Keep the white space at the edges as one space (it separates words across tags).
            if text.starts_with(char::is_whitespace) && !t.is_empty() {
                t.insert(0, ' ');
            }
            if text.ends_with(char::is_whitespace) && !t.ends_with(' ') {
                t.push(' ');
            }
            if t.is_empty() {
                t.push(' ');
            }
        }
        t = match style.transform {
            Transform::Upper => t.to_uppercase(),
            Transform::Lower => t.to_lowercase(),
            Transform::None => t,
        };
        if self.para_style.is_none() {
            if t.trim().is_empty() && !pre {
                return;
            }
            // Anonymous paragraph: the nearest block ancestor's style.
            self.para_style = self.stack.iter().rev().find(|(_, s)| s.display != Display::Inline).map(|(_, s)| s.clone());
            t = t.trim_start().to_string();
        }
        let run = Run { text: t, style: style.run.clone(), link: self.link.clone() };
        match self.runs.last_mut() {
            Some(last) if last.style == run.style && last.link == run.link => last.text.push_str(&run.text),
            _ => self.runs.push(run),
        }
    }

    fn marker(&self, style: &Style) -> Option<String> {
        let n = *self.lists.last()?;
        let roman = |mut n: usize| {
            let mut s = String::new();
            for (v, r) in [
                (1000, "m"),
                (900, "cm"),
                (500, "d"),
                (400, "cd"),
                (100, "c"),
                (90, "xc"),
                (50, "l"),
                (40, "xl"),
                (10, "x"),
                (9, "ix"),
                (5, "v"),
                (4, "iv"),
                (1, "i"),
            ] {
                while n >= v {
                    s.push_str(r);
                    n -= v;
                }
            }
            s
        };
        let alpha = |n: usize| char::from(b'a' + ((n.saturating_sub(1)) % 26) as u8).to_string();
        Some(match style.list {
            ListStyle::None => return None,
            ListStyle::Disc => "•".into(),
            ListStyle::Decimal => format!("{n}."),
            ListStyle::LowerAlpha => format!("{}.", alpha(n)),
            ListStyle::UpperAlpha => format!("{}.", alpha(n).to_uppercase()),
            ListStyle::LowerRoman | ListStyle::UpperRoman if n == 0 || n > MAX_ROMAN => format!("{n}."),
            ListStyle::LowerRoman => format!("{}.", roman(n)),
            ListStyle::UpperRoman => format!("{}.", roman(n).to_uppercase()),
        })
    }

    /// Walk `node` and its subtree.
    pub fn node(&mut self, node: roxmltree::Node) {
        if node.is_text() {
            if let Some(t) = node.text() {
                self.text(t);
            }
            return;
        }
        if !node.is_element() {
            return;
        }
        if self.stack.len() >= MAX_DEPTH {
            // Too deep to walk: the words only, gathered without recursing.
            for t in node.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()) {
                self.text(t);
            }
            let note = format!("{}: elements nested more than {MAX_DEPTH} deep were kept as plain text", self.path);
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
            return;
        }
        let tag = node.tag_name().name().to_ascii_lowercase();
        let element = Element {
            tag: tag.clone(),
            id: node.attribute("id").map(str::to_string),
            classes: node.attribute("class").map(|c| c.split_whitespace().map(str::to_string).collect()).unwrap_or_default(),
        };
        let style = self.style_of(node, &element);
        if style.display == Display::None || node.attribute("hidden").is_some() {
            return;
        }
        let id = node.attribute("id").map(|id| format!("{}#{id}", self.path));
        match tag.as_str() {
            "img" | "image" => {
                let src = node.attribute("src").or_else(|| node.attributes().find(|a| a.name() == "href").map(|a| a.value()));
                if let Some(path) = src.and_then(|s| super::package::resolve(self.path, s)) {
                    self.flush();
                    let alt = node.attribute("alt").unwrap_or_default().to_string();
                    let mut anchors = std::mem::take(&mut self.anchors);
                    anchors.extend(id);
                    self.blocks.push(Block::Image { path, alt, style, anchors });
                }
                return;
            }
            "svg" => {
                // An SVG wrapper around a picture (common in fixed-layout and cover pages).
                if let Some(image) = node.descendants().find(|d| d.tag_name().name() == "image") {
                    self.stack.push((element, style));
                    self.node(image);
                    self.stack.pop();
                } else {
                    self.notes.push(format!("{}: an inline SVG drawing was left out", self.path));
                }
                return;
            }
            "br" => {
                if self.para_style.is_some() {
                    self.runs.push(Run { text: "\n".into(), style: style.run.clone(), link: None });
                }
                return;
            }
            "hr" => {
                self.flush();
                self.blocks.push(Block::Rule { style });
                return;
            }
            "table" => {
                self.flush();
                let table = self.table(node, &element, &style);
                self.blocks.push(table);
                return;
            }
            "math" => return,
            _ => {}
        }
        if let Some(abs) = style.absolute {
            // A positioned box (fixed layout): its own blocks.
            self.flush();
            let mut inner = Builder::new(self.cascade, self.path);
            inner.stack = self.stack.clone();
            inner.stack.push((element.clone(), Style { absolute: None, ..style.clone() }));
            for child in node.children() {
                inner.node(child);
            }
            inner.flush();
            self.notes.append(&mut inner.notes);
            self.blocks.push(Block::Positioned { rect: abs, blocks: inner.blocks });
            return;
        }
        let block = style.display != Display::Inline;
        if block {
            self.flush();
            if style.break_before && !self.blocks.is_empty() {
                self.blocks.push(Block::PageBreak);
            }
            if style.margin[0] > 0.0 {
                self.blocks.push(Block::Gap(style.margin[0]));
            }
        }
        if let Some(id) = id {
            self.anchors.push(id);
        }
        let heading = match tag.as_str() {
            "h1" => Some(1),
            "h2" => Some(2),
            "h3" => Some(3),
            "h4" => Some(4),
            "h5" => Some(5),
            "h6" => Some(6),
            _ => None,
        };
        let is_list = matches!(tag.as_str(), "ul" | "ol");
        if is_list {
            let start = node.attribute("start").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).min(MAX_LIST_START);
            self.lists.push(start.saturating_sub(1));
        }
        let saved_link = self.link.clone();
        if tag == "a"
            && let Some(href) = node.attribute("href")
        {
            self.link =
                if href.contains("://") || href.starts_with("mailto:") { Some(href.to_string()) } else { super::package::resolve(self.path, href) };
        }
        let st = style.clone();
        self.stack.push((element, style));
        if block {
            self.para_style = Some(st.clone());
            self.heading = heading;
            if st.display == Display::ListItem {
                if let Some(n) = self.lists.last_mut() {
                    *n = n.saturating_add(1);
                }
                if let Some(m) = self.marker(&st) {
                    self.runs.push(Run { text: format!("{m}\u{a0}"), style: st.run.clone(), link: None });
                }
            }
            // The paragraph starts empty: only text makes it count.
            if self.runs.is_empty() {
                self.para_style = None;
                self.heading = heading;
            }
        }
        for child in node.children() {
            self.node(child);
        }
        self.stack.pop();
        self.link = saved_link;
        if is_list {
            self.lists.pop();
        }
        if block {
            self.flush();
            self.heading = None;
            if st.margin[2] > 0.0 {
                self.blocks.push(Block::Gap(st.margin[2]));
            }
            if st.break_after {
                self.blocks.push(Block::PageBreak);
            }
        }
    }

    fn table(&mut self, node: roxmltree::Node, element: &Element, style: &Style) -> Block {
        let mut rows = Vec::new();
        self.stack.push((element.clone(), style.clone()));
        let row_nodes: Vec<_> = node
            .descendants()
            .filter(|d| d.tag_name().name() == "tr" && d.ancestors().skip(1).find(|a| a.tag_name().name() == "table") == Some(node))
            .collect();
        for tr in row_nodes {
            let mut cells = Vec::new();
            for td in tr.children().filter(|c| matches!(c.tag_name().name(), "td" | "th")) {
                let mut inner = Builder::new(self.cascade, self.path);
                inner.stack = self.stack.clone();
                inner.node(td);
                inner.flush();
                self.notes.append(&mut inner.notes);
                cells.push(inner.blocks);
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        self.stack.pop();
        let mut anchors = std::mem::take(&mut self.anchors);
        if let Some(id) = node.attribute("id") {
            anchors.push(format!("{}#{id}", self.path));
        }
        Block::Table { rows, style: style.clone(), anchors }
    }

    /// The blocks, with any ids left at the end placed there.
    pub fn finish(mut self) -> (Vec<Block>, Vec<String>) {
        self.flush();
        if !self.anchors.is_empty() {
            self.blocks.push(Block::Anchor(std::mem::take(&mut self.anchors)));
        }
        (self.blocks, self.notes)
    }
}

/// The style sheets a content document uses: linked ones (read through `read`) and `<style>`.
pub fn sheets(xml: &roxmltree::Document, path: &str, read: &mut dyn FnMut(&str) -> Option<String>) -> Vec<Sheet> {
    let mut out = Vec::new();
    for n in xml.descendants() {
        match n.tag_name().name() {
            "link" if n.attribute("rel").is_some_and(|r| r.split_whitespace().any(|x| x.eq_ignore_ascii_case("stylesheet"))) => {
                if let Some(target) = n.attribute("href").and_then(|h| super::package::resolve(path, h))
                    && let Some(text) = read(&target)
                {
                    out.push(css::parse(&text, &target));
                }
            }
            "style" => out.push(css::parse(n.text().unwrap_or_default(), path)),
            _ => {}
        }
    }
    out
}

/// The `width`/`height` of a fixed-layout document's `<meta name="viewport">`, in CSS pixels.
pub fn viewport(xml: &roxmltree::Document) -> Option<(f64, f64)> {
    let content = xml.descendants().find(|n| n.tag_name().name() == "meta" && n.attribute("name") == Some("viewport"))?.attribute("content")?;
    let mut size: HashMap<&str, f64> = HashMap::new();
    for part in content.split([',', ';']) {
        if let Some((k, v)) = part.split_once('=')
            && let Ok(n) = v.trim().trim_end_matches("px").parse::<f64>()
        {
            size.insert(k.trim(), n);
        }
    }
    let (w, h) = (*size.get("width")?, *size.get("height")?);
    (w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(html: &str, css_text: &str) -> Vec<Block> {
        let doc = super::super::package::parse(html).unwrap();
        let cascade = Cascade::new(vec![css::parse(css_text, "OEBPS/s.css")]);
        let mut b = Builder::new(&cascade, "OEBPS/c.xhtml");
        b.node(doc.root_element());
        b.finish().0.into_iter().filter(|b| !matches!(b, Block::Gap(_))).collect()
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { runs, .. } => Some(runs.iter().map(|r| r.text.as_str()).collect()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn paragraphs_headings_and_inline_styles() {
        let b = blocks(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><style>p { text-indent: 1em }</style></head>
               <body><h1 id="c1">Chapter <em>One</em></h1><p class="x">Some   <b>bold</b>
               text.</p><p style="color: #f00; font-size: 6pt">Red</p><div>Loose <span>text</span><p>inner</p></div></body></html>"#,
            "p { text-indent: 1em } .x { text-align: justify } p.x b { color: rgb(0, 0, 255) }",
        );
        assert_eq!(texts(&b), ["Chapter One", "Some bold text.", "Red", "Loose text", "inner"]);
        let Block::Text { runs, heading, anchors, style } = &b[0] else { panic!() };
        assert_eq!((*heading, anchors.as_slice()), (Some(1), ["OEBPS/c.xhtml#c1".to_string()].as_slice()));
        assert!(runs[0].style.bold && runs[1].style.italic && (runs[0].style.size - 24.0).abs() < 1e-9 && style.margin[0] > 0.0);
        let Block::Text { runs, style, .. } = &b[1] else { panic!() };
        assert_eq!(style.align, Align::Justify);
        assert_eq!((style.indent, runs[1].style.color), (12.0, [0.0, 0.0, 1.0]));
        let Block::Text { runs, .. } = &b[2] else { panic!() };
        assert_eq!((runs[0].style.color, runs[0].style.size), ([1.0, 0.0, 0.0], 6.0));
    }

    #[test]
    fn lists_links_images_tables_and_breaks() {
        let b = blocks(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><ol><li>One</li><li>Two <a href="b.xhtml#n">note</a></li></ol>
               <img src="../img/p.png" alt="A cat"/><p style="page-break-before: always">After</p>
               <table><tr><td>a</td><td>b</td></tr><tr><th>c</th></tr></table><span id="end"/></body></html>"#,
            "",
        );
        assert_eq!(texts(&b)[..2], ["1.\u{a0}One", "2.\u{a0}Two note"]);
        let Block::Text { runs, .. } = &b[1] else { panic!() };
        assert_eq!(runs.last().unwrap().link.as_deref(), Some("OEBPS/b.xhtml#n"));
        assert!(matches!(&b[2], Block::Image { path, alt, .. } if path == "img/p.png" && alt == "A cat"), "{:?}", b[2]);
        assert_eq!(b[3], Block::PageBreak);
        assert!(matches!(&b[5], Block::Table { rows, .. } if rows.len() == 2 && rows[0].len() == 2));
        assert_eq!(b.last(), Some(&Block::Anchor(vec!["OEBPS/c.xhtml#end".into()])));
    }

    #[test]
    fn lengths_and_colours() {
        assert_eq!(length("16px", 12.0, 0.0), Some(12.0));
        assert_eq!(length("1.5em", 10.0, 0.0), Some(15.0));
        assert_eq!(length("50%", 10.0, 200.0), Some(100.0));
        assert_eq!(color("#fff"), Some([1.0; 3]));
        assert_eq!(color("rgb(255 0 0)"), Some([1.0, 0.0, 0.0]));
        assert_eq!(color("teal").map(|c| (c[1] * 255.0).round()), Some(128.0));
    }
}
