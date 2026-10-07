# pdfcraft-a11y

Layer L4: accessibility (execution plan M12.3). Today: the Accessibility Checker's full check.

```rust
let report = check(&doc, &Options::default());          // 31 of 32 rules (colour contrast off)
let html = report_html(&report, "file.pdf", "2026-10-02");
```

- **32 rules in 7 categories**, as in Acrobat's full check: Document (permission flag,
  image-only, tagged, reading order, language, title, bookmarks, colour contrast), Page Content
  (tagged content, tagged annotations, tab order, character encoding, tagged multimedia, and
  four manual checks), Forms (tagged fields, descriptions), Alternate Text (figures, nested,
  associated with content, hides annotation, other elements), Tables (rows, cells, headers,
  regularity with row/column spans, summary), Lists (items, Lbl/LBody) and Headings (nesting).
- Each result is Passed, Failed (with findings and pages), Needs manual check, or Skipped.
- **Content checks** walk the page content streams (and form XObjects) tracking marked
  content: painting outside MCID-marked or `/Artifact` content is untagged.
- **Structure checks** read the structure tree through the role map; table regularity counts
  `ColSpan`/`RowSpan` attributes.
- The report is a self-contained HTML page. Fixes that edit the document (language, title, tab
  order) live in the engine (`Document::accessibility_fix`).

Not yet: autotagging, the Tags/Order/Content panels, the Reading Order tool (M12.1–M12.2).
