//! Word (WordprocessingML, ECMA-376 Part 1 §17) to PDF. The body's paragraphs and tables flow
//! onto pages sized by each section; styles, direct formatting, list numbering, tab stops,
//! images, headers and footers (with page-number fields) and footnotes are carried over.
//! Floating objects are placed in the flow where they are anchored, and text boxes' text joins
//! the flow after their paragraph.

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef};
use roxmltree::Node;

use super::canvas::{Canvas, Page};
use super::flow::{self, Align, Field, Inline, Laid, LineRule, Para, fin};
use super::xml::{self, Rels, attr, attr_f64, child, children, flag, is, rel_attr};
use super::zip::Zip;
use super::{Notes, RunStyle, family_of, hex_color};

/// Deepest nesting of content controls, tables and text boxes followed.
const MAX_DEPTH: usize = 24;

/// Longest numbering-level text kept, in characters: every paragraph the level numbers copies
/// it, so a hostile level would otherwise be multiplied by the paragraph count.
const MAX_LVL_TEXT: usize = 64;

fn twips(v: f64) -> f64 {
    v / 20.0
}

// ── styles ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default, Debug)]
struct RPr {
    font: Option<String>,
    /// `Some(true)` for the theme's major (heading) font, `Some(false)` for the minor one.
    theme_major: Option<bool>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Option<[f64; 3]>>,
    size: Option<f64>,
    highlight: Option<Option<[f64; 3]>>,
    vert: Option<i8>,
    caps: Option<bool>,
    hidden: Option<bool>,
    style: Option<String>,
}

impl RPr {
    fn merge(&mut self, o: &RPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(font, theme_major, bold, italic, underline, strike, color, size, highlight, vert, caps, hidden, style);
        if o.font.is_some() {
            self.theme_major = o.theme_major;
        }
    }

    fn parse(n: Node<'_, '_>) -> RPr {
        let mut r = RPr::default();
        for c in n.children().filter(Node::is_element) {
            let val = attr(c, "val");
            match c.tag_name().name() {
                "rStyle" => r.style = val.map(str::to_owned),
                "rFonts" => {
                    if let Some(f) = attr(c, "ascii").or_else(|| attr(c, "hAnsi")) {
                        r.font = Some(f.to_owned());
                        r.theme_major = None;
                    } else if let Some(t) = attr(c, "asciiTheme").or_else(|| attr(c, "hAnsiTheme")) {
                        r.font = Some(String::new());
                        r.theme_major = Some(t.starts_with("major"));
                    }
                }
                "b" => r.bold = Some(flag(val)),
                "i" => r.italic = Some(flag(val)),
                "u" => r.underline = Some(val.is_some_and(|v| v != "none")),
                "strike" | "dstrike" => r.strike = Some(flag(val)),
                "color" => r.color = val.map(|v| if v == "auto" { None } else { hex_color(v) }),
                "sz" => r.size = attr_f64(c, "val").map(|v| v / 2.0).filter(|v| *v > 0.0),
                "highlight" => r.highlight = val.map(highlight_color),
                "shd" => {
                    if let Some(f) = attr(c, "fill").filter(|f| *f != "auto").and_then(hex_color) {
                        r.highlight = Some(Some(f));
                    }
                }
                "vertAlign" => {
                    r.vert = Some(match val {
                        Some("superscript") => 1,
                        Some("subscript") => -1,
                        _ => 0,
                    })
                }
                "caps" | "smallCaps" => r.caps = Some(flag(val)),
                "vanish" | "specVanish" => r.hidden = Some(flag(val)),
                _ => {}
            }
        }
        r
    }

    fn style(&self, theme: &Theme) -> RunStyle {
        let font = match (&self.font, self.theme_major) {
            (_, Some(true)) => theme.major.as_str(),
            (_, Some(false)) => theme.minor.as_str(),
            (Some(f), None) => f.as_str(),
            (None, None) => theme.minor.as_str(),
        };
        RunStyle {
            fam: family_of(font),
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            size: fin(self.size.unwrap_or(10.0), 10.0).clamp(1.0, 400.0),
            color: self.color.flatten().unwrap_or([0.0; 3]),
            underline: self.underline.unwrap_or(false),
            strike: self.strike.unwrap_or(false),
            highlight: self.highlight.flatten(),
            vert: self.vert.unwrap_or(0),
            caps: self.caps.unwrap_or(false),
            hidden: self.hidden.unwrap_or(false),
        }
    }
}

