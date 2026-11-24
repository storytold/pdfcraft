# SyncTeX: find a PDF position's LaTeX source, and a source line's PDF position

PdfCraft can read the SyncTeX data that LaTeX writes next to a PDF, in both directions.

- **Inverse search** (PDF to source): name a point on a page and get the source file, line and
  column that produced it.
- **Forward search** (source to PDF): name a line of a source file and get the page and the
  rectangles it produced.

Both are automation tools, for `pdfcraft-cli run` and the MCP server. The desktop app does not use
them yet: there is no click on the page that opens an editor, and no command an editor can run to
scroll PdfCraft to a line.

## Build with SyncTeX

Build with `-synctex=1` (or put `\synctex=1` in the source). TeX then writes `<name>.synctex.gz`,
or `<name>.synctex` with a negative number, beside the PDF. PdfCraft reads whichever of those two
sits beside the PDF, `.synctex.gz` first, or the file a call names with `synctex`. It reads the
file again after each rebuild.

## The tools

Positions are in points from the top-left of the displayed page, as `page_render` and `text_find`
report them.

```json
[
  {"tool":"doc_open","args":{"path":"main.pdf"}},
  {"tool":"synctex_inverse","args":{"doc":1,"page":1,"at":[120,125]}}
]
```

The result has the source `file`, `line` and `column`. TeX does not always record a column.

```json
[
  {"tool":"doc_open","args":{"path":"main.pdf"}},
  {"tool":"synctex_forward","args":{"doc":1,"file":"DIR/main.tex","line":3}}
]
```

The result has the 1-based `page` and the `rects` as `[x0, y0, x1, y1]`. `file` must be one of
the SyncTeX file's inputs; an error lists some. If the given line produced no output, such as a
blank line or a comment, PdfCraft uses the nearest line that did. Glue and kerns do not count as
output. `column` is accepted and ignored.

Run a script with `pdfcraft-cli run --script steps.json --root DIR`, where `DIR` is the folder
that holds `main.pdf`. The PDF is named from that folder. The source `file` is named as the SyncTeX
file names it, which is usually its full path.

When a lookup finds nothing, the error says why:

- "no SyncTeX file found; looked for ..." names the files it looked for and how to make one.
- "the SyncTeX file has no source text at that position" for a point on blank space.
- "... is not a source file of this document" lists some of the SyncTeX file's inputs.

## Limits

- Lookups assume page N of the PDF is sheet N of the build. After you reorder, delete or insert
  pages in PdfCraft, they can point to the wrong place until you rebuild.
- Only LaTeX output is supported: not DVI, Typst or ConTeXt.
