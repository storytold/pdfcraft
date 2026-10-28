# PR Proposal 32: Installed Hangul (Korean) System Font Fallback

**Issue Reference:** #3505, #3517
**Crates Affected:** `crates/ui-egui`

## Summary
Korean characters in tab titles, bookmarks, and recent file lists displayed as replacement tofu boxes because no embedded UI font carries Hangul and no system fallback was wired for Korean text.

## Implementation Details
- In `crates/ui-egui/src/system_fonts.rs`:
  - Added `HANGUL_PROBE` (`\u{AC00}`).
  - Added `hangul_fallback()` and `hangul_candidates()`, looking up non-Adobe system fonts: Malgun Gothic, Gulim, Batang on Windows; Apple SD Gothic Neo on macOS; Nanum Gothic, UnDotum on Linux.
- In `crates/ui-egui/src/theme.rs`:
  - Registered `SYSTEM_HANGUL_FALLBACK` in `installed_font_definitions` across all proportional and monospace font stacks.
