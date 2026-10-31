//! Template XML → [`model`](crate::model). Only what the layout engine uses is read; everything
//! else in the template is ignored, never rejected.

use base64::Engine;
use roxmltree::Node as XmlNode;

use crate::XfaError;
use crate::model::{
    Access, Anchor, Area, Border, Caption, ChoiceList, Color, Common, Corner, Draw, Edge, ExclGroup, Field, FontSpec, HAlign, Layout, Margin, Node,
    Occur, PageArea, PageRole, PageSet, ParaSpec, Paragraph, Placement, Presence, Prop, Props, Rect, RichText, Run, Script, Stroke, Subform,
    Template, Ui, VAlign, Value,
};

/// Deepest element nesting followed (templates are shallow; hostile ones are not).
const MAX_DEPTH: usize = 64;
/// Most XML nodes a template may have.
const MAX_NODES: u32 = 2_000_000;
/// Largest decoded inline image.
const MAX_IMAGE: usize = 32 << 20;
/// Most items a choice list keeps.
const MAX_ITEMS: usize = 1000;
/// Longest text run kept.
const MAX_TEXT: usize = 1 << 20;

/// Parse a measurement such as `12.7mm`, `72pt`, `1in`, `2.54cm` into points. Bare numbers are
/// points. `None` for anything else (including `em`).
pub fn measure(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let v: f64 = num.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let k = match unit.trim() {
        "" | "pt" => 1.0,
        "mm" => 72.0 / 25.4,
        "cm" => 72.0 / 2.54,
        "in" => 72.0,
        "px" => 0.75,
        "pc" => 12.0,
        _ => return None,
    };
    Some((v * k).clamp(-100_000.0, 100_000.0))
}

/// The attribute `name` of `n` read as a measurement in points (see [`measure`]); `None`
/// when it is missing or unreadable.
fn attr_measure(n: XmlNode, name: &str) -> Option<f64> {
    n.attribute(name).and_then(measure)
}

/// A horizontal scale such as `90%` (a bare number is a percentage too), as a fraction, within
/// 1% and 1000%; `None` when it isn't a positive number.
fn scale(v: &str) -> Option<f64> {
    let v: f64 = v.trim().trim_end_matches('%').trim().parse().ok()?;
    (v.is_finite() && v > 0.0).then(|| (v / 100.0).clamp(0.01, 10.0))
}

/// The first child element of `n` named `name` (by local name, in any namespace).
fn child<'a, 'd>(n: XmlNode<'a, 'd>, name: &str) -> Option<XmlNode<'a, 'd>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'd>(n: XmlNode<'a, 'd>, name: &'a str) -> impl Iterator<Item = XmlNode<'a, 'd>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The text of an image element: base64 is read whole up to the image cap (`text_of` stops at
/// 1 MiB, which would cut a photo short).
fn image_text_of(n: XmlNode) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for d in n.descendants() {
        if d.is_text()
            && let Some(t) = d.text()
        {
            out.extend(t.bytes().filter(|b| !b.is_ascii_whitespace()));
            if out.len() > MAX_IMAGE / 3 * 4 + 4 {
                return None;
            }
        }
    }
    Some(out)
}

fn text_of(n: XmlNode) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if d.is_text()
            && let Some(t) = d.text()
        {
            s.push_str(t);
            if s.len() > MAX_TEXT {
                s.truncate(s.char_indices().take_while(|(i, _)| *i < MAX_TEXT).last().map_or(0, |(i, c)| i + c.len_utf8()));
                break;
            }
        }
    }
    s
}

fn parse_margin(n: Option<XmlNode>) -> Margin {
    let Some(n) = n else { return Margin::default() };
    Margin {
        top: attr_measure(n, "topInset").unwrap_or(0.0),
        right: attr_measure(n, "rightInset").unwrap_or(0.0),
        bottom: attr_measure(n, "bottomInset").unwrap_or(0.0),
        left: attr_measure(n, "leftInset").unwrap_or(0.0),
    }
}

fn parse_color(n: Option<XmlNode>) -> Option<Color> {
    n.and_then(|c| c.attribute("value")).and_then(Color::parse)
}

fn parse_edge(n: XmlNode) -> Edge {
    Edge {
        presence: Presence::parse(n.attribute("presence")),
        thickness: attr_measure(n, "thickness").unwrap_or(0.5).max(0.0),
        color: parse_color(child(n, "color")).unwrap_or(Color::BLACK),
        stroke: match n.attribute("stroke") {
            Some("dashed" | "dashDot" | "dashDotDot") => Stroke::Dashed,
            Some("dotted") => Stroke::Dotted,
            Some("raised" | "lowered" | "etched" | "embossed") => Stroke::Relief,
            _ => Stroke::Solid,
        },
    }
}

/// `<fill>`: a colour, or white when the element is present without one.
fn parse_fill(n: Option<XmlNode>) -> Option<Color> {
    let n = n?;
    if Presence::parse(n.attribute("presence")) != Presence::Visible {
        return None;
    }
    Some(parse_color(child(n, "color")).unwrap_or(Color::WHITE))
}

/// `<border>`: edges top, right, bottom, left (XFA 3.3 p. 38), the sides not listed drawn as
/// readers draw them ([`drawn_side`]), not as p. 38's text says.
fn parse_border(n: Option<XmlNode>) -> Option<Border> {
    let n = n?;
    let edges: Vec<Edge> = children(n, "edge").take(4).map(parse_edge).collect();
    let four = std::array::from_fn(|i| drawn_side(edges.len(), i).and_then(|k| edges.get(k).copied()).unwrap_or_default());
    Some(Border { presence: Presence::parse(n.attribute("presence")), edges: four, corners: parse_corners(n), fill: parse_fill(child(n, "fill")) })
}

