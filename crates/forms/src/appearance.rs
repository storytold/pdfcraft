//! Field appearances (§12.7.4.3 and §12.5.5), execution plan M6.2.
//!
//! Text and choice fields are drawn from their `/DA` (font, size, colour) and the widget's `/MK`
//! (background and border colours) and `/BS` (border width and style), inside a `/Tx BMC … EMC`
//! marked-content block as viewers expect. Supported: single-line, multiline (wrapped), comb,
//! password, quadding, auto font size (`0 Tf`), combo boxes and list boxes (selection shown).
//!
//! The `/DA` font is used when the form's `/DR` defines it as a simple font that can encode the
//! displayed text (using its own encoding and Differences), or as a composite font with a
//! predefined Unicode CMap such as `UniJIS-UTF16-H`, as Japanese forms use. Otherwise (unsupported
//! characters, other composite fonts, missing resources) Helvetica is used. Text the chosen font
//! can't hold (outside WinAnsi in Helvetica, or beyond U+FFFF in a UCS-2 CMap) is drawn as `?`, as
//! Acrobat keeps a value its field font can't show: the value itself is stored as typed,
//! [`field_appearance_drawn`] returns the first such character, and the caller marks the widget
//! (`pdfcraft_fonts::UNDRAWN_MARK`) and asks other viewers to redraw the field
//! (`/NeedAppearances`). Widths are the field font's own (its `/Widths`, or the standard-14
//! metrics), Helvetica's when Helvetica stands in, and one em per full-width character in
//! composite fonts.
//!
//! `/MK /R` (0, 90, 180, 270, counterclockwise) lays the content out in the upright box and sets
//! the appearance `/Matrix` so that box lands on the widget rectangle. Any other value is 0, and
//! rotation 0 leaves `/Matrix` off.

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{UnicodeCMap, cjk_width, helvetica_width, literal, win_ansi, wrap_with};

use crate::{Field, FieldKind, Widget, acroform, flags};

/// Format a number for content streams.
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Format a number for content streams (up to three decimals).
pub fn fmt(v: f64) -> String {
    n(v)
}

/// The font size an auto-sized (`0 Tf`) text field uses for `text` in a `width` × `height` box
/// (inside the padding), measured by `width_of(text, size)`, never under 4 pt. Single-line text
/// takes the size the height gives, as Acrobat Reader draws it however tall the box (the size is
/// implementation-defined, ISO 32000-2 §12.7.4.3), and shrinks until it fits the width; multiline
/// text takes the largest size up to 12 pt whose wrapped lines fit the height. The page
/// appearance and the on-screen editor share this.
pub fn auto_text_size(text: &str, width: f64, height: f64, multiline: bool, width_of: impl Fn(&str, f64) -> f64) -> f64 {
    let wrap_width = if width.is_finite() { width.max(1.0) } else { 1.0 };
    auto_text_size_by(
        width,
        height,
        multiline,
        |size| width_of(text, size),
        |size| wrap_with(text, size, wrap_width, &width_of).len() as f64 * size * 1.15,
    )
}

/// The automatic font size of a comb field (`Field::is_comb`) whose cells are `height` tall inside
/// the padding: 4 to 12 pt, as only the height bounds a one-character cell. The page appearance
/// and the on-screen editor share this.
pub fn comb_auto_text_size(height: f64) -> f64 {
    if height.is_finite() { (height / 1.15).clamp(4.0, 12.0) } else { 4.0 }
}

/// The tallest box single-line auto-size follows, as a sanity bound: the largest page side
/// ISO 32000-1 Annex C recommends.
pub(crate) const MAX_AUTO_HEIGHT: f64 = 14_400.0;

/// [`auto_text_size`] with the text measured by the caller: `line_width(size)` is its width on
/// one line, `wrapped_height(size)` its height wrapped at `width`. The screen measures in its
/// own font, whose widths and line breaks differ from the page's.
pub fn auto_text_size_by(width: f64, height: f64, multiline: bool, line_width: impl Fn(f64) -> f64, wrapped_height: impl Fn(f64) -> f64) -> f64 {
    let (width, height) = (if width.is_finite() { width.max(1.0) } else { 1.0 }, if height.is_finite() { height } else { 0.0 });
    if multiline {
        let mut size = 12.0;
        // A NaN height from the measurer compares false, so it keeps the size it has.
        while size > 4.0 && wrapped_height(size) > height {
            size -= 0.5;
        }
        return size;
    }
    let size = (height.min(MAX_AUTO_HEIGHT) / 1.15).max(4.0);
    let tw = line_width(size);
    if tw.is_finite() && tw > width { (size * width / tw).max(4.0) } else { size }
}

/// How a text field's widget lays its value out in [`field_appearance_drawn`], for asking
/// whether a value fits it ([`TextBox::fits`]): what a field that doesn't scroll (`DoNotScroll`,
/// ISO 32000-2 Table 228) still takes. Built from the same pieces the appearance uses: the `/DA`
/// font from the form's `/DR` (Helvetica where that font can't show the text), the room inside
/// the border and the 2 pt padding, the `/DA` size (or the automatic size), and wrapping for a
/// multi-line field; the two must change together.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    /// The `/DA` font from `/DR`, when it is one the appearance can draw with.
    font: Option<FontText>,
    /// Width and height inside the border and padding.
    inner_w: f64,
    inner_h: f64,
    /// The `/DA` size; `None` for an automatic (`0 Tf`) size, which shrinks to fit.
    size: Option<f64>,
    multiline: bool,
    password: bool,
    /// A comb field: its cells, not its width, bound it (`/MaxLen`).
    comb: bool,
}

/// The [`TextBox`] widget `w` of text field `f` lays its value out in.
pub fn text_box(doc: &Document, f: &Field, w: &Widget) -> TextBox {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(&f.da));
    let pad = 2.0 + border_width(doc, &wd);
    let multiline = f.has(flags::MULTILINE);
    TextBox {
        font: dr_font(doc, &da.font).map(|(_, enc)| enc),
        inner_w: (place.layout_w - 2.0 * pad).max(1.0),
        inner_h: place.layout_h - 2.0 * pad,
        size: (da.size > 0.0 && da.size.is_finite()).then_some(da.size),
        multiline,
        password: f.has(flags::PASSWORD),
        comb: f.is_comb(),
    }
}

