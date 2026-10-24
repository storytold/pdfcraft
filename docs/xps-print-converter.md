# Independent print conversion (experimental)

This branch converts a supported subset of XPS/OpenXPS packages directly into PDF through
PdfCraft's COS writer. It does not invoke Microsoft Print to PDF, OCR, or a page rasterizer.
The optional experimental combined installer adds an experimental Windows 11 24H2+ x64 native
queue. The Linux CUPS 2.x plugin instead preserves application-supplied PDF bytes.
See [printer setup and platform limits](../apps/pdfcraft-printer/README.md).

The engine entry point is `Session::create_from_xps(title, bytes)`. The shared CLI/MCP
tool is `doc_create` with `from: "xps"` and a package `path`. Save its returned document
with `doc_save`. For example, a CLI script under the allowed root can contain:

```json
[
  {"tool":"doc_create","args":{"from":"xps","path":"sample.xps"}},
  {"tool":"doc_save","args":{"doc":1,"path":"sample.pdf"}}
]
```

Run in a fresh session with `pdfcraft-cli run --root . --script convert.json`; the calls
also work individually through MCP using the returned document ID. XPS is not added to
GUI Open or batch conversion filters yet.

Supported content includes whole or interleaved OPC parts, UTF-8/UTF-16 fixed pages, affine transforms and path clipping;
left-to-right glyph runs with explicit positions/advances and an embedded TrueType font
(including GUID-obfuscated ODTTF); solid opaque RGB fills/strokes and M/L/H/V/C/Z paths;
and non-tiled absolute PNG/JPEG image brushes. Text receives `/ToUnicode` mappings for
searching and highlighting. Images keep their source pixels; JPEG data is retained.
Geometry is converted from XPS's 96 units/inch to PDF's 72 points/inch.

Unsupported constructs fail the whole conversion: complex glyph clusters, RTL/sideways
runs, simulated styles, gradients, transparency groups, resource brushes, image tiling,
brush transforms, relative brush units, arcs, quadratic paths and advanced stroke styles.
Unknown drawing elements and attributes are refused. Text already supplied as outlines
or pixels cannot become selectable without a separate recognition step. This converter
does not recover Word semantics, forms or accessibility structure.

Inputs are read without extraction or network access. Limits include 256 MiB input and
aggregate decoded package parts, 64 MiB per part/page stream, 20,000 ZIP entries, a 16 MiB
central directory, 100,000 XML nodes per part, 64 drawing levels, 1,000 pages and 500,000
glyphs per job (10,000 per run). Decoded image pixels are limited to 64 MiB per image and
256 MiB per job. ZIP64, multi-disk archives, DTDs and external part URIs are refused.
These are input/decode limits, not a global process-memory guarantee.

Tests generate fixtures using the repository's open Inter font. Unit checks cover Unicode
text, vectors, obfuscated fonts, UTF-16, image objects and malformed/unsupported packages.
The automation regression converts, finds and highlights words, saves/reopens, renders the
result and checks root confinement and unchanged document lists after failure. A private
local Word XPS sample and a captured native print job also retained searchable/highlightable text after save/reopen; their
proprietary source fonts and output are not repository fixtures.

Format references: [ECMA-388 OpenXPS](https://ecma-international.org/publications-and-standards/standards/ecma-388/)
and Microsoft's [Glyphs.Indices documentation](https://github.com/MicrosoftDocs/winapps-winrt-api/blob/docs/microsoft.ui.xaml.documents/glyphs_indices.md).
