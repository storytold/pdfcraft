# PR Proposal 26: Preserve Content Boundaries When Merging Image Fragments

**Issue Reference:** #3622, #3507
**Crates Affected:** `crates/optimize`

## Summary
When optimizing PDFs with `merge_fragments: true`, `content_bytes` joined `/Contents` pieces with a newline (`\n`). If the preceding content stream piece ended with an unclosed comment (`% ...`), appending a newline terminated the comment early, converting lines in the following piece (such as hidden text) into active drawing instructions.

## Implementation Details
- In `crates/optimize/src/images.rs`:
  - Updated `content_bytes` to join stream pieces with a space (`b' '`) instead of `b'\n'`, aligning with `pdfcraft_content::Pieces::join`, hayro page loading, and MuPDF/pdftotext.
  - Added unit test `content_bytes_joins_streams_with_space_not_newline`.
