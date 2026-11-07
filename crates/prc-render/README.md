# pdfcraft-prc-render

A headless CPU renderer for PRC 3D models read by `pdfcraft-prc`: placed meshes in world space, a fitted camera, a
depth-buffered triangle fill and headlight shading, into a straight-RGBA image.

- **Layer:** L3. Depends on `pdfcraft-prc` only.
- **Licence:** MIT OR Apache-2.0.
- `#![forbid(unsafe_code)]`. Never panics on input: image size, triangle count and every index are checked, and
  damage returns `RenderError`.

## API

```rust
assemble(&PrcFile) -> Result<Vec<WorldMesh>, RenderError>   // every placement's mesh moved into the world and coloured
place(&TriangleMesh, &Matrix, colour, triangle_colours) -> WorldMesh
Camera::fit(&Bounds, direction, up, perspective, aspect) / Camera::fit_meshes(&[WorldMesh], ...)
Camera::from_view(c2w, center_of_orbit, ViewLens, annotation_size, fallback) // a PDF 3D view's camera (§13.6.4)
render(&[WorldMesh], &Camera, &RenderOptions) -> Result<Image, RenderError>
```

- With no product tree, each tessellated mesh is drawn where it lies, in the default colour.
- A triangle's own colour wins over its mesh's, which wins over `RenderOptions::colour`.
- Opaque triangles are drawn first, then translucent ones blended over them; alpha 0 is not drawn.
- `Lighting::Headlight` (default) shades by the angle to the view direction; `Lighting::Unlit` draws flat colour.
- Perspective triangles are clipped at a near plane 1/10,000 of the eye-to-target distance from the eye; orthographic views are not clipped.
- `MAX_RENDER_PIXELS` (2^24, about 16 MP) bounds the image: at most 64 MB of RGBA plus 128 MB of depth.
  `MAX_ASSEMBLED_TRIANGLES` and `MAX_ASSEMBLED_VERTICES` (2^24 each) bound one `assemble` call. `MAX_RENDER_WORK` (2^31) bounds
  the pixel centres one `render` tests, summed over each triangle's clipped bounding box; past it the render returns
  `RenderError::TooMuchWork`. That is still seconds of CPU at the top, so callers on a UI thread should render small or off-thread.
- Not yet: shadows, ambient occlusion, specular light, scene lights, textures, wires and markups.
