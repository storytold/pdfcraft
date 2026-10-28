# PR Proposal 27: Doubled-Sign Numbers in Hayro Content Stream Lexer

**Issue Reference:** #3630, #3565
**Crates Affected:** `vendor/hayro-syntax`

## Summary
A number written with a doubled sign (such as `--280` or `+-280`) was read by hayro's content stream lexer as two numbers: `0` and `-280`. When passed to transform operators such as `cm`, the operator received seven operands, discarded the first, and constructed a degenerate transform matrix (`0 0 1 0 -280 10`) that blanked the entire page.

## Implementation Details
- In `vendor/hayro-syntax/src/object/number.rs`:
  - Updated `read_inner` to absorb up to two leading signs (`--`, `+-`, `-+`, `++`), setting `negative = true` if either sign is `-`.
  - Allowed trailing digits to parse as a single unified numeric token matching `crates/cos`'s lexer (#3299) and MuPDF.
