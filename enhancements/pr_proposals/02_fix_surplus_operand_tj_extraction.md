# Pull Request: fix: read the last array of a surplus-operand TJ, not the first (#3569)

## Summary
In PDF content streams, operators consume operands from the top of the evaluation stack (which corresponds to `.last()` in parsed operand vectors). 

When extra or surplus operands were pushed onto the stack before a `TJ` operator (e.g. `[surplus] [(actual) -10 (text)] TJ`), multiple crates (`crates/edit`, `crates/a11y`, `crates/redact`) were extracting operands with `.first()` instead of `.last()`, causing them to extract the stale or invalid stack item instead of the actual `TJ` array. In contrast, other operators in the same matches (such as `Tj`, `'`, `"`) correctly consumed `op.operands.last()`.

This broke text editing, accessibility heading extraction, and redaction tag analysis on PDFs with surplus operands.

## Changes
- `crates/edit/src/text.rs`: Changed `b"TJ" => op.operands.first()` to `op.operands.last()`.
- `crates/a11y/src/headings.rs`: Changed `b"TJ" => op.operands.first()` to `op.operands.last()`.
- `crates/redact/src/interp.rs`: Changed `b"TJ" => op.operands.first()` to `op.operands.last()`.
- `crates/redact/src/tags.rs`: Changed `b"TJ" => op.operands.first()` to `op.operands.last()`.
- `crates/edit/src/tests.rs`: Added synthetic regression test `surplus_operands_before_tj_array_reads_last_operand`.

## Compliance
- Preserves all existing functionality and interfaces.
- Eliminates parser disagreements between PdfCraft, MuPDF, and pdftotext.
