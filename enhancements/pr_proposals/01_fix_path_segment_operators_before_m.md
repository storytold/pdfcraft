# Pull Request: fix(edit): guard path segment operators before m in image scan (#3581)

## Summary
In `crates/edit/src/images.rs`, scanning page content streams with path operators (`l`, `c`, `y`, `h`) appearing before any `m` operator called Kurbo's `BezPath::line_to`, `curve_to`, or `close_path` on an empty path. Kurbo panics by contract when `line_to` or `curve_to` is invoked on an empty `BezPath` (no initial `move_to`).

This defect caused application panics in `object_list`, `page_images`, and Office export whenever a malformed or scanned PDF with untidy vector streams was opened.

Per ISO 32000-2 §8.5.2, conforming PDF readers either begin a new subpath at the operand coordinates (equivalent to `m`) or ignore operators before the first `m`.

## Changes
- In `crates/edit/src/images.rs`:
  - `b"l"`: If `self.path.elements().is_empty()`, begin subpath with `self.path.move_to(self.point(x, y))` instead of panicking on `line_to`.
  - `b"c"`: If `self.path.elements().is_empty()`, begin subpath with `self.path.move_to(self.point(x1, y1))` before executing `curve_to`.
  - `b"y"`: If `self.path.elements().is_empty()`, begin subpath with `self.path.move_to(self.point(x1, y1))` before executing `curve_to`.
  - `b"h"`: Guard with `if !self.path.elements().is_empty()`.
- In `crates/edit/src/tests.rs`:
  - Added synthetic regression test `path_segment_operators_before_m_do_not_panic_in_image_scan` validating that a stream starting with `l`, `c`, `y`, `h` before `m` completes without panicking.

## Compliance
- Follows `AGENTS.md` §4: Never Crash policy.
- Zero unsafe code.
- Zero panicking paths.