/// Which of the `listed` edges (or corners) of a border is drawn on side `index`: edges go
/// top, right, bottom, left and corners top-left, top-right, bottom-right, bottom-left (XFA 3.3
/// p. 38). For a border that lists fewer than four, p. 38 says the last one listed is reused
/// for the rest; readers draw a side not listed from the edge or corner on the same side
/// instead, and so does this: one serves every side, two alternate (0, 1, 0, 1), three reuse
/// the second for the fourth (0, 1, 2, 1). Edges already followed the readers; corners now do
/// too. The scripting host's reads mirror the same pattern so a script reads what is drawn.
/// `None` when none is listed.
pub(crate) fn drawn_side(listed: usize, index: usize) -> Option<usize> {
    match listed {
        0 => None,
        _ if index < listed => Some(index),
        1 => Some(0),
        2 => Some(index % 2),
        _ => Some(1),
    }
}

/// The `<corner>` children of a border or rectangle, top-left, top-right, bottom-right then
/// bottom-left, the ones not listed drawn as [`drawn_side`] picks, as edges are. Without any,
/// the corners are square.
fn parse_corners(n: XmlNode) -> [Corner; 4] {
    let given: Vec<Corner> = children(n, "corner")
        .take(4)
        .map(|c| Corner {
            presence: Presence::parse(c.attribute("presence")),
            radius: attr_measure(c, "radius").filter(|r| r.is_finite()).unwrap_or(0.0).max(0.0),
            inverted: c.attribute("inverted") == Some("1"),
        })
        .collect();
    std::array::from_fn(|i| drawn_side(given.len(), i).and_then(|k| given.get(k).copied()).unwrap_or_default())
}

/// `<font>`: typeface, size, weight, posture, underline, fill color and horizontal scale.
/// What the element doesn't set, or a missing element, is left `None` to inherit.
fn parse_font(n: Option<XmlNode>) -> FontSpec {
    let Some(n) = n else { return FontSpec::default() };
    FontSpec {
        typeface: n.attribute("typeface").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        size: attr_measure(n, "size").filter(|s| *s > 0.0),
        bold: n.attribute("weight").map(|w| w == "bold"),
        italic: n.attribute("posture").map(|p| p == "italic"),
        underline: n.attribute("underline").map(|u| u != "0"),
        color: parse_color(child(n, "fill").and_then(|f| child(f, "color"))),
        h_scale: n.attribute("fontHorizontalScale").and_then(scale),
    }
}

fn parse_para(n: Option<XmlNode>) -> ParaSpec {
    let Some(n) = n else { return ParaSpec::default() };
    ParaSpec {
        h_align: HAlign::parse(n.attribute("hAlign")),
        v_align: VAlign::parse(n.attribute("vAlign")),
        line_height: attr_measure(n, "lineHeight").filter(|v| *v > 0.0),
        margin_left: attr_measure(n, "marginLeft"),
        margin_right: attr_measure(n, "marginRight"),
        space_above: attr_measure(n, "spaceAbove"),
        space_below: attr_measure(n, "spaceBelow"),
        text_indent: attr_measure(n, "textIndent"),
    }
}

fn parse_common(n: XmlNode, warnings: &mut Vec<String>) -> Common {
    Common {
        props: std::sync::Arc::new(parse_props(n, warnings)),
        name: n.attribute("name").map(str::to_string).filter(|s| !s.is_empty()),
        id: n.attribute("id").map(str::to_string),
        x: attr_measure(n, "x"),
        y: attr_measure(n, "y"),
        anchor: Anchor::parse(n.attribute("anchorType")),
        w: attr_measure(n, "w"),
        h: attr_measure(n, "h"),
        min_w: attr_measure(n, "minW"),
        max_w: attr_measure(n, "maxW"),
        min_h: attr_measure(n, "minH"),
        max_h: attr_measure(n, "maxH"),
        presence: Presence::parse(n.attribute("presence")),
        margin: parse_margin(child(n, "margin")),
        border: parse_border(child(n, "border")),
        font: parse_font(child(n, "font")),
        para: parse_para(child(n, "para")),
        col_span: n.attribute("colSpan").and_then(|c| c.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 64),
    }
}

/// The property elements of an object that scripts read through the object model (XFA 3.3
/// ch. 17: border p. 629, font p. 741, para p. 800, margin p. 776, ui p. 877, validate p. 880,
/// occur p. 786, break p. 632, breakBefore p. 637, breakAfter p. 635, assist p. 609, format
/// p. 745, value p. 882, bind p. 623, desc p. 674, extras p. 724, keep p. 762, caption p. 643,
/// items p. 758, calculate p. 641).
const PROP_TAGS: &[&str] = &[
    "border",
    "font",
    "para",
    "margin",
    "ui",
    "validate",
    "occur",
    "break",
    "breakBefore",
    "breakAfter",
    "assist",
    "format",
    "value",
    "bind",
    "desc",
    "extras",
    "keep",
    "caption",
    "items",
    "calculate",
];
/// Most property elements kept for one object, all levels together. A field written by a form
/// designer holds about 40 (four edges and corners with their colours, a fill, a font, a widget
/// with its own border); past this the rest are left out and a warning says so.
pub const MAX_OBJECT_PROPS: usize = 256;
/// Most bytes of tags, attribute names, values and text kept for one object's properties: a
/// designer's field holds well under 2 KiB.
pub const MAX_OBJECT_PROP_BYTES: usize = 16 << 10;
/// Deepest property element kept below an object (`ui > textEdit > border > edge > color` is 5).
const MAX_PROP_DEPTH: usize = 8;
/// Most attributes kept on one element.
const MAX_PROP_ATTRS: usize = 64;
/// Longest validation or calculation script kept for scripts to read, in bytes: the script
/// engine's own limit on a script (`pdfcraft_js`'s `MAX_SCRIPT_BYTES`), past which it would not run anyway.
const MAX_VALIDATE_SCRIPT_BYTES: usize = 256 * 1024;
/// The warning when an object's properties passed a limit.
pub const PROPS_CUT_WARNING: &str = "an object has more property elements, attributes or text than scripts are given; the rest are left out";

