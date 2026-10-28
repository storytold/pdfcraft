# PR Proposal 0036: Deduplicate Repeated Kids and Protect Page-Tree Walks

- **PR ID**: `PR-0036`
- **Issue Reference**: Fixes #1798, #1926, #3608
- **Severity**: High (DoS / Memory Protection)
- **Component**: `crates/annot/src/lib.rs`, `crates/forms/src/lib.rs`, `crates/organize/src/lib.rs`, `crates/organize/src/tests.rs`

## Description
In `crates/annot/src/lib.rs`, `crates/forms/src/lib.rs`, and `crates/organize/src/lib.rs`, the page-tree walk loops pushed every `/Kids` reference onto the stack before the `seen` check. When a document's `/Kids` array contained repeated entries (e.g. 10,000+ duplicate child references in malicious or malformed PDFs), the stack allocated and held every single entry simultaneously, causing massive memory bloat and potential OOM crashes.

## Resolution
- In all three page-tree walkers, deduplicate child references within each `/Kids` node before pushing to the stack.
- Retained the 1,000,000 total node traversal safeguard.
- Added regression test `page_walk_skips_repeated_and_cyclic_kids_in_organize` in `crates/organize/src/tests.rs`.
