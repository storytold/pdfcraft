# PR: Step Over Malformed Operators Instead of Aborting Content Stream (#3568, #3636)

## Summary
- **Issue Reference:** Resolves #3568, #3636
- **Crate:** `vendor/hayro-syntax`, `crates/content`
- **Scope:** `TypedIter::next` in `vendor/hayro-syntax/src/content/mod.rs`.

## Root Cause
In `TypedIter::next`, when an invalid operator occurred (such as `20 100 100 re` where `re` has only 3 operands instead of 4), `TypedInstruction::dispatch(&op)` returned `None`.
For path operators, the function previously aborted by returning `None`, which completely terminated the content stream iterator. Consequently:
- Rendering stopped, leaving the remainder of the page blank.
- Text extraction dropped all text following the bad operator.
- Search and search-based redaction failed to find words appearing later on the page.

## Fix
- Replaced the early termination with a continue loop in `TypedIter::next`.
- Skips malformed operators so that parsing seamlessly continues to subsequent tokens, aligning behavior with MuPDF, pdftotext, and PDFium.
- Added regression test `malformed_operators_are_skipped_without_ending_stream` in `crates/content/src/tests.rs`.
