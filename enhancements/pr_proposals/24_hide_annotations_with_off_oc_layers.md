# PR Proposal 24: Hide Annotations whose /OC is an OFF Layer

**Issue Reference:** #3624, #3572
**Crates Affected:** `vendor/hayro-interpret`

## Summary
An annotation dictionary can carry its own `/OC` (Optional Content) entry, tying the annotation to an optional content group (OCG) or membership dictionary (OCMD). Previously, the annotation rendering loop in `interpret_page` only checked `/F` flags (Hidden, NoView) and "Hide all comments", completely omitting `/OC` inspection. This caused hidden layer comments to be rendered in the viewer and printed.

## Implementation Details
- In `vendor/hayro-interpret/src/ocg.rs`:
  - Factored out `ocmd_is_active(&self, ocmd: &Dict<'_>) -> bool` to share evaluation logic across marked content and annotations.
  - Implemented `oc_is_visible(&self, props: &Dict<'_>, ref_id: Option<ObjectIdentifier>) -> bool` to query visibility without modifying the content-stream visibility stack.
- In `vendor/hayro-interpret/src/interpret/mod.rs`:
  - Resolved `annot.get::<Dict<'_>>(OC)` and skipped drawing any annotation when its optional content group or membership dictionary evaluates to OFF (`!context.ocg_state.oc_is_visible`).
