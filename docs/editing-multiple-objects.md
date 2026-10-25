# Move, align and arrange page objects

In **Edit a PDF → Edit text & images**, select several items on the same page:

- **Shift-click**, **Ctrl-click**, or **Command-click** toggles an item. An ordinary selected
  paragraph, image, or added item becomes the first member when you extend its selection.
- A plain click on a vector path's painted shape (not its empty box) selects it, and a drag moves
  it. Where a paragraph's, image's or added item's box covers the press, that object takes the
  click whatever the painting order, even when the path is drawn over it (a page-sized background
  image doesn't): start a selection rectangle on empty page space, then Shift-click the other
  object to leave just the path. A path covering most of the page (a background) stays out of
  clicks and plain marquees; Shift-drag over the whole page to select it.
- Drag through empty page space to draw a selection rectangle. Items intersecting the rectangle
  are selected; hold one of those modifiers to add to the selection.
- Drag the selection to move all its items by the same distance. Blue outlines preview the move;
  releasing commits it as **one undo step**. Escape cancels the gesture or clears the selection.
- Switching tabs, opening Home or a modal dialog, or losing window focus cancels a drag preview.
  Returning to the document keeps the selection without committing a release from elsewhere.
- Modifier-clicks prefer added items and text over background artwork, so labels remain selectable.
- Clear the selection or double-click an item to return to ordinary text editing and image resizing.
  Finish or discard a text draft before starting a multi-selection.

The supported items are the existing paragraph boxes, raster Image XObjects, whole Form artwork,
painted vector paths, and text/images added by PdfCraft. A Form remains one item, including its
nested artwork and resources. Added items appear once in the selection inventory. Selections stay on
one page and contain at most 1,000 items; pages with more than 10,000 recognised objects are refused
for group selection. Content streams must decode completely within 64 MiB each and 128 MiB per page,
with a 1,024-level graphics-state limit and 100,000 content operations. These limits apply to both
the original page and the moved result; unreadable or missing streams are refused. Moving between
pages, inline images, resizing several items at once, and vertical or clipping text are outside
this operation's scope. A refusal changes nothing.

Added streams move by changing their outer placement, preserving drawing bytes, fonts, crop paths,
and image transforms even if the page was rotated after the item was added. Their original local
coordinate basis is retained in `/PCAdded/MoveOrigin` so metadata and source placement agree after
repeated moves. Ordinary content updates replace that stream and reset the basis.

Existing text moves without retyping, font substitution, or reflow: its original encoded glyph
strings and kerning are replayed with temporary numeric `TJ` adjustments and text rise. The reader
performs the original glyph advances, so following unselected text keeps its position. Paragraph
indices can change when detection groups lines differently after a move. The UI remaps its own
selection only when the new geometry matches uniquely; other document edits clear stale selection.

## Vector paths

A vector path is one painted shape: its construction operators through the operator that fills or
strokes it. Not selectable: clipping paths, paths that clip and paint at once, unpainted paths,
paths inside a text object or under a text clip, the contents of Form artwork, PdfCraft's own marks
(background, watermark, header and footer) and added content. A page with more than 5,000 paths,
with clipping too complex to hit-test, with paths and other objects together over 10,000, or with a
path whose coordinates or clipping are beyond what the path reader accepts, leaves its paths out;
its other objects can still be selected.

- **Move.** A drag wraps the path in `q cm … Q`. A path alone in its own saved state with its
  private clip moves with that clip; otherwise it moves under the clip in effect. A move that would
  take it off the page or out of its clip is refused, and so is moving or resizing a path painted
  with a pattern (the pattern would not move with it), alone, in a group or by Align and
  Distribute, and moving or resizing a path drawn flat by its transform.
- **Resize.** With one path selected, drag a corner handle; the opposite corner stays where it is
  and proportions are not kept. The line width and dash scale with the path, so a stroked path's box
  lands close to, not exactly on, where you let go. On a thin rule the handles only stretch it along
  its length; a path too small to grab inside its handles can still be moved from its middle.
- **Recolour.** **Fill colour** and **Stroke colour** in the Edit a PDF panel show the colour of
  the first selected path that paints that part (grey when it isn't a DeviceGray, DeviceRGB or
  DeviceCMYK colour: ICC-based, Lab, indexed, spot and pattern colours show grey) and open the colour
  grid; Escape closes the grid and keeps the selection. A pick recolours every selected path that
  paints that part, as one undo step, and the selection stays. The new colour is DeviceRGB whatever
  the path used: CMYK, spot and pattern colours are replaced.
- **Delete.** Delete or Backspace removes the selected paths, images and Form artwork in one step
  and one rewrite of the page, leaving the graphics state and marked content around them. A
  selection with paths and text or added items shows a notice and deletes nothing; paragraphs and
  added items, with any images beside them, go in one undo step. The keys are left
  alone while typing, in Organize Pages, and while the pointer is over a panel rather than the page
  (the Pages panel deletes pages with them); move the pointer back over the page, or use
  right-click ▸ Delete.

## The selection's menu

Right-click the page the selection is on:

- **Arrange ▸ Bring to front, Bring forward, Send backward, Send to back**, for one selected vector
  path, image or Form artwork. The order is among the objects it overlaps: forward and backward pass
  the next one, front and back all of them. The object's own operators (with a private clip and the
  `q`/`Q` and marked content around it) are cut out and replayed at the new place with the graphics
  state they were drawn with, so it looks the same, and the selection follows it.
