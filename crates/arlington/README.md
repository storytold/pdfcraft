# pdfcraft-arlington

Structural validation of a PDF's object graph against the
[Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model), the PDF
Association's machine-readable form of ISO 32000-2: every dictionary and array a PDF can hold, its
keys and their types, which are required, the values they may take, the PDF version each key
appeared in or was deprecated in, and whether a value must be direct or an indirect reference.

- **Layer:** L0, standalone. No other PdfCraft crate and no dependencies: the caller hands the
  document over through the `Source` trait, so this crate never parses a file. The engine adapts
  the editor's object layer (`crates/engine/src/structure.rs`), which means unsaved edits are
  validated too.
- `#![forbid(unsafe_code)]` (workspace) and the never-panic clippy deny list. Builds for wasm.

## API

```rust
let report = pdfcraft_arlington::validate(&source, &Options::default());
report.is_valid();                 // no errors
for f in &report.findings {        // errors first, then warnings, then information
    f.severity; f.rule;            // Error / Warning / Info; Required, Type, Value, Indirect, …
    f.object; f.page; f.rect;      // the indirect object, its page, an annotation's rectangle
    f.path;                        // trailer/Root/Pages/Kids/1/Annots/0
    f.message();                   // English; f.said(&words) through a translator (WORDINGS)
}
report.not_checked;                // rules written as predicates it does not evaluate, counted
// On a worker: told how many objects are done every few hundred; `false` stops it (None).
let report = pdfcraft_arlington::validate_with_progress(&source, &Options::default(), &mut |n| keep_going(n));
```

`MemSource` holds a document in memory, for tests and fuzzing.

## What is checked

The walk starts at the trailer (`FileTrailer`) and follows every key the model links, breadth
first, so a finding's path is the shortest one to it. For each object:

| Column | Check | Severity |
|---|---|---|
| Required | a required key or array element is missing (inherited attributes are looked up through `/Parent`, and a field's `/DA` and `/Q` in the form's document-wide defaults) | error |
| Type | the value's type is one the row allows; rectangles have four numbers, matrices six | error |
| Type (`date`, `string-ascii`) | the string is a date (ISO 32000-2 §7.9.4) / ASCII | warning / error |
| PossibleValues | the value is one of the listed values, or meets the `fn:Eval` rule; `fn:RequiredValue` | error |
| IndirectReference | `TRUE` / `fn:MustBeIndirect` values are references; `fn:MustBeDirect` ones are not | error |
| SinceVersion | the key, type or value is not newer than the file's PDF version (header, or the catalog's `/Version` when later); what a newer key leads to isn't reported again | warning |
| DeprecatedIn, `fn:Deprecated` | the key or value is not deprecated in the file's version | warning |
| SpecialCase | `fn:Eval(...)` rules hold | warning |
| Link | the object fits one of the types its key links to | warning when none fits |
| Key | the key is in the model (form fields and their merged widget share keys) | information |

Name and number trees are walked to their leaves, and the leaves checked against the tree's
linked types. Arrays with too many elements, and repeating arrays not in whole sets, are reported.
A repeating set with optional members (an attribute object, optionally followed by its revision
number) is matched in order, an optional member the element doesn't fit being left out; only a set
whose members are all required has to come whole.

**Which type a linked object is** is read off the object: among the candidates in the Link column,
the one whose fixed values (`/Type`, `/Subtype`, `/S`, an array's first elements) and
special-case rules the object meets, and whose keys it has. A candidate the object contradicts
(a required fixed value it doesn't have) is out however many of its keys the object has. When
every candidate is contradicted (an action with an `/S` no action type has), the object is
reported once as fitting none and is not checked further, rather than checked against a wrong type.

### Predicates

Evaluated: `fn:Eval`, `fn:IsRequired`, `fn:Not`, `fn:SinceVersion`, `fn:BeforeVersion`,
`fn:IsPDFVersion`, `fn:Deprecated`, `fn:IsPresent`, `fn:Contains`, `fn:ArrayLength`,
`fn:StringLength`, `fn:RectWidth`, `fn:RectHeight`, `fn:BitSet`, `fn:BitClear`, `fn:BitsSet`,
`fn:BitsClear`, `fn:FileSize`, `fn:NumberOfPages`, `fn:IsPDFTagged`, `fn:NotStandard14Font`,
`fn:IsHexString`, `fn:MustBeDirect`, `fn:MustBeIndirect`, `fn:RequiredValue`; comparisons and
`+ - * mod`; paths through `parent::` and `trailer::` (`trailer::Catalog` is `/Root`).

That is 25 of the 45 predicate kinds the model uses, and 81% of its predicate uses.

Evaluation is three-valued: a missing key or a value of the wrong kind leaves a rule undecided,
and it is neither passed nor failed. Every other predicate (`fn:InNameTree`, `fn:InKeyMap`,
`fn:PageProperty`, `fn:Extension`, `fn:Ignore`, `fn:IsMeaningful`, …) is counted in
`Report::not_checked` and never guessed. Keys introduced only under an extension
(`fn:Extension(…)` in SinceVersion) are not reported as unknown or as too new.

## Bounds

A seen-set per (object, type) stops cycles; direct nesting is followed 64 levels and trees 64
deep; arrays and dictionaries are checked to 50,000 entries; the walk stops after
`Options::max_objects` (200,000) objects and keeps `Options::max_findings` (1,000) findings per
severity while the totals go on counting. A property test feeds arbitrary cyclic, ill-typed graphs.

## The model data

`data/arlington.tsv` is the model's `tsv/latest` at the pinned commit (see `MODEL_COMMIT`), joined
into one file by `cargo xtask arlington --from <checkout>`, which refuses any other content (a
SHA-256 digest over every file, pinned in `xtask/src/arlington.rs`) and leaves out only the
header rows and the free-text Note column. The model is Apache-2.0 (PDF Association); the file is
attributed in `ATTRIBUTION.toml` and its notice is in `NOTICE`. Behaviour follows the model's
`INTERNAL_GRAMMAR.md`; the model repository's own validator was not consulted.

## Not done

- Unevaluated predicates (above), including all name-tree and key-map membership tests.
- Content streams (the model doesn't describe them), cross-reference streams and earlier trailers
  (the walk sees the merged trailer), and the linearization dictionary.
- Extensions are not tracked: a key or value only valid under an extension is not judged.
- A comparison with another Arlington implementation, run as an external process on a corpus.
  The pdf.js corpus has been run (see `core.arlington-validation` in `parity/`) and its false
  positives fixed, but nothing was compared with a second validator.