/// What is left of one object's property budget.
struct PropBudget {
    elements: usize,
    bytes: usize,
    cut: bool,
}

impl PropBudget {
    /// `s` when the bytes are left for it (charged), else `None` and the cut noted.
    fn text(&mut self, s: &str) -> Option<String> {
        match self.bytes.checked_sub(s.len()) {
            Some(left) => {
                self.bytes = left;
                Some(s.to_string())
            }
            None => {
                self.cut = true;
                None
            }
        }
    }

    /// The attributes of `n`, to [`MAX_PROP_ATTRS`] and while the bytes last.
    fn attrs(&mut self, n: XmlNode) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for a in n.attributes() {
            if out.len() >= MAX_PROP_ATTRS {
                self.cut = true;
                break;
            }
            let Some(name) = self.text(a.name()) else { break };
            let Some(value) = self.text(a.value()) else { break };
            out.push((name, value));
        }
        out
    }
}

/// Most `<items>` of one object kept as lists ([`items_prop`]): a field has two at most, the
/// shown list and the saved values (XFA 3.3 p. 726, `<items/> [0..2]`), the two the field's own
/// lists are read from. Forms do carry more (one public form has a third, empty, saved list);
/// any further one is kept as an ordinary property, every entry charged to the object's budget.
const MAX_ITEM_LISTS: usize = 2;

/// An `<items>` element (XFA 3.3 p. 758) as scripts read it: its attributes and its entries,
/// each with its tag and text, as the choice list or check button shows them ([`parse_items`]:
/// the same [`MAX_ITEMS`] and the same text), so a script reads what the list holds. The
/// element and its attributes are charged to the object's budget like any property; its
/// entries are not (a list of a few hundred countries would otherwise crowd out every other
/// property), so they are bounded by the list's own limits instead: [`MAX_ITEMS`] entries of
/// at most [`MAX_TEXT`] each, in at most [`MAX_ITEM_LISTS`] lists, no more than the field's
/// own lists hold. An entry's attributes are not kept: scripts read an entry's value.
fn items_prop(n: XmlNode, b: &mut PropBudget) -> Option<Prop> {
    if b.elements == 0 {
        b.cut = true;
        return None;
    }
    b.elements -= 1;
    let tag = b.text(n.tag_name().name())?;
    let attrs = b.attrs(n);
    let children = n
        .children()
        .filter(|c| c.is_element())
        .take(MAX_ITEMS)
        .map(|c| Prop { tag: c.tag_name().name().to_string(), attrs: Vec::new(), text: Some(std::sync::Arc::from(text_of(c))), children: Vec::new() })
        .collect();
    Some(Prop { tag, attrs, text: None, children })
}

/// The property elements of `n` scripts read: its own attributes and the [`PROP_TAGS`]
/// elements with what they hold, within [`MAX_OBJECT_PROPS`] elements,
/// [`MAX_OBJECT_PROP_BYTES`] of text, [`MAX_PROP_DEPTH`] levels and [`MAX_PROP_ATTRS`]
/// attributes an element; past them the rest is left out and [`PROPS_CUT_WARNING`] is added
/// once. Only elements in the object's namespace are properties (XFA 3.3 p. 598); a `script` is
/// read only in a `validate` or a `calculate`, and only its first (forms read its text, which
/// is not charged to the budget but kept within [`MAX_VALIDATE_SCRIPT_BYTES`]), the first
/// [`MAX_ITEM_LISTS`] `items` as lists (see [`items_prop`]), `extras` only as the object's own, the XHTML
/// of `exData` not at all, and of `value` only its content element's tag and attributes (the
/// data is the object's value, read elsewhere).
fn parse_props(n: XmlNode, warnings: &mut Vec<String>) -> Props {
    /// The property elements under `n`, whose tag is `within` (empty for the object itself):
    /// `extras` in the object and inside one (forms keep named values there), `script` in a
    /// `validate` or a `calculate` (XFA 3.3 p. 880, p. 641, p. 827).
    fn elements<'a, 'd>(n: XmlNode<'a, 'd>, within: &str) -> impl Iterator<Item = XmlNode<'a, 'd>> + use<'a, 'd> {
        let (extras, script) = (within.is_empty() || within == "extras", matches!(within, "validate" | "calculate"));
        let ns = n.tag_name().namespace();
        n.children().filter(move |c| {
            c.is_element()
                && c.tag_name().namespace() == ns
                && match c.tag_name().name() {
                    "script" => script,
                    "extras" => extras,
                    _ => true,
                }
        })
    }
    /// `n` as a property, `depth` below the object; `bare`: its tag and attributes only.
    fn prop(n: XmlNode, depth: usize, bare: bool, b: &mut PropBudget) -> Option<Prop> {
        if depth > MAX_PROP_DEPTH || b.elements == 0 {
            b.cut = true;
            return None;
        }
        b.elements -= 1;
        let tag = b.text(n.tag_name().name())?;
        let mut p = Prop { attrs: b.attrs(n), tag, text: None, children: Vec::new() };
        if bare {
            return Some(p);
        }
        if p.tag == "exData" {
            // Rich text: its words, as scripts read a caption's rich text (`exData.value`), not
            // its XHTML elements.
            let t = text_of(n);
            let t = t.trim();
            if !t.is_empty() {
                p.text = b.text(t).map(std::sync::Arc::from);
            }
            return Some(p);
        }
        if p.tag == "script" {
            // A validation's or calculation's script: its text is not charged to the budget, so a long script
            // leaves the other properties their room, but it is kept only within the script
            // engine's own limit on a script, past which it would not run anyway.
            let t = text_of(n);
            let t = t.trim();
            if t.len() > MAX_VALIDATE_SCRIPT_BYTES {
                b.cut = true;
            } else if !t.is_empty() {
                p.text = Some(std::sync::Arc::from(t));
            }
            return Some(p);
        }
        let mut kids = elements(n, p.tag.as_str()).peekable();
        if kids.peek().is_none() {
            // The element's own text only: not that of children left out (an `extras`, a
            // `script`), and no more than the budget holds.
            let mut own = String::new();
            for t in n.children().filter(|c| c.is_text()).filter_map(|c| c.text()) {
                if own.len().saturating_add(t.len()) > b.bytes {
                    b.cut = true;
                    own.clear();
                    break;
                }
                own.push_str(t);
            }
            let t = own.trim();
            if !t.is_empty() {
                p.text = b.text(t).map(std::sync::Arc::from);
            }
            return Some(p);
        }
        // The object's own value holds its data, read elsewhere: of it only the content
        // element's tag. A caption's value is its label, and is read whole.
        let content_only = p.tag == "value" && depth == 1;
        // A validation or a calculation has one script (XFA 3.3 p. 880, p. 641: [0..1]); any
        // more are not read.
        let mut scripts = 0usize;
        for k in kids {
            if k.tag_name().name() == "script" {
                scripts = scripts.saturating_add(1);
                if scripts > 1 {
                    continue;
                }
            }
            match prop(k, depth + 1, content_only, b) {
                Some(c) => p.children.push(c),
                None => break,
            }
        }
        Some(p)
    }
    let mut b = PropBudget { elements: MAX_OBJECT_PROPS, bytes: MAX_OBJECT_PROP_BYTES, cut: false };
    let attrs = b.attrs(n);
    let mut out = Vec::new();
    // `desc` and `extras` last: they can hold the most (forms keep data there), so a cut falls
    // on them before the appearance and validation properties (their order means nothing,
    // XFA 3.3 p. 78).
    let bulky = |c: &XmlNode| matches!(c.tag_name().name(), "desc" | "extras");
    let first = elements(n, "").filter(|c| PROP_TAGS.contains(&c.tag_name().name()) && !bulky(c));
    let mut lists = 0usize;
    for c in first.chain(elements(n, "").filter(bulky)) {
        let p = if c.tag_name().name() == "items" && lists < MAX_ITEM_LISTS {
            lists += 1;
            items_prop(c, &mut b)
        } else {
            prop(c, 1, false, &mut b)
        };
        match p {
            Some(p) => out.push(p),
            None => break,
        }
    }
    if b.cut && !warnings.iter().any(|w| w == PROPS_CUT_WARNING) {
        warnings.push(PROPS_CUT_WARNING.into());
    }
    Props { attrs, elements: out }
}

