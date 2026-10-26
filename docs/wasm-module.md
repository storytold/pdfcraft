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

## Editing what a page shows

Boxes are `[x0, y0, x1, y1]` in the displayed page's points from its top-left, y down, the same
space as `view` above, so a page drawn at `scale` passes on what the person touched divided by `scale`.

```js
const objects = JSON.parse(pdf.objects(0)); // [{ kind, index, type, text, view }]: text, images and added items
pdf.move_objects(0, JSON.stringify([{ kind: "text", index: 0 }]), 12, 0); // by displayed points
pdf.resize_object(0, "added", 0, [20, 20, 160, 90]);
pdf.add_text(0, [20, 40, 180, 80], "Hello", JSON.stringify({ size: 18, bold: true, color: [0.8, 0, 0], align: "center" }));
pdf.add_image(0, "logo.png", pngBytes, [100, 200, 160, 260]); // or no box: centred at its natural size
pdf.set_text(0, "text", 2, undefined, JSON.stringify({ italic: true })); // restyle, keeping the words
pdf.delete_object(0, "added", 1);
pdf.cover(0, [10, 10, 80, 30]); // a white box over what is there (the content stays in the file)
```

## Reading and marking up

Same displayed coordinates.

```js
JSON.parse(pdf.find("invoice", false, false, 100)); // [{ page, text, rects }]: every match
JSON.parse(pdf.select_text(0, [20, 40], [180, 52])); // { text, rects } between two points
pdf.markup(0, "highlight", JSON.stringify(rects), [1, 0.9, 0]); // or "underline", "strikeout"
pdf.add_link(0, [20, 40, 120, 60], "https://example.com"); // or "page:3"
JSON.parse(pdf.links(0)); // [{ index, view, uri | page }]
JSON.parse(pdf.annotations(0)); // [{ index, subtype, view, contents }], pdf.delete_annotation(0, index)
JSON.parse(pdf.fields(0)); // [{ name, kind, value, view }]
pdf.set_field("name", JSON.stringify({ text: "Ada" })); // { check: true } or { choice: ["a"] }
```

`kind` is `text` (a paragraph the page already had), `image` (a picture it already had) or `added`
(something added with `add_text` or `add_image`).

An encrypted file throws an error that starts with `needs-password:` or `wrong-password:`, so the
page can ask and try again. Page and block indices are 0-based.

`Date.now()` stands in for the system clock, which wasm does not have.
