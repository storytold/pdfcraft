//! The rules a script's property change must meet to be kept (XFA 3.3 ch. 3, "Properties vs.
//! Children", p. 78, and the elements of ch. 17), shared word for word by the form
//! (`pdfcraft_xfa::restyle`, which re-exports this crate as `pdfcraft_xfa::prop_rules`) and the
//! scripting host (`pdfcraft_js`), both through their manifests, so a script never reads back a
//! change the form drops. Standard library only.

/// Most steps a property path takes from its object: the template reader keeps 8 levels of
/// property elements (`pdfcraft_xfa::parse::MAX_PROP_DEPTH`).
pub const MAX_STEPS: usize = 8;
/// Highest occurrence a step may name. On the 245 dynamic forms of 500 the highest a change names
/// is 3; a border's edges and corners go to 3 and a validation message holds one text per test
/// and an unnamed one (4), so 7 leaves room; a higher one only multiplies what a path can make.
pub const MAX_INDEX: usize = 7;
/// Longest value one change may set, in bytes: the property budget of one whole object as the
/// template is read (`pdfcraft_xfa::parse::MAX_OBJECT_PROP_BYTES`).
pub const MAX_VALUE: usize = 16 << 10;

/// Highest entry of a field's item list a step may name: the template reader keeps 1,000
/// entries a list (`pdfcraft_xfa::parse::MAX_ITEMS`). On the 500 forms the longest list holds
/// 615 entries (342 on the 245 dynamic ones). A change names an entry the list holds; it never
/// adds one (`pdfcraft_xfa::restyle::apply_props`), as a script reaches only the entries there are.
pub const MAX_ITEM_INDEX: usize = 999;
/// How many of an object's `<items>` are lists, whose entries go to [`MAX_ITEM_INDEX`]: the
/// shown list and the saved values (`pdfcraft_xfa::parse::MAX_ITEM_LISTS`). A further one is an
/// ordinary property, its entries named up to [`MAX_INDEX`].
pub const ITEM_LISTS: usize = 2;

