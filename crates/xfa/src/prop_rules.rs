//! The rules a script's property change must meet to be kept (XFA 3.3 ch. 3, "Properties vs.
//! Children", p. 78, and the elements of ch. 17), shared word for word by the form
//! (`pdfcraft_xfa::restyle`) and the scripting host (`pdfcraft_js`, which includes this file),
//! so a script never reads back a change the form drops. Standard library only.

/// Most steps a property path takes from its object: the template reader keeps 8 levels of
/// property elements (`pdfcraft_xfa::parse::MAX_PROP_DEPTH`).
pub const MAX_STEPS: usize = 8;
/// Highest occurrence a step may name. On the 163 dynamic forms of 367 the highest a change names
/// is 2; a border's edges and corners go to 3 and a validation message holds one text per test
/// and an unnamed one (4), so 7 leaves room; a higher one only multiplies what a path can make.
pub const MAX_INDEX: usize = 7;
/// Longest value one change may set, in bytes: the property budget of one whole object as the
/// template is read (`pdfcraft_xfa::parse::MAX_OBJECT_PROP_BYTES`).
pub const MAX_VALUE: usize = 16 << 10;

/// Highest entry of a field's item list a step may name: the template reader keeps 1,000
/// entries a list (`pdfcraft_xfa::parse::MAX_ITEMS`).
pub const MAX_ITEM_INDEX: usize = 999;

/// The property elements of an object a change may start at: those the template reader keeps
/// for scripts (XFA 3.3 p. 78; caption p. 643, items p. 758, calculate p. 641).
pub const ROOTS: &[&str] = &[
    "border",
    "ui",
    "font",
    "margin",
    "para",
    "assist",
    "validate",
    "format",
    "occur",
    "break",
    "breakBefore",
    "breakAfter",
    "bind",
    "desc",
    "extras",
    "keep",
    "value",
    "caption",
    "items",
    "calculate",
];

/// Elements whose text a change may set: what the objects' messages, tooltips, spoken texts and
/// pictures hold, and a caption's label (its `text`, or the words of its rich text `exData`).
/// A `script`'s text is not among them (a script is not changed here), nor anything under the
/// object's own `value` (the object's value is changed through its data). An entry of an item
/// list holds text whatever its tag (`text`, `integer`, …, XFA 3.3 p. 758).
pub const TEXTS: &[&str] = &["text", "toolTip", "speak", "picture"];

/// Attributes that hold a measurement (`0.5pt`, `2mm`, `12`): XFA 3.3 edge p. 687, corner
/// p. 663, font p. 741, check button p. 650, margin p. 776, para p. 800, caption p. 643.
pub const MEASURES: &[&str] = &[
    "thickness",
    "reserve",
    "radius",
    "size",
    "topInset",
    "bottomInset",
    "leftInset",
    "rightInset",
    "marginLeft",
    "marginRight",
    "spaceAbove",
    "spaceBelow",
    "textIndent",
    "lineHeight",
    "tabDefault",
];

/// Attributes that hold a whole number: occur p. 786 (`max` may be -1, unlimited), text
/// `maxChars` p. 863, comb `numberOfCells` p. 657, barcode `dataLength`, exData `maxLength`.
pub const INTEGERS: &[&str] = &["min", "max", "initial", "maxChars", "numberOfCells", "dataLength", "maxLength"];

/// An XML name as the template's tags and attributes are: a letter or `_`, then letters,
/// digits, `_`, `-` or `.`, at most 64 bytes.
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The steps of a property path (`border#0/edge#1`) as (tag, occurrence), when it is one a
/// change may name: 1 to [`MAX_STEPS`] steps, each a name and an occurrence up to
/// [`MAX_INDEX`], starting at one of the [`ROOTS`]; a border's or rectangle's edges and
/// corners occur at most four times (XFA 3.3 p. 629), so none past the fourth.
pub fn steps(path: &str) -> Option<Vec<(&str, usize)>> {
    let mut out: Vec<(&str, usize)> = Vec::new();
    for step in path.split('/') {
        let (tag, index) = step.rsplit_once('#')?;
        let index: usize = index.parse().ok()?;
        // An item list's entries go as far as the list does; every other step to MAX_INDEX.
        let in_items = matches!(out.as_slice(), [("items", _)]);
        let most = if in_items { MAX_ITEM_INDEX } else { MAX_INDEX };
        if !is_name(tag) || index > most || out.len() >= MAX_STEPS {
            return None;
        }
        let in_box = out.last().is_some_and(|(t, _)| matches!(*t, "border" | "rectangle"));
        if in_box && matches!(tag, "edge" | "corner") && index > 3 {
            return None;
        }
        out.push((tag, index));
    }
    out.first().filter(|(tag, _)| ROOTS.contains(tag))?;
    Some(out)
}

/// A measurement such as `12.7mm`, `72pt`, `1in`, `2.54cm` in points; bare numbers are
/// points. `None` for anything else (including `em`). The template reader reads measurements
/// with it (`pdfcraft_xfa::parse::measure`).
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

/// The three components of a colour `r,g,b` (0-255, read as numbers; more than three are
/// ignored). The template reader reads colours with it (`pdfcraft_xfa::model::Color::parse`).
pub fn color_components(s: &str) -> Option<[f64; 3]> {
    let mut parts = s.split(',').map(|p| p.trim().parse::<f64>().ok());
    Some([parts.next()??, parts.next()??, parts.next()??])
}

/// Whether a change may set `value` as attribute `attr` (`None`: the text) of the property at
/// `path`; `Err` says why not. A path that is no property ([`steps`]), text where a property
/// holds none, a colour that is not `r,g,b`, a measurement or a whole number that doesn't read,
/// and the attributes that say how a script runs or how the data binds (a validation script's,
/// `bind`'s `match` and `ref`, which the form doesn't read again once open) are refused; the
/// property keeps what it had. Other attributes take any value, as the template's do: one a
/// reader doesn't know reads as the attribute's default (XFA 3.3 p. 598).
pub fn check(path: &str, attr: Option<&str>, value: &str) -> Result<(), String> {
    let Some(st) = steps(path) else { return Err(format!("{path} is not a property scripts may change")) };
    if value.len() > MAX_VALUE {
        return Err(format!("the value is longer than {MAX_VALUE} bytes"));
    }
    let tag = st.last().map_or("", |(t, _)| *t);
    let Some(attr) = attr else {
        let under_value = st.first().is_some_and(|(t, _)| *t == "value");
        let label = matches!(st.as_slice(), [("caption", _), ("value", _), ("text" | "exData", _)]);
        let entry = matches!(st.as_slice(), [("items", _), (_, _)]);
        return if (TEXTS.contains(&tag) && !under_value) || label || entry {
            Ok(())
        } else {
            Err(format!("{tag} holds no text scripts may set here"))
        };
    };
    if !is_name(attr) {
        return Err(format!("{attr:?} is not an attribute name"));
    }
    if tag == "script" || (tag == "bind" && matches!(attr, "match" | "ref")) {
        return Err(format!("{tag} {attr} is not changed once the form is open"));
    }
    if tag == "color" && attr == "value" && color_components(value).is_none() {
        return Err(format!("{value:?} is not a colour (r,g,b)"));
    }
    if MEASURES.contains(&attr) && measure(value).is_none() {
        return Err(format!("{value:?} is not a measurement"));
    }
    if INTEGERS.contains(&attr) && value.trim().parse::<i64>().is_err() {
        return Err(format!("{value:?} is not a whole number"));
    }
    Ok(())
}