/// Rich text: XHTML `body > p > span…`. `br` starts a new paragraph; `xfa:embed` spans become
/// embedded fields; the text of an `<a href>` keeps where it links to.
fn parse_rich(body: XmlNode) -> RichText {
    #[derive(Clone, Default)]
    struct Style {
        bold: Option<bool>,
        italic: Option<bool>,
        underline: Option<bool>,
        size: Option<f64>,
        h_scale: Option<f64>,
        /// The `href` of the `<a>` the text is in.
        link: Option<std::sync::Arc<str>>,
    }
    /// `base` with the CSS declarations in `css` applied: weight, style, underline, size and
    /// `xfa-font-horizontal-scale`. Anything else is ignored.
    fn styled(base: &Style, css: Option<&str>) -> Style {
        let mut s = base.clone();
        for decl in css.unwrap_or("").split(';') {
            let Some((k, v)) = decl.split_once(':') else { continue };
            let v = v.trim();
            match k.trim() {
                "font-weight" => s.bold = Some(v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600)),
                "font-style" => s.italic = Some(v == "italic" || v == "oblique"),
                "text-decoration" => s.underline = Some(v.contains("underline")),
                "font-size" => s.size = measure(v).filter(|x| *x > 0.0),
                "xfa-font-horizontal-scale" => s.h_scale = scale(v),
                _ => {}
            }
        }
        s
    }
    // Paragraph boundaries from `p` are implicit (dropped when empty); `br` makes an explicit
    // line break that stays even when the line is blank.
    /// Add the text under `n` to `out` as runs in `style`, as the elements it meets restyle it;
    /// `p` and `br` start paragraphs (`explicit` marks the `br` ones) and `embed` spans add
    /// embedded fields. Nothing deeper than `MAX_DEPTH` is read.
    fn walk(n: XmlNode, style: &Style, out: &mut RichText, explicit: &mut Vec<bool>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for c in n.children() {
            if c.is_text() {
                if let Some(t) = c.text()
                    && !t.is_empty()
                {
                    push_run(
                        out,
                        explicit,
                        Run {
                            text: t.to_string(),
                            bold: style.bold,
                            italic: style.italic,
                            underline: style.underline,
                            size: style.size,
                            h_scale: style.h_scale,
                            embed: None,
                            link: style.link.clone(),
                        },
                    );
                }
                continue;
            }
            if !c.is_element() {
                continue;
            }
            match c.tag_name().name() {
                "p" => {
                    let align = c.attribute("style").and_then(|s| {
                        s.split(';')
                            .find_map(|d| d.split_once(':').filter(|(k, _)| k.trim() == "text-align").and_then(|(_, v)| HAlign::parse(Some(v))))
                    });
                    out.paragraphs.push(Paragraph { runs: Vec::new(), h_align: align });
                    explicit.push(false);
                    walk(c, &styled(style, c.attribute("style")), out, explicit, depth + 1);
                    // The next text starts a new paragraph.
                    out.paragraphs.push(Paragraph::default());
                    explicit.push(false);
                }
                "br" => {
                    out.paragraphs.push(Paragraph::default());
                    explicit.push(true);
                }
                "span" | "b" | "i" | "u" | "a" | "div" | "font" => {
                    let mut st = styled(style, c.attribute("style"));
                    match c.tag_name().name() {
                        "b" => st.bold = Some(true),
                        "i" => st.italic = Some(true),
                        "u" => st.underline = Some(true),
                        // A link's text keeps where it points; an `<a>` without one (a named
                        // anchor) leaves the text as it was.
                        "a" => {
                            if let Some(href) = c.attribute("href").map(str::trim).filter(|h| !h.is_empty()) {
                                st.link = Some(std::sync::Arc::from(href));
                            }
                        }
                        _ => {}
                    }
                    let embed = c.attributes().find(|a| a.name() == "embed").map(|a| a.value().trim_start_matches('#').to_string());
                    if let Some(e) = embed {
                        push_run(
                            out,
                            explicit,
                            Run {
                                text: String::new(),
                                bold: st.bold,
                                italic: st.italic,
                                underline: st.underline,
                                size: st.size,
                                h_scale: st.h_scale,
                                embed: Some(e),
                                link: st.link.clone(),
                            },
                        );
                    }
                    walk(c, &st, out, explicit, depth + 1);
                }
                _ => walk(c, style, out, explicit, depth + 1),
            }
        }
    }
    fn push_run(out: &mut RichText, explicit: &mut Vec<bool>, run: Run) {
        if out.paragraphs.is_empty() {
            out.paragraphs.push(Paragraph::default());
            explicit.push(false);
        }
        if let Some(p) = out.paragraphs.last_mut() {
            p.runs.push(run);
        }
    }
    let mut out = RichText::default();
    let mut explicit = Vec::new();
    walk(body, &Style::default(), &mut out, &mut explicit, 0);
    // Drop the empty paragraphs `p` boundaries leave behind; blank lines from `br` stay.
    let mut cleaned: Vec<Paragraph> = Vec::new();
    for (i, p) in out.paragraphs.into_iter().enumerate() {
        let blank = p.runs.iter().all(|r| r.text.trim().is_empty() && r.embed.is_none());
        if blank && !explicit.get(i).copied().unwrap_or(false) {
            continue;
        }
        cleaned.push(p);
    }
    while cleaned.last().is_some_and(|p| p.runs.iter().all(|r| r.text.trim().is_empty() && r.embed.is_none())) {
        cleaned.pop();
    }
    RichText { paragraphs: cleaned }
}