- **Align ▸ Left, Center horizontally, Right, Top, Center vertically, Bottom**, for two or more
  objects of any kind, on their combined bounds in the page's own orientation (its `/Rotate` is
  followed; a view turned with View ▸ Rotate view clockwise or counterclockwise doesn't change which
  edge is Left). Sizes are kept.
- **Distribute ▸ Horizontally, Vertically**, for three or more: the gaps become equal and the
  outermost two stay put.
- **Delete**, which deletes what the Delete key would: anything but vector paths with text or
  added items.

Each is one undo step, and only the content streams that change become new objects. A refusal is
shown as a notice and changes nothing. Arrange refuses text, items added in PdfCraft and PdfCraft's
page marks (watermark, header and footer, background), which keep their own layer, so Bring to front
stays under a watermark; nothing overlapping in that direction; an object inside a text object;
moving past content clipped differently or into different transparency or blending; an object
tagged together with other content (`/MCID`, `/ActualText`, `/Alt` or `/E`); a shape that also
clips; too many graphics-state settings; and content too irregular to rewrite safely. Align and
Distribute refuse objects already in place, a vector path painted with a pattern, and an image or
vector path partly hidden by a clipping path shared with other content or by the page edge, which
would show a different part once moved; move it instead.

## Engine

`Document::editable_objects(page)` returns a mixed inventory with zero-based `ObjectTarget`
references and user-space rectangles; vector paths come after images, in painting order, and
`Document::page_paths(page)` gives their outlines, paint and colours (`path_at` hit-tests them,
topmost first). Apply these through `Session`:

- `Edit::MoveObjects { page, objects, offset }` moves the objects together.
- `Edit::AlignObjects { page, objects, align }` aligns or distributes them (`ObjectAlign`).
- `Edit::TransformPath { page, index, rect }` moves and resizes one path onto a user-space box.
- `Edit::RecolourObjects { page, objects, fill, stroke }` repaints paths in DeviceRGB.
- `Edit::DeleteObjects { page, objects }` deletes paths, images and Form artwork.
- `Edit::ArrangeObjects { page, object, order }` restacks one path or image (`ZOrder`);
  `Document::arranged_target` says where it will be listed afterwards, without changing anything.

Targets resolve together before mutation. Permission checks, dirty state, history, incremental
saving, and refreshing use the same transaction as other edits.

`Document::object_move_offset(page, [dx, dy])` converts a displayed-page vector (points, right/down)
to the user-space vector required by the edit, accounting for the crop box, `/Rotate`, and `/UserUnit`.
A frontend with an additional view rotation converts its pointer positions to displayed page space
first. Duplicate, missing, non-finite, oversized, or unsupported targets fail atomically. Engine callers
re-list after changes; the automation generation guard detects stale references before the edit.

## CLI and MCP

Both frontends use the same tools. `object_list` returns a document `generation` and items with
`kind` (`added`, `text`, `image`, or `path`), 1-based `index`, `type`, displayed `rect`, and optional
`text`; path rows also give `fill` and `stroke` colours (null when that part isn't painted or isn't
a Gray/RGB/CMYK colour) and the stroke `line_width`. Treat those references as belonging to that
generation and pass it back when editing:

```json
[
  {"tool":"doc_open","args":{"path":"input.pdf"}},
  {"tool":"object_list","args":{"doc":1,"page":1}}
]
```

After choosing references from the returned list, use a move call like this in the same MCP
session. For a CLI script, include `doc_open` in that script before the move:

```json
[
  {"tool":"object_move","args":{
    "doc":1,"page":1,"generation":0,
    "objects":[{"kind":"text","index":1},{"kind":"image","index":1}],
    "offset":[24,18]
  }},
  {"tool":"doc_save","args":{"doc":1,"path":"moved.pdf"}}
]
```

The other tools take the same references and optional `generation`: `object_align` (`action`:
`left`, `centre`, `right`, `top`, `middle`, `bottom`, `distribute_horizontal` or
`distribute_vertical`), `object_resize` (one `path` `object` and a displayed `rect`),
`object_recolour` (`path` references, `fill` and/or `stroke` as `#RRGGBB` or a colour name),
`object_delete` (`image` and `path` references) and `object_arrange` (one `path` or `image`
`object` and an `order` of `front`, `forward`, `backward` or `back`; it returns the object's new
reference). Each success is one undo step. The [automation guide](../crates/automation/README.md)
has the details.

Use the actual generation and references from your document, rather than assuming these example
values. `offset` is in displayed points: positive x moves right, positive y moves down. The optional
generation guard rejects a changed document before interpreting references. Re-list after any edit,
undo, redo, or save; there is no claim that an index persists across generations. Run a script with
`pdfcraft-cli run --script steps.json --root DIR` or call these tools through the opt-in MCP server.

Regression coverage: `crates/edit/src/tests.rs` (glyph bytes, neighbours, split streams, shared Forms,
rotated/cropped/UserUnit pages and refusal), `crates/edit/src/paths_tests.rs` and
`path_edit_tests.rs` (finding, moving, resizing, recolouring and deleting paths), `align.rs`,
`arrange_tests.rs` and `delete.rs` (alignment, painting order and byte-preserving deletion),
`crates/engine/src/tests.rs` (permissions, history, rendering and save/reopen),
`crates/automation/tests/it/automation.rs` (generation guards, hostile references and save/reopen), and
`crates/ui-egui/tests/it/object_selection.rs` (real-shell selection, gestures, the Delete key, the
selection's menu and the colour swatches).
