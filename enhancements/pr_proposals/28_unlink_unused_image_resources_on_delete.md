# PR Proposal 28: Unlink Unused Image Resources on Delete / Replace

**Issue Reference:** #3523, #3511
**Crates Affected:** `crates/edit`

## Summary
When editing a PDF, deleting or replacing an image removed the `Do` operator from the page content stream but left the old image alias in the page's `/Resources /XObject` dictionary. Consequently, full-rewrite / Save As retained the unreachable image data, preventing file size reduction and leaving deleted private photos in the document.

## Implementation Details
- In `crates/edit/src/images.rs`:
  - When `ImageChange::Delete` or `ImageChange::Replace` is applied, checked whether the old image's resource name is still referenced by any other `Do` operator on the page.
  - If unused elsewhere on the page, unlinked the old image alias from `/Resources /XObject`.
  - Added regression test `deleting_an_image_unlinks_it_from_xobject_resources` in `crates/edit/src/tests.rs`.