/// `<value>`: text, rich text, an image, a line or a rectangle.
fn parse_value(n: Option<XmlNode>, warnings: &mut Vec<String>) -> Value {
    let Some(n) = n else { return Value::Empty };
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "text" => return Value::Text(text_of(c)),
            "exData" => {
                let html = c.attribute("contentType").is_some_and(|t| t.contains("html")) || child(c, "body").is_some();
                if html {
                    let body = child(c, "body").unwrap_or(c);
                    return Value::Rich(parse_rich(body));
                }
                return Value::Text(text_of(c));
            }
            "image" => {
                let Some(raw) = image_text_of(c) else {
                    warnings.push("an image is larger than 32 MB; it is left out".into());
                    return Value::Empty;
                };
                let data = if c.attribute("transferEncoding").is_none_or(|e| e == "base64") {
                    match base64::engine::general_purpose::STANDARD.decode(&raw) {
                        Ok(d) => d,
                        Err(_) => {
                            warnings.push("an image's base64 data could not be decoded; it is left out".into());
                            return Value::Empty;
                        }
                    }
                } else {
                    raw
                };
                if data.len() > MAX_IMAGE {
                    warnings.push("an image is larger than 32 MB; it is left out".into());
                    return Value::Empty;
                }
                return Value::Image { content_type: c.attribute("contentType").unwrap_or("").to_string(), data };
            }
            "line" => {
                return Value::Line { slope_up: c.attribute("slope") == Some("/"), edge: child(c, "edge").map(parse_edge).unwrap_or_default() };
            }
            "rectangle" => {
                let mut b = parse_border(Some(c)).unwrap_or_default();
                b.fill = parse_fill(child(c, "fill"));
                return Value::Rectangle(Box::new(b));
            }
            "date" | "time" | "dateTime" | "integer" | "decimal" | "float" | "boolean" => return Value::Text(text_of(c)),
            _ => {}
        }
    }
    Value::Empty
}

fn parse_caption(n: Option<XmlNode>, warnings: &mut Vec<String>) -> Option<Caption> {
    let n = n?;
    Some(Caption {
        placement: match n.attribute("placement") {
            Some("right") => Placement::Right,
            Some("top") => Placement::Top,
            Some("bottom") => Placement::Bottom,
            Some("inline") => Placement::Inline,
            _ => Placement::Left,
        },
        // A reserve of zero or less sizes the caption to its content, as none does (XFA 3.3,
        // the caption's reserve).
        reserve: attr_measure(n, "reserve").filter(|r| *r > 0.0),
        presence: Presence::parse(n.attribute("presence")),
        value: parse_value(child(n, "value"), warnings),
        font: parse_font(child(n, "font")),
        para: parse_para(child(n, "para")),
    })
}

/// The field's item lists: what is shown (the first `<items>` that is not the saved values,
/// also a check button's on and off values) and, for choice lists, the values saved to the
/// data (`<items save="1">`, usually hidden), empty when the shown text is the value.
fn parse_items(n: XmlNode, warnings: &mut Vec<String>) -> (Vec<String>, Vec<String>) {
    let lists: Vec<XmlNode> = children(n, "items").collect();
    let is_saved = |i: &XmlNode| i.attribute("save") == Some("1") || i.attribute("presence") == Some("hidden");
    let mut over = false;
    let mut texts = |i: &XmlNode| -> Vec<String> {
        let all: Vec<String> = i.children().filter(|c| c.is_element()).map(text_of).take(MAX_ITEMS + 1).collect();
        if all.len() > MAX_ITEMS {
            over = true;
        }
        all.into_iter().take(MAX_ITEMS).collect()
    };
    let shown = lists.iter().find(|i| !is_saved(i)).or(lists.first());
    let saved = lists.iter().find(|i| is_saved(i) && shown.is_none_or(|s| s != *i));
    let shown: Vec<String> = shown.map(&mut texts).unwrap_or_default();
    let mut saved: Vec<String> = saved.map(&mut texts).unwrap_or_default();
    if over && warnings.len() < 100 {
        warnings.push(format!(
            "{} has more than {MAX_ITEMS} items; the rest are left out",
            n.attribute("name").map_or_else(|| "a choice list".to_string(), |name| format!("the choice list {name}"))
        ));
    }
    // A saved list shorter than the shown one is padded with the shown text.
    if !saved.is_empty() && saved.len() < shown.len() {
        saved.extend(shown.iter().skip(saved.len()).cloned());
    }
    (shown, saved)
}

