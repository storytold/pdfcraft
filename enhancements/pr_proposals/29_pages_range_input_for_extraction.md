# PR Proposal 29: Pages Range Input for Extraction Dialog

**Issue Reference:** #3593
**Crates Affected:** `crates/ui-egui`

## Summary
The page extraction modal previously only allowed extracting pre-selected thumbnails from the view or the currently viewed page. Users frequently need to extract arbitrary page ranges (such as `1-5, 8, 11-13`) without having to manually select them in thumbnail grids.

## Implementation Details
- In `crates/ui-egui/src/files.rs`:
  - Added `pages_input: String` to `ExtractDraft`.
  - In `extract_selection`, parsed the user's input with `pdfcraft_print::select_pages` when non-empty, defaulting to `target_pages()` when blank.
- In `crates/ui-egui/src/dialogs.rs`:
  - Added a dedicated "Pages" text edit field with placeholder hint `"e.g. 1-5, 8, 11-13"`.
