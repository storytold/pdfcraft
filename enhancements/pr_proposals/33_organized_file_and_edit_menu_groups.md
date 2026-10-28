# PR Proposal 33: Organized File and Edit Menu Groups

**Issue Reference:** #3524
**Crates Affected:** `crates/ui-egui`

## Summary
The File menu was rendered as a flat list of ~30 items with no grouping or visual separators, making scanning difficult.

## Implementation Details
- In `crates/ui-egui/src/commands.rs`:
  - In `registry_menu`, added visual separators before `create.blank`, `file.save`, `print.dialog`, `export.image`, `optimize.reduce`, and `protect.password` in the File menu.
  - Added separators before `edit.find`, `edit.copy`, `form.clear`, and `app.preferences` in the Edit menu.
