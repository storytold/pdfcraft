# Architecture

The authority for the crate graph is **`xtask/src/layers.rs`**, and `cargo xtask layers` fails the
build on any violation. This document explains that table: what each layer is for, which crates
exist, which are reserved, and which boundaries must not move.

## 1. Layers

A crate may depend on **lower** layers only. Same-layer edges exist only where
`layers::SIDEWAYS` lists them. UI toolkits (`egui`, `eframe`, `winit`, `wgpu*`, `rfd`) are
forbidden below L7.

| Layer | Purpose | Shipping today | Reserved (registered, not yet written) |
|---|---|---|---|
| **L0** | Foundation, standalone, publishable on their own | `geom`, `filters`, `crypt` | `arlington` |
| **L1** | The PDF object layer | `cos` | — |
| **L2** | Core model over the object graph | `content`, `model`, `fonts` | `color`, `ops` |
| **L3** | Core services | `render`, `annot`, `forms`, `js`, `xfa` | `text`, `ml` |
| **L4** | Features | `edit`, `organize`, `redact`, `xfdf`, `sign`, `ocr`, `create`, `export`, `optimize`, `preflight`, `a11y`, `compare`, `measure`, `print` | `security`, `prepress`, `search`, `media`, `ai` |
| **L5** | Interaction | — | `viewport`, `tools` |
| **L6** | Façade | `engine` | — |
| **L7** | Platform and frontends | `ui-egui`, `automation` | `platform`, `ui-common` |
| **test** | Test support; dev-dependency only, never a normal dependency | — | `testkit`, `oracle` |
| **L8** | Apps and tooling, exempt from layering | `pdfcraft`, `pdfcraft-cli`, `pdfcraft-web`, `xtask` | — |

Sixteen reserved names are already in the table. Adding one of them is not an architectural
decision — it has been made. Adding a name that is *not* in the table is: register it in
`layers.rs` first, and say why here.

### Standalone crates

`geom`, `filters`, `crypt` may use **no** workspace crate at all. `cos` may use only
`filters`, `crypt` and `geom` (`layers::STANDALONE_DEPS`). This keeps the parser and the
crypto independently auditable and reusable.

### Allowed same-layer edges

`redact → edit`, `a11y → preflight`, `prepress → preflight`, `export → ocr`, `sign → security`,
`ui-egui → ui-common`, `ui-egui → platform`, `automation → platform`.

**`forms → js` is deliberately absent.** Forms reach the script engine through a trait the
façade injects (`ActionRunner`), never directly, so a build without JavaScript is possible and
a malicious script cannot reach the field model except through that one seam. A test in
`layers.rs` pins this.

## 2. Boundaries that must not move

**L7 is the only place a frontend exists.** `engine` is the whole API surface a frontend gets.
`ui-egui` is one implementation; replacing it must not require touching anything below L6.

**`automation` is L7 and headless.** Every user-facing feature is reachable without a GUI:
engine API → tool in `crates/automation/src/tools.rs` → `pdfcraft-cli run` and MCP share one
table (AGENTS.md §3). A feature without a tool is not finished.

**The UI control channel is a separate seam** (`crates/ui-egui/src/control.rs`), driving the
*running* app over loopback with a per-launch token. It reads the AccessKit tree, so every
widget an agent can find is also a widget a screen reader can find. Making a widget inspectable
and making it accessible are the same work.

**`pdfcraft-render` is a façade over the bootstrap.** It is the only crate that depends on
`hayro` or `lopdf` in production; everywhere else those are `[dev-dependencies]` used as test
oracles. Its public API — `inspect() -> DocInfo`, `PageRenderer`/`RenderPool`, `text::PageText` —
is what `engine` and `ui-egui` consume. **Keep that API frozen while the renderer is replaced.**
This containment is what makes the renderer swap a bounded project rather than a rewrite.

## 3. The editing model

Each open document is a `pdfcraft_cos::Document`: the object graph plus a copy-on-write
overlay. An edit runs on a clone; on success the previous state goes on the undo stack (clones
share unchanged data, so snapshots are cheap). After every edit the *working file* is produced by
an incremental write — original bytes plus one appended revision — and the view is refreshed from
those bytes, so what you see is exactly what Save will write. Saving rebases onto the written
bytes.

Consequences worth keeping:
- the original bytes of an opened file are never rewritten in place;
- a full rewrite happens only where it must (redaction, PDF/A conversion, print imposition);
- unknown data survives round trips, because nothing is reconstructed that wasn't understood.

## 4. Everything is a command

`crates/engine/src/commands.rs` is the registry: id, label, menu, shortcut, and a `Needs`
precondition that encodes document security (`Assembly`, `Modification`, `Annotate`,
`FillForms`, `Security`, …). Menus, the ⌘K palette, keyboard shortcuts and the control channel
all read it, so every surface agrees on what exists and when it is enabled.

`crates/engine/src/catalog.rs` holds Acrobat's "All tools" information architecture as data:
tool groups, sections, icons, hues, and an `Availability` of `Ready`, `Planned(milestone)` or
`Provider`. The UI renders the catalogue, including what is not built yet. Keep `Availability`
honest — it is the app telling the user the truth.

## 5. Where the font work lands

`fonts` is L2 and already depends on `cos`. The font engine (execution plan phase F) grows
**there**; no new crate is needed and none is reserved for it. It currently reads font
*dictionaries* (`/Widths`, `/W`, `/DW`, `/Encoding`, `/Differences`, `/ToUnicode`) but never a
font *program*. Reading, subsetting and re-embedding font programs is the prerequisite for both
the renderer (L3 `render`) and editing existing text (L4 `edit`), which is why it comes first.

`text` (L3, reserved) is where extraction, reading order and shaping move once they outgrow
`render::text`.