impl TextBox {
    /// Whether `text` fits: on one line within the width, or, multi-line, wrapped within the
    /// height, as the appearance draws it. An automatic size shrinks before text stops fitting,
    /// so such a field is full only when the text no longer fits at its smallest size (4 pt). A
    /// comb field always fits (its `/MaxLen` bounds it).
    pub fn fits(&self, text: &str) -> bool {
        if self.comb || text.is_empty() {
            return true;
        }
        let shown: String = if self.password { "*".repeat(text.chars().count()) } else { text.to_string() };
        // The font the appearance would choose for this text.
        let can_show = |enc: &FontText| if self.multiline { shown.split(['\n', '\r']).all(|l| enc.can_encode(l)) } else { enc.can_encode(&shown) };
        let font = self.font.as_ref().filter(|enc| can_show(enc)).unwrap_or(&FontText::WinAnsi);
        let width_of = |t: &str, size: f64| font.width(t, size);
        // The smallest an automatic size goes ([`auto_text_size_by`]).
        let size = self.size.unwrap_or(4.0);
        if self.multiline {
            // As the appearance draws it: the first baseline is 0.85 em below the padded top and
            // a line is drawn while its descent (0.207 em) stays inside the border, so the
            // padding below the last line is not needed: `inner_h` plus 2 pt.
            // Long words break by character, so a line is wider than the box only when it is a
            // single character wider than the box, which the appearance still cuts at the sides.
            let lines = wrap_with(&shown, size, self.inner_w, width_of);
            let above = lines.len().saturating_sub(1) as f64;
            above * size * 1.15 + 1.057 * size <= self.inner_h + 2.0 + 0.01 && lines.iter().all(|l| width_of(l, size) <= self.inner_w + 0.01)
        } else {
            // A line break in a single-line value is drawn as a space.
            let line: String = shown.chars().map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c }).collect();
            width_of(&line, size) <= self.inner_w + 1e-6
        }
    }

    /// An edit from `before` to `after` as a field that doesn't scroll takes it: all of it when
    /// `after` fits, and otherwise as much of the text it inserted (typed or pasted) as still
    /// fits, keeping what it removed removed. Returns the text and where the caret goes (in
    /// characters, after what was kept of the insertion).
    pub fn fitted_edit(&self, before: &str, after: &str) -> (String, usize) {
        let (b, a): (Vec<char>, Vec<char>) = (before.chars().collect(), after.chars().collect());
        // The edit replaced b[p..b.len() - q] with a[p..a.len() - q].
        let p = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
        let most = b.len().min(a.len()).saturating_sub(p);
        let q = b.iter().rev().zip(a.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
        // q <= a.len() - p, so these are in range; `get` keeps it so if that ever changes.
        let end = a.len().saturating_sub(q);
        let (prefix, inserted, suffix) = (a.get(..p).unwrap_or_default(), a.get(p..end).unwrap_or_default(), a.get(end..).unwrap_or_default());
        let with = |k: usize| prefix.iter().chain(inserted.get(..k).unwrap_or_default()).chain(suffix).collect::<String>();
        if self.fits(after) {
            return (after.to_string(), p + inserted.len());
        }
        // The longest of the insertion's first `from..=to` characters that fits, halving: in one
        // font a longer text is never narrower. `None` when even `from` doesn't fit.
        let longest = |from: usize, to: usize| -> Option<usize> {
            if from > to || !self.fits(&with(from)) {
                return None;
            }
            let (mut lo, mut hi) = (from, to);
            while lo < hi {
                let mid = lo + (hi - lo).div_ceil(2);
                if self.fits(&with(mid)) { lo = mid } else { hi = mid - 1 }
            }
            Some(lo)
        };
        // The font can change once, from the form's own to Helvetica, at the first inserted
        // character the form's font can't show: each side of that point is searched on its own.
        let n = inserted.len();
        let kept = match self.font_switch(prefix, inserted, suffix) {
            Some(t) => longest(0, t.saturating_sub(1)).into_iter().chain(longest(t, n)).max(),
            None => longest(0, n),
        }
        .unwrap_or(0);
        (with(kept), p + kept)
    }

    /// Where an insertion changes the font [`Self::fits`] measures in: how much of `inserted`
    /// (between `prefix` and `suffix`) first includes a character the form's own font can't show,
    /// when everything around it can be shown in that font. `None` when the font never changes.
    fn font_switch(&self, prefix: &[char], inserted: &[char], suffix: &[char]) -> Option<usize> {
        let enc = self.font.as_ref().filter(|_| !self.password)?;
        // As `fits` decides: a multi-line value is checked line by line, its breaks aside.
        let shows = |c: &char| (self.multiline && matches!(c, '\n' | '\r')) || enc.can_encode(&c.to_string());
        if !prefix.iter().chain(suffix).all(shows) {
            return None;
        }
        inserted.iter().position(|c| !shows(c)).map(|i| i + 1)
    }
}

/// `/MK /R` as a quadrant, or 0 when it is missing or not a multiple of 90.
pub(crate) fn mk_rotation(doc: &Document, wd: &Dict) -> i64 {
    let Some(raw) = wd.get(b"MK") else { return 0 };
    let mk_obj = doc.resolve(raw);
    let Some(mk) = mk_obj.as_dict() else { return 0 };
    let Some(r_obj) = mk.get(b"R") else { return 0 };
    let resolved = doc.resolve(r_obj);
    let Some(r) = degrees(&resolved) else { return 0 };
    match r.rem_euclid(360) {
        q @ (0 | 90 | 180 | 270) => q,
        _ => 0,
    }
}

fn degrees(o: &Object) -> Option<i64> {
    if let Some(i) = o.as_int() {
        return Some(i);
    }
    let v = o.as_f64()?;
    (v.is_finite() && v.abs() <= 1.0e6).then_some(v.round() as i64)
}

/// Where an appearance is drawn: the upright layout box, and the `/Matrix` that turns it onto
/// the widget rectangle. Rotation 0 has no matrix.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub layout_w: f64,
    pub layout_h: f64,
    pub matrix: Option<[f64; 6]>,
}

pub(crate) fn placement(rotation: i64, page_w: f64, page_h: f64) -> Placement {
    let (layout_w, layout_h, matrix) = match rotation {
        // (x, y) -> (page_w - y, x): text that ran to the right runs upward.
        90 => (page_h, page_w, Some([0.0, 1.0, -1.0, 0.0, page_w, 0.0])),
        180 => (page_w, page_h, Some([-1.0, 0.0, 0.0, -1.0, page_w, page_h])),
        // (x, y) -> (y, page_h - x): text that ran to the right runs downward.
        270 => (page_h, page_w, Some([0.0, -1.0, 1.0, 0.0, 0.0, page_h])),
        _ => (page_w, page_h, None),
    };
    Placement { layout_w, layout_h, matrix }
}