/// A `<field>` element as the model's [`Field`]: its widget kind, caption, value, items,
/// pictures (edit and data), scripts and access.
fn parse_field(n: XmlNode, warnings: &mut Vec<String>) -> Field {
    let ui = child(n, "ui");
    let ui_el = ui.and_then(|u| u.children().find(|c| c.is_element() && c.tag_name().name() != "extras" && c.tag_name().name() != "picture"));
    let kind = match ui_el.map(|e| e.tag_name().name()) {
        None | Some("textEdit") => Ui::TextEdit,
        Some("numericEdit") => Ui::NumericEdit,
        Some("dateTimeEdit") => Ui::DateTimeEdit,
        Some("checkButton") => Ui::CheckButton,
        Some("choiceList") => Ui::ChoiceList,
        Some("button") => Ui::Button,
        Some("signature") => Ui::Signature,
        Some("imageEdit") => Ui::ImageEdit,
        Some("barcode") => Ui::Barcode,
        Some("passwordEdit") => Ui::PasswordEdit,
        Some(_) => Ui::Unknown,
    };
    let value_el = child(n, "value");
    let max_chars = value_el
        .and_then(|v| child(v, "text"))
        .and_then(|t| t.attribute("maxChars"))
        .and_then(|m| m.trim().parse::<usize>().ok())
        // XFA 3.3, exData/maxLength: a character limit excluding markup; -1 is unlimited.
        .or_else(|| value_el.and_then(|v| child(v, "exData")).and_then(|e| e.attribute("maxLength")).and_then(|m| m.trim().parse::<usize>().ok()))
        // Preserve the existing zero/unbounded convention of generated form fields.
        .filter(|m| *m > 0);
    let scripts = parse_events(n);
    let numeric = matches!(kind, Ui::NumericEdit)
        || value_el.is_some_and(|v| v.children().any(|c| c.is_element() && matches!(c.tag_name().name(), "decimal" | "integer" | "float")));
    let (items, item_values) = parse_items(n, warnings);
    let choice = match (kind == Ui::ChoiceList, ui_el) {
        (true, Some(e)) => {
            let open = e.attribute("open").unwrap_or("onEntry");
            ChoiceList {
                list_box: matches!(open, "always" | "multiSelect"),
                multi: open == "multiSelect",
                editable: e.attribute("textEntry").is_some_and(|t| t == "1" || t == "true"),
            }
        }
        _ => ChoiceList::default(),
    };
    let multiline = ui_el.is_some_and(|e| e.attribute("multiLine").is_some_and(|m| m == "1" || m == "true"));
    let validate_el = child(n, "validate");
    let validate_message = validate_el
        .and_then(|v| child(v, "message"))
        .and_then(|m| children(m, "text").find(|t| t.attribute("name") == Some("scriptTest")).or_else(|| child(m, "text")))
        .map(text_of)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    Field {
        common: parse_common(n, warnings),
        ui: kind,
        ui_border: ui_el.and_then(|e| parse_border(child(e, "border"))),
        ui_margin: parse_margin(ui_el.and_then(|e| child(e, "margin"))),
        check_size: ui_el.and_then(|e| attr_measure(e, "size")).filter(|s| *s > 0.0),
        check_round: ui_el.is_some_and(|e| e.attribute("shape") == Some("round")),
        multiline,
        no_scroll: ui_el.is_some_and(|e| e.attribute(if multiline { "vScrollPolicy" } else { "hScrollPolicy" }) == Some("off")),
        caption: parse_caption(child(n, "caption"), warnings),
        value: parse_value(value_el, warnings),
        items,
        item_values,
        choice,
        max_chars,
        tooltip: child(n, "assist").and_then(|a| child(a, "toolTip")).map(text_of).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        access: match n.attribute("access") {
            Some("readOnly") => Access::ReadOnly,
            Some("protected") => Access::Protected,
            Some("nonInteractive") => Access::NonInteractive,
            _ => Access::Open,
        },
        picture: ui.and_then(|u| child(u, "picture")).map(text_of).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        data_picture: child(n, "bind").and_then(|b| child(b, "picture")).map(text_of).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        scripts,
        calculate: child(n, "calculate").and_then(|c| child(c, "script")).map(|s| parse_script(s, "calculate")),
        validate: validate_el.and_then(|v| child(v, "script")).map(|s| parse_script(s, "validate")),
        validate_message,
        numeric,
        display_picture: child(n, "format").and_then(|f| child(f, "picture")).map(text_of).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
    }
}

fn parse_script(s: XmlNode, activity: &str) -> Script {
    let ct = s.attribute("contentType").unwrap_or("");
    Script { activity: activity.to_string(), text: text_of(s).trim().to_string(), formcalc: !ct.to_ascii_lowercase().contains("javascript") }
}

/// `<event activity="…"><script>…</script></event>` children.
fn parse_events(n: XmlNode) -> Vec<Script> {
    let mut scripts = Vec::new();
    for ev in children(n, "event").take(64) {
        if let Some(s) = child(ev, "script") {
            let activity = ev.attribute("activity").unwrap_or("click");
            scripts.push(parse_script(s, activity));
        }
    }
    scripts
}

