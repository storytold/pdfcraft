//! The text of a book that isn't well-formed is read loosely; its character references are
//! decoded in one pass.

use super::*;

#[test]
fn hex_references_are_decoded() {
    assert_eq!(decode_entities("&#x201C;Hi&#X201d; &#65; &#x1F600;"), "\u{201C}Hi\u{201D} A \u{1F600}");
}

#[test]
fn an_escaped_ampersand_is_decoded_once() {
    assert_eq!(decode_entities("a &amp;lt; b"), "a &lt; b");
    assert_eq!(decode_entities("&amp;#65;"), "&#65;");
    assert_eq!(decode_entities("&amp;amp;"), "&amp;");
}

#[test]
fn named_and_odd_references() {
    assert_eq!(decode_entities("&lt;b&gt; &quot;q&quot; &apos;s&apos; &amp; &mdash;"), "<b> \"q\" 's' & \u{2014}");
    // Not references: left as they are.
    assert_eq!(decode_entities("AT&T; &#xZZ; &#; & done &#1114112;"), "AT&T; &#xZZ; &#; & done &#1114112;");
}
