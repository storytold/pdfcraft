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
- `comment_image_preview` returns JSON and a PNG: `layer: "background"` (the default) gives the page without the selected image signature/initials; `layer: "image"` gives its embedded image with alpha. The metadata supplies its displayed rectangle, document rotation, `image_rotation` (how far clockwise the image is shown turned: 0 for one placed by PdfCraft, which keeps it upright as displayed) and annotation opacity. Each result can be saved with the CLI's `--out`. Cache the background and transform the image for live movement/resizing; `comment_edit` commits the final geometry in one undo step. Previewing never changes the document.
- For proportional image signature resizing, use the embedded image layer's width/height ratio rather than a previously stretched annotation rectangle. The GUI preserves this ratio at corners with the opposite corner anchored; edge midpoint handles change only their own axis. `comment_edit` accepts the resulting rectangle.
- `doc_close` refuses to drop unsaved changes unless `discard_changes: true`. `doc_save` writes atomically, incrementally in place, and in full for a new path. Atomic output writes call `sync_all` on the staging file before replacing the destination. A write or sync error leaves the destination intact, removes staging and leaves the document dirty for retry. A filesystem that does not support syncing returns an error; there is no unsynced fallback. This flushes file contents, but does not guarantee persistence of the parent-directory rename across power loss.
- With a root set, every read and write path must resolve inside it (symlinks and `..` included). Relative paths resolve inside the root, and `..` is resolved by name before the check. Every path outside the root gets the same refusal (`<path> is outside the allowed directory <root>`), whether or not it exists, so a confined agent can't probe the rest of the disk; on Windows another network share or device path (`\\host\share`, `\\?\UNC\…`, `\\.\…`) is refused without being contacted. "Not found" and other filesystem errors are reported only for paths inside the root. Tools that write several files into a folder name them after the document or input file, with separators, colons and control characters replaced by `_`, so a name can't lead them out of the folder. The root itself must be a folder.
- `Automation::with_automation_roots` holds a read folder and a write folder (`pdfcraft-cli mcp --automation-read-root DIR --automation-write-root DIR`, see [docs/mcp.md](../../docs/mcp.md)). Reads must lie inside the read root and writes inside the write root, links followed. A root left out grants none of its access. Refusals read `automation path rejected: outside the read root <root>: <path>`. It applies on top of `with_root`.

## Text search and cache limits

`text_word_count` counts the current snapshot without filling the text cache. It returns totals
and per-page Unicode word/character counts, with optional tagged-table, tagged-code and
image/OCR exclusions. See [word count](../../docs/word-count.md) for counting rules and limits.

Extracted page text is reused in a session-wide LRU cache: at most **64 MiB** of estimated owned
text storage and **4,096 pages**, shared by all open documents. Accounting includes glyph-vector
capacity, each glyph string's capacity, line indexes and word-break flags. Cache hits share the
renderer result instead of copying glyphs. Oversized pages remain usable by the current call but
are not cached; evicted pages can be extracted again. A changed document version invalidates its
cached text on the next text request, and closing a document removes its entries. Cache entries
do not keep an old PDF byte buffer alive.

This bounds retained text-cache data, not total process memory: active extraction batches,
renderers, document buffers and the requested output have separate lifetimes. Search still reads
growing batches (8 to 128 pages), with one lookahead match to determine continuation.

`text_find` keeps matches from readable pages when another reached page fails extraction. It
returns `warnings` with 1-based `page`, `error` and `error_truncated`, plus `warning_count` and
`warnings_truncated`. At most 100 warnings and 1,024 UTF-8 bytes per error are returned. Failures
are retried on later calls; they are not cached as empty pages. Warnings describe pages visited in
search order, up to the continuation point, not failures in unvisited later pages.

`complete: true` means the requested search was exhausted with no page failures and no further
matches. `next: {start_page, skip}` exists only when another match would exceed `limit`. A search
that reaches the end with warnings therefore has `complete: false` **without** `next`. Callers
should follow `next` when present and separately inspect warnings. A capped search can return
both. The existing `query`, `count`, `matches` and rectangle semantics are unchanged.

`text_extract`, comment placement and redaction keep strict all-requested-pages-must-succeed
semantics; they never silently act on partial text.

## Shared field properties

`form_set_props` accepts either `field` (one name) or `fields` (1–1000 unique names):

```json
{"doc": 1, "fields": ["first_name", "last_name"], "required": true,
 "appearance": {"width": 3, "border": "#1473E6"}}
```

