# PR: Save Commits Open Edit Text Boxes and Line Editors Before Writing File (#3546, #3496)

## Summary
- **Issue Reference:** Resolves #3546, #3496
- **Crate:** `crates/ui-egui`
- **Scope:** Save workflow in `commit_typing_in` (`crates/ui-egui/src/editing.rs`).

## Root Cause
When saving via `save_view` / `save_then`, the app invokes `commit_typing_in` to flush in-progress input. Previously, `commit_typing_in` checked form fields (`v.forms.focus`) and Fill & Sign type boxes (`v.fill_text`), but omitted:
1. Newly added text boxes (`v.content.draft`), which only committed on canvas click-away.
2. In-progress paragraph edits (`v.line_editor`).
If a user edited text and pressed Save (Ctrl+S) while focus remained inside the box, the save wrote the pre-edit file to disk without committing the pending text, causing user edits to be lost upon closing or reloading.

## Fix
- Extended `commit_typing_in` to recognize pending edits in `v.content.draft` and `v.line_editor`.
- Added helper `commit_open_content_draft` to finalize draft text boxes via `ContentView::finish_draft` before bytes are serialized.
- Added helper `commit_open_line_editor` to finalize active paragraph edits via `edit_text_ui::finished_edit`.
- Ensured background tabs with active text drafts or paragraph edits are brought forward and committed prior to saving.
