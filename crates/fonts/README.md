# pdfcraft-fonts

Layer L2. Font helpers for the appearance streams PdfCraft generates (comments, form fields).

Today it holds what generated appearances need without a font program:

- `helvetica_width`: an approximation of Helvetica's proportions by character class. No metrics
  file or font program from any vendor is bundled; widths are PdfCraft's own estimates, good
  enough for line breaking and alignment, not for typesetting.
- `wrap`: greedy line breaking with that measure (paragraphs on newlines, long words split).
- `win_ansi`: Unicode → WinAnsiEncoding bytes (`?` for characters it can't represent), and
  `literal` to write bytes as a PDF literal string.

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
- `document_japanese_font` / `japanese_glyph`: the face (Shippori Mincho, then BIZ UDMincho) whose
  outlines become the Type 3 fallback font for Japanese text written into PDFs. Without it,
  `japanese_glyph` returns `GlyphError::NoFont` and the editor reports a clear error.
- `SHIPPORI_MINCHO`: Shippori Mincho's bytes, or `None`.

wasm32 builds embed only BIZ UDPGothic Regular and any `Arab` or `Telu` face, to keep the web build small: the web build
currently has no Chinese face, so Chinese there still shows the replacement glyph.
Font files are never
committed here (`AGENTS.md` §1.4; team members: [craftrules `standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md), internal).

The full font subsystem (parsing, shaping, subsetting and embedding) arrives with M2.2/M7.
