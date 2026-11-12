# pdfcraft-xfa-rules

The rules a script's property change to an XFA form object must meet to be kept (XFA 3.3 ch. 3,
"Properties vs. Children", p. 78, and the elements of ch. 17): which property paths a change may
take, and which colours, measurements, whole numbers and sizes it may set.

- **Layer:** L0, standalone. Standard library only; no dependency on any other crate.
- **Used by:** `pdfcraft-xfa`, which checks a change before the form keeps it (and re-exports this
  crate as `pdfcraft_xfa::prop_rules`), and `pdfcraft-js`, whose XFA host checks a script's write
  before the script can read it back. One crate for both, so a script never reads back a value the
  form drops.
- `#![forbid(unsafe_code)]` through the workspace lints. Never panics: path steps, occurrences and
  values are bounded (`MAX_STEPS`, `MAX_INDEX`, `MAX_ITEM_INDEX`, `MAX_VALUE`), and colours and
  measurements that aren't finite numbers are refused.

## API

```rust
check(path, attr, value) -> Result<(), String>   // whether a change may set `value` at `path`
steps(path) -> Option<Vec<(&str, usize)>>         // `border#0/edge#1` as (tag, occurrence) steps
measure(s) -> Option<f64>                         // an XFA measurement, in points
color_components(s) -> Option<[f64; 3]>           // `r,g,b`, each 0-255
is_occurrence(key) -> bool                        // `occur#0@initial|min|max`, kept by the subform
```
