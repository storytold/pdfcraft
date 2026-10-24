# Web host bridge

The WebAssembly build (`apps/pdfcraft-web`) can run inside an `<iframe>` on a page that stores
files itself. The two talk with `postMessage`. Messages are plain objects with a `type`. Only a
parent window on the app's own origin is heard or answered; with a parent on another origin there
is no bridge and the app runs as if unframed (saves download).

| Message | Direction | Fields | Meaning |
|---|---|---|---|
| `pdfcraft:ready` | app to host | `version` | The app is listening; send files now. |
| `pdfcraft:open` | host to app | `name`, `bytes` (`ArrayBuffer` or `Uint8Array`) | Open a PDF in a tab, like a `?file=` URL. |
| `pdfcraft:open-refused` | app to host | `name`, `reason`, `size`, `limit`, `error` | An open was not done. Not fatal: the session keeps running. |
| `pdfcraft:save` | app to host | `name`, `saveAs`, `bytes` (transferred `ArrayBuffer`), optional `export` | Store this file. The host reports its own errors. |
| `pdfcraft:dirty` | app to host | `dirty` | Unsaved work appeared or went away. |
| `pdfcraft:failed` | app to host | `error` | The app couldn't start. Fatal. |

## Saves and exports

A save of the document (Save, Save As) sends `pdfcraft:save` with `saveAs` set as the user chose
and no `export` field. A successful save marks the document clean.

Every other file the app would download in a browser (exports such as images or text, reports,
printed PDFs) is sent as `pdfcraft:save` with `export: true` and `saveAs: true`. It is a new file,
not the open document: the host should ask where to put it (or store it alongside), never
overwrite the open document with it. An export does not mark the document clean.

## Refused opens

The only `reason` today is `"too-large"`: `size` (bytes) is over `limit`, which is 256 MiB
(`MAX_OPEN_BYTES` in `apps/pdfcraft-web/src/main.rs`). wasm32 has at most 4 GiB of linear memory
(browsers often grant less), and an opened document is held twice at once (the host's buffer
and the copy in wasm memory) before it is parsed, rendered and re-serialised in the same heap.
The check happens before the copy. Hosts should show `error` to the user and keep the session.

In host mode the parent guards leaving the page; the app shows no `beforeunload` prompt.
