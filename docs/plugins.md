# Plug-ins

PdfCraft runs plug-ins: small WebAssembly modules that work on an open document through the same
tools as `pdfcraft-cli run` and the MCP server. They run in the sandbox of the shared
[`craft-plugin`](https://github.com/storytold/craft-libs/tree/main/crates/plugin) crate (RFC-0046):
a metered interpreter with a memory cap and no file system, network, clock or randomness. Its
README describes the manifest (`craft-plugin.json`), the `.craftplugin` bundle and the ABI a
plug-in is written against.

A plug-in for PdfCraft names `pdfcraft` in its manifest's `apps` (command plug-ins, API 1–6).
Manifests that name no app, and plug-ins of other kinds, are refused.

## Using plug-ins

In the desktop app, **Plug-ins ▸ Manage plug-ins…** opens the Plug-ins window:

- **Installed**: each plug-in with *Check for update* and *Remove*, and the plug-in folders that
  couldn't be read or hold a plug-in for another app.
- **Install**: from a file (`.craftplugin`), from a GitHub repository (`owner/repo` or
  `owner/repo@tag`; the release must publish the bundle's SHA-256 in `SHA256SUMS`), or from a
  catalog (an `index.json` URL, which is remembered).

Nothing is installed before a confirmation that shows the plug-in's version, author, licence, kind,
source, SHA-256 and what it may do (read, or read and change the document; the tools it asks for).
It says so plainly when the plug-in comes from a file or from a repository that no catalog in use
lists, and when an update asks for different permissions.

Each installed plug-in's commands are in the **Plug-ins** menu. A command with parameters opens a
form generated from its manifest; it then runs on the active document, told which pages are
selected. Panels (`ui_show`) aren't shown yet: PdfCraft says so instead.

Plug-ins are kept in the settings folder's `Plug-ins` folder, with their settings (at most 64 KB
each, keys up to 128 characters) and the catalogs in `plugin-settings.json` beside it. The web
build has no plug-ins yet.

## One undo step, and nothing left behind

A run is **one undo step** named after the plug-in ("Undo Plug-in: Page tools"), however many edits
it makes (at most 99). A run that fails — a trap, an exhausted instruction budget, a refusal it
passes on, a non-zero status — leaves the document, its undo and redo history and the plug-in's
settings exactly as they were. The engine sets the document's editing state aside before the run
(`Session::edit_mark`) and folds or restores it afterwards (`fold_edits`, `restore_mark`).

## What a plug-in may call

A plug-in calls automation tools by name (`page_rotate`) or by the registry command a tool automates
(`page.rotate`). Every call is checked, in order:

1. the tool is on PdfCraft's allowlist (`ALLOWED` in `crates/automation/src/plugins.rs`); a tool
   added later is refused until it is reviewed;
2. it is in the manifest's `permissions.commands` (exact ids or `prefix*`; empty means any allowed
   tool);
3. no parameter that names a file or folder (`path`, `file`, `image`, `out`…) holds text;
4. the call is pinned to the document the run was started on: `doc` is filled in, another
   document's id is refused;
5. a plug-in with `read` access can't run a tool that changes the document, and a call that
   changed it anyway is put back and refused.

Allowed: reading the document (info, text, fonts, bookmarks, fields, comments, links, measurements,
accessibility and PDF/A checks, audits), and changing it through undoable edits: pages, page labels
and boxes, document properties, bookmarks, form fields and their values, redaction *marks*, added
text and text edits, links, measurements, comments, headers, footers, watermarks, backgrounds,
printer marks, colour and hairline fixes, and accessibility tags.

Refused, and why:

| Refused | Why |
|---|---|
| Opening, saving, closing, exporting, combining, splitting, extracting; inserting or replacing pages from a file; importing or exporting data, bookmarks or comments; batches; every report written to a file | A plug-in never gets a path: the person chooses what is written where |
| Other documents and new ones (`doc_list`, `doc_compare*`, `doc_create`), the object and comment clipboards | A run works on its own document |
| Undo and redo, `command_run`, `command_batch`, other plug-ins (`plugin_*`) | The run is one undo step; no nesting |
| Applying redactions and sanitizing (`redact_apply`, `doc_remove_hidden`) | They remove content for good once saved: a plug-in may mark, the person applies |
| Passwords and security, signatures, certificates, Fill & Sign, attachments | Security and identity are the person's |
| JavaScript (`js_*`, link and form scripts and actions) | A plug-in is already code; a document's own scripts run only as filling a form runs them |
| OCR and its dictionaries, page enhancement, rendering to images, printing, standards conversions, reducing and optimizing | Downloads, long work over pixels, devices |
| Preferences and view state (`js_enabled`, reading colours, layer visibility, hiding comments, the Fill & Sign date format) | The person's settings |

## Headless: tools, CLI and MCP

| Tool | Does |
|---|---|
| `plugin_list` | Installed plug-ins (manifest, kind, SHA-256, source), problems, catalogs, whether they are kept in a folder and can be downloaded |
| `plugin_install` | `path` \| `data` (base64) \| `repo` (`owner/repo[@tag]`) \| `catalog` + `id` (+ `version`); `review: true` describes the bundle and keeps it, and the install that follows passes the review's `sha256` |
| `plugin_update` | Whether `id` has a newer release, reviewed, with the `plugin_install` arguments that install it |
| `plugin_remove` | Removes `id` and its settings |
| `plugin_run` | Runs `id`'s `command` with `params` on `doc` (default: the only open one), with an optional `selection` (1-based pages) and lower `fuel` |
| `plugin_catalog_list` | The packages of the catalogs added (or of `url`) that this PdfCraft runs |
| `plugin_catalog_add` | Remembers a catalog URL |

`pdfcraft-cli run` and `pdfcraft-cli mcp` take `--plugins DIR` to use the plug-ins installed in
`DIR/Plug-ins` (the desktop app's settings folder); without it, plug-ins are installed for that run
only. The CLI has no HTTPS client: it installs from files and `file://` catalogs. Paths honour
`--root` and the automation roots, and a confined session reads no `file://` catalog.
