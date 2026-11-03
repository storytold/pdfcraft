//! Bounds for the XML that PDF features read (XFA forms, XFDF), checked before an XML parser builds
//! a tree from it.

/// Element nesting depth of `text`, skipping comments, CDATA, processing instructions, the DOCTYPE
/// and quoted attribute values. A tag that closes itself (`<field/>`) is one level while it is read
/// and does not stay open, so a long run of them cannot accumulate into the bound. It cannot
/// under-count a real element; a stray `<` in text is invalid XML and only makes it over-count,
/// which errs towards refusing the document.
pub fn xml_nesting_depth(text: &str) -> usize {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            max = max.max(depth);
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    i += 1;
                    break;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::xml_nesting_depth as d;

    /// A tag that closes itself is one level while it is read, not a level left open, so a flat
    /// file with thousands of them stays shallow.
    #[test]
    fn self_closing_elements_do_not_accumulate_in_the_depth() {
        assert_eq!(d("<a/>"), 1);
        assert_eq!(d("<a><b/></a>"), 2);
        assert_eq!(d(&"<empty/>".repeat(2_000)), 1);
        assert_eq!(d(&format!("<fields>{}</fields>", r#"<field name="a"/>"#.repeat(2_000))), 2);
    }

    /// Markup that is not an element does not count, and a quoted `>`, `<` or `/` does not end or
    /// start a tag; nothing real is under-counted.
    #[test]
    fn comments_cdata_instructions_and_quotes_are_skipped() {
        assert_eq!(d("<a><b><c/></b></a>"), 3);
        assert_eq!(d(r#"<?xml version="1.0"?><!-- <x> --><a/>"#), 1);
        assert_eq!(d("<a><![CDATA[<not-an-element>]]></a>"), 1);
        assert_eq!(d(r#"<a b="<"><c/></a>"#), 2);
        assert_eq!(d("<?xml version=\"1.0\"?><!DOCTYPE x><a><!-- <b><c> --><![CDATA[<d><e>]]></a>"), 1);
        assert_eq!(d(r#"<a t="x>y/"><b/></a>"#), 2);
        assert_eq!(d("<a><b><c></c></b></a>"), 3);
        assert_eq!(d(""), 0);
        assert_eq!(d("<a><b"), 2, "an unterminated tag still counts");
    }
}
