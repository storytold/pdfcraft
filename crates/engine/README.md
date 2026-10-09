# pdfcraft-engine

The façade every frontend talks to. It holds the open documents, their edit history and the tool
catalogue; frontends never reach past it into the parsing, rendering or editing crates.

- **Layer:** L6. It depends on most of L1–L4 and on nothing above itself. **No UI toolkit** —
  `egui`, `winit`, `eframe` and `rfd` are forbidden below L7, and `xtask layers` enforces it.
- The crate denies `unwrap`/`expect`/`panic!`/`todo!`. Entry points that run untrusted-document
  code keep a last-resort `catch_unwind` guard so an escaped panic becomes an error and the user
  keeps their document.

Two frontends exist on top of it today: `pdfcraft-ui-egui` (desktop and web) and
`pdfcraft-automation` (headless tools, the CLI and MCP). Anything one can do, the other must be
able to do — see AGENTS.md §3.

## The editing model

Each open document holds a `pdfcraft_cos::Document`: the object graph plus a copy-on-write overlay.

1. An edit runs **on a clone**. On success the previous state is pushed onto the undo stack. Clones
   share all unchanged data, so a snapshot is cheap — this is what makes 100 levels of undo
   affordable on a 200 MB file.
2. After every edit the **working file** is produced by an incremental write: the original bytes
   plus one appended revision.
3. The view is refreshed from those bytes, so **what you see is exactly what Save will write**.
4. Saving rebases onto the written bytes, so the next save appends only new edits.

Consequences worth preserving: the original bytes of an opened file are never rewritten in place; a
full rewrite happens only where it must (redaction, PDF/A conversion, print imposition, a changed
password); and unknown data survives a round trip because nothing is reconstructed that wasn't
understood.

## API

```rust
let mut s = Session::new();
let id = s.open("report.pdf", Some(path), bytes, None)?;   // -> DocId

s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 })?;
let label = s.undo(id)?;                                    // "Rotate pages"
s.redo(id)?;

let doc = s.get(id).ok_or("gone")?;                         // &Document: queries, never edits
doc.can_undo(); doc.editable(); doc.allows_printing();
doc.text_blocks(page); doc.page_boxes(page); doc.security_summary();

let bytes = s.save_bytes(id)?;        // incremental: keeps signatures over earlier revisions valid
let bytes = s.save_full_bytes(id)?;   // full rewrite, garbage-collected
s.mark_saved(id, bytes.clone(), path)?;
```

`Edit` is one enum of 75 variants — every undoable change in the product, from
`RotatePages` to `AddAnnotation`, `SetFieldValue`, `ApplyRedactions` and `AddWatermark`. Adding a
feature means adding a variant and handling it in `apply`, which is what keeps undo, the dirty
flag, autosave and the automation tools consistent for free.

`Session` also owns the operations that produce a *new* document rather than editing one:
`create_blank`, `create_from_images`, `create_from_text`, `combine`, `extract`, `split`,
`split_by_size`, `reduced_bytes`, `optimized_bytes`, `print_pdf`, `export_data`, and the signing
family (`sign`, `sign_with_timestamp`, `timestamp_document`, `embed_ltv`).

## Modules

| Module | What it holds |
|---|---|
| `commands` | The command registry: id, label, menu, shortcut, and a `Needs` precondition encoding document security. Menus, the ⌘K palette, shortcuts and the control channel all read it, so every surface agrees on what exists and when it is enabled |
| `catalog` | Acrobat's "All tools" information architecture as data — groups, sections, icons, hues, and an `Availability` of `Ready`, `Planned(milestone)` or `Provider`. The UI renders what is *not* built yet, honestly |
| `actions` | PDF action dispatch (go-to, URI, named, set-layer-state, JavaScript) |
| `links` | Link creation, properties, and finding URLs in page text |
| `js` | The bridge to `pdfcraft-js`. `forms` never depends on `js` directly; the engine injects it, so a build without JavaScript is possible |
| `xfa`, `ocr`, `compare`, `export` | Façades over the L3/L4 crates of the same names |

## Not done here

- **Rendering is still borrowed.** `pdfcraft-render` wraps `hayro` and inspects through `lopdf`
  (ADR-0004). The engine only sees `inspect() -> DocInfo`, `RenderPool` and `text::PageText`, and
  that API stays frozen while M2 replaces what is behind it.
- Performance budgets (`misc.performance-budgets`) are not set or enforced.
- Some shipped features are still reachable only from the UI, not through an `automation` tool;
  `cargo xtask parity` lists them; closing that gap is tracked in the execution plan (D4).
