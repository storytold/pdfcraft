# Derived image pixel cache: implementation and measurements

> **Last reviewed:** 2026-10-25 · **Last updated:** 2026-10-25 · **Change:** minor (unmerged rendering performance candidate; no readiness or parity re-estimate)

The source cache introduced by #1117 and refined by [#2436](https://github.com/storytold/pdfcraft/pull/2436)
retains decoded images up to 64 MiB per interpreter cache. A larger source bypasses retention.
Every repeated placement then decodes and resizes the full image, even when all placements use
the same small output. Smaller cached sources still have their pixels cloned and resized on
each draw.

This candidate caches the final, premultiplied image pixmap before placement. A hit bypasses
decoding, source-buffer cloning, resizing and colour expansion. It uses the existing resize,
interpolation, snapping and compositing behavior; no sampling policy changes.

## Existing work and backend ownership

The timing baseline is main `833cd63c8a0aec826595640b356b3213355957bd`, fetched and verified on
2026-10-25. The development checkout was clean before this task. The local branch was subsequently
refreshed onto `83ba28680f6c780ed7ae553f25555765e4802aeb`: the only upstream changes were updater
dependency pins and associated Windows lockfile resolution. Rendering source was verified
byte-identical across the refresh, and the release benchmark was rebuilt and measured on that
updated base. No backup checkout was modified.

