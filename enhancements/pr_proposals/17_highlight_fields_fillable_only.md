# PR: Forms: Highlight Fields Marks Only Fields That Can Be Filled (#3584)

## Summary
- **Issue Reference:** Resolves #3584
- **Crate:** `crates/ui-egui`
- **Scope:** Canvas form field highlight painting loop in `crates/ui-egui/src/canvas.rs`.

## Root Cause
When the "Highlight existing fields" feature was active (`view.highlight_fields`), the canvas highlighted every visible widget on the page, including read-only fields (`/Ff` bit 1 `READ_ONLY`). This mislead users into clicking locked or read-only fields that could not be edited or filled.

## Fix
- Updated the field highlight loop in `crates/ui-egui/src/canvas.rs` to inspect `f.has(pdfcraft_engine::field_flags::READ_ONLY)` and skip tinting read-only fields.
- Fillable fields retain their soft-blue background tint and red borders for required fields.
- When form scripts unlock a field dynamically (`access = "open"`), the field naturally gains highlighting on the next frame.
