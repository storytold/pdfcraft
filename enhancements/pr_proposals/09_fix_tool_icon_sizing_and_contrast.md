# Pull Request: fix(ui): enlarge and tile tool row icons with crisp stroke rendering

## Summary
In `crates/ui-egui/src/panels.rs`, the left "All tools" drawer and tool sub-panels painted tool icons in a constrained `20×20px` rectangle at only `19.0px` (or `17.0px` in subpanels) with no background plate. Coupled with a thin `1.75` stroke width in `crates/ui-egui/src/icons.rs`, tool icons such as "Combine files" and "Add comments" rendered as tiny, spindly, low-contrast line art.

## Changes
- `crates/ui-egui/src/icons.rs`:
  - Updated SVG rasterization style `stroke_width` from `"1.75"` to author-standard `"2.0"` for sharp, high-contrast vector rendering across all displays.
- `crates/ui-egui/src/panels.rs`:
  - Enclosed each tool row icon in a `24×24px` tile with `CornerRadius::same(6)`.
  - Added a subtle tinted backdrop plate using `icon_color.gamma_multiply(0.14)` (matching the tool's Acrobat color-family hue).
  - Adjusted row label start offset from `36.0px` to `39.0px` for balanced visual hierarchy and padding.
  - Applied the same 24px tile alignment to sub-panel tool items.
