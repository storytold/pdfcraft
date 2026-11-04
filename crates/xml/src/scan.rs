//! The check [`crate::parse`] runs before roxmltree sees a text: one forward pass that measures
//! nesting depth, refuses a DTD and adds up the parser's superlinear work.
//!
//! # The work model (roxmltree 0.21.2, `src/parse.rs` and `src/lib.rs`)
//!
//! roxmltree keeps, for each element, a list of the namespace bindings in scope: the element's own
//! declarations, all of them, followed by one entry for each inherited prefix it doesn't declare. A
//! default namespace declared twice on one tag is two entries; roxmltree doesn't check those for
//! duplicates (`process_attribute`). The scan keeps an upper bound on that list's length, as counts
//! per prefix: where roxmltree keeps one inherited entry for a prefix, the scan keeps every entry the
//! declaring element had.
//!
//! For each start tag with `a` ordinary attributes, `p` of them prefixed (other than `xml:`), and `x`
//! namespace declarations (`xmlns:n`, and every attribute whose local name is `xmlns`, `p:xmlns`
//! too: roxmltree reads those as default declarations), where `s` is the length of the parent's list and `t` of the element's:
//!
//! | roxmltree loop | comparisons | of what |
//! |---|---|---|
//! | duplicate-attribute check (`resolve_attributes`): each attribute against the ones before it | a², twice over when the tag has namespaced attributes | expanded names: namespace URI, then local name |
//! | duplicate-namespace check (`process_attribute` → `Namespaces::exists`): each declaration against the tag's earlier ones | x² | prefixes |
//! | inherited entries copied into a declaring element (`resolve_namespaces` → `exists`), only when x > 0 | s·(s + x) | prefixes |
//! | prefix lookup (`get_ns_idx_by_prefix`): the list scanned for the element name and each prefixed attribute | t·(1 + p) | prefixes |
//!
//! A string comparison costs one unit plus one per 16 bytes it may compare ([`comparison`]): equal
//! lengths make it a full `memcmp`. Comparing two namespaced attributes resolves both namespaces
//! and compares URIs and then local names, so it is charged as two comparisons. A prefix comparison is charged at the longest prefix declared
//! so far. An attribute comparison is charged at the longest local name on the tag, plus, when the
//! tag has namespaced attributes (their URIs are compared first), the longest namespace URI declared
//! so far. Every count is an upper bound, so the total never under-states the work.
//!
//! Not charged: inserting a new (prefix, URI) pair into roxmltree's sorted namespace table shifts the
//! table, but the table holds at most 65,536 pairs (`NamespacesLimitReached` past it). Measured:
//! 65,000 pairs inserted in the worst order took 170 ms. Everything else is linear in the input, or
//! bounded by the node limit.
//!
//! # Lexical states
//!
//! Comments end at the first `-->` after `<!--`, CDATA at the first `]]>`, processing instructions
//! at the first `?>` after `<?` (so `<!-->` and `<?>` don't close themselves). Quoted attribute
//! values are skipped whole, so a `>`, `=` or `/` inside one counts for nothing. A `<!DOCTYPE`
//! outside those is refused; any other `<!…>` declaration ends at its first `>` (roxmltree refuses
//! those too). A stray `<` in text is not XML, and is counted as a start tag: that can only
//! over-state depth and work, which errs towards refusing.

use std::collections::HashMap;

use crate::{Limits, XmlError};

/// Bytes a string comparison covers per unit of work, on top of one unit for the call. A unit
/// measured between 0.7 and 6 ns across the shapes the tests cover (`memcmp` about 0.1 ns a byte),
/// so 16 bytes errs towards refusing.
const BYTES_PER_UNIT: u64 = 16;

/// The length of the `xml` prefix's namespace URI, which `xml:` attributes compare by.
const XML_NAMESPACE_LEN: usize = "http://www.w3.org/XML/1998/namespace".len();

/// What one comparison of strings up to `bytes` long costs.
fn comparison(bytes: usize) -> u64 {
    1u64.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX) / BYTES_PER_UNIT)
}

/// What [`measure`] found in a text that stayed within its limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The deepest element nesting.
    pub depth: usize,
    /// The parser work, in the model's units.
    pub work: u64,
}

/// Whether `text` stays within `limits`: no DTD, no deeper nesting than `limits.max_depth`, and no
/// more work than `limits.max_work`.
pub fn check(text: &str, limits: Limits) -> Result<(), XmlError> {
    measure(text, limits).map(|_| ())
}

