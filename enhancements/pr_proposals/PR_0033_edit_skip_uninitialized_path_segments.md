# PR Proposal 0033: Guard Image Scan Against Uninitialized Subpaths in Kurbo

- **PR ID**: `PR-0033`
- **Issue Reference**: Fixes #3581, #3621
- **Severity**: High (Crash Prevention / Stability)
- **Component**: `crates/edit/src/images.rs`, `crates/edit/src/tests.rs`

## Description
A content stream that uses a path segment operator (`l`, `c`, `v`, `y`, `h`) before any `m` (MoveTo) or `re` (Rectangle) has no active subpath to extend. In `crates/edit/src/images.rs`, these operators were passed directly to `kurbo`'s `BezPath::line_to`/`curve_to`/`close_path`, which panics with:
```
thread 'main' panicked at kurbo\src\bezpath.rs:242:
uninitialized subpath (missing MoveTo)
```
Consequently, operations such as `object_list`, `page_images`, `image_hit`, and document export would crash on such documents.

## Resolution
- Guarded `l`, `c`, `y`, and `h` in `ImageState::apply` to verify that `!self.path.elements().is_empty()` before calling curve/line operators.
- Added comprehensive regression tests in `crates/edit/src/tests.rs` verifying that invalid leading path segments are safely ignored while subsequent valid image operators are discovered.
