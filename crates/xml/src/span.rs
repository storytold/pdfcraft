//! Byte spans inside a parsed tree, for edits that splice the original text instead of
//! re-serialising it (so everything an edit doesn't touch stays byte for byte).
//!
//! Each function takes the text the tree was parsed from and a range roxmltree reported for a
//! node or attribute (`Node::range`, `Attribute::range`). roxmltree has already judged the text
//! well-formed, so these only have to tell apart what a well-formed start tag can hold: quoted
//! attribute values. They never run on unparsed text.

use std::ops::Range;

/// Where an element's start tag ends (the index of its `>`), skipping quoted attribute values,
/// which may hold a `>`. `element` is the element's `Node::range`.
pub fn start_tag_end(text: &str, element: &Range<usize>) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote = None;
    for i in element.clone() {
        let b = *bytes.get(i)?;
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// An element's qualified name as written (`dc:title`). `element` is its `Node::range`.
pub fn qname<'s>(text: &'s str, element: &Range<usize>) -> Option<&'s str> {
    let rest = text.get(element.start.checked_add(1)?..element.end)?;
    let end = rest.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    rest.get(..end).filter(|n| !n.is_empty())
}

/// Where an element's content is: from past its start tag to its end tag's `<`. `None` for an
/// empty-element tag (`<a/>`), which has no content to replace. `element` is its `Node::range`.
///
/// The end tag is the last `</` in the element's range: comments, CDATA sections and processing
/// instructions inside the element all end before it, so a `</` they hold comes earlier.
pub fn content(text: &str, element: &Range<usize>) -> Option<Range<usize>> {
    let tag_end = start_tag_end(text, element)?;
    if text.get(..tag_end)?.ends_with('/') {
        return None;
    }
    let start = tag_end.checked_add(1)?;
    let end = element.start.checked_add(text.get(element.clone())?.rfind("</")?)?;
    (start <= end).then_some(start..end)
}

/// An attribute's value as written, without its quotes, and the quote character. `attribute` is
/// its `Attribute::range` (from the name through the closing quote).
///
/// roxmltree's own `Attribute::range_value` stores the name length in 16 bits and the `=` with
/// its spaces in 8, so it is wrong past a 65,535-byte name or 254 spaces around `=`. This finds
/// the opening quote instead: names hold no quotes, so it is the first one.
pub fn attribute_value(text: &str, attribute: &Range<usize>) -> Option<(Range<usize>, char)> {
    let written = text.get(attribute.clone())?;
    let open = written.find(['"', '\''])?;
    let quote = written.get(open..)?.chars().next()?;
    let close = attribute.end.checked_sub(1)?;
    (text.get(close..attribute.end)? == quote.to_string()).then_some(())?;
    let start = attribute.start.checked_add(open)?.checked_add(1)?;
    (start <= close).then_some((start..close, quote))
}
