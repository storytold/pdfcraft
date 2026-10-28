# PR: Prevent Integer Overflow When Rotating Added Images Through Automation (#3187)

## Summary
- **Issue Reference:** Resolves #3187
- **Crate:** `crates/automation`
- **Scope:** `content_update` argument handling in `crates/automation/src/content.rs`.

## Root Cause
When calling `content_update` on an added image with a rotation argument, the rotation calculation previously added the degrees directly into an intermediate `i64`:
```rust
i.rotation = ((i64::from(i.rotation) * 90 + deg).rem_euclid(360) / 90) as u8;
```
If an image was already rotated (e.g. `i.rotation = 1` for 90°) and `deg` was a very large positive `i64` multiple of 360 (such as `9223372036854775800`), adding `i64::from(i.rotation) * 90` caused an `attempt to add with overflow` panic in debug builds, crashing the CLI tool and returning internal errors through the MCP server.

## Fix
- Normalize the requested angle to quarter turns before adding it to the current orientation:
```rust
let turns = (deg.rem_euclid(360) / 90) as u8;
i.rotation = (i.rotation + turns) % 4;
```
- Completely eliminates integer overflow on arbitrary `i64` rotation inputs.
- Added regression test in `crates/automation/tests/automation.rs`.