pub(crate) fn set_form_box(d: &mut Dict, place: Placement) {
    let box_ = [0.0, 0.0, place.layout_w, place.layout_h];
    d.set(b"BBox".to_vec(), Object::Array(box_.iter().map(|v| Object::Real(*v)).collect()));
    if let Some(m) = place.matrix {
        d.set(b"Matrix".to_vec(), Object::Array(m.iter().map(|v| Object::Real(*v)).collect()));
    }
}

/// A parsed default appearance string: font resource name, size (0 = auto) and colour operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Da {
    pub font: String,
    pub size: f64,
    /// The colour operation as written (`0 g`, `1 0 0 rg`, `0 0 0 1 k`).
    pub color: String,
}

pub fn parse_da(da: &str) -> Da {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut out = Da { font: "Helv".into(), size: 0.0, color: "0 g".into() };
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 2 => {
                out.font = toks[i - 2].trim_start_matches('/').to_string();
                out.size = toks[i - 1].parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0).min(300.0);
            }
            "g" if i >= 1 && toks[i - 1].parse::<f64>().is_ok() => out.color = format!("{} g", toks[i - 1]),
            "rg" if i >= 3 && toks[i - 3..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} rg", toks[i - 3..i].join(" ")),
            "k" if i >= 4 && toks[i - 4..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} k", toks[i - 4..i].join(" ")),
            _ => {}
        }
    }
    out
}

/// How a field font's text is encoded and measured.
#[derive(Clone, Debug, PartialEq)]
enum FontText {
    WinAnsi,
    /// A reused simple font's encoding, including indirect dictionaries and Differences.
    Simple(Box<Metrics>),
    /// A composite font with a predefined Unicode CMap.
    Unicode(UnicodeCMap),
}

impl FontText {
    fn encode(&self, s: &str) -> Vec<u8> {
        match self {
            Self::WinAnsi => win_ansi(s),
            // Selection checks every displayed string first; wrapping only removes line breaks.
            Self::Simple(metrics) => metrics.encode(&s.replace('\t', " ")).unwrap_or_default(),
            Self::Unicode(cmap) => cmap.encode(s),
        }
    }

    fn can_encode(&self, s: &str) -> bool {
        match self {
            Self::Simple(metrics) => metrics.encode(&s.replace('\t', " ")).is_some(),
            _ => true,
        }
    }

    /// The first character of `s` the encoding can't hold, which [`Self::encode`] writes as `?`
    /// (a reused simple font is only chosen when it holds them all): reported by
    /// [`field_appearance_drawn`], so that `?` is never silent (AGENTS.md §4).
    fn first_unencodable(&self, s: &str) -> Option<char> {
        match self {
            Self::WinAnsi => pdfcraft_fonts::first_undrawable(s),
            Self::Simple(metrics) => {
                s.chars().map(|c| if c == '\t' { ' ' } else { c }).find(|c| metrics.encode(c.encode_utf8(&mut [0; 4])).is_none())
            }
            Self::Unicode(cmap) => cmap.first_unencodable(s),
        }
    }

    fn width(&self, s: &str, size: f64) -> f64 {
        match self {
            Self::WinAnsi => helvetica_width(s, size),
            // The font's `/Widths`, or its standard-14 metrics (Courier, Times) when it has none.
            Self::Simple(metrics) => match metrics.encode(&s.replace(['\t', '\r', '\n'], " ")) {
                Some(bytes) => metrics.codes(&bytes).iter().map(|&(code, _)| metrics.width(code)).sum::<f64>() * size,
                None => helvetica_width(s, size),
            },
            Self::Unicode(_) => cjk_width(s, size),
        }
    }
}

/// The font resource for `name` from the form's `/DR`, if it is a font we can encode for.
fn dr_font(doc: &Document, name: &str) -> Option<(Object, FontText)> {
    let af = acroform(doc)?;
    let dr = doc.resolve(af.get(b"DR")?);
    let fonts = doc.resolve(dr.as_dict()?.get(b"Font")?);
    let entry = fonts.as_dict()?.get(name.as_bytes())?.clone();
    let font = doc.resolve(&entry);
    let fd = font.as_dict()?;
    match fd.name(b"Subtype") {
        Some(b"Type1" | b"TrueType" | b"MMType1") => {}
        // Japanese, Chinese and Korean forms: a CID font addressed by Unicode (a non-embedded
        // `/HeiseiMin-W3` with `/UniJIS-UCS2-H`, say). Embedded CMap streams and Identity-H need
        // the font's own mapping, so those still fall back to Helvetica.
        Some(b"Type0") => return Some((entry, FontText::Unicode(UnicodeCMap::from_name(fd.name(b"Encoding")?)?))),
        _ => return None,
    }
    // Symbolic fonts (ZapfDingbats, Symbol) can't show WinAnsi text.
    if matches!(fd.name(b"BaseFont"), Some(b"ZapfDingbats" | b"Symbol")) {
        return None;
    }
    Some((entry, FontText::Simple(Box::new(Metrics::from_dict(doc, fd)))))
}

/// A push button's caption, centered in a `width` × `height` box, in the font, size and color its
/// `/DA` asks for: the font from the form's `/DR`, else Helvetica, and the size `auto` when the
/// `/DA` leaves it to the viewer (0). Returns the content operators, the font resource (name and
/// object), and the first character drawn as `?` because the font can't show it.
pub(crate) fn button_caption(doc: &Document, da: &str, caption: &str, width: f64, height: f64, auto: f64) -> (Vec<u8>, String, Object, Option<char>) {
    let da = parse_da(da);
    let (name, font, enc) = match dr_font(doc, &da.font).filter(|(_, enc)| enc.can_encode(caption)) {
        Some((o, enc)) => (da.font.clone(), o, enc),
        None => ("Helv".to_string(), helvetica(), FontText::WinAnsi),
    };
    let missing = enc.first_unencodable(caption);
    let size = if da.size > 0.0 { da.size } else { auto };
    let tw = enc.width(caption, size);
    let mut out =
        format!("BT /{name} {size:.2} Tf {} 1 0 0 1 {:.3} {:.3} Tm ", da.color, (width - tw) / 2.0, (height - size * 0.7) / 2.0).into_bytes();
    out.extend(literal(&enc.encode(caption)));
    out.extend_from_slice(b" Tj ET\n");
    (out, name, font, missing)
}

