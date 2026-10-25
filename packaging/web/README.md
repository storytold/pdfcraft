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

## Same-document host mode (no iframe)

A host application can also run PdfCraft from its own document: it loads the wasm-bindgen module
itself and gives PdfCraft one of its own canvases, so there is no iframe and no `?host=parent`.
It opts in with `data-host="page"` on that canvas — the exact value `page` — and the canvas
keeps the id the app looks up (`id="pdfcraft"`).

The page and the app then talk with DOM `CustomEvent`s on the canvas instead of `postMessage`.
The app dispatches the same messages it would post to a parent — `pdfcraft:ready`,
`pdfcraft:save`, `pdfcraft:dirty`, `pdfcraft:failed` — with the message fields (without the
`type`) as the event's `detail`, and it opens a file when the page dispatches a `pdfcraft:open`
event with `detail: { name, bytes }`. Events do not bubble, so listen on the canvas itself.

Add the listeners before `init`: the app announces itself with `pdfcraft:ready` while `init` is
still running, and a file sent in reply would otherwise be missed.

```js
import init from "./pdfcraft-web-<hash>.js"; // the wasm-bindgen glue shipped with the app

const canvas = document.getElementById("pdfcraft");
canvas.dataset.host = "page";             // or <canvas id="pdfcraft" data-host="page">

canvas.addEventListener("pdfcraft:ready", () => {
  // The app is listening; hand it a document now, if you have one.
  canvas.dispatchEvent(new CustomEvent("pdfcraft:open", {
    detail: { name: "handout.pdf", bytes: pdfBytes },   // ArrayBuffer or Uint8Array
  }));
});
canvas.addEventListener("pdfcraft:save", (event) => {
  const { name, saveAs, bytes } = event.detail;         // bytes is an ArrayBuffer
  // Store the file under `name`; `saveAs` is true for Save As. Report your own errors.
});
canvas.addEventListener("pdfcraft:dirty", (event) => {
  unsaved = event.detail.dirty;                         // warn before leaving, as you like
});
canvas.addEventListener("pdfcraft:failed", (event) => {
  showError(event.detail.error);                        // the app couldn't start
});

await init({ module_or_path: "./pdfcraft-web-<hash>_bg.wasm" });
```

- Saves are not downloads in this mode: the `bytes` in `pdfcraft:save` hold the saved PDF, and
  the host decides where the file goes.
- `?author=<name>` still signs the user's comments, as with `?host=parent`.
- An unframed page that uses `?host=parent` keeps the normal app with downloads; only
  `data-host="page"` turns this mode on.

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
