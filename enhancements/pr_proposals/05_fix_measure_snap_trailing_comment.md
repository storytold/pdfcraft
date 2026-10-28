# Pull Request: fix(measure): join /Contents pieces with space instead of newline in snap (#3570)

## Summary
In `crates/measure/src/snap.rs`, when snapping to geometry on pages whose `/Contents` is an array of streams, pieces were joined with a newline (`b'\n'`). When a piece ended with a comment (`% ...` without a newline), joining with a newline terminated the comment early, exposing geometry (such as rectangles) on the next line that were never drawn by the renderer (which joins pieces with a space, per ISO 32000-2 Table 31).

This caused `measure_snap` to offer snapping points for invisible, unrendered shapes.

## Changes
- `crates/measure/src/snap.rs`: Changed `joined.push(b'\n')` to `joined.push(b' ')`, ensuring comment continuations are evaluated identically to the renderer.
- `crates/measure/src/tests.rs`: Added regression test `trailing_comment_in_contents_piece_does_not_unmask_geometry_in_snap`.