The batch is atomic and creates one undo step. A missing or locked field, invalid option,
unknown argument or malformed appearance refuses the whole edit. Set `locked: false` to
unlock and change fields together. Only supplied properties change: each field/widget keeps
its other colours, fonts, values and unknown PDF dictionary keys. `"none"` removes a border
or fill colour. Names, rectangles and rotations require the single-field form. Results keep
`field` for single edits and return `fields` for batches. CLI and MCP use the same tool.

## MCP server

**Opt-in only.** Nothing starts it automatically, and it opens no port. It runs while `pdfcraft-cli mcp [--root DIR]` runs, normally launched by an agent from its MCP configuration. It stops when stdin closes. The CLI's `mcp` Cargo feature (on by default) compiles it out entirely when disabled.

`pdfcraft-cli mcp --compact` (or `McpServer::with_compact(true)`) keeps `tools/list` short for agents with small context budgets. It lists only the core tools (`mcp::COMPACT_CORE_TOOLS`: open, info, save, close, render, text extract and find, combine, split, undo) plus the common command/inspection/preview tools and two meta tools: `tool_search` (optional `query` and `category`, the name prefix; or `name` for one tool's full `input_schema`) and `tool_call` (`name` and `arguments`, run through the same `Automation::call`). `tools/call` by name keeps working for every tool, and the server instructions mention the meta tools. Without the flag nothing changes.

Implemented: `initialize` (protocol 2025-06-18, 2025-03-26, 2024-11-05), `ping`, `tools/list` (with `readOnlyHint`/`destructiveHint` annotations), `tools/call` (JSON results also returned as `structuredContent`; images as `image/png`), and `resources/list`, `resources/templates/list` and `resources/read`. The resources expose the open documents read-only: `pdfcraft://doc/{doc}/info` (JSON), `…/text`, `…/page/{page}/text` and `…/page/{page}/image{?dpi}` (PNG, 1–600 dpi). Tool failures come back as `isError: true` results, so the agent can read and recover from them.

### From an installed release

The release packages ship `pdfcraft-cli` alongside the desktop app, so no build is needed:

| Install | CLI |
|---|---|
| Windows (MSI) | `C:\Program Files\PdfCraft\pdfcraft-cli.exe` by default (wherever you installed it otherwise), not on `PATH` |
| Linux (deb, rpm) | `/usr/bin/pdfcraft-cli` |
| macOS | the separate `pdfcraft-cli-<version>-macos-<arch>.zip` release asset (the `.app` holds only the desktop app) |

```sh
# Windows, default install folder
claude mcp add pdfcraft -- "C:\Program Files\PdfCraft\pdfcraft-cli.exe" mcp --root <dir>
# Linux, or macOS with the CLI unzipped onto PATH
claude mcp add pdfcraft -- pdfcraft-cli mcp --root <dir>
```

## Adding a tool

1. Add the engine capability first, with its tests (the tool is a thin adapter).
2. Add a `ToolDef` in `src/tools.rs`: a precise description, the schema, `ro()`/`destructive()`, and `cmd()` if a registry command exists.
3. Handle it in `Automation::call`, validating pages and positions with the existing helpers.
4. Add an end-to-end test in `tests/automation.rs`. `tool_table_is_well_formed` checks names, schemas and command links.


## Moving several page objects

`object_list {doc, page}` inventories recognised paragraphs, Image/Form artwork and added items
with generation-scoped kind/index references. `object_move {doc, page, objects, offset, generation?}`
moves 1–1000 references atomically in displayed points (right/down), preserving source glyph bytes
and fonts. A refusal changes nothing; success is one undo step. See
[the selection and movement guide](../../docs/editing-multiple-objects.md) for limits and examples.

## Accessibility review record

Open the original and edited PDFs as separate documents, then call
`accessibility_review_record {before, after, path}`. The tool writes a JSON record atomically within
`--root`: SHA-256 hashes of both working PDFs, rule statuses and finding counts before and after,
changed rules, and rules still failed or requiring manual review. `all: true` also includes the
colour-contrast rule. It does not change either PDF or certify accessibility. Keep the original open
while making edits to the second document so the comparison has a stable baseline.

## Common command tools

Following FilmCraft's MCP conventions, `command_list {doc?, filter?, enabled_only?}` keeps its
`{commands: [...]}` result and adds each mapped tool's parameter schema. `command_run {id, params?}`
runs that command through its existing headless tool: include `doc` and other tool arguments in
`params`. Commands without a headless mapping return a tool error. Unknown parameter keys are
ignored with a `warnings` array; required values and paths still go through the original validation.

`command_batch {steps: [{id, params?}], stop_on_error?}` returns `completed`, `failed`, and per-step
`{ok, result?, error?}` entries. The default is to stop at the first error; earlier edits remain.
MCP marks a batch with any failed steps as `isError`, including through compact `tool_call`.
`doc_inspect {doc?}` returns one document's info, or `{documents: [...]}` for all open documents.
`render_preview {doc?, page?, max_side?}` returns a PNG (page defaults to 1, maximum side defaults
to 1024, allowed 1–4096). Omit `doc` only when exactly one document is open.

All task tools remain available; compact mode includes the five common tools. Every MCP tool has
all four annotation hints. Unknown top-level argument keys return JSON-RPC `-32602` with the
accepted keys; failures in the invoked tool remain `isError` results. An escaped tool panic is
reported as an internal tool error, and the server continues serving.

Exports complete synchronously. `progressToken` and cancellation notifications are harmlessly
ignored; no background-job or cancellation behavior is added.

Session resources `pdfcraft://document` and `pdfcraft://commands`, and the refusal of unknown argument keys, are described in [docs/mcp.md](../../docs/mcp.md).

For inline editor previews, `page_render` accepts `hide_text_block` (a 1-based index from
`text_paragraphs`). It renders the page without that paragraph’s source glyphs and leaves the
working document unchanged.

`page_render.hide_added_text` does the same for an added text item's 1-based index from
`content_list` or `object_list`. It removes only that item in a read-only preview and is
mutually exclusive with `hide_text_block`.


## Existing text and document fonts

`text_font_files` discovers eligible installed files on request (or specified confined folders),
`text_font_import` registers a full document face, and `text_fonts` lists source/full resources.
Use `text_edit.font_resource` to choose a resource, or keep source faces and let matching full
faces complete absent subset glyphs. `text_edit.ranges` accepts disjoint UTF-8 byte ranges;
`text_edit_layout` measures the same resource/range draft without committing. All changes are
undoable and survive saving/reopening. `image_hit` shares the UI's curve and hole hit testing.
The bounds, limits and font requirements are in [existing-text editing](../../docs/editing-existing-text.md)
and [image editing](../../docs/editing-images.md).

## Opening local GoToR links

`link_open {doc, page, index, password?}` follows a local PDF link using `link_list`
numbering. Relative file specifications open normally. An **absolute** file specification
requires `allow_absolute: true`, after reviewing the target reported by `link_list`.
Omitting the argument or passing false refuses the request before checking or reading the
target file. This is an intentional compatibility change for callers that previously opened
absolute GoToR targets without explicit consent. Opting in does not bypass `--root`, symlink
confinement, regular-file, PDF-extension, size, or password validation. Consent applies only
to that call, including each password retry; it is never remembered for other links.

The desktop UI asks Allow/Cancel before following absolute GoToR targets and displays the
complete path. Relative links retain their existing behavior. Named destinations remain
unapplied with a warning.

## Folder batches

`doc_batch` runs conversion, OCR and compression over files and optionally recursive
folders. The CLI also exposes it as `pdfcraft-cli batch`. See the
[batch guide](../../docs/cli-batch.md) for examples, output protection, errors and limits.

## Viewer navigation history

A running PdfCraft instance launched with `--control FILE` exposes its per-tab history through
`pdfcraft-cli ui --control FILE command id=view.previous_view` and
`pdfcraft-cli ui --control FILE command id=view.next_view`. `commands` reports whether each action
is enabled for the active document. These are the same actions as the rail buttons, View → Page
navigation, and Ctrl+[ / Ctrl+] (⌘[ / ⌘] on macOS).

Explicit page jumps, including search-result selection and Next/Previous match, enter history
when the page changes. Automatic search refresh, continuous scrolling, and within-page scroll
movement do not. Existing page-step commands and single-page wheel turns retain their history
behavior. Back/Forward
returns to page tops, does not restore zoom, does not switch documents, and is not saved across
restarts. Standalone engine/MCP sessions have no viewport, so they do not have a separate
navigation-history tool; use the opt-in UI control channel to navigate the displayed document.

## Date locale preferences

`fill_sign_date_format` accepts `format: "auto"` to restore the date language's default and
returns `automatic` alongside the resolved `format`. Brazilian Portuguese (`language: "pt-br"`)
uses `dd/mm/yyyy` automatically; other locales retain the existing default for now. An explicit
pattern, including `m/d/yyyy`, is preserved when the language changes. The preference also
localizes new dynamic `comment_add` stamp by-lines in Brazilian Portuguese, using a 24-hour
clock; their date layout is independent of the Fill & Sign pattern.
