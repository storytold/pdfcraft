//! PowerPoint (PresentationML, ECMA-376 Part 1 §19, with DrawingML §20) to PDF: one page per
//! visible slide, at the slide size. Backgrounds, shapes (fills, outlines, preset and custom
//! geometry), pictures (with cropping), text (with placeholder inheritance from the layout and
//! master, theme colours and fonts, bullets and numbering, autofit scaling), groups, tables and
//! connectors are drawn. Master and layout decorations are drawn under the slide's own shapes.

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef};
use roxmltree::Node;

use super::canvas::{Canvas, Page, n};
use super::flow::{self, Align, Field, Inline, Laid, LineRule, Para, fin};
use super::xml::{self, Rels, attr, attr_f64, child, children, flag, is, rel_attr};
use super::zip::Zip;
use super::{Notes, RunStyle, family_of, hex_color};

const EMU: f64 = 12_700.0;
const MAX_DEPTH: usize = 24;
/// The shapes drawn on one slide, its master's and layout's included.
const MAX_SHAPES: usize = 20_000;
/// The shapes drawn across the whole deck (a master's shapes count again on every slide).
const MAX_DECK_SHAPES: usize = 500_000;
/// A `frame`'s border width and a `donut`'s ring width, as fractions of the shape's shorter side:
/// the presets' default adjust values (12500 and 25000 per 100000). Set adjust values are not read.
const FRAME_ADJ: f64 = 0.125;
const DONUT_ADJ: f64 = 0.25;

// ── colours ─────────────────────────────────────────────────────────────────────────────────

struct Colors {
    scheme: HashMap<String, [f64; 3]>,
    map: HashMap<String, String>,
    major: String,
    minor: String,
}

impl Colors {
    fn load(theme: Option<Node<'_, '_>>, master: Option<Node<'_, '_>>) -> Colors {
        let mut c = Colors { scheme: HashMap::new(), map: HashMap::new(), major: "Calibri Light".into(), minor: "Calibri".into() };
        if let Some(t) = theme {
            if let Some(cs) = t.descendants().find(|n| is(*n, "clrScheme")) {
                for e in cs.children().filter(Node::is_element) {
                    if let Some(col) = e.children().find(Node::is_element).and_then(|v| base_color(v, &c, None)) {
                        c.scheme.insert(e.tag_name().name().to_owned(), col);
                    }
                }
            }
            for (name, slot) in [("majorFont", 0), ("minorFont", 1)] {
                if let Some(face) =
                    t.descendants().find(|n| is(*n, name)).and_then(|f| child(f, "latin")).and_then(|l| attr(l, "typeface")).filter(|f| !f.is_empty())
                {
                    if slot == 0 {
                        c.major = face.to_owned();
                    } else {
                        c.minor = face.to_owned();
                    }
                }
            }
        }
        let defaults = [("bg1", "lt1"), ("tx1", "dk1"), ("bg2", "lt2"), ("tx2", "dk2")];
        for (k, v) in defaults {
            c.map.insert(k.into(), v.into());
        }
        if let Some(m) = master.and_then(|m| child(m, "clrMap")) {
            for a in m.attributes() {
                c.map.insert(a.name().to_owned(), a.value().to_owned());
            }
        }
        c
    }

    fn scheme(&self, name: &str) -> Option<[f64; 3]> {
        let mapped = self.map.get(name).map_or(name, String::as_str);
        self.scheme.get(mapped).or_else(|| self.scheme.get(name)).copied()
    }

    fn font(&self, face: &str) -> String {
        match face {
            "+mj-lt" | "+mj-ea" | "+mj-cs" => self.major.clone(),
            "+mn-lt" | "+mn-ea" | "+mn-cs" => self.minor.clone(),
            f => f.to_owned(),
        }
    }
}

fn preset_color(name: &str) -> Option<[f64; 3]> {
    Some(match name {
        "black" => [0.0; 3],
        "white" => [1.0; 3],
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 0.5, 0.0],
        "lime" => [0.0, 1.0, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        "yellow" => [1.0, 1.0, 0.0],
        "gray" | "grey" => [0.5; 3],
        "lightGray" | "ltGray" => [0.83; 3],
        "darkGray" | "dkGray" => [0.66; 3],
        "orange" => [1.0, 0.65, 0.0],
        "navy" => [0.0, 0.0, 0.5],
        _ => return None,
    })
}

fn rgb_to_hsl([r, g, b]: [f64; 3]) -> [f64; 3] {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-9 {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, s, l]
}

fn hsl_to_rgb([h, s, l]: [f64; 3]) -> [f64; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// A colour element (`a:srgbClr`, `a:schemeClr`…) with its modifiers.
fn base_color(v: Node<'_, '_>, c: &Colors, ph: Option<[f64; 3]>) -> Option<[f64; 3]> {
    let pct = |name: &str| v.children().find(|m| is(*m, name)).and_then(|m| attr_f64(m, "val")).map(|x| x / 100_000.0);
    let mut col = match v.tag_name().name() {
        "srgbClr" => hex_color(attr(v, "val")?)?,
        "schemeClr" => match attr(v, "val")? {
            "phClr" => ph.unwrap_or([0.0; 3]),
            s => c.scheme(s)?,
        },
        "sysClr" => attr(v, "lastClr").and_then(hex_color).unwrap_or(if attr(v, "val") == Some("window") { [1.0; 3] } else { [0.0; 3] }),
        "prstClr" => preset_color(attr(v, "val")?)?,
        "scrgbClr" => {
            let g = |k: &str| attr_f64(v, k).map_or(0.0, |x| (x / 100_000.0).clamp(0.0, 1.0).powf(1.0 / 2.2));
            [g("r"), g("g"), g("b")]
        }
        "hslClr" => {
            let h = attr_f64(v, "hue").unwrap_or(0.0) / 21_600_000.0;
            let s = attr_f64(v, "sat").unwrap_or(0.0) / 100_000.0;
            let l = attr_f64(v, "lum").unwrap_or(0.0) / 100_000.0;
            hsl_to_rgb([h, s, l])
        }
        _ => return None,
    };
    if let Some(t) = pct("tint") {
        col = col.map(|x| x * t + (1.0 - t));
    }
    if let Some(s) = pct("shade") {
        col = col.map(|x| x * s);
    }
    if pct("lumMod").is_some() || pct("lumOff").is_some() {
        let mut hsl = rgb_to_hsl(col);
        hsl[2] = hsl[2] * pct("lumMod").unwrap_or(1.0) + pct("lumOff").unwrap_or(0.0);
        col = hsl_to_rgb(hsl);
    }
    Some(col.map(|x| fin(x, 0.0).clamp(0.0, 1.0)))
}

/// The colour of a fill-like element's first colour child (`a:solidFill`, a style reference…).
fn color_in(n: Node<'_, '_>, c: &Colors, ph: Option<[f64; 3]>) -> Option<[f64; 3]> {
    n.children().filter(Node::is_element).find_map(|v| base_color(v, c, ph))
}

// ── text styles ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Bullet {
    None,
    Char(String),
    Auto(String, i64),
}

#[derive(Clone, Default, Debug)]
struct Lvl {
    algn: Option<Align>,
    mar_l: Option<f64>,
    indent: Option<f64>,
    line: Option<LineRule>,
    before: Option<Spacing>,
    after: Option<Spacing>,
    bullet: Option<Bullet>,
    bullet_color: Option<[f64; 3]>,
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<[f64; 3]>,
    font: Option<String>,
    caps: Option<bool>,
    baseline: Option<i8>,
}

#[derive(Clone, Copy, Debug)]
enum Spacing {
    Points(f64),
    Percent(f64),
}

