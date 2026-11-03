# Hosting PdfCraft for the web

`pdfcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `pdfcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `pdfcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `pdfcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/pdfcraft-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/pdfcraft/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

Add `?file=<URL-encoded document URL>` to open a PDF on startup. The document must be served
from the same origin or allow the app's origin through CORS. If the user starts working before
the download finishes, the downloaded file opens in a background tab; the current document,
workspace and Save target stay selected. Click the downloaded file's tab to switch to it.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. That takes the
  download from about 13 MB to about 5 MB. You can also precompress (`brotli -k *.wasm`) and let
  the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** PdfCraft doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /pdfcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/pdfcraft/"
  title="PdfCraft PDF workbench"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work. Preferences are kept in the iframe's `localStorage`. Browsers
  that partition or block third-party storage may forget them between visits, and the app
  then starts with defaults.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`. Without
  `allow-same-origin` there's no storage. Without `allow-downloads`, Save and Export (browser
  downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer selection and fallback flags

PdfCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |
| `?cpu` | Force the CPU canvas path (slowest, most compatible) |

For example: `<iframe src="https://example.com/pdfcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.

## Host mode: saving into the embedding page

A page on the same origin as PdfCraft can use it as its PDF editor, as the Nextcloud app does:
it hands documents in and takes saves back over `postMessage`, instead of the user downloading
the file. Add `?host=parent` to the iframe `src`. Messages are plain objects with a `type`.
PdfCraft hears only its parent window on its own origin, and posts only to it.

| Message | Direction | Fields |
|---|---|---|
| `pdfcraft:ready` | to the page | `version`. PdfCraft is listening: send `pdfcraft:init` (optional) and files now. |
| `pdfcraft:init` | from the page | `acknowledgeSaves`, `saveTimeoutMs`. Both optional, see below. |
| `pdfcraft:open` | from the page | `name`, `bytes` (an `ArrayBuffer` or `Uint8Array` holding a PDF). It opens in a tab. |
| `pdfcraft:save` | to the page | `name`, `saveAs` (`true` for Save As), `bytes` (a transferred `ArrayBuffer`), and `saveId` after `acknowledgeSaves`. |
| `pdfcraft:saved` | from the page | `saveId`: the file of that save is stored. |
| `pdfcraft:save-failed` | from the page | `saveId`, `error` (a string; the user sees it). |
| `pdfcraft:dirty` | to the page | `dirty`: whether any document has unsaved work. Sent when it changes. |
| `pdfcraft:failed` | to the page | `error`: PdfCraft could not start (no WebGL, for example). |

`?author=<name>` on the `src` signs the user's comments with the page's name for them, unless
they chose a name in PdfCraft.

### When a save counts

**Without `acknowledgeSaves`**, which is the original protocol and what existing embedders keep
getting, a save counts as done once `pdfcraft:save` is posted. The document is marked saved,
`pdfcraft:dirty` turns `false`, the browser stops warning before leaving, and Save in the "save
changes?" prompt closes the document at once. If the page then fails to store the file, or
isn't listening, the edits are lost when the document is closed. The page reports its own
errors.

**With `acknowledgeSaves: true`**, the page promises to answer every save, and PdfCraft waits
for the answer:

- The document keeps its unsaved work (`pdfcraft:dirty` stays `true`, and the browser still
  warns before leaving) until `pdfcraft:saved` names the save's `saveId`.
- Save in the "save changes?" prompt closes the document only then.
- `pdfcraft:save-failed`, or no answer within `saveTimeoutMs` milliseconds (120000 by
  default, kept between 1000 and 3600000), leaves the document open and unsaved, and PdfCraft
  says "Couldn't save *name*: *error*". The user can save again. A later answer for that save
  is ignored.
- If the user edits the document while the page stores it, the page has the earlier version:
  the newer edits stay unsaved, and the document doesn't close.
- An answer whose `saveId` no save is waiting for is ignored. Ids are strings, unique to this
  copy of PdfCraft: send them back unchanged, and don't answer a save of an earlier copy after
  the iframe reloads (it sends `pdfcraft:ready` again).
- Up to eight saves wait at once. Another save is refused with a notice until one is answered.

Send `pdfcraft:init` once, in answer to `pdfcraft:ready`. Older versions of PdfCraft ignore it
and send `pdfcraft:save` without a `saveId`: such a save counts once posted, as above.

```js
const frame = document.querySelector("iframe");
const post = (message) => frame.contentWindow.postMessage(message, location.origin);
window.addEventListener("message", async (event) => {
  if (event.origin !== location.origin || event.source !== frame.contentWindow) return;
  const message = event.data;
  if (message.type === "pdfcraft:ready") {
    post({ type: "pdfcraft:init", acknowledgeSaves: true, saveTimeoutMs: 300000 });
    post({ type: "pdfcraft:open", name: "report.pdf", bytes: await (await fetch("report.pdf")).arrayBuffer() });
  } else if (message.type === "pdfcraft:save") {
    try {
      await storeFile(message.name, message.bytes, message.saveAs); // your upload
      if (message.saveId !== undefined) post({ type: "pdfcraft:saved", saveId: message.saveId });
    } catch (error) {
      if (message.saveId !== undefined) post({ type: "pdfcraft:save-failed", saveId: message.saveId, error: String(error) });
    }
  }
});
```
