//! Wordings with `{name}` holes, filled for the reports and messages crates build from them.

/// A wording with its holes filled.
///
/// A hole is a name in braces, such as `{p}`; what is not in `values` is left as it is written.
/// Values go in during a single pass over the wording, so a value that itself contains braces is
/// never read as a hole: a document's own words reach the screen as they stand.
pub fn say(pattern: &str, values: &[(&str, String)]) -> String {
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
        match values.iter().find(|(name, _)| *name == hole) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(hole),
        }
        rest = from_brace.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::say;

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
}