fn parse_occur(n: Option<XmlNode>) -> Occur {
    let Some(n) = n else { return Occur::default() };
    let int = |a: &str| n.attribute(a).and_then(|v| v.trim().parse::<i64>().ok());
    let min = int("min").unwrap_or(1).clamp(0, 1000) as usize;
    let max = match int("max") {
        Some(m) if m < 0 => None,
        Some(m) => Some((m.clamp(0, 1000) as usize).max(min)),
        None => Some(min.max(1)),
    };
    let initial = int("initial").map_or(min, |i| i.clamp(0, 1000) as usize).max(min);
    Occur { min, max, initial: max.map_or(initial, |m| initial.min(m)) }
}

fn parse_page_set(n: XmlNode, warnings: &mut Vec<String>, depth: usize) -> PageSet {
    let mut areas = Vec::new();
    for pa in children(n, "pageArea") {
        let medium = child(pa, "medium");
        let short = medium.and_then(|m| attr_measure(m, "short")).unwrap_or(612.0).clamp(1.0, 14_400.0);
        let long = medium.and_then(|m| attr_measure(m, "long")).unwrap_or(792.0).clamp(1.0, 14_400.0);
        let (width, height) = if medium.and_then(|m| m.attribute("orientation")) == Some("landscape") { (long, short) } else { (short, long) };
        let mut content: Vec<Rect> = children(pa, "contentArea")
            .map(|c| {
                Rect::new(
                    attr_measure(c, "x").unwrap_or(0.0),
                    attr_measure(c, "y").unwrap_or(0.0),
                    attr_measure(c, "w").unwrap_or(width).clamp(1.0, width),
                    attr_measure(c, "h").unwrap_or(height).clamp(1.0, height),
                )
            })
            .collect();
        if content.is_empty() {
            content.push(Rect::new(0.0, 0.0, width, height));
        }
        let items = pa.children().filter(|c| c.is_element()).filter_map(|c| parse_node(c, warnings, depth + 1)).collect();
        // A page area with no `<occur>` has no limit and no minimum (a subform's default of one
        // would exhaust it, and demand a page for every master).
        let occur = child(pa, "occur").map(|o| parse_occur(Some(o)));
        let (occur_min, occur_max) = occur.map_or((0, None), |o| (o.min, o.max));
        areas.push(PageArea { name: pa.attribute("name").map(str::to_string), width, height, content, items, occur_max, occur_min });
    }
    // Nested page sets: take their areas too.
    for ps in children(n, "pageSet") {
        if depth < MAX_DEPTH {
            areas.extend(parse_page_set(ps, warnings, depth + 1).areas);
        }
    }
    PageSet { areas }
}

fn parse_subform(n: XmlNode, warnings: &mut Vec<String>, depth: usize) -> Subform {
    let common = parse_common(n, warnings);
    let mut children_out = Vec::new();
    let mut page_set = None;
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "pageSet" => page_set = Some(parse_page_set(c, warnings, depth + 1)),
            _ => {
                if let Some(node) = parse_node(c, warnings, depth + 1) {
                    children_out.push(node);
                }
            }
        }
    }
    let column_widths: Vec<f64> =
        n.attribute("columnWidths").map(|s| s.split_whitespace().filter_map(measure).map(|w| w.max(0.0)).take(256).collect()).unwrap_or_default();
    let break_to_page = |name: &str| children(n, name).any(|b| b.attribute("targetType").is_none_or(|t| t == "pageArea" || t == "contentArea"));
    Subform {
        common,
        layout: Layout::parse(n.attribute("layout")),
        children: children_out,
        scripts: parse_events(n),
        page_set,
        occur: parse_occur(child(n, "occur")),
        break_before_page: break_to_page("breakBefore")
            || child(n, "break").is_some_and(|b| b.attribute("before").is_some_and(|v| v == "pageArea" || v == "contentArea")),
        break_after_page: break_to_page("breakAfter"),
        column_widths,
        overflow_leader: child(n, "overflow").and_then(|o| o.attribute("leader")).map(str::to_string).filter(|s| !s.is_empty()),
    }
}

fn parse_node(c: XmlNode, warnings: &mut Vec<String>, depth: usize) -> Option<Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    Some(match c.tag_name().name() {
        "subform" => Node::Subform(Box::new(parse_subform(c, warnings, depth))),
        "subformSet" => {
            // A set of alternative subforms: lay out its members in order.
            let mut sf = parse_subform(c, warnings, depth);
            sf.layout = Layout::Tb;
            Node::Subform(Box::new(sf))
        }
        "area" => Node::Area(Box::new(Area {
            common: parse_common(c, warnings),
            children: c.children().filter(|k| k.is_element()).filter_map(|k| parse_node(k, warnings, depth + 1)).collect(),
        })),
        "field" => Node::Field(Box::new(parse_field(c, warnings))),
        "draw" => Node::Draw(Box::new(Draw { common: parse_common(c, warnings), value: parse_value(child(c, "value"), warnings) })),
        "exclGroup" => Node::ExclGroup(Box::new(ExclGroup {
            common: parse_common(c, warnings),
            layout: Layout::parse(c.attribute("layout")),
            fields: children(c, "field").map(|f| parse_field(f, warnings)).collect(),
            tooltip: child(c, "assist").and_then(|a| child(a, "toolTip")).map(text_of).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
            scripts: parse_events(c),
        })),
        _ => return None,
    })
}

/// Which page-number role a field with a layout:ready script plays, if any.
fn page_role(f: &Field) -> Option<PageRole> {
    let script = f.scripts.iter().find(|s| s.activity == "ready" || s.activity == "layoutReady")?;
    if script.text.contains("pageCount") {
        Some(PageRole::Count)
    } else if script.text.contains("layout.page(") || script.text.contains("layout.absPage(") {
        Some(PageRole::Number)
    } else {
        None
    }
}

