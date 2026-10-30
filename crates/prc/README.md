# pdfcraft-prc

A reader for PRC (ISO 14739-1), the 3D model format a PDF `/3D` stream of `/Subtype /PRC` carries (ISO 32000-2 §13.6.3).

- **Layer:** L0, standalone. No dependency on any other PdfCraft crate.
- **Licence:** MIT OR Apache-2.0. Inflation through `flate2` with the pure-Rust `miniz_oxide` backend.
- `#![forbid(unsafe_code)]`. Never panics on input: every read is bounded and returns `PrcError`.

## API

```rust
PrcFile::parse(data) -> Result<PrcFile, PrcError>                 // header, file structures, model file
PrcFile::parse_with_limit(data, max_inflated) -> Result<PrcFile, PrcError>
FileStructure::section(SectionKind) -> &[u8]                       // inflated globals / tree / tessellation / geometry / extra geometry
FileStructure::schema() -> Result<Schema, PrcError>                // the globals section's leading schema
FileStructure::tessellations() -> Result<Vec<Tessellation>, PrcError> // meshes, wires, markups, compressed meshes
Schema::read(&mut BitReader) / Schema::skip_added_fields(entity_type, &mut BitReader)
BitReader: bit, bits, character, unsigned_integer, integer, integer_vbn, double_vbn, nbits_then_unsigned, string, float_as_bytes, double
```

- `MAX_INFLATED_BYTES` (512 MiB) bounds every section together; `TooLarge` reports the limit hit.
- Schema programs (how a newer producer describes fields it added) run under a step ceiling (2^22) and a nesting ceiling (64).
- `Unsupported` names constructs the reader cannot step over (schema pointer fields, `Value_CurveIs3D`).
- `Tessellation::Mesh` is a `TriangleMesh`: positions, triangles, one triangle range per face, the stored normals and
  texture coordinates with each triangle's corner indices into them, and per-triangle line-attribute styles.
  `Wire` is polylines; `Markup` is read past.
- `Tessellation::Compressed` is read to its end so later entities decode; its triangles are not rebuilt yet.
- A face may carry at most 8 texture-coordinate sets; more is `Malformed`. Optimised vertex colours are `Unsupported`.
- Not yet in the crate: the product tree and rebuilding compressed meshes.
- The `Double` code table's rows come from [prc-rs](https://github.com/ralovich/prc-rs) (MIT); its notice is in `src/acof/table.rs`.
  A few reading choices also follow prc-rs; each is marked `// prc-rs:` in the source.