/// The property elements of an object a change may start at: those the template reader keeps
/// for scripts (XFA 3.3 p. 78; caption p. 643, items p. 758, calculate p. 641). How often a
/// subform occurs (`occur`, p. 786) is not among them: it is the subform's, not one object's,
/// and no change to it is kept.
pub const ROOTS: &[&str] = &[
    "border",
    "ui",
    "font",
    "margin",
    "para",
    "assist",
    "validate",
    "format",
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

/// Attributes that hold a whole number: text `maxChars` p. 863, comb `numberOfCells` p. 657,
/// barcode `dataLength`, exData `maxLength`, and `min`, `max` and `initial` wherever a
/// property has them.
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
/// change may name: 1 to [`MAX_STEPS`] steps, each a name and an occurrence in plain decimal
/// digits (no sign, no leading zero, so each property has one path) up to [`MAX_INDEX`] (an
/// entry of one of the first [`ITEM_LISTS`] item lists up to
/// [`MAX_ITEM_INDEX`], and nothing under it), starting at one of the [`ROOTS`]; a border's or
/// rectangle's edges and corners occur at most four times (XFA 3.3 p. 629), so none past the
/// fourth.
pub fn steps(path: &str) -> Option<Vec<(&str, usize)>> {
    let mut out: Vec<(&str, usize)> = Vec::new();
    for step in path.split('/') {
        let (tag, index) = step.rsplit_once('#')?;
        // Written one way only (`0`, `12`; not `+0`, `012`), so no two paths name one property.
        let plain = index == "0" || (!index.starts_with('0') && index.bytes().all(|b| b.is_ascii_digit()));
        let index: usize = index.parse().ok().filter(|_| plain)?;
        // An item list's entries go as far as the list does; every other step to MAX_INDEX.
        let in_list = matches!(out.as_slice(), [("items", k)] if *k < ITEM_LISTS);
        let most = if in_list { MAX_ITEM_INDEX } else { MAX_INDEX };
        // An entry holds its value and nothing else (XFA 3.3 p. 758; the reader keeps no more).
        let past_entry = matches!(out.as_slice(), [("items", k), _, ..] if *k < ITEM_LISTS);
        if !is_name(tag) || index > most || out.len() >= MAX_STEPS || past_entry {
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
/// holds none, a colour that is not `r,g,b` of finite numbers (the form draws `NaN` as 0, so a
/// script would read back a colour other than the one drawn), a measurement or a whole number
/// that doesn't read,
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
    if tag == "color" && attr == "value" && color_components(value).is_none_or(|c| c.iter().any(|v| !v.is_finite())) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const COLOR: &str = "border#0/fill#0/color#0";

    /// A colour is three finite numbers: `NaN` and infinities (which `f64` parses) are refused,
    /// as the form would draw them as 0, and what a script read back would not be drawn.
    #[test]
    fn colours_are_three_finite_numbers() {
        for ok in ["51,102,153", " 1 , 2 , 3 ", "0,0,0,9", "255.5,0,0"] {
            assert_eq!(check(COLOR, Some("value"), ok), Ok(()), "{ok}");
        }
        for bad in ["NaN,0,0", "0,inf,0", "0,0,-infinity", "red", "1,2", "", "1,,2"] {
            assert!(check(COLOR, Some("value"), bad).is_err(), "{bad}");
        }
        // The template reader still reads them (and zeroes them when drawing).
        assert!(color_components("NaN,0,0").is_some_and(|c| c[0].is_nan()));
    }

    /// Measurements read in points, in every unit XFA 3.3 names; non-finite numbers, `em`,
    /// exponents and unknown units are refused, and huge values are clamped.
    #[test]
    fn measurements_read_in_points_and_refuse_what_is_not_one() {
        assert_eq!(measure("72pt"), Some(72.0));
        assert_eq!(measure("1in"), Some(72.0));
        assert_eq!(measure(" 12 "), Some(12.0));
        assert_eq!(measure("2pc"), Some(24.0));
        assert!(measure("25.4mm").is_some_and(|v| (v - 72.0).abs() < 1e-9));
        assert!(measure("2.54cm").is_some_and(|v| (v - 72.0).abs() < 1e-9));
        assert_eq!(measure("1e309in"), None);
        assert_eq!(measure("NaNpt"), None);
        assert_eq!(measure("infpt"), None);
        assert_eq!(measure("2em"), None);
        assert_eq!(measure("thick"), None);
        assert_eq!(measure("1e3pt"), None, "no exponents: the unit starts at the first letter");
        assert_eq!(measure("100000000in"), Some(100_000.0));
        assert_eq!(measure("-100000000in"), Some(-100_000.0));
        assert!(check("font#0", Some("size"), "1e309in").is_err());
        assert_eq!(check("caption#0", Some("reserve"), "-5in"), Ok(()));
    }

    /// Whole numbers where an attribute holds one.
    #[test]
    fn whole_numbers_are_whole() {
        let comb = "ui#0/textEdit#0/comb#0";
        assert_eq!(check(comb, Some("numberOfCells"), "-1"), Ok(()));
        assert_eq!(check(comb, Some("numberOfCells"), " 3 "), Ok(()));
        for bad in ["three", "1.5", "", "99999999999999999999", "x"] {
            assert!(check(comb, Some("numberOfCells"), bad).is_err(), "{bad}");
        }
    }

    /// How often a subform occurs is no property a change may name.
    #[test]
    fn occur_is_not_changed() {
        for attr in ["min", "max", "initial"] {
            assert!(check("occur#0", Some(attr), "2").is_err(), "{attr}");
        }
        assert!(steps("occur#0").is_none());
    }

    /// Paths are bounded: up to [`MAX_STEPS`] steps from one of the [`ROOTS`], each up to
    /// [`MAX_INDEX`]; an entry of the first two item lists up to [`MAX_ITEM_INDEX`] with nothing
    /// under it, a third list's up to [`MAX_INDEX`]; a border's edges and corners up to 3.
    #[test]
    fn paths_are_bounded() {
        assert_eq!(steps("border#0/edge#1"), Some(vec![("border", 0), ("edge", 1)]));
        assert!(steps("a#0/b#0/c#0/d#0/e#0/f#0/g#0/h#0").is_none(), "not a root");
        assert!(steps("border#0/a#0/b#0/c#0/d#0/e#0/f#0/g#0").is_some(), "{MAX_STEPS} steps");
        assert!(steps("border#0/a#0/b#0/c#0/d#0/e#0/f#0/g#0/h#0").is_none(), "one more");
        assert!(steps("font#7").is_some());
        assert!(steps("font#8").is_none());
        assert!(steps("font").is_none());
        assert!(steps("font#x").is_none());
        assert!(steps("font#-1").is_none());
        // One spelling per occurrence: no second path to the property `font#5` names.
        assert_eq!(steps("font#5"), Some(vec![("font", 5)]));
        for alias in ["font#+5", "font#05", "font#00", "font#", "font#5 ", "font#\u{0665}"] {
            assert!(steps(alias).is_none(), "{alias}");
        }
        assert_eq!(check("font#0", Some("size"), "10pt"), Ok(()));
        assert!(check("font#+0", Some("size"), "10pt").is_err());
        assert!(steps("").is_none());
        assert!(steps("nowhere#0").is_none());
        assert!(steps("border#0/edge#3").is_some());
        assert!(steps("border#0/edge#4").is_none());
        assert!(steps("border#0/corner#4").is_none());
        assert!(steps("ui#0/edge#4").is_some(), "only a border's or rectangle's edges stop at 3");
        assert!(steps("items#0/integer#999").is_some());
        assert!(steps("items#1/text#999").is_some());
        assert!(steps("items#0/integer#1000").is_none());
        assert!(steps("items#0/text#0/x#0").is_none(), "nothing under an entry");
        assert!(steps("items#2/text#7").is_some());
        assert!(steps("items#2/text#8").is_none(), "a third list is an ordinary property");
        assert!(steps("bad name#0").is_none());
    }

    /// Texts are set only where a property holds one; scripts and bindings are not changed.
    #[test]
    fn texts_scripts_and_bindings() {
        assert_eq!(check("assist#0/toolTip#0", None, "tip"), Ok(()));
        assert_eq!(check("caption#0/value#0/text#0", None, "Label"), Ok(()));
        assert_eq!(check("items#0/text#3", None, "entry"), Ok(()));
        assert!(check("value#0/text#0", None, "x").is_err(), "the object's value goes through its data");
        assert!(check("font#0", None, "x").is_err());
        assert!(check("validate#0/script#0", None, "true").is_err());
        assert!(check("validate#0/script#0", Some("contentType"), "x").is_err());
        assert!(check("bind#0", Some("match"), "none").is_err());
        assert!(check("bind#0", Some("ref"), "$.x").is_err());
        assert_eq!(check("value#0", Some("override"), "1"), Ok(()));
        assert!(check("font#0", Some("bad name"), "1").is_err());
        assert!(check("assist#0/toolTip#0", None, &"x".repeat(MAX_VALUE + 1)).is_err());
        assert_eq!(check("assist#0/toolTip#0", None, &"x".repeat(MAX_VALUE)), Ok(()));
    }

    #[test]
    fn names() {
        assert!(is_name("_a-1.b"));
        assert!(!is_name("1a"));
        assert!(!is_name(""));
        assert!(!is_name(&"a".repeat(65)));
    }
}
