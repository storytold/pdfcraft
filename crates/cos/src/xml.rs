//! Bounds for the XML that PDF features read (XFA forms, XFDF), checked before an XML parser builds
//! a tree from it.

/// Element nesting depth of `text`, skipping comments, CDATA, processing instructions, the DOCTYPE
/// and quoted attribute values. A tag that closes itself (`<field/>`) is one level while it is read
/// and does not stay open, so a long run of them cannot accumulate into the bound. A DOCTYPE is
/// skipped whole, its internal subset and quoted literals included (one malformed enough to leave
/// a quote or `[` open runs to the end of the text, but every caller refuses DTDs anyway).
/// Otherwise it cannot under-count a real element; a stray `<` in text is invalid XML and only
/// makes it over-count, which errs towards refusing the document.
pub fn xml_nesting_depth(text: &str) -> usize {
    xml_shape(text).depth
}

/// How deep and how attribute-heavy a piece of XML is, as [`xml_shape`] measures it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XmlShape {
    /// The element nesting depth ([`xml_nesting_depth`]).
    pub depth: usize,
    /// Attributes in all start tags: each unquoted `=` in a start tag introduces one, namespace
    /// bindings included.
    pub attributes: usize,
    /// The most attributes in any one start tag.
    pub max_tag_attributes: usize,
}

/// [`xml_nesting_depth`] together with the attribute counts, from the same single scan, so a
/// reader can bound an XML parser's work before it builds a tree.
pub fn xml_shape(text: &str) -> XmlShape {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth) = (0usize, 0usize);
    let mut shape = XmlShape::default();
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!DOCTYPE") {
            i = doctype_end(b, i + 9);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            shape.depth = shape.depth.max(depth);
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            let mut attributes = 0usize;
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
                } else if c == b'=' {
                    attributes += 1;
                    shape.attributes = shape.attributes.saturating_add(1);
                    last = c;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            shape.max_tag_attributes = shape.max_tag_attributes.max(attributes);
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    shape
}

/// Past the end of the DOCTYPE whose body starts at `from`: its `>`, skipping quoted literals and
/// its internal subset (`[ … ]`, where an entity value or a comment can hold `>` and `<`). One
/// forward pass; an unterminated DOCTYPE runs to the end of the text.
fn doctype_end(b: &[u8], from: usize) -> usize {
    let (mut i, mut quote, mut subset) = (from, 0u8, false);
    while let Some(&c) = b.get(i) {
        i += 1;
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if subset && c == b'<' && b.get(i..).is_some_and(|s| s.starts_with(b"!--")) {
            // A comment in the subset, which may hold a lone quote.
            i = b.get(i + 3..).and_then(|s| s.windows(3).position(|w| w == b"-->")).map_or(b.len(), |p| i + 3 + p + 3);
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'[' => subset = true,
                b']' => subset = false,
                b'>' if !subset => return i,
                _ => {}
            }
        }
    }
    b.len()
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

    /// A DOCTYPE's internal subset is skipped whole: markup in an entity value or a comment
    /// there is not counted, and a `>` inside it does not end the DOCTYPE.
    #[test]
    fn a_doctype_internal_subset_is_skipped() {
        assert_eq!(d(r#"<!DOCTYPE a [<!ENTITY e "<x><y><z>">]><a/>"#), 1);
        assert_eq!(d(r#"<!DOCTYPE r [<!ENTITY e "a> <y q=">]><r><a/></r>"#), 2);
        assert_eq!(d(r#"<!DOCTYPE r SYSTEM "a>b"><r><a/></r>"#), 2);
        assert_eq!(d("<!DOCTYPE r [<!-- it's > --><!ENTITY e 'v'>]><r><a/></r>"), 2);
        assert_eq!(d("<!DOCTYPE r [<!ENTITY e 'v'"), 0, "an unterminated subset runs to the end");
        assert_eq!(d(&format!("<!DOCTYPE r [{}<r/>", "[".repeat(100_000))), 0, "one pass");
        // Another declaration still ends at its first `>`, a quote in it or not.
        assert_eq!(d(r#"<a><!x "><b><c/></b></a>"#), 3);
    }

    /// Each unquoted `=` in a start tag is an attribute; one in a quoted value, an end tag,
    /// a comment, CDATA or a processing instruction is not.
    #[test]
    fn attributes_are_counted_per_tag_and_in_all() {
        use super::{XmlShape, xml_shape};
        let shape = |depth, attributes, max_tag_attributes| XmlShape { depth, attributes, max_tag_attributes };
        assert_eq!(xml_shape(r#"<a x="1" y='2'><b z="a=b"/></a>"#), shape(2, 3, 2));
        assert_eq!(xml_shape(r#"<?pi a="1"?><!-- b="2" --><a><![CDATA[c="3"]]></a d="4">"#), shape(1, 0, 0));
        assert_eq!(xml_shape(r#"<r xmlns:n="urn:x" n:a="1"/>"#), shape(1, 2, 2));
        assert_eq!(xml_shape(""), XmlShape::default());
    }
}
