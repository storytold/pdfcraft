# PR Proposal 30: Spaces in Non-Embedded Japanese CID Fonts

**Issue Reference:** #3601
**Crates Affected:** `vendor/hayro-interpret`

## Summary
When non-embedded Japanese CID fonts (such as `MS-Mincho`) map space characters (such as `U+2002` EN SPACE or `U+3000` IDEOGRAPHIC SPACE) and the substitute OpenType font lacks that specific code point, the renderer emitted `.notdef` boxes, drawing rows of visible boxes instead of blank spaces.

## Implementation Details
- In `vendor/hayro-interpret/src/font/cid.rs`:
  - In `Type0Font::map_via_unicode`, added a fallback in the OpenType mapping branch: if an absent spacing character (`U+00A0`, `U+2000..=U+200A`, `U+202F`, `U+205F`, `U+3000`) has no glyph in the substitute face, fallback to mapping ordinary space `' '` (`U+0020`).
  - Added similar fallback for CFF substitute fonts looking up glyph name `"space"`.
