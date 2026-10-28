# PR Proposal 31: macOS Devanagari Font Fallback Paths

**Issue Reference:** #3598, #3597
**Crates Affected:** `crates/ui-egui`

## Summary
`system_fonts.rs` had typographical errors in its macOS Devanagari candidate list: `DevanagariSangamMN.ttc` was missing spaces (Apple's real file is `Devanagari Sangam MN.ttc`), and `Supplemental/Kohinoor.ttc` did not exist (it resides at `/System/Library/Fonts/Kohinoor.ttc`). When `DevanagariMT.ttc` was absent, Hindi text failed to resolve system fonts.

## Implementation Details
- In `crates/ui-egui/src/system_fonts.rs`:
  - Corrected `"Devanagari Sangam MN.ttc"`.
  - Added lookups in both `/System/Library/Fonts/` and `/System/Library/Fonts/Supplemental/` for `DevanagariMT.ttc`, `Devanagari Sangam MN.ttc`, and `Kohinoor.ttc`.
