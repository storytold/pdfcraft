//! Wordings with `{name}` holes, filled for the reports and messages crates build from them.

/// A wording with its holes filled.
///
/// A hole is a name in braces, such as `{p}`; what is not in `values` is left as it is written.
/// Values go in during a single pass over the wording, so a value that itself contains braces is
/// never read as a hole: a document's own words reach the screen as they stand.
pub fn say(pattern: &str, values: &[(&str, String)]) -> String {
    say_with(pattern, |hole| values.iter().find(|(name, _)| *name == hole).map(|(_, value)| value.as_str()))
}

/// [`say`] with each hole's value from `lookup`, which gets the hole with its braces (`{p}`) and
/// returns `None` to leave it as written. For callers whose values are borrowed or keyed another
/// way, so nothing is copied to build a table first.
pub fn say_with<'v>(pattern: &str, lookup: impl Fn(&str) -> Option<&'v str>) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        // `find` counts from the brace, and `}` is one byte, so this is a char boundary; `get`
        // rather than slicing in case it ever isn't.
        let Some(close) = from_brace.find('}') else {
            // A brace with nothing closing it is just text.
            out.push_str(from_brace);
            return out;
        };
        let Some(hole) = from_brace.get(..=close) else { return out + from_brace };
        out.push_str(lookup(hole).unwrap_or(hole));
        rest = from_brace.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{say, say_with};

    /// `say` puts the values in, and what it puts in is never read as a hole of its own: a document
    /// that names a font `{p}` cannot make a finding say which page it is on.
    #[test]
    fn say_fills_holes_once() {
        let values = [("{p}", "7".to_string()), ("{}", "{p} {n}".to_string())];
        assert_eq!(
            say("Font {} on page {p} doesn't map its characters to Unicode", &values),
            "Font {p} {n} on page 7 doesn't map its characters to Unicode"
        );
        // A hole with nothing for it, and a brace with nothing closing it, are left as they stand.
        assert_eq!(say("{a} {p} {", &values), "{a} 7 {");
        assert_eq!(say("é {p} ü", &values), "é 7 ü");
        assert_eq!(say("", &values), "");
        // A hole is from a `{` to the next `}`: braces are not nested, so `{{p}}` names `{{p}`.
        assert_eq!(say("{{p}} {a{p}}", &values), "{{p}} {a{p}}");
    }

    /// `say_with` hands the lookup each hole with its braces, leaves a hole it has nothing for as
    /// written, and doesn't read a value as a hole.
    #[test]
    fn say_with_asks_for_each_hole() {
        let lookup = |hole: &str| match hole {
            "{p}" => Some("7"),
            "{name}" => Some("{p}"),
            _ => None,
        };
        assert_eq!(say_with("{name} on page {p}, {other}", lookup), "{p} on page 7, {other}");
        assert_eq!(say_with("open {p", lookup), "open {p");
    }
}
