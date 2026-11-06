# pdfcraft-xml

Bounded XML parsing (L0, standalone). Every PdfCraft feature that reads XML (XFA forms, XFDF and
XML form data, XMP metadata) reads it from a file someone else made, so the work a parse may cost
is checked before the parser starts.

```rust
let doc = pdfcraft_xml::parse(text, pdfcraft_xml::Limits::new(1_000_000))?;
```

`parse` is the one way into [`roxmltree`](https://github.com/RazrFalcon/roxmltree) in the
workspace. It refuses, before parsing:

- a DTD (`<!DOCTYPE`), which nothing in PdfCraft reads;
- nesting deeper than `Limits::max_depth` (1,024 by default), which bounds the callers' own
  recursive walks of the tree;
- more than `Limits::max_work` of the parser's superlinear work. roxmltree 0.21.2 checks for
  duplicate attributes and namespace declarations, and copies inherited namespace bindings, in
  loops quadratic in the input, comparing strings whose length the input also chooses. Measured
  without this check, 60,000 attributes on one element took 9.8 s, and a 436 KB file of nested
  namespace declarations took 156 s. `src/scan.rs` documents the cost model loop by loop, with
  comparisons charged by length. The default budget is at most about 0.4 s of parser time where
  it was measured.

roxmltree then judges well-formedness and enforces the node limit each caller picks.

The work budget works around the parser's current loops. When roxmltree removes them, relax the
budget (the model in `src/scan.rs` names each loop it charges for).

`span` finds byte ranges in the parsed text, for edits that splice it instead of re-serialising
it: where an element's start tag ends, its name as written, its content, and an attribute's value.
The value comes from `Attribute::range`, because roxmltree's `range_value` is wrong past a
65,535-byte name or 254 spaces around `=`. The functions take ranges the parser reported, so they
only ever look at text roxmltree has already accepted.