impl Lvl {
    fn merge(&mut self, o: &Lvl) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(algn, mar_l, indent, line, before, after, bullet, bullet_color, size, bold, italic, underline, strike, color, font, caps, baseline);
    }

    /// Run properties (`a:rPr`, `a:defRPr`, `a:endParaRPr`) into this level.
    fn run_props(&mut self, r: Node<'_, '_>, c: &Colors) {
        if let Some(v) = attr_f64(r, "sz") {
            self.size = Some(v / 100.0);
        }
        if let Some(v) = attr(r, "b") {
            self.bold = Some(flag(Some(v)));
        }
        if let Some(v) = attr(r, "i") {
            self.italic = Some(flag(Some(v)));
        }
        if let Some(v) = attr(r, "u") {
            self.underline = Some(v != "none");
        }
        if let Some(v) = attr(r, "strike") {
            self.strike = Some(v != "noStrike");
        }
        if let Some(v) = attr(r, "cap") {
            self.caps = Some(v == "all");
        }
        if let Some(v) = attr_f64(r, "baseline") {
            self.baseline = Some(if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            });
        }
        for e in r.children().filter(Node::is_element) {
            match e.tag_name().name() {
                "solidFill" => self.color = color_in(e, c, None).or(self.color),
                "latin" => {
                    if let Some(f) = attr(e, "typeface") {
                        self.font = Some(c.font(f));
                    }
                }
                _ => {}
            }
        }
    }

    /// Paragraph properties (`a:pPr`, `a:lvlNpPr`) into this level, with their `a:defRPr`.
    fn para_props(&mut self, p: Node<'_, '_>, c: &Colors) {
        if let Some(v) = attr(p, "algn") {
            self.algn = Some(match v {
                "ctr" => Align::Center,
                "r" => Align::Right,
                "just" | "dist" | "justLow" | "thaiDist" => Align::Justify,
                _ => Align::Left,
            });
        }
        if let Some(v) = attr_f64(p, "marL") {
            self.mar_l = Some(v / EMU);
        }
        if let Some(v) = attr_f64(p, "indent") {
            self.indent = Some(v / EMU);
        }
        for e in p.children().filter(Node::is_element) {
            let spacing = |e: Node<'_, '_>| {
                if let Some(v) = child(e, "spcPts").and_then(|s| attr_f64(s, "val")) {
                    Some(Spacing::Points(v / 100.0))
                } else {
                    child(e, "spcPct").and_then(|s| attr_f64(s, "val")).map(|v| Spacing::Percent(v / 100_000.0))
                }
            };
            match e.tag_name().name() {
                "lnSpc" => {
                    self.line = spacing(e).map(|s| match s {
                        Spacing::Points(p) => LineRule::Exact(p),
                        Spacing::Percent(p) => LineRule::Auto(p),
                    })
                }
                "spcBef" => self.before = spacing(e),
                "spcAft" => self.after = spacing(e),
                "buNone" => self.bullet = Some(Bullet::None),
                "buChar" => self.bullet = Some(Bullet::Char(attr(e, "char").unwrap_or("•").to_owned())),
                "buAutoNum" => {
                    let start = attr_f64(e, "startAt").unwrap_or(1.0).clamp(-1e6, 1e6) as i64;
                    self.bullet = Some(Bullet::Auto(attr(e, "type").unwrap_or("arabicPeriod").to_owned(), start));
                }
                "buClr" => self.bullet_color = color_in(e, c, None),
                "defRPr" => self.run_props(e, c),
                _ => {}
            }
        }
    }
}

/// Nine levels of a list style (`a:lstStyle`, `p:titleStyle`…).
#[derive(Clone, Default, Debug)]
struct Levels([Lvl; 9]);

impl Levels {
    fn parse(n: Node<'_, '_>, c: &Colors) -> Levels {
        let mut l = Levels::default();
        for (i, slot) in l.0.iter_mut().enumerate() {
            if let Some(p) = child(n, &format!("lvl{}pPr", i + 1)) {
                slot.para_props(p, c);
            }
        }
        l
    }

    fn merge(&mut self, o: &Levels) {
        for (a, b) in self.0.iter_mut().zip(o.0.iter()) {
            a.merge(b);
        }
    }
}

// ── shapes ──────────────────────────────────────────────────────────────────────────────────

/// Maps a shape's EMU coordinates to slide EMU coordinates (group transforms).
#[derive(Clone, Copy, Debug)]
struct Xf {
    sx: f64,
    sy: f64,
    tx: f64,
    ty: f64,
}

impl Xf {
    const ID: Xf = Xf { sx: 1.0, sy: 1.0, tx: 0.0, ty: 0.0 };

    fn rect(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        (self.tx + x * self.sx, self.ty + y * self.sy, w * self.sx, h * self.sy)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    rot: f64,
    flip_h: bool,
    flip_v: bool,
}

fn frame_of(xfrm: Node<'_, '_>) -> Option<Frame> {
    let off = child(xfrm, "off")?;
    let ext = child(xfrm, "ext")?;
    Some(Frame {
        x: attr_f64(off, "x").unwrap_or(0.0),
        y: attr_f64(off, "y").unwrap_or(0.0),
        w: attr_f64(ext, "cx").unwrap_or(0.0).max(0.0),
        h: attr_f64(ext, "cy").unwrap_or(0.0).max(0.0),
        rot: attr_f64(xfrm, "rot").unwrap_or(0.0) / 60_000.0,
        flip_h: flag(attr(xfrm, "flipH")) && attr(xfrm, "flipH").is_some(),
        flip_v: flag(attr(xfrm, "flipV")) && attr(xfrm, "flipV").is_some(),
    })
}

/// A placeholder's type and index (`p:nvPr/p:ph`).
fn placeholder(sp: Node<'_, '_>) -> Option<(String, Option<String>)> {
    let ph = sp
        .children()
        .find(|c| c.is_element() && c.tag_name().name().starts_with("nv"))
        .and_then(|nv| child(nv, "nvPr"))
        .and_then(|p| child(p, "ph"))?;
    Some((attr(ph, "type").unwrap_or("body").to_owned(), attr(ph, "idx").map(str::to_owned)))
}

fn ph_kind(ty: &str) -> &str {
    match ty {
        "ctrTitle" | "title" => "title",
        "subTitle" | "body" | "obj" => "body",
        t => t,
    }
}

/// A layout's or master's placeholders, found in one walk of its shape tree: the first of each
/// index and the first of each kind. A slide looks its placeholders up here for every shape, so a
/// large inherited tree is walked once per slide, not once per lookup.
#[derive(Default)]
struct Placeholders<'a, 'i> {
    by_idx: HashMap<String, Node<'a, 'i>>,
    by_kind: HashMap<String, Node<'a, 'i>>,
}

impl<'a, 'i> Placeholders<'a, 'i> {
    fn of(tree: Option<Node<'a, 'i>>) -> Self {
        let mut p = Placeholders::default();
        for n in tree.into_iter().flat_map(|t| t.descendants().filter(|n| is(*n, "sp"))) {
            let Some((ty, idx)) = placeholder(n) else { continue };
            if let Some(i) = idx {
                p.by_idx.entry(i).or_insert(n);
            }
            p.by_kind.entry(ph_kind(&ty).to_owned()).or_insert(n);
        }
        p
    }

    /// The matching placeholder: by index, else by type.
    fn find(&self, ty: &str, idx: Option<&str>) -> Option<Node<'a, 'i>> {
        idx.and_then(|i| self.by_idx.get(i)).or_else(|| self.by_kind.get(ph_kind(ty))).copied()
    }
}

struct Inherit<'a, 'i, 'p> {
    layout: Option<Node<'a, 'i>>,
    master: Option<Node<'a, 'i>>,
    layout_ph: &'p Placeholders<'a, 'i>,
    master_ph: &'p Placeholders<'a, 'i>,
    title: Levels,
    body: Levels,
    other: Levels,
}

struct Cx<'z, 'd> {
    zip: &'z Zip<'z>,
    doc: &'d mut Document,
    notes: &'d mut Notes,
    images: HashMap<String, Option<ObjRef>>,
    slide_no: usize,
    slides: usize,
    /// Shapes met on this slide and in the whole deck, and whether this slide was cut short.
    shapes: usize,
    deck_shapes: usize,
    cut: bool,
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
}

