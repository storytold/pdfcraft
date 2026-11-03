# pdfcraft-testkit

Synthetic PDF fixtures: **one way to write the bytes, so tests differ only where they mean to.**

- **Layer:** test support (`Class::Testkit` in `xtask/src/layers.rs`). A dev-dependency only, never
  compiled into the product.
- **No workspace dependencies**, deliberately. The crates at the bottom of the layering (`cos`,
  `geom`, `filters`, `crypt`, `arlington`) can use it in their own tests without a cycle, which is
  also why it hands back bytes rather than a parsed `Document`.

## Why it exists

55 files hand-rolled the same serialiser — number the objects, record each one's offset, emit a
classic cross-reference table and a trailer — **111 times over**. The object *bodies* are properly
the business of the crate under test. The envelope around them is not, and every copy is a place
for a fixture to be quietly wrong in a way that makes a test prove less than it appears to.

## API

```rust
use pdfcraft_testkit::Pdf;

let bytes = Pdf::new()
    .version("1.4")                                        // default 1.7
    .obj("<< /Type /Catalog /Pages 2 0 R >>")              // becomes 1 0 R
    .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
    .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>")
    .stream("", b"BT /F1 12 Tf (hi) Tj ET")                // /Length computed
    .trailer_entry("/Info 5 0 R")
    .build();
```

Objects are numbered from 1 in the order they are added, all at generation 0; the trailer always
names `1 0 R` as `/Root` and carries the right `/Size`. `next_obj()` gives the number the next
object will get, for forward references. `stream_obj(dict, data)` builds a stream body on its own,
for callers that assemble their object list first.

## Byte-identical by construction

`the_builder_is_byte_identical_to_the_serialiser_it_replaces` keeps a verbatim copy of the
hand-rolled serialiser and asserts the builder agrees with it, across versions and trailer
variations. Migrating a crate must not change a single byte of its fixtures — otherwise the test
that fixture feeds silently starts testing something else.

The one intended difference: an empty stream dictionary gives `<< /Length N >>` rather than the
doubled space `<<  /Length N >>` that some copies produced. Parsers accept both.

## Migration status

Three crates are migrated (`cos`, `measure`, `redact`), taking the count from 111 copies in 55
files to **108 in 53**. The rest are follow-ups, deliberately not done in one sweep: `ui-egui`
alone accounts for 27 of those files and is the most contested area in the repository, so a single
large mechanical change there would rot before it landed.

Not every copy should move. A test that is *about* a malformed or unusual cross-reference table
needs to write its own bytes; this builder is for the fixtures that are merely scaffolding for
something else.
