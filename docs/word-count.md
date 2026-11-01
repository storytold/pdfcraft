# Word count

File ▸ Word count counts the current document, including unsaved edits. Select **Current page
only** to count just the displayed page. On desktop, counting runs in the background, one page
at a time. Closing the dialog or changing its options cancels the old count between pages.

Words use Unicode word boundaries (UAX #29); punctuation alone is not a word. Characters are
Unicode scalar values, not bytes or displayed glyphs. “Characters without spaces” removes all
Unicode whitespace. Characters include the line breaks and spaces reconstructed by text
extraction. The total is the sum of page counts, with no extra separator between pages.

The three independent exclusions are off by default:

- **Tagged tables:** Table, TR, TH, TD, THead, TBody and TFoot content.
- **Tagged code blocks:** Code content.
- **Image/OCR text:** Figure content and invisible text, including typical OCR layers. Invisible
  text can have other uses too. Visible OCR text without Figure tags is not identifiable as OCR.

Tags can be explicit marked-content tags or page-local MCIDs classified through the PDF's
structure tree, including parent roles and RoleMap aliases. These are semantic exclusions, not
visual table/code recognition. Untagged or unsupported structure stays in the count; the dialog
reports when structure classification is incomplete. On pages with Form XObjects, structure-tree
MCID exclusions are conservatively disabled because forms have separate MCID namespaces.
Explicit Table/Code/Figure marked content still works there. Cyclic and oversized structure
graphs are bounded and reported as incomplete.

PDF `/ActualText` replacements are respected. If one replacement spans both included and
excluded glyphs, there is no reliable mapping of its words to the selected content: counting
fails with an explanation instead of silently returning the wrong total. Disable exclusions to
count that page. Unreadable pages likewise fail the operation, without a misleading partial total.

Images alone contain no extractable words. Run Scan & OCR first when counting scanned pages.
Copy restrictions are respected. Counting never edits, rasterizes or saves the document, and
keeps only one page's extracted text at a time, plus the resulting page statistics.

## CLI and MCP

`text_word_count` is the shared read-only automation tool:

```json
{"tool":"text_word_count","args":{"doc":1,"pages":[1,3],"exclude_tables":true,"exclude_code":true,"exclude_images":false}}
```

Omit `pages` to count all pages. Page numbers are 1-based; duplicates count once. The result
contains `total`, per-page `pages` entries with `page` and `counts`, `pages_without_text`, and
`unclassified_structure`. Each count has `words`, `characters`, and `characters_without_spaces`.
`pages_without_text` is measured after exclusions, so it can also include pages whose entire
text was deliberately excluded.

The engine entry point is `pdfcraft_engine::word_count::count`. Its page indexes are 0-based;
its progress callback can cancel between pages. Frontends must enforce document copy
permissions before supplying an `ExportSource`.
