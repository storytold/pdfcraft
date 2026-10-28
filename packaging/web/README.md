# Hosting PdfCraft for the web

`pdfcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `pdfcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `pdfcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `pdfcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `pdfcraft-ocr-worker-<hash>.js`, `_bg.wasm`, `_loader.js` | The text recognition worker (about 3 MB), started only when OCR runs |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |
| `models/` | The text recognition (OCR) models, about 12 MB, with their licences (CC-BY-SA-4.0) |

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

## Text recognition (OCR)

Scan & OCR ▸ Recognize Text downloads `models/text-detection.onnx` and
`models/text-recognition.onnx`, relative to the page, the first time it is opened in a visit.
Keep `models/` beside `index.html`. The app loads only the files whose SHA-256 matches
`ATTRIBUTION.toml`, so a wrong file fails like a missing one. Their file names carry no content
hash, so the app asks the server before using a cached copy (`no-cache`): the server must answer
that check correctly (an ETag, or a `Last-Modified` that changes whenever the file does). Without
the models, everything else works and the Recognize Text dialog says the download failed.

Pages are read in Web Workers (`pdfcraft-ocr-worker-<hash>_loader.js`, a module worker), so the
app keeps responding, and two pages are read at once on a machine with three or more cores. Each
worker needs about 600 MB while it reads; idle workers close after a minute. The workers need no
special headers, but a `Content-Security-Policy` must allow them (`worker-src 'self'`, or
`script-src 'self'`). Where they can't start, the app reads the pages itself, a page at a time,
and doesn't respond while each page is read.

From a source build, `trunk build --release` doesn't fetch the models. Add them with
`cargo xtask models dist/web/models` (after the build, which replaces `dist/web`).

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

The build uses WebAssembly SIMD (`.cargo/config.toml`), which reads a scanned page about 2.4
times faster. Every current browser has it (Chrome and Edge 91, Firefox 89, Safari 16.4 and
later). Older browsers can't load the app.
