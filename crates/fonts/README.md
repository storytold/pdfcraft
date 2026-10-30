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
  and shaped Thai Add Text content. Harfrust positions stacked vowel/tone marks and decomposes
  Sara Am; cluster ToUnicode entries preserve the original text. The shared Type 3 writer
  embeds outlines, so reopening does not depend on installed fonts.
- `SHIPPORI_MINCHO`: Shippori Mincho's bytes, or `None`.

Generated Japanese Type 3 fonts record the fallback face's family and style in an indirect
`FontDescriptor`. `pdf::Metrics::base_font` uses its `FontName` when the Type 3 dictionary has
no `BaseFont`, so text inspection and edit-tool results identify the fallback after save/reopen.
The descriptor does not change glyph programs or replace the Type 3 `FontBBox` metrics when
`Ascent`/`Descent` are omitted (PDF Reference 1.7, tables 5.9 and 5.19). The small web input has
only Gothic Regular, so it cannot preserve bold. No new font assets or changes to that input are
needed.

wasm32 builds embed only BIZ UDPGothic Regular and any `Arab`, `Telu`, `Hebr` or `Thai` face, to keep the web build small: the web build
currently has no Chinese face, so Chinese there still shows the replacement glyph.
Font files are never
committed here (`AGENTS.md` §1.4; team members: [craftrules `standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md), internal).

The full font subsystem (parsing, shaping, subsetting and embedding) arrives with M2.2/M7.

`appearance_text` lays out Arabic/Hebrew/Thai mixed with WinAnsi text for annotation appearances.
It returns document-independent Type 3 font templates (outlines and ToUnicode) and per-line
ActualText. `materialize_fonts` promotes their glyph and CMap streams to indirect objects when
the annotation installs an appearance, so cached templates also work in fresh display copies.
`wrap_logical` preserves whitespace and extended graphemes, using exponential/binary probing
to bound repeated measurement of long words. Joining controls reach shaping with their letters;
font coverage does not require cmap glyphs for Unicode default ignorables.


Document font inputs use `embedding`: bounded static TrueType/OpenType-CFF validation,
exact program preservation, Unicode/CID/GID mapping and full-face subset completion.
`shaping` uses the same program with harfrust and UAX #9; source UI fallback fonts are
never exported. See [existing-text editing](../../docs/editing-existing-text.md).
