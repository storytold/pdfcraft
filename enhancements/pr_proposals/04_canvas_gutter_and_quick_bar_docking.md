# Enhancement Proposal: Dynamic Canvas Gutter & Quick Tool Docking

## Context & User Problem
In `crates/ui-egui/src/canvas.rs`, a static constant `const SIDE: f32 = 70.0;` is defined:
```rust
/// Horizontal gutter that keeps pages clear of the floating quick-action bar.
const SIDE: f32 = 70.0;
```
This constant is unconditionally added to left and right canvas offsets:
1. When the Left Tool Drawer is open ("All tools", width 240–280px), the 70px gutter pushes the document an additional 70px to the right, creating a jarring, unutilized vertical empty band.
2. The quick-action bar hovers at a static floating coordinate (`pos = area.left_top() + vec2(14.0, 14.0)`). When zooming in (e.g. 136%), it floats directly on top of the document page margins and content.
3. The visual design is preserved, but the spacing and responsiveness need to be made functional and smart.

## Proposed Solution (Preserving Full Visual Design)
1. **Dynamic Gutter Calculation:**
   Replace static `SIDE = 70.0` with dynamic calculation:
   ```rust
   fn canvas_side_gutter(app: &PdfCraftApp) -> f32 {
       if app.left_open {
           // When the left tools panel is already open, reduce canvas dead space
           20.0
       } else {
           // When standalone, provide the comfortable 70px breathing room
           70.0
       }
   }
   ```
2. **Context-Aware Quick Bar Auto-Stow / Dock:**
   When the user enters a dedicated workspace mode (e.g. "Edit a PDF" or "Organize pages"), the floating quick-bar can gracefully dock into the mode bar or hide its redundant tools to prevent overlapping user artwork.
