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