/// A non-embedded Helvetica font dictionary in WinAnsi encoding, the fallback when the form's
/// `/DR` has no font that can show the text.
fn helvetica() -> Object {
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    Object::Dict(f)
}

fn color_array(doc: &Document, mk: Option<&Dict>, key: &[u8]) -> Option<String> {
    let a = doc.resolve(mk?.get(key)?);
    let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        [g] => Some(format!("{} g", n(*g))),
        [r, g, b] => Some(format!("{} {} {} rg", n(*r), n(*g), n(*b))),
        [c, m, y, k] => Some(format!("{} {} {} {} k", n(*c), n(*m), n(*y), n(*k))),
        _ => None,
    }
}

fn stroke_op(fill: &str) -> String {
    match fill.rsplit_once(' ') {
        Some((nums, "g")) => format!("{nums} G"),
        Some((nums, "rg")) => format!("{nums} RG"),
        Some((nums, "k")) => format!("{nums} K"),
        _ => "0 G".into(),
    }
}

/// Background and border from `/MK` and `/BS`; returns (content, border width). A round XFA
/// check button's ([`is_round`]) are a circle inside the widget instead of its rectangle.
fn frame(doc: &Document, wd: &Dict, w: f64, h: f64) -> (String, f64) {
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
    let mut c = String::new();
    if is_round(doc, wd) {
        return round_frame(doc, wd, mk.as_ref(), w, h);
    }
    if let Some(bg) = color_array(doc, mk.as_ref(), b"BG") {
        c.push_str(&format!("{bg}\n0 0 {} {} re f\n", n(w), n(h)));
    }
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    let bw = bs.as_ref().and_then(|b| b.get(b"W")).and_then(|x| doc.resolve(x).as_f64()).unwrap_or(1.0).max(0.0);
    if let Some(bc) = color_array(doc, mk.as_ref(), b"BC")
        && bw > 0.0
    {
        let style = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
        if style == b"U" {
            // Underline: only the bottom edge.
            c.push_str(&format!("{}\n{} w\n0 {} m {} {} l S\n", stroke_op(&bc), n(bw), n(bw / 2.0), n(w), n(bw / 2.0)));
            return (c, bw);
        }
        let dashed = style == b"D";
        c.push_str(&format!(
            "{}\n{} w\n{}{} {} {} {} re S\n[] 0 d\n",
            stroke_op(&bc),
            n(bw),
            if dashed { "[3] 0 d\n" } else { "" },
            n(bw / 2.0),
            n(bw / 2.0),
            n(w - bw),
            n(h - bw)
        ));
        if style == b"B" || style == b"I" {
            // Beveled: a light top-left and a darker bottom-right inside the border; inset:
            // grey top-left and light grey bottom-right.
            let (tl, br) = if style == b"B" { ("1 g", "0.5 g") } else { ("0.5 g", "0.75 g") };
            let (x0, y0, x1, y1) = (bw, bw, w - bw, h - bw);
            let k = bw;
            c.push_str(&format!(
                "{tl}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n{br}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n",
                n(x0),
                n(y0),
                n(x0),
                n(y1),
                n(x1),
                n(y1),
                n(x1 - k),
                n(y1 - k),
                n(x0 + k),
                n(y1 - k),
                n(x0 + k),
                n(y0 + k),
                n(x1),
                n(y1),
                n(x1),
                n(y0),
                n(x0),
                n(y0),
                n(x0 + k),
                n(y0 + k),
                n(x1 - k),
                n(y0 + k),
                n(x1 - k),
                n(y1 - k)
            ));
            return (c, 2.0 * bw);
        }
        return (c, bw);
    }
    (c, 0.0)
}

/// Whether `wd` is a check box or radio button PdfCraft laid out for a round XFA check button
/// (`/PCXfaRound true`, written by the XFA layout).
pub(crate) fn is_round(doc: &Document, wd: &Dict) -> bool {
    matches!(wd.get(b"PCXfaRound").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true)))
}

/// [`frame`] for a round check button: the `/MK /BG` background as a disc and the `/MK /BC`
/// border as a circle, both centred in the `w` × `h` box and as wide as its shorter side. A
/// dashed border stays dashed and an underline stays an underline; any other style is drawn
/// solid, and a border as wide as the circle fills it. Returns the border width.
fn round_frame(doc: &Document, wd: &Dict, mk: Option<&Dict>, w: f64, h: f64) -> (String, f64) {
    let mut c = String::new();
    let (cx, cy, r) = (w / 2.0, h / 2.0, w.min(h).max(0.0) / 2.0);
    if let Some(bg) = color_array(doc, mk, b"BG") {
        c.push_str(&format!("{bg}\n{}f\n", circle_path(cx, cy, r)));
    }
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    let bw = bs.as_ref().and_then(|b| b.get(b"W")).and_then(|x| doc.resolve(x).as_f64()).unwrap_or(1.0).max(0.0);
    let style = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
    match color_array(doc, mk, b"BC") {
        // An underline stays one: only the bottom edge, as for any widget.
        Some(bc) if bw > 0.0 && style == b"U" => {
            c.push_str(&format!("{}\n{} w\n0 {} m {} {} l S\n", stroke_op(&bc), n(bw), n(bw / 2.0), n(w), n(bw / 2.0)));
            (c, bw)
        }
        // A border as wide as the circle fills it: a disc in the border's colour.
        Some(bc) if bw > 0.0 && bw >= 2.0 * r => {
            c.push_str(&format!("{bc}\n{}f\n", circle_path(cx, cy, r)));
            (c, 2.0 * r)
        }
        Some(bc) if bw > 0.0 => {
            let dashed = style == b"D";
            // The stroke is centred on the path: inset by half its width, it stays inside.
            let path = circle_path(cx, cy, r - bw / 2.0);
            c.push_str(&format!("{}\n{} w\n{}{path}S\n[] 0 d\n", stroke_op(&bc), n(bw), if dashed { "[3] 0 d\n" } else { "" }));
            (c, bw)
        }
        _ => (c, 0.0),
    }
}