/// [`check`], returning what it measured. Stops at the first limit passed.
pub(crate) fn measure(text: &str, limits: Limits) -> Result<Shape, XmlError> {
    let b = text.as_bytes();
    let mut scope = Scope::default();
    let mut shape = Shape::default();
    let (mut i, mut depth) = (0usize, 0usize);
    while let Some(&c) = b.get(i) {
        let rest = b.get(i..).unwrap_or_default();
        if c != b'<' {
            i += 1;
        } else if rest.starts_with(b"<!--") {
            i = after(b, i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = after(b, i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = after(b, i + 2, b"?>");
        } else if rest.starts_with(b"<!DOCTYPE") {
            return Err(XmlError::Doctype);
        } else if rest.starts_with(b"<!") {
            i = after(b, i + 2, b">");
        } else if rest.starts_with(b"</") {
            scope.close(depth);
            depth = depth.saturating_sub(1);
            i = after(b, i + 2, b">");
        } else {
            depth = depth.saturating_add(1);
            if depth > limits.max_depth {
                return Err(XmlError::TooDeep { limit: limits.max_depth });
            }
            shape.depth = shape.depth.max(depth);
            let tag = StartTag::read(text, i + 1, max_declarations(limits.max_work));
            if tag.too_many_declarations {
                return Err(XmlError::TooComplex { limit: limits.max_work });
            }
            i = tag.end;
            let self_closing = tag.self_closing;
            shape.work = shape.work.saturating_add(scope.open(tag, depth));
            if shape.work > limits.max_work {
                return Err(XmlError::TooComplex { limit: limits.max_work });
            }
            if self_closing {
                scope.close(depth);
                depth = depth.saturating_sub(1);
            }
        }
    }
    Ok(shape)
}

/// The most namespace declarations one start tag may have within `max_work`: past it, their
/// duplicate check alone (x² comparisons of at least one unit) is over the budget, so the scan stops
/// collecting them, and its memory stays bounded by the budget rather than by the input.
fn max_declarations(max_work: u64) -> usize {
    usize::try_from(max_work.isqrt()).unwrap_or(usize::MAX)
}

/// Past the first `needle` at or after `from`; the end of `b` when there is none.
fn after(b: &[u8], from: usize, needle: &[u8]) -> usize {
    b.get(from..)
        .and_then(|s| s.windows(needle.len()).position(|w| w == needle))
        .map_or(b.len(), |p| from.saturating_add(p).saturating_add(needle.len()))
}

/// The parts of a start tag the work model needs.
#[derive(Debug, Default)]
struct StartTag<'a> {
    /// Ordinary attributes (not namespace declarations).
    attributes: u64,
    /// Ordinary attributes with a prefix other than `xml` (`xml:` needs no lookup).
    prefixed: u64,
    /// The longest local name among the ordinary attributes.
    longest_local: usize,
    /// Whether an ordinary attribute has the `xml:` prefix.
    xml_attribute: bool,
    /// The declarations: prefix (`""` for the default namespace) and the length of the URI.
    declares: Vec<(&'a str, usize)>,
    /// Whether it has more declarations than the scan collects (see [`max_declarations`]).
    too_many_declarations: bool,
    /// Whether it ends in `/>`.
    self_closing: bool,
    /// Where the scan continues: past its `>`, or the end of the text.
    end: usize,
}

impl<'a> StartTag<'a> {
    /// The start tag whose name begins at `from` (just after its `<`). Every position sliced at is
    /// that of an ASCII delimiter, so every slice is on a char boundary; `get` keeps it total anyway.
    fn read(text: &'a str, from: usize, max_declarations: usize) -> Self {
        let b = text.as_bytes();
        let mut tag = StartTag::default();
        let (mut i, mut quote, mut last) = (from, 0u8, 0u8);
        // The token being read, and the one before it: an attribute's name is the last token before
        // its `=`. The first token is the element's own name.
        let (mut token, mut previous): (Option<usize>, Option<(usize, usize)>) = (None, None);
        let mut named = false;
        // The declaration whose quoted URI is being read, and where its value starts.
        let (mut declaring, mut value_start) = (false, 0usize);
        while let Some(&c) = b.get(i) {
            if quote != 0 {
                if c == quote {
                    quote = 0;
                    if declaring && let Some(d) = tag.declares.last_mut() {
                        d.1 = i.saturating_sub(value_start);
                    }
                    declaring = false;
                }
                i += 1;
                continue;
            }
            let ends_token = matches!(c, b'"' | b'\'' | b'>' | b'=') || c.is_ascii_whitespace();
            if ends_token && let Some(start) = token.take() {
                if named {
                    previous = Some((start, i));
                } else {
                    named = true;
                }
            }
            match c {
                b'>' => {
                    i += 1;
                    break;
                }
                b'"' | b'\'' => {
                    quote = c;
                    value_start = i + 1;
                    last = c;
                }
                b'=' => {
                    declaring = tag.attribute(previous.take().and_then(|(s, e)| text.get(s..e)).unwrap_or_default(), max_declarations);
                    if tag.too_many_declarations {
                        break;
                    }
                    last = c;
                }
                c if c.is_ascii_whitespace() => {}
                _ => {
                    token.get_or_insert(i);
                    last = c;
                }
            }
            i += 1;
        }
        tag.self_closing = last == b'/';
        tag.end = i;
        tag
    }

    /// Count the attribute called `name` (empty when the `=` had no name before it); whether it is
    /// a namespace declaration, whose value is the URI. Classified as roxmltree's
    /// `process_attribute` does: the prefix `xmlns` declares the local name, and otherwise the
    /// local name `xmlns` declares the default namespace, whatever its prefix (`p:xmlns`).
    fn attribute(&mut self, name: &'a str, max_declarations: usize) -> bool {
        let (prefix, local) = name.split_once(':').unwrap_or(("", name));
        let declared = if prefix == "xmlns" {
            Some(local)
        } else if local == "xmlns" {
            Some("")
        } else {
            None
        };
        if let Some(declared) = declared {
            if self.declares.len() >= max_declarations {
                self.too_many_declarations = true;
            } else {
                self.declares.push((declared, 0));
            }
            return true;
        }
        self.attributes = self.attributes.saturating_add(1);
        match prefix {
            "" => {}
            "xml" => self.xml_attribute = true,
            _ => self.prefixed = self.prefixed.saturating_add(1),
        }
        self.longest_local = self.longest_local.max(local.len());
        false
    }
}

/// roxmltree's in-scope namespace list as the scan goes (see the module docs).
#[derive(Debug, Default)]
struct Scope<'a> {
    /// How many entries the current list holds for each prefix; absent means none.
    entries: HashMap<&'a str, u64>,
    /// The current list's length: the sum of `entries`.
    len: u64,
    /// The longest prefix and namespace URI declared so far: an upper bound on what any comparison
    /// of them covers.
    longest_prefix: usize,
    longest_uri: usize,
    /// For each open element that declares namespaces: its depth, and the entry counts its
    /// declarations replaced, to put back when it closes.
    frames: Vec<(usize, Vec<(&'a str, u64)>)>,
}

impl<'a> Scope<'a> {
    /// Open the element `tag` at `depth`: put its declarations in scope, and return what roxmltree's
    /// superlinear loops will spend on it (the table in the module docs).
    fn open(&mut self, tag: StartTag<'a>, depth: usize) -> u64 {
        for &(prefix, uri) in &tag.declares {
            self.longest_prefix = self.longest_prefix.max(prefix.len());
            self.longest_uri = self.longest_uri.max(uri);
        }
        let prefix_cost = comparison(self.longest_prefix);
        // Attributes are compared by namespace URI first; only namespaced ones have one to compare.
        let mut uri_bytes = if tag.xml_attribute { XML_NAMESPACE_LEN } else { 0 };
        if tag.prefixed > 0 {
            uri_bytes = uri_bytes.max(self.longest_uri);
        }
        let comparisons = if uri_bytes > 0 { 2 } else { 1 };
        let attribute_cost = comparison(uri_bytes.saturating_add(tag.longest_local)).saturating_mul(comparisons);
        let declared = u64::try_from(tag.declares.len()).unwrap_or(u64::MAX);
        let a = tag.attributes;
        let mut work = a.saturating_mul(a).saturating_mul(attribute_cost);
        work = work.saturating_add(declared.saturating_mul(declared).saturating_mul(prefix_cost));
        if !tag.declares.is_empty() {
            let inherited = self.len;
            work = work.saturating_add(inherited.saturating_mul(inherited.saturating_add(declared)).saturating_mul(prefix_cost));
            // The element's own entries replace the parent's for every prefix it declares.
            let mut own: HashMap<&'a str, u64> = HashMap::new();
            for &(prefix, _) in &tag.declares {
                let n = own.entry(prefix).or_insert(0);
                *n = n.saturating_add(1);
            }
            let mut replaced = Vec::with_capacity(own.len());
            for (prefix, n) in own {
                let old = self.entries.insert(prefix, n).unwrap_or(0);
                self.len = self.len.saturating_sub(old).saturating_add(n);
                replaced.push((prefix, old));
            }
            self.frames.push((depth, replaced));
        }
        let lookups = tag.prefixed.saturating_add(1);
        work.saturating_add(self.len.saturating_mul(lookups).saturating_mul(prefix_cost))
    }

    /// Close the element at `depth`: put back the entries its declarations replaced, if any.
    fn close(&mut self, depth: usize) {
        if self.frames.last().is_none_or(|(d, _)| *d != depth) {
            return;
        }
        let Some((_, replaced)) = self.frames.pop() else { return };
        for (prefix, old) in replaced {
            let current = if old == 0 { self.entries.remove(prefix) } else { self.entries.insert(prefix, old) };
            self.len = self.len.saturating_sub(current.unwrap_or(0)).saturating_add(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tag with more declarations than fit the budget stops being collected at the cap, instead of
    /// growing with the input before the budget is checked.
    #[test]
    fn declarations_past_the_cap_are_not_collected() {
        let declarations: String = (0..10).map(|i| format!(" xmlns:p{i}='u'")).collect();
        let text = format!("<r{declarations}/>");
        let tag = StartTag::read(&text, 1, 4);
        assert!(tag.too_many_declarations);
        assert_eq!(tag.declares.len(), 4);
        let tag = StartTag::read(&text, 1, 10);
        assert!(!tag.too_many_declarations);
        assert_eq!(tag.declares.len(), 10);
        assert_eq!(max_declarations(crate::MAX_WORK), 8_192);
    }
}
