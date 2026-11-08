# Colour-preserving dark reading

Preferences → Reading colours → **Colour-preserving dark** selects an optional page-display
curve. It keeps hues in solid text and line art while changing white paper to dark grey and
black ink to off-white. The application theme is independent of this setting.

The same mode is available through the existing automation tool:

```json
{"tool":"view_reading_colors","args":{"doc":1,"mode":"color-preserving-dark"}}
```

Selecting another reading-colour mode replaces this one; the curves never stack. The setting
is saved with the existing app-wide reading preference and seeded into every document as it
opens. Older preferences without `preserve_hues` keep the existing luminance interpolation.

## Curve and provenance

The Rust implementation was written independently from the mathematical reference developed
in Sebyoshi and published in [the discussion of #3281](https://github.com/storytold/pdfcraft/pull/3281#issuecomment-6725501625).
It needs no Apple framework or additional dependency.

Input is straight, gamma-encoded sRGB. Convert it to HSL, neutralise low-chroma paper with
`S1 = S × smoothstep(0.12, 0.25, max(R,G,B) − min(R,G,B))`, then invert lightness with a
hue-dependent exponent. For the fully saturated hue at lightness 0.5, compute the weighted
gamma-RGB brightness `Yh = 0.2126R + 0.7152G + 0.0722B`. Let
`q = 0.25 + 0.75 × min(1, Yh / 0.6)`, `e = 0.96 × (1 − S1 × (1 − q))`, and
`L1 = (1 − L)^e`. Convert `(H,S1,L1)` back to RGB and interpolate each channel from paper
to ink. Alpha is copied unchanged. The gamma-RGB weighting is not WCAG luminance.

The preset stores paper `[31,31,31]` and ink `[230,230,230]` in PdfCraft's existing 8-bit
palette format. Sebyoshi's reference uses exact normalized endpoints 0.12 and 0.90. The
reference-colour tests allow one channel level for that endpoint quantization; this is a
direct paint transform, without Sebyoshi's display-LUT interpolation.

The renderer's existing rules still exclude pictures, shadings, tiling patterns and annotation
appearances from colour replacement. A pending page uses the same paper colour as its raster.
Print previews, export and the document's saved bytes retain their original colours.

## Limits

Preserving a bright yellow background can leave light ink with poor contrast. In the Sebyoshi
reference, input `[240,224,48]` becomes `[198,185,43]`, with about 1.62:1 contrast against mapped
black text. This mode has no general contrast guarantee; the existing high-contrast presets
remain available. Image-only scans and raster graphic/photo classification are separate work.

## Verification

- `cargo test -p pdfcraft-render color_preserving_dark` checks reference colours, alpha and paint
  flags, actual page rendering, tile agreement, preserved image and annotation paints, and printing.
- `cargo test -p pdfcraft-automation color_preserving_dark` checks the tool schema, mode replacement,
  rendered coloured text and original image pixels, toggling off, and unchanged source bytes.
- `cargo test -p pdfcraft-ui-egui color_preserving_dark` checks selection through the real
  preferences UI, application-theme independence, persistence, document seeding and old settings.
- `cargo test -p pdfcraft-engine image_exports_ignore_reading_colors_and_keep_unsaved_edits`
  checks original-colour PNG, JPEG and TIFF output for every reading preset and custom
  palettes, including parallel export and unsaved edits.
