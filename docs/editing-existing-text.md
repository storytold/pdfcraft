# Editing existing text and source subsets

Click a text region in **Edit a PDF → Edit text and images**. One editor contains its
source rows and mixed styles. Embedded TrueType and OpenType/CFF faces are used in memory;
source font programs, colour, size and baseline anchors stay intact. A textless preview
prevents a second copy underneath. The frame shows the live extent as text grows or shrinks.
Rows a text edit doesn't reach keep their original operators, so their kerning, `TJ` word
gaps and glyph widths stay exactly as they were. Only rows whose characters changed, or that
moved because an earlier row wrapped, are rewritten, and formatting a selection rewrites
only the rows it covers. Changing the whole paragraph's formatting, width or position rewrites
every row.

Add Text also opens existing text when it is clicked; a blank-space click starts a new item.
Both text tools retype an added text item with one click. Draft boxes grow to the page edge
before wrapping, and the saved item uses that width. A single visual source row grows across font/style boundaries;
multi-row paragraphs retain their existing wrap width. Retyping uses a preview with only
that added item's old drawing removed, so deleted draft characters do not show underneath.

A subset font contains only some of the original face's glyphs. A new character cannot be
reconstructed from a glyph the PDF does not contain. **Load font…** imports an openly licensed,
embeddable standalone `.ttf` or `.otf` face. **Installed fonts…** discovers eligible user-installed
faces on request, off the UI thread; the document-font menu selects resources already on the
page. Select **Keep source fonts** to preserve each run's original face. A matching full face
completes its subsets automatically, including new Chinese characters. When the PDF itself embeds
the complete program of a paragraph's font (not a subset) and its licence allows editable
embedding, a character the font's encoding can't show is drawn with that same program. A
character no available face has is refused with an error that names it. Choosing another face
is explicit. Fonts with restricted embedding, unsupported collections, excluded families or
no open-licence declaration are refused. Interface fallback fonts are never exported.

The full selected program, glyph mappings, widths and Unicode text are embedded in the PDF.
Saving and reopening therefore needs no locally installed font. Editing the same text again
reuses the font resource the page already has, and every resource made from one program
shares a single copy of it. Original subset bytes are
preserved. Optional matching faces from `CRAFT_FONTS_DIR` can also complete source subsets;
this works only when that build input actually contains the full face. This change does not
add a font binary or change the release font pin. Fonts shared by releases belong in
`storytold/craft-fonts` under the repository's asset rules.

Rewrapped Chinese and Japanese text, which has no spaces between words, may break between any
two characters. A row never begins with closing punctuation, a closing bracket or quote, a small
kana or the prolonged sound mark, and never ends with an opening bracket or quote or a currency
sign before an amount: the line-start and line-end rules of JIS X 4051, as in Unicode line
breaking (UAX #14). Latin words inside such text are not split. Korean and Thai still break only
at spaces.

Select text within the editor before changing size, colour, font, underline or spacing to
format that selection. Automation supports up to 1024 disjoint UTF-8 byte ranges with the
same properties; invalid or overlapping ranges fail atomically. Untouched source formatting
survives. OpenType shaping uses the selected/source face's substitution and positioning
rules, preserves marks and ligatures, and applies Unicode bidi ordering for a single face.
Logical text remains available after save/reopen through ToUnicode and ActualText.
Added Arabic text records ActualText per shaped line for copying and searching in typing order.
ActualText already supplies typing order, so the editor preserves it without applying
a second bidi reversal; adjacent visual glyph runs still recover their own reading order.
Only right-to-left ActualText is kept apart this way: left-to-right spans, such as the Latin
ligatures (`fi`, `fl`) Word and Chrome wrap in ActualText mid-word, join their line as before.

A justified paragraph stretches each row but its last to the paragraph's width by widening its
spaces. Word spacing (`Tw`) widens only single-byte code 32, so where the font shows a space with
another code (a composite font, the generated Type 3 fallback) each space is followed by a `TJ`
adjustment instead. Justifying a paragraph with several formats in it is not supported yet: it is
left-aligned.

For headless editing, use `text_font_files` → `text_font_import` → `text_fonts` and pass a
resource to `text_edit.font_resource`. `text_edit.ranges` holds selected range formatting.
`text_edit_layout` accepts resource/ranges too and measures a draft without applying it.
All file inputs respect CLI root confinement; importing a font and applying text are undoable.

On a scanned page, recognised text (Scan & OCR) is an invisible layer over the picture. Edit
text groups it into phrases: words joined by a word space, with a table cell, a tab stop and each
scanned line kept apart. Applying an edit draws the new text visibly in the scan's ink colour (or
the one chosen) and first covers the old words with the paper colour sampled around them. Each
word keeps its position and width, and a longer phrase grows rightwards rather than wrapping. The
cover is one flat colour. Empty OCR glyphs are replaced with a visible fallback face whose advances
are calibrated to the source word's measured width; the scanned typeface is not reconstructed.
This applies to any invisible text (rendering mode 3), not just a PdfCraft OCR layer: editing
invisible text, such as another OCR tool's layer, makes the rewritten paragraph visible over a
painted cover in the sampled paper colour. Edit text also groups invisible text differently: its lines never
merge into multi-line paragraphs, and words split at gaps of 0.6 em or more.

Explicit marked paragraph/heading scopes prevent unrelated tagged regions from merging.
Compatible geometry groups untagged content, including runs split across content streams
or equivalent graphics transforms. Geometry cannot infer author intent in every untagged
PDF. Semantic structure-tree preservation, legacy Type 1/Type 3 inline font preview,
font collections/variable-font instances, mixed-font/style contextual runs and complete rotated
layout fidelity still need broader coverage. Do not interpret the passing real-file checks
as a guarantee for every PDF.

Private acceptance tests are opt-in and keep inputs unmodified. They exercise actual Chinese
keyboard insertion/deletion on all four pages, source-face/baseline checks and identical
in-memory renders before and after saving/reopening. Synthetic tests cover full TrueType
mapping, OpenType/CFF, Arabic marks/ligatures/mixed bidi, repeated edits, disjoint styles,
invalid ranges, font selection and root confinement. CLI acceptance repeats insertion/deletion
and two saves/reopens on four pages; independent PDFium checks extraction and rendering.
No private document, font or screenshot
is included in the repository.
