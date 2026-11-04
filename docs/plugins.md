# Plug-ins: the WebAssembly plug-in API (ABI v1)

PdfCraft does not host native Acrobat plug-ins: that needs unsafe FFI into
arbitrary machine code and cannot work in the browser build. Instead,
plug-ins are **WebAssembly modules** run in a sandbox. One `.wasm` file
works on macOS, Windows, Linux and the web.

This follows the plug-in conventions of the other Craft apps — PhotoCraft
(`docs/plugins.md` there) and VectorCraft, which itself follows PhotoCraft:
**one `.wasm` file per plug-in, the manifest embedded in the module, the
same manifest fields, the same `wasmi` sandbox and limits.** There is no
package format and no PdfCraft-specific file extension to learn. Where
the siblings declare a `filter` (PhotoCraft) or `effect` (VectorCraft)
kind, PdfCraft declares `command` plug-ins: document tools that receive a
JSON context and answer with a message and host-interpreted actions.

This page is the contract for plug-in authors and the reference for the
host (`crates/plugin`, the SDK in `crates/plugin-sdk`).

## Using plug-ins

In the app: **Menu ▸ Plugins ▸ Manage plugins…** installs a `.wasm` file,
lists the installed plug-ins and enables, disables or removes them
(without needing a document open); installed plug-ins also appear in the
Plugins menu and the Plugins side panel, one item per declared command.
Dropping a `.wasm` file into the plug-ins folder works too: every
`*.wasm` there is loaded at startup (not recursive), like the sibling
apps' plug-ins folder.

The folder is the per-user data folder's `Plugins` directory
(`~/Library/Application Support/PdfCraft/Plugins` on macOS,
`%LOCALAPPDATA%/PdfCraft/Plugins` on Windows,
`$XDG_DATA_HOME/pdfcraft/plugins` on Linux; `PdfCraftData/Plugins` in
portable mode).

Plug-ins are installed per user, not per document, and survive PdfCraft
updates as long as their ABI version is supported; an incompatible
plug-in is listed with the reason and never loaded.

## The sandbox

The host is [`wasmi`](https://github.com/wasmi-labs/wasmi), a pure-Rust
WebAssembly interpreter. Limits follow the siblings: no host imports (no
WASI, file system, network, clock), modules up to 32 MiB, an instruction
(fuel) budget per call plus a per-input-byte allowance, and a fresh
instance for every run, so no state survives between calls. Traps,
exhausted budgets and malformed answers become error messages; the
document is left untouched.

## ABI v1

Same shape as the siblings (`pc_*` in PhotoCraft, `vc_*` in
VectorCraft), with the `pdf_` prefix. All integers are `i32` unless
noted; pointers are offsets into the module's memory.

| Export | Signature | Meaning |
|---|---|---|
| `memory` | memory | The module's linear memory. |
| `pdf_abi_version` | `() -> i32` | Must return `1`. |
| `pdf_manifest` | `() -> i64` | `(len << 32) \| ptr` of the UTF-8 manifest JSON. |
| `pdf_alloc` | `(size) -> ptr` | A block of `size` bytes the host may write, or `0` on failure. |
| `pdf_run` | `(input, input_len, params, params_len) -> i64` | Runs one command; `(len << 32) \| ptr` of the output JSON, or a negative error code. |

### Manifest

Common fields with the siblings, plus PdfCraft's `commands` extension:

```json
{
  "id": "org.example.hello",
  "name": "Hello",
  "version": "0.1.0",
  "kind": "command",
  "author": "Example",
  "description": "What it does.",
  "commands": [
    {"id": "greet", "label": "Greet", "description": "Say hello to the document."}
  ]
}
```

- `id`: 1–64 characters from `A-Z a-z 0-9 . _ -` (reverse-DNS
  recommended). Installing a module with the same id replaces the old one.
- `kind`: `"command"` (the only kind in v1).

### Input and output

`pdf_run` receives `{"command": "…", "context": {…}}` as its input JSON
(the `params` argument is reserved for the parameter schema the siblings
share; v1 passes `{}`). The context currently carries `document.name`,
`document.page_count`, `document.current_page` and the current page's
`width`/`height`.

The output JSON is `{"message": "…", "actions": […]}`. Actions in v1:
`show_message`, `set_document_info` (`{key, value}`) and `open_document`
(`{name, data_base64}`). Guests never touch the document directly; the
host applies actions through its ordinary, undoable paths.

## Writing a plug-in in Rust

`examples/plugins/hello` is a complete, generic example built on
`pdfcraft-plugin-sdk`:

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
# install the produced .wasm from PdfCraft's Plugins panel or menu
```

Any language that targets core WebAssembly works, as long as the module
has no imports and exports the ABI above.

## Not in v1

Panel / UI plug-ins, plug-in-provided file formats, host callbacks
(progress, cancel), and the shared `params` dialog schema. Those belong
in later ABI versions, designed with the same compatibility rules.
