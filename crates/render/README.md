# pdfcraft-render

Page rasterization and document inspection. **Bootstrap (ADR-0004):** pages render through the
vendored `hayro` (hayro-interpret + vello_cpu, patches in `vendor/README.md`) and documents are
inspected through the lazy `cos` reader. Both move to `model` and the DisplayList devices in
M1–M2; the public API stays, so the engine and UI don't change with them.

- `RenderPool` / `PageRenderer`: background and single-threaded page rasters (premultiplied
  RGBA8), tiles, previews and text extraction. Every render runs under `catch_unwind`.
- `inspect`: document information, pages, outline, annotations, fields, layers, attachments.
- `text`: page text and search.
- `separations`: Output Preview (below).

## Output Preview: separations, coverage, overprinting

`PageRenderer::page_inks(page)` lists the inks a page prints with and
`PageRenderer::separate(page, scale, overprint)` returns its `Separations`: one plate per ink,
each ink's tint per pixel. On top of that: coverage of every ink and the total at a pixel
(`coverage_at`), the page's highest total area coverage (`max_total`), the pixels above a
limit (`above`), per-ink highest tint and printed area (`stats`), one plate as film (`film`),
and the page as the chosen inks print it (`composite`, optionally with the pixels above a
coverage limit painted in a warning colour).

**How a plate is made.** The page is rendered once per ink with hayro-interpret's plate mode
(`InterpreterSettings::plate`, vendored patch 53). Every paint draws that ink's tint as a gray
film instead of its colour; a paint that overprints without specifying the ink leaves the film
alone. Everything else — clipping, anti-aliasing, soft masks, transparency groups, blending,
patterns, Type 3 glyphs, annotations — is the renderer's own, so a plate is the page as drawn,
one ink at a time. We chose this over interpreting content into coverage ourselves (which
would duplicate the renderer) and over rewriting colour spaces in the PDF (which cannot express
overprinting, a property of the graphics state rather than of the colour).

What a colour puts on a plate follows its colour space (ISO 32000-2 §8.6):

| Colour space | Inks it specifies | Tint on a plate |
|---|---|---|
| DeviceCMYK | the four process inks | the component |
| ICCBased with 4 components | the four process inks | the component; in another profile than the output intent's, converted to its CMYK (below) |
| DeviceGray | the four process inks | black `1 − gray`, the others 0 (§10.4.2.2) |
| CalGray, ICCBased with 1 component | the four process inks | through the output intent (below); without one, black `1 − gray` |
| DeviceRGB, CalRGB, Lab, ICCBased with 3 components | the four process inks | through the output intent (below); without one, through sRGB and §10.4.2.4 |
| Separation | its colorant; `All` every ink; `None` nothing | the tint |
| DeviceN | its named colorants (process names are the process inks) | that component |
| Indexed | its base space's | the palette entry's |

Overprinting follows §8.6.7: with `OP` (strokes) or `op` (everything else; `OP` alone sets
both) a paint leaves the plates of inks its colour space doesn't specify as they are, and with
`OPM 1` a DeviceCMYK component of 0 leaves its plate too. Without overprinting every paint
knocks out (tint 0) the inks it doesn't specify. Soft masks keep their own colours: a mask's
luminosity never comes from a plate. The inks are the four process inks (always listed, as a
press runs them) and then the spot inks the page's paints use, in the order first painted,
found through `InterpreterSettings::ink_sink`; an ink only a soft mask uses is not listed.

**Output intents: colour-managed process colour.** A printing condition is what the process
plates simulate. It comes from the page's own `/OutputIntents` (PDF 2.0), else the document
catalog's (ISO 32000-2 §14.11.5): the first output intent whose `/DestOutputProfile` is a CMYK
ICC profile with PCS-to-device (BToA) tables, a `GTS_PDFX` intent before the others. Process
colour then converts to that profile's CMYK with moxcms (vendored patch 54,
`color::CmykSimulation`), relative colorimetric — the initial rendering intent of §8.6.5.8,
ICC.1's media-relative colorimetric intent, through the profile's colorimetric (tag 1) tables, or
its perceptual (tag 0) ones when it has no others:

- DeviceRGB as sRGB, as pages are drawn; CalRGB and CalGray through their sRGB colours;
- Lab (relative to its white point, Bradford-adapted to D50) and ICCBased colours — RGB, gray,
  L\*a\*b\*, and CMYK in another profile (CMYK to CMYK) — through a D50 RGB hub with ROMM
  (ProPhoto) primaries. Its gamut holds practically every surface colour, so nothing printable
  clips on the way, and paper white and black land on grid points of the conversion. moxcms 0.8
  without its `any_to_any` feature converts CMYK to CMYK and gray to CMYK only through such an
  intermediate; an L\*a\*b\* intermediate was tried first and left about 0.5% of ink on paper white.
- DeviceCMYK and ICCBased CMYK in the intent's own profile (the same bytes, in any stream) are
  that printing condition's CMYK already and stay as they are. DeviceGray is a device colour,
  which a CMYK device prints with its black ink alone (§10.4.2.2), so it stays on black.

The composite and the process swatches show the inks through the same profile's device-to-PCS
tables: a soft proof of that printing condition. Without a usable CMYK output intent the device
formulas above apply. `Separations::simulation` records which was used, and why the formulas
were; the panel shows "Simulation: <profile description>" or "Simulation: device formulas", and
`print_output_preview` returns it as `simulation`.

**What is approximated** (each a known difference from Acrobat's Output Preview):

- Only the document's output intent is simulated: there is no Simulation Profile menu to choose
  another. Without a usable CMYK output intent (none, an RGB or gray one, a profile without BToA
  tables, an external `DestOutputProfileRef`, or a registered condition with no profile embedded)
  RGB, CIE-based and ICC colours become CMYK by the ISO device formulas with black generation and
  undercolour removal as the identity, so RGB artwork separates into heavier black and lighter
  CMY than a colour-managed conversion would, and ICCBased CMYK is taken as it is.
- Through an output intent, every conversion is relative colorimetric: the graphics state's
  rendering intent (`ri`, `/RI`, an image's `/Intent`) isn't read, as hayro doesn't read it, and
  black point compensation isn't applied (moxcms 0.8 has none). CalRGB and CalGray clip to sRGB
  on the way. `DefaultGray`, `DefaultRGB` and `DefaultCMYK` aren't applied (hayro ignores them),
  so DeviceGray stays on black and DeviceRGB is read as sRGB. CMYK to CMYK is colorimetric: black
  alone in another profile becomes a four-ink black (no black preservation). A colour whose own
  ICC profile fails to convert falls back to the device formulas.
- The composite shows the process plates through the output intent's profile, or without one
  through the CMYK profile pages are drawn with (a CGATS-compatible one), and multiplies each
  spot ink's own appearance (its alternate colour at that tint) over them. Real inks don't mix
  exactly multiplicatively. Paper colour and black ink simulation are not offered.
- `OPM 1` applies to colours set in DeviceCMYK only, not to ICCBased CMYK, images or shadings.
- Transfer functions, `BG`/`UCR` in the graphics state and halftones are not applied to plates.
- Plates are blended as gray films, which is ISO's complemented blending for separable blend
  modes in a subtractive space; the non-separable modes (Hue, Saturation, Color, Luminosity) and
  transparency groups that blend in RGB are approximated.
- A spot ink's appearance comes from the first colour space that names it.
- At most `MAX_SPOT_INKS` (32) spot inks are separated per page; more are listed, not separated.
  Plates hold at most `MAX_PLATE_PIXELS` (4 MP) each, so very large pages separate at a lower
  resolution. Coverage is per pixel at that resolution, anti-aliased edges included.
- No Ink Manager: spot inks can't be aliased or converted to process.