/// A closed circle of radius `r` around (`cx`, `cy`) as four Bézier arcs, ending with a space
/// so a paint operator can follow.
fn circle_path(cx: f64, cy: f64, r: f64) -> String {
    // The control-point distance for a quarter circle.
    let k = 0.5523 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c ",
        n(cx + r),
        n(cy),
        n(cx + r),
        n(cy + k),
        n(cx + k),
        n(cy + r),
        n(cx),
        n(cy + r),
        n(cx - k),
        n(cy + r),
        n(cx - r),
        n(cy + k),
        n(cx - r),
        n(cy),
        n(cx - r),
        n(cy - k),
        n(cx - k),
        n(cy - r),
        n(cx),
        n(cy - r),
        n(cx + k),
        n(cy - r),
        n(cx + r),
        n(cy - k),
        n(cx + r),
        n(cy)
    )
}

/// The width [`frame`] insets the content by: the `/BS` border width when the widget has a
/// border colour, doubled for beveled and inset borders, and 0 without a border.
fn border_width(doc: &Document, wd: &Dict) -> f64 {
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    let bw = line_width(doc, wd);
    if color_array(doc, mk.as_ref(), b"BC").is_none() || bw <= 0.0 {
        return 0.0;
    }
    let style = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
    if style == b"B" || style == b"I" { 2.0 * bw } else { bw }
}

/// The widget's border line width, `/BS /W` (default 1, Table 168), as [`frame`] strokes it.
fn line_width(doc: &Document, wd: &Dict) -> f64 {
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    // `f64::max` turns NaN into 0, as it does a negative width.
    bs.as_ref().and_then(|b| b.get(b"W")).and_then(|x| doc.resolve(x).as_f64()).unwrap_or(1.0).max(0.0)
}

/// Comb fields stop drawing their cells past this many; they would be a solid block anyway.
const MAX_COMB_DIVIDERS: usize = 500;

/// The lines between the `cells` positions of a comb field `w` × `h` (ISO 32000-2 Table 228,
/// Comb: the field is divided into `MaxLen` equally spaced positions), in the border's colour and
/// width, inside the border. None without a border, or past [`MAX_COMB_DIVIDERS`] cells.
fn comb_dividers(doc: &Document, wd: &Dict, w: f64, h: f64, cells: usize) -> String {
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
    let inset = border_width(doc, wd);
    let lw = line_width(doc, wd);
    let Some(bc) = color_array(doc, mk.as_ref(), b"BC") else { return String::new() };
    if !(inset > 0.0 && inset.is_finite() && lw > 0.0 && lw.is_finite() && (2..=MAX_COMB_DIVIDERS).contains(&cells)) {
        return String::new();
    }
    let cell = w / cells as f64;
    let mut c = format!("{}\n{} w\n", stroke_op(&bc), n(lw));
    for i in 1..cells {
        let x = cell * i as f64;
        c.push_str(&format!("{} {} m {} {} l\n", n(x), n(inset), n(x), n(h - inset)));
    }
    c.push_str("S\n");
    c
}

/// Where a list box draws its options: one row per option from the top, starting with option
/// `top` (`/TI`, §12.7.4.4). The appearance PdfCraft writes and the viewer's in-place list share
/// it, so a click picks the option drawn under the pointer (#1524).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListRows {
    /// The first option shown (`/TI`).
    pub top: usize,
    /// From the widget's top edge to the first row, in points.
    pub inset: f64,
    /// The height of one row, in points.
    pub row: f64,
    /// The text size, in points.
    pub size: f64,
}

impl ListRows {
    /// How many whole rows fit in a list box `height` points tall (at least one).
    pub fn visible(&self, height: f64) -> usize {
        if !(height.is_finite() && self.row > 0.0) {
            return 1;
        }
        let rows = ((height - self.inset) / self.row).floor();
        if rows >= 1.0 { rows.min(10_000.0) as usize } else { 1 }
    }

    /// The option at `y` points below the widget's top edge when the list starts at option `top`,
    /// or `None` above the first row. The caller checks it against the number of options.
    pub fn option_at(&self, top: usize, y: f64) -> Option<usize> {
        if !(y.is_finite() && self.row > 0.0) || y < self.inset {
            return None;
        }
        let k = ((y - self.inset) / self.row).floor();
        (k < 1.0e6).then(|| top.saturating_add(k as usize))
    }

    /// The top index that shows as many of the `selected` option indices as fit in `visible`
    /// rows, staying at `current` when they are all shown already, among `count` options.
    pub fn top_showing(current: usize, selected: &[usize], visible: usize, count: usize) -> usize {
        let visible = visible.max(1);
        let last_top = count.saturating_sub(visible);
        let (Some(&first), Some(&last)) = (selected.iter().min(), selected.iter().max()) else { return current.min(last_top) };
        let top = if first >= current && last < current.saturating_add(visible) {
            current
        } else if last.saturating_sub(first) >= visible || first < current {
            first
        } else {
            // Scroll down just far enough that the last selected option is the bottom row.
            last.saturating_add(1).saturating_sub(visible)
        };
        top.min(last_top)
    }
}

/// `/TI` of a list box: on the widget, else on the field.
fn top_index(doc: &Document, wd: &Dict, field: Option<&Dict>) -> usize {
    let ti = |d: &Dict| d.get(b"TI").and_then(|o| doc.resolve(o).as_int());
    ti(wd).or_else(|| field.and_then(ti)).and_then(|i| usize::try_from(i).ok()).unwrap_or(0)
}

/// The rows of a list box whose field's `/DA` is `f_da`, in widget `wd`.
pub(crate) fn list_rows_of(doc: &Document, f_da: &str, field: Option<&Dict>, wd: &Dict) -> ListRows {
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(f_da));
    let size = if da.size > 0.0 { da.size } else { 12.0 };
    ListRows { top: top_index(doc, wd, field), inset: 2.0 + border_width(doc, wd), row: size * 1.15, size }
}

/// The rows of list box `f` as drawn in widget `w`.
pub fn list_rows(doc: &Document, f: &Field, w: &Widget) -> ListRows {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let fobj = doc.get(f.obj);
    list_rows_of(doc, &f.da, fobj.as_dict(), &wd)
}

/// The appearance of a text or choice field's widget showing `values`. Characters its font can't
/// hold come out as `?`; [`field_appearance_drawn`] says which.
pub fn field_appearance(doc: &Document, f: &Field, w: &Widget, values: &[String]) -> Stream {
    field_appearance_as(doc, f, w, values, true)
}

