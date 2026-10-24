# pdfcraft-annot

Comments (annotations), ISO 32000-2 §12.5. Layer L3, depends on `pdfcraft-cos`, `pdfcraft-fonts` and `pdfcraft-model`.

## What it does

- **Builders** for the comment types Acrobat's commenting tools create: sticky note (`Text` +
  `Popup`), highlight, underline, strikethrough, squiggly, rectangle (`Square`), oval (`Circle`),
  line and arrow (`Line` with `/LE`), freehand drawing (`Ink`) and text box (`FreeText`).
- **Appearance streams** (`appearance::build`) generated from the annotation dictionary, so new
  and restyled comments look the same in every viewer. Highlights multiply (`/BM /Multiply`).
  A line's leader lines (`/LL`, `/LLE`, `/LLO`) and caption (`/Cap`, `/CP`, `/CO`) are drawn as
  ISO 32000-2 Table 178 places them, the caption in black Helvetica 10.
  Note icons are PdfCraft's own drawings. Text boxes use standard Helvetica (WinAnsi), with
  line breaking by approximate Helvetica proportions (no font program is bundled). Text it can't
  draw (in a text box, callout, Fill & Sign text, line caption, measurement label or dynamic
  stamp's by-line; `appearance::undrawable_text`) is kept as typed and drawn as `?`, and the comment is marked with
  the first such character (`pdfcraft_fonts::UNDRAWN_MARK`) so the engine reports the edit and
  flattening refuses to make it permanent. Two refusals (`AnnotError::Undrawable`): a redaction
  mark's overlay text, which becomes page content when applied; and a restyle or resize of a
  comment another program drew with a font that has those characters, whose appearance is kept
  rather than redrawn with `?` (new text for it is taken).
- **Edits:** reply (`/IRT`), set review status (a `/State` reply, as Acrobat's "Set status"
  does), change text, move, resize, restyle, delete (with pop-up and replies).

## API sketch

```rust
let index = add_annotation(&mut doc, &NewAnnotation { page, shape, style, contents, author }, &meta)?;
add_reply(&mut doc, page, index, "Agreed", "Ada", &meta)?;
set_review_state(&mut doc, page, index, ReviewState::Accepted, "Ada", &meta)?;
move_annotation(&mut doc, page, index, dx, dy, &meta)?;
set_style(&mut doc, page, index, Some(rgb), Some(0.5), Some(2.0), &meta)?;
delete_annotation(&mut doc, page, index)?;
signature_image(&doc, page, index)?; // embedded image XObject of Fill & Sign signature/initials
```

A comment is addressed by `(page, index in /Annots)`, the same pair
`pdfcraft_render::Annotation` reports. `Meta` carries the date and the `/NM` id so edits stay
deterministic; the engine supplies them.

## Fidelity rules

- Unknown keys are kept. Inline annotation dictionaries are promoted to indirect objects only
  when an edit needs a reference to them.
- `set_style` refuses (and changes nothing) when it cannot redraw the comment, e.g. stamps,
  cloudy borders or callouts, instead of leaving a stale appearance.
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

## Not yet

Callouts, polygons and polylines, clouds, carets (insert/replace text), stamps, file
attachments, rich text (`/RC`, `/DS`), and the shared named-appearance cache Acrobat writes.
