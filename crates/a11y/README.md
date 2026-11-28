# pdfcraft-a11y

Layer L4: accessibility (execution plan M12.3). Today: the Accessibility Checker's full check.

```rust
let report = check(&doc, &Options::default());          // 31 of 32 rules (colour contrast off)
let html = report_html(&report, "file.pdf", "2026-10-02", "en", &in_english);  // or an interface's words
```

- **32 rules in 7 categories**, as in Acrobat's full check: Document (permission flag,
  image-only, tagged, reading order, language, title, bookmarks, colour contrast), Page Content
  (tagged content, tagged annotations, tab order, character encoding, tagged multimedia, and
  four manual checks), Forms (tagged fields, descriptions), Alternate Text (figures, nested,
  associated with content, hides annotation, other elements), Tables (rows, cells, headers,
  regularity with row/column spans, summary), Lists (items, Lbl/LBody) and Headings (nesting).
- Each result is Passed, Failed (with findings and pages), Needs manual check, or Skipped.
- Structure rules also report `tested`, the number of applicable structure elements inspected
  (including passing and failing elements). Zero distinguishes an empty check from an inspected
  pass; it does not establish accessibility. Other rules, manual checks and skipped rules use
  `None` (`null` in automation), not an invented count. Statuses and summary counters still count
  rules, including passes with zero inspected elements.
- **Content checks** walk the page content streams (and form XObjects) tracking marked
  content: painting outside MCID-marked or `/Artifact` content is untagged.
- **Character encoding** accepts a predefined non-Identity CJK CMap or an Adobe CJK
  character collection (`GB1`, `CNS1`, `Japan1`, `Korea1`, `KR`), including Identity-H/V.
  Unknown CMaps and foreign registries do not establish that route. This is a font-level
  declaration check, not validation of every ToUnicode entry or used glyph.
- **Structure checks** read the structure tree through the role map; table regularity counts
  `ColSpan`/`RowSpan` attributes.
- The report is a self-contained HTML page. Fixes that edit the document (language, title, tab
  order) live in the engine (`Document::accessibility_fix`).

Not yet: autotagging, the Tags/Order/Content panels, the Reading Order tool (M12.1–M12.2).

`tested` counts figures for `figures-alt-text`, formulas for `other-elements-alt-text`, and
elements with nonempty `/Alt` or `/ActualText` for the other alternate-text rules. Tables count
for headers, regularity and summary; `TR` elements for rows; `TH`/`TD` for cells; `LI` for list
items; `Lbl`/`LBody` for their parent check; and numbered headings for heading nesting. These are
the elements the existing bounded tree walk actually visits, after role mapping: repeated
indirect references count once, inline elements count as encountered, and elements beyond the
depth limit do not count. Counts are not truncated with findings. As with those rules' findings,
the `pages` option does not restrict structure coverage; it restricts page rules only.
