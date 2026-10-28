# Hosting PdfCraft for the web

`pdfcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a static site in `pdfcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `manifest.webmanifest` | Relative install metadata and the PDF file-handler declaration. |
| `icons/256x256/`, `icons/512x512/` | PNG icons referenced by the manifest. |
| `pdfcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `pdfcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code.
Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/pdfcraft-web/Trunk.toml`), so the site works at a domain root (`https://example.com/`), under a prefix (`https://example.com/tools/pdfcraft/`) and from a CDN bucket.
The asset names carry a content hash, so they can be cached forever.
Revalidate `index.html` and `manifest.webmanifest`.
The manifest derives its identity from the prefix-local `start_url`, so separate hosting paths remain separate installed apps.

Add `?file=<URL-encoded document URL>` to open a PDF on startup.
The document must be served from the same origin or allow the app's origin through CORS.
If the user starts working before the download finishes, the downloaded file opens in a background tab; the current document, workspace and Save target stay selected.
Click the downloaded file's tab to switch to it.

## Open PDFs with the installed web app

The package includes a relative web manifest that declares an in-scope handler for PDF files.
On desktop Chrome and Edge versions that support installed web apps and file handlers, users may install PdfCraft through the browser and then choose it from the browser or operating system's Open With flow for a PDF.
Availability and prompts vary by browser and operating system.
The current source has been checked with Chrome 156 on macOS; Windows association UI and Edge require their own acceptance run.
The declaration does not change the user's default PDF application.

Opening a file through this handler requires a browser-granted read permission.
PdfCraft reads the granted file bytes into a new document and does not treat the launch handle as a filesystem path or a write-back destination.
The original file remains unchanged, and Save continues to download a file through the browser.

A launch accepts files up to 64 MiB each.
The startup queue accepts at most 8 files and 128 MiB in total, with at most 16 file handles pending browser reads.
At most 8 individual launch failures are retained before one overflow notice is used.
These limits apply to this file-launch intake path, not to the renderer's total memory use or other ways of opening documents.
Browsers without the file-launch API continue to use the ordinary web app and its existing startup URL behavior.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`.
  Browsers refuse to stream-compile it under any other type, and the app then loads slowly or not at all.
  Serve `.js` as `text/javascript` and `.webmanifest` as `application/manifest+json` so browsers can read the install and file-handler metadata.
  Most hosts already serve the `.wasm` and `.js` types correctly.
  For nginx, check that `mime.types` has `application/wasm wasm;` and `application/manifest+json webmanifest;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`.
  That takes the download from about 13 MB to about 5 MB.
  You can also precompress (`brotli -k *.wasm`) and let the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and `.js` files, and `no-cache` on `index.html` and `manifest.webmanifest` so deployments can update the file-handler declaration and its relative asset references.
- **HTTPS:** installation, file handling and clipboard access require a secure context, such as HTTPS or loopback HTTP (`http://localhost`).
- **No special isolation headers:** PdfCraft doesn't use `SharedArrayBuffer`, so it doesn't need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`.
  If your site already sends COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin` when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /pdfcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; application/manifest+json webmanifest; image/png png; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* (index\.html|manifest\.webmanifest)$ { add_header Cache-Control "no-cache"; }
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
- **Cross-origin embeds** work.
  Preferences are kept in the iframe's `localStorage`.
  Browsers that partition or block third-party storage may forget them between visits, and the app then starts with defaults.
- **Sandboxed iframes** need at least `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`.
  Without `allow-same-origin` there's no storage.
  Without `allow-downloads`, Save and Export (browser downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Browser rendering

The current web entry point uses the Glow renderer and needs browser WebGL support.
The `?file=`, `?host=parent` and `?author=` parameters configure document startup and embedding.
The earlier `?webgl` and `?cpu` renderer flags are not read by the current entry point.