/// Slide EMU to PDF points.
struct Space {
    h: f64,
}

impl Space {
    fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        (x / EMU, self.h - y / EMU)
    }
}

/// A shape's outline as path operators, and whether it is filled even-odd (it has a hole).
fn path_ops(geom: Option<Node<'_, '_>>, w: f64, h: f64, notes: &mut Notes) -> Option<(String, bool)> {
    let (x0, y0) = (0.0, 0.0);
    let geom = geom?;
    if is(geom, "custGeom") {
        let mut s = String::new();
        for p in geom.descendants().filter(|n| is(*n, "path")) {
            let pw = attr_f64(p, "w").filter(|v| *v > 0.0).unwrap_or(w.max(1.0));
            let ph = attr_f64(p, "h").filter(|v| *v > 0.0).unwrap_or(h.max(1.0));
            let pt = |n: Node<'_, '_>| -> Option<String> {
                let x = attr_f64(n, "x")? * w / pw;
                let y = attr_f64(n, "y")? * h / ph;
                Some(format!("{} {}", n_(x), n_(-y)))
            };
            for c in p.children().filter(Node::is_element) {
                let pts: Vec<String> = children(c, "pt").filter_map(pt).collect();
                match (c.tag_name().name(), pts.as_slice()) {
                    ("moveTo", [a]) => s.push_str(&format!("{a} m ")),
                    ("lnTo", [a]) => s.push_str(&format!("{a} l ")),
                    ("cubicBezTo", [a, b, d]) => s.push_str(&format!("{a} {b} {d} c ")),
                    ("quadBezTo", [a, b]) => s.push_str(&format!("{a} {b} {b} c ")),
                    ("close", _) => s.push_str("h "),
                    ("arcTo", _) => notes.add("curved custom shape edges, drawn straight"),
                    _ => {}
                }
            }
        }
        return (!s.is_empty()).then_some((s, false));
    }
    let prst = attr(geom, "prst").unwrap_or("rect");
    let (w, h) = (w.max(0.0), h.max(0.0));
    let poly = |pts: &[(f64, f64)]| -> String {
        let mut s = String::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            s.push_str(&format!("{} {} {} ", n_(x0 + x), n_(y0 - y), if i == 0 { "m" } else { "l" }));
        }
        s.push_str("h ");
        s
    };
    // An ellipse centred on (cx, cy), from four Bézier quarters.
    let oval = |cx: f64, cy: f64, rx: f64, ry: f64| -> String {
        let k = 0.552_284_75;
        format!(
            "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
            n_(cx + rx),
            n_(-cy),
            n_(cx + rx),
            n_(-(cy + ry * k)),
            n_(cx + rx * k),
            n_(-(cy + ry)),
            n_(cx),
            n_(-(cy + ry)),
            n_(cx - rx * k),
            n_(-(cy + ry)),
            n_(cx - rx),
            n_(-(cy + ry * k)),
            n_(cx - rx),
            n_(-cy),
            n_(cx - rx),
            n_(-(cy - ry * k)),
            n_(cx - rx * k),
            n_(-(cy - ry)),
            n_(cx),
            n_(-(cy - ry)),
            n_(cx + rx * k),
            n_(-(cy - ry)),
            n_(cx + rx),
            n_(-(cy - ry * k)),
            n_(cx + rx),
            n_(-cy),
        )
    };
    let m = w.min(h);
    let path = match prst {
        "rect" | "flowChartProcess" | "textBox" => poly(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
        "ellipse" | "flowChartConnector" => oval(w / 2.0, h / 2.0, w / 2.0, h / 2.0),
        // Shapes with a hole: the outline and the hole, filled even-odd.
        "frame" => {
            let d = m * FRAME_ADJ;
            let hole = poly(&[(d, d), (w - d, d), (w - d, h - d), (d, h - d)]);
            return Some((format!("{}{hole}", poly(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)])), true));
        }
        "donut" => {
            let d = m * DONUT_ADJ;
            let hole = oval(w / 2.0, h / 2.0, (w / 2.0 - d).max(0.0), (h / 2.0 - d).max(0.0));
            return Some((format!("{}{hole}", oval(w / 2.0, h / 2.0, w / 2.0, h / 2.0)), true));
        }
        "roundRect" | "flowChartAlternateProcess" => {
            let r = m * 0.166_67;
            let k = r * 0.447_7;
            format!(
                "{} 0 m {} 0 l {} 0 {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c 0 {} l 0 {} {} 0 {} 0 c h ",
                n_(r),
                n_(w - r),
                n_(w - k),
                n_(w),
                n_(-k),
                n_(w),
                n_(-r),
                n_(w),
                n_(-(h - r)),
                n_(w),
                n_(-(h - k)),
                n_(w - k),
                n_(-h),
                n_(w - r),
                n_(-h),
                n_(r),
                n_(-h),
                n_(k),
                n_(-h),
                n_(0.0),
                n_(-(h - k)),
                n_(0.0),
                n_(-(h - r)),
                n_(-r),
                n_(-k),
                n_(k),
                n_(r),
            )
        }
        "triangle" | "flowChartExtract" => poly(&[(w / 2.0, 0.0), (w, h), (0.0, h)]),
        "rtTriangle" => poly(&[(0.0, 0.0), (w, h), (0.0, h)]),
        "diamond" | "flowChartDecision" => poly(&[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)]),
        "parallelogram" | "flowChartInputOutput" => poly(&[(w * 0.25, 0.0), (w, 0.0), (w * 0.75, h), (0.0, h)]),
        "trapezoid" => poly(&[(w * 0.25, 0.0), (w * 0.75, 0.0), (w, h), (0.0, h)]),
        "pentagon" | "homePlate" => poly(&[(0.0, 0.0), (w - m * 0.5, 0.0), (w, h / 2.0), (w - m * 0.5, h), (0.0, h)]),
        "chevron" => poly(&[(0.0, 0.0), (w - m * 0.5, 0.0), (w, h / 2.0), (w - m * 0.5, h), (0.0, h), (m * 0.5, h / 2.0)]),
        "hexagon" => poly(&[(w * 0.25, 0.0), (w * 0.75, 0.0), (w, h / 2.0), (w * 0.75, h), (w * 0.25, h), (0.0, h / 2.0)]),
        "octagon" => {
            let a = m * 0.29;
            poly(&[(a, 0.0), (w - a, 0.0), (w, a), (w, h - a), (w - a, h), (a, h), (0.0, h - a), (0.0, a)])
        }
        "rightArrow" => poly(&[
            (0.0, h * 0.25),
            (w - m * 0.5, h * 0.25),
            (w - m * 0.5, 0.0),
            (w, h / 2.0),
            (w - m * 0.5, h),
            (w - m * 0.5, h * 0.75),
            (0.0, h * 0.75),
        ]),
        "leftArrow" => poly(&[(w, h * 0.25), (m * 0.5, h * 0.25), (m * 0.5, 0.0), (0.0, h / 2.0), (m * 0.5, h), (m * 0.5, h * 0.75), (w, h * 0.75)]),
        "upArrow" => poly(&[(w * 0.25, h), (w * 0.25, m * 0.5), (0.0, m * 0.5), (w / 2.0, 0.0), (w, m * 0.5), (w * 0.75, m * 0.5), (w * 0.75, h)]),
        "downArrow" => poly(&[
            (w * 0.25, 0.0),
            (w * 0.75, 0.0),
            (w * 0.75, h - m * 0.5),
            (w, h - m * 0.5),
            (w / 2.0, h),
            (0.0, h - m * 0.5),
            (w * 0.25, h - m * 0.5),
        ]),
        "plus" | "flowChartSummingJunction" => {
            let a = m * 0.25;
            poly(&[
                (a, 0.0),
                (w - a, 0.0),
                (w - a, a),
                (w, a),
                (w, h - a),
                (w - a, h - a),
                (w - a, h),
                (a, h),
                (a, h - a),
                (0.0, h - a),
                (0.0, a),
                (a, a),
            ])
        }
        "line" | "straightConnector1" => format!("0 0 m {} {} l ", n_(w), n_(-h)),
        _ => {
            notes.add("shapes of a kind the importer approximates by their bounding rectangle");
            poly(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)])
        }
    };
    Some((path, false))
}

