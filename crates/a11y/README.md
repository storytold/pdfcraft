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

- **PDF/UA verification** (`pdfua::verify`): every failure condition of the Matterhorn Protocol
  1.1 for PDF/UA-1 (ISO 14289-1), each Passed or Failed (with the page, tag or object), Needs
  manual check (the 47 a person must judge) or Not checked (the four machine conditions not
  checked yet, and any a check couldn't decide for the file). Several are the checker's own rules
  read as conditions; the rest are PDF/UA's own (metadata and identification, role mapping to
  PDF 1.7 types, languages, annotations, optional content, embedded files…). The font conditions
  (checkpoint 31, and 17-003 for formulas) are in `pdfua/fonts.rs`: each font the content uses
  is held up against its CMap, its ToUnicode map, its embedded program (read through
  `pdfcraft_fonts::program`) and the codes the content shows with it. PDF/UA-2 in part: its
  identification, PDF 2.0 and the conditions it keeps unchanged.

Not yet: autotagging, the Tags/Order/Content panels, the Reading Order tool (M12.1–M12.2).
