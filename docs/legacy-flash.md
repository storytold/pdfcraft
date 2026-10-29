# Blank Flash/SWF diagrams in older PDFs

Some older manuals put their diagrams in Flash (SWF) files embedded in a PDF, rather than drawing
them as page content. PdfCraft does not play Flash. A page can therefore look blank even when
its text and embedded diagram are still present. This is not a request to install Flash.

PdfCraft recognises legacy Flash configurations in RichMedia annotations, SWF asset names and
embedded streams labelled `application/x-shockwave-flash`. It shows a warning above the page,
including when the document also has a form or security notice. `doc_info` reports the same
warning and `has_legacy_flash` flag. Detection is limited to RichMedia annotations; other legacy
multimedia containers and undeclared Flash content may remain unidentified.

## Save an embedded diagram

1. Open the **Attachments** panel and select **Save attachment…** for the diagram, often named
   `diagram.swf` or similar. The desktop app asks where to save it; the web app downloads it.
2. Keep the original PDF. Saving an asset does not edit the PDF or activate its multimedia.
3. For a static replacement, ask the document's author or publisher for a PDF containing ordinary
   page diagrams. If you already have a trusted external viewer or conversion workflow, the saved
   asset can be its input. PdfCraft does not launch it, fetch external files, or provide a Flash
   runtime or converter. An asset may depend on other embedded files and may not work alone.

Automation can perform the same extraction. Call `doc_info`, then `attachment_save` with the
attachment's **1-based position** in its `attachments` array and an explicit output path:

```json
{"tool":"attachment_save","args":{"doc":1,"index":1,"path":"diagram.swf"}}
```

The output must pass the configured write-root checks and is written atomically. Password-protected
files use the password supplied when opening the document. External file references are refused;
embedded-file decoding has the existing 1 GiB limit. No content is opened or executed by extraction.

RichMedia inventory is bounded to 1,024 assets and 8,192 visited nodes, edges, configurations and
entries per document, with a name-tree depth limit of 32 and the shared attachment-text budget.
Extraction matches the listed asset’s raw name-tree key, so differently encoded keys with the
same displayed text cannot select a different file after a budget omission. Document-level
attachments preserve raw key identity too. Duplicate display keys still contribute Flash
indicators, while the listing retains the first matching asset. Deleting annotations refreshes
attachment positions, including through undo and redo.
When a work or asset limit is reached, `doc_info` warns that additional assets may be omitted.
A malformed or cyclic asset tree cannot cause an unbounded walk. These limits concern inventory,
not Flash playback: playback remains unsupported.
