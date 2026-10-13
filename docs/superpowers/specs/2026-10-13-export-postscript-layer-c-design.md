# Layer C + Layer B follow-ups design — PS/EPS export: transparency flattened, true CMYK/Separation, tiling patterns, AGL naming (approved 2026-10-13, btw session)

> Handoff note: this design was brainstormed and approved in a side session.
> Main-thread pickup: (1) create a worktree (using-git-worktrees; stack on `ps-layer-b`
> after it merges — W1 is the promised small follow-up PR; rebase/retarget as stacks land);
> (2) commit this as `docs/superpowers/specs/2026-10-13-export-postscript-layer-c-design.md`;
> (3) run writing-plans for the implementation plan (the wave structure W1–W3 maps to plan
> tasks); (4) execute (subagent-driven or native, per the plan-time choice). The design below
> is the approved content, verbatim in substance.

## Context

Layer A (PR #1785) shipped the exporter with Type 1 passthrough and vectorize-everything-else;
Layer B (ps-layer-b) shipped fonts as text: Type 42 with synthesized cmap (+ subsetting), CFF
promotion to CIDFontType0, CID composites, Type 1 subsetting + dedup. Its carve-outs — true
CMYK/Separation emission and tiling patterns (~10–15 h follow-up PR) — plus AGL naming for
post-3.0 name-less TrueType extraction were deferred here. Layer C is transparency: blend
modes, soft masks and opacity groups have no PostScript equivalent, so they must flatten to
raster regions via the renderer's own compositing (vello_cpu).

Current state the design builds on:
- Transparency today: `push_transparency_group`/`set_blend_mode`/`set_soft_mask` note-and-drop
  (one note per page); the transparency oracle fixture never enters the raster comparison.
- Flat paints today: `paint_ops` emits `setgray`/`setrgbcolor` only — the interpreter resolves
  paints to RGBA before the device sees them, so original CMYK/Separation components are
  invisible at the device boundary. Shadings already emit `/DeviceCMYK` (`device_space()`).
- Tiling today: `tiling_approx` (flat gray + note); the recorded Layer A Task 6 decision names
  what the vendor withholds: the raw cell stream, `pub(crate)` fields (`/PaintType`), and no
  `Device` handle to drive the cell through.
- Names today: rendering and txtwrite extraction resolve through `/Encoding` glyph names
  (Layer B's falsification of the cmap-code theory); synthesized `gNNNN` names extract as
  control codes.

## Approved decisions

1. **One combined design, three waves, two shippable stack points.** W1 = the carve-outs PR
   (CMYK/Separation + tiling + AGL, 10–18 h); W2+W3 = Layer C (30–50 h). CMYK/tiling first:
   they shrink the degradation list immediately, and their content inside transparent regions
   gets rasterized by C anyway.
2. **Flatten DPI: fixed constant 300.** No new tool options; deterministic tests; the
   disclosure note names the rasterization.
3. **CMYK + Separation, not DeviceN.** DeviceCMYK flat paints → `setcmykcolor`; Separation →
   PS `/Separation` colorspaces with the PDF's tint function (true spot colors). DeviceN
   keeps today's RGB fallback + note as a disclosed gap.
4. **Flattening engine: shadow-scene incremental flattener.** A parallel vello_cpu scene
   recorded from the device's own draw calls; regions composited exactly as `page_render`
   composites them; every transparency construct becomes oracle-verifiable. (Rejected:
   group-only alpha overlays — wrong for non-Normal blends; whole-page raster — abandons
   vector for all content on such pages.)
5. **Tiling via vendor patch + true `makepattern`.** The cell driven through this device
   (Type 3 `interpret` precedent); cells containing transparency degrade to the flat-gray
   note in W1, then route into the flattener from W2 on.
6. **AGL naming via `/uniXXXX`.** Reverse-map name-less gids through the font's original cmap
   subtable (kept in the sfnts since Layer B W1); gs's AGL resolves `uniXXXX` natively — no
   AGLFN table ships. Deterministic: lowest codepoint wins. Rendering unchanged.
7. **Successful flattening still discloses itself:** one note per page ("transparency
   flattened to raster") — the picture is faithful but the region is no longer vector;
   never-silent applies both ways.
8. **Flattener failure falls back, never loses output:** flush the withheld vector buffer and
   emit today's note. Worst case equals today's behavior.

## Scope detail

- **CMYK/Separation (W1).** Vendor additive patch exposing a paint's source space and
  components (`DeviceGray`/`DeviceRGB`/`DeviceCMYK`/`Separation`), marked `PdfCraft patch
  (ps-export)` per the `Function::parsed()`/`ColorSpace::device_space()` precedent. Then:
  Gray/RGB unchanged; CMYK → `setcmykcolor` in `paint_ops` (fills, strokes and text runs
  already share it); Separation → a `/Separation <name> /DeviceCMYK <tintTransform>`
  colorspace resource in the prolog, one per unique colorant (deduplicated like font
  programs), used via `setcolorspace`/`setcolor`; the tint transform reuses the
  PDF-function→PS conversion `shading.rs` already has. Unconvertible tint function → RGB
  fallback + note naming the colorant. DeviceN → existing RGB fallback + note.
- **Tiling patterns (W1).** Vendor patch: the cell's content driven through *this* device
  (`TilingPattern::interpret_cell`, the Type 3 `g.interpret` precedent), `/PaintType`
  exposed (colored vs uncolored decides whether the fill color becomes a pattern parameter),
  plus XStep/YStep/bbox/matrix. Emission: one `PatternType 1` resource per unique pattern
  (deduplicated), its `PaintProc` holding the re-emitted cell ops (the device drives the cell
  into the resource with its own state); the page does `makepattern` + `setpattern`. Hostile
  or transparent cell content → flat-gray note in W1; transparent cells route into the
  flattener from W2 on.
- **AGL naming (W1).** In `fonts.rs`'s name resolution, gids whose names would be synthesized
  (`gNNNN`: post 3.0 TrueType without CFF) get reverse-mapped gid→unicode through the font's
  original cmap subtable and emitted as `/uniXXXX` names (up to six hex digits for astral
  codepoints) in `/Encoding` and `/CharStrings`. Deterministic: lowest codepoint wins.
- **Layer C core (W2).** A new `crates/ps-export/src/flatten.rs` owns the **shadow scene**:
  - Recording: every draw the device emits — path fills/strokes (total CTM), images (decoded
    bytes), glyphs (their outlines, the same `outline.outline()` the vectorize arm uses) — is
    recorded into a vello_cpu scene. Text stays text in the PS but outlines in the scene,
    which is exactly what the raster renderer paints.
  - Groups: `push_transparency_group(opacity, mask, blend)` pushes the same layer onto the
    scene (masks interpreted through the same machinery the renderer uses; if the vendor does
    not expose mask interpretation, that is one more additive patch). When a group carries
    real transparency (opacity < 1, a mask, or non-Normal blend — the current note's
    condition), the device switches PS output into a **side buffer**: draws keep recording
    into the scene but their PS ops are withheld.
  - Flatten at pop: the group's accumulated bbox (clipped to the page) renders from the scene
    at 300 dpi → one opaque raster, emitted through the existing `images.rs` machinery
    (Flate) at that point in the stream, and recorded back into the scene as an opaque image
    so later overlapping groups composite over the flattened result. The withheld buffer is
    discarded. The backdrop's vector ops stay in the PS underneath — covered exactly; later
    content draws on top; z-order correct.
- **Layer C completion (W3).** Non-Normal blend modes (vello composites them inside the
  scene); nested groups (uniform rule: flatten at every pop of a real-transparency group; an
  inner group's raster lands in the outer group's buffer and scene, the outer pop flattens
  the union — no special cases); interactions — tiling cells inside groups, images, glyphs;
  size and safety caps below. Edge rule: blend-mode or soft-mask state arriving outside any
  real-transparency group keeps today's note — flattening only ever burns the region a group
  actually covers (whole-page rasterization was rejected).

## Components

| Unit | Does |
|---|---|
| `flatten.rs` (new) | The shadow scene: parallel vello_cpu scene recording, region rendering at 300 dpi, opaque raster emission through `images.rs`, pixel-area cap, panic guard |
| `device.rs` rework | `paint_ops` gains the CMYK arm; tiling paints gain `setpattern`; group side-buffering and flatten orchestration (push/pop, bbox accumulation, buffer swap/flush) |
| `shading.rs` extend (or `color.rs`) | `/Separation` colorspace resources with tint transforms (reuses `function_text`); per-colorant dedup |
| tiling emission (extend `shading.rs` or new `pattern.rs`) | `PatternType 1` resources, `PaintProc` cell re-emission via `interpret_cell`, `makepattern`/`setpattern`, per-pattern dedup |
| `fonts.rs` extend | `/uniXXXX` AGL naming for synthesized names (cmap reverse map over the sfnts' kept subtable) |
| vendor patches (hayro) | Paint source space + components; `TilingPattern::interpret_cell` + `/PaintType` + geometry; mask interpretation if not reachable — all additive, marked `PdfCraft patch (ps-export)` |

## Data flow

Interpreter → `PsDevice` → {`Emitter` (PS bytes) | shadow scene (vello_cpu)}. A
real-transparency group's draws go to the scene and a withheld PS buffer; at pop the region
raster emits as an image and records back into the scene, the buffer discards; on failure the
buffer flushes as vector and the note stands. One scene per exported page; the PS stream and
the scene stay visually equivalent by construction (both consume the same draws).

## Error handling

- Degrade ladder, per arm: CMYK/tiling/separation failures fall back to today's RGB/flat-gray
  + note naming the thing that declined; flattener failure (region render error, area cap
  exceeded — cap ≈ 100 Mpx against hostile full-bleed groups, or a scene panic wrapped in
  the engine-entry `catch_unwind` guard the renderer itself uses) flushes the withheld
  vector and keeps today's note. Never silent-wrong output; never lost output; never a crash.
- Checked arithmetic on all region math (bbox ∩ page, dpi scaling in `f64` with finite
  guards); PS output stays pure ASCII (rasters ride the existing image pipeline).

## Testing

1. Byte-level unit tests per module (synthetic CMYK/separation/tiling/cmap fixtures in-test,
   Layer A/B style), including hostile fixtures: unconvertible tint functions, hostile tiling
   cells, broken mask streams, huge group bboxes.
2. gs oracle gates per arm: CMYK (raster-match; gs's `setcmykcolor` conversion vs the
   renderer's may need a documented tolerance constant in the `TEXT_TOLERANCE` tradition —
   measured, named, never a silent widening of 2/255); Separation (colorant name + tint
   renders); tiling (repeated tiles visible in the raster); AGL (post-3.0 fixture emits
   `/uniXXXX`, txtwrite extracts).
3. **The Layer C headline gate:** `transparency_page_matches_ghostscript` enters the raster
   comparison (today it asserts notes only), plus soft-mask, blend-mode, nested-group and
   tiling-inside-group fixtures. gs resamples our 300 dpi region rasters while PdfCraft
   renders the source vectors — a documented flatten-region tolerance is pre-authorized by
   the same rule.
4. Parity flips per wave (the Task 9 pattern, warned-vs-documented distinction intact): W1
   removes the CMYK/tiling "Not yet" tails; W3 removes "transparency unflattened" and moves
   the note to the disclosure wording; roadmap M10, gaps §8, file-format-parity, ROADMAP log
   updated with each PR.

## Sequencing (waves, each shippable)

| Wave | Contents | Estimate |
|---|---|---|
| W1 | CMYK/Separation + tiling + AGL (the promised small PR) | 10–18 h |
| W2 | `flatten.rs`: shadow scene, side buffers, opacity groups + soft masks flattened | 15–25 h |
| W3 | Blend modes, nested groups, interactions, caps, full gates + parity flip | 15–25 h |
| | **Total** | **40–68 h** |

(Band check: W1 sits at the carve-outs' 10–15 h with the AGL addition; W2+W3 sit inside
Layer C's 30–60 h.)

## PR logistics

W1 stacks on `ps-layer-b` after it merges (it is the promised follow-up PR); W2+W3 stack on
W1; retarget to `main` as the stack lands. Vendor patches ride the PR that first needs them,
marked `PdfCraft patch (ps-export)`.

## Layer B rulings this design relies on

- The vendor additive-patch precedent (`Function::parsed()`, `ColorSpace::device_space()`) —
  the paint-space, tiling-cell and (if needed) mask patches follow it.
- Layer B W1 keeps the font's original cmap subtable in the sfnts — the AGL reverse map
  reads it.
- The gs name-resolution finding (codes resolve through `/Encoding` names → `/CharStrings`;
  extraction rides names) — `/uniXXXX` names carry extraction, and gs's AGL resolves them
  without any table.
- The `TEXT_TOLERANCE` precedent — documented, measured constants for CMYK conversion and
  flatten-region resampling deltas; the global 2/255 is never silently widened.
- The oracle gate bar institutionalized in Layer B ("no arm merges without ink + raster
  parity") — Layer C's headline is the transparency fixture finally meeting it.
- The Task 9 parity pattern and the warned-vs-documented distinction — every note wording in
  this design follows it.
