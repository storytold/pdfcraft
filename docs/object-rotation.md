# Corner transforms

Select **Edit a PDF → Edit text & images**, then click a text block or image.
A curved-arrow control sits outside each of its four corners. Pictures selected
with the ordinary Select tool also show the controls. Drag any arrow around the
centre pivot, then release to apply the rotation. Hold Shift for 15-degree steps.
Escape, switching tabs, opening a dialog, or losing window focus cancels the drag.

The moving outline and angle are a preview; the document changes only on release,
with one Undo step. Each arrow faces its own corner. Square corner grips scale
proportionally about the opposite corner, including the glyph size for text; dragging the selected
object moves it. Rotation and scaling can be followed by movement without
reselecting. The right-click menus are unchanged.
If text is still being typed, the first press applies that text draft; drag again
after it has been applied. Save the PDF normally to retain the rotation.

Rotation keeps source glyph bytes, fonts and image data. Added text retains its
angle when it is subsequently retyped or moved. A grouped Form is one artwork
item. This does not add selection or editing of individual objects inside a Form,
or rotation of a mixed multi-selection. Added items with inherited surrounding
transforms or non-uniform placement are refused rather than repositioned. Text
in a text object containing a clipping path, or with vertical writing, is refused
without changing the document. Rotation and scaling also refuse marked-content
spans opened inside a text object: splitting or duplicating those tags could
corrupt accessibility metadata. Spans surrounding text objects remain supported.
A scanned picture of text remains a picture.

Scaling keeps the existing angle. Extremely small/large results are refused
without changing the document (added text remains within the existing 1–500 pt
font limits). In Select mode, click an image before dragging it to move it;
unselected text continues to use normal text selection. Select source text for
transforms through **Edit text & images**.

## Automation

Use `object_list` to obtain the current inventory and generation, then:

```json
{"tool":"object_rotate","args":{"doc":1,"page":1,"kind":"text","index":1,"degrees":30,"generation":0}}
```

`object_scale` accepts the same target plus `factor` (0.01–100) and a fixed
`pivot: [x, y]` in displayed page points. `object_move` accepts a list of targets
and an `offset: [dx, dy]` in displayed points.

Pages and indexes are 1-based. Positive angles turn clockwise around the centre
of the visible object bounds. Kinds are `text`, `image`, or `added`. A stale supplied
generation is refused; query the inventory again after editing.

## Validation

Engine regressions cover glyph preservation, unselected text state, clipping-text
refusal, image placement, save/reopen, added-text edits, and undo/redo. The GUI
regressions exercise the actual corner widgets, release-only mutation, Escape,
source text, Select-tool images, Shift snapping, and rotate → scale → move →
undo/redo → save/reopen for both source and added text/images. The automation regression
checks stale references and save/reopen through the shared tool table.

Review regressions check PDF 1.7 operator context (q/cm/Q outside BT/ET),
restore the text and line matrices for following glyphs and line moves, and
retain added-object rotation through indirect metadata, retyping and save/reopen.

Additional regressions cover changed line counts after retyping rotated text,
inline and named ActualText spans inside and outside text objects, and the live
added-text move preview through pointer release and raster redraw. ActualText
markup surrounding text objects and neighboring text survive rotation, scaling
and save/reopen. Inner marked-content spans (BMC/BDC, including MCIDs) are refused
without changing the saved bytes. A closed inner span does not prevent transforming
a later, unmarked paragraph.
