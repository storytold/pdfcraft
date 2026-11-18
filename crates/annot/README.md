# pdfcraft-annot

Comments (annotations), ISO 32000-2 §12.5. Layer L3, depends on `pdfcraft-cos`, `pdfcraft-fonts` and `pdfcraft-model`.

## What it does

- **Builders** for the comment types Acrobat's commenting tools create: sticky note (`Text` +
  `Popup`), highlight, underline, strikethrough, squiggly, rectangle (`Square`), oval (`Circle`),
  line and arrow (`Line` with `/LE`), freehand drawing (`Ink`), text box and callout
  (`FreeText`), polygon (including cloudy borders), polyline, caret, rubber and custom stamp,
  and file attachment.
- **Appearance streams** (`appearance::build`) generated from the annotation dictionary, so new
  and restyled comments look the same in every viewer. Highlights multiply (`/BM /Multiply`).
  A line's leader lines (`/LL`, `/LLE`, `/LLO`) and caption (`/Cap`, `/CP`, `/CO`) are drawn as
  ISO 32000-2 Table 178 places them, the caption in black Helvetica 10.
  Note icons are PdfCraft's own drawings. Text boxes, callouts and Fill & Sign text draw WinAnsi
  text in a standard face: Helvetica for a new box, or the standard face a box's `/DA` or `/DS`
  names (Times, Courier), with line breaking by that face's standard-14 metrics. Thai is
  shaped and drawn from the craft-fonts Thai face with Add text's writer
  (`pdfcraft_fonts::shaped_text`: Type 3 glyphs with a ToUnicode map), the box sized to the ink
  of its stacked marks and descenders; a build without a Thai face refuses it
  (`AnnotError::Undrawable`) instead of drawing `?`. Arabic and Hebrew (and Thai beside them) use
  their optional craft-fonts faces with OpenType shaping, bidirectional layout and self-contained
  Type 3 outlines (`pdfcraft_fonts::appearance_text`). Each line's ActualText holds its text
  in typing order, including marks, ligatures and mixed Latin/digits; the glyphs themselves are
  drawn in visual order, and their ToUnicode gives each glyph's own letters, so a reader that
  ignores ActualText reads the letters in drawing order. Latin beside them keeps the box's
  standard face. Wrapping preserves spaces and breaks long words only at grapheme boundaries;
  LF, CR, CRLF, U+0085 and U+2029 each end a paragraph, and U+2028 starts a new line inside one. Other
  text it can't
  draw (in a text box, callout, Fill & Sign text, line caption, measurement label or dynamic
  stamp's by-line; `appearance::undrawable_text`) is kept as typed and drawn as `?`, and the comment is marked with
  the first such character (`pdfcraft_fonts::UNDRAWN_MARK`) so the engine reports the edit and
  flattening refuses to make it permanent. Two refusals (`AnnotError::Undrawable`): a redaction
  mark's overlay text, which becomes page content when applied; and a restyle or resize of a
  comment another program drew with a font that has those characters, whose appearance is kept
  rather than redrawn with `?` (new text for it is taken).
  Fill & Sign sizes its text boxes from those shaped advances and ink heights, so large Arabic
  marks and pointed Hebrew fit the appearance. A new box keeps `/Q 0`. Under `/Q 0`, which is
  also what a box without `/Q` has, each paragraph starts at the edge its own direction reads
  from: a right-to-left paragraph at the right, including text retyped into an English box, and
  a left-to-right paragraph beside it at the left. `/Q 1` and `/Q 2` centre or right-align every
  paragraph, as before. Rich text keeps the same paragraph directions and breaks as the drawing.
  Only text that has Arabic or Hebrew is drawn and measured this way (`shaped_width`,
  `text_height`); captions, stamps and labels are drawn in Helvetica and keep `text_width`.
- **Edits:** reply (`/IRT`), set review status (a `/State` reply, as Acrobat's "Set status"
  does), change text, move, resize, restyle, delete (with pop-up and replies), and open, close
  or move the pop-up window (`set_popup`: `/Popup` `/Open` and `/Rect`, created beside a comment
  that has none; a note's own `/Open` follows). `summaries` report each comment's pop-up.

## API sketch

```rust
let index = add_annotation(&mut doc, &NewAnnotation { page, shape, style, contents, author }, &meta)?;
add_reply(&mut doc, page, index, "Agreed", "Ada", &meta)?;
set_review_state(&mut doc, page, index, ReviewState::Accepted, "Ada", &meta)?;
move_annotation(&mut doc, page, index, dx, dy, &meta)?;
set_style(&mut doc, page, index, StyleChange::new(Some(rgb), Some(0.5), Some(2.0), None), &meta)?;
delete_annotation(&mut doc, page, index)?;
signature_image(&doc, page, index)?; // embedded image XObject of Fill & Sign signature/initials
```

A comment is addressed by `(page, index in /Annots)`, the same pair
`pdfcraft_render::Annotation` reports. `Meta` carries the date and the `/NM` id so edits stay
deterministic; the engine supplies them.

## Fidelity rules

- Unknown keys are kept. Inline annotation dictionaries are promoted to indirect objects only
  when an edit needs a reference to them.
- `set_style` refuses when it cannot redraw the comment, e.g. a stamp
  whose appearance cannot be regenerated, instead of leaving a stale appearance.
- Replies get the parent's `/Rect` and an empty appearance: they appear in comment lists but
  never paint a second icon.
- Resizing a stamp changes its `/Rect` and preserves its original appearance and resources;
  viewers scale that appearance into the new rectangle. Locked stamps refuse the edit.
- Picture stamps (a natural-size or explicit-rectangle image, a PDF page, or an image
  signature) and PdfCraft's rubber stamps counterrotate their appearance on rotated pages, so
  the picture or label reads upright as the page is displayed. Restyling a picture stamp keeps
  that appearance matrix. The rotation is the page's `/Rotate` at placement.
- Sticky notes and file attachments are not counterrotated: they carry the NoRotate flag and
  their icon is drawn in an unturned 20 × 20 box. A viewer keeps them upright on a turned page,
  about the upper-left corner of `/Rect` (ISO 32000-2 §12.5.3). PdfCraft's renderer does so
  (vendored hayro-interpret patch 37), and `Summary::no_rotate` tells the UI where the icon is
  drawn.

## Rich text and remaining limits

Text-box text and colour edits regenerate `/RC` (XHTML) and `/DS` from the plain
`/Contents`, `/DA` and `/Q`, so rich-text editors show the same text and style. These edits
replace imported rich-text formatting. The shared named-appearance cache Acrobat writes
is not implemented.
