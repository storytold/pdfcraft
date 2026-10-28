# Bundle 1: Core Engine, Decoders & 2D Vector Rasterizer

## Overview
Bundle 1 implements the foundational decoding, storage, rasterization, and structural validation layers for PdfCraft, directly targeting the critical requirements of **Milestone M1** (Core Document Model & Filter Safety) and **Milestone M2** (Display Model & Vector Renderer).

---

## Parity Gaps Addressed

### Milestone M1 (Core Document Model & Filter Safety)
* `[P0]` **Image Codecs in Filters Crate:** Full clean-room decoders for DCT (`JPEG`), JPX (`JPEG 2000`), JBIG2 (bi-level facsimile), and CCITT Fax (Group 3 1D/2D and Group 4 MMR).
* `[P0]` **Lazy Loading of GB-Scale Files:** `BoundedDocumentReader` with streaming cross-reference parsing and an LRU memory-bounded object cache to keep memory usage under strict budgets regardless of document size.
* `[P1]` **Attachment-Only Encryption (`/EFF`):** Evaluates ISO 32000-1 §7.6.5 encryption dictionaries so documents open without a password while embedded file attachments remain encrypted.
* `[P2]` **Deterministic Full-Save Mode:** Byte-for-byte reproducible PDF serialization with ordered object ID generation and stable digest IDs.

### Milestone M2 (Display Model & Vector Renderer)
* `[P0]` **Page Rasterizer:** 2D graphics state machine (`CTM`, clipping paths, color spaces, line styles) and path execution (`m`, `l`, `c`, `re`, `f`, `S`, `Do`) rendering to 32-bit RGBA pixel buffers.
* `[P0]` **Shadings, Gradients & Tiling Patterns:** Axial (Type 2), Radial (Type 3), and Coons patch mesh shading interpolators, plus Type 1 tiling pattern tessellation.
* `[P0]` **Blend Modes & Soft Masks:** Complete ISO 32000-1 §11 Porter-Duff compositing, separable/non-separable blend modes (Multiply, Screen, Overlay, etc.), and luminosity soft masks.
* `[P0]` **Type 3 Font Interpretation:** Full execution of `/CharProcs` streams supporting `d0` (colored) and `d1` (uncolored stencil) glyph operators.
* `[P0]` **CJK CID Fonts & CMaps:** Adobe-Japan1, Adobe-GB1, and Adobe-CNS1 CMap parsing and fallback mapping when `/ToUnicode` is missing.
* `[P1]` **Structural Arlington PDF Model Validation:** Machine-readable validation of dictionary schemas against DARPA SafeDocs Arlington specifications.

---

## Crate Layout

```
bundle_1_core_decoders_and_rasterizer/
├── Cargo.toml
├── README.md
├── crates/
│   ├── codecs/       # DCT, JPX, JBIG2, CCITT fax decoders & safe filter pipeline
│   ├── storage/      # Bounded lazy reader, LRU object cache, /EFF encryption, deterministic writer
│   ├── rasterizer/   # 2D page rasterizer, blend modes, shadings, Type 3 fonts, CJK CMap
│   └── arlington/    # Arlington model structural dictionary validator
└── tests/
    └── integration_tests.rs
```

---

## Clean-Room Standards & Safety Compliance
* **Clean-Room:** Zero reverse engineering or binary decompilation of Adobe Acrobat binaries (`AcroExportX.dll`, `AGM.dll`, etc.). Behavior is implemented strictly according to ISO 32000-1, ISO 32000-2, ITU-T T.4/T.6, and the open Arlington PDF model.
* **Never-Crash:** Every slice operation uses `.get()`, arithmetic operations use checked/saturating math, and memory allocations are guarded against decompression bombs.
