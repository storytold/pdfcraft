# PDF analysis reports

`hot_folder_analysis::analyze(bytes, pages, should_stop)` returns UTF-8 JSON with
`format: "pdfcraft-analysis"` and `version: 1`. It reads existing PDF text and
drawing instructions. It never runs OCR, document scripts, rendering or a save.
An empty page selection means all pages; otherwise zero-based page indexes retain
the caller's order. Duplicates and invalid indexes are rejected.

The document section records PDF version, byte size, page count, encryption,
standard Document Info text fields, XMP presence and parser repair notices. Each
selected page records media/crop boxes, normalized rotation, UserUnit, displayed
size in points, text blocks and image placements. Images inside Form XObjects and
inline images are included. No embedded image bytes are copied into the report.

Rectangles are `[x0, y0, x1, y1]` in unrotated PDF default user space, with the y
axis upward. Multiply by the page's `user_unit` for physical points; apply its
crop and rotation separately for displayed coordinates. Image matrices map an
image unit square into that same PDF coordinate system. Text font size is the
existing text-space size, not a measured raster size.

`content` distinguishes:

- `existing_text`: at least one nonempty text block was decoded, including an
  already existing hidden searchable text layer.
- `image_without_text`: image drawing instructions exist but no text was decoded.
  This is a possible scan, not proof that the image contains text.
- `no_extractable_text`: text/vector/shading instructions exist without decoded
  text. Missing font mappings are reported rather than silently treated as blank.
- `empty_content`: no analyzed painting/text instructions were found. Annotation
  appearances and dynamic forms are not included, so this does not certify that
  a rendered page is visually blank.

Text blocks follow existing content order, which may differ from human reading
order in complex layouts. Image rectangles are placement bounds before clipping,
masks, transparency or optional-layer visibility. The report does not infer
tables, detect semantic regions, parse XMP fields, or visually classify scanned
pages. These limitations are also included in the JSON itself.

The operation rejects reports over 16 MiB, decoded text over 8 MiB and more than
50,000 reported regions. Input is limited to 512 MiB and 10,000 pages; each selected
page permits 8 MiB expanded content, 2 MiB per stream, 100,000 operators, 256 form
visits, form depth 12 and graphics-state depth 256. Cycles and malformed content
return an error instead of publishing a silently truncated report. Extraction
permissions are honored; the API does not take a password to unlock protected
input. Cancellation is checked between traversal/extraction phases and content
operators; underlying parser/font operations have their existing bounded work.

Call this synchronous API from a worker. In a Hot Folder task, analysis-only mode
must suppress OCR and scan transformations before analysis. A report accompanying
a processed-PDF export describes those processed bytes, including any text layer
explicitly added by the task's earlier OCR step.

Validation: `cargo test -p pdfcraft-engine hot_folder_analysis --lib` covers Unicode
metadata, text/image/empty/vector pages, nested and inline image coordinates,
Rotate/UserUnit, deterministic output, selection order, unchanged input bytes,
cancellation, malformed input, cyclic forms and oversized streams. All fixtures
are generated from original test code without distributed document assets.