fn highlight_color(v: &str) -> Option<[f64; 3]> {
    Some(match v {
        "yellow" => [1.0, 1.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "cyan" => [0.0, 1.0, 1.0],
        "magenta" => [1.0, 0.0, 1.0],
        "blue" => [0.0, 0.0, 1.0],
        "red" => [1.0, 0.0, 0.0],
        "darkBlue" => [0.0, 0.0, 0.5],
        "darkCyan" => [0.0, 0.5, 0.5],
        "darkGreen" => [0.0, 0.5, 0.0],
        "darkMagenta" => [0.5, 0.0, 0.5],
        "darkRed" => [0.5, 0.0, 0.0],
        "darkYellow" => [0.5, 0.5, 0.0],
        "darkGray" => [0.5, 0.5, 0.5],
        "lightGray" => [0.75, 0.75, 0.75],
        "black" => [0.0, 0.0, 0.0],
        "white" => [1.0, 1.0, 1.0],
        _ => return None,
    })
}

#[derive(Clone, Default, Debug)]
struct PPr {
    style: Option<String>,
    align: Option<Align>,
    left: Option<f64>,
    right: Option<f64>,
    first: Option<f64>,
    before: Option<f64>,
    after: Option<f64>,
    line: Option<LineRule>,
    num: Option<(String, usize)>,
    tabs: Vec<(f64, bool)>,
    page_break_before: Option<bool>,
    shade: Option<Option<[f64; 3]>>,
    keep_next: Option<bool>,
    contextual: Option<bool>,
}

impl PPr {
    fn merge(&mut self, o: &PPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(style, align, left, right, first, before, after, line, num, page_break_before, shade, keep_next, contextual);
        for (pos, clear) in &o.tabs {
            self.tabs.retain(|(p, _)| (p - pos).abs() > 0.5);
            if !clear {
                self.tabs.push((*pos, false));
            }
        }
    }

    fn parse(n: Node<'_, '_>) -> PPr {
        let mut p = PPr::default();
        for c in n.children().filter(Node::is_element) {
            let val = attr(c, "val");
            match c.tag_name().name() {
                "pStyle" => p.style = val.map(str::to_owned),
                "jc" => {
                    p.align = Some(match val {
                        Some("center") => Align::Center,
                        Some("right" | "end") => Align::Right,
                        Some("both" | "distribute" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute") => Align::Justify,
                        _ => Align::Left,
                    })
                }
                "ind" => {
                    if let Some(v) = attr_f64(c, "left").or_else(|| attr_f64(c, "start")) {
                        p.left = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "right").or_else(|| attr_f64(c, "end")) {
                        p.right = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "hanging") {
                        p.first = Some(-twips(v));
                    } else if let Some(v) = attr_f64(c, "firstLine") {
                        p.first = Some(twips(v));
                    }
                }
                "spacing" => {
                    if let Some(v) = attr_f64(c, "before") {
                        p.before = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "after") {
                        p.after = Some(twips(v));
                    }
                    if flag(attr(c, "beforeAutospacing")) && attr(c, "beforeAutospacing").is_some() {
                        p.before = Some(14.0);
                    }
                    if flag(attr(c, "afterAutospacing")) && attr(c, "afterAutospacing").is_some() {
                        p.after = Some(14.0);
                    }
                    if let Some(v) = attr_f64(c, "line") {
                        p.line = Some(match attr(c, "lineRule") {
                            Some("exact") => LineRule::Exact(twips(v.abs())),
                            Some("atLeast") => LineRule::AtLeast(twips(v)),
                            _ => LineRule::Auto(v / 240.0),
                        });
                    }
                }
                "numPr" => {
                    let id = child(c, "numId").and_then(|n| attr(n, "val")).unwrap_or("0").to_owned();
                    let lvl = child(c, "ilvl").and_then(|n| attr_f64(n, "val")).unwrap_or(0.0).clamp(0.0, 8.0) as usize;
                    p.num = Some((id, lvl));
                }
                "tabs" => {
                    for t in children(c, "tab") {
                        if let Some(pos) = attr_f64(t, "pos") {
                            p.tabs.push((twips(pos), attr(t, "val") == Some("clear")));
                        }
                    }
                }
                "pageBreakBefore" => p.page_break_before = Some(flag(val)),
                "keepNext" => p.keep_next = Some(flag(val)),
                "contextualSpacing" => p.contextual = Some(flag(val)),
                "shd" => p.shade = Some(attr(c, "fill").filter(|f| *f != "auto").and_then(hex_color)),
                _ => {}
            }
        }
        p
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Border {
    width: f64,
    color: [f64; 3],
}

/// Table or cell borders; `None` = not specified here, `Some(None)` = explicitly none.
#[derive(Clone, Copy, Default, Debug)]
struct Borders {
    top: Option<Option<Border>>,
    bottom: Option<Option<Border>>,
    left: Option<Option<Border>>,
    right: Option<Option<Border>>,
    inside_h: Option<Option<Border>>,
    inside_v: Option<Option<Border>>,
}

impl Borders {
    fn parse(n: Node<'_, '_>) -> Borders {
        let side = |names: &[&str]| -> Option<Option<Border>> {
            let c = names.iter().find_map(|name| child(n, name))?;
            let val = attr(c, "val").unwrap_or("single");
            if matches!(val, "nil" | "none") {
                return Some(None);
            }
            let width = attr_f64(c, "sz").map_or(0.5, |s| (s / 8.0).clamp(0.25, 6.0));
            let color = attr(c, "color").filter(|c| *c != "auto").and_then(hex_color).unwrap_or([0.0; 3]);
            Some(Some(Border { width, color }))
        };
        Borders {
            top: side(&["top"]),
            bottom: side(&["bottom"]),
            left: side(&["left", "start"]),
            right: side(&["right", "end"]),
            inside_h: side(&["insideH"]),
            inside_v: side(&["insideV"]),
        }
    }

    fn merge(&mut self, o: &Borders) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(top, bottom, left, right, inside_h, inside_v);
    }
}

#[derive(Clone, Default, Debug)]
struct StyleDef {
    based_on: Option<String>,
    p: PPr,
    r: RPr,
    borders: Option<Borders>,
}

struct Theme {
    major: String,
    minor: String,
}

struct Styles {
    p_default: PPr,
    r_default: RPr,
    by_id: HashMap<String, StyleDef>,
    default_para: Option<String>,
    default_table: Option<String>,
}

impl Styles {
    /// A style and the styles it is based on, root first (cycles and deep chains are cut).
    fn chain(&self, id: &str) -> Vec<&StyleDef> {
        let mut out = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            if seen.contains(&i) || seen.len() > 16 {
                break;
            }
            seen.push(i);
            let Some(s) = self.by_id.get(i) else { break };
            out.push(s);
            cur = s.based_on.as_deref();
        }
        out.reverse();
        out
    }

    fn para_style(&self, id: Option<&str>) -> (PPr, RPr) {
        let mut p = self.p_default.clone();
        let mut r = self.r_default.clone();
        if let Some(id) = id.or(self.default_para.as_deref()) {
            for s in self.chain(id) {
                p.merge(&s.p);
                r.merge(&s.r);
            }
        }
        (p, r)
    }

    fn run_style(&self, id: &str) -> RPr {
        let mut r = RPr::default();
        for s in self.chain(id) {
            r.merge(&s.r);
        }
        r
    }

    fn table_borders(&self, id: Option<&str>) -> Borders {
        let mut b = Borders::default();
        if let Some(id) = id.or(self.default_table.as_deref()) {
            for s in self.chain(id) {
                if let Some(sb) = &s.borders {
                    b.merge(sb);
                }
            }
        }
        b
    }
}

fn load_theme(zip: &Zip<'_>, rels: &Rels) -> Theme {
    let mut t = Theme { major: "Calibri Light".into(), minor: "Calibri".into() };
    let Some(part) = rels.of_type("theme") else { return t };
    let Ok(Some(text)) = zip.text(part) else { return t };
    let Ok(d) = xml::parse(&text) else { return t };
    for (name, slot) in [("majorFont", &mut t.major), ("minorFont", &mut t.minor)] {
        if let Some(face) =
            d.descendants().find(|n| is(*n, name)).and_then(|f| child(f, "latin")).and_then(|l| attr(l, "typeface")).filter(|f| !f.is_empty())
        {
            *slot = face.to_owned();
        }
    }
    t
}

fn load_styles(zip: &Zip<'_>, rels: &Rels) -> Result<Styles, String> {
    let mut s = Styles { p_default: PPr::default(), r_default: RPr::default(), by_id: HashMap::new(), default_para: None, default_table: None };
    let Some(part) = rels.of_type("styles") else { return Ok(s) };
    let Some(text) = zip.text(part)? else { return Ok(s) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    if let Some(defaults) = child(root, "docDefaults") {
        if let Some(r) = xml::path(defaults, &["rPrDefault", "rPr"]) {
            s.r_default = RPr::parse(r);
        }
        if let Some(p) = xml::path(defaults, &["pPrDefault", "pPr"]) {
            s.p_default = PPr::parse(p);
        }
    }
    for st in children(root, "style") {
        let Some(id) = attr(st, "styleId") else { continue };
        let ty = attr(st, "type").unwrap_or("paragraph");
        let def = StyleDef {
            based_on: child(st, "basedOn").and_then(|b| attr(b, "val")).map(str::to_owned),
            p: child(st, "pPr").map(PPr::parse).unwrap_or_default(),
            r: child(st, "rPr").map(RPr::parse).unwrap_or_default(),
            borders: xml::path(st, &["tblPr", "tblBorders"]).map(Borders::parse),
        };
        if flag(attr(st, "default")) && attr(st, "default").is_some() {
            match ty {
                "paragraph" => s.default_para = Some(id.to_owned()),
                "table" => s.default_table = Some(id.to_owned()),
                _ => {}
            }
        }
        s.by_id.insert(id.to_owned(), def);
    }
    Ok(s)
}

// ── numbering ───────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default, Debug)]
struct Level {
    format: String,
    text: String,
    start: i64,
    p: PPr,
    r: RPr,
}

/// A list instance: its nine levels and its start overrides.
type NumDef = (Vec<Level>, Vec<(usize, i64)>);

/// A table cell laid out: its paragraphs, and its lines and gaps in order (height, paragraph and
/// line, or `None` for spacing).
type CellLines = (Vec<Laid>, Vec<(f64, Option<(usize, usize)>)>);

#[derive(Default)]
struct Numbering {
    /// numId → its definition.
    nums: HashMap<String, NumDef>,
    counters: HashMap<String, [i64; 9]>,
}

fn load_numbering(zip: &Zip<'_>, rels: &Rels) -> Result<Numbering, String> {
    let mut out = Numbering::default();
    let Some(part) = rels.of_type("numbering") else { return Ok(out) };
    let Some(text) = zip.text(part)? else { return Ok(out) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    let mut abstracts: HashMap<String, Vec<Level>> = HashMap::new();
    for a in children(root, "abstractNum") {
        let Some(id) = attr(a, "abstractNumId") else { continue };
        let mut levels = vec![Level::default(); 9];
        for l in children(a, "lvl") {
            let i = attr_f64(l, "ilvl").unwrap_or(0.0).clamp(0.0, 8.0) as usize;
            let Some(slot) = levels.get_mut(i) else { continue };
            *slot = Level {
                format: child(l, "numFmt").and_then(|n| attr(n, "val")).unwrap_or("decimal").to_owned(),
                text: child(l, "lvlText").and_then(|n| attr(n, "val")).unwrap_or_default().chars().take(MAX_LVL_TEXT).collect(),
                start: child(l, "start").and_then(|n| attr_f64(n, "val")).unwrap_or(1.0).clamp(-1e9, 1e9) as i64,
                p: child(l, "pPr").map(PPr::parse).unwrap_or_default(),
                r: child(l, "rPr").map(RPr::parse).unwrap_or_default(),
            };
        }
        abstracts.insert(id.to_owned(), levels);
    }
    for n in children(root, "num") {
        let Some(id) = attr(n, "numId") else { continue };
        let Some(abs) = child(n, "abstractNumId").and_then(|a| attr(a, "val")) else { continue };
        let Some(levels) = abstracts.get(abs) else { continue };
        let overrides = children(n, "lvlOverride")
            .filter_map(|o| {
                let lvl = attr_f64(o, "ilvl")?.clamp(0.0, 8.0) as usize;
                let start = child(o, "startOverride").and_then(|s| attr_f64(s, "val"))?.clamp(-1e9, 1e9) as i64;
                Some((lvl, start))
            })
            .collect();
        out.nums.insert(id.to_owned(), (levels.clone(), overrides));
    }
    Ok(out)
}

fn roman(mut n: i64) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
    let table = [
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
    ];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn letters(n: i64) -> String {
    if n < 1 {
        return n.to_string();
    }
    // Word repeats the letter: a … z, aa … zz.
    let n = n.min(780);
    let c = char::from(b'a' + ((n - 1) % 26) as u8);
    std::iter::repeat_n(c, ((n - 1) / 26 + 1) as usize).collect()
}

fn format_number(n: i64, format: &str) -> String {
    match format {
        "lowerLetter" => letters(n),
        "upperLetter" => letters(n).to_uppercase(),
        "lowerRoman" => roman(n),
        "upperRoman" => roman(n).to_uppercase(),
        "decimalZero" if (0..10).contains(&n) => format!("0{n}"),
        "none" => String::new(),
        _ => n.to_string(),
    }
}

impl Numbering {
    /// The label of the next item at `lvl` of list `id`, its paragraph and run properties.
    fn next(&mut self, id: &str, lvl: usize) -> Option<(String, PPr, RPr)> {
        let (levels, overrides) = self.nums.get(id)?;
        let level = levels.get(lvl)?.clone();
        let starts: Vec<i64> = (0..9)
            .map(|i| overrides.iter().find(|(l, _)| *l == i).map(|(_, s)| *s).or_else(|| levels.get(i).map(|l| l.start)).unwrap_or(1))
            .collect();
        let counters = self.counters.entry(id.to_owned()).or_insert([i64::MIN; 9]);
        let slot = counters.get_mut(lvl)?;
        *slot = if *slot == i64::MIN { starts.get(lvl).copied().unwrap_or(1) } else { slot.saturating_add(1) };
        for deeper in counters.iter_mut().skip(lvl + 1) {
            *deeper = i64::MIN;
        }
        let label = if level.format == "bullet" {
            level.text.clone()
        } else {
            let mut t = level.text.clone();
            for i in (0..9).rev() {
                let key = format!("%{}", i + 1);
                if t.contains(&key) {
                    let v = counters.get(i).copied().filter(|v| *v != i64::MIN).or_else(|| starts.get(i).copied()).unwrap_or(1);
                    let fmt = levels.get(i).map_or("decimal", |l| l.format.as_str());
                    t = t.replace(&key, &format_number(v, fmt));
                }
            }
            t
        };
        Some((label, level.p, level.r))
    }
}

// ── document model ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Cell {
    span: usize,
    paras: Vec<Para>,
    shade: Option<[f64; 3]>,
    borders: Borders,
    /// `Some(true)` starts a vertical merge, `Some(false)` continues one.
    vmerge: Option<bool>,
}

#[derive(Clone, Debug)]
struct Row {
    cells: Vec<Cell>,
    /// Minimum height, or the exact height when the bool is true.
    height: Option<(f64, bool)>,
}

#[derive(Clone, Debug)]
struct Table {
    cols: Vec<f64>,
    rows: Vec<Row>,
    borders: Borders,
    indent: f64,
    center: bool,
    pad: (f64, f64),
}

#[derive(Clone, Debug)]
enum Block {
    Para(Para),
    Table(Table),
    PageBreak,
}

#[derive(Clone, Debug)]
struct Section {
    w: f64,
    h: f64,
    margins: [f64; 4],
    header_dist: f64,
    footer_dist: f64,
    continuous: bool,
    header: Option<String>,
    footer: Option<String>,
    first_header: Option<String>,
    first_footer: Option<String>,
    title_page: bool,
}

impl Default for Section {
    fn default() -> Self {
        Section {
            w: 612.0,
            h: 792.0,
            margins: [72.0; 4],
            header_dist: 36.0,
            footer_dist: 36.0,
            continuous: false,
            header: None,
            footer: None,
            first_header: None,
            first_footer: None,
            title_page: false,
        }
    }
}

fn parse_section(n: Node<'_, '_>, prev: &Section) -> Section {
    let mut s = Section { continuous: false, title_page: false, first_header: None, first_footer: None, ..prev.clone() };
    if let Some(sz) = child(n, "pgSz") {
        let (w, h) = (attr_f64(sz, "w").map(twips), attr_f64(sz, "h").map(twips));
        if let (Some(w), Some(h)) = (w, h) {
            s.w = w;
            s.h = h;
            if attr(sz, "orient") == Some("landscape") && s.w < s.h {
                std::mem::swap(&mut s.w, &mut s.h);
            }
        }
    }
    if let Some(m) = child(n, "pgMar") {
        let get = |k: &str, d: f64| attr_f64(m, k).map_or(d, |v| twips(v.abs()));
        s.margins = [get("top", 72.0), get("right", 72.0), get("bottom", 72.0), get("left", 72.0)];
        s.header_dist = get("header", 36.0);
        s.footer_dist = get("footer", 36.0);
    }
    s.w = fin(s.w, 612.0).clamp(72.0, 14_400.0);
    s.h = fin(s.h, 792.0).clamp(72.0, 14_400.0);
    // Margins may not leave less than an inch of text area.
    let [t, r, b, l] = s.margins.map(|m| fin(m, 72.0).clamp(0.0, 7_000.0));
    let (l, r) = if l + r > s.w - 72.0 { (36.0f64.min(l), 36.0f64.min(r)) } else { (l, r) };
    let (t, b) = if t + b > s.h - 72.0 { (36.0f64.min(t), 36.0f64.min(b)) } else { (t, b) };
    s.margins = [t, r, b, l];
    s.continuous = child(n, "type").and_then(|t| attr(t, "val")) == Some("continuous");
    s.title_page = child(n, "titlePg").is_some_and(|t| flag(attr(t, "val")));
    for (tag, first, default) in [("headerReference", &mut s.first_header, &mut s.header), ("footerReference", &mut s.first_footer, &mut s.footer)] {
        for r in children(n, tag) {
            let Some(id) = rel_attr(r, "id") else { continue };
            match attr(r, "type") {
                Some("first") => *first = Some(id.to_owned()),
                Some("even") => {}
                _ => *default = Some(id.to_owned()),
            }
        }
    }
    s
}

// ── reading ─────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
enum FieldState {
    #[default]
    None,
    Instr(String),
    /// Showing the cached result; `true` when the field was replaced by a live page field.
    Result(bool),
}

struct Cx<'z, 'd> {
    zip: &'z Zip<'z>,
    doc: &'d mut Document,
    notes: &'d mut Notes,
    styles: Styles,
    numbering: Numbering,
    theme: Theme,
    images: HashMap<String, Option<ObjRef>>,
    /// Footnote and endnote references in order: (kind, id, label).
    note_refs: Vec<(&'static str, String, String)>,
    /// How many footnote and endnote references `note_refs` holds, for their labels.
    footnotes: usize,
    endnotes: usize,
    /// Reading the body (and its notes), not a header or footer: the body is laid out before
    /// the page count is known, so a NUMPAGES field there keeps the total Word saved.
    in_body: bool,
}

impl Cx<'_, '_> {
    fn image(&mut self, rels: &Rels, id: &str) -> Option<ObjRef> {
        if rels.is_external(id) {
            self.notes.add("linked (not embedded) images, drawn as empty boxes");
            return None;
        }
        let part = rels.target(id)?.to_owned();
        if let Some(cached) = self.images.get(&part) {
            return *cached;
        }
        let loaded = match self.zip.read(&part) {
            Ok(Some(bytes)) => crate::image_xobject(self.doc, &part, &bytes).ok().map(|(r, _)| r),
            _ => None,
        };
        if loaded.is_none() {
            self.notes.add("images in a format the importer can't draw (EMF, WMF, SVG…), drawn as empty boxes");
        }
        self.images.insert(part, loaded);
        loaded
    }

    fn para_props(&mut self, ppr: Option<Node<'_, '_>>) -> (PPr, RPr, Option<(String, RunStyle)>) {
        let direct = ppr.map(PPr::parse).unwrap_or_default();
        let (mut p, mut mark) = self.styles.para_style(direct.style.as_deref());
        // Numbering's indents sit between the style's and the paragraph's own.
        let num = direct.num.clone().or_else(|| p.num.clone());
        let mut label = None;
        if let Some((id, lvl)) = num
            && id != "0"
            && let Some((text, lp, lr)) = self.numbering.next(&id, lvl)
        {
            p.merge(&PPr { num: None, style: None, ..lp });
            let mut lstyle = mark.clone();
            lstyle.merge(&lr);
            if !text.is_empty() {
                let style = lstyle.style(&self.theme);
                label = Some((self.notes.drawable(&text), RunStyle { underline: false, strike: false, highlight: None, ..style }));
            }
        }
        p.merge(&direct);
        if let Some(r) = ppr.and_then(|n| child(n, "rPr")) {
            mark.merge(&RPr::parse(r));
        }
        (p, mark, label)
    }

    fn to_para(&self, p: &PPr, mark: &RPr) -> Para {
        Para {
            align: p.align.unwrap_or(Align::Left),
            left: p.left.unwrap_or(0.0),
            right: p.right.unwrap_or(0.0),
            first: p.first.unwrap_or(0.0),
            before: p.before.unwrap_or(0.0),
            after: p.after.unwrap_or(0.0),
            line: p.line.unwrap_or(LineRule::Auto(1.0)),
            tabs: p.tabs.iter().filter(|(_, c)| !c).map(|(t, _)| *t).collect(),
            base: mark.style(&self.theme),
            page_break_before: p.page_break_before.unwrap_or(false),
            shade: p.shade.flatten(),
            keep_next: p.keep_next.unwrap_or(false),
            contextual: p.contextual.unwrap_or(false).then(|| p.style.clone().or_else(|| self.styles.default_para.clone()).unwrap_or_default()),
            ..Para::default()
        }
    }

    /// Read a paragraph. Page breaks inside it split it into several blocks; text boxes'
    /// paragraphs follow it.
    fn paragraph(&mut self, n: Node<'_, '_>, rels: &Rels, depth: usize, out: &mut Vec<Block>) {
        let (p, mark, label) = self.para_props(child(n, "pPr"));
        let mut para = self.to_para(&p, &mark);
        para.label = label;
        let mut extra = Vec::new();
        let mut state = FieldState::None;
        let mut pieces: Vec<Block> = Vec::new();
        // Runs start from the paragraph style's run properties; the paragraph mark's own
        // formatting only styles the mark (and an empty paragraph's height).
        let (_, base) = self.styles.para_style(p.style.as_deref());
        self.inline(n, rels, &base, &mut para, &mut pieces, &mut extra, &mut state, depth);
        pieces.push(Block::Para(para));
        out.extend(pieces);
        out.extend(extra);
    }

    #[allow(clippy::too_many_arguments)]
    fn inline(
        &mut self,
        n: Node<'_, '_>,
        rels: &Rels,
        base: &RPr,
        para: &mut Para,
        out: &mut Vec<Block>,
        extra: &mut Vec<Block>,
        state: &mut FieldState,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            self.notes.add("content beyond the nesting limit, left out");
            return;
        }
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "r" => self.run(c, rels, base, para, out, extra, state, depth),
                "hyperlink" | "smartTag" | "ins" | "customXml" | "dir" | "bdo" | "moveTo" => {
                    self.inline(c, rels, base, para, out, extra, state, depth + 1)
                }
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.inline(content, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                "fldSimple" => {
                    let field = page_field(attr(c, "instr").unwrap_or_default()).filter(|f| *f == Field::Page || !self.in_body);
                    if let Some(field) = field {
                        para.items.push(Inline::Field(field, self.run_props(None, base)));
                    } else {
                        self.inline(c, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                "oMath" | "oMathPara" => {
                    let style = self.run_props(None, base);
                    let text = xml::text_of(c, "t");
                    if !text.is_empty() {
                        let text = self.notes.drawable(&text);
                        para.items.push(Inline::Text(text, RunStyle { italic: true, ..style }));
                        self.notes.add("equations, written as plain text");
                    }
                }
                _ => {}
            }
        }
    }

    fn run_props(&self, rpr: Option<Node<'_, '_>>, mark: &RPr) -> RunStyle {
        // The paragraph style's run properties, then the character style's, then direct ones.
        let mut r = mark.clone();
        r.style = None;
        let direct = rpr.map(RPr::parse).unwrap_or_default();
        if let Some(id) = &direct.style {
            r.merge(&self.styles.run_style(id));
        }
        r.merge(&direct);
        r.style(&self.theme)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        r: Node<'_, '_>,
        rels: &Rels,
        base: &RPr,
        para: &mut Para,
        out: &mut Vec<Block>,
        extra: &mut Vec<Block>,
        state: &mut FieldState,
        depth: usize,
    ) {
        let style = self.run_props(child(r, "rPr"), base);
        for c in r.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            match (name, &mut *state) {
                ("fldChar", _) => match attr(c, "fldCharType") {
                    Some("begin") => *state = FieldState::Instr(String::new()),
                    Some("separate") => {
                        let field = if let FieldState::Instr(i) = &*state { page_field(i) } else { None };
                        let field = field.filter(|f| *f == Field::Page || !self.in_body);
                        if let Some(field) = field {
                            para.items.push(Inline::Field(field, style.clone()));
                        }
                        let live = field.is_some();
                        *state = FieldState::Result(live);
                    }
                    Some("end") => {
                        // A field with no cached result: only page fields can still be shown.
                        if let FieldState::Instr(i) = &*state
                            && let Some(field) = page_field(i)
                        {
                            para.items.push(Inline::Field(field, style.clone()));
                        }
                        *state = FieldState::None;
                    }
                    _ => {}
                },
                ("instrText", FieldState::Instr(i)) if i.len() < 4096 => i.push_str(c.text().unwrap_or_default()),
                (_, FieldState::Instr(_)) | (_, FieldState::Result(true)) => {}
                ("t", _) => {
                    let text = self.notes.drawable(c.text().unwrap_or_default());
                    para.items.push(Inline::Text(text, style.clone()));
                }
                ("tab" | "ptab", _) => para.items.push(Inline::Tab),
                ("br" | "cr", _) => match attr(c, "type") {
                    Some("page") => {
                        // Content after a page break starts the next page.
                        let rest = Para { items: Vec::new(), before: 0.0, label: None, ..para.clone() };
                        let done = std::mem::replace(para, rest);
                        out.push(Block::Para(Para { after: 0.0, ..done }));
                        out.push(Block::PageBreak);
                    }
                    _ => para.items.push(Inline::Break),
                },
                ("noBreakHyphen", _) => para.items.push(Inline::Text("-".into(), style.clone())),
                ("sym", _) => {
                    let ch = attr(c, "char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32).unwrap_or('•');
                    let text = self.notes.drawable(&ch.to_string());
                    para.items.push(Inline::Text(text, style.clone()));
                }
                ("drawing", _) => self.drawing(c, rels, para, extra, depth),
                // Word 2010 and later wrap shapes and text boxes in markup compatibility: a
                // `Choice` with DrawingML and a `Fallback` with VML. One of them is read.
                ("AlternateContent", _) if depth < MAX_DEPTH => {
                    if let Some(pick) = child(c, "Choice").or_else(|| child(c, "Fallback")) {
                        self.run(pick, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                ("pict" | "object", _) => self.vml(c, rels, para, extra, depth),
                ("footnoteReference" | "endnoteReference", _) => {
                    let kind = if name == "footnoteReference" { "footnote" } else { "endnote" };
                    // A running count per kind: recounting the earlier references made each one cost
                    // as much as all before it.
                    let count = if kind == "footnote" { &mut self.footnotes } else { &mut self.endnotes };
                    let number = count.saturating_add(1);
                    let label = if kind == "endnote" { roman(i64::try_from(number).unwrap_or(1)) } else { number.to_string() };
                    if let Some(id) = attr(c, "id") {
                        self.note_refs.push((kind, id.to_owned(), label.clone()));
                        *count = number;
                    }
                    para.items.push(Inline::Text(label, RunStyle { vert: 1, ..style.clone() }));
                }
                _ => {}
            }
        }
    }

    /// A DrawingML object: a picture, a text box (its text joins the flow), or a placeholder.
    fn drawing(&mut self, d: Node<'_, '_>, rels: &Rels, para: &mut Para, extra: &mut Vec<Block>, depth: usize) {
        let Some(holder) = d.children().find(|c| is(*c, "inline") || is(*c, "anchor")) else { return };
        let extent = child(holder, "extent");
        let w = extent.and_then(|e| attr_f64(e, "cx")).map_or(72.0, |v| v / 12_700.0);
        let h = extent.and_then(|e| attr_f64(e, "cy")).map_or(72.0, |v| v / 12_700.0);
        let Some(data) = holder.descendants().find(|n| is(*n, "graphicData")) else { return };
        let uri = attr(data, "uri").unwrap_or_default();
        if let Some(blip) = data.descendants().find(|n| is(*n, "blip")) {
            let r = rel_attr(blip, "embed").and_then(|id| self.image(rels, id));
            if r.is_none() && rel_attr(blip, "link").is_some() {
                self.notes.add("linked (not embedded) images, drawn as empty boxes");
            }
            para.items.push(Inline::Image(r, w, h));
            return;
        }
        let boxes = outermost_boxes(data);
        if !boxes.is_empty() {
            self.notes.add("text boxes and shapes, their text placed in the flow");
            for b in boxes {
                self.blocks(b, rels, depth + 1, extra);
            }
            return;
        }
        if uri.ends_with("/chart") || uri.contains("chart") {
            self.notes.add("charts, drawn as empty boxes");
        } else if uri.contains("diagram") {
            self.notes.add("SmartArt diagrams, drawn as empty boxes");
        } else if uri.contains("wordprocessingShape") || uri.contains("wordprocessingGroup") || uri.contains("wordprocessingCanvas") {
            // A shape without text or picture: decoration the importer doesn't draw.
            self.notes.add("drawing shapes left out");
            return;
        } else {
            self.notes.add("embedded objects, drawn as empty boxes");
        }
        para.items.push(Inline::Image(None, w, h));
    }

    /// Legacy VML content (`w:pict`, `w:object`): an image preview or a text box.
    fn vml(&mut self, v: Node<'_, '_>, rels: &Rels, para: &mut Para, extra: &mut Vec<Block>, depth: usize) {
        if let Some(img) = v.descendants().find(|n| is(*n, "imagedata")) {
            let size = v.descendants().find(|n| attr(*n, "style").is_some()).and_then(|n| attr(n, "style")).map(vml_size).unwrap_or((72.0, 72.0));
            let r = rel_attr(img, "id").and_then(|id| self.image(rels, id));
            para.items.push(Inline::Image(r, size.0, size.1));
            return;
        }
        let boxes = outermost_boxes(v);
        if !boxes.is_empty() {
            self.notes.add("text boxes and shapes, their text placed in the flow");
            for b in boxes {
                self.blocks(b, rels, depth + 1, extra);
            }
        }
    }

    fn table(&mut self, t: Node<'_, '_>, rels: &Rels, depth: usize) -> Table {
        let tpr = child(t, "tblPr");
        let style_id = tpr.and_then(|p| child(p, "tblStyle")).and_then(|s| attr(s, "val"));
        let mut borders = self.styles.table_borders(style_id);
        if let Some(b) = tpr.and_then(|p| child(p, "tblBorders")) {
            borders.merge(&Borders::parse(b));
        }
        let cols: Vec<f64> = xml::path(t, &["tblGrid"])
            .map(|g| children(g, "gridCol").map(|c| attr_f64(c, "w").map_or(0.0, |w| twips(w).max(0.0))).collect())
            .unwrap_or_default();
        let indent = tpr.and_then(|p| child(p, "tblInd")).and_then(|i| attr_f64(i, "w")).map_or(0.0, twips);
        let center = tpr.and_then(|p| child(p, "jc")).and_then(|j| attr(j, "val")) == Some("center");
        let mar = tpr.and_then(|p| child(p, "tblCellMar"));
        let side = |names: [&str; 2]| mar.and_then(|m| names.iter().find_map(|n| child(m, n))).and_then(|s| attr_f64(s, "w")).map(twips);
        let pad = (side(["left", "start"]).unwrap_or(5.4), side(["right", "end"]).unwrap_or(5.4));
        let mut rows = Vec::new();
        for tr in children(t, "tr") {
            let height = xml::path(tr, &["trPr", "trHeight"]).and_then(|h| Some((twips(attr_f64(h, "val")?), attr(h, "hRule") == Some("exact"))));
            let mut cells = Vec::new();
            for tc in tr.children().filter(|c| is(*c, "tc") || is(*c, "sdt")) {
                let tc = if is(tc, "sdt") {
                    match xml::path(tc, &["sdtContent", "tc"]) {
                        Some(c) => c,
                        None => continue,
                    }
                } else {
                    tc
                };
                let pr = child(tc, "tcPr");
                let span = pr.and_then(|p| child(p, "gridSpan")).and_then(|g| attr_f64(g, "val")).map_or(1, |v| v.clamp(1.0, 64.0) as usize);
                let vmerge = pr.and_then(|p| child(p, "vMerge")).map(|v| attr(v, "val") == Some("restart"));
                let shade = pr.and_then(|p| child(p, "shd")).and_then(|s| attr(s, "fill")).filter(|f| *f != "auto").and_then(hex_color);
                let cb = pr.and_then(|p| child(p, "tcBorders")).map(Borders::parse).unwrap_or_default();
                let mut blocks = Vec::new();
                self.blocks(tc, rels, depth + 1, &mut blocks);
                let paras = blocks
                    .into_iter()
                    .flat_map(|b| match b {
                        Block::Para(p) => vec![p],
                        Block::Table(t) => {
                            self.notes.add("nested tables, their text placed in the cell");
                            t.rows.into_iter().flat_map(|r| r.cells.into_iter().flat_map(|c| c.paras)).collect()
                        }
                        Block::PageBreak => Vec::new(),
                    })
                    .collect();
                cells.push(Cell { span, paras, shade, borders: cb, vmerge });
            }
            rows.push(Row { cells, height });
        }
        Table { cols, rows, borders, indent, center, pad }
    }

    /// Read block content (a body, a cell, a header, a text box).
    fn blocks(&mut self, n: Node<'_, '_>, rels: &Rels, depth: usize, out: &mut Vec<Block>) {
        if depth > MAX_DEPTH {
            self.notes.add("content beyond the nesting limit, left out");
            return;
        }
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "p" => self.paragraph(c, rels, depth, out),
                "tbl" => {
                    let t = self.table(c, rels, depth);
                    out.push(Block::Table(t));
                }
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.blocks(content, rels, depth + 1, out);
                    }
                }
                "customXml" | "ins" | "moveTo" => self.blocks(c, rels, depth + 1, out),
                _ => {}
            }
        }
    }

    /// The paragraphs of a header or footer part.
    fn header(&mut self, rels: &Rels, id: Option<&str>) -> Vec<Block> {
        let Some(part) = id.and_then(|id| rels.target(id)).map(str::to_owned) else { return Vec::new() };
        let Ok(Some(text)) = self.zip.text(&part) else { return Vec::new() };
        let Ok(d) = xml::parse(&text) else { return Vec::new() };
        let Ok(hrels) = Rels::load(self.zip, &part) else { return Vec::new() };
        let mut out = Vec::new();
        self.blocks(d.root_element(), &hrels, 0, &mut out);
        out
    }

    /// Footnote or endnote paragraphs, keyed by id.
    fn note_bodies(&mut self, rels: &Rels, ty: &str, tag: &str) -> HashMap<String, Vec<Block>> {
        let mut out = HashMap::new();
        let Some(part) = rels.of_type(ty).map(str::to_owned) else { return out };
        let Ok(Some(text)) = self.zip.text(&part) else { return out };
        let Ok(d) = xml::parse(&text) else { return out };
        let Ok(nrels) = Rels::load(self.zip, &part) else { return out };
        for n in children(d.root_element(), tag) {
            if let Some(id) = attr(n, "id") {
                let mut blocks = Vec::new();
                self.blocks(n, &nrels, 0, &mut blocks);
                out.insert(id.to_owned(), blocks);
            }
        }
        out
    }
}

/// The text boxes in `n` that aren't inside another of its text boxes: an inner box is read with
/// the box holding it, so collecting it here too would place it once per level of nesting.
fn outermost_boxes<'a, 'i>(n: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    n.descendants().filter(|b| is(*b, "txbxContent") && !b.ancestors().skip(1).take_while(|a| *a != n).any(|a| is(a, "txbxContent"))).collect()
}

fn vml_size(style: &str) -> (f64, f64) {
    let get = |key: &str| {
        style.split(';').find_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            if k.trim() != key {
                return None;
            }
            let v = v.trim();
            let (num, unit) = v.find(|c: char| c.is_ascii_alphabetic()).map_or((v, "px"), |i| (v.get(..i).unwrap_or(v), v.get(i..).unwrap_or("")));
            let x: f64 = num.trim().parse().ok().filter(|x: &f64| x.is_finite())?;
            Some(match unit {
                "pt" => x,
                "in" => x * 72.0,
                "cm" => x * 72.0 / 2.54,
                "mm" => x * 72.0 / 25.4,
                _ => x * 0.75,
            })
        })
    };
    (get("width").unwrap_or(72.0).clamp(1.0, 14_400.0), get("height").unwrap_or(72.0).clamp(1.0, 14_400.0))
}

/// The page field a field instruction names, by its first word: `PAGEREF` (a table of contents'
/// or cross-reference's page number) and `PAGES…` look alike but are not page fields, and keep
/// their cached result.
fn page_field(instr: &str) -> Option<Field> {
    let name = instr.split_whitespace().next()?;
    if name.eq_ignore_ascii_case("PAGE") {
        Some(Field::Page)
    } else if name.eq_ignore_ascii_case("NUMPAGES") || name.eq_ignore_ascii_case("SECTIONPAGES") {
        Some(Field::Pages)
    } else {
        None
    }
}

// ── layout ──────────────────────────────────────────────────────────────────────────────────

struct Flow<'c> {
    canvas: &'c mut Canvas,
    sec: Section,
    /// Index of the page being filled, and the PDF y of the next line's top.
    page: usize,
    y: f64,
    at_top: bool,
    /// The section in force on each page.
    page_sections: Vec<usize>,
    section_index: usize,
    /// The previous paragraph's contextual-spacing style, and its space after (already added).
    prev_contextual: Option<(String, f64)>,
}

impl Flow<'_> {
    fn top(&self) -> f64 {
        self.sec.h - self.sec.margins[0]
    }

    fn bottom(&self) -> f64 {
        self.sec.margins[2]
    }

    fn x0(&self) -> f64 {
        self.sec.margins[3]
    }

    fn width(&self) -> f64 {
        (self.sec.w - self.sec.margins[1] - self.sec.margins[3]).max(36.0)
    }

    fn new_page(&mut self) -> Result<(), String> {
        self.page = self.canvas.new_page(self.sec.w, self.sec.h)?;
        self.page_sections.push(self.section_index);
        self.y = self.top();
        self.at_top = true;
        Ok(())
    }

    fn page_mut(&mut self) -> Option<&mut Page> {
        self.canvas.page(self.page)
    }

    /// Height of a block's first line (with its space before), for keep-with-next.
    fn first_line(&self, b: Option<&Block>) -> f64 {
        match b {
            Some(Block::Para(p)) => {
                let l = flow::layout(p, self.width(), (1, 1));
                l.before + l.lines.first().map_or(0.0, |l| l.height)
            }
            Some(Block::Table(_)) => 20.0,
            _ => 0.0,
        }
    }

    fn para(&mut self, p: &Para, next: Option<&Block>) -> Result<(), String> {
        if p.page_break_before && !self.at_top {
            self.new_page()?;
        }
        // A PAGE field shows the page the paragraph starts on (the page count isn't known yet).
        let laid = flow::layout(p, self.width(), (self.page.saturating_add(1), 1));
        // Contextual spacing: no space between paragraphs of the same style.
        let mut before = laid.before;
        if let (Some(style), Some((prev, prev_after))) = (&p.contextual, &self.prev_contextual)
            && style == prev
        {
            // The previous paragraph's space after was left on the page it ended; a paragraph
            // starting a new page has none to take back.
            if !self.at_top {
                self.y += prev_after;
            }
            before = 0.0;
        }
        self.prev_contextual = None;
        // Keep with next: move to a new page rather than strand this paragraph at the bottom.
        if p.keep_next && !self.at_top {
            let need = before + laid.lines.iter().map(|l| l.height).sum::<f64>() + laid.after + self.first_line(next);
            if self.y - need < self.bottom() && need <= self.top() - self.bottom() {
                self.new_page()?;
            }
        }
        if !self.at_top {
            self.y -= before;
        }
        let x0 = self.x0();
        for line in &laid.lines {
            if self.y - line.height < self.bottom() - 0.5 && !self.at_top {
                self.new_page()?;
            }
            let y = self.y;
            if let Some(c) = laid.shade
                && let Some(page) = self.page_mut()
            {
                page.fill_rect(x0 + laid.left, y - line.height, laid.right - laid.left, line.height, c);
            }
            if let Some(page) = self.page_mut() {
                flow::draw_line(page, &laid, line, x0, y);
            }
            self.y -= line.height;
            self.at_top = false;
        }
        self.y -= laid.after;
        self.prev_contextual = p.contextual.clone().map(|s| (s, laid.after));
        Ok(())
    }

    fn table(&mut self, t: &Table) -> Result<(), String> {
        self.prev_contextual = None;
        let area = self.width();
        let ncols = t.cols.len().max(t.rows.iter().map(|r| r.cells.iter().map(|c| c.span).sum::<usize>()).max().unwrap_or(1)).max(1);
        let mut cols = t.cols.clone();
        cols.resize(ncols, 0.0);
        let total: f64 = cols.iter().sum();
        if total <= 1.0 {
            cols = vec![area / ncols as f64; ncols];
        } else if total > area - t.indent.max(0.0) {
            let k = (area - t.indent.max(0.0)).max(36.0) / total;
            cols.iter_mut().for_each(|c| *c *= k);
        }
        let table_w: f64 = cols.iter().sum();
        let x_table = self.x0() + if t.center { ((area - table_w) / 2.0).max(0.0) } else { t.indent.clamp(-self.x0(), area) };
        let edges: Vec<f64> = std::iter::once(0.0)
            .chain(cols.iter().scan(0.0, |acc, c| {
                *acc += c;
                Some(*acc)
            }))
            .collect();

        // Grid position of each cell, for vertical merges.
        let grid: Vec<Vec<usize>> = t
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .scan(0usize, |g, c| {
                        let at = *g;
                        *g = g.saturating_add(c.span);
                        Some(at)
                    })
                    .collect()
            })
            .collect();

        for (ri, row) in t.rows.iter().enumerate() {
            // Lay out each cell's paragraphs into (height, paragraph, line) items.
            let mut cells: Vec<CellLines> = Vec::new();
            for (ci, cell) in row.cells.iter().enumerate() {
                let g = grid.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(0);
                let x1 = edges.get(g).copied().unwrap_or(0.0);
                let x2 = edges.get(g.saturating_add(cell.span).min(ncols)).copied().unwrap_or(table_w);
                let inner = (x2 - x1 - t.pad.0 - t.pad.1).max(4.0);
                let laids: Vec<Laid> = if cell.vmerge == Some(false) {
                    Vec::new()
                } else {
                    cell.paras.iter().map(|p| flow::layout(p, inner, (self.page.saturating_add(1), 1))).collect()
                };
                let mut items = Vec::new();
                for (pi, l) in laids.iter().enumerate() {
                    let same = |a: usize, b: usize| matches!((cell.paras.get(a).and_then(|p| p.contextual.as_ref()), cell.paras.get(b).and_then(|p| p.contextual.as_ref())), (Some(x), Some(y)) if x == y);
                    if pi > 0 && l.before > 0.0 && !same(pi - 1, pi) {
                        items.push((l.before, None));
                    }
                    for (li, line) in l.lines.iter().enumerate() {
                        items.push((line.height, Some((pi, li))));
                    }
                    if pi + 1 < laids.len() && l.after > 0.0 && !same(pi, pi + 1) {
                        items.push((l.after, None));
                    }
                }
                cells.push((laids, items));
            }
            let content = |items: &[(f64, Option<(usize, usize)>)]| items.iter().map(|(h, _)| *h).sum::<f64>() + 2.0;
            let mut row_h = cells.iter().map(|(_, items)| content(items)).fold(0.0, f64::max);
            if let Some((h, exact)) = row.height {
                let h = fin(h, 0.0).clamp(0.0, 10_000.0);
                row_h = if exact { h } else { row_h.max(h) };
            }
            let full = self.top() - self.bottom();
            if self.y - row_h < self.bottom() && !self.at_top && row_h <= full {
                self.new_page()?;
            }
            // Draw the row, continuing it on new pages while it doesn't fit.
            let mut cursors = vec![0usize; cells.len()];
            let mut first_part = true;
            loop {
                let avail = self.y - self.bottom();
                let remaining_h = row_h.min(
                    cells
                        .iter()
                        .zip(&cursors)
                        .map(|((_, items), c)| content(items.get(*c..).unwrap_or_default()))
                        .fold(if first_part { row_h } else { 0.0 }, f64::max),
                );
                let part_h = if remaining_h <= avail || self.at_top && avail <= 0.0 { remaining_h } else { avail.max(0.0) };
                let y_top = self.y;
                // On a fresh page each cell places at least one item even if it is taller than
                // the page (it is cut at the bottom), so a row always moves on.
                let fresh = self.at_top;
                // A row's grid positions strictly increase (every span is at least 1), so the
                // cell below is found by binary search rather than a scan per cell.
                let next_vmerge = |g: usize| {
                    let below = ri.checked_add(1);
                    let (Some(nr), Some(ng)) = (below.and_then(|b| t.rows.get(b)), below.and_then(|b| grid.get(b))) else { return false };
                    ng.binary_search(&g).ok().and_then(|i| nr.cells.get(i)).is_some_and(|c| c.vmerge == Some(false))
                };
                let mut done = true;
                for (ci, cell) in row.cells.iter().enumerate() {
                    let g = grid.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(0);
                    let x1 = x_table + edges.get(g).copied().unwrap_or(0.0);
                    let x2 = x_table + edges.get(g.saturating_add(cell.span).min(ncols)).copied().unwrap_or(table_w);
                    let page = match self.canvas.page(self.page) {
                        Some(p) => p,
                        None => break,
                    };
                    if let Some(c) = cell.shade {
                        page.fill_rect(x1, y_top - part_h, x2 - x1, part_h, c);
                    }
                    let Some((laids, items)) = cells.get(ci) else { continue };
                    let Some(cursor) = cursors.get_mut(ci) else { continue };
                    let mut y = y_top - 1.0;
                    let mut placed = 0usize;
                    while let Some((h, what)) = items.get(*cursor) {
                        let fits = y - h >= y_top - part_h - 0.01;
                        let forced = fresh && placed == 0;
                        if !(fits || forced) {
                            break;
                        }
                        placed += 1;
                        if let Some((pi, li)) = what
                            && let Some(l) = laids.get(*pi)
                            && let Some(line) = l.lines.get(*li)
                        {
                            flow::draw_line(page, l, line, x1 + t.pad.0, y);
                        }
                        y -= h;
                        *cursor += 1;
                    }
                    if *cursor < items.len() {
                        done = false;
                    }
                    // Borders: the cell's own, else the table's outer or inside ones.
                    let first_col = g == 0;
                    let last_col = g.saturating_add(cell.span) >= ncols;
                    let first_row = ri == 0;
                    let last_row = ri + 1 == t.rows.len();
                    let pick = |own: Option<Option<Border>>, outer: Option<Option<Border>>, inside: Option<Option<Border>>, is_outer: bool| {
                        own.unwrap_or(if is_outer { outer.flatten() } else { inside.flatten() })
                    };
                    let continues_below = next_vmerge(g);
                    let top = if cell.vmerge == Some(false) || !first_part {
                        None
                    } else {
                        pick(cell.borders.top, t.borders.top, t.borders.inside_h, first_row)
                    };
                    let bottom =
                        if continues_below || !done { None } else { pick(cell.borders.bottom, t.borders.bottom, t.borders.inside_h, last_row) };
                    let left = pick(cell.borders.left, t.borders.left, t.borders.inside_v, first_col);
                    let right = pick(cell.borders.right, t.borders.right, t.borders.inside_v, last_col);
                    let yb = y_top - part_h;
                    if let Some(b) = top {
                        page.line(x1, y_top, x2, y_top, b.width, b.color);
                    }
                    if let Some(b) = bottom {
                        page.line(x1, yb, x2, yb, b.width, b.color);
                    }
                    if let Some(b) = left {
                        page.line(x1, y_top, x1, yb, b.width, b.color);
                    }
                    if let Some(b) = right {
                        page.line(x2, y_top, x2, yb, b.width, b.color);
                    }
                }
                self.y = y_top - part_h;
                self.at_top = false;
                if done || part_h >= remaining_h {
                    break;
                }
                self.new_page()?;
                first_part = false;
            }
        }
        // Word leaves no gap after a table; the next paragraph's spacing applies.
        Ok(())
    }
}

