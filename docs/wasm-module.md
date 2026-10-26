# The engine as a wasm module (`apps/pdfcraft-wasm`)

`pdfcraft-web` is the whole application in a browser tab. `pdfcraft-wasm` is the other way to
use the engine on the web: a headless module, with no interface, for a page that has its own.
It opens a PDF from bytes, draws pages to RGBA pixels, lists and rewrites the text blocks,
moves, rotates, inserts and deletes pages, undoes and redoes, and saves.

The logic is in `src/core.rs` with plain Rust types, and is tested natively (`cargo test -p
pdfcraft-wasm`). `src/bindings.rs` (wasm32 only) converts to and from JS types and nothing else.

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo build -p pdfcraft-wasm --target wasm32-unknown-unknown --release
# the CLI version must match the wasm-bindgen version in Cargo.lock
wasm-bindgen --target web --out-dir out \
  target/wasm32-unknown-unknown/release/pdfcraft_wasm.wasm
```

The size is dominated by the engine's features, not by this crate: in the release build the Acrobat
JavaScript interpreter (boa and its parser) is about a quarter of the code.

## Use

```js
import init, { Pdf } from "./out/pdfcraft_wasm.js";
await init();

const pdf = new Pdf(bytes, undefined); // the second argument is a password, if any
const [width, height] = pdf.page_size(0); // points; empty if the page does not exist
const page = pdf.render(0, 2.0, undefined); // scale = device pixels per point
const pixels = page.take_rgba(); // premultiplied RGBA8, page.width x page.height

pdf.edit_text_block(0, 0, "New text");
const saved = pdf.save(); // incremental: the original bytes stay as a prefix
```

`pdf.text_blocks(page)` returns JSON. Each block has `rect` in PDF user space and `view`, the same box as
displayed (points from the top-left, y down, after the page's rotation), which is what a page drawn at
`scale` needs for hit testing and an overlay: multiply by `scale`.

An encrypted file throws an error that starts with `needs-password:` or `wrong-password:`, so the
page can ask and try again. Page and block indices are 0-based.

`Date.now()` stands in for the system clock, which wasm does not have.
