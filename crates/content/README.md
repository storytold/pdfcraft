# pdfcraft-content

Layer L2: content streams (ISO 32000-2 §7.8.2, §8, §9).

```rust
let parsed = parse(&stream.decoded()?);       // operators with operands and source spans
for op in &parsed.ops { if op.is("Tj") { /* … */ } }
let bytes = serialize_ops(&parsed.ops);       // write back (inline images kept whole)
let m = Matrix::from_operands(&op.operands);  // cm / Tm arithmetic, invert, bbox
```

The parser is tolerant (unreadable bytes are skipped and counted, never fatal) and never
panics. `redact` and (later) `edit` build their interpreters on it; the graphics-state tracker
and the `Patch` API of architecture §6 grow here.

An inline image ends where the renderer's tokenizer (`hayro-syntax`) ends it, so the parser
and the renderer read the same operators after the image, and redaction removes what the page
actually draws. Only where that tokenizer reads no image (its parameters don't read, or no `EI`
passes its checks) does the parser locate `EI` by its own rules: unfiltered images with built-in
or Indexed colour spaces, and image masks, by their sample layout (including per-row byte
padding), other images by the tolerant delimiter heuristic. Such an image is counted in
`Parsed::ambiguous_images`, because the operators after it may not be the ones the page draws.