/// [`field_appearance`], where `format` false means `values` are already what to show (a
/// custom Format script ran).
pub fn field_appearance_as(doc: &Document, f: &Field, w: &Widget, values: &[String], format: bool) -> Stream {
    field_appearance_drawn(doc, f, w, values, format).0
}

/// [`field_appearance_as`] with the first character it drew as `?` because the field's font can't
/// hold it: the form's font is a simple (WinAnsi) font, Helvetica stands in for one PdfCraft can't
/// write with (an embedded subset, `Identity-H`), or a UCS-2 CMap meets a character beyond U+FFFF.
pub fn field_appearance_drawn(doc: &Document, f: &Field, w: &Widget, values: &[String], format: bool) -> (Stream, Option<char>) {
    let (stream, unencodable, _) = field_appearance_fit(doc, f, w, values, format);
    (stream, unencodable)
}

/// A text value that its box cuts off at the field's font size (#4018).
#[derive(Clone, Debug, PartialEq)]
pub enum Overflow {
    /// One line `width` points wide, with `room` for it, at `size` points.
    Wide { width: f64, room: f64, size: f64 },
    /// `lines` wrapped lines, of which the box shows `room`, at `size` points.
    Tall { lines: usize, room: usize, size: f64 },
    /// One line `height` points high (ascent to descent), with `room` for it, at `size` points.
    High { height: f64, room: f64, size: f64 },
}

impl std::fmt::Display for Overflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wide { width, room, size } => {
                write!(f, "the value does not fit: {} pt wide at {} pt, room for {} pt", fmt(*width), fmt(*size), fmt(*room))
            }
            Self::Tall { lines, room, size } => write!(f, "the value does not fit: {lines} lines at {} pt, room for {room}", fmt(*size)),
            Self::High { height, room, size } => {
                write!(f, "the value does not fit: {} pt high at {} pt, room for {} pt", fmt(*height), fmt(*size), fmt(*room))
            }
        }
    }
}

/// Whether the text field's value, drawn in widget `w`, is cut off by its box.
pub fn value_overflow(doc: &Document, f: &Field, w: &Widget) -> Option<Overflow> {
    if f.kind != FieldKind::Text {
        return None;
    }
    field_appearance_fit(doc, f, w, &f.value, true).2
}

/// [`field_appearance_drawn`], and whether the text drawn is cut off by the box.
fn field_appearance_fit(doc: &Document, f: &Field, w: &Widget, values: &[String], format: bool) -> (Stream, Option<char>, Option<Overflow>) {
    // The Format event: what is shown, not what is stored.
    let formatted: Vec<String>;
    let values = if format
        && matches!(f.kind, crate::FieldKind::Text | crate::FieldKind::Combo)
        && values.len() == 1
        && f.actions.format != crate::af::Format::None
    {
        formatted = vec![crate::af::format_value(&f.actions.format, &values[0])];
        &formatted[..]
    } else {
        values
    };
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(&f.da));
    let text: String = if f.kind == FieldKind::Combo {
        values.iter().map(|v| f.options.iter().find(|(e, _)| e == v).map_or(v.clone(), |(_, d)| d.clone())).collect::<Vec<_>>().join(", ")
    } else {
        values.first().cloned().unwrap_or_default()
    };
    let text = if f.has(flags::PASSWORD) { "*".repeat(text.chars().count()) } else { text };
    let can_show = |enc: &FontText| {
        if f.kind == FieldKind::List {
            f.options.iter().all(|(_, display)| enc.can_encode(display))
        } else if f.kind == FieldKind::Text && f.has(flags::MULTILINE) {
            text.split(['\n', '\r']).all(|line| enc.can_encode(line))
        } else {
            enc.can_encode(&text)
        }
    };
    let (font_name, font_obj, enc) = match dr_font(doc, &da.font).filter(|(_, enc)| can_show(enc)) {
        Some((o, enc)) => (da.font.clone(), o, enc),
        None => ("Helv".to_string(), helvetica(), FontText::WinAnsi),
    };
    let width_of = |text: &str, size: f64| enc.width(text, size);
    let (mut c, bw) = frame(doc, &wd, width, height);
    let pad = 2.0 + bw;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let q = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let mut body: Vec<u8> = Vec::new();
    // The first character drawn that the font can't hold (drawn as `?`), for the caller to report.
    let mut unencodable: Option<char> = None;
    let mut show = |body: &mut Vec<u8>, x: f64, y: f64, text: &str| {
        // A line break in a single-line value is a space, as in viewers (multiline text is split).
        let text: String = text.chars().map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c }).collect();
        if unencodable.is_none() {
            unencodable = enc.first_unencodable(&text);
        }
        body.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
        body.extend(literal(&enc.encode(&text)));
        body.extend_from_slice(b" Tj\n");
    };
    let x_for = |text: &str, size: f64| -> f64 {
        let tw = width_of(text, size);
        match q {
            1 => pad + (inner_w - tw) / 2.0,
            2 => width - pad - tw,
            _ => pad,
        }
    };
    let mut size = da.size;
    let mut overflow = None;
    match f.kind {
        FieldKind::List => {
            // Every option, one per line from the top (from `/TI`); selected ones highlighted.
            let fobj = doc.get(f.obj);
            let rows = list_rows_of(doc, &f.da, fobj.as_dict(), &wd);
            size = rows.size;
            let line = rows.row;
            let mut y = height - rows.inset;
            for (export, display) in f.options.iter().skip(rows.top) {
                if y - line < 0.0 {
                    break;
                }
                if values.contains(export) {
                    c.push_str(&format!("0.6 0.75 0.86 rg\n{} {} {} {} re f\n", n(bw), n(y - line), n(width - 2.0 * bw), n(line)));
                }
                show(&mut body, pad, y - line + size * 0.25, display);
                y -= line;
            }
        }
        _ => {
            let comb = f.is_comb();
            if f.kind == FieldKind::Text && f.has(flags::MULTILINE) {
                if size == 0.0 {
                    size = auto_text_size(&text, inner_w, height - 2.0 * pad, true, width_of);
                }
                let mut y = height - pad - size * 0.85;
                let lines = wrap_with(&text, size, inner_w, width_of);
                // The lines whose descent stays inside the border; the rest are cut off.
                let mut room = 0;
                for line in &lines {
                    if y < -size {
                        break;
                    }
                    if y - 0.207 * size >= bw - 0.01 {
                        room += 1;
                    }
                    show(&mut body, x_for(line, size), y, line);
                    y -= size * 1.15;
                }
                if room < lines.len() {
                    overflow = Some(Overflow::Tall { lines: lines.len(), room, size });
                } else if let Some(tw) = lines.iter().map(|l| width_of(l, size)).filter(|tw| *tw > inner_w + 0.01).reduce(f64::max) {
                    // A character wider than the box is a line of its own, still cut at the sides.
                    overflow = Some(Overflow::Wide { width: tw, room: inner_w, size });
                }
            } else {
                if size == 0.0 {
                    size = if comb {
                        comb_auto_text_size(height - 2.0 * pad)
                    } else {
                        auto_text_size(&text, inner_w, height - 2.0 * pad, false, width_of)
                    };
                }
                // Centre Helvetica's ascent (0.718) and descent (0.207) vertically.
                let y = (height - 0.925 * size) / 2.0 + 0.207 * size;
                // The clip box is inside the border; a line taller than it is cut at the top and bottom.
                let room_h = height - 2.0 * bw;
                if 0.925 * size > room_h + 0.01 {
                    overflow = Some(Overflow::High { height: 0.925 * size, room: room_h, size });
                }
                if comb {
                    let cells = f.max_len.unwrap_or(1).max(1);
                    let cell = width / cells as f64;
                    c.push_str(&comb_dividers(doc, &wd, width, height, cells));
                    for (i, ch) in text.chars().take(cells).enumerate() {
                        let s = ch.to_string();
                        show(&mut body, cell * i as f64 + (cell - width_of(&s, size)) / 2.0, y, &s);
                    }
                } else {
                    let tw = width_of(&text.replace(['\n', '\r'], " "), size);
                    if overflow.is_none() && tw > inner_w + 0.01 {
                        overflow = Some(Overflow::Wide { width: tw, room: inner_w, size });
                    }
                    show(&mut body, x_for(&text, size), y, &text);
                }
            }
        }
    }
    // Acrobat regenerates only what is inside /Tx BMC when it redraws a field (e.g. on focus
    // changes), discarding what came before it (issues #1613, #633). The background and border
    // therefore go inside the marked-content block too, so a redraw keeps them with the text.
    let mut content = format!(
        "/Tx BMC\nq\n{c}{} {} {} {} re W n\nBT\n/{} {} Tf\n{}\n",
        n(bw),
        n(bw),
        n(width - 2.0 * bw),
        n(height - 2.0 * bw),
        font_name,
        n(size),
        da.color
    )
    .into_bytes();
    content.extend(body);
    content.extend_from_slice(b"ET\nQ\nEMC\n");
    let mut fonts = Dict::new();
    fonts.set(font_name.into_bytes(), font_obj);
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(res));
    // Nothing drawn is nothing cut, however small the box.
    let overflow = overflow.filter(|_| !text.is_empty());
    (Stream::flate(d, &content), unencodable, overflow)
}