pub(crate) fn convert(zip: &Zip<'_>, main: &str, doc: &mut Document, notes: &mut Notes) -> Result<usize, String> {
    let rels = Rels::load(zip, main)?;
    let text = zip.text(main)?.ok_or_else(|| format!("{main} is missing"))?;
    let xd = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let body = child(xd.root_element(), "body").ok_or("the document has no body")?;
    let theme = load_theme(zip, &rels);
    let styles = load_styles(zip, &rels)?;
    let numbering = load_numbering(zip, &rels)?;
    let mut cx =
        Cx { zip, doc, notes, styles, numbering, theme, images: HashMap::new(), note_refs: Vec::new(), footnotes: 0, endnotes: 0, in_body: true };

    // Split the body into sections: a paragraph carrying a sectPr ends one.
    let final_sect = child(body, "sectPr");
    let mut sections: Vec<(Section, Vec<Block>)> = Vec::new();
    let mut current: Vec<Block> = Vec::new();
    let mut prev = Section::default();
    for c in body.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "p" => {
                cx.paragraph(c, &rels, 0, &mut current);
                if let Some(s) = xml::path(c, &["pPr", "sectPr"]) {
                    let sec = parse_section(s, &prev);
                    prev = sec.clone();
                    sections.push((sec, std::mem::take(&mut current)));
                }
            }
            "tbl" => {
                let t = cx.table(c, &rels, 0);
                current.push(Block::Table(t));
            }
            "sdt" | "customXml" | "ins" | "moveTo" => {
                let inner = if is(c, "sdt") { child(c, "sdtContent") } else { Some(c) };
                if let Some(inner) = inner {
                    cx.blocks(inner, &rels, 1, &mut current);
                }
            }
            _ => {}
        }
    }
    let last = final_sect.map(|s| parse_section(s, &prev)).unwrap_or(prev);
    sections.push((last, current));

    // Footnotes and endnotes go after the body, each with its reference mark.
    if !cx.note_refs.is_empty() {
        let refs = std::mem::take(&mut cx.note_refs);
        let foot = cx.note_bodies(&rels, "footnotes", "footnote");
        let end = cx.note_bodies(&rels, "endnotes", "endnote");
        cx.notes.add("footnotes and endnotes, placed at the end of the document");
        if let Some((_, blocks)) = sections.last_mut() {
            let rule = Para {
                items: vec![Inline::Text("______________".into(), RunStyle { color: [0.4; 3], ..RunStyle::default() })],
                before: 12.0,
                after: 6.0,
                ..Para::default()
            };
            blocks.push(Block::Para(rule));
            // Each note once, however often it is referenced: a long note referenced again and
            // again would otherwise be copied once per reference.
            let mut placed = std::collections::HashSet::new();
            for (kind, id, label) in refs {
                let body = if kind == "footnote" { foot.get(&id) } else { end.get(&id) };
                let Some(body) = body else { continue };
                if !placed.insert((kind, id)) {
                    continue;
                }
                for (i, b) in body.iter().enumerate() {
                    match b {
                        Block::Para(p) if i == 0 => {
                            let mut p = p.clone();
                            let st =
                                p.items.iter().find_map(|it| if let Inline::Text(_, s) = it { Some(s.clone()) } else { None }).unwrap_or_default();
                            p.items.insert(0, Inline::Text(format!("{label} "), RunStyle { vert: 1, ..st }));
                            blocks.push(Block::Para(p));
                        }
                        other => blocks.push(other.clone()),
                    }
                }
            }
        }
    }

    // Lay the body out.
    let mut canvas = Canvas::default();
    let first = sections.first().map(|(s, _)| s.clone()).unwrap_or_default();
    let mut fl =
        Flow { canvas: &mut canvas, sec: first, page: 0, y: 0.0, at_top: true, page_sections: Vec::new(), section_index: 0, prev_contextual: None };
    fl.new_page()?;
    for (si, (sec, blocks)) in sections.iter().enumerate() {
        if si > 0 {
            let same_size = (fl.sec.w - sec.w).abs() < 0.5 && (fl.sec.h - sec.h).abs() < 0.5;
            fl.sec = sec.clone();
            fl.section_index = si;
            if !(sec.continuous && same_size) || fl.at_top {
                if !fl.at_top {
                    fl.new_page()?;
                } else {
                    // The empty page takes this section's size and starts at its top margin.
                    if let Some(s) = fl.page_sections.last_mut() {
                        *s = si;
                    }
                    fl.canvas.resize(fl.page, fl.sec.w, fl.sec.h);
                    fl.y = fl.top();
                }
            }
        }
        for (bi, b) in blocks.iter().enumerate() {
            match b {
                Block::Para(p) => fl.para(p, blocks.get(bi + 1))?,
                Block::Table(t) => fl.table(t)?,
                Block::PageBreak => fl.new_page()?,
            }
        }
    }
    let page_sections = std::mem::take(&mut fl.page_sections);
    drop(fl);

    // Headers and footers, now that the page count is known.
    let total = canvas.pages.len();
    let mut cache: HashMap<Option<String>, Vec<Block>> = HashMap::new();
    let mut section_first_page: Vec<Option<usize>> = vec![None; sections.len()];
    for (pi, si) in page_sections.iter().enumerate() {
        if let Some(slot) = section_first_page.get_mut(*si)
            && slot.is_none()
        {
            *slot = Some(pi);
        }
    }
    // Headers and footers are laid out per page, with the page count known.
    cx.in_body = false;
    for (pi, si) in page_sections.iter().enumerate() {
        let Some((sec, _)) = sections.get(*si) else { continue };
        let first_of_section = section_first_page.get(*si).copied().flatten() == Some(pi);
        let use_first = sec.title_page && first_of_section;
        for footer in [false, true] {
            let id = match (footer, use_first) {
                (false, true) => sec.first_header.clone(),
                (false, false) => sec.header.clone(),
                (true, true) => sec.first_footer.clone(),
                (true, false) => sec.footer.clone(),
            };
            if id.is_none() {
                continue;
            }
            if !cache.contains_key(&id) {
                let blocks = cx.header(&rels, id.as_deref());
                cache.insert(id.clone(), blocks);
            }
            let Some(blocks) = cache.get(&id) else { continue };
            let width = (sec.w - sec.margins[1] - sec.margins[3]).max(36.0);
            let laids: Vec<Laid> = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Para(p) => Some(flow::layout(p, width, (pi + 1, total))),
                    Block::Table(t) => {
                        let text: Vec<Inline> = t
                            .rows
                            .iter()
                            .flat_map(|r| {
                                r.cells.iter().flat_map(|c| c.paras.iter().flat_map(|p| p.items.clone()).chain(std::iter::once(Inline::Tab)))
                            })
                            .collect();
                        Some(flow::layout(&Para { items: text, ..Para::default() }, width, (pi + 1, total)))
                    }
                    Block::PageBreak => None,
                })
                .collect();
            let height: f64 = laids.iter().map(Laid::height).sum();
            let Some(page) = canvas.page(pi) else { continue };
            let mut y = if footer { sec.footer_dist + height } else { sec.h - sec.header_dist };
            for l in &laids {
                y -= flow::draw(page, l, sec.margins[3], y);
            }
        }
    }
    canvas.finish(cx.doc).map_err(|e| e.to_string())
}