fn collect_page_roles(nodes: &[Node], out: &mut Vec<(String, PageRole)>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        match n {
            Node::Field(f) => {
                if let (Some(id), Some(role)) = (&f.common.id, page_role(f)) {
                    out.push((id.clone(), role));
                }
            }
            Node::Subform(s) => {
                collect_page_roles(&s.children, out, depth + 1);
                if let Some(ps) = &s.page_set {
                    for a in &ps.areas {
                        collect_page_roles(&a.items, out, depth + 1);
                    }
                }
            }
            Node::Area(a) => collect_page_roles(&a.children, out, depth + 1),
            Node::ExclGroup(_) | Node::Draw(_) => {}
        }
    }
}

/// Parse a template packet. Returns the template and the warnings met on the way.
pub fn parse(xml: &str) -> Result<(Template, Vec<String>), XfaError> {
    // No bigger than a packet the reader accepts can come to as text ([`crate::read_packets`]), so
    // what parsing keeps of it (item lists are held by the field and its properties) stays within
    // a few times that, whoever calls it.
    let limit = crate::packets::MAX_TEMPLATE_TEXT;
    if xml.len() > limit {
        return Err(XfaError::TooLarge(format!("the template is {} bytes, past the {limit} bytes the reader accepts", xml.len())));
    }
    let depth = crate::xml_nesting_depth(xml);
    if depth > crate::MAX_XML_DEPTH {
        return Err(XfaError::Malformed(format!(
            "the template nests XML elements {depth} deep, past the {} the reader accepts",
            crate::MAX_XML_DEPTH
        )));
    }
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| XfaError::Malformed(format!("template XML: {e}")))?;
    let root = doc.root_element();
    // The XDP's config packet has a <template> element too: the one we want is in the
    // xfa-template namespace, or at least has a subform in it.
    let is_template = |n: &XmlNode| {
        n.is_element()
            && n.tag_name().name() == "template"
            && (n.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-template")) || children(*n, "subform").next().is_some())
    };
    let template = if is_template(&root) {
        root
    } else {
        root.descendants().find(is_template).ok_or_else(|| XfaError::Malformed("no <template> element".into()))?
    };
    let mut warnings = Vec::new();
    let sf = children(template, "subform").next().ok_or_else(|| XfaError::Malformed("the template has no root subform".into()))?;
    let mut root_sf = parse_subform(sf, &mut warnings, 0);
    if root_sf.page_set.is_none() {
        // A page set may sit beside the root subform.
        root_sf.page_set = child(template, "pageSet").map(|ps| parse_page_set(ps, &mut warnings, 0));
    }
    // The form packet beside the template holds the state the form was saved in.
    let saved = root
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "form" && n.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-form")));
    if let Some(saved_root) = saved.and_then(|f| children(f, "subform").next()) {
        if let Some(p) = saved_root.attribute("presence") {
            root_sf.common.presence = Presence::parse(Some(p));
        }
        apply_saved_state(&mut root_sf.children, saved_root, 0);
    }
    let mut page_roles = Vec::new();
    collect_page_roles(&[Node::Subform(Box::new(root_sf.clone()))], &mut page_roles, 0);
    Ok((Template { root: root_sf, page_roles, cache: Default::default() }, warnings))
}

fn parse_access(a: Option<&str>) -> Option<Access> {
    Some(match a? {
        "readOnly" => Access::ReadOnly,
        "protected" => Access::Protected,
        "nonInteractive" => Access::NonInteractive,
        "open" => Access::Open,
        _ => return None,
    })
}

/// Apply the saved state of a form packet (`saved`: the element matching the container whose
/// children `nodes` are) to the template: the `presence` and `access` a script gave an object
/// before the form was saved, which is restored on opening. Objects are matched by
/// name and position among the same-named; of the instances of a repeating subform only the
/// first is read, as the template has it once.
fn apply_saved_state(nodes: &mut [Node], saved: XmlNode, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in saved.children().filter(|c| c.is_element()) {
        if !matches!(c.tag_name().name(), "subform" | "subformSet" | "area" | "field" | "draw" | "exclGroup") {
            continue;
        }
        let Some(name) = c.attribute("name").filter(|n| !n.is_empty()) else { continue };
        let nth = {
            let k = seen.entry(name).or_insert(0);
            *k += 1;
            *k - 1
        };
        let same: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.common().name.as_deref() == Some(name)).map(|(i, _)| i).collect();
        // More saved objects of the name than the template has: further instances.
        let Some(target) = same.get(nth).copied().and_then(|i| nodes.get_mut(i)) else { continue };
        let presence = c.attribute("presence").map(|p| Presence::parse(Some(p)));
        match target {
            Node::Subform(s) => {
                if let Some(p) = presence {
                    s.common.presence = p;
                }
                apply_saved_state(&mut s.children, c, depth + 1);
            }
            Node::Area(a) => {
                if let Some(p) = presence {
                    a.common.presence = p;
                }
                apply_saved_state(&mut a.children, c, depth + 1);
            }
            Node::Field(f) => {
                if let Some(p) = presence {
                    f.common.presence = p;
                }
                if let Some(a) = parse_access(c.attribute("access")) {
                    f.access = a;
                }
                // A caption a script changed (a button's label, say) is saved as plain text.
                let text = child(c, "caption").and_then(|k| child(k, "value")).and_then(|v| child(v, "text")).and_then(|t| t.text());
                if let (Some(caption), Some(text)) = (f.caption.as_mut(), text) {
                    caption.value = Value::Text(text.to_string());
                }
            }
            Node::Draw(d) => {
                if let Some(p) = presence {
                    d.common.presence = p;
                }
            }
            Node::ExclGroup(g) => {
                if let Some(p) = presence {
                    g.common.presence = p;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "props_tests.rs"]
mod props_tests;
