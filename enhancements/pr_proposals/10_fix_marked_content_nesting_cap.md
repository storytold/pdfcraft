# PR: Fix Marked-Content Nesting Cap Dropping / Mis-scoping /ActualText (#3573)

## Summary
- **Issue Reference:** Resolves #3573
- **Crate:** `crates/edit`
- **Scope:** Marked content stack tracking in `interpret` (`crates/edit/src/text.rs`).

## Root Cause
When extracting text lines and regions (used by text editing, Office export, and redaction), the PDF content stream interpreter bounded nested marked-content sequences (`BMC` / `BDC`) to at most 256 levels (`if marked.len() < 256`).
However, when nesting exceeded 256 levels, `BMC` / `BDC` was skipped while `EMC` continued to pop from the `marked` and `actual` stacks unconditionally. Consequently, from depth 256 onwards, each inner `EMC` closed its *parent* or *enclosing* sequence instead of its own, causing `/ActualText` to be prematurely popped, dropped, or attributed to the wrong text runs.

## Fix
- Added `excess_marked: usize` to `Carry` and `interpret`.
- When depth reaches 256, excess marked content entries increment `excess_marked` using `.saturating_add(1)` instead of being ignored.
- When `EMC` is encountered, if `excess_marked > 0`, it decrements `excess_marked -= 1`. Only when `excess_marked == 0` does it pop from `marked` and `actual`.
- Preserved `excess_marked` in `Carry` so state transitions across multiple concatenated content streams remain symmetric.
- Added regression test `marked_content_nesting_past_256_preserves_actual_text` in `crates/edit/src/tests.rs`.