Piohrun explicitly deferred scaled-copy caching in #2436: "I'll do it once they land", referring
to #1739 and #2288. The PR discussion and reviews contain no subsequent implementation claim.
On 2026-10-25, the contributor's 15 PRs created on or after October 15 and all 27 public fork branches were
checked. Their unmerged Hayro history contains no subsequent cache implementation; older merge
commits are not a new cache. The later monochrome optimizer/downsampling work changes stored PDF
images rather than renderer reuse. This establishes public evidence of an intention, not proof
about unpublished local work. See [the contributor's PRs](https://github.com/storytold/pdfcraft/pulls?q=is%3Apr+author%3APiohrun)
and [fork branches](https://github.com/Piohrun/pdfcraft/branches). A separate author search found no PRs by Piohrun in `storytold/craft-libs`.

The following remain open and unmerged at the checked heads:

| PR | Checked head | Relationship |
|---|---|---|
| [#2068](https://github.com/storytold/pdfcraft/pull/2068) | `60a82e85` | Moves the patched Hayro backend into shared `craft-pdf`; it does not add this cache. |
| [#1739](https://github.com/storytold/pdfcraft/pull/1739) | `ac99fbce` | Exact-area minification policy; no derived-result cache. |
| [#1586](https://github.com/storytold/pdfcraft/pull/1586) | `75e7078a` | Filtering beyond 2:1 reduction; no derived-result cache. |
| [#3683](https://github.com/storytold/pdfcraft/pull/3683) | `dd121ab4` | Broader image-minification policy; no derived-result cache. |
| [#3858](https://github.com/storytold/pdfcraft/pull/3858) | `9798db30` | Area shrinking and bilinear enlargement; no derived-result cache. |

This patch belongs to the backend, not the app adapter. If #2068 lands first, forward-port the
isolated cache module, resource-key accessor, renderer integration and tests to the shared
backend reviewed by [craft-libs #106](https://github.com/storytold/craft-libs/pull/106). Do not
restore local vendor trees or duplicate the cache in the app adapter. If a sampling-policy PR
lands first, review its size/snap planning together with `derived_image_candidate` and rerun the
cache-on/off differential tests. No contributor branch or shared repository was modified.

## Bounds, keys and lifetime

- Retained pixel payload is strictly limited to **16 MiB per render cache**, with **512 entries**
  bounding bookkeeping. FIFO eviction is deterministic; hits do not reorder entries. An oversized
  result is not retained and does not evict useful entries. Reservations and borrow conflicts
  fall back to uncached rendering.
- Values contain an `Arc<Pixmap>` of the derived result. They do not contain the original decoded
  source or encoded stream. The existing decoded-source budget remains 64 MiB.
- Keys use the existing decoded-image identity: stream location, dictionary hash, object number
  and generation, decoding settings, interpolation and preview/final quality. Final output
  dimensions and the backend transparency hint are added. Placement, clipping and blending
  execute afresh on each hit. Snap dimensions are computed before lookup using existing rules.
- Eligibility is deliberately limited to ordinary device Gray/RGB raster XObjects without
  colour-space substitution. Inline images, transfer functions, masks, stencils, JPX, Type3,
  tiling-pattern rendering and knockout groups keep the existing path. Ambiguous size plans,
  original-size results, failed decodes and mismatched decoded dimensions are not retained.
- The cache shares `RenderCache`'s document/worker lifetime and clone semantics. Existing content
  invalidation and parser retirement replace it along with the interpreter cache. Metadata-only
  changes can retain it because the rendered source is unchanged.
- The byte limit covers retained cache pixels, not total renderer memory. It is **per `RenderCache`**;
  each native render worker owns one. With up to seven workers, a document render pool can retain
  up to **112 MiB** of derived pixels. Multiple active documents multiply that allowance. Active
  draw commands may hold an Arc after eviction, and transient decoding/resampling buffers, other
  caches and page output buffers add separate memory costs.

## Reproducible benchmark

The contributor-original fixture is generated in Rust by
`crates/render/examples/image_cache_bench.rs`; no binary fixture or font is committed. It writes
only under ignored `target/image-cache-bench/`. A deterministic Flate RGB source is placed at
240 by 240 device pixels, either once or nine times on a 1000 by 1000 point page. A 5000-square
source decodes to 75,000,000 bytes, exceeding the existing 64 MiB source budget. Its final RGBA
copy occupies 230,400 bytes. A 1024-square control fits the existing source cache.

Build and run from the repository root:

```powershell
cargo build -p pdfcraft-render --example image_cache_bench --release -j 2 --locked
target/release/examples/image_cache_bench.exe 5000 9 3 true
target/release/examples/image_cache_bench.exe 5000 9 3 false
target/release/examples/image_cache_bench.exe 1024 9 3 true
target/release/examples/image_cache_bench.exe 5000 1 3 true
```

Measurements use Windows x64, AMD64 Family 25 Model 33, Rust 1.98.1 and the repository's release
profile (thin LTO, one codegen unit). Each workload uses three fresh processes, each rendering
three rounds with the same document and `RenderCache`. Reported cold time is the median first
round; warm time is the median of six subsequent rounds. Fixture generation, parsing, hashing
and output writes are outside the render timer. "Cold" describes the renderer cache, not the OS
filesystem cache. No compiler ran during these timing samples.

Peak working set is the median of the three process peaks, collected through Windows
`Process.PeakWorkingSet64`, sampled every 10 ms. It includes fixture generation and decoding,
not just retained cache data. These are synthetic workload measurements, not whole-app estimates.

| Source / copies / interpolation | Cold before → after (ms) | Warm before → after (ms) | Peak working set before → after (MiB) |
|---|---:|---:|---:|
| 5000² / 9 / true | 2233.392 → 253.326 | 2260.367 → 2.895 | 279.203 → 275.902 |
| 5000² / 9 / false | 2040.547 → 227.566 | 2061.175 → 2.895 | 278.227 → 275.867 |
| 1024² / 9 / true | 25.841 → 14.049 | 14.747 → 2.783 | 23.164 → 21.055 |
| 5000² / 1 / true | 250.847 → 244.888 | 255.475 → 1.613 | 276.434 → 275.953 |

The single-placement cold control still pays the full initial decode; its small timing change
should be treated as variation. The large improvement comes from avoiding repeated work. Peak
memory changes little because the first large decode is still necessary. Warm cache lifetime
matches the native worker design. Separate CLI `page_render` calls currently construct fresh
render caches; they benefit from repeated placements within each render, not these cross-round
warm timings.

## Correctness evidence and validation

All four benchmark outputs were compared byte-for-byte using SHA-256 over the saved RGBA buffers;
before and after match. All timed rounds also report stable output hashes.

The existing `pdfcraft-oracle` probe, PAM/PGM decoders and symmetric neighbourhood comparison
were used with external Poppler 26.07.0 at 72 dpi, tolerance 8:

| Workload | Disagreement before | Disagreement after |
|---|---:|---:|
| 5000² / 9 / true | 3.402400% | 3.402400% |
| 5000² / 9 / false | 4.049300% | 4.049300% |
| 1024² / 9 / true | 1.481100% | 1.481100% |
| 5000² / 1 / true | 0.386600% | 0.386600% |

These unchanged scores establish no additional disagreement for the fixtures; they do not
resolve existing sampling differences with Poppler.

Seven focused tests cover shared cache hits, repeated/warm decode avoidance, FIFO eviction,
the exact production byte limit, oversized entries, metadata bounds, document replacement and
fresh-cache invalidation, cache-on/off byte identity across sampling/placement/preview/tile/paint
settings, image masks/soft masks and cancellation. The repeated-hit regression was run against
the original renderer and failed with zero hits instead of one; the cache implementation was
then restored. The full scoped release run passed 461 tests: 18 Hayro unit tests, 400
pdfcraft-render unit tests and 43 render integration tests, with one existing ignored integration
test. The graphics-state soft-mask differential case is included in that run.

Other completed validation:

- `cargo fmt --all --check` and `git diff --check`.
- Strict Clippy for Hayro/render across all targets, and hayro-interpret's library, with
  `-D warnings`. An initial broader hayro-interpret example check cannot compile the existing
  `extract_images` example because its manifest lacks a direct `image` dependency; no unrelated
  manifest change was made.
- `cargo xtask layers` and `cargo xtask assets`: no violations.
- Native release CLI build, then `pdfcraft-cli run --script ... --root ... --report` with two
  successful `doc_open` / `page_render` pairs at 72 dpi. Both PNGs are 1000 by 1000 pixels.
  The CLI PAM agrees exactly with baseline pixels after white-background compositing
  (`max_delta=0`, zero differing pixels), and the rendered PNG was inspected.
- The full test run and CLI validation preceded the dependency-only base refresh. Renderer
  sources were preserved exactly; the final release benchmark build, output identity and
  timings were checked after the refresh. The updated dependency metadata is retained from
  upstream without local manifest or lockfile edits.

Only the Windows native target is installed. No full corpus, other operating system, WASM,
browser or GPU qualification is claimed.

## Changed files

| File | Purpose |
|---|---|
| `vendor/hayro/src/derived_image_cache.rs` | FIFO cache, bounds, and seven focused tests. |
| `vendor/hayro/src/renderer.rs` | Pre-decode lookup and admission of successfully resized pixmaps. |
| `vendor/hayro/src/lib.rs` | Document/worker cache ownership and lifetime. |
| `vendor/hayro-interpret/src/types.rs` | Narrow opaque resource-key accessor. |
| `vendor/hayro-interpret/src/x_object.rs` | Eligibility checks and complete existing decode identity. |
| `crates/render/examples/image_cache_bench.rs` | Bounded synthetic generator and timing/hash runner. |
| `vendor/README.md` | Backend patch provenance and regression references. |
| `ROADMAP.md`, `docs/roadmap.md` | Record this unmerged candidate without changing readiness claims. |
| `docs/rendering-image-cache.md` | Measurements, validation, constraints and migration handoff. |
