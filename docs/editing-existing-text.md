# Editing existing text and source subsets

In **Edit text and images**, text drawn inside a Form XObject (grouped artwork) is edited
with one click: clicking its text prepares that page in an undoable **Prepare grouped text**
step and opens the paragraph clicked for typing. Nothing is prepared while pages are only
shown or scrolled. A drag still moves the selected group as a whole; a click on grouped
artwork that preparation wouldn't open (a logo, a refused group) selects it as before, and
changes nothing. To select a text group for moving, right-click it. While a paragraph is open
or another edit is waiting, the click selects the group instead, as before; the next click
prepares it. The group's original operators, clip, placement and resources become page
content; shared forms on other pages stay unchanged. Ungroup (the artwork's right-click menu)
draws one placement in place with the same code. Headless callers use `text_prepare`, then
list `text_paragraphs` and call `text_edit`.
This edits existing glyph operators; it adds neither a cover rectangle nor an Add Text box.
Preparation refuses knockout/blended groups, groups that blend in a colour space other than
the page group's (DeviceRGB on a page that names none), forms whose default
colour spaces differ from the page's, tagged forms, forms drawn inside text objects or
unfinished paths, groups whose nested content can't be checked and unreadable content. Forms
that draw only images or vector artwork are left grouped. Decoding is capped at 16 MiB, 64
placements and eight expansion levels per page; text grouped deeper is reported. Unsupported
groups remain intact: `text_prepare` lists each with its reason, and the repair log records
them when the page is prepared. A narrowly fingerprinted legacy THSarabunPSK exporter map can
also be repaired in copied page fonts, only when every glyph the subset kept advances as its
assumed Thai character does; `text_prepare` repairs it also for text that was never grouped,
and the repair log records both preparation and map recovery.

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
character no available face has is refused with an error that names it, and a CJK character
without a craft-fonts build says so. Replacement text takes the craft-fonts Japanese faces when
they can draw all of it, and the bundled Inter face (`assets/fonts`) for the Latin, Greek and
Cyrillic letters they lack (Uzbek `қизил`, Kazakh `әже`): that face needs no build input, so
these work in every build, the web one included. Choosing another face
is explicit. Fonts with restricted embedding, unsupported collections, excluded families or
no open-licence declaration are refused. Interface fallback fonts are never exported.

### Keeping the paragraph's typeface

When the very font a paragraph uses is installed on the machine (the PostScript name the PDF
gives, or its family and style, exactly), its complete program completes the embedded subset
the way a matching craft-fonts face does: the paragraph keeps its own font for every character
it had and the new ones come from the installed program, which is embedded whole. Only a face
whose own embedding permission (`fsType`) allows editable embedding is used, never for a font
the document does not embed, and `PDFCRAFT_SYSTEM_FONTS=0` turns it off
(`crates/fonts/src/installed.rs`).

Otherwise a paragraph whose embedded font lacks a character keeps that font for every
character it has, and only the missing ones are drawn by a stand-in, as outlines. The whole
paragraph is no longer set in a standard font for one new letter (#2378), which changed its
face, its widths and so its line breaks. The stand-in is the font's metric-compatible
counterpart from craft-fonts' Latin catalogue (`fonts/latin-manifest.txt`: Liberation Sans for
Arial and Helvetica, Liberation Serif for Times New Roman, Liberation Mono for Courier New,
Carlito for Calibri, Caladea for Cambria), else a serif, monospaced or sans face of the source's
kind, in its weight and slant as near as the family has them, else the bundled Inter face. Latin,
Greek and Cyrillic text no longer takes a Japanese face; text with Japanese in it keeps the
Japanese faces first, so a line keeps one look. Text none of whose letters the subset has, a
subset whose program cannot be read and turned text are still set in the standard font.

A font the document names without embedding (`Verdana`, `ArialMT`) often lists `/Widths` only
for the characters its text used. A new character is written in that font once its advance is
known: from the same font installed on the machine, from the published metrics of the standard
font the name asks for exactly (not `ArialNarrow` or `HelveticaNeue`), or from its
metric-compatible counterpart. Only the font dictionary's `/FirstChar`, `/LastChar` and
`/Widths` change. Nothing is embedded, and a refused edit puts them back.

### Characters a row holds that its font cannot write

InDesign and Word draw a tab as a glyph of no width and move the text after it, so a bulleted
item reads `•\t text`. No font has that tab to write again, and any edit to such a paragraph was
refused. The tab becomes the spaces that span its gap, in the source rows too, so a row the edit
does not reach still keeps its operators. A no-break space, or another space character the font
has no code for, is written as a gap of the space's advance.

### Rows that outgrow their place

When an edit makes a row too long, the rows after it flow on from it up to the next break that
was typed, as in a word processor: the words that no longer fit are not left on a row of their
own with every later row pushed down a line. A break counts as a wrap when a `/PCSoft` marker
says so or, in a file written elsewhere, when the row ended where its next word no longer fit.
Each row wraps at the paragraph's right edge from its own left edge, so the rows of a hanging
indent stay inside the paragraph, and a row an edit adds keeps the last source row's indent.
Rows before the long one, and paragraphs none of whose rows grew too long, are left as they are.

### Fonts the inline editor can show

Besides TrueType and OpenType/CFF programs, the inline editor reads a bare CFF program
(`/FontFile3` with `/Subtype` `/Type1C` or `/CIDFontType0C`, as Distiller, InDesign and Word
write most PostScript-flavoured fonts) through an editor-only OpenType wrapper
(`crates/fonts/src/pdf/cff.rs`): the program is unchanged, a Unicode `cmap` is built the way
the page selects glyphs, and the wrapper is marked so that no edit embeds it. Such text used
to be drawn in an interface face squeezed into the source's widths, its letters over each
other. A stand-in face much wider than its source (for a Type 1 or Type 3 font, which still
preview in an interface face) is now drawn smaller instead.

### What is embedded

The full selected program, glyph mappings, widths and Unicode text are embedded in the PDF.
Saving and reopening therefore needs no locally installed font. Editing the same text again
reuses the font resource the page already has, and every resource made from one program
shares a single copy of it. The Type 3 fonts generated for Japanese or Hebrew characters the
page's fonts lack are made anew by each edit; after an edit, those nothing on the page shows any
more leave the page's resources, unless some content that could name them can't be read.
Original subset bytes are
preserved. Optional matching faces from `CRAFT_FONTS_DIR` can also complete source subsets;
this works only when that build input actually contains the full face. This change does not
add a font binary or change the release font pin. Fonts shared by releases belong in
`storytold/craft-fonts` under the repository's asset rules.

With **Keep source fonts**, an upright single-style paragraph can combine its source font
with a fallback even when the replacement has no explicit line break (#2750). Characters
the source can draw keep that font and its advances; only missing characters enter the
generated Type 3 fonts, in batches. Wrapping and alignment measure both faces, including
character spacing and horizontal scaling. Explicit family/weight changes and the existing
turned, right-to-left and visual-order paths retain their selection rules. The styled-path
fallback grouping below remains independent of this single-style path.

**Keep source fonts** retains the existing Thai family, weight and point size. For legacy
THSarabunPSK subsets, PdfCraft reconstructs an editor-only Unicode cmap from the PDF's
Unicode and CID-to-glyph maps. HarfRust uses the retained Thai positional glyphs, and the
saved text refers to the original PDF font resources. Several subsets of the same exact
face can supply Thai and punctuation glyphs, including a Thai run whose clusters are split
across subsets, and a subset whose glyph for a character is empty is skipped; Regular and
Bold stay separate. Source font
programs are never modified or replaced. Text settings, including character/word spacing,
horizontal scale, rise, color and paint mode, remain in the PDF text state.

A source subset that lacks a needed contextual glyph produces an error asking for the
complete original face or an explicit replacement. Cross-family replacement requires an
explicit choice. The document-font menu offers **Noto Sans Thai**: pass its `text_fonts`
resource (normally `builtin:thai`) as `text_edit.font_resource` or a range's `font_resource`. This replacement
requires the optional craft-fonts Thai input and currently uses Regular. A matching Noto
subset can still be completed by that same bundled family when it is Regular and upright; a
subset its name or descriptor marks bold or italic needs an explicit replacement.

The bundled variable Thai face is written as bounded Type 3 glyph outlines, with shaped
vowel/tone positions and logical Unicode text, rather than embedded as a static font.
Saved paragraphs can be reopened and edited again without installing a font. Modern Thai
shaping of vowels and tone marks uses the source's GPOS mark positioning (a `mark` or `mkmk` feature in the script and language system the shaper
selects for Thai, not just a GPOS table); GSUB substitution is optional, as specified by
[the OpenType Thai model](https://learn.microsoft.com/en-us/typography/script-development/thai).
Verified legacy Thai PUA fonts use positional substitutions instead of requiring GPOS; their
alternates are used only when each kept one has a mark's zero advance (or, for the two
consonant forms, an advance) and, where the PDF's own map names its character, the character
that alternate stands for. A THSarabunPSK program with its own Thai mark
positioning, such as a complete face loaded with Load font, is shaped by that layout instead,
still in its own codes. One without it takes the legacy path only when every Thai glyph it keeps
is where the legacy glyph order puts it, and is otherwise shaped as an ordinary source font. A
subset needs only the glyphs its new text uses. When no subset can show the new text in its
existing codes, a complete program of the same face and style (one added with Load font, CFF
included) shapes the whole span with its own layout, Latin rows too, rather than the edit being
refused; a Regular program never completes text the source marks bold or italic.
Mixed contextual style runs and dictionary word segmentation remain limited. No font binary
or release pin changes are needed.

When mixed-style text needs outline fallback, characters using the same actual fallback face
share Type 3 fonts, with at most 240 distinct glyphs per font. Adjacent characters with the same
font and source style are written as one run. Latin characters that the source font can encode
stay in it; Japanese serif/sans and weight choices remain separate. More than 240 missing glyphs
use additional fonts; a styled paragraph needing more than 15,360 distinct fallback glyphs is
refused with an error. Unsupported glyphs are refused before fallback objects are written.
This does not expand glyph coverage, shape mixed contextual runs or remove old unused fonts
when editing again. The single-style Keep source fonts path above has its own fallback grouping.

Rewrapped Chinese and Japanese text, which has no spaces between words, may break between any
two characters. A row never begins with closing punctuation, a closing bracket or quote, a small
kana or the prolonged sound mark, and never ends with an opening bracket or quote or a currency
sign before an amount: the line-start and line-end rules of JIS X 4051, as in Unicode line
breaking (UAX #14). Latin words inside such text are not split. Korean and Thai still break only
at spaces.

Text pasted from a browser or a word processor often carries an invisible break instead of a
newline. Every Unicode row break starts a new row: U+2028 LINE SEPARATOR, U+2029 PARAGRAPH
SEPARATOR, U+0085 NEXT LINE, the vertical tab, the form feed and the information separators
(U+001C to U+001E), alongside the carriage return and the newline. These move the text down a
row rather than being looked up in a font, so a paste no longer fails because no face has an
outline for them. Other white space is still written as one space.

Select text within the editor before changing size, colour, font, underline or spacing to
format that selection. Automation supports up to 1024 disjoint UTF-8 byte ranges with the
same properties; invalid or overlapping ranges fail atomically. Untouched source formatting
survives. OpenType shaping uses the selected/source face's substitution and positioning
rules, preserves marks and ligatures, and applies Unicode bidi ordering for a single face.
Each shaped run is one text object: its glyphs are shown in one `TJ` array whose adjustments
place each glyph where shaping put it, and a mark set above or below its letter is raised
with the text rise. Logical text remains available after save/reopen through ToUnicode and ActualText.
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
word keeps its position and width, and a longer phrase grows rightwards rather than wrapping.
OCR sizes each word from its own box, so a phrase's words can come out several points apart: the
editor and the rewrite set the whole phrase in one size, the median of its words' sizes.
A text edit covers and redraws only the words it changes: the rest of the line stays the scan, and
a deleted word leaves its gap, because the words after it are pixels. A replacement a little too
wide is squeezed across to fit its place (by up to a quarter); wider than that it takes over the
word it would run into, and new words with nothing to replace take a neighbour.
The whole phrase is redrawn for a change of formatting or a new line, and for a layer whose words
would move without the ones before them. While the phrase is open, the editor still shows all of
it in the layer's Helvetica. The cover is one flat colour, and the text uses the layer's Helvetica, not the scanned typeface.
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

## Paragraph numbers (automation)

A paragraph's number is its position in a list that is regrouped from geometry on every read,
so an edit that merges two paragraphs, or splits one, moves the others. After `text_paragraphs`
the numbers name the paragraphs *that listing showed*: `text_edit`, `text_edit_layout` and
`page_render`'s `hide_text_block` find the paragraph again by its text and box, so editing paragraph 3 then paragraph 1 still edits the
two the listing showed. A paragraph an edit merged into a neighbour (or changed) is refused, and so is the edited paragraph
itself when the edit merged it into one, with
a message to list the page again, instead of rewriting whichever paragraph took its number
(#2516). The result of an edit names the numbers that are gone in `stale_paragraphs`. Listing
a page again renumbers it. The listings kept are capped (256 pages); once the cap drops
them, a page of an affected document is refused until it is listed again, instead of reading its
numbers as positions. Without a listing, a number is a position in the page's current
paragraphs, as before. The inline editor holds the paragraph a switching click asked for the
same way. What is still missing is an identity that survives save and reopen (design: #2508).
