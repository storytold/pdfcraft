# PdfCraft architecture

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first public architecture document, written from the code on main) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

How PdfCraft is built today. The design rationale and the planned crates live in
`plan/architecture.md` and `plan/adr/` (local-only); this file describes what is on `main`. Each
crate's `README.md` has the detail.

## Shape

One Cargo workspace (`Cargo.toml`, edition 2024, Rust ≥ 1.90, `MIT OR Apache-2.0`), 31 library
crates in `crates/`, three binaries in `apps/`, build tooling in `xtask/`, and patched third-party
crates in `vendor/`. About 152k lines of Rust and 1,765 tests (2026-10-10). Rust only: no
handwritten JS/TS, in the product or the build.

| Binary | What it is |
|---|---|
| `apps/pdfcraft` | The desktop app (macOS, Windows, Linux, FreeBSD): eframe/egui on wgpu, single instance, Apple events, update check on request |
| `apps/pdfcraft-cli` | `inspect`, `edit`, `combine`, `extract`, `split`, `run` (any automation tool or a JSON script), `tools`, `ui` (drive the running app), and the opt-in `mcp` server |
| `apps/pdfcraft-web` | The same egui shell compiled to WebAssembly |

## Layers

`cargo xtask layers` enforces the layering (`xtask/src/layers.rs`). A crate may depend only on
crates in lower layers; nothing below L7 may depend on egui, winit, eframe or rfd; L0–L1 crates are
standalone and publishable.

| Layer | Crates today | Role |
|---|---|---|
| L0 foundation | `geom`, `filters`, `crypt` | Geometry; stream filters (Flate, LZW, predictors, ASCII85, ASCIIHex, RunLength); standard security handler R2–R6 (RC4, AES-128/256, SASLprep) |
| L1 objects | `cos` | Lazy, tolerant parser with xref repair; copy-on-write object graph; incremental and full writers with object streams; decryption on load and re-encryption on save |
| L2 core model | `content`, `model`, `fonts` | Content-stream operators with source spans and a serializer; typed page tree with inherited attributes and display geometry (`/UserUnit`, `/Rotate`); font metrics and encodings for generated appearances |
| L3 core services | `render`, `annot`, `forms`, `js`, `xfa` | Rendering and inspection (bootstrap: vendored `hayro`, `lopdf`); annotation builders and appearance streams; AcroForm model, filling, appearances; sandboxed JavaScript (boa) and FormCalc; XFA template layout and data |
| L4 features | `edit`, `organize`, `redact`, `xfdf`, `sign`, `ocr`, `create`, `export`, `optimize`, `preflight`, `a11y`, `compare`, `measure`, `print` | One crate per Acrobat tool family |
| L6 façade | `engine` | Session, open documents, edit history, the command registry and tool catalogue. Frontends never reach past it |
| L7 platform and frontends | `platform`, `ui-egui`, `automation` | OS services; the egui shell (desktop and web); headless JSON-Schema tools, CLI and MCP |

Planned crates that the layer table already reserves (`arlington`, `color`, `ops`, `text`, `ml`,
`security`, `prepress`, `search`, `media`, `ai`, `viewport`, `tools`, `ui-common`) don't exist yet.

## Data model and editing

The PDF object graph *is* the model (`crates/engine/README.md` §The editing model):

1. Each open document holds a `pdfcraft_cos::Document`: the parsed objects plus a copy-on-write
   overlay. Unknown data is preserved because nothing is rebuilt that wasn't understood.
2. An edit runs on a clone; on success the previous state goes onto the undo stack. Clones share
   unchanged data, so snapshots are cheap.
3. After every edit an incremental write produces the working file (original bytes plus one
   revision); the view is refreshed from those bytes, so what you see is what Save writes.
4. Saves are incremental and atomic. A full rewrite happens only where it must: redaction, PDF/A
   conversion, print imposition, a changed password, Save As with garbage collection.
5. Autosave and crash recovery keep encrypted documents encrypted. Signed documents are protected
   from rewrites.

## Rendering and text

- Pages are rasterized on the CPU by the vendored `hayro` family (`hayro`, `hayro-interpret`,
  `hayro-syntax`, `hayro-jbig2`), with local patches for appearance states, layers, resource
  limits, cancellation, tiles and `/UserUnit` (`vendor/README.md`). A render pool renders visible
  pages first, then neighbours and thumbnails; tiles keep deep zoom sharp; a watchdog skips
  pathological pages.
- The raster is uploaded to the GPU as a texture; egui draws the interface through wgpu (OpenGL
  fallback when a device can't be configured).
- Text extraction (reading order, columns, vertical CJK, rotated text) feeds find, select, copy,
  redaction, export and compare; word-F1 0.98 against `pdftotext` on the corpus.
- The replacement (M2, ADR-0004) is the `model` crate, our own font engine and DisplayList devices;
  only the first pieces of `model` exist. See [gaps.md](gaps.md) gap 2.

## File I/O

- PDF in: any revision chain, xref tables or streams, hybrid files, damaged files (repaired and
  logged), all standard-security revisions. Image codecs (DCT, JPX, JBIG2, CCITT) decode through
  the bootstrap renderer.
- PDF out: incremental or full saves, object streams, re-encryption, resource de-duplication when
  combining.
- Other formats: images and text in (`create`); PNG/JPEG/TIFF, text, RTF, Word and HTML out
  (`export`, `render`); FDF/XFDF/XML/CSV/TXT form and comment data (`xfdf`); PKCS #12, X.509,
  PEM/DER certificates (`sign`). Full matrix: [file-format-parity.md](file-format-parity.md).

## Agent control

Every user-facing feature is reachable headlessly (`AGENTS.md` §3):

- the **command registry** (`crates/engine/src/commands.rs`, 184 commands) generates menus,
  shortcuts and the command palette;
- the **automation tool table** (`crates/automation`, 141 tools) is shared by `pdfcraft-cli run`
  and the opt-in **MCP server** (stdio only; `pdfcraft-cli mcp`), confined to `--root`;
- the opt-in **UI control channel** (`pdfcraft --control FILE`; loopback, per-launch token) lets an
  agent inspect, click, type, run commands and take screenshots of the real interface.

## Safety

- `unsafe_code = "forbid"` workspace-wide; crates deny `unwrap`/`expect`/`panic!`; engine entry
  points keep a last-resort `catch_unwind` (`AGENTS.md` §4).
- Every input-derived size is bounded (decompression, images, recursion, content per page).
- `cargo xtask fuzz` runs nightly; every finding becomes a synthetic regression test.

## Build and quality gates

`cargo xtask ci` runs fmt, clippy (`-D warnings`), tests, the layering check, the WASM check,
`xtask assets` (asset licences and hashes), `xtask parity` (checklist claims must cite real commands,
tools and tests) and `xtask deny` (dependency licences). Other tasks: `xtask fuzz`, `xtask corpus` and `xtask check` (robustness
sweep over the pinned pdf.js corpus), `xtask text-oracle` (word F1 against `pdftotext`),
`xtask demo-pdf`, `xtask screenshots`, `xtask models` (OCR models), `xtask version`. Releases are
built by `.github/workflows/release.yml` ([releasing.md](releasing.md)).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | Created from the code on main (crate list, `xtask/src/layers.rs`, crate READMEs, `vendor/README.md`) |
