# Bundle 2: Viewing, Tiled Deep Zoom, WebAssembly & Page Assembly

## Overview
Bundle 2 implements deep zoom pyramid tile rendering, zero-install WebAssembly browser viewing, AccessKit screen-reader tree generation, and non-destructive page organization with crash recovery journals, directly targeting **Milestone M3** (Viewing, Tiling & WASM) and **Milestone M4** (Page Organization & Structure).

---

## Parity Gaps Addressed

### Milestone M3 (Viewing, Tiling & WASM)
* `[P0]` **Tiled Deep Zoom (up to 6400%):** `TilePyramidCache` and `ViewportCuller` divide pages into hierarchical multi-scale pyramids ($100\%$ to $6400\%$) and only rasterize visible tiles intersecting the viewport.
* `[P0]` **WebAssembly Browser Runtime:** `WasmPdfViewer` compiles to pure `wasm32-unknown-unknown` without system C dependencies, handling document loading and rendering RGBA pixel tiles for browser Canvas2D/WebGL.
* `[P0]` **Screen-Reader Bridge (AccessKit):** `ScreenReaderTreeBuilder` converts PDF Logical Structure trees into native OS accessibility nodes (UIA / NSAccessibility / AT-SPI).

### Milestone M4 (Page Organization & Assembly)
* `[P1]` **Crash Recovery Journal:** Write-ahead transaction log tracking all page insertions, deletions, rotations, and crops to restore uncommitted state after unexpected crashes or page refreshes.
* `[P1]` **Drag-and-Drop & Range Selection:** Non-destructive page assembly, filtering by odd/even, portrait/landscape, and complex range expressions (`"1-3, 5, 8-end"`).
* `[P1]` **Bookmark Actions & Navigation:** Bookmark tree manipulation (`/Outlines`) with color, bold/italic style flags, and target page destinations (`/Fit`, `/XYZ`, `/FitH`, `/FitV`).

---

## Crate Layout

```
bundle_2_viewing_tiling_wasm_and_assembly/
├── Cargo.toml
├── README.md
├── crates/
│   ├── tiling/        # Multi-resolution pyramid tile cache & viewport culling
│   ├── wasm_bridge/   # WebAssembly browser client bridge
│   ├── a11y_bridge/   # AccessKit screen-reader accessibility node mapper
│   └── assembly/      # Page assembly, crash recovery journal, bookmarks & range filters
└── tests/
    └── integration_tests.rs
```
