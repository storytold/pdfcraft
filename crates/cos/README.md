# pdfcraft-cos

The PDF object layer (ISO 32000-2 §7): the COS object graph, parsed lazily and tolerantly, edited
through a copy-on-write overlay, and written back incrementally or as a full rewrite.

- **Layer:** L1, standalone. The layering table lets it use only `pdfcraft-filters`,
  `pdfcraft-crypt` and `pdfcraft-geom` (`xtask/src/layers.rs::STANDALONE_DEPS`); it currently uses
  the first two. That keeps the parser and the crypto independently auditable and reusable.
- `#![forbid(unsafe_code)]`, and the crate denies `unwrap`/`expect`/`panic!`/`todo!`. Builds for
  `wasm32-unknown-unknown`.

**The object graph is the document model.** Nothing is lifted into a tidier representation and
written back out, because that is how data gets silently dropped. Typed views live above, in
`pdfcraft-model`.

## API

```rust
let doc = Document::open(bytes)?;                               // Arc<Vec<u8>>
let doc = Document::open_with_password(bytes, Some("owner"))?;  // encrypted files

let root = doc.root();                               // Option<ObjRef> — the catalog
let obj  = doc.get(ObjRef { num: 12, gen: 0 });      // load (and cache): Arc<Object>
let real = doc.resolve(&obj);                        // follow one indirect reference
let d    = doc.dict(&obj);                           // Option<Dict>, resolving first
let data = stream.decoded()?;                        // filters applied on demand

doc.set(r, Object::Dict(d));                         // edit: lands in the overlay
let r = doc.add(obj);                                // append a new object
doc.free(r);

let bytes = write_incremental(&doc, &SaveOptions::default())?;  // append a revision
let bytes = write_full(&doc, &SaveOptions::default())?;         // garbage-collected rewrite
```

Both writers **return the bytes**; the caller decides where they go, which is what lets the engine
produce a working file in memory after every edit and keep saving atomic.

Useful alongside: `doc.repair_log()` (what was reconstructed), `doc.revisions()` and
`doc.revision_ends()` (the incremental history), `doc.is_edited(num)` and `doc.modified_objects()`,
`doc.permissions()` and `doc.security()`, `doc.set_encryption()` / `doc.remove_encryption()`, and
`doc.require_full_save()` for edits that cannot be expressed incrementally.

`Document` is **cheap to clone**: the original bytes, the xref index and the parse caches are
shared, so a clone is a snapshot. Undo/redo and background jobs hold clones while the UI edits
another. Edits never touch the original bytes.

## Reading

- **Every cross-reference form:** classic tables with their quirky subsections, cross-reference
  streams, hybrid-reference files (`/XRefStm`), and object streams.
- **Repair.** A file whose xref is wrong or missing is reconstructed by scanning for `obj`
  headers. Repairs are *recorded*, never silent, so the UI can say the file was damaged.
- **Tolerant, never fatal.** The lexer works on a byte slice and a position; it allocates nothing
  for skipped data, bounds-checks every index, and caps nesting depth. Malformed input yields
  `CosError`, never a panic — this crate is the first thing a hostile file meets.
- **Lazy.** Objects are parsed on first use and cached. (Whole-file buffering is still the case;
  `ByteSource` streaming is `core.lazy-loading`, not yet done.)
- **Encryption** is handled on load and save through `pdfcraft-crypt`: every standard-security
  revision R2–R6 (RC4, AES-128/256), crypt filters, `/EFF` embedded-file encryption, and
  `/EncryptMetadata`. `security.rs` knows what the spec exempts — the `/Encrypt` dictionary, xref
  streams, the trailer `/ID`, and objects inside an object stream (the stream is encrypted whole).

## Writing

`write_incremental` appends only the edited objects, a cross-reference section **in the same
style as the file's last one**, and a trailer with `/Prev`. The original bytes are never modified,
which is what keeps a signature over an earlier revision valid.

`write_full` writes only what the trailer can reach, renumbered densely — the Save As and
garbage-collecting path. By default (`SaveOptions::object_streams`) it packs non-stream objects
into compressed object streams with a cross-reference stream (PDF 1.5+); turning that off writes a
classic table any reader accepts.

Dictionaries keep their key order and streams keep their *encoded* bytes, so rewritten objects stay
recognisable in a diff and byte-stable when re-serialized.

## Errors

`CosError`: `NotPdf`, `Syntax { offset, detail }`, `MissingObject`, `NotADictionary`, `Filter`,
`NeedsPassword`, `WrongPassword`, `Security`, `Poisoned`.

## Not done here

- Streaming very large files within a memory budget (`ByteSource`, `core.lazy-loading`); files are
  read whole today.
- Linearized output, Fast Web View (`core.linearization`, M11).
- Structural validation against the Arlington model (`core.arlington-validation`), which will live
  in the reserved `arlington` crate.
- Image codecs. `pdfcraft-filters` carries the non-image filters; DCT, JPX, JBIG2 and CCITT are
  still decoded through the bootstrap renderer (`core.image-filters`, M2.6).