/// [`n`] for coordinates already in points.
fn n_(v: f64) -> String {
    n(v)
}

struct ShapeStyle {
    fill: Option<[f64; 3]>,
    image: Option<Option<ObjRef>>,
    line: Option<(f64, [f64; 3])>,
    font_color: Option<[f64; 3]>,
}

fn shape_style(cx: &mut Cx<'_, '_>, sp: Node<'_, '_>, sppr: Option<Node<'_, '_>>, rels: &Rels, c: &Colors) -> ShapeStyle {
    let style = child(sp, "style");
    let r = |name: &str| style.and_then(|s| child(s, name));
    let ref_color = |name: &str| r(name).filter(|f| attr_f64(*f, "idx").unwrap_or(0.0) > 0.0).and_then(|f| color_in(f, c, None));
    let mut out = ShapeStyle { fill: None, image: None, line: None, font_color: r("fontRef").and_then(|f| color_in(f, c, None)) };
    let mut fill_set = false;
    if let Some(p) = sppr {
        for e in p.children().filter(Node::is_element) {
            match e.tag_name().name() {
                "noFill" => fill_set = true,
                "solidFill" => {
                    out.fill = color_in(e, c, ref_color("fillRef"));
                    fill_set = true;
                }
                "gradFill" => {
                    // The gradient's middle stop stands for it.
                    let stops: Vec<[f64; 3]> =
                        e.descendants().filter(|n| is(*n, "gs")).filter_map(|g| color_in(g, c, ref_color("fillRef"))).collect();
                    out.fill = stops.get(stops.len() / 2).copied();
                    fill_set = true;
                    cx.notes.add("gradient fills, drawn in one colour");
                }
                "pattFill" => {
                    out.fill = child(e, "fgClr").and_then(|f| color_in(f, c, None));
                    fill_set = true;
                }
                "blipFill" => {
                    out.image = Some(child(e, "blip").and_then(|b| rel_attr(b, "embed")).and_then(|id| cx.image(rels, id)));
                    fill_set = true;
                }
                "ln" => {
                    let w = attr_f64(e, "w").map_or(0.75, |w| (w / EMU).clamp(0.1, 50.0));
                    if child(e, "noFill").is_some() {
                        out.line = None;
                    } else if let Some(col) = child(e, "solidFill").and_then(|f| color_in(f, c, ref_color("lnRef"))) {
                        out.line = Some((w, col));
                    } else if let Some(col) = ref_color("lnRef") {
                        out.line = Some((w, col));
                    }
                }
                _ => {}
            }
        }
    }
    if !fill_set {
        out.fill = ref_color("fillRef");
    }
    if out.line.is_none() && sppr.and_then(|p| child(p, "ln")).is_none() {
        out.line = ref_color("lnRef").map(|c| (0.75, c));
    }
    out
}

fn auto_label(ty: &str, n: i64) -> String {
    let num = |n: i64| -> String {
        if ty.starts_with("alphaLc") {
            char::from_u32(u32::from(b'a') + ((n.max(1) - 1) % 26) as u32).map(String::from).unwrap_or_default()
        } else if ty.starts_with("alphaUc") {
            char::from_u32(u32::from(b'A') + ((n.max(1) - 1) % 26) as u32).map(String::from).unwrap_or_default()
        } else if ty.starts_with("romanLc") {
            roman(n)
        } else if ty.starts_with("romanUc") {
            roman(n).to_uppercase()
        } else {
            n.to_string()
        }
    };
    let v = num(n);
    if ty.ends_with("ParenBoth") {
        format!("({v})")
    } else if ty.ends_with("ParenR") {
        format!("{v})")
    } else if ty.ends_with("Plain") {
        v
    } else {
        format!("{v}.")
    }
}

fn roman(mut n: i64) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
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
}

