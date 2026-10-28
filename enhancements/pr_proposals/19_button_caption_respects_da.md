# PR: M6: Push Button Caption Uses Font, Size and Color of its /DA (#3543)

## Summary
- **Issue Reference:** Resolves #3543
- **Crate:** `crates/forms`
- **Scope:** Push button appearance rendering in `button_appearance` (`crates/forms/src/author.rs`).

## Root Cause
Previously, `button_appearance` ignored the field/widget's `/DA` default appearance string. It hardcoded:
- Font: `/Helv` (Helvetica regular)
- Color: `0 g` (black)
- Size: `((height - 4.0) * 0.6).clamp(4.0, 14.0)`
On forms such as DA Form 31 where buttons request custom font weights (e.g. `/HeBo` Helvetica-Bold), specific font sizes (e.g. `9 Tf`), or custom theme colors, button captions were drawn small, thin, and in plain black.

## Fix
- Parsed `/DA` via `crate::appearance::parse_da`.
- Extracted font resource name (`da.font`), requested size (`da.size`), and color operator (`da.color`).
- When `da.size > 0.0`, the explicit size is honored; otherwise, it falls back to the proportional clamp.
- Mapped font resource names (including `/HeBo`, `/HeOb`, `/TiRo`, `/TiBo`, `/Cour`) to corresponding Standard 14 BaseFonts in the appearance stream's `/Resources /Font`.
- Preserved caption text and icon positioning.
