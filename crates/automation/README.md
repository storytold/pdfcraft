# pdfcraft-automation

Agent control for PdfCraft: a headless tool table over the engine, and an opt-in MCP server.

- **Layer:** L7 (architecture §13), but headless. It depends on `engine`, `render` and `organize`, never on a UI toolkit.
- **Status:** over a hundred tools; `pdfcraft-cli tools` lists the current set with their schemas (the count isn't repeated here, so it can't go stale). They cover inspection, rendering, text, page edits and organizing, page labels, bookmarks, comments and their review, form filling and authoring (fields, properties, scripts, data exchange, detection), redaction and sanitizing, password protection and signatures, calibrated 2D measurements and CSV export, adding text, images, links and stamps, optimizing, OCR, printing, export, metadata, undo/redo, save, combine, extract and split. Comment tools take geometry in the same top-left-origin points as everything else, and `comment_add` can mark text by searching for it (`find`). The MCP server also serves the open documents as resources, page images included. The running app's UI is driven separately, through the `ui.*` control channel in `pdfcraft-ui-egui`.

## API

```rust
let mut a = Automation::new().with_root("/work")?;      // optional: confine all paths
let doc = a.call("doc_open", &json!({ "path": "in.pdf" }))?;   // Vec<Content>
a.write_output("page1.png", &png)?;                      // save a result yourself, under the same root rules
tools() -> Vec<ToolDef>                                  // name, title, description, input_schema, read_only, destructive, command
mcp::McpServer::new(a).serve(stdin, stdout)?             // newline-delimited JSON-RPC 2.0
```

`Content` is `Json(Value)` or `Png { data, width, height }`. Errors are `UnknownTool`, `InvalidArgs` (the call didn't match the schema) or `Failed` (a readable message for the agent).

## Conventions

- Tool names are `snake_case` (`page_rotate`): MCP clients reject dots. `ToolDef::command` links a tool to the registry id it automates (`page.rotate`), and `command_list` reports the link.
- Pages and positions are **1-based**. Rectangles are PDF points with the origin at the top-left of the displayed page.
- Unknown arguments are rejected, so typos fail loudly.
- Tools that change a document return its summary (`doc`, `pages`, `dirty`, `undo`, `redo`, …).
- `comment_image_preview` returns JSON and a PNG: `layer: "background"` (the default) gives the page without the selected image signature/initials; `layer: "image"` gives its embedded image with alpha. The metadata supplies its displayed rectangle, document rotation and annotation opacity. Each result can be saved with the CLI's `--out`. Cache the background and transform the image for live movement/resizing; `comment_edit` commits the final geometry in one undo step. Previewing never changes the document.
- For proportional image signature resizing, use the embedded image layer's width/height ratio rather than a previously stretched annotation rectangle. The GUI preserves this ratio at corners with the opposite corner anchored; edge midpoint handles change only their own axis. `comment_edit` accepts the resulting rectangle.
- `doc_close` refuses to drop unsaved changes unless `discard_changes: true`. `doc_save` writes atomically, incrementally in place, and in full for a new path.
- With a root set, every read and write path must resolve inside it (symlinks and `..` included). Relative paths resolve inside the root, and `..` is resolved by name before the check. Every path outside the root gets the same refusal (`<path> is outside the allowed directory <root>`), whether or not it exists, so a confined agent can't probe the rest of the disk; on Windows another network share or device path (`\\host\share`, `\\?\UNC\…`, `\\.\…`) is refused without being contacted. "Not found" and other filesystem errors are reported only for paths inside the root. Tools that write several files into a folder name them after the document or input file, with separators, colons and control characters replaced by `_`, so a name can't lead them out of the folder. The root itself must be a folder.

## MCP server

**Opt-in only.** Nothing starts it automatically, and it opens no port. It runs while `pdfcraft-cli mcp [--root DIR]` runs, normally launched by an agent from its MCP configuration. It stops when stdin closes. The CLI's `mcp` Cargo feature (on by default) compiles it out entirely when disabled.

`pdfcraft-cli mcp --compact` (or `McpServer::with_compact(true)`) keeps `tools/list` short for agents with small context budgets. It lists only the core tools (`mcp::COMPACT_CORE_TOOLS`: open, info, save, close, render, text extract and find, combine, split, undo) plus two meta tools: `tool_search` (optional `query` and `category`, the name prefix; or `name` for one tool's full `input_schema`) and `tool_call` (`name` and `arguments`, run through the same `Automation::call`). `tools/call` by name keeps working for every tool, and the server instructions mention the meta tools. Without the flag nothing changes.

Implemented: `initialize` (protocol 2025-06-18, 2025-03-26, 2024-11-05), `ping`, `tools/list` (with `readOnlyHint`/`destructiveHint` annotations), `tools/call` (JSON results also returned as `structuredContent`; images as `image/png`), and `resources/list`, `resources/templates/list` and `resources/read`. The resources expose the open documents read-only: `pdfcraft://doc/{doc}/info` (JSON), `…/text`, `…/page/{page}/text` and `…/page/{page}/image{?dpi}` (PNG, 1–600 dpi). Tool failures come back as `isError: true` results, so the agent can read and recover from them.

## Adding a tool

1. Add the engine capability first, with its tests (the tool is a thin adapter).
2. Add a `ToolDef` in `src/tools.rs`: a precise description, the schema, `ro()`/`destructive()`, and `cmd()` if a registry command exists.
3. Handle it in `Automation::call`, validating pages and positions with the existing helpers.
4. Add an end-to-end test in `tests/automation.rs`. `tool_table_is_well_formed` checks names, schemas and command links.