/// A paragraph's bullet or list number at level `lvl`, advancing the per-level `counters`.
fn bullet_label(
    notes: &mut Notes,
    style: &Lvl,
    base: &RunStyle,
    has_text: bool,
    lvl: usize,
    counters: &mut [Option<i64>; 9],
) -> Option<(String, RunStyle)> {
    // Bullets show only on paragraphs with text.
    match &style.bullet {
        Some(Bullet::Char(ch)) if has_text => {
            counters.iter_mut().skip(lvl).for_each(|c| *c = None);
            let mut bs = base.clone();
            if let Some(bc) = style.bullet_color {
                bs.color = bc;
            }
            Some((notes.drawable(ch), bs))
        }
        Some(Bullet::Auto(ty, start)) if has_text => {
            let slot = counters.get_mut(lvl);
            let v = match slot {
                Some(Some(v)) => v.saturating_add(1),
                _ => *start,
            };
            if let Some(s) = counters.get_mut(lvl) {
                *s = Some(v);
            }
            counters.iter_mut().skip(lvl + 1).for_each(|c| *c = None);
            Some((auto_label(ty, v), base.clone()))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn text_body(
    cx: &mut Cx<'_, '_>,
    page: &mut Page,
    sp: Node<'_, '_>,
    body: Node<'_, '_>,
    (x, y, w, h): (f64, f64, f64, f64),
    inh: &Inherit<'_, '_, '_>,
    c: &Colors,
    font_color: Option<[f64; 3]>,
) {
    // The list style chain: master text styles, master and layout placeholders, the shape's own.
    let ph = placeholder(sp);
    // Date, footer, slide number and header placeholders use the master's other-text style.
    let mut levels = match ph.as_ref().map(|(t, _)| ph_kind(t)) {
        Some("title") => inh.title.clone(),
        Some("dt" | "ftr" | "sldNum" | "hdr") | None => inh.other.clone(),
        Some(_) => inh.body.clone(),
    };
    let mut body_props: Vec<Node<'_, '_>> = Vec::new();
    if let Some((ty, idx)) = &ph {
        // A slide placeholder finds its layout's by index, but the layout's finds the master's by
        // type: the master's index 2 is its date, not a second content placeholder.
        for (tree, idx) in [(inh.master_ph, None), (inh.layout_ph, idx.as_deref())] {
            if let Some(m) = tree.find(ty, idx).and_then(|s| child(s, "txBody")) {
                if let Some(ls) = child(m, "lstStyle") {
                    levels.merge(&Levels::parse(ls, c));
                }
                if let Some(bp) = child(m, "bodyPr") {
                    body_props.push(bp);
                }
            }
        }
    }
    // The shape style's font colour (`p:style/a:fontRef`) replaces the inherited text colour;
    // colours set on the shape's own list style, paragraphs or runs still win.
    if let Some(fc) = font_color {
        levels.0.iter_mut().for_each(|l| l.color = Some(fc));
    }
    if let Some(ls) = child(body, "lstStyle") {
        levels.merge(&Levels::parse(ls, c));
    }
    if let Some(bp) = child(body, "bodyPr") {
        body_props.push(bp);
    }
    let bp_attr = |k: &str| body_props.iter().rev().find_map(|b| attr(*b, k));
    let bp_f = |k: &str| body_props.iter().rev().find_map(|b| attr_f64(*b, k));
    let ins = |k: &str, d: f64| bp_f(k).map_or(d, |v| v / EMU);
    let (l_in, t_in, r_in, b_in) = (ins("lIns", 7.2), ins("tIns", 3.6), ins("rIns", 7.2), ins("bIns", 3.6));
    let fit = body_props.iter().rev().find_map(|b| child(*b, "normAutofit"));
    let scale = fit.and_then(|f| attr_f64(f, "fontScale")).map_or(1.0, |v| (v / 100_000.0).clamp(0.05, 1.0));
    let ln_reduce = fit.and_then(|f| attr_f64(f, "lnSpcReduction")).map_or(0.0, |v| (v / 100_000.0).clamp(0.0, 0.9));
    if bp_attr("vert").is_some_and(|v| v != "horz") {
        cx.notes.add("vertical text, drawn horizontally");
    }
    let no_wrap = bp_attr("wrap") == Some("none");
    let inner_w = (w - l_in - r_in).max(4.0);
    let mut paras = Vec::new();
    let mut counters: [Option<i64>; 9] = [None; 9];
    for p in children(body, "p") {
        let ppr = child(p, "pPr");
        let lvl = ppr.and_then(|n| attr_f64(n, "lvl")).map_or(0, |v| v.clamp(0.0, 8.0) as usize);
        let mut style = levels.0.get(lvl).cloned().unwrap_or_default();
        if let Some(pp) = ppr {
            style.para_props(pp, c);
        }
        let to_run = |l: &Lvl| -> RunStyle {
            let size = fin(l.size.unwrap_or(18.0) * scale, 18.0).clamp(1.0, 400.0);
            RunStyle {
                fam: family_of(&l.font.clone().unwrap_or_else(|| c.minor.clone())),
                bold: l.bold.unwrap_or(false),
                italic: l.italic.unwrap_or(false),
                size,
                color: l.color.or(font_color).unwrap_or([0.0; 3]),
                underline: l.underline.unwrap_or(false),
                strike: l.strike.unwrap_or(false),
                highlight: None,
                vert: l.baseline.unwrap_or(0),
                caps: l.caps.unwrap_or(false),
                hidden: false,
            }
        };
        let base = to_run(&style);
        let mut items = Vec::new();
        let mut has_text = false;
        for r in p.children().filter(Node::is_element) {
            let mut rl = style.clone();
            if let Some(rp) = child(r, "rPr") {
                rl.run_props(rp, c);
            }
            match r.tag_name().name() {
                "r" => {
                    let t = xml::text_of(r, "t");
                    if !t.is_empty() {
                        has_text = true;
                        for (i, piece) in t.split(['\t', '\n']).enumerate() {
                            if i > 0 {
                                items.push(Inline::Tab);
                            }
                            let piece = cx.notes.drawable(piece);
                            items.push(Inline::Text(piece, to_run(&rl)));
                        }
                    }
                }
                "br" => items.push(Inline::Break),
                "fld" => {
                    has_text = true;
                    if attr(r, "type") == Some("slidenum") {
                        items.push(Inline::Field(Field::Page, to_run(&rl)));
                    } else {
                        let t = cx.notes.drawable(&xml::text_of(r, "t"));
                        items.push(Inline::Text(t, to_run(&rl)));
                    }
                }
                _ => {}
            }
        }
        let label = bullet_label(cx.notes, &style, &base, has_text, lvl, &mut counters);
        let size = base.size;
        let spacing = |s: Option<Spacing>| match s {
            Some(Spacing::Points(p)) => p,
            Some(Spacing::Percent(p)) => p * size * 1.2,
            None => 0.0,
        };
        let line = match style.line {
            Some(LineRule::Auto(m)) => LineRule::Auto((m * 1.2 / 1.15 * (1.0 - ln_reduce)).max(0.3)),
            Some(other) => other,
            None => LineRule::Auto((1.2 / 1.15 * (1.0 - ln_reduce)).max(0.3)),
        };
        let mar_l = style.mar_l.unwrap_or(0.0);
        paras.push(Para {
            items,
            align: style.algn.unwrap_or(Align::Left),
            left: mar_l,
            first: style.indent.unwrap_or(0.0),
            before: spacing(style.before),
            after: spacing(style.after),
            line,
            label,
            base,
            ..Para::default()
        });
    }
    let width = if no_wrap && paras.iter().all(|p| p.align == Align::Left) { inner_w.max(5_000.0) } else { inner_w };
    let laid: Vec<Laid> = paras.iter().map(|p| flow::layout(p, width, (cx.slide_no, cx.slides))).collect();
    // Space before the first paragraph is not added at the top of a text box.
    let total: f64 = laid.iter().enumerate().map(|(i, l)| if i == 0 { l.height() - l.before } else { l.height() }).sum();
    let avail = h - t_in - b_in;
    let mut top = match bp_attr("anchor") {
        Some("ctr") => y - t_in - ((avail - total) / 2.0),
        Some("b") => y - t_in - (avail - total),
        _ => y - t_in,
    };
    for (i, l) in laid.iter().enumerate() {
        if i == 0 {
            top += l.before;
        }
        top -= flow::draw(page, l, x + l_in, top);
    }
}

/// Draw one shape tree's children.
#[allow(clippy::too_many_arguments)]
fn shapes(
    cx: &mut Cx<'_, '_>,
    page: &mut Page,
    tree: Node<'_, '_>,
    rels: &Rels,
    xf: Xf,
    sp: &Space,
    inh: &Inherit<'_, '_, '_>,
    c: &Colors,
    skip_ph: bool,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for e in tree.children().filter(Node::is_element) {
        cx.shapes = cx.shapes.saturating_add(1);
        cx.deck_shapes = cx.deck_shapes.saturating_add(1);
        if cx.shapes > MAX_SHAPES || cx.deck_shapes > MAX_DECK_SHAPES {
            if !cx.cut {
                cx.cut = true;
                cx.notes.add("slides with more shapes than the importer draws, cut short");
            }
            return;
        }
        let name = e.tag_name().name();
        if name == "AlternateContent" {
            if let Some(choice) = child(e, "Choice") {
                shapes(cx, page, choice, rels, xf, sp, inh, c, skip_ph, depth + 1);
            }
            continue;
        }
        let nv = e.children().find(|c| c.is_element() && c.tag_name().name().starts_with("nv"));
        if nv.and_then(|nv| child(nv, "cNvPr")).is_some_and(|p| flag(attr(p, "hidden")) && attr(p, "hidden").is_some()) {
            continue;
        }
        let ph = placeholder(e);
        if skip_ph && ph.is_some() {
            continue;
        }
        let sppr = child(e, "spPr").or_else(|| child(e, "grpSpPr"));
        // Geometry: the shape's own, else its placeholder's on the layout, else the master's.
        let own = sppr.and_then(|p| child(p, "xfrm")).or_else(|| child(e, "xfrm")).and_then(frame_of);
        let frame = own.or_else(|| {
            let (ty, idx) = ph.as_ref()?;
            [(inh.layout_ph, idx.as_deref()), (inh.master_ph, None)]
                .into_iter()
                .find_map(|(t, idx)| t.find(ty, idx).and_then(|s| child(s, "spPr")).and_then(|p| child(p, "xfrm")).and_then(frame_of))
        });
        match name {
            "grpSp" => {
                let Some(gx) = sppr.and_then(|p| child(p, "xfrm")) else {
                    shapes(cx, page, e, rels, xf, sp, inh, c, skip_ph, depth + 1);
                    continue;
                };
                let f = frame_of(gx).unwrap_or_default();
                let ch_off = child(gx, "chOff");
                let ch_ext = child(gx, "chExt");
                let (cox, coy) = (ch_off.and_then(|o| attr_f64(o, "x")).unwrap_or(f.x), ch_off.and_then(|o| attr_f64(o, "y")).unwrap_or(f.y));
                let (cw, chh) = (ch_ext.and_then(|o| attr_f64(o, "cx")).unwrap_or(f.w), ch_ext.and_then(|o| attr_f64(o, "cy")).unwrap_or(f.h));
                let sx = if cw.abs() > 1.0 { f.w / cw } else { 1.0 };
                let sy = if chh.abs() > 1.0 { f.h / chh } else { 1.0 };
                let local = Xf { sx, sy, tx: f.x - cox * sx, ty: f.y - coy * sy };
                let combined = Xf { sx: xf.sx * local.sx, sy: xf.sy * local.sy, tx: xf.tx + local.tx * xf.sx, ty: xf.ty + local.ty * xf.sy };
                shapes(cx, page, e, rels, combined, sp, inh, c, skip_ph, depth + 1);
            }
            "sp" | "cxnSp" | "pic" | "graphicFrame" => {
                let Some(f) = frame else { continue };
                let (ex, ey, ew, eh) = xf.rect(f.x, f.y, f.w, f.h);
                let (x, ytop) = sp.pt(ex, ey);
                let (w, h) = (fin(ew / EMU, 0.0), fin(eh / EMU, 0.0));
                if !(x.is_finite() && ytop.is_finite()) {
                    continue;
                }
                page.save();
                let (cxp, cyp) = (x + w / 2.0, ytop - h / 2.0);
                if f.rot.abs() > 0.01 || f.flip_h || f.flip_v {
                    let a = (-f.rot).to_radians();
                    let (sa, ca) = (a.sin(), a.cos());
                    let (fx, fy) = (if f.flip_h { -1.0 } else { 1.0 }, if f.flip_v { -1.0 } else { 1.0 });
                    page.raw(&format!(
                        "1 0 0 1 {} {} cm {} {} {} {} 0 0 cm {} 0 0 {} 0 0 cm 1 0 0 1 {} {} cm\n",
                        n(cxp),
                        n(cyp),
                        n(ca),
                        n(sa),
                        n(-sa),
                        n(ca),
                        n(fx),
                        n(fy),
                        n(-cxp),
                        n(-cyp)
                    ));
                }
                match name {
                    "pic" => {
                        let blip = child(e, "blipFill").and_then(|b| child(b, "blip"));
                        let r = blip.and_then(|b| rel_attr(b, "embed")).and_then(|id| cx.image(rels, id));
                        let crop = child(e, "blipFill").and_then(|b| child(b, "srcRect"));
                        let k = |name: &str| crop.and_then(|s| attr_f64(s, name)).map_or(0.0, |v| (v / 100_000.0).clamp(-10.0, 0.99));
                        let (cl, ct, cr, cb) = (k("l"), k("t"), k("r"), k("b"));
                        let fw = w / (1.0 - cl - cr).max(0.01);
                        let fh = h / (1.0 - ct - cb).max(0.01);
                        if blip.is_some() {
                            page.save();
                            page.clip_rect(x, ytop - h, w, h);
                            match r {
                                Some(r) => page.image(r, x - cl * fw, ytop - h - cb * fh, fw, fh),
                                None => page.placeholder(x, ytop - h, w, h),
                            }
                            page.restore();
                        }
                        let st = shape_style(cx, e, child(e, "spPr"), rels, c);
                        if let Some((lw, col)) = st.line {
                            page.stroke_rect(x, ytop - h, w, h, lw, col);
                        }
                    }
                    "graphicFrame" => graphic_frame(cx, page, e, rels, (x, ytop, w, h), inh, c),
                    _ => {
                        let st = shape_style(cx, e, sppr, rels, c);
                        let geom = sppr.and_then(|p| child(p, "prstGeom").or_else(|| child(p, "custGeom")));
                        let is_line = name == "cxnSp" || geom.and_then(|g| attr(g, "prst")).is_some_and(|p| p == "line" || p.contains("Connector"));
                        if st.fill.is_some() || st.image.is_some() || st.line.is_some() {
                            // Without a geometry a shape is its rectangle.
                            let (path, even_odd) = path_ops(geom, w, h, cx.notes)
                                .unwrap_or_else(|| (format!("0 0 m {} 0 l {} {} l 0 {} l h ", n(w), n(w), n(-h), n(-h)), false));
                            let star = if even_odd { "*" } else { "" };
                            if let Some(Some(img)) = st.image {
                                // A picture fill is clipped to the shape's outline.
                                page.raw(&format!("q 1 0 0 1 {} {} cm {path}W{star} n 1 0 0 1 {} {} cm\n", n(x), n(ytop), n(-x), n(-ytop)));
                                page.image(img, x, ytop - h, w, h);
                                page.restore();
                            }
                            let fill = if is_line { None } else { st.fill };
                            let (op, star) = match (fill, st.line) {
                                (Some(_), Some(_)) => ("B", star),
                                (Some(_), None) => ("f", star),
                                (None, Some(_)) => ("S", ""),
                                (None, None) => ("n", ""),
                            };
                            let mut s = String::new();
                            if let Some(col) = fill {
                                s.push_str(&format!("{} {} {} rg ", n(col[0]), n(col[1]), n(col[2])));
                            }
                            if let Some((lw, col)) = st.line {
                                s.push_str(&format!("{} {} {} RG {} w ", n(col[0]), n(col[1]), n(col[2]), n(lw)));
                            }
                            page.raw(&format!("q 1 0 0 1 {} {} cm\n{s}{path}{op}{star}\nQ\n", n(x), n(ytop)));
                        }
                        if let Some(body) = child(e, "txBody") {
                            text_body(cx, page, e, body, (x, ytop, w, h), inh, c, st.font_color);
                        }
                    }
                }
                page.restore();
            }
            _ => {}
        }
    }
}

/// Draw a graphic frame: a table, an embedded picture, or a placeholder box for anything else.
fn graphic_frame(
    cx: &mut Cx<'_, '_>,
    page: &mut Page,
    e: Node<'_, '_>,
    rels: &Rels,
    (x, ytop, w, h): (f64, f64, f64, f64),
    inh: &Inherit<'_, '_, '_>,
    c: &Colors,
) {
    let data = e.descendants().find(|d| is(*d, "graphicData"));
    let uri = data.and_then(|d| attr(d, "uri")).unwrap_or_default();
    if let Some(tbl) = data.and_then(|d| child(d, "tbl")) {
        table(cx, page, tbl, (x, ytop, w, h), inh, c);
    } else if let Some(blip) = data.and_then(|d| d.descendants().find(|b| is(*b, "blip"))) {
        let r = rel_attr(blip, "embed").and_then(|id| cx.image(rels, id));
        match r {
            Some(r) => page.image(r, x, ytop - h, w, h),
            None => page.placeholder(x, ytop - h, w, h),
        }
    } else {
        cx.notes.add(if uri.contains("chart") {
            "charts, drawn as empty boxes"
        } else if uri.contains("diagram") {
            "SmartArt diagrams, drawn as empty boxes"
        } else {
            "embedded objects, drawn as empty boxes"
        });
        page.placeholder(x, ytop - h, w, h);
    }
}

fn table(cx: &mut Cx<'_, '_>, page: &mut Page, tbl: Node<'_, '_>, (x, ytop, w, _h): (f64, f64, f64, f64), inh: &Inherit<'_, '_, '_>, c: &Colors) {
    let cols: Vec<f64> = child(tbl, "tblGrid")
        .map(|g| children(g, "gridCol").map(|c| attr_f64(c, "w").map_or(0.0, |w| (w / EMU).max(0.0))).collect())
        .unwrap_or_default();
    let ncols = cols.len().max(1);
    let total: f64 = cols.iter().sum();
    let cols = if total <= 1.0 { vec![w / ncols as f64; ncols] } else { cols };
    let edges: Vec<f64> = std::iter::once(0.0)
        .chain(cols.iter().scan(0.0, |a, c| {
            *a += c;
            Some(*a)
        }))
        .collect();
    let tpr = child(tbl, "tblPr");
    let styled = tpr.and_then(|p| child(p, "tableStyleId")).is_some();
    let first_row = tpr.is_some_and(|p| flag(attr(p, "firstRow")) && attr(p, "firstRow").is_some());
    let band = tpr.is_some_and(|p| flag(attr(p, "bandRow")) && attr(p, "bandRow").is_some());
    let accent = c.scheme("accent1").unwrap_or([0.27, 0.45, 0.77]);
    let tint = |t: f64| accent.map(|v| v * t + (1.0 - t));
    let mut y = ytop;
    for (ri, tr) in children(tbl, "tr").enumerate() {
        let min_h = attr_f64(tr, "h").map_or(0.0, |v| v / EMU);
        let header = styled && first_row && ri == 0;
        let mut cells = Vec::new();
        let mut g = 0usize;
        for tc in children(tr, "tc") {
            // A row has one cell per grid column: a cell spanning columns is followed by `hMerge`
            // placeholders for the rest, so each cell moves the cursor by one column, and a
            // placeholder draws nothing (the spanning cell covers it).
            let col = g;
            g = g.saturating_add(1);
            if flag(attr(tc, "hMerge")) && attr(tc, "hMerge").is_some() {
                continue;
            }
            let span = attr_f64(tc, "gridSpan").map_or(1, |v| v.clamp(1.0, 64.0) as usize);
            let merged = flag(attr(tc, "vMerge")) && attr(tc, "vMerge").is_some();
            let x1 = edges.get(col).copied().unwrap_or(0.0);
            let x2 = edges.get(col.saturating_add(span).min(ncols)).copied().unwrap_or(w);
            let pr = child(tc, "tcPr");
            let mar = |k: &str, d: f64| pr.and_then(|p| attr_f64(p, k)).map_or(d, |v| v / EMU);
            let fill = pr.and_then(|p| child(p, "solidFill")).and_then(|f| color_in(f, c, None)).or(if header {
                Some(accent)
            } else if styled && band {
                Some(if (ri + usize::from(!first_row)) % 2 == 1 { tint(0.4) } else { tint(0.2) })
            } else if styled {
                Some(tint(0.2))
            } else {
                None
            });
            let font_color = if header { Some([1.0; 3]) } else { None };
            cells.push((tc, x1, x2, merged, fill, font_color, mar("marL", 7.2), mar("marR", 7.2), mar("marT", 3.6), mar("marB", 3.6)));
        }
        // Row height: the tallest cell's text.
        let mut row_h = min_h;
        let mut laid_cells = Vec::new();
        for (tc, x1, x2, merged, fill, fc, ml, mr, mt, mb) in &cells {
            let mut paras = Vec::new();
            if !merged && let Some(body) = child(*tc, "txBody") {
                paras.push(body);
            }
            let inner = (x2 - x1 - ml - mr).max(4.0);
            let mut h = 0.0;
            let mut laid = Vec::new();
            for body in &paras {
                let ls = cell_text(cx, *body, inner, inh, c, if *fc == Some([1.0; 3]) { Some([1.0; 3]) } else { None }, header);
                h += ls.iter().map(Laid::height).sum::<f64>();
                laid.push(ls);
            }
            row_h = row_h.max(h + mt + mb);
            laid_cells.push((*x1, *x2, *fill, laid, *ml, *mt));
        }
        for (x1, x2, fill, laid, ml, mt) in laid_cells {
            if let Some(col) = fill {
                page.fill_rect(x + x1, y - row_h, x2 - x1, row_h, col);
            }
            if styled {
                page.stroke_rect(x + x1, y - row_h, x2 - x1, row_h, 1.0, [1.0; 3]);
            } else {
                page.stroke_rect(x + x1, y - row_h, x2 - x1, row_h, 0.5, [0.3; 3]);
            }
            let mut ty = y - mt;
            for ls in laid {
                for l in ls {
                    ty -= flow::draw(page, &l, x + x1 + ml, ty);
                }
            }
        }
        y -= row_h;
    }
}

/// A table cell's paragraphs, laid out `width` points wide.
fn cell_text(
    cx: &mut Cx<'_, '_>,
    body: Node<'_, '_>,
    width: f64,
    inh: &Inherit<'_, '_, '_>,
    c: &Colors,
    color: Option<[f64; 3]>,
    bold: bool,
) -> Vec<Laid> {
    let mut levels = inh.other.clone();
    if let Some(ls) = child(body, "lstStyle") {
        levels.merge(&Levels::parse(ls, c));
    }
    let mut out = Vec::new();
    for p in children(body, "p") {
        let mut style = levels.0.first().cloned().unwrap_or_default();
        if let Some(pp) = child(p, "pPr") {
            style.para_props(pp, c);
        }
        let mut items = Vec::new();
        let mut base = None;
        for r in p.children().filter(Node::is_element) {
            let name = r.tag_name().name();
            if name == "br" {
                items.push(Inline::Break);
                continue;
            }
            if name != "r" && name != "fld" {
                continue;
            }
            let mut rl = style.clone();
            // A colour on the run itself wins over the table style's, which wins over the
            // inherited text colour.
            let mut own = Lvl::default();
            if let Some(rp) = child(r, "rPr") {
                rl.run_props(rp, c);
                own.run_props(rp, c);
            }
            let rs = RunStyle {
                fam: family_of(&rl.font.clone().unwrap_or_else(|| c.minor.clone())),
                bold: rl.bold.unwrap_or(bold),
                italic: rl.italic.unwrap_or(false),
                size: fin(rl.size.unwrap_or(18.0), 18.0).clamp(1.0, 400.0),
                color: own.color.or(color).or(rl.color).unwrap_or([0.0; 3]),
                underline: rl.underline.unwrap_or(false),
                ..RunStyle::default()
            };
            base.get_or_insert_with(|| rs.clone());
            if name == "fld" && attr(r, "type") == Some("slidenum") {
                items.push(Inline::Field(Field::Page, rs));
            } else {
                let t = cx.notes.drawable(&xml::text_of(r, "t"));
                items.push(Inline::Text(t, rs));
            }
        }
        let base = base.unwrap_or(RunStyle { size: fin(style.size.unwrap_or(18.0), 18.0).clamp(1.0, 400.0), ..RunStyle::default() });
        out.push(flow::layout(&Para { items, align: style.algn.unwrap_or(Align::Left), base, ..Para::default() }, width, (cx.slide_no, cx.slides)));
    }
    out
}

fn background(cx: &mut Cx<'_, '_>, page: &mut Page, sources: &[(Option<Node<'_, '_>>, &Rels)], c: &Colors, w: f64, h: f64) {
    for (root, rels) in sources {
        let Some(bg) = root.and_then(|r| xml::path(r, &["cSld", "bg"])) else { continue };
        if let Some(pr) = child(bg, "bgPr") {
            if let Some(f) = child(pr, "solidFill").and_then(|f| color_in(f, c, None)) {
                page.fill_rect(0.0, 0.0, w, h, f);
                return;
            }
            if let Some(g) = child(pr, "gradFill") {
                let stops: Vec<[f64; 3]> = g.descendants().filter(|n| is(*n, "gs")).filter_map(|s| color_in(s, c, None)).collect();
                if let Some(col) = stops.get(stops.len() / 2) {
                    page.fill_rect(0.0, 0.0, w, h, *col);
                    cx.notes.add("gradient fills, drawn in one colour");
                    return;
                }
            }
            if let Some(id) = child(pr, "blipFill").and_then(|b| child(b, "blip")).and_then(|b| rel_attr(b, "embed")) {
                if let Some(r) = cx.image(rels, id) {
                    page.image(r, 0.0, 0.0, w, h);
                }
                return;
            }
            if let Some(p) = child(pr, "pattFill").and_then(|p| child(p, "fgClr")).and_then(|f| color_in(f, c, None)) {
                page.fill_rect(0.0, 0.0, w, h, p);
                return;
            }
        }
        if let Some(r) = child(bg, "bgRef") {
            if let Some(col) = color_in(r, c, None) {
                page.fill_rect(0.0, 0.0, w, h, col);
            }
            return;
        }
    }
}

pub(crate) fn convert(zip: &Zip<'_>, main: &str, doc: &mut Document, notes: &mut Notes) -> Result<usize, String> {
    let rels = Rels::load(zip, main)?;
    let text = zip.text(main)?.ok_or_else(|| format!("{main} is missing"))?;
    let pres = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let root = pres.root_element();
    let size = child(root, "sldSz");
    let w = size.and_then(|s| attr_f64(s, "cx")).map_or(720.0, |v| v / EMU);
    let h = size.and_then(|s| attr_f64(s, "cy")).map_or(540.0, |v| v / EMU);
    let (w, h) = (fin(w, 720.0).clamp(72.0, 14_400.0), fin(h, 540.0).clamp(72.0, 14_400.0));
    let slide_parts: Vec<String> = child(root, "sldIdLst")
        .map(|l| children(l, "sldId").filter_map(|s| rel_attr(s, "id")).filter_map(|id| rels.target(id)).map(str::to_owned).collect())
        .unwrap_or_default();
    let mut texts: HashMap<String, String> = HashMap::new();
    let mut load = |part: &str| -> Result<(), String> {
        if !texts.contains_key(part)
            && let Some(t) = zip.text(part)?
        {
            texts.insert(part.to_owned(), t);
        }
        Ok(())
    };
    // Read every slide, layout, master and theme part up front.
    let mut plan = Vec::new();
    for part in &slide_parts {
        load(part)?;
        let srels = Rels::load(zip, part)?;
        let layout = srels.of_type("slideLayout").map(str::to_owned);
        let lrels = match &layout {
            Some(l) => {
                load(l)?;
                Rels::load(zip, l)?
            }
            None => Rels::load(zip, "")?,
        };
        let master = lrels.of_type("slideMaster").map(str::to_owned);
        let mrels = match &master {
            Some(m) => {
                load(m)?;
                Rels::load(zip, m)?
            }
            None => Rels::load(zip, "")?,
        };
        let theme = mrels.of_type("theme").map(str::to_owned);
        if let Some(t) = &theme {
            load(t)?;
        }
        plan.push((part.clone(), srels, layout, lrels, master, mrels, theme));
    }
    let default_text = child(root, "defaultTextStyle");
    let mut canvas = Canvas::default();
    // Each slide drawn, with its place among the deck's readable slides: a hidden slide keeps its
    // place, so the slides after it are not renumbered.
    let mut readable = 0usize;
    let visible: Vec<(usize, usize)> = plan
        .iter()
        .enumerate()
        .filter_map(|(i, (part, ..))| {
            // A slide that can't be read is left out here, not when drawing, so it isn't counted
            // in the slide numbers either.
            let Some(slide) = texts.get(part).and_then(|t| xml::parse(t).ok()) else {
                notes.add("slides whose part is missing or damaged, left out");
                return None;
            };
            let place = readable;
            readable = readable.saturating_add(1);
            let shown = attr(slide.root_element(), "show").is_none_or(|v| flag(Some(v)));
            if !shown {
                notes.add("hidden slides, left out as PowerPoint does when saving a PDF");
            }
            shown.then_some((i, place))
        })
        .collect();
    // Layouts, masters and themes are shared by many slides: each is parsed once, here. Slides are
    // parsed one at a time in the loop, so only one is held at once.
    let mut shared: HashMap<&str, roxmltree::Document<'_>> = HashMap::new();
    // Only the visible slides' parts: a hidden slide is never drawn, so neither is its master.
    for part in visible.iter().filter_map(|(i, _)| plan.get(*i)).flat_map(|(_, _, l, _, m, _, t)| [l, m, t]).flatten() {
        if !shared.contains_key(part.as_str())
            && let Some(d) = texts.get(part).and_then(|t| xml::parse(t).ok())
        {
            shared.insert(part.as_str(), d);
        }
    }
    // And each one's placeholders indexed once (a slide looks them up for every shape).
    let placeholders: HashMap<&str, Placeholders<'_, '_>> =
        shared.iter().map(|(p, d)| (*p, Placeholders::of(xml::path(d.root_element(), &["cSld", "spTree"])))).collect();
    let none = Placeholders::default();
    // Slides are numbered from `firstSlideNum`.
    let first_no = attr_f64(root, "firstSlideNum").map_or(1, |v| fin(v, 1.0).clamp(0.0, 9999.0) as usize);
    let mut cx = Cx { zip, doc, notes, images: HashMap::new(), slide_no: 0, slides: visible.len(), shapes: 0, deck_shapes: 0, cut: false };
    for (i, place) in &visible {
        let Some((part, srels, layout, lrels, master, mrels, theme)) = plan.get(*i) else { continue };
        let parsed = |p: &Option<String>| p.as_deref().and_then(|p| shared.get(p));
        let Some(slide) = texts.get(part).and_then(|t| xml::parse(t).ok()) else { continue };
        let layout_doc = parsed(layout);
        let master_doc = parsed(master);
        let theme_doc = parsed(theme);
        let colors = Colors::load(theme_doc.as_ref().map(|d| d.root_element()), master_doc.as_ref().map(|d| d.root_element()));
        let tx = master_doc.as_ref().and_then(|d| child(d.root_element(), "txStyles"));
        let lv = |name: &str| tx.and_then(|t| child(t, name)).map(|n| Levels::parse(n, &colors)).unwrap_or_default();
        let mut other = default_text.map(|d| Levels::parse(d, &colors)).unwrap_or_default();
        other.merge(&lv("otherStyle"));
        // Placeholder text defaults to the theme's text colour (tx1) where nothing else says.
        let tx1 = colors.scheme("tx1");
        let mut title = lv("titleStyle");
        let mut body = lv("bodyStyle");
        for l in title.0.iter_mut().chain(body.0.iter_mut()).chain(other.0.iter_mut()) {
            if l.color.is_none() {
                l.color = tx1;
            }
        }
        let major = colors.major.clone();
        title.0.iter_mut().filter(|l| l.font.is_none()).for_each(|l| l.font = Some(major.clone()));
        let layout_tree = layout_doc.as_ref().and_then(|d| xml::path(d.root_element(), &["cSld", "spTree"]));
        let master_tree = master_doc.as_ref().and_then(|d| xml::path(d.root_element(), &["cSld", "spTree"]));
        let inh = Inherit {
            layout: layout_tree,
            master: master_tree,
            layout_ph: layout.as_deref().and_then(|p| placeholders.get(p)).unwrap_or(&none),
            master_ph: master.as_deref().and_then(|p| placeholders.get(p)).unwrap_or(&none),
            title,
            body,
            other,
        };
        cx.slide_no = first_no.saturating_add(*place);
        (cx.shapes, cx.cut) = (0, false);
        let pi = canvas.new_page(w, h)?;
        let Some(page) = canvas.page(pi) else { continue };
        let space = Space { h };
        let sroot = slide.root_element();
        let lroot = layout_doc.as_ref().map(|d| d.root_element());
        let mroot = master_doc.as_ref().map(|d| d.root_element());
        background(&mut cx, page, &[(Some(sroot), srels), (lroot, lrels), (mroot, mrels)], &colors, w, h);
        let show_master = |n: Option<Node<'_, '_>>| n.is_none_or(|n| attr(n, "showMasterSp").is_none_or(|v| flag(Some(v))));
        if show_master(Some(sroot)) {
            if show_master(lroot)
                && let Some(t) = inh.master
            {
                shapes(&mut cx, page, t, mrels, Xf::ID, &space, &inh, &colors, true, 0);
            }
            if let Some(t) = inh.layout {
                shapes(&mut cx, page, t, lrels, Xf::ID, &space, &inh, &colors, true, 0);
            }
        }
        if let Some(tree) = xml::path(sroot, &["cSld", "spTree"]) {
            shapes(&mut cx, page, tree, srels, Xf::ID, &space, &inh, &colors, false, 0);
        }
    }
    canvas.finish(cx.doc).map_err(|e| e.to_string())
}
