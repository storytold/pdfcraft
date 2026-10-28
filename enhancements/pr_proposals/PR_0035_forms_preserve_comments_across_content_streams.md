# PR Proposal 0035: Preserve Comments Across Content Stream Boundaries in Form Detection

- **PR ID**: `PR-0035`
- **Issue Reference**: Fixes #3502, #3625
- **Severity**: Medium (Form Detection Fidelity)
- **Component**: `crates/forms/src/detect.rs`, `crates/forms/src/tests.rs`

## Description
When a page's content is split across multiple content streams in an array, `crates/forms/src/detect.rs` previously concatenated decoded stream pieces by inserting a newline (`b'\n'`) between each stream.
If stream 1 ended with a comment (e.g., `% commented out box`), the synthetic newline terminated the comment prematurely, causing subsequent stream tokens to be interpreted as active operators and detecting phantom form fields.

## Resolution
- Joined raw content streams using `pdfcraft_content::Pieces::join(&raw_pieces).data()`, which separates pieces with a space (`b' '`) rather than a newline.
- Under PDF specification ISO 32000-2, comments are terminated only by `\r` or `\n`, matching the behavior of `hayro-interpret` and standard PDF engines.
- Added regression test in `crates/forms/src/tests.rs` verifying that commented boxes spanning stream boundaries are not detected as active fields.
