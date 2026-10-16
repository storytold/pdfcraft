# pdfcraft-fonts

Layer L2. Font helpers for the appearance streams PdfCraft generates (comments, form fields).

Today it holds what generated appearances need without a font program:

- `helvetica_width`: the **exact** standard-14 advance widths, via `Std14::Helvetica`. It was an
  estimate by character class until the generated tables landed; those tables are Adobe-authored
  non-visual technical data, the item `AGENTS.md` §1.1 permits, carried in `std14_metrics.rs` with
  `adobe_data = true` in `ATTRIBUTION.toml`. **No font program or typeface from any vendor is
  bundled** — widths and encodings only. Reach for `Std14` directly for any other face.
- `wrap`: greedy line breaking with that measure (paragraphs on newlines, long words split).
- `win_ansi`: Unicode → WinAnsiEncoding bytes (`?` for characters it can't represent), and
  `literal` to write bytes as a PDF literal string. Nothing writes that `?` silently. Callers
  check `first_undrawable` (or `UnicodeCMap::first_unencodable`, for a character beyond U+FFFF in
  UCS-2), then either refuse with `undrawable_message` (one wording that names the character and
  what can be written instead) where the `?` would be permanent page content, or, for a form
  value or a comment's text, which is stored as typed, draw the `?` and mark the annotation with
  `set_undrawn_mark` (`UNDRAWN_MARK`, read back by `undrawn_mark`) so the edit is reported and
  flattening refuses. `script_missing` names what the typed-signature script font and its Inter fallback both lack.

It also holds the fonts of the optional [craft-fonts](https://github.com/storytold/craft-fonts)
build input. `build.rs` embeds them as `CRAFT_FONTS` when the build sets
`CRAFT_FONTS_DIR=<checkout>` (and fails if it can't while `CRAFT_FONTS_REQUIRED=1`, as releases
set). Without that variable `CRAFT_FONTS` is empty and everything below copes:

- `ui_japanese_fonts`: the `Jpan` faces for the interface, BIZ UDPGothic first.
- `ui_chinese_fonts`: the `Hans` faces for the interface, in manifest order (without one,
  Chinese characters the Japanese faces lack show the replacement glyph).
- `ui_cjk_fonts(prefer_hans)`: both in fallback order for the UI language (Chinese group first
  in Chinese mode, so one line never mixes faces with different vertical metrics).
- `ui_arabic_fonts`: the `Arab` faces for Arabic-script interface text (file names, document
  titles), in manifest order.
- `ui_telugu_fonts`: the `Telu` faces for Telugu interface text (the Telugu catalog, file names,
  document titles), in manifest order.
- `ui_hebrew_fonts` / `document_hebrew_font(bold)`: the `Hebr` faces (Noto Sans Hebrew) for
  Hebrew interface text, and the face whose outlines become the Type 3 fallback for Hebrew text
  written by Edit text (Bold when asked for and present, else Regular). `None` without a
  craft-fonts input that has one; the editor then reports a clear error.
- `shape_hebrew` / `hebrew_has`: the Regular `Hebr` face shaped with harfrust for Hebrew Add
  Text content, one cluster per letter with its points and cantillation marks (positioned by the
  face), drawn by the same Type 3 writer as Arabic and Thai. `GlyphError::NoFont` without the face.
- `document_japanese_font` / `japanese_glyph`: the face (Shippori Mincho, then BIZ UDMincho) whose
  outlines become the Type 3 fallback font for Japanese text written into PDFs. Without it,
  `japanese_glyph` returns `GlyphError::NoFont` and the editor reports a clear error.
- `document_japanese_font_for_style` / `japanese_glyph_from`: select and draw a real Japanese
  fallback face for edited text. Sans sources prefer BIZ UDPGothic Regular/Bold; serif sources
  keep Mincho. Type 3 paths and advances both come from that selected face. Missing weights
  fall back to a real Regular face, never synthetic bold; no italic face is supplied. The
  default `document_japanese_font` / `japanese_glyph` APIs still use regular document Mincho.
- `document_arabic_font` / `shape_arabic` / `arabic_glyph`: the first `Arab` face, shaped with
  harfrust (joining forms, ligatures, mark positions) into clusters for Arabic text written into
  PDFs; the editor draws each cluster as one Type 3 glyph with a ToUnicode entry and per-line ActualText for logical copy/search order. Without the face
  they return `GlyphError::NoFont` and the editor reports a clear error.
- `document_japanese_fonts_for_style`: every `Jpan` face in that order of preference, so the
  editor can use the first one that has all of the replacement's glyphs (the faces differ in
  coverage, e.g. of Cyrillic).
- `ui_thai_fonts` / `document_thai_font` / `shape_thai`: the `Thai` faces for the interface
  and shaped Thai content. Harfrust positions stacked vowel/tone marks and decomposes
  Sara Am; cluster ToUnicode entries preserve the original text. The shared Type 3 writer
  (`shaped_text`) embeds outlines, so reopening does not depend on installed fonts: Add text,
  text boxes, Fill & Sign text, form fields, headers, footers and watermarks all write with it
  (`shaped_text::appearance_lines` for appearances, whose glyph streams `store_streams` moves into
  the document). `wrap_with` never breaks a line before a Thai mark.
- `craft_glyph(face, ch)` and `face_has(bytes, ch)`: bounded outlines and glyph coverage for any bundled face.
- `SHIPPORI_MINCHO`: Shippori Mincho's bytes, or `None`.
- `TextPlan::for_words`: how the OCR output writes text beyond WinAnsi — per word the first
  face (document face first) whose glyphs cover it, planned once per page into Type 3 fonts
  (codes, advances, `?` stand-ins) that `pdfcraft-edit` registers and the `pdfcraft-ocr` layers
  draw with; characters no face has are counted (`missing`) so the output reports them.

Generated Japanese Type 3 fonts record the fallback face's family and style in an indirect
`FontDescriptor`. `pdf::Metrics::base_font` uses its `FontName` when the Type 3 dictionary has
no `BaseFont`, so text inspection and edit-tool results identify the fallback after save/reopen.
The descriptor does not change glyph programs or replace the Type 3 `FontBBox` metrics when
`Ascent`/`Descent` are omitted (PDF Reference 1.7, tables 5.9 and 5.19). The default small web input has
only Gothic Regular, so it cannot preserve bold. No new font assets or changes to that input are
needed.

By default, wasm32 builds embed only BIZ UDPGothic Regular and any `Arab`, `Telu`, `Hebr`
`Thai` or `Deva` face, keeping deployment sizes smaller. The optional Simplified Chinese
manifest is native-only, so it does not supply Chinese fonts to the web build.

Set `CRAFT_FONTS_WEB=all` to embed every otherwise permitted font listed in the configured
`CRAFT_FONTS_DIR/fonts/manifest.txt`. For example, on Linux:

```bash
CRAFT_FONTS_DIR=/path/to/craft-fonts \
CRAFT_FONTS_WEB=all \
cargo build --release \
  --target wasm32-unknown-unknown \
  -p pdfcraft-web
```

This is a **build-time** option, not runtime font loading: it does not fetch a served manifest
or persist fonts uploaded in a browser session. Unset keeps the existing default selection;
any value other than `all` (including an empty value) is a build error. Valid settings do not
change native Linux, Windows or macOS font selection, optional-input handling, or
`CRAFT_FONTS_REQUIRED` behavior. All modes keep the prohibited-family exclusions in AGENTS.md
§1.1 (Source Han/Serif/Sans and Noto CJK). The build filter does not audit arbitrary fonts'
origins or licenses: custom inputs must comply with the entire asset policy.

Embedding additional fonts increases WASM output size. Self-hosted deployments must verify
their own hosting limits and font licenses, including any required notices.
Font files are never
committed here (`AGENTS.md` §1.4).

The full font subsystem (parsing, shaping, subsetting and embedding) arrives with M2.2/M7.


Document font inputs use `embedding`: bounded static TrueType/OpenType-CFF validation,
exact program preservation, Unicode/CID/GID mapping and full-face subset completion.
`shaping` uses the same program with harfrust and UAX #9; source UI fallback fonts are
never exported. See [existing-text editing](../../docs/editing-existing-text.md).

`pdf::embedded_sfnt_within` reconstructs a missing Unicode cmap for Identity-H TrueType
subsets from the PDF's ToUnicode and CID-to-glyph maps, and completes a THSarabunPSK subset's
Latin-only one with them. Only the in-memory editor copy changes;
the PDF's font program stays intact. `pdf::LegacyThaiFont` shapes verified THSarabunPSK
positional glyphs into their original PDF codes, retaining the source face and weight. Missing
contextual glyphs are refused. `pdf::recovered_thai_map` recognizes narrowly fingerprinted
damaged exporter maps; callers must record repairs and apply them to page-owned font copies.
