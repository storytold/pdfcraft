# MCP conventions

Start explicitly with `pdfcraft-cli mcp [--root DIR] [--compact] [--automation-read-root DIR] [--automation-write-root DIR]`. The server uses JSON lines on
stdin/stdout, opens no port, and stops on EOF. `--root` confines all reads and writes, including
commands invoked through the common tools. Logs stay on stderr.

Supply `--root` at most once, followed by a nonempty directory path. Missing operands,
repeated roots and another long option in place of the directory are refused before any
script is read or server is started. Prefix a directory name beginning with `--` with `./`.

## Confining file access

```sh
pdfcraft-cli mcp --automation-read-root /work/project --automation-write-root /work/project/out
```

```json
{"mcpServers": {"pdfcraft": {"command": "/abs/path/target/release/pdfcraft-cli",
  "args": ["mcp", "--automation-read-root", "/work/project", "--automation-write-root", "/work/project/out"]}}}
```

The two flags (VectorCraft, PhotoCraft and the other Craft apps take the same ones, so one client
config works across the suite) keep the agent to the folders you give it:

- Every file a tool reads must lie inside the **read root**: `doc_open`, `page_insert_file`,
  `page_replace`, `doc_combine` and `doc_create_multiple` inputs, `doc_create` images, stamp,
  picture, attachment, signature and form-image files, `doc_watermark` and `doc_background` files,
  `doc_import_data`, `form_merge_data`, the files `action_run` and `ocr_recognize_files` process,
  digital ID, graphic and certificate files for signing and `sign_trust`, `text_font_import`, and
  the folders `text_font_files` searches. Without `directories`, `text_font_files` searches the read
  root instead of the installed fonts.
- Every file a tool writes must lie inside the **write root**: `doc_save` (also saving in place),
  `page_extract`, `doc_combine`, `doc_create_multiple`, `doc_split`, `doc_reduce`, `doc_optimize`,
  `doc_print` to a file, the `doc_export_*` tools, `image_save`, `measure_export`,
  `comments_summarize`, the accessibility report and review record, `doc_compare_report`,
  `form_merge_data`, the output folders of `action_run` and `ocr_recognize_files`, `sign_id_create`
  and `sign_document`.
- Read and write access are separate. Give the same folder twice for both. A root left out grants
  none of its access: with only `--automation-read-root`, nothing can be written to disk, though
  tools that give their result back instead of a path still do. Without either flag nothing changes.
- A path is made absolute against the server's working directory, then its links are followed: a file
  (or folder) link that leads out of a root is outside it. For a file that doesn't exist yet, its
  nearest existing folder's links are followed and the rest must be plain names, so `..` below a
  folder that doesn't exist is refused, as is a link that leads nowhere (writing through it would
  create its target). The result is compared with the root by whole folder names: `/work2/x` is not
  inside `/work`. On Windows case is ignored, UNC paths are resolved like drive-letter ones, and
  device names (`CON`, `NUL`, `COM1`...) are refused.
- A refused path is an ordinary tool error the agent can read and correct, for example
  `automation path rejected: outside the write root /work/project/out: /etc/x.pdf` or
  `automation filesystem access is not granted: write authority is absent: /work/project/out/x.pdf`.
  Tools that handle several files report a refused one in its own entry.
- The roots must be existing folders; a missing one stops the server with an error. Each flag is
  given once (`--flag <dir>` or `--flag=<dir>`).
- `--root DIR` still works and can be combined with these flags. Both then apply: a path must be
  inside `--root` and inside the matching automation root.

`pdfcraft-cli mcp` always runs the session in its own process (there is no flag to connect it to a
running app), so the roots cover every server it starts. The desktop app is not confined by them.

What the roots do not cover: tools that take no path still read and change the open documents, and
`text_extract` and the other inspection tools return their content. Keychain and Windows
certificate-store identities for `sign_document` are not files and are not checked. Sending a job
to a printer is not a file write. No tool reaches the network on its own: `sign_document` sends one
RFC 3161 request (a hash of the signature value, never the document) only to the
`timestamp_server` it is given, and it is the one tool whose annotations say `openWorldHint: true`
(as does the compact mode's `tool_call`, which can run it). Every path a tool reads or writes goes through
`Automation::resolve`, and a unit test keeps the tool modules from touching the file system
without it.

A path is checked, then opened: another program that swaps a link in between can still win that
race, so the roots keep an agent to the files it was given rather than guarding against other
software on the computer.

## Tools

| Tool | Arguments |
|---|---|
| `command_list` | `doc?`, `filter?`, `enabled_only?`; returns `{commands: [...]}` with mapped tool schemas |
| `command_run` | `id`, `params?` (the mapped task tool's arguments, including `doc` when required) |
| `command_batch` | `steps: [{id, params?}]`, `stop_on_error?` (default true) |
| `doc_inspect` | `doc?`; one document's info or `{documents: [...]}` for the session |
| `render_preview` | `doc?`, `page?` (1-based, default 1), `max_side?` (1–4096, default 1024) |

All existing task tools remain available. Compact mode lists these five tools alongside its
existing core set and `tool_search` / `tool_call`. There are no legacy-name aliases. The full
[automation guide](../crates/automation/README.md) describes the tools, resources, coordinate
system, root confinement, and errors.

```text
command_run   {"id":"file.open","params":{"path":"input.pdf"}}
              → {"doc":1,...}
command_list  {"doc":1,"filter":"rotate"}
command_run   {"id":"page.rotate","params":{"doc":1,"degrees":90}}
render_preview {"doc":1,"page":1,"max_side":512}
```

Every tool has a title and the four MCP annotation hints. Unknown top-level argument keys return
JSON-RPC `-32602`; command parameter keys that do not belong to the mapped task tool are ignored
with warnings. Task failures and escaped tool panics return `isError: true`, and the server keeps
serving. A failed batch includes per-step errors and is marked `isError: true`.

Exports complete synchronously. A call's `progressToken` and cancellation notifications are
harmlessly ignored; the server does not provide background export progress or cancellation.

## Resources and protocol versions

`pdfcraft://document` returns the session's document information (as `doc_inspect` without
arguments), including an empty `documents` array before any PDF is opened. `pdfcraft://commands`
returns the command catalog (as `command_list`). Existing per-document info, text and page-image
resources and their templates are preserved.

Unknown top-level argument keys are refused with JSON-RPC `-32602` (invalid params) instead of
being ignored, so a misspelled argument is reported rather than silently dropped. Clients that
sent extra keys before get an error now.
