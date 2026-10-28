# PR: Page-Tree Walks in annot, forms and organize Skip Repeated Children (#3608)

## Summary
- **Issue Reference:** Resolves #3608
- **Crates:** `crates/annot`, `crates/forms`, `crates/organize`
- **Scope:** Page-tree traversal functions:
  - `page_refs` in `crates/annot/src/lib.rs`
  - `page_refs` in `crates/forms/src/lib.rs`
  - `walk` in `crates/organize/src/lib.rs`

## Root Cause
When walking the PDF page tree, each walker previously pushed every reference listed in a node's `/Kids` array onto an exploration stack before checking whether the child had already been visited in `seen`.
In PDFs where `/Kids` contains repeated entries pointing to the same child or multiple nodes share a `/Kids` array:
- All duplicated references were pushed onto the stack simultaneously.
- In `organize::walk`, ancestor attributes (`INHERITABLE` dictionary, including `/Resources`) were cloned for every queued child.
This resulted in unnecessary memory bloat and allocation overhead.

## Fix
- Filtered child references against `!seen.contains(...)` prior to pushing onto the traversal stack in `annot::page_refs`, `forms::page_refs`, and `organize::walk`.
- Avoided redundant memory allocation and duplicate dictionary cloning for repeated children in malformed or cyclic page trees.