/// On/Off appearances for a check box or radio button that has none (a check mark or a dot,
/// drawn as paths, so no symbol font is needed).
/// The mark is drawn in the colour of the widget's `/DA`, else of `f_da` (the field's): the
/// Appearance tab's Text Color (#4261).
/// The streams are added to `doc` as indirect objects (streams can't be direct objects).
pub fn check_box_states(doc: &mut Document, w: &Widget, kind: FieldKind, on_name: &str, f_da: &str) -> Dict {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let (frame_c, _) = frame(doc, &wd, width, height);
    let style = crate::author::CheckStyle::of_widget(doc, &wd, kind);
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(f_da));
    let mut form = |content: String| -> Object {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        set_form_box(&mut d, place);
        Object::Ref(doc.add(Object::Stream(Stream::flate(d, content.as_bytes()))))
    };
    let s = width.min(height);
    let mark = check_mark(style, width / 2.0, height / 2.0, s, &da.color);
    let mut nd = Dict::new();
    nd.set(crate::name_bytes(on_name), form(format!("{frame_c}{mark}")));
    nd.set(b"Off".to_vec(), form(frame_c));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Dict(nd));
    ap
}

/// The on-state mark for `style` (#94), centred on (`cx`, `cy`) in a box whose smaller side is
/// `s`, drawn in the fill colour operation `color` (`0 g`, `0 0 1 rg`).
fn check_mark(style: crate::author::CheckStyle, cx: f64, cy: f64, s: f64, color: &str) -> String {
    use crate::author::CheckStyle;
    let (fill, stroke) = (color, stroke_op(color));
    // A filled polygon through `pts`.
    let polygon = |pts: &[(f64, f64)]| {
        let mut c = format!("{fill}\n");
        for (i, (x, y)) in pts.iter().enumerate() {
            c.push_str(&format!("{} {} {} ", n(*x), n(*y), if i == 0 { "m" } else { "l" }));
        }
        c.push_str("h f\n");
        c
    };
    let line = n((s * 0.1).max(1.0));
    match style {
        CheckStyle::Check => format!(
            "{stroke}\n{line} w 1 J 1 j\n{} {} m {} {} l {} {} l S\n",
            n(cx - s * 0.28),
            n(cy),
            n(cx - s * 0.08),
            n(cy - s * 0.22),
            n(cx + s * 0.3),
            n(cy + s * 0.25)
        ),
        CheckStyle::Cross => {
            let d = s * 0.25;
            format!(
                "{stroke}\n{line} w 1 J\n{} {} m {} {} l S\n{} {} m {} {} l S\n",
                n(cx - d),
                n(cy - d),
                n(cx + d),
                n(cy + d),
                n(cx - d),
                n(cy + d),
                n(cx + d),
                n(cy - d)
            )
        }
        CheckStyle::Square => {
            let d = s * 0.22;
            format!("{fill}\n{} {} {} {} re f\n", n(cx - d), n(cy - d), n(2.0 * d), n(2.0 * d))
        }
        CheckStyle::Diamond => {
            let d = s * 0.3;
            polygon(&[(cx, cy + d), (cx + d, cy), (cx, cy - d), (cx - d, cy)])
        }
        CheckStyle::Star => {
            // Five points: outer and inner radii alternate, the first point straight up.
            let (outer, inner) = (s * 0.32, s * 0.32 * 0.382);
            let pts: Vec<(f64, f64)> = (0..10)
                .map(|i| {
                    let a = std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::PI / 5.0;
                    let r = if i % 2 == 0 { outer } else { inner };
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect();
            polygon(&pts)
        }
        CheckStyle::Circle => {
            let r = s * 0.25;
            let k = 0.5523 * r;
            format!(
                "{fill}\n{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f\n",
                n(cx + r),
                n(cy),
                n(cx + r),
                n(cy + k),
                n(cx + k),
                n(cy + r),
                n(cx),
                n(cy + r),
                n(cx - k),
                n(cy + r),
                n(cx - r),
                n(cy + k),
                n(cx - r),
                n(cy),
                n(cx - r),
                n(cy - k),
                n(cx - k),
                n(cy - r),
                n(cx),
                n(cy - r),
                n(cx + k),
                n(cy - r),
                n(cx + r),
                n(cy - k),
                n(cx + r),
                n(cy)
            )
        }
    }
}

/// How a check box or radio button widget `w` of field `f` looks in state on (`on`) or Off when
/// the file has no appearance for that state, or leaves its appearances to the viewer
/// (`/NeedAppearances`): its `/MK` background and border ([`frame`]), and when on, its caption
/// `/MK /CA` (ISO 32000-2 Table 192: the normal caption, which check boxes and radio buttons may
/// use too) as a ZapfDingbats character, as in the check box example of
/// 12.7.5.2.3: `4` (a check mark) by default, `l` (a dot) for a radio button. The character is
/// drawn in the `/DA` colour, at the `/DA` size or, for an automatic size (`0 Tf`), filling most
/// of the box. ZapfDingbats is one of the standard 14 fonts, so no font program is needed.
pub(crate) fn button_state_shown(doc: &Document, f: &Field, w: &Widget, on: bool) -> Stream {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let (frame_c, bw) = frame(doc, &wd, width, height);
    let mut content = frame_c.into_bytes();
    let mut res = Dict::new();
    if on {
        let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
        let caption = mk.as_ref().and_then(|mk| mk.get(b"CA")).and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text()));
        let dingbats = pdfcraft_fonts::Std14::ZapfDingbats;
        // The caption's first character as a ZapfDingbats code, when the font has a glyph there.
        let code = caption
            .and_then(|s| s.chars().next())
            .and_then(|ch| u8::try_from(u32::from(ch)).ok())
            .filter(|&code| dingbats.width(code) > 0.0)
            .unwrap_or(if f.kind == FieldKind::Radio { b'l' } else { b'4' });
        let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(&f.da));
        // The glyph's advance and (about) its height, in ems.
        let (advance, tall) = (dingbats.width(code) / 1000.0, 0.75);
        let room = |side: f64| (side - 2.0 * bw - 2.0).max(1.0);
        // An automatic size leaves a margin of a tenth of the room on each side.
        let size = if da.size > 0.0 { da.size } else { (0.8 * room(width) / advance.max(0.1)).min(0.8 * room(height) / tall).clamp(1.0, 300.0) };
        let (x, y) = ((width - advance * size) / 2.0, (height - tall * size) / 2.0 + 0.05 * size);
        content.extend(format!("q\nBT\n/ZaDb {} Tf\n{}\n{} {} Td\n", n(size), da.color, n(x), n(y)).bytes());
        content.extend(literal(&[code]));
        content.extend_from_slice(b" Tj\nET\nQ\n");
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("ZapfDingbats"));
        let mut fonts = Dict::new();
        fonts.set(b"ZaDb".to_vec(), Object::Dict(font));
        res.set(b"Font".to_vec(), Object::Dict(fonts));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Stream::flate(d, &content)
}

