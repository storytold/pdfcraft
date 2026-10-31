# pdfcraft-a11y

Layer L4: accessibility (execution plan M12). The Accessibility Checker's full check, alternate
text, auto-tagging of text-only files, and the tagging edits behind the Tags panel and the Reading
Order tool.

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

## Tagging (`tagging`)

The Tags panel's structural edits and the Reading Order tool, on any tag tree:

```rust
let page = page_tagging(&doc, 0)?;                        // units, annotations, numbered regions
tag_selection(&mut doc, 0, &Selection::Units(vec![0]), "H1", None, None)?;
content_to_artifact(&mut doc, 0, &Selection::Rect([0.0, 0.0, 612.0, 40.0]))?;
move_tag_to(&mut doc, tag, Destination::Before(other))?;  // any parent
delete_tag(&mut doc, tag, /* keep_contents */ true)?;
tag_to_artifact(&mut doc, tag)?;                          // the whole subtree
assert!(structure_problems(&doc).is_empty());
```

- A page's content is read as **units**: a tagged or artifact marked-content sequence, or one
  object drawn outside both (a text-showing operator, a painted path, an image or form XObject,
  an inline image, a shading), with its box in user space.
- Rewrites **splice** marked-content operators in at operator boundaries and copy every other
  byte, so pages draw exactly as before; a content stream another page shares is copied, not
  changed.
- The **parent tree** is read and extended in any number-tree layout (flat `/Nums`, `/Kids` with
  `/Limits`, indirect arrays, a direct root); every edit keeps `/P`, parent-tree entries, MCIDs
  and `/ParentTreeNextKey` consistent, and new types must be standard or role-mapped.
- Edits work on a copy and refuse what they can't do safely (a tag listed twice or stored
  inline, content in a form XObject, nested tagged content, a page whose content can't be
  decoded exactly), changing nothing.

Not yet: automatic tagging of mixed content, the table editor, artifact types.
