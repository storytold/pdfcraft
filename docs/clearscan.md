# ClearScan

> **Last reviewed:** 2026-10-31 | **Last updated:** 2026-10-31 | **Change:** minor (Recognize Text ▸ Editable text and images runs ClearScan)

ClearScan replaces the lettering on a scanned page with resolution-independent outlines.
The page image loses that lettering, and the outlines are drawn over it as a Type 3 font
with a `/ToUnicode` map, so the text stays selectable and searchable. Photographs, rules
and other non-text marks stay in the image. Comments, links and bookmarks on the page stay.

On the desktop, Scan & OCR ▸ Recognize text ▸ In this file ▸ Output ▸ Editable text and
images runs the same step. Sensitivity (0–100) and downsample (1–8) appear only for that
output. The run is a worker thread with stage progress and Cancel; pages already built are
kept, as one undo step named ClearScan. In multiple files stays Searchable Image (Exact).

Headless, run it with `pdfcraft-cli run` or the MCP server (`pdfcraft-cli mcp`), which share
the automation tool table.

```sh
pdfcraft-cli run --root . --script clearscan.json
```

`clearscan.json`:

```json
[
  { "tool": "doc_open", "args": { "path": "scan.pdf" } },
  {
    "tool": "ocr_clearscan",
    "args": {
      "doc": 1,
      "pages": [1],
      "dpi": 300,
      "sensitivity": 50,
      "downsample": 1,
      "language": "en",
      "skip_text_pages": true
    }
  }
]
```

The result is JSON. `pages` is one object per page (1-based): recognised `text` when the
document allows copying, `glyphs`, `clusters` and `erased`, or `skipped` / `error`.
`progress` is the stage log (`render`, `preprocess`, `recognize`, `components`, `align`,
`cluster`, `vectorize`, `compose`, `done`). The call is synchronous, so the log arrives
with the result. Successful pages are one undoable step named ClearScan.

| Argument | Meaning |
|---|---|
| `pages` | 1-based pages. Omit it to read every page. |
| `dpi` | Render resolution, 72–600. Default 300. A scan is not enlarged past its own pixels. |
| `sensitivity` | Binarizer, 0–100. 0 keeps only the darkest ink. 100 picks up faint strokes. 50 is the ordinary paper setting (Sauvola `k` = 0.34). |
| `downsample` | Integer box shrink after rendering, 1–8. Default 1. The page keeps its size in points. |
| `language` | Hint. `en` only. The models read the Latin alphabet without accents. |
| `skip_text_pages` | Default true. A page that already has text is left alone. |

A page is refused, with a message, when the image is empty, the dimensions overflow, or
the models are not installed (`ocr_status`). A panic in the pipeline is reported on that
page and does not discard the other pages. The models are required only when a page is
actually read; a document whose pages already contain text does not load them.

## What the page becomes

The scan is turned to greyscale, deskewed, and binarized (Sauvola on a page-sized image,
Otsu on a small one). Connected components separate glyphs from speckles, rules and
photographs. Glyphs are lined up with the recognised characters, clustered by shape, and
averaged on their centroids. Each master is traced to cubic Bézier curves and stored as an
uncolored Type 3 glyph (`d1`, `m`/`l`/`c`/`h`/`f`). A simple font holds 255 drawing glyphs;
further shapes use `F2`, `F3`, … up to 16 fonts. Past that, extra clusters are counted and
left out.

The background is the deskewed scan with each glyph, plus a one-pixel halo, painted in the
paper colour sampled around it. A photograph or rule that overlaps the halo stays. Grey,
and colour whose channels match, is Flate DeviceGray. A flat tint is Flate DeviceRGB. A
page whose colour varies like a photograph is JPEG. There is no JBIG2 encoder in this
tree (the usual one is AGPL), so a monochrome scan stays Flate.

The content stream draws that image, then fills the Type 3 glyphs in black at each
occurrence's baseline.

## Clean-room

No Adobe font, sample or icon is used. The stages are the published methods they name:
Otsu (1979), Sauvola and Pietikäinen (2000), Suzuki and Abe border tracing (1985),
Douglas–Peucker, and Schneider's cubic fit (1990) via `kurbo`. Recognition is the `ocrs`
engine (MIT/Apache-2.0) with its CC-BY-SA models. Nothing is taken from potrace, autotrace
or any other copyleft program.

## Still missing

Edit text does not open these Type 3 glyphs as an editor session. A textured background
under a letter becomes one flat paper colour. Only the Latin models are available. Quality
is not claimed to match Acrobat.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-31 | minor | Recognize Text ▸ Editable text and images runs ClearScan on a worker, with progress, cancel and one undo step. |
| 2026-10-31 | minor | Headless ClearScan: `ocr_clearscan` on the CLI and MCP, one undoable step. |