/// How a push button widget `w` of field `f` with no normal appearance looks: its `/MK`
/// background and border, in the `/BS` width and style ([`frame`]: Table 168, `B` beveled, `I`
/// inset), with its caption `/MK /CA` (Table 192) centred in the `/DA` font, size and colour.
/// A button with an icon (`/MK /I`) is drawn as PdfCraft draws its own buttons, icon included.
pub(crate) fn push_button_shown(doc: &Document, f: &Field, w: &Widget) -> Stream {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
    if mk.contains(b"I") {
        return crate::author::button_appearance(doc, w, &f.da).0;
    }
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let (frame_c, _) = frame(doc, &wd, width, height);
    let mut content = frame_c.into_bytes();
    let caption = mk.get(b"CA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).unwrap_or_default();
    let mut fonts = Dict::new();
    if !caption.is_empty() && mk.get(b"TP").and_then(|t| doc.resolve(t).as_int()) != Some(1) {
        let da = wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text()));
        // The automatic size PdfCraft's own buttons use.
        let auto = ((height - 4.0) * 0.6).clamp(4.0, 14.0);
        let (ops, name, font, _) = button_caption(doc, da.as_deref().unwrap_or(&f.da), &caption, width, height, auto);
        content.extend(ops);
        fonts.set(name.into_bytes(), font);
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Stream::flate(d, &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_appearance_strings_parse() {
        assert_eq!(parse_da("/Helv 0 Tf 0 g"), Da { font: "Helv".into(), size: 0.0, color: "0 g".into() });
        assert_eq!(parse_da("0.1 0.2 0.3 rg /F1 11 Tf"), Da { font: "F1".into(), size: 11.0, color: "0.1 0.2 0.3 rg".into() });
        assert_eq!(parse_da("/Cour 9 Tf 0 0 0 1 k").color, "0 0 0 1 k");
        assert_eq!(parse_da("garbage").font, "Helv");
        assert_eq!(stroke_op("1 0 0 rg"), "1 0 0 RG");
    }

    #[test]
    fn list_rows_visibility_and_scrolling() {
        let rows = ListRows { top: 0, inset: 2.0, row: 11.5, size: 10.0 };
        assert_eq!(rows.visible(46.0), 3);
        assert_eq!(rows.option_at(0, 1.0), None);
        assert_eq!(rows.option_at(0, 5.0), Some(0));
        assert_eq!(rows.option_at(0, 15.0), Some(1));
        assert_eq!(rows.option_at(2, 5.0), Some(2));

        // When Tue (1) and Thu (3) are selected in 5 options with 3 visible rows:
        assert_eq!(ListRows::top_showing(0, &[1, 3], 3, 5), 1);
        // When staying within view:
        assert_eq!(ListRows::top_showing(1, &[1, 2], 3, 5), 1);
        // When selecting before current top:
        assert_eq!(ListRows::top_showing(2, &[0], 3, 5), 0);
        // When empty selection:
        assert_eq!(ListRows::top_showing(0, &[], 3, 5), 0);
    }
}
