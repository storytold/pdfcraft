//! Every input here is small, or refused by the linear scan before roxmltree sees it: no test may
//! reach the parser's quadratic loops.

use crate::scan::{Shape, measure};
use crate::{Limits, MAX_DEPTH, XmlError, parse};

/// Generous enough for every test shape, so only the bound under test can refuse.
const ROOMY: Limits = Limits { max_depth: MAX_DEPTH, max_nodes: 1_000_000, max_work: u64::MAX };

fn depth(text: &str) -> usize {
    measure(text, ROOMY).map(|s| s.depth).unwrap_or(usize::MAX)
}

fn work(text: &str) -> u64 {
    measure(text, ROOMY).map(|s| s.work).unwrap_or(u64::MAX)
}

fn with_work(max_work: u64) -> Limits {
    Limits { max_work, ..ROOMY }
}

// ── lexical states (slop-hunt's XML scanner table, rows 1–15) ─────────────────────────────────

/// Rows 1, 2: a quoted `>` (or a quote of the other kind) doesn't end a start tag.
#[test]
fn quoted_values_do_not_end_a_tag() {
    assert_eq!(depth(r#"<a t="x>y"><b/></a>"#), 2);
    assert_eq!(depth(r#"<a t='x>"y'><b/></a>"#), 2);
    assert_eq!(depth(r#"<a t="x>y/"><b/></a>"#), 2, "a quoted `/` doesn't close the tag");
    assert_eq!(depth(r#"<a t="/>"><b/></a>"#), 2, "a quoted `/>` doesn't close the tag, which would under-count");
    assert_eq!(depth(r#"<a t='/>'><b/></a>"#), 2);
    assert_eq!(depth(r#"<a b="<"><c/></a>"#), 2);
}

/// Rows 3, 4, 7: a processing instruction ends only at `?>`, searched after its `<?`; markup in it is not counted.
#[test]
fn processing_instructions_end_at_their_terminator() {
    assert_eq!(depth("<?pi a > b ?><r/>"), 1);
    assert_eq!(depth("<a><?>?><b/></a>"), 2, "`<?>` is not a whole instruction");
    assert_eq!(depth("<r><?x <a><b> ?></r>"), 1);
    assert_eq!(depth(r#"<?xml version="1.0"?><a/>"#), 1);
}

/// Rows 4, 5, 6, 8: comments end at the first `-->` after `<!--`, CDATA at `]]>`; neither counts what it holds.
#[test]
fn comments_and_cdata_are_skipped() {
    assert_eq!(depth("<!-->x--><r><s/></r>"), 2, "`<!-->` is not a whole comment");
    assert_eq!(depth("<a><![CDATA[<b></b>]]></a>"), 1);
    assert_eq!(depth("<!-- <rdf:li>x</rdf:li> --><r/>"), 1);
    assert_eq!(depth("<r><!-- </r> --></r>"), 1);
    assert_eq!(depth("<a><!-- <b><c> --><![CDATA[<d><e>]]></a>"), 1);
    assert_eq!(work(r#"<?pi a="1"?><!-- b="2" --><a><![CDATA[c="3"]]></a>"#), 0, "no attributes counted outside start tags");
}

/// Row 9 and trailing markup: a comment after the root changes nothing.
#[test]
fn trailing_comments_count_for_nothing() {
    assert_eq!(depth("<x><r/></x><!-- </r> <y><z> -->"), 2);
}

/// Row 10: an attribute-like text inside another attribute's value is not an attribute.
#[test]
fn attribute_lookalikes_in_values_are_not_attributes() {
    // Two attributes, compared by the xml namespace's URI (36 bytes) and a 4-byte local name, two
    // comparisons each; no prefix lookup for `xml:`.
    assert_eq!(work(r#"<a note='xml:lang="de"' xml:lang="en"/>"#), 4 * 6);
    assert_eq!(work(r#"<a b="x=y=z"/>"#), 1);
}

/// Row 12: a DOCTYPE is refused on sight, internal subset or not, but not when it is only text.
#[test]
fn a_doctype_is_refused_unless_it_is_only_text() {
    for text in
        ["<!DOCTYPE r><r/>", r#"<!DOCTYPE r [<!ENTITY e "<x>"> <!-- > -->]><r/>"#, "<!DOCTYPE r [<?pi it's?>]><a><b><c/></b></a>", "<r/><!DOCTYPE r>"]
    {
        assert_eq!(measure(text, ROOMY), Err(XmlError::Doctype), "{text}");
        assert_eq!(parse(text, ROOMY).err(), Some(XmlError::Doctype), "{text}");
    }
    for text in ["<!-- <!DOCTYPE r> --><r/>", "<r><![CDATA[<!DOCTYPE r>]]></r>", "<?pi <!DOCTYPE r> ?><r/>"] {
        assert!(parse(text, ROOMY).is_ok(), "{text}");
    }
}

/// Row 13: a tag that closes itself is one level while it is read, not a level left open.
#[test]
fn self_closing_elements_do_not_accumulate() {
    assert_eq!(depth("<a/>"), 1);
    assert_eq!(depth("<a/><a/><a/>"), 1);
    assert_eq!(depth("<a><b/></a>"), 2);
    assert_eq!(depth(&"<empty/>".repeat(2_000)), 1);
    assert_eq!(depth(&format!("<fields>{}</fields>", r#"<field name="a"/>"#.repeat(2_000))), 2);
}

/// Rows 14, 15: every cut of a document, wherever it ends (inside a comment, CDATA, a PI, a quoted
/// value, a tag, or a multi-byte character's neighbours), is scanned to its end without panicking,
/// and a cut never counts deeper than the whole by more than the one level a trailing `<` adds.
#[test]
fn every_truncation_is_scanned() {
    let doc = r#"<?xml version="1.0"?><r a="1>" b='/' xmlns:é="urn:é"><!-- c ' --><![CDATA[<d>]]><é:s/><t u="v">é</t></r>"#;
    let whole = measure(doc, ROOMY).unwrap();
    assert_eq!(whole.depth, 2);
    for end in (0..=doc.len()).filter(|&e| doc.is_char_boundary(e)) {
        let cut = measure(&doc[..end], ROOMY).unwrap();
        assert!(cut.depth <= whole.depth + 1, "{end}");
    }
    for text in ["<!-- never closed", r#"<a t="never"#, "<a", "<", "</", "<!", "<?", "<![CDATA[", "é<é é='é'>"] {
        assert!(measure(text, ROOMY).is_ok(), "{text}");
    }
}

// ── the work model ────────────────────────────────────────────────────────────────────────────

/// Duplicate-attribute checking costs the square of a tag's attributes: the budget refuses one past it.
#[test]
fn attributes_cost_their_square() {
    let tag = |n: usize| format!("<p{}/>", (0..n).map(|i| format!(" a{i}='v'")).collect::<String>());
    assert_eq!(work(&tag(10)), 100);
    assert!(parse(&tag(10), with_work(100)).is_ok());
    assert_eq!(parse(&tag(11), with_work(100)).err(), Some(XmlError::TooComplex { limit: 100 }));
}

/// Namespace declarations on one tag: their duplicate check (x²) plus the element's prefix lookup.
#[test]
fn declarations_cost_their_square_and_the_lookup() {
    let tag = |n: usize| format!("<p{}/>", (0..n).map(|i| format!(" xmlns:n{i}='u{i}'")).collect::<String>());
    assert_eq!(work(&tag(9)), 81 + 9);
    assert!(parse(&tag(9), with_work(90)).is_ok());
    assert_eq!(parse(&tag(10), with_work(90)).err(), Some(XmlError::TooComplex { limit: 90 }));
    // A prefixed attribute is one more lookup, and its duplicate check counts twice.
    assert_eq!(work(r#"<r xmlns:n="urn:x" n:a="1"/>"#), 2 + 1 + 2);
}

/// Each element declaring a new prefix copies every inherited binding: a chain of n such elements
/// costs Σ (k² + 1).
#[test]
fn inherited_bindings_are_charged_to_each_declaring_element() {
    let chain = |n: usize| {
        let open: String = (0..n).map(|k| format!("<e xmlns:p{k}='u{k}'>")).collect();
        format!("{open}{}", "</e>".repeat(n))
    };
    assert_eq!(work(&chain(1)), 2);
    assert_eq!(work(&chain(3)), 2 + 5 + 10);
    assert_eq!(work(&chain(20)), (1..=20u64).map(|k| k * k + 1).sum::<u64>());
}

/// A prefix declared again where it is already bound shadows, so generators that repeat
/// `xmlns:xsi` on every element stay linear.
#[test]
fn redeclaring_a_bound_prefix_stays_linear() {
    let doc = format!("<r xmlns:xsi='u'>{}</r>", "<c xmlns:xsi='u'/>".repeat(1_000));
    assert_eq!(work(&doc), 2 + 4 * 1_000);
    // Declarations leave scope with their element.
    let siblings = format!("<r>{}</r>", (0..100).map(|i| format!("<c xmlns:p{i}='u'/>")).collect::<String>());
    assert_eq!(work(&siblings), 100 * (1 + 1));
}

/// Comparisons cost by the bytes they cover: one unit plus one per 16 bytes. Namespaced attributes
/// are compared by URI first, so a long URI makes every pair of them expensive; long local names
/// do the same for any attributes.
#[test]
fn comparisons_are_charged_by_their_length() {
    let uri = "u".repeat(32);
    // Two prefixed attributes: 2² comparisons, each resolving both namespaces and comparing a
    // 32-byte URI and a 1-byte name (2 × 3 units), one declaration, and the element and two
    // attributes looked up in a one-entry list.
    assert_eq!(work(&format!("<r xmlns:p='{uri}' p:a='' p:b=''/>")), 4 * 6 + 1 + 3);
    // The same names unprefixed have no URI to compare.
    assert_eq!(work(&format!("<r xmlns:p='{uri}' a='' b=''/>")), 4 + 1 + 1);
    let name = "n".repeat(32);
    assert_eq!(work(&format!("<r {name}1='' {name}2=''/>")), 4 * 3);
    // At the default budget: 11,000 attributes sharing one 100 KB URI (10 minutes for the
    // parser), and 2,000 attributes with 1 KB names.
    let limits = Limits::new(2_000_000);
    let complex = Err(XmlError::TooComplex { limit: crate::MAX_WORK });
    let attrs: String = (0..11_000).map(|i| format!(" p:a{i}=''")).collect();
    assert_eq!(crate::scan::check(&format!("<r xmlns:p='{}'{attrs}/>", "u".repeat(100_000)), limits), complex);
    // 10,000 prefixed attributes under one short URI: two namespace lookups and two comparisons
    // per pair (measured 17 ns a pair).
    let attrs: String = (0..10_000).map(|i| format!(" p:a{i:04}=''")).collect();
    assert_eq!(crate::scan::check(&format!("<r xmlns:p='uuuuuuuuuu'{attrs}/>"), limits), complex);
    let attrs: String = (0..2_000).map(|i| format!(" {}{i:05}=''", "n".repeat(1_000))).collect();
    assert_eq!(crate::scan::check(&format!("<r{attrs}/>"), limits), complex);
}

/// roxmltree doesn't check a default namespace declared twice on one tag: every declaration is an
/// entry in the in-scope list, which each descendant scans for its own name.
#[test]
fn repeated_default_declarations_lengthen_the_list() {
    // Three declarations (3²), a three-entry list for the root and for each child.
    assert_eq!(work("<r xmlns='u' xmlns='u' xmlns='u'><c/><c/></r>"), 9 + 3 + 3 + 3);
    let defaults = " xmlns='u'".repeat(3_000);
    let doc = format!("<r{defaults} xmlns:k='k'>{}</r>", "<k:c/>".repeat(50_000));
    assert_eq!(crate::scan::check(&doc, Limits::new(2_000_000)), Err(XmlError::TooComplex { limit: crate::MAX_WORK }));
    // roxmltree reads any attribute whose local name is `xmlns` as a default declaration, whatever
    // its prefix, so `p:xmlns` lengthens the list too.
    assert_eq!(work("<r p:xmlns='u' q:xmlns='u' xmlns='u'><c/><c/></r>"), 9 + 3 + 3 + 3);
    let prefixed = " p:xmlns='u'".repeat(3_000);
    let doc = format!("<r{prefixed} xmlns:k='k'>{}</r>", "<k:c/>".repeat(50_000));
    assert_eq!(crate::scan::check(&doc, Limits::new(2_000_000)), Err(XmlError::TooComplex { limit: crate::MAX_WORK }));
}

/// One tag with more declarations than the budget's square root is refused as soon as the scan
/// reaches the one past it.
#[test]
fn declarations_past_the_budget_square_root_are_refused() {
    let tag = |n: usize| format!("<p{}/>", (0..n).map(|i| format!(" xmlns:n{i}='u'")).collect::<String>());
    // 64 declarations: 64² for their duplicate check and 64 for the element name's lookup, exactly
    // the budget, whose square root is 64.
    let budget = 64 * 64 + 64;
    assert_eq!(work(&tag(64)), budget);
    assert_eq!(crate::scan::check(&tag(64), with_work(budget)), Ok(()));
    assert_eq!(crate::scan::check(&tag(65), with_work(budget)), Err(XmlError::TooComplex { limit: budget }));
}

/// The shapes measured hanging roxmltree 0.21.2 (9.8 s, 9.2 s, 156 s) are refused by the scan,
/// which stops at the budget, long before the parser would have started.
#[test]
fn shapes_that_hang_the_parser_are_refused_at_the_default_budget() {
    let limits = Limits::new(5_000_000);
    let complex = Err(XmlError::TooComplex { limit: crate::MAX_WORK });
    let attributes = format!("<p{}/>", (0..60_000).map(|i| format!(" a{i}='v'")).collect::<String>());
    assert_eq!(crate::scan::check(&attributes, limits), complex);
    let declarations = format!("<p{}/>", (0..60_000).map(|i| format!(" xmlns:n{i}='u{i}'")).collect::<String>());
    assert_eq!(crate::scan::check(&declarations, limits), complex);
    let root: String = (0..2_000).map(|i| format!(" xmlns:n{i}='u{i}'")).collect();
    let children: String = (0..20_000).map(|i| format!("<c xmlns:k{i}='k'/>")).collect();
    assert_eq!(crate::scan::check(&format!("<r{root}>{children}</r>"), limits), complex);
    // 1,024 nested elements declaring four prefixes each: within the old per-tag (128), total
    // (4,096) and depth (1,024) caps, yet about 5·10⁹ comparisons for the parser.
    let open: String = (0..1_024).map(|k| format!("<e{}>", (0..4).map(|j| format!(" xmlns:p{k}x{j}='u'")).collect::<String>())).collect();
    assert_eq!(crate::scan::check(&format!("{open}{}", "</e>".repeat(1_024)), limits), complex);
}

/// What real documents look like stays far inside the default budget, and parses.
#[test]
fn large_ordinary_documents_are_within_the_default_budget() {
    let fields: String = (0..50_000).map(|i| format!(r#"<field name="f{i}" x="1in" y="2in" w="3in" h="4mm"/>"#)).collect();
    let doc = format!(r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform>{fields}</subform></template>"#);
    let shape: Shape = measure(&doc, Limits::new(2_000_000)).unwrap();
    assert!(shape.work < crate::MAX_WORK / 20, "{}", shape.work);
    assert!(parse(&doc, Limits::new(2_000_000)).is_ok());
}

// ── parse ─────────────────────────────────────────────────────────────────────────────────────

/// The depth bound is inclusive, and deep text is refused by the scan without parsing.
#[test]
fn depth_is_bounded_before_parsing() {
    let nested = |n: usize| format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
    let limits = ROOMY.with_max_depth(3);
    assert!(parse(&nested(3), limits).is_ok());
    assert_eq!(parse(&nested(4), limits).err(), Some(XmlError::TooDeep { limit: 3 }));
    assert_eq!(parse(&nested(100_000), Limits::new(1_000_000)).err(), Some(XmlError::TooDeep { limit: MAX_DEPTH }));
    assert!(parse(&nested(MAX_DEPTH), Limits::new(1_000_000)).is_ok());
}

/// What the scan lets through, roxmltree judges: malformed text and the node limit come back as
/// its own errors.
#[test]
fn the_parser_still_judges_well_formedness_and_node_count() {
    assert_eq!(parse("<a>", ROOMY).err(), Some(XmlError::Malformed(roxmltree::Error::UnclosedRootNode)));
    assert!(matches!(parse("<p><dc:title/></p>", ROOMY), Err(XmlError::Malformed(roxmltree::Error::UnknownNamespace(p, _))) if p == "dc"));
    let nodes = parse("<r><a/><b/><c/></r>", Limits::new(3)).err();
    assert_eq!(nodes, Some(XmlError::Malformed(roxmltree::Error::NodesLimitReached)));
    assert!(nodes.is_some_and(|e| e.is_limit()));
    assert!(!XmlError::Doctype.is_limit());
}

/// The messages callers pass on name what was wrong.
#[test]
fn errors_say_what_was_refused() {
    assert!(XmlError::TooDeep { limit: 64 }.to_string().contains("64 deep"));
    assert!(XmlError::Doctype.to_string().contains("DTD"));
    assert!(XmlError::TooComplex { limit: 1 }.to_string().contains("attributes"));
}

// ── spans for splicing edits (`span`) ─────────────────────────────────────────────────────────

use crate::span;

#[test]
fn start_tags_end_past_quoted_values_and_names_read_as_written() {
    let text = r#"<r xmlns:d="u"><d:t a="x>y" b='/>'>v</d:t><e/></r>"#;
    let doc = parse(text, ROOMY).unwrap();
    let t = doc.root_element().first_element_child().unwrap();
    let range = t.range();
    assert_eq!(span::start_tag_end(text, &range).map(|i| &text[..=i]), Some(r#"<r xmlns:d="u"><d:t a="x>y" b='/>'>"#));
    assert_eq!(span::qname(text, &range), Some("d:t"));
    assert_eq!(span::content(text, &range).map(|c| &text[c]), Some("v"));
    let e = t.next_sibling_element().unwrap();
    assert_eq!(span::content(text, &e.range()), None, "an empty-element tag has no content");
}

#[test]
fn content_ends_at_the_end_tag_not_at_markup_inside() {
    let text = "<a>x<!-- </a> --><![CDATA[</a>]]><?p </a>?>y</a >";
    let doc = parse(text, ROOMY).unwrap();
    let range = doc.root_element().range();
    assert_eq!(span::content(text, &range).map(|c| &text[c]), Some("x<!-- </a> --><![CDATA[</a>]]><?p </a>?>y"));
    let empty = "<a></a>";
    let doc = parse(empty, ROOMY).unwrap();
    assert_eq!(span::content(empty, &doc.root_element().range()), Some(3..3));
}

/// roxmltree's `range_value` keeps the `=` and its spaces in a u8, so past 254 spaces its start is
/// wrong; `span::attribute_value` finds the quote instead.
#[test]
fn attribute_values_are_found_past_range_values_limits() {
    let text = format!("<a n{}={}'v\"&gt;' m=\"\"/>", " ".repeat(300), " ".repeat(300));
    let doc = parse(&text, ROOMY).unwrap();
    let mut attrs = doc.root_element().attributes();
    let n = attrs.next().unwrap();
    assert_ne!(text.get(n.range_value()), Some("v\"&gt;"), "roxmltree's own range is off here");
    assert_eq!(span::attribute_value(&text, &n.range()).map(|(r, q)| (&text[r], q)), Some(("v\"&gt;", '\'')));
    let m = attrs.next().unwrap();
    assert_eq!(span::attribute_value(&text, &m.range()).map(|(r, q)| (r.is_empty(), q)), Some((true, '"')));
}

#[test]
fn spans_outside_the_text_are_none() {
    #[allow(clippy::reversed_empty_ranges)] // a backwards range is one of the inputs under test
    let backwards = 5..2;
    for range in [4..100, backwards, 9..12] {
        assert_eq!(span::start_tag_end("<a/>", &range), None, "{range:?}");
        assert_eq!(span::qname("<a/>", &range), None, "{range:?}");
        assert_eq!(span::content("<a/>", &range), None, "{range:?}");
        assert_eq!(span::attribute_value("<a/>", &range), None, "{range:?}");
    }
}
