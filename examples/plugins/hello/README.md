# Hello — example PdfCraft plug-in

One `.wasm` file, manifest embedded — the convention shared with
PhotoCraft and VectorCraft plug-ins. Build it with:

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

Then install `target/wasm32-unknown-unknown/release/pdfcraft_plugin_hello.wasm`
from PdfCraft's Plugins panel or Plugins menu, or drop the file into the
PdfCraft plug-ins folder.
