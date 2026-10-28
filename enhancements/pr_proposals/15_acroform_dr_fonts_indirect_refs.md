# PR: Make AcroForm Default Resources Font Entries Indirect References for Adobe Reader (#3441)

## Summary
- **Issue Reference:** Resolves #3441
- **Crate:** `crates/forms`
- **Scope:** `ensure_form` in `crates/forms/src/author.rs`.

## Root Cause
When adding form fields (text fields, date fields, etc.), `ensure_form` provisions default fonts (`/Helv`, `/TiRo`, `/Cour`) in `/AcroForm /DR /Font`.
Previously, these fonts were added as direct inline dictionaries (`fonts.set(name, Object::Dict(f))`), and `/DR` was replaced with an inline dictionary (`af.set(b"DR", Object::Dict(dr))`).
When such a PDF is opened in Adobe Acrobat / Reader and the user edits a text field, Acrobat uses an OS text widget while the field is active, but regenerates the field's appearance stream (`/AP /N`) when the user moves focus away. Adobe Reader's font loader strictly requires font entries in `/DR /Font` to be indirect object references (`Object::Ref(font_ref)`).
Because `/DR /Font /Helv` was a direct dictionary, Adobe Reader failed to resolve the font during appearance generation; it painted the `/MK /BG` background rectangle but skipped the text run, causing the entered text to disappear under a solid background fill.

## Fix
- In `ensure_form`, each default font is registered in the document as an indirect object (`doc.add(Object::Dict(f))`) and stored in `/DR /Font` as `Object::Ref(font_ref)`.
- If `/DR` already exists as an indirect reference in `/AcroForm`, it is updated in-place via `doc.update_dict(dr_r, ...)` rather than overwritten with an inline dictionary.
- If `/DR` does not exist, it is allocated as an indirect object and assigned as `Object::Ref(dr_r)`.
- Added unit test `default_form_fonts_are_indirect_references_for_adobe_reader` in `crates/forms/src/tests.rs`.
